//! The camera match's sheet: the Look section's "Fit this camera's
//! look". The scope is surveyed for its bodies and picture styles off
//! the window's thread, the sheet says what the fit will do with each,
//! and Fit runs [`crate::camera_match::run`] on a thread of its own,
//! its progress and each group's result coming back here, the look
//! list read again when it is done.
//!
//! Each body-and-style line has a box, every one checked at first; Fit
//! runs over the checked groups, and the choice is kept for the
//! scope's next run (`settings::match_unchecked`). A refit's sheet
//! lists only the look's own groups, all checked, and keeps nothing.
//!
//! The run develops under the open picture's display curve, which the
//! sheet says, and every table it writes is that curve's. Opened from
//! the Look section's refit, the sheet runs over the chosen look's own
//! group (and its donor, for a borrowed look) rather than the scope's
//! every group.

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
    /// The display curve the run develops under: the open picture's
    /// when the sheet was opened.
    curve: greycard_edit::DisplayCurve,
    /// The look a refit is for, when the sheet was opened for one, and
    /// its tables as the look list had them.
    refit: Option<(String, Vec<greycard_edit::look::Entry>)>,
    /// What the unchecked groups are remembered under: "library", or
    /// the folder's path.
    scope_key: String,
    /// The groups left unchecked, by their look names. A group not
    /// here is checked, so one seen for the first time is.
    unchecked: std::collections::BTreeSet<String>,
}

impl Sheet {
    /// The groups the run is over: the scope's, or for a refit the
    /// look's own and its donor's. A refit whose look no group in the
    /// scope makes runs over none, so Fit is off, and
    /// [`Sheet::curve_note`] says why: a run over every group is the
    /// sheet's own job, not the refit's.
    fn groups(&self) -> Option<Vec<Group>> {
        let survey = self.survey.as_ref()?;
        Some(match &self.refit {
            Some((name, tables)) => {
                let tables: Vec<&greycard_edit::look::Entry> = tables.iter().collect();
                fit::refit_groups(&survey.groups, name, &tables).unwrap_or_default()
            }
            None => survey.groups.clone(),
        })
    }

    /// One flag a group of [`Sheet::groups`]: whether it is checked.
    fn chosen(&self, groups: &[Group]) -> Vec<bool> {
        groups
            .iter()
            .map(|g| !self.unchecked.contains(&g.name()))
            .collect()
    }

    /// The box of group `i` pressed: the scope's groups now unchecked,
    /// to keep. None for a group the sheet does not list, and while a
    /// run is under way.
    fn toggle(&mut self, i: usize) -> Option<Vec<String>> {
        if self.cancel.is_some() {
            return None;
        }
        let name = self.groups()?.get(i)?.name();
        if !self.unchecked.remove(&name) {
            self.unchecked.insert(name);
        }
        Some(self.unchecked.iter().cloned().collect())
    }

    /// The line that says which curve the run develops under, and
    /// what a refit is for.
    fn curve_note(&self) -> String {
        let under = self.curve.phrase();
        let mut out = format!("Fits under {under}; the look applies only there.");
        if let Some((name, _)) = &self.refit {
            match (&self.survey, self.groups().is_some_and(|g| !g.is_empty())) {
                (Some(_), false) => out.push_str(&format!(" Nothing here to refit {name} from.")),
                _ => out.push_str(&format!(" Refitting {name}.")),
            }
        }
        out
    }
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

/// What a scope's choice is kept under: "library", or the folder as the
/// disk spells it.
fn scope_key(at: &Where) -> String {
    if at.scope == LIBRARY {
        return "library".to_string();
    }
    // The folder as the browser spells it: no call to the disk on the
    // window's thread, which a root that does not answer would hang.
    match &at.folder {
        Some(f) => f.display().to_string(),
        None => String::new(),
    }
}

/// Whether a tick is written to the settings file: not in a batch run,
/// which leaves the user's settings alone. (A test has no settings file
/// unless it sets one.) Reading is always done, so a capture shows the
/// choice the scope was left with.
fn persists(st: &State) -> bool {
    !st.batch
}

/// The groups the scope was left with unchecked.
fn remembered(key: &str) -> Vec<String> {
    if key.is_empty() {
        return Vec::new();
    }
    crate::settings::unchecked_for(&crate::settings::Settings::load().match_unchecked, key)
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
        Plan::Donor => out.push_str(" — read only, for a borrow"),
        Plan::Skip(why) if why == fit::NOT_CHOSEN => out.push_str(" — not chosen"),
        Plan::Skip(why) => out.push_str(&format!(" — skipped: {why}")),
    }
    out
}

