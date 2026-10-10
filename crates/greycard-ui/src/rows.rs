//! The index's rows standing in for sidecars.
//!
//! A frame in the browser's list has a `Sidecar` in `State::sidecars`
//! from the moment it is listed. Where the list came from the index (a
//! view of the roots, a folder the index knows) that sidecar was not
//! read from disk: it is a stand-in built from the frame's row, which
//! mirrors the sidecar's meta, whether it holds a develop and which way
//! up its picture is shown (`greycard_library::RowMeta`). That is all
//! the grid, the strip, the badges and the filter ask of a frame, so a
//! view of ten thousand frames opens without a sidecar read, and a
//! frame under a root that is offline is listed like any other.
//!
//! The sidecar itself is read the first time something needs the whole
//! of it — the edit, the history, the snapshots, the turn, the XMP mark
//! — through [`load_frame`] or [`load_frames`], and from then on the
//! copy in memory is the one that counts, as it always was. The rule
//! for the code that reaches into `st.sidecars`:
//!
//! - `.meta` may be read from any frame's sidecar, stand-in or not.
//! - The turns a picture is shown at come from the row while the frame
//!   stands in (`FromRow::turns`): `thumb_turns`, `thumb_turns_of`.
//! - Anything else (`.current`, `.history`, `.turn`, `.xmp`, the
//!   snapshots, a `record`) wants the frame loaded first. Every action
//!   that wants it is a [`request`]: the paths it is pressed on and
//!   what to do with them, run when their sidecars are in, on exactly
//!   those paths, in the order the requests were made. Opening a frame
//!   is one (the frame is current from the pick, its panel read-only
//!   until its sidecar lands); so are a key, a turn, a sync, a paste,
//!   a preset and an export over a selection. A request never cancels
//!   another. The frame on screen is loaded, unless its root is
//!   offline or its sidecar is still on its way (`State::picking`).
//! - A sidecar that is standing in is never written
//!   (`panel::edit::writable`): a save of a stand-in would put the
//!   default edit over the frame's own.
//! - A frame under a root that is offline cannot be loaded. A key, a
//!   turn, a sync, a paste, a preset, an export, a delete or the
//!   rejects' move leaves it out and says so; nothing runs from it as
//!   a source; nothing is written under its root.
//!
//! The stand-in is as right as the row, which is as right as the last
//! pass over the folder or the last save indexed. A pass that finds the
//! sidecar changed rewrites the row, and the window takes the new meta
//! into every frame still standing in ([`apply_row`]); a frame already
//! loaded keeps what memory holds, as a merge always did.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use greycard_edit::{Edit, Sidecar};
use greycard_library::RowMeta;

use crate::panel::browser::{file_name, load_sidecar_said};
use slint::ComponentHandle;

use crate::roots::Progress;
use crate::worker::{Job, Worker};
use crate::{App, State};

/// What a frame's row stands in for while its sidecar has not been
/// read, kept beside the sidecar in `State::from_row`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FromRow {
    /// The sidecar was read from disk, or there is none to read
    /// (`--no-sidecars`): the row stands in for nothing, and the
    /// sidecar in memory is the whole of it.
    pub(crate) read: bool,
    /// The quarter turns and the mirror the row says the picture is
    /// shown at, used while the sidecar stands in; once read, the
    /// sidecar's own edit says.
    pub(crate) turns: (u8, bool),
    /// The file's content hash and its mtime stamp, from the row: the
    /// key its thumbnails are kept under in the cache, for a frame
    /// whose file cannot be read (its root offline).
    pub(crate) key: Option<(String, u64)>,
    /// The row says the sidecar holds a develop: a read that finds no
    /// sidecar then is not believed over it.
    pub(crate) edited: bool,
    /// The row says the sidecar records an export, for the mark on the
    /// frame's cell while the sidecar stands in.
    pub(crate) exported: bool,
    /// The frame's ISO as the row had it (`Some(None)`: the camera
    /// gave none), kept past the read; `None` for a frame never
    /// stood in for. What a learned blend is judged against.
    pub(crate) iso: Option<Option<u32>>,
    /// The hash of the sidecar the row was read from, `None` for none:
    /// a row that comes in with another says the sidecar changed under
    /// the frame (`edited::changed`).
    pub(crate) sidecar: Option<String>,
    /// The last read of the sidecar did not take, and why: it could not
    /// be read, or it was not there though the row says it holds a
    /// develop. The frame stands in still, and nothing is written over
    /// the file; the next request for it reads it again.
    pub(crate) unread: Option<String>,
}

impl FromRow {
    /// A frame whose sidecar has been read, or has nothing to read.
    pub(crate) fn read() -> Self {
        FromRow {
            read: true,
            turns: (0, false),
            key: None,
            edited: false,
            exported: false,
            iso: None,
            sidecar: None,
            unread: None,
        }
    }

    /// A frame standing in from `row`.
    pub(crate) fn of(row: &RowMeta) -> Self {
        FromRow {
            read: false,
            turns: row.shown_turns(),
            key: Some((row.hash.clone(), row.mtime.max(0) as u64)),
            edited: row.edited,
            exported: row.exported,
            iso: Some(row.iso),
            sidecar: row.sidecar_hash.clone(),
            unread: None,
        }
    }
}

