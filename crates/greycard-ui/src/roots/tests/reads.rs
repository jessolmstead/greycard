//! Tests of the reads: folders locked or reached through links, views
//! read from the rows, and folder opens.

use super::*;

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
    crate::testing::remove_dir_retry(&dir);
}

/// The merge asks the disk nothing: a list whose folders are all
/// gone merges as given, and a read landed after its folder went
/// merges what the read saw.
#[test]
fn a_merge_over_folders_all_gone_completes() {
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    let nowhere = crate::testing::folder(4);
    let read: HashMap<PathBuf, Held> = nowhere
        .iter()
        .map(|p| {
            (
                p.clone(),
                Held {
                    sidecar: Sidecar::default(),
                    from_row: rows::FromRow::read(),
                    id: None,
                    trouble: None,
                },
            )
        })
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
    crate::testing::remove_dir_retry(&dir);
    land(&state, &app, &worker, found);
    assert_eq!(state.borrow().files, files);
    assert!(!state.borrow().library.merging);
}

/// A view of the roots is built from the index's rows: each frame's
/// meta, whether it has a develop and how its picture turns come
/// from its row, and no sidecar is read. Opening a frame reads its
/// own, and only its own.
#[test]
fn a_view_of_the_roots_reads_no_sidecar() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-view");
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.files, files);
        assert_eq!(disk_reads_under(&dir), 0, "no sidecar read for the view");
        assert!(st.from_row.iter().all(|r| !r.read), "every frame stands in");
        assert_eq!(st.sidecars[0].meta.rating, 3);
        assert_eq!(st.sidecars[0].meta.flag, meta::Flag::Pick);
        assert_eq!(st.sidecars[1].meta.keywords, ["Harbor"]);
        assert_eq!(st.sidecars[2].meta, Meta::default());
        assert_eq!(
            crate::panel::browser::thumb_turns(&st, &app, 1),
            (1, false),
            "the edit's quarter turn, from the row"
        );
        assert_eq!(crate::panel::browser::thumb_turns(&st, &app, 0), (0, false));
        let thumbs = app.get_thumbs();
        let x = thumbs.row_data(0).unwrap();
        assert_eq!((x.rating, x.flag, x.offline), (3, 1, false));
        assert!(!st.library.loading);
    }
    // Opening y reads its sidecar, off the window's thread, and
    // nothing else's: y is current from the pick, its panel the
    // stand-in's, and opened when the read lands.
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::open_row(&mut st, &app, &worker, 1, false);
        assert_eq!(st.current, Some(1));
        assert!(!st.from_row[1].read, "on its way");
        assert_eq!(st.pick_pending, Some(files[1].clone()));
        assert!(app.get_status().contains("reading"), "{}", app.get_status());
    }
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.current, Some(1));
        assert!(st.pick_pending.is_none());
        assert!(st.from_row[1].read);
        assert!(!st.from_row[0].read && !st.from_row[2].read);
        assert_eq!(disk_reads_under(&dir), 1);
        assert_eq!(st.sidecars[1].current.geometry.turns, 1);
        assert_eq!(st.sidecars[1].history.len(), 1);
        assert_eq!(st.sidecars[1].meta.keywords, ["Harbor"]);
    }
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// The sidecar stays the truth: a sidecar changed on disk is read by
/// the next pass over its folder, and the pass's word has the list
/// read again, so every frame still standing in takes its row's new
/// meta, even when the list itself did not change. A frame already
/// loaded keeps what memory holds.
#[test]
fn a_frame_s_meta_follows_the_pass_while_its_sidecar_stands_in() {
    let (dir, files, mut writer, app, state, worker) = rated_under_a_root("rows-refresh");
    let a = dir.join("a");
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    // z loaded and edited in memory, unsaved.
    {
        let mut st = state.borrow_mut();
        assert!(rows::load_frame(&mut st, &app, 2));
        st.sidecars[2].meta.rating = 1;
    }
    // Another tool rates x five, gives y a label, and z four.
    let mut s = Sidecar::load(&files[0]).unwrap().unwrap();
    s.meta.rating = 5;
    s.save(&files[0]).unwrap();
    let mut s = Sidecar::load(&files[1]).unwrap().unwrap();
    s.meta.label = meta::Label::Red;
    s.save(&files[1]).unwrap();
    let mut s = Sidecar::default();
    s.meta.rating = 4;
    s.save(&files[2]).unwrap();
    let report = writer.index_folder(&a, &mut |_| {}).unwrap();
    assert_eq!(report.meta_refreshed, 3, "{report:?}");
    assert!(background_done(&state, &app, &a, &report, false, false));
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.files, files, "the same list");
        assert_eq!(st.sidecars[0].meta.rating, 5, "x's row, refreshed");
        assert!(!st.from_row[0].read);
        assert_eq!(st.sidecars[1].meta.label, meta::Label::Red);
        assert_eq!(st.sidecars[2].meta.rating, 1, "z as edited in memory");
        let thumbs = app.get_thumbs();
        assert_eq!(thumbs.row_data(0).unwrap().rating, 5, "the badge followed");
        assert_eq!(thumbs.row_data(1).unwrap().label, meta::Label::Red.code());
        assert_eq!(disk_reads_under(&dir), 1, "only z's sidecar was read");
    }
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A rating key on a frame standing in reads its whole sidecar first,
/// changes it, and writes it; the save's pass over the file then
/// brings the row up to what was written.
#[test]
fn a_key_on_a_view_frame_reads_its_sidecar_then_writes_it() {
    let (dir, files, mut writer, app, state, worker) = rated_under_a_root("rows-key");
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let mut st = state.borrow_mut();
        let (settled, next) =
            crate::panel::browser::set_meta(&mut st, &app, &[1], meta::Change::Rating(4));
        assert_eq!(settled, meta::Change::Rating(4));
        assert_eq!(next, None);
        assert!(st.from_row[1].read, "read for the key");
        assert!(!st.from_row[0].read);
        assert_eq!(disk_reads_under(&dir), 1);
        assert_eq!(st.sidecars[1].meta.rating, 4);
        assert_eq!(st.sidecars[1].history.len(), 1, "the develop kept");
    }
    let on_disk = Sidecar::load(&files[1]).unwrap().unwrap();
    assert_eq!(on_disk.meta.rating, 4);
    assert_eq!(on_disk.meta.keywords, ["Harbor"]);
    assert_eq!(on_disk.history.len(), 1, "the develop written back whole");
    // As the save's index_file would: the row follows.
    writer.index_file(&files[1]).unwrap();
    let row = writer
        .rows_of_with(&files[1..2], &mut |d| d.to_path_buf())
        .unwrap()
        .remove(0)
        .unwrap();
    assert_eq!((row.meta.rating, row.edited), (4, true));
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A folder's own open reads nothing on the window's thread: the
/// read is queued (here) and the list lands with it. A folder the
/// index does not know has every sidecar read off the window's
/// thread; one it knows stands in from its rows. An empty folder is
/// said, and the list left alone.
#[test]
fn a_folder_open_reads_off_the_window_s_thread_or_from_the_rows() {
    let dir = scratch("folder-open");
    let files = frames(&dir, &["a.tif", "b.tif", "c.tif"]);
    let mut s = Sidecar::default();
    s.meta.rating = 2;
    s.save(&files[0]).unwrap();
    let mut s = Sidecar::default();
    s.meta.rating = 4;
    s.save(&files[1]).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    state.borrow_mut().write_sidecars = true;
    open_listing(
        &state,
        &app,
        &worker,
        Listing::Folder(dir.clone()),
        Select::Last(Some(files[1].clone())),
    );
    assert_eq!(sent(), 1);
    {
        let st = state.borrow();
        assert!(st.files.is_empty(), "nothing until the read lands");
        assert!(st.library.loading, "and the bar's timer runs meanwhile");
        assert_eq!(disk_reads_under(&dir), 0);
    }
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.files, files);
        assert_eq!(st.view, View::Folder);
        assert!(!st.library.loading);
        assert!(
            st.from_row.iter().all(|r| r.read),
            "no index: every sidecar read"
        );
        assert_eq!(disk_reads_under(&dir), 3);
        let ratings: Vec<u8> = st.sidecars.iter().map(|s| s.meta.rating).collect();
        assert_eq!(ratings, [2, 4, 0]);
        // Opened from the rendering setup, which no test runs.
        assert_eq!(
            st.select_at_start,
            Some(1),
            "the last file open, matched off the thread"
        );
    }
    // Indexed: the rows stand in, and nothing is read.
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_folder(&dir, &mut |_| {}).unwrap();
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.write_sidecars = true;
        st.index_path = Some(db.clone());
    }
    open_listing(
        &state,
        &app,
        &worker,
        Listing::Files(files.clone()),
        Select::Row(2),
    );
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.files, files);
        assert!(st.from_row.iter().all(|r| !r.read), "the rows stand in");
        assert_eq!(disk_reads_under(&dir), 3, "nothing more read");
        let ratings: Vec<u8> = st.sidecars.iter().map(|s| s.meta.rating).collect();
        assert_eq!(ratings, [2, 4, 0]);
        assert_eq!(st.select_at_start, Some(2));
    }
    // An empty folder: said, the list as it was.
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    crate::panel::browser::open_folder(&state, &app, &worker, &empty);
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.files, files, "left alone");
        assert!(!st.library.loading);
        assert!(
            app.get_status().contains("no pictures in"),
            "{}",
            app.get_status()
        );
    }
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A folder on a share that does not answer: its open is looked at
/// with the roots' wait, off the window's thread, and said; the list,
/// the view and the bar are as they were, and no sidecar or listing
/// under it is read. An open sent after it overtakes it, and the
/// hung one's word does not land over the new list. Recently opened
/// says the same of it.
#[test]
fn a_folder_that_does_not_answer_is_said_and_overtaken() {
    let dir = scratch("hung-folder");
    let here = dir.join("here");
    let gone = dir.join("share").join("day");
    let next = dir.join("next");
    for d in [&here, &gone, &next] {
        std::fs::create_dir_all(d).unwrap();
    }
    let files = frames(&here, &["a.tif", "b.tif"]);
    let next_files = frames(&next, &["c.tif"]);
    frames(&gone, &["d.tif"]);
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    state.borrow_mut().write_sidecars = true;
    crate::panel::browser::open_folder(&state, &app, &worker, &here);
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().files, files);
    tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(dir.join("share")));
    crate::panel::browser::open_folder(&state, &app, &worker, &gone);
    assert!(state.borrow().library.loading, "the card is up meanwhile");
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.files, files, "the list is as it was");
        assert_eq!(st.view, View::Folder);
        assert!(!st.library.loading);
        assert_eq!(disk_reads_under(&gone), 0);
        assert!(!st.library.canonical.contains_key(&gone));
    }
    assert!(
        app.get_status().contains("isn't reachable"),
        "{}",
        app.get_status()
    );
    // Overtaken: the hung folder's open, then another before it
    // lands. The second's list is what stays, and nothing of the
    // first is said over it.
    crate::panel::browser::open_folder(&state, &app, &worker, &gone);
    crate::panel::browser::open_folder(&state, &app, &worker, &next);
    assert_eq!(land_all(&state, &app, &worker), 2);
    assert_eq!(state.borrow().files, next_files);
    assert!(
        !app.get_status().contains("isn't reachable"),
        "{}",
        app.get_status()
    );
    // Recently opened: the same look, the same words.
    crate::panel::recent::choose_for_test(&state, &app, &worker, &gone);
    assert!(
        app.get_status().contains("isn't reachable"),
        "{}",
        app.get_status()
    );
    assert_eq!(state.borrow().files, next_files);
    tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// A folder opened through a link is made canonical by the read, off
/// the window's thread, and Recently opened records the folder it
/// points to from that, not from a look of its own.
#[cfg(unix)]
#[test]
fn a_folder_opened_through_a_link_is_recorded_as_the_read_made_it() {
    let dir = scratch("link-open");
    let real = dir.join("real");
    std::fs::create_dir_all(&real).unwrap();
    frames(&real, &["a.tif"]);
    let link = dir.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    crate::panel::browser::open_folder(&state, &app, &worker, &link);
    land_all(&state, &app, &worker);
    let st = state.borrow();
    assert_eq!(st.files, vec![link.join("a.tif")]);
    assert_eq!(st.library.canonical.get(&link), Some(&real));
    assert_eq!(st.recent.open.as_deref(), Some(real.as_path()));
    drop(st);
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}
