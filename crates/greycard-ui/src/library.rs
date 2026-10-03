//! The library index as the editor keeps it (notes §160): the open
//! folder indexed on a thread of its own, each sidecar write's row
//! brought up to date after it, and the filter bar's EXIF questions
//! answered from it.
//!
//! Two connections. The indexer's thread holds the one that writes:
//! `index_folder` when a folder opens and `index_file` after a save,
//! neither on the UI thread, so the first frame never waits for a
//! pass. A pass stops between batches when the window has moved to
//! another folder, or when a save's row is waiting, and takes up
//! again after. The UI thread holds one opened read-only once the
//! indexer has made the file, and asks it the filter's questions —
//! which frames pass the EXIF tests, and each facet's `GROUP BY`. Its
//! busy timeout is a few milliseconds: under write-ahead logging a
//! reader waits only while the log is recovered or checkpointed, and
//! the window keeps its last answer then rather than wait.
//!
//! The passes themselves run on a lane of their own, a thread with a
//! third connection, which the indexer's thread waits on. A root is
//! looked at before a pass over it begins, with the roots' 3 s look,
//! and one that does not answer is skipped; a pass that goes quiet in
//! the middle (a share that answered the look and then stopped) is set
//! aside on its lane after [`PASS_WAIT`], the same 3 s, and its root
//! is skipped until it is back, when its report lands as any other. So
//! a hung root holds the other roots' passes and their reports for that
//! once at most. The rows of the window's own saves (`index_file`) and
//! deletes (`forget`) are still written on the indexer's thread itself,
//! unguarded: one for a frame on a share that hangs holds it there.
//!
//! The keyword's chips are counted from the sidecars in hand, as the
//! meta rows are: the keyword is the sidecar's, the window has it
//! before any row does, and under `--no-sidecars` the rows say what
//! the disk says and the window does not.
//!
//! The database is the user's, `Library::user_path()`, unless
//! `--library` names another. A test never starts an indexer on it:
//! it opens a library of its own in its own directory, and the state
//! it builds has none.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use greycard_library::{Change, FacetCount, Library, Report};

use crate::filter::{self, Facet};
use crate::*;

/// How often a long pass tells the window how far it has got, so a
/// facet fills in and a frame the filter does not want goes, while
/// the pass is still running.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// How often a pass in the background says it has got further: less
/// often than a folder's, since each word has the list under the
/// roots read again.
const BACKGROUND_EVERY: Duration = Duration::from_secs(2);

/// The most chips a facet's row offers; the chips on are kept past
/// it. A zoom's focal lengths or a day of ISOs can run to dozens,
/// and the rest are a term away in the text field (`focal:70`).
pub(crate) const CHIPS_A_FACET: usize = 12;

/// How long the window's reads wait on a lock before keeping the
/// last answer.
const READ_WAIT: Duration = Duration::from_millis(20);

/// A pass that failed — the library locked by another process past
/// its timeout, say — is asked again this long after, this many
/// times, before the window says the index is unavailable.
const RETRY_AFTER: Duration = Duration::from_secs(2);
const RETRIES: u32 = 5;

/// The generation that stops every pass: the editor is leaving.
const LEAVING: u64 = u64::MAX;

/// What the window asks of the indexer.
enum Ask {
    /// Index these folders, the generation saying which open asked.
    Folders { dirs: Vec<PathBuf>, generation: u64 },
    /// Bring this file's row up to date after its sidecar was saved.
    File(PathBuf),
    /// Forget these files' rows: the window deleted the files.
    Forget(Vec<PathBuf>),
    /// Take these files' rows from the first path to the second: the
    /// window moved the files itself (Move rejects, Move back).
    Moved(Vec<(PathBuf, PathBuf)>),
    /// The launch pass: each root's tree, one after another, in the
    /// background of everything else; with the network mounts below
    /// them that their walks leave out, as the same look found them, so
    /// the two never arrive apart.
    Roots {
        roots: Vec<PathBuf>,
        left_out: Vec<PathBuf>,
    },
    /// What the watcher saw change under the roots, in the
    /// background too, but ahead of the launch pass.
    Changes(Vec<Change>),
    /// The timer's tick over the roots on a network mount: a tree pass
    /// over each, behind everything else ([`queue_poll`]).
    Poll(Vec<PathBuf>),
    /// The network mounts below the local roots, all of them as the
    /// last watcher's build found them: a local root's walk leaves them
    /// out, since each is passed over as its own root, so a share there
    /// that hangs holds its own pass and not the local root's.
    LeftOut(Vec<PathBuf>),
    /// The editor is leaving: wakes a thread waiting on the next
    /// ask, which the watcher's end of the channel would otherwise
    /// keep waiting.
    Leave,
}

/// A pass in the background: the launch pass over a root, a change the
/// watcher saw, or the timer's tree pass over a root on a network
/// mount.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Background {
    Root(PathBuf),
    Change(Change),
    Poll(PathBuf),
}

impl Background {
    fn path(&self) -> &Path {
        match self {
            Background::Root(p) | Background::Poll(p) => p,
            Background::Change(c) => c.path(),
        }
    }
}

/// What the indexer tells the window.
#[derive(Debug)]
pub(crate) enum Told {
    /// The library is open, at this path; the window may read it.
    Opened(PathBuf),
    /// It could not be opened, or the thread died, and nothing more
    /// will be indexed this session.
    Failed(String),
    /// A pass is `done` files of `total` into the folders asked for.
    Progress {
        generation: u64,
        done: usize,
        total: usize,
    },
    /// The folders asked for are indexed, or the pass over one of
    /// them failed with `error` — the library locked, most likely —
    /// and the rest were done.
    Indexed {
        generation: u64,
        report: Report,
        seconds: f64,
        error: Option<String>,
    },
    /// Rows after saves are up to date.
    FilesIndexed,
    /// The rows of files the window deleted are gone, this many.
    Forgotten(usize),
    /// A pass in the background is done: a root's tree for the
    /// launch pass (`launch`), or a folder or a tree the watcher saw
    /// change. `error` as for `Indexed`.
    Background {
        path: PathBuf,
        launch: bool,
        report: Report,
        seconds: f64,
        error: Option<String>,
    },
    /// A pass in the background has got further under `path`, and
    /// has written what it found so far.
    BackgroundProgress { path: PathBuf },
    /// A pass over `path` was not made, or was left partway: `root`
    /// did not answer the look before it, or stopped answering in the
    /// middle of it. Its rows are as they were, and the next pass over
    /// it (the next tick, the next change) tries again. `first` when
    /// this is the first word of it since the root last answered, so
    /// the window says it once. `generation` for the window's own
    /// folder pass, none for one in the background.
    Skipped {
        path: PathBuf,
        root: PathBuf,
        launch: bool,
        generation: Option<u64>,
        first: bool,
    },
}

/// The indexer's thread, and the way to ask it things.
pub(crate) struct Indexer {
    asks: mpsc::Sender<Ask>,
    /// The folder pass the window wants. A pass for any other stops
    /// at its next batch.
    wanted: Arc<AtomicU64>,
    /// Saves whose rows are waiting: a pass stops at its next batch
    /// to let them through, and takes up again after.
    files_waiting: Arc<AtomicUsize>,
    /// A folder the window opened is waiting: a pass in the
    /// background stops at its next batch for it.
    folders_waiting: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

/// The indexer's ear, for a thread that is not the window's: the
/// watcher hands its changes on through this.
#[derive(Clone)]
pub(crate) struct Asker(mpsc::Sender<Ask>);

impl Asker {
    pub(crate) fn changes(&self, changes: Vec<Change>) {
        let _ = self.0.send(Ask::Changes(changes));
    }

    /// A tree pass over each of `roots`, for the timer: see
    /// [`queue_poll`].
    pub(crate) fn poll(&self, roots: Vec<PathBuf>) {
        let _ = self.0.send(Ask::Poll(roots));
    }
}

/// How the indexer keeps a root that does not answer from holding the
/// rest: the look at a root before a pass over it, and how long a pass
/// may go without a word before it is set aside.
pub(crate) struct Guard {
    /// Whether a root answers, within whatever wait the look keeps.
    /// The editor's is the roots' 3 s look, one thread a root at most.
    pub(crate) answers: Box<dyn Fn(&Path) -> bool + Send>,
    /// A pass that has said nothing (not a file begun, not a folder
    /// left) for this long is set aside on its lane, where it goes on
    /// alone and lands if it ever comes back, and the indexer takes up
    /// the next pass on a fresh lane.
    pub(crate) wait: Duration,
    /// A test's hand in the pass: called before each file a pass in
    /// the background looks at, on the pass's thread.
    #[cfg(test)]
    pub(crate) each_file: Option<EachFile>,
}

/// A test's hand in each file a pass looks at.
#[cfg(test)]
pub(crate) type EachFile = Arc<dyn Fn(&Path) + Send + Sync>;

/// How long a pass may go without a word before it is set aside: the
/// roots' own wait (3 s). A word is a file begun, a batch written or a
/// folder left, each a few round trips. A share's disks waking from
/// sleep can take longer, which costs nothing here: the pass set aside
/// goes on, and its report lands when it is done; only the other roots'
/// passes no longer wait for it.
const PASS_WAIT: Duration = crate::roots::ROOT_WAIT;

/// How long the indexer waits on the way out for a lane that is not set
/// aside to leave: the pass in hand was moving a moment ago, so this is
/// its last file and its last batch.
const LANE_LEAVE: Duration = Duration::from_secs(5);

impl Default for Guard {
    fn default() -> Self {
        Guard {
            answers: Box::new(|root: &Path| crate::roots::answers(root)),
            wait: PASS_WAIT,
            #[cfg(test)]
            each_file: None,
        }
    }
}

impl Indexer {
    /// Open the library at `path` on a thread of its own and wait
    /// there for folders and files. `told` is called on that thread.
    pub(crate) fn start(
        path: PathBuf,
        told: impl Fn(Told) + Send + 'static,
    ) -> std::io::Result<Indexer> {
        Self::start_with(path, told, Guard::default())
    }

