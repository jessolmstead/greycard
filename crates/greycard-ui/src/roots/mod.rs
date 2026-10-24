//! The library's roots in the editor (notes §72): the folders the
//! user has added, a pass over them on launch, a watcher on them
//! while the editor runs, and the all-roots view, which is every file
//! under them in one list.
//!
//! The roots are `greycard_library::Roots`, kept in `roots.json`
//! beside the library's database, so a `--library` elsewhere carries
//! its own. The launch pass is `index_tree` over each root on the
//! indexer's thread, behind everything the window asks: an mtime
//! pass, since a file whose size and mtime are unchanged is not read.
//! The watcher's changes go to the same thread, ahead of the launch
//! pass and behind the window. A root that is not there (a drive
//! unplugged) is offline: no pass, no watch, its row says so, and
//! the view leaves its files out rather than show them as gone.
//!
//! Which roots are there, which are on a network mount, and the
//! watcher on the rest are all found off the window's thread
//! ([`Plan::build`](watcher::Plan::build)), and the watcher is put in place when it is
//! ready, unless the roots have changed since it was asked for. A root
//! on a network mount is not watched, since inotify does not see what
//! the server does; a pass goes over it every so often instead
//! ([`Poll`]).
//!
//! The all-roots view is the index's list of the files under the
//! roots, by folder then name, with the filter bar over it as over a
//! folder. It can be the whole library, and it reads no sidecar: each
//! frame's meta, whether it has a develop and which way up it is shown
//! come from its row, standing in for the sidecar until something
//! needs the whole of it (`crate::rows`). A root that is offline is in
//! the view too, its frames dimmed and their pictures from the cache.
//! A folder's own open takes the same path: its frames' rows where the
//! index has them, and the rest of its sidecars read on the rayon pool
//! off the window's thread, while the window keeps the list it has and
//! shows a bar.
//!
//! A list already open is kept up with the disk rather than opened
//! again: a pass that finds a file added, gone or moved, or a sidecar
//! changed, has the list read again off the window's thread (the roots
//! and folders looked at, the rows asked of the index, the frame on
//! screen looked for; one read out at a time, the passes said meanwhile
//! folded into one more), and merged on the window's thread into what
//! it holds, which asks the disk nothing. Each frame keeps its sidecar,
//! its picture and its place in the selection by its path, a frame
//! whose row stands in takes the row's new meta, and every frame still
//! without a picture is asked for one again. The frame on screen is
//! followed to where the index says it went, and never to a copy of it.
//!
//! A root can have a name of the user's, given from its row's menu
//! (right-click, Rename...), and shown wherever the
//! folder's own name would be. It is a label and nothing more: the
//! root is its path everywhere, and naming it moves nothing.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use greycard_library::roots::Added;
use greycard_library::{Roots, RowMeta};

use crate::panel::browser::{open_loaded, rebuild_browser};
use crate::rows::{self, Held};
use crate::*;

mod checks;
mod kept_up;
mod landing;
mod look;
mod pane;
#[cfg(test)]
pub(crate) mod tests;
mod view;
mod watcher;

use self::{
    checks::offline_of,
    kept_up::{empty_view, merge},
    landing::land,
    look::send_off,
    pane::{add, open_folder_to_add, open_sheet, remove, rename},
    view::{loading_done, read_order},
    watcher::{Poll, online, watch},
};
pub(crate) use self::{
    checks::{answer_of, answers, answers_of},
    kept_up::{ask_owed, background_done, folders_passed},
    pane::{
        add_and_open, edit_roots, one_root_named, recount, root_name, said, show, taken_from_file,
    },
    view::{loading_words, open_listing, open_view, refresh_view},
    watcher::{poll_every, restart_poll, start},
};

