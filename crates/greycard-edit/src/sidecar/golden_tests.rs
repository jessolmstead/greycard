//! States written at schema 10's cut are written back byte for byte by
//! every later build (§724): a field added to a state is skipped at its
//! default, and none is renamed, so an earlier build's states keep
//! their bytes and their ids. The golden file holds one of nearly
//! everything a state can say, written by this build.
//!
//! It is written once at the cut (0.13.0) and never again (the
//! writers refuse once `testing::SCHEMA_10_CUT` is set): a change
//! that makes these tests fail is a change to how an earlier build's
//! states are written, and is the bug. Before the cut, a step of §724
//! that changes a state's shape writes it again by running the ignored
//! test with `GREYCARD_WRITE_GOLDEN=1`.

use super::*;
use crate::retouch::{Method, Patch};
use crate::sync::{Stamp, state_id};
use crate::testing::exposed;

/// The golden file, as committed.
const GOLDEN: &str = include_str!("../../fixtures/states-at-10.gcd");

/// The sidecar the golden file was written from: a root at the start,
/// a step with every placed thing (`placed_tests::everything`), a white
/// balance as shot with an offset, one with words, an export record, a snapshot, a repair spot with a chosen
/// source, states placed under two turns, a step undone, a version with
/// states and one nobody developed, meta, and a revision.
fn golden() -> Sidecar {
    // Under a fixed clock and install id: no wall-clock time and no
    // real id lands in the fixture.
    crate::testing::fixed(1_800_000_150, "0123456789abcdef", golden_made)
}

fn golden_made() -> Sidecar {
    let mut s = Sidecar::default();
    s.record(exposed(0.4));
    s.record(super::placed_tests::everything());
    let mut offset = exposed(0.5);
    offset.white_balance = crate::WhiteBalance::Offset {
        mired: 10.0,
        tint: 0.0,
    };
    s.record(offset);
    s.record_as(exposed(-0.2), Some("Preset: Faded film".into()));
    let current = s.current().clone();
    s.record_export(
        &current,
        Exported {
            file: "/out/frame.jpg".into(),
            preset: Some("Web".into()),
            at: 1_800_000_000,
        },
    );
    s.take_snapshot("Before the spot", 1_800_000_100);
    let mut spotted = s.current().clone();
    spotted.retouch.patches.push(Patch {
        id: spotted.retouch.next_id(),
        method: Method::Heal,
        points: vec![[0.4, 0.6]],
        radius: 0.03,
        ..Patch::default()
    });
    s.record(spotted.clone());
    let mut done = spotted;
    done.retouch.patches.last_mut().unwrap().source = Some([0.05, -0.02]);
    s.take_sources(&done.retouch.patches);
    s.frame_known(1.5);
    s.turn_by(1, None).unwrap();
    let mut after = s.current().clone();
    after.light.exposure += 0.1;
    s.record(after);
    let mut undone = s.current().clone();
    undone.light.exposure += 0.1;
    s.record(undone);
    s.undo();
    let id = s.new_version(None, Some("Warm")).unwrap();
    s.line_mut(Some(&id)).unwrap().record(exposed(0.9));
    // Renamed: its `named` time (§724).
    s.rename_version(&id, "Warm, kept");
    s.new_version_from_original(Edit::default(), Some("Plain"));
    // A snapshot renamed, one removed, a version deleted, and a swap with
    // the original there and back: each leaves its time or its
    // tombstone (§724).
    let gone = s.take_snapshot("Gone", 1_800_000_120);
    s.remove_snapshot(gone);
    s.rename_snapshot(0, "Before the spot, kept");
    let spare = s.new_version(None, Some("Spare")).unwrap();
    s.delete_version(&spare);
    s.swap_with_original(&id);
    s.swap_with_original(&id);
    s.meta.rating = 4;
    s.meta.keywords.push("kept".into());
    s.fill_ids();
    s.revise(&Stamp {
        at: 1_800_000_200,
        host: "golden".into(),
    });
    s
}

