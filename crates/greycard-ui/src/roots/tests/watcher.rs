//! Tests of the watcher, the timer and the network mounts.

use super::*;

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
    s.state.borrow_mut().window.write_sidecars = true;
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
        st.views.index = Some(indexer);
        st.views.library.roots = Roots::from_list(roots.to_vec());
    }
    (app, state, worker)
}

fn leave(state: &Rc<RefCell<State>>, dir: &Path) {
    let mut st = state.borrow_mut();
    st.views.library.watcher = None;
    st.views.library.poll = None;
    if let Some(indexer) = st.views.index.take() {
        indexer.stop(Duration::from_secs(20));
    }
    drop(st);
    let _ = std::fs::remove_dir_all(dir);
}

fn watched(st: &State) -> Vec<PathBuf> {
    st.views
        .library
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
    assert!(state.borrow().views.library.watcher.is_none());
    state.borrow_mut().views.library.roots = Roots::from_list(vec![a.clone(), b.clone()]);
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
    assert!(state.borrow().views.library.starting);
    state.borrow_mut().views.library.roots = Roots::from_list(vec![a.clone()]);
    watch(&mut state.borrow_mut(), &app);
    let mut plans = builds().into_iter();
    let (launch, newer) = (plans.next().unwrap(), plans.next().unwrap());
    assert!(build_in_place(&mut state.borrow_mut(), newer));
    assert!(state.borrow().views.library.offline.is_empty());
    assert!(!build_in_place(&mut state.borrow_mut(), launch));
    let st = state.borrow();
    assert!(!st.views.library.starting);
    assert!(
        st.views.library.offline.is_empty(),
        "{:?}",
        st.views.library.offline
    );
    assert_eq!(st.views.library.launch_left, 1, "the pass over a, not gone");
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
        state.borrow().views.library.roots.list(),
        std::slice::from_ref(&a)
    );
    assert!(state.borrow().views.library.watcher.is_none());
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

/// A root taken out by a path the list does not have as it is (a
/// link to it): the window's thread only sends the path off, and
/// the root goes when the look made off it lands. A path that does
/// not answer is said, and nothing is taken out.
#[cfg(unix)]
#[test]
fn a_remove_by_an_unseen_path_is_made_canonical_off_the_window_s_thread() {
    let dir = scratch("remove-unseen");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let link = dir.join("to-a");
    std::os::unix::fs::symlink(&a, &link).unwrap();
    let (app, state, worker) = watched_window(&dir, &[a.clone(), b.clone()]);
    HOLD_CHECKS.with(|h| *h.borrow_mut() = true);
    remove(&state, &app, &worker, &link);
    assert_eq!(
        state.borrow().views.library.roots.list(),
        [a.clone(), b.clone()],
        "nothing changed on the window's thread"
    );
    assert_eq!(
        CHECKS_SENT.with(|c| c.borrow().clone()),
        vec![(link.clone(), Check::Remove)]
    );
    assert!(builds().is_empty());
    assert_eq!(run_checks(&state, &app, &worker), 1);
    assert_eq!(
        state.borrow().views.library.roots.list(),
        std::slice::from_ref(&b)
    );
    assert!(app.get_status().contains("is out of the library"));
    // A path that does not answer: said, b kept.
    builds();
    NOT_ANSWERING.with(|n| n.borrow_mut().push(dir.join("share")));
    remove(&state, &app, &worker, &dir.join("share").join("b"));
    assert_eq!(run_checks(&state, &app, &worker), 1);
    assert_eq!(
        state.borrow().views.library.roots.list(),
        std::slice::from_ref(&b)
    );
    assert!(
        app.get_status().contains("timed out after 3 s"),
        "{}",
        app.get_status()
    );
    // The list's own spelling goes at once, nothing sent.
    remove(&state, &app, &worker, &b);
    assert!(state.borrow().views.library.roots.is_empty());
    assert_eq!(run_checks(&state, &app, &worker), 0);
    HOLD_CHECKS.with(|h| *h.borrow_mut() = false);
    NOT_ANSWERING.with(|n| n.borrow_mut().clear());
    builds();
    leave(&state, &dir);
}

