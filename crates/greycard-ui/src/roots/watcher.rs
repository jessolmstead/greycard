//! The watcher on the roots: the launch pass and the watcher built off
//! the window's thread, the network mounts and the timer over them.

use super::*;

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
    let Some(indexer) = &st.views.index else {
        return;
    };
    if st.window.batch && st.views.library.wanted.is_none() {
        return;
    }
    if st.views.library.roots.is_empty() {
        return;
    }
    st.views.library.watch_token += 1;
    let plan = Plan {
        token: st.views.library.watch_token,
        roots: st.views.library.roots.clone(),
        launch: Some(
            st.files
                .first()
                .and_then(|f| f.parent())
                .map(Path::to_path_buf),
        ),
        asker: (!st.window.batch).then(|| indexer.asker()),
    };
    if st.window.batch {
        build_in_place(st, plan);
    } else {
        // Not when no thread could be had: nothing will land to end it.
        st.views.library.starting = send_build(app, plan);
    }
}

/// The watcher on the roots as they are now, asked for off the
/// window's thread. The one there is kept until the new one is ready,
/// so the roots it watches are not left unwatched meanwhile, a root
/// just taken out among them: a pass over that one costs a folder's
/// walk at most, and its rows are left out of the view either way.
pub(crate) fn watch(st: &mut State, app: &App) {
    st.views.library.watch_token += 1;
    let Some(indexer) = &st.views.index else {
        return;
    };
    let plan = Plan {
        token: st.views.library.watch_token,
        roots: st.views.library.roots.clone(),
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
    /// The network mounts below the local roots, which their walks leave
    /// out: told to the indexer before the launch pass.
    left_out: Vec<PathBuf>,
}

/// A watcher's build done, off the window's thread.
pub(crate) struct Built {
    token: u64,
    /// The roots that did not answer.
    offline: HashSet<PathBuf>,
    /// The roots that answered and are on a network mount, with its
    /// type: not watched.
    remote: Vec<(PathBuf, String)>,
    /// The network mounts below the local roots, left out of the watch
    /// and of the local roots' walks.
    left_out: Vec<PathBuf>,
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
        let (mut remote, mut local) = (Vec::new(), Vec::new());
        // The network mounts below a local root (a share at
        // `~/Pictures/NAS` under `~/Pictures`): left out of the watch,
        // which would walk the share a round trip a folder and see only
        // this machine's changes there, and passed over on the timer
        // as a network root is. Found before the launch's order is
        // handed over, so the local roots' launch passes leave them out
        // of their walks too.
        let mut except: Vec<PathBuf> = Vec::new();
        for root in online.iter().cloned() {
            match remote_fs(&root) {
                Some(fs) => {
                    if WATCH_REMOTE {
                        local.push(root.clone());
                    }
                    remote.push((root, fs));
                }
                None => {
                    if !WATCH_REMOTE {
                        for (point, fs) in remote_under_fs(&root) {
                            match fs
                                .map(Mount::Network)
                                .unwrap_or_else(|| mount_on_purpose(&root, &point))
                            {
                                Mount::Network(fs) => {
                                    except.push(point.clone());
                                    remote.push((point, fs));
                                }
                                // Mounted a local disk: watched and
                                // walked with its root, as any folder.
                                Mount::Local => {}
                                Mount::Unknown => except.push(point),
                            }
                        }
                    }
                    local.push(root);
                }
            }
        }
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
                left_out: except.clone(),
            });
        }
        let watcher = match asker {
            Some(asker) if !local.is_empty() => {
                let started = Instant::now();
                let (watcher, failed) = greycard_library::Watcher::start_except(
                    &local,
                    &except,
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
            left_out: except,
            watcher,
        }
    }
}

/// Whether a root on a network mount is watched all the same. On
/// Windows it is: ReadDirectoryChangesW over SMB is one call a root,
/// not one a folder, and the server sends its own changes back
/// (CHANGE_NOTIFY). inotify and FSEvents see only this machine's.
pub(super) const WATCH_REMOTE: bool = cfg!(windows);

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

/// What an automount below a root turned out to mount.
enum Mount {
    Network(String),
    Local,
    /// It did not answer, or is still only an automount: left out of the
    /// watch and the walks, and not polled.
    Unknown,
}

/// An automount below a local root not mounted yet (`autofs` alone in
/// the table, a systemd `x-systemd.automount` before its first use):
/// mounted on purpose, by the roots' look into it on this thread (a
/// listing, which is what mounts it), then classified by the table as it
/// is after. Left alone, nothing in the editor would ever look into it,
/// since it is left out of the watch and the walks: it would never mount,
/// and an archive mounted that way would drop out of the library.
fn mount_on_purpose(root: &Path, point: &Path) -> Mount {
    if answer_of(point, ROOT_WAIT).is_none() {
        tracing::info!(
            "library: not watching {}: an automount below the root {} that did not answer \
             when looked into; looked at again when the roots change or the editor starts",
            point.display(),
            root.display()
        );
        return Mount::Unknown;
    }
    match remote_under_fs(root)
        .into_iter()
        .find(|(p, _)| p == point)
        .map(|(_, fs)| fs)
    {
        Some(Some(fs)) => Mount::Network(fs),
        Some(None) => {
            tracing::info!(
                "library: not watching {}: an automount below the root {} that mounted \
                 nothing when looked into",
                point.display(),
                root.display()
            );
            Mount::Unknown
        }
        // Not among the network mounts or the automounts any more: a
        // local disk mounted there.
        None => Mount::Local,
    }
}

