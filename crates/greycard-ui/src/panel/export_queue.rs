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

use crate::export_queue::{self, Entry, QueuedFrame};
use crate::panel::browser::chosen_frames;
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
    match greycard_edit::Sidecar::load(source) {
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
            frames: queue::frames(e.frames.len()).into(),
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
        n => format!("; {} not there, left queued", queue::frames(n)),
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
fn queue_pressed(
    state: &Rc<RefCell<State>>,
    app: &App,
    pressed: &crate::versions::Key,
    frames: &[usize],
) {
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
        app.set_status(format!("not queued: {e:#}").into());
        app.set_export_open(true);
        return;
    }
    // A subfolder that is not one would stop the run; one that is
    // goes beside each frame, and none goes into the folder the
    // sheet's Folder row shows, as Export would.
    let sub = match sheet.subfolder() {
        Ok(sub) => sub,
        Err(e) => {
            app.set_status(format!("not queued: {e}").into());
            app.set_export_open(true);
            return;
        }
    };
    let entry = entry_of(frames, sheet, preset_in_use(app));
    if entry.frames.is_empty() {
        app.set_status("nothing to queue".into());
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
        format!(
            "{} queued; {} in the queue",
            queue::frames(n),
            export_queue::sets(st.work.export_queue.len())
        )
        .into(),
    );
}