/// What the browser lists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum View {
    /// A folder, or files the desktop asked to open.
    #[default]
    Folder,
    /// Every file under the roots, or under one of them.
    Roots(Option<PathBuf>),
    /// One folder under a root, from the index as a view of the root
    /// is: the frames directly in it, or with `deep` those in the
    /// folders under it too. Still that root's view, narrowed: its
    /// row stays lit.
    Branch {
        root: PathBuf,
        folder: PathBuf,
        deep: bool,
    },
    /// A collection, by its id: its pictures from the index, each
    /// once, in the collection's own order, or in the order chosen for
    /// it (`sort`).
    Collection { id: String },
    /// The export queue: every frame of every entry, each under its
    /// destination's heading, in the queue's order. A frame queued
    /// twice is listed twice: each is an export.
    Queue,
}

impl View {
    /// The list comes from the index's rows, not from a folder's
    /// listing on disk.
    pub(crate) fn lists_rows(&self) -> bool {
        !matches!(self, View::Folder)
    }

    /// What the index is asked to list: the files under each of these
    /// folders, or with the flag, those directly in the one folder.
    fn listed(&self, all: &[PathBuf]) -> (Vec<PathBuf>, bool) {
        match self {
            View::Folder => (Vec::new(), false),
            View::Roots(None) => (all.to_vec(), false),
            View::Roots(Some(r)) => (vec![r.clone()], false),
            View::Branch { folder, deep, .. } => (vec![folder.clone()], !deep),
            View::Collection { .. } | View::Queue => (Vec::new(), false),
        }
    }

    /// The collection this view shows, by its id.
    pub(crate) fn collection(&self) -> Option<&str> {
        match self {
            View::Collection { id, .. } => Some(id),
            _ => None,
        }
    }
}

/// The index's rows for a view, in `order`: under each of `under`, or
/// with `only`, directly in the one folder.
fn rows_listed(
    lib: &greycard_library::Library,
    under: &[PathBuf],
    only: bool,
    order: greycard_library::Order,
) -> greycard_library::Result<Vec<(PathBuf, RowMeta)>> {
    match (only, under) {
        (true, [folder]) => lib.rows_in_canonical(folder, order),
        _ => lib.rows_under_canonical(under, order),
    }
}

/// How many frames a view of the index lists, for the status line,
/// counted by the index's own ranges.
fn count_listed(lib: &greycard_library::Library, under: &[PathBuf], only: bool) -> usize {
    match (only, under) {
        (true, [folder]) => lib.count_in_canonical(folder).unwrap_or(0),
        _ => under
            .iter()
            .map(|r| lib.count_under_canonical(r).unwrap_or(0))
            .sum(),
    }
}

