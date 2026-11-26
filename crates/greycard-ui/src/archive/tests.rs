use super::*;
use greycard_edit::Placement;
use greycard_library::fixture::{A7, R5, R6, write_frame};

pub fn scratch(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "greycard-ui-archive-{what}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dunce::canonicalize(&dir).unwrap()
}

/// A rated sidecar on `frame`, saved `times` times, in `placement`.
fn rate(frame: &Path, stars: u8, times: u32, placement: Placement) {
    let mut s = Sidecar::load_as_file(frame)
        .ok()
        .flatten()
        .unwrap_or_default();
    s.meta.rating = stars;
    for _ in 0..times {
        s.save_in(frame, placement).unwrap();
    }
}

/// A shoot of three frames under `root/shoot`, the second rated
/// beside, the third rated under the hidden folder; and a rejects
/// folder in the shoot with one frame.
fn shoot(root: &Path) -> Vec<PathBuf> {
    let dir = root.join("shoot");
    std::fs::create_dir_all(dir.join(crate::rejects::REJECTS)).unwrap();
    let files: Vec<PathBuf> = ["a.tif", "b.tif", "c.tif"]
        .iter()
        .map(|n| dir.join(n))
        .collect();
    for (i, (f, cam)) in files.iter().zip([&R5, &R6, &A7]).enumerate() {
        write_frame(f, cam, i as u16 + 1);
    }
    rate(&files[1], 3, 1, Placement::Beside);
    rate(&files[2], 2, 1, Placement::Folder);
    write_frame(&dir.join(crate::rejects::REJECTS).join("r.tif"), &R5, 9);
    files
}

fn quiet_hooks<'a>(cancel: &'a AtomicBool, bytes: &'a AtomicU64) -> Hooks<'a> {
    Hooks::new(&|| {}, cancel, bytes)
}

fn index(db: &Path, roots: &[&Path]) {
    let mut lib = Library::open(db).unwrap();
    for r in roots {
        lib.index_tree(r, &mut |_| {}).unwrap();
    }
}

fn backup_ask(local: &Path, nas: &Path, db: &Path, rejects: bool) -> Ask {
    Ask {
        direction: Direction::BackUp,
        frames: Frames::Folder(local.join("shoot")),
        include_rejects: rejects,
        sidecars: true,
        base: local.to_path_buf(),
        dest: nas.join("local"),
        other_side: vec![nas.to_path_buf()],
        skip: vec![nas.to_path_buf()],
        index: Some(db.to_path_buf()),
    }
}

#[test]
fn a_folder_is_backed_up_whole_with_its_sidecars_in_place_and_the_rejects_left() {
    let dir = scratch("backup");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    std::fs::create_dir_all(&nas).unwrap();
    let files = shoot(&local);
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let ask = backup_ask(&local, &nas, &db, false);
    let plan = plan(&ask, &no_beat());
    assert_eq!(plan.rejects, 1);
    assert_eq!(plan.copies().0, 3);
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    let report = run(&plan, &quiet_hooks(&cancel, &bytes));
    assert_eq!(report.copied.len(), 3, "{report:?}");
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(report.sidecar_files, 2);
    let there = nas.join("local").join("shoot");
    for f in &files {
        let copy = there.join(f.file_name().unwrap());
        assert_eq!(std::fs::read(f).unwrap(), std::fs::read(&copy).unwrap());
        // The time kept, so the index's size and mtime agree.
        assert_eq!(
            std::fs::metadata(f).unwrap().modified().unwrap(),
            std::fs::metadata(&copy).unwrap().modified().unwrap()
        );
    }
    // The placements kept: b beside, c under the hidden folder.
    assert!(there.join("b.tif.gcd").is_file());
    assert!(there.join(SIDECAR_FOLDER).join("c.tif.gcd").is_file());
    assert!(!there.join("c.tif.gcd").exists());
    // The rejects left out, and nothing temporary left behind.
    assert!(!there.join(crate::rejects::REJECTS).exists());
    let temps = std::fs::read_dir(&there)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(TEMPORARY))
        .count();
    assert_eq!(temps, 0);
    assert_eq!(
        bytes.load(Ordering::Relaxed),
        files
            .iter()
            .map(|f| std::fs::metadata(f).unwrap().len())
            .sum::<u64>()
    );
    assert_eq!(report.folders, std::slice::from_ref(&there));

    // The rows written, with their whole hashes, on both sides.
    record(&db, &report, &no_beat());
    let lib = Library::open_read_only(&db).unwrap();
    let whole = crate::import::hash_whole(&files[0]).unwrap();
    assert_eq!(lib.whole_hash(&files[0]).unwrap(), Some(whole.clone()));
    assert_eq!(lib.whole_hash(&there.join("a.tif")).unwrap(), Some(whole));
    // Closed before the folder goes: Windows refuses to remove an
    // open file.
    drop(lib);

    // Asked again: everything there, nothing to do.
    let again = super::plan(&ask, &no_beat());
    assert!(!again.has_work(), "{again:?}");
    assert_eq!(again.same(), (3, 0));

    // The rejects asked for: the one frame, under its folder.
    let with = super::plan(&backup_ask(&local, &nas, &db, true), &no_beat());
    assert_eq!(with.rejects, 1);
    assert_eq!(with.copies().0, 1);
    run(&with, &quiet_hooks(&cancel, &bytes));
    assert!(there.join(crate::rejects::REJECTS).join("r.tif").is_file());
    greycard_render::testing::remove_dir_retry(&dir);
}

/// A shoot renamed here after its backup, or an archive reorganized
/// by hand, is still backed up: found by its hash, anywhere under
/// the archive, and nothing copied a second time.
#[test]
fn a_frame_already_on_the_archive_under_another_folder_is_not_copied_again() {
    let dir = scratch("hash-first");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    let files = shoot(&local);
    let elsewhere = nas.join("clients").join("by hand");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::copy(&files[0], elsewhere.join("renamed.tif")).unwrap();
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
    let a = plan.items.iter().find(|i| i.frame == files[0]).unwrap();
    assert_eq!(
        a.what,
        What::Same {
            there: elsewhere.join("renamed.tif"),
            elsewhere: true
        }
    );
    assert_eq!(plan.same(), (1, 1));
    assert_eq!(plan.copies().0, 2);

    // A row with no file behind it is a frame not there.
    std::fs::remove_file(elsewhere.join("renamed.tif")).unwrap();
    let plan = super::plan(&backup_ask(&local, &nas, &db, false), &no_beat());
    assert_eq!(plan.copies().0, 3);
    greycard_render::testing::remove_dir_retry(&dir);
}

