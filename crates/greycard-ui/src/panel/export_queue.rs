//! The export queue in the window: Add to queue on the export sheet,
//! the sets listed and the count at the sheet's foot, a set's removal,
//! Export the queue and Clear.
//!
//! An entry is what Export would have sent at the moment it was queued
//! (the frames, each frame's edit, the sheet, the preset in use and the
//! folder), and running one is a set's export: `start_set_after` and the
//! worker's frames, with the status plate's Stop as its cancel. The
//! entries run in order, one set after the other; each frame done with
//! leaves its entry, and an entry with no frames left leaves the queue.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use crate::tr;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::export_queue::{self, Entry, QueuedFrame};
use crate::panel::browser::chosen_frames;
use crate::panel::deliver::{SetFrames, preset_in_use, pressed_frames, read_sheet};
use crate::worker::Worker;
use crate::{App, QueueRow, STATE, Shared, State, queue};

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
    /// The ids of the entry's frames done with because they failed:
    /// their tiles are not marked exported while the set runs.
    pub(crate) failed: Vec<u64>,
    /// The frames of this run's earlier entries, and of this one, that
    /// were not there to send and were left queued.
    pub(crate) left_behind: usize,
    /// The run so far, across its entries: what the status line counts
    /// in, since a queue built a frame at a time is a set of one an
    /// entry.
    pub(crate) whole: Whole,
    /// Which look for the files this is, so a late answer to one the
    /// queue has since let go of is not taken for the current one.
    pub(crate) probe: u64,
    /// The names the set's frames were sent under, for a test to hand
    /// back as the worker would.
    #[cfg(test)]
    pub(crate) sent: Vec<PathBuf>,
}

/// A run of the queue across its entries: the frames sent in the sets
/// before this one, what they came to, when it began, and where they
/// went.
#[derive(Clone)]
pub(crate) struct Whole {
    pub(crate) before: usize,
    pub(crate) tally: queue::Tally,
    pub(crate) started: std::time::Instant,
    pub(crate) places: Vec<String>,
}

impl Whole {
    pub(crate) fn new() -> Self {
        Self {
            before: 0,
            tally: queue::Tally::default(),
            started: std::time::Instant::now(),
            places: Vec::new(),
        }
    }

    /// `set`'s tally added in, the set done with.
    fn add(&mut self, set: &queue::Set, tally: &queue::Tally) {
        self.before += set.total;
        self.tally.exported += tally.exported;
        self.tally.skipped += tally.skipped;
        self.tally.canceled += tally.canceled;
        self.tally.failed.extend(tally.failed.iter().cloned());
        self.tally.parts.0 += tally.parts.0;
        self.tally.parts.1 += tally.parts.1;
        let place = set.place();
        if !self.places.contains(&place) {
            self.places.push(place);
        }
    }
}

/// The frames the queue's run is to send in all, while `set` is its
/// running set: those sent before it, its own, and every frame of the
/// entries after it. A frame later found not there, or taken out of the
/// queue, leaves the count as it is found.
fn run_total(st: &State, run: &Run, set: &queue::Set) -> usize {
    let after: usize = st
        .work
        .export_queue
        .iter()
        .skip(run.entry + 1)
        .map(|e| e.frames.len())
        .sum();
    run.whole.before + set.total + after
}

/// The id of frame `index` of `set`, when the set is the queue run's.
pub(crate) fn id_in_run(st: &State, set: &Arc<queue::Set>, index: usize) -> Option<u64> {
    let run = st.work.queue_run.as_ref().filter(|r| r.is(set))?;
    let at = *run.at.get(index)?;
    Some(st.work.export_queue.get(run.entry)?.frames.get(at)?.id)
}

/// The status line while frame `index` of `set` is written: counted
/// across the queue's run when the set is the run's, else in the set.
pub(crate) fn progress_line(st: &State, set: &Arc<queue::Set>, index: usize, name: &str) -> String {
    match st.work.queue_run.as_ref().filter(|r| r.is(set)) {
        Some(run) => queue::progress_line(run.whole.before + index, run_total(st, run, set), name),
        None => queue::progress_line(index, set.total, name),
    }
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
    /// Each frame, by its place in the entry: there to export, or not,
    /// and why.
    present: Vec<Presence>,
    /// The folder the entry goes to is there (or it goes beside each
    /// file).
    folder: bool,
    /// The watermark's PNG is there (or the mark is not an image).
    mark: bool,
}