/// What went wrong reading a sidecar from disk, the default edit
/// standing in for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Trouble {
    /// No `.gcd` beside the frame.
    Missing,
    /// There is one, and it could not be read: the error.
    Unreadable(String),
}

impl Default for FromRow {
    fn default() -> Self {
        Self::read()
    }
}

/// A frame's sidecar as the browser first holds it, with what a merge
/// carries beside it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Held {
    pub(crate) sidecar: Sidecar,
    /// The frame is a raw to seed the learned-denoiser blend of from
    /// its ISO on its first open. A disk read decides it by the
    /// sidecar itself (`files::never_developed`: the default edit, no
    /// history, no snapshot); from a row it is only a first guess
    /// (`!row.edited`, which leaves a seeded blend and a history out of
    /// it), and the read that lands replaces it.
    pub(crate) seed: bool,
    pub(crate) from_row: FromRow,
    /// The frame's row in the index, when the read that brought this
    /// asked the index and it had one.
    pub(crate) id: Option<i64>,
    /// Read from disk, and the `.gcd` was not there or could not be
    /// read: the default stands in the sidecar. Over a frame standing
    /// in from its row, that is not taken as the frame's sidecar
    /// ([`take`]).
    pub(crate) trouble: Option<Trouble>,
}

/// The frames whose sidecars were read from disk since the process
/// started, for the tests that say a view of the roots reads none:
/// by path, since the tests run beside each other.
#[cfg(test)]
pub(crate) static DISK_READS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// A stand-in for `path`'s sidecar from its row: the row's meta, the
/// frame's default edit, nothing read. With sidecars off there is
/// nothing to stand in for, and the frame is held as read.
pub(crate) fn from_row(path: &Path, row: &RowMeta, write_sidecars: bool) -> Held {
    if !write_sidecars {
        return from_nothing();
    }
    let raw = !greycard_core::picture::is_picture_path(path);
    Held {
        sidecar: Sidecar {
            current: if raw {
                Edit::default()
            } else {
                Edit::for_picture()
            },
            meta: row.meta.clone(),
            ..Sidecar::default()
        },
        seed: raw && !row.edited,
        from_row: FromRow::of(row),
        id: Some(row.id),
        trouble: None,
    }
}

/// `path`'s sidecar read from disk, as a folder's open always read it.
pub(crate) fn from_disk(path: &Path, write_sidecars: bool) -> Held {
    if !write_sidecars {
        return from_nothing();
    }
    #[cfg(test)]
    DISK_READS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(path.to_path_buf());
    let (sidecar, seed, trouble) = load_sidecar_said(path, true);
    Held {
        sidecar,
        seed,
        from_row: FromRow::read(),
        id: None,
        trouble,
    }
}

/// A frame with sidecars off: the default, and nothing to read.
fn from_nothing() -> Held {
    Held {
        sidecar: Sidecar::default(),
        seed: false,
        from_row: FromRow::read(),
        id: None,
        trouble: None,
    }
}

