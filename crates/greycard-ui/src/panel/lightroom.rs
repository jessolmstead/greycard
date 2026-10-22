//! The Lightroom catalog import from the window: a catalog chosen, the
//! sheet asking where each of its roots is now and the set its
//! collections go under, the import's dry run in a short summary, then
//! the write on a thread of its own, its progress on the status line
//! and under the pane's button, and a line of what was written. The
//! rest of the report is under Details on both.
//!
//! The sheet decides nothing the command does not: every answer it
//! takes is a field of [`lrcat::Options`], each with the command's flag
//! (`--root`, `--set`, `--force`, `--no-creations`), and what it shows
//! is the report's, in the sheet's shape ([`lrcat::Report::sheet`]).
//! The guess at a root's folder is the command's too: where the catalog
//! says, else a folder of the root's name beside the catalog or under
//! one of the library's roots.
//!
//! The window's thread never reads the catalog or the disk: each dry
//! run and the write run on a thread and post back, a dry run asked
//! for later winning over one that answers late. A dry run says how far
//! it has come under the buttons and can be canceled, which stops its
//! thread; a write cannot.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use greycard_library::lightroom_catalog::{self as lrcat, Progress, Report};

use crate::files::tilde;
use crate::*;

/// What the sheet says when the run writes no sidecars: the report is
/// shown, and Write stays off.
pub(crate) const NO_SIDECARS: &str =
    "This run writes no sidecars (--no-sidecars), so nothing can be imported.";

/// The sheet's side of the state.
#[derive(Default)]
pub(crate) struct Sheet {
    pub(crate) catalog: Option<PathBuf>,
    /// Where the user said each root is: its path as the catalog has
    /// it, and the folder here.
    pub(crate) roots: Vec<(String, PathBuf)>,
    /// The last report: the dry run's, or the write's.
    pub(crate) report: Option<Report>,
    /// The dry run the window last asked for.
    pub(crate) generation: u64,
    /// What the sheet does once the next dry run lands, for a capture.
    pub(crate) then: Then,
    /// The write's thread, while it runs.
    pub(crate) thread: Option<std::thread::JoinHandle<()>>,
    /// A dry run asked for and not landed.
    pub(crate) reading: bool,
    /// What stops that dry run's thread: set by Cancel, or by a later
    /// dry run, whose answer wins anyway.
    pub(crate) stop: Option<Arc<AtomicBool>>,
    /// The roots the next catalog opened starts from, as if chosen on
    /// the sheet: `--root` with `--lightroom`.
    pub(crate) seed: Vec<(String, PathBuf)>,
}

impl Sheet {
    /// Whether anything is under way: a dry run reading, a write, or a
    /// step waiting on one. What a capture waits out.
    pub(crate) fn busy(&self) -> bool {
        self.reading || self.thread.is_some() || self.then != Then::Nothing
    }
}

/// What follows a dry run without a click: for `--sheet`'s steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Then {
    #[default]
    Nothing,
    /// The report shown, as Check shows it.
    Report,
    /// The write started, as Write starts it.
    Write,
}

/// The import's options as the sheet has them: the roots the user
/// chose, the set as typed, the switches.
fn options(st: &State, app: &App, write: bool) -> Option<lrcat::Options> {
    Some(lrcat::Options {
        roots: st.lightroom.roots.clone(),
        write,
        force: app.get_lr_force(),
        // The field starts at the catalog's name; empty is none.
        set: Some(app.get_lr_set().to_string()),
        library: st.views.index_path.clone()?,
        sidecars_in_folder: st.window.placement == greycard_edit::Placement::Folder,
        no_creations: !app.get_lr_creations(),
    })
}

/// A catalog chosen: the sheet opened on it, its roots read.
pub(crate) fn open(st: &mut State, app: &App, catalog: PathBuf) {
    if st.lightroom.thread.is_some() {
        return;
    }
    let name = catalog
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    st.lightroom.catalog = Some(catalog.clone());
    st.lightroom.roots = std::mem::take(&mut st.lightroom.seed);
    st.lightroom.report = None;
    app.set_lr_title(format!("Import {name} from Lightroom").into());
    app.set_lr_subtitle(tilde(&catalog).into());
    app.set_lr_note("".into());
    app.set_lr_set(name.into());
    app.set_lr_creations(true);
    app.set_lr_force(false);
    app.set_lr_step(0);
    app.set_lr_open(true);
    if st.views.index_path.is_none() {
        app.set_lr_note(
            "There is no library to import into: this installation has no data folder.".into(),
        );
        show(st, app);
        return;
    }
    dry_run(st, app);
}

/// The dry run the sheet stands for, on a thread.
pub(crate) fn dry_run(st: &mut State, app: &App) {
    let (Some(catalog), Some(opts)) = (st.lightroom.catalog.clone(), options(st, app, false))
    else {
        return;
    };
    if let Some(last) = st.lightroom.stop.take() {
        last.store(true, Ordering::Relaxed);
    }
    let stop = Arc::new(AtomicBool::new(false));
    st.lightroom.stop = Some(stop.clone());
    st.lightroom.generation += 1;
    st.lightroom.reading = true;
    let generation = st.lightroom.generation;
    app.set_lr_busy(true);
    app.set_lr_busy_text("reading the catalog...".into());
    app.set_lr_fraction(-1.0);
    let weak = app.as_weak();
    std::thread::spawn(move || {
        let told = weak.clone();
        let mut last = Instant::now() - Duration::from_secs(1);
        let read = caught(|| {
            lrcat::import_until(
                &catalog,
                &opts,
                &mut |p| {
                    if !due(&mut last, p) {
                        return;
                    }
                    let _ = told.upgrade_in_event_loop(move |app| {
                        let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                            return;
                        };
                        heard(&state.borrow(), &app, generation, p);
                    });
                },
                &stop,
            )
        });
        let _ = weak.upgrade_in_event_loop(move |app| {
            let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                return;
            };
            landed(&state, &app, generation, read);
        });
    });
}

/// The import run on its thread, a panic in it an error like any
/// other: the sheet is never left waiting on a thread that is gone.
pub(crate) fn caught(
    run: impl FnOnce() -> std::result::Result<Report, lrcat::ImportError>,
) -> std::result::Result<Report, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(read) => read.map_err(|e| e.to_string()),
        Err(payload) => Err(format!(
            "the import failed: {}",
            crate::worker::panic_message(&*payload)
        )),
    }
}

