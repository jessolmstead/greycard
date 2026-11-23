use super::*;
use crate::testing::{state_for, window};
use greycard_library::fixture::{A7, R5, write_frame};
use greycard_library::sync_note::note_disk_save;
use std::cell::RefCell;

type Sent = Box<dyn FnOnce(&Rc<Shared>, &App, &Rc<Worker>)>;

thread_local! {
    pub(crate) static SENT: RefCell<Vec<Sent>> = RefCell::new(Vec::new());
    /// The next job sent is heard as set aside first, then late.
    pub(crate) static ASIDE_NEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run the jobs sent off and land them, as the threads and the
/// event loop would, until none are left; how many there were.
pub(crate) fn land_sent(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>) -> usize {
    let mut n = 0;
    loop {
        let jobs: Vec<Sent> = SENT.with(|s| s.borrow_mut().drain(..).collect());
        if jobs.is_empty() {
            return n;
        }
        for job in jobs {
            job(state, app, worker);
            n += 1;
        }
    }
}

fn scratch(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "greycard-sync-ui-{what}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dunce::canonicalize(&dir).unwrap()
}

/// A local root and an archive root, each with the shoot's two
/// frames, the archive's a byte-for-byte copy, and the index over
/// both at `db`.
fn two_places(dir: &Path) -> (PathBuf, PathBuf, Vec<PathBuf>, PathBuf) {
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    let shoot = local.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    let a = shoot.join("a.tif");
    let b = shoot.join("b.tif");
    write_frame(&a, &R5, 1);
    write_frame(&b, &A7, 2);
    let there = nas.join("shoot");
    std::fs::create_dir_all(&there).unwrap();
    std::fs::copy(&a, there.join("a.tif")).unwrap();
    std::fs::copy(&b, there.join("b.tif")).unwrap();
    let db = dir.join("library.sqlite");
    let mut lib = Library::open(&db).unwrap();
    lib.index_tree(&local, &mut |_| {}).unwrap();
    lib.index_tree(&nas, &mut |_| {}).unwrap();
    (local, nas, vec![a, b], db)
}

/// A window over `files` with the two roots, `nas` an archive, the
/// index opened as the window opens it (which says what waits),
/// sidecars written.
fn opened(
    app: &App,
    dir: &Path,
    files: Vec<PathBuf>,
    local: &Path,
    nas: &Path,
    db: &Path,
) -> (Rc<Shared>, Rc<Worker>) {
    let (state, worker) = state_for(app, files);
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.window.write_sidecars = true;
        st.views.index_path = Some(db.to_path_buf());
        let file = dir.join("roots.json");
        let mut roots = greycard_library::Roots::default();
        roots.add(local).unwrap();
        roots.add(nas).unwrap();
        roots.set_archive(nas, true);
        roots.set_name(nas, "Archive");
        roots.save(&file).unwrap();
        st.views.library.roots = roots;
        st.views.library.file = Some(file);
        for f in st.files.clone() {
            if let Ok(Some(s)) = Sidecar::load_as_file(&f) {
                let i = st.files.iter().position(|x| *x == f).unwrap();
                st.sidecars[i] = s;
            }
        }
        crate::library::open_reader(&mut st, app);
    }
    (state, worker)
}

/// A window closed in a test that opens another over the same
/// index: its reader let go, since the app's callbacks keep the
/// state alive past the drop, and Windows will not remove a folder
/// holding an open database.
fn close(state: Rc<Shared>, worker: Rc<Worker>) {
    state.borrow_mut().views.index_reader = None;
    drop(worker);
    drop(state);
}

fn exposed(stops: f32) -> greycard_edit::Edit {
    let mut e = greycard_edit::Edit::default();
    e.light.exposure = stops;
    e
}

fn exposures_of(s: &Sidecar) -> Vec<f32> {
    s.history
        .iter()
        .map(|h| h.edit().light.exposure)
        .chain(std::iter::once(s.current().light.exposure))
        .collect()
}

fn no_beat() -> Beat {
    archive::no_beat()
}

fn pending_for(db: &Path, archive: &Path) -> Vec<greycard_library::Pending> {
    Library::open_read_only(db)
        .unwrap()
        .pending_for(archive)
        .unwrap()
}

fn save(state: &Rc<Shared>, stops: f32) {
    let mut st = state.borrow_mut();
    crate::panel::edit::save_edit(&mut st, exposed(stops));
}

/// The laptop saved the copy: on from what is there, at a time of
/// its own.
fn laptop_saves(copy: &Path, stops: f32) -> Sidecar {
    let mut theirs = Sidecar::load_as_file(copy).unwrap().unwrap_or_default();
    theirs.record(exposed(stops));
    theirs
        .save_in_stamped(
            copy,
            Placement::Beside,
            &greycard_edit::sync::Stamp {
                at: 1 << 40,
                host: "laptop".into(),
            },
        )
        .unwrap();
    theirs
}

#[test]
fn the_check_tells_our_write_from_another_machines_and_a_failed_stat() {
    let dir = scratch("check");
    let dest = dir.join("a.tif.gcd");
    assert_eq!(check(&dest, None), Check::Absent);
    std::fs::write(&dest, b"{}").unwrap();
    let meta = std::fs::metadata(&dest).unwrap();
    let ours = ArchiveWrite {
        path: dest.clone(),
        revision: "r".into(),
        size: meta.len(),
        mtime: greycard_library::mtime_of(&meta),
        written: 1,
    };
    assert_eq!(check(&dest, Some(&ours)), Check::Ours);
    assert_eq!(check(&dest, None), Check::Changed, "a file we never wrote");
    let other = ArchiveWrite {
        size: meta.len() + 1,
        ..ours.clone()
    };
    assert_eq!(check(&dest, Some(&other)), Check::Changed);
    let later = ArchiveWrite {
        mtime: ours.mtime + 1,
        ..ours
    };
    assert_eq!(check(&dest, Some(&later)), Check::Changed);
    // A stat that fails other than by the file being away is a
    // failure, not an absence: a file where a folder should be.
    // Windows reports a path under a file as not found, so there it
    // reads as away, and the write that follows fails instead.
    let under_file = dest.join("x.gcd");
    if cfg!(windows) {
        assert_eq!(check(&under_file, None), Check::Absent);
    } else {
        assert!(matches!(check(&under_file, None), Check::Failed(_)));
    }
    crate::testing::remove_dir_retry(&dir);
}

/// The write itself, off any window: a copy with no sidecar gets
/// ours and the write is recorded; the same write again is seen as
/// ours by its stat and goes; a copy another machine saved since
/// is read and compared, and comes back to settle when it is not
/// behind; an archive that does not answer leaves the frame waiting
/// in the index with its reason; the row is there before any
/// attempt.
#[test]
fn a_write_goes_to_the_copy_checks_what_stands_there_and_waits_when_it_cannot() {
    let dir = scratch("write-through");
    let (local, nas, files, db) = two_places(&dir);
    let frame = files[0].clone();
    let copy = nas.join("shoot").join("a.tif");
    let mut sidecar = Sidecar::default();
    sidecar.record(exposed(0.5));
    sidecar.save(&frame).unwrap();
    let hash = Library::open_read_only(&db)
        .unwrap()
        .by_path(&frame)
        .unwrap()
        .unwrap()
        .hash;
    let write = |s: &Sidecar| Write {
        frame: frame.clone(),
        hash: hash.clone(),
        source: Source::Memory(Box::new(s.clone())),
        placement: Placement::Beside,
        xmp: false,
        copies: vec![Copy {
            archive: nas.clone(),
            copy: copy.clone(),
        }],
    };
    let out = write_through(&write(&sidecar), &db, &no_beat());
    assert!(matches!(out[..], [Outcome::Written { .. }]), "{out:?}");
    let there = Sidecar::load_as_file(&copy).unwrap().unwrap();
    assert_eq!(compare(&sidecar, &there), Compared::Same);
    let lib = Library::open_read_only(&db).unwrap();
    let w = lib.archive_write(&hash, &nas, &copy).unwrap().unwrap();
    assert_eq!(w.revision, sidecar.revisions.last().unwrap().hash);
    assert!(lib.pending_for(&nas).unwrap().is_empty());
    drop(lib);
    sidecar.record(exposed(1.0));
    sidecar.save(&frame).unwrap();
    let out = write_through(&write(&sidecar), &db, &no_beat());
    assert!(matches!(out[..], [Outcome::Written { .. }]), "{out:?}");
    assert_eq!(
        Sidecar::load_as_file(&copy)
            .unwrap()
            .unwrap()
            .current()
            .light
            .exposure,
        1.0
    );
    // Another machine saved the copy since: handed back to settle.
    laptop_saves(&copy, 2.0);
    let out = write_through(&write(&sidecar), &db, &no_beat());
    match &out[..] {
        [Outcome::Take { theirs, .. }] => assert_eq!(theirs.current().light.exposure, 2.0),
        other => panic!("{other:?}"),
    }
    let p = pending_for(&db, &nas);
    assert_eq!((p.len(), p[0].reason.as_str()), (1, "to join"));
    assert_eq!(
        Sidecar::load_as_file(&copy)
            .unwrap()
            .unwrap()
            .current()
            .light
            .exposure,
        2.0
    );
    // The archive away: waiting, with the reason, the row counted.
    crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
    let out = write_through(&write(&sidecar), &db, &no_beat());
    assert!(
        matches!(&out[..], [Outcome::Waiting { reason, .. }] if reason == "offline"),
        "{out:?}"
    );
    crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
    let p = pending_for(&db, &nas);
    assert_eq!(
        (p.len(), p[0].reason.as_str(), p[0].tries),
        (1, "offline", 2)
    );
    // A sidecar with no revision is refused.
    let mut old = Sidecar::default();
    old.record(exposed(9.0));
    let out = write_through(&write(&old), &db, &no_beat());
    assert!(
        matches!(&out[..], [Outcome::Waiting { reason, .. }] if reason.starts_with("no revision")),
        "{out:?}"
    );
    let _ = local;
    crate::testing::remove_dir_retry(&dir);
}