/// Each of `files` held: from its row where the index has one, else
/// read from disk on the rayon pool. `read`, when given, is bumped for
/// each sidecar read from disk, for the window's bar. The order is the
/// list's.
pub(crate) fn bring(
    files: &[PathBuf],
    rows: &[Option<RowMeta>],
    write_sidecars: bool,
    read: Option<&AtomicUsize>,
) -> Vec<Held> {
    use rayon::prelude::*;
    debug_assert_eq!(files.len(), rows.len());
    files
        .par_iter()
        .zip(rows.par_iter())
        .map(|(path, row)| match row {
            Some(row) => from_row(path, row, write_sidecars),
            None => {
                let held = from_disk(path, write_sidecars);
                if let Some(read) = read {
                    read.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                held
            }
        })
        .collect()
}

/// How many of `rows` have no row, which is how many sidecars
/// [`bring`] reads from disk.
pub(crate) fn to_read(rows: &[Option<RowMeta>]) -> usize {
    rows.iter().filter(|r| r.is_none()).count()
}

/// The root file `i` is under that is offline, if any.
pub(crate) fn offline_root(st: &State, i: usize) -> Option<PathBuf> {
    let path = st.files.get(i)?;
    st.library
        .offline
        .iter()
        .find(|r| path.starts_with(r))
        .cloned()
}

/// Whether file `i` is under a root that is offline.
pub(crate) fn is_offline(st: &State, i: usize) -> bool {
    !st.library.offline.is_empty() && offline_root(st, i).is_some()
}

/// The status line's word for a frame that cannot be reached, and what
/// that costs (`", nothing to sync from"`), or nothing more.
fn say_offline(st: &State, app: &App, root: &Path, what: &str) {
    let name = crate::roots::root_name(&st.library.roots, root);
    app.set_status(format!("{name} is offline{what}").into());
}

/// Say that frame `i`'s root is offline, and what that costs. For a
/// frame under no offline root nothing is said.
pub(crate) fn say_offline_for(st: &State, app: &App, i: usize, what: &str) {
    if let Some(root) = offline_root(st, i) {
        say_offline(st, app, &root, what);
    }
}

/// Whether frame `i`'s whole sidecar is in memory: read from disk, or
/// nothing to read. False while its row stands in.
pub(crate) fn is_loaded(st: &State, i: usize) -> bool {
    st.from_row.get(i).is_none_or(|r| r.read)
}

/// Whether the panel holds frame `i`'s own edit, so what it does may be
/// recorded on the frame: the frame is loaded, and the panel is not
/// still the stand-in's it showed while the sidecar was on its way.
pub(crate) fn panel_is_frames(st: &State, i: usize) -> bool {
    is_loaded(st, i)
        && (st.panel_stand_in.is_none()
            || st.panel_stand_in.as_deref() != st.files.get(i).map(PathBuf::as_path))
}

/// Why nothing the panel does over frame `i` may be recorded, when its
/// sidecar still stands in: its root is offline, its read is still
/// out (the pick pending), or the read was given up on. A change
/// recorded on the stand-in could not be written (`writable`), and the
/// read's landing would put the whole sidecar over it; so it is
/// refused, and this is what the status line says. None for a frame
/// loaded.
pub(crate) fn not_kept(st: &State, i: usize) -> Option<String> {
    refused(st, i, "the change is not kept")
}

/// [`not_kept`]'s reason, ending in `what` was refused ("nothing is
/// exported") rather than a change.
pub(crate) fn refused(st: &State, i: usize, what: &str) -> Option<String> {
    if panel_is_frames(st, i) {
        return None;
    }
    if is_loaded(st, i) {
        let name = file_name(st.files.get(i)?);
        return Some(format!("the panel is not {name}'s own yet: {what}"));
    }
    if let Some(root) = offline_root(st, i) {
        let name = crate::roots::root_name(&st.library.roots, &root);
        return Some(format!("{name} is offline: {what}"));
    }
    let name = file_name(st.files.get(i)?);
    Some(if st.from_row.get(i).is_some_and(|r| r.unread.is_some()) {
        format!("{name}'s sidecar could not be read: {what}")
    } else if st.pick_pending.as_deref() == st.files.get(i).map(PathBuf::as_path) {
        format!("{name}'s sidecar is still being read: {what}")
    } else {
        format!("{name}'s sidecar did not come in: {what}")
    })
}

/// What runs on the window's thread once a request's sidecars are in:
/// handed, for each path the request named, the frame it is at now,
/// or `None` for one no longer listed. Never the selection as it is by
/// then: what was pressed on is what is acted on.
pub(crate) type Action =
    Box<dyn FnOnce(&Rc<RefCell<State>>, &App, &Rc<Worker>, Vec<Option<usize>>)>;

/// A request for some frames' sidecars, with what to do once they are
/// in. Requests run in the order they were made, each on exactly the
/// paths it named; a later one never cancels an earlier one.
pub(crate) struct Pending {
    id: u64,
    paths: Vec<PathBuf>,
    action: Option<Action>,
    /// What the action is, for the status line when it is dropped or a
    /// frame it named is gone: "rating key", "turn", "open of x.cr3".
    what: String,
    /// The request is a frame's pick (`open_row`): dropped, it lets the
    /// pick go.
    pick: bool,
    /// The read is in, or there was nothing to read: ready to run once
    /// every request before it has.
    landed: bool,
}

/// The dropped requests, said: "3 rating keys and the open of x.cr3".
fn describe(dropped: &[Pending]) -> String {
    let mut kinds: Vec<(&str, usize, bool)> = Vec::new();
    for p in dropped {
        match kinds.iter_mut().find(|(w, _, _)| *w == p.what) {
            Some((_, n, _)) => *n += 1,
            None => kinds.push((&p.what, 1, p.pick)),
        }
    }
    let words: Vec<String> = kinds
        .iter()
        .map(|(what, n, pick)| match (n, pick) {
            (_, true) => format!("the {what}"),
            (1, false) => format!("the {what}"),
            (n, false) => format!("{n} {what}s"),
        })
        .collect();
    match words.len() {
        0 => String::new(),
        1 => words[0].clone(),
        n => format!("{} and {}", words[..n - 1].join(", "), words[n - 1]),
    }
}

/// The requests out, in order, and the bar over their reads: state of
/// its own, beside the roots view's read and its bar (`roots::Library`),
/// which a frames' read never touches.
#[derive(Default)]
pub(crate) struct Loads {
    pub(crate) queue: VecDeque<Pending>,
    next: u64,
    /// Every read out on its thread, by its request's id, with the
    /// paths it reads: a request dropped from the queue still lands its
    /// read, so this, not the queue, is what is in flight.
    pub(crate) in_flight: std::collections::HashMap<u64, Vec<PathBuf>>,
    /// How many sidecars the reads out cover, and how many are in.
    pub(crate) progress: Option<Arc<Progress>>,
    /// Since when a read has been out.
    pub(crate) since: Option<Instant>,
    /// The count read and its total as the timer last saw them, when
    /// the count last moved (for the give-up), and when either did: a
    /// request joining a read out adds to the total, and is not
    /// "Waiting" from its first tick.
    seen: (usize, usize),
    moved: Option<Instant>,
    counted: Option<Instant>,
    pub(crate) timer: slint::Timer,
}

impl Loads {
    /// Whether any request is still waiting on its read.
    pub(crate) fn reading(&self) -> bool {
        self.queue.iter().any(|p| !p.landed)
    }

    /// What is queued, in order, for the tests.
    #[cfg(test)]
    pub(crate) fn whats(&self) -> Vec<String> {
        self.queue.iter().map(|p| p.what.clone()).collect()
    }
}

/// How long a frames' read may be out, its count not moving, before
/// it is given up on: the roots view's two limits (§192).
const FRAMES_GIVE_UP: Duration = Duration::from_secs(60);
const FRAMES_STALLED: Duration = Duration::from_secs(10);

/// Where each of `paths` is in the list now.
fn resolve(st: &State, paths: &[PathBuf]) -> Vec<Option<usize>> {
    let at: HashMap<&Path, usize> = st
        .files
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_path(), i))
        .collect();
    paths.iter().map(|p| at.get(p.as_path()).copied()).collect()
}

