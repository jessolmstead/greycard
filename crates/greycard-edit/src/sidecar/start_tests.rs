//! A line nobody developed holds no state (§724): its reader makes its
//! start, a seed changes the start before any id exists, and the first
//! thing that needs a state records the start whole as the root.

use super::*;
use crate::{Exported, Line};

/// Every recorded state's id checks against its parent's and its
/// content as written: nothing was changed under an id.
fn ids_hold(line: &Line) {
    let mut parent = sync::ZERO_ID.to_string();
    for step in &line.history {
        let id = step.id.clone().expect("an id");
        assert_eq!(
            id,
            sync::state_id(&parent, &step.written(), step.label.as_deref()),
            "a state's id is its content's"
        );
        parent = id;
    }
    assert_eq!(
        line.current_id.as_deref(),
        Some(
            sync::state_id(
                &parent,
                &line.current_written(),
                line.current_label.as_deref()
            )
            .as_str()
        ),
        "the current state's id is its content's"
    );
}

/// The keys of the file's top level.
fn keys(s: &Sidecar) -> Vec<String> {
    let file: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
    file.as_object().unwrap().keys().cloned().collect()
}

/// A line nobody developed writes no state, and a seed changes its start
/// without one: the first step records the seeded start as its root,
/// under an id made from it, and nothing was ever rewritten.
#[test]
fn a_seed_changes_the_start_and_the_first_step_records_it_as_the_root() {
    let mut s = Sidecar::default();
    s.meta.rating = 2;
    assert!(!s.line.recorded());
    let written = keys(&s);
    for key in ["current", "history", "snapshots", "id"] {
        assert!(!written.contains(&key.to_string()), "{key}: {written:?}");
    }
    // The ISO's blend, as the open, the export, and a preset seed it.
    assert!(s.line.seed(|e| e.noise.learned_strength = 0.4));
    assert!(
        !s.line.seed(|e| e.noise.learned_strength = 0.4),
        "no change"
    );
    assert!(!s.line.recorded());
    assert_eq!(s.current_id, None);
    assert!(!keys(&s).contains(&"current".to_string()));
    // Read back, it is still no state; the reader makes the start again.
    let mut back = Sidecar::from_json(&s.to_json(), &crate::Edit::default()).unwrap();
    assert!(!back.line.recorded());
    assert_eq!(back.meta.rating, 2);
    back.set_start(&Edit::for_picture());
    assert_eq!(*back.current(), Edit::for_picture());
    // A step: the seeded start is the root, the step on top of it.
    let seeded = s.current().clone();
    let mut brighter = seeded.clone();
    brighter.light.exposure = 0.5;
    assert!(s.record(brighter.clone()));
    assert!(s.line.recorded());
    assert_eq!(s.history.len(), 1);
    assert_eq!(s.history[0].edit, seeded);
    assert_eq!(*s.current(), brighter);
    ids_hold(&s.line);
    // A recorded line takes no seed.
    assert!(!s.line.seed(|e| e.noise.learned_strength = 0.9));
    let back = Sidecar::from_json(&s.to_json(), &crate::Edit::default()).unwrap();
    assert_eq!(back.line, s.line);
}

/// An export of a frame nobody developed, its start seeded as the export
/// rendered it, records that start as the root the export is noted on.
#[test]
fn an_export_from_the_start_records_the_seeded_start_as_the_root() {
    let mut s = Sidecar::default();
    s.line.seed(|e| {
        e.noise.learned_strength = 0.4;
        e.lens.enabled = false;
    });
    let edit = s.current().clone();
    let exported = Exported {
        file: "/out/a.jpg".into(),
        preset: None,
        at: 7,
    };
    assert!(s.record_export(&edit, exported.clone()));
    assert!(s.line.recorded());
    assert!(s.history.is_empty());
    assert_eq!(*s.current(), edit);
    assert_eq!(s.current_exports, [exported]);
    ids_hold(&s.line);
    assert!(keys(&s).contains(&"current".to_string()));
}

/// A snapshot or a step on a frame nobody developed records the start
/// first, as the state it is made in.
#[test]
fn a_snapshot_or_a_step_records_the_start_first() {
    let mut s = Sidecar::default();
    s.take_snapshot("kept", 1);
    assert!(s.line.recorded());
    ids_hold(&s.line);
    let mut s = Sidecar::default();
    s.line.seed(|e| e.noise.learned_strength = 0.4);
    let start = s.current().clone();
    let root = sync::state_id(sync::ZERO_ID, &start, None);
    {
        let mut e = s.current().clone();
        e.light.exposure = 1.0;
        s.record(e);
    }
    assert!(s.line.recorded());
    assert_eq!(s.history[0].id.as_deref(), Some(root.as_str()));
    assert_eq!(s.current_parent.as_deref(), Some(root.as_str()));
    ids_hold(&s.line);
}