#[test]
fn a_copy_that_does_not_read_back_the_same_is_removed_and_named() {
    let dir = scratch("mismatch");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    std::fs::create_dir_all(&nas).unwrap();
    let files = shoot(&local);
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    // The temporary of a.tif turned bad between the write and the
    // read-back, as a failing disk or a cable pulled would.
    let tamper = |tmp: &Path| {
        if tmp.to_string_lossy().contains(".a.tif.") {
            let mut b = std::fs::read(tmp).unwrap();
            b[100] ^= 0xff;
            std::fs::write(tmp, b).unwrap();
        }
    };
    let hooks = Hooks {
        tamper: Some(&tamper),
        ..quiet_hooks(&cancel, &bytes)
    };
    let report = run(&plan, &hooks);
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(report.failed[0].0, files[0]);
    assert!(report.failed[0].1.contains("does not read back the same"));
    let there = nas.join("local").join("shoot");
    assert!(!there.join("a.tif").exists());
    assert!(there.join("b.tif").is_file());
    let left: Vec<_> = std::fs::read_dir(&there)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(TEMPORARY))
        .collect();
    assert!(left.is_empty(), "the temporary removed");
    greycard_render::testing::remove_dir_retry(&dir);
}

#[test]
fn a_temporary_left_by_a_crash_is_removed_by_the_next_run_and_said() {
    let dir = scratch("stale");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    let there = nas.join("local").join("shoot");
    std::fs::create_dir_all(&there).unwrap();
    shoot(&local);
    // A process that cannot be running here, an hour and more ago.
    let dead = u32::MAX;
    let aged = |p: &Path| {
        std::fs::write(p, b"half a frame").unwrap();
        let then = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
        std::fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(then)
            .unwrap();
    };
    let stale = there.join(format!(".a.tif.{dead}-0{TEMPORARY}"));
    aged(&stale);
    // Written to a moment ago: a copy under way on another machine.
    let fresh = there.join(format!(".b.tif.{dead}-1{TEMPORARY}"));
    std::fs::write(&fresh, b"being written").unwrap();
    // This process's own, set aside and still writing, however old.
    let own = there.join(format!(".c.tif.{}-2{TEMPORARY}", std::process::id()));
    aged(&own);
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
    // Never counted as the frame.
    assert_eq!(plan.copies().0, 3);
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    let report = run(&plan, &quiet_hooks(&cancel, &bytes));
    assert_eq!(report.stale, std::slice::from_ref(&stale));
    assert!(!stale.exists());
    assert!(fresh.exists() && own.exists(), "a live writer's are kept");
    greycard_render::testing::remove_dir_retry(&dir);
}

/// A frame there already takes the newer `.gcd` across, in either
/// direction (§233): here's when it is the newer, the archive's
/// home when it went on from this one; a frame with no sidecar of
/// its own against one there is left and named.
#[test]
fn a_newer_sidecar_goes_across_in_either_direction_and_a_bare_frame_is_named() {
    let dir = scratch("sidecars");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    std::fs::create_dir_all(&nas).unwrap();
    let files = shoot(&local);
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let ask = backup_ask(&local, &nas, &db, false);
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    run(&plan(&ask, &no_beat()), &quiet_hooks(&cancel, &bytes));
    let there = nas.join("local").join("shoot");
    // Here: b culled again after the backup (saved twice more).
    rate(&files[1], 5, 2, Placement::Beside);
    // There: c edited on the archive's side, from another machine.
    rate(&there.join("c.tif"), 4, 3, Placement::Folder);
    // And a, never rated here, rated on the archive.
    rate(&there.join("a.tif"), 1, 1, Placement::Beside);
    index(&db, &[&local, &nas]);
    let plan = plan(&ask, &no_beat());
    assert_eq!(plan.copies().0, 0, "{plan:?}");
    assert_eq!(plan.sidecars_only(), 2, "{plan:?}");
    assert_eq!(plan.theirs_newer(), [files[0].as_path()]);
    let report = run(&plan, &quiet_hooks(&cancel, &bytes));
    assert_eq!((report.sidecars_only, report.sidecar_files), (2, 2));
    let read = |f: &Path| Sidecar::load_as_file(f).unwrap().unwrap().meta.rating;
    assert_eq!(read(&there.join("b.tif")), 5);
    assert_eq!(
        read(&there.join("c.tif")),
        4,
        "the archive's newer edit kept"
    );
    assert_eq!(
        read(&files[2]),
        4,
        "and brought home: it went on from this one"
    );
    assert!(
        !files[2].with_file_name("c.tif.gcd").exists(),
        "to where c keeps its own: the hidden folder"
    );

    // Bring back, the other way: a's sidecar comes home, and b and
    // c, the same on both sides now, are skipped.
    let back = Ask {
        direction: Direction::BringBack,
        frames: Frames::Folder(there.clone()),
        include_rejects: false,
        sidecars: true,
        base: nas.join("local"),
        dest: local.clone(),
        other_side: vec![local.clone()],
        skip: vec![local.clone()],
        index: Some(db.clone()),
    };
    index(&db, &[&local, &nas]);
    let plan = super::plan(&back, &no_beat());
    assert_eq!(plan.copies().0, 0, "{plan:?}");
    assert_eq!(plan.sidecars_only(), 1, "a: {plan:?}");
    assert_eq!(plan.same(), (2, 0));
    run(&plan, &quiet_hooks(&cancel, &bytes));
    assert_eq!(read(&files[2]), 4);
    assert_eq!(read(&files[0]), 1);
    greycard_render::testing::remove_dir_retry(&dir);
}

