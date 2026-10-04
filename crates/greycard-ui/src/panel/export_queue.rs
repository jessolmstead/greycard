//! The export queue in the window: Add to queue on the export sheet,
//! the count at the sheet's foot, Export the queue and Clear.
//!
//! An entry is what Export would have sent at the moment it was queued
//! (the frames, each frame's edit, the sheet, the preset in use and the
//! folder), and running one is a set's export: `start_set_after` and the
//! worker's frames, with the status plate's Stop as its cancel. The
//! entries run in order, one set after the other; each frame done with
//! leaves its entry, and an entry with no frames left leaves the queue.

use crate::export_queue::{self, Entry, QueuedFrame};
use crate::panel::browser::{chosen_frames, file_name};
use crate::panel::deliver::{SetFrames, preset_in_use, pressed_frames, read_sheet};
use crate::queue;
use crate::*;

/// An entry of the queue on its way through the worker, while the
/// queue runs.
pub(crate) struct Run {
    /// The entry, by its place in the queue.
    pub(crate) entry: usize,
    /// The set its frames were sent as; none while its files are still
    /// being looked for.
    pub(crate) set: Option<Arc<queue::Set>>,
    /// For each frame of the set, by its place in the set, its place in
    /// the entry (a frame whose file was not there was not sent).
    pub(crate) at: Vec<usize>,
    /// The entry's frames done with, by their place in it: written,
    /// skipped or failed. The rest stay queued.
    pub(crate) done: Vec<bool>,
    /// The frames of this run's earlier entries, and of this one, that
    /// were not there to send and were left queued.
    pub(crate) left_behind: usize,
    /// Which look for the files this is, so a late answer to one the
    /// queue has since let go of is not taken for the current one.
    pub(crate) probe: u64,
    /// The names the set's frames were sent under, for a test to hand
    /// back as the worker would.
    #[cfg(test)]
    pub(crate) sent: Vec<PathBuf>,
}

impl Run {
    /// Whether `set` is this run's.
    fn is(&self, set: &Arc<queue::Set>) -> bool {
        self.set.as_ref().is_some_and(|s| Arc::ptr_eq(s, set))
    }
}

/// What the look for an entry's files found, off the window's thread:
/// a stale network mount can hang a stat for as long as it likes.
pub(crate) struct Found {
    /// Each frame's file, by its place in the entry: there or not.
    present: Vec<bool>,
    /// The folder the entry goes to is there (or it goes beside each
    /// file).
    folder: bool,
    /// The watermark's PNG is there (or the mark is not an image).
    mark: bool,
}

/// Look for `sources`, `folder` and the mark's `png`.
fn look(sources: &[PathBuf], folder: Option<&Path>, png: Option<&Path>) -> Found {
    Found {
        present: sources.iter().map(|s| s.is_file()).collect(),
        folder: folder.is_none_or(Path::is_dir),
        mark: png.is_none_or(Path::is_file),
    }
}

/// The watermark's PNG an entry needs, if its mark is an image.
fn mark_png(entry: &Entry) -> Option<PathBuf> {
    match entry.sheet.settings().watermark.map(|m| m.kind) {
        Some(crate::watermark::Kind::Image { path }) => Some(path),
        _ => None,
    }
}

#[cfg(test)]
thread_local! {
    /// What the folder chooser answers in a test, in place of asking:
    /// `Some(None)` for each beside its own file.
    pub(crate) static CHOSEN: RefCell<Option<Option<PathBuf>>> = const { RefCell::new(None) };
}

/// The count at the sheet's foot.
pub(crate) fn show_queue(st: &State, app: &App) {
    app.set_export_queue_sets(st.export_queue.len() as i32);
    app.set_export_queue_line(export_queue::count_line(&st.export_queue).into());
    app.set_export_queue_running(st.queue_run.as_ref().is_some_and(|r| r.set.is_some()));
}

/// Write the queue now, as it is: every change is kept at once. While
/// an entry runs, the frames of it already done with are left out of
/// the file (not out of the entry, which the run still counts by
/// place), so a window closed or a crash mid-entry does not send them
/// again at the next run.
fn keep(st: &State) {
    let Some(path) = &st.export_queue_file else {
        return;
    };
    match &st.queue_run {
        Some(run) if run.done.iter().any(|d| *d) => {
            let mut queue = st.export_queue.clone();
            if let Some(entry) = queue.get_mut(run.entry) {
                let mut done = run.done.iter();
                entry
                    .frames
                    .retain(|_| !done.next().copied().unwrap_or(false));
                if entry.frames.is_empty() {
                    queue.remove(run.entry);
                }
            }
            export_queue::save(path, &queue);
        }
        _ => export_queue::save(path, &st.export_queue),
    }
}

/// The words for frames left queued because their files were not
/// there: "; 2 frames not there, left queued", or nothing.
fn left_behind(n: usize) -> String {
    match n {
        0 => String::new(),
        n => format!("; {} not there, left queued", queue::frames(n)),
    }
}

/// The queue the last session left, read and shown at launch, never
/// run: `settings` is the settings file the queue sits beside, and
/// `write` whether this run may change it (not a snapshot or a batch
/// run).
pub(crate) fn open_at_launch(st: &mut State, app: &App, settings: Option<&Path>, write: bool) {
    let path = settings.map(export_queue::path_beside);
    st.export_queue = path.as_deref().map(export_queue::load).unwrap_or_default();
    st.export_queue_file = path.filter(|_| write);
    if !st.export_queue.is_empty() {
        tracing::info!(
            "export queue: {}, waiting",
            export_queue::count_line(&st.export_queue)
        );
    }
    show_queue(st, app);
}

