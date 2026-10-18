//! RawTherapee's profiles brought in from the window: offered once when
//! a folder opens holding `.pp3` files beside its frames, then a sheet
//! with the import's dry run as the command prints it, a switch for
//! writing over the frames with edits here already, the write on a
//! thread of its own with its progress, and the report again with what
//! was written.
//!
//! The sheet decides nothing the command does not: every answer it
//! takes is a field of [`rt::Options`] with the command's flag
//! (`--write`, `--force`), and what it shows is the report's own lines
//! ([`rt::Report::lines`]).
//!
//! **The offer, once a folder.** When a folder opens, a thread off the
//! window's counts its frames with a profile beside them that an import
//! would take unasked (those with no history and no stars, label or
//! flag here); with any, a bar
//! under the folder's name offers them. Not now answers it, and so does
//! a write that brings any frame in; Import... alone does not, so a
//! sheet opened and canceled leaves the offer for the folder's next
//! open. An answered folder is never offered again: the settings keep it
//! (`rawtherapee_answered`), by its canonical path. Not in the folder:
//! a `.greycard` made there to hold the answer would send that folder's
//! new sidecars into it from then on, and an answer is this machine's
//! user's, not the shoot's. What travels with the shoot is what the
//! import wrote: a frame it brought in has a history or meta and is not
//! counted again, so a folder imported elsewhere is not offered where it
//! lands. A folder is looked at once a run; the watcher's and the
//! index's passes merge into the list and never open it again, so they
//! never ask. Nor is a folder under a root that is offline looked at,
//! nor any folder in a run told to write no sidecars.
//!
//! The window's thread never reads the folder or the profiles: the
//! look, each dry run and the write run on a thread and post back, a
//! dry run asked for later winning over one that answers late.

use std::collections::{HashMap, HashSet};

use greycard_library::rawtherapee::{self as rt, Progress, Report, Voice};

use crate::files::tilde;
use crate::*;

/// The sheet's side of the state, and the offer's.
#[derive(Default)]
pub(crate) struct Sheet {
    /// The folder the sheet is over.
    pub(crate) folder: Option<PathBuf>,
    /// The last report: the dry run's, or the write's.
    pub(crate) report: Option<Report>,
    /// The dry run the window last asked for.
    pub(crate) generation: u64,
    /// A dry run asked for and not landed.
    pub(crate) reading: bool,
    /// The write's thread, while it runs.
    pub(crate) thread: Option<std::thread::JoinHandle<()>>,
    /// What the sheet does once the next dry run lands, for a capture.
    pub(crate) then: Then,
    /// The folders answered this run, by Not now, or by a write that
    /// brought a frame in: what the settings keep, and all a run with no
    /// settings file has.
    pub(crate) answered: HashSet<PathBuf>,
    /// The folders looked at this run, and how many of their frames
    /// were waiting.
    pub(crate) looked: HashMap<PathBuf, usize>,
    /// A folder being looked at.
    pub(crate) looking: Option<PathBuf>,
    /// The folder offered now.
    pub(crate) offered: Option<PathBuf>,
    /// Offer whatever was answered, in a batch run too: for a capture
    /// of the offer.
    pub(crate) force_offer: bool,
}

impl Sheet {
    /// Whether anything is under way: a look, a dry run, a write, or a
    /// step waiting on one. What a capture waits out.
    pub(crate) fn busy(&self) -> bool {
        self.looking.is_some()
            || self.reading
            || self.thread.is_some()
            || self.then != Then::Nothing
    }
}

/// What follows a dry run without a click: for `--sheet`'s steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Then {
    #[default]
    Nothing,
    /// The write started, as Write starts it.
    Write,
}

/// How many frames the sheet lists; a line says how many more.
pub(crate) const SHOWN_FRAMES: usize = 200;

/// The offer's words for `n` frames waiting: frames, since a picture
/// with no raw of its name beside it can have a profile too.
fn offer_words(n: usize) -> String {
    format!(
        "RawTherapee profiles found beside {n} {}",
        if n == 1 { "frame" } else { "frames" }
    )
}

/// What the sheet says when the run writes no sidecars.
pub(crate) const NO_SIDECARS: &str =
    "This run writes no sidecars (--no-sidecars), so nothing can be imported.";

/// Whether the settings at `file` have `dir` answered.
pub(crate) fn answered_in(file: &Path, dir: &Path) -> bool {
    settings::Settings::load_from(file)
        .rawtherapee_answered
        .iter()
        .any(|d| Path::new(d) == dir)
}