/// "Add this folder" over a folder no read has made canonical (here,
/// reached through a link): offered from the path as listed, with no
/// look on the window's thread, and added canonical once the look
/// made off it lands. Through a link into a root it is said to be
/// covered. A folder that does not answer is said, and not added.
#[cfg(unix)]
#[test]
fn add_this_folder_on_an_unseen_folder_looks_off_the_window_s_thread() {
    let dir = scratch("add-unseen");
    let real = dir.join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = dir.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let (app, state, worker) = watched_window(&dir, &[]);
    on(&real, None);
    {
        let mut st = state.borrow_mut();
        st.files = vec![link.join("x.tif")];
        assert!(st.views.library.canonical.is_empty(), "no read has seen it");
    }
    show(&state.borrow(), &app);
    assert!(app.get_library_can_add_open());
    HOLD_CHECKS.with(|h| *h.borrow_mut() = true);
    app.invoke_library_root_add_open();
    assert!(state.borrow().views.library.roots.is_empty());
    assert_eq!(
        CHECKS_SENT.with(|c| c.borrow().clone()),
        vec![(link.clone(), Check::Add)]
    );
    assert_eq!(run_checks(&state, &app, &worker), 1);
    assert_eq!(
        state.borrow().views.library.roots.list(),
        std::slice::from_ref(&real)
    );
    builds();
    // Now under a root, through the same unseen link: still offered
    // (the window does not know where the link goes), and the add
    // says it is covered.
    show(&state.borrow(), &app);
    assert!(app.get_library_can_add_open());
    app.invoke_library_root_add_open();
    assert_eq!(run_checks(&state, &app, &worker), 1);
    assert!(
        app.get_status().starts_with("already in the library"),
        "{}",
        app.get_status()
    );
    // A read that made it canonical: not offered.
    state
        .borrow_mut()
        .views
        .library
        .canonical
        .insert(link.clone(), real.clone());
    show(&state.borrow(), &app);
    assert!(!app.get_library_can_add_open());
    // A folder that does not answer.
    NOT_ANSWERING.with(|n| n.borrow_mut().push(dir.join("share")));
    add(&state, &app, &worker, &dir.join("share"));
    assert_eq!(run_checks(&state, &app, &worker), 1);
    assert!(
        app.get_status().starts_with("not added") && app.get_status().contains("3 s"),
        "{}",
        app.get_status()
    );
    assert_eq!(
        state.borrow().views.library.roots.list(),
        std::slice::from_ref(&real)
    );
    HOLD_CHECKS.with(|h| *h.borrow_mut() = false);
    NOT_ANSWERING.with(|n| n.borrow_mut().clear());
    builds();
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
    state.borrow_mut().views.library.poll_every = Some(Duration::from_secs(600));
    watch(&mut state.borrow_mut(), &app);
    let plan = builds().into_iter().next().unwrap();
    assert!(build_in_place(&mut state.borrow_mut(), plan));
    {
        let st = state.borrow();
        if WATCH_REMOTE {
            assert_eq!(watched(&st), vec![a.clone(), b.clone()]);
            assert!(st.views.library.poll.is_none());
        } else {
            assert_eq!(watched(&st), vec![a.clone()]);
            assert!(st.views.library.poll.is_some());
        }
        assert_eq!(
            st.views.library.remote,
            vec![(b.clone(), "nfs4".to_string())]
        );
        assert!(st.views.library.said_remote.contains(&b));
    }
    // The network root taken out: its timer goes at once, before
    // the new watcher lands. Put back for the rest.
    remove(&state, &app, &worker, &b);
    assert!(state.borrow().views.library.poll.is_none());
    assert!(state.borrow().views.library.remote.is_empty());
    builds();
    state.borrow_mut().views.library.roots = Roots::from_list(vec![a.clone(), b.clone()]);
    // With the timer off, none; and with no root on a network
    // mount, none either.
    state.borrow_mut().views.library.poll_every = None;
    watch(&mut state.borrow_mut(), &app);
    let plan = builds().into_iter().next().unwrap();
    assert!(build_in_place(&mut state.borrow_mut(), plan));
    assert!(state.borrow().views.library.poll.is_none());
    on(&b, None);
    state.borrow_mut().views.library.poll_every = Some(Duration::from_secs(600));
    watch(&mut state.borrow_mut(), &app);
    let plan = builds().into_iter().next().unwrap();
    assert!(build_in_place(&mut state.borrow_mut(), plan));
    {
        let st = state.borrow();
        assert_eq!(watched(&st), vec![a, b]);
        assert!(st.views.library.remote.is_empty());
        assert!(
            st.views.library.poll.is_none(),
            "a local root is never polled"
        );
    }
    leave(&state, &dir);
}