/// Add to queue, the set's sidecars in hand: what Export would take
/// from `pressed` and `frames` now, into a folder the desktop's chooser
/// names, appended to the queue. Nothing is exported.
fn queue_pressed(state: &Rc<RefCell<State>>, app: &App, pressed: &Path, frames: &[usize]) {
    if state.borrow().export_choosing {
        return;
    }
    let Some((raw, _, frames, _)) = pressed_frames(state, app, pressed, frames) else {
        return;
    };
    let sheet = read_sheet(app);
    // A mark asked for with nothing to draw would fail every frame
    // when the queue runs: say so now and keep the sheet up.
    if let Some(Err(e)) = sheet.settings().watermark.as_ref().map(|m| m.check()) {
        app.set_status(format!("not queued: {e:#}").into());
        app.set_export_open(true);
        return;
    }
    let entry = entry_of(frames, sheet, preset_in_use(app));
    if entry.frames.is_empty() {
        app.set_status("nothing to queue".into());
        return;
    }
    #[cfg(test)]
    if let Some(folder) = CHOSEN.with(|c| c.borrow_mut().take()) {
        add(&mut state.borrow_mut(), app, Entry { folder, ..entry });
        return;
    }
    let start = raw.parent().map(Path::to_path_buf).unwrap_or_default();
    state.borrow_mut().export_choosing = true;
    app.set_status(
        format!(
            "choosing where the {} queued go...",
            queue::frames(entry.frames.len())
        )
        .into(),
    );
    let weak = app.as_weak();
    export::choose_folder("Queue an export to a folder", start, move |chosen| {
        let _ = weak.upgrade_in_event_loop(move |app| {
            let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                return;
            };
            let mut st = state.borrow_mut();
            let folder = match chosen {
                Ok(Some(folder)) => Some(folder),
                Ok(None) => {
                    st.export_choosing = false;
                    app.set_status("nothing queued".into());
                    return;
                }
                // No chooser to ask: each beside its own file when the
                // queue runs, as a set with no chooser goes.
                Err(e) => {
                    tracing::warn!("folder chooser: {e:#}; queued to go beside each file");
                    None
                }
            };
            add(&mut st, &app, Entry { folder, ..entry });
        });
    });
}

/// An entry of `frames` under `sheet`, its folder still to be named.
fn entry_of(frames: SetFrames, sheet: crate::sheet::Sheet, preset: Option<String>) -> Entry {
    Entry {
        queued: crate::panel::history::now(),
        folder: None,
        preset,
        sheet,
        frames: frames
            .into_iter()
            .map(|(source, edit, turn, seed_blend)| QueuedFrame {
                // Whole, since the queue outlives this run's working
                // directory: a folder opened by a relative path would
                // name nothing from the next launch's.
                source: std::path::absolute(&source).unwrap_or(source),
                edit,
                turn,
                seed_blend,
            })
            .collect(),
    }
}

/// Append `entry`, keep the file, and say where the queue stands.
pub(crate) fn add(st: &mut State, app: &App, entry: Entry) {
    st.export_choosing = false;
    let n = entry.frames.len();
    tracing::info!(
        "queued {} {}: {}",
        queue::frames(n),
        match &entry.folder {
            Some(f) => format!("to {}", f.display()),
            None => "beside their files".to_string(),
        },
        entry.sheet.settings().describe()
    );
    st.export_queue.push(entry);
    keep(st);
    show_queue(st, app);
    app.set_status(
        format!(
            "{} queued; {} in the queue",
            queue::frames(n),
            export_queue::sets(st.export_queue.len())
        )
        .into(),
    );
}

/// Export the queue: its entries in order, from the first.
pub(crate) fn run(st: &mut State, app: &App) {
    if st.exporting.is_some() || st.export_choosing || st.queue_run.is_some() {
        app.set_status("an export is running; the queue waits for it".into());
        return;
    }
    if st.export_queue.is_empty() {
        app.set_status("nothing queued".into());
        return;
    }
    begin_at(st, app, 0, 0);
}

/// Look for the files of entry `index` (off the window's thread) and,
/// once they are found, send it as a set; past the last entry, the
/// run is over. `left_behind` counts the frames earlier entries left
/// queued for not being there.
fn begin_at(st: &mut State, app: &App, index: usize, left_behind: usize) {
    let Some(entry) = st.export_queue.get(index) else {
        st.queue_run = None;
        show_queue(st, app);
        return;
    };
    let sources: Vec<PathBuf> = entry.frames.iter().map(|f| f.source.clone()).collect();
    let folder = entry.folder.clone();
    let png = mark_png(entry);
    st.queue_probes += 1;
    let probe = st.queue_probes;
    st.queue_run = Some(Run {
        entry: index,
        set: None,
        at: Vec::new(),
        done: vec![false; sources.len()],
        left_behind,
        probe,
        #[cfg(test)]
        sent: Vec::new(),
    });
    show_queue(st, app);
    // A test answers at once; the window asks a thread, whose answer
    // comes back through the event loop.
    if cfg!(test) {
        let found = look(&sources, folder.as_deref(), png.as_deref());
        found_files(st, app, probe, found);
        return;
    }
    app.set_status("looking for the queued files...".into());
    let weak = app.as_weak();
    std::thread::spawn(move || {
        let found = look(&sources, folder.as_deref(), png.as_deref());
        let _ = weak.upgrade_in_event_loop(move |app| {
            if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                found_files(&mut state.borrow_mut(), &app, probe, found);
            }
        });
    });
}

