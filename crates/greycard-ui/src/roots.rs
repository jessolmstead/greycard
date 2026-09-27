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
//! unplugged) is offline: no pass, no watch, its chip says so, and
//! the view leaves its files out rather than show them as gone.
//!
//! Which roots are there, which are on a network mount, and the
//! watcher on the rest are all found off the window's thread
//! ([`Plan::build`]), and the watcher is put in place when it is
//! ready, unless the roots have changed since it was asked for. A root
//! on a network mount is not watched, since inotify does not see what
//! the server does; a pass goes over it every so often instead
//! ([`Poll`]).
//!
//! The all-roots view is the index's list of the files under the
//! roots, by folder then name, with the filter bar over it as over a
//! folder. It can be the whole library, so its sidecars are read on
//! the rayon pool rather than the window's thread: the window keeps
//! the folder it has until the new list is ready, and says it is
//! reading meanwhile.
//!
//! A list already open is kept up with the disk rather than opened
//! again: a pass that finds a file added, gone or moved has the list
//! read again off the window's thread (the roots and folders looked
//! at, the new files' sidecars read, the frame on screen looked for;
//! one read out at a time, the passes said meanwhile folded into one
//! more), and merged on the window's thread into what it holds, which
//! asks the disk nothing. Each frame keeps its sidecar, its picture
//! and its place in the selection by its path, and every frame still
//! without a picture is asked for one again. The frame on screen is followed to where the index says it
//! went, and never to a copy of it.
//!
//! A root can have a name of the user's, given from its chip's menu
//! (right-click, Rename...), and shown wherever the
//! folder's own name would be. It is a label and nothing more: the
//! root is its path everywhere, and naming it moves nothing.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use greycard_library::Roots;
use greycard_library::roots::Added;

use crate::panel::browser::{load_sidecars_parallel, open_loaded, rebuild_browser};
use crate::panel::cull::drop_placeholder;
use crate::*;

/// What the browser lists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum View {
    /// A folder, or files the desktop asked to open.
    #[default]
    Folder,
    /// Every file under the roots, or under one of them.
    Roots(Option<PathBuf>),
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
    /// browser is about to be replaced, and is not merged into.
    pub(crate) loading: bool,
    pub(crate) loading_since: Option<Instant>,
    /// How far the view's read has got, bumped on the pool; read by
    /// `loading_timer` to fill the bar.
    pub(crate) progress: Option<Arc<Progress>>,
    pub(crate) loading_timer: slint::Timer,
    /// The count as the timer last saw it, and when it last moved: a
    /// read past [`READ_GIVES_UP`] whose count has stood still for
    /// [`LOADING_STALLED`] is given up on by the timer.
    pub(crate) progress_seen: usize,
    pub(crate) progress_moved: Option<Instant>,
    /// The list is being read again off the window's thread; a
    /// refresh asked for meanwhile is done once when it lands
    /// (`stale`).
    pub(crate) merging: bool,
    pub(crate) merge_since: Option<Instant>,
    /// The read out's number: one given up on is dropped when it lands.
    pub(crate) merge_token: u64,
    pub(crate) stale: bool,
    /// The roots found offline when last looked at, off the window's
    /// thread (or on launch): what their chips say.
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
}

/// Whether a root is there to be read: a drive unplugged takes its
/// mount point with it, or leaves it empty and unreadable.
pub(crate) fn online(root: &Path) -> bool {
    std::fs::read_dir(root).is_ok()
}

/// Start the roots on launch: the launch pass asked for, the open
/// folder's root first, and the watcher on them. A root that is not
/// there is left out of both; a watcher that cannot start is said and
/// left, the launch pass the fallback. A batch run (an export, a
/// capture) starts neither, unless it is a capture of the view, which
/// gets the pass and no watcher.
///
/// All of it is found off the window's thread ([`Plan::build`]) and
/// asked for when it lands: which roots answer, the open folder's
/// canonical form, and the watcher. A capture of the view does it in
/// place, since it waits for the pass anyway.
pub(crate) fn start(st: &mut State, app: &App) {
    let Some(indexer) = &st.index else {
        return;
    };
    if st.batch && st.library.wanted.is_none() {
        return;
    }
    if st.library.roots.is_empty() {
        return;
    }
    st.library.watch_token += 1;
    let plan = Plan {
        token: st.library.watch_token,
        roots: st.library.roots.clone(),
        launch: Some(
            st.files
                .first()
                .and_then(|f| f.parent())
                .map(Path::to_path_buf),
        ),
        asker: (!st.batch).then(|| indexer.asker()),
    };
    if st.batch {
        build_in_place(st, plan);
    } else {
        // Not when no thread could be had: nothing will land to end it.
        st.library.starting = send_build(app, plan);
    }
}

/// The watcher on the roots as they are now, asked for off the
/// window's thread. The one there is kept until the new one is ready,
/// so the roots it watches are not left unwatched meanwhile, a root
/// just taken out among them: a pass over that one costs a folder's
/// walk at most, and its rows are left out of the view either way.
pub(crate) fn watch(st: &mut State, app: &App) {
    st.library.watch_token += 1;
    let Some(indexer) = &st.index else {
        return;
    };
    let plan = Plan {
        token: st.library.watch_token,
        roots: st.library.roots.clone(),
        launch: None,
        asker: Some(indexer.asker()),
    };
    send_build(app, plan);
}

/// What a watcher's build is asked, taken from the window's state when
/// it is asked for.
pub(crate) struct Plan {
    token: u64,
    roots: Roots,
    /// At launch: the open folder, as the browser has it, whose root is
    /// passed over first. None for a build that asks for no pass.
    launch: Option<Option<PathBuf>>,
    /// The watcher's way to the indexer; none for no watcher.
    asker: Option<crate::library::Asker>,
}

/// The launch's look at the roots, handed over as soon as it is done
/// so the launch pass does not wait for the watcher.
pub(crate) struct Launch {
    /// The build's number: what it saw of the roots is kept only when
    /// no build has been asked for since.
    token: u64,
    /// The roots that answered, in the order to pass over them.
    order: Vec<PathBuf>,
    /// The roots that did not.
    offline: HashSet<PathBuf>,
}

/// A watcher's build done, off the window's thread.
pub(crate) struct Built {
    token: u64,
    /// The roots that did not answer.
    offline: HashSet<PathBuf>,
    /// The roots that answered and are on a network mount, with its
    /// type: not watched.
    remote: Vec<(PathBuf, String)>,
    watcher: Option<greycard_library::Watcher>,
}

impl Plan {
    /// The roots looked at (each on a thread of its own, one that does
    /// not answer in 3 s taken for offline), the launch's order handed
    /// to `launched` at a launch, the ones on a network mount set
    /// aside, and the rest watched. Asks the disk, so never on the
    /// window's thread but for a capture.
    pub(crate) fn build(self, launched: impl FnOnce(Launch)) -> Built {
        let Plan {
            token,
            roots,
            launch,
            asker,
        } = self;
        let list = roots.list().to_vec();
        let offline = offline_of(&list, ROOT_WAIT);
        let online: Vec<PathBuf> = list
            .iter()
            .filter(|r| !offline.contains(*r))
            .cloned()
            .collect();
        if let Some(open) = launch {
            for root in list.iter().filter(|r| offline.contains(*r)) {
                tracing::info!(
                    "library: {} is offline; left as the index has it",
                    root.display()
                );
            }
            let mut order = online.clone();
            if let Some(first) = open
                .and_then(|d| dunce::canonicalize(d).ok())
                .and_then(|d| roots.root_of(&d).map(Path::to_path_buf))
                .filter(|r| order.contains(r))
            {
                order.retain(|r| *r != first);
                order.insert(0, first);
            }
            launched(Launch {
                token,
                order,
                offline: offline.clone(),
            });
        }
        let (mut remote, mut local) = (Vec::new(), Vec::new());
        for root in online {
            match remote_fs(&root) {
                Some(fs) => {
                    if WATCH_REMOTE {
                        local.push(root.clone());
                    }
                    remote.push((root, fs));
                }
                None => local.push(root),
            }
        }
        let watcher = match asker {
            Some(asker) if !local.is_empty() => {
                let started = Instant::now();
                let (watcher, failed) = greycard_library::Watcher::start(
                    &local,
                    greycard_library::roots::QUIET,
                    greycard_library::roots::LONGEST,
                    move |changes| {
                        tracing::debug!("watch: {changes:?}");
                        asker.changes(changes);
                    },
                );
                for (path, why) in &failed {
                    tracing::warn!(
                        "library: not watching {} ({why}); it is brought up to date when the editor starts",
                        path.display()
                    );
                }
                if let Some(w) = &watcher {
                    tracing::info!(
                        "library: watching {} root(s), set up in {:.0} ms",
                        w.watched.len(),
                        started.elapsed().as_secs_f64() * 1e3
                    );
                }
                watcher
            }
            _ => None,
        };
        Built {
            token,
            offline,
            remote,
            watcher,
        }
    }
}

/// Whether a root on a network mount is watched all the same. On
/// Windows it is: ReadDirectoryChangesW over SMB is one call a root,
/// not one a folder, and the server sends its own changes back
/// (CHANGE_NOTIFY). inotify and FSEvents see only this machine's.
const WATCH_REMOTE: bool = cfg!(windows);

/// The network filesystem a root is on, if it is on one.
#[cfg(not(test))]
fn remote_fs(root: &Path) -> Option<String> {
    crate::mounts::remote(root)
}

/// In a test, a root is on the mount the test says, so a temporary
/// directory on NFS cannot change what a test sees.
#[cfg(test)]
fn remote_fs(root: &Path) -> Option<String> {
    tests::REMOTE
        .with(|r| r.borrow().get(root).cloned())
        .unwrap_or_else(|| panic!("the test says nothing of what {} is on", root.display()))
}

/// A build done in place, on the window's thread: for a capture, and
/// for the tests.
fn build_in_place(st: &mut State, plan: Plan) -> bool {
    let mut launch = None;
    let built = plan.build(|l| launch = Some(l));
    if let Some(launch) = launch {
        take_launch(st, launch);
    }
    take_built(st, built)
}

/// The launch's look landed, on the window's thread: the roots that
/// did not answer said offline, and the launch pass asked for over the
/// roots still in the list.
fn take_launch(st: &mut State, launch: Launch) {
    let Launch {
        token,
        mut order,
        offline,
    } = launch;
    st.library.starting = false;
    // Only from the build asked for last: a newer one's look at the
    // roots is not put back by an older one's.
    if token == st.library.watch_token {
        st.library.offline = offline;
    }
    // A root taken out meanwhile is not passed over; one added
    // meanwhile asked for its own pass.
    order.retain(|r| st.library.roots.list().contains(r));
    if !order.is_empty()
        && let Some(indexer) = &st.index
    {
        tracing::info!(
            "library: {} root(s), a pass over each on the indexer's thread",
            order.len()
        );
        st.library.launch_left = order.len();
        indexer.roots(order);
    }
}

/// A watcher's build landed, on the window's thread: the watcher, the
/// offline roots and the network ones taken, when no build has been
/// asked for since. False when it was dropped for that.
pub(crate) fn take_built(st: &mut State, built: Built) -> bool {
    let Built {
        token,
        offline,
        remote,
        watcher,
    } = built;
    if token != st.library.watch_token {
        tracing::debug!("library: a watcher built for roots that have changed since, dropped");
        return false;
    }
    st.library.offline = offline;
    for (root, fs) in &remote {
        if !st.library.said_remote.insert(root.clone()) {
            continue;
        }
        if WATCH_REMOTE {
            tracing::info!(
                "library: {} is on a network drive ({fs}); watched, since Windows forwards the server's changes",
                root.display()
            );
        } else {
            tracing::info!(
                "library: not watching {}: it is on {fs}, a network filesystem, where a watch sees only this machine's changes; it is passed over at launch{}",
                root.display(),
                match st.library.poll_every {
                    Some(every) => format!(" and every {} min", every.as_secs().div_ceil(60)),
                    None => String::new(),
                }
            );
        }
    }
    st.library.remote = remote;
    st.library.watcher = watcher;
    restart_poll(st);
    true
}

/// The timer over the roots on a network mount, as `library.remote`
/// has them now: none when there are none, when it is off, on a batch
/// run, and on Windows, where they are watched.
fn restart_poll(st: &mut State) {
    let far: Vec<PathBuf> = st.library.remote.iter().map(|(r, _)| r.clone()).collect();
    st.library.poll = match (st.library.poll_every, &st.index) {
        (Some(every), Some(indexer)) if !far.is_empty() && !st.batch && !WATCH_REMOTE => {
            let asker = indexer.asker();
            Poll::start(far, every, move |roots| asker.poll(roots))
        }
        _ => None,
    };
}

/// Build a watcher off the window's thread, and take it on the
/// window's thread when it is done. The thread is left to itself: the
/// editor does not wait for it on the way out. False when no thread
/// could be had.
#[cfg(not(test))]
fn send_build(app: &App, plan: Plan) -> bool {
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard roots watch".into())
        .spawn(move || {
            // Each landing, then the roots' row shown again.
            let on_window = |f: Box<dyn FnOnce(&mut State) + Send>| {
                let app_weak = app_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let (Some(app), Some(state)) = (
                        app_weak.upgrade(),
                        crate::STATE.with(|s| s.borrow().clone()),
                    ) else {
                        return;
                    };
                    let mut st = state.borrow_mut();
                    f(&mut st);
                    show(&st, &app);
                });
            };
            let built = plan.build(|launch| on_window(Box::new(move |st| take_launch(st, launch))));
            on_window(Box::new(move |st| {
                take_built(st, built);
            }));
        });
    match spawned {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!("library: no thread to watch the roots on: {e}");
            false
        }
    }
}

/// In a test a build is queued, and the test runs and takes it: there
/// is no event loop to land it on.
#[cfg(test)]
fn send_build(_app: &App, plan: Plan) -> bool {
    tests::BUILDS.with(|b| b.borrow_mut().push(plan));
    true
}

/// A pass over the roots no watcher can see (those on a network mount),
/// every so often, on a thread of its own, for as long as it is held.
/// A root that does not answer is left for the next time, and the
/// indexer skips a tick for a root whose pass is still waiting or under
/// way ([`crate::library::Asker::poll`]).
pub(crate) struct Poll {
    _stop: std::sync::mpsc::Sender<()>,
}

impl Poll {
    pub(crate) fn start(
        roots: Vec<PathBuf>,
        every: Duration,
        pass: impl Fn(Vec<PathBuf>) + Send + 'static,
    ) -> Option<Poll> {
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let wait = move || {
            matches!(
                stopped.recv_timeout(every),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            )
        };
        let spawned = std::thread::Builder::new()
            .name("greycard roots poll".into())
            .spawn(move || {
                poll(&roots, wait, |r| offline_of(r, ROOT_WAIT), &pass);
            });
        match spawned {
            Ok(_) => Some(Poll { _stop: stop }),
            Err(e) => {
                tracing::warn!("library: no thread to pass over the network roots on: {e}");
                None
            }
        }
    }
}

/// The timer's body: at each `wait` that says it is time, a tree pass
/// over each of `roots` that answers; until `wait` says to stop. How
/// many times it asked.
fn poll(
    roots: &[PathBuf],
    mut wait: impl FnMut() -> bool,
    offline: impl Fn(&[PathBuf]) -> HashSet<PathBuf>,
    pass: &dyn Fn(Vec<PathBuf>),
) -> usize {
    let mut asked = 0;
    while wait() {
        let away = offline(roots);
        let changes: Vec<PathBuf> = roots
            .iter()
            .filter(|r| !away.contains(*r))
            .cloned()
            .collect();
        tracing::debug!(
            "library: {} network root(s) passed over by the timer",
            changes.len()
        );
        if !changes.is_empty() {
            pass(changes);
            asked += 1;
        }
    }
    asked
}

/// Each root's count, as the index holds it now.
pub(crate) fn recount(st: &mut State) {
    let Some(lib) = &st.index_reader else {
        return;
    };
    let counts: Vec<usize> = st
        .library
        .roots
        .list()
        .iter()
        .map(|r| lib.count_under_canonical(r).unwrap_or(0))
        .collect();
    st.library.counts = counts;
}

/// The roots' row in the grid's header.
pub(crate) fn show(st: &State, app: &App) {
    let roots = st.library.roots.list();
    let on = match &st.view {
        View::Roots(r) => Some(r.clone()),
        View::Folder => None,
    };
    let chips: Vec<RootChip> = roots
        .iter()
        .enumerate()
        .map(|(i, r)| RootChip {
            name: root_name(&st.library.roots, r).into(),
            path: r.to_string_lossy().into_owned().into(),
            count: st.library.counts.get(i).copied().unwrap_or(0) as i32,
            on: on.as_ref() == Some(&Some(r.clone())),
            offline: st.library.offline.contains(r),
        })
        .collect();
    app.set_library_roots(ModelRc::new(VecModel::from(chips)));
    app.set_library_all_on(on == Some(None));
    app.set_library_all_count(st.library.counts.iter().sum::<usize>() as i32);
    app.set_library_can_add_open(open_folder_to_add(st).is_some());
    app.set_library_note(
        if roots.is_empty() {
            "No folders in the library yet"
        } else {
            ""
        }
        .into(),
    );
}

/// A root as its chip names it: the name the user gave it, else the
/// folder's own name, or the whole path for a disk's root, which has
/// none.
pub(crate) fn root_name(roots: &Roots, root: &Path) -> String {
    roots.label(root)
}