/// A file a build before version 10 wrote with its one state, `edit`,
/// and nothing else.
fn version_nine(edit: &Edit) -> String {
    let line = Line::of(edit.clone(), 0).unwrap();
    let s = Sidecar {
        line,
        ..Sidecar::default()
    };
    let mut file: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
    file["current"]["version"] = 9.into();
    file.to_string()
}

/// A line a build before version 10 wrote untouched, at the start its
/// reader makes (its blend seeded or not), holds no state from here: its
/// blend is kept, so the picture is the one it was, and nothing of it is
/// written back. A state the start would not give (a raw's default on a
/// picture), one with a step, and one this build wrote all stay.
#[test]
fn a_version_nine_line_untouched_at_its_start_holds_no_state_from_here() {
    let mut seeded = Edit::for_picture();
    seeded.noise.learned_strength = 0.4;
    let picture = Edit::for_picture();
    let s = Sidecar::from_json(&version_nine(&seeded), &picture).unwrap();
    assert!(!s.line.recorded());
    assert_eq!(*s.current(), seeded, "the blend kept");
    assert_eq!(s.current_id, None);
    assert!(!keys(&s).contains(&"current".to_string()));

    // A raw's default on a picture is a picture the start does not give.
    let s = Sidecar::from_json(&version_nine(&Edit::default()), &picture).unwrap();
    assert!(s.line.recorded());
    assert_eq!(*s.current(), Edit::default());

    // With steps, it was developed, though it is back at its start.
    let mut stepped = Sidecar {
        line: Line::of(Edit::for_picture(), 0).unwrap(),
        ..Sidecar::default()
    };
    let mut e = Edit::for_picture();
    e.light.exposure = 0.3;
    assert!(stepped.record(e));
    assert!(stepped.record(Edit::for_picture()));
    let mut file: serde_json::Value = serde_json::from_str(&stepped.to_json()).unwrap();
    file["current"]["version"] = 9.into();
    let s = Sidecar::from_json(&file.to_string(), &picture).unwrap();
    assert!(s.line.recorded());

    // One this build wrote reads as it was written.
    let s = Sidecar {
        line: Line::of(Edit::for_picture(), 0).unwrap(),
        ..Sidecar::default()
    };
    let back = Sidecar::from_json(&s.to_json(), &picture).unwrap();
    assert!(back.line.recorded());
    assert_eq!(back.line, s.line);
}

/// A version 9 line with words of its own says what it is, at its start
/// or not: an import that came out at the start, and a virtual copy made
/// from it, stay recorded, words and all; the line beside them with none
/// holds no state from here (§724).
#[test]
fn a_version_nine_line_with_words_stays_recorded() {
    let start = Edit::default();
    let mut s = Sidecar {
        line: Line::of(start.clone(), 0).unwrap(),
        ..Sidecar::default()
    };
    s.line.current_label = Some(crate::history::import_label("Lightroom"));
    let copy = s.new_version(None, Some("Copy")).unwrap();
    let copy_label = s.version(&copy).unwrap().line.current_label.clone();
    assert!(copy_label.is_some());
    s.line.current_label = None;
    let mut file: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
    file["current"]["version"] = 9.into();
    file["versions"][0]["line"]["current"]["version"] = 9.into();
    let read = Sidecar::from_json(&file.to_string(), &start).unwrap();
    assert!(!read.line.recorded(), "no words: no state");
    let version = &read.version(&copy).unwrap().line;
    assert!(version.recorded());
    assert_eq!(version.current_label, copy_label);
    // And an import's own line with its words.
    let mut lr = Sidecar {
        line: Line::of(start.clone(), 0).unwrap(),
        ..Sidecar::default()
    };
    lr.line.current_label = Some(crate::history::import_label("Lightroom"));
    let mut file: serde_json::Value = serde_json::from_str(&lr.to_json()).unwrap();
    file["current"]["version"] = 9.into();
    let read = Sidecar::from_json(&file.to_string(), &start).unwrap();
    assert!(read.line.recorded());
    assert_eq!(read.current_label, lr.current_label);
}