/// What the look found of one queued frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presence {
    /// The file is not there: left queued for a run when it is back.
    Absent,
    /// The file, and the version when the frame is one, are there.
    There,
    /// The file is there and its sidecar reads, and the version the
    /// frame is of is not in it: deleted since the frame was queued,
    /// with no line to export or to record on.
    Deleted,
    /// The file is there but whether the version is cannot be told (a
    /// sidecar that will not read, or none where the version was):
    /// left queued, as a file not there is, never taken for deleted.
    Unsure,
}

/// One queued frame as the look checks it: its file, the version it is
/// of, and the window's own answer on the version when it holds the
/// file read (the copy in memory is the truth then, and the disk is not
/// asked).
pub(crate) struct Check {
    source: PathBuf,
    version: Option<String>,
    held: Option<bool>,
}

/// Look for the frames `checks`, `folder` and the mark's `png`: off the
/// window's thread, since a stale mount can hang a stat, or a sidecar's
/// read, for as long as it likes.
fn look(checks: &[Check], folder: Option<&Path>, png: Option<&Path>) -> Found {
    Found {
        present: checks
            .iter()
            .map(|c| {
                if !c.source.is_file() {
                    return Presence::Absent;
                }
                match (&c.version, c.held) {
                    (None, _) => Presence::There,
                    (Some(_), Some(true)) => Presence::There,
                    (Some(_), Some(false)) => Presence::Deleted,
                    (Some(id), None) => version_presence(&c.source, id),
                }
            })
            .collect(),
        folder: folder.is_none_or(Path::is_dir),
        mark: png.is_none_or(Path::is_file),
    }
}

/// Whether version `id` is in the sidecar on disk beside `source`: only
/// a sidecar that reads and lacks the id says deleted; one that will not
/// read, or none at all, cannot tell. A read of the disk, so off the
/// window's thread.
pub(crate) fn version_presence(source: &Path, id: &str) -> Presence {
    match greycard_edit::Sidecar::load_as_file(source) {
        Ok(Some(s)) if s.version(id).is_some() => Presence::There,
        Ok(Some(_)) => Presence::Deleted,
        Ok(None) | Err(_) => Presence::Unsure,
    }
}

/// The watermark's PNG an entry needs, if its mark is an image.
fn mark_png(entry: &Entry) -> Option<PathBuf> {
    match entry.sheet.settings().watermark.map(|m| m.kind) {
        Some(crate::watermark::Kind::Image { path }) => Some(path),
        _ => None,
    }
}

/// The sets listed and the count at the sheet's foot: the set the
/// queue is on marked, its bin off.
pub(crate) fn show_queue(st: &State, app: &App) {
    app.set_export_queue_sets(st.work.export_queue.len() as i32);
    app.set_export_queue_line(export_queue::count_line(&st.work.export_queue).into());
    app.set_export_queue_running(st.work.queue_run.as_ref().is_some_and(|r| r.set.is_some()));
    app.set_export_queue_busy(st.work.queue_run.is_some());
    let on = st.work.queue_run.as_ref().map(|r| r.entry);
    let rows: Vec<QueueRow> = st
        .work
        .export_queue
        .iter()
        .enumerate()
        .map(|(i, e)| QueueRow {
            name: export_queue::name(e).into(),
            frames: tr!("{n} frame" | "{n} frames" % e.frames.len()).into(),
            folder: export_queue::place_shown(e).into(),
            running: on == Some(i),
        })
        .collect();
    app.set_export_queue_rows(ModelRc::new(VecModel::from(rows)));
    // The queue's view: its count, and its tiles' marks as the run goes.
    crate::panel::queue_view::show_count(st, app);
    crate::panel::queue_view::marks(st, app);
}

