//! The roots' tests: the hooks the production code reaches, and the
//! helpers more than one file of tests uses.

use super::*;
// A glob, so a name reached only by the unix-only tests is no unused
// import on Windows.
use super::{kept_up::*, pane::*, view::*, watcher::*};
use crate::testing::{state_for, window};
use greycard_library::fixture::{A7, R5, R6, write_frame};
use std::time::Duration;

mod lists;
mod loading;
mod pairs;
mod pane;
mod picks;
mod reads;
mod sort;
mod watcher;

thread_local! {
    /// The reads sent off the window's thread, waiting for the
    /// test to run and land them.
    pub(super) static SENT: RefCell<Vec<Look>> = const { RefCell::new(Vec::new()) };
    /// The watchers asked for, waiting for the test to build and
    /// take them.
    pub(super) static BUILDS: RefCell<Vec<Plan>> = const { RefCell::new(Vec::new()) };
    /// What the test says each root is on: a network mount's type, or
    /// none for a local disk. A root it says nothing of is a panic.
    pub(super) static REMOTE: RefCell<HashMap<PathBuf, Option<String>>> = RefCell::new(HashMap::new());
    /// The network mounts the test says are below a root, with their
    /// types; none for a root it says nothing of.
    pub(super) static REMOTE_UNDER: RefCell<HashMap<PathBuf, Below>> = RefCell::new(HashMap::new());
    /// What an automount below a root mounts once looked into: a
    /// network type, none for still only an automount, or no entry
    /// at all (`None` in the map) for a local disk.
    pub(super) static AFTER_LOOK: RefCell<HashMap<PathBuf, Option<Option<String>>>> = RefCell::new(HashMap::new());
    /// The paths looked at by `answer_of`, in order.
    pub(super) static LOOKED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    /// Paths the test says do not answer a look, as a share gone
    /// away would not: each is taken for no answer at once, on this
    /// thread, with no look.
    pub(crate) static NOT_ANSWERING: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    /// The folders' looks for an add or a remove are queued, not
    /// run in place, while this is set.
    pub(super) static HOLD_CHECKS: RefCell<bool> = const { RefCell::new(false) };
    pub(super) static CHECKS_SENT: RefCell<Vec<(PathBuf, Check)>> = const { RefCell::new(Vec::new()) };
}

/// The looks queued while held, run and landed, as the thread and
/// the event loop would; how many.
#[cfg(unix)]
fn run_checks(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) -> usize {
    let queued: Vec<(PathBuf, Check)> = CHECKS_SENT.with(|c| c.borrow_mut().drain(..).collect());
    for (dir, check) in &queued {
        let checked = checked_within(*check, dir);
        check.land(state, app, worker, dir, checked);
    }
    queued.len()
}

/// The mounts below a root, each with its network type, or none for
/// an automount not mounted yet.
pub(super) type Below = Vec<(PathBuf, Option<String>)>;

/// Whether the test said `path`, or a folder above it, does not
/// answer.
pub(super) fn not_answering(path: &Path) -> bool {
    NOT_ANSWERING.with(|n| n.borrow().iter().any(|p| path.starts_with(p)))
}

/// The reads sent, how many.
fn sent() -> usize {
    SENT.with(|s| s.borrow().len())
}

/// Every read sent run and landed, and those their landing sent;
/// how many.
fn land_all(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) -> usize {
    land_sent(state, app, worker)
}

/// The sidecars a read would bring for the files of `next` the
/// window does not list yet.
fn read_new(st: &State, next: &[PathBuf]) -> HashMap<PathBuf, Held> {
    let fresh: Vec<PathBuf> = next
        .iter()
        .filter(|p| !st.files.contains(p))
        .cloned()
        .collect();
    let none = vec![None; fresh.len()];
    let held = rows::bring(&fresh, &none, st.window.write_sidecars, None);
    fresh.into_iter().zip(held).collect()
}

/// The frame on screen as a read would find it, with the index at
/// `db` to follow it by.
fn seen_now(st: &State, db: Option<&Path>) -> OnScreen {
    st.current
        .and_then(|c| {
            let path = st.path_at(c)?.clone();
            let id = st.views.index_ids.get(st.file_at(c)?).copied().flatten();
            let lib = db.and_then(|db| greycard_library::Library::open_read_only(db).ok());
            Some(OnScreen::look(path, id, lib.as_ref()))
        })
        .unwrap_or_default()
}

/// `merge` as a landed read would call it: the new files' sidecars
/// and the frame on screen read first.
fn merge_read(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Worker,
    next: Vec<PathBuf>,
    db: Option<&Path>,
) -> Option<usize> {
    let (read, seen) = {
        let st = state.borrow();
        (read_new(&st, &next), seen_now(&st, db))
    };
    let brought = Brought {
        read,
        seen,
        rows: None,
    };
    merge(&mut state.borrow_mut(), app, worker, next, brought).0
}

/// A folder of this test's own, canonical, as the index keys
/// folders (macOS's temporary directory is behind a link).
fn scratch(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "greycard-ui-roots-{what}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dunce::canonicalize(&dir).unwrap()
}

fn frames(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let p = dir.join(n);
            write_frame(&p, [&R5, &R6, &A7][i % 3], i as u16);
            p
        })
        .collect()
}