/// Export the queue: its entries in order, from the first.
pub(crate) fn run(st: &mut State, app: &App) {
    if st.work.exporting.is_some() || st.work.export_choosing || st.work.queue_run.is_some() {
        app.set_status("an export is running; the queue waits for it".into());
        return;
    }
    if st.work.export_queue.is_empty() {
        app.set_status("nothing queued".into());
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
    app.set_status("looking for the queued files...".into());
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
            format!(
                "the queue stopped: {why}; {} left in the queue",
                export_queue::count_line(&st.work.export_queue)
            )
            .into(),
        );
    };
    // An export begun while the files were looked for has the worker.
    if st.work.exporting.is_some() {
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
    let (folder, sub, settings, on_exists, preset) = (
        entry.folder.clone(),
        entry.sheet.subfolder().ok().flatten(),
        entry.sheet.settings(),
        entry.sheet.on_exists(),
        entry.preset.clone(),
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
    let deleted_said = match deleted_names.as_slice() {
        [] => String::new(),
        names => format!(
            "; {} {} deleted; not exported",
            crate::panel::deliver::listed(names),
            if names.len() == 1 { "was" } else { "were" }
        ),
    };
    if frames.is_empty() {
        let said = if missing.is_empty() {
            deleted_said.trim_start_matches("; ").to_string()
        } else {
            format!(
                "{} not there; left queued{deleted_said}",
                crate::panel::deliver::listed(&missing)
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
    let Some(set) =
        crate::panel::deliver::start_set(st, app, frames, folder, sub, settings, on_exists, preset)
    else {
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
        format!(
            "exporting {} from the queue, {} after it...{deleted_said}",
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
    if let Some(run) = st.work.queue_run.as_mut()
        && run.is(set)
        && ran
        && let Some(&i) = run.at.get(index)
        && let Some(d) = run.done.get_mut(i)
    {
        *d = true;
        keep(st);
    }
    if st.work.queue_busy.is_some() && st.work.queue_busy == id_in_run(st, set, index) {
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
            0 => "nothing left in the queue".to_string(),
            _ => format!(
                "{} left in the queue",
                export_queue::count_line(&st.work.export_queue)
            ),
        };
        app.set_status(format!("{line}{behind}; {left}").into());
        return;
    }
    if next >= st.work.export_queue.len() {
        let end = if st.work.export_queue.is_empty() {
            "the queue is done".to_string()
        } else {
            format!(
                "{} left in the queue",
                export_queue::count_line(&st.work.export_queue)
            )
        };
        app.set_status(format!("{line}{behind}; {end}").into());
        return;
    }
    begin_at(st, app, next, run.left_behind, run.whole);
}

/// Clear: every entry and the file. Not while an entry is being
/// exported; while its files are still being looked for, the look is
/// let go of.
pub(crate) fn clear(st: &mut State, app: &App) {
    if st.work.queue_run.as_ref().is_some_and(|r| r.set.is_some()) {
        app.set_status("the queue is running; stop it first".into());
        return;
    }
    let n = st.work.export_queue.len();
    st.work.queue_run = None;
    st.work.export_queue.clear();
    crate::panel::queue_view::changed(st, app);
    keep(st);
    show_queue(st, app);
    app.set_status(format!("export queue cleared ({})", export_queue::sets(n)).into());
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
                    None,
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
            let run = st.work.queue_run.as_ref().expect("the queue runs");
            let f = &st.work.export_queue[run.entry].frames[run.at[index]];
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
                version: None,
                done,
            },
        );
    }

    /// The running set is finished.
    fn finish(app: &App, state: &Rc<RefCell<State>>) {
        let set = state
            .borrow()
            .work
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
            st.views.picked = vec![0, 2];
            st.sidecars[2].current.light.exposure = 1.0;
            st.sidecars[2].turn = 3;
            st.work.export_queue_file = Some(queue_file(&dir));
            st.work.export_presets = vec![ExportPreset {
                name: "Web".into(),
                sheet: web.clone(),
            }];
        }
        app.invoke_export_preset_chosen("Web".into());
        app.set_exposure(0.5);
        // The folder the sheet's row shows, last chosen: no chooser.
        state.borrow_mut().work.export_last_folder = Some(dir.join("out"));
        app.invoke_export_queue_add();

        let st = state.borrow();
        assert_eq!(st.work.export_queue.len(), 1);
        let e = &st.work.export_queue[0];
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
        assert_eq!(export_queue::load(&queue_file(&dir)), st.work.export_queue);
        assert_eq!(app.get_export_queue_sets(), 1);
        assert_eq!(app.get_export_queue_line(), "1 set, 2 frames queued");
        assert_eq!(app.get_status(), "2 frames queued; 1 set in the queue");
        // Nothing was exported.
        assert!(st.work.exporting.is_none() && st.work.queue_run.is_none());
        assert!(!dir.join("out").exists());
        // The panel's edit was made a state, as Export makes it.
        assert_eq!(st.sidecars[0].current.light.exposure, 0.5);
        drop(st);

        // The sheet and the sidecar move on; the entry does not.
        app.set_export_size("1024".into());
        app.invoke_export_sheet_changed();
        state.borrow_mut().sidecars[2].current.light.exposure = -1.0;
        let queued = state.borrow().work.export_queue[0].clone();
        assert_eq!(queued.sheet.size, "2048");
        assert_eq!(queued.frames[1].edit.light.exposure, 1.0);
        assert_eq!(export_queue::load(&queue_file(&dir))[0], queued);

        // A second set: the count says both.
        // None chosen: the frame's own folder, as the row shows it.
        {
            let mut st = state.borrow_mut();
            st.views.picked = vec![0];
            st.work.export_last_folder = None;
        }
        app.invoke_export_queue_add();
        assert_eq!(app.get_export_queue_line(), "2 sets, 3 frames queued");
        assert_eq!(app.get_status(), "1 frame queued; 2 sets in the queue");
        let second = state.borrow().work.export_queue[1].clone();
        assert_eq!(second.sheet.size, "1024");
        assert_eq!(second.preset, None, "an edited sheet names no preset");
        assert_eq!(second.folder.as_deref(), Some(dir.as_path()));

        // With a subfolder on the sheet, the entry goes beside each
        // frame, into it.
        app.set_export_subfolder("export".into());
        app.invoke_export_queue_add();
        let third = state.borrow().work.export_queue[2].clone();
        assert_eq!(third.folder, None);
        assert_eq!(
            third.sheet.subfolder().unwrap().as_deref(),
            Some(Path::new("export"))
        );
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
            st.work.export_queue_file = Some(queue_file(&dir));
            give_edits(&mut st);
            let a = entry(&st, &[0, 1], Some("Web"), "2048");
            let b = entry(&st, &[2], Some("Print"), "Full");
            add(&mut st, &app, a);
            add(&mut st, &app, b);
        }
        app.invoke_export_queue_run();
        {
            let st = state.borrow();
            let set = st.work.exporting.as_ref().expect("the first set runs");
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
            assert_eq!(st.work.export_queue.len(), 1, "the first entry is done");
            assert_eq!(st.work.export_queue[0].preset.as_deref(), Some("Print"));
            assert_eq!(export_queue::load(&queue_file(&dir)), st.work.export_queue);
            let set = st.work.exporting.as_ref().expect("the second set runs");
            assert_eq!(set.total, 1);
            assert_eq!(set.preset.as_deref(), Some("Print"));
            assert_eq!(set.settings.long_edge, None);
            assert_eq!(app.get_export_queue_line(), "1 set, 1 frame queued");
        }
        frame(&app, &state, 0, exported("/nowhere/out/IMG_0002.jpg"));
        finish(&app, &state);
        let st = state.borrow();
        assert!(st.work.export_queue.is_empty());
        assert!(st.work.exporting.is_none() && st.work.queue_run.is_none());
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

    /// The status line counts across the whole run, not within each
    /// entry's set: two entries of two and one frames say "3 of 3" on
    /// the last, and the finished line counts all three.
    #[test]
    fn the_progress_counts_across_the_whole_run() {
        let (dir, files) = shoot("whole", 3);
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.work.export_queue_file = Some(queue_file(&dir));
            give_edits(&mut st);
            let a = entry(&st, &[0, 1], Some("Web"), "2048");
            let b = entry(&st, &[2], Some("Web"), "2048");
            add(&mut st, &app, a);
            add(&mut st, &app, b);
        }
        app.invoke_export_queue_run();
        let started = |index: usize, name: &str| {
            let set = state.borrow().work.exporting.clone().unwrap();
            crate::panel::deliver::deliver(
                &app,
                Outcome::SetFrameStarted {
                    set,
                    index,
                    name: name.to_string(),
                },
            );
            app.get_status().to_string()
        };
        assert_eq!(started(0, "IMG_0000.jpg"), "exporting 1 of 3: IMG_0000.jpg");
        frame(&app, &state, 0, exported("/nowhere/out/IMG_0000.jpg"));
        assert_eq!(started(1, "IMG_0001.jpg"), "exporting 2 of 3: IMG_0001.jpg");
        frame(&app, &state, 1, exported("/nowhere/out/IMG_0001.jpg"));
        finish(&app, &state);
        assert_eq!(started(0, "IMG_0002.jpg"), "exporting 3 of 3: IMG_0002.jpg");
        frame(&app, &state, 0, exported("/nowhere/out/IMG_0002.jpg"));
        finish(&app, &state);
        // The finished line is the whole run's: the worker's own threads
        // fail these empty files at their pace, so only its shape is
        // fixed here, and the sum is checked below.
        let status = app.get_status().to_string();
        assert!(
            status.starts_with("exported ") && status.ends_with("; the queue is done"),
            "{status}"
        );
        let mut whole = Whole::new();
        let one = |n| queue::Tally {
            exported: n,
            failed: vec![("x".into(), "why".into())],
            ..queue::Tally::default()
        };
        let set = |n| queue::Set::new(n, None, Default::default(), Default::default());
        whole.add(&set(2), &one(1));
        whole.add(&set(1), &one(1));
        assert_eq!(
            (whole.before, whole.tally.exported, whole.tally.failed.len()),
            (3, 2, 2)
        );
        assert_eq!(whole.places, ["beside their files"]);
        drop(state);
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
            st.work.export_queue_file = Some(queue_file(&dir));
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
        assert!(st.work.exporting.is_none() && st.work.queue_run.is_none());
        assert_eq!(st.work.export_queue.len(), 2);
        let left: Vec<&Path> = st.work.export_queue[0]
            .frames
            .iter()
            .map(|f| f.source.as_path())
            .collect();
        assert_eq!(left, vec![files[1].as_path(), files[2].as_path()]);
        assert_eq!(st.work.export_queue[1].frames.len(), 1);
        assert_eq!(export_queue::load(&queue_file(&dir)), st.work.export_queue);
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
        assert_eq!(
            st.work.export_queue.len(),
            1,
            "the failed frame's entry is done"
        );
        assert!(st.work.queue_run.is_some(), "and the next entry runs");
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
            assert!(st.work.export_queue.is_empty());
            give_edits(&mut st);
            let a = entry(&st, &[0], Some("Web"), "2048");
            let b = entry(&st, &[0, 1], None, "Full");
            add(&mut st, &app, a);
            add(&mut st, &app, b);
            st.work.export_queue.clone()
        };
        let app = crate::testing::window(2);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        open_at_launch(&mut state.borrow_mut(), &app, Some(&settings), true);
        let st = state.borrow();
        assert_eq!(st.work.export_queue, queued);
        assert_eq!(st.work.export_queue[1].frames[1].edit.light.exposure, 0.5);
        assert_eq!(app.get_export_queue_sets(), 2);
        assert_eq!(app.get_export_queue_line(), "2 sets, 3 frames queued");
        assert!(st.work.exporting.is_none() && st.work.queue_run.is_none());
        assert!(!app.get_export_running());
        drop(st);
        // A run that may not write (a snapshot) reads it and leaves it.
        let app = crate::testing::window(2);
        let (state, _worker) = crate::testing::state_for(&app, files);
        open_at_launch(&mut state.borrow_mut(), &app, Some(&settings), false);
        assert_eq!(state.borrow().work.export_queue, queued);
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
            st.work.export_queue_file = Some(queue_file(&dir));
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
            assert_eq!(st.work.export_queue.len(), 2, "nothing is dropped");
            let run = st.work.queue_run.as_ref().expect("the next entry runs");
            assert_eq!(run.entry, 1);
            assert_eq!(run.set.as_ref().unwrap().total, 1);
            assert_eq!(run.at, vec![1]);
        }
        frame(&app, &state, 0, exported("/nowhere/out/IMG_0002.jpg"));
        finish(&app, &state);
        let st = state.borrow();
        assert!(st.work.queue_run.is_none());
        let left: Vec<Vec<&Path>> = st
            .work
            .export_queue
            .iter()
            .map(|e| e.frames.iter().map(|f| f.source.as_path()).collect())
            .collect();
        assert_eq!(
            left,
            vec![vec![files[0].as_path()], vec![files[1].as_path()]]
        );
        assert_eq!(export_queue::load(&queue_file(&dir)), st.work.export_queue);
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
            assert!(st.work.queue_run.is_none() && st.work.exporting.is_none());
            assert_eq!(st.work.export_queue[0].frames.len(), 2);
        }
        let status = app.get_status().to_string();
        assert!(status.starts_with("the queue stopped: "), "{status}");
        assert!(status.contains("unplugged is not there"), "{status}");

        // The folder back, a mark whose PNG is gone.
        std::fs::create_dir_all(dir.join("unplugged")).unwrap();
        {
            let mut st = state.borrow_mut();
            st.work.export_queue[0].sheet.mark = crate::sheet::MARK_IMAGE.into();
            st.work.export_queue[0].sheet.mark_image =
                dir.join("logo.png").to_string_lossy().into();
        }
        app.invoke_export_queue_run();
        assert!(state.borrow().work.queue_run.is_none());
        assert_eq!(state.borrow().work.export_queue[0].frames.len(), 2);
        let status = app.get_status().to_string();
        assert!(status.contains("the watermark's PNG"), "{status}");

        // Both there: it runs.
        image::RgbaImage::new(4, 4)
            .save(dir.join("logo.png"))
            .unwrap();
        app.invoke_export_queue_run();
        assert!(
            state
                .borrow()
                .work
                .queue_run
                .as_ref()
                .unwrap()
                .set
                .is_some()
        );
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
            st.work.export_queue_file = Some(queue_file(&dir));
            give_edits(&mut st);
            let a = entry(&st, &[0, 1, 2], None, "2048");
            let b = entry(&st, &[0], None, "Full");
            add(&mut st, &app, a);
            add(&mut st, &app, b);
        }
        app.invoke_export_queue_run();
        frame(&app, &state, 0, exported("/nowhere/out/IMG_0000.jpg"));
        // No SetDone: the window goes away here.
        assert_eq!(state.borrow().work.export_queue[0].frames.len(), 3);
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
        assert_eq!(
            state.borrow().work.queue_run.as_ref().unwrap().sent,
            vec![rel]
        );
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
        app.invoke_export_queue_add();
        assert!(state.borrow().work.export_queue.is_empty());
        assert!(app.get_export_open());
        assert!(
            app.get_status().starts_with("not queued: "),
            "{}",
            app.get_status()
        );
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
                version: None,
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
        let run = st
            .work
            .queue_run
            .as_ref()
            .expect("the queue's entry still runs");
        assert!(run.done.iter().all(|d| !d));
        assert!(st.work.exporting.is_some());
        assert_eq!(st.work.export_queue[0].frames.len(), 2);
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
            assert!(crate::rows::wants_seed(&st, 0), "a raw never developed");
            let mut e = entry(&st, &[0], Some("Web"), "2048");
            e.frames[0].seed_blend = true;
            let queued = e.frames[0].edit.clone();
            add(&mut st, &app, e);
            queued
        };
        app.invoke_export_queue_run();
        let (set, source) = {
            let st = state.borrow();
            let run = st.work.queue_run.as_ref().unwrap();
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
                version: None,
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
        assert!(!crate::rows::wants_seed(&st, 0));
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
            st.work.export_queue_file = Some(queue_file(&dir));
            let a = entry(&st, &[0, 1], None, "2048");
            let b = entry(&st, &[1], None, "2048");
            add(&mut st, &app, a);
            add(&mut st, &app, b);
        }
        assert!(queue_file(&dir).exists());
        app.invoke_export_queue_run();
        app.invoke_export_queue_clear();
        assert_eq!(
            state.borrow().work.export_queue.len(),
            2,
            "not while it runs"
        );
        app.invoke_export_stop();
        frame(&app, &state, 0, queue::Done::Canceled);
        frame(&app, &state, 1, queue::Done::Canceled);
        finish(&app, &state);
        assert_eq!(state.borrow().work.export_queue.len(), 2);
        app.invoke_export_queue_clear();
        assert!(state.borrow().work.export_queue.is_empty());
        assert!(!queue_file(&dir).exists());
        assert_eq!(app.get_export_queue_sets(), 0);
        assert_eq!(app.get_status(), "export queue cleared (2 sets)");
        crate::testing::remove_dir_retry(&dir);
    }

    /// A sidecar written by a later build (its edits at a schema this
    /// build does not read) is Unsure, not Deleted: the frame stays
    /// queued for a build that reads it.
    #[test]
    fn a_later_build_s_sidecar_is_unsure() {
        let (dir, files) = shoot("later-build-sidecar", 1);
        let mut s = greycard_edit::Sidecar::default();
        let id = s.new_version(None, Some("Warm")).unwrap();
        s.save(&files[0]).unwrap();
        let gcd = greycard_edit::Sidecar::find(&files[0]).unwrap();
        let text = std::fs::read_to_string(&gcd).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["current"]["version"] = serde_json::json!(99);
        v["versions"][0]["line"]["current"]["version"] = serde_json::json!(99);
        std::fs::write(&gcd, serde_json::to_vec(&v).unwrap()).unwrap();
        let p = version_presence(&files[0], &id);
        crate::testing::remove_dir_retry(&dir);
        assert_eq!(p, Presence::Unsure);
    }

    /// A sidecar there but unreadable (a later build's, a corrupt file,
    /// a share that cuts the read short) is not a version deleted: the
    /// look cannot tell, and the frame is left queued; nor is a frame
    /// whose sidecar is gone altogether. Only a sidecar that reads and
    /// lacks the version says deleted.
    #[test]
    fn an_unreadable_sidecar_is_not_a_deleted_version() {
        let (dir, files) = shoot("unreadable-version", 1);
        let mut s = greycard_edit::Sidecar::default();
        let id = s.new_version(None, Some("Warm")).unwrap();
        s.save(&files[0]).unwrap();
        assert_eq!(version_presence(&files[0], &id), Presence::There);
        assert_eq!(version_presence(&files[0], "none"), Presence::Deleted);
        let gcd = greycard_edit::Sidecar::find(&files[0]).unwrap();
        std::fs::write(&gcd, b"{ not json").unwrap();
        assert_eq!(version_presence(&files[0], &id), Presence::Unsure);
        std::fs::remove_file(&gcd).unwrap();
        assert_eq!(version_presence(&files[0], &id), Presence::Unsure);
        // Through the run: the frame is left queued and said not there,
        // not dropped from the entry.
        std::fs::write(&gcd, b"{ not json").unwrap();
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            let mut e = entry(&st, &[], Some("Web"), "2048");
            e.frames.push(crate::export_queue::QueuedFrame {
                source: files[0].clone(),
                edit: Edit::default(),
                turn: 0,
                seed_blend: false,
                version: Some(queue::Version {
                    id: id.clone(),
                    name: "Warm".into(),
                }),
                id: export_queue::fresh_id(),
            });
            add(&mut st, &app, e);
        }
        app.invoke_export_queue_run();
        let st = state.borrow();
        assert_eq!(st.work.export_queue.len(), 1, "left queued");
        assert_eq!(st.work.export_queue[0].frames.len(), 1);
        assert!(st.work.queue_run.is_none());
        assert!(
            app.get_status().contains("not there; left queued"),
            "{}",
            app.get_status()
        );
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// An entry whose every frame was a version deleted before the run
    /// goes from the queue with them, and the run goes on to the next
    /// entry in its place.
    #[test]
    fn an_entry_of_deleted_versions_alone_leaves_the_queue() {
        let (dir, files) = shoot("deleted-versions-entry", 1);
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            let mut e = entry(&st, &[0], Some("Web"), "2048");
            e.frames[0].version = Some(queue::Version {
                id: "0123456789abcdef".into(),
                name: "Warm".into(),
            });
            add(&mut st, &app, e);
            let e = entry(&st, &[0], Some("Web"), "2048");
            add(&mut st, &app, e);
        }
        app.invoke_export_queue_run();
        let st = state.borrow();
        assert_eq!(st.work.export_queue.len(), 1, "the emptied entry went");
        assert_eq!(st.work.export_queue[0].frames[0].version, None);
        let run = st
            .work
            .queue_run
            .as_ref()
            .expect("the run went on to the next entry");
        assert_eq!(run.entry, 0, "which is at the first place now");
        assert_eq!(run.sent.len(), 1);
        drop(st);
        finish(&app, &state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// An entry queued for a version that was deleted before the run is
    /// left out of the run and said, rather than exported from the edit
    /// the entry stored and recorded nowhere; the file's own frame beside
    /// it goes out.
    #[test]
    fn a_queued_version_deleted_before_the_run_is_left_out_and_said() {
        let (dir, files) = shoot("deleted-version", 1);
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            let mut e = entry(&st, &[0, 0], Some("Web"), "2048");
            e.frames[1].version = Some(queue::Version {
                id: "0123456789abcdef".into(),
                name: "Warm".into(),
            });
            add(&mut st, &app, e);
        }
        app.invoke_export_queue_run();
        let st = state.borrow();
        let run = st
            .work
            .queue_run
            .as_ref()
            .expect("the run began on the file's own");
        assert_eq!(run.sent.len(), 1, "the file's own frame alone");
        assert_eq!(
            st.work.export_queue[0].frames.len(),
            1,
            "the version's out of the entry"
        );
        assert!(
            app.get_status()
                .contains("IMG_0000 (Warm) was deleted; not exported"),
            "{}",
            app.get_status()
        );
        drop(st);
        finish(&app, &state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The sheet's rows, as (name, frames, folder, running).
    fn rows(app: &App) -> Vec<(String, String, String, bool)> {
        app.get_export_queue_rows()
            .iter()
            .map(|r| {
                (
                    r.name.to_string(),
                    r.frames.to_string(),
                    r.folder.to_string(),
                    r.running,
                )
            })
            .collect()
    }

    /// The presets of the entries in the file, in order.
    fn presets_on_disk(dir: &Path) -> Vec<Option<String>> {
        export_queue::load(&queue_file(dir))
            .into_iter()
            .map(|e| e.preset)
            .collect()
    }

    /// The sheet lists the sets as the file holds them: the preset or
    /// the format when there is none, the frames, and the folder with
    /// home shortened, or the subfolder beside each file.
    #[test]
    fn the_rows_mirror_the_file() {
        let (dir, files) = shoot("rows", 3);
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        let home = dirs::home_dir().unwrap_or_default().join("out");
        {
            let mut st = state.borrow_mut();
            st.work.export_queue_file = Some(queue_file(&dir));
            let a = Entry {
                folder: Some(home.clone()),
                ..entry(&st, &[0, 1], Some("Web"), "2048")
            };
            let mut b = entry(&st, &[2], None, "Full");
            b.sheet.subfolder = "export".into();
            add(&mut st, &app, a);
            add(&mut st, &app, b);
        }
        let on_disk = export_queue::load(&queue_file(&dir));
        assert_eq!(on_disk.len(), 2);
        assert_eq!(
            rows(&app),
            vec![
                (
                    "Web".into(),
                    "2 frames".into(),
                    crate::report::path_shown(&home),
                    false
                ),
                ("JPEG".into(), "1 frame".into(), "export".into(), false),
            ]
        );
        if cfg!(unix) {
            assert_eq!(rows(&app)[0].2, "~/out");
        }
        crate::testing::remove_dir_retry(&dir);
    }

    /// A set's bin takes it out of the sheet and the file at once, the
    /// others left in their order; the last one out leaves no file.
    #[test]
    fn removing_the_middle_set_leaves_the_others_in_order() {
        let (dir, files) = shoot("remove", 3);
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.work.export_queue_file = Some(queue_file(&dir));
            for (i, name) in ["A", "B", "C"].into_iter().enumerate() {
                let e = entry(&st, &[i], Some(name), "2048");
                add(&mut st, &app, e);
            }
        }
        app.invoke_export_queue_remove(1);
        let names: Vec<String> = rows(&app).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["A", "C"]);
        assert_eq!(
            presets_on_disk(&dir),
            [Some("A".to_string()), Some("C".to_string())]
        );
        assert_eq!(app.get_export_queue_line(), "2 sets, 2 frames queued");
        // Out of range is nothing.
        app.invoke_export_queue_remove(7);
        app.invoke_export_queue_remove(-1);
        assert_eq!(rows(&app).len(), 2);
        app.invoke_export_queue_remove(0);
        app.invoke_export_queue_remove(0);
        assert!(rows(&app).is_empty());
        assert!(!queue_file(&dir).exists());
        crate::testing::remove_dir_retry(&dir);
    }

    /// While the queue runs, the set it is on is marked and cannot be
    /// removed; a set after it removed is not begun, and one before it
    /// (left queued, its file not there) moves the run up with it, so
    /// the run goes on to the right set.
    #[test]
    fn a_set_removed_during_a_run_is_skipped() {
        let (dir, files) = shoot("remove-run", 4);
        let app = crate::testing::window(4);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.work.export_queue_file = Some(queue_file(&dir));
            give_edits(&mut st);
            for (i, name) in ["A", "B", "C", "D"].into_iter().enumerate() {
                let e = entry(&st, &[i], Some(name), "2048");
                add(&mut st, &app, e);
            }
        }
        // A's file is gone: it stays queued and the run is on B.
        std::fs::remove_file(&files[0]).unwrap();
        app.invoke_export_queue_run();
        let running = |app: &App| -> Vec<bool> { rows(app).into_iter().map(|r| r.3).collect() };
        assert_eq!(running(&app), [false, true, false, false]);
        // B is the run's: its bin does nothing.
        app.invoke_export_queue_remove(1);
        assert_eq!(rows(&app).len(), 4);
        // C, after it, and A, before it.
        app.invoke_export_queue_remove(2);
        app.invoke_export_queue_remove(0);
        assert_eq!(running(&app), [true, false]);
        assert_eq!(
            presets_on_disk(&dir),
            [Some("B".to_string()), Some("D".to_string())]
        );
        assert_eq!(state.borrow().work.queue_run.as_ref().unwrap().entry, 0);
        frame(&app, &state, 0, exported("/nowhere/out/IMG_0001.jpg"));
        finish(&app, &state);
        {
            let st = state.borrow();
            let set = st.work.exporting.as_ref().expect("the next set runs");
            assert_eq!(set.preset.as_deref(), Some("D"), "C is not begun");
        }
        assert_eq!(rows(&app).len(), 1);
        assert_eq!(running(&app), [true]);
        frame(&app, &state, 0, exported("/nowhere/out/IMG_0003.jpg"));
        finish(&app, &state);
        let st = state.borrow();
        assert!(st.work.export_queue.is_empty() && st.work.queue_run.is_none());
        assert!(
            st.sidecars[2].current_exports.is_empty(),
            "C was not exported"
        );
        assert!(!queue_file(&dir).exists());
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// Past five rows the list scrolls in its own box: the sheet is as
    /// tall with nine sets as with five, and taller than with one.
    #[test]
    fn the_sheet_does_not_grow_past_five_rows() {
        use i_slint_backend_testing::ElementHandle;
        let (dir, files) = shoot("rows-tall", 9);
        let app = crate::testing::window(9);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 1400.0));
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        app.invoke_select(0);
        app.set_export_open(true);
        let height = |app: &App| {
            app.window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(0.0, 0.0),
                });
            ElementHandle::find_by_element_id(app, "ExportSheet::card")
                .next()
                .expect("the sheet")
                .size()
                .height
        };
        let mut seen = Vec::new();
        for i in 0..9 {
            {
                let mut st = state.borrow_mut();
                let e = entry(&st, &[i], Some("Web"), "2048");
                add(&mut st, &app, e);
            }
            seen.push(height(&app));
        }
        assert!(seen[0] < seen[4], "{seen:?}");
        assert!(seen[3] < seen[4], "{seen:?}");
        for h in &seen[5..] {
            assert_eq!(*h, seen[4], "{seen:?}");
        }
        // Well inside the window: the room was there to grow.
        assert!(seen[4] < 1400.0 - 200.0, "{seen:?}");
        crate::testing::remove_dir_retry(&dir);
    }
}
