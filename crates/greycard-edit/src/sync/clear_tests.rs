//! Clear history (discussion 167): what it keeps, its undo, and the
//! join of a cleared copy with copies that still have the steps.

use std::path::{Path, PathBuf};

use super::{Compared, Side, Stamp, compare, join, state_ids};
use crate::brush::{Op, Stroke};
use crate::mask::{Component, Mode, Shape};
use crate::{Adjustment, Edit, Placement, Sidecar};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "greycard-clear-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn exposed(stops: f32) -> Edit {
    let mut e = Edit::default();
    e.light.exposure = stops;
    e
}

fn save(s: &mut Sidecar, raw: &Path, at: u64, host: &str) {
    s.save_in_stamped(
        raw,
        Placement::Beside,
        &Stamp {
            at,
            host: host.into(),
        },
    )
    .unwrap();
}

fn reread(raw: &Path) -> Sidecar {
    Sidecar::read_as_file(&Sidecar::path_for(raw)).unwrap()
}

/// The exposures of every state, the current last.
fn exposures(s: &Sidecar) -> Vec<f32> {
    s.history
        .iter()
        .map(|s| s.edit().light.exposure)
        .chain([s.current().light.exposure])
        .collect()
}

/// A frame edited at 0.5, 1, 1.5, 2, and 2.5 on the desk, a snapshot
/// at 1.5, saved, and the file copied to the laptop: both read back.
fn two_places(dir: &Path) -> (Sidecar, Sidecar, PathBuf, PathBuf) {
    let desk = dir.join("desk").join("IMG_0001.CR3");
    let laptop = dir.join("laptop").join("IMG_0001.CR3");
    std::fs::create_dir_all(desk.parent().unwrap()).unwrap();
    std::fs::create_dir_all(laptop.parent().unwrap()).unwrap();
    let mut s = Sidecar::default();
    for x in [0.5, 1.0, 1.5] {
        s.record(exposed(x));
    }
    s.take_snapshot("Middle", 10);
    for x in [2.0, 2.5] {
        s.record(exposed(x));
    }
    save(&mut s, &desk, 100, "desk");
    std::fs::copy(Sidecar::path_for(&desk), Sidecar::path_for(&laptop)).unwrap();
    (reread(&desk), reread(&laptop), desk, laptop)
}

/// Original, the current state, and the snapshots stay; the current
/// edit, its id, and its words are the same; what was undone goes with
/// the rest; and every state taken out leaves a tombstone.
#[test]
fn a_clear_keeps_the_original_the_current_state_and_the_snapshots() {
    let mut s = Sidecar::default();
    for x in [0.5, 1.0, 1.5] {
        s.record(exposed(x));
    }
    s.take_snapshot("Kept", 10);
    s.record_as(exposed(2.0), Some("Preset: Night".into()));
    s.record(exposed(3.0));
    s.undo();
    let before = s.clone();
    assert!(s.can_clear_history());
    let gone: Vec<String> = state_ids(&before)[1..before.history.len()]
        .iter()
        .cloned()
        .chain(
            before
                .redo
                .iter()
                .filter_map(|r| r.id().map(str::to_string)),
        )
        .collect();
    assert!(s.clear_history());
    assert_eq!(exposures(&s), [0.0, 2.0]);
    assert_eq!(s.history[0], before.history[0]);
    assert_eq!(s.current(), before.current());
    assert_eq!(s.current_id(), before.current_id());
    assert_eq!(s.current_label(), Some("Preset: Night"));
    assert_eq!(s.snapshots, before.snapshots);
    assert!(s.redo.is_empty());
    let laid: Vec<&String> = s.cleared.keys().collect();
    let mut want: Vec<&String> = gone.iter().collect();
    want.sort();
    assert_eq!(laid, want);
    // Nothing left to clear; the snapshot still restores, and the
    // first state is still gone to.
    assert!(!s.can_clear_history() && !s.clear_history());
    let mut walk = s.clone();
    assert!(walk.restore_snapshot(0));
    assert_eq!(walk.current().light.exposure, 1.5);
    let mut walk = s.clone();
    assert!(walk.go_to(0));
    assert_eq!(walk.current(), &Edit::default());
}

