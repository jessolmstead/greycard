//! The camera match's sheet: the Look section's "Fit this camera's
//! look". The scope is surveyed for its bodies and picture styles off
//! the window's thread, the sheet says what the fit will do with each,
//! and Fit runs [`crate::camera_match::run`] on a thread of its own,
//! its progress and each group's result coming back here, the look
//! list read again when it is done.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::camera_match::{self as fit, Group, Plan, Progress, Survey};
use crate::panel::assets::show_looks;
use crate::*;

/// The scopes, as the sheet names them.
pub(crate) const LIBRARY: &str = "Library";
pub(crate) const FOLDER: &str = "This folder";

/// The sheet's state between its callbacks.
#[derive(Default)]
pub(crate) struct Sheet {
    /// Counts the surveys and the runs asked for, so an answer to one
    /// the sheet has since moved on from is dropped.
    generation: u64,
    survey: Option<Survey>,
    /// The run under way, to stop it.
    cancel: Option<Arc<AtomicBool>>,
    results: Vec<String>,
}

/// Where the library is and its roots, when one is indexed.
fn library_of(st: &State) -> Option<(PathBuf, Vec<PathBuf>)> {
    let path = st.index_path.clone()?;
    let roots = st.library.roots.list().to_vec();
    (!roots.is_empty()).then_some((path, roots))
}

/// The folder of the open frame, or of the browser's first.
fn folder_of(st: &State) -> Option<PathBuf> {
    let file = st
        .current
        .and_then(|i| st.files.get(i))
        .or(st.files.first())?;
    file.parent().map(Path::to_path_buf)
}

/// What a scope is surveyed from, read on the window's thread.
#[derive(Debug, Clone)]
struct Where {
    scope: String,
    library: Option<(PathBuf, Vec<PathBuf>)>,
    folder: Option<PathBuf>,
}