/// A root as the status line says it: its path, after its name when
/// it has one, so a root named alike to another is still told apart.
fn said(roots: &Roots, root: &Path) -> String {
    match roots.name(root) {
        Some(name) => format!("{name} ({})", root.display()),
        None => root.display().to_string(),
    }
}

/// The folder's own name, as the root sheet offers it for an empty
/// name.
fn folder_name(dir: &Path) -> String {
    root_name(&Roots::default(), dir)
}

/// The open folder, when the browser shows one and it is not under a
/// root yet: what "Add this folder" adds.
fn open_folder_to_add(st: &State) -> Option<PathBuf> {
    if st.view != View::Folder {
        return None;
    }
    let dir = st.files.first()?.parent()?;
    // The folder's canonical form as the window has kept it since the
    // folder was opened; asked of the disk only when it has not.
    let dir = match st.library.canonical.get(dir) {
        Some(key) => key.clone(),
        None => dunce::canonicalize(dir).ok()?,
    };
    st.library.roots.root_of(&dir).is_none().then_some(dir)
}

/// Change the roots: through the file as it is on disk now when there
/// is one, so a root another editor added or removed meanwhile is
/// kept, else in memory alone.
fn edit_roots<T>(
    st: &mut State,
    change: impl FnOnce(&mut Roots) -> T,
) -> std::result::Result<T, String> {
    match &st.library.file {
        Some(path) => match Roots::edit(path, change) {
            Ok((roots, out)) => {
                st.library.roots = roots;
                Ok(out)
            }
            Err(e) => Err(format!("{}: {e}", path.display())),
        },
        None => Ok(change(&mut st.library.roots)),
    }
}

/// Add a folder to the roots: kept, watched, passed over, and the
/// view brought up if it lists the roots. It goes by its folder's
/// name until it is given one from its chip's menu.
///
/// The folder is made canonical and looked at off the window's thread
/// (a folder on a share can take a round trip, or never answer), and
/// added when that is done.
pub(crate) fn add(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path) {
    app.set_status(format!("adding {}...", dir.display()).into());
    send_check(state, app, worker, dir.to_path_buf());
}

/// Look at a folder to add off the window's thread, and add it on the
/// window's thread when that is done.
#[cfg(not(test))]
fn send_check(_state: &Rc<RefCell<State>>, app: &App, _worker: &Rc<Worker>, dir: PathBuf) {
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard roots add".into())
        .spawn(move || {
            let checked = Roots::checked(&dir);
            let _ = slint::invoke_from_event_loop(move || {
                let (Some(app), Some(state), Some(worker)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                    crate::WORKER.with(|w| w.borrow().clone()),
                ) else {
                    return;
                };
                add_checked(&state, &app, &worker, checked);
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("library: {e}");
        app.set_status(format!("not added: {e}").into());
    }
}

/// In a test the folder is looked at in place: there is no event loop
/// to land it on.
#[cfg(test)]
fn send_check(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: PathBuf) {
    add_checked(state, app, worker, Roots::checked(&dir));
}

/// A folder to add, looked at: kept, watched, passed over.
fn add_checked(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    checked: std::result::Result<PathBuf, greycard_library::roots::RootError>,
) {
    let dir = match checked {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!("library: {e}");
            app.set_status(format!("not added: {e}").into());
            return;
        }
    };
    let mut st = state.borrow_mut();
    let added = match edit_roots(&mut st, |r| r.add_checked(dir)) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("library: roots not saved: {e}");
            app.set_status(format!("not added: {e}").into());
            return;
        }
    };
    let new = match added {
        Ok(Added::New(root)) => {
            app.set_status(
                format!(
                    "{} is in the library now; right-click its chip to name it",
                    said(&st.library.roots, &root)
                )
                .into(),
            );
            root
        }
        Ok(Added::Absorbed(root, under)) => {
            app.set_status(
                format!(
                    "{} is in the library now, in place of {} folder{} under it",
                    said(&st.library.roots, &root),
                    under.len(),
                    if under.len() == 1 { "" } else { "s" }
                )
                .into(),
            );
            root
        }
        Ok(Added::Covered(root)) => {
            app.set_status(
                format!(
                    "already in the library, under {}",
                    said(&st.library.roots, &root)
                )
                .into(),
            );
            return;
        }
        Err(e) => {
            tracing::warn!("library: {e}");
            app.set_status(format!("not added: {e}").into());
            return;
        }
    };
    tracing::info!("library: added {}", new.display());
    if let Some(indexer) = &st.index {
        indexer.roots(vec![new]);
    }
    watch(&mut st, app);
    recount(&mut st);
    show(&st, app);
    let refresh = matches!(st.view, View::Roots(None));
    drop(st);
    if refresh {
        refresh_view(state, app, worker);
    }
}

/// Take a folder out of the roots. The folder and its files are not
/// touched, nor are their rows in the index; the all-roots view no
/// longer lists them.
pub(crate) fn remove(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path) {
    let mut st = state.borrow_mut();
    let was = said(&st.library.roots, dir);
    match edit_roots(&mut st, |r| r.remove(dir)) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            tracing::warn!("library: roots not saved: {e}");
            app.set_status(format!("not removed: {e}").into());
            return;
        }
    }
    tracing::info!(
        "library: removed {}; nothing on disk touched",
        dir.display()
    );
    app.set_status(format!("{was} is out of the library; its files are where they were").into());
    // The watcher there is kept until the one on the rest is ready, so
    // the other roots are not left unwatched while it is built (on a
    // large tree, many seconds). The timer is started again at once
    // without the root, which asks the disk nothing.
    st.library.remote.retain(|(r, _)| r != dir);
    restart_poll(&mut st);
    watch(&mut st, app);
    recount(&mut st);
    let view = st.view.clone();
    let refresh = match &view {
        View::Roots(Some(r)) if r == dir => {
            st.view = View::Roots(None);
            true
        }
        View::Roots(_) => true,
        View::Folder => false,
    };
    show(&st, app);
    drop(st);
    if refresh {
        refresh_view(state, app, worker);
    }
}

/// Give a root a name, or with an empty one take its name away. The
/// root is still its path, and nothing on disk is touched; only its
/// chip and what the status line calls it change.
pub(crate) fn rename(state: &Rc<RefCell<State>>, app: &App, dir: &Path, name: &str) {
    let mut st = state.borrow_mut();
    let before = st.library.roots.name(dir).map(str::to_owned);
    match edit_roots(&mut st, |r| r.set_name(dir, name)) {
        Ok(true) => {}
        Ok(false) => {
            app.set_status(format!("{} is not in the library", dir.display()).into());
            return;
        }
        Err(e) => {
            tracing::warn!("library: roots not saved: {e}");
            app.set_status(format!("not renamed: {e}").into());
            return;
        }
    }
    let after = st.library.roots.name(dir).map(str::to_owned);
    if after != before {
        tracing::info!("library: {} named {:?}", dir.display(), after);
    }
    app.set_status(
        match &after {
            Some(name) => format!(
                "{} is called {name} in the library; the folder is as it was",
                dir.display()
            ),
            None => format!("{} goes by its folder's name", dir.display()),
        }
        .into(),
    );
    show(&st, app);
}

/// The root sheet opened over a root, with the name it has.
fn open_sheet(st: &State, app: &App, dir: &Path) {
    app.set_root_sheet_path(dir.to_string_lossy().into_owned().into());
    app.set_root_sheet_folder(folder_name(dir).into());
    app.set_root_sheet_name(st.library.roots.name(dir).unwrap_or("").into());
    app.set_root_sheet_open(true);
}

/// The roots a view lists, as the roots file has them. Which of them
/// are there is looked at off the window's thread, by [`Look::run`].
fn roots_of(st: &State, view: &View) -> Vec<PathBuf> {
    match view {
        View::Folder => Vec::new(),
        View::Roots(None) => st.library.roots.list().to_vec(),
        View::Roots(Some(r)) => vec![r.clone()],
    }
}

/// Every root, and the view's own if it is not among them: each is
/// looked at off the window's thread, for its chip.
fn every_root(st: &State, view: &View) -> Vec<PathBuf> {
    let mut all = st.library.roots.list().to_vec();
    for r in roots_of(st, view) {
        if !all.contains(&r) {
            all.push(r);
        }
    }
    all
}

/// How long a root has to answer whether it is there before a read
/// takes it for offline: a hard-mounted network share gone away does
/// not answer at all.
const ROOT_WAIT: Duration = Duration::from_secs(3);

/// How long a read may be out before the next refresh gives up on it
/// and sends another; what it brings after that is dropped.
const READ_GIVES_UP: Duration = Duration::from_secs(60);

/// How long a view's read runs before its bar is shown: a root on a
/// local disk is in before this, and never flashes one.
pub(crate) const LOADING_SHOWN_AFTER: Duration = Duration::from_millis(200);

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
    /// A view opened: its list replaces the browser's once read.
    Open {
        view: View,
        asked: Instant,
        last: String,
    },
    /// The browser's list read again, and merged into what it holds.
    Merge,
}

/// Where the list comes from.
enum Source {
    /// A folder's files, listed off the window's thread; the folder is
    /// made canonical there by every read, so a link retargeted
    /// meanwhile is seen.
    Folder(PathBuf),
    /// The index's rows under the view's roots, as it lists them; those
    /// under a root offline or in a folder that cannot be read are left
    /// out off the window's thread.
    Roots {
        roots: Vec<PathBuf>,
        files: Vec<PathBuf>,
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
    pub(crate) read: HashMap<PathBuf, (Sidecar, bool)>,
    pub(crate) seen: OnScreen,
    /// Each listed file's row, by path; none when the read had no index
    /// to ask, and the merge then asks it.
    pub(crate) ids: Option<HashMap<PathBuf, Option<i64>>>,
}

/// A read done: the list as the disk has it now, and the sidecars of
/// the files new to it.
pub(crate) struct Found {
    purpose: Purpose,
    generation: u64,
    token: u64,
    epoch: u64,
    /// None when the folder could not be listed.
    files: Option<Vec<PathBuf>>,
    /// The files new to the list, in the list's order, and their
    /// sidecars.
    fresh: Vec<PathBuf>,
    sidecars: Vec<Sidecar>,
    seed: Vec<bool>,
    ids: Option<HashMap<PathBuf, Option<i64>>>,
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
        let mut offline = HashSet::new();
        let mut asked: Vec<(PathBuf, Answer)> = Vec::new();
        {
            let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
            for root in roots {
                if let Some(answer) = out.get(root) {
                    let answered = *answer.0.lock().unwrap_or_else(|e| e.into_inner());
                    if answered.is_none() {
                        offline.insert(root.clone());
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
                    // No thread to be had: offline for this read.
                    offline.insert(root.clone());
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
            match *there {
                Some(true) => {}
                Some(false) => {
                    offline.insert(root);
                }
                None => {
                    tracing::warn!(
                        "library: {} did not answer in {} s; offline until it does",
                        root.display(),
                        wait.as_secs()
                    );
                    offline.insert(root);
                }
            }
        }
        offline
    }
}

/// The editor's looks at the roots.
static CHECKS: LazyLock<Checks> = LazyLock::new(|| Checks::new(online));

/// Which roots are offline, by [`Checks::offline`].
fn offline_of(roots: &[PathBuf], wait: Duration) -> HashSet<PathBuf> {
    CHECKS.offline(roots, wait)
}

impl Look {
    /// The read, on a thread that is not the window's: the roots and
    /// the folders not known yet looked at, the folder listed, the
    /// list's rows asked of the index, the new files' sidecars read on
    /// the pool, and the frame on screen looked for.
    pub(crate) fn run(self) -> Found {
        let started = Instant::now();
        let offline = (!self.roots.is_empty()).then(|| offline_of(&self.roots, ROOT_WAIT));
        let mut readable = Vec::new();
        let mut unreadable = Vec::new();
        let mut canonical = Vec::new();
        let mut looked = 0;
        let files = match self.source {
            Source::Folder(dir) => {
                let key = greycard_library::key_folder(&dir);
                canonical.push((dir.clone(), key));
                files::list_files(&dir).ok()
            }
            Source::Roots { roots, files } => {
                let away: Vec<&PathBuf> = roots
                    .iter()
                    .filter(|r| offline.as_ref().is_some_and(|o| o.contains(*r)))
                    .collect();
                let mut seen: HashMap<PathBuf, bool> = HashMap::new();
                let kept: Vec<PathBuf> = files
                    .into_iter()
                    .filter(|f| !away.iter().any(|r| f.starts_with(r)))
                    .filter(|f| {
                        let Some(dir) = f.parent() else {
                            return false;
                        };
                        if self.known.contains(dir) {
                            return true;
                        }
                        if let Some(&can) = seen.get(dir) {
                            return can;
                        }
                        looked += 1;
                        let can = std::fs::read_dir(dir).is_ok();
                        seen.insert(dir.to_path_buf(), can);
                        if can {
                            readable.push(dir.to_path_buf());
                        } else {
                            unreadable.push(dir.to_path_buf());
                        }
                        can
                    })
                    .collect();
                // The index's own paths: their folders are canonical.
                let mut folders: HashSet<&Path> = HashSet::new();
                for f in &kept {
                    if let Some(dir) = f.parent()
                        && folders.insert(dir)
                    {
                        canonical.push((dir.to_path_buf(), dir.to_path_buf()));
                    }
                }
                Some(kept)
            }
        };
        let lib = self
            .index
            .as_deref()
            .and_then(|p| greycard_library::Library::open_read_only(p).ok());
        let ids = match (&lib, &files) {
            (Some(lib), Some(files)) => {
                let known: HashMap<&Path, &Path> = canonical
                    .iter()
                    .map(|(d, c)| (d.as_path(), c.as_path()))
                    .collect();
                lib.ids_of_with(files, &mut |dir| {
                    known
                        .get(dir)
                        .map(|c| c.to_path_buf())
                        .unwrap_or_else(|| greycard_library::key_folder(dir))
                })
                .ok()
                .map(|ids| files.iter().cloned().zip(ids).collect())
            }
            _ => None,
        };
        let fresh: Vec<PathBuf> = files
            .iter()
            .flatten()
            .filter(|f| !self.have.contains(*f))
            .cloned()
            .collect();
        if let Some(p) = &self.progress {
            p.total.store(fresh.len(), Ordering::Relaxed);
        }
        let (sidecars, seed) = load_sidecars_parallel(
            &fresh,
            self.write,
            self.progress.as_deref().map(|p| &p.read),
        );
        // The frame on screen under a root this read found offline is not
        // looked at: a stat under a share that does not answer does not
        // come back. It is taken as there, and stays listed.
        let on_screen = self
            .on_screen
            .filter(|(path, _)| {
                !offline
                    .as_ref()
                    .is_some_and(|o| o.iter().any(|r| path.starts_with(r)))
            })
            .map(|(path, id)| OnScreen::look(path, id, lib.as_ref()))
            .unwrap_or_default();
        let seconds = started.elapsed().as_secs_f64();
        tracing::debug!(
            "library: the list read off the window's thread in {:.1} ms: {} files, {} new, \
             {} folder(s) and {} root(s) looked at",
            seconds * 1e3,
            files.as_ref().map_or(0, Vec::len),
            fresh.len(),
            looked,
            self.roots.len()
        );
        Found {
            purpose: self.purpose,
            generation: self.generation,
            token: self.token,
            epoch: self.epoch,
            files,
            fresh,
            sidecars,
            seed,
            ids,
            offline,
            readable,
            unreadable,
            canonical,
            on_screen,
            seconds,
        }
    }
}

/// Send a read off the window's thread; what it finds is landed on the
/// window's thread when it is done. False when it could not be sent.
#[cfg(not(test))]
fn send_off(app: &App, look: Look) -> bool {
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard roots read".into())
        .spawn(move || {
            let found = look.run();
            let _ = slint::invoke_from_event_loop(move || {
                let (Some(app), Some(state), Some(worker)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                    crate::WORKER.with(|w| w.borrow().clone()),
                ) else {
                    return;
                };
                land(&state, &app, &worker, found);
            });
        });
    match spawned {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!("library: {e}");
            false
        }
    }
}

/// In a test a read is queued, and the test runs and lands it
/// (`tests::land_all`): there is no event loop to land it on.
#[cfg(test)]
fn send_off(_app: &App, look: Look) -> bool {
    tests::SENT.with(|s| s.borrow_mut().push(look));
    true
}

/// A read done, on the window's thread: what it saw of the roots and
/// the folders kept, and the list opened or merged. Nothing here asks
/// the disk.
fn land(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, found: Found) {
    let Found {
        purpose,
        generation,
        token,
        epoch,
        files,
        fresh,
        sidecars,
        seed,
        ids,
        offline,
        readable,
        unreadable,
        canonical,
        on_screen,
        seconds,
    } = found;
    {
        let st = state.borrow();
        // A read given up on brings nothing: another is out for it.
        let given_up = match purpose {
            Purpose::Merge => token != st.library.merge_token,
            Purpose::Open { .. } => st.view_generation != generation,
        };
        if given_up {
            tracing::debug!("library: a read given up on, or overtaken, came in; dropped");
            return;
        }
    }
    // What the read saw is kept only from a read of the list the window
    // holds now; a merge read overtaken by a view opened since still
    // clears `merging` below.
    if state.borrow().view_generation == generation {
        let mut st = state.borrow_mut();
        st.library.canonical.extend(canonical);
        for dir in &readable {
            st.library.unreadable.remove(dir);
        }
        st.library.unreadable.extend(unreadable);
        // What the read saw of a folder is kept unless a pass has gone
        // over the folder since the read was asked for, or the window
        // no longer remembers the passes back that far.
        if epoch >= st.library.passes_from {
            let lib = &mut st.library;
            for dir in readable {
                let passed = lib
                    .passes
                    .iter()
                    .any(|(e, p)| *e > epoch && dir.starts_with(p));
                if !passed {
                    lib.readable.insert(dir);
                }
            }
        }
        if let Some(offline) = offline
            && st.library.offline != offline
        {
            st.library.offline = offline;
            show(&st, app);
        }
    }
    match purpose {
        Purpose::Open { view, asked, last } => {
            // A later list owns `loading`, and clears it itself.
            if state.borrow().view_generation != generation {
                return;
            }
            let files = files.unwrap_or_default();
            tracing::info!(
                "library: {} frames in the view, their sidecars read in {:.2} s on the pool",
                files.len(),
                seconds
            );
            if files.is_empty() {
                take_empty(state, app, worker, view, asked);
                return;
            }
            let select = (!last.is_empty())
                .then(|| greycard_library::key_path(Path::new(&last)))
                .and_then(|last| files.iter().position(|f| *f == last))
                .unwrap_or(0);
            {
                let mut st = state.borrow_mut();
                st.view = view;
                loading_done(&mut st.library, app);
            }
            open_loaded(state, app, worker, files, sidecars, seed, select);
            let mut st = state.borrow_mut();
            st.library.awaiting = false;
            st.library.painted = Some((asked, "the view asked for"));
            tracing::info!(
                "library: the view in the browser {:.0} ms after it was asked for",
                asked.elapsed().as_secs_f64() * 1e3
            );
            show(&st, app);
        }
        Purpose::Merge => {
            let mut again = {
                let mut st = state.borrow_mut();
                st.library.merging = false;
                st.library.merge_since = None;
                std::mem::take(&mut st.library.stale)
            };
            let ours = state.borrow().view_generation == generation;
            if let (true, Some(files)) = (ours, files) {
                let read: HashMap<PathBuf, (Sidecar, bool)> = fresh
                    .into_iter()
                    .zip(sidecars.into_iter().zip(seed))
                    .collect();
                // A file new to the list since the read was asked for
                // has no sidecar here: read again, rather than merged
                // with a blank one a save would write over its own.
                let whole = {
                    let st = state.borrow();
                    let have: HashSet<&PathBuf> = st.files.iter().collect();
                    files
                        .iter()
                        .all(|f| have.contains(f) || read.contains_key(f))
                };
                if whole {
                    let brought = Brought {
                        read,
                        seen: on_screen,
                        ids,
                    };
                    let next = merge(&mut state.borrow_mut(), app, worker, files, brought);
                    if let Some(row) = next {
                        app.invoke_select(row as i32);
                    }
                } else {
                    again = true;
                }
            }
            if again {
                refresh_view(state, app, worker);
                return;
            }
            // The launch pass done and nothing came of it: said, and a
            // capture waiting on the view ends.
            let empty = {
                let st = state.borrow();
                matches!(st.view, View::Roots(_))
                    && st.files.is_empty()
                    && st.library.launch_left == 0
                    && !st.library.starting
                    && !st.library.loading
            };
            if empty {
                empty_view(&mut state.borrow_mut(), app);
            }
        }
    }
}

/// An empty view taken: nothing indexed yet, most likely, or nothing
/// there that can be read. It fills in as the launch pass reaches the
/// roots. A capture of it waits for that pass when there is one to
/// wait for, and ends when there is none.
fn take_empty(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    view: View,
    asked: Instant,
) {
    let passing = {
        let mut st = state.borrow_mut();
        st.view = view;
        st.library.launch_left > 0 || st.library.starting
    };
    open_loaded(state, app, worker, Vec::new(), Vec::new(), Vec::new(), 0);
    let mut st = state.borrow_mut();
    loading_done(&mut st.library, app);
    st.library.painted = Some((asked, "the view asked for"));
    show(&st, app);
    if passing {
        app.set_status("Nothing indexed under the roots yet; the pass is running".into());
    } else {
        empty_view(&mut st, app);
    }
}

/// Show every file under the roots, or under one: the list from the
/// index; the roots and folders looked at and the sidecars read off
/// the window's thread; and the browser's list replaced once they are
/// in. The window goes on with what it has meanwhile. An open is asked
/// for by hand, so every folder is looked at again.
pub(crate) fn open_view(state: &Rc<RefCell<State>>, app: &App, _worker: &Rc<Worker>, view: View) {
    let mut st = state.borrow_mut();
    let Some(lib) = &st.index_reader else {
        // Asked before the indexer has opened the library: done when
        // it has.
        st.library.wanted = Some(view);
        app.set_status("opening the library index...".into());
        return;
    };
    let asked = Instant::now();
    let roots = roots_of(&st, &view);
    let files = match lib.paths_under_canonical(&roots) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("library: {e}");
            app.set_status(format!("the library index could not be read: {e}").into());
            st.library.awaiting = false;
            loading_done(&mut st.library, app);
            return;
        }
    };
    tracing::info!(
        "library: {} files under {} root(s), listed in {:.1} ms",
        files.len(),
        roots.len(),
        asked.elapsed().as_secs_f64() * 1e3
    );
    st.view_generation += 1;
    // The frame on screen stays on screen when it is in the new list;
    // else the last one open, as a folder's open does.
    let last = if files.is_empty() {
        String::new()
    } else {
        st.current
            .and_then(|c| st.files.get(c))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| settings::Settings::load().last_file)
    };
    st.library.readable.clear();
    st.library.readable_epoch += 1;
    app.set_status(format!("reading {} frames...", files.len()).into());
    let progress = Arc::new(Progress {
        total: AtomicUsize::new(files.len()),
        read: AtomicUsize::new(0),
    });
    let look = Look {
        generation: st.view_generation,
        token: 0,
        epoch: st.library.readable_epoch,
        roots: every_root(&st, &view),
        source: Source::Roots { roots, files },
        known: HashSet::new(),
        have: HashSet::new(),
        write: st.write_sidecars,
        on_screen: None,
        index: None,
        progress: Some(progress.clone()),
        purpose: Purpose::Open { view, asked, last },
    };
    drop(st);
    loading_started(state, app, progress);
    if !send_off(app, look) {
        let mut st = state.borrow_mut();
        st.library.awaiting = false;
        loading_done(&mut st.library, app);
    }
}

