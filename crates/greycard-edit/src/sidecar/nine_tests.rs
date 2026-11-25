//! A build before floors (0.12, schema 9) reads a file with no state
//! and writes it back at its schema. Where that would lose something of
//! ours, the file is one it refuses; where it would lose nothing (a
//! rating, a flag, a turn), it stays one that build may write.

use super::*;
use crate::testing::{exposed, scratch};

/// What a build at schema 9 refuses, as 0.12.0 has it
/// (`Sidecar::from_json`, `migrate_line`, and `migrate`): a state past
/// its schema as a line's current, in its history, or in a snapshot,
/// on the file's own line or on a version's with an id.
fn refused_at_nine(json: &str) -> bool {
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    let past = |e: &serde_json::Value| e.get("version").and_then(|v| v.as_u64()) > Some(9);
    let line = |l: &serde_json::Value| {
        l.get("current").is_some_and(past)
            || l.get("history")
                .and_then(|h| h.as_array())
                .is_some_and(|h| h.iter().any(past))
            || l.get("snapshots")
                .and_then(|s| s.as_array())
                .is_some_and(|s| s.iter().filter_map(|s| s.get("edit")).any(past))
    };
    line(&value)
        || value
            .get("versions")
            .and_then(|v| v.as_array())
            .is_some_and(|vs| {
                vs.iter()
                    .filter(|v| {
                        v.get("id")
                            .and_then(|i| i.as_str())
                            .is_some_and(|i| !i.trim().is_empty())
                    })
                    .filter_map(|v| v.get("line"))
                    .any(line)
            })
}

fn rated() -> Sidecar {
    let mut s = Sidecar::default();
    s.meta.rating = 3;
    s.meta.flag = meta::Flag::Pick;
    s.turn = 1;
    s
}

/// Each field of ours a build at schema 9 would drop, on a file with no
/// state.
fn ours_alone() -> Vec<(&'static str, Sidecar)> {
    let with = |f: fn(&mut Sidecar)| {
        let mut s = rated();
        f(&mut s);
        s
    };
    vec![
        ("an owed turn", with(|s| s.turn_owed = Some(1))),
        (
            "an owed turn's shape",
            with(|s| {
                s.turn_owed = Some(1);
                s.shape = Measured::new(0, 1.5);
            }),
        ),
        (
            "a version's tombstone",
            with(|s| {
                s.removed.insert("v1".into(), 100);
            }),
        ),
        (
            "a snapshot's tombstone",
            with(|s| {
                s.line.snapshots_removed.insert("s1".into(), 100);
            }),
        ),
        ("a swap's time", with(|s| s.original_at = 100)),
        (
            "a line's source",
            with(|s| {
                s.line.sources.insert("p".into(), [0.25, 0.5]);
            }),
        ),
    ]
}

/// A file with no state and nothing of ours but its floors (a rating, a
/// flag, a turn) stays one a build at schema 9 reads and writes: it
/// loses nothing there, and two machines on either build share it. So
/// does one with copies set aside, which that build drops at no cost.
#[test]
fn a_rating_and_a_turn_alone_stay_writable_at_nine() {
    let mut s = rated();
    for _ in 0..2 {
        let json = s.to_json();
        assert!(!refused_at_nine(&json), "{json}");
        assert!(!json.contains("history"), "{json}");
        s.set_aside = vec!["0123456789abcdef0123456789abcdef".into()];
    }
}

/// A file with no state that holds something a build at schema 9 would
/// drop is one that build refuses, and reads here as it was written:
/// still no state, nothing more.
#[test]
fn a_file_with_no_state_and_ours_is_refused_at_nine() {
    for (what, s) in ours_alone() {
        let json = s.to_json();
        assert!(refused_at_nine(&json), "{what}: {json}");
        // The stop is at the first schema with floors, written out
        // here so a bump of ours cannot move it: a later build whose
        // floors stay at 10 writes a stop this build reads.
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            value["history"],
            serde_json::json!([{"version": 10}]),
            "{what}"
        );
        let back = Sidecar::from_json(&json, &Edit::default()).unwrap();
        assert!(!back.lines().any(Line::recorded), "{what}");
        assert!(back.history.is_empty(), "{what}");
        assert_eq!(back.meta.rating, 3, "{what}");
        assert_eq!(back.to_json(), json, "{what}: written again the same");
    }
}

/// Saved in either place, by a save or by a write, the file on disk is
/// the one a build at schema 9 refuses.
#[test]
fn both_places_write_a_file_refused_at_nine() {
    for placement in [Placement::Beside, Placement::Folder] {
        let dir = scratch("nine-places");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let mut s = rated();
        s.turn_owed = Some(3);
        s.save_in(&raw, placement).unwrap();
        let path = Sidecar::path_in(&raw, placement);
        assert!(
            refused_at_nine(&std::fs::read_to_string(&path).unwrap()),
            "{placement:?}"
        );
        let back = Sidecar::load(&raw, &Edit::default()).unwrap().unwrap();
        assert_eq!(back.turn_owed, Some(3), "{placement:?}");
        assert!(!back.lines().any(Line::recorded), "{placement:?}");
        let copy = dir.join("copy.gcd");
        back.write_to(&copy).unwrap();
        assert!(
            refused_at_nine(&std::fs::read_to_string(&copy).unwrap()),
            "{placement:?}"
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }
}

/// A file with a state needs no stop: every state is written at this
/// build's schema, one written at 9 too once it is read here.
#[test]
fn a_file_with_a_state_is_refused_at_nine_by_its_states() {
    let mut s = rated();
    s.record(exposed(1.0));
    let json = s.to_json();
    assert!(refused_at_nine(&json));
    let nine = r#"{"current": {"version": 9, "light": {"exposure": 1.0}},
        "meta": {"rating": 2}, "saved": 4}"#;
    let mut s = Sidecar::from_json(nine, &Edit::default()).unwrap();
    s.meta.rating = 4;
    let json = s.to_json();
    assert!(refused_at_nine(&json), "{json}");
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["history"], serde_json::json!([]), "no stop: {json}");
}

/// A version a build at schema 9 would lose part of: a key on the
/// version it does not know, or one on the version's line. What it
/// knows of either loses nothing.
#[test]
fn a_version_s_keys_are_judged_as_the_file_s() {
    let file = |version: serde_json::Value| serde_json::json!({"floors": {"read": 10, "write": 10}, "versions": [version]});
    let known = serde_json::json!({"id": "v", "name": "Warm", "made": 1,
        "line": {"step": "Exposure", "exported": []}});
    assert!(!lost_at_nine(&file(known.clone())));
    let mut named = known.clone();
    named["named"] = 5.into();
    assert!(lost_at_nine(&file(named)), "a version's rename time");
    let mut line = known;
    line["line"]["snapshots_removed"] = serde_json::json!({"s": 5});
    assert!(lost_at_nine(&file(line)), "a key on a version's line");
}

/// A later build writes the stop at 10 too: a file this build may read
/// and write by its floors, holding that stop, reads here, no state.
#[test]
fn a_stop_at_ten_reads_whatever_wrote_it() {
    assert_eq!(FLOORS_SINCE, 10);
    let json = r#"{"floors": {"read": 10, "write": 10}, "history": [{"version": 10}],
        "meta": {"rating": 2}, "turn_owed": 1}"#;
    let s = Sidecar::from_json(json, &Edit::default()).unwrap();
    assert!(!s.read_only());
    assert!(!s.lines().any(Line::recorded));
    assert_eq!(s.meta.rating, 2);
}