/// A scope's groups: the library's from the index; a folder's from
/// the index when the folder is under a root and the index has read
/// the tags of everything in it, else from the files themselves.
fn survey(at: &Where) -> Result<Survey, String> {
    if at.scope == LIBRARY {
        let (path, roots) = at.library.as_ref().ok_or("no library is indexed")?;
        let lib = greycard_library::Library::open_read_only(path).map_err(|e| e.to_string())?;
        return fit::survey_library(&lib, Some(roots)).map_err(|e| e.to_string());
    }
    let folder = at.folder.as_ref().ok_or("no folder is open")?;
    if let Some((path, roots)) = &at.library
        && roots.iter().any(|r| folder.starts_with(r))
        && let Some(found) = from_index(path, folder)
    {
        return Ok(found);
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder)
        .map_err(|e| format!("{}: {e}", folder.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    Ok(fit::survey_files(&files))
}

/// A folder's groups from the index, its own frames only and not its
/// subfolders'; none when a raw in the folder has no row yet, when the
/// index has not read the tags of every raw under it, or when it will
/// not open. The raws with no fixed style are the folder's raws the
/// groups do not hold.
fn from_index(path: &Path, folder: &Path) -> Option<Survey> {
    let lib = greycard_library::Library::open_read_only(path).ok()?;
    let canonical = dunce::canonicalize(folder).ok()?;
    let raws: Vec<PathBuf> = std::fs::read_dir(&canonical)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && greycard_core::decode::is_raw_path(p))
        .collect();
    if !indexed_whole(&lib.ids_of(&raws).ok()?) {
        return None;
    }
    let roots = [canonical.clone()];
    let mut survey = fit::survey_library(&lib, Some(&roots)).ok()?;
    if survey.unread > 0 {
        return None;
    }
    for g in &mut survey.groups {
        g.frames
            .retain(|f| f.path.parent() == Some(canonical.as_path()));
    }
    survey.groups.retain(|g| !g.frames.is_empty());
    let grouped: usize = survey.groups.iter().map(|g| g.frames.len()).sum();
    survey.no_style = raws.len().saturating_sub(grouped);
    Some(survey)
}

/// Whether the index has a row for every one of a folder's raws.
fn indexed_whole(ids: &[Option<i64>]) -> bool {
    ids.iter().all(Option::is_some)
}

/// One line for a group before the run: its frames and what the fit
/// will do with it.
pub(crate) fn group_line(g: &Group, plan: &Plan) -> String {
    let n = g.frames.len();
    let mut out = format!(
        "{}: {n} frame{}, {} with the adaptive settings off",
        g.name(),
        if n == 1 { "" } else { "s" },
        g.fixed()
    );
    match plan {
        Plan::Fit { replaces } => {
            out.push_str(&format!(" — fits on {} of them", g.candidates()));
            if g.left_out() > 0 {
                out.push_str(&format!(
                    "; the {} with an adaptive setting on are left out",
                    g.left_out()
                ));
            }
            if g.one_folder() {
                out.push_str("; all from one folder, so the fit will be narrow");
            }
            if let Some(warning) = replaces {
                out.push_str(&format!("; {warning}"));
            }
        }
        Plan::Borrow { from, replaces, .. } => {
            out.push_str(&format!(" — too few; borrows the look fitted on {from}"));
            if let Some(warning) = replaces {
                out.push_str(&format!("; {warning}"));
            }
        }
        Plan::Skip(why) => out.push_str(&format!(" — skipped: {why}")),
    }
    out
}

/// The line under the groups: what the survey left out of them.
pub(crate) fn summary(survey: &Survey) -> String {
    let mut out = Vec::new();
    if survey.unread > 0 {
        out.push(format!(
            "{} raw{} here {} not had the picture style read yet; the next pass over \
             their folders reads it.",
            survey.unread,
            if survey.unread == 1 { "" } else { "s" },
            if survey.unread == 1 { "has" } else { "have" },
        ));
    }
    if survey.no_style > 0 {
        out.push(format!(
            "{} raw{} no fixed picture style (an adaptive one such as Auto, or a \
             maker whose styles are not read yet) and {} not grouped.",
            survey.no_style,
            if survey.no_style == 1 {
                " has"
            } else {
                "s have"
            },
            if survey.no_style == 1 { "is" } else { "are" },
        ));
    }
    out.join(" ")
}

fn strings(lines: &[String]) -> ModelRc<slint::SharedString> {
    let v: Vec<slint::SharedString> = lines.iter().map(|s| s.as_str().into()).collect();
    ModelRc::new(VecModel::from(v))
}

/// Put what the sheet knows on it.
fn show(st: &State, app: &App) {
    let sheet = &st.camera_match;
    let running = sheet.cancel.is_some();
    app.set_match_running(running);
    app.set_match_results(strings(&sheet.results));
    match &sheet.survey {
        Some(survey) => {
            let existing = greycard_edit::look::store_dir()
                .map(|d| fit::existing_in(&d))
                .unwrap_or_default();
            let plans = fit::plan(&survey.groups, &existing, app.get_match_replace());
            let lines: Vec<String> = survey
                .groups
                .iter()
                .zip(&plans)
                .map(|(g, p)| group_line(g, p))
                .collect();
            app.set_match_groups(strings(&lines));
            app.set_match_summary(summary(survey).into());
            // Any group: one that will be skipped is still reported,
            // and a run is how the user finds out which frames register.
            app.set_match_can_fit(!plans.is_empty());
        }
        None => {
            app.set_match_groups(strings(&[]));
            app.set_match_can_fit(false);
        }
    }
}

/// Survey the sheet's scope on a thread and show the answer.
fn start_survey(st: &mut State, app: &App) {
    st.camera_match.generation += 1;
    st.camera_match.survey = None;
    let generation = st.camera_match.generation;
    let at = Where {
        scope: app.get_match_scope().to_string(),
        library: library_of(st),
        folder: folder_of(st),
    };
    show(st, app);
    app.set_match_summary("reading the frames' picture styles...".into());
    let app_weak = app.as_weak();
    std::thread::spawn(move || {
        let found = survey(&at);
        let _ = slint::invoke_from_event_loop(move || {
            let (Some(app), Some(state)) = (app_weak.upgrade(), STATE.with(|s| s.borrow().clone()))
            else {
                return;
            };
            let mut st = state.borrow_mut();
            if st.camera_match.generation != generation {
                return;
            }
            match found {
                Ok(survey) => {
                    st.camera_match.survey = Some(survey);
                    show(&st, &app);
                }
                Err(why) => {
                    show(&st, &app);
                    app.set_match_summary(why.into());
                }
            }
        });
    });
}

/// Open the sheet on the library when there is one, else the folder.
fn open(st: &mut State, app: &App) {
    let library = library_of(st).is_some();
    let scopes: Vec<String> = if library {
        vec![LIBRARY.into(), FOLDER.into()]
    } else {
        vec![FOLDER.into()]
    };
    app.set_match_scopes(strings(&scopes));
    app.set_match_scope_note(
        if library {
            ""
        } else {
            "No library yet, so this folder alone: add folders to the library for a fit \
             across sessions, lights and lenses."
        }
        .into(),
    );
    if st.camera_match.cancel.is_none() {
        app.set_match_scope(if library { LIBRARY } else { FOLDER }.into());
        st.camera_match.results.clear();
        app.set_match_progress("".into());
        start_survey(st, app);
    } else {
        show(st, app);
    }
    app.set_match_open(true);
}

/// Run the fit over the surveyed groups on a thread of its own.
fn start_run(st: &mut State, app: &App) {
    let Some(survey) = &st.camera_match.survey else {
        return;
    };
    if st.camera_match.cancel.is_some() || survey.groups.is_empty() {
        return;
    }
    let Some(store) = greycard_edit::look::store_dir() else {
        app.set_match_progress("There is no look directory on this machine.".into());
        return;
    };
    let groups = survey.groups.clone();
    let replace = app.get_match_replace();
    let cancel = Arc::new(AtomicBool::new(false));
    st.camera_match.cancel = Some(cancel.clone());
    st.camera_match.results.clear();
    st.camera_match.generation += 1;
    let generation = st.camera_match.generation;
    show(st, app);
    app.set_match_progress("starting...".into());
    let app_weak = app.as_weak();
    // Each message is handled on the window's thread, in order.
    let post = move |message: Message| {
        let app_weak = app_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let (Some(app), Some(state)) = (app_weak.upgrade(), STATE.with(|s| s.borrow().clone()))
            else {
                return;
            };
            let mut st = state.borrow_mut();
            heard(&mut st, &app, generation, message);
        });
    };
    std::thread::spawn(move || {
        let lenses = greycard_lens::Store::user()
            .ok()
            .and_then(|s| s.load())
            .map(|(db, _)| db);
        let started = std::time::Instant::now();
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fit::run(
                &groups,
                &store,
                replace,
                lenses.as_ref(),
                &cancel,
                &mut |p| post(Message::Progress(p)),
            )
        }));
        let end = match ran {
            Ok(results) => {
                let written = results
                    .iter()
                    .filter(|r| {
                        matches!(
                            r.outcome,
                            fit::Outcome::Fitted { .. } | fit::Outcome::Borrowed { .. }
                        )
                    })
                    .count();
                tracing::info!(
                    "camera match: {} groups, {written} looks written in {:.1} s",
                    results.len(),
                    started.elapsed().as_secs_f64()
                );
                match (cancel.load(Ordering::Relaxed), written) {
                    (true, _) => format!("Stopped. {written} written."),
                    (false, 0) => "Done: no look written.".to_string(),
                    (false, 1) => format!("Done: 1 look written to {}.", store.display()),
                    (false, n) => format!("Done: {n} looks written to {}.", store.display()),
                }
            }
            Err(panic) => {
                let why = crate::worker::panic_message(&*panic);
                tracing::error!("camera match: {why}");
                format!("The fit stopped: {why}")
            }
        };
        post(Message::Finished(end));
    });
}