/// Through the window: an edit saved here is written through to
/// the archive's copy off the window's thread, and the two sidecars
/// are the same file; a burst of saves is one write; the XMP beside
/// the copy says what the sidecar says, when XMPs are on.
#[test]
fn an_edit_saved_here_is_written_through_and_both_sidecars_are_equal() {
    let dir = scratch("window");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    state.borrow_mut().window.xmp_sidecars = true;
    state.borrow_mut().sidecars[0].meta.rating = 3;
    save(&state, 0.75);
    let here = Sidecar::load_as_file(&files[0]).unwrap().unwrap();
    assert_eq!(here.current().light.exposure, 0.75);
    save(&state, 0.8);
    save(&state, 0.9);
    {
        let st = state.borrow();
        assert!(st.work.sync.in_flight.contains(&files[0]));
        assert_eq!(st.work.sync.queued.len(), 1);
    }
    let landed = land_sent(&state, &app, &worker);
    assert_eq!(landed, 2, "the first write, then the latest");
    let copy = nas.join("shoot").join("a.tif");
    let here = Sidecar::load_as_file(&files[0]).unwrap().unwrap();
    let there = Sidecar::load_as_file(&copy).unwrap().unwrap();
    assert_eq!(there.current().light.exposure, 0.9);
    assert_eq!(compare(&here, &there), Compared::Same);
    assert_eq!(here, there);
    assert!(
        !greycard_edit_io::xmp::paths_of(&copy).is_empty(),
        "the copy's XMP"
    );
    assert!(state.borrow().work.sync.queued.is_empty());
    assert!(state.borrow().work.sync.in_flight.is_empty());
    assert_eq!(app.get_sync_note(), "");
    assert!(pending_for(&db, &nas).is_empty());
    // XMPs off: none written beside the copy.
    let dir2 = scratch("window-noxmp");
    let (local2, nas2, files2, db2) = two_places(&dir2);
    let app2 = window(2);
    let (state2, worker2) = opened(&app2, &dir2, files2.clone(), &local2, &nas2, &db2);
    state2.borrow_mut().sidecars[0].meta.rating = 3;
    save(&state2, 0.5);
    land_sent(&state2, &app2, &worker2);
    assert!(greycard_edit_io::xmp::paths_of(&nas2.join("shoot").join("a.tif")).is_empty());
    crate::testing::remove_scratch(state2, &dir2);
    crate::testing::remove_scratch(state, &dir);
}

/// The archive away: the save is kept here, the write waits in the
/// index and the status line says so; the window closed and opened
/// again says it from the index at once; the archive back, the
/// catch-up writes it and the note goes.
#[test]
fn with_the_archive_away_the_write_waits_and_the_catch_up_converges() {
    let dir = scratch("catch-up");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
    save(&state, 0.6);
    land_sent(&state, &app, &worker);
    let copy = nas.join("shoot").join("a.tif");
    assert!(
        Sidecar::load_as_file(&copy).unwrap().is_none(),
        "nothing written while away"
    );
    assert_eq!(pending_for(&db, &nas).len(), 1);
    assert_eq!(app.get_sync_note(), "1 edit waiting for Archive");
    {
        let mut st = state.borrow_mut();
        st.current = Some(1);
    }
    save(&state, -0.6);
    land_sent(&state, &app, &worker);
    assert_eq!(app.get_sync_note(), "2 edits waiting for Archive");
    // The window closes and opens again: the launch says it.
    close(state, worker);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    assert_eq!(app.get_sync_note(), "2 edits waiting for Archive");
    // The archive answers a look: the catch-up runs, one job a frame.
    crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
    {
        let mut st = state.borrow_mut();
        heard_from(&mut st, &app, &nas, true);
        assert_eq!(st.work.sync.in_flight.len(), 2);
    }
    land_sent(&state, &app, &worker);
    let here = Sidecar::load_as_file(&files[0]).unwrap().unwrap();
    let there = Sidecar::load_as_file(&copy).unwrap().unwrap();
    assert_eq!(compare(&here, &there), Compared::Same);
    let b_copy = nas.join("shoot").join("b.tif");
    assert_eq!(
        Sidecar::load_as_file(&b_copy)
            .unwrap()
            .unwrap()
            .current()
            .light
            .exposure,
        -0.6
    );
    assert!(pending_for(&db, &nas).is_empty());
    assert_eq!(app.get_sync_note(), "");
    {
        let mut st = state.borrow_mut();
        heard_from(&mut st, &app, &nas, true);
    }
    assert_eq!(land_sent(&state, &app, &worker), 0);
    crate::testing::remove_scratch(state, &dir);
}

/// A copy another machine went on from while ours was saved too:
/// the write finds it not ours and not behind, hands it back, the
/// window joins the two against what it holds now, saves, and the
/// write goes again; both copies end the same file, the laptop's
/// branch kept.
#[test]
fn a_copy_saved_elsewhere_is_joined_here_and_written_back() {
    let dir = scratch("join");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    save(&state, 0.5);
    land_sent(&state, &app, &worker);
    let copy = nas.join("shoot").join("a.tif");
    laptop_saves(&copy, -1.0);
    save(&state, 2.0);
    land_sent(&state, &app, &worker);
    let here = Sidecar::load_as_file(&files[0]).unwrap().unwrap();
    let there = Sidecar::load_as_file(&copy).unwrap().unwrap();
    assert_eq!(compare(&here, &there), Compared::Same, "{here:?}");
    let exposures = exposures_of(&here);
    assert!(
        exposures.contains(&-1.0) && exposures.contains(&2.0),
        "{exposures:?}"
    );
    assert!(
        here.current_label()
            .is_some_and(|l| l.starts_with("Reconciled"))
    );
    assert_eq!(state.borrow().sidecars[0], here);
    assert!(pending_for(&db, &nas).is_empty());
    crate::testing::remove_scratch(state, &dir);
}

/// The archive's copy saved by a newer greycard (§724): the sync
/// writes neither copy, the newer one left as it is and the reason
/// noted; the save here is the window's own.
#[test]
fn a_copy_a_newer_build_saved_is_left_and_so_is_ours() {
    let dir = scratch("newer");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    save(&state, 0.5);
    land_sent(&state, &app, &worker);
    let copy = nas.join("shoot").join("a.tif");
    let path = Sidecar::find(&copy).unwrap();
    let newer = r#"{"floors": {"read": 10, "write": 11}, "current": {"version": 11, "light": {"exposure": -1.0}}, "saved": 99}"#;
    std::fs::write(&path, newer).unwrap();
    save(&state, 2.0);
    land_sent(&state, &app, &worker);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
    let here = Sidecar::load_as_file(&files[0]).unwrap().unwrap();
    assert_eq!(here.current().light.exposure, 2.0);
    assert!(!here.read_only());
    let pending = pending_for(&db, &nas);
    assert!(
        pending.iter().any(|p| p.reason.contains("read-only")),
        "{pending:?}"
    );
    crate::testing::remove_scratch(state, &dir);
}