    /// [`Indexer::start`] with the looks and the stall of `guard`.
    pub(crate) fn start_with(
        path: PathBuf,
        told: impl Fn(Told) + Send + 'static,
        guard: Guard,
    ) -> std::io::Result<Indexer> {
        let (asks, waiting) = mpsc::channel();
        let wanted = Arc::new(AtomicU64::new(0));
        let files_waiting = Arc::new(AtomicUsize::new(0));
        let folders_waiting = Arc::new(AtomicBool::new(false));
        let waits = Waits {
            wanted: wanted.clone(),
            files: files_waiting.clone(),
            folders: folders_waiting.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("greycard index".into())
            .spawn(move || run(path, waiting, told, waits, guard))?;
        Ok(Indexer {
            asks,
            wanted,
            files_waiting,
            folders_waiting,
            thread,
        })
    }

    pub(crate) fn folders(&self, dirs: Vec<PathBuf>, generation: u64) {
        self.wanted.store(generation, Ordering::SeqCst);
        self.folders_waiting.store(true, Ordering::SeqCst);
        let _ = self.asks.send(Ask::Folders { dirs, generation });
    }

    /// The launch pass over the roots, in this order, behind every
    /// folder the window asks for and every save.
    pub(crate) fn roots(&self, roots: Vec<PathBuf>) {
        self.launch(roots, Vec::new());
    }

    /// [`Indexer::roots`], with the network mounts below them that their
    /// walks leave out, in the one ask.
    pub(crate) fn launch(&self, roots: Vec<PathBuf>, left_out: Vec<PathBuf>) {
        let _ = self.asks.send(Ask::Roots { roots, left_out });
    }

    /// The network mounts below the local roots, which their walks
    /// leave out ([`Ask::LeftOut`]).
    pub(crate) fn left_out(&self, points: Vec<PathBuf>) {
        let _ = self.asks.send(Ask::LeftOut(points));
    }

    pub(crate) fn asker(&self) -> Asker {
        Asker(self.asks.clone())
    }

    pub(crate) fn file(&self, path: PathBuf) {
        self.files_waiting.fetch_add(1, Ordering::SeqCst);
        if self.asks.send(Ask::File(path)).is_err() {
            self.files_waiting.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Forget the rows of files the window has deleted. A pass under
    /// way gives way to it as it does to a save, and it is done after
    /// any save asked before it, so a sidecar written just before the
    /// delete does not bring a row back.
    pub(crate) fn forget(&self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        self.files_waiting.fetch_add(1, Ordering::SeqCst);
        if self.asks.send(Ask::Forget(paths)).is_err() {
            self.files_waiting.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Take the rows of files the window has moved, each from the
    /// first path to the second, and read each again where it is now.
    /// Told rather than found: a pass over a folder the move emptied
    /// takes it for a drive that is away and sees no move. Done ahead
    /// of the saves waiting with it, so a sidecar written at the new
    /// path after the move finds the row there and does not add one.
    pub(crate) fn moved(&self, moves: Vec<(PathBuf, PathBuf)>) {
        if moves.is_empty() {
            return;
        }
        self.files_waiting.fetch_add(1, Ordering::SeqCst);
        if self.asks.send(Ask::Moved(moves)).is_err() {
            self.files_waiting.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Stop the pass at its next batch, and wait up to `within` for
    /// the thread to put the library down, so its connection closes
    /// and the write-ahead log and its index go with it: a test's
    /// directory cannot be removed while the file is open on
    /// Windows, and a session should not leave the two files behind.
    pub(crate) fn stop(self, within: Duration) {
        self.wanted.store(LEAVING, Ordering::SeqCst);
        // The watcher may still hold an `Asker`, so the channel does
        // not close with this end: the thread is woken to see
        // `LEAVING` instead.
        let _ = self.asks.send(Ask::Leave);
        drop(self.asks);
        let asked = Instant::now();
        while !self.thread.is_finished() && asked.elapsed() < within {
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.thread.is_finished() {
            let _ = self.thread.join();
        } else {
            tracing::warn!("index: still busy on the way out; leaving it");
        }
    }
}

/// What the window has asked for that a pass in hand gives way to.
#[derive(Clone)]
struct Waits {
    /// The folder pass the window wants.
    wanted: Arc<AtomicU64>,
    /// Saves whose rows are waiting.
    files: Arc<AtomicUsize>,
    /// A folder pass asked for and not yet taken up.
    folders: Arc<AtomicBool>,
}

/// The thread's body, a panic in it said to the window rather than
/// leaving it waiting on a pass that will never end. The probe
/// catches a decoder's panic at the file already; this is for
/// everything else.
fn run(
    path: PathBuf,
    waiting: mpsc::Receiver<Ask>,
    told: impl Fn(Told),
    waits: Waits,
    guard: Guard,
) {
    let told = &told;
    let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        serve(path, waiting, told, &waits, &guard)
    }));
    if let Err(payload) = held {
        told(Told::Failed(format!(
            "the indexer panicked: {}",
            crate::worker::panic_message(payload.as_ref())
        )));
    }
}

/// A folder pass under way, kept across the saves it stops for.
struct Pass {
    dirs: Vec<PathBuf>,
    generation: u64,
    started: Instant,
}

/// Put a pass in the background's queue, once: a folder the watcher
/// names twice before it is reached is one pass. The watcher's go
/// ahead of the launch pass's roots and the timer's ticks, which are
/// long and can wait.
fn queue(background: &mut VecDeque<Background>, job: Background) {
    if background.contains(&job) {
        return;
    }
    match job {
        Background::Change(_) => {
            let at = background
                .iter()
                .position(|b| matches!(b, Background::Root(_) | Background::Poll(_)))
                .unwrap_or(background.len());
            background.insert(at, job);
        }
        Background::Root(_) | Background::Poll(_) => background.push_back(job),
    }
}

/// The timer's tick over a root on a network mount: a tree pass over
/// it, at the back, behind the launch pass's roots, so a tick never
/// holds up a local root's launch pass. Nothing when a pass over the
/// whole root is already waiting: the launch pass's, one stopped
/// partway (a stopped pass goes back into the queue, and the queue is
/// only read between passes, so one under way is in it), or the last
/// tick's. So a slow walk of a large share is never doubled, and a
/// launch walk is never finished by a tick and then walked again. True
/// when it was queued.
fn queue_poll(background: &mut VecDeque<Background>, root: PathBuf) -> bool {
    // A pass over a tree above it covers it as well: a network mount
    // below a local root is walked by that root's launch pass.
    let whole = |b: &Background| match b {
        Background::Root(r) | Background::Poll(r) | Background::Change(Change::Tree(r)) => {
            root.starts_with(r)
        }
        Background::Change(Change::Folder(_)) => false,
    };
    if background.iter().any(whole) {
        return false;
    }
    background.push_back(Background::Poll(root));
    true
}

/// A word from a pass on the lane: that it is still moving, or one for
/// the window, handed on by the indexer's thread.
enum Word {
    Beat,
    Told(Box<Told>),
}

/// What the lane sends back while a pass runs.
enum Back<R> {
    Word(Word),
    Done(R),
    Panicked(Box<dyn std::any::Any + Send>),
}

type LaneJob = Box<dyn FnOnce(&mut Library) + Send>;

thread_local! {
    /// The pass in hand's word that it is still moving, for the lane's
    /// busy handler: a wait on the library's lock is the library's, not
    /// a share's that stopped answering.
    static LANE_BEAT: std::cell::RefCell<Option<Box<dyn Fn()>>> =
        const { std::cell::RefCell::new(None) };
}

/// How many times the lane's busy handler waits on a held lock before
/// the query answers busy: 10 ms each, so the five seconds of
/// rusqlite's own timeout.
const LANE_BUSY_TRIES: i32 = 500;

/// The lane's connection's busy handler: says the pass is moving, and
/// waits a moment, up to [`LANE_BUSY_TRIES`] times.
fn lane_busy(tries: i32) -> bool {
    LANE_BEAT.with(|b| {
        if let Some(beat) = &*b.borrow() {
            beat();
        }
    });
    if tries >= LANE_BUSY_TRIES {
        return false;
    }
    std::thread::sleep(Duration::from_millis(10));
    true
}

/// The thread the passes run on, with a connection of its own to the
/// library, so a pass gone quiet on a root that stopped answering can be
/// set aside there while the indexer goes on with the other roots on a
/// fresh lane. The pass's disk reads are made with no transaction open
/// (`index.rs`), so one set aside holds no lock the next pass wants,
/// unless it went quiet inside a batch's write, where a sidecar is read.
struct Lane {
    jobs: mpsc::Sender<LaneJob>,
    thread: std::thread::JoinHandle<()>,
}

impl Lane {
    fn start(path: &Path) -> Option<Lane> {
        let (jobs, taken) = mpsc::channel::<LaneJob>();
        let path = path.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("greycard index pass".into())
            .spawn(move || {
                let mut lib = match Library::open(&path) {
                    Ok(lib) => lib,
                    Err(e) => {
                        // Each job dropped untouched: the indexer hears
                        // its end of the reply go.
                        tracing::warn!("index: no connection for the passes: {e}");
                        return;
                    }
                };
                if let Err(e) = lib.set_busy_handler(lane_busy) {
                    tracing::debug!("index: the passes' busy handler: {e}");
                }
                // Ends when the lane is dropped: set aside, its pass done.
                for job in taken {
                    job(&mut lib);
                }
            });
        match spawned {
            Ok(thread) => Some(Lane { jobs, thread }),
            Err(e) => {
                tracing::warn!("index: no thread for the passes: {e}");
                None
            }
        }
    }

    /// The lane ended and waited for, up to `within`: its jobs' end
    /// dropped, its thread joined once it has left. Only for a lane
    /// still in the indexer's slot on the way out, which is idle or on
    /// a pass that is moving; one set aside is out of the slot and is
    /// never waited on. Without the wait the lane's connection outlives
    /// the indexer's `stop`, and a test on Windows that then removes its
    /// library finds the file still open.
    fn finish(self, within: Duration) {
        drop(self.jobs);
        let asked = Instant::now();
        while !self.thread.is_finished() && asked.elapsed() < within {
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.thread.is_finished() {
            let _ = self.thread.join();
        } else {
            tracing::warn!("index: the passes' lane is still busy on the way out; leaving it");
        }
    }
}

/// A pass set aside on its lane: what it will say, and when it is done.
struct Aside<R> {
    answers: mpsc::Receiver<Back<R>>,
}

/// How a pass on the lane went.
enum Ran<R> {
    Done(R),
    /// It said nothing for the guard's wait, and was set aside on its
    /// lane, which goes on with it alone.
    Aside(Aside<R>),
    /// The lane went (its connection would not open, or no thread).
    Lost,
}

/// One word or the end from a pass, handled on the indexer's thread:
/// a word for the window is handed on, a panic is the indexer's own.
/// The result when it is done.
fn heard<R>(back: Back<R>, told: &dyn Fn(Told)) -> Option<R> {
    match back {
        Back::Word(Word::Beat) => None,
        Back::Word(Word::Told(t)) => {
            told(*t);
            None
        }
        Back::Done(r) => Some(r),
        // Said to the window as the indexer's own, as it was when the
        // passes ran on its thread.
        Back::Panicked(p) => std::panic::resume_unwind(p),
    }
}

/// Run `job` on the lane, started if there is none, and wait for it
/// here, handing on its words to the window and taking each as a sign
/// it is moving. One that goes quiet for `wait` (or while the editor
/// leaves) is set aside with its lane, and the next pass gets a fresh
/// one: a share that answered its look and then stopped holds its own
/// pass and no other.
fn on_lane<R: Send + 'static>(
    lane: &mut Option<Lane>,
    path: &Path,
    wait: Duration,
    told: &dyn Fn(Told),
    leaving: &dyn Fn() -> bool,
    job: impl FnOnce(&mut Library, &dyn Fn(Word)) -> R + Send + 'static,
) -> Ran<R> {
    if lane.is_none() {
        *lane = Lane::start(path);
    }
    let Some(running) = lane.as_ref() else {
        return Ran::Lost;
    };
    let (back, answers) = mpsc::channel::<Back<R>>();
    let boxed: LaneJob = Box::new(move |lib| {
        let word = |w: Word| {
            let _ = back.send(Back::Word(w));
        };
        let beat = back.clone();
        LANE_BEAT.with(|b| {
            *b.borrow_mut() = Some(Box::new(move || {
                let _ = beat.send(Back::Word(Word::Beat));
            }))
        });
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(lib, &word)));
        LANE_BEAT.with(|b| *b.borrow_mut() = None);
        let _ = back.send(match out {
            Ok(r) => Back::Done(r),
            Err(p) => Back::Panicked(p),
        });
    });
    if running.jobs.send(boxed).is_err() {
        *lane = None;
        return Ran::Lost;
    }
    let tick = wait.min(Duration::from_millis(200));
    let mut last = Instant::now();
    loop {
        match answers.recv_timeout(tick) {
            Ok(back) => {
                last = Instant::now();
                if let Some(r) = heard(back, told) {
                    return Ran::Done(r);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last.elapsed() >= wait || leaving() {
                    *lane = None;
                    return Ran::Aside(Aside { answers });
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                *lane = None;
                return Ran::Lost;
            }
        }
    }
}

/// The root a pass's path is under, of the roots the indexer has been
/// asked to pass over: what its look is at, and what a pass set aside
/// is kept against.
fn root_for(known: &[PathBuf], path: &Path) -> Option<PathBuf> {
    known
        .iter()
        .filter(|r| path.starts_with(r))
        .max_by_key(|r| r.as_os_str().len())
        .cloned()
}

/// What a pass in the background brings back: the pass's report, and
/// the walk to take up again when it stopped partway.
type Passed = (
    greycard_library::Result<Report>,
    Option<greycard_library::TreeWalk>,
);

/// A pass in the background set aside on its lane.
struct AsideJob {
    job: Background,
    tree: Option<PathBuf>,
    key: PathBuf,
    started: Instant,
    aside: Aside<Passed>,
}

/// The window's folder pass set aside on its lane.
struct AsideFolder {
    key: PathBuf,
    aside: Aside<Option<Pass>>,
}

/// The passes set aside, and the roots said to the window as not
/// answering since they last did.
#[derive(Default)]
struct Hung {
    jobs: Vec<AsideJob>,
    folders: Vec<AsideFolder>,
    said: HashSet<PathBuf>,
}

impl Hung {
    /// Whether a pass over `path` would meet a pass still set aside: it
    /// is in a tree set aside, or, for a pass over a whole tree (`tree`),
    /// one set aside is in it, unless that one is a mount the tree's walk
    /// leaves out (`left_out`). Such a pass is skipped, with no look,
    /// until the one set aside is back, so no two passes walk one tree.
    fn out(&self, path: &Path, tree: bool, left_out: &[PathBuf]) -> bool {
        let meets = |key: &PathBuf| {
            path.starts_with(key) || (tree && key.starts_with(path) && !left_out.contains(key))
        };
        self.jobs.iter().any(|a| meets(&a.key)) || self.folders.iter().any(|a| meets(&a.key))
    }

    /// `root` did not answer: true the first time since it last did.
    fn first(&mut self, root: &Path) -> bool {
        self.said.insert(root.to_path_buf())
    }

    /// `root` answered: the next time it does not is said again.
    fn answered(&mut self, root: &Path) {
        self.said.remove(root);
    }
}

/// What the indexer's thread holds for the passes it hands the lane.
struct Held<'a> {
    lane: &'a mut Option<Lane>,
    path: &'a Path,
    hung: &'a mut Hung,
    known: &'a [PathBuf],
    left_out: &'a [PathBuf],
}

fn serve(
    path: PathBuf,
    waiting: mpsc::Receiver<Ask>,
    told: &dyn Fn(Told),
    waits: &Waits,
    guard: &Guard,
) {
    let mut lib = match Library::open(&path) {
        Ok(lib) => lib,
        Err(e) => {
            told(Told::Failed(format!("{}: {e}", path.display())));
            return;
        }
    };
    told(Told::Opened(path.clone()));
    let wanted = &*waits.wanted;
    let files_waiting = &*waits.files;
    let leaving = || wanted.load(Ordering::SeqCst) == LEAVING;
    let mut pending: Option<Pass> = None;
    let mut background: VecDeque<Background> = VecDeque::new();
    // The tree passes stopped partway, by their folder.
    let mut walks: HashMap<PathBuf, greycard_library::TreeWalk> = HashMap::new();
    // The passes run on a lane of their own, so one gone quiet on a root
    // that stopped answering is set aside there and the rest go on.
    let mut lane: Option<Lane> = None;
    let mut hung = Hung::default();
    // The roots asked to be passed over, for each pass's look.
    let mut known: Vec<PathBuf> = Vec::new();
    // The network mounts below local roots, left out of their walks.
    let mut left_out: Vec<PathBuf> = Vec::new();
    loop {
        if leaving() {
            if let Some(lane) = lane.take() {
                lane.finish(LANE_LEAVE);
            }
            return;
        }
        // The passes set aside that have come back since, or said
        // something: landed as if they had never been set aside.
        let mut index = 0;
        while index < hung.jobs.len() {
            match hung.jobs[index].aside.answers.try_recv() {
                Ok(back) => {
                    if let Some(passed) = heard(back, told) {
                        let a = hung.jobs.remove(index);
                        tracing::info!("index: the pass over {} came back", a.job.path().display());
                        hung.answered(&a.key);
                        if let Some(job) =
                            background_done(a.job, a.tree, a.started, passed, &mut walks, told)
                        {
                            background.push_front(job);
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => index += 1,
                // The lane died with it (it panicked past its catch):
                // nothing more will come, and the root is free again.
                Err(mpsc::TryRecvError::Disconnected) => {
                    hung.jobs.remove(index);
                }
            }
        }
        let mut index = 0;
        while index < hung.folders.len() {
            match hung.folders[index].aside.answers.try_recv() {
                Ok(back) => {
                    if let Some(left) = heard(back, told) {
                        let a = hung.folders.remove(index);
                        hung.answered(&a.key);
                        // Stopped for a save meanwhile: taken up again
                        // when it is still the folder the window wants.
                        if let Some(pass) = left
                            && pending.is_none()
                            && wanted.load(Ordering::SeqCst) == pass.generation
                        {
                            pending = Some(pass);
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => index += 1,
                Err(mpsc::TryRecvError::Disconnected) => {
                    hung.folders.remove(index);
                }
            }
        }
        // With a pass to take up again, only what is already waiting;
        // with passes set aside, the next ask or a moment, to look at
        // them again; with neither, wait for the next ask.
        let first = if pending.is_some() || !background.is_empty() {
            waiting.try_recv().ok()
        } else if !hung.jobs.is_empty() || !hung.folders.is_empty() {
            match waiting.recv_timeout(Duration::from_millis(200)) {
                Ok(ask) => Some(ask),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match waiting.recv() {
                Ok(ask) => Some(ask),
                Err(_) => return,
            }
        };
        // Everything waiting, at once: the files deduplicated, and
        // of the folders only the newest, since a folder the window
        // has already left is nobody's question.
        let mut files: Vec<PathBuf> = Vec::new();
        let mut forget: Vec<PathBuf> = Vec::new();
        let mut moves: Vec<(PathBuf, PathBuf)> = Vec::new();
        for ask in first.into_iter().chain(waiting.try_iter()) {
            match ask {
                Ask::Moved(more) => {
                    files_waiting.fetch_sub(1, Ordering::SeqCst);
                    moves.extend(more);
                }
                Ask::File(p) => {
                    files_waiting.fetch_sub(1, Ordering::SeqCst);
                    if !files.contains(&p) {
                        files.push(p);
                    }
                }
                Ask::Forget(paths) => {
                    files_waiting.fetch_sub(1, Ordering::SeqCst);
                    forget.extend(paths);
                }
                Ask::Folders { dirs, generation } => {
                    waits.folders.store(false, Ordering::SeqCst);
                    pending = Some(Pass {
                        dirs,
                        generation,
                        started: Instant::now(),
                    });
                }
                Ask::Roots {
                    roots,
                    left_out: more,
                } => {
                    // Added to what is left out, not put in its place: a
                    // launch's look from a build since overtaken knows
                    // less than the newer build's `LeftOut`, never more
                    // that is wrong to leave out.
                    for point in more {
                        if !known.contains(&point) {
                            known.push(point.clone());
                        }
                        if !left_out.contains(&point) {
                            left_out.push(point);
                        }
                    }
                    for root in roots {
                        if !known.contains(&root) {
                            known.push(root.clone());
                        }
                        queue(&mut background, Background::Root(root));
                    }
                }
                Ask::Changes(changes) => {
                    for change in changes {
                        queue(&mut background, Background::Change(change));
                    }
                }
                Ask::Poll(roots) => {
                    for root in roots {
                        if !known.contains(&root) {
                            known.push(root.clone());
                        }
                        if !queue_poll(&mut background, root.clone()) {
                            tracing::debug!(
                                "index: {} is still to be passed over; the timer's tick skipped",
                                root.display()
                            );
                        }
                    }
                }
                Ask::LeftOut(points) => {
                    for point in &points {
                        if !known.contains(point) {
                            known.push(point.clone());
                        }
                    }
                    left_out = points;
                }
                Ask::Leave => return,
            }
        }
        // The window's own moves first, so the saves after them find
        // each row at its new path; each read again there.
        for (from, to) in &moves {
            match lib.file_moved(from, to) {
                Ok(_) => {
                    if !files.contains(to) {
                        files.push(to.clone());
                    }
                }
                Err(e) => tracing::warn!("index: {} moved, its row not: {e}", from.display()),
            }
        }
        if !files.is_empty() {
            for f in &files {
                if let Err(e) = lib.index_file(f) {
                    tracing::warn!("index: {}: {e}", f.display());
                }
            }
            told(Told::FilesIndexed);
        }
        // After the saves: a save of a frame deleted since is a row
        // written for a file that is gone, and this takes it away.
        if !forget.is_empty() {
            match lib.forget(&forget) {
                Ok(n) => told(Told::Forgotten(n)),
                Err(e) => tracing::warn!("index: rows of deleted files not forgotten: {e}"),
            }
        }
        if let Some(pass) = pending.take() {
            let mut held = Held {
                lane: &mut lane,
                path: &path,
                hung: &mut hung,
                known: &known,
                left_out: &left_out,
            };
            pending = window_pass_guarded(&mut held, pass, told, waits, guard);
            continue;
        }
        let Some(job) = background.pop_front() else {
            continue;
        };
        let tree = match &job {
            Background::Root(dir)
            | Background::Poll(dir)
            | Background::Change(Change::Tree(dir)) => Some(dir.clone()),
            Background::Change(Change::Folder(_)) => None,
        };
        // The root the pass is under, looked at before a pass is begun
        // (not when one stopped partway is taken up again): a root that
        // does not answer is skipped, its rows left as they are, and the
        // other roots' passes go on. One whose last pass was set aside
        // is skipped without a look until that pass is back.
        let root = root_for(&known, job.path());
        let key = root.clone().unwrap_or_else(|| job.path().to_path_buf());
        let resumed = tree.as_ref().is_some_and(|d| walks.contains_key(d));
        let skip = if hung.out(job.path(), tree.is_some(), &left_out) {
            true
        } else if let (Some(root), false) = (&root, resumed) {
            !(guard.answers)(root)
        } else {
            false
        };
        if skip {
            if let Some(dir) = &tree {
                walks.remove(dir);
            }
            let first = hung.first(&key);
            if first {
                tracing::warn!(
                    "index: {} does not answer; its pass is skipped and its rows kept as they are",
                    key.display()
                );
            }
            told(Told::Skipped {
                path: job.path().to_path_buf(),
                root: key,
                launch: matches!(job, Background::Root(_)),
                generation: None,
                first,
            });
            continue;
        }
        let started = Instant::now();
        let walk = tree.as_ref().and_then(|d| walks.remove(d));
        let job_on_lane = job.clone();
        let tree_on_lane = tree.clone();
        let waits_on_lane = waits.clone();
        let leave_out = left_out.clone();
        #[cfg(test)]
        let each_file = guard.each_file.clone();
        let ran = on_lane(
            &mut lane,
            &path,
            guard.wait,
            told,
            &leaving,
            move |lib, word| -> Passed {
                let job = job_on_lane;
                let waits = waits_on_lane;
                // In the background: anything the window asks for comes
                // first, and this pass takes up again after it, from
                // where it stopped (a tree's walk is kept for that).
                let stop = || {
                    word(Word::Beat);
                    waits.wanted.load(Ordering::SeqCst) == LEAVING
                        || waits.files.load(Ordering::SeqCst) > 0
                        || waits.folders.load(Ordering::SeqCst)
                };
                // A long pass says now and then that it has written rows,
                // so a view of the roots fills in while a large root is
                // walked for the first time rather than all at once at
                // the end; and says nothing while it finds nothing new.
                let mut last = Instant::now();
                let mut written: (PathBuf, usize) = (PathBuf::new(), 0);
                let mut unsaid = false;
                let mut progress = |p: greycard_library::Progress<'_>| {
                    word(Word::Beat);
                    #[cfg(test)]
                    if let Some(hold) = &each_file {
                        hold(p.path);
                    }
                    let folder = p.path.parent().unwrap_or(Path::new(""));
                    if folder != written.0 {
                        written = (folder.to_path_buf(), 0);
                    }
                    if p.written > written.1 {
                        written.1 = p.written;
                        unsaid = true;
                    }
                    if unsaid && last.elapsed() >= BACKGROUND_EVERY {
                        last = Instant::now();
                        unsaid = false;
                        word(Word::Told(Box::new(Told::BackgroundProgress {
                            path: job.path().to_path_buf(),
                        })));
                    }
                };
                match (&job, tree_on_lane) {
                    (_, Some(dir)) => {
                        // A walk taken up again leaves out what is left out
                        // now: a mount found since it began (the roots
                        // changed) is not walked into by it.
                        let walk = match walk {
                            Some(w) => Ok(w),
                            None => lib.tree_walk(&dir),
                        }
                        .map(|w| w.leaving_out(&leave_out));
                        match walk {
                            Ok(mut w) => {
                                let r = lib.walk_until(&mut w, &mut progress, &stop);
                                let keep = r.as_ref().is_ok_and(|r| r.stopped).then_some(w);
                                (r, keep)
                            }
                            Err(e) => (Err(e), None),
                        }
                    }
                    (Background::Change(Change::Folder(dir)), None) => {
                        (lib.index_folder_until(dir, &mut progress, &stop), None)
                    }
                    _ => unreachable!("a tree job has its folder"),
                }
            },
        );
        let passed = match ran {
            Ran::Done(passed) => {
                hung.answered(&key);
                passed
            }
            Ran::Aside(aside) => {
                if leaving() {
                    return;
                }
                let first = hung.first(&key);
                tracing::warn!(
                    "index: the pass over {} has said nothing for {} s; set aside, and {} \
                     skipped until it is back",
                    job.path().display(),
                    guard.wait.as_secs_f64(),
                    key.display()
                );
                told(Told::Skipped {
                    path: job.path().to_path_buf(),
                    root: key.clone(),
                    launch: matches!(job, Background::Root(_)),
                    generation: None,
                    first,
                });
                // A launch pass is counted done by the word above: when it
                // lands, or is taken up again after stopping for a save,
                // it is a pass over the root and no longer the launch's,
                // so the window does not count it a second time.
                let job = match job {
                    Background::Root(dir) => Background::Poll(dir),
                    other => other,
                };
                hung.jobs.push(AsideJob {
                    job,
                    tree,
                    key,
                    started,
                    aside,
                });
                continue;
            }
            Ran::Lost => (
                Err(greycard_library::Error::Io(std::io::Error::other(
                    "the passes' connection to the library could not be had",
                ))),
                None,
            ),
        };
        if let Some(job) = background_done(job, tree, started, passed, &mut walks, told) {
            background.push_front(job);
        }
    }
}

/// A pass in the background done, on the lane or after being set aside:
/// a walk stopped partway kept, and the job handed back to go to the
/// front of the queue again; else its report told to the window.
fn background_done(
    job: Background,
    tree: Option<PathBuf>,
    started: Instant,
    (passed, keep): Passed,
    walks: &mut HashMap<PathBuf, greycard_library::TreeWalk>,
    told: &dyn Fn(Told),
) -> Option<Background> {
    if let (Some(dir), Some(w)) = (&tree, keep) {
        walks.insert(dir.clone(), w);
    }
    let (report, error) = match passed {
        Ok(r) if r.stopped => return Some(job),
        Ok(r) => (r, None),
        Err(e) => {
            // A folder the watcher named that went again before it was
            // reached, most likely: the pass over its parent, which the
            // same event brings, says it.
            tracing::debug!("index: {}: {e}", job.path().display());
            (Report::default(), Some(e.to_string()))
        }
    };
    told(Told::Background {
        path: job.path().to_path_buf(),
        launch: matches!(job, Background::Root(_)),
        report,
        seconds: started.elapsed().as_secs_f64(),
        error,
    });
    None
}

/// The window's folder pass on the lane, under the same guard as the
/// passes in the background: a folder on a share that stops answering
/// in the middle of it is set aside, said, and the indexer goes on; it
/// says its report if it comes back. What is left of the pass to take
/// up again, if anything.
fn window_pass_guarded(
    held: &mut Held<'_>,
    pass: Pass,
    told: &dyn Fn(Told),
    waits: &Waits,
    guard: &Guard,
) -> Option<Pass> {
    if waits.wanted.load(Ordering::SeqCst) != pass.generation {
        return None;
    }
    let generation = pass.generation;
    let first_dir = pass.dirs.first().cloned().unwrap_or_default();
    let key = root_for(held.known, &first_dir).unwrap_or_else(|| first_dir.clone());
    let skipped = |hung: &mut Hung, key: PathBuf| {
        let first = hung.first(&key);
        told(Told::Skipped {
            path: first_dir.clone(),
            root: key,
            launch: false,
            generation: Some(generation),
            first,
        });
    };
    // A folder's pass reads its folders alone, not the trees under them.
    if pass
        .dirs
        .iter()
        .any(|d| held.hung.out(d, false, held.left_out))
    {
        skipped(held.hung, key);
        return None;
    }
    let waits_on_lane = waits.clone();
    let leaving = || waits.wanted.load(Ordering::SeqCst) == LEAVING;
    #[cfg(test)]
    let each_file = guard.each_file.clone();
    let ran = on_lane(
        held.lane,
        held.path,
        guard.wait,
        told,
        &leaving,
        move |lib, word| {
            let beat = |_file: Option<&Path>| {
                word(Word::Beat);
                #[cfg(test)]
                if let (Some(hold), Some(file)) = (&each_file, _file) {
                    hold(file);
                }
            };
            window_pass(
                lib,
                pass,
                &|t| word(Word::Told(Box::new(t))),
                &beat,
                &waits_on_lane,
            )
        },
    );
    match ran {
        Ran::Done(left) => {
            held.hung.answered(&key);
            left
        }
        Ran::Aside(aside) => {
            tracing::warn!(
                "index: the pass over {} has said nothing for {} s; set aside",
                first_dir.display(),
                guard.wait.as_secs_f64()
            );
            skipped(held.hung, key.clone());
            held.hung.folders.push(AsideFolder { key, aside });
            None
        }
        Ran::Lost => {
            told(Told::Indexed {
                generation,
                report: Report::default(),
                seconds: 0.0,
                error: Some("the passes' connection to the library could not be had".into()),
            });
            None
        }
    }
}

/// The window's folder pass, stopped for a save or for a folder
/// since opened: what is left of it to take up again, if anything.
fn window_pass(
    lib: &mut Library,
    pass: Pass,
    told: &dyn Fn(Told),
    beat: &dyn Fn(Option<&Path>),
    waits: &Waits,
) -> Option<Pass> {
    let wanted = &*waits.wanted;
    let files_waiting = &*waits.files;
    let generation = pass.generation;
    if wanted.load(Ordering::SeqCst) != generation {
        return None;
    }
    let stop = || {
        beat(None);
        wanted.load(Ordering::SeqCst) != generation || files_waiting.load(Ordering::SeqCst) > 0
    };
    let mut report = Report::default();
    let mut error = None;
    let mut last = Instant::now();
    let mut before = 0;
    for dir in &pass.dirs {
        let passed = lib.index_folder_until(
            dir,
            &mut |p| {
                beat(Some(p.path));
                if last.elapsed() >= PROGRESS_EVERY {
                    last = Instant::now();
                    told(Told::Progress {
                        generation,
                        done: before + p.done,
                        total: before + p.total,
                    });
                }
            },
            &stop,
        );
        match passed {
            Ok(r) => {
                before += r.seen();
                let stopped = r.stopped;
                report.added += r.added;
                report.moved += r.moved;
                report.changed += r.changed;
                report.changed_files.extend(r.changed_files);
                report.meta_refreshed += r.meta_refreshed;
                report.unchanged += r.unchanged;
                report.styled += r.styled;
                report.returned += r.returned;
                report.missing += r.missing;
                report.unavailable.extend(r.unavailable);
                report.errors.extend(r.errors);
                if stopped {
                    report.stopped = true;
                    break;
                }
            }
            Err(e) => {
                tracing::warn!("index: {}: {e}", dir.display());
                error = Some(e.to_string());
            }
        }
    }
    if report.stopped {
        // For a save: taken up again once its row is written, the
        // files already done passing as unchanged. For a folder
        // since left: dropped.
        return (wanted.load(Ordering::SeqCst) == generation).then_some(pass);
    }
    told(Told::Indexed {
        generation,
        report,
        seconds: pass.started.elapsed().as_secs_f64(),
        error,
    });
    None
}

/// The folders a list of files is in, each once, in the order first
/// met.
pub(crate) fn folders_of(files: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for f in files {
        let dir = match f.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        if !out.contains(&dir) {
            out.push(dir);
        }
    }
    out
}

/// Ask the indexer, if there is one, for the folders of the files
/// now open. The files' rows as the index already holds them are
/// read at once: a folder indexed last week has its facets before
/// this pass has looked at a file. When the read that brought the
/// list asked the index already, `ids` are its answers, and the rows
/// are not read again here.
pub(crate) fn index_open_folder(st: &mut State, ids: Option<Vec<Option<i64>>>) {
    match ids {
        Some(ids) if ids.len() == st.files.len() => st.index_ids = ids,
        // Each folder's canonical form as the read that brought the
        // list made it, off the window's thread: a folder opened by
        // hand is made canonical again by every read of it, so a link
        // pointed elsewhere since is followed there and not here.
        _ => {
            refresh_ids(st);
        }
    }
    let Some(indexer) = &st.index else {
        return;
    };
    // The all-roots view's list came from the index: the launch pass
    // and the watcher keep its rows, and a pass over every folder
    // under every root for it would be the launch pass again. A folder
    // pass still running for the list before is dropped, by the
    // generation, so it does not report over the view.
    if st.view.lists_rows() {
        st.index_generation += 1;
        st.index_progress = None;
        indexer.folders(Vec::new(), st.index_generation);
        return;
    }
    st.index_generation += 1;
    st.index_progress = Some((0, st.files.len()));
    indexer.folders(folders_of(&st.files), st.index_generation);
}

/// A frame's sidecar was written: its row is brought up to date on
/// the indexer's thread, and the window hears when it is.
pub(crate) fn sidecar_written(st: &State, file: usize) {
    if let (Some(indexer), Some(path)) = (&st.index, st.files.get(file)) {
        indexer.file(path.clone());
    }
}

/// The window's read-only connection, opened once the indexer has
/// made the file, and tried again at each word from the indexer
/// while it will not open.
fn open_reader(st: &mut State) {
    let Some(path) = st.index_path.clone() else {
        return;
    };
    if st.index_reader.is_some() {
        return;
    }
    let opened = Library::open_read_only(&path).and_then(|lib| {
        lib.set_busy_timeout(READ_WAIT)?;
        Ok(lib)
    });
    match opened {
        Ok(lib) => {
            st.index_reader = Some(lib);
            if st.index_error.as_deref() == Some(READER_UNAVAILABLE) {
                st.index_error = None;
            }
        }
        Err(e) => {
            tracing::warn!("library index {}: {e}", path.display());
            st.index_error = Some(READER_UNAVAILABLE.to_string());
        }
    }
}

const READER_UNAVAILABLE: &str = "The library index could not be read";

/// Each file's row id, as the index holds it now; `None` for a file
/// it has no row for yet, and for every file while there is no index.
/// A read that fails — busy past its few milliseconds — keeps the
/// last answer. In a folder's list, each frame still standing in from
/// its row takes the row as it is now (`rows::apply_row`), so a pass
/// that read a changed sidecar shows at the next word from the
/// indexer; true when some frame's badges, turns or filter answer
/// changed that way, and the browser's rows want rebuilding.
///
/// Only the ids are read for the whole list, one column a row, since
/// this runs on the window's thread at every word from the indexer.
/// The rows themselves, meta and keywords parsed, are read for the
/// frames standing in, and only in a folder's list: a view of the
/// roots has its rows brought by the merge its pass starts, off the
/// window's thread (`roots::background_done`).
pub(crate) fn refresh_ids(st: &mut State) -> bool {
    let started = Instant::now();
    if st.index_reader.is_none() {
        st.index_ids = vec![None; st.files.len()];
        return false;
    }
    let standing: Vec<usize> = if st.view.lists_rows() {
        Vec::new()
    } else {
        (0..st.files.len())
            .filter(|&i| !crate::rows::is_loaded(st, i))
            .collect()
    };
    let paths: Vec<PathBuf> = standing.iter().map(|&i| st.files[i].clone()).collect();
    let (ids, rows) = {
        let lib = st.index_reader.as_ref().expect("a reader, looked at above");
        // Each folder's canonical form as the window has kept it: a
        // read off the window's thread hands them over with the list,
        // so the disk is asked here only for a folder no read has seen
        // yet.
        let known = &mut st.library.canonical;
        let mut canonical = |dir: &std::path::Path| {
            known
                .entry(dir.to_path_buf())
                .or_insert_with(|| greycard_library::key_folder(dir))
                .clone()
        };
        let ids = lib.ids_of_with(&st.files, &mut canonical);
        let rows = if paths.is_empty() || ids.is_err() {
            None
        } else {
            Some(lib.rows_of_with(&paths, &mut canonical))
        };
        (ids, rows)
    };
    match ids {
        Ok(ids) => st.index_ids = ids,
        Err(e) => {
            tracing::debug!("index: {e}; keeping the last answer");
            if st.index_ids.len() != st.files.len() {
                st.index_ids = vec![None; st.files.len()];
            }
            return false;
        }
    }
    let mut moved = false;
    match rows {
        Some(Ok(rows)) => {
            for (i, row) in standing.into_iter().zip(rows) {
                if let Some(row) = row {
                    moved |= crate::rows::apply_row(st, i, &row);
                }
            }
        }
        Some(Err(e)) => tracing::debug!("index: {e}; the rows standing in kept"),
        None => {}
    }
    tracing::debug!(
        "index: {} of {} frames have rows, read in {:.1} ms{}",
        st.index_ids.iter().flatten().count(),
        st.files.len(),
        started.elapsed().as_secs_f64() * 1e3,
        if moved { "; some meta moved" } else { "" }
    );
    moved
}

/// Which frames the index's tests pass, as [`filter::Frame::index`]
/// reads them: every frame when nothing is asked of the index, or
/// there is none; else a frame with a row passes when the row does,
/// and a frame with none passes until it has one. A failed read
/// keeps the last answer.
pub(crate) fn index_pass(st: &State) -> Vec<bool> {
    let typed = st.filter.typed();
    let everyone = vec![true; st.files.len()];
    let Some(lib) = &st.index_reader else {
        return everyone;
    };
    if !st.filter.asks_index(&typed) || st.index_ids.len() != st.files.len() {
        return everyone;
    }
    let started = Instant::now();
    let ids: Vec<i64> = st.index_ids.iter().flatten().copied().collect();
    match lib.ids_passing(&ids, &st.filter.index_filter(&typed, None)) {
        Ok(passing) => {
            tracing::debug!(
                "index: {} of {} rows pass in {:.1} ms",
                passing.len(),
                ids.len(),
                started.elapsed().as_secs_f64() * 1e3
            );
            st.index_ids
                .iter()
                .map(|id| id.is_none_or(|id| passing.contains(&id)))
                .collect()
        }
        Err(e) => {
            tracing::debug!("index: {e}; keeping the last answer");
            if st.index_passed.len() == st.files.len() {
                st.index_passed.clone()
            } else {
                everyone
            }
        }
    }
}

/// The keyword's chips, from the sidecars in hand: each keyword,
/// case folded, and how many frames hold it among those the other
/// groups leave — the meta tests, and the index's as the list was
/// last made. The chip shows the first spelling met, in file order.
fn keyword_counts(st: &State, typed: &filter::Typed) -> Vec<FacetCount> {
    let mut by: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for (i, (path, s)) in st.files.iter().zip(&st.sidecars).enumerate() {
        let frame = filter::Frame {
            path,
            meta: &s.meta,
            index: st.index_passed.get(i).copied().unwrap_or(true),
        };
        if !frame.index || !st.filter.shows_meta(typed, frame, false) {
            continue;
        }
        let mut seen = HashSet::new();
        for word in &s.meta.keywords {
            let folded = word.to_lowercase();
            if seen.insert(folded.clone()) {
                by.entry(folded).or_insert_with(|| (word.clone(), 0)).1 += 1;
            }
        }
    }
    let mut counts: Vec<FacetCount> = by
        .into_iter()
        .map(|(value, (label, count))| FacetCount {
            value,
            label,
            count,
        })
        .collect();
    counts.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    counts
}

/// Every facet's chips: each value among the frames the filter's
/// other tests leave, with its count — the rule the meta rows keep
/// (`filter::Counts`). The EXIF facets are one `GROUP BY` each on
/// the index: the tests the sidecars answer pick the rows
/// (`within`), and the index's own tests, less the facet's, are the
/// query's filter. The keyword is counted from the sidecars. A read
/// that fails keeps the last answer.
pub(crate) fn facet_counts(st: &State) -> Vec<(Facet, Vec<FacetCount>)> {
    let typed = st.filter.typed();
    let keywords = (Facet::Keyword, keyword_counts(st, &typed));
    let Some(lib) = &st.index_reader else {
        return vec![keywords];
    };
    if st.index_ids.len() != st.files.len() {
        return vec![keywords];
    }
    let within: Vec<i64> = st
        .files
        .iter()
        .zip(&st.sidecars)
        .zip(&st.index_ids)
        .filter_map(|((path, s), id)| {
            let frame = filter::Frame {
                path,
                meta: &s.meta,
                index: true,
            };
            id.filter(|_| st.filter.shows_meta(&typed, frame, true))
        })
        .collect();
    let mut rows = Vec::new();
    for facet in Facet::ALL {
        if facet == Facet::Keyword {
            continue;
        }
        let asked = st.filter.index_filter(&typed, Some(facet));
        match lib.facet_counts(facet, Some(&within), &asked) {
            Ok(counts) => rows.push((facet, counts)),
            Err(e) => {
                tracing::debug!("index: {facet:?}: {e}; keeping the last answer");
                let mut last = st.facets_last.borrow().clone();
                last.retain(|(f, _)| *f != Facet::Keyword);
                last.push(keywords);
                return last;
            }
        }
    }
    rows.push(keywords);
    *st.facets_last.borrow_mut() = rows.clone();
    rows
}

/// A facet's chips as the bar shows them: at most [`CHIPS_A_FACET`]
/// of the values not on — every value held more often than the
/// twelfth most held, then as many of those tied with it as there is
/// room for, in the facet's order — and every value on, whatever its
/// count, a value no frame holds any more among them at zero, since
/// a chip on is how the filter is undone. `label_of` names such a
/// value. The chips stand in the facet's own order.
pub(crate) fn chips_of(
    facet: Facet,
    counts: &[FacetCount],
    on: &[String],
    label_of: &dyn Fn(&str) -> String,
) -> Vec<FacetChip> {
    let off: Vec<&FacetCount> = counts.iter().filter(|c| !on.contains(&c.value)).collect();
    let keep: HashSet<&str> = if off.len() > CHIPS_A_FACET {
        let mut by_count: Vec<usize> = off.iter().map(|c| c.count).collect();
        by_count.sort_unstable_by(|a, b| b.cmp(a));
        let cut = by_count[CHIPS_A_FACET - 1];
        let mut keep: HashSet<&str> = off
            .iter()
            .filter(|c| c.count > cut)
            .map(|c| c.value.as_str())
            .collect();
        for c in off.iter().filter(|c| c.count == cut) {
            if keep.len() >= CHIPS_A_FACET {
                break;
            }
            keep.insert(&c.value);
        }
        keep
    } else {
        off.iter().map(|c| c.value.as_str()).collect()
    };
    let mut chips: Vec<FacetChip> = counts
        .iter()
        .filter(|c| on.contains(&c.value) || keep.contains(c.value.as_str()))
        .map(|c| FacetChip {
            text: filter::facet_chip_text(facet, &c.label).into(),
            key: c.value.clone().into(),
            count: c.count as i32,
            on: on.contains(&c.value),
        })
        .collect();
    for value in on {
        if !counts.iter().any(|c| c.value == *value) {
            chips.push(FacetChip {
                text: filter::facet_chip_text(facet, &label_of(value)).into(),
                key: value.clone().into(),
                count: 0,
                on: true,
            });
        }
    }
    chips
}

/// The rows the bar shows: a facet with nothing to offer and nothing
/// on is left out.
pub(crate) fn facet_rows(st: &State) -> Vec<FacetRow> {
    let started = Instant::now();
    let counts = facet_counts(st);
    if st.index_reader.is_some() {
        tracing::debug!(
            "index: six facets counted over {} frames in {:.1} ms",
            st.files.len(),
            started.elapsed().as_secs_f64() * 1e3
        );
    }
    // A keyword on that no frame the others leave holds is named as
    // some sidecar in the folder spells it, not folded.
    let label_of = |value: &str| -> String {
        st.sidecars
            .iter()
            .flat_map(|s| s.meta.keywords.iter())
            .find(|k| k.to_lowercase() == value)
            .cloned()
            .unwrap_or_else(|| value.to_string())
    };
    counts
        .into_iter()
        .filter_map(|(facet, counts)| {
            let on = st.filter.chosen(facet);
            let chips = if facet == Facet::Keyword {
                chips_of(facet, &counts, on, &label_of)
            } else {
                chips_of(facet, &counts, on, &|v| v.to_string())
            };
            (!chips.is_empty()).then(|| FacetRow {
                name: filter::facet_caption(facet).into(),
                code: filter::facet_slot(facet) as i32,
                chips: ModelRc::new(VecModel::from(chips)),
            })
        })
        .collect()
}

/// The rows split for the grid's header, which has two: camera and
/// lens, whose chips are long, then the rest, which are short.
pub(crate) fn split_facet_rows(rows: &[FacetRow]) -> (Vec<FacetRow>, Vec<FacetRow>) {
    let long = |row: &FacetRow| {
        [Facet::Camera, Facet::Lens]
            .iter()
            .any(|f| filter::facet_slot(*f) as i32 == row.code)
    };
    (
        rows.iter().filter(|r| long(r)).cloned().collect(),
        rows.iter().filter(|r| !long(r)).cloned().collect(),
    )
}

/// What the facets' row says while it has no chips.
pub(crate) fn facet_note(st: &State) -> String {
    if let Some(e) = &st.index_error {
        return e.clone();
    }
    match (&st.index_reader, st.index_progress) {
        (_, Some((done, total))) => format!("Indexing the folder: {done} of {total}"),
        (Some(_), None) => "No camera, lens or date in these files' EXIF".to_string(),
        (None, None) if st.index.is_some() => "Opening the library index".to_string(),
        (None, None) => String::new(),
    }
}

/// The facets `--filter` named, chosen once the index has rows to
/// choose them from, among the values the open frames hold: with
/// `:` a text facet's value by what it contains, with `=` by what it
/// is, case aside; a number's by what it equals either way. True
/// when something was chosen.
pub(crate) fn choose_wanted(st: &mut State) -> bool {
    let wanted = std::mem::take(&mut st.facets_wanted);
    let ids: Vec<i64> = st.index_ids.iter().flatten().copied().collect();
    let mut chose = false;
    for want in wanted {
        let counts = if want.facet == Facet::Keyword {
            keyword_counts(st, &filter::Typed::default())
        } else {
            let Some(lib) = &st.index_reader else {
                continue;
            };
            match lib.facet_counts(want.facet, Some(&ids), &greycard_library::Filter::default()) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("index: {e}");
                    continue;
                }
            }
        };
        let needle = want.value.to_lowercase();
        let number = |s: &str| s.trim().trim_end_matches("mm").parse::<f64>().ok();
        let hits: Vec<String> = counts
            .into_iter()
            .filter(|c| match want.facet {
                Facet::Iso | Facet::Focal => {
                    number(&c.value).is_some() && number(&c.value) == number(&want.value)
                }
                _ if want.exact => c.value.to_lowercase() == needle,
                _ => c.value.to_lowercase().contains(&needle),
            })
            .map(|c| c.value)
            .collect();
        if hits.is_empty() {
            tracing::warn!(
                "--filter {}{}{}: no frame here holds it",
                want.facet.field(),
                if want.exact { "=" } else { ":" },
                want.value
            );
        }
        for value in hits {
            if !st.filter.chosen(want.facet).contains(&value) {
                st.filter.toggle_facet(want.facet, &value);
                chose = true;
            }
        }
    }
    chose
}

/// What the indexer said, on the UI thread.
pub(crate) fn told(app: &App, told: Told) {
    let Some(state) = crate::STATE.with(|s| s.borrow().clone()) else {
        return;
    };
    // A reader that would not open is tried again at every word.
    open_reader(&mut state.borrow_mut());
    match told {
        Told::Opened(path) => {
            tracing::info!("library index {}", path.display());
            let mut st = state.borrow_mut();
            st.index_path = Some(path);
            open_reader(&mut st);
            crate::roots::recount(&mut st);
            // The launch's folder, when it lies under a root, has its
            // root's tree once the index can be read.
            crate::tree::want(&mut st, app);
            crate::roots::show(&st, app);
            let wanted = st.library.wanted.take();
            drop(st);
            reread(&state, app, false);
            if let (Some(view), Some(worker)) = (wanted, crate::WORKER.with(|w| w.borrow().clone()))
            {
                crate::roots::open_view(&state, app, &worker, view);
            }
        }
        Told::Background {
            path,
            launch,
            report,
            seconds,
            error,
        } => {
            let what = if launch { "root" } else { "change" };
            match &error {
                Some(e) => tracing::debug!("index: {what} {}: {e}", path.display()),
                None => tracing::info!(
                    "indexed {what} {} in {seconds:.3} s: {} added, {} moved, {} changed, \
                     {} meta refreshed, {} unchanged ({} styles read), {} missing",
                    path.display(),
                    report.added,
                    report.moved,
                    report.changed,
                    report.meta_refreshed,
                    report.unchanged,
                    report.styled,
                    report.missing,
                ),
            }
            // What the window knew of the folders under the pass is
            // looked at again by the next read.
            crate::roots::folders_passed(&mut state.borrow_mut(), &path);
            // A merge reads the rows and the facets again with the
            // list; only without one are they read here.
            if !crate::roots::background_done(&state, app, &path, &report, launch, error.is_some())
            {
                reread(&state, app, false);
            }
            // A pass over an archive is its answer: its queued moves run.
            if error.is_none() {
                let mut st = state.borrow_mut();
                if let Some(archive) = st.library.roots.archive_of(&path).map(Path::to_path_buf) {
                    crate::panel::archive::rejects::heard_from(&mut st, app, &archive, true);
                }
            }
        }
        Told::BackgroundProgress { path } => {
            // Rows written since the last word: read as a pass that
            // added them.
            let some = Report {
                added: 1,
                ..Report::default()
            };
            if !crate::roots::background_done(&state, app, &path, &some, false, false) {
                reread(&state, app, false);
            }
        }
        Told::Skipped {
            path,
            root,
            launch,
            generation,
            first,
        } => {
            crate::panel::archive::rejects::heard_from(&mut state.borrow_mut(), app, &root, false);
            let words = skipped_words(&state.borrow().library.roots, &root);
            if first {
                app.set_status(words.clone().into());
            }
            match generation {
                // The window's own folder: its pass is over, said in the
                // filter's line, and the rows the window has are kept.
                Some(generation) => {
                    let mut st = state.borrow_mut();
                    if generation != st.index_generation {
                        return;
                    }
                    st.index_progress = None;
                    st.awaiting_index = false;
                    st.index_error = Some(words);
                    crate::panel::cull::show_filter(&st, app);
                }
                // A pass in the background: as one that found nothing,
                // so a launch pass skipped still counts as done and a
                // capture waiting on the roots is let go. Nothing in the
                // index moved, so its rows and facets are not read again
                // (a dead share is skipped at every tick).
                None => {
                    crate::roots::background_done(
                        &state,
                        app,
                        &path,
                        &Report::default(),
                        launch,
                        false,
                    );
                }
            }
        }
        Told::Failed(message) => {
            tracing::warn!("no library index: {message}");
            let mut st = state.borrow_mut();
            st.index = None;
            // The view of the roots asked for will not come, and a
            // folder opened meanwhile is recorded as any other.
            st.library.wanted = None;
            st.index_progress = None;
            st.index_error = Some(format!("No library index: {message}"));
            st.awaiting_index = false;
            crate::panel::cull::show_filter(&st, app);
            app.window().request_redraw();
        }
        Told::Progress {
            generation,
            done,
            total,
        } => {
            if generation != state.borrow().index_generation {
                return;
            }
            state.borrow_mut().index_progress = Some((done, total));
            reread(&state, app, false);
        }
        Told::Indexed {
            generation,
            report,
            seconds,
            error,
        } => {
            if generation != state.borrow().index_generation {
                return;
            }
            tracing::info!(
                "indexed the folder in {seconds:.2} s: {} added, {} moved, {} changed, \
                 {} meta refreshed, {} unchanged ({} styles read), {} missing{}",
                report.added,
                report.moved,
                report.changed,
                report.meta_refreshed,
                report.unchanged,
                report.styled,
                report.missing,
                if report.errors.is_empty() {
                    String::new()
                } else {
                    format!(", {} unreadable", report.errors.len())
                }
            );
            for (path, e) in &report.errors {
                tracing::debug!("index: {}: {e}", path.display());
            }
            let mut st = state.borrow_mut();
            if let Some(e) = error {
                st.index_tries += 1;
                if st.index_tries <= RETRIES {
                    // Busy, most likely: said, and asked again in a
                    // moment. A capture waiting on the chips waits on.
                    st.index_error = Some(format!("Library index busy; trying again ({e})"));
                    st.index_progress = None;
                    let app_weak = app.as_weak();
                    slint::Timer::single_shot(RETRY_AFTER, move || {
                        let Some(app) = app_weak.upgrade() else {
                            return;
                        };
                        let Some(state) = crate::STATE.with(|s| s.borrow().clone()) else {
                            return;
                        };
                        let mut st = state.borrow_mut();
                        if st.index_generation == generation {
                            index_open_folder(&mut st, None);
                            crate::panel::cull::show_filter(&st, &app);
                        }
                    });
                    crate::panel::cull::show_filter(&st, app);
                    return;
                }
                tracing::warn!("index: giving up on the folder after {RETRIES} tries: {e}");
                st.index_error = Some(format!("Library index unavailable: {e}"));
            } else {
                st.index_tries = 0;
                st.index_error = None;
            }
            st.index_progress = None;
            let moved = refresh_ids(&mut st);
            let chose = !st.facets_wanted.is_empty() && choose_wanted(&mut st);
            st.awaiting_index = false;
            drop(st);
            reread(&state, app, chose || moved);
            app.window().request_redraw();
        }
        Told::FilesIndexed => reread(&state, app, false),
        Told::Forgotten(n) => {
            tracing::debug!("index: {n} rows of deleted files forgotten");
            reread(&state, app, false);
        }
    }
}

/// What the window says of a root whose pass was skipped, or left
/// partway, because it did not answer.
pub(crate) fn skipped_words(roots: &greycard_library::Roots, root: &Path) -> String {
    let name = if roots.list().iter().any(|r| r == root) {
        roots.label(root)
    } else {
        root.display().to_string()
    };
    format!(
        "{name} is not answering: its frames are kept as the index has them, and it is passed over again later"
    )
}

/// The index moved under the window: the rows read again, and the
/// browser's list rebuilt when the filter asks the index something
/// and the answer changed; otherwise only the chips.
fn reread(state: &Rc<RefCell<State>>, app: &App, changed: bool) {
    let mut st = state.borrow_mut();
    let moved = refresh_ids(&mut st);
    let pass = index_pass(&st);
    if changed || moved || pass != st.index_passed {
        // The list is made from this answer, not asked again.
        st.index_passed = pass;
        st.index_pass_ready = true;
        drop(st);
        crate::panel::cull::refilter(state, app, changed);
    } else {
        crate::panel::cull::show_filter(&st, app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_library::fixture::{A7, R5, R6, write_frame};

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-ui-library-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // Canonical, as the indexer makes a root before it walks it:
        // on macOS the temp dir is a symlink, and a hang hook's prefix
        // check on the uncanonical path would never match.
        dunce::canonicalize(&dir).unwrap()
    }

    /// A folder of five frames on a library of the test's own: two
    /// R6s, an R5, an A7, and a JPEG-like frame the index has not
    /// reached. The index is written through a connection of its
    /// own, as the indexer's thread would, and read through a
    /// read-only one, as the window does.
    fn folder(dir: &Path) -> (Vec<PathBuf>, Vec<Sidecar>, Library) {
        let files: Vec<PathBuf> = ["a.tif", "b.tif", "c.tif", "d.tif", "e.tif"]
            .iter()
            .map(|n| dir.join(n))
            .collect();
        write_frame(&files[0], &R6, 1);
        write_frame(&files[1], &R6, 2);
        write_frame(&files[2], &R5, 3);
        write_frame(&files[3], &A7, 4);
        let mut sidecars = vec![Sidecar::default(); files.len()];
        sidecars[0].meta.rating = 3;
        sidecars[0].meta.set_keywords(vec!["Harbor".into()]);
        sidecars[2].meta.rating = 4;
        sidecars[2]
            .meta
            .set_keywords(vec!["harbor".into(), "dusk".into()]);
        sidecars[3].meta.flag = meta::Flag::Pick;
        for (f, s) in files.iter().zip(&mut sidecars).take(4) {
            s.save(f).unwrap();
        }
        let db = dir.join("index").join("library.sqlite");
        let mut writer = Library::open(&db).unwrap();
        writer.index_folder(dir, &mut |_| {}).unwrap();
        // The fifth arrives after the pass: no row yet.
        write_frame(&files[4], &R6, 5);
        drop(writer);
        let reader = Library::open_read_only(&db).unwrap();
        (files, sidecars, reader)
    }

    /// A state with the folder's files, sidecars and index, as the
    /// window would have them after the first pass.
    fn state(app: &App, dir: &Path) -> State {
        let (files, sidecars, reader) = folder(dir);
        let mut st = State::empty(files, app);
        st.sidecars = sidecars;
        st.index_reader = Some(reader);
        refresh_ids(&mut st);
        st
    }

    fn shown(st: &State) -> Vec<usize> {
        let pass = index_pass(st);
        let frames: Vec<filter::Frame> = st
            .files
            .iter()
            .zip(&st.sidecars)
            .zip(&pass)
            .map(|((path, s), &index)| filter::Frame {
                path,
                meta: &s.meta,
                index,
            })
            .collect();
        st.filter.apply(&frames)
    }

    /// A facet's counts, the index's answer brought up to the filter
    /// first, as `rebuild_browser` brings it before the chips are
    /// shown.
    fn counts(st: &mut State, facet: Facet) -> Vec<(String, usize)> {
        st.index_passed = index_pass(st);
        facet_counts(st)
            .into_iter()
            .find(|(f, _)| *f == facet)
            .map(|(_, c)| c.into_iter().map(|c| (c.value, c.count)).collect())
            .unwrap_or_default()
    }

    fn pairs(want: &[(&str, usize)]) -> Vec<(String, usize)> {
        want.iter().map(|(v, n)| (v.to_string(), *n)).collect()
    }

    /// A camera chip is answered from the index, and a frame the
    /// index has not reached yet stays until it has.
    #[test]
    fn a_facet_chip_filters_by_the_index_and_a_frame_without_a_row_stays() {
        let dir = scratch("chip");
        let app = crate::testing::window(5);
        let mut st = state(&app, &dir);
        assert_eq!(st.index_ids.iter().filter(|i| i.is_some()).count(), 4);
        assert_eq!(st.index_ids[4], None, "the fifth has no row");
        assert_eq!(shown(&st), [0, 1, 2, 3, 4]);

        st.filter.toggle_facet(Facet::Camera, "Canon EOS R6m2");
        // The two R6s, and the fifth, which the index cannot yet say
        // is an R6 or not.
        assert_eq!(shown(&st), [0, 1, 4]);
        // Two chips of a facet are either.
        st.filter.toggle_facet(Facet::Camera, "Canon EOS R5");
        assert_eq!(shown(&st), [0, 1, 2, 4]);
        // A second facet narrows.
        st.filter.toggle_facet(Facet::Iso, "100");
        assert_eq!(shown(&st), [2, 4]);

        // Once the fifth has a row the index's word stands.
        let db = st.index_reader.as_ref().unwrap().path().to_path_buf();
        Library::open(&db)
            .unwrap()
            .index_file(&st.files[4])
            .unwrap();
        refresh_ids(&mut st);
        assert!(st.index_ids[4].is_some());
        assert_eq!(shown(&st), [2]);
        st.filter.toggle_facet(Facet::Iso, "100");
        assert_eq!(shown(&st), [0, 1, 2, 4]);
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A facet's count is how many frames hold each value among the
    /// frames the other groups leave, its own set aside: the meta
    /// rows' rule, and what one chip pressed alone would list.
    #[test]
    fn a_facet_counts_what_the_other_tests_leave_and_its_own_is_set_aside() {
        let dir = scratch("counts");
        let app = crate::testing::window(5);
        let mut st = state(&app, &dir);
        let sony = st
            .index_reader
            .as_ref()
            .unwrap()
            .by_path(&st.files[3])
            .unwrap()
            .unwrap()
            .exif
            .camera;
        assert_eq!(
            counts(&mut st, Facet::Camera),
            pairs(&[("Canon EOS R6m2", 2), ("Canon EOS R5", 1), (&sony, 1)])
        );
        assert_eq!(
            counts(&mut st, Facet::Iso),
            pairs(&[("100", 1), ("3200", 1), ("6400", 2)])
        );
        assert_eq!(
            counts(&mut st, Facet::Keyword),
            pairs(&[("harbor", 2), ("dusk", 1)])
        );

        // Three stars or more: the meta test narrows every facet.
        st.filter.stars = filter::Stars::AtLeast(3);
        assert_eq!(
            counts(&mut st, Facet::Camera),
            pairs(&[("Canon EOS R5", 1), ("Canon EOS R6m2", 1)])
        );
        st.filter.stars = filter::Stars::default();

        // An R6 chip on: the camera row is unmoved, since its own
        // group is set aside; the others narrow to the R6s.
        st.filter.toggle_facet(Facet::Camera, "Canon EOS R6m2");
        assert_eq!(
            counts(&mut st, Facet::Camera),
            pairs(&[("Canon EOS R6m2", 2), ("Canon EOS R5", 1), (&sony, 1)])
        );
        assert_eq!(counts(&mut st, Facet::Iso), pairs(&[("6400", 2)]));
        assert_eq!(counts(&mut st, Facet::Keyword), pairs(&[("harbor", 1)]));

        // And the rule itself, chip by chip: a count is what that
        // chip alone, the other groups as they stand, would list of
        // the frames the index has rows for.
        st.filter.stars = filter::Stars::AtLeast(1);
        st.index_passed = index_pass(&st);
        for facet in Facet::ALL {
            let rows = facet_counts(&st);
            let (_, chips) = rows.iter().find(|(f, _)| *f == facet).unwrap();
            for chip in chips {
                let mut alone = st.filter.clone();
                alone.facets[filter::facet_slot(facet)] = vec![chip.value.clone()];
                let saved = std::mem::replace(&mut st.filter, alone);
                let listed = shown(&st)
                    .into_iter()
                    .filter(|&i| st.index_ids[i].is_some())
                    .count();
                st.filter = saved;
                assert_eq!(listed, chip.count, "{facet:?} {}", chip.value);
            }
        }

        // The keyword is counted from the sidecars in hand: a keyword
        // taken off in memory is off the count before any row knows,
        // which is also what `--no-sidecars` needs.
        st.filter = filter::Filter::default();
        st.sidecars[2].meta.set_keywords(Vec::new());
        assert_eq!(counts(&mut st, Facet::Keyword), pairs(&[("harbor", 1)]));
        // And it needs no index at all.
        st.index_reader = None;
        refresh_ids(&mut st);
        assert_eq!(counts(&mut st, Facet::Keyword), pairs(&[("harbor", 1)]));
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The text field takes the §160 grammar: an EXIF term goes to
    /// the index, a meta term is answered from the sidecar, and a
    /// word is the word it always was.
    #[test]
    fn a_typed_term_is_answered_where_it_can_be() {
        let dir = scratch("typed");
        let app = crate::testing::window(5);
        let mut st = state(&app, &dir);
        st.filter.text = "camera:R6".into();
        assert_eq!(shown(&st), [0, 1, 4]);
        st.filter.text = "camera:R6 rating>=3".into();
        assert_eq!(shown(&st), [0]);
        st.filter.text = "iso>=3200 harbor".into();
        assert_eq!(shown(&st), [0]);
        // The sidecar's word, not the row's: a rating set in memory
        // counts before any index_file has run.
        st.sidecars[1].meta.rating = 5;
        st.filter.text = "rating=5".into();
        assert_eq!(shown(&st), [1]);
        // A term that does not parse is the word it was, and says so.
        st.filter.text = "rating>=9".into();
        assert_eq!(st.filter.typed().errors.len(), 1);
        assert_eq!(shown(&st), Vec::<usize>::new());
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The window's side: a facet chip pressed on the bar narrows the
    /// browser, and the bar shows the chip on with its count.
    #[test]
    fn a_chip_pressed_on_the_bar_narrows_the_browser() {
        use crate::testing::{state_for, window};
        let dir = scratch("bar");
        let app = window(0);
        let (files, sidecars, reader) = folder(&dir);
        let (state, _worker) = state_for(&app, files);
        {
            let mut st = state.borrow_mut();
            st.sidecars = sidecars;
            st.index_reader = Some(reader);
            refresh_ids(&mut st);
            crate::panel::browser::rebuild_browser(&mut st, &app);
        }
        assert_eq!(app.get_filter_shown(), 5);
        let camera = app.get_filter_facets().row_data(0).expect("a camera row");
        assert_eq!(camera.name, "Camera");
        assert_eq!(camera.chips.row_count(), 3);
        app.invoke_filter_facet_toggled(
            filter::facet_slot(Facet::Camera) as i32,
            "Canon EOS R6m2".into(),
        );
        assert_eq!(state.borrow().shown, [0, 1, 4]);
        assert_eq!(app.get_filter_shown(), 3);
        assert!(app.get_filter_on());
        let camera = app.get_filter_facets().row_data(0).unwrap();
        let chip = camera.chips.row_data(0).unwrap();
        assert_eq!(
            (chip.key.as_str(), chip.count, chip.on),
            ("Canon EOS R6m2", 2, true)
        );
        // Clear lets it go with the rest.
        app.invoke_filter_cleared();
        assert_eq!(state.borrow().shown, [0, 1, 2, 3, 4]);
        // The callbacks hold the state too; its connection goes here.
        state.borrow_mut().index_reader = None;
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A panic on the indexer's thread is said to the window as a
    /// failure, not left as a pass that never ends.
    #[test]
    fn a_panic_on_the_indexer_thread_is_a_failure_the_window_hears() {
        let dir = scratch("panic");
        let db = dir.join("library.sqlite");
        let (tx, rx) = std::sync::mpsc::channel();
        let indexer = Indexer::start(db, move |told| {
            if matches!(told, Told::Opened(_)) {
                panic!("a bug");
            }
            let _ = tx.send(told);
        })
        .expect("the indexer starts");
        match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(Told::Failed(message)) => assert!(message.contains("a bug"), "{message}"),
            other => panic!("{other:?}"),
        }
        indexer.stop(Duration::from_secs(20));
        crate::testing::remove_dir_retry(&dir);
    }

    /// A library another process holds locked past the writer's
    /// timeout: the pass says so in `Indexed`, so the window can say
    /// busy and ask again, rather than report a pass that did
    /// nothing as a folder with no EXIF.
    #[test]
    fn a_locked_library_is_an_error_in_the_pass_not_an_empty_folder() {
        let dir = scratch("locked");
        let shoot = dir.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        write_frame(&shoot.join("a.tif"), &R6, 1);
        let db = dir.join("library.sqlite");
        drop(Library::open(&db).unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let indexer = Indexer::start(db.clone(), move |told| {
            let _ = tx.send(told);
        })
        .expect("the indexer starts");
        let wait = || rx.recv_timeout(Duration::from_secs(30)).expect("an answer");
        assert!(matches!(wait(), Told::Opened(_)));
        let holder = rusqlite::Connection::open(&db).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        indexer.folders(vec![shoot.clone()], 1);
        loop {
            match wait() {
                Told::Indexed { error, .. } => {
                    assert!(error.is_some(), "the lock was said");
                    break;
                }
                Told::Progress { .. } => {}
                other => panic!("{other:?}"),
            }
        }
        holder.execute_batch("ROLLBACK").unwrap();
        drop(holder);
        indexer.folders(vec![shoot], 2);
        match wait() {
            Told::Indexed { error, report, .. } => {
                assert_eq!(error, None);
                assert_eq!(report.added, 1);
            }
            other => panic!("{other:?}"),
        }
        indexer.stop(Duration::from_secs(20));
        crate::testing::remove_dir_retry(&dir);
    }

    /// The grid's header has two facet rows: camera and lens on the
    /// first, whatever else the index counts on the second.
    #[test]
    fn the_headers_facets_split_into_camera_and_lens_then_the_rest() {
        use crate::testing::{state_for, window};
        let dir = scratch("rows");
        let app = window(0);
        let (files, sidecars, reader) = folder(&dir);
        let (state, _worker) = state_for(&app, files);
        {
            let mut st = state.borrow_mut();
            st.sidecars = sidecars;
            st.index_reader = Some(reader);
            refresh_ids(&mut st);
            crate::panel::browser::rebuild_browser(&mut st, &app);
        }
        let names = |rows: slint::ModelRc<FacetRow>| -> Vec<String> {
            rows.iter().map(|r| r.name.to_string()).collect()
        };
        let long = names(app.get_filter_facets_long());
        let short = names(app.get_filter_facets_short());
        assert_eq!(long, ["Camera", "Lens"]);
        assert!(!short.is_empty(), "{short:?}");
        assert!(
            short.iter().all(|n| n != "Camera" && n != "Lens"),
            "{short:?}"
        );
        // Together they are the rows the stacked bar shows.
        assert_eq!(
            long.len() + short.len(),
            app.get_filter_facets().row_count()
        );
        state.borrow_mut().index_reader = None;
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A facet row longer than the header scrolls sideways under a
    /// plain wheel, which is the only wheel most mice have.
    #[test]
    fn a_plain_wheel_scrolls_a_long_facet_row_sideways() {
        use slint::platform::WindowEvent;
        let app = crate::testing::window(0);
        app.window().set_size(slint::LogicalSize::new(800.0, 600.0));
        app.set_grid_open(true);
        let chips: Vec<FacetChip> = (0..40)
            .map(|i| FacetChip {
                text: format!("{}", 100 * (i + 1)).into(),
                key: format!("k{i}").into(),
                count: 1,
                on: false,
            })
            .collect();
        app.set_filter_facets_short(ModelRc::new(VecModel::from(vec![FacetRow {
            name: "ISO".into(),
            code: 2,
            chips: ModelRc::new(VecModel::from(chips)),
        }])));
        let pressed = Rc::new(RefCell::new(Vec::new()));
        let seen = pressed.clone();
        app.on_filter_facet_toggled(move |_, key| seen.borrow_mut().push(key.to_string()));
        // With no camera or lens row, the short facets take the
        // first row's place: the header's third, under the selection's
        // line and the meta chips with the words; the sheet starts at
        // the left pane's edge, 240 in.
        let (x, y) = (440.0, 92.0);
        crate::testing::click(&app, x, y);
        let position = slint::LogicalPosition::new(x, y);
        app.window().dispatch_event(WindowEvent::PointerScrolled {
            position,
            delta_x: 0.0,
            delta_y: -400.0,
        });
        // A pointer that has not moved since the wheel still points
        // at the chip that was under it, as far as Slint's hover is
        // concerned; a hand moves, and so does this.
        crate::testing::click(&app, x + 5.0, y);
        let pressed = pressed.borrow();
        assert_eq!(pressed.len(), 2, "{pressed:?}");
        let at = |k: &str| k[1..].parse::<i32>().unwrap();
        assert!(
            at(&pressed[1]) > at(&pressed[0]),
            "down the wheel is along the row"
        );
    }

    /// The indexer's thread: it makes the library, indexes the folders
    /// asked for, and brings a row up to date after a save, saying
    /// each as it goes — on a database of the test's own.
    #[test]
    fn the_indexer_indexes_a_folder_and_a_saved_sidecar() {
        let dir = scratch("indexer");
        let shoot = dir.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        let (a, b) = (shoot.join("a.tif"), shoot.join("b.tif"));
        write_frame(&a, &R6, 1);
        write_frame(&b, &R5, 2);
        let db = dir.join("data").join("library.sqlite");
        let (tx, rx) = std::sync::mpsc::channel();
        let indexer = Indexer::start(db.clone(), move |told| {
            let _ = tx.send(told);
        })
        .expect("the indexer starts");
        let wait = |want: &dyn Fn(&Told) -> bool| loop {
            let told = rx
                .recv_timeout(Duration::from_secs(20))
                .expect("the indexer answers");
            if want(&told) {
                return told;
            }
        };
        assert!(matches!(wait(&|t| matches!(t, Told::Opened(_))), Told::Opened(p) if p == db));
        indexer.folders(folders_of(&[a.clone(), b.clone()]), 7);
        match wait(&|t| matches!(t, Told::Indexed { .. })) {
            Told::Indexed {
                generation, report, ..
            } => {
                assert_eq!(generation, 7);
                assert_eq!(report.added, 2, "{report:?}");
            }
            other => panic!("{other:?}"),
        }
        let reader = Library::open_read_only(&db).unwrap();
        assert_eq!(reader.by_path(&a).unwrap().unwrap().meta.rating, 0);

        // A rating saved, and the row follows once the indexer says so.
        let mut s = Sidecar::default();
        s.meta.rating = 4;
        s.save(&a).unwrap();
        indexer.file(a.clone());
        wait(&|t| matches!(t, Told::FilesIndexed));
        assert_eq!(reader.by_path(&a).unwrap().unwrap().meta.rating, 4);
        indexer.stop(Duration::from_secs(20));
        drop(reader);
        crate::testing::remove_dir_retry(&dir);
    }

    /// Two roots of two frames each under `dir`, `a` and `b`, and an
    /// indexer on a library there with `guard`; what it tells, on a
    /// channel.
    fn two_roots(dir: &Path, guard: Guard) -> (PathBuf, PathBuf, Indexer, mpsc::Receiver<Told>) {
        let (a, b) = (dir.join("a"), dir.join("b"));
        for (root, seed) in [(&a, 1), (&b, 3)] {
            std::fs::create_dir_all(root.join("day")).unwrap();
            write_frame(&root.join("day").join("x.tif"), &R6, seed);
            write_frame(&root.join("day").join("y.tif"), &R5, seed + 1);
        }
        let (tx, rx) = mpsc::channel();
        let indexer = Indexer::start_with(
            dir.join("data").join("library.sqlite"),
            move |told| {
                let _ = tx.send(told);
            },
            guard,
        )
        .expect("the indexer starts");
        (a, b, indexer, rx)
    }

    fn wait_for(rx: &mpsc::Receiver<Told>, want: &dyn Fn(&Told) -> bool) -> Told {
        loop {
            let told = rx
                .recv_timeout(Duration::from_secs(20))
                .expect("the indexer answers");
            if want(&told) {
                return told;
            }
        }
    }

    /// A root that does not answer the look before its pass is skipped,
    /// said once, its rows left as they are, and the other root's pass
    /// and report come all the same.
    #[test]
    fn a_pass_over_a_root_that_does_not_answer_is_skipped_and_the_next_lands() {
        let dir = scratch("skipped");
        let asked = Arc::new(AtomicUsize::new(0));
        let counted = asked.clone();
        let guard = Guard {
            answers: Box::new(move |root| {
                counted.fetch_add(1, Ordering::SeqCst);
                !root.ends_with("a")
            }),
            ..Guard::default()
        };
        let (a, b, indexer, rx) = two_roots(&dir, guard);
        wait_for(&rx, &|t| matches!(t, Told::Opened(_)));
        indexer.roots(vec![a.clone(), b.clone()]);
        match wait_for(&rx, &|t| {
            matches!(t, Told::Skipped { .. } | Told::Background { .. })
        }) {
            Told::Skipped {
                path,
                root,
                launch,
                generation,
                first,
            } => {
                assert_eq!((path, root), (a.clone(), a.clone()));
                assert!(launch && first);
                assert_eq!(generation, None);
            }
            other => panic!("the root that did not answer comes first: {other:?}"),
        }
        match wait_for(&rx, &|t| matches!(t, Told::Background { .. })) {
            Told::Background {
                path,
                report,
                error,
                ..
            } => {
                assert_eq!(path, b);
                assert_eq!(error, None);
                assert_eq!(report.added, 2, "{report:?}");
            }
            other => panic!("{other:?}"),
        }
        // Asked again: skipped again, and not said again.
        indexer.asker().poll(vec![a.clone()]);
        match wait_for(&rx, &|t| matches!(t, Told::Skipped { .. })) {
            Told::Skipped { first, launch, .. } => assert!(!first && !launch),
            other => panic!("{other:?}"),
        }
        assert_eq!(asked.load(Ordering::SeqCst), 3, "one look a pass begun");
        let reader = Library::open_read_only(&dir.join("data").join("library.sqlite")).unwrap();
        assert_eq!(
            reader.count_under(&a).unwrap(),
            0,
            "nothing indexed under a"
        );
        assert_eq!(reader.count_under(&b).unwrap(), 2);
        drop(reader);
        indexer.stop(Duration::from_secs(20));
        crate::testing::remove_dir_retry(&dir);
    }

    /// A root that answers the look and then hangs in the middle of its
    /// walk: the pass is set aside once it has said nothing for the
    /// wait, said, and the other root's pass and report come while it
    /// is still stuck. The stuck root is skipped without a look while
    /// its pass is out; let go, that pass finishes and its report lands
    /// as any other.
    #[test]
    fn a_root_that_hangs_mid_walk_never_stops_the_other_roots_reports() {
        let dir = scratch("hangs");
        let released = Arc::new(AtomicBool::new(false));
        let let_go = released.clone();
        let hang_under = dir.join("a");
        let asked = Arc::new(AtomicUsize::new(0));
        let counted = asked.clone();
        let guard = Guard {
            answers: Box::new(move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
                true
            }),
            wait: Duration::from_millis(300),
            each_file: Some(Arc::new(move |file: &Path| {
                while file.starts_with(&hang_under) && !let_go.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            })),
        };
        let (a, b, indexer, rx) = two_roots(&dir, guard);
        wait_for(&rx, &|t| matches!(t, Told::Opened(_)));
        let begun = Instant::now();
        indexer.roots(vec![a.clone(), b.clone()]);
        match wait_for(&rx, &|t| {
            matches!(t, Told::Skipped { .. } | Told::Background { .. })
        }) {
            Told::Skipped {
                root,
                launch,
                first,
                ..
            } => {
                assert_eq!(root, a);
                assert!(launch && first);
            }
            other => panic!("{other:?}"),
        }
        match wait_for(&rx, &|t| matches!(t, Told::Background { .. })) {
            Told::Background { path, report, .. } => {
                assert_eq!(path, b);
                assert_eq!(report.added, 2, "{report:?}");
            }
            other => panic!("{other:?}"),
        }
        assert!(
            !released.load(Ordering::SeqCst),
            "b's report came while a's walk was still stuck"
        );
        assert!(begun.elapsed() < Duration::from_secs(10));
        // While its thread is out: skipped at once, with no look.
        let looks = asked.load(Ordering::SeqCst);
        indexer.asker().poll(vec![a.clone()]);
        match wait_for(&rx, &|t| matches!(t, Told::Skipped { .. })) {
            Told::Skipped { first, .. } => assert!(!first),
            other => panic!("{other:?}"),
        }
        assert_eq!(asked.load(Ordering::SeqCst), looks);
        // Let go: the pass set aside finishes, and its report lands.
        released.store(true, Ordering::SeqCst);
        match wait_for(&rx, &|t| matches!(t, Told::Background { .. })) {
            Told::Background {
                path,
                launch,
                report,
                error,
                ..
            } => {
                assert_eq!(path, a);
                assert!(!launch, "counted when it was set aside");
                assert_eq!(error, None);
                assert_eq!(report.added, 2, "{report:?}");
            }
            other => panic!("{other:?}"),
        }
        // And the root is passed over again at the next tick, looked at.
        indexer.asker().poll(vec![a.clone()]);
        match wait_for(&rx, &|t| {
            matches!(t, Told::Skipped { .. } | Told::Background { .. })
        }) {
            Told::Background { path, report, .. } => {
                assert_eq!(path, a);
                assert_eq!(report.unchanged, 2, "{report:?}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(asked.load(Ordering::SeqCst), looks + 1);
        let reader = Library::open_read_only(&dir.join("data").join("library.sqlite")).unwrap();
        assert_eq!(reader.count_under(&a).unwrap(), 2);
        drop(reader);
        indexer.stop(Duration::from_secs(20));
        crate::testing::remove_dir_retry(&dir);
    }

    /// The window hears a skipped pass: said on the status line once, a
    /// launch pass skipped counts as done for a capture waiting on the
    /// roots, and its own folder's pass skipped is said in the filter's
    /// line, its wait over.
    #[test]
    fn the_window_says_a_skipped_root_once_and_counts_its_launch_pass() {
        use crate::testing::{state_for, window};
        let dir = scratch("skipped-said");
        let root = dir.join("archive");
        std::fs::create_dir_all(&root).unwrap();
        let app = window(0);
        let (state, _worker) = state_for(&app, Vec::new());
        let root = {
            let mut st = state.borrow_mut();
            st.library.roots.add(&root).unwrap();
            let root = st.library.roots.list()[0].clone();
            st.library.roots.set_name(&root, "Archive");
            st.library.launch_left = 1;
            st.library.awaiting = true;
            root
        };
        let skipped = |first: bool, launch: bool, generation: Option<u64>| Told::Skipped {
            path: root.clone(),
            root: root.clone(),
            launch,
            generation,
            first,
        };
        told(&app, skipped(true, true, None));
        assert!(
            app.get_status().starts_with("Archive is not answering"),
            "{}",
            app.get_status()
        );
        assert_eq!(state.borrow().library.launch_left, 0);
        assert!(!state.borrow().library.awaiting);
        app.set_status("something else".into());
        told(&app, skipped(false, false, None));
        assert_eq!(app.get_status(), "something else", "said once");
        {
            let mut st = state.borrow_mut();
            st.index_generation = 9;
            st.awaiting_index = true;
            st.index_progress = Some((0, 3));
        }
        told(&app, skipped(false, false, Some(9)));
        let st = state.borrow();
        assert!(!st.awaiting_index);
        assert_eq!(st.index_progress, None);
        assert!(
            st.index_error
                .as_deref()
                .is_some_and(|e| e.starts_with("Archive is not answering"))
        );
        drop(st);
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A share mounted below a local root that hangs holds its own pass
    /// and not the local root's: the local root's walk leaves it out, its
    /// changes are passed over, and only the share's tick is set aside.
    #[test]
    fn a_hung_share_below_a_local_root_holds_only_its_own_pass() {
        let dir = scratch("below-hangs");
        let hang_under = dir.join("a").join("nas");
        let held = hang_under.clone();
        let released = Arc::new(AtomicBool::new(false));
        let let_go = released.clone();
        let guard = Guard {
            answers: Box::new(|_| true),
            wait: Duration::from_millis(300),
            each_file: Some(Arc::new(move |file: &Path| {
                while file.starts_with(&held) && !let_go.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            })),
        };
        let (a, b, indexer, rx) = two_roots(&dir, guard);
        std::fs::create_dir_all(&hang_under).unwrap();
        write_frame(&hang_under.join("n.tif"), &A7, 7);
        let nas = hang_under;
        wait_for(&rx, &|t| matches!(t, Told::Opened(_)));
        // The mounts left out come in the launch's own ask.
        indexer.launch(vec![a.clone(), b.clone()], vec![nas.clone()]);
        for root in [&a, &b] {
            match wait_for(&rx, &|t| {
                matches!(t, Told::Skipped { .. } | Told::Background { .. })
            }) {
                Told::Background { path, report, .. } => {
                    assert_eq!(&path, root);
                    assert_eq!(report.added, 2, "the share left out: {report:?}");
                }
                other => panic!("nothing set aside: {other:?}"),
            }
        }
        // The share's tick hangs, and is set aside alone.
        indexer.asker().poll(vec![nas.clone()]);
        match wait_for(&rx, &|t| {
            matches!(t, Told::Skipped { .. } | Told::Background { .. })
        }) {
            Told::Skipped { root, .. } => assert_eq!(root, nas),
            other => panic!("{other:?}"),
        }
        // A change in the local root, and the root's own tree, still pass.
        write_frame(&a.join("day").join("w.tif"), &R6, 8);
        indexer
            .asker()
            .changes(vec![Change::Folder(a.join("day")), Change::Tree(a.clone())]);
        for want in [a.join("day"), a.clone()] {
            match wait_for(&rx, &|t| {
                matches!(t, Told::Skipped { .. } | Told::Background { .. })
            }) {
                Told::Background { path, error, .. } => {
                    assert_eq!(path, want);
                    assert_eq!(error, None);
                }
                other => panic!("{other:?}"),
            }
        }
        released.store(true, Ordering::SeqCst);
        match wait_for(&rx, &|t| matches!(t, Told::Background { .. })) {
            Told::Background { path, report, .. } => {
                assert_eq!(path, nas);
                assert_eq!(report.added, 1, "{report:?}");
            }
            other => panic!("{other:?}"),
        }
        indexer.stop(Duration::from_secs(20));
        crate::testing::remove_dir_retry(&dir);
    }

    /// A pass set aside on a tree below a root (a share's tick, the share
    /// not known to be left out of the root's walk) keeps a pass over the
    /// root from walking into it: that pass is skipped until the share's
    /// is back, and then made.
    #[test]
    fn a_pass_over_a_tree_with_one_set_aside_in_it_waits() {
        let dir = scratch("tree-aside");
        let nas = dir.join("a").join("nas");
        let held = nas.clone();
        let released = Arc::new(AtomicBool::new(false));
        let let_go = released.clone();
        let guard = Guard {
            answers: Box::new(|_| true),
            wait: Duration::from_millis(300),
            each_file: Some(Arc::new(move |file: &Path| {
                while file.starts_with(&held) && !let_go.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            })),
        };
        let (a, _b, indexer, rx) = two_roots(&dir, guard);
        std::fs::create_dir_all(&nas).unwrap();
        write_frame(&nas.join("n.tif"), &A7, 7);
        wait_for(&rx, &|t| matches!(t, Told::Opened(_)));
        indexer.asker().poll(vec![nas.clone()]);
        match wait_for(&rx, &|t| {
            matches!(t, Told::Skipped { .. } | Told::Background { .. })
        }) {
            Told::Skipped { root, .. } => assert_eq!(root, nas),
            other => panic!("{other:?}"),
        }
        indexer.roots(vec![a.clone()]);
        match wait_for(&rx, &|t| {
            matches!(t, Told::Skipped { .. } | Told::Background { .. })
        }) {
            Told::Skipped { path, .. } => assert_eq!(path, a, "not walked into the share"),
            other => panic!("{other:?}"),
        }
        released.store(true, Ordering::SeqCst);
        match wait_for(&rx, &|t| matches!(t, Told::Background { .. })) {
            Told::Background { path, .. } => assert_eq!(path, nas),
            other => panic!("{other:?}"),
        }
        indexer.roots(vec![a.clone()]);
        match wait_for(&rx, &|t| {
            matches!(t, Told::Skipped { .. } | Told::Background { .. })
        }) {
            Told::Background { path, report, .. } => {
                assert_eq!(path, a);
                assert_eq!(report.added, 2, "{report:?}");
            }
            other => panic!("{other:?}"),
        }
        indexer.stop(Duration::from_secs(20));
        crate::testing::remove_dir_retry(&dir);
    }

    /// A launch pass set aside is counted once: the word that it was set
    /// aside says launch, and when it lands it does not, so three roots,
    /// the first set aside, are three launch passes done and no more.
    #[test]
    fn a_launch_pass_set_aside_is_counted_once() {
        let dir = scratch("launch-once");
        let released = Arc::new(AtomicBool::new(false));
        let let_go = released.clone();
        let hang_under = dir.join("a");
        let guard = Guard {
            answers: Box::new(|_| true),
            wait: Duration::from_millis(300),
            each_file: Some(Arc::new(move |file: &Path| {
                while file.starts_with(&hang_under) && !let_go.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            })),
        };
        let (a, b, indexer, rx) = two_roots(&dir, guard);
        let c = dir.join("c");
        std::fs::create_dir_all(&c).unwrap();
        write_frame(&c.join("z.tif"), &R6, 9);
        wait_for(&rx, &|t| matches!(t, Told::Opened(_)));
        indexer.roots(vec![a.clone(), b.clone(), c.clone()]);
        let mut launches = Vec::new();
        let mut a_landed = None;
        while a_landed.is_none() {
            match wait_for(&rx, &|t| {
                matches!(t, Told::Skipped { .. } | Told::Background { .. })
            }) {
                Told::Skipped { path, launch, .. } => {
                    if launch {
                        launches.push(path.clone());
                    }
                }
                Told::Background { path, launch, .. } => {
                    if launch {
                        launches.push(path.clone());
                    }
                    if path == a {
                        a_landed = Some(launch);
                    } else if path == c {
                        // Every other root done: let a go.
                        released.store(true, Ordering::SeqCst);
                    }
                }
                _ => {}
            }
        }
        assert_eq!(a_landed, Some(false), "a's pass lands as no launch pass");
        assert_eq!(launches, vec![a.clone(), b, c], "each root counted once");
        indexer.stop(Duration::from_secs(20));
        crate::testing::remove_dir_retry(&dir);
    }

    /// The window's own folder pass, stuck the same way, is said to the
    /// window with its generation, and the indexer goes on with the
    /// passes in the background.
    #[test]
    fn a_folder_pass_that_hangs_is_said_and_the_indexer_goes_on() {
        let dir = scratch("folder-hangs");
        let released = Arc::new(AtomicBool::new(false));
        let let_go = released.clone();
        let hang_under = dir.join("a");
        let guard = Guard {
            answers: Box::new(|_| true),
            wait: Duration::from_millis(300),
            each_file: Some(Arc::new(move |file: &Path| {
                while file.starts_with(&hang_under) && !let_go.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            })),
        };
        let (a, b, indexer, rx) = two_roots(&dir, guard);
        wait_for(&rx, &|t| matches!(t, Told::Opened(_)));
        indexer.folders(vec![a.join("day")], 3);
        match wait_for(&rx, &|t| {
            matches!(t, Told::Skipped { .. } | Told::Indexed { .. })
        }) {
            Told::Skipped {
                path,
                generation,
                first,
                launch,
                ..
            } => {
                assert_eq!(path, a.join("day"));
                assert_eq!(generation, Some(3));
                assert!(first && !launch);
            }
            other => panic!("{other:?}"),
        }
        indexer.roots(vec![b.clone()]);
        match wait_for(&rx, &|t| matches!(t, Told::Background { .. })) {
            Told::Background { path, report, .. } => {
                assert_eq!(path, b);
                assert_eq!(report.added, 2, "{report:?}");
            }
            other => panic!("{other:?}"),
        }
        assert!(!released.load(Ordering::SeqCst));
        // Let go: the folder's pass set aside finishes, and says so with
        // its own generation.
        released.store(true, Ordering::SeqCst);
        match wait_for(&rx, &|t| matches!(t, Told::Indexed { .. })) {
            Told::Indexed {
                generation,
                report,
                error,
                ..
            } => {
                assert_eq!(generation, 3);
                assert_eq!(error, None);
                assert_eq!(report.added, 2, "{report:?}");
            }
            other => panic!("{other:?}"),
        }
        indexer.stop(Duration::from_secs(20));
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_facet_row_keeps_its_chips_on_and_caps_the_rest() {
        let counts: Vec<FacetCount> = (0..20)
            .map(|i| FacetCount {
                value: (100 * (i + 1)).to_string(),
                label: (100 * (i + 1)).to_string(),
                count: if i == 3 { 1 } else { 20 - i },
            })
            .collect();
        let on = vec!["400".to_string(), "99999".to_string()];
        let chips = chips_of(Facet::Iso, &counts, &on, &|v| v.to_string());
        // Twelve by count, the one on kept past the cut, and a value
        // on that no frame holds any more, so it can be let go.
        assert_eq!(chips.len(), CHIPS_A_FACET + 2);
        assert!(chips.iter().any(|c| c.key == "400" && c.on));
        assert!(
            chips
                .iter()
                .any(|c| c.key == "99999" && c.on && c.count == 0)
        );
        // In the facet's own order, not the count's.
        let keys: Vec<i32> = chips
            .iter()
            .filter(|c| c.count > 0)
            .map(|c| c.key.parse().unwrap())
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(
            chips_of(Facet::Focal, &counts[..1], &[], &|v| v.to_string())[0].text,
            "100 mm"
        );
    }

    /// Ties at the cut do not push out the values held more often:
    /// a day's many one-frame focal lengths, first in the facet's
    /// order, leave the 200 mm held three times on the row.
    #[test]
    fn a_facet_row_keeps_the_most_held_when_the_cut_is_a_tie() {
        let mut counts: Vec<FacetCount> = (1..=16)
            .map(|mm| FacetCount {
                value: mm.to_string(),
                label: mm.to_string(),
                count: 1,
            })
            .collect();
        for (mm, n) in [(120, 2), (200, 3), (400, 6)] {
            counts.push(FacetCount {
                value: mm.to_string(),
                label: mm.to_string(),
                count: n,
            });
        }
        let chips = chips_of(Facet::Focal, &counts, &[], &|v| v.to_string());
        assert_eq!(chips.len(), CHIPS_A_FACET);
        for held in ["120", "200", "400"] {
            assert!(chips.iter().any(|c| c.key == held), "{held} mm kept");
        }
        // The rest of the room goes to the ties, in the facet's order.
        let ones: Vec<&str> = chips
            .iter()
            .filter(|c| c.count == 1)
            .map(|c| c.key.as_str())
            .collect();
        assert_eq!(ones, ["1", "2", "3", "4", "5", "6", "7", "8", "9"]);
        // A keyword on that no frame holds is named as written.
        let chips = chips_of(Facet::Keyword, &[], &["harbor".into()], &|_| {
            "Harbor".into()
        });
        assert_eq!(chips[0].text, "Harbor");
    }

    /// The timer's tick goes behind the launch pass, and is skipped for
    /// a root whose whole pass is still waiting or stopped partway:
    /// the launch's, or the last tick's.
    #[test]
    fn a_tick_waits_behind_the_launch_and_is_never_doubled() {
        let (share, local) = (PathBuf::from("/mnt/nas"), PathBuf::from("/home/p"));
        let mut q: VecDeque<Background> = VecDeque::new();
        queue(&mut q, Background::Root(share.clone()));
        queue(&mut q, Background::Root(local.clone()));
        // During the share's launch walk (in the queue, stopped or not
        // yet taken up): skipped.
        assert!(!queue_poll(&mut q, share.clone()));
        assert_eq!(q.len(), 2);
        // The launch walk done: a tick is queued, behind the local
        // root's launch pass.
        q.pop_front();
        assert!(queue_poll(&mut q, share.clone()));
        assert_eq!(
            Vec::from(q.clone()),
            vec![
                Background::Root(local.clone()),
                Background::Poll(share.clone())
            ]
        );
        // The next tick while that one is outstanding: skipped.
        assert!(!queue_poll(&mut q, share.clone()));
        assert_eq!(q.len(), 2);
        // A folder pass under the share is not a pass over the whole.
        q.clear();
        queue(
            &mut q,
            Background::Change(Change::Folder(share.join("day"))),
        );
        assert!(queue_poll(&mut q, share.clone()));
        assert_eq!(q.len(), 2);
        // A watcher's change still goes ahead of the tick.
        queue(&mut q, Background::Change(Change::Folder(local.join("x"))));
        assert_eq!(q.back(), Some(&Background::Poll(share)));
        // A share mounted below a local root: its tick is skipped while
        // the local root's launch pass, which walks it, is queued.
        q.clear();
        let below = local.join("NAS");
        queue(&mut q, Background::Root(local.clone()));
        assert!(!queue_poll(&mut q, below.clone()));
        q.clear();
        assert!(queue_poll(&mut q, below.clone()));
        assert_eq!(Vec::from(q), vec![Background::Poll(below)]);
    }
}