/// A view's read sent: `loading` set, and the window's timer started
/// on its count. The bar shows once the read has run past
/// [`LOADING_SHOWN_AFTER`], and goes when it lands or is given up on.
fn loading_started(state: &Rc<RefCell<State>>, app: &App, progress: Arc<Progress>) {
    let mut st = state.borrow_mut();
    let lib = &mut st.library;
    let now = Instant::now();
    lib.loading = true;
    lib.loading_since = Some(now);
    lib.progress = Some(progress);
    lib.progress_seen = 0;
    lib.progress_moved = Some(now);
    app.set_loading_shown(false);
    let (state_weak, app_weak) = (Rc::downgrade(state), app.as_weak());
    lib.loading_timer
        .start(slint::TimerMode::Repeated, LOADING_TICK, move || {
            if let (Some(state), Some(app)) = (state_weak.upgrade(), app_weak.upgrade()) {
                loading_tick(&mut state.borrow_mut(), &app, Instant::now());
            }
        });
}

/// The timer's tick while a view's read is out, at `now`: the bar
/// filled to the sidecars read, once the read has been out long enough
/// to show it. A read past [`READ_GIVES_UP`] whose count has not moved
/// in [`LOADING_STALLED`] is given up on here as `refresh_view` would,
/// since with no report coming nothing else would.
pub(crate) fn loading_tick(st: &mut State, app: &App, now: Instant) {
    let lib = &mut st.library;
    let (true, Some(since), Some(progress)) = (lib.loading, lib.loading_since, &lib.progress)
    else {
        loading_done(lib, app);
        return;
    };
    let total = progress.total.load(Ordering::Relaxed);
    let read = progress.read.load(Ordering::Relaxed).min(total);
    if read != lib.progress_seen {
        lib.progress_seen = read;
        lib.progress_moved = Some(now);
    }
    let out = now.saturating_duration_since(since);
    let still = now.saturating_duration_since(lib.progress_moved.unwrap_or(since));
    if out > READ_GIVES_UP && still > LOADING_STALLED {
        give_up_view(st, app);
        return;
    }
    if out < LOADING_SHOWN_AFTER {
        return;
    }
    let fraction = if total == 0 {
        0.0
    } else {
        read as f32 / total as f32
    };
    app.set_loading_fraction(fraction);
    app.set_loading_line(loading_words(read, total).into());
    if !app.get_loading_shown() {
        tracing::info!(
            "library: the view's bar shown {:.0} ms after it was asked for, {read} of {total} read",
            out.as_secs_f64() * 1e3
        );
        app.set_loading_shown(true);
    }
}

/// The view's read done, dropped or given up on: nothing loading, the
/// timer stopped and the bar gone.
pub(crate) fn loading_done(lib: &mut Library, app: &App) {
    lib.loading = false;
    lib.loading_since = None;
    lib.progress = None;
    lib.progress_moved = None;
    lib.loading_timer.stop();
    app.set_loading_shown(false);
}

/// A view's read given up on: the bar gone, a capture waiting on it let
/// go, and the read dropped by its generation if it ever lands.
fn give_up_view(st: &mut State, app: &App) {
    tracing::warn!("library: the view's read has not come in; given up");
    loading_done(&mut st.library, app);
    st.library.awaiting = false;
    st.view_generation += 1;
    app.set_status("the library's frames did not come in; the list is as it was".into());
}

/// The bar's words: "Reading 11,711 sidecars… 3,400".
fn loading_words(read: usize, total: usize) -> String {
    let word = if total == 1 { "sidecar" } else { "sidecars" };
    format!("Reading {} {word}… {}", grouped(total), grouped(read))
}

/// A count with its thousands set apart by commas.
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The read for the browser's list as it is, from the folder or from
/// the index. None when there is nothing to read it from.
fn ask(st: &State) -> Option<Look> {
    let (source, roots) = match &st.view {
        View::Folder => {
            let dir = st
                .files
                .iter()
                .enumerate()
                .find(|(i, _)| Some(*i) != st.current)
                .map(|(_, f)| f)
                .or(st.files.first())
                .and_then(|f| f.parent())
                .map(Path::to_path_buf)?;
            // A list the desktop handed over, files from here and
            // there, is not a folder's to read again. The frame on
            // screen may be elsewhere, followed there by a move.
            let elsewhere = st
                .files
                .iter()
                .enumerate()
                .any(|(i, f)| Some(i) != st.current && f.parent() != Some(dir.as_path()));
            if elsewhere {
                return None;
            }
            // A folder's view looks at no root: a root gone away (a
            // share that does not answer) is none of its business.
            (Source::Folder(dir), Vec::new())
        }
        View::Roots(_) => {
            let lib = st.index_reader.as_ref()?;
            let roots = roots_of(st, &st.view);
            match lib.paths_under_canonical(&roots) {
                Ok(files) => (Source::Roots { roots, files }, every_root(st, &st.view)),
                Err(e) => {
                    tracing::debug!("library: {e}; the list kept");
                    return None;
                }
            }
        }
    };
    let known = match source {
        Source::Roots { .. } => st.library.readable.clone(),
        Source::Folder(_) => HashSet::new(),
    };
    Some(Look {
        purpose: Purpose::Merge,
        generation: st.view_generation,
        token: st.library.merge_token,
        epoch: st.library.readable_epoch,
        source,
        roots,
        known,
        have: st.files.iter().cloned().collect(),
        write: st.write_sidecars,
        on_screen: st.current.and_then(|c| {
            Some((
                st.files.get(c)?.clone(),
                st.index_ids.get(c).copied().flatten(),
            ))
        }),
        index: st.index_path.clone(),
        progress: None,
    })
}

/// The browser's list read again, from the folder or from the index,
/// and merged into the window's. The read is off the window's thread,
/// and the new files' sidecars with it. One is out at a time: a
/// refresh asked for meanwhile, however many, is done once when it
/// lands (`stale`). A read out longer than [`READ_GIVES_UP`] (a share
/// that stopped answering mid-read) is given up on, and another sent.
/// True when a merge is coming; false when there was nothing to read
/// the list from.
pub(crate) fn refresh_view(state: &Rc<RefCell<State>>, app: &App, _worker: &Rc<Worker>) -> bool {
    let look = {
        let mut st = state.borrow_mut();
        let late = |since: Option<Instant>| since.is_some_and(|t| t.elapsed() > READ_GIVES_UP);
        if st.library.loading {
            if !late(st.library.loading_since) {
                return false;
            }
            give_up_view(&mut st, app);
        }
        if st.library.merging {
            if !late(st.library.merge_since) {
                st.library.stale = true;
                return true;
            }
            tracing::warn!("library: a read of the list has not come in; another sent");
        }
        st.library.merge_token += 1;
        let Some(look) = ask(&st) else {
            st.library.merging = false;
            st.library.merge_since = None;
            return false;
        };
        st.library.merging = true;
        st.library.merge_since = Some(Instant::now());
        st.library.stale = false;
        look
    };
    if !send_off(app, look) {
        let mut st = state.borrow_mut();
        st.library.merging = false;
        st.library.merge_since = None;
        return false;
    }
    true
}

/// A pass has finished under `path`: what the window knew of the
/// folders under it is forgotten, to be looked at again by the next
/// read off the window's thread, and a read out now does not put back
/// what it saw of them.
pub(crate) fn folders_passed(st: &mut State, path: &Path) {
    let lib = &mut st.library;
    lib.readable.retain(|dir| !dir.starts_with(path));
    lib.readable_epoch += 1;
    // A read out now may have seen the folders before the pass did: one
    // more is sent when it lands.
    if lib.merging {
        lib.stale = true;
    }
    lib.passes.push((lib.readable_epoch, path.to_path_buf()));
    if lib.passes.len() > PASSES_KEPT {
        let (dropped, _) = lib.passes.remove(0);
        lib.passes_from = dropped;
    }
}

/// The frame on screen, followed: when its file is not where the
/// window has it, and the index knows where the row it had went — a
/// move under a root, or a folder renamed — the window takes the new
/// path, so its next save lands beside the frame and not beside a
/// file that is gone. Never to a path the window already lists, which
/// would put one file in the list twice and the frame's edit over the
/// other's sidecar. By what `seen` found of the frame off the window's
/// thread. True when it moved.
pub(crate) fn follow_current(st: &mut State, seen: &OnScreen) -> bool {
    let Some(c) = st.current else {
        return false;
    };
    let Some(path) = st.files.get(c) else {
        return false;
    };
    if !seen.gone(path) {
        return false;
    }
    let Some(to) = seen.moved_to.as_ref().filter(|p| !st.files.contains(p)) else {
        return false;
    };
    tracing::info!("{} moved to {}; followed", path.display(), to.display());
    st.files[c] = to.clone();
    true
}

/// The frames still without a picture, to be asked for: after a list
/// is renumbered, every job queued names a file by a number that now
/// means another, and is thrown away when it arrives.
pub(crate) fn owed_thumbnails(st: &State) -> Vec<(usize, PathBuf)> {
    st.files
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            st.thumb_base.get(*i).is_some_and(Option::is_none)
                && !st.thumb_failed.get(*i).copied().unwrap_or(false)
        })
        .map(|(i, f)| (i, f.clone()))
        .collect()
}

/// The files the grid shows now, first and last, for their pictures
/// to be made first; none while the grid has not said.
pub(crate) fn on_screen(st: &State) -> Option<(usize, usize)> {
    let (first, last) = st.grid_shown?;
    let first = *st.shown.get(usize::try_from(first).ok()?)?;
    let last = *st.shown.get(
        usize::try_from(last)
            .ok()?
            .min(st.shown.len().checked_sub(1)?),
    )?;
    Some((first, last))
}