/// A version made here and deleted on the laptop after it had seen
/// every save of ours, nothing saved here since: the copy lands as
/// ours behind it and is taken whole, the version's cell goes once
/// the landing is done with its numbers, the file's own cell takes
/// the screen, and nothing is written back to the copy. A save here
/// meanwhile would have kept it instead, which is the join's rule
/// (a version edited on one copy and deleted on the other stays).
#[test]
fn a_copy_without_the_version_takes_its_cell_and_writes_nothing_back() {
    let dir = scratch("join-version");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    let id = {
        let mut st = state.borrow_mut();
        let id = st.sidecars[0].new_version(None, Some("Warm")).unwrap();
        crate::versions::settle(&mut st, &app, &worker);
        id
    };
    save(&state, 0.5);
    land_sent(&state, &app, &worker);
    let copy = nas.join("shoot").join("a.tif");
    assert!(
        Sidecar::load_as_file(&copy)
            .unwrap()
            .unwrap()
            .version(&id)
            .is_some(),
        "the copy has Warm"
    );
    // The laptop, with every save of ours in hand, deletes Warm and
    // saves on.
    let mut theirs = Sidecar::load_as_file(&copy).unwrap().unwrap();
    theirs.delete_version(&id).unwrap();
    theirs.record(exposed(-1.0));
    theirs
        .save_in_stamped(
            &copy,
            Placement::Beside,
            &greycard_edit::sync::Stamp {
                at: 1 << 40,
                host: "laptop".into(),
            },
        )
        .unwrap();
    let theirs = Sidecar::load_as_file(&copy).unwrap().unwrap();
    // The version's cell on screen when the copy lands.
    {
        let mut st = state.borrow_mut();
        assert_eq!(st.cells.len(), 3);
        st.current = Some(1);
    }
    settle(&state, &app, &worker, &files[0], &nas, &copy, theirs);
    land_sent(&state, &app, &worker);
    {
        let mut st = state.borrow_mut();
        assert!(
            st.sidecars[0].version(&id).is_none(),
            "the copy taken whole"
        );
        assert!(crate::versions::settle_pending(&mut st, &app, &worker) >= 1);
        let cells: Vec<usize> = st.cells.iter().map(|c| c.file).collect();
        assert_eq!(cells, [0, 1], "its cell went");
        assert_eq!(st.current, Some(0), "the file's own cell in its place");
    }
    let here = Sidecar::load_as_file(&files[0]).unwrap().unwrap();
    let there = Sidecar::load_as_file(&copy).unwrap().unwrap();
    assert!(here.version(&id).is_none());
    assert!(there.version(&id).is_none(), "nothing written back");
    assert_eq!(compare(&here, &there), Compared::Same);
    assert!(pending_for(&db, &nas).is_empty());
    crate::testing::remove_scratch(state, &dir);
}

/// The archive's copy ahead of the window's while another writer also
/// saved the local sidecar (the command line's preset): the copy is not
/// written over the local file unseen; that file is kept beside the
/// frame, the copy written, in one save, the frame on screen shown.
#[test]
fn a_copy_ahead_keeps_a_local_file_another_writer_saved() {
    let dir = scratch("take-local-changed");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    save(&state, 0.5);
    land_sent(&state, &app, &worker);
    let copy = nas.join("shoot").join("a.tif");
    let theirs = laptop_saves(&copy, -1.0);
    // The other writer here, a minute back.
    let mut cli = Sidecar::load_as_file(&files[0]).unwrap().unwrap();
    let mut grain = cli.current().clone();
    grain.grain.amount = 0.4;
    cli.record_as(grain, Some("Preset: Fine Grain".into()));
    cli.meta.rating = 4;
    let mut at = greycard_edit::sync::Stamp::now();
    at.at -= 60;
    cli.save_in_stamped(&files[0], Placement::Beside, &at)
        .unwrap();
    let before = greycard_edit::synced_writes();
    settle(&state, &app, &worker, &files[0], &nas, &copy, theirs);
    assert_eq!(
        greycard_edit::synced_writes() - before,
        2,
        "one save here, and the other writer's file kept beside it"
    );
    {
        let mut st = state.borrow_mut();
        assert!(crate::panel::history::show_settled_pending(&mut st, &app, &worker) >= 1);
    }
    land_sent(&state, &app, &worker);
    let here = Sidecar::load_as_file(&files[0]).unwrap().unwrap();
    assert!(exposures_of(&here).contains(&-1.0));
    assert_eq!(state.borrow().sidecars[0], here);
    let kept =
        Sidecar::read_as_file(&Sidecar::path_for(&files[0]).with_extension("gcd.other")).unwrap();
    assert_eq!(kept.meta.rating, 4);
    assert!(kept.current().grain.amount == 0.4);
    let there = Sidecar::load_as_file(&copy).unwrap().unwrap();
    assert_eq!(compare(&here, &there), Compared::Same, "written through");
    crate::testing::remove_scratch(state, &dir);
}

/// Probe D: a copy handed back as behind what the job carried lands
/// after the window saved again. The landing compares with what the
/// window holds now, so the newer saves stand.
#[test]
fn a_take_landing_after_newer_saves_keeps_them() {
    let dir = scratch("stale-take");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    save(&state, 0.5);
    land_sent(&state, &app, &worker);
    let copy = nas.join("shoot").join("a.tif");
    let theirs = Sidecar::load_as_file(&copy).unwrap().unwrap();
    save(&state, 0.8);
    save(&state, 0.9);
    // The landing of a job that compared the 0.5 snapshot and found
    // ours behind the copy's.
    settle(&state, &app, &worker, &files[0], &nas, &copy, theirs);
    land_sent(&state, &app, &worker);
    let here = Sidecar::load_as_file(&files[0]).unwrap().unwrap();
    assert!(
        exposures_of(&here).contains(&0.9),
        "{:?}",
        exposures_of(&here)
    );
    assert_eq!(here.current().light.exposure, 0.9);
    let there = Sidecar::load_as_file(&copy).unwrap().unwrap();
    assert_eq!(compare(&here, &there), Compared::Same);
    crate::testing::remove_scratch(state, &dir);
}

/// Probe B: a write set aside (the share went quiet) and the window
/// quit before it said anything: the frame is pending, from the row
/// the job made before it tried; opened again, the catch-up takes
/// it.
#[test]
fn a_quit_with_a_write_out_leaves_it_pending_and_the_next_launch_catches_up() {
    let dir = scratch("quit");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
    ASIDE_NEXT.with(|a| a.set(true));
    save(&state, 0.5);
    save(&state, 0.6);
    // Heard as set aside; its own end never comes before the quit.
    let first: Vec<Sent> = SENT.with(|s| s.borrow_mut().drain(..1).collect());
    for j in first {
        j(&state, &app, &worker);
    }
    // The job itself, still out: it ran its first step, the row.
    let out: Vec<Sent> = SENT.with(|s| s.borrow_mut().drain(..).collect());
    for j in out {
        j(&state, &app, &worker);
    }
    SENT.with(|s| s.borrow_mut().clear());
    let p = pending_for(&db, &nas);
    assert_eq!(
        p.len(),
        1,
        "the save that never reached the archive is pending"
    );
    close(state, worker);
    // Opened again, the archive answering.
    crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    assert_eq!(app.get_sync_note(), "1 edit waiting for Archive");
    heard_from(&mut state.borrow_mut(), &app, &nas, true);
    land_sent(&state, &app, &worker);
    let copy = nas.join("shoot").join("a.tif");
    assert_eq!(
        Sidecar::load_as_file(&copy)
            .unwrap()
            .unwrap()
            .current()
            .light
            .exposure,
        0.6
    );
    assert!(pending_for(&db, &nas).is_empty());
    crate::testing::remove_scratch(state, &dir);
}