/// A fresh line, and one with only its first state before the current,
/// have nothing to clear.
#[test]
fn a_line_with_nothing_between_its_first_and_current_has_nothing_to_clear() {
    let mut s = Sidecar::default();
    assert!(!s.can_clear_history() && !s.clear_history());
    s.record(exposed(1.0));
    assert!(!s.can_clear_history() && !s.clear_history());
    s.undo();
    // What was undone is something to clear.
    assert!(s.can_clear_history());
}

/// Back at the Original with steps undone (no history at all), or one
/// state past it with more undone (a history of one): the clear takes
/// what was undone alone, and Undo puts it back.
#[test]
fn a_clear_with_only_undone_states_takes_those_alone() {
    for back in [2, 1] {
        let mut s = Sidecar::default();
        for x in [0.5, 1.0] {
            s.record(exposed(x));
        }
        for _ in 0..back {
            s.undo();
        }
        let before = (exposures(&s), s.redo.len());
        assert_eq!(s.history.len(), 2 - back);
        assert!(s.can_clear_history());
        assert!(s.clear_history());
        assert_eq!(exposures(&s), before.0, "the edit unchanged");
        assert!(s.redo.is_empty());
        assert_eq!(s.cleared.len(), back);
        assert!(!s.can_redo() && s.can_undo());
        assert!(s.undo());
        assert_eq!((exposures(&s), s.redo.len()), before);
        assert!(s.cleared.is_empty());
        assert!(s.redo());
        assert!(s.redo.is_empty());
    }
}

/// Undo takes the clear back whole, tombstones and all; Redo clears
/// again; a step after the clear makes Undo an ordinary one.
#[test]
fn undo_takes_a_clear_back_and_redo_does_it_again() {
    let mut s = Sidecar::default();
    for x in [0.5, 1.0, 1.5, 2.0] {
        s.record(exposed(x));
    }
    s.undo();
    let before = s.clone();
    assert!(s.clear_history());
    let cleared = s.clone();
    assert!(s.can_undo());
    assert!(s.undo());
    assert_eq!(s, before);
    assert_eq!(s.redo.len(), 1);
    assert!(s.cleared.is_empty());
    assert!(s.can_redo());
    assert!(s.redo());
    // The tombstones laid again, at the time of the redo.
    let shape = |s: &Sidecar| {
        (
            exposures(s),
            s.redo.len(),
            s.cleared.keys().cloned().collect::<Vec<_>>(),
        )
    };
    assert_eq!(shape(&s), shape(&cleared));
    // Twice over, the same.
    assert!(s.undo() && s.redo());
    assert_eq!(shape(&s), shape(&cleared));
    // An ordinary undo after an undone clear forgets the clear's redo.
    assert!(s.undo());
    assert!(s.undo());
    assert_eq!(s.current().light.exposure, 1.0);
    assert!(s.redo());
    assert_eq!(shape(&s), shape(&before));
    assert!(s.redo());
    assert_eq!(exposures(&s), [0.0, 0.5, 1.0, 1.5, 2.0]);
    assert!(!s.redo());
    assert!(s.undo());
    // A step after a clear: Undo is that step's, and the clear stays.
    assert!(s.clear_history());
    s.record(exposed(4.0));
    assert!(s.undo());
    assert_eq!(exposures(&s), [0.0, 1.5]);
    assert!(s.undo());
    assert_eq!(exposures(&s), [0.0]);
    assert!(!s.undo());
}