/// Put `next` in the browser in place of the list it has, keeping
/// what each file that stays already has: its sidecar as edited, its
/// picture, its row in the index and its place in the selection. A
/// file new to the list has its sidecar from what the read `brought`
/// off the window's thread, and every file its row from there too; the
/// merge itself asks the disk nothing. Every frame without a picture
/// is asked for one again, in place of whatever was queued. A path
/// twice in `next` is kept once. The frame on screen stays on screen,
/// followed to its new path if the read found it moved; if it is gone,
/// nothing is kept for it and the nearest row is what the caller
/// opens, which this returns.
pub(crate) fn merge(
    st: &mut State,
    app: &App,
    worker: &Worker,
    next: Vec<PathBuf>,
    brought: Brought,
) -> Option<usize> {
    let Brought {
        mut read,
        seen,
        ids,
    } = brought;
    let seen = &seen;
    let followed = follow_current(st, seen);
    // The frame on screen gone and not followed anywhere is not in the
    // list, whatever the index still says of it: the last file of a
    // folder deleted leaves its row standing (§160's empty folder).
    let gone = st
        .current
        .and_then(|c| st.files.get(c))
        .filter(|p| !followed && seen.gone(p))
        .cloned();
    let mut listed = HashSet::new();
    let mut next: Vec<PathBuf> = next
        .into_iter()
        .filter(|p| Some(p) != gone.as_ref())
        .filter(|p| listed.insert(p.clone()))
        .collect();
    // A frame on screen that moved out of what this list covers (a
    // folder view, and the frame moved to another folder) stays in
    // the list, where its name sorts: it is still what is being
    // edited, and it is where the index says it is.
    if let Some(c) = st.current
        && let Some(path) = st.files.get(c)
        && gone.as_ref() != Some(path)
        && !listed.contains(path)
    {
        let at = next.partition_point(|p| p.file_name() < path.file_name());
        next.insert(at, path.clone());
    }
    if next == st.files {
        return None;
    }
    let started = Instant::now();
    let old: HashMap<&PathBuf, usize> = st.files.iter().enumerate().map(|(i, p)| (p, i)).collect();
    let from: Vec<Option<usize>> = next.iter().map(|p| old.get(p).copied()).collect();
    drop(old);
    let fresh = from.iter().filter(|f| f.is_none()).count();
    let count = next.len();
    let mut sidecars = Vec::with_capacity(count);
    let mut seed_blend = Vec::with_capacity(count);
    let mut thumb_base = Vec::with_capacity(count);
    let mut thumb_shown = Vec::with_capacity(count);
    let mut thumb_made = Vec::with_capacity(count);
    let mut thumb_asked = Vec::with_capacity(count);
    let mut thumb_failed = Vec::with_capacity(count);
    let mut index_ids = Vec::with_capacity(count);
    for (path, f) in next.iter().zip(&from) {
        match *f {
            Some(i) => {
                sidecars.push(std::mem::take(&mut st.sidecars[i]));
                seed_blend.push(st.seed_blend.get(i).copied().unwrap_or(false));
                thumb_base.push(st.thumb_base[i].take());
                thumb_shown.push(st.thumb_shown[i]);
                thumb_made.push(st.thumb_made[i]);
                thumb_asked.push(st.thumb_asked[i]);
                thumb_failed.push(st.thumb_failed.get(i).copied().unwrap_or(false));
                let old = st.index_ids.get(i).copied().flatten();
                index_ids.push(match ids.as_ref().and_then(|m| m.get(path)) {
                    Some(id) => *id,
                    None => old,
                });
            }
            None => {
                let (sidecar, seed) = read.remove(path).unwrap_or_default();
                sidecars.push(sidecar);
                seed_blend.push(seed);
                thumb_base.push(None);
                thumb_shown.push(None);
                thumb_made.push(0);
                thumb_asked.push(worker::THUMB_WIDTH);
                thumb_failed.push(false);
                index_ids.push(ids.as_ref().and_then(|m| m.get(path).copied().flatten()));
            }
        }
    }
    let mut new_of: Vec<Option<usize>> = vec![None; st.files.len()];
    for (at, f) in from.iter().enumerate() {
        if let Some(i) = *f {
            new_of[i] = Some(at);
        }
    }
    let to_new = |i: usize| new_of.get(i).copied().flatten();
    let old_current = st.current;
    let current = old_current.and_then(to_new);
    let renumbered = st.files.len() != count || from.iter().enumerate().any(|(i, f)| *f != Some(i));
    // What is keyed by the old numbering and cannot be carried over
    // by path cheaply: the culling pictures and the camera picture
    // standing in. Put down, and asked for again below.
    if renumbered {
        if let Some(cull) = st.cull.as_mut() {
            cull.cache.clear();
            cull.textures.clear();
            cull.failed.clear();
            cull.full_asked = None;
        }
        drop_placeholder(st, app);
        st.hold = None;
        st.prefetch.want(Vec::new());
        if let Some(run) = st.thumb_run.as_mut() {
            run.renumber(&from);
        }
    }
    st.picked = st.picked.iter().filter_map(|&i| to_new(i)).collect();
    let gone_row = old_current
        .filter(|_| current.is_none())
        .and_then(|c| st.shown.iter().position(|&f| f == c));
    st.files = next;
    st.sidecars = sidecars;
    st.seed_blend = seed_blend;
    st.thumb_base = thumb_base;
    st.thumb_shown = thumb_shown;
    st.thumb_made = thumb_made;
    st.thumb_asked = thumb_asked;
    st.thumb_failed = thumb_failed;
    st.index_ids = index_ids;
    st.index_passed = vec![true; count];
    st.index_pass_ready = false;
    st.current = current;
    if ids.is_none() {
        crate::library::refresh_ids(st);
    }
    let hidden = rebuild_browser(st, app);
    if renumbered {
        worker.replace_thumbnails(owed_thumbnails(st), on_screen(st));
    }
    tracing::info!(
        "browser: the list merged, {} frames ({} new) in {:.1} ms",
        count,
        fresh,
        started.elapsed().as_secs_f64() * 1e3
    );
    st.library.painted = Some((started, "the merge"));
    if st.cull.is_some()
        && let Some(c) = st.current
    {
        crate::panel::cull::cull_select(st, app, c);
    }
    match (current, gone_row) {
        // The frame on screen is gone from the disk: the nearest row
        // is opened, and nothing is saved for the file that went.
        (None, Some(row)) if !st.shown.is_empty() => Some(row.min(st.shown.len() - 1)),
        (None, _) if old_current.is_none() && !st.shown.is_empty() => Some(0),
        _ => hidden,
    }
}

/// What the indexer said of a pass in the background, on the UI
/// thread: the roots' counts again, the launch pass counted down, the
/// pictures of files the pass found changed asked for again, and the
/// list merged when the pass touched what it shows. True when the
/// list was read again, which reads the index's rows and facets with
/// it.
pub(crate) fn background_done(
    state: &Rc<RefCell<State>>,
    app: &App,
    path: &Path,
    report: &greycard_library::Report,
    launch: bool,
    failed: bool,
) -> bool {
    let changed = report.added
        + report.moved
        + report.changed
        + report.missing
        + report.returned
        + report.meta_refreshed
        > 0;
    let launch_done = {
        let mut st = state.borrow_mut();
        let mut done = false;
        if launch {
            st.library.launch_left = st.library.launch_left.saturating_sub(1);
            if st.library.launch_left == 0 {
                // A capture waiting on the pass goes: what it found is
                // merged below, a frame is opened from it, and the
                // capture waits for that frame's develop as ever.
                st.library.awaiting = false;
                done = true;
            }
        }
        recount(&mut st);
        show(&st, app);
        // A file changed on disk (a copy that finished) has another
        // picture: asked for again, those files and no others.
        let again = changed_rows(&st, report);
        if !again.is_empty()
            && let Some(worker) = crate::WORKER.with(|w| w.borrow().clone())
        {
            for i in again {
                worker.send(Job::Thumbnail {
                    index: i,
                    path: st.files[i].clone(),
                });
            }
        }
        done
    };
    // Whether the list is to be read again, by what the window holds
    // and what the pass said, the disk not asked: the read does that
    // off the window's thread. A pass under the frame on screen reads
    // it again whatever the pass says, since the frame may be gone: the
    // last file of a folder deleted leaves the folder empty, which the
    // index takes for a drive not mounted (§160) and marks nothing, so
    // no count moves. So does a pass that could not read a folder,
    // whose files leave the list.
    let touches = {
        let st = state.borrow();
        let under_current = st
            .current
            .and_then(|c| st.files.get(c))
            .is_some_and(|p| p.starts_with(path));
        // A pass that failed outright (a watcher's pass over the folder
        // that was just locked fails at that folder, and says nothing
        // in its report), or one over a folder the last read could not
        // read, may have changed which folders can be read.
        let over_unreadable = st
            .library
            .unreadable
            .iter()
            .any(|d| d.starts_with(path) || path.starts_with(d));
        let worth =
            changed || under_current || failed || over_unreadable || !report.unreadable.is_empty();
        match st.view.clone() {
            View::Roots(None) => worth,
            View::Roots(Some(r)) => worth && (path.starts_with(&r) || r.starts_with(path)),
            // The folder's own list is what the read brings, the frame
            // on screen with it. A folder whose canonical form the
            // window does not know yet is read: the read brings that.
            View::Folder => st.files.first().and_then(|f| f.parent()).is_some_and(|d| {
                st.library
                    .canonical
                    .get(d)
                    .is_none_or(|key| key.starts_with(path))
            }),
        }
    };
    let merged = touches
        && crate::WORKER
            .with(|w| w.borrow().clone())
            .is_some_and(|worker| refresh_view(state, app, &worker));
    if launch_done {
        let empty = {
            let st = state.borrow();
            matches!(st.view, View::Roots(_))
                && st.files.is_empty()
                && !st.library.loading
                && !st.library.merging
        };
        if empty {
            empty_view(&mut state.borrow_mut(), app);
        }
    }
    merged
}

/// The rows of the files a pass found changed, by the paths it names:
/// the list's own spelling first, else the index's canonical one for a
/// file of the same name.
pub(crate) fn changed_rows(st: &State, report: &greycard_library::Report) -> Vec<usize> {
    if report.changed_files.is_empty() {
        return Vec::new();
    }
    let changed: HashSet<&PathBuf> = report.changed_files.iter().collect();
    let names: HashSet<&std::ffi::OsStr> = report
        .changed_files
        .iter()
        .filter_map(|p| p.file_name())
        .collect();
    st.files
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            changed.contains(f)
                || (f.file_name().is_some_and(|n| names.contains(n))
                    && changed.contains(&keyed(st, f)))
        })
        .map(|(i, _)| i)
        .collect()
}

/// A file's path as the index keys it, by its folder's canonical form
/// as the window has kept it, else asked of the disk.
fn keyed(st: &State, f: &Path) -> PathBuf {
    match (
        f.parent().and_then(|d| st.library.canonical.get(d)),
        f.file_name(),
    ) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => greycard_library::key_path(f),
    }
}

/// The view is empty for good: nothing under the roots, and no pass
/// left to bring anything. Said, and a capture of it ends here, failed,
/// rather than wait for a picture that will never come.
pub(crate) fn empty_view(st: &mut State, app: &App) {
    let why = if st.library.roots.is_empty() {
        "the library has no folders: nothing to show"
    } else if st
        .library
        .roots
        .list()
        .iter()
        .all(|r| st.library.offline.contains(r))
    {
        "every folder in the library is offline: nothing to show"
    } else {
        "nothing under the library's folders"
    };
    app.set_status(why.into());
    tracing::warn!("library: {why}");
    st.library.awaiting = false;
    if st.batch {
        st.failed = true;
        let _ = slint::quit_event_loop();
    }
}