/// The roots as the editor holds them: the list, where it is kept,
/// and each root's file count as the index last said.
#[derive(Default)]
pub(crate) struct Library {
    pub(crate) roots: Roots,
    /// The roots file; none for a run whose roots came from the
    /// command line, which leaves the user's alone, and none when the
    /// file could not be read, so it is not written over.
    pub(crate) file: Option<PathBuf>,
    pub(crate) counts: Vec<usize>,
    pub(crate) watcher: Option<greycard_library::Watcher>,
    /// The number of the watcher asked for last: one built for roots
    /// that have changed since is dropped when it lands.
    pub(crate) watch_token: u64,
    /// The launch's look at the roots is out, and the launch pass not
    /// asked for yet.
    pub(crate) starting: bool,
    /// The roots on a network mount, with the mount's type: not
    /// watched, and passed over by `poll` instead.
    pub(crate) remote: Vec<(PathBuf, String)>,
    /// The roots said in the log to be on a network mount, so that it
    /// is said once a root.
    pub(crate) said_remote: HashSet<PathBuf>,
    /// The timer that passes over the roots on a network mount, and
    /// how often; none for never.
    pub(crate) poll: Option<Poll>,
    pub(crate) poll_every: Option<Duration>,
    /// A view asked for before the window could read the index.
    pub(crate) wanted: Option<View>,
    /// A capture waits for the view the command line asked for.
    pub(crate) awaiting: bool,
    /// Roots the launch pass has still to finish: a capture of the
    /// view waits for them when the index had nothing under the roots
    /// yet.
    pub(crate) launch_left: usize,
    /// The view is in the browser, and the next frame drawn is the
    /// first to show it: when it was asked for, and what, for the log.
    pub(crate) painted: Option<(Instant, &'static str)>,
    /// A view's sidecars are being read on the pool: the list in the
    /// browser is about to be replaced, and is not merged into. (A
    /// selection's sidecars being read for a key or a sync is
    /// `Views::loads`, with a bar of its own.)
    pub(crate) loading: bool,
    pub(crate) loading_since: Option<Instant>,
    /// How far the view's read has got, bumped on the pool; read by
    /// `loading_timer` to fill the bar.
    pub(crate) progress: Option<Arc<Progress>>,
    pub(crate) loading_timer: slint::Timer,
    /// The count read and its total as the timer last saw them, and
    /// when the count last moved: a read past [`READ_GIVES_UP`] whose
    /// count has stood still for [`LOADING_STALLED`] is given up on by
    /// the timer.
    pub(crate) progress_seen: (usize, usize),
    pub(crate) progress_moved: Option<Instant>,
    /// When the count or its total last moved: the bar says "Waiting"
    /// only after both stand still, so a read that spent its first
    /// seconds listing does not open on it.
    pub(crate) progress_counted: Option<Instant>,
    /// The one root the view's read is under, by its row's name: what
    /// the bar names when the count stands still. `None` for a read
    /// over several roots, or a folder under none.
    pub(crate) loading_where: Option<String>,
    /// The list is being read again off the window's thread; a
    /// refresh asked for meanwhile is done once when it lands
    /// (`stale`).
    pub(crate) merging: bool,
    pub(crate) merge_since: Option<Instant>,
    /// The read out's number: one given up on is dropped when it lands.
    pub(crate) merge_token: u64,
    pub(crate) stale: bool,
    /// The roots found offline when last looked at, off the window's
    /// thread (or on launch): what their rows say.
    pub(crate) offline: HashSet<PathBuf>,
    /// The folders under the roots a read has found can be read. A
    /// folder not here, one that could not be read among them, is
    /// looked at by the next read off the window's thread; a pass that
    /// finishes over a folder has it looked at again
    /// (`folders_passed`), and an open looks at every folder again.
    pub(crate) readable: HashSet<PathBuf>,
    /// Moved on at every finished pass, which is kept in `passes` under
    /// it, so a read that was out meanwhile does not keep what it saw
    /// of the folders the pass went over.
    pub(crate) readable_epoch: u64,
    pub(crate) passes: Vec<(u64, PathBuf)>,
    /// The epoch of the oldest pass forgotten: a read older than it
    /// keeps nothing.
    pub(crate) passes_from: u64,
    /// Folders as the browser lists them, and as the index spells them
    /// (canonical): given by the reads off the window's thread, so the
    /// window's own reads of the index ask the disk for none.
    pub(crate) canonical: HashMap<PathBuf, PathBuf>,
    /// The folders the last read to look at them could not read: a pass
    /// over one has the list read again, even one that found nothing
    /// changed (the folder readable again, its files as they were).
    pub(crate) unreadable: HashSet<PathBuf>,
    /// The roots' rows as the pane shows them: one model for the
    /// session, its rows replaced only when they change, since `show`
    /// runs at every pass and recount and a row rebuilt under the
    /// pointer loses its hover text and a press in progress.
    pub(crate) rows: Rc<VecModel<RootChip>>,
}

/// How long a root has to answer whether it is there before a read
/// takes it for offline: a hard-mounted network share gone away does
/// not answer at all.
pub(crate) const ROOT_WAIT: Duration = Duration::from_secs(3);

/// How long a read may be out before the next refresh gives up on it
/// and sends another; what it brings after that is dropped.
const READ_GIVES_UP: Duration = Duration::from_secs(60);

/// How long a view's read runs before its bar is shown: a root on a
/// local disk is in before this, and never flashes one.
pub(crate) const LOADING_SHOWN_AFTER: Duration = Duration::from_millis(200);

/// How long the count stands still before the bar names the root it
/// waits on instead: a sidecar on a local disk is read in far less.
pub(crate) const LOADING_WAITING_AFTER: Duration = Duration::from_secs(1);

/// How often the window looks at a view's read's count while it runs.
const LOADING_TICK: Duration = Duration::from_millis(100);

/// How long a view's read's count may stand still, once the read is
/// past [`READ_GIVES_UP`], before the timer gives up on it. Longer than
/// a tick: a share slow enough to take more than a tenth of a second a
/// sidecar would look stopped between two ticks while still moving.
const LOADING_STALLED: Duration = Duration::from_secs(10);

/// A view's read, how far along: the sidecars to read and those read,
/// shared by the read's thread and the pool with the window.
#[derive(Debug, Default)]
pub(crate) struct Progress {
    /// Set by the window from the index's list, then by the read to
    /// the files it kept (a root offline, a folder that cannot be read,
    /// left out).
    pub(crate) total: AtomicUsize,
    /// Bumped on the pool for each sidecar read.
    pub(crate) read: AtomicUsize,
}

/// How many finished passes the window remembers, for a read that
/// lands after them to know which folders they covered.
const PASSES_KEPT: usize = 64;

/// What a read off the window's thread is for.
enum Purpose {
    /// A view or a folder opened: its list replaces the browser's once
    /// read, `select` opened.
    Open {
        view: View,
        asked: Instant,
        select: Select,
    },
    /// The browser's list read again, and merged into what it holds.
    Merge,
}

/// Which frame an opened list starts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Select {
    /// This one of the list, or the last when the list is shorter.
    Row(usize),
    /// The file last open, if it is in the list, else the first. Found
    /// off the window's thread, since matching a path to the list makes
    /// folders canonical.
    Last(Option<PathBuf>),
}