/// Probe E: a write set aside goes on alone; the next save queues
/// behind it rather than running beside it; when the first lands,
/// late, the second goes, and the archive ends at the newest save.
#[test]
fn a_write_set_aside_keeps_the_frame_in_flight_and_the_newer_one_goes_after() {
    let dir = scratch("aside");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    ASIDE_NEXT.with(|a| a.set(true));
    save(&state, 0.5);
    let first: Vec<Sent> = SENT.with(|s| s.borrow_mut().drain(..).collect());
    for j in first {
        j(&state, &app, &worker);
    }
    let late: Vec<Sent> = SENT.with(|s| s.borrow_mut().drain(..).collect());
    assert_eq!(late.len(), 1);
    assert!(
        state.borrow().work.sync.in_flight.contains(&files[0]),
        "still out"
    );
    save(&state, 0.9);
    assert_eq!(
        state.borrow().work.sync.in_flight.len(),
        1,
        "no second job beside it"
    );
    assert_eq!(land_sent(&state, &app, &worker), 0, "nothing new sent");
    for j in late {
        j(&state, &app, &worker);
    }
    land_sent(&state, &app, &worker);
    let copy = nas.join("shoot").join("a.tif");
    let there = Sidecar::load_as_file(&copy).unwrap().unwrap();
    let here = Sidecar::load_as_file(&files[0]).unwrap().unwrap();
    assert_eq!(there.current().light.exposure, 0.9);
    assert_eq!(compare(&here, &there), Compared::Same);
    assert!(pending_for(&db, &nas).is_empty());
    assert_eq!(app.get_sync_note(), "");
    crate::testing::remove_scratch(state, &dir);
}

/// Probe C: one raw in two folders (a shoot and its selects), both
/// backed up; each local frame's edit goes to its own copy and
/// nowhere else; with no pairing to say which copy is whose,
/// nothing is written and the row says why.
#[test]
fn two_frames_with_one_hash_keep_their_own_edits() {
    let dir = scratch("two-frames");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    let shoot = local.join("shoot");
    let selects = local.join("selects");
    std::fs::create_dir_all(&shoot).unwrap();
    std::fs::create_dir_all(&selects).unwrap();
    let a = shoot.join("a.tif");
    write_frame(&a, &R5, 1);
    let a2 = selects.join("a.tif");
    std::fs::copy(&a, &a2).unwrap();
    for sub in ["shoot", "selects"] {
        std::fs::create_dir_all(nas.join(sub)).unwrap();
        std::fs::copy(&a, nas.join(sub).join("a.tif")).unwrap();
    }
    let db = dir.join("library.sqlite");
    {
        let mut lib = Library::open(&db).unwrap();
        lib.index_tree(&local, &mut |_| {}).unwrap();
        lib.index_tree(&nas, &mut |_| {}).unwrap();
    }
    let app = window(2);
    let (state, worker) = opened(&app, &dir, vec![a.clone(), a2.clone()], &local, &nas, &db);
    // No pairing yet: nothing written, the row says so.
    save(&state, 1.0);
    land_sent(&state, &app, &worker);
    let p = pending_for(&db, &nas);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].copy.as_os_str().is_empty());
    assert!(
        p[0].reason.contains("more than one copy"),
        "{}",
        p[0].reason
    );
    assert!(
        Sidecar::load_as_file(&nas.join("shoot").join("a.tif"))
            .unwrap()
            .is_none()
    );
    // The pairings say which is whose.
    {
        let mut st = state.borrow_mut();
        st.views
            .library
            .roots
            .set_backup_folder(&shoot, &nas, &nas.join("shoot"));
        st.views
            .library
            .roots
            .set_backup_folder(&selects, &nas, &nas.join("selects"));
        st.current = Some(1);
    }
    save(&state, -2.0);
    land_sent(&state, &app, &worker);
    state.borrow_mut().current = Some(0);
    save(&state, 1.5);
    land_sent(&state, &app, &worker);
    let show = |p: &Path| {
        Sidecar::load_as_file(p)
            .unwrap()
            .map(|s| exposures_of(&s))
            .unwrap_or_default()
    };
    assert!(
        !show(&a).contains(&-2.0),
        "the selects edit joined into the shoot's frame"
    );
    assert!(!show(&nas.join("selects").join("a.tif")).contains(&1.5));
    assert_eq!(show(&nas.join("shoot").join("a.tif")), show(&a));
    assert_eq!(show(&nas.join("selects").join("a.tif")), show(&a2));
    // The earlier row, re-paired by the catch-up, goes.
    heard_from(&mut state.borrow_mut(), &app, &nas, true);
    land_sent(&state, &app, &worker);
    assert!(
        pending_for(&db, &nas).is_empty(),
        "{:?}",
        pending_for(&db, &nas)
    );
    crate::testing::remove_scratch(state, &dir);
}

/// A catch-up and a save for one frame at once: the catch-up queues
/// behind the save's write, so the two never run beside each other,
/// and the archive ends at the save.
#[test]
fn a_catch_up_racing_a_save_queues_behind_it() {
    let dir = scratch("race");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
    save(&state, 0.3);
    land_sent(&state, &app, &worker);
    assert_eq!(pending_for(&db, &nas).len(), 1);
    crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
    // A save sends its write; the archive's answer arrives while it
    // is out.
    save(&state, 0.4);
    heard_from(&mut state.borrow_mut(), &app, &nas, true);
    assert_eq!(
        state.borrow().work.sync.in_flight.len(),
        1,
        "one job for the frame"
    );
    land_sent(&state, &app, &worker);
    let copy = nas.join("shoot").join("a.tif");
    assert_eq!(
        Sidecar::load_as_file(&copy)
            .unwrap()
            .unwrap()
            .current()
            .light
            .exposure,
        0.4
    );
    assert!(pending_for(&db, &nas).is_empty());
    crate::testing::remove_scratch(state, &dir);
}

/// Probe A, and the follow: the frame on screen follows to the
/// archive's copy when its root goes, after the copy is checked on
/// a job; a copy another machine saved first is settled, and the
/// frame follows after, so the next save never writes over it; with
/// a write waiting the frame stays.
#[test]
fn the_frame_on_screen_follows_to_the_archives_copy_once_the_copy_is_settled() {
    let dir = scratch("follow");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    save(&state, 0.5);
    land_sent(&state, &app, &worker);
    let copy = nas.join("shoot").join("a.tif");
    // The laptop saved the copy; the local disk is unplugged (probe
    // K: really gone, so the take and the save here go nowhere).
    laptop_saves(&copy, -1.0);
    let away = dir.join("local-away");
    greycard_core::testing::rename_retry(&local, &away);
    {
        let mut st = state.borrow_mut();
        // Open before its root went: the worker holds it by the
        // local path.
        st.develop.open_sent = Some((st.develop.generation, (files[0].clone(), None)));
        st.views.library.offline.insert(local.clone());
        follow_offline(&mut st, &app);
        assert!(st.work.sync.checking.contains(&files[0]));
        assert_eq!(st.files[0], files[0], "not followed before the check");
    }
    land_sent(&state, &app, &worker);
    // The check found the laptop's save: taken into the window,
    // written to the copy (the same, recorded), and the frame
    // followed once that landed.
    let there = Sidecar::load_as_file(&copy).unwrap().unwrap();
    assert!(
        exposures_of(&there).contains(&-1.0),
        "{:?}",
        exposures_of(&there)
    );
    assert_eq!(state.borrow().files[0], copy, "followed after settling");
    assert_eq!(
        state
            .borrow()
            .develop
            .open_sent
            .as_ref()
            .map(|(g, (p, _))| (*g, p.clone())),
        Some((state.borrow().develop.generation, copy.clone())),
        "opened again from the copy"
    );
    let status = app.get_status().to_string();
    assert!(
        status.contains("is offline") && status.contains("Archive"),
        "{status}"
    );
    // The window holds the copy's, and one revision more: the save
    // it tried here advanced it before the write failed. Not behind
    // the copy, so nothing of the laptop's is lost at the next save.
    assert!(matches!(
        compare(&state.borrow().sidecars[0], &there),
        Compared::Same | Compared::Behind(Side::Other)
    ));
    assert!(
        pending_for(&db, &nas).is_empty(),
        "{:?}",
        pending_for(&db, &nas)
    );
    // The next save goes to the copy, through the check, and the
    // laptop's branch stays.
    save(&state, 0.7);
    land_sent(&state, &app, &worker);
    let there = Sidecar::load_as_file(&copy).unwrap().unwrap();
    assert_eq!(there.current().light.exposure, 0.7);
    assert!(exposures_of(&there).contains(&-1.0));
    assert_eq!(compare(&state.borrow().sidecars[0], &there), Compared::Same);
    assert!(pending_for(&db, &nas).is_empty());
    assert_eq!(app.get_sync_note(), "");
    greycard_core::testing::rename_retry(&away, &local);
    assert_eq!(
        Sidecar::load_as_file(&files[0])
            .unwrap()
            .unwrap()
            .current()
            .light
            .exposure,
        0.5,
        "the file here, away through it all, is as it was"
    );
    // The root back: the sidecar here is brought up to the copy's,
    // off the window's thread, and the frame goes on following.
    {
        let mut st = state.borrow_mut();
        st.views.library.offline.clear();
        heard_over(&mut st, &app, &HashSet::new());
        assert!(
            st.work.sync.in_flight.contains(&copy),
            "one job, through the frame's queue"
        );
    }
    land_sent(&state, &app, &worker);
    let home = Sidecar::load_as_file(&files[0]).unwrap().unwrap();
    assert_eq!(home.current().light.exposure, 0.7);
    assert!(exposures_of(&home).contains(&-1.0));
    assert_eq!(compare(&home, &state.borrow().sidecars[0]), Compared::Same);
    assert_eq!(state.borrow().files[0], copy, "still followed");
    // Once a return: another look queues nothing.
    heard_over(&mut state.borrow_mut(), &app, &HashSet::new());
    assert_eq!(land_sent(&state, &app, &worker), 0);
    // With a write waiting, the frame stays.
    let app2 = window(2);
    let (state2, worker2) = opened(&app2, &dir, files.clone(), &local, &nas, &db);
    crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
    state2.borrow_mut().current = Some(1);
    save(&state2, 0.1);
    land_sent(&state2, &app2, &worker2);
    crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
    {
        let mut st = state2.borrow_mut();
        st.views.library.offline.insert(local.clone());
        follow_offline(&mut st, &app2);
        assert!(!st.work.sync.checking.contains(&files[1]));
        assert_eq!(st.files[1], files[1], "a write waits: it stays");
    }
    close(state2, worker2);
    crate::testing::remove_scratch(state, &dir);
}

