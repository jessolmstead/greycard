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
//! looked at before a pass over it begins, with the roots' 3 s look
//! and a second try on it up to 6 s (`roots::PASS_PATIENCE`), and one
//! that does not answer is skipped; a pass that goes quiet in
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

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use greycard_library::{Change, FacetCount, Library, Report};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::filter::{self, Facet};
use crate::{App, FacetChip, FacetRange, FacetRow, Shared, State, tr};

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
    /// The lens database changed: every develop key in the index was
    /// made with the old one and is cleared, to be filled by the next
    /// pass over each folder.
    KeysStale,
    /// Drop the archive writes owed by frames under this root, or to
    /// it: the root was taken out of the library (§233).
    ForgetPendingUnder(PathBuf),
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
    /// The focus scorer wrote scores for rows in these folders: said
    /// to the window from the indexer's thread ([`Told::Scored`]).
    Scored(Vec<PathBuf>),
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
    /// Rows after saves are up to date, with the files among them found
    /// rewritten in place since their last pass, for the collections.
    FilesIndexed(Vec<greycard_library::Rehash>),
    /// The window's own moves are in the rows: the folders they left
    /// and went to, so a root's tree over them is built again. A move
    /// inside a root moves no root's count, and the folder passes
    /// after it find the rows already where they are.
    Moved(Vec<PathBuf>),
    /// The rows of files the window deleted are gone, this many, and
    /// the folders they were in, so a root's tree over them is built
    /// again: a rejects folder deleted whole leaves no row to be a
    /// node, and a delete inside a root need not move its count.
    Forgotten(usize, Vec<PathBuf>),
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
    /// The focus scorer has written scores for rows in these folders
    /// (`greycard_library::score`): the rows are read again, so a list
    /// by focus or a `focus` term fills in.
    Scored(Vec<PathBuf>),
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
    /// What the thread has in hand: see [`Busy`].
    busy: Arc<Busy>,
    /// What the passes are told of the editor's sidecars: see
    /// [`Indexer::set_lightroom_takes`].
    lightroom: Arc<Mutex<Option<greycard_edit::Placement>>>,
    thread: std::thread::JoinHandle<()>,
    /// The focus scorer's thread ([`score_after`]).
    scorer: Option<std::thread::JoinHandle<()>>,
}

/// Whether the indexer has anything in hand, for the window: while it
/// has, the index may yet change its mind about a file it says is
/// missing (a pass marks a folder's absent rows missing as it finishes
/// the folder, and sees a move only when it reaches where the file went).
#[derive(Debug)]
pub(crate) struct Busy {
    /// Asks sent and not yet taken by the thread: counted up before the
    /// send and down as the thread takes each, so an ask in the channel
    /// is always counted.
    owed: AtomicUsize,
    /// The thread has a pass or an ask in hand. True from its start (the
    /// library opened, and rebuilt for a schema that moved, before any
    /// ask), and false only while it waits with nothing but passes set
    /// aside.
    working: AtomicBool,
    /// The roots of the passes set aside because they stopped answering.
    aside: std::sync::Mutex<Vec<PathBuf>>,
}