/// A root of three frames in the index, and a window on it with
/// the library open: what the loading bar's tests start from.
fn three_under_a_root(
    what: &str,
) -> (
    PathBuf,
    greycard_library::Library,
    App,
    Rc<RefCell<State>>,
    Rc<Worker>,
) {
    let dir = scratch(what);
    let a = dir.join("a");
    std::fs::create_dir_all(&a).unwrap();
    frames(&a, &["x.tif", "y.tif", "z.tif"]);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        // By name over the folders: the frames are told apart by their
        // place in the list this built, in that order.
        st.views.sort = crate::sort::Kept::by_name();
        st.views.library.roots.add(&a).unwrap();
    }
    (dir, writer, app, state, worker)
}

/// The count a view's read keeps, as the pool would bump it.
fn read_so_far(state: &Rc<RefCell<State>>, n: usize) {
    let st = state.borrow();
    let p = st
        .views
        .library
        .progress
        .as_ref()
        .expect("a view's read is out");
    p.read.store(n, Ordering::Relaxed);
}

/// The window's timer run on, as the event loop would.
fn tick_on(by: Duration) {
    i_slint_backend_testing::mock_elapsed_time(by);
    slint::platform::update_timers_and_animations();
}

/// How many of this test's frames had their sidecars read from disk.
fn disk_reads_under(dir: &Path) -> usize {
    rows::DISK_READS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|p| p.starts_with(dir))
        .count()
}

/// The three frames of `three_under_a_root` opened as a folder's
/// files, on a window with no index path: every sidecar read from
/// disk, which is what the bar counts.
fn open_three(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path) {
    let a = dir.join("a");
    let files = vec![a.join("x.tif"), a.join("y.tif"), a.join("z.tif")];
    open_listing(state, app, worker, Listing::Files(files), Select::Row(0));
}

/// A root of three frames with sidecars worth looking at, indexed,
/// on a window with the library open, sidecars on: x rated and
/// picked, y developed with a quarter turn and a keyword, z bare.
#[allow(clippy::type_complexity)]
fn rated_under_a_root(
    what: &str,
) -> (
    PathBuf,
    Vec<PathBuf>,
    greycard_library::Library,
    App,
    Rc<RefCell<State>>,
    Rc<Worker>,
) {
    let dir = scratch(what);
    let a = dir.join("a");
    std::fs::create_dir_all(&a).unwrap();
    let files = frames(&a, &["x.tif", "y.tif", "z.tif"]);
    let fresh = || Sidecar {
        line: greycard_edit::Line::of(greycard_edit::Edit::for_picture()),
        ..Sidecar::default()
    };
    let mut s = fresh();
    s.meta.rating = 3;
    s.meta.flag = meta::Flag::Pick;
    s.save(&files[0]).unwrap();
    let mut s = fresh();
    let mut e = greycard_edit::Edit::for_picture();
    e.geometry.turns = 1;
    s.record(e);
    s.meta.set_keywords(vec!["Harbor".into()]);
    s.save(&files[1]).unwrap();
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        // By name over the folders: the frames are told apart by their
        // place in the list this built, in that order.
        st.views.sort = crate::sort::Kept::by_name();
        st.views.index_path = Some(db);
        st.window.write_sidecars = true;
        st.views.library.roots.add(&a).unwrap();
    }
    (dir, files, writer, app, state, worker)
}

/// A root with two folders, `d` (two frames) and `e` (one), indexed,
/// and the all-roots view of it landed: what the review's repros
/// start from.
struct Shoot {
    dir: PathBuf,
    d: PathBuf,
    e: PathBuf,
    writer: greycard_library::Library,
    app: App,
    state: Rc<RefCell<State>>,
    worker: Rc<Worker>,
}

impl Shoot {
    fn new(what: &str) -> Shoot {
        let dir = scratch(what);
        let root = dir.join("root");
        let (d, e) = (root.join("d"), root.join("e"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::create_dir_all(&e).unwrap();
        frames(&d, &["d1.tif", "d2.tif"]);
        frames(&e, &["e1.tif"]);
        let db = dir.join("index").join("library.sqlite");
        let mut writer = greycard_library::Library::open(&db).unwrap();
        writer.index_tree(&root, &mut |_| {}).unwrap();
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            // By name over the folders: the frames are told apart by their
            // place in the list this built, in that order.
            st.views.sort = crate::sort::Kept::by_name();
            st.views.index_path = Some(db.clone());
            st.views.library.roots.add(&root).unwrap();
        }
        open_view(&state, &app, &worker, View::Roots(None));
        land_all(&state, &app, &worker);
        assert_eq!(state.borrow().files.len(), 3);
        Shoot {
            dir,
            d,
            e,
            writer,
            app,
            state,
            worker,
        }
    }

    /// A frame copied into `e` and its pass said: a read goes out,
    /// and is handed back unrun.
    fn copy_into_e(&mut self, name: &str) -> Look {
        frames(&self.e, &[name]);
        let report = self.writer.index_folder(&self.e, &mut |_| {}).unwrap();
        folders_passed(&mut self.state.borrow_mut(), &self.e);
        assert!(background_done(
            &self.state,
            &self.app,
            &self.e,
            &report,
            false,
            false
        ));
        SENT.with(|s| s.borrow_mut().pop()).unwrap()
    }

    // Only the tests that lock a folder count d's frames, and
    // those run on unix alone.
    #[cfg(unix)]
    fn in_d(&self) -> usize {
        let st = self.state.borrow();
        st.files.iter().filter(|f| f.starts_with(&self.d)).count()
    }

    fn done(self) {
        self.state.borrow_mut().views.index_reader = None;
        drop(self.state);
        drop(self.writer);
        crate::testing::remove_dir_retry(&self.dir);
    }
}