/// What a request left to do after it was queued.
enum Queued {
    /// Nothing to read and nothing ahead: the action runs now.
    Inline(Action, Vec<PathBuf>),
    /// Queued; these paths are to be read for it (none when an earlier
    /// request is reading them, or there is nothing to read).
    Later { id: u64, to_read: Vec<PathBuf> },
}

/// Queue a request for `paths`' sidecars with `action`, from the
/// window's state: which of them are to be read (standing in, under no
/// offline root, and not already being read for an earlier request).
fn queue(st: &mut State, paths: Vec<PathBuf>, what: &str, pick: bool, action: Action) -> Queued {
    let listed = resolve(st, &paths);
    let reading: HashSet<&Path> = st
        .loads
        .queue
        .iter()
        .filter(|p| !p.landed)
        .flat_map(|p| p.paths.iter().map(PathBuf::as_path))
        .collect();
    let to_read: Vec<PathBuf> = paths
        .iter()
        .zip(&listed)
        .filter(|(p, i)| {
            i.is_some_and(|i| !is_loaded(st, i) && !is_offline(st, i))
                && !reading.contains(p.as_path())
        })
        .map(|(p, _)| p.clone())
        .collect();
    if to_read.is_empty() && st.loads.queue.is_empty() {
        return Queued::Inline(action, paths);
    }
    st.loads.next += 1;
    let id = st.loads.next;
    let landed = to_read.is_empty();
    st.loads.queue.push_back(Pending {
        id,
        paths,
        action: Some(action),
        what: what.to_string(),
        pick,
        landed,
    });
    Queued::Later { id, to_read }
}

/// A request for `paths`' sidecars, and `action` once they are in: run
/// at once when every one is in memory already and nothing is queued
/// ahead; else queued, the sidecars not yet read read on the pool with
/// the bar up, and the action run on the window's thread in its turn.
/// Frames under an offline root are not read and not waited for; each
/// action says what it does about them. True when the action ran now.
pub(crate) fn request(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    paths: Vec<PathBuf>,
    what: &str,
    action: Action,
) -> bool {
    let queued = queue(&mut state.borrow_mut(), paths, what, false, action);
    match queued {
        Queued::Inline(action, paths) => {
            let at = resolve(&state.borrow(), &paths);
            action(state, app, worker, at.clone());
            say_gone(app, &paths, &at, what, false);
            true
        }
        Queued::Later { id, to_read } => {
            if to_read.is_empty() {
                run_ready(state, app, worker);
            } else {
                read(&mut state.borrow_mut(), app, id, to_read);
            }
            false
        }
    }
}

/// [`request`] from inside the window's state, for a caller that holds
/// it (`open_row`): always queued, never run inline. When nothing is to
/// be read and nothing is ahead, the action runs at the next turn of
/// the event loop; a test's `land_pending` runs it.
pub(crate) fn request_in(
    st: &mut State,
    app: &App,
    paths: Vec<PathBuf>,
    what: &str,
    pick: bool,
    action: Action,
) {
    match queue(st, paths, what, pick, action) {
        Queued::Inline(action, paths) => {
            st.loads.next += 1;
            let id = st.loads.next;
            st.loads.queue.push_back(Pending {
                id,
                paths,
                action: Some(action),
                what: what.to_string(),
                pick,
                landed: true,
            });
            if !cfg!(test) {
                let app_weak = app.as_weak();
                slint::Timer::single_shot(Duration::ZERO, move || {
                    let (Some(app), Some(state), Some(worker)) = (
                        app_weak.upgrade(),
                        crate::STATE.with(|s| s.borrow().clone()),
                        crate::WORKER.with(|w| w.borrow().clone()),
                    ) else {
                        return;
                    };
                    run_ready(&state, &app, &worker);
                });
            }
        }
        Queued::Later { id, to_read } => {
            if !to_read.is_empty() {
                read(st, app, id, to_read);
            }
        }
    }
}

/// A read a test's request asked for: the request's id, the list's
/// generation, the paths to read, and whether sidecars are on.
#[cfg(test)]
type QueuedRead = (u64, u64, Vec<PathBuf>, bool);

#[cfg(test)]
thread_local! {
    /// The reads a test's requests asked for, waiting for the test to
    /// run and land them (`land_pending`): there is no event loop to
    /// land them on, as the roots' reads are queued.
    pub(crate) static QUEUED: RefCell<Vec<QueuedRead>> = const { RefCell::new(Vec::new()) };
}

