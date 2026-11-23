//! A file says what may read it and what may write it (§724): a newer
//! build's file whose write floor is past this schema opens read-only
//! and is never written, by a save, a checked save, or a join; one whose
//! read floor is past it is refused; one with no floors is judged by its
//! states, as before them.

use super::*;
use crate::sync::{Stamp, join};
use crate::testing::{exposed, scratch};
use crate::{Error, VERSION};

fn read(json: &str) -> Result<Sidecar> {
    Sidecar::from_json(json, &Edit::default())
}

/// A file a build at schema 11 wrote: floors read 10, write `write`;
/// states at 11 with a field this build does not know and enum values
/// it does not know (a grain, a shape, a repair method, a lens choice,
/// a brush stroke's op, a component's mode, an aspect); meta; a turn;
/// a key at the top this build does not know.
fn schema_11(write: u32) -> String {
    let rest = serde_json::json!({
        "current": {
            "version": 11,
            "light": {"exposure": 0.5, "sparkle": 3.0},
            "grain": {"kind": "cinema", "amount": 20.0},
            "geometry": {"aspect": {"kind": "golden"}},
            "lens": {"chosen": "a_new_way"},
            "adjustments": [{
                "id": 1,
                "name": "One",
                "mask": {"components": [
                    {"mode": "xor", "shape": {"kind": "hologram", "depth": 2}},
                    {"shape": {"kind": "brush", "strokes": [{"points": [[0.1, 0.1]], "op": "smudge"}]}}
                ]}
            }],
            "retouch": {"patches": [{"id": 1, "method": "generate", "points": [[0.5, 0.5]]}]}
        },
        "id": "b",
        "parent": "a",
        "history": [{"version": 11, "light": {"exposure": 0.25}, "id": "a"}],
        "meta": {"rating": 3},
        "turn": 1,
        "constellations": ["a later build's own"],
    })
    .to_string();
    // Floors first, as every build that writes them writes them.
    format!(
        r#"{{"floors": {{"read": 10, "write": {write}}}, {}"#,
        &rest[1..]
    )
}

/// A file a build at schema 11 wrote, past this build's write floor,
/// opens with what this build knows of it, and read-only.
#[test]
fn a_newer_build_s_file_opens_read_only() {
    let s = read(&schema_11(11)).unwrap();
    assert!(s.read_only());
    assert_eq!(s.newer, Some(11));
    assert_eq!(s.current().light.exposure, 0.5);
    assert_eq!(s.history[0].edit().light.exposure, 0.25);
    assert_eq!(s.meta.rating, 3);
    assert_eq!(
        s.current().grain.kind,
        Default::default(),
        "an unknown value reads as the default"
    );
    assert_eq!(s.current().retouch.patches[0].method, Default::default());
    assert!(matches!(s.writable(), Err(Error::ReadOnly(11))));
}

/// A file whose write floor is this build's own is read and written as
/// any other: a later build that adds nothing this one would lose
/// leaves the floor where it was.
#[test]
fn a_file_at_our_write_floor_is_written() {
    let mut json: serde_json::Value = serde_json::from_str(&schema_11(VERSION)).unwrap();
    json["current"]["version"] = VERSION.into();
    json["history"][0]["version"] = VERSION.into();
    let s = read(&json.to_string());
    // Its unknown values are not read loosely: only a newer file's are.
    assert!(
        s.is_err(),
        "an unknown value in a file of this schema is refused, as ever"
    );
    let mut plain = Sidecar::default();
    plain.record(exposed(1.0));
    let json = plain.to_json();
    let back = read(&json).unwrap();
    assert!(!back.read_only());
    assert!(json.contains("\"floors\""));
}

/// Every file says its floors, this build's own, first.
#[test]
fn every_file_says_its_floors() {
    let s = Sidecar::default();
    let v: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
    assert_eq!(
        v["floors"],
        serde_json::json!({"read": VERSION, "write": VERSION})
    );
    assert!(
        s.to_json()
            .trim_start_matches(['{', ' ', '\n'])
            .starts_with("\"floors\"")
    );
}

/// A file whose read floor is past this build's schema is refused: it
/// holds what this build would read wrongly.
#[test]
fn a_read_floor_past_ours_is_refused() {
    let mut json: serde_json::Value = serde_json::from_str(&schema_11(11)).unwrap();
    json["floors"]["read"] = 11.into();
    assert!(matches!(read(&json.to_string()), Err(Error::Newer(11))));
}

