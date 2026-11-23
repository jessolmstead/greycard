//! Every state's parent is written (§724): the state it was made from,
//! set where its id is made, and what a join asks to tell a chain from a
//! branch. A state a build before version 10 wrote gets its parent once,
//! as it is read, from the one check of an id against content left.

use super::*;
use crate::Line;
use crate::look::{LookLut, LutChoice};
use crate::sync::{BY_CONTENT, ZERO_ID, join};
use crate::testing::exposed;

/// Each state's id and parent, oldest first, the current last.
fn chain(line: &Line) -> Vec<(Option<String>, Option<String>)> {
    line.history
        .iter()
        .map(|s| (s.id.clone(), s.parent.clone()))
        .chain(std::iter::once((
            line.current_id.clone(),
            line.current_parent.clone(),
        )))
        .collect()
}

/// Whether each state but the first was made from the one before it.
fn made_in_order(line: &Line) -> bool {
    let c = chain(line);
    c[0].1.is_none() && c.windows(2).all(|w| w[1].1.is_some() && w[1].1 == w[0].0)
}

fn read(json: &str) -> Sidecar {
    Sidecar::from_json(json, &Edit::default()).unwrap()
}

/// The root has no parent and writes none; each step's parent is the
/// state it was made from, written beside its id and read back as it
/// was.
#[test]
fn the_root_has_no_parent_and_each_step_names_the_state_it_was_made_from() {
    let mut s = Sidecar::default();
    assert!(s.record(exposed(1.0)));
    assert!(s.record(exposed(2.0)));
    assert!(made_in_order(&s.line));
    let file: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
    assert!(
        file["history"][0].get("parent").is_none(),
        "a root writes none"
    );
    assert_eq!(file["history"][1]["parent"], file["history"][0]["id"]);
    assert_eq!(file["parent"], file["history"][1]["id"]);
    let back = read(&s.to_json());
    assert_eq!(chain(&back.line), chain(&s.line));
}

/// A step after an undo is made from the state the undo went back to,
/// and an undo and its redo carry each state's parent with it.
#[test]
fn a_step_after_an_undo_is_made_from_the_state_undone_to() {
    let mut s = Sidecar::default();
    for e in [1.0, 2.0, 3.0] {
        s.record(exposed(e));
    }
    let before = chain(&s.line);
    assert!(s.undo() && s.redo());
    assert_eq!(chain(&s.line), before, "an undo and a redo move no parent");
    assert!(s.undo() && s.undo());
    let back_to = s.current_id.clone();
    assert!(s.record(exposed(4.0)));
    assert_eq!(s.current_parent, back_to);
    assert!(made_in_order(&s.line));
}

/// A snapshot restored is a new state made from the one it was restored
/// on, not from the state the snapshot was taken at.
#[test]
fn a_snapshot_restored_is_made_from_the_state_it_was_restored_on() {
    let mut s = Sidecar::default();
    s.record(exposed(1.0));
    let taken_at = s.current_id.clone();
    s.take_snapshot("one", 0);
    s.record(exposed(2.0));
    let on = s.current_id.clone();
    assert!(s.restore_snapshot(0));
    assert_eq!(s.current_parent, on);
    assert_ne!(s.current_parent, taken_at);
}

/// A version made from a line starts at a root of its own, and its
/// first step is made from that root.
#[test]
fn a_version_starts_at_a_root_of_its_own() {
    let mut s = Sidecar::default();
    s.record(exposed(1.0));
    let id = s.new_version(None, None).unwrap();
    let line = &s.version(&id).unwrap().line;
    assert!(line.current_id.is_some());
    assert_eq!(line.current_parent, None);
    let root = line.current_id.clone();
    s.line_mut(Some(&id)).unwrap().record(exposed(2.0));
    let line = &s.version(&id).unwrap().line;
    assert_eq!(line.current_parent, root);
}

/// A look renamed gives the renamed states and the ones after them new
/// ids, each made from the one before it as renamed; the states before
/// keep theirs and their parents.
#[test]
fn a_renamed_look_makes_its_run_again_parents_and_all() {
    let named = |name: &str, stops: f32| {
        let mut e = exposed(stops);
        e.look_lut = LookLut {
            lut: LutChoice::Named(name.into()),
            strength: 1.0,
        };
        e
    };
    let mut s = Sidecar::default();
    s.record(exposed(1.0));
    s.record(named("Neon", 2.0));
    s.record(named("Neon", 3.0));
    let before = chain(&s.line);
    assert!(s.rename_look("Neon", "Glow"));
    let after = chain(&s.line);
    assert_eq!(after[..2], before[..2], "the states before keep theirs");
    assert_ne!(after[2].0, before[2].0);
    assert_ne!(after[3].0, before[3].0);
    assert!(made_in_order(&s.line));
}