/// The header's callbacks: a root chosen, all of them, one added by
/// the chooser or as the folder open, one taken out, one named.
pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
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
                    .and_then(|i| st.files.get(i))
                    .and_then(|f| f.parent())
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(|| PathBuf::from("/"))
            };
            let app_weak = app.as_weak();
            app.set_status("choosing a folder to add to the library...".into());
            export::choose_folder("Add a folder to the library", start, move |chosen| {
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
                            app.set_status("the desktop offered no file chooser".into());
                        }
                    }
                });
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{state_for, window};
    use greycard_library::fixture::{A7, R5, R6, write_frame};
    use std::time::Duration;

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
    }

    /// The reads sent, how many.
    fn sent() -> usize {
        SENT.with(|s| s.borrow().len())
    }

    /// Every read sent run and landed, and those their landing sent;
    /// how many.
    fn land_all(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) -> usize {
        let mut landed = 0;
        loop {
            let looks: Vec<Look> = SENT.with(|s| s.borrow_mut().drain(..).collect());
            if looks.is_empty() {
                return landed;
            }
            for look in looks {
                land(state, app, worker, look.run());
                landed += 1;
            }
        }
    }

    /// The sidecars a read would bring for the files of `next` the
    /// window does not list yet.
    fn read_new(st: &State, next: &[PathBuf]) -> HashMap<PathBuf, (Sidecar, bool)> {
        let fresh: Vec<PathBuf> = next
            .iter()
            .filter(|p| !st.files.contains(p))
            .cloned()
            .collect();
        let (s, b) = load_sidecars_parallel(&fresh, st.write_sidecars, None);
        fresh.into_iter().zip(s.into_iter().zip(b)).collect()
    }

    /// The frame on screen as a read would find it, with the index at
    /// `db` to follow it by.
    fn seen_now(st: &State, db: Option<&Path>) -> OnScreen {
        st.current
            .and_then(|c| {
                let path = st.files.get(c)?.clone();
                let id = st.index_ids.get(c).copied().flatten();
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
            ids: None,
        };
        merge(&mut state.borrow_mut(), app, worker, next, brought)
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

    /// A list merged keeps what each frame that stays already has, by
    /// its path: the sidecar as edited in memory, the picture made,
    /// its place in the selection. A frame new to the list has its
    /// sidecar read from disk; one gone leaves the selection.
    #[test]
    fn a_list_merged_keeps_each_frames_own_by_its_path() {
        let dir = scratch("merge");
        let files = frames(&dir, &["a.tif", "b.tif", "c.tif"]);
        let app = window(3);
        let (state, worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.write_sidecars = true;
            st.sidecars[1].meta.rating = 4;
            st.thumb_base[2] = Some((1, 1, vec![9, 9, 9]));
            st.thumb_made[2] = 128;
            st.current = Some(1);
            st.picked = vec![1, 2];
            rebuild_browser(&mut st, &app);
        }
        let aa = frames(&dir, &["aa.tif"]).remove(0);
        let mut s = Sidecar::default();
        s.meta.rating = 2;
        s.save(&aa).unwrap();
        let next = vec![
            files[0].clone(),
            aa.clone(),
            files[1].clone(),
            files[2].clone(),
        ];
        let row = merge_read(&state, &app, &worker, next.clone(), None);
        {
            let st = state.borrow();
            assert_eq!(st.files, next);
            assert_eq!(st.sidecars[2].meta.rating, 4, "b's own, as edited");
            assert_eq!(st.sidecars[1].meta.rating, 2, "aa's, from its sidecar");
            assert_eq!(st.thumb_made[3], 128);
            assert!(st.thumb_base[3].is_some());
            assert_eq!(st.current, Some(2));
            assert_eq!(st.picked, vec![2, 3]);
            assert_eq!(row, None, "the frame on screen stays");
            assert_eq!(app.get_thumbs().row_count(), 4);
            assert_eq!(app.get_selected(), 2);
        }
        // c goes from the list: out of the selection too.
        let next = vec![files[0].clone(), aa.clone(), files[1].clone()];
        assert_eq!(merge_read(&state, &app, &worker, next, None), None);
        assert_eq!(state.borrow().picked, vec![2]);
        // The same list again is nothing to do.
        let same = state.borrow().files.clone();
        assert_eq!(merge_read(&state, &app, &worker, same, None), None);
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The frame on screen deleted from under the window: the nearest
    /// row is what is opened next, and nothing is kept as current, so
    /// no sidecar is written for a file that is gone.
    #[test]
    fn a_frame_on_screen_that_is_gone_hands_on_to_the_nearest() {
        let dir = scratch("gone");
        let files = frames(&dir, &["a.tif", "b.tif", "c.tif"]);
        let app = window(3);
        let (state, worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.current = Some(2);
            rebuild_browser(&mut st, &app);
        }
        std::fs::remove_file(&files[2]).unwrap();
        let next = files[..2].to_vec();
        let row = merge_read(&state, &app, &worker, next, None);
        assert_eq!(row, Some(1), "c's row, clamped to the last");
        assert_eq!(state.borrow().current, None);
        assert!(!files[2].with_extension("tif.gcd").exists());
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A frame on screen renamed, or moved to another folder, is
    /// followed there by its row in the index: its path in the window
    /// is the new one, its edit in memory is kept, and in a folder's
    /// view it stays in the list though the folder no longer has it.
    #[test]
    fn a_frame_on_screen_that_moved_is_followed_to_its_new_path() {
        let dir = scratch("follow");
        let (shoot, other) = (dir.join("shoot"), dir.join("other"));
        std::fs::create_dir_all(&shoot).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let files = frames(&shoot, &["a.tif", "b.tif", "c.tif"]);
        frames(&other, &["o.tif"]);
        let db = dir.join("index").join("library.sqlite");
        let mut writer = greycard_library::Library::open(&db).unwrap();
        writer.index_tree(&dir, &mut |_| {}).unwrap();
        let app = window(3);
        let (state, worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            crate::library::refresh_ids(&mut st);
            st.sidecars[1].meta.rating = 5;
            st.current = Some(1);
            rebuild_browser(&mut st, &app);
        }
        // Renamed within the folder.
        let renamed = shoot.join("bb.tif");
        std::fs::rename(&files[1], &renamed).unwrap();
        writer.index_folder(&shoot, &mut |_| {}).unwrap();
        let listed = crate::files::list_files(&shoot).unwrap();
        assert_eq!(merge_read(&state, &app, &worker, listed, Some(&db)), None);
        {
            let st = state.borrow();
            let c = st.current.expect("still on screen");
            assert_eq!(st.files[c], renamed);
            assert_eq!(st.sidecars[c].meta.rating, 5);
        }
        // Moved to the other folder: followed, and kept in the list.
        let moved = other.join("bb.tif");
        std::fs::rename(&renamed, &moved).unwrap();
        writer.index_tree(&dir, &mut |_| {}).unwrap();
        let listed = crate::files::list_files(&shoot).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(merge_read(&state, &app, &worker, listed, Some(&db)), None);
        {
            let st = state.borrow();
            let c = st.current.expect("still on screen");
            assert_eq!(st.files[c], moved);
            assert_eq!(st.files.len(), 3);
            assert_eq!(st.sidecars[c].meta.rating, 5);
        }
        // The window's thread keeps the state, and with it the
        // reader: closed by hand, since Windows will not remove a
        // folder with a database open in it.
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(writer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The roots' row: a chip a root with its count from the index,
    /// the all-roots chip with their sum, and "Add this folder" while
    /// the folder open is under none of them.
    #[test]
    fn the_header_lists_the_roots_with_their_counts() {
        let dir = scratch("header");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(a.join("day")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        frames(&a, &["x.tif"]);
        frames(&a.join("day"), &["y.tif", "z.tif"]);
        let open = frames(&b, &["o.tif"]);
        let db = dir.join("index").join("library.sqlite");
        greycard_library::Library::open(&db)
            .unwrap()
            .index_tree(&dir, &mut |_| {})
            .unwrap();
        let app = window(1);
        let (state, _worker) = state_for(&app, open);
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.library.roots.add(&a).unwrap();
        recount(&mut st);
        show(&st, &app);
        let chips = app.get_library_roots();
        assert_eq!(chips.row_count(), 1);
        let chip = chips.row_data(0).unwrap();
        assert_eq!((chip.name.as_str(), chip.count, chip.on), ("a", 3, false));
        assert_eq!(app.get_library_all_count(), 3);
        assert!(!app.get_library_all_on());
        assert!(app.get_library_can_add_open(), "b is under no root");
        st.library.roots.add(&b).unwrap();
        st.view = View::Roots(None);
        recount(&mut st);
        show(&st, &app);
        assert_eq!(app.get_library_all_count(), 4);
        assert!(app.get_library_all_on());
        assert!(!app.get_library_can_add_open());
        assert_eq!(app.get_library_note(), "");
        // A name given is the chip's, and the path is still the root.
        st.library.roots.set_name(&a, "Archive");
        assert_eq!(root_name(&st.library.roots, &a), "Archive");
        assert_eq!(root_name(&st.library.roots, &b), "b");
        show(&st, &app);
        let chip = app.get_library_roots().row_data(0).unwrap();
        assert_eq!(chip.name.as_str(), "Archive");
        assert_eq!(chip.path.as_str(), a.to_str().unwrap());
        st.index_reader = None;
        drop(st);
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The row in the grid's header is wired: along it a press finds
    /// the chooser's chip, then the all-roots chip, then the root's
    /// own, then its cross, in that order; and a row of roots longer
    /// than the header scrolls sideways under a plain wheel to its
    /// last root, rather than squeezing every name to an ellipsis
    /// (the review's six roots in 560 px).
    #[test]
    fn the_roots_row_hands_back_what_was_pressed() {
        let app = window(0);
        app.window()
            .set_size(slint::LogicalSize::new(1200.0, 700.0));
        app.set_grid_open(true);
        app.set_library_roots(ModelRc::new(VecModel::from(vec![RootChip {
            name: "shoots".into(),
            path: "/x/shoots".into(),
            count: 12,
            on: false,
            offline: false,
        }])));
        app.set_library_all_count(12);
        let seen = Rc::new(RefCell::new(Vec::<String>::new()));
        let s = seen.clone();
        app.on_library_root_picked(move |p| s.borrow_mut().push(format!("picked {p}")));
        let s = seen.clone();
        app.on_library_root_removed(move |p| s.borrow_mut().push(format!("removed {p}")));
        let s = seen.clone();
        app.on_library_root_add(move || s.borrow_mut().push("add".into()));
        let s = seen.clone();
        app.on_library_root_add_open(move || s.borrow_mut().push("add open".into()));
        // The row is the header's second, under the controls.
        for x in (0..600).step_by(3) {
            crate::testing::click(&app, x as f32, 56.0);
        }
        let firsts = |seen: &[String]| {
            let mut said: Vec<String> = Vec::new();
            for s in seen {
                if !said.contains(s) {
                    said.push(s.clone());
                }
            }
            said
        };
        assert_eq!(
            firsts(&seen.borrow()),
            ["add", "picked ", "picked /x/shoots", "removed /x/shoots"],
            "{:?}",
            seen.borrow()
        );

        // Six roots with long names in a narrow window.
        seen.borrow_mut().clear();
        app.window().set_size(slint::LogicalSize::new(560.0, 700.0));
        let six: Vec<RootChip> = (0..6)
            .map(|i| RootChip {
                name: format!("a long shoot folder name {i}").into(),
                path: format!("/x/r{i}").into(),
                count: 100,
                on: false,
                offline: false,
            })
            .collect();
        app.set_library_roots(ModelRc::new(VecModel::from(six)));
        let (x, y) = (400.0, 56.0);
        crate::testing::click(&app, x, y);
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(x, y),
                delta_x: 0.0,
                delta_y: -5000.0,
            });
        for x in (300..550).step_by(3) {
            crate::testing::click(&app, x as f32, y);
        }
        assert!(
            seen.borrow().iter().any(|s| s == "picked /x/r5"),
            "the last root is reached: {:?}",
            seen.borrow()
        );
    }

    fn right_click(app: &App, x: f32, y: f32) {
        use slint::platform::{PointerEventButton, WindowEvent};
        let position = slint::LogicalPosition::new(x, y);
        for event in [
            WindowEvent::PointerMoved { position },
            WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Right,
            },
            WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Right,
            },
        ] {
            app.window().dispatch_event(event);
        }
    }

    /// The first point along the roots row where a right-click opens a
    /// root's menu, from `from` on, stepping 3 px; the right button
    /// alone, which nothing in the row but a root's chip answers.
    fn menu_opens_at(app: &App, from: f32, y: f32) -> Option<f32> {
        (from as i32..600).step_by(3).map(|x| x as f32).find(|&x| {
            right_click(app, x, y);
            app.get_menu_up()
        })
    }

    /// A root is named from its chip's menu, and the name is only a
    /// label: the root is its path throughout. A plain add is one
    /// action, with no sheet. A right-click on the chip opens the menu
    /// and picks nothing, and the press that closes the menu is not a
    /// press on the chip. The chip is found with the right button
    /// alone; the two left clicks are on the chip, just above where
    /// its menu opened.
    #[test]
    fn a_root_is_named_from_its_menu() {
        let dir = scratch("rename");
        let (photos, b) = (dir.join("Photos"), dir.join("b"));
        std::fs::create_dir_all(&photos).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        frames(&photos, &["x.tif"]);
        let open = frames(&b, &["o.tif"]);
        let app = window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1200.0, 700.0));
        app.set_grid_open(true);
        let (state, _worker) = state_for(&app, open);
        state.borrow_mut().library.roots.add(&photos).unwrap();
        show(&state.borrow(), &app);
        slint::platform::update_timers_and_animations();

        // The roots row is the header's second, under the controls.
        let y = 56.0;
        let x =
            menu_opens_at(&app, 0.0, y).expect("a right-click on the root's chip opens its menu");
        assert_eq!(state.borrow().view, View::Folder, "nothing picked");
        assert!(state.borrow().library.wanted.is_none());
        // The press that closes it, on the chip just above where the
        // menu opened, is let go by.
        let (x, y) = (x + 8.0, y - 6.0);
        crate::testing::click(&app, x, y);
        assert!(!app.get_menu_up());
        assert!(state.borrow().library.wanted.is_none(), "not a pick");
        assert_eq!(
            state.borrow().library.roots.list(),
            std::slice::from_ref(&photos)
        );
        // The next is a press on the chip, which picks its root.
        crate::testing::click(&app, x, y);
        assert_eq!(
            state.borrow_mut().library.wanted.take(),
            Some(View::Roots(Some(photos.clone())))
        );

        // Rename..., from the menu.
        app.invoke_library_root_rename(photos.to_string_lossy().into_owned().into());
        assert!(app.get_root_sheet_open());
        assert_eq!(app.get_root_sheet_folder(), "Photos");
        assert_eq!(app.get_root_sheet_name(), "");
        app.set_root_sheet_name("Archive".into());
        app.invoke_root_sheet_done();
        assert!(!app.get_root_sheet_open());
        let chips = app.get_library_roots();
        let chip = chips.row_data(0).unwrap();
        assert_eq!(
            (chip.name.as_str(), chip.path.as_str()),
            ("Archive", photos.to_str().unwrap())
        );
        assert!(photos.is_dir(), "the folder is where it was");

        // Opened again, the sheet has the name to change; Escape,
        // with the field holding the focus, leaves it as it was.
        app.invoke_library_root_rename(photos.to_string_lossy().into_owned().into());
        assert_eq!(app.get_root_sheet_name(), "Archive");
        slint::platform::update_timers_and_animations();
        crate::testing::press(&app, "Q");
        assert_eq!(
            app.get_root_sheet_name(),
            "Q",
            "the name chosen, typed over"
        );
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(!app.get_root_sheet_open(), "Escape closed it");
        assert_eq!(state.borrow().library.roots.name(&photos), Some("Archive"));
        // A click beside the sheet closes it too.
        app.invoke_library_root_rename(photos.to_string_lossy().into_owned().into());
        crate::testing::click(&app, 20.0, 680.0);
        assert!(!app.get_root_sheet_open());

        // The folder open, added at a press with no sheet, by its
        // folder's name.
        assert!(app.get_library_can_add_open());
        app.invoke_library_root_add_open();
        assert!(!app.get_root_sheet_open());
        {
            let st = state.borrow();
            assert_eq!(st.library.roots.list(), [photos.clone(), b.clone()]);
            assert_eq!(st.library.roots.name(&b), None);
        }
        assert!(
            app.get_status().contains("right-click"),
            "{}",
            app.get_status()
        );
        let names: Vec<String> = app
            .get_library_roots()
            .iter()
            .map(|c| c.name.to_string())
            .collect();
        assert_eq!(names, ["Archive", "b"]);

        // Cleared, the name goes back to the folder's.
        app.invoke_library_root_rename(photos.to_string_lossy().into_owned().into());
        app.set_root_sheet_name("  ".into());
        app.invoke_root_sheet_done();
        assert_eq!(
            app.get_library_roots().row_data(0).unwrap().name.as_str(),
            "Photos"
        );
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// With a root's menu up, a right-click on another root opens that
    /// root's, and the left press that closes it does nothing on
    /// whatever pill it lands on: not the chooser, not the folder
    /// open added, not the all-roots view. The row's callbacks are
    /// the test's own, so no press reaches anything outside it.
    #[test]
    fn the_press_that_closes_a_roots_menu_is_nothing_more() {
        let app = window(0);
        app.window()
            .set_size(slint::LogicalSize::new(1200.0, 700.0));
        app.set_grid_open(true);
        let chip = |name: &str| RootChip {
            name: name.into(),
            path: format!("/x/{name}").into(),
            count: 3,
            on: false,
            offline: false,
        };
        app.set_library_roots(ModelRc::new(VecModel::from(vec![chip("one"), chip("two")])));
        app.set_library_all_count(6);
        app.set_library_can_add_open(true);
        let seen = Rc::new(RefCell::new(Vec::<String>::new()));
        let s = seen.clone();
        app.on_library_root_picked(move |p| s.borrow_mut().push(format!("picked {p}")));
        let s = seen.clone();
        app.on_library_root_removed(move |p| s.borrow_mut().push(format!("removed {p}")));
        let s = seen.clone();
        app.on_library_root_add(move || s.borrow_mut().push("add".into()));
        let s = seen.clone();
        app.on_library_root_add_open(move || s.borrow_mut().push("add open".into()));
        slint::platform::update_timers_and_animations();
        let y = 56.0;

        // The two chips, and the gap between them, by the right
        // button alone.
        let one = menu_opens_at(&app, 0.0, y).expect("the first root's chip");
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(!app.get_menu_up());
        let gap = (one as i32..600)
            .step_by(3)
            .map(|x| x as f32)
            .find(|&x| {
                right_click(&app, x, y);
                let up = app.get_menu_up();
                if up {
                    crate::testing::press(&app, slint::platform::Key::Escape);
                }
                !up
            })
            .expect("the first chip ends");
        let two = menu_opens_at(&app, gap, y).expect("the second root's chip");
        crate::testing::press(&app, slint::platform::Key::Escape);

        // A right-click on the second with the first's menu up opens
        // the second's.
        right_click(&app, one, y);
        assert!(app.get_menu_up());
        right_click(&app, two, y);
        assert!(app.get_menu_up(), "the second root's menu is up");
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(!app.get_menu_up());

        // Every point of the row before the first root, closing its
        // menu, does nothing there.
        for x in (0..one as i32).step_by(3) {
            right_click(&app, one, y);
            assert!(app.get_menu_up());
            crate::testing::click(&app, x as f32, y);
            assert!(!app.get_menu_up(), "closed at {x}");
        }
        assert!(seen.borrow().is_empty(), "{:?}", seen.borrow());
        // And with no menu up, the same row answers.
        for x in (0..one as i32).step_by(3) {
            crate::testing::click(&app, x as f32, y);
        }
        let said = seen.borrow();
        for want in ["add", "add open", "picked "] {
            assert!(said.iter().any(|s| s == want), "{want}: {said:?}");
        }
    }

    /// The preset sheet's name field holds the focus as the root
    /// sheet's does, and Escape closes it all the same.
    #[test]
    fn escape_closes_the_preset_sheet_from_its_field() {
        let app = window(0);
        app.set_preset_open(true);
        slint::platform::update_timers_and_animations();
        crate::testing::press(&app, "Q");
        assert_eq!(app.get_preset_name(), "Q", "the field has the keys");
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(!app.get_preset_open());
    }

    /// The indexer's background: the launch pass over a root, and a
    /// change the watcher saw, each said when done — on a library of
    /// the test's own, beside which its roots would be kept.
    #[test]
    fn the_indexer_passes_over_the_roots_and_the_changes() {
        use crate::library::Told;
        let dir = scratch("indexer");
        let root = dir.join("root");
        std::fs::create_dir_all(root.join("day")).unwrap();
        frames(&root, &["a.tif"]);
        frames(&root.join("day"), &["b.tif", "c.tif"]);
        let db = dir.join("data").join("library.sqlite");
        assert_eq!(
            Roots::path_beside(&db),
            dir.join("data").join(greycard_library::roots::FILE_NAME)
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let indexer = crate::library::Indexer::start(db.clone(), move |told| {
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
        wait(&|t| matches!(t, Told::Opened(_)));
        indexer.roots(vec![root.clone()]);
        match wait(&|t| matches!(t, Told::Background { .. })) {
            Told::Background {
                path,
                launch,
                report,
                error,
                ..
            } => {
                assert_eq!(path, root);
                assert!(launch);
                assert_eq!(error, None);
                assert_eq!(report.added, 3, "{report:?}");
            }
            other => panic!("{other:?}"),
        }
        frames(&root.join("day"), &["d.tif"]);
        indexer
            .asker()
            .changes(vec![greycard_library::Change::Folder(root.join("day"))]);
        match wait(&|t| matches!(t, Told::Background { .. })) {
            Told::Background {
                path,
                launch,
                report,
                ..
            } => {
                assert_eq!(path, root.join("day"));
                assert!(!launch);
                assert_eq!(report.added, 1, "{report:?}");
            }
            other => panic!("{other:?}"),
        }
        let reader = greycard_library::Library::open_read_only(&db).unwrap();
        assert_eq!(reader.count_under(&root).unwrap(), 4);
        drop(reader);
        // An asker still held, as the watcher holds one, does not keep
        // the thread from leaving.
        let _held = indexer.asker();
        indexer.stop(Duration::from_secs(20));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The first pixel of a row's picture on the window.
    fn row_pixel(app: &App, row: usize) -> Option<[u8; 3]> {
        let image = app.get_thumbs().row_data(row)?.image;
        let rgba = image.to_rgba8()?;
        let px = rgba.as_bytes();
        (px.len() >= 3).then(|| [px[0], px[1], px[2]])
    }

    /// B1, the review's repro: eight frames, two flagged reject, the
    /// rejects moved out. Every frame left keeps its own picture; the
    /// first cut carried the picture at old row r to new file r, and
    /// every frame after a reject showed its neighbor's.
    #[test]
    fn after_the_rejects_move_each_frame_keeps_its_own_picture() {
        let dir = scratch("rejects-pictures");
        let names: Vec<String> = (0..8).map(|i| format!("f{i}.tif")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let files = frames(&dir, &names);
        let app = window(8);
        let (state, worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.write_sidecars = true;
            for (i, f) in files.iter().enumerate() {
                // A picture of one pixel, its red the frame's number.
                st.thumb_base[i] = Some((1, 1, vec![10 * i as u8 + 5, 0, 0]));
                st.thumb_made[i] = 128;
                if i == 2 || i == 5 {
                    st.sidecars[i].meta.flag = meta::Flag::Reject;
                    st.sidecars[i].save(f).unwrap();
                }
            }
            rebuild_browser(&mut st, &app);
            for row in 0..8 {
                assert_eq!(row_pixel(&app, row), Some([10 * row as u8 + 5, 0, 0]));
            }
            crate::panel::cull::move_rejects(&mut st, &app, &worker);
            assert_eq!(st.files.len(), 6);
        }
        let st = state.borrow();
        for (row, &f) in st.shown.iter().enumerate() {
            let own = files.iter().position(|p| *p == st.files[f]).unwrap();
            assert_eq!(
                row_pixel(&app, row),
                Some([10 * own as u8 + 5, 0, 0]),
                "row {row}, {}",
                st.files[f].display()
            );
        }
        drop(st);
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// B2, the review's repro: a merge while the thumbnails are still
    /// coming (a raw copied in 0.74 s after the view came up). Every
    /// frame without a picture is asked for again under its new
    /// number; the first cut asked only for the new file, the jobs
    /// already queued arrived under numbers that now meant other files
    /// and were dropped, and the grid stayed blank.
    #[test]
    fn a_merge_while_the_pictures_are_coming_asks_for_every_one_still_owed() {
        let dir = scratch("merge-owed");
        let files = frames(&dir, &["a.tif", "b.tif", "c.tif", "d.tif"]);
        let app = window(4);
        let (state, _worker) = state_for(&app, files.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = Worker::new(move |outcome| {
            let _ = tx.send(outcome);
        });
        // b's picture is in; the rest were queued under the old
        // numbers, by the open, and have not come.
        {
            let mut st = state.borrow_mut();
            st.thumb_base[1] = Some((1, 1, vec![1, 2, 3]));
            st.thumb_made[1] = 128;
            rebuild_browser(&mut st, &app);
        }
        let aa = frames(&dir, &["aa.tif"]).remove(0);
        let next = vec![
            files[0].clone(),
            aa.clone(),
            files[1].clone(),
            files[2].clone(),
            files[3].clone(),
        ];
        merge_read(&state, &app, &worker, next.clone(), None);
        let owed: Vec<usize> = vec![0, 1, 3, 4];
        let mut came: Vec<usize> = Vec::new();
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while came.len() < owed.len() && std::time::Instant::now() < until {
            let Ok(outcome) = rx.recv_timeout(Duration::from_millis(200)) else {
                continue;
            };
            let (index, path) = match outcome {
                crate::worker::Outcome::Thumbnail { index, path, .. }
                | crate::worker::Outcome::NoThumbnail { index, path } => (index, path),
                _ => continue,
            };
            // What the window would take: a picture for the file
            // that is at that number now.
            if next.get(index) == Some(&path) && !came.contains(&index) {
                came.push(index);
            }
        }
        came.sort();
        assert_eq!(came, owed, "each frame still owed a picture gets one");
        worker.stop();
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// B3, the review's repro: the frame on screen copied to `backup/`
    /// and the original deleted. The frame is gone, not moved: the
    /// window does not take the copy's path for it (which would have
    /// saved its edit over the copy's sidecar), the copy is not in the
    /// list twice, and the nearest row is what opens next.
    #[test]
    fn a_frame_deleted_after_a_backup_is_gone_not_followed_to_the_copy() {
        let dir = scratch("backup");
        let (shoot, backup) = (dir.join("shoot"), dir.join("backup"));
        std::fs::create_dir_all(&shoot).unwrap();
        std::fs::create_dir_all(&backup).unwrap();
        let files = frames(&shoot, &["a.tif", "b.tif", "c.tif"]);
        let db = dir.join("index").join("library.sqlite");
        let mut writer = greycard_library::Library::open(&db).unwrap();
        writer.index_tree(&dir, &mut |_| {}).unwrap();
        let app = window(3);
        let (state, worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            crate::library::refresh_ids(&mut st);
            st.sidecars[1].meta.rating = 5;
            st.current = Some(1);
            rebuild_browser(&mut st, &app);
        }
        std::fs::copy(&files[1], backup.join("b.tif")).unwrap();
        writer.index_folder(&backup, &mut |_| {}).unwrap();
        std::fs::remove_file(&files[1]).unwrap();
        writer.index_folder(&shoot, &mut |_| {}).unwrap();
        // The all-roots view's list: the copy is under the root too.
        let listed = writer.paths_under(std::slice::from_ref(&dir)).unwrap();
        assert_eq!(listed.len(), 3);
        let row = merge_read(&state, &app, &worker, listed.clone(), Some(&db));
        {
            let st = state.borrow();
            assert_eq!(st.current, None, "the frame is gone, not followed");
            assert_eq!(st.files, listed, "each path once");
            assert!(row.is_some(), "the nearest row opens next");
            let copy = st.files.iter().position(|p| *p == backup.join("b.tif"));
            assert_eq!(st.sidecars[copy.unwrap()].meta.rating, 0, "the copy's own");
        }
        assert!(!backup.join("b.tif.gcd").exists());
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(writer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// B4, the review's repro: a filter kept from the last session
    /// (picks only) over a batch export of a file named on the command
    /// line, or a file double-clicked from the desktop. Neither puts
    /// it back; a session opened on a folder does.
    #[test]
    fn a_kept_filter_is_not_put_back_over_a_file_or_a_batch_run() {
        use crate::panel::startup::restores_filter;
        let dir = scratch("kept-filter");
        let files = frames(&dir, &["x.tif"]);
        let x = files[0].to_string_lossy().into_owned();
        let d = dir.to_string_lossy().into_owned();
        let cli = |args: &[&str]| {
            let mut all = vec!["greycard-ui"];
            all.extend_from_slice(args);
            Cli::parse_from(all)
        };
        assert!(!restores_filter(&cli(&[&x, "--export", "out.jpg"])));
        assert!(!restores_filter(&cli(&[&x])), "a file named");
        assert!(!restores_filter(&cli(&[&d, "--snapshot", "s.png"])));
        assert!(restores_filter(&cli(&[&d])), "a session on a folder");
        assert!(restores_filter(&cli(&[])), "a session on the last folder");

        // The desktop's open of a file mid-session: the filter goes
        // rather than hide the file asked for.
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.filter = filter::Filter::from_name("Picks").unwrap();
            st.library.loading = true;
        }
        crate::panel::browser::open_paths(&state, &app, &worker, &files);
        let st = state.borrow();
        assert!(st.filter.is_empty());
        assert_eq!(st.shown, [0]);
        assert!(!st.library.loading, "a folder's open is not a view loading");
        drop(st);
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A root that is not there (unplugged, its mount point gone) is
    /// offline: its chip says so, the view leaves it out, and the
    /// launch pass does not touch it, so its rows stay as they were
    /// rather than all go missing. A folder under a root that cannot
    /// be read is left out of the view the same way. Both are looked at
    /// off the window's thread.
    #[test]
    fn an_offline_root_is_shown_as_such_and_left_out() {
        let dir = scratch("offline");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(a.join("day")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        frames(&a, &["x.tif"]);
        frames(&a.join("day"), &["y.tif"]);
        frames(&b, &["z.tif"]);
        let db = dir.join("index").join("library.sqlite");
        let mut writer = greycard_library::Library::open(&db).unwrap();
        writer.index_tree(&dir, &mut |_| {}).unwrap();
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.library.roots.add(&a).unwrap();
            st.library.roots.add(&b).unwrap();
            recount(&mut st);
        }
        // b unplugged: the folder gone from the disk.
        let away = dir.join("b-away");
        std::fs::rename(&b, &away).unwrap();
        open_view(&state, &app, &worker, View::Roots(None));
        assert_eq!(land_all(&state, &app, &worker), 1);
        {
            let st = state.borrow();
            let chips = app.get_library_roots();
            assert!(!chips.row_data(0).unwrap().offline);
            assert!(chips.row_data(1).unwrap().offline);
            assert_eq!(st.files, [a.join("x.tif"), a.join("day").join("y.tif")]);
            assert_eq!(st.library.counts, vec![2, 1], "b's row kept as it was");
        }
        // The launch pass over an offline root is not asked for; had
        // it been, the tree pass would mark its row missing.
        assert_eq!(
            state
                .borrow()
                .library
                .roots
                .list()
                .iter()
                .filter(|r| online(r))
                .count(),
            1
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let day = a.join("day");
            std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o000)).unwrap();
            let locked = std::fs::read_dir(&day).is_err();
            // Opened again by hand: every folder looked at again.
            open_view(&state, &app, &worker, View::Roots(None));
            land_all(&state, &app, &worker);
            std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o755)).unwrap();
            if locked {
                assert_eq!(
                    state.borrow().files,
                    [a.join("x.tif")],
                    "the locked folder's frame left out"
                );
            }
        }
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(writer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A folder under a root that cannot be read (locked after the
    /// index walked it) leaves the list at the next read, which looks
    /// at it off the window's thread, and comes back at the read after
    /// it can be read again, with no pass over it and no open by hand:
    /// on a network share a failure can be a moment's. The pass that
    /// could not read it says so as a folder, and has the list read.
    #[cfg(unix)]
    #[test]
    fn a_folder_that_cannot_be_read_leaves_the_list_and_comes_back() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("unreadable");
        let root = dir.join("root");
        let (day, other) = (root.join("day"), root.join("other"));
        std::fs::create_dir_all(&day).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        frames(&root, &["x.tif"]);
        frames(&day, &["y.tif", "z.tif"]);
        let db = dir.join("index").join("library.sqlite");
        let mut writer = greycard_library::Library::open(&db).unwrap();
        writer.index_tree(&root, &mut |_| {}).unwrap();
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.index_path = Some(db.clone());
            st.library.roots.add(&root).unwrap();
        }
        open_view(&state, &app, &worker, View::Roots(None));
        land_all(&state, &app, &worker);
        assert_eq!(state.borrow().files.len(), 3);
        assert!(state.borrow().library.readable.contains(&day));

        std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&day).is_ok() {
            // Run as a user no permission stops: nothing to see.
            std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o755)).unwrap();
            state.borrow_mut().index_reader = None;
            return;
        }
        let report = writer.index_tree(&root, &mut |_| {}).unwrap();
        assert_eq!(report.unreadable, std::slice::from_ref(&day), "{report:?}");
        folders_passed(&mut state.borrow_mut(), &root);
        assert!(background_done(&state, &app, &root, &report, false, false));
        assert_eq!(sent(), 1);
        land_all(&state, &app, &worker);
        std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(state.borrow().files, [root.join("x.tif")]);
        assert!(
            !state.borrow().library.readable.contains(&day),
            "not kept as readable, and not kept as unreadable either"
        );

        // A file into another folder, and its pass: the read for it
        // looks at the locked folder again, finds it readable, and
        // brings its frames back.
        frames(&other, &["o.tif"]);
        let report = writer.index_folder(&other, &mut |_| {}).unwrap();
        folders_passed(&mut state.borrow_mut(), &other);
        assert!(background_done(&state, &app, &other, &report, false, false));
        land_all(&state, &app, &worker);
        assert_eq!(state.borrow().files.len(), 4);
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(writer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A pass over a folder lands while a read that looked at the
    /// folder is out: what the read saw of it is not kept, since the
    /// pass is newer, and the next read looks at it again.
    #[test]
    fn a_read_out_during_a_pass_keeps_nothing_of_the_pass_s_folders() {
        let dir = scratch("epoch");
        let root = dir.join("root");
        let (d, e) = (root.join("d"), root.join("e"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::create_dir_all(&e).unwrap();
        frames(&d, &["d1.tif"]);
        frames(&e, &["e1.tif"]);
        let db = dir.join("index").join("library.sqlite");
        let mut writer = greycard_library::Library::open(&db).unwrap();
        writer.index_tree(&root, &mut |_| {}).unwrap();
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.library.roots.add(&root).unwrap();
        }
        open_view(&state, &app, &worker, View::Roots(None));
        land_all(&state, &app, &worker);
        folders_passed(&mut state.borrow_mut(), &d);
        frames(&e, &["e2.tif"]);
        let report = writer.index_folder(&e, &mut |_| {}).unwrap();
        folders_passed(&mut state.borrow_mut(), &e);
        assert!(background_done(&state, &app, &e, &report, false, false));
        let found = SENT.with(|s| s.borrow_mut().pop()).unwrap().run();
        // A pass over d lands before the read does.
        folders_passed(&mut state.borrow_mut(), &d);
        land(&state, &app, &worker, found);
        let st = state.borrow();
        assert!(!st.library.readable.contains(&d), "d's look is older");
        assert!(st.library.readable.contains(&e), "e's is not");
        assert_eq!(st.files.len(), 3);
        drop(st);
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(writer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A read that never comes back (a share that stopped answering in
    /// the middle of it) does not hold the list for good: the next
    /// refresh past [`READ_GIVES_UP`] sends another, and the one given
    /// up on is dropped if it ever lands.
    #[test]
    fn a_read_that_never_lands_is_given_up_on() {
        let dir = scratch("given-up");
        let files = frames(&dir, &["a.tif", "b.tif"]);
        let app = window(0);
        let (state, worker) = state_for(&app, files[..1].to_vec());
        assert!(refresh_view(&state, &app, &worker));
        let lost = SENT.with(|s| s.borrow_mut().pop()).unwrap();
        assert!(refresh_view(&state, &app, &worker));
        assert_eq!(sent(), 0, "one out at a time");
        state.borrow_mut().library.merge_since =
            Some(Instant::now() - READ_GIVES_UP - Duration::from_secs(1));
        assert!(refresh_view(&state, &app, &worker));
        assert_eq!(sent(), 1, "another sent");
        land(&state, &app, &worker, lost.run());
        assert!(state.borrow().library.merging, "the lost one dropped");
        assert_eq!(state.borrow().files.len(), 1);
        land_all(&state, &app, &worker);
        assert_eq!(state.borrow().files, files);
        assert!(!state.borrow().library.merging);
        // A folder's view looks at no root.
        let look = ask(&state.borrow()).unwrap();
        assert!(look.roots.is_empty());
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Let `blocks` go: the test that holds it is done with it.
    static LET_GO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// A root's look that does not come back until `LET_GO`, as a stat
    /// under a hard-mounted share gone away does not.
    fn blocks(_: &Path) -> bool {
        while !LET_GO.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }

    /// A root that does not answer is offline for the read after the
    /// wait, and costs one thread: the reads after it take it for
    /// offline at once, without another thread or another wait, until
    /// it answers. Then it is looked at afresh.
    #[test]
    fn a_root_that_does_not_answer_costs_one_look_and_no_more_waits() {
        let checks = Checks::new(blocks);
        let hung = PathBuf::from("/hung/share");
        let wait = Duration::from_millis(200);
        let started = Instant::now();
        let off = checks.offline(std::slice::from_ref(&hung), wait);
        assert!(started.elapsed() >= wait, "the first read waits for it");
        assert_eq!(off, HashSet::from([hung.clone()]));
        for _ in 0..5 {
            let started = Instant::now();
            let off = checks.offline(std::slice::from_ref(&hung), wait);
            assert!(started.elapsed() < wait / 2, "later reads do not wait");
            assert_eq!(off, HashSet::from([hung.clone()]));
        }
        assert_eq!(checks.started.load(Ordering::Relaxed), 1, "one thread");
        LET_GO.store(true, Ordering::SeqCst);
        let until = Instant::now() + Duration::from_secs(5);
        while checks
            .offline(std::slice::from_ref(&hung), wait)
            .contains(&hung)
        {
            assert!(
                Instant::now() < until,
                "it answered, and is looked at again"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        // The real look, on a root that is not there and one that is.
        let dir = scratch("roots-answer");
        let gone = dir.join("gone");
        let real = Checks::new(online);
        assert_eq!(
            real.offline(&[dir.clone(), gone.clone()], ROOT_WAIT),
            HashSet::from([gone])
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The frame on screen under a root the read found offline is not
    /// looked at (a stat there would not come back): it stays on screen
    /// and in the list, while the rest of the root's frames leave.
    #[test]
    fn the_frame_on_screen_under_a_root_gone_offline_stays() {
        let dir = scratch("offline-current");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        frames(&a, &["x.tif"]);
        frames(&b, &["y.tif", "z.tif"]);
        let db = dir.join("index").join("library.sqlite");
        let mut writer = greycard_library::Library::open(&db).unwrap();
        writer.index_tree(&dir, &mut |_| {}).unwrap();
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.index_path = Some(db.clone());
            st.library.roots.add(&a).unwrap();
            st.library.roots.add(&b).unwrap();
        }
        open_view(&state, &app, &worker, View::Roots(None));
        land_all(&state, &app, &worker);
        let y = b.join("y.tif");
        {
            let mut st = state.borrow_mut();
            st.current = st.files.iter().position(|f| *f == y);
        }
        std::fs::rename(&b, dir.join("b-away")).unwrap();
        assert!(refresh_view(&state, &app, &worker));
        land_all(&state, &app, &worker);
        let st = state.borrow();
        assert!(st.library.offline.contains(&b));
        assert_eq!(st.current.map(|c| st.files[c].clone()), Some(y.clone()));
        assert_eq!(st.files, [a.join("x.tif"), y], "z leaves, the frame stays");
        drop(st);
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(writer);
        std::fs::remove_dir_all(&dir).unwrap();
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
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.library.roots.add(&a).unwrap();
        }
        (dir, writer, app, state, worker)
    }

    /// The count a view's read keeps, as the pool would bump it.
    fn read_so_far(state: &Rc<RefCell<State>>, n: usize) {
        let st = state.borrow();
        let p = st.library.progress.as_ref().expect("a view's read is out");
        p.read.store(n, Ordering::Relaxed);
    }

    /// The window's timer run on, as the event loop would.
    fn tick_on(by: Duration) {
        i_slint_backend_testing::mock_elapsed_time(by);
        slint::platform::update_timers_and_animations();
    }

    #[test]
    fn the_bar_s_words_group_the_thousands() {
        assert_eq!(loading_words(3400, 11711), "Reading 11,711 sidecars… 3,400");
        assert_eq!(loading_words(0, 1), "Reading 1 sidecar… 0");
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(1234567), "1,234,567");
    }

    /// A view whose sidecars take a while: no bar for the first
    /// moment, then one filled to the count read, by the window's own
    /// timer, and gone when the view lands.
    #[test]
    fn a_slow_view_shows_its_bar_after_the_wait_and_drops_it_on_landing() {
        let (dir, writer, app, state, worker) = three_under_a_root("loading-slow");
        open_view(&state, &app, &worker, View::Roots(None));
        let look = SENT.with(|s| s.borrow_mut().pop()).unwrap();
        assert!(state.borrow().library.loading);
        assert!(state.borrow().library.loading_timer.running());
        read_so_far(&state, 1);
        let since = state.borrow().library.loading_since.unwrap();
        loading_tick(
            &mut state.borrow_mut(),
            &app,
            since + LOADING_SHOWN_AFTER / 2,
        );
        assert!(!app.get_loading_shown(), "not within the first moment");
        loading_tick(
            &mut state.borrow_mut(),
            &app,
            since + LOADING_SHOWN_AFTER * 2,
        );
        assert!(app.get_loading_shown(), "past it");
        assert!((app.get_loading_fraction() - 1.0 / 3.0).abs() < 1e-6);
        // The window's own timer from here, the read put far enough
        // back that the machine's clock cannot matter.
        state.borrow_mut().library.loading_since =
            Some(Instant::now() - LOADING_SHOWN_AFTER - Duration::from_millis(10));
        tick_on(LOADING_TICK + Duration::from_millis(1));
        assert!(app.get_loading_shown());
        assert!((app.get_loading_fraction() - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(app.get_loading_line(), "Reading 3 sidecars… 1");
        read_so_far(&state, 2);
        tick_on(LOADING_TICK + Duration::from_millis(1));
        assert!((app.get_loading_fraction() - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(app.get_loading_line(), "Reading 3 sidecars… 2");
        // The read itself counts: what it brings is all three.
        land(&state, &app, &worker, look.run());
        {
            let st = state.borrow();
            assert_eq!(st.files.len(), 3);
            assert!(!st.library.loading);
            assert!(st.library.progress.is_none());
            assert!(!st.library.loading_timer.running(), "the timer stopped");
        }
        assert!(!app.get_loading_shown(), "gone on landing");
        tick_on(LOADING_TICK + Duration::from_millis(1));
        assert!(!app.get_loading_shown());
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(writer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A view that lands within the first moment never shows a bar.
    #[test]
    fn a_fast_view_never_shows_its_bar() {
        let (dir, writer, app, state, worker) = three_under_a_root("loading-fast");
        open_view(&state, &app, &worker, View::Roots(None));
        let since = state.borrow().library.loading_since.unwrap();
        for ms in [0, 100, 199] {
            loading_tick(
                &mut state.borrow_mut(),
                &app,
                since + Duration::from_millis(ms),
            );
            assert!(!app.get_loading_shown(), "{ms} ms");
        }
        assert_eq!(land_all(&state, &app, &worker), 1);
        assert!(!app.get_loading_shown());
        assert!(!state.borrow().library.loading_timer.running());
        loading_tick(
            &mut state.borrow_mut(),
            &app,
            since + LOADING_SHOWN_AFTER * 2,
        );
        assert!(!app.get_loading_shown(), "nothing loading, nothing shown");
        assert_eq!(state.borrow().files.len(), 3);
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(writer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A read that hangs with no report coming is given up on by the
    /// timer once it is past READ_GIVES_UP and its count has stood
    /// still; one still moving past it keeps its bar.
    #[test]
    fn the_timer_gives_up_on_a_stopped_read_and_not_a_moving_one() {
        let (dir, writer, app, state, worker) = three_under_a_root("loading-stalled");
        open_view(&state, &app, &worker, View::Roots(None));
        let lost = SENT.with(|s| s.borrow_mut().pop()).unwrap();
        let since = state.borrow().library.loading_since.unwrap();
        let tick = |at: Duration| loading_tick(&mut state.borrow_mut(), &app, since + at);
        // Moving: a sidecar every 20 s, well past the give-up.
        tick(Duration::from_secs(1));
        read_so_far(&state, 1);
        tick(READ_GIVES_UP - Duration::from_secs(5));
        read_so_far(&state, 2);
        tick(READ_GIVES_UP + Duration::from_secs(5));
        tick(READ_GIVES_UP + LOADING_STALLED);
        assert!(state.borrow().library.loading, "still moving: kept");
        assert!(app.get_loading_shown());
        assert_eq!(app.get_loading_line(), "Reading 3 sidecars… 2");
        // Stopped: the count at 2 for longer than LOADING_STALLED.
        tick(READ_GIVES_UP + Duration::from_secs(5) + LOADING_STALLED + Duration::from_secs(1));
        {
            let st = state.borrow();
            assert!(!st.library.loading, "given up");
            assert!(!st.library.loading_timer.running());
        }
        assert!(!app.get_loading_shown());
        assert!(
            app.get_status().contains("did not come in"),
            "{}",
            app.get_status()
        );
        land(&state, &app, &worker, lost.run());
        assert_eq!(
            state.borrow().view,
            View::Folder,
            "the late read opens nothing"
        );
        assert!(!app.get_loading_shown());
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(writer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A view's read given up on takes its bar with it, and the status
    /// line says why; a folder opened over a view's read does too.
    #[test]
    fn a_view_given_up_on_or_overtaken_drops_its_bar() {
        let (dir, writer, app, state, worker) = three_under_a_root("loading-given-up");
        open_view(&state, &app, &worker, View::Roots(None));
        let lost = SENT.with(|s| s.borrow_mut().pop()).unwrap();
        state.borrow_mut().library.loading_since =
            Some(Instant::now() - READ_GIVES_UP - Duration::from_secs(1));
        tick_on(LOADING_TICK + Duration::from_millis(1));
        assert!(app.get_loading_shown(), "still reading, and said so");
        assert_eq!(app.get_loading_line(), "Reading 3 sidecars… 0");
        refresh_view(&state, &app, &worker);
        assert!(!app.get_loading_shown());
        assert!(!state.borrow().library.loading_timer.running());
        assert!(
            app.get_status().contains("did not come in"),
            "{}",
            app.get_status()
        );
        land(&state, &app, &worker, lost.run());
        land_all(&state, &app, &worker);
        assert!(!app.get_loading_shown(), "the late read brings no bar back");

        // A folder opened while a view's read is out.
        open_view(&state, &app, &worker, View::Roots(None));
        state.borrow_mut().library.loading_since =
            Some(Instant::now() - LOADING_SHOWN_AFTER - Duration::from_millis(10));
        tick_on(LOADING_TICK + Duration::from_millis(1));
        assert!(app.get_loading_shown());
        let one = dir.join("a").join("x.tif");
        crate::panel::browser::open_paths(&state, &app, &worker, &[one]);
        assert!(!app.get_loading_shown());
        assert!(!state.borrow().library.loading_timer.running());
        land_all(&state, &app, &worker);
        assert!(!app.get_loading_shown());
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(writer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A view's read given up on: the status says so rather than
    /// "reading N frames..." for good, a capture waiting on it lets go,
    /// and when the read does land it changes nothing, the roots found
    /// offline among it.
    #[test]
    fn a_view_s_read_given_up_on_lets_go_and_lands_as_nothing() {
        let dir = scratch("open-given-up");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let files = frames(&a, &["x.tif"]);
        let db = dir.join("index").join("library.sqlite");
        let mut writer = greycard_library::Library::open(&db).unwrap();
        writer.index_tree(&dir, &mut |_| {}).unwrap();
        let app = window(0);
        let (state, worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.library.roots.add(&a).unwrap();
            st.library.roots.add(&b).unwrap();
            st.library.awaiting = true;
        }
        std::fs::remove_dir(&b).unwrap();
        open_view(&state, &app, &worker, View::Roots(None));
        assert!(app.get_status().contains("reading"), "{}", app.get_status());
        let lost = SENT.with(|s| s.borrow_mut().pop()).unwrap();
        state.borrow_mut().library.loading_since =
            Some(Instant::now() - READ_GIVES_UP - Duration::from_secs(1));
        refresh_view(&state, &app, &worker);
        {
            let st = state.borrow();
            assert!(!st.library.loading);
            assert!(!st.library.awaiting, "a capture lets go");
        }
        assert!(
            !app.get_status().contains("reading"),
            "{}",
            app.get_status()
        );
        land_all(&state, &app, &worker);
        land(&state, &app, &worker, lost.run());
        let st = state.borrow();
        assert!(st.library.offline.is_empty(), "the late read keeps nothing");
        assert_eq!(st.view, View::Folder, "nor opens its view");
        assert_eq!(st.files, files);
        drop(st);
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(writer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The review's round trip: a folder under a root locked, and the
    /// watcher's pass over that folder fails outright (an error, and a
    /// report that says nothing); then unlocked, and the pass over it
    /// finds nothing changed. Each has the list read, with no other
    /// report needed: the frames leave, and come back.
    #[cfg(unix)]
    #[test]
    fn a_folder_locked_and_unlocked_leaves_and_comes_back_by_its_own_passes() {
        use std::os::unix::fs::PermissionsExt;
        let mut s = Shoot::new("rv-chmod");
        std::fs::set_permissions(&s.d, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&s.d).is_ok() {
            std::fs::set_permissions(&s.d, std::fs::Permissions::from_mode(0o755)).unwrap();
            s.done();
            return;
        }
        let passed = s.writer.index_folder(&s.d, &mut |_| {});
        assert!(passed.is_err(), "the pass over the locked folder fails");
        folders_passed(&mut s.state.borrow_mut(), &s.d);
        let nothing = greycard_library::Report::default();
        assert!(background_done(
            &s.state, &s.app, &s.d, &nothing, false, true
        ));
        land_all(&s.state, &s.app, &s.worker);
        assert_eq!(s.in_d(), 0, "its frames leave");

        std::fs::set_permissions(&s.d, std::fs::Permissions::from_mode(0o755)).unwrap();
        let report = s.writer.index_folder(&s.d, &mut |_| {}).unwrap();
        assert_eq!(report.added + report.changed + report.missing, 0);
        folders_passed(&mut s.state.borrow_mut(), &s.d);
        assert!(background_done(
            &s.state, &s.app, &s.d, &report, false, false
        ));
        land_all(&s.state, &s.app, &s.worker);
        assert_eq!(s.in_d(), 2, "and come back");
        s.done();
    }

    /// A folder's view through a link, the link pointed elsewhere
    /// meanwhile: the next read makes the folder canonical again, so
    /// the rows, the passes and "Add this folder" go by where it points
    /// now.
    #[cfg(unix)]
    #[test]
    fn a_folder_through_a_link_retargeted_is_followed_by_the_next_read() {
        let dir = scratch("link");
        let (a, b, link) = (dir.join("a"), dir.join("b"), dir.join("link"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        frames(&a, &["x.tif"]);
        frames(&b, &["x.tif", "y.tif"]);
        std::os::unix::fs::symlink(&a, &link).unwrap();
        let app = window(0);
        let (state, worker) = state_for(&app, vec![link.join("x.tif")]);
        assert!(refresh_view(&state, &app, &worker));
        land_all(&state, &app, &worker);
        assert_eq!(state.borrow().library.canonical.get(&link), Some(&a));
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&b, &link).unwrap();
        assert!(refresh_view(&state, &app, &worker));
        land_all(&state, &app, &worker);
        let st = state.borrow();
        assert_eq!(st.library.canonical.get(&link), Some(&b));
        assert_eq!(st.files.len(), 2);
        drop(st);
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The merge asks the disk nothing: a list whose folders are all
    /// gone merges as given, and a read landed after its folder went
    /// merges what the read saw.
    #[test]
    fn a_merge_over_folders_all_gone_completes() {
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        let nowhere = crate::testing::folder(4);
        let read: HashMap<PathBuf, (Sidecar, bool)> = nowhere
            .iter()
            .map(|p| (p.clone(), (Sidecar::default(), false)))
            .collect();
        merge(
            &mut state.borrow_mut(),
            &app,
            &worker,
            nowhere.clone(),
            Brought {
                read,
                ..Brought::default()
            },
        );
        assert_eq!(state.borrow().files, nowhere);

        // A read of a folder, then the folder deleted before it lands.
        let dir = scratch("all-gone");
        let files = frames(&dir, &["a.tif", "b.tif"]);
        let (state, worker) = state_for(&app, files[..1].to_vec());
        assert!(refresh_view(&state, &app, &worker));
        let look = SENT.with(|s| s.borrow_mut().pop()).unwrap();
        let found = look.run();
        std::fs::remove_dir_all(&dir).unwrap();
        land(&state, &app, &worker, found);
        assert_eq!(state.borrow().files, files);
        assert!(!state.borrow().library.merging);
    }

    /// A burst of passes reported while a read is out: one read is out
    /// at a time, the burst asks for one more after it, and that one is
    /// merged once.
    #[test]
    fn a_burst_of_reports_is_one_read_and_one_merge() {
        let dir = scratch("burst");
        let root = dir.join("root");
        let days: Vec<PathBuf> = (0..6).map(|i| root.join(format!("day{i}"))).collect();
        for d in &days {
            std::fs::create_dir_all(d).unwrap();
            frames(d, &["a.tif"]);
        }
        let db = dir.join("index").join("library.sqlite");
        let mut writer = greycard_library::Library::open(&db).unwrap();
        writer.index_tree(&root, &mut |_| {}).unwrap();
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.library.roots.add(&root).unwrap();
        }
        open_view(&state, &app, &worker, View::Roots(None));
        land_all(&state, &app, &worker);
        assert_eq!(state.borrow().files.len(), 6);

        // A file copied into the first folder and its pass said: a read
        // goes out. Then one into each of the others, each pass said
        // while that read is out.
        frames(&days[0], &["b.tif"]);
        let report = writer.index_folder(&days[0], &mut |_| {}).unwrap();
        assert!(background_done(
            &state, &app, &days[0], &report, false, false
        ));
        assert_eq!(sent(), 1, "the first report sends a read");
        for d in &days[1..] {
            frames(d, &["b.tif"]);
            let report = writer.index_folder(d, &mut |_| {}).unwrap();
            assert!(background_done(&state, &app, d, &report, false, false));
        }
        assert_eq!(sent(), 1, "the rest wait for it");
        assert!(state.borrow().library.stale);

        // The read out lands and what it saw is merged; the burst's one
        // read after it goes out, and is merged once.
        state.borrow_mut().library.painted = None;
        let look = SENT.with(|s| s.borrow_mut().pop()).unwrap();
        land(&state, &app, &worker, look.run());
        assert!(state.borrow().library.painted.is_some(), "merged");
        assert_eq!(state.borrow().files.len(), 7);
        assert_eq!(sent(), 1, "one more read for the burst");
        state.borrow_mut().library.painted = None;
        assert_eq!(land_all(&state, &app, &worker), 1, "and none after it");
        let st = state.borrow();
        assert!(st.library.painted.is_some(), "merged");
        assert_eq!(st.files.len(), 12);
        assert!(!st.library.merging && !st.library.stale);
        drop(st);
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(writer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// M1: a capture of an empty all-roots view ends, failed, with a
    /// status saying so, rather than wait for a picture that will not
    /// come: the only root offline, or a root with nothing in it once
    /// the launch pass is done.
    #[test]
    fn a_capture_of_an_empty_view_ends() {
        let dir = scratch("empty-view");
        let (gone, empty) = (dir.join("unplugged"), dir.join("empty"));
        std::fs::create_dir_all(&gone).unwrap();
        std::fs::create_dir_all(&empty).unwrap();
        let db = dir.join("index").join("library.sqlite");
        drop(greycard_library::Library::open(&db).unwrap());
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.batch = true;
            st.library.awaiting = true;
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.library.roots.add(&gone).unwrap();
        }
        std::fs::remove_dir(&gone).unwrap();
        open_view(&state, &app, &worker, View::Roots(None));
        land_all(&state, &app, &worker);
        {
            let st = state.borrow();
            assert!(st.failed, "the capture ends");
            assert!(!st.library.awaiting);
        }
        assert!(app.get_status().contains("offline"), "{}", app.get_status());

        // A fresh library over an empty root: the view waits for the
        // launch pass, and ends when the pass brings nothing.
        {
            let mut st = state.borrow_mut();
            st.failed = false;
            st.library.awaiting = true;
            st.library.roots = Roots::default();
            st.library.roots.add(&empty).unwrap();
            st.library.launch_left = 1;
        }
        open_view(&state, &app, &worker, View::Roots(None));
        land_all(&state, &app, &worker);
        assert!(!state.borrow().failed, "the pass is still to come");
        let report = greycard_library::Report::default();
        background_done(&state, &app, &empty, &report, true, false);
        assert!(state.borrow().failed, "nothing came of the pass");
        assert!(app.get_status().contains("nothing"), "{}", app.get_status());
        state.borrow_mut().index_reader = None;
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// M2: a pass that found one file changed asks for that file's
    /// picture again, and no other under the root.
    #[test]
    fn a_changed_file_asks_for_its_own_picture_only() {
        let dir = scratch("changed-rows");
        let files = frames(&dir, &["a.tif", "b.tif", "c.tif"]);
        let app = window(3);
        let (state, _worker) = state_for(&app, files.clone());
        let report = greycard_library::Report {
            changed: 1,
            changed_files: vec![greycard_library::key_path(&files[1])],
            ..greycard_library::Report::default()
        };
        assert_eq!(changed_rows(&state.borrow(), &report), [1]);
        assert!(changed_rows(&state.borrow(), &greycard_library::Report::default()).is_empty());
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// M3: the frame on screen, the last file in its folder, deleted.
    /// The folder is left empty, which the index takes for a drive not
    /// mounted and marks nothing; the window still lets the frame go,
    /// and opens the nearest, rather than keep a frame whose next save
    /// would write a sidecar beside nothing.
    #[test]
    fn the_last_frame_of_a_folder_deleted_goes_from_the_window() {
        let dir = scratch("last-gone");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        frames(&a, &["x.tif", "y.tif"]);
        let lone = frames(&b, &["z.tif"]).remove(0);
        let db = dir.join("index").join("library.sqlite");
        let mut writer = greycard_library::Library::open(&db).unwrap();
        writer.index_tree(&dir, &mut |_| {}).unwrap();
        let listed = writer.paths_under(std::slice::from_ref(&dir)).unwrap();
        let app = window(3);
        let (state, worker) = state_for(&app, listed.clone());
        {
            let mut st = state.borrow_mut();
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.library.roots.add(&dir).unwrap();
            st.view = View::Roots(None);
            crate::library::refresh_ids(&mut st);
            st.current = listed.iter().position(|p| *p == lone);
            rebuild_browser(&mut st, &app);
        }
        std::fs::remove_file(&lone).unwrap();
        let report = writer.index_folder(&b, &mut |_| {}).unwrap();
        assert_eq!(report.missing, 0, "the index keeps the row: {report:?}");
        assert!(!report.unavailable.is_empty());
        background_done(&state, &app, &b, &report, false, false);
        assert_eq!(land_all(&state, &app, &worker), 1);
        {
            let st = state.borrow();
            assert_ne!(st.current.and_then(|c| st.files.get(c)), Some(&lone));
            assert!(!st.files.contains(&lone), "{:?}", st.files);
        }
        assert!(!lone.with_extension("tif.gcd").exists());
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(writer);
        std::fs::remove_dir_all(&dir).unwrap();
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
                st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
                st.index_path = Some(db.clone());
                st.library.roots.add(&root).unwrap();
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
            self.state.borrow_mut().index_reader = None;
            drop(self.state);
            drop(self.writer);
            std::fs::remove_dir_all(&self.dir).unwrap();
        }
    }

    /// The review's repro: a read out while `d` is briefly unreadable,
    /// then a pass over `d` (readable again, nothing changed) landing
    /// before the read. `d`'s frames come back with the read the pass
    /// has sent after it, not only at some later pass.
    #[cfg(unix)]
    #[test]
    fn a_pass_during_a_read_that_saw_a_blip_brings_the_folder_back() {
        use std::os::unix::fs::PermissionsExt;
        let mut s = Shoot::new("rv-epoch");
        folders_passed(&mut s.state.borrow_mut(), &s.d);
        let look = s.copy_into_e("e2.tif");
        std::fs::set_permissions(&s.d, std::fs::Permissions::from_mode(0o000)).unwrap();
        let found = look.run();
        std::fs::set_permissions(&s.d, std::fs::Permissions::from_mode(0o755)).unwrap();
        let report = s.writer.index_folder(&s.d, &mut |_| {}).unwrap();
        assert!(report.errors.is_empty());
        folders_passed(&mut s.state.borrow_mut(), &s.d);
        background_done(&s.state, &s.app, &s.d, &report, false, false);
        land(&s.state, &s.app, &s.worker, found);
        land_all(&s.state, &s.app, &s.worker);
        assert_eq!(s.in_d(), 2, "d is readable");
        let look = s.copy_into_e("e3.tif");
        land(&s.state, &s.app, &s.worker, look.run());
        land_all(&s.state, &s.app, &s.worker);
        assert_eq!(s.in_d(), 2);
        s.done();
    }

    /// The review's repro: a read that finds `d` unreadable for a moment,
    /// with no pass over `d` at all. The next read looks at it again,
    /// and its frames are back.
    #[cfg(unix)]
    #[test]
    fn a_folder_unreadable_for_a_moment_is_back_at_the_next_read() {
        use std::os::unix::fs::PermissionsExt;
        let mut s = Shoot::new("rv-blip");
        folders_passed(&mut s.state.borrow_mut(), &s.d);
        let look = s.copy_into_e("e2.tif");
        std::fs::set_permissions(&s.d, std::fs::Permissions::from_mode(0o000)).unwrap();
        let found = look.run();
        std::fs::set_permissions(&s.d, std::fs::Permissions::from_mode(0o755)).unwrap();
        land(&s.state, &s.app, &s.worker, found);
        land_all(&s.state, &s.app, &s.worker);
        for k in 3..5 {
            let look = s.copy_into_e(&format!("e{k}.tif"));
            land(&s.state, &s.app, &s.worker, look.run());
            land_all(&s.state, &s.app, &s.worker);
        }
        assert_eq!(s.in_d(), 2, "d is readable again, and its frames are back");
        s.done();
    }

    /// The review's check: a rating made in memory while a read is out
    /// survives the read's landing, and the frame on screen stays.
    #[test]
    fn an_edit_made_while_a_read_is_out_survives_its_landing() {
        let mut s = Shoot::new("rv-edit");
        s.state.borrow_mut().write_sidecars = true;
        let d1 = s.d.join("d1.tif");
        let mut on_disk = Sidecar::default();
        on_disk.meta.rating = 1;
        on_disk.save(&d1).unwrap();
        let found = s.copy_into_e("e2.tif").run();
        {
            let mut st = s.state.borrow_mut();
            let i = st.files.iter().position(|f| *f == d1).unwrap();
            st.current = Some(i);
            st.sidecars[i].meta.rating = 5;
        }
        land(&s.state, &s.app, &s.worker, found);
        land_all(&s.state, &s.app, &s.worker);
        {
            let st = s.state.borrow();
            let i = st.files.iter().position(|f| *f == d1).unwrap();
            assert_eq!(st.files.len(), 4);
            assert_eq!(st.sidecars[i].meta.rating, 5, "the edit in memory");
            assert_eq!(st.current.map(|c| st.files[c].clone()), Some(d1));
        }
        s.done();
    }

    /// The review's check: the frame on screen deleted while a read that
    /// saw it there is out. The landing keeps it; the pass over its
    /// folder that the deletion brings lets it go.
    #[test]
    fn the_frame_on_screen_deleted_while_a_read_is_out_goes_at_the_next() {
        let mut s = Shoot::new("rv-del");
        let d1 = s.d.join("d1.tif");
        {
            let mut st = s.state.borrow_mut();
            let i = st.files.iter().position(|f| *f == d1).unwrap();
            st.current = Some(i);
        }
        let found = s.copy_into_e("e2.tif").run();
        std::fs::remove_file(&d1).unwrap();
        land(&s.state, &s.app, &s.worker, found);
        assert!(s.state.borrow().files.contains(&d1), "as the read saw it");
        let report = s.writer.index_folder(&s.d, &mut |_| {}).unwrap();
        folders_passed(&mut s.state.borrow_mut(), &s.d);
        background_done(&s.state, &s.app, &s.d, &report, false, false);
        land_all(&s.state, &s.app, &s.worker);
        {
            let st = s.state.borrow();
            assert!(!st.files.contains(&d1));
            assert_ne!(st.current.map(|c| st.files[c].clone()), Some(d1));
        }
        s.done();
    }

    /// The watchers asked for, taken out of the queue.
    fn builds() -> Vec<Plan> {
        BUILDS.with(|b| b.borrow_mut().drain(..).collect())
    }

    /// Say what a root is on: a network mount's type, or none for a
    /// local disk, whatever the temporary directory is really on.
    fn on(root: &Path, fs: Option<&str>) {
        REMOTE.with(|r| {
            r.borrow_mut()
                .insert(root.to_path_buf(), fs.map(str::to_owned))
        });
    }

    /// A window with an indexer of its own over a library in `dir`, and
    /// `roots` as its roots, kept in memory alone, each on a local disk.
    fn watched_window(dir: &Path, roots: &[PathBuf]) -> (App, Rc<RefCell<State>>, Rc<Worker>) {
        for root in roots {
            on(root, None);
        }
        let app = window(3);
        let (state, worker) = state_for(&app, Vec::new());
        let db = dir.join("data").join("library.sqlite");
        let indexer = crate::library::Indexer::start(db, |_| {}).expect("the indexer starts");
        {
            let mut st = state.borrow_mut();
            st.index = Some(indexer);
            st.library.roots = Roots::from_list(roots.to_vec());
        }
        (app, state, worker)
    }

    fn leave(state: &Rc<RefCell<State>>, dir: &Path) {
        let mut st = state.borrow_mut();
        st.library.watcher = None;
        st.library.poll = None;
        if let Some(indexer) = st.index.take() {
            indexer.stop(Duration::from_secs(20));
        }
        drop(st);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn watched(st: &State) -> Vec<PathBuf> {
        st.library
            .watcher
            .as_ref()
            .map(|w| w.watched.clone())
            .unwrap_or_default()
    }

    /// The watcher is built off the window's thread, and a build for a
    /// set of roots that has changed since it was asked for is dropped
    /// when it lands, not put over the newer one.
    #[test]
    fn a_watcher_built_for_roots_since_changed_is_dropped() {
        let dir = scratch("stale-watch");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let (app, state, _worker) = watched_window(&dir, std::slice::from_ref(&a));
        on(&b, None);
        watch(&mut state.borrow_mut(), &app);
        // Nothing is watched until the build lands.
        assert!(state.borrow().library.watcher.is_none());
        state.borrow_mut().library.roots = Roots::from_list(vec![a.clone(), b.clone()]);
        watch(&mut state.borrow_mut(), &app);
        let plans = builds();
        assert_eq!(plans.len(), 2);
        let mut plans = plans.into_iter();
        let (first, second) = (plans.next().unwrap(), plans.next().unwrap());
        // The newer lands first, then the older, late.
        assert!(build_in_place(&mut state.borrow_mut(), second));
        assert_eq!(watched(&state.borrow()), vec![a.clone(), b.clone()]);
        assert!(!build_in_place(&mut state.borrow_mut(), first));
        assert_eq!(
            watched(&state.borrow()),
            vec![a, b],
            "the older build is not put over the newer"
        );
        leave(&state, &dir);
    }

    /// A root taken out while the watcher is being built: the build
    /// asked for before is not put in place when it lands. The watcher
    /// there is kept until the new one lands, so the roots that stay
    /// are watched all along, and the new one watches only them.
    #[test]
    fn a_root_taken_out_during_a_build_is_not_watched_after() {
        let dir = scratch("remove-watch");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let (app, state, worker) = watched_window(&dir, &[a.clone(), b.clone()]);
        watch(&mut state.borrow_mut(), &app);
        let before = builds();
        assert!(build_in_place(
            &mut state.borrow_mut(),
            before.into_iter().next().unwrap()
        ));
        assert_eq!(watched(&state.borrow()), vec![a.clone(), b.clone()]);
        // A new build out (the roots changed), then `b` taken out
        // before it lands.
        watch(&mut state.borrow_mut(), &app);
        remove(&state, &app, &worker, &b);
        assert_eq!(
            watched(&state.borrow()),
            vec![a.clone(), b.clone()],
            "the old watcher stays until the new one is ready"
        );
        let plans = builds();
        assert_eq!(plans.len(), 2);
        let mut plans = plans.into_iter();
        let (stale, now) = (plans.next().unwrap(), plans.next().unwrap());
        assert!(!build_in_place(&mut state.borrow_mut(), stale));
        assert_eq!(watched(&state.borrow()), vec![a.clone(), b.clone()]);
        assert!(build_in_place(&mut state.borrow_mut(), now));
        assert_eq!(watched(&state.borrow()), vec![a]);
        leave(&state, &dir);
    }

    /// The launch's look at the roots, landing after a newer build's,
    /// still asks for the launch pass over the roots still there, but
    /// does not put its offline roots over the newer build's.
    #[test]
    fn a_late_launch_look_does_not_undo_a_newer_one() {
        let dir = scratch("late-launch");
        let (a, gone) = (dir.join("a"), dir.join("gone"));
        std::fs::create_dir_all(&a).unwrap();
        let (app, state, _worker) = watched_window(&dir, &[a.clone(), gone.clone()]);
        start(&mut state.borrow_mut(), &app);
        assert!(state.borrow().library.starting);
        state.borrow_mut().library.roots = Roots::from_list(vec![a.clone()]);
        watch(&mut state.borrow_mut(), &app);
        let mut plans = builds().into_iter();
        let (launch, newer) = (plans.next().unwrap(), plans.next().unwrap());
        assert!(build_in_place(&mut state.borrow_mut(), newer));
        assert!(state.borrow().library.offline.is_empty());
        assert!(!build_in_place(&mut state.borrow_mut(), launch));
        let st = state.borrow();
        assert!(!st.library.starting);
        assert!(st.library.offline.is_empty(), "{:?}", st.library.offline);
        assert_eq!(st.library.launch_left, 1, "the pass over a, not gone");
        drop(st);
        leave(&state, &dir);
    }

    /// A root added: made canonical and looked at before it is kept
    /// (in place in a test), then its pass asked for and its watcher
    /// built off the window's thread.
    #[test]
    fn a_root_added_is_watched_when_its_build_lands() {
        let dir = scratch("add-watch");
        let a = dir.join("a");
        std::fs::create_dir_all(a.join("day")).unwrap();
        let (app, state, worker) = watched_window(&dir, &[]);
        on(&a, None);
        // Through a path that is not canonical.
        add(&state, &app, &worker, &a.join("day").join(".."));
        assert_eq!(
            state.borrow().library.roots.list(),
            std::slice::from_ref(&a)
        );
        assert!(state.borrow().library.watcher.is_none());
        let plans = builds();
        assert_eq!(plans.len(), 1);
        assert!(build_in_place(
            &mut state.borrow_mut(),
            plans.into_iter().next().unwrap()
        ));
        assert_eq!(watched(&state.borrow()), vec![a]);
        // A folder that is not there is refused, and nothing is built.
        add(&state, &app, &worker, &dir.join("nowhere"));
        assert!(app.get_status().starts_with("not added"));
        assert!(builds().is_empty());
        leave(&state, &dir);
    }

    /// A root on a network mount is not watched: it is said once, and
    /// passed over on the timer when there is one. The others are
    /// watched as ever. On Windows it is watched, and not polled.
    #[test]
    fn a_root_on_a_network_mount_is_not_watched_but_polled() {
        let dir = scratch("remote-watch");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let (app, state, worker) = watched_window(&dir, &[a.clone(), b.clone()]);
        on(&b, Some("nfs4"));
        state.borrow_mut().library.poll_every = Some(Duration::from_secs(600));
        watch(&mut state.borrow_mut(), &app);
        let plan = builds().into_iter().next().unwrap();
        assert!(build_in_place(&mut state.borrow_mut(), plan));
        {
            let st = state.borrow();
            if WATCH_REMOTE {
                assert_eq!(watched(&st), vec![a.clone(), b.clone()]);
                assert!(st.library.poll.is_none());
            } else {
                assert_eq!(watched(&st), vec![a.clone()]);
                assert!(st.library.poll.is_some());
            }
            assert_eq!(st.library.remote, vec![(b.clone(), "nfs4".to_string())]);
            assert!(st.library.said_remote.contains(&b));
        }
        // The network root taken out: its timer goes at once, before
        // the new watcher lands. Put back for the rest.
        remove(&state, &app, &worker, &b);
        assert!(state.borrow().library.poll.is_none());
        assert!(state.borrow().library.remote.is_empty());
        builds();
        state.borrow_mut().library.roots = Roots::from_list(vec![a.clone(), b.clone()]);
        // With the timer off, none; and with no root on a network
        // mount, none either.
        state.borrow_mut().library.poll_every = None;
        watch(&mut state.borrow_mut(), &app);
        let plan = builds().into_iter().next().unwrap();
        assert!(build_in_place(&mut state.borrow_mut(), plan));
        assert!(state.borrow().library.poll.is_none());
        on(&b, None);
        state.borrow_mut().library.poll_every = Some(Duration::from_secs(600));
        watch(&mut state.borrow_mut(), &app);
        let plan = builds().into_iter().next().unwrap();
        assert!(build_in_place(&mut state.borrow_mut(), plan));
        {
            let st = state.borrow();
            assert_eq!(watched(&st), vec![a, b]);
            assert!(st.library.remote.is_empty());
            assert!(st.library.poll.is_none(), "a local root is never polled");
        }
        leave(&state, &dir);
    }

    /// The timer's body, its clock a counter: a tree pass over each
    /// network root that answers at each tick, and none over one that
    /// does not; it stops when told.
    #[test]
    fn the_timer_passes_over_the_network_roots_that_answer() {
        let (a, b) = (PathBuf::from("/mnt/nas/a"), PathBuf::from("/mnt/nas/b"));
        let mut ticks = 0;
        let wait = || {
            ticks += 1;
            ticks <= 3
        };
        let away: HashSet<PathBuf> = [b.clone()].into();
        let asked = RefCell::new(Vec::new());
        let n = poll(
            &[a.clone(), b.clone()],
            wait,
            |_| away.clone(),
            &|changes| asked.borrow_mut().push(changes),
        );
        assert_eq!(n, 3);
        let asked = asked.into_inner();
        assert_eq!(asked.len(), 3);
        for batch in asked {
            assert_eq!(batch, vec![a.clone()]);
        }
        // All of them away: nothing asked.
        let mut once = true;
        let n = poll(
            std::slice::from_ref(&b),
            || std::mem::replace(&mut once, false),
            |_| away.clone(),
            &|_| panic!("nothing to pass over"),
        );
        assert_eq!(n, 0);
    }

    /// The timer on its own thread: it asks, and it stops when dropped.
    #[test]
    fn the_timer_asks_and_stops_when_dropped() {
        let dir = scratch("poll");
        let (tx, rx) = std::sync::mpsc::channel();
        let poll = Poll::start(vec![dir.clone()], Duration::from_millis(20), move |c| {
            let _ = tx.send(c);
        })
        .expect("a thread");
        let first = rx.recv_timeout(Duration::from_secs(10)).expect("a pass");
        assert_eq!(first, vec![dir.clone()]);
        drop(poll);
        // The thread goes, and its end of the channel with it.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                _ => assert!(Instant::now() < deadline, "the timer still runs"),
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