/// A file with no floors is judged by its states, as before floors: one
/// past this schema is refused.
#[test]
fn a_file_with_no_floors_is_judged_by_its_states() {
    let mut json: serde_json::Value = serde_json::from_str(&schema_11(11)).unwrap();
    json.as_object_mut().unwrap().remove("floors");
    assert!(matches!(read(&json.to_string()), Err(Error::Newer(11))));
}

/// Nothing writes a read-only file: not a save, a checked save, a write
/// elsewhere, nor a step, a meta change, or a turn on it; the file on
/// disk is as the newer build left it.
#[test]
fn nothing_writes_a_newer_build_s_file() {
    let dir = scratch("floors-write");
    let raw = dir.join("IMG_0001.CR3");
    std::fs::write(&raw, b"raw").unwrap();
    let path = Sidecar::path_for(&raw);
    let text = schema_11(11);
    std::fs::write(&path, &text).unwrap();
    let mut s = Sidecar::load(&raw, &Edit::default()).unwrap().unwrap();
    assert!(!s.record(exposed(2.0)), "a step is refused");
    s.meta.rating = 5;
    s.turn_by(1, Some(1.5));
    assert!(matches!(
        s.save_in(&raw, Placement::Beside),
        Err(Error::ReadOnly(11))
    ));
    let stamp = Stamp {
        at: 1,
        host: "here".into(),
    };
    assert!(matches!(
        s.save_checked(&raw, Placement::Beside, &stamp),
        Err(Error::ReadOnly(11))
    ));
    let elsewhere = dir.join("copy.gcd");
    assert!(matches!(s.write_to(&elsewhere), Err(Error::ReadOnly(11))));
    assert!(!elsewhere.exists());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        text,
        "left as it was"
    );
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A copy this build wrote, saved over a newer build's file on disk:
/// the checked save finds the newer file and writes nothing over it. A
/// join with a newer copy is read-only too, and is not written.
#[test]
fn a_newer_file_on_disk_or_in_a_join_is_not_written_over() {
    let dir = scratch("floors-over");
    let raw = dir.join("IMG_0001.CR3");
    std::fs::write(&raw, b"raw").unwrap();
    let mut ours = Sidecar::default();
    ours.record(exposed(1.0));
    ours.save_in(&raw, Placement::Beside).unwrap();
    let mut ours = Sidecar::load(&raw, &Edit::default()).unwrap().unwrap();
    // A newer build saves the frame meanwhile.
    let path = Sidecar::path_for(&raw);
    let text = schema_11(11);
    std::fs::write(&path, &text).unwrap();
    ours.record(exposed(1.5));
    let stamp = Stamp {
        at: 2,
        host: "here".into(),
    };
    assert!(matches!(
        ours.save_checked(&raw, Placement::Beside, &stamp),
        Err(Error::ReadOnly(11))
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    let theirs = read(&text).unwrap();
    for joined in [join(&ours, &theirs), join(&theirs, &ours)] {
        assert!(joined.read_only());
        assert!(joined.clone().save_in(&raw, Placement::Beside).is_err());
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A newer build's file in one place and this build's in the other,
/// both ways round: the newer is the frame's ([`Sidecar::find`]), so it
/// opens read-only and nothing is saved; and neither a save's clean-up
/// ([`Sidecar::drop_other`]) nor a move ([`Sidecar::settle`]) removes
/// it, nor one that will not read.
#[test]
fn a_newer_build_s_copy_in_the_other_place_is_never_removed() {
    for (newer_at, ours_at) in [
        (Placement::Beside, Placement::Folder),
        (Placement::Folder, Placement::Beside),
    ] {
        let dir = scratch("floors-places");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let mut ours = Sidecar::default();
        ours.record(exposed(1.0));
        ours.save_in(&raw, ours_at).unwrap();
        let newer = Sidecar::path_in(&raw, newer_at);
        std::fs::create_dir_all(newer.parent().unwrap()).unwrap();
        let text = schema_11(11);
        std::fs::write(&newer, &text).unwrap();
        assert_eq!(Sidecar::find(&raw), Some(newer.clone()), "{newer_at:?}");
        let s = Sidecar::load(&raw, &Edit::default()).unwrap().unwrap();
        assert!(s.read_only());
        Sidecar::drop_other(&raw, ours_at);
        assert_eq!(std::fs::read_to_string(&newer).unwrap(), text);
        assert_eq!(Sidecar::settle(&raw, ours_at).unwrap(), Settled::Kept);
        assert_eq!(Sidecar::settle(&raw, newer_at).unwrap(), Settled::Kept);
        assert_eq!(std::fs::read_to_string(&newer).unwrap(), text);
        assert!(Sidecar::path_in(&raw, ours_at).exists(), "ours left too");
        // One that will not parse is set aside, its bytes kept, and
        // stands in the way no more.
        std::fs::write(&newer, "{ not json").unwrap();
        Sidecar::drop_other(&raw, ours_at);
        assert!(!newer.exists());
        let bad = newer.with_file_name(format!(
            "{}.bad",
            newer.file_name().unwrap().to_string_lossy()
        ));
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), "{ not json");
        std::fs::write(&newer, "{ not json either").unwrap();
        assert_eq!(Sidecar::settle(&raw, ours_at).unwrap(), Settled::InPlace);
        assert!(!newer.exists() && Sidecar::path_in(&raw, ours_at).exists());
        greycard_core::testing::remove_dir_retry(&dir);
    }
}

/// Whether a copy is a newer build's is read from its head alone, and
/// what a copy is (ours, a newer build's, broken) is read again at each
/// ask: a newer build's file put in place of ours under the same length
/// is never taken for ours. A newer build's file cut off mid-write is a
/// newer build's, not broken.
#[test]
fn a_copy_s_verdict_is_read_fresh_and_floors_from_the_head() {
    let dir = scratch("floors-verdict");
    let raw = dir.join("IMG_0001.CR3");
    let path = Sidecar::path_for(&raw);
    std::fs::write(&path, schema_11(11)).unwrap();
    assert_eq!(Sidecar::copy_is(&path), crate::CopyIs::Kept);
    assert!(super::by_newer_build(&path));
    let mut ours = Sidecar::default();
    ours.record(exposed(1.0));
    let text = ours.to_json();
    std::fs::write(&path, &text).unwrap();
    assert!(!super::by_newer_build(&path));
    assert_eq!(Sidecar::copy_is(&path), crate::CopyIs::Ours);
    // Floors not at the head: a file from before floors, no newer's.
    std::fs::write(
        &path,
        r#"{"saved": 1, "floors": {"read": 10, "write": 11}}"#,
    )
    .unwrap();
    assert!(!super::by_newer_build(&path));
    std::fs::write(&path, "[1, 2").unwrap();
    assert_eq!(Sidecar::copy_is(&path), crate::CopyIs::Broken);
    // Ours, then a newer build's of the same length in its place.
    std::fs::write(&path, &text).unwrap();
    assert_eq!(Sidecar::copy_is(&path), crate::CopyIs::Ours);
    let newer = schema_11(11);
    let newer = format!("{newer:<width$}", width = text.len().max(newer.len()));
    std::fs::write(&path, &newer).unwrap();
    assert_eq!(Sidecar::copy_is(&path), crate::CopyIs::Kept);
    // A newer build's cut off mid-write.
    std::fs::write(&path, &schema_11(11)[..80]).unwrap();
    assert_eq!(Sidecar::copy_is(&path), crate::CopyIs::Kept);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A newer build saves the frame while this build holds it: the save
/// refuses, this copy's edit is kept beside the frame under the name
/// another writer's would be (`.other`), and the copy is read-only from
/// then, so nothing more is taken on it.
#[test]
fn a_save_over_a_newer_file_keeps_ours_beside_it_and_goes_read_only() {
    let dir = scratch("floors-aside");
    let raw = dir.join("IMG_0001.CR3");
    std::fs::write(&raw, b"raw").unwrap();
    let mut ours = Sidecar::default();
    ours.record(exposed(1.0));
    ours.save_in(&raw, Placement::Beside).unwrap();
    let mut ours = Sidecar::load(&raw, &Edit::default()).unwrap().unwrap();
    let path = Sidecar::path_for(&raw);
    let text = schema_11(11);
    std::fs::write(&path, &text).unwrap();
    assert!(ours.record(exposed(1.75)));
    let stamp = Stamp {
        at: 3,
        host: "here".into(),
    };
    let e = ours
        .save_checked_said(&raw, Placement::Beside, &stamp)
        .unwrap_err();
    assert!(matches!(e.error, Error::ReadOnly(11)));
    assert_eq!(e.asides.kept.len(), 1, "ours kept aside");
    let kept = Sidecar::read_as_file(&e.asides.kept[0]).unwrap();
    assert_eq!(kept.current().light.exposure, 1.75);
    assert!(e.asides.kept[0].to_string_lossy().ends_with(".other"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    assert!(ours.read_only());
    assert!(!ours.record(exposed(2.0)));
    greycard_core::testing::remove_dir_retry(&dir);
}