/// A version 9 raw nobody opened, its blend never seeded, written with
/// the lens section the other way from this reader's setting, reads as a
/// frame with no sidecar: at this reader's start (§724). One that was
/// opened, its blend seeded, keeps the picture it showed.
#[test]
fn a_version_nine_raw_never_opened_takes_either_lens_as_the_start() {
    let unseeded = Edit::raw_without_lens();
    let s = Sidecar::from_json(&version_nine(&unseeded), &Edit::default()).unwrap();
    assert!(!s.line.recorded());
    assert_eq!(*s.current(), Edit::default(), "this reader's start");
    let mut seeded = Edit::raw_without_lens();
    seeded.noise.learned_strength = 0.4;
    let s = Sidecar::from_json(&version_nine(&seeded), &Edit::default()).unwrap();
    assert!(s.line.recorded());
    assert_eq!(*s.current(), seeded);
}

/// A rating-only sidecar another writer saved meanwhile is the same
/// frame's: a copy of it read for a save or a join, under whatever start
/// that reader gave it, shows the window's start, never the other's
/// (§724). Before, a join handed a picture a raw's default and a raw the
/// lens the setting had turned off.
#[test]
fn another_writer_s_copy_of_a_line_nobody_developed_keeps_the_reader_s_start() {
    let dir = crate::testing::scratch("start-join");
    // A picture, read as its own; and a raw, read with lens corrections
    // off for new edits.
    for (name, start) in [
        ("IMG_0001.jpg", Edit::for_picture()),
        ("IMG_0002.CR3", Edit::raw_without_lens()),
    ] {
        let frame = dir.join(name);
        std::fs::write(&frame, b"frame").unwrap();
        let mut rated = Sidecar::default();
        rated.meta.rating = 3;
        rated.save(&frame).unwrap();
        let local = Sidecar::load(&frame, &start).unwrap().unwrap();
        assert_eq!(*local.current(), start, "{name}");
        // Another writer, with no settings, rates it again.
        let mut other = Sidecar::load_as_file(&frame).unwrap().unwrap();
        other.meta.rating = 4;
        other.save(&frame).unwrap();
        let path = Sidecar::path_for(&frame);
        // As the other writer's own reader read it, it is at that start.
        let theirs = Sidecar::read_as_file(&path).unwrap();
        let joined = sync::join(&local, &theirs);
        assert_eq!(joined.meta.rating, 4, "{name}");
        assert_eq!(*joined.current(), start, "{name}: the reader's start");
        // The checked save's read of the copy on disk takes ours.
        let Ok(checked::Read::Sidecar(disk)) = checked::read_changed(&path, &local.start) else {
            panic!("{name}: read");
        };
        assert_eq!(*disk.current(), start, "{name}");
    }
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A line nobody developed, seeded with its ISO's blend as an open seeds
/// it, keeps the seed through a join either way, a join with itself, and
/// the checked save's read of a copy another writer saved meanwhile; and
/// so does a version 9 line opened before, whose seeded blend the
/// migration kept (§724). Before, each of them put the start's default
/// blend back.
#[test]
fn a_seeded_start_survives_a_join_and_a_checked_read() {
    let dir = crate::testing::scratch("seed-join");
    let blend = |s: &Sidecar| s.current().noise.learned_strength;
    // A rating-only raw, seeded; and a version 9 raw opened before.
    let mut opened = Edit::default();
    opened.noise.learned_strength = 0.37;
    for (name, v9) in [("IMG_0001.CR3", false), ("IMG_0002.CR3", true)] {
        let raw = dir.join(name);
        std::fs::write(&raw, b"raw").unwrap();
        let path = Sidecar::path_for(&raw);
        if v9 {
            std::fs::write(&path, version_nine(&opened)).unwrap();
        } else {
            let mut rated = Sidecar::default();
            rated.meta.rating = 3;
            rated.save(&raw).unwrap();
        }
        let mut local = Sidecar::load_as_file(&raw).unwrap().unwrap();
        assert!(!local.line.recorded(), "{name}");
        if !v9 {
            local.line.seed(|e| e.noise.learned_strength = 0.37);
        }
        assert_eq!(blend(&local), 0.37, "{name}");
        // Another writer rates the file.
        let mut other = Sidecar::load_as_file(&raw).unwrap().unwrap();
        other.meta.rating = 4;
        other.save(&raw).unwrap();
        let theirs = Sidecar::read_as_file(&path).unwrap();
        for (how, joined) in [
            ("local first", sync::join(&local, &theirs)),
            ("theirs first", sync::join(&theirs, &local)),
            ("itself", sync::join(&local, &local.clone())),
        ] {
            assert_eq!(blend(&joined), 0.37, "{name}: {how}");
            assert!(!joined.line.recorded(), "{name}: {how}");
        }
        assert_eq!(sync::join(&local, &theirs).meta.rating, 4, "{name}");
        // The checked save weighs the other writer's file: whatever it
        // makes of it, the line is as this copy holds it, seed and all.
        let (checked, _) = local.reconciled(&raw, Placement::Beside, &sync::Stamp::now());
        let (_, taken) = checked.unwrap();
        let taken = taken.expect("the file on disk weighed");
        assert_eq!(blend(&taken), 0.37, "{name}: the save's read");
    }
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A copy kept beside a picture's sidecar (`X.JPG.gcd.other`) is read at
/// a picture's start, by the frame's name up to its `.gcd` (§724).
#[test]
fn a_kept_copy_of_a_picture_s_sidecar_reads_as_a_picture_s() {
    let dir = crate::testing::scratch("kept-copy-start");
    let kept = dir.join("X.JPG.gcd.other");
    let mut rated = Sidecar::default();
    rated.meta.rating = 2;
    std::fs::write(&kept, rated.to_json()).unwrap();
    let read = Sidecar::read_as_file(&kept).unwrap();
    assert_eq!(*read.current(), Edit::for_picture());
    greycard_core::testing::remove_dir_retry(&dir);
}

/// The checked save takes another machine's file whole when this copy
/// has nothing new: a seed writes nothing, so a seeded line nobody
/// developed is nothing new. The file it takes holds that line as this
/// copy does, seed and all (§724).
#[test]
fn a_save_that_takes_another_machine_s_file_keeps_the_seed() {
    let dir = crate::testing::scratch("seed-took");
    let raw = dir.join("IMG_0001.CR3");
    std::fs::write(&raw, b"raw").unwrap();
    let mut rated = Sidecar::default();
    rated.meta.rating = 3;
    rated.save(&raw).unwrap();
    let mut local = Sidecar::load_as_file(&raw).unwrap().unwrap();
    local.line.seed(|e| e.noise.learned_strength = 0.37);
    // Another machine makes a virtual copy from the defaults.
    let mut other = Sidecar::load_as_file(&raw).unwrap().unwrap();
    other
        .new_version_from_original(Edit::default(), None)
        .unwrap();
    other.save(&raw).unwrap();
    let (checked, _) = local.reconciled(&raw, Placement::Beside, &sync::Stamp::now());
    let (how, taken) = checked.unwrap();
    assert!(matches!(how, checked::Checked::Took), "{how:?}");
    let taken = taken.expect("the file taken");
    assert_eq!(taken.versions.len(), 1);
    assert_eq!(taken.current().noise.learned_strength, 0.37);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A version made from a line nobody developed that took its seed takes
/// the seed with it, and a join keeps it (§724).
#[test]
fn a_version_made_from_a_seeded_start_is_seeded() {
    let mut s = Sidecar::default();
    s.line.seed(|e| e.noise.learned_strength = 0.37);
    let id = s.new_version(None, Some("Copy")).unwrap();
    let version = &s.version(&id).unwrap().line;
    assert!(!version.recorded());
    assert!(version.seeded);
    // Against a copy whose version took no seed, a join keeps this one.
    let mut unseeded = s.clone();
    let line = &mut unseeded.versions[0].line;
    line.current.noise.learned_strength = Edit::default().noise.learned_strength;
    line.seeded = false;
    for joined in [sync::join(&unseeded, &s), sync::join(&s, &unseeded)] {
        let line = &joined.version(&id).unwrap().line;
        assert_eq!(line.current().noise.learned_strength, 0.37);
    }
}

/// A file with a history but no current state (found by the
/// `sidecar_json` fuzz target; no build writes one) reads as the line
/// nobody developed it writes as: no history held that the write drops,
/// so the shape that history asked for is not written once and lost on
/// the next read.
#[test]
fn a_history_with_no_current_state_is_not_held() {
    let json = r#"{"history":[{"retouch":{"patches":[{}]},"version":10}],
        "snapshots":[{"name":"S","taken":1,"edit":{"version":10}}],
        "versions":[{"id":"9e3779b97f4a7c15","line":{"history":[{"version":10}]}}],
        "turn":1,"shape":{"turn":0,"aspect":1}}"#;
    let s = Sidecar::from_json(json, &Edit::default()).unwrap();
    assert!(!s.line.recorded());
    assert!(s.line.history.is_empty());
    assert!(s.line.snapshots.is_empty());
    assert!(s.versions[0].line.history.is_empty());
    let written = s.to_json();
    let back = Sidecar::from_json(&written, &Edit::default()).unwrap();
    assert_eq!(back.to_json(), written, "the round trip is stable");
    assert_eq!(back, s);
}