/// What a chosen group that borrows must also say: that the group it
/// borrows from is not being fitted. Empty for anything else.
fn donor_note(plan: &Plan, chosen: &[bool]) -> &'static str {
    match plan {
        Plan::Borrow { donor, .. } if !chosen.get(*donor).copied().unwrap_or(true) => {
            "; its source is unchecked, so read only"
        }
        _ => "",
    }
}

/// The Fit button's words, and the line under it: the groups checked
/// are counted, the ones read only for a borrow are said, and when
/// none is checked the reason it is off. `donors` is how many groups
/// not checked are developed anyway because a checked one borrows from
/// them.
pub(crate) fn fit_label(checked: usize, have_groups: bool, donors: usize) -> (String, String) {
    match (checked, have_groups) {
        (0, true) => ("Fit".into(), "No group is checked.".into()),
        (0, false) => ("Fit".into(), String::new()),
        (n, _) => {
            let plural = |n: usize| if n == 1 { "" } else { "s" };
            let note = if donors > 0 {
                format!(
                    "Also reads {donors} unchecked group{} to borrow from.",
                    plural(donors)
                )
            } else {
                String::new()
            };
            (format!("Fit {n} group{}", plural(n)), note)
        }
    }
}

/// The line under the groups: what the survey left out of them.
pub(crate) fn summary(survey: &Survey) -> String {
    let mut out = Vec::new();
    if survey.unread > 0 {
        out.push(format!(
            "{} raw{} here {} no picture style read yet.",
            survey.unread,
            if survey.unread == 1 { "" } else { "s" },
            if survey.unread == 1 { "has" } else { "have" },
        ));
    }
    if survey.no_style > 0 {
        out.push(format!(
            "{} raw{} no fixed picture style and {} not grouped.",
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
    app.set_match_curve_note(sheet.curve_note().into());
    match (&sheet.survey, sheet.groups()) {
        (Some(survey), Some(groups)) => {
            let existing = greycard_edit::look::store_dir()
                .map(|d| fit::existing_in(&d))
                .unwrap_or_default();
            let chosen = sheet.chosen(&groups);
            let plans = fit::plan_chosen(
                &groups,
                &existing,
                sheet.curve,
                app.get_match_replace(),
                &chosen,
            );
            let rows: Vec<MatchGroup> = groups
                .iter()
                .zip(&plans)
                .zip(&chosen)
                .map(|((g, p), &checked)| MatchGroup {
                    name: g.name().into(),
                    text: format!("{}{}", group_line(g, p), donor_note(p, &chosen)).into(),
                    checked,
                })
                .collect();
            app.set_match_groups(ModelRc::new(VecModel::from(rows)));
            app.set_match_summary(summary(survey).into());
            // Any checked group: one that will be skipped is still
            // reported, and a run is how the user finds out which
            // frames register.
            let n = chosen.iter().filter(|c| **c).count();
            let donors = plans.iter().filter(|p| matches!(p, Plan::Donor)).count();
            let (label, note) = fit_label(n, !groups.is_empty(), donors);
            app.set_match_fit_label(label.into());
            app.set_match_fit_note(note.into());
            app.set_match_can_fit(n > 0);
        }
        _ => {
            app.set_match_groups(ModelRc::new(VecModel::from(Vec::<MatchGroup>::new())));
            app.set_match_fit_label("Fit".into());
            app.set_match_fit_note("".into());
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
    // What the scope's last run left unchecked; a refit's sheet starts
    // with everything of its look's checked and keeps nothing.
    st.camera_match.scope_key = scope_key(&at);
    st.camera_match.unchecked = if st.camera_match.refit.is_some() {
        Default::default()
    } else {
        remembered(&st.camera_match.scope_key).into_iter().collect()
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

/// Open the sheet on the library when there is one, else the folder,
/// to fit under the open picture's display curve; for a refit of the
/// look named, over that look's group. A run under way keeps the sheet
/// as it is.
pub(crate) fn open(st: &mut State, app: &App, refit: Option<String>) {
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
            "No library yet: this folder only."
        }
        .into(),
    );
    if st.camera_match.cancel.is_none() {
        // The LIGHT section's choice, which is the picture's even
        // before the edit has recorded a switch just made.
        st.camera_match.curve =
            greycard_edit::DisplayCurve::from_name(app.get_display_curve().as_str())
                .unwrap_or(st.edit.display_curve);
        st.camera_match.refit = refit.map(|name| {
            let tables = greycard_edit::look::tables_of(&st.looks, &name)
                .into_iter()
                .cloned()
                .collect();
            (name, tables)
        });
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
    let Some(groups) = st.camera_match.groups() else {
        return;
    };
    if st.camera_match.cancel.is_some() || groups.is_empty() {
        return;
    }
    let Some(store) = greycard_edit::look::store_dir() else {
        app.set_match_progress("There is no look directory on this machine.".into());
        return;
    };
    let chosen = st.camera_match.chosen(&groups);
    if !chosen.iter().any(|c| *c) {
        return;
    }
    let curve = st.camera_match.curve;
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
                fit::Choices {
                    curve,
                    replace,
                    chosen: &chosen,
                },
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
            // The new tables in the picker, and the look resolved
            // again: a refit replaces the table in place, and the
            // picture that asked for it now takes it.
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
            open(&mut state.borrow_mut(), &app, None);
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
        app.on_match_group_toggled(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(unchecked) = st.camera_match.toggle(i.max(0) as usize) else {
                return;
            };
            // Kept as it is ticked, for the scope's next run; a refit's
            // narrowed list is not the scope's choice.
            if persists(&st) && st.camera_match.refit.is_none() {
                let key = st.camera_match.scope_key.clone();
                if !key.is_empty() {
                    let mut settings = crate::settings::Settings::load();
                    crate::settings::remember_unchecked(
                        &mut settings.match_unchecked,
                        &key,
                        unchecked,
                    );
                    settings.save();
                }
            }
            show(&st, &app);
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
            make: "Canon".into(),
            model: camera.into(),
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
            look: None,
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
        crate::testing::remove_dir_retry(&dir);
    }

    /// Opened for a refit, the sheet runs over the look's group and
    /// says so and under which curve; a look nothing here makes leaves
    /// the sheet with nothing to run, Fit off, and the line says why.
    #[test]
    fn a_refit_sheet_runs_over_the_looks_group_under_the_pictures_curve() {
        let sheet = |refit: Option<&str>| Sheet {
            survey: Some(Survey {
                groups: vec![
                    group("Canon EOS R6m2", &["a", "b"], 30, 30),
                    group("Canon EOS R5m2", &["a"], 30, 30),
                ],
                unread: 0,
                no_style: 0,
            }),
            curve: greycard_edit::DisplayCurve::Agx,
            refit: refit.map(|name| (name.to_string(), Vec::new())),
            ..Default::default()
        };
        let names = |s: &Sheet| {
            s.groups()
                .unwrap()
                .iter()
                .map(Group::name)
                .collect::<Vec<_>>()
        };
        let all = sheet(None);
        assert_eq!(names(&all).len(), 2);
        assert!(
            all.curve_note().starts_with("Fits under AgX"),
            "{}",
            all.curve_note()
        );
        let one = sheet(Some("Canon EOS R5m2 Faithful"));
        assert_eq!(names(&one), ["Canon EOS R5m2 Faithful"]);
        assert!(
            one.curve_note()
                .ends_with("Refitting Canon EOS R5m2 Faithful."),
            "{}",
            one.curve_note()
        );
        let gone = sheet(Some("Slide Warm"));
        assert!(names(&gone).is_empty());
        assert!(
            gone.curve_note()
                .contains("Nothing here to refit Slide Warm"),
            "{}",
            gone.curve_note()
        );
    }

    fn two_bodies_two_styles() -> Sheet {
        let mut other = group("Canon EOS R5m2", &["a"], 30, 30);
        other.style = "Canon Standard".into();
        Sheet {
            survey: Some(Survey {
                groups: vec![
                    group("Canon EOS R6m2", &["a", "b"], 30, 30),
                    other,
                    group("Canon EOS R5m2", &["a"], 5, 5),
                ],
                unread: 0,
                no_style: 0,
            }),
            curve: greycard_edit::DisplayCurve::Channels,
            ..Default::default()
        }
    }

    /// Every group is checked at first; a press flips one, and what is
    /// unchecked is what the next sheet on the scope starts without.
    #[test]
    fn every_group_is_checked_at_first_and_the_choice_is_what_is_kept() {
        let mut sheet = two_bodies_two_styles();
        let groups = sheet.groups().unwrap();
        assert_eq!(sheet.chosen(&groups), [true, true, true]);
        let kept = sheet.toggle(1).unwrap();
        assert_eq!(kept, ["Canon EOS R5m2 Standard"]);
        assert_eq!(sheet.chosen(&groups), [true, false, true]);
        // The scope's next sheet: the same choice read back, and a group
        // never seen before is checked.
        let mut list = Vec::new();
        crate::settings::remember_unchecked(&mut list, "library", kept);
        let mut next = two_bodies_two_styles();
        next.unchecked = crate::settings::unchecked_for(&list, "library")
            .into_iter()
            .collect();
        let mut more = group("Sony ILCE-7M4", &["a"], 30, 30);
        more.style = "Sony Standard".into();
        let mut all = next.groups().unwrap();
        all.push(more);
        assert_eq!(next.chosen(&all), [true, false, true, true]);
        // Pressed again it is checked, and nothing is kept for the scope.
        assert!(sheet.toggle(1).unwrap().is_empty());
        // A press past the list, or while a run is under way, is nothing.
        assert!(sheet.toggle(9).is_none());
        sheet.cancel = Some(Arc::new(AtomicBool::new(false)));
        assert!(sheet.toggle(0).is_none());
    }

    /// Tick, save, reopen: a box unchecked on a scope is unchecked when
    /// the sheet is opened on it again, and a group not seen before is
    /// checked. The settings file is a scratch one.
    #[test]
    fn a_tick_is_saved_and_the_next_sheet_on_the_scope_has_it() {
        use crate::testing::{click, labeled, state_for, window};
        let dir = std::env::temp_dir().join(format!(
            "greycard-match-keep-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        crate::settings::use_file(Some(dir.join("settings.json")));
        let app = window(3);
        let (state, _worker) = state_for(&app, crate::testing::folder(3));
        let mut sheet = two_bodies_two_styles();
        sheet.scope_key = "/shoot/a".into();
        state.borrow_mut().camera_match = sheet;
        app.set_match_open(true);
        show(&state.borrow(), &app);
        let (at, size) = labeled(&app, "Fit Canon EOS R5m2 Standard");
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        let kept = crate::settings::Settings::load().match_unchecked;
        assert_eq!(
            crate::settings::unchecked_for(&kept, "/shoot/a"),
            ["Canon EOS R5m2 Standard"]
        );
        // Reopened: the scope's choice is read back, another scope's is
        // not touched, and a group the file never saw is checked.
        assert_eq!(remembered("/shoot/a"), ["Canon EOS R5m2 Standard"]);
        assert!(remembered("/shoot/b").is_empty());
        let mut next = two_bodies_two_styles();
        next.unchecked = remembered("/shoot/a").into_iter().collect();
        let groups = next.groups().unwrap();
        assert_eq!(next.chosen(&groups), [true, false, true]);
        crate::settings::use_file(None);
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn the_button_counts_the_groups_checked_and_says_why_it_is_off() {
        assert_eq!(fit_label(3, true, 0).0, "Fit 3 groups");
        assert_eq!(fit_label(3, true, 0).1, "");
        let (label, note) = fit_label(2, true, 1);
        assert_eq!(label, "Fit 2 groups");
        assert!(note.starts_with("Also reads 1 unchecked group"), "{note}");
        assert_eq!(fit_label(1, true, 0).0, "Fit 1 group");
        let (label, note) = fit_label(0, true, 0);
        assert_eq!(label, "Fit");
        assert!(note.contains("No group is checked"), "{note}");
        assert_eq!(fit_label(0, false, 0), ("Fit".to_string(), String::new()));
    }

    #[test]
    fn a_group_that_borrows_from_one_not_fitted_says_so() {
        let borrow = Plan::Borrow {
            from: "Canon EOS R6m2".into(),
            donor: 0,
            replaces: None,
        };
        assert!(donor_note(&borrow, &[false, true]).contains("read only"));
        assert_eq!(donor_note(&borrow, &[true, true]), "");
        let few = group("Canon EOS R5m2", &["a"], 5, 5);
        let line = group_line(&few, &Plan::Skip(fit::NOT_CHOSEN.into()));
        assert!(line.ends_with("not chosen"), "{line}");
        let donor = group_line(&few, &Plan::Donor);
        assert!(donor.contains("read only"), "{donor}");
    }

    /// The window: a box a group, found by its label and clicked inside
    /// its bounds; the Fit button counts the checked ones and goes off
    /// at none, with the reason under it.
    #[test]
    fn the_sheet_has_a_box_a_group_and_the_button_counts_them() {
        use crate::testing::{click, labeled, state_for, window};
        let app = window(3);
        let (state, _worker) = state_for(&app, crate::testing::folder(3));
        // The donor, and a body of the same style with too few frames
        // to fit on its own, which borrows from it.
        let mut sheet = two_bodies_two_styles();
        sheet.survey.as_mut().unwrap().groups = vec![
            group("Canon EOS R6m2", &["a", "b"], 30, 30),
            group("Canon EOS R5m2", &["a"], 5, 5),
        ];
        state.borrow_mut().camera_match = sheet;
        app.set_match_open(true);
        show(&state.borrow(), &app);
        assert_eq!(app.get_match_groups().row_count(), 2);
        assert!(app.get_match_groups().iter().all(|g| g.checked));
        assert_eq!(app.get_match_fit_label(), "Fit 2 groups");
        assert!(app.get_match_can_fit());
        let press = |name: &str| {
            let (at, size) = labeled(&app, name);
            click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        };
        // Unchecking the donor: the button counts one, its borrower says
        // the donor is not being fitted, and the donor's line says its
        // frames are read for the borrow and nothing of it is written.
        press("Fit Canon EOS R6m2 Faithful");
        assert_eq!(app.get_match_fit_label(), "Fit 1 group");
        assert!(
            app.get_match_fit_note()
                .starts_with("Also reads 1 unchecked group"),
            "{}",
            app.get_match_fit_note()
        );
        let rows: Vec<_> = app.get_match_groups().iter().collect();
        assert!(!rows[0].checked && rows[1].checked);
        assert!(
            rows[1].text.contains("source is unchecked"),
            "{}",
            rows[1].text
        );
        assert!(
            rows[0].text.contains("read only, for a borrow"),
            "{}",
            rows[0].text
        );
        // Unchecking the borrower too: nothing to fit, the button off with
        // its reason, and the donor is simply skipped.
        press("Fit Canon EOS R5m2 Faithful");
        assert_eq!(app.get_match_fit_label(), "Fit");
        assert!(!app.get_match_can_fit());
        assert!(app.get_match_fit_note().contains("No group is checked"));
        let rows: Vec<_> = app.get_match_groups().iter().collect();
        assert!(rows[0].text.ends_with("not chosen"), "{}", rows[0].text);
        // Fit does nothing at zero.
        app.invoke_match_fit();
        assert!(state.borrow().camera_match.cancel.is_none());
        // Checked again, one press back.
        press("Fit Canon EOS R6m2 Faithful");
        assert_eq!(app.get_match_fit_label(), "Fit 1 group");
        assert!(app.get_match_can_fit());
    }

    #[test]
    fn the_summary_names_what_was_not_grouped() {
        let s = Survey {
            groups: Vec::new(),
            unread: 1,
            no_style: 3,
        };
        let text = summary(&s);
        assert!(text.contains("1 raw here has no picture style"), "{text}");
        assert!(
            text.contains("3 raws have no fixed picture style"),
            "{text}"
        );
        assert!(summary(&Survey::default()).is_empty());
    }
}