/// Whether a step of the import is said: at least a tenth of a second
/// after the last, so the window is not asked to redraw for every file;
/// but each of the catalog's folders, each slow to look for, and the
/// finish always.
fn due(last: &mut Instant, p: Progress) -> bool {
    let now = Instant::now();
    let always = matches!(p, Progress::Mapping { .. } | Progress::Finishing);
    if !always && now - *last < Duration::from_millis(100) {
        return false;
    }
    *last = now;
    true
}

/// How far a dry run has come, said under the buttons, on the window's
/// thread. One asked for before the last, or canceled, says nothing.
pub(crate) fn heard(st: &State, app: &App, generation: u64, p: Progress) {
    if generation != st.lightroom.generation || !st.lightroom.reading {
        return;
    }
    app.set_lr_busy_text(format!("{}...", progress_words(p)).into());
    app.set_lr_fraction(fraction_of(p));
}

/// How far a step has come, 0 to 1, for the bar; below nothing for a
/// step that counts nothing, which keeps the bar's place.
fn fraction_of(p: Progress) -> f32 {
    match p {
        Progress::Mapping { done, of }
        | Progress::Looking { done, of }
        | Progress::Writing { done, of }
            if of > 0 =>
        {
            (done as f32 / of as f32).clamp(0.0, 1.0)
        }
        Progress::Finishing => 1.0,
        _ => -1.0,
    }
}

/// Cancel, Escape or a click beside the sheet: the sheet closed, and a
/// dry run under way stopped, its answer and what was to follow it
/// dropped. A write is not stopped, and its sheet stays until it lands.
pub(crate) fn cancel(st: &mut State, app: &App) {
    if st.lightroom.thread.is_some() {
        return;
    }
    if let Some(stop) = st.lightroom.stop.take() {
        stop.store(true, Ordering::Relaxed);
    }
    st.lightroom.generation += 1;
    st.lightroom.reading = false;
    st.lightroom.then = Then::Nothing;
    app.set_lr_busy(false);
    app.set_lr_open(false);
}

/// A dry run's answer, on the window's thread. One asked for before
/// the last, or canceled, says nothing.
pub(crate) fn landed(
    state: &Rc<RefCell<State>>,
    app: &App,
    generation: u64,
    read: std::result::Result<Report, String>,
) {
    let then = {
        let mut st = state.borrow_mut();
        if generation != st.lightroom.generation {
            return;
        }
        st.lightroom.reading = false;
        st.lightroom.stop = None;
        app.set_lr_busy(false);
        match read {
            Ok(report) => {
                app.set_lr_note("".into());
                st.lightroom.report = Some(report);
            }
            Err(e) => {
                tracing::warn!("lightroom import: {e}");
                app.set_lr_note(e.into());
                st.lightroom.report = None;
            }
        }
        show(&st, app);
        std::mem::take(&mut st.lightroom.then)
    };
    match then {
        Then::Nothing => {}
        Then::Report => app.set_lr_step(1),
        Then::Write => {
            app.set_lr_step(1);
            write(state, app);
        }
    }
}