/// Two `.gcd`s of one frame edited on both sides since the backup
/// (§233): Back up joins them and writes the join to both, so
/// neither side's edit is lost and the two read as the same; Bring
/// back then finds them the same. A copy that is simply behind is
/// written over whole, either way.
#[test]
fn a_pair_edited_on_both_sides_is_joined_by_back_up_and_the_same_to_bring_back() {
    use greycard_edit::sync::{Compared, compare};
    let dir = scratch("joined");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    std::fs::create_dir_all(&nas).unwrap();
    let files = shoot(&local);
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let ask = backup_ask(&local, &nas, &db, false);
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    run(&plan(&ask, &no_beat()), &quiet_hooks(&cancel, &bytes));
    let there = nas.join("local").join("shoot");
    // b: rated 5 here, and 4 on the archive from another machine,
    // each on from the backup's copy.
    rate(&files[1], 5, 1, Placement::Beside);
    {
        let mut theirs = Sidecar::load_as_file(&there.join("b.tif"))
            .unwrap()
            .unwrap();
        theirs.meta.rating = 4;
        theirs.meta.label = greycard_edit::meta::Label::Blue;
        theirs
            .save_in_stamped(
                &there.join("b.tif"),
                Placement::Beside,
                &greycard_edit::sync::Stamp {
                    at: 1 << 40,
                    host: "laptop".into(),
                },
            )
            .unwrap();
    }
    // c: edited here only; the archive's copy is behind.
    rate(&files[2], 1, 2, Placement::Folder);
    index(&db, &[&local, &nas]);
    let plan = plan(&ask, &no_beat());
    assert_eq!(plan.copies().0, 0, "{plan:?}");
    assert_eq!(plan.sidecars_only(), 2, "{plan:?}");
    assert!(plan.theirs_newer().is_empty(), "{plan:?}");
    let item = plan.items.iter().find(|i| i.frame == files[1]).unwrap();
    match &item.what {
        What::Sidecars {
            join,
            sidecars,
            held,
            ..
        } => {
            assert!(join.is_some(), "{item:?}");
            assert!(sidecars.is_empty() && !held);
        }
        other => panic!("{other:?}"),
    }
    let report = run(&plan, &quiet_hooks(&cancel, &bytes));
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!((report.sidecars_only, report.sidecar_files), (2, 3));
    let here = Sidecar::load_as_file(&files[1]).unwrap().unwrap();
    let copy = Sidecar::load_as_file(&there.join("b.tif"))
        .unwrap()
        .unwrap();
    assert_eq!(compare(&here, &copy), Compared::Same, "{here:?}\n{copy:?}");
    // The join holds both edits: the label set there alone, and
    // the rating, set on both sides, the later save's.
    assert_eq!(
        (here.meta.rating, here.meta.label),
        (4, greycard_edit::meta::Label::Blue)
    );
    assert_eq!(
        here.revisions.len(),
        4,
        "both branches' revisions, and the join's"
    );
    assert_eq!(
        Sidecar::load_as_file(&there.join("c.tif"))
            .unwrap()
            .unwrap()
            .meta
            .rating,
        1,
        "the copy behind written over whole"
    );
    // Bring back: nothing to do for either now.
    let back = Ask {
        direction: Direction::BringBack,
        frames: Frames::Folder(there.clone()),
        include_rejects: false,
        sidecars: true,
        base: nas.join("local"),
        dest: local.clone(),
        other_side: vec![local.clone()],
        skip: vec![local.clone()],
        index: Some(db.clone()),
    };
    index(&db, &[&local, &nas]);
    let plan = super::plan(&back, &no_beat());
    assert_eq!(plan.sidecars_only(), 0, "{plan:?}");
    assert_eq!(plan.same(), (3, 0), "{plan:?}");
    // Edited on both sides again: Bring back joins too, and the
    // frame here is the saved side.
    rate(&files[1], 2, 1, Placement::Beside);
    {
        let mut theirs = Sidecar::load_as_file(&there.join("b.tif"))
            .unwrap()
            .unwrap();
        theirs.meta.label = greycard_edit::meta::Label::Red;
        theirs
            .save_in_stamped(
                &there.join("b.tif"),
                Placement::Beside,
                &greycard_edit::sync::Stamp {
                    at: 1 << 41,
                    host: "laptop".into(),
                },
            )
            .unwrap();
    }
    let plan = super::plan(&back, &no_beat());
    assert_eq!(plan.sidecars_only(), 1, "{plan:?}");
    let report = run(&plan, &quiet_hooks(&cancel, &bytes));
    assert!(report.failed.is_empty(), "{report:?}");
    let here = Sidecar::load_as_file(&files[1]).unwrap().unwrap();
    let copy = Sidecar::load_as_file(&there.join("b.tif"))
        .unwrap()
        .unwrap();
    assert_eq!(compare(&here, &copy), Compared::Same);
    assert_eq!(
        (here.meta.rating, here.meta.label),
        (2, greycard_edit::meta::Label::Red)
    );
    greycard_render::testing::remove_dir_retry(&dir);
}

/// Probe L: Back up planned while the archive's copy is ahead (to
/// come home), and a rating made here while the sheet was up. The
/// run compares again: the two are a join now, and the rating is
/// kept with the archive's label.
#[test]
fn a_save_between_the_plan_and_the_run_is_kept() {
    let dir = scratch("stale-plan");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    std::fs::create_dir_all(&nas).unwrap();
    let files = shoot(&local);
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let ask = backup_ask(&local, &nas, &db, false);
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    run(&plan(&ask, &no_beat()), &quiet_hooks(&cancel, &bytes));
    let there = nas.join("local").join("shoot");
    {
        let mut theirs = Sidecar::load_as_file(&there.join("b.tif"))
            .unwrap()
            .unwrap();
        theirs.meta.label = greycard_edit::meta::Label::Blue;
        theirs
            .save_in_stamped(
                &there.join("b.tif"),
                Placement::Beside,
                &greycard_edit::sync::Stamp {
                    at: 1 << 40,
                    host: "laptop".into(),
                },
            )
            .unwrap();
    }
    index(&db, &[&local, &nas]);
    let planned = plan(&ask, &no_beat());
    assert_eq!(planned.sidecars_only(), 1, "{planned:?}");
    // The user rates b here while the sheet is up.
    rate(&files[1], 5, 1, Placement::Beside);
    let report = run(&planned, &quiet_hooks(&cancel, &bytes));
    assert!(report.failed.is_empty(), "{report:?}");
    let here = Sidecar::load_as_file(&files[1]).unwrap().unwrap();
    assert_eq!(
        (here.meta.rating, here.meta.label),
        (5, greycard_edit::meta::Label::Blue)
    );
    let copy = Sidecar::load_as_file(&there.join("b.tif"))
        .unwrap()
        .unwrap();
    assert_eq!(here, copy);
    greycard_render::testing::remove_dir_retry(&dir);
}