/// The network mounts below a local root, with each one's type.
#[cfg(not(test))]
fn remote_under_fs(root: &Path) -> Vec<(PathBuf, Option<String>)> {
    crate::mounts::remote_under(root)
}

/// In a test, the mounts the test says are below a root; none when it
/// says nothing. Once an automount has been looked into, what the test
/// says it mounted (`tests::AFTER_LOOK`) in its place.
#[cfg(test)]
fn remote_under_fs(root: &Path) -> Vec<(PathBuf, Option<String>)> {
    let said = tests::REMOTE_UNDER.with(|r| r.borrow().get(root).cloned().unwrap_or_default());
    said.into_iter()
        .filter_map(|(point, fs)| {
            let looked = tests::LOOKED.with(|l| l.borrow().contains(&point));
            match tests::AFTER_LOOK.with(|a| a.borrow().get(&point).cloned()) {
                Some(after) if looked => after.map(|fs| (point, fs)),
                _ => Some((point, fs)),
            }
        })
        .collect()
}

/// A build done in place, on the window's thread: for a capture, and
/// for the tests.
pub(super) fn build_in_place(st: &mut State, plan: Plan) -> bool {
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
        left_out,
    } = launch;
    st.views.library.starting = false;
    // Only from the build asked for last: a newer one's look at the
    // roots is not put back by an older one's.
    if token == st.views.library.watch_token {
        st.views.library.offline = offline;
        // An archive that answered runs its queued moves once there is
        // a window to run them on (`send_build`).
        let offline = st.views.library.offline.clone();
        crate::panel::archive::rejects::heard_at_launch(st, &offline);
    }
    // A root taken out meanwhile is not passed over; one added
    // meanwhile asked for its own pass.
    order.retain(|r| st.views.library.roots.list().contains(r));
    // With the mounts below the local roots that the same look found, in
    // one ask, so the launch passes leave them out.
    if !order.is_empty()
        && let Some(indexer) = &st.views.index
    {
        tracing::info!(
            "library: {} root(s), a pass over each on the indexer's thread",
            order.len()
        );
        st.views.library.launch_left = order.len();
        indexer.launch(order, left_out);
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
        left_out,
        watcher,
    } = built;
    if token != st.views.library.watch_token {
        tracing::debug!("library: a watcher built for roots that have changed since, dropped");
        return false;
    }
    if let Some(indexer) = &st.views.index {
        indexer.left_out(left_out);
    }
    st.views.library.offline = offline;
    for (root, fs) in &remote {
        if !st.views.library.said_remote.insert(root.clone()) {
            continue;
        }
        if WATCH_REMOTE {
            tracing::info!(
                "library: {} is on a network drive ({fs}); watched, since Windows forwards the server's changes",
                root.display()
            );
        } else {
            let under = st
                .views
                .library
                .roots
                .list()
                .iter()
                .find(|r| root.starts_with(r) && *r != root)
                .map(|r| format!(", a mount below the root {}", r.display()))
                .unwrap_or_default();
            tracing::info!(
                "library: not watching {}{under}: it is on {fs}, a network filesystem, where a watch sees only this machine's changes; it is passed over at launch{}",
                root.display(),
                match st.views.library.poll_every {
                    Some(every) => format!(" and every {} min", every.as_secs().div_ceil(60)),
                    None => String::new(),
                }
            );
        }
    }
    st.views.library.remote = remote;
    st.views.library.watcher = watcher;
    // The reduced develop of a frame under one of these runs alone.
    if let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) {
        worker.set_network_roots(
            st.views
                .library
                .remote
                .iter()
                .map(|(r, _)| r.clone())
                .collect(),
        );
    }
    restart_poll(st);
    true
}

/// The timer over the roots on a network mount, as `library.remote`
/// has them now: none when there are none, when it is off, on a batch
/// run, and on Windows, where they are watched.
pub(crate) fn restart_poll(st: &mut State) {
    let far: Vec<PathBuf> = st
        .views
        .library
        .remote
        .iter()
        .map(|(r, _)| r.clone())
        .collect();
    st.views.library.poll = match (st.views.library.poll_every, &st.views.index) {
        (Some(every), Some(indexer)) if !far.is_empty() && !st.window.batch && !WATCH_REMOTE => {
            let asker = indexer.asker();
            Poll::start(far, every, move |roots| asker.poll(roots))
        }
        _ => None,
    };
    if let (Some(poll), Some(every)) = (&st.views.library.poll, st.views.library.poll_every) {
        tracing::debug!(
            "library: {} network folder(s) passed over every {} min: {:?}",
            poll.roots.len(),
            every.as_secs().div_ceil(60),
            poll.roots
        );
    }
}

/// The timer's every for `minutes` from the settings: none for zero,
/// and a number of minutes past what a clock holds saturated.
pub(crate) fn poll_every(minutes: u64) -> Option<Duration> {
    (minutes > 0).then(|| Duration::from_secs(minutes.saturating_mul(60)))
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
                    crate::panel::archive::rejects::run_due(&mut st, &app);
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
    /// The roots it passes over, for the log and the tests.
    pub(crate) roots: Vec<PathBuf>,
}

impl Poll {
    pub(crate) fn start(
        roots: Vec<PathBuf>,
        every: Duration,
        pass: impl Fn(Vec<PathBuf>) + Send + 'static,
    ) -> Option<Poll> {
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let kept = roots.clone();
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
            Ok(_) => Some(Poll {
                _stop: stop,
                roots: kept,
            }),
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
pub(super) fn poll(
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
