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
    /// The launch pass: each root's tree, one after another, in the
    /// background of everything else.
    Roots(Vec<PathBuf>),
    /// What the watcher saw change under the roots, in the
    /// background too, but ahead of the launch pass.
    Changes(Vec<Change>),
    /// The editor is leaving: wakes a thread waiting on the next
    /// ask, which the watcher's end of the channel would otherwise
    /// keep waiting.
    Leave,
}

/// A pass in the background: the launch pass over a root, or a
/// change the watcher saw.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Background {
    Root(PathBuf),
    Change(Change),
}

impl Background {
    fn path(&self) -> &Path {
        match self {
            Background::Root(p) => p,
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
}

impl Indexer {
    /// Open the library at `path` on a thread of its own and wait
    /// there for folders and files. `told` is called on that thread.
    pub(crate) fn start(
        path: PathBuf,
        told: impl Fn(Told) + Send + 'static,
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
            .spawn(move || run(path, waiting, told, waits))?;
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
        let _ = self.asks.send(Ask::Roots(roots));
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
fn run(path: PathBuf, waiting: mpsc::Receiver<Ask>, told: impl Fn(Told), waits: Waits) {
    let told = &told;
    let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        serve(path, waiting, told, &waits)
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
/// ahead of the launch pass's roots, which are long and can wait.
fn queue(background: &mut VecDeque<Background>, job: Background) {
    if background.contains(&job) {
        return;
    }
    match job {
        Background::Change(_) => {
            let at = background
                .iter()
                .position(|b| matches!(b, Background::Root(_)))
                .unwrap_or(background.len());
            background.insert(at, job);
        }
        Background::Root(_) => background.push_back(job),
    }
}

fn serve(path: PathBuf, waiting: mpsc::Receiver<Ask>, told: &dyn Fn(Told), waits: &Waits) {
    let mut lib = match Library::open(&path) {
        Ok(lib) => lib,
        Err(e) => {
            told(Told::Failed(format!("{}: {e}", path.display())));
            return;
        }
    };
    told(Told::Opened(path));
    let wanted = &*waits.wanted;
    let files_waiting = &*waits.files;
    let mut pending: Option<Pass> = None;
    let mut background: VecDeque<Background> = VecDeque::new();
    // The tree passes stopped partway, by their folder.
    let mut walks: HashMap<PathBuf, greycard_library::TreeWalk> = HashMap::new();
    loop {
        if wanted.load(Ordering::SeqCst) == LEAVING {
            return;
        }
        // With a pass to take up again, only what is already
        // waiting; with none, wait for the next ask.
        let first = if pending.is_some() || !background.is_empty() {
            waiting.try_recv().ok()
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
        for ask in first.into_iter().chain(waiting.try_iter()) {
            match ask {
                Ask::File(p) => {
                    files_waiting.fetch_sub(1, Ordering::SeqCst);
                    if !files.contains(&p) {
                        files.push(p);
                    }
                }
                Ask::Folders { dirs, generation } => {
                    waits.folders.store(false, Ordering::SeqCst);
                    pending = Some(Pass {
                        dirs,
                        generation,
                        started: Instant::now(),
                    });
                }
                Ask::Roots(roots) => {
                    for root in roots {
                        queue(&mut background, Background::Root(root));
                    }
                }
                Ask::Changes(changes) => {
                    for change in changes {
                        queue(&mut background, Background::Change(change));
                    }
                }
                Ask::Leave => return,
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
        if let Some(pass) = pending.take() {
            pending = window_pass(&mut lib, pass, told, waits);
            continue;
        }
        let Some(job) = background.pop_front() else {
            continue;
        };
        // In the background: anything the window asks for comes
        // first, and this pass takes up again after it, from where it
        // stopped (a tree's walk is kept for that).
        let stop = || {
            wanted.load(Ordering::SeqCst) == LEAVING
                || files_waiting.load(Ordering::SeqCst) > 0
                || waits.folders.load(Ordering::SeqCst)
        };
        let started = Instant::now();
        // A long pass says now and then that it has written rows, so
        // a view of the roots fills in while a large root is walked
        // for the first time rather than all at once at the end; and
        // says nothing while it finds nothing new.
        let mut last = Instant::now();
        let mut written: (PathBuf, usize) = (PathBuf::new(), 0);
        let mut unsaid = false;
        let mut progress = |p: greycard_library::Progress<'_>| {
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
                told(Told::BackgroundProgress {
                    path: job.path().to_path_buf(),
                });
            }
        };
        let tree = match &job {
            Background::Root(dir) | Background::Change(Change::Tree(dir)) => Some(dir.clone()),
            Background::Change(Change::Folder(_)) => None,
        };
        let passed = match (&job, &tree) {
            (_, Some(dir)) => {
                let walk = match walks.remove(dir) {
                    Some(w) => Ok(w),
                    None => lib.tree_walk(dir),
                };
                walk.and_then(|mut w| {
                    let r = lib.walk_until(&mut w, &mut progress, &stop);
                    if r.as_ref().is_ok_and(|r| r.stopped) {
                        walks.insert(dir.clone(), w);
                    }
                    r
                })
            }
            (Background::Change(Change::Folder(dir)), None) => {
                lib.index_folder_until(dir, &mut progress, &stop)
            }
            _ => unreachable!("a tree job has its folder"),
        };
        let (report, error) = match passed {
            Ok(r) if r.stopped => {
                background.push_front(job);
                continue;
            }
            Ok(r) => (r, None),
            Err(e) => {
                // A folder the watcher named that went again before
                // it was reached, most likely: the pass over its
                // parent, which the same event brings, says it.
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
    }
}

/// The window's folder pass, stopped for a save or for a folder
/// since opened: what is left of it to take up again, if anything.
fn window_pass(lib: &mut Library, pass: Pass, told: &dyn Fn(Told), waits: &Waits) -> Option<Pass> {
    let wanted = &*waits.wanted;
    let files_waiting = &*waits.files;
    let generation = pass.generation;
    if wanted.load(Ordering::SeqCst) != generation {
        return None;
    }
    let stop =
        || wanted.load(Ordering::SeqCst) != generation || files_waiting.load(Ordering::SeqCst) > 0;
    let mut report = Report::default();
    let mut error = None;
    let mut last = Instant::now();
    let mut before = 0;
    for dir in &pass.dirs {
        let passed = lib.index_folder_until(
            dir,
            &mut |p| {
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
/// this pass has looked at a file.
pub(crate) fn index_open_folder(st: &mut State) {
    refresh_ids(st);
    let Some(indexer) = &st.index else {
        return;
    };
    // The all-roots view's list came from the index: the launch pass
    // and the watcher keep its rows, and a pass over every folder
    // under every root for it would be the launch pass again. A folder
    // pass still running for the list before is dropped, by the
    // generation, so it does not report over the view.
    if matches!(st.view, crate::roots::View::Roots(_)) {
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
/// last answer.
pub(crate) fn refresh_ids(st: &mut State) {
    let started = Instant::now();
    let Some(lib) = &st.index_reader else {
        st.index_ids = vec![None; st.files.len()];
        return;
    };
    match lib.ids_of(&st.files) {
        Ok(ids) => st.index_ids = ids,
        Err(e) => {
            tracing::debug!("index: {e}; keeping the last answer");
            if st.index_ids.len() != st.files.len() {
                st.index_ids = vec![None; st.files.len()];
            }
            return;
        }
    }
    tracing::debug!(
        "index: {} of {} frames have rows, read in {:.1} ms",
        st.index_ids.iter().flatten().count(),
        st.files.len(),
        started.elapsed().as_secs_f64() * 1e3
    );
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
                     {} meta refreshed, {} unchanged, {} missing",
                    path.display(),
                    report.added,
                    report.moved,
                    report.changed,
                    report.meta_refreshed,
                    report.unchanged,
                    report.missing,
                ),
            }
            // A merge reads the rows and the facets again with the
            // list; only without one are they read here.
            if !crate::roots::background_done(&state, app, &path, &report, launch) {
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
            if !crate::roots::background_done(&state, app, &path, &some, false) {
                reread(&state, app, false);
            }
        }
        Told::Failed(message) => {
            tracing::warn!("no library index: {message}");
            let mut st = state.borrow_mut();
            st.index = None;
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
                 {} meta refreshed, {} unchanged, {} missing{}",
                report.added,
                report.moved,
                report.changed,
                report.meta_refreshed,
                report.unchanged,
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
                            index_open_folder(&mut st);
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
            refresh_ids(&mut st);
            let chose = !st.facets_wanted.is_empty() && choose_wanted(&mut st);
            st.awaiting_index = false;
            drop(st);
            reread(&state, app, chose);
            app.window().request_redraw();
        }
        Told::FilesIndexed => reread(&state, app, false),
    }
}

/// The index moved under the window: the rows read again, and the
/// browser's list rebuilt when the filter asks the index something
/// and the answer changed; otherwise only the chips.
fn reread(state: &Rc<RefCell<State>>, app: &App, changed: bool) {
    let mut st = state.borrow_mut();
    refresh_ids(&mut st);
    let pass = index_pass(&st);
    if changed || pass != st.index_passed {
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
        dir
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        app.set_filter_facets(ModelRc::new(VecModel::from(vec![FacetRow {
            name: "ISO".into(),
            code: 2,
            chips: ModelRc::new(VecModel::from(chips)),
        }])));
        let pressed = Rc::new(RefCell::new(Vec::new()));
        let seen = pressed.clone();
        app.on_filter_facet_toggled(move |_, key| seen.borrow_mut().push(key.to_string()));
        // The facet row is the header's fourth, under the controls,
        // the library's roots and the meta chips.
        let (x, y) = (200.0, 120.0);
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
        std::fs::remove_dir_all(&dir).unwrap();
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
}