/// What the run says to the window.
enum Message {
    Progress(Progress),
    Finished(String),
}

fn heard(st: &mut State, app: &App, generation: u64, message: Message) {
    if st.camera_match.generation != generation {
        return;
    }
    match message {
        Message::Progress(Progress::Frame { group, index, of }) => {
            app.set_match_progress(format!("frame {index} of {of} in {group}").into());
        }
        Message::Progress(Progress::Done(result)) => {
            st.camera_match.results.push(result.line());
            app.set_match_results(strings(&st.camera_match.results));
        }
        Message::Finished(end) => {
            st.camera_match.cancel = None;
            app.set_match_progress(end.into());
            show(st, app);
            // The new tables in the picker.
            st.looks = greycard_edit::look::list();
            st.look_for = None;
            show_looks(st, &st.edit.look_lut, app);
            app.window().request_redraw();
        }
    }
}

/// Press Fit once the survey is read, as a snapshot's `--sheet
/// matched` asks: tried every 20 ms, `tries` times.
pub(crate) fn fit_when_read(app: &App, tries: u32) {
    let weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(20), move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        if app.get_match_can_fit() {
            app.invoke_match_fit();
        } else if tries > 0 {
            fit_when_read(&app, tries - 1);
        } else {
            tracing::warn!("--sheet matched: the scope was never read");
        }
    });
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>) {
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_match_asked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            open(&mut state.borrow_mut(), &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_match_scope_picked(move |_| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_survey(&mut state.borrow_mut(), &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_match_replace_toggled(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            show(&state.borrow(), &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_match_fit(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_run(&mut state.borrow_mut(), &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_match_cancel(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            if let Some(cancel) = &st.camera_match.cancel {
                cancel.store(true, Ordering::Relaxed);
                app.set_match_progress("stopping after this frame...".into());
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_match_close(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            if st.camera_match.cancel.is_none() {
                app.set_match_open(false);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_match::{Frame, MIN_FRAMES};

    fn group(camera: &str, folders: &[&str], n: usize, fixed: usize) -> Group {
        Group {
            camera: camera.into(),
            maker: "Canon".into(),
            style: "Canon Faithful".into(),
            frames: (0..n)
                .map(|i| Frame {
                    path: PathBuf::from(format!("/s/{}/{i:03}.CR3", folders[i % folders.len()])),
                    taken: None,
                    lens: None,
                    fixed: i < fixed,
                })
                .collect(),
        }
    }

    #[test]
    fn a_group_line_says_what_the_fit_will_do() {
        let g = group("Canon EOS R6m2", &["a", "b"], 60, 30);
        let line = group_line(&g, &Plan::Fit { replaces: None });
        assert!(
            line.starts_with("Canon EOS R6m2 Faithful: 60 frames, 30 with"),
            "{line}"
        );
        assert!(line.contains("fits on 30"), "{line}");
        assert!(
            line.contains("30 with an adaptive setting on are left out"),
            "{line}"
        );
        assert!(!line.contains("narrow"), "{line}");
        let one = group("Canon EOS R6m2", &["a"], MIN_FRAMES + 2, MIN_FRAMES + 2);
        assert!(group_line(&one, &Plan::Fit { replaces: None }).contains("narrow"));
        let warned = Plan::Fit {
            replaces: Some("replaces a table fitted on this body from 60 frames".into()),
        };
        assert!(group_line(&one, &warned).ends_with("from 60 frames"));
        let skipped = Plan::Skip("the table there was fitted on this body itself".into());
        assert!(group_line(&one, &skipped).contains("skipped: the table there"));
        let few = group("Canon EOS R5m2", &["a"], 5, 5);
        let line = group_line(
            &few,
            &Plan::Borrow {
                from: "Canon EOS R6m2".into(),
                donor: 0,
                replaces: None,
            },
        );
        assert!(
            line.contains("borrows the look fitted on Canon EOS R6m2"),
            "{line}"
        );
        assert!(!line.contains("narrow"), "{line}");
    }

    /// A folder the index holds whole is surveyed from it; a raw
    /// dropped in since the last pass sends the survey to the files.
    /// The raws here are not raws at all, which the index keeps as
    /// rows with no tags: enough to count them.
    #[test]
    fn a_folder_with_a_raw_the_index_has_not_seen_is_read_from_the_files() {
        assert!(indexed_whole(&[Some(1), Some(2)]));
        assert!(!indexed_whole(&[Some(1), None]));
        let dir = std::env::temp_dir().join(format!(
            "greycard-match-index-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let shoot = dir.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        std::fs::write(shoot.join("a.CR3"), b"not a raw").unwrap();
        let db = dir.join("library.sqlite");
        let mut lib = greycard_library::Library::open(&db).unwrap();
        lib.index_folder(&shoot, &mut |_| {}).unwrap();
        let found = from_index(&db, &shoot).expect("the folder is indexed whole");
        assert!(found.groups.is_empty());
        assert_eq!(found.no_style, 1);
        std::fs::write(shoot.join("b.CR3"), b"not a raw either").unwrap();
        assert_eq!(from_index(&db, &shoot), None);
        drop(lib);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_summary_names_what_was_not_grouped() {
        let s = Survey {
            groups: Vec::new(),
            unread: 1,
            no_style: 3,
        };
        let text = summary(&s);
        assert!(text.contains("1 raw here has not"), "{text}");
        assert!(
            text.contains("3 raws have no fixed picture style"),
            "{text}"
        );
        assert!(summary(&Survey::default()).is_empty());
    }
}