/// What a folder's open lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Listing {
    /// A folder, listed off the window's thread.
    Folder(PathBuf),
    /// Files given, as the desktop or the command line hand them over.
    Files(Vec<PathBuf>),
}

/// Where the list comes from.
enum Source {
    /// A folder's files, listed off the window's thread; the folder is
    /// made canonical there by every read, so a link retargeted
    /// meanwhile is seen.
    Folder(PathBuf),
    /// Files given, as they are; their folders made canonical off the
    /// window's thread.
    Files(Vec<PathBuf>),
    /// The index's rows under the view's roots, as it lists them; those
    /// in a folder that cannot be read are left out off the window's
    /// thread, and those under a root that is offline kept, standing
    /// in from their rows. The rows are read off the window's thread
    /// too, on a connection of the read's own, unless the window has
    /// no path to the index and read them itself (the tests' windows).
    /// With `only`, `roots` is one folder of a root's tree, and its own
    /// rows alone are listed. The rows under `hide`, the archives in a
    /// view of every root, are left out where their content key is also
    /// under a local root (§216): one frame, shown once, from here.
    Roots {
        roots: Vec<PathBuf>,
        only: bool,
        rows: Option<Vec<(PathBuf, RowMeta)>>,
        hide: Vec<PathBuf>,
    },
    /// A collection's references, shown from the index's rows: each
    /// one's file found off the window's thread, on the read's own
    /// connection ([`crate::collections::resolve`]), unless the window
    /// has no path to the index and found them itself. What the read
    /// found moved or rewritten, and how many are missing, come back
    /// with it for the collection's file and the status line.
    /// The export queue's frames, each file and version, in the view's
    /// order, with where each is in the queue: listed as files given
    /// are, their sidecars read on the pool or standing in from their
    /// rows.
    Queue {
        frames: Vec<crate::versions::Key>,
        listed: crate::panel::queue_view::Listed,
    },
    Collection {
        /// The collection's id: what the read finds is put into this
        /// one, whatever view is open when it lands.
        id: String,
        refs: Vec<crate::collections::Reference>,
        archives: Vec<PathBuf>,
        shown: Option<crate::collections::Shown>,
    },
}