/// The read for request `id`: `to_read`'s sidecars on the pool, landed
/// on the window's thread when in, under the list it was started on
/// (`State::view_generation`): started under another list, it fills
/// nothing. In a test, queued for `land_pending`.
fn read(st: &mut State, app: &App, id: u64, to_read: Vec<PathBuf>) {
    let write = st.write_sidecars;
    let generation = st.view_generation;
    bar_started(st, app, to_read.len());
    st.loads.in_flight.insert(id, to_read.clone());
    #[cfg(test)]
    {
        QUEUED.with(|q| q.borrow_mut().push((id, generation, to_read, write)));
    }
    #[cfg(not(test))]
    {
        let progress = st.loads.progress.clone();
        let app_weak = app.as_weak();
        let paths = to_read.clone();
        let spawned = std::thread::Builder::new()
            .name("greycard sidecars read".into())
            .spawn(move || {
                let started = Instant::now();
                let none = vec![None; to_read.len()];
                let held = bring(&to_read, &none, write, progress.as_deref().map(|p| &p.read));
                tracing::debug!(
                    "{} sidecar(s) read on the pool in {:.1} ms",
                    to_read.len(),
                    started.elapsed().as_secs_f64() * 1e3
                );
                let results: Vec<(PathBuf, Held)> = to_read.into_iter().zip(held).collect();
                let _ = slint::invoke_from_event_loop(move || {
                    let (Some(app), Some(state), Some(worker)) = (
                        app_weak.upgrade(),
                        crate::STATE.with(|s| s.borrow().clone()),
                        crate::WORKER.with(|w| w.borrow().clone()),
                    ) else {
                        return;
                    };
                    landed(&state, &app, &worker, id, generation, results);
                });
            });
        if let Err(e) = spawned {
            // No thread to be had: read here, on the window's thread.
            tracing::warn!("{e}; the sidecars read on the window's thread");
            st.loads.in_flight.remove(&id);
            let none = vec![None; paths.len()];
            let held = bring(&paths, &none, write, None);
            let results: Vec<(PathBuf, Held)> = paths.into_iter().zip(held).collect();
            for (path, h) in results {
                if let Some(i) = st.files.iter().position(|f| *f == path)
                    && !is_loaded(st, i)
                {
                    take(st, app, i, h);
                }
            }
            if let Some(p) = st.loads.queue.iter_mut().find(|p| p.id == id) {
                p.landed = true;
            }
            if !st.loads.reading() {
                bar_done(st, app);
            }
            // The actions run at the next turn: the caller holds the
            // state.
            let app_weak = app.as_weak();
            slint::Timer::single_shot(Duration::ZERO, move || {
                let (Some(app), Some(state), Some(worker)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                    crate::WORKER.with(|w| w.borrow().clone()),
                ) else {
                    return;
                };
                run_ready(&state, &app, &worker);
            });
        }
    }
}

/// A test's turn at the reads its requests queued: each read in place
/// and landed, its action run in its turn; how many reads landed.
#[cfg(test)]
pub(crate) fn land_pending(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) -> usize {
    let mut n = 0;
    loop {
        let reads: Vec<QueuedRead> = QUEUED.with(|q| q.borrow_mut().drain(..).collect());
        if reads.is_empty() {
            break;
        }
        for (id, generation, to_read, write) in reads {
            let none = vec![None; to_read.len()];
            let held = bring(&to_read, &none, write, None);
            landed(
                state,
                app,
                worker,
                id,
                generation,
                to_read.into_iter().zip(held).collect(),
            );
            n += 1;
        }
    }
    run_ready(state, app, worker);
    n
}

/// A read done, on the window's thread: each sidecar into the frame
/// its path is at, where that frame still stands in (a request dropped
/// meanwhile still brings the sidecars it read; they are as good), the
/// request marked, the bar moved on, and the requests ready to run
/// run in order. A read started under another list fills nothing: the
/// path may have been written since it started (a culling control, an
/// export's record), and its older bytes would be saved over the newer
/// by the next key. A stand-in is filled only by a read of its own list.
fn landed(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    id: u64,
    generation: u64,
    results: Vec<(PathBuf, Held)>,
) {
    {
        let mut st = state.borrow_mut();
        if st.view_generation == generation {
            for (path, h) in results {
                if let Some(i) = st.files.iter().position(|f| *f == path)
                    && !is_loaded(&st, i)
                {
                    take(&mut st, app, i, h);
                }
            }
        } else {
            tracing::debug!(
                "a read of {} sidecar(s) started under another list; nothing taken from it",
                results.len()
            );
        }
        match st.loads.queue.iter_mut().find(|p| p.id == id) {
            Some(p) => p.landed = true,
            None => tracing::debug!("a sidecar read came in for a request since dropped"),
        }
        st.loads.in_flight.remove(&id);
        crate::panel::look_rename::read_landed(&mut st, app, id);
        if !st.loads.reading() {
            bar_done(&mut st, app);
        }
    }
    run_ready(state, app, worker);
}