/// A state a join records again is made from the state before it in the
/// joined list, and the states it carries over keep their own parents.
#[test]
fn a_state_a_join_records_again_is_made_from_the_one_before_it() {
    let mut base = Sidecar::default();
    base.record(exposed(1.0));
    base.fill_ids();
    let (mut a, mut b) = (base.clone(), base.clone());
    a.record(exposed(2.0));
    a.modified = Some(20);
    b.record(exposed(3.0));
    b.modified = Some(10);
    let j = join(&a, &b);
    let known: Vec<_> = chain(&a.line).into_iter().chain(chain(&b.line)).collect();
    let c = chain(&j.line);
    let mut again = 0;
    for (i, state) in c.iter().enumerate() {
        match known.iter().find(|k| k.0 == state.0) {
            Some(k) => assert_eq!(k.1, state.1, "carried over with its parent"),
            None => {
                again += 1;
                assert_eq!(state.1, c[i - 1].0, "recorded again after the one before");
            }
        }
    }
    assert!(again > 0, "the join recorded a state again");
}

/// A file a build before version 10 wrote: each state at version 9, its
/// id made as such a build made it, from `made_from`'s id (its index in
/// `edits`, or None for the root), or none at all.
fn version_nine(edits: &[Edit], made_from: &[Option<usize>], with_ids: bool) -> String {
    let mut ids: Vec<String> = Vec::new();
    let mut states: Vec<serde_json::Value> = Vec::new();
    for (e, from) in edits.iter().zip(made_from) {
        let mut e = e.clone();
        e.version = 9;
        let parent = from.map_or(ZERO_ID.to_string(), |i| ids[i].clone());
        // As a build at 9 made it: over the whole edit.
        let id = crate::sync::whole_id(&parent, &e, None);
        let mut v = serde_json::to_value(&e).unwrap();
        if with_ids {
            v["id"] = id.clone().into();
        }
        ids.push(id);
        states.push(v);
    }
    // The current state's id sits at the top, beside it.
    let mut current = states.pop().unwrap();
    let id = current.as_object_mut().unwrap().remove("id");
    let mut file = serde_json::json!({ "history": states, "current": current });
    if let Some(id) = id {
        file["id"] = id;
    }
    file.to_string()
}

/// The join of `a` and `b` both ways, by parent and by content as it
/// was before §724: one join.
fn joins_alike(a: &Sidecar, b: &Sidecar) {
    for (x, y) in [(a, b), (b, a)] {
        BY_CONTENT.set(true);
        let old = join(x, y);
        BY_CONTENT.set(false);
        let new = join(x, y);
        assert_eq!(old, new, "by parent as by content");
    }
}

/// A version 9 file with a branch an earlier join carried over: the
/// branch's first state hangs off a state not before it, so its read
/// gives it no parent, and the state after it in the branch keeps its
/// own. A capped chain: the state whose parent the cap took has none.
/// Each joins with the copies it came from as it did by content.
#[test]
fn version_nine_branches_and_capped_chains_join_as_they_did() {
    let e = |stops: f32| exposed(stops);
    // r, a1, a2, then b1 and b2 carried over from a copy that went from r.
    let carried = read(&version_nine(
        &[e(1.0), e(2.0), e(3.0), e(4.0), e(5.0)],
        &[None, Some(0), Some(1), Some(0), Some(3)],
        true,
    ));
    let c = chain(&carried.line);
    let parents: Vec<_> = c.iter().map(|s| s.1.clone()).collect();
    assert_eq!(
        parents,
        [None, c[0].0.clone(), c[1].0.clone(), None, c[3].0.clone()]
    );
    let a = read(&version_nine(
        &[e(1.0), e(2.0), e(3.0)],
        &[None, Some(0), Some(1)],
        true,
    ));
    let b = read(&version_nine(
        &[e(1.0), e(4.0), e(5.0)],
        &[None, Some(0), Some(1)],
        true,
    ));
    // r, x1, x2, x3 written; the cap took x1 from the capped copy.
    let whole = read(&version_nine(
        &[e(1.0), e(6.0), e(7.0), e(8.0)],
        &[None, Some(0), Some(1), Some(2)],
        true,
    ));
    let json = version_nine(
        &[e(1.0), e(6.0), e(7.0), e(8.0)],
        &[None, Some(0), Some(1), Some(2)],
        true,
    );
    let mut file: serde_json::Value = serde_json::from_str(&json).unwrap();
    file["history"].as_array_mut().unwrap().remove(1);
    let capped = read(&file.to_string());
    let c = chain(&capped.line);
    assert_eq!(c[1].1, None, "the state whose parent the cap took");
    assert_eq!(c[2].1, c[1].0);
    for (x, y) in [
        (&carried, &a),
        (&carried, &b),
        (&a, &b),
        (&capped, &whole),
        (&capped, &carried),
    ] {
        joins_alike(x, y);
    }
}