/// What the look for the running entry's files found: the entry sent
/// as a set of the frames that are there, the rest left queued; or the
/// run stopped, the entry whole, when its folder or its watermark's PNG
/// is not there.
fn found_files(st: &mut State, app: &App, probe: u64, found: Found) {
    let Some(run) = st
        .queue_run
        .as_ref()
        .filter(|r| r.probe == probe && r.set.is_none())
    else {
        return;
    };
    let (index, mut left) = (run.entry, run.left_behind);
    let Some(entry) = st.export_queue.get(index) else {
        st.queue_run = None;
        show_queue(st, app);
        return;
    };
    let stop = |st: &mut State, why: String| {
        st.queue_run = None;
        show_queue(st, app);
        tracing::warn!("export queue stopped: {why}");
        app.set_status(
            format!(
                "the queue stopped: {why}; {} left in the queue",
                export_queue::count_line(&st.export_queue)
            )
            .into(),
        );
    };
    // An export begun while the files were looked for has the worker.
    if st.exporting.is_some() {
        stop(st, "another export began".into());
        return;
    }
    if !found.folder {
        let folder = entry.folder.clone().unwrap_or_default();
        stop(st, format!("{} is not there", folder.display()));
        return;
    }
    if !found.mark {
        let png = mark_png(entry).unwrap_or_default();
        stop(
            st,
            format!("the watermark's PNG {} is not there", png.display()),
        );
        return;
    }
    // Sent under the window's own name for a frame in the list (the
    // list may hold it by a relative path), so the worker knows its
    // open file and the history its row.
    let listed: std::collections::HashMap<PathBuf, &PathBuf> = st
        .files
        .iter()
        .filter_map(|p| std::path::absolute(p).ok().map(|a| (a, p)))
        .collect();
    let (mut frames, mut at, mut missing) = (Vec::new(), Vec::new(), Vec::new());
    for (i, (f, there)) in entry.frames.iter().zip(&found.present).enumerate() {
        if *there {
            let source = listed
                .get(&f.source)
                .map(|p| (*p).clone())
                .unwrap_or_else(|| f.source.clone());
            frames.push((source, f.edit.clone(), f.turn, f.seed_blend));
            at.push(i);
        } else {
            // Offline or gone: kept, for a run when it is back.
            tracing::warn!(
                "export queue: {} is not there; left queued",
                f.source.to_string_lossy()
            );
            missing.push(file_name(&f.source));
        }
    }
    left += missing.len();
    if frames.is_empty() {
        app.set_status(
            format!(
                "{} not there; left queued",
                crate::panel::deliver::listed(&missing)
            )
            .into(),
        );
        begin_at(st, app, index + 1, left);
        return;
    }
    let (folder, settings, on_exists, preset) = (
        entry.folder.clone(),
        entry.sheet.settings(),
        entry.sheet.on_exists(),
        entry.preset.clone(),
    );
    let count = frames.len();
    let after = st.export_queue.len() - index - 1;
    #[cfg(test)]
    let sent: Vec<PathBuf> = frames.iter().map(|f| f.0.clone()).collect();
    let Some(set) =
        crate::panel::deliver::start_set(st, app, frames, folder, settings, on_exists, preset)
    else {
        st.queue_run = None;
        show_queue(st, app);
        return;
    };
    if let Some(run) = st.queue_run.as_mut() {
        run.set = Some(set);
        run.at = at;
        run.left_behind = left;
        #[cfg(test)]
        {
            run.sent = sent;
        }
    }
    show_queue(st, app);
    app.set_status(
        format!(
            "exporting {} from the queue, {} after it...",
            queue::frames(count),
            export_queue::sets(after)
        )
        .into(),
    );
}

/// Frame `index` of `set` came back; `ran` when it was begun, not
/// passed over by a cancel. A frame begun is done with, however it
/// went, and the file says so at once.
pub(crate) fn frame_done(st: &mut State, set: &Arc<queue::Set>, index: usize, ran: bool) {
    if let Some(run) = st.queue_run.as_mut()
        && run.is(set)
        && ran
        && let Some(&i) = run.at.get(index)
        && let Some(d) = run.done.get_mut(i)
    {
        *d = true;
        keep(st);
    }
}

/// `set` is finished, its finished line `line`: when it was the
/// queue's, what is left of its entry stays queued, and the next entry
/// goes unless the set was stopped.
pub(crate) fn set_done(st: &mut State, app: &App, set: &Arc<queue::Set>, line: &str) {
    let Some(run) = st.queue_run.take_if(|r| r.is(set)) else {
        return;
    };
    let mut next = run.entry + 1;
    if let Some(entry) = st.export_queue.get_mut(run.entry) {
        let mut done = run.done.iter();
        entry
            .frames
            .retain(|_| !done.next().copied().unwrap_or(false));
        if entry.frames.is_empty() {
            st.export_queue.remove(run.entry);
            next = run.entry;
        }
    }
    keep(st);
    show_queue(st, app);
    let behind = left_behind(run.left_behind);
    if set.is_canceled() {
        let left = match st.export_queue.len() {
            0 => "nothing left in the queue".to_string(),
            _ => format!(
                "{} left in the queue",
                export_queue::count_line(&st.export_queue)
            ),
        };
        app.set_status(format!("{line}{behind}; {left}").into());
        return;
    }
    if next >= st.export_queue.len() {
        let end = if st.export_queue.is_empty() {
            "the queue is done".to_string()
        } else {
            format!(
                "{} left in the queue",
                export_queue::count_line(&st.export_queue)
            )
        };
        app.set_status(format!("{line}{behind}; {end}").into());
        return;
    }
    begin_at(st, app, next, run.left_behind);
}

