//! Tests of a list kept up with the disk: merges, the frame on screen
//! followed, the passes and the offline roots.

use super::*;

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
    crate::testing::remove_dir_retry(&dir);
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
    crate::testing::remove_dir_retry(&dir);
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
    crate::testing::remove_dir_retry(&dir);
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
    // Each word printed as it comes: the test has failed on the
    // Windows runner with no panic text in the log, and the capture
    // is shown on a failure.
    let wait = |want: &dyn Fn(&Told) -> bool| loop {
        let told = match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(told) => told,
            Err(e) => panic!("the indexer answers: {e}"),
        };
        eprintln!("told: {told:?}");
        if want(&told) {
            return told;
        }
    };
    wait(&|t| matches!(t, Told::Opened(_)));
    eprintln!("root: {}", root.display());
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
    eprintln!("counted");
    // An asker still held, as the watcher holds one, does not keep
    // the thread from leaving.
    let _held = indexer.asker();
    indexer.stop(Duration::from_secs(20));
    eprintln!("stopped");
    crate::testing::remove_dir_retry(&dir);
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
    // The strip with a cell for every frame, as it has once laid
    // out: only a row with a cell carries a picture.
    crate::cells::show(&app, crate::cells::View::Strip, 0, 8);
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
    crate::testing::remove_dir_retry(&dir);
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
        let (index, frame) = match outcome {
            crate::worker::Outcome::Thumbnail { index, frame, .. }
            | crate::worker::Outcome::NoThumbnail { index, frame } => (index, frame),
            _ => continue,
        };
        // What the window would take: a picture for the file
        // that is at that number now.
        if next.get(index) == Some(&frame.path) && !came.contains(&index) {
            came.push(index);
        }
    }
    came.sort();
    assert_eq!(came, owed, "each frame still owed a picture gets one");
    worker.stop();
    drop(state);
    crate::testing::remove_dir_retry(&dir);
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
    crate::testing::remove_dir_retry(&dir);
}