/// A copy read at version 9 and its twin written at 10 (the same ids,
/// each state now with `placed_under` and its parent) are one copy: the
/// same states, and a join of the two is either.
#[test]
fn a_version_nine_copy_and_its_twin_written_at_ten_are_one() {
    let e = |stops: f32| exposed(stops);
    let nine = read(&version_nine(
        &[e(1.0), e(2.0), e(3.0), e(4.0)],
        &[None, Some(0), Some(0), Some(2)],
        true,
    ));
    let json = nine.to_json();
    let file: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(file["history"][0]["version"], 10);
    assert!(file["history"][1].get("parent").is_some());
    assert!(
        file["history"][2].get("parent").is_none(),
        "a branch's first"
    );
    let ten = read(&json);
    assert_eq!(chain(&ten.line), chain(&nine.line));
    for (x, y) in [(&nine, &ten), (&ten, &nine)] {
        assert_eq!(chain(&join(x, y).line), chain(&ten.line));
    }
}

/// A file from before ids: nothing is given at its read; the ids made
/// for it are made in order, each with its parent, as two copies of it
/// make them, so the two join as one.
#[test]
fn a_file_from_before_ids_gets_its_parents_with_its_ids() {
    let e = |stops: f32| exposed(stops);
    let json = version_nine(&[e(1.0), e(2.0), e(3.0)], &[None, Some(0), Some(1)], false);
    let mut s = read(&json);
    assert!(
        chain(&s.line)
            .iter()
            .all(|(id, p)| id.is_none() && p.is_none())
    );
    assert!(s.fill_ids());
    assert!(made_in_order(&s.line));
    let mut other = read(&json);
    other.fill_ids();
    assert_eq!(chain(&other.line), chain(&s.line));
    assert_eq!(chain(&join(&s, &other).line), chain(&s.line));
    joins_alike(&s, &read(&json));
}

/// A state the read at version 9 left without a parent, since it does
/// not follow its parent in that copy (a branch carried over), takes
/// the parent the other copy's read found for its id, where it does.
#[test]
fn a_parent_one_copy_knows_is_the_join_s() {
    let e = |stops: f32| exposed(stops);
    // r, a, b, then x made from a, carried over after b.
    let carried = read(&version_nine(
        &[e(1.0), e(2.0), e(3.0), e(4.0)],
        &[None, Some(0), Some(1), Some(1)],
        true,
    ));
    // r, a, x: x after the state it was made from.
    let plain = read(&version_nine(
        &[e(1.0), e(2.0), e(4.0)],
        &[None, Some(0), Some(1)],
        true,
    ));
    let x = carried.current_id.clone();
    assert_eq!(plain.current_id, x, "one state, one id");
    assert_eq!(carried.current_parent, None);
    let a = plain.history[1].id.clone();
    assert_eq!(plain.current_parent, a);
    for (p, q) in [(&carried, &plain), (&plain, &carried)] {
        let j = join(p, q);
        let c = chain(&j.line);
        let found = c.iter().find(|(id, _)| *id == x).expect("x joined");
        assert_eq!(found.1, a, "the parent the other copy knows");
    }
}

/// A renamed look gives new ids to the renamed states and those made
/// from them, down their parents, and to no other: a state an earlier
/// join carried over after a renamed one, made from a state before it,
/// keeps its id and its parent.
#[test]
fn a_renamed_look_makes_again_only_what_was_made_from_it() {
    let named = |name: &str, stops: f32| {
        let mut e = exposed(stops);
        e.look_lut = LookLut {
            lut: LutChoice::Named(name.into()),
            strength: 1.0,
        };
        e
    };
    // r, a, b named Neon, c made from b, then x made from a carried
    // over after c.
    let mut s = read(&version_nine(
        &[
            exposed(1.0),
            exposed(2.0),
            named("Neon", 3.0),
            named("Neon", 4.0),
            exposed(5.0),
        ],
        &[None, Some(0), Some(1), Some(2), Some(1)],
        true,
    ));
    let before = chain(&s.line);
    assert_eq!(before[4].1, None, "x does not follow its parent here");
    assert!(s.rename_look("Neon", "Glow"));
    let after = chain(&s.line);
    assert_eq!(after[..2], before[..2], "r and a keep theirs");
    assert_ne!(after[2].0, before[2].0, "b is made again");
    assert_eq!(after[2].1, before[2].1, "from the same a");
    assert_ne!(after[3].0, before[3].0, "c is made from b again");
    assert_eq!(after[3].1, after[2].0);
    assert_eq!(after[4], before[4], "x, made from a, is no rename's");
}
