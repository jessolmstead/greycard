//! A state's process (§724): read as 1 where a file has none, kept as
//! made through a read, a write, a migration, and a join, and this
//! build's on a state the user makes.

use crate::sync;
use crate::testing::PROCESS;
use crate::{Edit, Line, Sidecar};

fn exposed(stops: f32) -> Edit {
    let mut e = Edit::default();
    e.light.exposure = stops;
    e
}

/// `f` with this thread's build at process `p`, put back after.
fn under<T>(p: u32, f: impl FnOnce() -> T) -> T {
    struct Back;
    impl Drop for Back {
        fn drop(&mut self) {
            PROCESS.set(None);
        }
    }
    let _back = Back;
    PROCESS.set(Some(p));
    f()
}

/// Every state a sidecar holds, each line's history, current, and
/// snapshots, and the undone ones it writes, as (process, exposure).
fn processes(s: &Sidecar) -> Vec<(u32, f32)> {
    let mut out = vec![];
    for l in s.lines() {
        for e in l
            .history
            .iter()
            .map(|h| &h.edit)
            .chain([&l.current])
            .chain(l.snapshots.iter().map(|x| &x.edit))
        {
            out.push((e.process, e.light.exposure));
        }
    }
    out
}

/// A file at 9, and an edit with none, read the process as 1; the
/// sparse form writes it.
#[test]
fn a_state_with_no_process_reads_as_the_first() {
    assert_eq!(Edit::from_json(r#"{"version": 9}"#).unwrap().process, 1);
    let nine = r#"{"version": 9, "current": {"version": 9, "light": {"exposure": 1.0}},
        "history": [{"version": 9}]}"#;
    let s = under(5, || Sidecar::from_json(nine, &Edit::default()).unwrap());
    assert!(
        processes(&s).iter().all(|(p, _)| *p == 1),
        "{:?}",
        processes(&s)
    );
    let written: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
    assert_eq!(written["current"]["process"], 1);
    assert_eq!(written["history"][0]["process"], 1);
}

/// A state keeps its own process through a write and a read by a build
/// at another, earlier or later; one above the build's is kept as
/// written and leaves the file writable.
#[test]
fn a_state_keeps_its_own_process_through_a_round_trip() {
    // Made by a build at 1.
    let s = under(1, || {
        let mut s = Sidecar::default();
        s.record(exposed(1.0));
        s.record(exposed(2.0));
        s.take_snapshot("Kept", 1_800_000_000);
        s.fill_ids();
        s
    });
    let made = processes(&s);
    assert!(made.iter().all(|(p, _)| *p == 1), "{made:?}");
    let back = under(5, || {
        let back = Sidecar::from_json(&s.to_json(), &Edit::default()).unwrap();
        Sidecar::from_json(&back.to_json(), &Edit::default()).unwrap()
    });
    assert_eq!(processes(&back), made);
    assert_eq!(back, s, "and nothing else moved");
    // Made by a later build, at 7: read by this one as written.
    let later = under(7, || {
        let mut l = Sidecar::default();
        l.record(exposed(3.0));
        l.fill_ids();
        l.to_json()
    });
    let read = Sidecar::from_json(&later, &Edit::default()).unwrap();
    assert!(processes(&read).iter().all(|(p, _)| *p == 7));
    assert!(!read.read_only(), "a process is no floor");
    assert_eq!(read.to_json(), later);
}

/// A step, a new version's first state, and a version from the
/// original take the build's process; what came before keeps its own,
/// and a restored snapshot is the state it kept. Only the process
/// differing is no step.
#[test]
fn a_state_the_user_makes_takes_the_build_s_process() {
    // Made by a build at 1.
    let mut s = under(1, || {
        let mut s = Sidecar::default();
        s.record(exposed(1.0));
        s.take_snapshot("At one", 1_800_000_000);
        s
    });
    under(5, || {
        let mut same = s.line.current.clone();
        same.process = 5;
        assert!(!s.record(same), "the process alone is no step");
        let mut next = s.line.current.clone();
        next.light.exposure = 2.0;
        assert!(s.record(next));
        assert_eq!(s.line.current.process, 5);
        assert!(s.line.history.iter().all(|h| h.edit.process == 1));
        let v = s.new_version(None, Some("V")).unwrap();
        assert_eq!(s.version(&v).unwrap().line.current.process, 5);
        let plain = s
            .new_version_from_original(exposed(0.0), Some("P"))
            .unwrap();
        assert_eq!(s.version(&plain).unwrap().line.current.process, 5);
        assert!(s.restore_snapshot(0));
        assert_eq!(s.line.current.process, 1, "the snapshot's own");
    });
    // A line on its own, as the panel's reset and paste record.
    let mut line = Line::default();
    under(3, || {
        line.record(exposed(4.0), 0);
        assert_eq!(line.current.process, 3);
    });
}

/// A join, both ways round, carries every state with its own process,
/// under a build at another.
#[test]
fn a_join_never_restamps_a_state() {
    // Made by a build at 1.
    let (a, b) = under(1, || {
        let mut base = Sidecar::default();
        base.record(exposed(1.0));
        let v = base.new_version(None, Some("V")).unwrap();
        base.fill_ids();
        let text = base.to_json();
        let copy = |stops: f32| {
            let mut c = Sidecar::from_json(&text, &Edit::default()).unwrap();
            c.record(exposed(stops));
            c.line_mut(Some(&v)).unwrap().record(exposed(stops + 0.5));
            c.fill_ids();
            c
        };
        (copy(2.0), copy(3.0))
    });
    under(5, || {
        for j in [sync::join(&a, &b), sync::join(&b, &a)] {
            let all = processes(&j);
            assert!(all.iter().all(|(p, _)| *p == 1), "{all:?}");
            assert!(all.iter().any(|(_, x)| *x == 2.0) && all.iter().any(|(_, x)| *x == 3.0));
        }
    });
}

/// An edit's text without its process is what it was before there was
/// one: the id a state is given at 9 and a picture's key are pinned
/// here to the hashes builds made before the process (the first, the
/// reviewer's, the same at a087baca, before the field, and after it).
/// A field moved ahead of `process` fails here, not silently.
#[test]
fn the_process_changes_no_id_at_9_and_no_picture_key() {
    let mut e = Edit::default();
    e.light.exposure = 2.0;
    e.process = 1;
    let whole = sync::whole_id(sync::ZERO_ID, &e, Some("x"));
    let filled = sync::filled_id(sync::ZERO_ID, &e, Some("x"));
    let key = e.develop_key_with(&[], None);
    assert_eq!(whole, PIN_WHOLE);
    assert_eq!(filled, PIN_FILLED);
    assert_eq!(key, PIN_KEY);
    // Under another process, the same.
    e.process = 7;
    assert_eq!(sync::whole_id(sync::ZERO_ID, &e, Some("x")), PIN_WHOLE);
    assert_eq!(e.develop_key_with(&[], None), PIN_KEY);
}

const PIN_WHOLE: &str = "00c1096bbf0bb5bbaf4d2d46098bf16098a9a4c9d7a88478172a2b8fea9a47c3";
const PIN_FILLED: &str = "1bb549c98a70c1b119f520e1d682ff9fbe11ef6d697da468e646e9e3c4e1b727";
const PIN_KEY: u64 = 0x7ef1_7a29_0e80_3a59;