/// A save made with no window open leaves the frame waiting in the
/// index, and the next window's catch-up writes it; a frame under
/// the archive, or with no copy, leaves nothing.
#[test]
fn a_save_with_no_window_open_is_noted_and_the_next_window_writes_it() {
    let dir = scratch("no-window");
    let (local, nas, files, db) = two_places(&dir);
    let mut roots = greycard_library::Roots::default();
    roots.add(&local).unwrap();
    roots.add(&nas).unwrap();
    roots.set_archive(&nas, true);
    let mut s = Sidecar::default();
    s.record(exposed(3.0));
    s.save(&files[1]).unwrap();
    assert_eq!(note_disk_save(&db, &roots, &files[1]), 1);
    assert_eq!(
        note_disk_save(&db, &roots, &nas.join("shoot").join("b.tif")),
        0
    );
    assert_eq!(
        note_disk_save(&dir.join("none.sqlite"), &roots, &files[1]),
        0
    );
    assert!(!dir.join("none.sqlite").exists());
    let p = pending_for(&db, &nas);
    assert_eq!(
        (p.len(), p[0].reason.as_str()),
        (1, "saved with no window open")
    );
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    assert_eq!(app.get_sync_note(), "1 edit waiting for Archive");
    heard_from(&mut state.borrow_mut(), &app, &nas, true);
    land_sent(&state, &app, &worker);
    let copy = nas.join("shoot").join("b.tif");
    assert_eq!(
        Sidecar::load_as_file(&copy)
            .unwrap()
            .unwrap()
            .current()
            .light
            .exposure,
        3.0
    );
    assert!(pending_for(&db, &nas).is_empty());
    assert_eq!(app.get_sync_note(), "");
    crate::testing::remove_scratch(state, &dir);
}

/// The quit's own save comes after the index reader is closed: its
/// archive write is noted waiting, not dropped, and the next
/// window's catch-up writes it. A write queued behind one still out
/// at the quit is noted the same way.
#[test]
fn a_save_at_the_quit_is_noted_and_the_next_window_writes_it() {
    let dir = scratch("at-quit");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    // Frame b: one write out, a newer one queued behind it, the
    // quit before either lands.
    state.borrow_mut().current = Some(1);
    save(&state, 0.2);
    save(&state, 0.3);
    assert_eq!(state.borrow().work.sync.queued.len(), 1);
    // Frame a: saved by the quit, the reader closed first, as
    // `startup::main` closes it.
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.views.index_reader = None;
    }
    save(&state, 0.7);
    assert_eq!(state.borrow().work.sync.deferred.len(), 1);
    assert_eq!(note_unsent_at_quit(&mut state.borrow_mut()), 2);
    {
        let st = state.borrow();
        assert!(st.work.sync.deferred.is_empty() && st.work.sync.queued.is_empty());
    }
    let mut p = pending_for(&db, &nas);
    p.sort_by(|x, y| x.frame.cmp(&y.frame));
    assert_eq!(p.len(), 2);
    assert!(p.iter().any(|r| r.reason == "saved as the window closed"));
    close(state, worker);
    // The next window: both frames reach the archive, each at its
    // last save.
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    heard_from(&mut state.borrow_mut(), &app, &nas, true);
    land_sent(&state, &app, &worker);
    let exposure = |name: &str| {
        Sidecar::load_as_file(&nas.join("shoot").join(name))
            .unwrap()
            .unwrap()
            .current()
            .light
            .exposure
    };
    assert_eq!((exposure("a.tif"), exposure("b.tif")), (0.7, 0.3));
    assert!(pending_for(&db, &nas).is_empty());
    crate::testing::remove_scratch(state, &dir);
}