impl Default for Busy {
    fn default() -> Busy {
        Busy {
            owed: AtomicUsize::new(0),
            working: AtomicBool::new(true),
            aside: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Busy {
    /// Whether anything is in hand or owed. A pass set aside on a root
    /// in `offline`, or under a network mount in `far`, is not counted:
    /// the frames there are kept by whoever asks, and a dead root left
    /// in the library (or a hung mount below a live one, never offline)
    /// would otherwise keep the indexer busy for the session.
    pub(crate) fn any(&self, offline: &HashSet<PathBuf>, far: &[PathBuf]) -> bool {
        let kept = |root: &PathBuf| {
            offline.iter().any(|o| root.starts_with(o)) || far.iter().any(|f| root.starts_with(f))
        };
        self.owed.load(Ordering::SeqCst) > 0
            || self.working.load(Ordering::SeqCst)
            || self
                .aside
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|root| !kept(root))
    }
}

/// Send `ask`, counted owed while it is in the channel.
fn send(asks: &mpsc::Sender<Ask>, busy: &Busy, ask: Ask) -> bool {
    busy.owed.fetch_add(1, Ordering::SeqCst);
    let sent = asks.send(ask).is_ok();
    if !sent {
        busy.owed.fetch_sub(1, Ordering::SeqCst);
    }
    sent
}

/// The indexer's ear, for a thread that is not the window's: the
/// watcher hands its changes on through this.
#[derive(Clone)]
pub(crate) struct Asker(mpsc::Sender<Ask>, Arc<Busy>);

impl Asker {
    pub(crate) fn changes(&self, changes: Vec<Change>) {
        send(&self.0, &self.1, Ask::Changes(changes));
    }

    /// A tree pass over each of `roots`, for the timer: see
    /// [`queue_poll`].
    pub(crate) fn poll(&self, roots: Vec<PathBuf>) {
        send(&self.0, &self.1, Ask::Poll(roots));
    }
}

/// How the indexer keeps a root that does not answer from holding the
/// rest: the look at a root before a pass over it, and how long a pass
/// may go without a word before it is set aside.
pub(crate) struct Guard {
    /// Whether a root answers, within whatever wait the look keeps.
    /// The editor's is the roots' 3 s look tried twice, up to 6 s in
    /// all (`roots::PASS_PATIENCE`), one thread a root at most.
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
        let busy = Arc::new(Busy::default());
        let lightroom = Arc::new(Mutex::new(None));
        let (score, nudged) = mpsc::channel();
        let waits = Waits {
            wanted: wanted.clone(),
            files: files_waiting.clone(),
            folders: folders_waiting.clone(),
            busy: busy.clone(),
            lightroom: lightroom.clone(),
            score,
        };
        let scorer = {
            let (path, asker, wanted) = (
                path.clone(),
                Asker(asks.clone(), busy.clone()),
                wanted.clone(),
            );
            std::thread::Builder::new()
                .name("greycard focus scores".into())
                .spawn(move || score_after(&path, &nudged, &asker, &wanted))
                .map_err(|e| tracing::warn!("index: no thread for the focus scores: {e}"))
                .ok()
        };
        let thread = std::thread::Builder::new()
            .name("greycard index".into())
            .spawn(move || run(path, waiting, told, waits, guard))?;
        Ok(Indexer {
            asks,
            wanted,
            files_waiting,
            folders_waiting,
            busy,
            lightroom,
            thread,
            scorer,
        })
    }

    /// Whether a pass is under way, or anything asked and not yet done:
    /// see [`Busy::any`].
    pub(crate) fn busy(&self, offline: &HashSet<PathBuf>, far: &[PathBuf]) -> bool {
        self.busy.any(offline, far)
    }

    fn ask(&self, ask: Ask) -> bool {
        send(&self.asks, &self.busy, ask)
    }

    /// Where the passes write the sidecar of a raw whose Lightroom
    /// develop they take, as the editor's setting says; none (sidecars
    /// off, and until it is set) takes nothing. From the next pass.
    pub(crate) fn set_lightroom_takes(&self, placement: Option<greycard_edit::Placement>) {
        *self.lightroom.lock().unwrap_or_else(|e| e.into_inner()) = placement;
    }

    pub(crate) fn folders(&self, dirs: Vec<PathBuf>, generation: u64) {
        self.wanted.store(generation, Ordering::SeqCst);
        self.folders_waiting.store(true, Ordering::SeqCst);
        self.ask(Ask::Folders { dirs, generation });
    }

    /// The launch pass over the roots, in this order, behind every
    /// folder the window asks for and every save.
    pub(crate) fn roots(&self, roots: Vec<PathBuf>) {
        self.launch(roots, Vec::new());
    }

    /// [`Indexer::roots`], with the network mounts below them that their
    /// walks leave out, in the one ask.
    pub(crate) fn launch(&self, roots: Vec<PathBuf>, left_out: Vec<PathBuf>) {
        self.ask(Ask::Roots { roots, left_out });
    }

    /// The network mounts below the local roots, which their walks
    /// leave out ([`Ask::LeftOut`]).
    pub(crate) fn left_out(&self, points: Vec<PathBuf>) {
        self.ask(Ask::LeftOut(points));
    }

    pub(crate) fn asker(&self) -> Asker {
        Asker(self.asks.clone(), self.busy.clone())
    }

    pub(crate) fn file(&self, path: PathBuf) {
        self.files_waiting.fetch_add(1, Ordering::SeqCst);
        if !self.ask(Ask::File(path)) {
            self.files_waiting.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// The lens database was fetched or updated: the develop keys in
    /// the index are cleared (`Ask::KeysStale`).
    pub(crate) fn keys_stale(&self) {
        self.ask(Ask::KeysStale);
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
        if !self.ask(Ask::Forget(paths)) {
            self.files_waiting.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// A root taken out of the library: the archive writes owed by the
    /// frames under it, or to it, are dropped (§233). The files' rows
    /// stay, as they always have.
    pub(crate) fn forget_pending_under(&self, root: PathBuf) {
        self.ask(Ask::ForgetPendingUnder(root));
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
        if !self.ask(Ask::Moved(moves)) {
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
        self.ask(Ask::Leave);
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
        // The scorer's connection closed with it, for the same reason.
        if let Some(scorer) = self.scorer {
            while !scorer.is_finished() && asked.elapsed() < within {
                std::thread::sleep(Duration::from_millis(5));
            }
            if scorer.is_finished() {
                let _ = scorer.join();
            } else {
                tracing::warn!(
                    "index: the focus scores still reading on the way out; leaving them"
                );
            }
        }
    }
}

/// How often the scorer tells the window of the scores it wrote while
/// a sweep goes on, besides once at its end.
const SCORED_EVERY: Duration = Duration::from_secs(2);

/// The focus scorer's body: woken after each pass (`nudged`), it scores
/// the rows the passes left unscored (`greycard_library::score`), on a
/// connection of its own opened at the first word, so the passes write
/// their rows at their own speed and the scores land behind them. What
/// it wrote goes to the window through the indexer (`Ask::Scored`).
fn score_after(path: &Path, nudged: &mpsc::Receiver<()>, asker: &Asker, wanted: &AtomicU64) {
    let leaving = || wanted.load(Ordering::SeqCst) == LEAVING;
    let mut lib: Option<Library> = None;
    loop {
        match nudged.recv_timeout(Duration::from_millis(200)) {
            Ok(()) => {}
            Err(mpsc::RecvTimeoutError::Timeout) if !leaving() => continue,
            _ => return,
        }
        // Every word waiting is one sweep.
        while nudged.try_recv().is_ok() {}
        if leaving() {
            return;
        }
        if lib.is_none() {
            match Library::open(path) {
                Ok(mut l) => {
                    if let Err(e) = l.set_busy_handler(lane_busy) {
                        tracing::debug!("index: the focus scores' busy handler: {e}");
                    }
                    let mut l = l;
                    l.set_network(
                        |p| greycard_ui::mounts::remote(p).is_some(),
                        Default::default(),
                    );
                    lib = Some(l);
                }
                Err(e) => {
                    tracing::warn!("index: no connection for the focus scores: {e}");
                    continue;
                }
            }
        }
        let Some(lib) = lib.as_mut() else {
            continue;
        };
        let started = Instant::now();
        let mut said = Instant::now();
        let mut unsaid: Vec<PathBuf> = Vec::new();
        let mut landed = |folders: &[PathBuf]| {
            for f in folders {
                if !unsaid.contains(f) {
                    unsaid.push(f.clone());
                }
            }
            if said.elapsed() >= SCORED_EVERY {
                said = Instant::now();
                send(&asker.0, &asker.1, Ask::Scored(std::mem::take(&mut unsaid)));
            }
        };
        let swept = greycard_library::score::score(
            lib,
            greycard_library::score::THREADS,
            &leaving,
            &mut landed,
        );
        if !unsaid.is_empty() {
            send(&asker.0, &asker.1, Ask::Scored(unsaid));
        }
        match swept {
            Ok(r) if r.scored > 0 => tracing::info!(
                "index: {} focus score(s) in {:.2} s",
                r.scored,
                started.elapsed().as_secs_f64()
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!("index: focus scores: {e}"),
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
    /// What the thread has in hand, for the window.
    busy: Arc<Busy>,
    /// Where a pass writes the sidecar of a raw whose Lightroom develop
    /// it takes, as the editor's setting says; none takes nothing.
    lightroom: Arc<Mutex<Option<greycard_edit::Placement>>>,
    /// Wakes the focus scorer: a pass wrote rows ([`score_after`]).
    score: mpsc::Sender<()>,
}

impl Waits {
    fn lightroom(&self) -> Option<greycard_edit::Placement> {
        *self.lightroom.lock().unwrap_or_else(|e| e.into_inner())
    }
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
    // A pass done, or a save's rows in: the scorer is woken for the
    // rows they left unscored.
    let nudge = waits.score.clone();
    let told = &|t: Told| {
        let wrote = matches!(
            t,
            Told::Indexed { .. } | Told::Background { .. } | Told::FilesIndexed(_)
        );
        told(t);
        if wrote {
            let _ = nudge.send(());
        }
    };
    let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        serve(path, waiting, told, &waits, &guard)
    }));
    if let Err(payload) = held {
        told(Told::Failed(tr!(
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
                lib.set_network(
                    |p| greycard_ui::mounts::remote(p).is_some(),
                    Default::default(),
                );
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
    // The develop keys in the rows were made under a recipe, a lens
    // database and the looks and profiles as they were then: any of
    // those moved since (a cache wiped, a refit while the editor was
    // away, a build that raised the recipe), they are cleared here for
    // the passes to make anew.
    match lib.settle_develop_keys() {
        Ok(Some(n)) => tracing::info!(
            "index: {n} develop key(s) cleared, made under what the develop is not any more"
        ),
        Ok(None) => {}
        Err(e) => tracing::warn!("index: the develop keys were not checked: {e}"),
    }
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
                    // In hand again before it leaves the set aside.
                    waits.busy.working.store(true, Ordering::SeqCst);
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
                    waits.busy.working.store(true, Ordering::SeqCst);
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
        // The roots of the passes set aside, said before the thread
        // may go idle on them.
        {
            let roots: Vec<PathBuf> = hung
                .jobs
                .iter()
                .map(|a| a.key.clone())
                .chain(hung.folders.iter().map(|a| a.key.clone()))
                .collect();
            let mut aside = waits.busy.aside.lock().unwrap_or_else(|e| e.into_inner());
            if *aside != roots {
                *aside = roots;
            }
        }
        // With a pass to take up again, only what is already waiting;
        // with passes set aside, the next ask or a moment, to look at
        // them again; with neither, wait for the next ask. Idle in both
        // waits: an ask sent meanwhile is counted owed until taken.
        let first = if pending.is_some() || !background.is_empty() {
            waiting.try_recv().ok()
        } else if !hung.jobs.is_empty() || !hung.folders.is_empty() {
            waits.busy.working.store(false, Ordering::SeqCst);
            match waiting.recv_timeout(Duration::from_millis(200)) {
                Ok(ask) => {
                    waits.busy.working.store(true, Ordering::SeqCst);
                    Some(ask)
                }
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            waits.busy.working.store(false, Ordering::SeqCst);
            match waiting.recv() {
                Ok(ask) => {
                    waits.busy.working.store(true, Ordering::SeqCst);
                    Some(ask)
                }
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
            // Taken: in hand now (working is set), owed no more.
            waits.busy.owed.fetch_sub(1, Ordering::SeqCst);
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
                Ask::KeysStale => {
                    match lib.clear_develop_keys() {
                        Ok(n) => tracing::info!(
                            "index: {n} develop key(s) cleared: what they are made under moved"
                        ),
                        Err(e) => tracing::warn!("index: the develop keys were not cleared: {e}"),
                    }
                    // What they will be made under from here, so the
                    // next launch does not clear them again.
                    if let Err(e) = lib.settle_develop_keys() {
                        tracing::warn!("index: what the develop keys are made under: {e}");
                    }
                }
                Ask::ForgetPendingUnder(root) => match lib.clear_pending_under(&root) {
                    Ok(n) if n > 0 => {
                        tracing::info!(
                            "index: {n} archive write(s) owed under {} dropped with the root",
                            root.display()
                        );
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("index: pending rows not dropped: {e}"),
                },
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
                Ask::Scored(folders) => told(Told::Scored(folders)),
                Ask::Leave => return,
            }
        }
        // The window's own moves first, so the saves after them find
        // each row at its new path; each read again there.
        let mut moved_through: Vec<PathBuf> = Vec::new();
        for (from, to) in &moves {
            match lib.file_moved(from, to) {
                Ok(_) => {
                    if !files.contains(to) {
                        files.push(to.clone());
                    }
                    for dir in [from.parent(), to.parent()].into_iter().flatten() {
                        if !moved_through.iter().any(|d| d == dir) {
                            moved_through.push(dir.to_path_buf());
                        }
                    }
                }
                Err(e) => tracing::warn!("index: {} moved, its row not: {e}", from.display()),
            }
        }
        if !moved_through.is_empty() {
            told(Told::Moved(moved_through));
        }
        if !files.is_empty() {
            lib.set_lightroom_takes(waits.lightroom());
            let mut rehashed = Vec::new();
            for f in &files {
                match lib.index_file(f) {
                    Ok(r) => rehashed.extend(r.rehashed),
                    Err(e) => tracing::warn!("index: {}: {e}", f.display()),
                }
            }
            told(Told::FilesIndexed(rehashed));
        }
        // After the saves: a save of a frame deleted since is a row
        // written for a file that is gone, and this takes it away.
        if !forget.is_empty() {
            match lib.forget(&forget) {
                Ok(n) => {
                    let mut folders: Vec<PathBuf> = Vec::new();
                    for dir in forget.iter().filter_map(|f| f.parent()) {
                        if !folders.iter().any(|d| d == dir) {
                            folders.push(dir.to_path_buf());
                        }
                    }
                    told(Told::Forgotten(n, folders));
                }
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
                lib.set_lightroom_takes(waits.lightroom());
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
                Err(greycard_library::Error::Io(std::io::Error::other(tr!(
                    "the passes' connection to the library could not be had"
                )))),
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
    let root = key.clone();
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
            lib.set_lightroom_takes(waits_on_lane.lightroom());
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
                &root,
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
                error: Some(tr!(
                    "the passes' connection to the library could not be had"
                )),
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
    root: &Path,
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
    let mut unanswered: Option<PathBuf> = None;
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
                // Every field, the moves and the renames among them: a
                // pass here may be the second half of a rename the
                // library pairs once.
                let stopped = r.stopped;
                report.add(r);
                if stopped {
                    break;
                }
            }
            // A read over a network mount that did not answer in its
            // limit: the folder is skipped as one whose root did not
            // answer, in the same words, and not asked again at once.
            // Taken for a busy library, it was retried five times, a
            // thread stranded on the share each time.
            Err(greycard_library::Error::Io(e)) if e.kind() == std::io::ErrorKind::TimedOut => {
                tracing::warn!("index: {}: {e}; skipped", dir.display());
                unanswered.get_or_insert(dir.clone());
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
    let skipped = error.is_none().then_some(unanswered).flatten();
    told(Told::Indexed {
        generation,
        report,
        seconds: pass.started.elapsed().as_secs_f64(),
        error,
    });
    if let Some(dir) = skipped {
        told(Told::Skipped {
            path: dir,
            root: root.to_path_buf(),
            launch: false,
            generation: Some(generation),
            first: false,
        });
    }
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
        Some(ids) if ids.len() == st.files.len() => st.views.index_ids = ids,
        // Each folder's canonical form as the read that brought the
        // list made it, off the window's thread: a folder opened by
        // hand is made canonical again by every read of it, so a link
        // pointed elsewhere since is followed there and not here.
        _ => {
            refresh_ids(st);
        }
    }
    let Some(indexer) = &st.views.index else {
        return;
    };
    // The all-roots view's list came from the index: the launch pass
    // and the watcher keep its rows, and a pass over every folder
    // under every root for it would be the launch pass again. A folder
    // pass still running for the list before is dropped, by the
    // generation, so it does not report over the view.
    if st.views.view.lists_rows() {
        drop_folder_pass(st);
        return;
    }
    st.views.index_generation += 1;
    st.views.index_progress = Some((0, st.files.len()));
    indexer.folders(folders_of(&st.files), st.views.index_generation);
}

/// A folder pass still running for the list before dropped, by the
/// generation, so it does not report over a list of the index's rows.
pub(crate) fn drop_folder_pass(st: &mut State) {
    let Some(indexer) = &st.views.index else {
        return;
    };
    st.views.index_generation += 1;
    st.views.index_progress = None;
    indexer.folders(Vec::new(), st.views.index_generation);
}

/// A file's sidecar was written: its row is brought up to date on
/// the indexer's thread, and the window hears when it is; and its
/// write to the archive's copy is queued (§233, [`crate::sync`]).
pub(crate) fn sidecar_written(st: &mut State, file: usize) {
    if let (Some(indexer), Some(path)) = (&st.views.index, st.files.get(file)) {
        indexer.file(path.clone());
    }
    // The pictures made from the edits, a cell at a time: the open
    // frame's kept from its develop, any other cell's (a version pasted
    // onto, a frame rated) shown as its line now is, made by the reduced
    // develop when none is kept.
    for c in st.cells_of_file(file) {
        crate::edited::saved(st, c);
    }
    crate::sync::after_save(st, file);
}

/// The window's read-only connection, opened once the indexer has
/// made the file, and tried again at each word from the indexer
/// while it will not open.
pub(crate) fn open_reader(st: &mut State, app: &App) {
    let Some(path) = st.views.index_path.clone() else {
        return;
    };
    if st.views.index_reader.is_some() {
        return;
    }
    let opened = Library::open_read_only(&path).and_then(|lib| {
        lib.set_busy_timeout(READ_WAIT)?;
        Ok(lib)
    });
    match opened {
        Ok(mut lib) => {
            lib.set_jpegs_apart(st.views.jpegs_apart);
            st.views.index_reader = Some(lib);
            if st.views.index_error.as_deref() == Some(READER_UNAVAILABLE) {
                st.views.index_error = None;
            }
            // The saves made before the reader was open are queued
            // now, and what waits for the archives is read from the
            // list the index kept across the quit (§233).
            crate::sync::reader_opened(st);
            crate::sync::refresh_note(st, app);
        }
        Err(e) => {
            tracing::warn!("library index {}: {e}", path.display());
            st.views.index_error = Some(READER_UNAVAILABLE.to_string());
        }
    }
}

/// In English, marked for the catalog: kept and compared as it is,
/// translated where it is shown.
pub(crate) const READER_UNAVAILABLE: &str =
    crate::marked::tr!("The library index could not be read");

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
    if st.views.index_reader.is_none() {
        st.views.index_ids = vec![None; st.files.len()];
        return false;
    }
    let standing: Vec<usize> = if st.views.view.lists_rows() {
        Vec::new()
    } else {
        (0..st.files.len())
            .filter(|&i| !crate::rows::file_is_loaded(st, i))
            .collect()
    };
    let paths: Vec<PathBuf> = standing.iter().map(|&i| st.files[i].clone()).collect();
    let (ids, rows) = {
        let lib = st
            .views
            .index_reader
            .as_ref()
            .expect("a reader, looked at above");
        // Each folder's canonical form as the window has kept it: a
        // read off the window's thread hands them over with the list,
        // so the disk is asked here only for a folder no read has seen
        // yet.
        let known = &mut st.views.library.canonical;
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
        Ok(ids) => st.views.index_ids = ids,
        Err(e) => {
            tracing::debug!("index: {e}; keeping the last answer");
            if st.views.index_ids.len() != st.files.len() {
                st.views.index_ids = vec![None; st.files.len()];
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
        st.views.index_ids.iter().flatten().count(),
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
    let typed = st.views.filter.typed();
    let everyone = vec![true; st.files.len()];
    let Some(lib) = &st.views.index_reader else {
        return everyone;
    };
    if !st.views.filter.asks_index(&typed) || st.views.index_ids.len() != st.files.len() {
        return everyone;
    }
    let started = Instant::now();
    let ids: Vec<i64> = st.views.index_ids.iter().flatten().copied().collect();
    match lib.ids_passing(&ids, &st.views.filter.index_filter(&typed, None)) {
        Ok(passing) => {
            tracing::debug!(
                "index: {} of {} rows pass in {:.1} ms",
                passing.len(),
                ids.len(),
                started.elapsed().as_secs_f64() * 1e3
            );
            st.views
                .index_ids
                .iter()
                .map(|id| id.is_none_or(|id| passing.contains(&id)))
                .collect()
        }
        Err(e) => {
            tracing::debug!("index: {e}; keeping the last answer");
            if st.views.index_passed.len() == st.files.len() {
                st.views.index_passed.clone()
            } else {
                everyone
            }
        }
    }
}

/// A facet's chips counted from the frames in hand
/// ([`filter::in_hand`]): each value and how many frames hold it
/// among those the other groups leave — the meta tests, and the
/// index's as the list was last made. The keyword's are from the
/// sidecars, case folded, the chip showing the first spelling met in
/// file order; the kind's are from the paths.
fn counts_in_hand(
    st: &State,
    typed: &filter::Typed,
    facet: Facet,
    behind: Option<&[bool]>,
) -> Vec<FacetCount> {
    let mut by: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for (i, (path, s)) in st.files.iter().zip(&st.sidecars).enumerate() {
        if behind.is_some_and(|b| b.get(i).copied().unwrap_or(false)) {
            continue;
        }
        let frame = filter::Frame {
            path,
            meta: &s.meta,
            index: st.views.index_passed.get(i).copied().unwrap_or(true),
        };
        if !frame.index || !st.views.filter.shows_meta(typed, frame, Some(facet)) {
            continue;
        }
        if facet == Facet::Kind {
            if let Some(kind) = greycard_library::filter::kind_of(path) {
                by.entry(kind.to_string())
                    .or_insert_with(|| (kind.to_string(), 0))
                    .1 += 1;
            }
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

/// Every facet's chips, in [`Facet::ALL`]'s order: each value among
/// the frames the filter's other tests leave, with its count — the
/// rule the meta rows keep (`filter::Counts`). ISO and the focal
/// length have no chips: the CULLING section's ranges stand for them
/// ([`facet_ranges`]), and they are not counted here. The EXIF facets are
/// one `GROUP BY` each on the index: the tests the frames in hand
/// answer pick the rows (`within`), and the index's own tests, less
/// the facet's, are the query's filter. The keyword and the kind are
/// counted in hand, with or without an index. A read that fails
/// keeps the last answer.
pub(crate) fn facet_counts(st: &State) -> Vec<(Facet, Vec<FacetCount>)> {
    let typed = st.views.filter.typed();
    // A closed stack is its top: the frames behind it are not counted.
    let behind = crate::stacks::behind(st);
    let in_hand: Vec<(Facet, Vec<FacetCount>)> = Facet::ALL
        .into_iter()
        .filter(|f| filter::in_hand(*f))
        .map(|f| (f, counts_in_hand(st, &typed, f, behind.as_deref())))
        .collect();
    let Some(lib) = &st.views.index_reader else {
        return in_hand;
    };
    if st.views.index_ids.len() != st.files.len() {
        return in_hand;
    }
    let within: Vec<i64> = st
        .files
        .iter()
        .zip(&st.sidecars)
        .zip(&st.views.index_ids)
        .enumerate()
        .filter(|(i, _)| !behind.as_ref().is_some_and(|b| b[*i]))
        .filter_map(|(_, ((path, s), id))| {
            let frame = filter::Frame {
                path,
                meta: &s.meta,
                index: true,
            };
            id.filter(|_| st.views.filter.shows_meta(&typed, frame, None))
        })
        .collect();
    // The index's rows and the ones in hand, back in the bar's order.
    let in_order = |mut rows: Vec<(Facet, Vec<FacetCount>)>, in_hand| {
        rows.extend(in_hand);
        rows.sort_by_key(|(f, _)| filter::facet_slot(*f));
        rows
    };
    let mut rows = Vec::new();
    for facet in Facet::ALL {
        if filter::in_hand(facet) || filter::ranged(facet).is_some() {
            continue;
        }
        let asked = st.views.filter.index_filter(&typed, Some(facet));
        match lib.facet_counts(facet, Some(&within), &asked) {
            Ok(counts) => rows.push((facet, counts)),
            Err(e) => {
                tracing::debug!("index: {facet:?}: {e}; keeping the last answer");
                let mut last = st.views.facets_last.borrow().clone();
                last.retain(|(f, _)| !filter::in_hand(*f));
                return in_order(last, in_hand);
            }
        }
    }
    let rows = in_order(rows, in_hand);
    *st.views.facets_last.borrow_mut() = rows.clone();
    rows
}

/// A facet's chips as the bar shows them: at most [`CHIPS_A_FACET`]
/// of the values neither on nor held out (`out`) — every value held
/// more often than the twelfth most held, then as many of those tied
/// with it as there is room for, in the facet's order — and every
/// value on or held out, whatever its count, a value no frame holds
/// any more among them at zero, since pressing the chip is how the
/// filter is undone. `label_of` names such a value. The chips stand
/// in the facet's own order.
pub(crate) fn chips_of(
    facet: Facet,
    counts: &[FacetCount],
    on: &[String],
    out: &[String],
    label_of: &dyn Fn(&str) -> String,
) -> Vec<FacetChip> {
    let held = |v: &String| on.contains(v) || out.contains(v);
    let off: Vec<&FacetCount> = counts.iter().filter(|c| !held(&c.value)).collect();
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
        .filter(|c| held(&c.value) || keep.contains(c.value.as_str()))
        .map(|c| FacetChip {
            text: filter::facet_chip_text(facet, &c.label).into(),
            key: c.value.clone().into(),
            count: c.count as i32,
            on: on.contains(&c.value),
            out: out.contains(&c.value),
        })
        .collect();
    for value in on.iter().chain(out) {
        if !counts.iter().any(|c| c.value == *value) {
            chips.push(FacetChip {
                text: filter::facet_chip_text(facet, &label_of(value)).into(),
                key: value.clone().into(),
                count: 0,
                on: on.contains(value),
                out: out.contains(value),
            });
        }
    }
    chips
}

/// The rows the bar shows: a facet with nothing to offer and nothing
/// on is left out, and so is a kind with one chip and it off: every
/// frame in an all-raw folder is a raw, and a row that says so asks
/// nothing a press could narrow.
pub(crate) fn facet_rows(st: &State) -> Vec<FacetRow> {
    let started = Instant::now();
    let counts = facet_counts(st);
    if st.views.index_reader.is_some() {
        tracing::debug!(
            "index: {} facets counted over {} frames in {:.1} ms",
            Facet::ALL.len(),
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
            let on = st.views.filter.chosen(facet);
            let out = st.views.filter.held_out(facet);
            let chips = if facet == Facet::Keyword {
                chips_of(facet, &counts, on, out, &label_of)
            } else {
                chips_of(facet, &counts, on, out, &|v| v.to_string())
            };
            let says_nothing = chips.is_empty()
                || (facet == Facet::Kind && chips.len() == 1 && !chips[0].on && !chips[0].out);
            (!says_nothing).then(|| FacetRow {
                name: crate::i18n::text(filter::facet_caption(facet)).into(),
                code: filter::facet_slot(facet) as i32,
                chips: ModelRc::new(VecModel::from(chips)),
            })
        })
        .collect()
}

/// The rows split for the grid's header, which has two: camera and
/// lens, whose chips are long, then the rest, which are short, the
/// kind first among them as it is in [`Facet::ALL`].
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

/// A ranged facet's values over the whole folder, as the index counts
/// them: not narrowed by the rest of the filter, as a chip row is, so
/// the stops stay put while the filter changes and the range's own
/// ends never take values off it. The frames behind a closed stack are
/// left out, as the chips' counts leave them. None until the index has
/// the folder's rows. The answer depends on the frames and their rows
/// alone, so it is kept against them and read again only when they
/// change (another folder, a frame indexed, a stack opened or closed);
/// a read that fails keeps the last answer and asks again next time.
fn folder_values(st: &State) -> Vec<(Facet, Vec<FacetCount>)> {
    use std::hash::{Hash, Hasher};
    let Some(lib) = &st.views.index_reader else {
        return Vec::new();
    };
    if st.views.index_ids.len() != st.files.len() {
        return Vec::new();
    }
    let behind = crate::stacks::behind(st);
    let within: Vec<i64> = (st.views.index_ids.iter().enumerate())
        .filter(|(i, _)| !behind.as_ref().is_some_and(|b| b[*i]))
        .filter_map(|(_, id)| *id)
        .collect();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    lib.path().hash(&mut hasher);
    within.hash(&mut hasher);
    let key = hasher.finish();
    let mut kept = st.views.ranges_last.borrow_mut();
    if let Some((k, values)) = kept.as_ref()
        && *k == key
    {
        return values.clone();
    }
    let mut values = Vec::new();
    for (facet, _) in filter::RANGED {
        match lib.facet_counts(facet, Some(&within), &greycard_library::Filter::default()) {
            Ok(counts) => values.push((facet, counts)),
            Err(e) => {
                tracing::debug!("index: {facet:?} range: {e}; keeping the last answer");
                return kept.as_ref().map(|(_, v)| v.clone()).unwrap_or_default();
            }
        }
    }
    *kept = Some((key, values.clone()));
    values
}

/// A ranged facet's row: the values in order, as shown and as a term
/// writes them, and the ends the filter's text puts them at.
pub(crate) fn range_of(facet: Facet, counts: &[FacetCount], text: &str) -> FacetRange {
    let mut values: Vec<(f64, &FacetCount)> = counts
        .iter()
        .filter_map(|c| c.value.parse::<f64>().ok().map(|v| (v, c)))
        .collect();
    values.sort_by(|a, b| a.0.total_cmp(&b.0));
    let numbers: Vec<f64> = values.iter().map(|(v, _)| *v).collect();
    let shown: Vec<SharedString> = values
        .iter()
        .map(|(_, c)| filter::facet_chip_text(facet, &c.label).into())
        .collect();
    let keys: Vec<SharedString> = values
        .iter()
        .map(|(_, c)| c.value.as_str().into())
        .collect();
    let widest = shown
        .iter()
        .max_by_key(|s| s.chars().count())
        .cloned()
        .unwrap_or_default();
    let (low, high, on) = filter::ranged(facet)
        .map(|field| filter::range_ends(text, field, &numbers))
        .unwrap_or((0, numbers.len().saturating_sub(1), false));
    FacetRange {
        name: crate::i18n::text(filter::facet_caption(facet)).into(),
        code: filter::facet_slot(facet) as i32,
        values: ModelRc::new(VecModel::from(shown)),
        keys: ModelRc::new(VecModel::from(keys)),
        widest,
        low: low as i32,
        high: high as i32,
        on,
    }
}

/// The CULLING section's ranges, in [`filter::RANGED`]'s order.
pub(crate) fn facet_ranges(st: &State) -> Vec<FacetRange> {
    let values = folder_values(st);
    filter::RANGED
        .iter()
        .map(|(facet, _)| {
            let counts = (values.iter().find(|(f, _)| f == facet))
                .map(|(_, c)| c.as_slice())
                .unwrap_or_default();
            range_of(*facet, counts, &st.views.filter.text)
        })
        .collect()
}

/// What the facets' row says before its chips: the index's error, its
/// progress or its opening, whatever rows there are — a mixed folder
/// has its kind's row before the index has a row of its own — and,
/// with no rows at all, that the files have no EXIF to count.
pub(crate) fn facet_note(st: &State, rows: bool) -> String {
    if let Some(e) = &st.views.index_error {
        // READER_UNAVAILABLE is kept in English, to be compared; any
        // other error is in the window's words already.
        return if e == READER_UNAVAILABLE {
            crate::i18n::text(e)
        } else {
            e.clone()
        };
    }
    match (&st.views.index_reader, st.views.index_progress) {
        (_, Some((done, total))) => {
            tr!(
                "Indexing the folder: {done} of {total}",
                done = done,
                total = total
            )
        }
        (Some(_), None) if rows => String::new(),
        (Some(_), None) => tr!("No camera, lens or date in these files' EXIF"),
        (None, None) if st.views.index.is_some() => tr!("Opening the library index"),
        (None, None) => String::new(),
    }
}

/// The facets `--filter` named, chosen once the index has rows to
/// choose them from, among the values the open frames hold: with
/// `:` a text facet's value by what it contains, with `=` by what it
/// is, case aside; a number's by what it equals either way. True
/// when something was chosen.
pub(crate) fn choose_wanted(st: &mut State) -> bool {
    let wanted = std::mem::take(&mut st.launch.facets_wanted);
    let ids: Vec<i64> = st.views.index_ids.iter().flatten().copied().collect();
    let mut chose = false;
    for want in wanted {
        let counts = if filter::in_hand(want.facet) {
            counts_in_hand(st, &filter::Typed::default(), want.facet, None)
        } else {
            let Some(lib) = &st.views.index_reader else {
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
        // ISO and the focal length never come here: `filter::from_cli`
        // leaves their terms for the text.
        let needle = want.value.to_lowercase();
        let hits: Vec<String> = counts
            .into_iter()
            .filter(|c| {
                if want.exact {
                    c.value.to_lowercase() == needle
                } else {
                    c.value.to_lowercase().contains(&needle)
                }
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
            if !st.views.filter.chosen(want.facet).contains(&value) {
                st.views.filter.toggle_facet(want.facet, &value);
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
    open_reader(&mut state.borrow_mut(), app);
    // A pass may have brought words in from files the window never
    // wrote (an XMP, a catalog, another copy): the Keywords field's
    // words are read again the next time it is used. A save's row is
    // not such a pass; the words the window puts on it are its own.
    if matches!(
        told,
        Told::Opened(_) | Told::Indexed { .. } | Told::Background { .. }
    ) {
        crate::panel::words::forget_vocabulary(&mut state.borrow_mut());
    }
    match told {
        Told::Opened(path) => {
            tracing::info!("library index {}", path.display());
            let mut st = state.borrow_mut();
            st.views.index_path = Some(path);
            open_reader(&mut st, app);
            crate::roots::recount(&mut st);
            crate::panel::collections::members_changed(&mut st, app);
            // The launch's folder, when it lies under a root, has its
            // root's tree once the index can be read.
            crate::tree::want(&mut st, app);
            crate::roots::show(&st, app);
            let wanted = st.views.library.wanted.take();
            // The index is open: the last folder kept for its failing is
            // not wanted any more.
            st.launch.last_folder = None;
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
            // A file another tool rewrote in place heals the collections
            // that hold it, in the same pass that found it; the marks
            // follow the rows the pass wrote.
            crate::panel::collections::healed(&state, app, &report.rehashed);
            crate::panel::collections::report_landed(&mut state.borrow_mut(), app, &report);
            crate::panel::stacks::healed(&state, app, crate::stacks::Found::of(&report));
            crate::panel::carried::landed(&state, app, &report.sets);
            crate::panel::carried::leftovers(&state, &report.sets_stale);
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
                if let Some(archive) = st
                    .views
                    .library
                    .roots
                    .archive_of(&path)
                    .map(Path::to_path_buf)
                {
                    crate::panel::archive::rejects::heard_from(&mut st, app, &archive, true);
                    crate::sync::heard_from(&mut st, app, &archive, true);
                }
            }
        }
        Told::Scored(folders) => {
            // Read as a pass that refreshed rows there: a list under one
            // of them read again (a sort by focus), else the rows.
            let some = Report {
                meta_refreshed: 1,
                ..Report::default()
            };
            let merged = folders
                .iter()
                .any(|f| crate::roots::background_done(&state, app, f, &some, false, false));
            if !merged {
                reread(&state, app, false);
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
            crate::sync::heard_from(&mut state.borrow_mut(), app, &root, false);
            let words = skipped_words(&state.borrow().views.library.roots, &root);
            if first {
                app.set_status(words.clone().into());
            }
            match generation {
                // The window's own folder: its pass is over, said in the
                // filter's line, and the rows the window has are kept.
                Some(generation) => {
                    let mut st = state.borrow_mut();
                    if generation != st.views.index_generation {
                        return;
                    }
                    st.views.index_progress = None;
                    st.launch.awaiting_index = false;
                    st.views.index_error = Some(words);
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
            st.views.index = None;
            // The view of the roots asked for will not come, and a
            // folder opened meanwhile is recorded as any other. A
            // capture waiting on it ends.
            let wanted = st.views.library.wanted.take();
            st.views.library.awaiting = false;
            // A launch that was to open its start folder's view opens
            // the last folder instead, as it would with no root.
            let last = st.launch.last_folder.take();
            let last = last.filter(|_| wanted.is_some() && st.files.is_empty());
            st.views.index_progress = None;
            st.views.index_error = Some(tr!("No library index: {message}", message = message));
            st.launch.awaiting_index = false;
            crate::panel::cull::show_filter(&st, app);
            app.window().request_redraw();
            drop(st);
            if let (Some(dir), Some(worker)) = (last, crate::WORKER.with(|w| w.borrow().clone())) {
                tracing::info!("opening the last folder {} instead", dir.display());
                crate::panel::browser::open_folder(&state, app, &worker, &dir);
            }
        }
        Told::Progress {
            generation,
            done,
            total,
        } => {
            if generation != state.borrow().views.index_generation {
                return;
            }
            state.borrow_mut().views.index_progress = Some((done, total));
            reread(&state, app, false);
        }
        Told::Indexed {
            generation,
            report,
            seconds,
            error,
        } => {
            // Healed whichever folder the pass was over: the file it
            // found rewritten is rewritten whether or not the window
            // is still there.
            crate::panel::collections::healed(&state, app, &report.rehashed);
            crate::panel::collections::report_landed(&mut state.borrow_mut(), app, &report);
            crate::panel::stacks::healed(&state, app, crate::stacks::Found::of(&report));
            crate::panel::carried::landed(&state, app, &report.sets);
            crate::panel::carried::leftovers(&state, &report.sets_stale);
            if generation != state.borrow().views.index_generation {
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
                st.views.index_tries += 1;
                if st.views.index_tries <= RETRIES {
                    // Busy, most likely: said, and asked again in a
                    // moment. A capture waiting on the chips waits on.
                    st.views.index_error =
                        Some(tr!("Library index busy; trying again ({e})", e = e));
                    st.views.index_progress = None;
                    let app_weak = app.as_weak();
                    slint::Timer::single_shot(RETRY_AFTER, move || {
                        let Some(app) = app_weak.upgrade() else {
                            return;
                        };
                        let Some(state) = crate::STATE.with(|s| s.borrow().clone()) else {
                            return;
                        };
                        let mut st = state.borrow_mut();
                        if st.views.index_generation == generation {
                            index_open_folder(&mut st, None);
                            crate::panel::cull::show_filter(&st, &app);
                        }
                    });
                    crate::panel::cull::show_filter(&st, app);
                    return;
                }
                tracing::warn!("index: giving up on the folder after {RETRIES} tries: {e}");
                st.views.index_error = Some(tr!("Library index unavailable: {e}", e = e));
            } else {
                st.views.index_tries = 0;
                st.views.index_error = None;
            }
            st.views.index_progress = None;
            let moved = refresh_ids(&mut st);
            let chose = !st.launch.facets_wanted.is_empty() && choose_wanted(&mut st);
            st.launch.awaiting_index = false;
            // A folder sorted by what only its rows say (its capture
            // times, its ratings) and read before the index had them is
            // read again in that order once the pass has written them.
            let resort = st.views.view == crate::roots::View::Folder
                && st.views.sort.choice(&st.views.view).order().map(|o| o.by)
                    != Some(greycard_library::SortBy::Name)
                && report.added + report.changed + report.meta_refreshed > 0;
            drop(st);
            reread(&state, app, chose || moved);
            if resort && let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) {
                crate::roots::refresh_view(&state, app, &worker);
            }
            app.window().request_redraw();
        }
        Told::FilesIndexed(rehashed) => {
            // A save's row moves no member unless the file under it was
            // rewritten: only then are the marks asked again.
            if !rehashed.is_empty() {
                crate::panel::collections::healed(&state, app, &rehashed);
                crate::panel::collections::members_changed(&mut state.borrow_mut(), app);
                crate::panel::stacks::healed(
                    &state,
                    app,
                    crate::stacks::Found {
                        moved: &[],
                        folders: &[],
                        rehashed: &rehashed,
                    },
                );
            }
            reread(&state, app, false);
            // A file another editor wrote for Edit in, listed now its
            // row is in.
            crate::panel::edit_in::indexed(&state, app);
            crate::panel::merge::indexed(&state, app);
            // A shift's rows written: a list by capture time is read
            // again in the new order.
            if let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) {
                crate::panel::time_shift::indexed(&state, app, &worker);
            }
        }
        Told::Moved(folders) => {
            let mut st = state.borrow_mut();
            for folder in &folders {
                crate::tree::passed(&mut st, folder);
            }
            crate::tree::want(&mut st, app);
        }
        Told::Forgotten(n, folders) => {
            tracing::debug!("index: {n} rows of deleted files forgotten");
            {
                let mut st = state.borrow_mut();
                for folder in &folders {
                    crate::tree::passed(&mut st, folder);
                }
                crate::tree::want(&mut st, app);
                // Rows gone outside any pass's report.
                crate::panel::collections::members_changed(&mut st, app);
            }
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
    tr!(
        "{name} isn't reachable; showing its last indexed frames",
        name = name
    )
}

/// The index moved under the window: the rows read again, and the
/// browser's list rebuilt when the filter asks the index something
/// and the answer changed; otherwise only the chips.
fn reread(state: &Rc<Shared>, app: &App, changed: bool) {
    let mut st = state.borrow_mut();
    let moved = refresh_ids(&mut st);
    let pass = index_pass(&st);
    if changed || moved || pass != st.views.index_passed {
        // The list is made from this answer, not asked again.
        st.views.index_passed = pass;
        st.views.index_pass_ready = true;
        drop(st);
        crate::panel::cull::refilter(state, app, changed);
    } else {
        crate::panel::cull::show_filter(&st, app);
        drop(st);
    }
    // The frame under the loupe may be one this answer reached.
    crate::panel::cull::show_shot(&state.borrow(), app);
}

#[cfg(test)]
mod busy_tests {
    use super::*;

    /// An ask in the channel is busy before the thread takes it, and a
    /// pass set aside on a root counts until that root is offline.
    #[test]
    fn busy_counts_an_ask_owed_and_a_pass_aside_on_a_root_not_offline() {
        let (tx, rx) = mpsc::channel();
        let busy = Busy::default();
        busy.working.store(false, Ordering::SeqCst);
        let none = HashSet::new();
        assert!(!busy.any(&none, &[]));
        assert!(send(&tx, &busy, Ask::KeysStale));
        assert!(busy.any(&none, &[]), "owed while in the channel");
        let _ = rx.recv().unwrap();
        busy.owed.fetch_sub(1, Ordering::SeqCst);
        assert!(!busy.any(&none, &[]));
        let root = PathBuf::from("/photos/nas");
        busy.aside.lock().unwrap().push(root.clone());
        assert!(
            busy.any(&none, &[]),
            "a pass set aside on a root not known offline"
        );
        assert!(!busy.any(&HashSet::from([root]), &[]));
        // A network mount below a live root: never offline, kept by far.
        assert!(!busy.any(&none, &[PathBuf::from("/photos")]));
    }
}

#[cfg(test)]
mod tests;