/// Run every request at the front of the queue whose read is in, in
/// order, each on the frames its paths are at now.
pub(crate) fn run_ready(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) {
    loop {
        let next = {
            let mut st = state.borrow_mut();
            match st.loads.queue.front() {
                Some(p) if p.landed => st.loads.queue.pop_front(),
                _ => None,
            }
        };
        let Some(mut p) = next else {
            return;
        };
        let at = resolve(&state.borrow(), &p.paths);
        if let Some(action) = p.action.take() {
            action(state, app, worker, at.clone());
        }
        say_gone(app, &p.paths, &at, &p.what, p.pick);
    }
}

/// A frame a request named that is no longer in the list when the
/// request runs (moved, or gone) took nothing of it: said, after the
/// action has had its say.
fn say_gone(app: &App, paths: &[PathBuf], at: &[Option<usize>], what: &str, pick: bool) {
    let gone: Vec<&PathBuf> = paths
        .iter()
        .zip(at)
        .filter(|(_, i)| i.is_none())
        .map(|(p, _)| p)
        .collect();
    let said = match gone.as_slice() {
        [] => return,
        [one] if pick => format!("{} moved or is gone; not opened", file_name(one)),
        [one] => format!(
            "{} moved or is gone; the {what} was not applied to it",
            file_name(one)
        ),
        many => format!(
            "{} frames moved or are gone; the {what} was not applied to them",
            many.len()
        ),
    };
    tracing::info!("{said}");
    app.set_status(said.into());
}

/// The pick whose sidecar was on its way is over, opened or not: no
/// pick pending, and the "reading X's sidecar..." line it put up goes
/// with it, when it is still up (an undo that selected another frame,
/// Escape out of culling, a click elsewhere, the read given up on).
/// In culling the bar goes with it even when the line is gone: the
/// mode's own line takes the status over while a slow read is out,
/// and nothing else there puts the bar up.
pub(crate) fn pick_over(st: &mut State, app: &App) {
    if st.pick_pending.take().is_none() {
        return;
    }
    if app.get_status().starts_with("reading ") {
        app.set_status("".into());
        app.set_busy(false);
    } else if st.cull.is_some() {
        app.set_busy(false);
    }
}

/// The list was replaced: every request out is dropped, actions and
/// all, since the frames they named are gone; a read still out lands
/// on whatever of its paths the new list has.
pub(crate) fn list_replaced(st: &mut State, app: &App) {
    if !st.loads.queue.is_empty() {
        tracing::debug!(
            "{} request(s) for sidecars dropped with the list",
            st.loads.queue.len()
        );
    }
    st.loads.queue.clear();
    pick_over(st, app);
    bar_done(st, app);
}

/// A read of `n` sidecars going out: the bar's count, and the bar's
/// timer if it is not running. The bar itself is the roots view's
/// (`loading_shown` and its line), shown here only while no view's
/// read owns it.
fn bar_started(st: &mut State, app: &App, n: usize) {
    let now = Instant::now();
    let progress = st
        .loads
        .progress
        .get_or_insert_with(|| Arc::new(Progress::default()));
    let total = progress.total.fetch_add(n, Ordering::Relaxed) + n;
    let read = progress.read.load(Ordering::Relaxed);
    if st.loads.since.is_none() {
        st.loads.since = Some(now);
        st.loads.moved = Some(now);
        st.loads.counted = Some(now);
        st.loads.seen = (read, total);
        let app_weak = app.as_weak();
        st.loads.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(100),
            move || {
                let (Some(app), Some(state), Some(worker)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                    crate::WORKER.with(|w| w.borrow().clone()),
                ) else {
                    return;
                };
                bar_tick(&state, &app, &worker, Instant::now());
            },
        );
    }
}

/// The timer's tick while frames' reads are out, at `now`: the bar
/// filled to the sidecars read, once a read has been out
/// `roots::LOADING_SHOWN_AFTER`; a read out past the give-up whose
/// count has stood still is given up on, its requests dropped.
pub(crate) fn bar_tick(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, now: Instant) {
    let gave_up = {
        let mut st = state.borrow_mut();
        let (Some(since), Some(progress)) = (st.loads.since, st.loads.progress.clone()) else {
            bar_done(&mut st, app);
            return;
        };
        let total = progress.total.load(Ordering::Relaxed);
        let read = progress.read.load(Ordering::Relaxed).min(total);
        if (read, total) != st.loads.seen {
            if read != st.loads.seen.0 {
                st.loads.moved = Some(now);
            }
            st.loads.seen = (read, total);
            st.loads.counted = Some(now);
        }
        let out = now.saturating_duration_since(since);
        let still = now.saturating_duration_since(st.loads.moved.unwrap_or(since));
        let settled = now.saturating_duration_since(st.loads.counted.unwrap_or(since));
        if out > FRAMES_GIVE_UP && still > FRAMES_STALLED {
            // Every request is dropped, those waiting on the read and
            // those queued behind them alike, and said by name; a pick
            // among them is let go, the frame left as an offline one is
            // shown, its stand-in on the panel.
            let dropped: Vec<Pending> = st.loads.queue.drain(..).collect();
            if dropped.iter().any(|p| p.pick) {
                pick_over(&mut st, app);
                app.set_busy(false);
            }
            let said = format!(
                "{} dropped: the sidecars did not come in",
                describe(&dropped)
            );
            tracing::warn!("{said}");
            bar_done(&mut st, app);
            app.set_status(said.into());
            true
        } else {
            if out >= crate::roots::LOADING_SHOWN_AFTER && !st.library.loading {
                let fraction = if total == 0 {
                    0.0
                } else {
                    read as f32 / total as f32
                };
                app.set_loading_fraction(fraction);
                let waiting = (settled >= crate::roots::LOADING_WAITING_AFTER && read < total)
                    .then(|| {
                        let out = st.loads.queue.iter().filter(|p| !p.landed);
                        let paths = out.flat_map(|p| &p.paths);
                        crate::roots::one_root_named(&st.library.roots, paths.map(PathBuf::as_path))
                    })
                    .flatten();
                app.set_loading_line(
                    crate::roots::loading_words(read, total, waiting.as_deref()).into(),
                );
                app.set_loading_shown(true);
            }
            false
        }
    };
    if gave_up {
        run_ready(state, app, worker);
    }
}