/// The golden file read and written again is the same bytes.
#[test]
fn states_written_at_schema_10_s_cut_write_back_byte_for_byte() {
    let read = Sidecar::from_json(GOLDEN, &Edit::default()).unwrap();
    assert_eq!(
        read.to_json(),
        GOLDEN,
        "an earlier build's states are written back as they were (see the module's notes)"
    );
}

/// Every state's id is made again from it as written and its parent:
/// the recipe of an id is the golden file's.
#[test]
fn the_golden_file_s_ids_are_their_states_as_written() {
    let read = Sidecar::from_json(GOLDEN, &Edit::default()).unwrap();
    let mut checked = 0;
    for line in read.lines() {
        if !line.recorded() {
            continue;
        }
        let states = line
            .history
            .iter()
            .map(|s| (s.written().into_owned(), s.label(), s.id(), s.parent()))
            .chain(std::iter::once((
                line.current_written().into_owned(),
                line.current_label(),
                line.current_id(),
                line.current_parent(),
            )));
        for (written, label, id, parent) in states {
            let made = state_id(parent.unwrap_or(sync::ZERO_ID), &written, label);
            assert_eq!(id, Some(made.as_str()));
            checked += 1;
        }
    }
    assert!(checked > 5);
}

/// Every chosen source's key is made again from a patch as written and
/// the turn it was placed under: the recipe of a key is the golden
/// file's.
#[test]
fn the_golden_file_s_source_keys_are_their_patches_as_written() {
    let read = Sidecar::from_json(GOLDEN, &Edit::default()).unwrap();
    let mut made = std::collections::BTreeSet::new();
    let mut kept = std::collections::BTreeSet::new();
    for line in read.lines() {
        kept.extend(line.sources.keys().cloned());
        let copies = std::iter::once((&line.current, &line.current_written))
            .chain(line.history.iter().map(|s| (&s.edit, &s.written)))
            .chain(line.snapshots.iter().map(|s| (&s.edit, &s.written)));
        for (shown, written) in copies {
            let under = written.placed_under(shown);
            for p in &written.edit(shown).retouch.patches {
                made.insert(crate::line::source_key(p, under));
            }
        }
    }
    assert!(!kept.is_empty());
    assert!(kept.is_subset(&made), "{kept:?} not in {made:?}");
}

/// Writes the golden file again. Only before schema 10's cut.
#[test]
#[ignore]
fn write_the_golden_file() {
    if !crate::testing::may_write_golden() {
        return;
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/states-at-10.gcd");
    std::fs::write(path, golden().to_json()).unwrap();
}

/// What a read that knew the frame's shape would have made: the shape
/// said, taken from another copy, or an owed turn made by it. Each
/// leaves the copy at its revision, as `join` asks (§724), and a real
/// change after still shows.
#[test]
fn a_copy_told_its_shape_stays_at_its_revision() {
    let check = |what: &str, step: &dyn Fn(&mut Sidecar)| {
        let mut s = Sidecar::from_json(GOLDEN, &Edit::default()).unwrap();
        assert!(s.at_its_revision(), "{what}: as read");
        step(&mut s);
        assert!(s.at_its_revision(), "{what}: after");
        s.meta.rating = 1;
        assert!(!s.at_its_revision(), "{what}: a change after");
    };
    check("frame_known", &|s| s.frame_known(1.5));
    check("take_shape", &|s| {
        let turn = s.turn;
        s.take_shape(super::Measured::new(turn, 1.5));
    });
    // A turn owed by an XMP, made once the frame's shape is known.
    let mut owed: serde_json::Value = serde_json::from_str(GOLDEN).unwrap();
    owed["turn_owed"] = serde_json::json!(2);
    let owed = owed.to_string();
    let mut s = Sidecar::from_json(&owed, &Edit::default()).unwrap();
    assert!(s.at_its_revision(), "owed: as read");
    assert_eq!(s.make_owed_turn(|_| Some(1.5)), Some(1));
    assert!(s.at_its_revision(), "owed: after");
    s.meta.rating = 1;
    assert!(!s.at_its_revision(), "owed: a change after");
}