/// A frame followed onto its archive copy while its own root is
/// away has no file here: the quit's save of it, made after the
/// reader closed, sent no job and was lost. Through the quit's own
/// saves (`startup::save_at_quit`): the save reaches the copy within
/// the wait; a copy saved elsewhere since is joined, not written
/// over; a job of the frame's still out goes first and the quit's
/// write after it, never beside it; and a write that has not ended
/// in the wait is noted waiting, which the next window settles.
#[test]
fn a_followed_frames_save_at_the_quit_reaches_its_copy() {
    use crate::panel::startup::save_at_quit;
    let dir = scratch("followed-quit");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    save(&state, 0.5);
    land_sent(&state, &app, &worker);
    // The local disk is unplugged, and the frame on screen follows
    // its copy.
    let copy = nas.join("shoot").join("a.tif");
    let away = dir.join("local-away");
    greycard_core::testing::rename_retry(&local, &away);
    {
        let mut st = state.borrow_mut();
        st.views.library.offline.insert(local.clone());
        follow_offline(&mut st, &app);
    }
    land_sent(&state, &app, &worker);
    assert_eq!(state.borrow().files[0], copy, "followed");
    save(&state, 0.7);
    land_sent(&state, &app, &worker);
    let on_copy = || Sidecar::load_as_file(&copy).unwrap().unwrap();
    assert_eq!(on_copy().current().light.exposure, 0.7);
    let recorded = || {
        let lib = Library::open_read_only(&db).unwrap();
        let hash = lib.by_path(&copy).unwrap().unwrap().hash;
        lib.archive_write(&hash, &nas, &copy)
            .unwrap()
            .map(|w| w.revision)
    };
    let latest = |state: &Rc<Shared>| {
        state.borrow().sidecars[0]
            .revisions
            .last()
            .map(|r| r.hash.clone())
    };
    // The quit: the reader closed, as `startup::main` closes it,
    // then its saves.
    state.borrow_mut().views.index_reader = None;
    assert_eq!(
        save_at_quit(&mut state.borrow_mut(), exposed(0.9), QUIT_WRITE_WAIT),
        QuitWrites { ended: 1, noted: 0 }
    );
    assert!(SENT.with(|s| s.borrow().is_empty()), "no job: written");
    assert_eq!(on_copy().current().light.exposure, 0.9);
    assert_eq!(
        compare(&state.borrow().sidecars[0], &on_copy()),
        Compared::Same
    );
    assert!(pending_for(&db, &nas).is_empty());
    assert_eq!(recorded(), latest(&state), "recorded, as a job's write is");
    // Recorded, it is not written again.
    assert_eq!(
        save_at_quit(&mut state.borrow_mut(), exposed(0.9), QUIT_WRITE_WAIT),
        QuitWrites::default()
    );
    // A save's job still out at the quit: the quit's write waits
    // for it and does not go beside it. Here the job cannot run
    // until the test lands it, so the wait runs out first: the row
    // is noted and the copy is untouched.
    crate::library::open_reader(&mut state.borrow_mut(), &app);
    save(&state, 1.2);
    assert!(state.borrow().work.sync.in_flight.contains(&copy));
    state.borrow_mut().views.index_reader = None;
    let before = std::fs::read(copy_sidecar(&copy, Placement::Beside)).unwrap();
    assert_eq!(
        save_at_quit(
            &mut state.borrow_mut(),
            exposed(1.4),
            Duration::from_millis(200)
        ),
        QuitWrites { ended: 0, noted: 1 }
    );
    assert_eq!(
        std::fs::read(copy_sidecar(&copy, Placement::Beside)).unwrap(),
        before,
        "nothing written beside the job"
    );
    let p = pending_for(&db, &nas);
    assert_eq!(
        p.iter()
            .map(|r| (r.frame.as_path(), r.reason.as_str()))
            .collect::<Vec<_>>(),
        vec![(copy.as_path(), "saved as the window closed")]
    );
    // The older save's job ends; the quit's write goes after it, so
    // the copy ends on the newer save and it is recorded.
    land_sent(&state, &app, &worker);
    let start = std::time::Instant::now();
    while recorded() != latest(&state) {
        assert!(start.elapsed() < Duration::from_secs(20), "never written");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(on_copy().current().light.exposure, 1.4);
    assert!(exposures_of(&on_copy()).contains(&1.2));
    assert!(pending_for(&db, &nas).is_empty());
    // Another machine saved the copy meanwhile: the quit's save is
    // joined with it, and neither edit is lost.
    laptop_saves(&copy, -2.0);
    assert_eq!(
        save_at_quit(&mut state.borrow_mut(), exposed(1.6), QUIT_WRITE_WAIT),
        QuitWrites { ended: 1, noted: 0 }
    );
    let joined = exposures_of(&on_copy());
    assert!(
        joined.contains(&1.6) && joined.contains(&-2.0),
        "{joined:?}"
    );
    assert!(pending_for(&db, &nas).is_empty());
    assert_eq!(
        recorded(),
        on_copy().revisions.last().map(|r| r.hash.clone()),
        "the join recorded"
    );
    // A write that has not ended in the wait, the process ending
    // under it: noted waiting.
    fn never_lands(_: &Write, _: &Path) -> Vec<Outcome> {
        std::thread::sleep(Duration::from_secs(2));
        Vec::new()
    }
    let before = std::fs::read(copy_sidecar(&copy, Placement::Beside)).unwrap();
    save(&state, 1.3);
    assert_eq!(
        write_followed_with(&mut state.borrow_mut(), Duration::ZERO, never_lands),
        QuitWrites { ended: 0, noted: 1 }
    );
    let p = pending_for(&db, &nas);
    assert_eq!(
        p.iter()
            .map(|r| (r.frame.as_path(), r.reason.as_str()))
            .collect::<Vec<_>>(),
        vec![(copy.as_path(), "saved as the window closed")]
    );
    close(state, worker);
    // The next window says it waits, and its catch-up settles the
    // row against what stands on the copy, which is whole.
    greycard_core::testing::rename_retry(&away, &local);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    assert_eq!(app.get_sync_note(), "1 edit waiting for Archive");
    heard_from(&mut state.borrow_mut(), &app, &nas, true);
    land_sent(&state, &app, &worker);
    assert!(pending_for(&db, &nas).is_empty());
    assert_eq!(
        std::fs::read(copy_sidecar(&copy, Placement::Beside)).unwrap(),
        before
    );
    crate::testing::remove_scratch(state, &dir);
}

/// Frame `c`'s edit on the panel, as an open shows it, and into
/// culling from there, staying on a key rather than moving on.
fn cull_from(state: &Rc<Shared>, app: &App, c: usize) {
    let mut st = state.borrow_mut();
    st.current = Some(c);
    let edit = st.sidecars[c].current().clone();
    crate::panel::edit::show_edit(&st, &edit, app, None);
    st.develop.edit = edit;
    st.culling.cull_move_on = false;
    crate::panel::cull::enter_cull(&mut st, app, 1);
    assert!(st.culling.cull.is_some());
}

/// A quit from culling records nothing from the panel, which is
/// no frame's there, and still makes the rest of its saves: a
/// followed frame's last save, a culling key's whose job is still
/// out, waits for that job, is noted waiting when it has not ended
/// in the wait, and reaches the copy after it; the panel's edit
/// (another frame's, here made plain by a value no frame has) is
/// nowhere in it.
#[test]
fn a_quit_from_culling_still_writes_a_followed_frames_copy() {
    use crate::panel::startup::save_at_quit;
    let dir = scratch("cull-quit-followed");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    save(&state, 0.5);
    land_sent(&state, &app, &worker);
    let copy = nas.join("shoot").join("a.tif");
    let away = dir.join("local-away");
    greycard_core::testing::rename_retry(&local, &away);
    {
        let mut st = state.borrow_mut();
        st.views.library.offline.insert(local.clone());
        follow_offline(&mut st, &app);
    }
    land_sent(&state, &app, &worker);
    assert_eq!(state.borrow().files[0], copy, "followed");
    cull_from(&state, &app, 0);
    assert!(
        SENT.with(|s| s.borrow().is_empty()),
        "nothing new on the panel on the way in"
    );
    // A rating, its write sent and not landed.
    assert!(app.invoke_meta_key("3".into()));
    assert_eq!(state.borrow().sidecars[0].meta.rating, 3);
    assert!(state.borrow().work.sync.in_flight.contains(&copy));
    let on_copy = || Sidecar::load_as_file(&copy).unwrap().unwrap();
    assert_eq!(on_copy().meta.rating, 0);
    state.borrow_mut().views.index_reader = None;
    assert_eq!(
        save_at_quit(
            &mut state.borrow_mut(),
            exposed(2.0),
            Duration::from_millis(200)
        ),
        QuitWrites { ended: 0, noted: 1 },
        "the followed frame's write made, and noted"
    );
    assert_eq!(
        pending_for(&db, &nas)
            .iter()
            .map(|r| (r.frame.as_path(), r.reason.as_str()))
            .collect::<Vec<_>>(),
        vec![(copy.as_path(), "saved as the window closed")]
    );
    assert!(!exposures_of(&state.borrow().sidecars[0]).contains(&2.0));
    // The key's job lands; the quit's write goes after it.
    land_sent(&state, &app, &worker);
    let latest = state.borrow().sidecars[0]
        .revisions
        .last()
        .map(|r| r.hash.clone());
    let recorded = || {
        let lib = Library::open_read_only(&db).unwrap();
        let hash = lib.by_path(&copy).unwrap().unwrap().hash;
        lib.archive_write(&hash, &nas, &copy)
            .unwrap()
            .map(|w| w.revision)
    };
    let start = std::time::Instant::now();
    while recorded() != latest {
        assert!(start.elapsed() < Duration::from_secs(20), "never written");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(on_copy().meta.rating, 3);
    assert_eq!(on_copy().current().light.exposure, 0.5);
    assert!(!exposures_of(&on_copy()).contains(&2.0), "not the panel's");
    greycard_core::testing::rename_retry(&away, &local);
    crate::testing::remove_scratch(state, &dir);
}

/// A quit from culling still notes the archive writes that never
/// went out (§261): a culling key's write queued behind one still
/// out is noted waiting for the next window. The panel's edit is
/// recorded nowhere.
#[test]
fn a_quit_from_culling_still_notes_the_writes_not_sent() {
    use crate::panel::startup::save_at_quit;
    let dir = scratch("cull-quit-noted");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    cull_from(&state, &app, 1);
    assert!(SENT.with(|s| s.borrow().is_empty()));
    // Two keys on b: one write out, the next queued behind it.
    assert!(app.invoke_meta_key("2".into()));
    assert!(app.invoke_meta_key("3".into()));
    assert_eq!(state.borrow().sidecars[1].meta.rating, 3);
    assert_eq!(state.borrow().work.sync.queued.len(), 1);
    let on_disk = std::fs::read(Sidecar::path_for(&files[1])).unwrap();
    state.borrow_mut().views.index_reader = None;
    save_at_quit(
        &mut state.borrow_mut(),
        exposed(2.0),
        crate::sync::QUIT_WRITE_WAIT,
    );
    assert!(state.borrow().work.sync.queued.is_empty());
    let p = pending_for(&db, &nas);
    assert_eq!(
        p.iter()
            .map(|r| (r.frame.as_path(), r.reason.as_str()))
            .collect::<Vec<_>>(),
        vec![(files[1].as_path(), "saved as the window closed")]
    );
    assert_eq!(
        std::fs::read(Sidecar::path_for(&files[1])).unwrap(),
        on_disk,
        "the panel's edit not written onto b"
    );
    assert!(!exposures_of(&state.borrow().sidecars[1]).contains(&2.0));
    drop(worker);
    crate::testing::remove_scratch(state, &dir);
}

/// An export recorded on a frame the window has let go of, and a
/// stand-in un-rejected, reach the archive from the file.
#[test]
fn a_save_made_on_disk_reaches_the_archive_from_the_file() {
    let dir = scratch("disk-save");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    let mut on_disk = Sidecar::default();
    on_disk.record(exposed(4.0));
    on_disk.save(&files[1]).unwrap();
    after_disk_save(&mut state.borrow_mut(), &files[1]);
    land_sent(&state, &app, &worker);
    let copy = nas.join("shoot").join("b.tif");
    assert_eq!(
        Sidecar::load_as_file(&copy)
            .unwrap()
            .unwrap()
            .current()
            .light
            .exposure,
        4.0
    );
    crate::testing::remove_scratch(state, &dir);
}
/// One raw in a shoot and in its selects; the archive holds the
/// shoot's copy, and the selects' too when `both_on_nas`.
fn dup_setup(
    dir: &Path,
    both_on_nas: bool,
) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    let shoot = local.join("shoot");
    let selects = local.join("selects");
    std::fs::create_dir_all(&shoot).unwrap();
    std::fs::create_dir_all(&selects).unwrap();
    let a = shoot.join("a.tif");
    write_frame(&a, &R5, 1);
    let a2 = selects.join("a.tif");
    std::fs::copy(&a, &a2).unwrap();
    let subs: &[&str] = if both_on_nas {
        &["shoot", "selects"]
    } else {
        &["shoot"]
    };
    for sub in subs {
        std::fs::create_dir_all(nas.join(sub)).unwrap();
        std::fs::copy(&a, nas.join(sub).join("a.tif")).unwrap();
    }
    let db = dir.join("library.sqlite");
    {
        let mut lib = Library::open(&db).unwrap();
        lib.index_tree(&local, &mut |_| {}).unwrap();
        lib.index_tree(&nas, &mut |_| {}).unwrap();
    }
    (local, nas, shoot, a, a2, db)
}

fn hash_of(db: &Path, f: &Path) -> String {
    Library::open_read_only(db)
        .unwrap()
        .by_path(f)
        .unwrap()
        .unwrap()
        .hash
}

fn show(p: &Path) -> Vec<f32> {
    Sidecar::load_as_file(p)
        .unwrap()
        .map(|s| exposures_of(&s))
        .unwrap_or_default()
}

/// Probe F: a row for a frame the window does not hold, whose local
/// disk is unplugged while the archive answers, is kept with the
/// reason, not dropped as gone; the disk back, the next catch-up
/// writes it.
#[test]
fn a_catch_up_keeps_the_row_of_a_frame_whose_disk_is_away() {
    let dir = scratch("disk-away");
    let (local, nas, files, db) = two_places(&dir);
    {
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
        save(&state, 0.5);
        land_sent(&state, &app, &worker);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
        assert_eq!(pending_for(&db, &nas).len(), 1);
        state.borrow_mut().views.index_reader = None;
        drop(state);
        drop(worker);
    }
    // Another window, over an archive frame; the local disk gone.
    let app = window(1);
    let other = nas.join("shoot").join("b.tif");
    let (state, worker) = opened(&app, &dir, vec![other], &local, &nas, &db);
    let away = dir.join("local-away");
    greycard_core::testing::rename_retry(&local, &away);
    heard_from(&mut state.borrow_mut(), &app, &nas, true);
    land_sent(&state, &app, &worker);
    let after = pending_for(&db, &nas);
    assert_eq!(after.len(), 1, "{after:?}");
    assert!(
        after[0].reason.contains("disk may be away"),
        "{}",
        after[0].reason
    );
    assert_eq!(app.get_sync_note(), "1 edit waiting for Archive");
    // The disk back: the next answering look writes it.
    greycard_core::testing::rename_retry(&away, &local);
    heard_from(&mut state.borrow_mut(), &app, &nas, true);
    land_sent(&state, &app, &worker);
    assert!(pending_for(&db, &nas).is_empty());
    let copy = nas.join("shoot").join("a.tif");
    assert_eq!(
        Sidecar::load_as_file(&copy)
            .unwrap()
            .unwrap()
            .current()
            .light
            .exposure,
        0.5
    );
    crate::testing::remove_scratch(state, &dir);
}

/// Probe G: the selects frame's row saying its copy is not known
/// is its own; the shoot frame's paired write does not clear it.
/// A pairing for the selects folder does.
#[test]
fn a_frames_unknown_copy_row_is_not_cleared_by_another_frames_write() {
    let dir = scratch("own-row");
    let (local, nas, shoot, a, a2, db) = dup_setup(&dir, true);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, vec![a.clone(), a2.clone()], &local, &nas, &db);
    state
        .borrow_mut()
        .views
        .library
        .roots
        .set_backup_folder(&shoot, &nas, &nas.join("shoot"));
    state.borrow_mut().current = Some(1);
    save(&state, -2.0);
    land_sent(&state, &app, &worker);
    let p = pending_for(&db, &nas);
    assert_eq!(p.len(), 1);
    assert!(
        p[0].frame == a2 && p[0].copy.as_os_str().is_empty(),
        "{p:?}"
    );
    assert_eq!(
        app.get_sync_note(),
        "1 edit waiting for Archive; 1 can't tell which copy is its own"
    );
    state.borrow_mut().current = Some(0);
    save(&state, 1.0);
    land_sent(&state, &app, &worker);
    let p = pending_for(&db, &nas);
    assert!(
        p.iter().any(|r| r.frame == a2),
        "the selects frame's row stands: {p:?}"
    );
    assert!(
        show(&nas.join("selects").join("a.tif")).is_empty(),
        "nothing written to it"
    );
    state.borrow_mut().views.library.roots.set_backup_folder(
        &local.join("selects"),
        &nas,
        &nas.join("selects"),
    );
    heard_from(&mut state.borrow_mut(), &app, &nas, true);
    land_sent(&state, &app, &worker);
    assert!(
        pending_for(&db, &nas).is_empty(),
        "{:?}",
        pending_for(&db, &nas)
    );
    assert_eq!(show(&nas.join("selects").join("a.tif")), show(&a2));
    assert_eq!(show(&nas.join("shoot").join("a.tif")), show(&a));
    crate::testing::remove_scratch(state, &dir);
}