/// Write the queue now, as it is: every change is kept at once. While
/// an entry runs, the frames of it already done with are left out of
/// the file (not out of the entry, which the run still counts by
/// place), so a window closed or a crash mid-entry does not send them
/// again at the next run.
pub(crate) fn keep(st: &State) {
    let Some(path) = &st.work.export_queue_file else {
        return;
    };
    match &st.work.queue_run {
        Some(run) if run.done.iter().any(|d| *d) => {
            let mut queue = st.work.export_queue.clone();
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
        _ => export_queue::save(path, &st.work.export_queue),
    }
}

/// The words for frames left queued because their files were not
/// there: "; 2 frames not there, left queued", or nothing.
fn left_behind(n: usize) -> String {
    match n {
        0 => String::new(),
        n => tr!("; {n} frame not there, left queued" | "; {n} frames not there, left queued" % n),
    }
}

/// The queue the last session left, read and shown at launch, never
/// run: `settings` is the settings file the queue sits beside, and
/// `write` whether this run may change it (not a snapshot or a batch
/// run).
pub(crate) fn open_at_launch(st: &mut State, app: &App, settings: Option<&Path>, write: bool) {
    let path = settings.map(export_queue::path_beside);
    st.work.export_queue = path.as_deref().map(export_queue::load).unwrap_or_default();
    st.work.export_queue_file = path.filter(|_| write);
    if !st.work.export_queue.is_empty() {
        tracing::info!(
            "export queue: {}, waiting",
            export_queue::count_line(&st.work.export_queue)
        );
    }
    show_queue(st, app);
}

/// Add to queue, the set's sidecars in hand: what Export would take
/// from `pressed` and `frames` now, into the folder the sheet's Folder
/// row shows or, with a subfolder on the sheet, into that beside each
/// frame, appended to the queue. Nothing is exported.
fn queue_pressed(state: &Rc<Shared>, app: &App, pressed: &crate::versions::Key, frames: &[usize]) {
    if state.borrow().work.export_choosing {
        return;
    }
    let Some((raw, _, frames, _)) = pressed_frames(state, app, pressed, frames) else {
        return;
    };
    let sheet = read_sheet(app);
    // A mark asked for with nothing to draw would fail every frame
    // when the queue runs: say so now and keep the sheet up.
    if let Some(Err(e)) = sheet.settings().watermark.as_ref().map(|m| m.check()) {
        app.set_status(tr!("not queued: {why}", why = format!("{e:#}")).into());
        app.set_export_open(true);
        return;
    }
    // A subfolder that is not one would stop the run; one that is
    // goes beside each frame, and none goes into the folder the
    // sheet's Folder row shows, as Export would.
    let sub = match sheet.subfolder() {
        Ok(sub) => sub,
        Err(e) => {
            app.set_status(tr!("not queued: {why}", why = e).into());
            app.set_export_open(true);
            return;
        }
    };
    let entry = entry_of(frames, sheet, preset_in_use(app));
    if entry.frames.is_empty() {
        app.set_status(tr!("nothing to queue").into());
        return;
    }
    let mut st = state.borrow_mut();
    // Whether the folder is still there is looked at when the queue
    // runs, which stops on one that is not.
    let folder = sub
        .is_none()
        .then(|| crate::panel::deliver::sheet_folder(&st, &raw));
    add(&mut st, app, Entry { folder, ..entry });
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
            .map(|(source, edit, turn, seed_blend, version)| QueuedFrame {
                // Whole, since the queue outlives this run's working
                // directory: a folder opened by a relative path would
                // name nothing from the next launch's.
                source: std::path::absolute(&source).unwrap_or(source),
                edit,
                turn,
                seed_blend,
                version,
                id: export_queue::fresh_id(),
            })
            .collect(),
    }
}

/// Append `entry`, keep the file, and say where the queue stands.
pub(crate) fn add(st: &mut State, app: &App, entry: Entry) {
    st.work.export_choosing = false;
    let n = entry.frames.len();
    tracing::info!(
        "queued {} {}: {}",
        queue::frames(n),
        queue::place(
            entry.folder.as_deref(),
            entry.sheet.subfolder().ok().flatten().as_deref()
        ),
        entry.sheet.settings().describe()
    );
    st.work.export_queue.push(entry);
    crate::panel::queue_view::changed(st, app);
    keep(st);
    show_queue(st, app);
    app.set_status(
        tr!(
            "{n} frame queued; {sets} in the queue" | "{n} frames queued; {sets} in the queue" % n,
            sets = export_queue::sets(st.work.export_queue.len())
        )
        .into(),
    );
}

/// Export the queue: its entries in order, from the first.
pub(crate) fn run(st: &mut State, app: &App) {
    if st.work.exporting.is_some() || st.work.export_choosing || st.work.queue_run.is_some() {
        app.set_status(tr!("an export is running; the queue waits for it").into());
        return;
    }
    if st.work.export_queue.is_empty() {
        app.set_status(tr!("nothing queued").into());
        return;
    }
    begin_at(st, app, 0, 0, Whole::new());
}