/// No frames' read is out: the bar's timer stopped, and the bar gone
/// unless a view's read has it.
pub(crate) fn bar_done(st: &mut State, app: &App) {
    st.loads.since = None;
    st.loads.progress = None;
    st.loads.moved = None;
    st.loads.counted = None;
    st.loads.seen = (0, 0);
    st.loads.timer.stop();
    if !st.library.loading {
        app.set_loading_shown(false);
    }
}

/// Frame `i`'s whole sidecar in hand: read now if it was standing in
/// from its row, one read on this thread. False when it cannot be
/// read, its root being offline, which the status line then says.
/// The meta the read brings replaces the row's (a stale row, an XMP
/// beside the frame taken), and the frame's badges follow.
pub(crate) fn load_frame(st: &mut State, app: &App, i: usize) -> bool {
    if st.from_row.get(i).is_none_or(|r| r.read) {
        return true;
    }
    if let Some(root) = offline_root(st, i) {
        tracing::info!(
            "{}: its sidecar is not read, {} being offline",
            file_name(&st.files[i]),
            root.display()
        );
        say_offline(st, app, &root, "");
        return false;
    }
    if let Some(why) = st.from_row[i].unread.clone() {
        // A request's read just failed on it: not tried again here, on
        // the window's thread; said again.
        app.set_status(why.into());
        return false;
    }
    let held = from_disk(&st.files[i], st.write_sidecars);
    take(st, app, i, held)
}

/// Every frame of `frames` loaded, those under an offline root left
/// out and said; the ones in hand, in the order given. More than one
/// to read is read on the rayon pool.
pub(crate) fn load_frames(st: &mut State, app: &App, frames: &[usize]) -> Vec<usize> {
    let mut kept = Vec::with_capacity(frames.len());
    let mut to_read = Vec::new();
    let mut offline: Option<PathBuf> = None;
    let mut unread: Option<String> = None;
    for &i in frames {
        if st.from_row.get(i).is_none_or(|r| r.read) {
            kept.push(i);
        } else if let Some(root) = offline_root(st, i) {
            offline.get_or_insert(root);
        } else if let Some(why) = &st.from_row[i].unread {
            // A request's read just failed on it: left out, not read
            // again on the window's thread.
            unread.get_or_insert_with(|| why.clone());
        } else {
            kept.push(i);
            to_read.push(i);
        }
    }
    if let Some(root) = offline {
        let left_out = frames.len() - kept.len();
        tracing::info!(
            "{left_out} frame(s) under {} left out: it is offline",
            root.display()
        );
        say_offline(st, app, &root, ": left out");
    }
    if let Some(why) = unread {
        app.set_status(why.into());
    }
    let mut failed = Vec::new();
    match to_read.len() {
        0 => {}
        1 => {
            let i = to_read[0];
            let held = from_disk(&st.files[i], st.write_sidecars);
            if !take(st, app, i, held) {
                failed.push(i);
            }
        }
        n => {
            let started = std::time::Instant::now();
            let paths: Vec<PathBuf> = to_read.iter().map(|&i| st.files[i].clone()).collect();
            let rows = vec![None; n];
            let held = bring(&paths, &rows, st.write_sidecars, None);
            for (i, h) in to_read.into_iter().zip(held) {
                if !take(st, app, i, h) {
                    failed.push(i);
                }
            }
            tracing::debug!(
                "{n} sidecars read on the pool in {:.1} ms",
                started.elapsed().as_secs_f64() * 1e3
            );
        }
    }
    // A sidecar that could not be read is left out: whatever the
    // caller would write, it would write over the frame's own.
    kept.retain(|i| !failed.contains(i));
    kept
}

/// A read of frame `i`'s sidecar landing with `sidecar`, as a request's
/// does: for a test of what comes in late.
#[cfg(test)]
pub(crate) fn land_read(st: &mut State, app: &App, i: usize, sidecar: Sidecar) -> bool {
    let held = Held {
        sidecar,
        ..from_nothing()
    };
    take(st, app, i, held)
}