/// Bring back skips a frame already under a local root, wherever it
/// is there, and names it when the local sidecar is the newer.
#[test]
fn bring_back_skips_a_frame_already_local() {
    let dir = scratch("bring-back");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    let on_nas = nas.join("2024").join("shoot");
    std::fs::create_dir_all(&on_nas).unwrap();
    let old = ["x.tif", "y.tif"].map(|n| on_nas.join(n));
    write_frame(&old[0], &R5, 1);
    write_frame(&old[1], &R6, 2);
    // y is here already, under another name and folder, and rated
    // here since.
    let mine = local.join("kept").join("y-copy.tif");
    std::fs::create_dir_all(mine.parent().unwrap()).unwrap();
    std::fs::copy(&old[1], &mine).unwrap();
    rate(&mine, 5, 1, Placement::Beside);
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let ask = Ask {
        direction: Direction::BringBack,
        frames: Frames::Chosen(old.to_vec()),
        include_rejects: false,
        sidecars: true,
        base: nas.clone(),
        dest: local.join("from-archive"),
        other_side: vec![local.clone()],
        skip: vec![local.clone()],
        index: Some(db.clone()),
    };
    let plan = plan(&ask, &no_beat());
    assert_eq!(plan.copies().0, 1);
    assert_eq!(plan.theirs_newer(), [old[1].as_path()]);
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    let report = run(&plan, &quiet_hooks(&cancel, &bytes));
    assert_eq!(report.copied.len(), 1);
    assert!(
        local
            .join("from-archive")
            .join("2024")
            .join("shoot")
            .join("x.tif")
            .is_file()
    );
    assert!(
        !local
            .join("from-archive")
            .join("2024")
            .join("shoot")
            .join("y.tif")
            .exists()
    );
    greycard_render::testing::remove_dir_retry(&dir);
}

/// Another file at the destination's name is never written over.
#[test]
fn a_different_file_at_the_name_is_left_and_named() {
    let dir = scratch("taken");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    let files = shoot(&local);
    let there = nas.join("local").join("shoot");
    std::fs::create_dir_all(&there).unwrap();
    std::fs::write(there.join("a.tif"), b"another picture").unwrap();
    let db = dir.join("library.sqlite");
    index(&db, &[&local]);
    let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
    assert_eq!(plan.taken(), [files[0].as_path()]);
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    run(&plan, &quiet_hooks(&cancel, &bytes));
    assert_eq!(
        std::fs::read(there.join("a.tif")).unwrap(),
        b"another picture"
    );
    greycard_render::testing::remove_dir_retry(&dir);
}

#[test]
fn a_cancel_stops_between_frames() {
    let dir = scratch("cancel");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    std::fs::create_dir_all(&nas).unwrap();
    shoot(&local);
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    // Canceled while the first frame is copied: it finishes, and
    // the rest are not begun.
    let hand = |_: &Path| cancel.store(true, Ordering::SeqCst);
    let hooks = Hooks {
        before_frame: Some(&hand),
        ..quiet_hooks(&cancel, &bytes)
    };
    let report = run(&plan, &hooks);
    assert_eq!(report.copied.len(), 1);
    assert_eq!(report.canceled, 2);
    greycard_render::testing::remove_dir_retry(&dir);
}

/// The share-hang path, with a slow fake: a job that beats, then
/// says nothing for longer than the wait, is set aside, and its end
/// lands later, late; one that keeps beating is never set aside.
#[test]
fn a_job_that_goes_quiet_is_set_aside_and_lands_late() {
    let (tx, rx) = mpsc::channel();
    let held = Arc::new(AtomicBool::new(true));
    let hold = held.clone();
    supervise(
        "test hang",
        Duration::from_millis(100),
        move |beat| {
            beat();
            // The share stops answering until the test lets go.
            while hold.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            beat();
            7
        },
        move |h| {
            let _ = tx.send(match h {
                Heard::Aside | Heard::Lost => None,
                Heard::Done { result, late } => Some((result, late)),
            });
        },
    )
    .unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), None);
    held.store(false, Ordering::SeqCst);
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        Some((7, true))
    );

    let (tx, rx) = mpsc::channel();
    supervise(
        "test slow",
        // Well over the beats' gap: a loaded runner (macOS CI) can
        // sleep 20 ms for a good deal longer, and the clock's wait
        // under load is no test's business.
        Duration::from_secs(2),
        |beat| {
            // Slow, but saying so: twenty beats 20 ms apart.
            for _ in 0..20 {
                std::thread::sleep(Duration::from_millis(20));
                beat();
            }
            1
        },
        move |h| {
            let _ = tx.send(match h {
                Heard::Aside | Heard::Lost => None,
                Heard::Done { result, late } => Some((result, late)),
            });
        },
    )
    .unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        Some((1, false))
    );
}

/// The copy itself beats as it goes: a stall in the middle of a
/// frame, behind a supervisor with a short wait, sets the run aside,
/// and once let go it finishes the frame and stops there, the
/// cancel the window set when it heard.
#[test]
fn a_copy_that_stalls_mid_run_is_set_aside_and_stops_after_its_frame() {
    let dir = scratch("stall");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    std::fs::create_dir_all(&nas).unwrap();
    shoot(&local);
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
    let cancel = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(true));
    let (tx, rx) = mpsc::channel();
    let (job_cancel, hold) = (cancel.clone(), held.clone());
    supervise(
        "test stall",
        // Longer than a sync of the first frame on a slow disk
        // (the Windows runner's), which does not beat: the stall
        // this test means is the hold on the second frame.
        Duration::from_secs(1),
        move |beat| {
            let bytes = AtomicU64::new(0);
            let hand = |f: &Path| {
                if f.ends_with("b.tif") {
                    while hold.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            };
            let hooks = Hooks {
                beat: &**beat,
                cancel: &job_cancel,
                bytes: &bytes,
                before_frame: Some(&hand),
                tamper: None,
                on_sync: None,
            };
            run(&plan, &hooks)
        },
        move |h| {
            let _ = tx.send(h);
        },
    )
    .unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        Heard::Aside
    ));
    // What the window does on hearing it: no more frames after this.
    cancel.store(true, Ordering::SeqCst);
    held.store(false, Ordering::SeqCst);
    match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
        Heard::Done { result, late } => {
            assert!(late);
            assert_eq!(result.copied.len(), 2, "a, and b once let go");
            assert_eq!(result.canceled, 1);
        }
        Heard::Aside | Heard::Lost => panic!("set aside twice, or lost"),
    }
    greycard_render::testing::remove_dir_retry(&dir);
}