/// A list to read off the window's thread: everything the read needs,
/// taken from the window's state when it was asked for, so the read
/// touches nothing of it and the window touches no disk.
pub(crate) struct Look {
    purpose: Purpose,
    /// The browser's list when this was asked for: a later one is not
    /// merged into.
    generation: u64,
    /// Which merge read this is: one given up on is dropped when it
    /// lands.
    token: u64,
    /// `Library::readable_epoch` as it was: what the read saw of a
    /// folder is not kept if a pass has since gone over it.
    epoch: u64,
    source: Source,
    /// The roots to look at for whether each is there: every root for
    /// a view of the roots, none for a folder's.
    roots: Vec<PathBuf>,
    /// The folders the window knows can be read.
    known: HashSet<PathBuf>,
    /// The paths in the window's list: a file not among them is new,
    /// and its sidecar is read.
    have: HashSet<PathBuf>,
    write: bool,
    /// The frame on screen and its row, to see whether it is still
    /// there, and where the index says it went if not.
    on_screen: Option<(PathBuf, Option<i64>)>,
    /// The index's database, opened read-only for the list's rows and
    /// to follow a frame that is not where the window has it.
    index: Option<PathBuf>,
    /// A view's read: its count, for the window's bar.
    progress: Option<Arc<Progress>>,
    /// A camera's JPEG beside its raw is listed as a frame of its own;
    /// else it is left out, and found as the raw's companion.
    apart: bool,
    /// The order the list is read in, the view's choice: none for a
    /// collection's own.
    order: Option<greycard_library::Order>,
}

/// What the frame on screen was found to be, off the window's thread.
#[derive(Debug, Clone, Default)]
pub(crate) struct OnScreen {
    /// The frame's path when it was looked at.
    path: Option<PathBuf>,
    /// Its file is there.
    there: bool,
    /// It is not, and the index says its row is at this path, a file
    /// that is there.
    moved_to: Option<PathBuf>,
}

impl OnScreen {
    /// The frame at `path` looked at: one stat, and the index asked
    /// where its row went only when the file is not there.
    pub(crate) fn look(
        path: PathBuf,
        id: Option<i64>,
        index: Option<&greycard_library::Library>,
    ) -> Self {
        if path.exists() {
            return OnScreen {
                path: Some(path),
                there: true,
                moved_to: None,
            };
        }
        let moved_to = id
            .zip(index)
            .and_then(|(id, lib)| lib.found_at(&[id]).ok()?.remove(&id))
            .filter(|p| p.is_file());
        OnScreen {
            path: Some(path),
            there: false,
            moved_to,
        }
    }

    /// Whether `path` is the frame looked at, and its file was gone.
    fn gone(&self, path: &Path) -> bool {
        !self.there && self.path.as_deref() == Some(path)
    }
}

/// What a read brings the merge besides the list: the new files'
/// sidecars, the frame on screen as found, and the list's rows in the
/// index, so the merge asks neither the disk nor the index's folders.
#[derive(Default)]
pub(crate) struct Brought {
    /// The files new to the list, each with its sidecar as read from
    /// disk or standing in from its row.
    pub(crate) read: HashMap<PathBuf, Held>,
    pub(crate) seen: OnScreen,
    /// Each listed file's row, by path, where the index has one; none
    /// at all when the read had no index to ask, and the merge then
    /// asks it. A frame whose sidecar stands in takes its row's meta.
    pub(crate) rows: Option<HashMap<PathBuf, RowMeta>>,
}