/// Probe H: one raw in two local folders, the shoot alone backed up
/// and paired; the selects frame's sole hit on the archive is the
/// shoot's copy, and is not written.
#[test]
fn a_sole_hit_that_is_another_frames_copy_is_not_written() {
    let dir = scratch("sole-hit");
    let (local, nas, shoot, a, a2, db) = dup_setup(&dir, false);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, vec![a.clone(), a2.clone()], &local, &nas, &db);
    state
        .borrow_mut()
        .views
        .library
        .roots
        .set_backup_folder(&shoot, &nas, &nas.join("shoot"));
    save(&state, 1.0);
    land_sent(&state, &app, &worker);
    state.borrow_mut().current = Some(1);
    save(&state, -2.0);
    land_sent(&state, &app, &worker);
    let there = nas.join("shoot").join("a.tif");
    assert!(
        !show(&there).contains(&-2.0),
        "the selects edit went into the shoot's copy"
    );
    assert!(
        !show(&a2).contains(&1.0),
        "the shoot's edit joined into the selects frame"
    );
    assert_eq!(show(&there), show(&a));
    let p = pending_for(&db, &nas);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].frame == a2 && p[0].copy.as_os_str().is_empty());
    crate::testing::remove_scratch(state, &dir);
}

/// Probe I: a Settle job saved the join to the file and recorded
/// the copy as ours while the window held the file as read before
/// it. The window's save after finds the stat ours but the revision
/// not in its lineage, compares, and comes back to join: the
/// laptop's edit is in the copy still.
#[test]
fn a_window_save_after_a_settle_job_joins_rather_than_writes_over() {
    let dir = scratch("settle-race");
    let (local, nas, files, db) = two_places(&dir);
    let a = files[0].clone();
    let copy = nas.join("shoot").join("a.tif");
    let mut s = Sidecar::default();
    s.record(exposed(0.5));
    s.save(&a).unwrap();
    let hash = hash_of(&db, &a);
    let target = Copy {
        archive: nas.clone(),
        copy: copy.clone(),
    };
    let w = |source: Source| Write {
        frame: a.clone(),
        hash: hash.clone(),
        source,
        placement: Placement::Beside,
        xmp: false,
        copies: vec![target.clone()],
    };
    assert!(matches!(
        write_through(&w(Source::Disk), &db, &no_beat())[..],
        [Outcome::Written { .. }]
    ));
    laptop_saves(&copy, -1.0);
    // The window reads the file now.
    let mut window_s = Sidecar::load_as_file(&a).unwrap().unwrap();
    // The Settle job: the catch-up's Take on a frame not held.
    let theirs = Sidecar::read_as_file(&Sidecar::find(&copy).unwrap()).unwrap();
    // The file here is behind the copy: taken whole, and recorded.
    let out = write_through(&w(Source::Settle(Box::new(theirs))), &db, &no_beat());
    assert!(matches!(out[..], [Outcome::Same { .. }]), "{out:?}");
    assert!(show(&a).contains(&-1.0), "the copy's taken here");
    // The window's edit, save and write.
    window_s.record(exposed(0.7));
    window_s.save(&a).unwrap();
    let out = write_through(&w(Source::Memory(Box::new(window_s))), &db, &no_beat());
    assert!(matches!(out[..], [Outcome::Take { .. }]), "{out:?}");
    assert!(
        show(&copy).contains(&-1.0),
        "the laptop's edit stands in the copy"
    );
    let _ = local;
    crate::testing::remove_dir_retry(&dir);
}