/// The clear is held for the session only: a frame read again from its
/// file has nothing to take back, its Undo an ordinary one.
#[test]
fn a_clear_read_back_from_its_file_is_for_good() {
    let dir = scratch("for-good");
    let raw = dir.join("IMG_0001.CR3");
    let mut s = Sidecar::default();
    for x in [0.5, 1.0, 1.5] {
        s.record(exposed(x));
    }
    assert!(s.clear_history());
    save(&mut s, &raw, 100, "desk");
    let mut back = reread(&raw);
    assert_eq!(back.cleared, s.cleared);
    assert_eq!(exposures(&back), [0.0, 1.5]);
    assert!(back.undo());
    assert_eq!(exposures(&back), [0.0]);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// The other copy untouched since: a join either way is the clear.
#[test]
fn a_cleared_copy_joined_with_an_untouched_one_stays_cleared() {
    let dir = scratch("untouched");
    let (mut desk, laptop, desk_raw, _) = two_places(&dir);
    assert!(desk.clear_history());
    save(&mut desk, &desk_raw, 200, "desk");
    let desk = reread(&desk_raw);
    for j in [join(&desk, &laptop), join(&laptop, &desk)] {
        assert_eq!(exposures(&j), [0.0, 2.5]);
        assert_eq!(j.snapshots, desk.snapshots);
    }
    assert_eq!(compare(&laptop, &desk), Compared::Behind(Side::Local));
    greycard_core::testing::remove_dir_retry(&dir);
}

/// The other copy edited after the common point, saved before or after
/// the clear: the cleared steps do not come back, from either side,
/// and the other copy's edits are kept.
#[test]
fn a_cleared_copy_joined_with_one_edited_since_brings_nothing_back_and_loses_nothing() {
    for laptop_at in [150, 300] {
        let dir = scratch("edited");
        let (mut desk, mut laptop, desk_raw, laptop_raw) = two_places(&dir);
        assert!(desk.clear_history());
        save(&mut desk, &desk_raw, 200, "desk");
        let desk = reread(&desk_raw);
        laptop.record(exposed(-1.0));
        laptop.record(exposed(-2.0));
        save(&mut laptop, &laptop_raw, laptop_at, "laptop");
        let laptop = reread(&laptop_raw);
        let gone: Vec<&String> = desk.cleared.keys().collect();
        for j in [join(&desk, &laptop), join(&laptop, &desk)] {
            let held = state_ids(&j);
            assert!(
                gone.iter().all(|id| !held.contains(id)),
                "a cleared state came back at {laptop_at}: {:?}",
                exposures(&j)
            );
            let shown = exposures(&j);
            assert!(!shown.contains(&0.5) && !shown.contains(&1.0) && !shown.contains(&1.5));
            assert!(shown.contains(&-1.0) && shown.contains(&-2.0), "{shown:?}");
            assert_eq!(j.snapshots, desk.snapshots);
            assert_eq!(j.history[0].id(), desk.history[0].id());
            assert_eq!(j.cleared, desk.cleared);
        }
        let ab = join(&desk, &laptop);
        assert_eq!(ab, join(&laptop, &desk));
        // The laptop's own edits stand as they are, in order.
        if laptop_at == 300 {
            assert_eq!(exposures(&ab), [0.0, 2.5, -1.0, -2.0]);
        } else {
            assert_eq!(exposures(&ab), [0.0, 2.5, -1.0, -2.0, 2.5]);
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }
}

/// A third copy, still with the old steps and an edit of its own, met
/// after the desk and the laptop joined: the tombstones travelled with
/// the join, so nothing cleared comes back from it either.
#[test]
fn a_third_copy_brings_back_nothing_the_first_cleared() {
    let dir = scratch("third");
    let (mut desk, mut laptop, desk_raw, laptop_raw) = two_places(&dir);
    let studio_raw = dir.join("studio").join("IMG_0001.CR3");
    std::fs::create_dir_all(studio_raw.parent().unwrap()).unwrap();
    std::fs::copy(
        Sidecar::path_for(&laptop_raw),
        Sidecar::path_for(&studio_raw),
    )
    .unwrap();
    let mut studio = reread(&studio_raw);
    assert!(desk.clear_history());
    save(&mut desk, &desk_raw, 200, "desk");
    laptop.record(exposed(-1.0));
    save(&mut laptop, &laptop_raw, 300, "laptop");
    let mut j = join(&reread(&desk_raw), &reread(&laptop_raw));
    save(&mut j, &laptop_raw, 400, "laptop");
    let j = reread(&laptop_raw);
    studio.record(exposed(7.0));
    save(&mut studio, &studio_raw, 350, "studio");
    let studio = reread(&studio_raw);
    let gone: Vec<&String> = desk.cleared.keys().collect();
    for k in [join(&j, &studio), join(&studio, &j)] {
        let held = state_ids(&k);
        assert!(
            gone.iter().all(|id| !held.contains(id)),
            "{:?}",
            exposures(&k)
        );
        let shown = exposures(&k);
        assert!(shown.contains(&-1.0) && shown.contains(&7.0), "{shown:?}");
        assert_eq!(k.snapshots, desk.snapshots);
    }
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A copy whose current state is one the other cleared keeps it: no
/// tombstone takes a current state, and the join drops the tombstone
/// of what it holds.
#[test]
fn a_tombstone_never_takes_the_other_copys_current_state() {
    let dir = scratch("current");
    let (mut desk, mut laptop, desk_raw, laptop_raw) = two_places(&dir);
    laptop.undo();
    let stood = laptop.current_id().map(str::to_string);
    laptop.meta.rating = 3;
    save(&mut laptop, &laptop_raw, 300, "laptop");
    let laptop = reread(&laptop_raw);
    assert!(desk.clear_history());
    save(&mut desk, &desk_raw, 200, "desk");
    let desk = reread(&desk_raw);
    assert!(desk.cleared.contains_key(stood.as_deref().unwrap()));
    for j in [join(&desk, &laptop), join(&laptop, &desk)] {
        let held = state_ids(&j);
        assert!(
            held.contains(stood.as_ref().unwrap()),
            "{:?}",
            exposures(&j)
        );
        assert!(!j.cleared.contains_key(stood.as_deref().unwrap()));
    }
    greycard_core::testing::remove_dir_retry(&dir);
}

/// Two copies from before revisions are compared by their state ids: a
/// cleared one stands among the other's states but is not behind it,
/// so it is not written over with the steps it took out.
#[test]
fn a_cleared_copy_is_not_behind_one_that_still_has_the_steps() {
    let mut s = Sidecar::default();
    for x in [0.5, 1.0, 1.5] {
        s.record(exposed(x));
    }
    s.fill_ids();
    let mut cleared = s.clone();
    assert!(cleared.clear_history());
    assert_eq!(compare(&cleared, &s), Compared::Diverged);
    assert_eq!(compare(&s, &cleared), Compared::Diverged);
    assert_eq!(exposures(&join(&cleared, &s)), [0.0, 1.5]);
    assert_eq!(exposures(&join(&s, &cleared)), [0.0, 1.5]);
}

/// The sidecar on disk shrinks at the next save: fifty steps on a
/// brush mask, each a stroke of forty points.
#[test]
fn a_cleared_history_of_fifty_brush_steps_saves_small() {
    let dir = scratch("size");
    let raw = dir.join("IMG_0001.CR3");
    let mut s = Sidecar::default();
    let mut e = Edit::default();
    e.adjustments.push(Adjustment {
        id: 1,
        name: "Mask 1".into(),
        mask: crate::mask::Mask {
            components: vec![Component {
                shape: Shape::Brush {
                    strokes: Vec::new(),
                },
                mode: Mode::Add,
                invert: false,
                enabled: true,
            }],
            ..Default::default()
        },
        ..Default::default()
    });
    s.record(e.clone());
    for step in 0..50 {
        let Shape::Brush { strokes } = &mut e.adjustments[0].mask.components[0].shape else {
            unreachable!()
        };
        let mut stroke = Stroke::new(Op::Add, 0.05, 0.5, 1.0);
        stroke.points = (0..40)
            .map(|i| {
                [
                    0.1 + 0.017 * i as f32 + 0.0031 * step as f32,
                    0.2 + 0.011 * step as f32 + 0.0007 * i as f32,
                ]
            })
            .collect();
        strokes.push(stroke);
        e.adjustments[0].look.light.exposure = 0.01 * step as f32;
        s.record(e.clone());
    }
    s.take_snapshot("Kept", 5);
    save(&mut s, &raw, 100, "desk");
    let before = std::fs::metadata(Sidecar::path_for(&raw)).unwrap().len();
    let current = s.current().clone();
    assert!(s.clear_history());
    save(&mut s, &raw, 200, "desk");
    let after = std::fs::metadata(Sidecar::path_for(&raw)).unwrap().len();
    eprintln!("fifty brush steps: {before} bytes, cleared {after} bytes");
    assert!(after * 10 < before, "{before} -> {after}");
    let back = reread(&raw);
    assert_eq!(back.current(), &current);
    assert_eq!(back.snapshots.len(), 1);
    greycard_core::testing::remove_dir_retry(&dir);
}