/// The sheet as the state has it.
pub(crate) fn show(st: &State, app: &App) {
    let report = st.lightroom.report.as_ref();
    let roots: Vec<LightroomRoot> = report
        .map(|r| {
            let names: Vec<String> = r.roots.iter().map(|root| root.short_name()).collect();
            r.roots
                .iter()
                .zip(&names)
                .map(|(root, name)| {
                    let ambiguous = names.iter().filter(|n| *n == name).count() > 1;
                    let taken = root.local.as_deref().filter(|_| root.confirmed).map(tilde);
                    let under = match (ambiguous, taken) {
                        (true, Some(at)) => format!("{} at {at}", root.catalog),
                        (false, Some(at)) => at,
                        (true, None) => root.catalog.clone(),
                        (false, None) => String::new(),
                    };
                    LightroomRoot {
                        name: name.as_str().into(),
                        catalog: root.catalog.as_str().into(),
                        under: under.into(),
                        state: root.state(&tilde).into(),
                        taken: root.confirmed,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    app.set_lr_roots(ModelRc::new(VecModel::from(roots)));
    let sheet = report.map(|r| r.sheet(&tilde)).unwrap_or_default();
    let (warning, remedy) = sheet.warning.unwrap_or_default();
    app.set_lr_warning(warning.into());
    app.set_lr_remedy(remedy.into());
    app.set_lr_summary(sheet.summary.into());
    app.set_lr_done(sheet.done.unwrap_or_default().into());
    let details: Vec<ReportLine> = sheet
        .details
        .into_iter()
        .map(|(label, text)| ReportLine {
            label: label.into(),
            text: text.into(),
        })
        .collect();
    app.set_lr_details(ModelRc::new(VecModel::from(details)));
    app.set_lr_has_creations(report.is_some_and(|r| {
        r.collections.iter().any(|c| c.creation.is_some()) || !r.creations_left.is_empty()
    }));
    app.set_lr_already(report.map_or(0, |r| r.already) as i32);
    app.set_lr_ready(
        report.is_some_and(|r| r.found > 0)
            && st.views.index_path.is_some()
            && st.window.write_sidecars,
    );
    if !st.window.write_sidecars && app.get_lr_note().is_empty() {
        app.set_lr_note(NO_SIDECARS.into());
    }
}

/// A root's folder chosen: the mapping taken, the dry run again. A
/// folder chosen after the sheet was closed is let go.
pub(crate) fn root_chosen(st: &mut State, app: &App, row: usize, folder: PathBuf) {
    if !app.get_lr_open() {
        return;
    }
    let Some(catalog) = st
        .lightroom
        .report
        .as_ref()
        .and_then(|r| r.roots.get(row))
        .map(|r| r.catalog.clone())
    else {
        return;
    };
    st.lightroom.roots.retain(|(from, _)| *from != catalog);
    st.lightroom.roots.push((catalog, folder));
    dry_run(st, app);
}

/// Write: the sheet goes, the import runs on its thread, its progress
/// said as it goes; the sheet comes back with the report when done.
pub(crate) fn write(state: &Rc<RefCell<State>>, app: &App) {
    let mut st = state.borrow_mut();
    if st.lightroom.thread.is_some() {
        return;
    }
    if !st.window.write_sidecars {
        app.set_lr_note(NO_SIDECARS.into());
        return;
    }
    let (Some(catalog), Some(opts)) = (st.lightroom.catalog.clone(), write_options(&mut st, app))
    else {
        return;
    };
    // The sheet stays up while it writes, so nothing is edited under
    // it; its progress is on the status line and under the button.
    app.set_lr_busy(true);
    app.set_lr_busy_text("writing...".into());
    app.set_lr_fraction(0.0);
    app.set_lr_running(true);
    say(app, "importing the catalog: reading it...".into());
    let weak = app.as_weak();
    st.lightroom.thread = Some(std::thread::spawn(move || {
        let told = weak.clone();
        let mut last = Instant::now() - Duration::from_secs(1);
        let report = caught(|| {
            lrcat::import_with(&catalog, &opts, &mut |p| {
                if !due(&mut last, p) {
                    return;
                }
                let line = progress_line(p);
                let _ = told.upgrade_in_event_loop(move |app| {
                    if app.get_lr_running() {
                        say(&app, line);
                        app.set_lr_busy_text(format!("{}...", progress_words(p)).into());
                        app.set_lr_fraction(fraction_of(p));
                    }
                });
            })
        });
        // The written frames as the index keys them too, asked of the
        // disk here rather than on the window's thread.
        let keys = keys_of(&report);
        let _ = weak.upgrade_in_event_loop(move |app| {
            let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                return;
            };
            wrote(&state, &app, report, keys);
        });
    }));
}

/// The options a write runs under, the window made ready for it. The
/// folders the report showed, guessed or chosen, are the ones written
/// to: a write never guesses again, and cannot land where the report
/// did not say. And the frame on screen, its panel moved and not yet
/// saved, is saved first, so the import reads what the window holds.
pub(crate) fn write_options(st: &mut State, app: &App) -> Option<lrcat::Options> {
    let mut opts = options(st, app, true)?;
    if let Some(report) = &st.lightroom.report {
        opts.roots = resolved(report, &opts.roots);
    }
    // A panel moved and not yet saved (its save waiting on the panel
    // to rest) is saved now: the frame then has an edit of its own, and
    // the import leaves it alone unless told to write over it. A panel
    // at rest is the sidecar already, and nothing is written.
    if st.current.is_some() && st.culling.cull.is_none() && st.develop.save_timer.running() {
        st.develop.save_timer.stop();
        let outgoing = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
        crate::panel::edit::save_edit(st, outgoing);
    }
    Some(opts)
}

/// The frames a write gave sidecars to, as the import found them and as
/// the index keys them: what the window's list may hold them as.
pub(crate) fn keys_of(report: &std::result::Result<Report, String>) -> Vec<PathBuf> {
    let Some(w) = report.as_ref().ok().and_then(|r| r.written.as_ref()) else {
        return Vec::new();
    };
    w.frames
        .iter()
        .flat_map(|p| [p.clone(), greycard_library::key_path(p)])
        .collect()
}

/// The roots a write is given: each the report took, at the folder it
/// took it at, and the user's own for the rest.
fn resolved(report: &Report, given: &[(String, PathBuf)]) -> Vec<(String, PathBuf)> {
    let mut roots: Vec<(String, PathBuf)> = report
        .roots
        .iter()
        .filter(|r| r.confirmed)
        .filter_map(|r| Some((r.catalog.clone(), r.local.clone()?)))
        .collect();
    for (from, to) in given {
        let taken = report
            .roots
            .iter()
            .any(|r| r.confirmed && (r.catalog == *from || r.name == *from));
        if !taken {
            roots.push((from.clone(), to.clone()));
        }
    }
    roots
}

/// The status line's words for how far the import has come.
pub(crate) fn progress_line(p: Progress) -> String {
    p.line()
}

/// How far the import has come, as the status line and the sheet say
/// it: the library's words, which the command says too.
fn progress_words(p: Progress) -> String {
    p.to_string()
}

/// The write done, however it went: the line said, the window's
/// collections, stacks and roots read again from what it wrote, and
/// the sheet back with the report.
pub(crate) fn wrote(
    state: &Rc<RefCell<State>>,
    app: &App,
    report: std::result::Result<Report, String>,
    keys: Vec<PathBuf>,
) {
    app.set_lr_running(false);
    app.set_lr_busy(false);
    let line = {
        let mut st = state.borrow_mut();
        st.lightroom.thread = None;
        match report {
            Ok(mut report) => {
                // The write was given the report's folders as mappings;
                // each root says how the report found it, as it did.
                if let Some(before) = &st.lightroom.report {
                    for r in &mut report.roots {
                        if let Some(b) = before.roots.iter().find(|b| b.catalog == r.catalog) {
                            r.how = b.how.clone();
                        }
                    }
                }
                let line = match &report.written {
                    Some(w) => {
                        let n = |k: usize, one: &str, many: &str| {
                            format!("{k} {}", if k == 1 { one } else { many })
                        };
                        format!(
                            "imported the catalog: {} written, {} left alone, {}, {}",
                            n(w.sidecars, "frame", "frames"),
                            w.left_alone,
                            n(
                                w.collections + w.joined + w.smart,
                                "collection",
                                "collections"
                            ),
                            n(w.stacks, "stack", "stacks")
                        )
                    }
                    None => "imported the catalog".to_string(),
                };
                app.set_lr_note("".into());
                st.lightroom.report = Some(report);
                line
            }
            Err(e) => {
                tracing::warn!("lightroom import: {e}");
                app.set_lr_note(format!("Not imported: {e}").into());
                format!("the catalog was not imported: {e}")
            }
        }
    };
    took_up(state, app);
    reread_written(state, app, keys, "lightroom import");
    say(app, line.clone());
    {
        let st = state.borrow();
        show(&st, app);
    }
    app.set_lr_step(2);
    app.set_lr_open(true);
    // The pane's line goes after a while; the status plate keeps it
    // until the next words.
    let weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_secs(20), move || {
        if let Some(app) = weak.upgrade()
            && !app.get_lr_running()
            && app.get_lr_status() == line.as_str()
        {
            app.set_lr_status("".into());
        }
    });
}

/// What the write changed on disk taken up by the window: the
/// collections and the stacks read again, and each root it added
/// kept, watched and passed over, as the roots pane's own add does.
fn took_up(state: &Rc<RefCell<State>>, app: &App) {
    let added: Vec<PathBuf> = {
        let mut st = state.borrow_mut();
        let Some(path) = st.views.index_path.clone() else {
            return;
        };
        let (rows, names) = (
            st.views.collections.rows.clone(),
            st.views.collections.names.clone(),
        );
        st.views.collections =
            crate::collections::Store::load(crate::collections::Store::dir_beside(&path));
        (st.views.collections.rows, st.views.collections.names) = (rows, names);
        crate::panel::collections::show(&st, app);
        st.views.stacks = crate::stacks::Store::load(greycard_library::stacks::dir_in(
            &crate::collections::Store::dir_beside(&path),
        ));
        st.lightroom
            .report
            .as_ref()
            .and_then(|r| r.written.as_ref())
            .map(|w| {
                w.roots
                    .iter()
                    .filter(|(_, said)| said.added())
                    .map(|(dir, _)| dir.clone())
                    .collect()
            })
            .unwrap_or_default()
    };
    if !added.is_empty() {
        let worker = WORKER.with(|w| w.borrow().clone());
        crate::roots::taken_from_file(state, app, worker.as_ref(), added);
    }
}

/// The frames the write gave sidecars to, as the window holds them,
/// read again from what was written: each file's sidecar taken from the
/// disk, its cells' pictures asked again, and the frame on screen opened
/// again on its sidecar with nothing of its old panel saved over it, so
/// the next key or slider moves from the import's edit and not over it.
/// `whose` names the import in the log. The RawTherapee profiles'
/// import takes up its write the same way.
pub(crate) fn reread_written(
    state: &Rc<RefCell<State>>,
    app: &App,
    keys: Vec<PathBuf>,
    whose: &str,
) {
    let mut st = state.borrow_mut();
    let written: std::collections::HashSet<PathBuf> = keys.into_iter().collect();
    let files: Vec<usize> = st
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| written.contains(*f))
        .map(|(i, _)| i)
        .collect();
    for &file in &files {
        crate::rows::reread_file(&mut st, app, file);
        for c in st.cells_of_file(file) {
            crate::edited::changed(&mut st, c);
        }
    }
    let on_screen = st.current.filter(|&c| {
        st.culling.cull.is_none() && st.file_at(c).is_some_and(|f| files.contains(&f))
    });
    if let Some(c) = on_screen
        && let Some(worker) = WORKER.with(|w| w.borrow().clone())
    {
        st.current = None;
        crate::panel::browser::open_frame(&mut st, app, &worker, c);
    }
    tracing::info!(
        "{whose}: {} of the frames written held by the window, read again",
        files.len()
    );
}

fn say(app: &App, line: String) {
    app.set_status(line.as_str().into());
    app.set_lr_status(line.into());
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>) {
    {
        let weak = app.as_weak();
        app.on_lr_asked(move || {
            let app = weak.unwrap();
            let start = dirs::picture_dir()
                .or_else(dirs::home_dir)
                .unwrap_or_default();
            app.set_status("choosing a Lightroom catalog...".into());
            let weak = app.as_weak();
            // A test opens no chooser: it answers as a cancel.
            chooser::choose_open(
                "Import a Lightroom catalog",
                start,
                (
                    "Lightroom catalogs".to_string(),
                    vec!["*.lrcat".into(), "*.LRCAT".into()],
                ),
                move |chosen| {
                    let _ = weak.upgrade_in_event_loop(move |app| {
                        let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                            return;
                        };
                        match chosen {
                            Ok(Some(catalog)) => open(&mut state.borrow_mut(), &app, catalog),
                            Ok(None) => app.set_status("import canceled".into()),
                            Err(e) => {
                                tracing::warn!("file chooser: {e:#}");
                                app.set_status(chooser::chooser_failed(&e).into());
                            }
                        }
                    });
                },
            );
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_lr_choose_root(move |row| {
            let app = weak.unwrap();
            let st = state.borrow();
            let Some(root) = st
                .lightroom
                .report
                .as_ref()
                .and_then(|r| r.roots.get(row as usize))
            else {
                return;
            };
            let start = root
                .local
                .clone()
                .or_else(|| {
                    st.lightroom
                        .catalog
                        .as_deref()
                        .and_then(Path::parent)
                        .map(Path::to_path_buf)
                })
                .or_else(dirs::home_dir)
                .unwrap_or_default();
            let weak = app.as_weak();
            chooser::choose_folder("Where the catalog's folder is now", start, move |chosen| {
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                        return;
                    };
                    match chosen {
                        Ok(Some(folder)) => {
                            root_chosen(&mut state.borrow_mut(), &app, row as usize, folder)
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!("folder chooser: {e:#}");
                            app.set_lr_note(chooser::chooser_failed(&e).into());
                        }
                    }
                });
            });
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_lr_check(move || {
            let app = weak.unwrap();
            let mut st = state.borrow_mut();
            // The set, the switches as they are now: the report is of
            // what a write would do with them.
            st.lightroom.then = Then::Report;
            dry_run(&mut st, &app);
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_lr_cancel(move || {
            let app = weak.unwrap();
            cancel(&mut state.borrow_mut(), &app);
        });
    }
    {
        let weak = app.as_weak();
        app.on_lr_back(move || {
            weak.unwrap().set_lr_step(0);
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_lr_write(move || {
            let app = weak.unwrap();
            write(&state, &app);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_library::lightroom_catalog::Options;

    /// A catalog of two frames under one root, the files a folder of
    /// the root's name beside it, written as Lightroom keeps one; one
    /// frame rated and developed, the other rated, both in a collection.
    fn catalog(dir: &Path) -> PathBuf {
        let path = dir.join("Shoot.lrcat");
        let shots = dir.join("Shoots");
        std::fs::create_dir_all(&shots).unwrap();
        for n in ["IMG_0001.CR3", "IMG_0002.CR3"] {
            std::fs::write(shots.join(n), format!("not a raw, {n}")).unwrap();
        }
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE Adobe_variablesTable (id_local INTEGER PRIMARY KEY, name, value);
             INSERT INTO Adobe_variablesTable (name, value) VALUES ('Adobe_DBVersion', '1504001');
             CREATE TABLE AgLibraryRootFolder (id_local INTEGER PRIMARY KEY, id_global,
                 absolutePath, name, relativePathFromCatalog);
             CREATE TABLE AgLibraryFolder (id_local INTEGER PRIMARY KEY, id_global, parentId,
                 pathFromRoot, rootFolder);
             CREATE TABLE AgLibraryFile (id_local INTEGER PRIMARY KEY, id_global, baseName,
                 extension, folder);
             CREATE TABLE Adobe_images (id_local INTEGER PRIMARY KEY, id_global, rootFile,
                 masterImage, copyName, rating, pick, colorLabels NOT NULL DEFAULT '');
             CREATE TABLE AgLibraryKeyword (id_local INTEGER PRIMARY KEY, id_global, name,
                 parent);
             CREATE TABLE AgLibraryKeywordImage (id_local INTEGER PRIMARY KEY, image, tag);
             CREATE TABLE AgLibraryIPTC (id_local INTEGER PRIMARY KEY, image, caption);
             CREATE TABLE Adobe_AdditionalMetadata (id_local INTEGER PRIMARY KEY, image, xmp);
             CREATE TABLE Adobe_libraryImageDevelopHistoryStep (id_local INTEGER PRIMARY KEY,
                 image, name);
             CREATE TABLE Adobe_imageDevelopSettings (id_local INTEGER PRIMARY KEY, image, text,
                 processVersion, hasDevelopAdjustmentsEx);
             CREATE TABLE AgLibraryCollection (id_local INTEGER PRIMARY KEY, creationId, name,
                 parent, systemOnly);
             CREATE TABLE AgLibraryCollectionImage (id_local INTEGER PRIMARY KEY, collection,
                 image, positionInCollection);
             CREATE TABLE AgLibraryCollectionContent (id_local INTEGER PRIMARY KEY, collection,
                 content, owningModule);
             CREATE TABLE AgLibraryFolderStack (id_local INTEGER PRIMARY KEY, id_global);
             INSERT INTO AgLibraryRootFolder VALUES (1, 'r', 'D:/Away/Shoots/', 'Shoots', NULL);
             INSERT INTO AgLibraryFolder VALUES (2, 'f', NULL, '', 1);
             INSERT INTO AgLibraryFile VALUES (10, 'a', 'IMG_0001', 'CR3', 2);
             INSERT INTO AgLibraryFile VALUES (11, 'b', 'IMG_0002', 'CR3', 2);
             INSERT INTO Adobe_images VALUES (100, 'i', 10, NULL, NULL, 3.0, 1.0, '');
             INSERT INTO Adobe_images VALUES (101, 'j', 11, NULL, NULL, 2.0, 0.0, '');
             INSERT INTO Adobe_imageDevelopSettings (image, text, processVersion,
                 hasDevelopAdjustmentsEx) VALUES (100, 's = { Exposure2012 = 0.5 }', '15.4', 1.0);
             INSERT INTO AgLibraryCollection VALUES (1, 'com.adobe.ag.library.collection',
                 'Picks', NULL, 0.0);
             INSERT INTO AgLibraryCollection VALUES (2, 'com.adobe.ag.bookExport',
                 'Album', NULL, 0.0);
             INSERT INTO AgLibraryCollectionImage VALUES (1, 1, 100, 'a');
             INSERT INTO AgLibraryCollectionImage VALUES (2, 1, 101, 'b');
             INSERT INTO AgLibraryCollectionImage VALUES (3, 2, 100, 'a');",
        )
        .unwrap();
        path
    }

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-lightroom-sheet-{what}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The dry run the sheet asked for, run here and landed as its
    /// thread would land it.
    fn land_now(state: &Rc<RefCell<State>>, app: &App) {
        let (catalog, opts, generation) = {
            let st = state.borrow();
            (
                st.lightroom.catalog.clone().unwrap(),
                options(&st, app, false).unwrap(),
                st.lightroom.generation,
            )
        };
        let read = lrcat::import(&catalog, &opts).map_err(|e| e.to_string());
        landed(state, app, generation, read);
    }

    #[test]
    fn the_sheet_asks_the_roots_and_the_set_then_shows_the_summary() {
        let dir = scratch("steps");
        let catalog = catalog(&dir);
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        let library = dir.join("library").join("library.sqlite");
        state.borrow_mut().views.index_path = Some(library.clone());

        // The roots: the folder of the root's name beside the catalog,
        // guessed, and its files found.
        open(&mut state.borrow_mut(), &app, catalog.clone());
        assert!(app.get_lr_open());
        assert_eq!(app.get_lr_step(), 0);
        assert_eq!(app.get_lr_set(), "Shoot", "the catalog's name");
        assert!(app.get_lr_busy());
        land_now(&state, &app);
        assert!(!app.get_lr_busy());
        let roots = app.get_lr_roots();
        assert_eq!(roots.row_count(), 1);
        let r = roots.row_data(0).unwrap();
        assert_eq!(r.catalog, "D:/Away/Shoots/");
        assert_eq!(r.name, "Shoots");
        assert_eq!(
            r.under,
            tilde(&dir.join("Shoots")),
            "where it was taken, and no catalog path for the only root of its name"
        );
        assert!(r.taken);
        assert_eq!(r.state, "found, 2 of 2 checked");
        assert!(app.get_lr_has_creations(), "the book");
        assert!(app.get_lr_ready());

        // A folder chosen in its place: the mapping is the command's
        // --root, and the dry run says so.
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        root_chosen(&mut state.borrow_mut(), &app, 0, elsewhere.clone());
        land_now(&state, &app);
        let r = app.get_lr_roots().row_data(0).unwrap();
        assert!(!r.taken);
        assert!(r.state.starts_with("not found in "), "{}", r.state);
        assert!(r.state.ends_with("elsewhere"), "{}", r.state);
        assert!(!app.get_lr_ready(), "nothing found, nothing to write");
        root_chosen(&mut state.borrow_mut(), &app, 0, dir.join("Shoots"));
        land_now(&state, &app);
        assert!(app.get_lr_ready());

        // The set emptied and the book left out, then Check: the
        // summary, and under Details the rest of the report for those
        // options.
        app.set_lr_set("".into());
        app.set_lr_creations(false);
        state.borrow_mut().lightroom.then = Then::Report;
        state.borrow_mut().lightroom.generation += 1;
        land_now(&state, &app);
        assert_eq!(app.get_lr_step(), 1);
        assert_eq!(
            app.get_lr_summary(),
            "2 frames, all found. 1 with edits. 1 collection."
        );
        assert_eq!(app.get_lr_warning(), "", "the catalog has no log to miss");
        assert_eq!(app.get_lr_done(), "", "nothing written yet");
        let opts = Options {
            roots: vec![("D:/Away/Shoots/".into(), dir.join("Shoots"))],
            set: Some(String::new()),
            library: library.clone(),
            no_creations: true,
            ..Options::default()
        };
        let command = lrcat::import(&catalog, &opts).unwrap();
        let details: Vec<(String, String)> = app
            .get_lr_details()
            .iter()
            .map(|l| (l.label.to_string(), l.text.to_string()))
            .collect();
        assert_eq!(details, command.sheet(&tilde).details);
        let has =
            |label: &str, text: &str| details.iter().any(|(l, t)| l == label && t.contains(text));
        assert!(has("catalog", "Shoot.lrcat"), "{details:?}");
        assert!(has("written by", "schema 1504001"), "{details:?}");
        assert!(
            has("folder", "D:/Away/Shoots/ at ")
                && has("folder", ", as chosen: 2 of 2 files found"),
            "{details:?}"
        );
        assert!(has("", "Picks: 2 frames, 2 found"), "{details:?}");
        assert!(has("left out", "Album"), "{details:?}");
        assert!(has("sidecars", "new ones"), "{details:?}");
        // Only what applies: nothing missing, no copies, no stacks.
        for absent in ["not found", "copies", "stacks", "smart", "wrote"] {
            assert!(
                !details.iter().any(|(l, _)| l == absent),
                "{absent}: {details:?}"
            );
        }
        // The sheet's words, never the command's flags.
        assert!(
            !details.iter().any(|(_, t)| t.contains("--")),
            "{details:?}"
        );
        assert!(!app.get_lr_summary().contains("--"));
        // The command says what it always said.
        assert_eq!(
            command
                .lines()
                .iter()
                .filter(|(_, t)| t.contains("--write"))
                .count(),
            1
        );

        // A dry run that answers after a later one was asked says
        // nothing.
        let stale = state.borrow().lightroom.generation - 1;
        landed(&state, &app, stale, Err("late".into()));
        assert_eq!(app.get_lr_note(), "");
        crate::testing::remove_scratch(state, &dir);
    }

    /// A dry run says under the buttons how far it has come, its
    /// catalog's folders and then its files; one asked for before the
    /// last, or one landed, says nothing more.
    #[test]
    fn a_dry_run_says_how_far_it_has_come() {
        let dir = crate::testing::scratch_dir("lightroom-progress");
        let catalog = catalog(&dir);
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        state.borrow_mut().views.index_path = Some(dir.join("library").join("library.sqlite"));
        open(&mut state.borrow_mut(), &app, catalog);
        assert_eq!(app.get_lr_busy_text(), "reading the catalog...");
        assert_eq!(app.get_lr_fraction(), -1.0, "the read counts nothing");
        let generation = state.borrow().lightroom.generation;
        heard(
            &state.borrow(),
            &app,
            generation,
            Progress::Mapping { done: 3, of: 12 },
        );
        assert_eq!(app.get_lr_fraction(), 0.25, "the bar follows the step");
        assert_eq!(
            app.get_lr_busy_text(),
            "looking for the catalog's folders, 3 of 12..."
        );
        heard(
            &state.borrow(),
            &app,
            generation,
            Progress::Looking {
                done: 1200,
                of: 23000,
            },
        );
        assert_eq!(
            app.get_lr_busy_text(),
            "looking for the catalog's files, 1,200 of 23,000..."
        );
        // The status line's words are the same, the import named.
        assert_eq!(
            progress_line(Progress::Mapping { done: 3, of: 12 }),
            "importing the catalog: looking for the catalog's folders, 3 of 12"
        );
        // A stale run says nothing.
        heard(
            &state.borrow(),
            &app,
            generation - 1,
            Progress::Mapping { done: 1, of: 12 },
        );
        assert_eq!(
            app.get_lr_busy_text(),
            "looking for the catalog's files, 1,200 of 23,000..."
        );
        // Nor one that has landed, its last word posted behind it.
        land_now(&state, &app);
        assert!(!app.get_lr_busy());
        heard(
            &state.borrow(),
            &app,
            generation,
            Progress::Mapping { done: 1, of: 12 },
        );
        assert_eq!(
            app.get_lr_busy_text(),
            "looking for the catalog's files, 1,200 of 23,000..."
        );
        crate::testing::remove_scratch(state, &dir);
    }

    /// Cancel while a dry run reads: the sheet closes, its thread is
    /// told to stop, and its answer, landing late, is dropped with the
    /// write that was to follow it. While a write runs, Cancel does
    /// nothing.
    #[test]
    fn cancel_during_a_dry_run_closes_the_sheet_and_drops_its_answer() {
        let dir = crate::testing::scratch_dir("lightroom-cancel");
        let catalog = catalog(&dir);
        let library = dir.join("library").join("library.sqlite");
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        state.borrow_mut().views.index_path = Some(library.clone());
        open(&mut state.borrow_mut(), &app, catalog.clone());
        assert!(app.get_lr_busy());
        // As a --sheet capture's write step asks for it.
        state.borrow_mut().lightroom.then = Then::Write;
        let (generation, stop, opts) = {
            let st = state.borrow();
            (
                st.lightroom.generation,
                st.lightroom.stop.clone().unwrap(),
                options(&st, &app, false).unwrap(),
            )
        };
        assert!(!stop.load(Ordering::Relaxed));
        app.invoke_lr_cancel();
        assert!(!app.get_lr_open());
        assert!(!app.get_lr_busy());
        assert!(stop.load(Ordering::Relaxed), "the thread told to stop");
        {
            let st = state.borrow();
            assert!(st.lightroom.generation > generation);
            assert!(
                !st.lightroom.busy(),
                "nothing left for a capture to wait on"
            );
        }
        // The answer lands late, whole, as if it had not heard the stop.
        let read = caught(|| lrcat::import(&catalog, &opts));
        assert!(read.is_ok());
        landed(&state, &app, generation, read);
        assert!(!app.get_lr_open());
        assert_eq!(app.get_lr_step(), 0);
        assert!(!app.get_lr_running());
        {
            let st = state.borrow();
            assert!(st.lightroom.report.is_none(), "the answer dropped");
            assert!(st.lightroom.thread.is_none(), "no write started");
        }
        for n in ["IMG_0001.CR3", "IMG_0002.CR3"] {
            assert!(greycard_edit::Sidecar::find(&dir.join("Shoots").join(n)).is_none());
        }
        assert!(!crate::collections::Store::dir_beside(&library).exists());

        // A folder the chooser answers with after the sheet closed
        // starts nothing.
        state.borrow_mut().lightroom.report = Some(lrcat::import(&catalog, &opts).unwrap());
        let closed = state.borrow().lightroom.generation;
        root_chosen(&mut state.borrow_mut(), &app, 0, dir.join("Shoots"));
        assert_eq!(state.borrow().lightroom.generation, closed);
        assert!(!state.borrow().lightroom.reading);
        assert!(!app.get_lr_busy());

        // A write running: Cancel leaves the sheet up.
        open(&mut state.borrow_mut(), &app, catalog);
        land_now(&state, &app);
        state.borrow_mut().lightroom.thread = Some(std::thread::spawn(|| {}));
        let generation = state.borrow().lightroom.generation;
        app.invoke_lr_cancel();
        assert!(app.get_lr_open());
        assert_eq!(state.borrow().lightroom.generation, generation);
        let thread = state.borrow_mut().lightroom.thread.take().unwrap();
        thread.join().unwrap();
        crate::testing::remove_scratch(state, &dir);
    }

    /// A panic on the import's thread lands as the import's error: the
    /// sheet says it, and its buttons are back.
    #[test]
    fn a_panic_on_the_import_s_thread_lands_as_a_note() {
        let dir = crate::testing::scratch_dir("lightroom-panic");
        let catalog = catalog(&dir);
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        state.borrow_mut().views.index_path = Some(dir.join("library").join("library.sqlite"));
        open(&mut state.borrow_mut(), &app, catalog);
        assert!(app.get_lr_busy());
        let generation = state.borrow().lightroom.generation;
        let read = std::thread::spawn(|| caught(|| panic!("a catalog unlike any other")))
            .join()
            .unwrap();
        assert_eq!(
            read.as_ref().err().map(String::as_str),
            Some("the import failed: a catalog unlike any other")
        );
        landed(&state, &app, generation, read);
        assert!(!app.get_lr_busy());
        assert!(app.get_lr_open());
        assert_eq!(
            app.get_lr_note(),
            "the import failed: a catalog unlike any other"
        );
        assert!(!state.borrow().lightroom.busy());
        crate::testing::remove_scratch(state, &dir);
    }

    /// The catalog's two frames as small TIFFs the window opens, the
    /// first developed with a turned crop.
    fn pictures(dir: &Path) -> PathBuf {
        use greycard_library::fixture::{R5, R6, write_frame};
        let path = catalog(dir);
        let shots = dir.join("Shoots");
        for n in ["IMG_0001.CR3", "IMG_0002.CR3"] {
            std::fs::remove_file(shots.join(n)).unwrap();
        }
        write_frame(&shots.join("a.tif"), &R5, 1);
        write_frame(&shots.join("b.tif"), &R6, 2);
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "UPDATE AgLibraryFile SET baseName = 'a', extension = 'tif' WHERE id_local = 10;
             UPDATE AgLibraryFile SET baseName = 'b', extension = 'tif' WHERE id_local = 11;
             UPDATE Adobe_imageDevelopSettings SET text =
                 's = { Exposure2012 = 0.5, CropLeft = 0.25, CropRight = 0.75, CropAngle = 1 }'
                 WHERE image = 100;",
        )
        .unwrap();
        path
    }

    /// The frame open in the window when the write lands is opened again
    /// on what was written: a key pressed after it writes the import's
    /// edit with the rating, never the panel from before the write.
    #[test]
    fn the_frame_on_screen_takes_the_write_and_a_key_after_it_keeps_it() {
        let dir = crate::testing::scratch_dir("lightroom-on-screen");
        let catalog = pictures(&dir);
        let shots = dir.join("Shoots");
        let app = crate::testing::window(0);
        let (state, worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        crate::panel::browser::open_folder(&state, &app, &worker, &shots);
        crate::roots::land_sent(&state, &app, &worker);
        WORKER.with(|w| *w.borrow_mut() = Some(worker.clone()));
        app.invoke_select(0);
        assert_eq!(state.borrow().current, Some(0));
        let a = shots.join("a.tif");
        assert!(greycard_edit::Sidecar::load(&a).unwrap().is_none());
        state.borrow_mut().views.index_path = Some(dir.join("library").join("library.sqlite"));
        open(&mut state.borrow_mut(), &app, catalog.clone());
        land_now(&state, &app);
        // The write as Write runs it, landed.
        let opts = write_options(&mut state.borrow_mut(), &app).unwrap();
        let report = lrcat::import_with(&catalog, &opts, &mut |_| {}).map_err(|e| e.to_string());
        let keys = keys_of(&report);
        wrote(&state, &app, report, keys);
        let written = greycard_edit::Sidecar::load(&a).unwrap().unwrap();
        assert!(!written.history.is_empty(), "the import's state");
        assert!(written.current.geometry.crop.is_some());
        // The panel is the written edit's.
        assert_eq!(app.get_exposure(), written.current.light.exposure);
        // A rating after it: the import's edit kept, the rating with it.
        assert!(app.invoke_meta_key("3".into()));
        let after = greycard_edit::Sidecar::load(&a).unwrap().unwrap();
        assert_eq!(after.meta.rating, 3);
        assert_eq!(after.history.len(), written.history.len());
        assert_eq!(after.current.light.exposure, written.current.light.exposure);
        assert_eq!(after.current.geometry, written.current.geometry);
        WORKER.with(|w| *w.borrow_mut() = None);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A panel moved and waiting to be saved when Write is pressed is
    /// saved first, and the frame, an edit of its own now, is left alone.
    #[test]
    fn a_panel_not_yet_saved_is_saved_and_its_frame_left_alone() {
        let dir = crate::testing::scratch_dir("lightroom-unsaved");
        let catalog = pictures(&dir);
        let shots = dir.join("Shoots");
        let app = crate::testing::window(0);
        let (state, worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        crate::panel::browser::open_folder(&state, &app, &worker, &shots);
        crate::roots::land_sent(&state, &app, &worker);
        app.invoke_select(0);
        state.borrow_mut().views.index_path = Some(dir.join("library").join("library.sqlite"));
        open(&mut state.borrow_mut(), &app, catalog.clone());
        land_now(&state, &app);
        // The slider moved, its save waiting on the panel to rest.
        app.set_exposure(0.8);
        state.borrow().develop.save_timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_secs(60),
            || {},
        );
        let opts = write_options(&mut state.borrow_mut(), &app).unwrap();
        assert!(!state.borrow().develop.save_timer.running());
        let a = shots.join("a.tif");
        let saved = greycard_edit::Sidecar::load(&a).unwrap().unwrap();
        assert_eq!(saved.current.light.exposure, 0.8);
        let report = lrcat::import_with(&catalog, &opts, &mut |_| {}).unwrap();
        let w = report.written.unwrap();
        assert_eq!(w.left_alone, 1, "the frame with the user's edit");
        let after = greycard_edit::Sidecar::load(&a).unwrap().unwrap();
        assert_eq!(after.current.light.exposure, 0.8);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A run told to write no sidecars shows the report with Write off,
    /// says why, and a Write pressed anyway writes nothing.
    #[test]
    fn with_no_sidecars_the_report_shows_and_write_is_off() {
        let dir = scratch("no-sidecars");
        let catalog = catalog(&dir);
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        let library = dir.join("library").join("library.sqlite");
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = false;
            st.views.index_path = Some(library.clone());
        }
        open(&mut state.borrow_mut(), &app, catalog);
        land_now(&state, &app);
        assert_eq!(app.get_lr_roots().row_count(), 1);
        assert!(!app.get_lr_ready());
        assert_eq!(app.get_lr_note(), NO_SIDECARS);
        write(&state, &app);
        assert!(state.borrow().lightroom.thread.is_none());
        assert!(!app.get_lr_running());
        for n in ["IMG_0001.CR3", "IMG_0002.CR3"] {
            assert!(greycard_edit::Sidecar::find(&dir.join("Shoots").join(n)).is_none());
        }
        assert!(!crate::collections::Store::dir_beside(&library).exists());
        crate::testing::remove_scratch(state, &dir);
    }

    #[test]
    fn a_write_goes_to_the_folders_the_report_showed() {
        let dir = scratch("resolved");
        let catalog = catalog(&dir);
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        state.borrow_mut().views.index_path = Some(dir.join("library").join("library.sqlite"));
        open(&mut state.borrow_mut(), &app, catalog);
        land_now(&state, &app);
        // The guess beside the catalog becomes the write's mapping.
        let opts = write_options(&mut state.borrow_mut(), &app).unwrap();
        assert_eq!(
            opts.roots,
            vec![("D:/Away/Shoots/".to_string(), dir.join("Shoots"))]
        );
        crate::testing::remove_scratch(state, &dir);
    }

    #[test]
    fn a_write_lands_with_the_report_and_the_window_takes_up_what_it_wrote() {
        let dir = scratch("write");
        let catalog = catalog(&dir);
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        let library = dir.join("library").join("library.sqlite");
        state.borrow_mut().views.index_path = Some(library.clone());
        open(&mut state.borrow_mut(), &app, catalog.clone());
        land_now(&state, &app);
        // The write as its thread runs it, landed: given the report's
        // folders, as Write gives them.
        let opts = write_options(&mut state.borrow_mut(), &app).unwrap();
        let mut seen = Vec::new();
        let report = lrcat::import_with(&catalog, &opts, &mut |p| seen.push(progress_line(p)));
        assert!(
            seen.iter()
                .any(|l| l == "importing the catalog: writing 2 of 2 frames"),
            "{seen:?}"
        );
        let report = report.map_err(|e| e.to_string());
        let keys = keys_of(&report);
        wrote(&state, &app, report, keys);
        assert!(app.get_lr_open());
        assert_eq!(app.get_lr_step(), 2);
        assert!(!app.get_lr_running());
        assert!(
            app.get_status()
                .starts_with("imported the catalog: 2 frames written"),
            "{}",
            app.get_status()
        );
        assert_eq!(app.get_lr_done(), "Wrote 2 sidecars.");
        // Under Details, what the write did, and the root as the report
        // found it, not "as chosen" for the mapping the write was
        // handed.
        let details: Vec<(String, String)> = app
            .get_lr_details()
            .iter()
            .map(|l| (l.label.to_string(), l.text.to_string()))
            .collect();
        assert!(
            details.contains(&("wrote".into(), "collections: 2 written".into())),
            "{details:?}"
        );
        assert!(
            details
                .iter()
                .any(|(l, t)| l == "root" && t.contains("Shoots")),
            "{details:?}"
        );
        assert!(
            details
                .iter()
                .any(|(l, t)| l == "folder"
                    && t.contains(", beside the catalog: 2 of 2 files found")),
            "{details:?}"
        );
        assert!(
            !details.iter().any(|(_, t)| t.contains("as chosen")),
            "{details:?}"
        );
        // The window's collections are the ones it wrote: Picks and
        // the book, under the catalog's set.
        let st = state.borrow();
        let names: Vec<String> = st
            .views
            .collections
            .list
            .iter()
            .map(|c| c.name.clone())
            .collect();
        assert!(names.iter().any(|n| n == "Picks"), "{names:?}");
        assert!(names.iter().any(|n| n == "Album"), "{names:?}");
        assert!(
            st.views
                .library
                .roots
                .list()
                .iter()
                .any(|r| r.ends_with("Shoots")),
            "{:?}",
            st.views.library.roots.list()
        );
        drop(st);
        crate::testing::remove_scratch(state, &dir);
    }

    /// Two roots of one folder name: each says the catalog's path to it
    /// under the name, and a root of a name of its own does not; a
    /// root taken says where.
    #[test]
    fn roots_of_one_name_say_their_paths() {
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        let root = |catalog: &str| lrcat::RootReport {
            catalog: catalog.into(),
            name: String::new(),
            local: None,
            how: lrcat::Found::Not,
            checked: 0,
            checked_found: 0,
            confirmed: false,
            files: 3,
            found: 0,
            near_miss: None,
        };
        state.borrow_mut().lightroom.report = Some(Report {
            roots: vec![
                root("D:/2024/Shoots/"),
                lrcat::RootReport {
                    local: Some("/here/Shoots".into()),
                    confirmed: true,
                    ..root("E:/2025/Shoots")
                },
                lrcat::RootReport {
                    local: Some("/here/Family".into()),
                    confirmed: true,
                    ..root("E:/Family")
                },
                root("E:/Gone"),
            ],
            ..Report::default()
        });
        show(&state.borrow(), &app);
        let roots: Vec<LightroomRoot> = app.get_lr_roots().iter().collect();
        let shown: Vec<(&str, &str, &str)> = roots
            .iter()
            .map(|r| (r.name.as_str(), r.under.as_str(), r.state.as_str()))
            .collect();
        assert_eq!(
            shown,
            vec![
                ("Shoots", "D:/2024/Shoots/", "not found"),
                (
                    "Shoots",
                    "E:/2025/Shoots at /here/Shoots",
                    "found, 0 of 3 checked"
                ),
                ("Family", "/here/Family", "found, 0 of 3 checked"),
                ("Gone", "", "not found"),
            ]
        );
        assert_eq!(roots[1].catalog, "E:/2025/Shoots");
    }
}