/// Probe J: a save noted with no window open never brings the
/// library up: an older library is left as it is, byte for byte,
/// and the frame's copy waits for a window's save.
#[test]
fn a_note_with_no_window_open_leaves_an_older_library_untouched() {
    let dir = scratch("older-index");
    let (local, nas, files, db) = two_places(&dir);
    // The index at schema 5: the sync tables gone, the version down.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "DROP TABLE archive_writes; DROP TABLE pending; PRAGMA user_version = 5;",
        )
        .unwrap();
    }
    let before = std::fs::read(&db).unwrap();
    let mut roots = greycard_library::Roots::default();
    roots.add(&local).unwrap();
    roots.add(&nas).unwrap();
    roots.set_archive(&nas, true);
    assert_eq!(note_disk_save(&db, &roots, &files[1]), 0);
    assert_eq!(std::fs::read(&db).unwrap(), before, "not a byte changed");
    crate::testing::remove_dir_retry(&dir);
}

/// A save made before the index reader is open (a launch preset's)
/// is not dropped: it waits in memory and is queued when the
/// reader opens.
#[test]
fn a_save_before_the_reader_opens_is_queued_when_it_does() {
    let dir = scratch("deferred");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = None;
    }
    save(&state, 0.4);
    {
        let st = state.borrow();
        assert_eq!(st.work.sync.deferred.len(), 1);
        assert!(st.work.sync.queued.is_empty() && st.work.sync.in_flight.is_empty());
    }
    assert_eq!(land_sent(&state, &app, &worker), 0);
    crate::library::open_reader(&mut state.borrow_mut(), &app);
    assert!(state.borrow().work.sync.deferred.is_empty());
    assert_eq!(state.borrow().work.sync.in_flight.len(), 1);
    land_sent(&state, &app, &worker);
    let copy = nas.join("shoot").join("a.tif");
    assert_eq!(
        Sidecar::load_as_file(&copy)
            .unwrap()
            .unwrap()
            .current()
            .light
            .exposure,
        0.4
    );
    crate::testing::remove_scratch(state, &dir);
}
/// A frame deleted by another hand from a folder that is there has
/// its row dropped at the next catch-up; the editor's own Delete
/// drops it with the frame's row. A folder gone whole keeps the
/// row (the disk-away test above).
#[test]
fn a_frame_gone_from_a_present_folder_has_its_row_dropped() {
    let dir = scratch("deleted");
    let (local, nas, files, db) = two_places(&dir);
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
    save(&state, 0.5);
    land_sent(&state, &app, &worker);
    state.borrow_mut().current = Some(1);
    save(&state, 0.6);
    land_sent(&state, &app, &worker);
    crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
    assert_eq!(pending_for(&db, &nas).len(), 2);
    // b deleted by the editor: its row and its pending rows go.
    Library::open(&db)
        .unwrap()
        .forget(std::slice::from_ref(&files[1]))
        .unwrap();
    assert_eq!(pending_for(&db, &nas).len(), 1);
    // a deleted by another hand, the window over other frames now.
    let other = vec![nas.join("shoot").join("b.tif")];
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(worker);
    std::fs::remove_file(&files[0]).unwrap();
    for s in Sidecar::find(&files[0]).into_iter() {
        std::fs::remove_file(s).unwrap();
    }
    let app = window(1);
    let (state, worker) = opened(&app, &dir, other, &local, &nas, &db);
    assert_eq!(app.get_sync_note(), "1 edit waiting for Archive");
    heard_from(&mut state.borrow_mut(), &app, &nas, true);
    land_sent(&state, &app, &worker);
    assert!(
        pending_for(&db, &nas).is_empty(),
        "{:?}",
        pending_for(&db, &nas)
    );
    assert_eq!(app.get_sync_note(), "");
    assert!(
        Sidecar::load_as_file(&nas.join("shoot").join("a.tif"))
            .unwrap()
            .is_none()
    );
    crate::testing::remove_scratch(state, &dir);
}
/// Probe R2: the local sidecar of a followed frame edited on its own
/// while the root was away (another build, a headless export): the
/// root back, it is not written over; it is left for Back up or
/// Bring back to join.
#[test]
fn bringing_home_leaves_a_local_sidecar_that_went_its_own_way() {
    let dir = scratch("home-diverged");
    let (local, nas, files, db) = two_places(&dir);
    let copy = nas.join("shoot").join("a.tif");
    let app = window(2);
    let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
    save(&state, 0.5);
    land_sent(&state, &app, &worker);
    {
        let mut st = state.borrow_mut();
        st.views.library.offline.insert(local.clone());
        follow_offline(&mut st, &app);
    }
    land_sent(&state, &app, &worker);
    assert_eq!(state.borrow().files[0], copy);
    save(&state, 0.7);
    land_sent(&state, &app, &worker);
    // The local file edited on its own while the frame was followed.
    let mut there = Sidecar::load_as_file(&files[0]).unwrap().unwrap();
    there.record(exposed(-3.0));
    there.save(&files[0]).unwrap();
    {
        let mut st = state.borrow_mut();
        st.views.library.offline.clear();
        heard_over(&mut st, &app, &HashSet::new());
    }
    land_sent(&state, &app, &worker);
    let home = Sidecar::load_as_file(&files[0]).unwrap().unwrap();
    assert!(
        exposures_of(&home).contains(&-3.0),
        "the local edit was written over"
    );
    assert!(!exposures_of(&home).contains(&0.7));
    crate::testing::remove_scratch(state, &dir);
}

/// A frame at the top of a root that is a fixed mount point: the
/// disk unplugged, the folder lists empty, and the row is kept; a
/// folder with anything else in it and the frame gone is a delete,
/// and the row goes.
#[test]
fn an_empty_folder_keeps_the_row_of_a_frame_that_was_in_it() {
    let dir = scratch("mount");
    let (mount, nas) = (dir.join("mount"), dir.join("nas"));
    std::fs::create_dir_all(&mount).unwrap();
    std::fs::create_dir_all(&nas).unwrap();
    let a = mount.join("a.tif");
    write_frame(&a, &R5, 1);
    std::fs::copy(&a, nas.join("a.tif")).unwrap();
    let db = dir.join("library.sqlite");
    {
        let mut lib = Library::open(&db).unwrap();
        lib.index_tree(&mount, &mut |_| {}).unwrap();
        lib.index_tree(&nas, &mut |_| {}).unwrap();
    }
    let app = window(1);
    let (state, worker) = opened(&app, &dir, vec![a.clone()], &mount, &nas, &db);
    crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
    save(&state, 0.5);
    land_sent(&state, &app, &worker);
    crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
    assert_eq!(pending_for(&db, &nas).len(), 1);
    // The window over the archive's frame; the mount point empty.
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(worker);
    std::fs::remove_file(&a).unwrap();
    for s in Sidecar::find(&a).into_iter() {
        std::fs::remove_file(s).unwrap();
    }
    assert!(std::fs::read_dir(&mount).unwrap().next().is_none());
    let app = window(1);
    let (state, worker) = opened(&app, &dir, vec![nas.join("a.tif")], &mount, &nas, &db);
    heard_from(&mut state.borrow_mut(), &app, &nas, true);
    land_sent(&state, &app, &worker);
    let p = pending_for(&db, &nas);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].reason.contains("disk may be away"));
    // Something else in the folder: the frame was deleted.
    std::fs::write(mount.join("notes.txt"), b"x").unwrap();
    heard_from(&mut state.borrow_mut(), &app, &nas, true);
    land_sent(&state, &app, &worker);
    assert!(pending_for(&db, &nas).is_empty());
    crate::testing::remove_scratch(state, &dir);
}