/// A share that stops answering inside a copy's sync, the one that
/// does not beat, is still set aside once the wait is up, and the
/// run, let go, ends its frame and reports the rest not copied.
#[test]
fn a_copy_whose_sync_hangs_is_set_aside_and_reports_the_rest() {
    let dir = scratch("sync-hang");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    std::fs::create_dir_all(&nas).unwrap();
    shoot(&local);
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
    let cancel = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(true));
    let (tx, rx) = mpsc::channel();
    let (job_cancel, hold) = (cancel.clone(), held.clone());
    supervise(
        "test sync hang",
        Duration::from_millis(500),
        move |beat| {
            let bytes = AtomicU64::new(0);
            let on_sync = |_: u64| {
                while hold.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                None
            };
            let hooks = Hooks {
                on_sync: Some(&on_sync),
                ..Hooks::new(&**beat, &job_cancel, &bytes)
            };
            run(&plan, &hooks)
        },
        move |h| {
            let _ = tx.send(h);
        },
    )
    .unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        Heard::Aside
    ));
    cancel.store(true, Ordering::SeqCst);
    held.store(false, Ordering::SeqCst);
    match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
        Heard::Done { result, late } => {
            assert!(late);
            assert_eq!(result.copied.len(), 1, "the frame in hand lands");
            assert_eq!(result.canceled, 2);
            let (from, to, _, whole) = &result.copied[0];
            assert_eq!(&crate::import::hash_whole(from).unwrap(), whole);
            assert_eq!(&crate::import::hash_whole(to).unwrap(), whole);
        }
        Heard::Aside | Heard::Lost => panic!("set aside twice, or lost"),
    }
    greycard_render::testing::remove_dir_retry(&dir);
}

/// A link into a share is the share: the folder asked about is the one
/// the link leads to, found from the nearest folder there is.
#[cfg(unix)]
#[test]
fn a_link_into_a_share_is_far() {
    let dir = scratch("far-link");
    let share = dir.join("share");
    std::fs::create_dir_all(share.join("year")).unwrap();
    let link = dir.join("archive");
    std::os::unix::fs::symlink(&share, &link).unwrap();
    let canonical = dunce::canonicalize(&share).unwrap();
    let remote = |p: &Path| p.starts_with(&canonical);
    assert!(remote_by(&link.join("year/new shoot/a.cr3"), &remote));
    assert!(remote_by(&link, &remote));
    assert!(!remote_by(&dir.join("local/shoot"), &remote));
    greycard_render::testing::remove_dir_retry(&dir);
}

/// The index of a landed copy that has to read it whole, over a share
/// as slow as 250 ms a quarter megabyte, beats at every read: under a
/// 2 s watch it is never set aside.
#[test]
fn an_index_read_whole_off_a_slow_share_beats_in_time() {
    struct Slow(usize);
    impl std::io::Read for Slow {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            assert!(buf.len() <= 256 << 10, "a read too big to beat in time");
            std::thread::sleep(Duration::from_millis(buf.len() as u64 * 1000 / (1 << 20)));
            let k = buf.len().min(self.0);
            buf[..k].fill(7);
            self.0 -= k;
            Ok(k)
        }
    }
    let n = (3 << 20) + 5;
    let (tx, rx) = mpsc::channel();
    supervise(
        "test index read",
        Duration::from_secs(2),
        move |beat| greycard_core::decode::read_beating(&mut Slow(n), n, &**beat).map(|b| b.len()),
        move |h| {
            let _ = tx.send(match h {
                Heard::Done { result, late } => Some((result.unwrap(), late)),
                Heard::Aside | Heard::Lost => None,
            });
        },
    )
    .unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(30)).unwrap(),
        Some((n, false))
    );
}

/// A big folder on a slow share lists for a while: the sweep beats at
/// every entry, not the folder once, and still takes only a stale
/// temporary of no process running here.
#[test]
fn the_sweep_beats_at_every_entry() {
    let dir = scratch("sweep-beats");
    let n = 40;
    for i in 0..n {
        std::fs::write(dir.join(format!("IMG_{i:04}.CR3")), b"x").unwrap();
    }
    let stale = dir.join(format!(".IMG_9999.CR3{TEMPORARY}"));
    std::fs::write(&stale, b"half").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(SystemTime::now() - STALE_AFTER - Duration::from_secs(60))
        .unwrap();
    let beats = std::cell::Cell::new(0usize);
    let gone = sweep(&dir, &|| beats.set(beats.get() + 1));
    assert_eq!(gone, [stale]);
    assert_eq!(beats.get(), n + 1);
    greycard_render::testing::remove_dir_retry(&dir);
}

#[test]
fn all_roots_hides_the_archives_copies_of_local_frames() {
    let rows = vec![
        (PathBuf::from("/local/a.cr3"), "ha".to_string()),
        (PathBuf::from("/nas/old/z.cr3"), "hz".to_string()),
        (PathBuf::from("/nas/local/a.cr3"), "ha".to_string()),
        (
            PathBuf::from("/nas/elsewhere/a-renamed.cr3"),
            "ha".to_string(),
        ),
        (PathBuf::from("/other/b.cr3"), "hb".to_string()),
    ];
    let kept: Vec<PathBuf> = hide_local_copies(
        rows.clone(),
        &[PathBuf::from("/nas")],
        |h| h.as_str(),
        |_| true,
    )
    .into_iter()
    .map(|(p, _)| p)
    .collect();
    assert_eq!(
        kept,
        [
            PathBuf::from("/local/a.cr3"),
            PathBuf::from("/nas/old/z.cr3"),
            PathBuf::from("/other/b.cr3")
        ]
    );
    // The local copy not there (its root offline, its folder
    // gone): the archive's copies are the frame, and listed.
    let kept: Vec<PathBuf> = hide_local_copies(
        rows.clone(),
        &[PathBuf::from("/nas")],
        |h| h.as_str(),
        |p| !p.starts_with("/local"),
    )
    .into_iter()
    .map(|(p, _)| p)
    .collect();
    assert_eq!(kept.len(), 5, "{kept:?}");
    // No archive: every row.
    assert_eq!(
        hide_local_copies(rows.clone(), &[], |h| h.as_str(), |_| true).len(),
        5
    );
}