/// Look for the files of entry `index` (off the window's thread) and,
/// once they are found, send it as a set; past the last entry, the
/// run is over. `left_behind` counts the frames earlier entries left
/// queued for not being there.
fn begin_at(st: &mut State, app: &App, index: usize, left_behind: usize, whole: Whole) {
    let Some(entry) = st.work.export_queue.get(index) else {
        st.work.queue_run = None;
        show_queue(st, app);
        return;
    };
    let checks: Vec<Check> = entry
        .frames
        .iter()
        .map(|f| Check {
            source: f.source.clone(),
            version: f.version.as_ref().map(|v| v.id.clone()),
            // The window's copy of the sidecar, where it holds the file
            // read, says whether the version is there; the thread reads
            // the disk for the rest.
            held: f.version.as_ref().and_then(|v| {
                let file = st.file_by_path(&f.source)?;
                crate::rows::file_is_loaded(st, file)
                    .then(|| st.sidecars[file].version(&v.id).is_some())
            }),
        })
        .collect();
    let folder = entry.folder.clone();
    let png = mark_png(entry);
    st.work.queue_probes += 1;
    let probe = st.work.queue_probes;
    st.work.queue_run = Some(Run {
        entry: index,
        set: None,
        at: Vec::new(),
        done: vec![false; checks.len()],
        failed: Vec::new(),
        left_behind,
        whole,
        probe,
        #[cfg(test)]
        sent: Vec::new(),
    });
    show_queue(st, app);
    // A test answers at once; the window asks a thread, whose answer
    // comes back through the event loop.
    if cfg!(test) {
        let found = look(&checks, folder.as_deref(), png.as_deref());
        found_files(st, app, probe, found);
        return;
    }
    app.set_status(tr!("looking for the queued files...").into());
    let weak = app.as_weak();
    std::thread::spawn(move || {
        let found = look(&checks, folder.as_deref(), png.as_deref());
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
        .work
        .queue_run
        .as_ref()
        .filter(|r| r.probe == probe && r.set.is_none())
    else {
        return;
    };
    let (index, mut left) = (run.entry, run.left_behind);
    let Some(entry) = st.work.export_queue.get(index) else {
        st.work.queue_run = None;
        show_queue(st, app);
        return;
    };
    let stop = |st: &mut State, why: String| {
        st.work.queue_run = None;
        show_queue(st, app);
        tracing::warn!("export queue stopped: {why}");
        app.set_status(
            tr!(
                "the queue stopped: {why}; {left} left in the queue",
                why = why,
                left = export_queue::count_line(&st.work.export_queue)
            )
            .into(),
        );
    };
    // An export begun while the files were looked for has the worker.
    if st.work.exporting.is_some() {
        stop(st, tr!("another export began"));
        return;
    }
    if !found.folder {
        let folder = entry.folder.clone().unwrap_or_default();
        stop(st, tr!("{folder} is not there", folder = folder.display()));
        return;
    }
    if !found.mark {
        let png = mark_png(entry).unwrap_or_default();
        stop(
            st,
            tr!(
                "the watermark's PNG {file} is not there",
                file = png.display()
            ),
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
    let mut deleted: Vec<usize> = Vec::new();
    let mut deleted_names: Vec<String> = Vec::new();
    for (i, (f, presence)) in entry.frames.iter().zip(&found.present).enumerate() {
        match presence {
            Presence::There => {
                let source = listed
                    .get(&f.source)
                    .map(|p| (*p).clone())
                    .unwrap_or_else(|| f.source.clone());
                frames.push((
                    source,
                    f.edit.clone(),
                    f.turn,
                    f.seed_blend,
                    f.version.clone(),
                ));
                at.push(i);
            }
            // A version deleted since the entry was queued has no line to
            // export or to record on: left out, and said, rather than
            // written from the edit the entry stored.
            Presence::Deleted => {
                let name = queue::label_of(&f.source, f.version.as_ref());
                tracing::warn!("export queue: {name} was deleted; not exported");
                deleted.push(i);
                deleted_names.push(name);
            }
            // Offline or gone, or a version whose sidecar cannot say:
            // kept, for a run when it is back.
            Presence::Absent | Presence::Unsure => {
                tracing::warn!(
                    "export queue: {} is not there; left queued",
                    queue::label_of(&f.source, f.version.as_ref())
                );
                missing.push(queue::label_of(&f.source, f.version.as_ref()));
            }
        }
    }
    left += missing.len();
    // A subfolder the sheet was queued with that no longer reads as
    // one (a file edited by hand) goes beside each file, as an entry
    // with no folder always has.
    let (folder, sub, settings, on_exists, preset, name) = (
        entry.folder.clone(),
        entry.sheet.subfolder().ok().flatten(),
        entry.sheet.settings(),
        entry.sheet.on_exists(),
        entry.preset.clone(),
        entry.sheet.name.clone(),
    );
    // The deleted versions out of the entry, and the places of the rest
    // moved up with them; the entry's own fields were read above. An
    // entry with nothing left in it goes with them, so it does not
    // stand in the queue with no frame to run.
    let mut emptied = false;
    if !deleted.is_empty() {
        if let Some(entry) = st.work.export_queue.get_mut(index) {
            for &i in deleted.iter().rev() {
                entry.frames.remove(i);
            }
            emptied = entry.frames.is_empty();
        }
        if emptied {
            st.work.export_queue.remove(index);
        }
        for a in &mut at {
            *a -= deleted.iter().filter(|&&d| d < *a).count();
        }
        if let Some(run) = st.work.queue_run.as_mut() {
            run.done = vec![false; run.done.len() - deleted.len()];
        }
        crate::panel::queue_view::changed(st, app);
        keep(st);
    }
    let deleted = match deleted_names.as_slice() {
        [] => None,
        names => Some(tr!(
            "{names} was deleted; not exported"
                | "{names} were deleted; not exported" % names.len(),
            names = crate::panel::deliver::listed(names)
        )),
    };
    // Joined onto a line that says more, after its own words.
    let deleted_said = deleted
        .as_ref()
        .map(|d| format!("; {d}"))
        .unwrap_or_default();
    if frames.is_empty() {
        let said = if missing.is_empty() {
            deleted.unwrap_or_default()
        } else {
            tr!(
                "{names} not there; left queued{deleted}",
                names = crate::panel::deliver::listed(&missing),
                deleted = deleted_said
            )
        };
        app.set_status(said.into());
        // The next entry: at this place when this one went with its
        // deleted versions, after it otherwise.
        let whole = st
            .work
            .queue_run
            .as_ref()
            .map_or_else(Whole::new, |r| r.whole.clone());
        begin_at(
            st,
            app,
            if emptied { index } else { index + 1 },
            left,
            whole,
        );
        return;
    }
    let count = frames.len();
    let after = st.work.export_queue.len() - index - 1;
    #[cfg(test)]
    let sent: Vec<PathBuf> = frames.iter().map(|f| f.0.clone()).collect();
    let stems = crate::panel::deliver::set_stems(st, &frames, &name);
    let set = crate::panel::deliver::Pending {
        frames,
        folder,
        sub,
        settings,
        preset,
        stems,
    };
    let Some(set) = crate::panel::deliver::start_set(st, app, set, on_exists) else {
        st.work.queue_run = None;
        show_queue(st, app);
        return;
    };
    if let Some(run) = st.work.queue_run.as_mut() {
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
        tr!(
            "exporting {n} frame from the queue, {sets} after it...{deleted}"
                | "exporting {n} frames from the queue, {sets} after it...{deleted}" % count,
            sets = export_queue::sets(after),
            deleted = deleted_said
        )
        .into(),
    );
}

/// Frame `index` of `set` came back; `ran` when it was begun, not
/// passed over by a cancel, and `failed` when it failed. A frame begun
/// is done with, however it went, and the file says so at once.
pub(crate) fn frame_done(
    st: &mut State,
    set: &Arc<queue::Set>,
    index: usize,
    ran: bool,
    failed: bool,
) {
    let id = id_in_run(st, set, index);
    if let Some(run) = st.work.queue_run.as_mut()
        && run.is(set)
        && ran
        && let Some(&i) = run.at.get(index)
        && let Some(d) = run.done.get_mut(i)
    {
        *d = true;
        if failed && let Some(id) = id {
            run.failed.push(id);
        }
        keep(st);
    }
    if st.work.queue_busy.is_some() && st.work.queue_busy == id {
        st.work.queue_busy = None;
    }
}

/// `set` is finished, its finished line `line`: when it was the
/// queue's, what is left of its entry stays queued, and the next entry
/// goes unless the set was stopped.
pub(crate) fn set_done(
    st: &mut State,
    app: &App,
    set: &Arc<queue::Set>,
    tally: &queue::Tally,
    line: &str,
) {
    let Some(run) = st.work.queue_run.as_ref().filter(|r| r.is(set)) else {
        return;
    };
    // The finished line counts the whole run, as its progress did.
    let total = run_total(st, run, set);
    let Some(mut run) = st.work.queue_run.take() else {
        return;
    };
    tracing::debug!("export queue: set of entry {} done: {line}", run.entry);
    run.whole.add(set, tally);
    let place = match run.whole.places.as_slice() {
        [one] => one.clone(),
        _ => String::new(),
    };
    let line = &queue::finished_line(
        &run.whole.tally,
        total,
        &place,
        run.whole.started.elapsed().as_secs_f64(),
    );
    let mut next = run.entry + 1;
    if let Some(entry) = st.work.export_queue.get_mut(run.entry) {
        let mut done = run.done.iter();
        entry
            .frames
            .retain(|_| !done.next().copied().unwrap_or(false));
        if entry.frames.is_empty() {
            st.work.export_queue.remove(run.entry);
            next = run.entry;
        }
    }
    crate::panel::queue_view::changed(st, app);
    keep(st);
    show_queue(st, app);
    let behind = left_behind(run.left_behind);
    if set.is_canceled() {
        let left = match st.work.export_queue.len() {
            0 => tr!("nothing left in the queue"),
            _ => tr!(
                "{left} left in the queue",
                left = export_queue::count_line(&st.work.export_queue)
            ),
        };
        app.set_status(
            tr!(
                "{line}{behind}; {left}",
                line = line,
                behind = behind,
                left = left
            )
            .into(),
        );
        return;
    }
    if next >= st.work.export_queue.len() {
        let end = if st.work.export_queue.is_empty() {
            tr!("the queue is done")
        } else {
            tr!(
                "{left} left in the queue",
                left = export_queue::count_line(&st.work.export_queue)
            )
        };
        app.set_status(
            tr!(
                "{line}{behind}; {left}",
                line = line,
                behind = behind,
                left = end
            )
            .into(),
        );
        return;
    }
    begin_at(st, app, next, run.left_behind, run.whole);
}

/// Clear: every entry and the file. Not while an entry is being
/// exported; while its files are still being looked for, the look is
/// let go of.
pub(crate) fn clear(st: &mut State, app: &App) {
    if st.work.queue_run.as_ref().is_some_and(|r| r.set.is_some()) {
        app.set_status(tr!("the queue is running; stop it first").into());
        return;
    }
    let n = st.work.export_queue.len();
    st.work.queue_run = None;
    st.work.export_queue.clear();
    crate::panel::queue_view::changed(st, app);
    keep(st);
    show_queue(st, app);
    app.set_status(
        tr!(
            "export queue cleared ({sets})",
            sets = export_queue::sets(n)
        )
        .into(),
    );
}

/// Take set `index` out of the queue and the file at once. Not the set
/// the queue is on, whose bin is off: a run reaches its next set by
/// place, so one removed before the run's own moves the run up with
/// it, and one after it is simply not there to begin.
pub(crate) fn remove(st: &mut State, app: &App, index: usize) {
    if index >= st.work.export_queue.len() {
        return;
    }
    if st.work.queue_run.as_ref().is_some_and(|r| r.entry == index) {
        tracing::info!("export queue: set {index} is running; not removed");
        return;
    }
    let gone = st.work.export_queue.remove(index);
    if let Some(run) = st.work.queue_run.as_mut()
        && index < run.entry
    {
        run.entry -= 1;
    }
    crate::panel::queue_view::changed(st, app);
    keep(st);
    show_queue(st, app);
    tracing::info!(
        "export queue: removed {} ({}); {}",
        export_queue::name(&gone),
        queue::frames(gone.frames.len()),
        export_queue::count_line(&st.work.export_queue)
    );
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
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
                let paths: Vec<crate::versions::Key> = chosen_frames(&st)
                    .into_iter()
                    .filter_map(|c| st.key_of(c))
                    .collect();
                let Some(pressed) = st.key_of(c) else {
                    return;
                };
                (pressed, paths)
            };
            crate::rows::request(
                &state,
                &app,
                &worker,
                paths,
                crate::rows::Scope::Line,
                crate::rows::Request::Queue,
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
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_export_queue_remove(move |index| {
            if let Some(app) = app_weak.upgrade()
                && let Ok(index) = usize::try_from(index)
            {
                remove(&mut state.borrow_mut(), &app, index);
            }
        });
    }
}

#[cfg(test)]
mod tests;