/// `held` becomes frame `i`'s: the sidecar, the seed, and the row no
/// longer standing in. Badges follow the meta the read brought. A read
/// that did not take is not taken over a frame standing in from its
/// row — a `.gcd` that could not be read, or none found where the row
/// says there is a develop — since the next save would write the
/// default edit over the frame's own: the frame stands in still, the
/// status line says why, and false.
fn take(st: &mut State, app: &App, i: usize, held: Held) -> bool {
    let Held {
        sidecar,
        seed,
        from_row,
        trouble,
        ..
    } = held;
    let standing_in = st.from_row.get(i).is_some_and(|r| !r.read);
    let why = match &trouble {
        Some(Trouble::Unreadable(e)) if standing_in => Some(format!(
            "{}'s sidecar could not be read ({e}); nothing is written over it",
            file_name(&st.files[i])
        )),
        Some(Trouble::Missing)
            if standing_in && (st.from_row[i].edited || st.from_row[i].exported) =>
        {
            Some(format!(
                "{}'s sidecar was not found, though the library has an edit or an \
                 export of it on record; nothing is written over it",
                file_name(&st.files[i])
            ))
        }
        _ => None,
    };
    if let Some(why) = why {
        tracing::warn!("{why}");
        app.set_status(why.clone().into());
        st.from_row[i].unread = Some(why);
        return false;
    }
    st.sidecars[i] = sidecar;
    // A read that was out when a look it names was renamed: it takes the
    // new name, the open frame's too, before the panel is opened on it.
    crate::panel::look_rename::caught_up(st, i);
    if let Some(s) = st.seed_blend.get_mut(i) {
        *s = seed;
    }
    let iso = st.from_row[i].iso;
    st.from_row[i] = FromRow { iso, ..from_row };
    // The cell follows what the read found: the meta, and the marks
    // for an edit and an export, which the row only stood in for.
    crate::panel::browser::show_badges(st, app, i);
    // Its edit is known now: shown in its cells, or the camera's.
    crate::edited::refresh(st, i);
    // The frame on screen, its panel the stand-in's: opened on its own
    // sidecar now, whatever brought it in (a stalled read after the
    // give-up, a key's read, a preset's load), so the next slider
    // moves from the frame's edit and not over it. A pick still
    // pending is opened by its own request; culling's panel is not
    // the frame's.
    if st.current == Some(i)
        && st.cull.is_none()
        && st.pick_pending.is_none()
        && st.panel_stand_in.as_deref() == st.files.get(i).map(PathBuf::as_path)
        && let Some(worker) = crate::WORKER.with(|w| w.borrow().clone())
    {
        crate::panel::browser::open_frame(st, app, &worker, i);
    }
    true
}

/// Frame `i`'s row as the index has it now, taken into the frame while
/// its sidecar stands in: the meta, the seed and the turns. A frame
/// already loaded keeps what memory holds. True when what the browser
/// shows of the frame changed (its badges, its turns, what the filter
/// would say).
pub(crate) fn apply_row(st: &mut State, i: usize, row: &RowMeta) -> bool {
    let Some(from) = st.from_row.get_mut(i) else {
        return false;
    };
    if from.read {
        return false;
    }
    let raw = !greycard_core::picture::is_picture_path(&st.files[i]);
    let next = FromRow::of(row);
    let edited = from.edited != next.edited;
    let rewritten = from.sidecar != next.sidecar;
    let turned = from.turns != next.turns || edited || from.exported != next.exported;
    *from = next;
    // Whether it shows an edit, and under what key, as the row now says.
    if rewritten {
        crate::edited::changed(st, i);
    } else if edited {
        crate::edited::refresh(st, i);
    }
    // A first guess from the row; the read that lands decides the seed.
    if let Some(s) = st.seed_blend.get_mut(i) {
        *s = raw && !row.edited;
    }
    let moved = st.sidecars[i].meta != row.meta;
    if moved {
        st.sidecars[i].meta = row.meta.clone();
    }
    moved || turned
}

/// The thumbnail job for file `i`: made from the file, or, under a
/// root that is offline, looked up in the cache by the row's key and
/// nothing else, since the file cannot be read.
pub(crate) fn thumb_job(st: &State, i: usize) -> Job {
    let path = st.files[i].clone();
    if is_offline(st, i)
        && let Some((hash, stamp)) = st.from_row.get(i).and_then(|r| r.key.clone())
    {
        return Job::CachedThumbnail {
            index: i,
            path,
            hash,
            stamp,
        };
    }
    Job::Thumbnail { index: i, path }
}

/// The roots found offline changed under the list: a frame whose root
/// came back has its picture asked for again, since a miss in the
/// cache while it was offline was counted as a failure; every tile's
/// dimming follows in the rebuild the caller does.
pub(crate) fn roots_changed(
    st: &mut State,
    worker: &crate::worker::Worker,
    was: &HashSet<PathBuf>,
    now: &HashSet<PathBuf>,
) {
    let back: Vec<&PathBuf> = was.iter().filter(|r| !now.contains(*r)).collect();
    if back.is_empty() {
        return;
    }
    let mut again = 0;
    for i in 0..st.files.len() {
        if back.iter().any(|r| st.files[i].starts_with(r))
            && st.thumb_failed.get(i) == Some(&true)
            && st.thumb_base.get(i).is_some_and(Option::is_none)
        {
            st.thumb_failed[i] = false;
            worker.send(Job::Thumbnail {
                index: i,
                path: st.files[i].clone(),
            });
            again += 1;
        }
    }
    if again > 0 {
        tracing::info!("library: {again} picture(s) asked for again, their root back online");
    }
}