#[test]
fn the_counts_find_each_frame_on_each_archive() {
    let dir = scratch("counts");
    let (local, nas, cloud) = (dir.join("local"), dir.join("nas"), dir.join("cloud"));
    std::fs::create_dir_all(&nas).unwrap();
    std::fs::create_dir_all(&cloud).unwrap();
    let files = shoot(&local);
    std::fs::copy(&files[0], nas.join("a.tif")).unwrap();
    std::fs::copy(&files[0], cloud.join("a.tif")).unwrap();
    std::fs::copy(&files[1], cloud.join("b.tif")).unwrap();
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas, &cloud]);
    let lib = Library::open_read_only(&db).unwrap();
    let on = on_archives(
        &files,
        &[nas.clone(), cloud.clone()],
        Some(&lib),
        &|| {},
        &|| false,
    )
    .unwrap();
    let ons: Vec<Vec<usize>> = on.iter().map(|w| w.on.clone()).collect();
    assert_eq!(ons, vec![vec![0, 1], vec![1], vec![]]);
    assert!(on.iter().all(|w| w.unknown.is_empty()));
    // Stopped: nothing.
    assert_eq!(
        on_archives(
            &files,
            std::slice::from_ref(&nas),
            Some(&lib),
            &|| {},
            &|| true
        ),
        None
    );
    drop(lib);
    // A copy that did not keep its time: unknown to a count, which
    // reads nothing whole, and no whole hash is taken.
    let late = nas.join("c.tif");
    std::fs::copy(&files[2], &late).unwrap();
    set_time(&late, Duration::from_secs(60 * 60));
    index(&db, &[&nas]);
    let lib = Library::open_read_only(&db).unwrap();
    let on = on_archives(
        std::slice::from_ref(&files[2]),
        std::slice::from_ref(&nas),
        Some(&lib),
        &|| {},
        &|| false,
    )
    .unwrap();
    assert_eq!(
        on,
        vec![Where {
            on: Vec::new(),
            unknown: vec![0]
        }]
    );
    assert_eq!(lib.whole_hash(&late).unwrap(), None);
    assert_eq!(lib.whole_hash(&files[2]).unwrap(), None);
    drop(lib);
    greycard_render::testing::remove_dir_retry(&dir);
}

#[test]
fn a_sidecar_is_named_after_the_frame_it_goes_with() {
    let frame = Path::new("/here/IMG_1.CR3");
    let to = Path::new("/there/x/renamed.CR3");
    assert_eq!(
        sidecar_at(Path::new("/here/IMG_1.CR3.gcd"), frame, to),
        PathBuf::from("/there/x/renamed.CR3.gcd")
    );
    assert_eq!(
        sidecar_at(Path::new("/here/.greycard/IMG_1.CR3.gcd"), frame, to),
        PathBuf::from("/there/x/.greycard/renamed.CR3.gcd")
    );
    assert_eq!(
        sidecar_at(Path::new("/here/IMG_1.xmp"), frame, to),
        PathBuf::from("/there/x/renamed.xmp")
    );
    assert_eq!(
        sidecar_at(Path::new("/here/IMG_1.CR3.xmp"), frame, to),
        PathBuf::from("/there/x/renamed.CR3.xmp")
    );
}

fn set_time(p: &Path, ago: Duration) {
    std::fs::File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(SystemTime::now() - ago)
        .unwrap();
}

/// Each sidecar file is judged on its own: a `.gcd` newer here by
/// its counter goes across, while the archive's XMP, edited later on
/// another machine, stays as it is and names the frame.
#[test]
fn a_newer_xmp_over_there_is_kept_while_the_newer_gcd_goes() {
    let dir = scratch("per-file");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    std::fs::create_dir_all(&nas).unwrap();
    let files = shoot(&local);
    let b = &files[1];
    let mut meta = Sidecar::load_as_file(b).unwrap().unwrap().meta;
    greycard_edit_io::xmp::save(b, &meta, None).unwrap();
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let ask = backup_ask(&local, &nas, &db, false);
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    run(&plan(&ask, &no_beat()), &quiet_hooks(&cancel, &bytes));
    let there = nas.join("local").join("shoot").join("b.tif");
    let xmps_there = greycard_edit_io::xmp::paths_of(&there);
    assert_eq!(xmps_there.len(), 1);
    // Here: two more saves of the edit, and the XMP an hour old.
    rate(b, 5, 2, Placement::Beside);
    for x in greycard_edit_io::xmp::paths_of(b) {
        set_time(&x, Duration::from_secs(60 * 60));
    }
    // There: the XMP written again, now, by another tool.
    meta.label = greycard_edit::meta::Label::Green;
    greycard_edit_io::xmp::save(&there, &meta, None).unwrap();
    let theirs_before = std::fs::read(&xmps_there[0]).unwrap();
    index(&db, &[&local, &nas]);
    let plan = plan(&ask, &no_beat());
    let item = plan.items.iter().find(|i| &i.frame == b).unwrap();
    match &item.what {
        What::Sidecars { sidecars, held, .. } => {
            assert!(*held);
            assert_eq!(sidecars.len(), 1, "{sidecars:?}");
            assert_eq!(sidecars[0].from.extension().unwrap(), "gcd");
        }
        other => panic!("{other:?}"),
    }
    assert!(plan.theirs_newer().contains(&b.as_path()));
    let report = run(&plan, &quiet_hooks(&cancel, &bytes));
    assert!(report.theirs_newer.contains(b));
    assert_eq!(
        Sidecar::load_as_file(&there).unwrap().unwrap().meta.rating,
        5
    );
    assert_eq!(std::fs::read(&xmps_there[0]).unwrap(), theirs_before);

    // The other way, the same rule: the archive's newer XMP is the
    // one that would go home; the `.gcd`s are the same bytes now.
    let (back, held, _) = compare_sidecars(&there, b);
    assert!(!held);
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].to, greycard_edit_io::xmp::paths_of(b)[0]);
    assert!(back[0].replace);
    greycard_render::testing::remove_dir_retry(&dir);
}