/// B4, the review's repro: a filter kept from the last session
/// (picks only) over a capture of a folder, or a file named on the
/// command line or double-clicked from the desktop. None puts it
/// back; a session opened on a folder does. (An export from the
/// command line has no browser to filter: `headless`.)
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
    assert!(!restores_filter(&cli(&[&d, "--screenshot", "s.png"])));
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
    assert!(state.borrow().filter.is_empty(), "cleared at once");
    assert_eq!(land_all(&state, &app, &worker), 1);
    let st = state.borrow();
    assert!(st.filter.is_empty());
    assert_eq!(st.shown, [0]);
    assert!(!st.library.loading, "the folder's own load is done");
    drop(st);
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// A frame under an offline root is culled from its local preview,
/// found by the key its row gives, and nothing of its file is read:
/// the loupe asks for the preview alone, the preview comes from the
/// cache, and it is marked as the local preview. A frame of the
/// same root with no preview kept says so and is not asked for; a
/// frame under a root on a network mount asks for the preview and
/// then its file; one on a local disk, its file.
#[test]
fn an_offline_frame_is_culled_from_its_local_preview_and_its_file_is_not_read() {
    use crate::previews::{self, Previews};
    let dir = scratch("offline-preview");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let x = frames(&a, &["x.tif"]).remove(0);
    let zs = frames(&b, &["z.tif", "zz.tif"]);
    let (z, zz) = (zs[0].clone(), zs[1].clone());
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.write_sidecars = true;
        st.library.roots.add(&a).unwrap();
        st.library.roots.add(&b).unwrap();
        recount(&mut st);
    }
    let away = dir.join("b-away");
    crate::testing::rename_away(&b, &away);
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    let (zi, zzi) = {
        let st = state.borrow();
        let at = |p: &PathBuf| st.files.iter().position(|f| f == p).unwrap();
        (at(&z), at(&zz))
    };
    // z's preview, as the pool made it while the root was there,
    // under the key its row gives.
    let (hash, stamp) = state.borrow().from_row[zi]
        .key
        .clone()
        .expect("the row's key");
    let cache: crate::worker::ThumbCache = std::sync::Arc::new(std::sync::Mutex::new(Some(
        greycard_library::Thumbs::at(dir.join("thumbs"), greycard_library::thumbs::DEFAULT_CAP)
            .with_previews(
                crate::previews::SIZE,
                greycard_library::thumbs::DEFAULT_PREVIEW_CAP,
            ),
    )));
    let kept = greycard_library::thumbs::Thumb {
        width: 300,
        height: 200,
        rgb: vec![120; 300 * 200 * 3],
    };
    let cache_put = |hash: &str, stamp: u64| {
        cache
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .put_at(
                hash,
                previews::SIZE,
                crate::worker::thumb_tag((0, stamp)),
                &kept,
                previews::QUALITY,
            )
            .unwrap()
    };
    cache_put(&hash, stamp);
    let local = std::sync::Arc::new(Previews::new(cache.clone()));
    // Culling on z.
    {
        let mut st = state.borrow_mut();
        st.prefetch.set_previews(local.clone());
        st.current = Some(zi);
        crate::panel::cull::enter_cull(&mut st, &app, 1);
        assert!(st.cull.is_some());
    }
    let asked = state.borrow().prefetch.asked();
    let want = asked.iter().find(|w| w.file == zi).expect("z asked for");
    assert_eq!(
        want.from,
        crate::cull::Source::Preview {
            hash: hash.clone(),
            stamp
        },
        "the preview alone"
    );
    let zz_want = asked.iter().find(|w| w.file == zzi).expect("zz asked for");
    assert!(matches!(zz_want.from, crate::cull::Source::Preview { .. }));
    let x_want = asked.iter().find(|w| w.path == x).expect("x asked for");
    assert_eq!(
        x_want.from,
        crate::cull::Source::File,
        "a local root's file"
    );
    // The decode thread's work, here: the preview from the cache,
    // and for zz, which has none kept, the word that says so.
    let fetched = |want: &crate::cull::Want| {
        let got = std::cell::RefCell::new(Vec::new());
        crate::cull::fetch(want, Some(&local), &|l| got.borrow_mut().push(l));
        got.into_inner()
    };
    let got = fetched(want);
    assert_eq!(got.len(), 1);
    crate::panel::cull::deliver_preview(&app, got.into_iter().next().unwrap());
    let got = fetched(zz_want);
    assert!(matches!(&got[..], [crate::cull::Loaded::NoPreview { .. }]));
    crate::panel::cull::deliver_preview(&app, got.into_iter().next().unwrap());
    assert_eq!(
        state
            .borrow()
            .cull
            .as_ref()
            .unwrap()
            .failed
            .get(&zzi)
            .map(String::as_str),
        Some(crate::cull::NO_LOCAL_PREVIEW)
    );
    {
        let st = state.borrow();
        let shown = st.cull.as_ref().unwrap().cache.get(zi).expect("up");
        assert!(shown.local, "marked as the local preview");
        assert_eq!((shown.width, shown.height), (300, 200));
        assert!(!shown.small());
    }
    let read = |p: &Path| {
        crate::cull::FILE_READS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|r| r == p)
    };
    assert!(!read(&z), "nothing of z's file read");
    assert!(!read(&zz), "nor of zz's");
    assert_eq!(disk_reads_under(&away), 0);
    // Its words on the status line.
    let named = [crate::panel::cull::Named {
        file: zi,
        size: (300, 200),
        small: false,
        own: true,
        local: crate::cull::Origin::Local,
        version: None,
    }];
    let line = crate::panel::cull::cull_status(
        state.borrow().cull.as_ref().unwrap(),
        zi,
        0.0,
        true,
        &named,
    );
    assert!(
        line.starts_with("culling: the local preview, 300 \u{d7} 200, fitted"),
        "{line}"
    );
    // The same view with a's root on a network mount: its frame
    // asks for its local preview and its file, every preview of
    // the window ahead of every file.
    let xi = state.borrow().files.iter().position(|f| *f == x).unwrap();
    {
        let mut st = state.borrow_mut();
        st.library.remote = vec![(a.clone(), "nfs".into())];
        crate::panel::cull::cull_refresh(&mut st);
    }
    let asked = state.borrow().prefetch.asked();
    let local_at = asked
        .iter()
        .position(|w| w.file == xi && matches!(w.from, crate::cull::Source::Preview { .. }))
        .expect("x's local preview asked for");
    let file_at = asked
        .iter()
        .position(|w| w.file == xi && w.from == crate::cull::Source::File)
        .expect("x's file asked for");
    let first_file = asked
        .iter()
        .position(|w| w.from == crate::cull::Source::File)
        .unwrap();
    assert!(local_at < first_file && first_file <= file_at, "{asked:?}");
    // Its local preview up, then its file fails (the share has
    // stopped answering): the preview stays, and the next step asks
    // for neither again.
    let (x_hash, x_stamp) = state.borrow().from_row[xi].key.clone().unwrap();
    cache_put(&x_hash, x_stamp);
    let x_local = asked[local_at].clone();
    let got = fetched(&x_local);
    crate::panel::cull::deliver_preview(&app, got.into_iter().next().unwrap());
    crate::panel::cull::deliver_preview(
        &app,
        crate::cull::Loaded::Failed {
            file: xi,
            frame: crate::versions::Key::file(x.clone()),
            size: asked[file_at].size,
            message: "the share did not answer".into(),
        },
    );
    {
        let mut st = state.borrow_mut();
        let cull = st.cull.as_ref().unwrap();
        assert!(cull.cache.get(xi).is_some_and(|p| p.local), "stays up");
        assert!(cull.file_failed.contains(&xi));
        assert!(!cull.failed.contains_key(&xi), "no failure over it");
        crate::panel::cull::cull_refresh(&mut st);
    }
    let asked = state.borrow().prefetch.asked();
    assert!(
        !asked.iter().any(|w| w.file == xi),
        "not asked again: {asked:?}"
    );
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A frame with a copy on an archive is one frame (§216): All roots
/// lists the local copy and leaves the archive's out while the local
/// one is there. When it is not, its root offline or its folder
/// gone after a backup, the frame is listed from the archive, with a
/// picture, rather than as a dimmed stand-in for a file that cannot
/// be opened.
#[test]
fn all_roots_lists_the_archives_copy_when_the_local_one_is_away() {
    let dir = scratch("two-places");
    let (a, nas) = (dir.join("a"), dir.join("nas"));
    std::fs::create_dir_all(a.join("day")).unwrap();
    std::fs::create_dir_all(nas.join("day")).unwrap();
    let x = a.join("x.tif");
    let y = a.join("day").join("y.tif");
    write_frame(&x, &R5, 1);
    write_frame(&y, &R6, 2);
    write_frame(&nas.join("z.tif"), &A7, 3);
    std::fs::copy(&x, nas.join("x.tif")).unwrap();
    std::fs::copy(&y, nas.join("day").join("y.tif")).unwrap();
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.library.roots.add(&a).unwrap();
        st.library.roots.add(&nas).unwrap();
        st.library.roots.set_archive(&nas, true);
        recount(&mut st);
    }
    let listed = |state: &Rc<RefCell<State>>| {
        let mut files = state.borrow().files.clone();
        files.sort();
        files
    };
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert_eq!(
        listed(&state),
        [y.clone(), x.clone(), nas.join("z.tif")],
        "the copies hidden"
    );

    // The day's folder deleted after its backup: its frame is the
    // archive's now.
    std::fs::remove_dir_all(a.join("day")).unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert_eq!(
        listed(&state),
        [x.clone(), nas.join("day").join("y.tif"), nas.join("z.tif")]
    );

    // The root unplugged: both from the archive, and not dimmed.
    crate::testing::rename_away(&a, &dir.join("a-away"));
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert_eq!(
        listed(&state),
        [
            nas.join("day").join("y.tif"),
            nas.join("x.tif"),
            nas.join("z.tif")
        ]
    );
    let thumbs = app.get_thumbs();
    for i in 0..3 {
        assert!(
            !thumbs.row_data(i).unwrap().offline,
            "listed from the archive"
        );
    }
    drop(writer);
    crate::testing::remove_scratch(state, &dir);
}