/// Clear: every entry and the file. Not while an entry is being
/// exported; while its files are still being looked for, the look is
/// let go of.
pub(crate) fn clear(st: &mut State, app: &App) {
    if st.queue_run.as_ref().is_some_and(|r| r.set.is_some()) {
        app.set_status("the queue is running; stop it first".into());
        return;
    }
    let n = st.export_queue.len();
    st.queue_run = None;
    st.export_queue.clear();
    keep(st);
    show_queue(st, app);
    app.set_status(format!("export queue cleared ({})", export_queue::sets(n)).into());
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    // Add to queue: the selection or the frame, as Export takes it.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_export_queue_add(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (pressed, paths) = {
                let st = state.borrow();
                let Some(c) = st.current else {
                    return;
                };
                let paths: Vec<PathBuf> = chosen_frames(&st)
                    .into_iter()
                    .map(|i| st.files[i].clone())
                    .collect();
                (st.files[c].clone(), paths)
            };
            crate::rows::request(
                &state,
                &app,
                &worker,
                paths,
                "queue",
                Box::new(move |state, app, _, at| {
                    let frames: Vec<usize> = at.into_iter().flatten().collect();
                    queue_pressed(state, app, &pressed, &frames);
                }),
            );
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_export_queue_run(move || {
            if let Some(app) = app_weak.upgrade() {
                run(&mut state.borrow_mut(), &app);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_export_queue_clear(move || {
            if let Some(app) = app_weak.upgrade() {
                clear(&mut state.borrow_mut(), &app);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::{ExportPreset, Sheet};
    use crate::worker::Outcome;

    /// A folder of `n` frames that are there on disk (empty files: the
    /// queue asks only that they exist).
    fn shoot(tag: &str, n: usize) -> (PathBuf, Vec<PathBuf>) {
        let dir = std::env::temp_dir().join(format!("greycard-queue-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<PathBuf> = (0..n)
            .map(|i| {
                let f = dir.join(format!("IMG_{i:04}.CR3"));
                std::fs::write(&f, b"").unwrap();
                f
            })
            .collect();
        (dir, files)
    }

    fn queue_file(dir: &Path) -> PathBuf {
        dir.join("config").join(export_queue::FILE)
    }

    /// An entry of `frames` (by row) under each frame's sidecar edit,
    /// with `preset` and a long edge of `size`.
    fn entry(st: &State, frames: &[usize], preset: Option<&str>, size: &str) -> Entry {
        let frames = frames
            .iter()
            .map(|&i| {
                (
                    st.files[i].clone(),
                    st.sidecars[i].current.clone(),
                    0,
                    false,
                )
            })
            .collect();
        Entry {
            folder: None,
            ..entry_of(
                frames,
                Sheet {
                    size: size.into(),
                    ..Sheet::default()
                },
                preset.map(str::to_string),
            )
        }
    }

    /// Each frame given an edit of its own, recorded as a state, so an
    /// export of it has a state to be a line on.
    fn give_edits(st: &mut State) {
        for i in 0..st.files.len() {
            let mut e = Edit::default();
            e.light.exposure = 0.25 * (i as f32 + 1.0);
            st.sidecars[i].record(e);
        }
    }

    fn exported(path: &str) -> queue::Done {
        queue::Done::Exported {
            path: PathBuf::from(path),
            seconds: 1.0,
            note: None,
            left_out: Vec::new(),
        }
    }

    /// Frame `index` of the running set came back as `done`.
    fn frame(app: &App, state: &Rc<RefCell<State>>, index: usize, done: queue::Done) {
        let (set, source, edit) = {
            let st = state.borrow();
            let run = st.queue_run.as_ref().expect("the queue runs");
            let f = &st.export_queue[run.entry].frames[run.at[index]];
            // The name the worker was sent, as it hands it back.
            let set = run.set.clone().expect("the entry was sent");
            (set, run.sent[index].clone(), f.edit.clone())
        };
        crate::panel::deliver::deliver(
            app,
            Outcome::SetFrameDone {
                set,
                index,
                source,
                edit,
                done,
            },
        );
    }

    /// The running set is finished.
    fn finish(app: &App, state: &Rc<RefCell<State>>) {
        let set = state
            .borrow()
            .queue_run
            .as_ref()
            .unwrap()
            .set
            .clone()
            .unwrap();
        let tally = set.tally();
        crate::panel::deliver::deliver(app, Outcome::SetDone { set, tally });
    }

    /// Add to queue takes the selection as Export would (the frame on
    /// screen under the panel's edit, the other under its sidecar's),
    /// the sheet and the preset in use, writes the file and shows the
    /// count; a later change to the sheet or to a sidecar leaves the
    /// entry as it was queued.
    #[test]
    fn add_to_queue_keeps_what_export_would_take_and_writes_it() {
        let (dir, files) = shoot("add", 3);
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        let web = Sheet {
            size: "2048".into(),
            quality: 85.0,
            ..Sheet::default()
        };
        {
            let mut st = state.borrow_mut();
            st.current = Some(0);
            st.picked = vec![0, 2];
            st.sidecars[2].current.light.exposure = 1.0;
            st.sidecars[2].turn = 3;
            st.export_queue_file = Some(queue_file(&dir));
            st.export_presets = vec![ExportPreset {
                name: "Web".into(),
                sheet: web.clone(),
            }];
        }
        app.invoke_export_preset_chosen("Web".into());
        app.set_exposure(0.5);
        CHOSEN.with(|c| *c.borrow_mut() = Some(Some(dir.join("out"))));
        app.invoke_export_queue_add();

        let st = state.borrow();
        assert_eq!(st.export_queue.len(), 1);
        let e = &st.export_queue[0];
        let got: Vec<(PathBuf, f32, u8)> = e
            .frames
            .iter()
            .map(|f| (f.source.clone(), f.edit.light.exposure, f.turn))
            .collect();
        assert_eq!(
            got,
            vec![(files[0].clone(), 0.5, 0), (files[2].clone(), 1.0, 3)]
        );
        assert!(e.sheet.same(&web));
        assert_eq!(e.preset.as_deref(), Some("Web"));
        assert_eq!(e.folder.as_deref(), Some(dir.join("out").as_path()));
        assert_eq!(export_queue::load(&queue_file(&dir)), st.export_queue);
        assert_eq!(app.get_export_queue_sets(), 1);
        assert_eq!(app.get_export_queue_line(), "1 set, 2 frames queued");
        assert_eq!(app.get_status(), "2 frames queued; 1 set in the queue");
        // Nothing was exported.
        assert!(st.exporting.is_none() && st.queue_run.is_none());
        assert!(!dir.join("out").exists());
        // The panel's edit was made a state, as Export makes it.
        assert_eq!(st.sidecars[0].current.light.exposure, 0.5);
        drop(st);

        // The sheet and the sidecar move on; the entry does not.
        app.set_export_size("1024".into());
        app.invoke_export_sheet_changed();
        state.borrow_mut().sidecars[2].current.light.exposure = -1.0;
        let queued = state.borrow().export_queue[0].clone();
        assert_eq!(queued.sheet.size, "2048");
        assert_eq!(queued.frames[1].edit.light.exposure, 1.0);
        assert_eq!(export_queue::load(&queue_file(&dir))[0], queued);

        // A second set: the count says both.
        state.borrow_mut().picked = vec![0];
        CHOSEN.with(|c| *c.borrow_mut() = Some(None));
        app.invoke_export_queue_add();
        assert_eq!(app.get_export_queue_line(), "2 sets, 3 frames queued");
        assert_eq!(app.get_status(), "1 frame queued; 2 sets in the queue");
        let second = state.borrow().export_queue[1].clone();
        assert_eq!(second.sheet.size, "1024");
        assert_eq!(second.preset, None, "an edited sheet names no preset");
        assert_eq!(second.folder, None);
        crate::testing::remove_dir_retry(&dir);
    }

    /// Export the queue runs the entries in order, each as a set under
    /// its own sheet and preset, each frame a line in its history, and
    /// an entry leaves the queue (and the file) once its frames are
    /// done; the last one done says so.
    #[test]
    fn the_queue_runs_in_order_and_each_entry_leaves_when_done() {
        let (dir, files) = shoot("run", 3);
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.export_queue_file = Some(queue_file(&dir));
            give_edits(&mut st);
            let a = entry(&st, &[0, 1], Some("Web"), "2048");
            let b = entry(&st, &[2], Some("Print"), "Full");
            add(&mut st, &app, a);
            add(&mut st, &app, b);
        }
        app.invoke_export_queue_run();
        {
            let st = state.borrow();
            let set = st.exporting.as_ref().expect("the first set runs");
            assert_eq!(set.total, 2);
            assert_eq!(set.preset.as_deref(), Some("Web"));
            assert_eq!(set.settings.long_edge, Some(2048));
            assert!(app.get_export_running());
            assert!(app.get_export_queue_running());
        }
        frame(&app, &state, 0, exported("/nowhere/out/IMG_0000.jpg"));
        frame(&app, &state, 1, exported("/nowhere/out/IMG_0001.jpg"));
        finish(&app, &state);
        {
            let st = state.borrow();
            assert_eq!(st.export_queue.len(), 1, "the first entry is done");
            assert_eq!(st.export_queue[0].preset.as_deref(), Some("Print"));
            assert_eq!(export_queue::load(&queue_file(&dir)), st.export_queue);
            let set = st.exporting.as_ref().expect("the second set runs");
            assert_eq!(set.total, 1);
            assert_eq!(set.preset.as_deref(), Some("Print"));
            assert_eq!(set.settings.long_edge, None);
            assert_eq!(app.get_export_queue_line(), "1 set, 1 frame queued");
        }
        frame(&app, &state, 0, exported("/nowhere/out/IMG_0002.jpg"));
        finish(&app, &state);
        let st = state.borrow();
        assert!(st.export_queue.is_empty());
        assert!(st.exporting.is_none() && st.queue_run.is_none());
        assert!(!queue_file(&dir).exists(), "an empty queue leaves no file");
        assert_eq!(app.get_export_queue_sets(), 0);
        assert!(!app.get_export_running());
        assert!(
            app.get_status().ends_with("; the queue is done"),
            "{}",
            app.get_status()
        );
        // Each frame a line in its history, with its entry's preset.
        for (i, preset) in [(0, "Web"), (1, "Web"), (2, "Print")] {
            let s = &st.sidecars[i];
            assert_eq!(s.current_exports.len(), 1, "frame {i}");
            assert_eq!(s.current_exports[0].preset.as_deref(), Some(preset));
            assert!(
                s.current_exports[0]
                    .file
                    .ends_with(&format!("IMG_000{i}.jpg"))
            );
        }
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// Stop while an entry's frame is in hand: that frame is done, the
    /// frames passed over stay queued in their entry, the entries after
    /// it are left as they were, and nothing more begins.
    #[test]
    fn a_stop_leaves_the_rest_queued() {
        let (dir, files) = shoot("stop", 4);
        let app = crate::testing::window(4);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.export_queue_file = Some(queue_file(&dir));
            give_edits(&mut st);
            let a = entry(&st, &[0, 1, 2], None, "2048");
            let b = entry(&st, &[3], None, "2048");
            add(&mut st, &app, a);
            add(&mut st, &app, b);
        }
        app.invoke_export_queue_run();
        frame(&app, &state, 0, exported("/nowhere/out/IMG_0000.jpg"));
        app.invoke_export_stop();
        frame(&app, &state, 1, queue::Done::Canceled);
        frame(&app, &state, 2, queue::Done::Canceled);
        finish(&app, &state);
        let st = state.borrow();
        assert!(st.exporting.is_none() && st.queue_run.is_none());
        assert_eq!(st.export_queue.len(), 2);
        let left: Vec<&Path> = st.export_queue[0]
            .frames
            .iter()
            .map(|f| f.source.as_path())
            .collect();
        assert_eq!(left, vec![files[1].as_path(), files[2].as_path()]);
        assert_eq!(st.export_queue[1].frames.len(), 1);
        assert_eq!(export_queue::load(&queue_file(&dir)), st.export_queue);
        assert!(
            app.get_status()
                .ends_with("; 2 sets, 3 frames queued left in the queue"),
            "{}",
            app.get_status()
        );
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A frame that fails is the set's failure as in any set: no line in
    /// its history, the rest of its entry written, the entry done, and
    /// the queue goes on to the next.
    #[test]
    fn a_failed_frame_does_not_stop_the_queue() {
        let (dir, files) = shoot("fail", 3);
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            give_edits(&mut st);
            let a = entry(&st, &[0, 1], Some("Web"), "2048");
            let b = entry(&st, &[2], Some("Web"), "2048");
            add(&mut st, &app, a);
            add(&mut st, &app, b);
        }
        app.invoke_export_queue_run();
        frame(
            &app,
            &state,
            0,
            queue::Done::Failed {
                message: "not a raw".into(),
            },
        );
        frame(&app, &state, 1, exported("/nowhere/out/IMG_0001.jpg"));
        finish(&app, &state);
        let st = state.borrow();
        assert_eq!(st.export_queue.len(), 1, "the failed frame's entry is done");
        assert!(st.queue_run.is_some(), "and the next entry runs");
        assert!(st.sidecars[0].current_exports.is_empty());
        assert_eq!(st.sidecars[1].current_exports.len(), 1);
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The queue across a restart: a fresh window and state read the
    /// file, show the count, and start nothing.
    #[test]
    fn the_queue_is_kept_across_a_restart_and_not_run() {
        let (dir, files) = shoot("restart", 2);
        let settings = dir.join("config").join("settings.json");
        let queued = {
            let app = crate::testing::window(2);
            let (state, _worker) = crate::testing::state_for(&app, files.clone());
            let mut st = state.borrow_mut();
            open_at_launch(&mut st, &app, Some(&settings), true);
            assert!(st.export_queue.is_empty());
            give_edits(&mut st);
            let a = entry(&st, &[0], Some("Web"), "2048");
            let b = entry(&st, &[0, 1], None, "Full");
            add(&mut st, &app, a);
            add(&mut st, &app, b);
            st.export_queue.clone()
        };
        let app = crate::testing::window(2);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        open_at_launch(&mut state.borrow_mut(), &app, Some(&settings), true);
        let st = state.borrow();
        assert_eq!(st.export_queue, queued);
        assert_eq!(st.export_queue[1].frames[1].edit.light.exposure, 0.5);
        assert_eq!(app.get_export_queue_sets(), 2);
        assert_eq!(app.get_export_queue_line(), "2 sets, 3 frames queued");
        assert!(st.exporting.is_none() && st.queue_run.is_none());
        assert!(!app.get_export_running());
        drop(st);
        // A run that may not write (a snapshot) reads it and leaves it.
        let app = crate::testing::window(2);
        let (state, _worker) = crate::testing::state_for(&app, files);
        open_at_launch(&mut state.borrow_mut(), &app, Some(&settings), false);
        assert_eq!(state.borrow().export_queue, queued);
        clear(&mut state.borrow_mut(), &app);
        assert_eq!(export_queue::load(&queue_file(&dir)), queued);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A frame whose file is not there when the queue reaches it (a
    /// drive unplugged, a root offline, or deleted) is said and left
    /// queued, not sent and not dropped; an entry with none of its files
    /// there is passed over, kept, and the next one begun.
    #[test]
    fn a_missing_file_is_left_queued_and_said() {
        let (dir, files) = shoot("gone", 3);
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.export_queue_file = Some(queue_file(&dir));
            give_edits(&mut st);
            let a = entry(&st, &[0], None, "2048");
            let b = entry(&st, &[1, 2], None, "2048");
            add(&mut st, &app, a);
            add(&mut st, &app, b);
        }
        std::fs::remove_file(&files[0]).unwrap();
        std::fs::remove_file(&files[1]).unwrap();
        app.invoke_export_queue_run();
        {
            let st = state.borrow();
            assert_eq!(st.export_queue.len(), 2, "nothing is dropped");
            let run = st.queue_run.as_ref().expect("the next entry runs");
            assert_eq!(run.entry, 1);
            assert_eq!(run.set.as_ref().unwrap().total, 1);
            assert_eq!(run.at, vec![1]);
        }
        frame(&app, &state, 0, exported("/nowhere/out/IMG_0002.jpg"));
        finish(&app, &state);
        let st = state.borrow();
        assert!(st.queue_run.is_none());
        let left: Vec<Vec<&Path>> = st
            .export_queue
            .iter()
            .map(|e| e.frames.iter().map(|f| f.source.as_path()).collect())
            .collect();
        assert_eq!(
            left,
            vec![vec![files[0].as_path()], vec![files[1].as_path()]]
        );
        assert_eq!(export_queue::load(&queue_file(&dir)), st.export_queue);
        let status = app.get_status().to_string();
        assert!(
            status.contains("; 2 frames not there, left queued; 2 sets, 2 frames queued left"),
            "{status}"
        );
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// An entry whose folder, or whose watermark's PNG, is not there
    /// stops the run with the reason and is left whole: running later
    /// with the drive unplugged must not drain the queue.
    #[test]
    fn a_missing_folder_or_mark_stops_the_queue_and_keeps_the_entry() {
        let (dir, files) = shoot("nofolder", 2);
        let app = crate::testing::window(2);
        let (state, _worker) = crate::testing::state_for(&app, files);
        {
            let mut st = state.borrow_mut();
            let mut a = entry(&st, &[0, 1], None, "2048");
            a.folder = Some(dir.join("unplugged"));
            add(&mut st, &app, a);
        }
        app.invoke_export_queue_run();
        {
            let st = state.borrow();
            assert!(st.queue_run.is_none() && st.exporting.is_none());
            assert_eq!(st.export_queue[0].frames.len(), 2);
        }
        let status = app.get_status().to_string();
        assert!(status.starts_with("the queue stopped: "), "{status}");
        assert!(status.contains("unplugged is not there"), "{status}");

        // The folder back, a mark whose PNG is gone.
        std::fs::create_dir_all(dir.join("unplugged")).unwrap();
        {
            let mut st = state.borrow_mut();
            st.export_queue[0].sheet.mark = crate::sheet::MARK_IMAGE.into();
            st.export_queue[0].sheet.mark_image = dir.join("logo.png").to_string_lossy().into();
        }
        app.invoke_export_queue_run();
        assert!(state.borrow().queue_run.is_none());
        assert_eq!(state.borrow().export_queue[0].frames.len(), 2);
        let status = app.get_status().to_string();
        assert!(status.contains("the watermark's PNG"), "{status}");

        // Both there: it runs.
        image::RgbaImage::new(4, 4)
            .save(dir.join("logo.png"))
            .unwrap();
        app.invoke_export_queue_run();
        assert!(state.borrow().queue_run.as_ref().unwrap().set.is_some());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A window closed (or a crash) after some of an entry's frames are
    /// written: the file no longer holds those, so the next run does not
    /// send them again; the entry in memory keeps them for the run's
    /// count.
    #[test]
    fn a_window_closed_mid_entry_does_not_send_its_done_frames_again() {
        let (dir, files) = shoot("crash", 3);
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.export_queue_file = Some(queue_file(&dir));
            give_edits(&mut st);
            let a = entry(&st, &[0, 1, 2], None, "2048");
            let b = entry(&st, &[0], None, "Full");
            add(&mut st, &app, a);
            add(&mut st, &app, b);
        }
        app.invoke_export_queue_run();
        frame(&app, &state, 0, exported("/nowhere/out/IMG_0000.jpg"));
        // No SetDone: the window goes away here.
        assert_eq!(state.borrow().export_queue[0].frames.len(), 3);
        let on_disk = export_queue::load(&queue_file(&dir));
        assert_eq!(on_disk.len(), 2);
        let left: Vec<&Path> = on_disk[0]
            .frames
            .iter()
            .map(|f| f.source.as_path())
            .collect();
        assert_eq!(left, vec![files[1].as_path(), files[2].as_path()]);
        // All three done: the entry is gone from the file, the next kept.
        frame(&app, &state, 1, exported("/nowhere/out/IMG_0001.jpg"));
        frame(&app, &state, 2, exported("/nowhere/out/IMG_0002.jpg"));
        let on_disk = export_queue::load(&queue_file(&dir));
        assert_eq!(on_disk.len(), 1);
        assert_eq!(on_disk[0].sheet.size, "Full");
        crate::testing::remove_dir_retry(&dir);
    }

    /// A folder opened by a relative path: the queue keeps each frame
    /// whole, and sends it under the list's own name, so the history
    /// line lands on the frame in the list.
    #[test]
    fn a_relative_name_is_kept_whole_and_sent_as_the_list_has_it() {
        let (dir, files) = shoot("relative", 1);
        // The file by a path relative to the working directory: up
        // to what the two share, then down to the file. On Windows
        // the two may sit on different drives, which no relative
        // path can join; then there is nothing to test.
        let cwd = std::env::current_dir().unwrap();
        let shared = cwd
            .components()
            .zip(files[0].components())
            .take_while(|(a, b)| a == b)
            .count();
        if shared == 0 {
            crate::testing::remove_dir_retry(&dir);
            return;
        }
        let mut rel = PathBuf::new();
        for _ in cwd.components().skip(shared) {
            rel.push("..");
        }
        for c in files[0].components().skip(shared) {
            rel.push(c);
        }
        assert!(rel.is_relative() && rel.is_file(), "{}", rel.display());
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, vec![rel.clone()]);
        {
            let mut st = state.borrow_mut();
            give_edits(&mut st);
            let e = entry(&st, &[0], Some("Web"), "2048");
            assert!(e.frames[0].source.is_absolute());
            add(&mut st, &app, e);
        }
        app.invoke_export_queue_run();
        assert_eq!(state.borrow().queue_run.as_ref().unwrap().sent, vec![rel]);
        frame(&app, &state, 0, exported("/nowhere/out/IMG_0000.jpg"));
        finish(&app, &state);
        assert_eq!(state.borrow().sidecars[0].current_exports.len(), 1);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A mark with nothing to draw is refused at Add to queue, the sheet
    /// left up, nothing queued.
    #[test]
    fn an_empty_mark_is_not_queued() {
        let (dir, files) = shoot("mark", 1);
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, files);
        state.borrow_mut().current = Some(0);
        app.set_export_mark(crate::sheet::MARK_IMAGE.into());
        app.set_export_mark_image("".into());
        app.set_export_open(false);
        CHOSEN.with(|c| *c.borrow_mut() = Some(None));
        app.invoke_export_queue_add();
        assert!(state.borrow().export_queue.is_empty());
        assert!(app.get_export_open());
        assert!(
            app.get_status().starts_with("not queued: "),
            "{}",
            app.get_status()
        );
        CHOSEN.with(|c| c.borrow_mut().take());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A plain set's frames and end, while the queue runs an entry of its
    /// own, do not touch the queue.
    #[test]
    fn a_plain_set_leaves_the_queue_alone() {
        let (dir, files) = shoot("plain", 2);
        let app = crate::testing::window(2);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            give_edits(&mut st);
            let a = entry(&st, &[0, 1], None, "2048");
            add(&mut st, &app, a);
        }
        app.invoke_export_queue_run();
        let other = Arc::new(queue::Set::new(
            1,
            None,
            export::Settings::default(),
            export::OnExists::Increment,
        ));
        crate::panel::deliver::deliver(
            &app,
            Outcome::SetFrameDone {
                set: other.clone(),
                index: 0,
                source: files[0].clone(),
                edit: Edit::default(),
                done: exported("/nowhere/out/X.jpg"),
            },
        );
        crate::panel::deliver::deliver(
            &app,
            Outcome::SetDone {
                set: other,
                tally: queue::Tally::default(),
            },
        );
        let st = state.borrow();
        let run = st.queue_run.as_ref().expect("the queue's entry still runs");
        assert!(run.done.iter().all(|d| !d));
        assert!(st.exporting.is_some());
        assert_eq!(st.export_queue[0].frames.len(), 2);
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A frame queued with its blend still to be seeded comes back under
    /// the edit the worker rendered, seeded: it is still a line in its
    /// history, and the frame takes the seed, as any set's frame does.
    #[test]
    fn a_seeded_frame_is_a_line_in_its_history() {
        let (dir, files) = shoot("seed", 1);
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, files);
        let queued = {
            let mut st = state.borrow_mut();
            st.seed_blend[0] = true;
            let mut e = entry(&st, &[0], Some("Web"), "2048");
            e.frames[0].seed_blend = true;
            let queued = e.frames[0].edit.clone();
            add(&mut st, &app, e);
            queued
        };
        app.invoke_export_queue_run();
        let (set, source) = {
            let st = state.borrow();
            let run = st.queue_run.as_ref().unwrap();
            (run.set.clone().unwrap(), run.sent[0].clone())
        };
        let mut rendered = queued.clone();
        rendered.noise.learned_strength = 0.35;
        assert_ne!(rendered, queued);
        crate::panel::deliver::deliver(
            &app,
            Outcome::SetFrameDone {
                set,
                index: 0,
                source,
                edit: rendered,
                done: exported("/nowhere/out/IMG_0000.jpg"),
            },
        );
        finish(&app, &state);
        let st = state.borrow();
        assert_eq!(st.sidecars[0].current_exports.len(), 1);
        assert_eq!(
            st.sidecars[0].current_exports[0].preset.as_deref(),
            Some("Web")
        );
        assert_eq!(st.sidecars[0].current.noise.learned_strength, 0.35);
        assert!(!st.seed_blend[0]);
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// Clear empties the queue and removes the file; not while it runs.
    #[test]
    fn clear_empties_the_queue_and_the_file() {
        let (dir, files) = shoot("clear", 2);
        let app = crate::testing::window(2);
        let (state, _worker) = crate::testing::state_for(&app, files);
        {
            let mut st = state.borrow_mut();
            st.export_queue_file = Some(queue_file(&dir));
            let a = entry(&st, &[0, 1], None, "2048");
            let b = entry(&st, &[1], None, "2048");
            add(&mut st, &app, a);
            add(&mut st, &app, b);
        }
        assert!(queue_file(&dir).exists());
        app.invoke_export_queue_run();
        app.invoke_export_queue_clear();
        assert_eq!(state.borrow().export_queue.len(), 2, "not while it runs");
        app.invoke_export_stop();
        frame(&app, &state, 0, queue::Done::Canceled);
        frame(&app, &state, 1, queue::Done::Canceled);
        finish(&app, &state);
        assert_eq!(state.borrow().export_queue.len(), 2);
        app.invoke_export_queue_clear();
        assert!(state.borrow().export_queue.is_empty());
        assert!(!queue_file(&dir).exists());
        assert_eq!(app.get_export_queue_sets(), 0);
        assert_eq!(app.get_status(), "export queue cleared (2 sets)");
        crate::testing::remove_dir_retry(&dir);
    }
}