/// Back up and Bring back leave a pair alone when either side is a
/// newer greycard's (§724): held and named, nothing copied or joined.
#[test]
fn a_newer_build_s_sidecar_on_either_side_holds_the_pair() {
    let dir = scratch("newer-pair");
    let mine = dir.join("mine").join("IMG_0001.CR3");
    let theirs = dir.join("theirs").join("IMG_0001.CR3");
    for f in [&mine, &theirs] {
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    }
    let mut ours = Sidecar::default();
    ours.meta.rating = 2;
    ours.save(&mine).unwrap();
    let newer = r#"{"floors": {"read": 10, "write": 11}, "meta": {"rating": 4}}"#;
    std::fs::write(Sidecar::path_for(&theirs), newer).unwrap();
    for (a, b) in [(&mine, &theirs), (&theirs, &mine)] {
        let (sends, held, join) = compare_sidecars(a, b);
        assert!(held);
        assert!(
            sends
                .iter()
                .all(|s| s.from.extension().is_none_or(|x| x != "gcd")),
            "{sends:?}"
        );
        assert!(join.is_none());
    }
    assert_eq!(
        std::fs::read_to_string(Sidecar::path_for(&theirs)).unwrap(),
        newer
    );
    greycard_render::testing::remove_dir_retry(&dir);
}

/// A sidecar left at the destination with no frame beside it keeps
/// the frame out: it would land beside another's edit.
#[test]
fn a_leftover_sidecar_at_the_destination_keeps_the_frame_out() {
    let dir = scratch("leftover");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    let files = shoot(&local);
    let there = nas.join("local").join("shoot");
    std::fs::create_dir_all(&there).unwrap();
    std::fs::write(there.join("b.tif.gcd"), br#"{"saved":9}"#).unwrap();
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
    let item = plan.items.iter().find(|i| i.frame == files[1]).unwrap();
    assert_eq!(
        item.what,
        What::Taken {
            to: there.join("b.tif.gcd")
        }
    );
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    let report = run(&plan, &quiet_hooks(&cancel, &bytes));
    assert!(report.taken.contains(&files[1]));
    assert!(!there.join("b.tif").exists());
    assert_eq!(
        std::fs::read(there.join("b.tif.gcd")).unwrap(),
        br#"{"saved":9}"#
    );
    // The others went whole.
    assert!(there.join("a.tif").is_file() && there.join("c.tif").is_file());
    greycard_render::testing::remove_dir_retry(&dir);
}

/// The copy's rows wait on a library another writer holds without
/// going quiet: the wait beats, and the copy is not set aside.
#[test]
fn writing_the_rows_beats_while_the_library_is_locked() {
    let dir = scratch("locked");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    std::fs::create_dir_all(&nas).unwrap();
    shoot(&local);
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    let report = run(&plan, &quiet_hooks(&cancel, &bytes));
    assert_eq!(report.copied.len(), 3);
    // Another writer holds the lock for well past the wait. The
    // wait is half a second: the lock's beats come every 10 ms, so
    // a tighter window only measures how long a loaded runner can
    // starve the thread (a 150 ms one failed once on Windows CI).
    let holder = rusqlite::Connection::open(&db).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let (tx, rx) = mpsc::channel();
    let job_db = db.clone();
    supervise(
        "test locked",
        Duration::from_millis(500),
        move |beat| record(&job_db, &report, beat),
        move |h| {
            let _ = tx.send(match h {
                Heard::Aside | Heard::Lost => "aside",
                Heard::Done { late: false, .. } => "done",
                Heard::Done { late: true, .. } => "late",
            });
        },
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    holder.execute_batch("COMMIT;").unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), "done");
    drop(holder);
    let lib = Library::open_read_only(&db).unwrap();
    assert!(
        lib.whole_hash(&nas.join("local").join("shoot").join("a.tif"))
            .unwrap()
            .is_some()
    );
    drop(lib);
    greycard_render::testing::remove_dir_retry(&dir);
}

/// §197's open case, settled the hash-first way: a file at the
/// destination with the frame's head and size and another time is
/// read whole on both sides. Other bytes: not the frame, and its name
/// is taken, so the frame is left and named. The same bytes: the
/// same frame. Either way both whole hashes are kept, so the next
/// look reads nothing whole again.
#[test]
fn a_file_changed_under_the_same_head_and_size_is_settled_by_its_whole_hash() {
    let dir = scratch("same-head");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    let shoot_dir = local.join("shoot");
    std::fs::create_dir_all(&shoot_dir).unwrap();
    let there = nas.join("local").join("shoot");
    std::fs::create_dir_all(&there).unwrap();
    // Past the 64 KB head, so a byte after it changes no key.
    let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let mine = shoot_dir.join("export.tif");
    std::fs::write(&mine, &body).unwrap();
    let mut other = body.clone();
    other[150_000] ^= 0xff;
    let copy = there.join("export.tif");
    std::fs::write(&copy, &other).unwrap();
    set_time(&copy, Duration::from_secs(60 * 60));
    // The same bytes, another time: a copy that did not keep it.
    let same = shoot_dir.join("same.tif");
    std::fs::write(&same, &body[..190_000]).unwrap();
    let same_there = there.join("same.tif");
    std::fs::write(&same_there, &body[..190_000]).unwrap();
    set_time(&same_there, Duration::from_secs(60 * 60));
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let ask = Ask {
        frames: Frames::Chosen(vec![mine.clone(), same.clone()]),
        ..backup_ask(&local, &nas, &db, false)
    };
    let first = plan(&ask, &no_beat());
    assert_eq!(first.items[0].what, What::Taken { to: copy.clone() });
    assert_eq!(first.taken(), [mine.as_path()]);
    assert!(
        matches!(first.items[1].what, What::Same { .. }),
        "{:?}",
        first.items[1].what
    );
    assert_eq!(first.same(), (1, 0), "counted once");
    // The hashes kept on the rows.
    let lib = Library::open_read_only(&db).unwrap();
    for (p, b) in [
        (&mine, &body[..]),
        (&copy, &other[..]),
        (&same, &body[..190_000]),
    ] {
        let whole = blake3::hash(b).to_hex().to_string();
        assert_eq!(lib.whole_hash(p).unwrap(), Some(whole), "{}", p.display());
    }
    drop(lib);
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    run(&first, &quiet_hooks(&cancel, &bytes));
    assert_eq!(std::fs::read(&copy).unwrap(), other, "left alone");
    // Asked again: decided from the rows, the same way.
    let second = plan(&ask, &no_beat());
    assert_eq!(second.items, first.items);
    greycard_render::testing::remove_dir_retry(&dir);
}