/// A folder opened (the one [`crate::panel::recent::opened`] records):
/// an offer of another folder goes, and this one is looked at off the
/// window's thread unless answered or looked at already this run. Never
/// in a batch run, which nobody is watching and whose picture a bar
/// would spoil, unless a capture asked for the offer; never in a run that
/// writes no sidecars, which could not import them; and never for a
/// folder under a root that is offline, whose disk may not answer at all
/// (it is not counted as looked at, so it is looked at once it is back).
pub(crate) fn folder_opened(st: &mut State, app: &App) {
    let dir = st.views.recent.open.clone();
    if st.rawtherapee.offered.is_some() && st.rawtherapee.offered != dir {
        st.rawtherapee.offered = None;
        app.set_rt_offer("".into());
    }
    let Some(dir) = dir else {
        return;
    };
    let force = st.rawtherapee.force_offer;
    if st.window.batch && !force {
        return;
    }
    if !force && st.rawtherapee.answered.contains(&dir) {
        return;
    }
    if !st.window.write_sidecars {
        return;
    }
    if st
        .views
        .library
        .offline
        .iter()
        .any(|root| dir.starts_with(root))
    {
        return;
    }
    if let Some(&n) = st.rawtherapee.looked.get(&dir) {
        offer(st, app, &dir, n);
        return;
    }
    if st.rawtherapee.looking.as_ref() == Some(&dir) {
        return;
    }
    st.rawtherapee.looking = Some(dir.clone());
    let file = st.window.settings_file.clone();
    let weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard rawtherapee look".into())
        .spawn(move || {
            let answered = !force && file.as_deref().is_some_and(|f| answered_in(f, &dir));
            let waiting = (!answered).then(|| rt::waiting_in(&dir));
            let _ = weak.upgrade_in_event_loop(move |app| {
                let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                    return;
                };
                looked(&mut state.borrow_mut(), &app, dir, waiting);
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("rawtherapee: no thread to look at the folder on: {e}");
        st.rawtherapee.looking = None;
    }
}

/// A folder's look landed: answered already in the settings (`None`),
/// or this many of its frames waiting, offered when it is still open.
pub(crate) fn looked(st: &mut State, app: &App, dir: PathBuf, waiting: Option<usize>) {
    if st.rawtherapee.looking.as_ref() == Some(&dir) {
        st.rawtherapee.looking = None;
    }
    let Some(n) = waiting else {
        st.rawtherapee.answered.insert(dir);
        return;
    };
    st.rawtherapee.looked.insert(dir.clone(), n);
    if n > 0 {
        tracing::info!(
            "rawtherapee: {n} frames with a profile beside them waiting in {}",
            dir.display()
        );
    }
    if st.views.recent.open.as_ref() == Some(&dir) {
        offer(st, app, &dir, n);
    }
}

/// The bar up for `dir`, when any of its frames wait and it has not
/// been answered.
fn offer(st: &mut State, app: &App, dir: &Path, n: usize) {
    if n == 0 || (!st.rawtherapee.force_offer && st.rawtherapee.answered.contains(dir)) {
        return;
    }
    st.rawtherapee.offered = Some(dir.to_path_buf());
    app.set_rt_offer(offer_words(n).into());
}

/// The offer answered with Not now: the bar goes, and the folder is
/// not offered again ([`remember`]).
pub(crate) fn answer(st: &mut State, app: &App) -> Option<PathBuf> {
    let dir = st.rawtherapee.offered.clone()?;
    remember(st, app, &dir);
    Some(dir)
}

/// `dir` answered, by Not now or by a write that brought a frame in: its
/// bar goes if it is up, and it is not offered again, this run or the
/// next. A batch run leaves the user's settings alone; a run with no
/// settings file keeps it in the window.
pub(crate) fn remember(st: &mut State, app: &App, dir: &Path) {
    if st.rawtherapee.offered.as_deref() == Some(dir) {
        st.rawtherapee.offered = None;
        app.set_rt_offer("".into());
    }
    let dir = dir.to_path_buf();
    st.rawtherapee.answered.insert(dir.clone());
    if let Some(file) = st.window.settings_file.clone().filter(|_| !st.window.batch) {
        let mut kept = settings::Settings::load_from(&file);
        let spelled = dir.to_string_lossy().into_owned();
        if !kept.rawtherapee_answered.contains(&spelled) {
            kept.rawtherapee_answered.push(spelled);
            kept.save_to(&file);
        }
    }
}

/// Import... on the offer: the bar put away and the sheet opened over
/// its folder, which is not answered by it. Canceled, the sheet leaves
/// the folder to be offered again when it next opens; a write that
/// brings a frame in answers it.
pub(crate) fn offer_taken(st: &mut State, app: &App) {
    let Some(dir) = st.rawtherapee.offered.take() else {
        return;
    };
    app.set_rt_offer("".into());
    open(st, app, dir);
}

/// The import's options as the sheet has them.
fn options(st: &State, app: &App, write: bool) -> rt::Options {
    rt::Options {
        write,
        force: app.get_rt_force(),
        sidecars_in_folder: Some(st.window.placement == greycard_edit::Placement::Folder),
    }
}

/// The sheet opened over `dir`, its dry run asked for.
pub(crate) fn open(st: &mut State, app: &App, dir: PathBuf) {
    if st.rawtherapee.thread.is_some() {
        return;
    }
    st.rawtherapee.folder = Some(dir.clone());
    st.rawtherapee.report = None;
    app.set_rt_title("Import RawTherapee profiles".into());
    app.set_rt_subtitle(tilde(&dir).into());
    app.set_rt_note("".into());
    app.set_rt_force(false);
    app.set_rt_step(0);
    app.set_rt_open(true);
    show(st, app);
    dry_run(st, app);
}

/// The dry run the sheet stands for, on a thread.
pub(crate) fn dry_run(st: &mut State, app: &App) {
    let Some(dir) = st.rawtherapee.folder.clone() else {
        return;
    };
    let opts = options(st, app, false);
    st.rawtherapee.generation += 1;
    st.rawtherapee.reading = true;
    let generation = st.rawtherapee.generation;
    app.set_rt_busy(true);
    app.set_rt_busy_text("reading the profiles...".into());
    let weak = app.as_weak();
    std::thread::spawn(move || {
        let read = rt::import(&[dir], &opts).map_err(|e| e.to_string());
        let _ = weak.upgrade_in_event_loop(move |app| {
            let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                return;
            };
            landed(&state, &app, generation, read);
        });
    });
}

/// A dry run's answer, on the window's thread. One asked for before
/// the last says nothing.
pub(crate) fn landed(
    state: &Rc<RefCell<State>>,
    app: &App,
    generation: u64,
    read: std::result::Result<Report, String>,
) {
    let then = {
        let mut st = state.borrow_mut();
        if generation != st.rawtherapee.generation {
            return;
        }
        st.rawtherapee.reading = false;
        app.set_rt_busy(false);
        match read {
            Ok(report) => {
                app.set_rt_note("".into());
                st.rawtherapee.report = Some(report);
            }
            Err(e) => {
                tracing::warn!("rawtherapee import: {e}");
                app.set_rt_note(e.into());
                st.rawtherapee.report = None;
            }
        }
        show(&st, app);
        std::mem::take(&mut st.rawtherapee.then)
    };
    if then == Then::Write {
        write(state, app);
    }
}

/// The report's lines as the sheet holds them.
fn report_lines(lines: Vec<(String, String)>) -> Vec<ReportLine> {
    lines
        .into_iter()
        .map(|(label, text)| ReportLine {
            label: label.into(),
            text: text.into(),
        })
        .collect()
}

/// The sheet as the state has it: the totals over the frames' lines,
/// which scroll.
pub(crate) fn show(st: &State, app: &App) {
    let report = st.rawtherapee.report.as_ref();
    let totals = report.map(|r| r.totals(Voice::Sheet)).unwrap_or_default();
    // The first few hundred frames: the sheet lays out every line it
    // holds at once, and the totals count the rest.
    let lines = report
        .map(|r| r.frame_lines_upto(Voice::Sheet, SHOWN_FRAMES))
        .unwrap_or_default();
    app.set_rt_totals(ModelRc::new(VecModel::from(report_lines(totals))));
    app.set_rt_lines(ModelRc::new(VecModel::from(report_lines(lines))));
    // The switch shows while any frame waits for it, and stays once
    // on, so it can be turned off again.
    let held = report.map_or(0, |r| {
        r.held() + r.profiled.iter().filter(|f| f.forced).count()
    });
    app.set_rt_held(held as i32);
    app.set_rt_ready(
        st.window.write_sidecars && report.is_some_and(|r| !r.write && r.to_write() > 0),
    );
    if !st.window.write_sidecars && app.get_rt_note().is_empty() {
        app.set_rt_note(NO_SIDECARS.into());
    }
    if let (Some(r), Some(dir)) = (report, st.rawtherapee.folder.as_deref()) {
        app.set_rt_subtitle(
            format!(
                "{}; {} {}, {} with a profile",
                tilde(dir),
                r.frames,
                if r.frames == 1 { "frame" } else { "frames" },
                r.profiled.len()
            )
            .into(),
        );
    }
}

/// Write: the import runs on its thread, its progress said as it goes;
/// the sheet stays up with its buttons waiting, and shows the report
/// when done.
pub(crate) fn write(state: &Rc<RefCell<State>>, app: &App) {
    let mut st = state.borrow_mut();
    if st.rawtherapee.thread.is_some() {
        return;
    }
    let Some(dir) = st.rawtherapee.folder.clone() else {
        return;
    };
    if !st.window.write_sidecars {
        app.set_rt_note(NO_SIDECARS.into());
        return;
    }
    let opts = write_options(&mut st, app);
    app.set_rt_busy(true);
    app.set_rt_writing(true);
    app.set_rt_busy_text("writing...".into());
    app.set_status("importing the RawTherapee profiles...".into());
    let weak = app.as_weak();
    st.rawtherapee.thread = Some(std::thread::spawn(move || {
        let told = weak.clone();
        let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
        let report = rt::import_with(&[dir], &opts, &mut |p| {
            // A tenth of a second apart at most: the window is not
            // asked to redraw for every frame.
            let now = std::time::Instant::now();
            if p.done < p.of && now - last < std::time::Duration::from_millis(100) {
                return;
            }
            last = now;
            let line = progress_line(p);
            let _ = told.upgrade_in_event_loop(move |app| {
                if app.get_rt_busy() {
                    app.set_rt_busy_text(line.as_str().into());
                    app.set_status(line.into());
                }
            });
        })
        .map_err(|e| e.to_string());
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

/// The options a write runs under, the window made ready for it: the
/// frame on screen, its panel moved and not yet saved, is saved first,
/// so the import reads what the window holds. The frame then has an
/// edit of its own, and the import leaves it alone unless told to
/// write over it. A panel at rest is the sidecar already, and nothing
/// is written.
pub(crate) fn write_options(st: &mut State, app: &App) -> rt::Options {
    if st.current.is_some() && st.culling.cull.is_none() && st.develop.save_timer.running() {
        st.develop.save_timer.stop();
        let outgoing = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
        crate::panel::edit::save_edit(st, outgoing);
    }
    options(st, app, true)
}

/// The frames a write gave sidecars to, as the import found them and as
/// the index keys them: what the window's list may hold them as.
pub(crate) fn keys_of(report: &std::result::Result<Report, String>) -> Vec<PathBuf> {
    let Ok(report) = report else {
        return Vec::new();
    };
    report
        .written()
        .iter()
        .flat_map(|p| [p.clone(), greycard_library::key_path(p)])
        .collect()
}

/// The words for how far the write has come.
pub(crate) fn progress_line(p: Progress) -> String {
    format!(
        "importing the RawTherapee profiles: {} of {} frames",
        p.done, p.of
    )
}

/// The write done, however it went: the line said, every frame written
/// that the window holds read again from the disk, the frame on screen
/// opened again on what was written, and the sheet on its last step.
pub(crate) fn wrote(
    state: &Rc<RefCell<State>>,
    app: &App,
    report: std::result::Result<Report, String>,
    keys: Vec<PathBuf>,
) {
    app.set_rt_busy(false);
    app.set_rt_writing(false);
    let line = {
        let mut st = state.borrow_mut();
        st.rawtherapee.thread = None;
        match report {
            Ok(report) => {
                let written = report.written().len();
                let left = report.held();
                let failed = report.failed();
                let mut line = format!(
                    "imported the RawTherapee profiles: {written} {} written",
                    if written == 1 { "frame" } else { "frames" }
                );
                if left > 0 {
                    line.push_str(&format!(", {left} left alone"));
                }
                if failed > 0 {
                    line.push_str(&format!(", {failed} would not write"));
                }
                app.set_rt_note("".into());
                // A frame brought in answers the folder: it has come in,
                // and is not offered again.
                if written > 0
                    && let Some(dir) = st.rawtherapee.folder.clone()
                {
                    st.rawtherapee.looked.insert(dir.clone(), 0);
                    remember(&mut st, app, &dir);
                }
                st.rawtherapee.report = Some(report);
                line
            }
            Err(e) => {
                tracing::warn!("rawtherapee import: {e}");
                app.set_rt_note(format!("Not imported: {e}").into());
                format!("the RawTherapee profiles were not imported: {e}")
            }
        }
    };
    crate::panel::lightroom::reread_written(state, app, keys, "rawtherapee import");
    app.set_status(line.into());
    {
        let st = state.borrow();
        show(&st, app);
    }
    app.set_rt_step(1);
    app.set_rt_open(true);
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>) {
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_rt_offer_import(move || {
            let app = weak.unwrap();
            offer_taken(&mut state.borrow_mut(), &app);
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_rt_offer_dismissed(move || {
            let app = weak.unwrap();
            if answer(&mut state.borrow_mut(), &app).is_some() {
                app.set_status(
                    "the profiles are not offered again for this folder; \
                     greycard rawtherapee imports them from the command line"
                        .into(),
                );
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_rt_force_toggled(move || {
            let app = weak.unwrap();
            dry_run(&mut state.borrow_mut(), &app);
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_rt_write(move || {
            let app = weak.unwrap();
            write(&state, &app);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A profile of RawTherapee's shape: the meta, an exposure, a white
    /// balance, and a tool with no place here.
    const PROFILE: &str = "\
[Version]
AppVersion=5.11
Version=351

[General]
Rank=3
ColorLabel=2
InTrash=false

[Exposure]
Compensation=1.25

[White Balance]
Setting=Custom
Temperature=5800
Green=1

[Local Contrast]
Enabled=true
";

    /// Two small TIFFs the window opens, a profile beside each.
    fn folder(dir: &Path) -> PathBuf {
        use greycard_library::fixture::{R5, R6, write_frame};
        let shots = dir.join("Shots");
        std::fs::create_dir_all(&shots).unwrap();
        write_frame(&shots.join("a.tif"), &R5, 1);
        write_frame(&shots.join("b.tif"), &R6, 2);
        std::fs::write(shots.join("a.tif.pp3"), PROFILE).unwrap();
        std::fs::write(shots.join("b.pp3"), PROFILE.replace("Rank=3", "Rank=5")).unwrap();
        shots
    }

    /// The dry run the sheet asked for, run here and landed as its
    /// thread would land it.
    fn land_now(state: &Rc<RefCell<State>>, app: &App) {
        let (dir, opts, generation) = {
            let st = state.borrow();
            (
                st.rawtherapee.folder.clone().unwrap(),
                options(&st, app, false),
                st.rawtherapee.generation,
            )
        };
        let read = rt::import(&[dir], &opts).map_err(|e| e.to_string());
        landed(state, app, generation, read);
    }

    /// The write as Write runs it, landed.
    fn write_now(state: &Rc<RefCell<State>>, app: &App) -> Vec<String> {
        let (dir, opts) = {
            let mut st = state.borrow_mut();
            let opts = write_options(&mut st, app);
            (st.rawtherapee.folder.clone().unwrap(), opts)
        };
        let mut seen = Vec::new();
        let report = rt::import_with(&[dir], &opts, &mut |p| seen.push(progress_line(p)))
            .map_err(|e| e.to_string());
        let keys = keys_of(&report);
        wrote(state, app, report, keys);
        seen
    }

    fn texts(model: ModelRc<ReportLine>) -> Vec<(String, String)> {
        model
            .iter()
            .map(|l| (l.label.to_string(), l.text.to_string()))
            .collect()
    }

    #[test]
    fn the_offer_is_answered_once_and_the_settings_keep_it() {
        let dir = crate::testing::scratch_dir("rawtherapee-offer");
        let shots = folder(&dir);
        let settings_file = dir.join("settings.json");
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        {
            let mut st = state.borrow_mut();
            st.window.settings_file = Some(settings_file.clone());
            st.views.recent.open = Some(shots.clone());
        }
        // The look, as its thread does it: two frames waiting.
        assert!(!answered_in(&settings_file, &shots));
        let waiting = rt::waiting_in(&shots);
        assert_eq!(waiting, 2);
        state.borrow_mut().rawtherapee.looking = Some(shots.clone());
        looked(&mut state.borrow_mut(), &app, shots.clone(), Some(waiting));
        assert!(!state.borrow().rawtherapee.busy());
        assert_eq!(
            app.get_rt_offer(),
            "RawTherapee profiles found beside 2 frames"
        );
        // Opened again this run: offered from what the look found, not
        // looked at again.
        folder_opened(&mut state.borrow_mut(), &app);
        assert!(state.borrow().rawtherapee.looking.is_none());
        assert_eq!(
            app.get_rt_offer(),
            "RawTherapee profiles found beside 2 frames"
        );
        // Another folder open: the bar goes with its folder.
        state.borrow_mut().views.recent.open = Some(dir.clone());
        state.borrow_mut().rawtherapee.looked.insert(dir.clone(), 0);
        folder_opened(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_rt_offer(), "");
        state.borrow_mut().views.recent.open = Some(shots.clone());
        folder_opened(&mut state.borrow_mut(), &app);
        assert_ne!(app.get_rt_offer(), "");

        // Not now: the bar goes, and the settings file has the folder.
        app.invoke_rt_offer_dismissed();
        assert_eq!(app.get_rt_offer(), "");
        assert!(state.borrow().rawtherapee.answered.contains(&shots));
        assert!(answered_in(&settings_file, &shots));
        let kept = settings::Settings::load_from(&settings_file);
        assert_eq!(
            kept.rawtherapee_answered,
            vec![shots.to_string_lossy().into_owned()]
        );
        // Opened again, and in the next run, it is not offered: this
        // run by the window's memory, the next by the settings', which
        // its look reads and lands as answered.
        folder_opened(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_rt_offer(), "");
        let app2 = crate::testing::window(0);
        let (next, _w) = crate::testing::state_for(&app2, Vec::new());
        next.borrow_mut().window.settings_file = Some(settings_file.clone());
        next.borrow_mut().views.recent.open = Some(shots.clone());
        let waiting = (!answered_in(&settings_file, &shots)).then(|| rt::waiting_in(&shots));
        assert_eq!(waiting, None);
        looked(&mut next.borrow_mut(), &app2, shots.clone(), waiting);
        assert_eq!(app2.get_rt_offer(), "");
        assert!(next.borrow().rawtherapee.answered.contains(&shots));
        // A run with no settings file to write keeps the answer in the
        // window alone.
        let app3 = crate::testing::window(0);
        let (third, _w) = crate::testing::state_for(&app3, Vec::new());
        {
            let mut st = third.borrow_mut();
            st.window.write_sidecars = true;
            st.views.recent.open = Some(shots.clone());
        }
        assert!(third.borrow().window.settings_file.is_none());
        looked(&mut third.borrow_mut(), &app3, shots.clone(), Some(2));
        assert_ne!(app3.get_rt_offer(), "");
        app3.invoke_rt_offer_dismissed();
        assert!(third.borrow().rawtherapee.answered.contains(&shots));
        folder_opened(&mut third.borrow_mut(), &app3);
        assert_eq!(app3.get_rt_offer(), "");
        assert!(third.borrow().rawtherapee.looking.is_none());
        // And a batch run never offers: no look, no line.
        let app4 = crate::testing::window(0);
        let (fourth, _w) = crate::testing::state_for(&app4, Vec::new());
        {
            let mut st = fourth.borrow_mut();
            st.window.write_sidecars = true;
            st.window.batch = true;
            st.views.recent.open = Some(shots.clone());
        }
        folder_opened(&mut fourth.borrow_mut(), &app4);
        assert!(fourth.borrow().rawtherapee.looking.is_none());
        assert_eq!(app4.get_rt_offer(), "");
        crate::testing::remove_scratch(state, &dir);
    }

    /// A run told to write no sidecars offers nothing, and its sheet
    /// shows the report with Write off and says why.
    #[test]
    fn with_no_sidecars_there_is_no_offer_and_write_is_off() {
        let dir = crate::testing::scratch_dir("rawtherapee-no-sidecars");
        let shots = folder(&dir);
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = false;
            st.views.recent.open = Some(shots.clone());
        }
        folder_opened(&mut state.borrow_mut(), &app);
        assert!(state.borrow().rawtherapee.looking.is_none(), "no look");
        assert_eq!(app.get_rt_offer(), "");
        open(&mut state.borrow_mut(), &app, shots.clone());
        land_now(&state, &app);
        assert!(!app.get_rt_ready());
        assert_eq!(app.get_rt_note(), NO_SIDECARS);
        assert!(texts(app.get_rt_lines()).iter().any(|(l, _)| l == "frame"));
        write(&state, &app);
        assert!(state.borrow().rawtherapee.thread.is_none());
        assert!(!app.get_rt_writing());
        assert!(greycard_edit::Sidecar::find(&shots.join("a.tif")).is_none());
        crate::testing::remove_scratch(state, &dir);
    }

    /// A folder under a root that is offline is not looked at, and not
    /// counted as looked at: once the root is back, it is.
    #[test]
    fn a_folder_under_an_offline_root_is_not_looked_at() {
        let dir = crate::testing::scratch_dir("rawtherapee-offline");
        let shots = folder(&dir);
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.views.library.offline.insert(dir.clone());
            st.views.recent.open = Some(shots.clone());
        }
        folder_opened(&mut state.borrow_mut(), &app);
        assert!(state.borrow().rawtherapee.looking.is_none());
        assert!(!state.borrow().rawtherapee.looked.contains_key(&shots));
        assert!(!state.borrow().rawtherapee.busy());
        // Back online: looked at.
        state.borrow_mut().views.library.offline.clear();
        folder_opened(&mut state.borrow_mut(), &app);
        assert_eq!(state.borrow().rawtherapee.looking.as_ref(), Some(&shots));
        crate::testing::remove_scratch(state, &dir);
    }

    #[test]
    fn import_opens_the_sheet_on_the_command_s_report_and_writes_it() {
        let dir = crate::testing::scratch_dir("rawtherapee-sheet");
        let shots = folder(&dir);
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        state.borrow_mut().views.recent.open = Some(shots.clone());
        looked(&mut state.borrow_mut(), &app, shots.clone(), Some(2));
        // Import... opens the sheet on its dry run; it answers nothing
        // yet.
        app.invoke_rt_offer_import();
        assert_eq!(app.get_rt_offer(), "");
        assert!(!state.borrow().rawtherapee.answered.contains(&shots));
        assert!(app.get_rt_open());
        assert_eq!(app.get_rt_step(), 0);
        assert!(app.get_rt_busy());
        // A dry run reading leaves Cancel and Escape free: only a write
        // holds the sheet.
        assert!(!app.get_rt_writing());
        land_now(&state, &app);
        assert!(!app.get_rt_busy());
        assert!(app.get_rt_ready());
        assert_eq!(app.get_rt_held(), 0, "nothing waits for the switch");

        // The sheet's lines are the command's for the same options, in
        // the sheet's voice: the totals over the frames.
        let command = rt::import(
            std::slice::from_ref(&shots),
            &rt::Options {
                write: false,
                force: false,
                sidecars_in_folder: Some(false),
            },
        )
        .unwrap();
        let totals = texts(app.get_rt_totals());
        let lines = texts(app.get_rt_lines());
        assert_eq!(totals, command.totals(Voice::Sheet));
        assert_eq!(lines, command.frame_lines(Voice::Sheet));
        let mut all = lines.clone();
        all.extend(totals.clone());
        assert_eq!(all, command.lines(Voice::Sheet));
        assert!(
            totals.contains(&("frames".into(), "2 found, 2 with a profile".into())),
            "{totals:?}"
        );
        assert!(
            totals.contains(&("no place".into(), "Local Contrast (2)".into())),
            "{totals:?}"
        );
        assert!(
            lines.contains(&("frame".into(), "a.tif".into())),
            "{lines:?}"
        );
        assert!(!all.iter().any(|(_, t)| t.contains("--")), "{all:?}");
        // The command's own says its flags where the sheet does not.
        assert!(
            command
                .lines(Voice::Command)
                .iter()
                .any(|(_, t)| t.contains("--write"))
        );

        // A dry run that answers after a later one was asked says
        // nothing.
        let stale = state.borrow().rawtherapee.generation;
        state.borrow_mut().rawtherapee.generation += 1;
        landed(&state, &app, stale, Err("late".into()));
        assert_eq!(app.get_rt_note(), "");

        // Written: the step, the line, both frames.
        let seen = write_now(&state, &app);
        assert!(
            seen.iter()
                .any(|l| l == "importing the RawTherapee profiles: 2 of 2 frames"),
            "{seen:?}"
        );
        assert_eq!(app.get_rt_step(), 1);
        assert!(!app.get_rt_busy());
        assert!(!app.get_rt_ready());
        // Frames brought in answer the folder.
        assert!(state.borrow().rawtherapee.answered.contains(&shots));
        folder_opened(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_rt_offer(), "");
        assert_eq!(
            app.get_status(),
            "imported the RawTherapee profiles: 2 frames written"
        );
        let totals = texts(app.get_rt_totals());
        assert!(
            totals.contains(&("edits".into(), "2 carried, as a state of their own".into())),
            "{totals:?}"
        );
        for (f, stars) in [("a.tif", 3), ("b.tif", 5)] {
            let s = greycard_edit::Sidecar::load(&shots.join(f))
                .unwrap()
                .unwrap();
            assert_eq!(s.meta.rating, stars);
            assert_eq!(s.current.light.exposure, 1.25);
        }

        // Run again: both held back, and the switch shows for them;
        // turned on, the dry run says what a forced write does.
        open(&mut state.borrow_mut(), &app, shots.clone());
        land_now(&state, &app);
        assert_eq!(app.get_rt_held(), 2);
        assert!(!app.get_rt_ready(), "nothing to write unforced");
        app.set_rt_force(true);
        app.invoke_rt_force_toggled();
        land_now(&state, &app);
        assert_eq!(app.get_rt_held(), 2, "the switch stays to be turned off");
        let lines = texts(app.get_rt_lines());
        assert!(lines.iter().any(|(l, _)| l == "  forced"), "{lines:?}");
        crate::testing::remove_scratch(state, &dir);
    }

    /// Import... then Cancel answers nothing: the folder opened again is
    /// offered again, and the settings have not taken it.
    #[test]
    fn a_sheet_canceled_leaves_the_offer_for_the_next_open() {
        let dir = crate::testing::scratch_dir("rawtherapee-cancel");
        let shots = folder(&dir);
        let settings_file = dir.join("settings.json");
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        {
            let mut st = state.borrow_mut();
            st.window.settings_file = Some(settings_file.clone());
            st.views.recent.open = Some(shots.clone());
        }
        looked(&mut state.borrow_mut(), &app, shots.clone(), Some(2));
        assert_ne!(app.get_rt_offer(), "");
        app.invoke_rt_offer_import();
        assert!(app.get_rt_open());
        land_now(&state, &app);
        // Canceled, as the sheet's Cancel or Escape closes it.
        app.set_rt_open(false);
        assert!(!state.borrow().rawtherapee.answered.contains(&shots));
        assert!(!answered_in(&settings_file, &shots));
        // Another folder, then this one again: the offer is back.
        state.borrow_mut().views.recent.open = Some(dir.clone());
        state.borrow_mut().rawtherapee.looked.insert(dir.clone(), 0);
        folder_opened(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_rt_offer(), "");
        state.borrow_mut().views.recent.open = Some(shots.clone());
        folder_opened(&mut state.borrow_mut(), &app);
        assert_eq!(
            app.get_rt_offer(),
            "RawTherapee profiles found beside 2 frames"
        );
        // And a write that brings nothing in answers nothing either: both
        // frames given a history first, so both are held back.
        rt::import(
            std::slice::from_ref(&shots),
            &rt::Options {
                write: true,
                force: false,
                sidecars_in_folder: None,
            },
        )
        .unwrap();
        app.invoke_rt_offer_import();
        land_now(&state, &app);
        let report = rt::import(
            std::slice::from_ref(&shots),
            &rt::Options {
                write: true,
                force: false,
                sidecars_in_folder: None,
            },
        )
        .map_err(|e| e.to_string());
        let keys = keys_of(&report);
        wrote(&state, &app, report, keys);
        assert!(!state.borrow().rawtherapee.answered.contains(&shots));
        assert!(!answered_in(&settings_file, &shots));
        crate::testing::remove_scratch(state, &dir);
    }

    /// A folder of more frames than the sheet lists: the first ones, a
    /// line for the rest, and the totals over all of them.
    #[test]
    fn the_sheet_lists_the_first_frames_and_says_how_many_more() {
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        let frame = |i: usize| rt::FrameReport {
            file: PathBuf::from(format!("/shots/IMG_{i:04}.CR3")),
            profile: PathBuf::from(format!("/shots/IMG_{i:04}.CR3.pp3")),
            source: "RawTherapee 5.11, profile version 351".into(),
            said: vec![("exact", "3 stars".into())],
            skipped: Vec::new(),
            here: rt::Here::Nothing,
            forced: false,
            replaced: Vec::new(),
            meta: true,
            edit: false,
            versions: 0,
            fate: rt::Fate::Would,
        };
        let report = Report {
            frames: SHOWN_FRAMES + 50,
            passed: Vec::new(),
            profiled: (0..SHOWN_FRAMES + 50).map(frame).collect(),
            write: false,
        };
        {
            let mut st = state.borrow_mut();
            st.rawtherapee.folder = Some(PathBuf::from("/shots"));
            st.rawtherapee.report = Some(report.clone());
            show(&st, &app);
        }
        let lines = texts(app.get_rt_lines());
        let per_frame = frame(0).lines(Voice::Sheet).len();
        assert_eq!(lines.len(), SHOWN_FRAMES * per_frame + 1);
        assert_eq!(
            lines.last().unwrap(),
            &(
                "and more".to_string(),
                "50 more frames with a profile, not listed here; the totals count them".to_string()
            )
        );
        let named: Vec<&String> = lines
            .iter()
            .filter(|(l, _)| l == "frame")
            .map(|(_, t)| t)
            .collect();
        assert_eq!(named.len(), SHOWN_FRAMES);
        assert_eq!(named[0], "IMG_0000.CR3");
        // The totals count every frame.
        let totals = texts(app.get_rt_totals());
        assert!(
            totals.contains(&(
                "meta".into(),
                format!("{} would be carried exactly", SHOWN_FRAMES + 50)
            )),
            "{totals:?}"
        );
        // The command, which never caps, lists all of them.
        assert_eq!(
            report.frame_lines(Voice::Command).len(),
            (SHOWN_FRAMES + 50) * per_frame
        );
    }

    /// The frame open in the window when the write lands is opened again
    /// on what was written: a key pressed after it writes the import's
    /// edit with the rating, never the panel from before the write.
    #[test]
    fn the_frame_on_screen_takes_the_write_and_a_key_after_it_keeps_it() {
        let dir = crate::testing::scratch_dir("rawtherapee-on-screen");
        let shots = folder(&dir);
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
        open(&mut state.borrow_mut(), &app, shots.clone());
        land_now(&state, &app);
        write_now(&state, &app);
        let written = greycard_edit::Sidecar::load(&a).unwrap().unwrap();
        assert!(!written.history.is_empty(), "the import's state");
        assert_eq!(written.current.light.exposure, 1.25);
        // The panel is the written edit's.
        assert_eq!(app.get_exposure(), 1.25);
        // A rating after it: the import's edit kept, the rating with it.
        assert_eq!(written.meta.rating, 3);
        assert!(app.invoke_meta_key("4".into()));
        let after = greycard_edit::Sidecar::load(&a).unwrap().unwrap();
        assert_eq!(after.meta.rating, 4);
        assert_eq!(after.meta.label, written.meta.label);
        assert_eq!(after.history.len(), written.history.len());
        assert_eq!(after.current.light.exposure, 1.25);
        assert_eq!(after.current.white_balance, written.current.white_balance);
        WORKER.with(|w| *w.borrow_mut() = None);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A panel moved and waiting to be saved when Write is pressed is
    /// saved first, and the frame, an edit of its own now, is left alone.
    #[test]
    fn a_panel_not_yet_saved_is_saved_and_its_frame_left_alone() {
        let dir = crate::testing::scratch_dir("rawtherapee-unsaved");
        let shots = folder(&dir);
        let app = crate::testing::window(0);
        let (state, worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        crate::panel::browser::open_folder(&state, &app, &worker, &shots);
        crate::roots::land_sent(&state, &app, &worker);
        app.invoke_select(0);
        open(&mut state.borrow_mut(), &app, shots.clone());
        land_now(&state, &app);
        app.set_exposure(0.8);
        state.borrow().develop.save_timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_secs(60),
            || {},
        );
        write_now(&state, &app);
        assert!(!state.borrow().develop.save_timer.running());
        let a = greycard_edit::Sidecar::load(&shots.join("a.tif"))
            .unwrap()
            .unwrap();
        assert_eq!(a.current.light.exposure, 0.8);
        let report = state.borrow().rawtherapee.report.clone().unwrap();
        assert_eq!(report.held(), 1, "the frame with the user's edit");
        assert_eq!(report.written(), [shots.join("b.tif")]);
        assert_eq!(
            app.get_status(),
            "imported the RawTherapee profiles: 1 frame written, 1 left alone"
        );
        crate::testing::remove_scratch(state, &dir);
    }
}