/// A read done: the list as the disk has it now, and the sidecars of
/// the files new to it.
pub(crate) struct Found {
    purpose: Purpose,
    generation: u64,
    token: u64,
    epoch: u64,
    /// None when the folder could not be listed, or had nothing in it.
    files: Option<Vec<PathBuf>>,
    /// Why there is no list, for the status line.
    said: Option<String>,
    /// The files new to the list, in the list's order, and their
    /// sidecars as brought.
    fresh: Vec<PathBuf>,
    held: Vec<Held>,
    /// How many of those were read from disk; the rest stand in from
    /// their rows.
    from_disk: usize,
    rows: Option<HashMap<PathBuf, RowMeta>>,
    /// The frame to open, for an open.
    select: usize,
    /// The roots found offline; none when no root was looked at.
    offline: Option<HashSet<PathBuf>>,
    /// The folders looked at that could be read. One that could not is
    /// not kept, and is looked at again by the next read: on a network
    /// share a failure can be a moment's.
    readable: Vec<PathBuf>,
    /// The folders looked at that could not be read: a pass over one
    /// has the list read again, even when it finds nothing changed.
    unreadable: Vec<PathBuf>,
    /// Folders as listed, and as the index spells them.
    canonical: Vec<(PathBuf, PathBuf)>,
    on_screen: OnScreen,
    seconds: f64,
    /// A collection's read: its id, how many of its references are
    /// missing, the hints of those under no offline root, and those
    /// found moved or rewritten.
    collection: Option<(String, usize, Vec<String>, Vec<crate::collections::Change>)>,
    /// A collection's read: the frames it shows, each file and version,
    /// in its order.
    frames: Option<Vec<crate::versions::Key>>,
    /// The export queue's read: where each of `frames` is in the queue.
    queue: Option<crate::panel::queue_view::Listed>,
    /// The camera's JPEGs beside the listed raws, raw by raw: from the
    /// folder's listing, or from the index's rows for a view of it.
    companions: Vec<(PathBuf, Vec<PathBuf>)>,
    /// The order the list was read in: a view opened before a sort was
    /// chosen lands in the order it was asked in, and is read again.
    order: Option<greycard_library::Order>,
}

/// One root's look, out on a thread of its own: its answer when it has
/// come.
type Answer = Arc<(Mutex<Option<bool>>, Condvar)>;

/// The roots' looks out, one a root at most. A root that has not
/// answered its last look is offline to every read until it does,
/// without another thread or another wait: a hard-mounted share gone
/// away does not answer at all, and a thread asking it is stuck for as
/// long as it is gone.
pub(crate) struct Checks {
    out: Mutex<HashMap<PathBuf, Answer>>,
    look: fn(&Path) -> bool,
    /// Threads started, for the log and the tests.
    started: AtomicUsize,
}

impl Checks {
    pub(crate) fn new(look: fn(&Path) -> bool) -> Checks {
        Checks {
            out: Mutex::new(HashMap::new()),
            look,
            started: AtomicUsize::new(0),
        }
    }

    /// Which of `roots` are offline: each root with no look out gets
    /// one, and has up to `wait` to answer; one whose last look has
    /// not answered yet is offline now, without waiting.
    pub(crate) fn offline(&self, roots: &[PathBuf], wait: Duration) -> HashSet<PathBuf> {
        self.answers(roots, wait)
            .into_iter()
            .filter(|(_, there)| *there != Some(true))
            .map(|(root, _)| root)
            .collect()
    }