/// A root that is not there (unplugged, its mount point gone) is
/// offline: its row says so, the launch pass does not touch it, so
/// its rows stay as they were rather than all go missing, and its
/// frames are in the view all the same, from their rows: dimmed,
/// badged, and filtered like any other. Opening one says why
/// nothing more happens, and a key on one changes nothing. A folder
/// under a root that cannot be read is left out of the view. Both
/// are looked at off the window's thread.
#[test]
fn an_offline_root_is_listed_dimmed_and_its_frames_left_alone() {
    let dir = scratch("offline");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(a.join("day")).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    frames(&a, &["x.tif"]);
    frames(&a.join("day"), &["y.tif"]);
    let z = frames(&b, &["z.tif"]).remove(0);
    let mut s = Sidecar::default();
    s.meta.rating = 3;
    s.meta.flag = meta::Flag::Pick;
    s.save(&z).unwrap();
    let z_sidecar = Sidecar::path_for(&z);
    let z_bytes = std::fs::read(&z_sidecar).unwrap();
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.write_sidecars = true;
        st.library.roots.add(&a).unwrap();
        st.library.roots.add(&b).unwrap();
        recount(&mut st);
    }
    // b unplugged: the folder gone from the disk.
    let away = dir.join("b-away");
    crate::testing::rename_away(&b, &away);
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        let chips = app.get_library_roots();
        assert!(!chips.row_data(0).unwrap().offline);
        assert!(chips.row_data(1).unwrap().offline);
        assert_eq!(
            st.files,
            [a.join("x.tif"), a.join("day").join("y.tif"), z.clone()],
            "the offline root's frame is listed"
        );
        assert_eq!(st.library.counts, vec![2, 1], "b's row kept as it was");
        let thumbs = app.get_thumbs();
        assert!(!thumbs.row_data(0).unwrap().offline);
        let tile = thumbs.row_data(2).unwrap();
        assert!(tile.offline, "dimmed");
        assert_eq!((tile.rating, tile.flag), (3, 1), "badged from its row");
        assert!(!st.from_row[2].read);
        assert!(
            st.from_row[2].key.is_some(),
            "the cache's key, from the row"
        );
        assert_eq!(disk_reads_under(&dir), 0);
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
    // The filter works on it.
    {
        let mut st = state.borrow_mut();
        st.filter = filter::Filter::from_name("Picks").unwrap();
        rebuild_browser(&mut st, &app);
        assert_eq!(st.shown, [2], "the pick under the offline root");
    }
    // Opening it: selected, and told why there is no picture.
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::open_row(&mut st, &app, &worker, 0, false);
        assert_eq!(st.current, Some(2));
        assert!(!st.from_row[2].read, "nothing read");
        assert!(
            app.get_status().contains("is offline"),
            "{}",
            app.get_status()
        );
        // A key: nothing changes, and nothing is written.
        app.set_status("".into());
        let (_, next) =
            crate::panel::browser::set_meta(&mut st, &app, &[2], meta::Change::Rating(5));
        assert_eq!(next, None);
        assert_eq!(st.sidecars[2].meta.rating, 3);
        assert!(
            app.get_status().contains("is offline"),
            "{}",
            app.get_status()
        );
        assert_eq!(disk_reads_under(&dir), 0);
    }
    // The sidecar went away with its root, and is as it was there.
    let moved = away.join(z_sidecar.file_name().unwrap());
    assert_eq!(
        std::fs::read(&moved).unwrap(),
        z_bytes,
        "the sidecar as it was"
    );
    assert!(!b.exists(), "nothing written where the root was");
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
                [a.join("x.tif"), z.clone()],
                "the locked folder's frame left out"
            );
        }
    }
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
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
    crate::testing::remove_dir_retry(&dir);
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
    crate::testing::remove_dir_retry(&dir);
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
    crate::testing::remove_dir_retry(&dir);
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
    crate::testing::remove_dir_retry(&dir);
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
    crate::testing::rename_away(&b, &dir.join("b-away"));
    assert!(refresh_view(&state, &app, &worker));
    land_all(&state, &app, &worker);
    let st = state.borrow();
    assert!(st.library.offline.contains(&b));
    assert_eq!(st.current.map(|c| st.files[c].clone()), Some(y.clone()));
    assert_eq!(
        st.files,
        [a.join("x.tif"), y, b.join("z.tif")],
        "the root's frames stay, dimmed, the frame on screen among them"
    );
    let thumbs = app.get_thumbs();
    assert!(!thumbs.row_data(0).unwrap().offline);
    assert!(thumbs.row_data(1).unwrap().offline);
    assert!(thumbs.row_data(2).unwrap().offline);
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}