/// A share mounted below a local root is left out of the root's
/// watch and passed over on the timer, as a network root is; the
/// rest of the root is watched. Taking the root out takes the mount
/// off the timer at once.
#[test]
fn a_network_mount_below_a_local_root_is_not_watched_but_polled() {
    let dir = scratch("remote-below");
    let (a, b) = (dir.join("a"), dir.join("b"));
    let nas = a.join("NAS");
    std::fs::create_dir_all(nas.join("2026")).unwrap();
    std::fs::create_dir_all(a.join("day")).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let (app, state, worker) = watched_window(&dir, &[a.clone(), b.clone()]);
    let auto = a.join("Later");
    std::fs::create_dir_all(&auto).unwrap();
    REMOTE_UNDER.with(|r| {
        r.borrow_mut().insert(
            a.clone(),
            vec![
                (nas.clone(), Some("cifs".to_string())),
                (auto.clone(), None),
            ],
        )
    });
    state.borrow_mut().views.library.poll_every = Some(Duration::from_secs(600));
    watch(&mut state.borrow_mut(), &app);
    let plan = builds().into_iter().next().unwrap();
    assert!(build_in_place(&mut state.borrow_mut(), plan));
    {
        let st = state.borrow();
        let watcher = st.views.library.watcher.as_ref().expect("a watcher");
        if WATCH_REMOTE {
            assert!(watcher.except.is_empty());
            assert!(st.views.library.poll.is_none());
        } else {
            assert_eq!(watcher.watched, vec![a.clone(), b.clone()]);
            // The automount not mounted yet left out of the watch
            // too, and not polled: what it mounts is not known.
            assert_eq!(watcher.except, vec![nas.clone(), auto.clone()]);
            assert_eq!(
                st.views.library.remote,
                vec![(nas.clone(), "cifs".to_string())]
            );
            let poll = st.views.library.poll.as_ref().expect("the mount is polled");
            assert_eq!(poll.roots, std::slice::from_ref(&nas));
            assert!(st.views.library.said_remote.contains(&nas));
        }
    }
    // The root taken out: the mount below it goes off the timer at
    // once, before the new watcher lands.
    remove(&state, &app, &worker, &a);
    assert!(state.borrow().views.library.poll.is_none());
    assert!(state.borrow().views.library.remote.is_empty());
    builds();
    REMOTE_UNDER.with(|r| r.borrow_mut().clear());
    leave(&state, &dir);
}

/// An automount below a local root not mounted yet is looked into by
/// the build, which mounts it, and classified by what it mounted: a
/// share is left out and polled, a local disk is watched with its
/// root, and one that does not answer is left out and not polled.
#[test]
fn an_automount_below_a_root_is_mounted_on_purpose_and_classified() {
    if WATCH_REMOTE {
        return;
    }
    let dir = scratch("automount");
    let a = dir.join("a");
    let (share, disk, dead) = (a.join("Archive"), a.join("Card"), a.join("Gone"));
    for d in [&share, &disk, &dead] {
        std::fs::create_dir_all(d).unwrap();
    }
    let (app, state, _worker) = watched_window(&dir, std::slice::from_ref(&a));
    REMOTE_UNDER.with(|r| {
        r.borrow_mut().insert(
            a.clone(),
            vec![
                (share.clone(), None),
                (disk.clone(), None),
                (dead.clone(), None),
            ],
        )
    });
    AFTER_LOOK.with(|l| {
        let mut l = l.borrow_mut();
        l.insert(share.clone(), Some(Some("nfs4".to_string())));
        l.insert(disk.clone(), None);
    });
    NOT_ANSWERING.with(|n| n.borrow_mut().push(dead.clone()));
    LOOKED.with(|l| l.borrow_mut().clear());
    state.borrow_mut().views.library.poll_every = Some(Duration::from_secs(600));
    watch(&mut state.borrow_mut(), &app);
    let plan = builds().into_iter().next().unwrap();
    assert!(build_in_place(&mut state.borrow_mut(), plan));
    {
        let st = state.borrow();
        let looked = LOOKED.with(|l| l.borrow().clone());
        for d in [&share, &disk, &dead] {
            assert!(looked.contains(d), "{} looked into", d.display());
        }
        let watcher = st.views.library.watcher.as_ref().expect("a watcher");
        assert_eq!(watcher.except, vec![share.clone(), dead.clone()]);
        assert_eq!(
            st.views.library.remote,
            vec![(share.clone(), "nfs4".to_string())]
        );
        let poll = st.views.library.poll.as_ref().expect("the share is polled");
        assert_eq!(poll.roots, std::slice::from_ref(&share));
    }
    REMOTE_UNDER.with(|r| r.borrow_mut().clear());
    AFTER_LOOK.with(|l| l.borrow_mut().clear());
    NOT_ANSWERING.with(|n| n.borrow_mut().clear());
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
    crate::testing::remove_dir_retry(&dir);
}