    /// Each of `roots` looked at, as [`Checks::offline`] looks: whether
    /// it is there, or `None` when it has not answered within `wait`
    /// (or its last look is still out, or no thread could be had for
    /// one).
    pub(crate) fn answers(
        &self,
        roots: &[PathBuf],
        wait: Duration,
    ) -> HashMap<PathBuf, Option<bool>> {
        let mut answers = HashMap::new();
        let mut asked: Vec<(PathBuf, Answer)> = Vec::new();
        {
            let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
            for root in roots {
                if let Some(answer) = out.get(root) {
                    let answered = *answer.0.lock().unwrap_or_else(|e| e.into_inner());
                    if answered.is_none() {
                        answers.insert(root.clone(), None);
                        continue;
                    }
                }
                let answer: Answer = Arc::new((Mutex::new(None), Condvar::new()));
                let (for_thread, root_for_thread, look) = (answer.clone(), root.clone(), self.look);
                let spawned = std::thread::Builder::new()
                    .name("greycard root check".into())
                    .spawn(move || {
                        let there = look(&root_for_thread);
                        let (slot, told) = &*for_thread;
                        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(there);
                        told.notify_all();
                    });
                if spawned.is_err() {
                    // No thread to be had: no answer for this read.
                    answers.insert(root.clone(), None);
                    continue;
                }
                self.started.fetch_add(1, Ordering::Relaxed);
                out.insert(root.clone(), answer.clone());
                asked.push((root.clone(), answer));
            }
        }
        let until = Instant::now() + wait;
        for (root, answer) in asked {
            let (slot, told) = &*answer;
            let mut there = slot.lock().unwrap_or_else(|e| e.into_inner());
            while there.is_none() {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                there = told
                    .wait_timeout(there, left)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            if there.is_none() {
                tracing::warn!(
                    "library: {} did not answer in {} s; offline until it does",
                    root.display(),
                    wait.as_secs()
                );
            }
            answers.insert(root, *there);
        }
        answers
    }
}

/// The header's callbacks: a root chosen, all of them, one added by
/// the chooser or as the folder open, one taken out, one named.
pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    app.set_library_roots(ModelRc::from(state.borrow().views.library.rows.clone()));
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_library_root_rename(move |path| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            open_sheet(&state.borrow(), &app, Path::new(path.as_str()));
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_root_sheet_done(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if !app.get_root_sheet_open() {
                return;
            }
            app.set_root_sheet_open(false);
            let dir = PathBuf::from(app.get_root_sheet_path().as_str());
            let name = app.get_root_sheet_name();
            rename(&state, &app, &dir, &name);
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_library_root_picked(move |path| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let view = if path.is_empty() {
                View::Roots(None)
            } else {
                View::Roots(Some(PathBuf::from(path.as_str())))
            };
            open_view(&state, &app, &worker, view);
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_library_root_removed(move |path| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            remove(&state, &app, &worker, Path::new(path.as_str()));
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_library_root_add_open(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let dir = open_folder_to_add(&state.borrow());
            if let Some(dir) = dir {
                add(&state, &app, &worker, &dir);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_library_root_add(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let start = {
                let st = state.borrow();
                st.current
                    .and_then(|i| st.path_at(i))
                    .and_then(|f| f.parent())
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(|| PathBuf::from("/"))
            };
            let app_weak = app.as_weak();
            app.set_status("choosing a folder to add to the library...".into());
            chooser::choose_folder("Add a folder to the library", start, move |chosen| {
                let app_weak = app_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(app) = app_weak.upgrade() else {
                        return;
                    };
                    let (Some(state), Some(worker)) = (
                        crate::STATE.with(|s| s.borrow().clone()),
                        crate::WORKER.with(|w| w.borrow().clone()),
                    ) else {
                        return;
                    };
                    match chosen {
                        Ok(Some(dir)) => add(&state, &app, &worker, &dir),
                        Ok(None) => app.set_status("nothing added".into()),
                        Err(e) => {
                            tracing::warn!("file chooser: {e:#}");
                            app.set_status(chooser::chooser_failed(&e).into());
                        }
                    }
                });
            });
        });
    }
}

/// A test's turn at the reads of views and folders sent off the
/// window's thread: each run and landed, as the thread and the event
/// loop would; how many there were.
#[cfg(test)]
pub(crate) fn land_sent(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) -> usize {
    land_sent_in(state, app, worker, false)
}

/// [`land_sent`], the reads sent landing last first: a thread that was
/// sent later and came back sooner.
#[cfg(test)]
pub(crate) fn land_sent_last_first(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
) -> usize {
    land_sent_in(state, app, worker, true)
}

#[cfg(test)]
fn land_sent_in(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    reversed: bool,
) -> usize {
    let mut landed = 0;
    loop {
        let mut looks: Vec<Look> = tests::SENT.with(|s| s.borrow_mut().drain(..).collect());
        if reversed {
            looks.reverse();
        }
        // The folder trees a landing asked for, built and landed with
        // it; not counted, since they are not reads of the list.
        let trees = crate::tree::land_sent(state, app);
        if looks.is_empty() && trees == 0 {
            return landed;
        }
        for look in looks {
            land(state, app, worker, look.run());
            landed += 1;
        }
    }
}