/// A copy to the archive takes the frame's `.gcd` and XMPs, never what
/// saves put aside beside it (other writers' files kept, files that
/// would not parse): those stay with the frame where it is.
#[test]
fn a_copy_leaves_what_saves_put_aside() {
    let dir = scratch("asides");
    let files = shoot(&dir);
    let a = &files[0];
    Sidecar::default().save(a).unwrap();
    let gcd = Sidecar::path_for(a);
    for end in [".other", ".2.other", ".bad"] {
        std::fs::write(format!("{}{end}", gcd.display()), b"aside").unwrap();
    }
    assert_eq!(greycard_edit::asides_of(&gcd).len(), 3);
    assert_eq!(travelling(a), [gcd]);
    greycard_render::testing::remove_dir_retry(&dir);
}

/// A frame with no sidecar of its own is still kept out by a sidecar
/// left at its destination name, of either placement or an XMP, and
/// with sidecars off too.
#[test]
fn a_frame_with_no_sidecar_is_kept_out_by_one_left_at_the_destination() {
    let dir = scratch("stray");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    let files = shoot(&local);
    let a = &files[0];
    assert!(travelling(a).is_empty(), "a has no sidecar here");
    let there = nas.join("local").join("shoot");
    std::fs::create_dir_all(there.join(SIDECAR_FOLDER)).unwrap();
    let db = dir.join("library.sqlite");
    index(&db, &[&local]);
    for (stray, sidecars) in [
        (there.join("a.tif.gcd"), true),
        (there.join(SIDECAR_FOLDER).join("a.tif.gcd"), true),
        (there.join("a.xmp"), true),
        (there.join("a.tif.gcd"), false),
    ] {
        std::fs::write(&stray, b"another frame's").unwrap();
        let ask = Ask {
            sidecars,
            ..backup_ask(&local, &nas, &db, false)
        };
        let plan = plan(&ask, &no_beat());
        let item = plan.items.iter().find(|i| &i.frame == a).unwrap();
        assert_eq!(
            item.what,
            What::Taken { to: stray.clone() },
            "{}",
            stray.display()
        );
        std::fs::remove_file(&stray).unwrap();
    }
    // One appearing between the plan and the copy keeps it out too.
    let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
    std::fs::write(there.join("a.tif.gcd"), b"late").unwrap();
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    let report = run(&plan, &quiet_hooks(&cancel, &bytes));
    assert!(report.taken.contains(a));
    assert!(!there.join("a.tif").exists());
    greycard_render::testing::remove_dir_retry(&dir);
}

/// A job whose thread ends without a result is said, so nothing
/// waits on it.
#[test]
fn a_job_that_panics_is_heard_as_lost() {
    let (tx, rx) = mpsc::channel();
    supervise(
        "test lost",
        Duration::from_secs(10),
        |beat: &Beat| -> u32 {
            beat();
            std::panic::resume_unwind(Box::new("the test's own"))
        },
        move |h| {
            let _ = tx.send(matches!(h, Heard::Lost));
        },
    )
    .unwrap();
    assert!(rx.recv_timeout(Duration::from_secs(10)).unwrap());
}

/// A walk told to stop ends at once.
#[test]
fn a_walk_told_to_stop_ends() {
    let dir = scratch("walk-stop");
    write_frame(&dir.join("a.tif"), &R5, 1);
    let (frames, _) = walk(&dir, false, &[], &|| {}, &|| true);
    assert!(frames.is_empty());
    greycard_render::testing::remove_dir_retry(&dir);
}

/// A folder of no root's with the archive inside it: the walk never
/// goes into the archive.
#[test]
fn a_walk_leaves_the_archive_out() {
    let dir = scratch("walk-skip");
    let top = dir.join("top");
    let nas = top.join("nas");
    std::fs::create_dir_all(&nas).unwrap();
    write_frame(&top.join("a.tif"), &R5, 1);
    write_frame(&nas.join("z.tif"), &R6, 2);
    let (frames, _) = walk(&top, false, std::slice::from_ref(&nas), &|| {}, &|| false);
    assert_eq!(frames, [top.join("a.tif")]);
    greycard_render::testing::remove_dir_retry(&dir);
}

/// Under `--no-sidecars` the frames go without them, and nothing of
/// a sidecar is compared.
#[test]
fn with_sidecars_off_the_frames_go_alone() {
    let dir = scratch("no-sidecars");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    std::fs::create_dir_all(&nas).unwrap();
    shoot(&local);
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let ask = Ask {
        sidecars: false,
        ..backup_ask(&local, &nas, &db, false)
    };
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    let report = run(&plan(&ask, &no_beat()), &quiet_hooks(&cancel, &bytes));
    assert_eq!((report.copied.len(), report.sidecar_files), (3, 0));
    let there = nas.join("local").join("shoot");
    assert!(!there.join("b.tif.gcd").exists());
    assert!(!there.join(SIDECAR_FOLDER).exists());
    greycard_render::testing::remove_dir_retry(&dir);
}

/// A whole hash on a row is the file's only while the row's size and
/// time are: a frame written again in place under the same start and
/// size after its backup, before any pass, is read again rather than
/// taken for its old self, and backed up when it differs.
#[test]
fn a_whole_hash_on_a_row_is_trusted_only_while_the_file_is_unchanged() {
    let dir = scratch("stale-whole");
    let (local, nas) = (dir.join("local"), dir.join("nas"));
    let shoot_dir = local.join("shoot");
    std::fs::create_dir_all(&shoot_dir).unwrap();
    std::fs::create_dir_all(&nas).unwrap();
    let body: Vec<u8> = (0..200_000u32).map(|i| (i % 249) as u8).collect();
    let mine = shoot_dir.join("export.tif");
    std::fs::write(&mine, &body).unwrap();
    set_time(&mine, Duration::from_secs(2 * 60 * 60));
    let db = dir.join("library.sqlite");
    index(&db, &[&local, &nas]);
    let ask = Ask {
        frames: Frames::Chosen(vec![mine.clone()]),
        ..backup_ask(&local, &nas, &db, false)
    };
    let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
    let first = plan(&ask, &no_beat());
    let report = run(&first, &quiet_hooks(&cancel, &bytes));
    record(&db, &report, &no_beat());
    index(&db, &[&nas]);
    // Written again in place: the same start and size, other bytes,
    // a new time; no pass since, so the row still has the old hash.
    let mut again = body.clone();
    again[150_000] ^= 0xff;
    std::fs::write(&mine, &again).unwrap();
    let second = plan(&ask, &no_beat());
    let copy = nas.join("local").join("shoot").join("export.tif");
    assert_eq!(
        second.items[0].what,
        What::Taken { to: copy.clone() },
        "not the frame backed up before"
    );
    greycard_render::testing::remove_dir_retry(&dir);
}
