//! States placed under a turn and shown under the sidecar's (§724):
//! a turn changes the sidecar's turn and no state.

use std::collections::HashMap;

use super::*;
use crate::brush::Stroke;
use crate::geometry::{Aspect, Crop};
use crate::mask::{Component, Mask, Person, Pick, Route, Shape, Signature, Turned};
use crate::retouch::{self, Method, Patch};
use crate::testing::scratch;
use crate::{Adjustment, Checked, Line, Snapshot, VERSION};

/// An edit with one of everything a turn carries round: a crop, a
/// keystone and a held aspect, every placed mask shape (and two that
/// are not placed), a brush, AI picks and boxes, a Part's person, and a
/// repair patch with a source and a further stroke.
pub(crate) fn everything() -> Edit {
    let mut e = Edit::default();
    e.light.exposure = 0.3;
    e.geometry.crop = Some(Crop::of([0.1, 0.2, 0.5, 0.6]));
    e.geometry.vertical = 4.0;
    e.geometry.horizontal = -2.5;
    e.geometry.aspect = Aspect::Ratio { w: 3.0, h: 2.0 };
    e.geometry.portrait = true;
    let shape = |shape| Component {
        shape,
        ..Component::default()
    };
    e.adjustments.push(Adjustment {
        id: 1,
        name: "All".into(),
        mask: Mask {
            components: vec![
                shape(Shape::Linear {
                    from: [0.1, 0.2],
                    to: [0.4, 0.9],
                }),
                shape(Shape::Radial {
                    center: [0.48, 0.42],
                    radius: [0.12, 0.09],
                    angle: 20.0,
                    feather: 0.5,
                }),
                shape(Shape::Brush {
                    strokes: vec![Stroke {
                        points: vec![[0.2, 0.3], [0.25, 0.31]],
                        radius: 0.03,
                        feather: 0.5,
                        flow: 1.0,
                        op: Default::default(),
                    }],
                }),
                shape(Shape::Sky {
                    picks: vec![Pick {
                        pos: [0.6, 0.1],
                        positive: true,
                    }],
                }),
                shape(Shape::Object {
                    picks: vec![Pick {
                        pos: [0.3, 0.5],
                        positive: false,
                    }],
                    boxes: vec![[[0.1, 0.15], [0.3, 0.4]]],
                }),
                shape(Shape::Part {
                    phrase: "iris of the eye".into(),
                    route: Route::Eye,
                    person: Some(Person {
                        signature: Signature {
                            kind: "test".into(),
                            values: vec![0.1, 0.2],
                        },
                        at: [0.55, 0.35],
                        picture: None,
                    }),
                }),
                shape(Shape::Subject {}),
                shape(Shape::LUMINANCE),
            ],
            ..Mask::default()
        },
        ..Adjustment::default()
    });
    e.retouch.patches.push(Patch {
        id: 1,
        method: Method::Heal,
        points: vec![[0.31, 0.77]],
        radius: 0.02,
        source: Some([0.05, -0.03]),
        more: vec![retouch::Stroke {
            points: vec![[0.7, 0.5], [0.72, 0.52]],
            radius: 0.015,
        }],
        ..Patch::default()
    });
    e
}

/// The picture's width over its height under the turn `turn`, of a
/// frame `aspect0` wide under none: the inverse a quarter turn either
/// way, as [`Measured::aspect_at`] has it.
fn aspect_at(aspect0: f32, turn: u8) -> f32 {
    Measured {
        turn: 0,
        aspect: aspect0,
    }
    .aspect_at(turn)
}

/// What a build before §724 made of `edit` with one turn of the frame
/// `quarters` clockwise, the picture `aspect` wide before it: today's
/// `Edit::turn`, the map `turn_by` took.
fn turned(edit: &Edit, quarters: i32, aspect: f32) -> Edit {
    let mut e = edit.clone();
    e.placed_under = 0;
    e.turn(quarters, Some(Turned::new(quarters, aspect)));
    e
}

fn stamp(at: u64, host: &str) -> sync::Stamp {
    sync::Stamp {
        at,
        host: host.into(),
    }
}

/// Every state of every line: a key that names it for good (its id, or
/// for a snapshot its line and name), the state as written, and the
/// copy shown.
fn states(s: &Sidecar) -> Vec<(String, Edit, Edit)> {
    let mut out = Vec::new();
    for (v, line) in s.lines().enumerate() {
        for step in line.history.iter().chain(line.redo.iter()) {
            out.push((
                step.id.clone().expect("an id"),
                step.written().into_owned(),
                step.edit.clone(),
            ));
        }
        // A line nobody developed holds no state to name.
        if line.recorded() {
            out.push((
                line.current_id.clone().expect("an id"),
                line.current_written().into_owned(),
                line.current.clone(),
            ));
        }
        for snap in &line.snapshots {
            out.push((
                format!("{v}:{}", snap.name),
                snap.written().into_owned(),
                snap.edit.clone(),
            ));
        }
    }
    out
}

/// A state placed under turn 1 and shown under turn 3 is what today's
/// turn by two made of it, for everything a turn carries round, on a
/// landscape frame and a portrait one, and in every state the sidecar
/// holds; and so for every pair of turns.
#[test]
fn a_state_shown_under_another_turn_is_today_s_turn_of_it() {
    for aspect0 in [1.5f32, 2.0 / 3.0] {
        for under in 0..4u8 {
            for turn in 0..4u8 {
                let mut s = Sidecar::default();
                s.frame_known(aspect0);
                s.turn_by(i32::from(under), None);
                // The current, one behind it, one undone, and one kept
                // by name, all placed under `under`.
                let mut brighter = everything();
                brighter.light.exposure = 1.0;
                assert!(s.record(everything()));
                s.take_snapshot("kept", 0);
                assert!(s.record(brighter.clone()));
                assert!(s.undo());
                assert_eq!(s.current_written().placed_under, under);

                s.turn_by(i32::from(turn) - i32::from(under), None);
                assert_eq!(s.turn, turn);
                let q = i32::from(turn) - i32::from(under);
                let a = aspect_at(aspect0, under);
                let want = turned(&everything(), q, a);
                let why = format!("{aspect0} {under} to {turn}");
                assert_eq!(s.current, want, "{why}");
                assert_eq!(s.snapshots[0].edit, want, "{why}");
                assert_eq!(s.redo[0].edit, turned(&brighter, q, a), "{why}");
                // The state as written is as it was recorded.
                let mut written = everything();
                written.placed_under = under;
                assert_eq!(*s.current_written(), written, "{why}");
                assert!(!s.needs_frame());
            }
        }
    }
    // The case named: under 1, shown under 3, is today's turn by two.
    let mut s = Sidecar::default();
    s.frame_known(1.5);
    s.turn_by(1, None);
    s.record(everything());
    s.turn_by(2, None);
    assert_eq!(s.current, turned(&everything(), 2, 1.0 / 1.5));
}

/// Written, turned four times through `turn_by` and `turn_into`, and
/// saved after each: every state is written byte for byte as it was but
/// for the sidecar's turn and the shape the turn was made by, written
/// while a state is placed under another turn; and four quarter turns
/// show every state as written again, to the bit.
#[test]
fn a_turn_changes_no_written_state() {
    let dir = scratch("placed-turns");
    let raw = dir.join("IMG_0001.CR3");
    let mut s = Sidecar::default();
    s.frame_known(1.5);
    s.record(everything());
    let mut e = everything();
    e.light.exposure = 0.7;
    s.record(e);
    s.take_snapshot("kept", 1);
    s.new_version(None, None);
    s.save_in_stamped(&raw, Placement::Beside, &stamp(100, "desk"))
        .unwrap();
    // The states as the file has them: every key but the file's own
    // facts a save moves.
    let lines = |path: &Path| {
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let o = v.as_object_mut().unwrap();
        let turn = o.remove("turn");
        let shape = o.remove("shape");
        for k in ["saved", "revisions", "meta_at"] {
            o.remove(k);
        }
        (v, turn, shape)
    };
    let (first, none, no_shape) = lines(&Sidecar::path_for(&raw));
    assert_eq!((none, no_shape), (None, None));
    let shown: Vec<Edit> = states(&s).into_iter().map(|(_, _, e)| e).collect();
    for k in 1..=4u64 {
        let mut read = Sidecar::read_as_file(&Sidecar::path_for(&raw)).unwrap();
        read.frame_known(aspect_at(1.5, read.turn));
        if k % 2 == 0 {
            read.turn_by(1, None);
        } else {
            let mut one = [read];
            assert_eq!(crate::turn_into(&mut one, &[0], 1, |_| None), [0]);
            [read] = one;
        }
        read.save_in_stamped(&raw, Placement::Beside, &stamp(100 + k, "desk"))
            .unwrap();
        let (now, turn, shape) = lines(&Sidecar::path_for(&raw));
        assert_eq!(now, first, "turn {k}: the states are as written");
        let turn = turn.map_or(0, |t| t.as_u64().unwrap());
        assert_eq!(turn, k % 4, "turn {k}");
        // The shape beside the turn while the states are placed under
        // another, and gone with the turn that brings them home.
        assert_eq!(shape.is_some(), k % 4 != 0, "turn {k}");
        if k == 4 {
            // Four quarters: shown as written, to the bit.
            let back: Vec<Edit> = states(&read).into_iter().map(|(_, _, e)| e).collect();
            assert_eq!(back, shown);
            assert_eq!(read.current.to_json(), s.current.to_json());
        }
    }
    greycard_core::testing::remove_dir_retry(&dir);
}

/// The same four turns in memory, never saved: each made again from the
/// state as written, so the fourth gives back the bits.
#[test]
fn four_quarter_turns_show_every_state_as_written() {
    for aspect0 in [1.5f32, 0.75] {
        let mut s = Sidecar::default();
        s.frame_known(aspect0);
        s.record(everything());
        let before = s.clone();
        for _ in 0..4 {
            s.turn_by(1, None);
        }
        assert_eq!(s.current, before.current);
        assert_eq!(
            serde_json::to_string(&s.current).unwrap(),
            serde_json::to_string(&before.current).unwrap()
        );
        for _ in 0..4 {
            s.turn_by(-1, None);
        }
        assert_eq!(s.current, everything());
    }
}

/// A version 9 sidecar as a build before §724 left it after two turns
/// of the frame: every state turned in place, quarter by quarter with
/// the picture's aspect each time, the file's turn 2. What the build
/// showed of each state is what the file says of it.
fn version_nine_turned(aspect0: f32) -> (String, Vec<Edit>) {
    let mut s = Sidecar::default();
    s.record(everything());
    s.take_snapshot("kept", 1);
    let mut e = everything();
    e.light.exposure = 0.7;
    s.record_as(e, Some("Preset: Warm".into()));
    s.fill_ids();
    let mut file: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
    let turn_in_place = |state: &mut serde_json::Value| {
        let mut edit: Edit = serde_json::from_value(state.clone()).unwrap();
        for k in 0..2u8 {
            edit.turn(1, Some(Turned::new(1, aspect_at(aspect0, k))));
        }
        let mut turned = serde_json::to_value(&edit).unwrap();
        let t = turned.as_object_mut().unwrap();
        t.insert("version".into(), 9.into());
        // The words, records and id beside the edit's fields stay.
        for (k, v) in state.as_object().unwrap() {
            if ["step", "exported", "id"].contains(&k.as_str()) {
                t.insert(k.clone(), v.clone());
            }
        }
        *state = turned;
    };
    let o = file.as_object_mut().unwrap();
    turn_in_place(o.get_mut("current").unwrap());
    for h in o.get_mut("history").unwrap().as_array_mut().unwrap() {
        turn_in_place(h);
    }
    for snap in o.get_mut("snapshots").unwrap().as_array_mut().unwrap() {
        turn_in_place(snap.get_mut("edit").unwrap());
    }
    o.insert("turn".into(), 2.into());
    let json = crate::layout::lay_out(&file.to_string());
    // What the build showed: each state as it read it.
    let shown = |state: &serde_json::Value| Edit::from_json(&state.to_string()).unwrap();
    let mut showed: Vec<Edit> = file["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(shown)
        .collect();
    showed.push(shown(&file["current"]));
    showed.push(shown(&file["snapshots"][0]["edit"]));
    (json, showed)
}

/// A version 9 sidecar that was turned in place reads with every state
/// placed under its turn and shows exactly as it showed, with nothing
/// to measure; reading writes nothing; saved, it is version 10 and
/// reads back the same.
#[test]
fn a_version_nine_sidecar_turned_in_place_shows_as_it_showed() {
    for aspect0 in [1.5f32, 0.75] {
        let (json, showed) = version_nine_turned(aspect0);
        let dir = scratch("placed-nine");
        let raw = dir.join("IMG_0001.CR3");
        let path = Sidecar::path_for(&raw);
        std::fs::write(&path, &json).unwrap();
        let read = Sidecar::read_as_file(&path).unwrap();
        assert_eq!(read.turn, 2);
        assert!(!read.needs_frame(), "nothing to measure");
        let mut shown: Vec<Edit> = read.history.iter().map(|h| h.edit.clone()).collect();
        shown.push(read.current.clone());
        shown.push(read.snapshots[0].edit.clone());
        assert_eq!(shown, showed);
        assert!(read.history.iter().all(|h| h.written().placed_under == 2));
        assert_eq!(read.current_written().placed_under, 2);
        assert_eq!(read.snapshots[0].written().placed_under, 2);
        // Nothing is written at the upgrade.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), json);
        // Saved, it is version 10, each state placed under 2, and reads
        // back the same.
        let mut saved = read.clone();
        saved
            .save_in_stamped(&raw, Placement::Beside, &stamp(200, "desk"))
            .unwrap();
        let file: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["current"]["version"], VERSION);
        assert_eq!(VERSION, 10);
        assert_eq!(file["current"]["placed_under"], 2);
        assert_eq!(file["history"][1]["placed_under"], 2);
        assert_eq!(file["snapshots"][0]["edit"]["placed_under"], 2);
        let back = Sidecar::read_as_file(&path).unwrap();
        assert_eq!(back, saved);
        assert_eq!(back.current, read.current);
        // And a turn back to none shows every state as the build that
        // turned it would have turned it back.
        let mut unturned = back.clone();
        unturned.frame_known(aspect_at(aspect0, 2));
        unturned.turn_by(-2, None);
        assert_eq!(
            unturned.current,
            turned(&read.current, 2, aspect_at(aspect0, 2))
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }
}

/// Every state of a sidecar written now is at version 10: a build
/// before it refuses the file whole, by `migrate`'s newer schema, rather
/// than show a state placed under another turn a turn off. An unturned
/// frame's file says nothing of `placed_under`.
#[test]
fn a_file_written_now_is_version_ten_and_an_unturned_one_has_no_placed_under() {
    let mut s = Sidecar::default();
    s.record(everything());
    s.take_snapshot("kept", 1);
    let json = s.to_json();
    assert!(!json.contains("placed_under"), "{json}");
    let file: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(file["current"]["version"], 10);
    assert_eq!(file["history"][0]["version"], 10);
    assert_eq!(file["snapshots"][0]["edit"]["version"], 10);
    // An edit at a version past this build's is refused, as a build at
    // 9 refuses one at 10.
    let mut newer = file.clone();
    newer["current"]["version"] = (VERSION + 1).into();
    assert!(matches!(
        Sidecar::from_json(&newer.to_string(), &crate::Edit::default()),
        Err(crate::Error::Newer(v)) if v == VERSION + 1
    ));
}

/// A new state's id is made, as ever, from its parent's and its content
/// as written: the same as before §724 on an unturned frame, and with
/// the turn it was placed under on a turned one.
#[test]
fn a_new_state_s_id_is_made_from_its_content_as_written() {
    let mut s = Sidecar::default();
    s.record(everything());
    let parent = s.history[0].id.clone().unwrap();
    assert_eq!(
        s.current_id.as_deref(),
        Some(sync::state_id(&parent, &everything(), None).as_str())
    );
    s.frame_known(1.5);
    s.turn_by(1, None);
    let mut e = s.current.clone();
    e.light.exposure = 2.0;
    s.record(e.clone());
    let parent = s.history[1].id.clone().unwrap();
    let mut written = e;
    written.placed_under = 1;
    assert_eq!(*s.current_written(), written);
    assert_eq!(
        s.current_id.as_deref(),
        Some(sync::state_id(&parent, &written, None).as_str())
    );
    // The earlier state kept its id through the turn.
    assert_eq!(s.history[1].id.as_deref(), Some(parent.as_str()));
}

/// Two copies of one sidecar, one turned and the other edited: their
/// join, under the turned copy's turn, shows every state right, the
/// edited copy's own state among them, which a join that took states
/// as they came showed a quarter turn off.
#[test]
fn a_join_shows_every_state_under_the_joined_turn() {
    let dir = scratch("placed-join");
    let raw = dir.join("IMG_0001.CR3");
    let path = Sidecar::path_for(&raw);
    let mut s = Sidecar::default();
    s.record(everything());
    s.save_in_stamped(&raw, Placement::Beside, &stamp(100, "desk"))
        .unwrap();
    let mut turned_copy = Sidecar::read_as_file(&path).unwrap();
    turned_copy.frame_known(1.5);
    turned_copy.turn_by(1, None);
    turned_copy.advance(&stamp(300, "laptop"));
    let mut edited = Sidecar::read_as_file(&path).unwrap();
    let mut e = edited.current.clone();
    e.light.exposure = 1.0;
    assert!(edited.record(e.clone()));
    edited.advance(&stamp(200, "desk"));
    greycard_core::testing::remove_dir_retry(&dir);

    for j in [
        sync::join(&turned_copy, &edited),
        sync::join(&edited, &turned_copy),
    ] {
        assert_eq!(j.turn, 1);
        // The edited copy's own state, placed under no turn, is there
        // and shown turned with the frame.
        let want = turned(&e, 1, 1.5);
        assert!(
            j.history.iter().any(|h| h.edit == want) || j.current == want,
            "the edited copy's state, shown under the joined turn"
        );
        assert!(
            !j.history.iter().any(|h| h.edit == e) && j.current != e,
            "and not as it was shown under its own"
        );
        // Every state: its shown copy is its written one turned from
        // where it was placed.
        for (_, written, shown) in states(&j) {
            let q = 1 - i32::from(written.placed_under);
            assert_eq!(
                shown,
                turned(&written, q, aspect_at(1.5, written.placed_under))
            );
        }
    }
}

/// A file this build turned carries the shape the turn was made by, so a
/// reader with no frame to measure, and a reader that never measured
/// (window B, below, which only rated the frame), shows every state as the
/// writer that turned it did; and a step recorded on what it shows keeps
/// the masks where they are.
#[test]
fn a_turned_file_shows_right_to_a_reader_that_never_measured() {
    let dir = scratch("placed-reader");
    let raw = dir.join("IMG_0001.CR3");
    let path = Sidecar::path_for(&raw);
    let mut s = Sidecar::default();
    s.record(everything());
    s.save_in_stamped(&raw, Placement::Beside, &stamp(100, "desk"))
        .unwrap();
    // Window A measures and turns; window B read the file before and
    // only changes a rating.
    let mut a = Sidecar::read_as_file(&path).unwrap();
    let mut b = Sidecar::read_as_file(&path).unwrap();
    assert!(a.turn_by(1, Some(1.5)).is_some());
    a.save_checked(&raw, Placement::Beside, &stamp(200, "desk"))
        .unwrap();
    b.meta.rating = 3;
    let checked = b
        .save_checked(&raw, Placement::Beside, &stamp(300, "laptop"))
        .unwrap();
    assert_eq!(checked, Checked::Joined);
    assert_eq!((b.turn, b.meta.rating), (1, 3));
    assert!(!b.needs_frame(), "the file says the shape");
    let want = turned(&everything(), 1, 1.5);
    assert_eq!(b.current, want);
    // A step on what B shows keeps the masks where they are, for good.
    let mut e = b.current.clone();
    e.light.exposure = 2.0;
    assert!(b.record(e.clone()));
    b.save_checked(&raw, Placement::Beside, &stamp(400, "laptop"))
        .unwrap();
    let back = Sidecar::from_json(
        &std::fs::read_to_string(&path).unwrap(),
        &crate::Edit::default(),
    )
    .unwrap();
    assert_eq!(back.current, e);
    let mut was = turned(&everything(), 1, 1.5);
    was.light.exposure = 2.0;
    assert_eq!(back.current, was);
    // Turned back by a reader that never measured either: the file's
    // shape turns it, and every state shows as it was placed.
    let mut c = Sidecar::from_json(
        &std::fs::read_to_string(&path).unwrap(),
        &crate::Edit::default(),
    )
    .unwrap();
    assert!(c.turn_by(-1, None).is_some());
    assert_eq!(c.history[1].edit, everything());
    greycard_core::testing::remove_dir_retry(&dir);
}

/// The same file with no frame to say its shape: the states wait, and the
/// current one is read-only. Nothing made from it may take a mask a quarter
/// turn off: no step is recorded, it hands out no edit to develop or copy,
/// takes no repair source, and a version made from it or a snapshot
/// restored keeps the turn it was placed under. Saved meanwhile, every
/// state is written as it was; told the shape, every state shows as the
/// turn that knew it did, and the frame takes steps again.
#[test]
fn a_state_no_file_can_measure_waits_and_nothing_is_made_from_it() {
    let dir = scratch("placed-waits");
    let raw = dir.join("IMG_0001.CR3");
    let path = Sidecar::path_for(&raw);
    let mut s = Sidecar::default();
    s.record(everything());
    s.take_snapshot("kept", 1);
    assert!(s.turn_by(1, Some(1.5)).is_some());
    let mut file: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
    file.as_object_mut().unwrap().remove("shape");
    std::fs::write(&path, file.to_string()).unwrap();

    let mut read = Sidecar::read_as_file(&path).unwrap();
    assert!(read.needs_frame());
    assert!(read.current_waits());
    assert_eq!(read.ready(), None);
    assert_eq!(read.current_written(), s.current_written());
    // No step on it.
    let mut e = read.current.clone();
    e.light.exposure = 2.0;
    assert!(!read.record(e.clone()));
    assert!(!read.line_mut(None).unwrap().record(e));
    // No source taken for a patch shown where it was placed.
    let mut done = read.current.retouch.patches[0].clone();
    done.source = Some([0.2, 0.2]);
    assert!(!read.take_sources(&[done]));
    // A version made from it, and a snapshot restored, keep where it was
    // placed.
    let id = read.new_version(None, None).unwrap();
    let version = &read.version(&id).unwrap().line;
    assert_eq!(version.current_written().placed_under, 0);
    assert!(version.current_waits());
    assert!(
        !read.restore_snapshot(0),
        "the snapshot is the current state"
    );

    // Saved meanwhile: as written, the new version's state with them.
    let before = std::fs::read_to_string(&path).unwrap();
    read.save_in_stamped(&raw, Placement::Beside, &stamp(200, "desk"))
        .unwrap();
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let was: serde_json::Value = serde_json::from_str(&before).unwrap();
    assert_eq!(after["current"], was["current"]);
    assert_eq!(after["history"], was["history"]);
    assert_eq!(after["snapshots"], was["snapshots"]);
    assert_eq!(after["shape"], serde_json::Value::Null, "none known");

    // Told the shape: every state shows as the turn that knew it did.
    read.frame_known(1.0 / 1.5);
    assert!(!read.needs_frame());
    assert_eq!(read.current, s.current);
    assert_eq!(read.snapshots[0].edit, s.snapshots[0].edit);
    assert_eq!(read.version(&id).unwrap().line.current, s.current);
    assert_eq!(read.ready(), Some(&s.current));
    let mut e = read.current.clone();
    e.light.exposure = 2.0;
    assert!(read.record(e));
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A line put on a turned sidecar records its next state under the
/// sidecar's turn, whatever turn the line was made for: the turn comes
/// from the sidecar as the state is recorded. Read back, the state shows
/// as it was recorded, on version 0 and on a version's line alike.
#[test]
fn a_line_put_on_a_turned_sidecar_records_under_the_sidecar_s_turn() {
    let crop = Crop::of([0.1, 0.2, 0.3, 0.4]);
    for made_for in [0u8, 1] {
        let mut s = Sidecar::default();
        s.record(everything());
        assert!(s.turn_by(1, Some(1.5)).is_some());
        let id = s.new_version_from_original(Edit::default(), None).unwrap();
        // As an import does: the line replaced, from where the frame
        // starts.
        s.line = Line::of(Edit::default(), made_for).unwrap();
        s.versions[0].line = Line::of(Edit::default(), made_for).unwrap();
        let mut e = Edit::default();
        e.geometry.crop = Some(crop);
        assert!(s.record_as(e.clone(), Some("Lightroom".into())));
        assert!(s.line_mut(Some(&id)).unwrap().record(e.clone()));
        assert_eq!(s.current_written().placed_under, 1, "made for {made_for}");
        let back = Sidecar::from_json(&s.to_json(), &crate::Edit::default()).unwrap();
        assert_eq!(
            back.current.geometry.crop,
            Some(crop),
            "made for {made_for}"
        );
        assert_eq!(
            back.version(&id).unwrap().line.current.geometry.crop,
            Some(crop),
            "made for {made_for}"
        );
    }
}

/// A turned version 9 file whose ids a build at 9 made: they verify here,
/// in the shape that build made them, so a twin whose cap took two of its
/// states joins with it and brings them back once, no state twice. A file
/// from before ids is given the ids such a build gave it.
#[test]
fn a_version_nine_file_s_ids_hold_in_a_join_and_a_fill() {
    let nine = |x: f32| {
        let mut e = crate::testing::exposed(x);
        e.version = 9;
        e
    };
    let mut ids = Vec::new();
    let mut parent = sync::ZERO_ID.to_string();
    for x in 0..7 {
        parent = sync::whole_id(&parent, &nine(x as f32), None);
        ids.push(parent.clone());
    }
    let file = |history: &[usize], with_ids: bool| {
        let state = |k: usize| {
            let mut v = serde_json::to_value(nine(k as f32)).unwrap();
            if with_ids {
                v["id"] = ids[k].clone().into();
            }
            v
        };
        let mut f = serde_json::json!({
            "current": state(6),
            "history": history.iter().map(|&k| state(k)).collect::<Vec<_>>(),
            "turn": 1,
        });
        if !with_ids {
            f.as_object_mut().unwrap().remove("id");
        }
        f.to_string()
    };
    let full =
        Sidecar::from_json(&file(&[0, 1, 2, 3, 4, 5], true), &crate::Edit::default()).unwrap();
    let capped = Sidecar::from_json(&file(&[0, 3, 4, 5], true), &crate::Edit::default()).unwrap();
    assert!(full.history.iter().all(|h| h.written().placed_under == 1));
    for j in [sync::join(&capped, &full), sync::join(&full, &capped)] {
        let got: Vec<f32> = j
            .history
            .iter()
            .map(|h| h.edit.light.exposure)
            .chain(std::iter::once(j.current.light.exposure))
            .collect();
        assert_eq!(got, [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }
    // From before ids: filled as the build at 9 filled them.
    let mut old =
        Sidecar::from_json(&file(&[0, 1, 2, 3, 4, 5], false), &crate::Edit::default()).unwrap();
    old.fill_ids();
    let filled: Vec<String> = old
        .history
        .iter()
        .map(|h| h.id.clone().unwrap())
        .chain(old.current_id.clone())
        .collect();
    assert_eq!(filled, ids);
}

/// A small generator of its own, so a seed runs the same again.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }

    fn unit(&mut self) -> f32 {
        self.below(1000) as f32 / 1000.0
    }
}

/// Random records, undos, redos, turns, snapshots, versions, and saves
/// read back: every state, once recorded, is written the same for
/// good, and every state is shown as its written self turned from the
/// turn it was placed under to the sidecar's.
#[test]
fn written_states_never_change_and_each_shows_turned_from_where_it_was_placed() {
    let dir = scratch("placed-property");
    let raw = dir.join("IMG_0001.CR3");
    for seed in 1..=16u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let aspect0 = if seed % 2 == 0 { 1.5 } else { 0.75 };
        let mut s = Sidecar::default();
        s.frame_known(aspect0);
        let mut seen: HashMap<String, Edit> = HashMap::new();
        let mut snaps = 0;
        for step in 0..60 {
            let ids: Vec<Option<String>> = std::iter::once(None)
                .chain(s.versions.iter().map(|v| Some(v.id.clone())))
                .collect();
            let v = ids[rng.below(ids.len() as u64) as usize].clone();
            match rng.below(10) {
                0..=3 => {
                    let mut line = s.line_mut(v.as_deref()).unwrap();
                    let mut e = line.current.clone();
                    e.light.exposure = rng.unit();
                    match rng.below(4) {
                        0 => e = everything(),
                        1 => {
                            e.geometry.crop =
                                Some(Crop::of([rng.unit() * 0.3, rng.unit() * 0.3, 0.5, 0.6]))
                        }
                        2 => e.adjustments.push(Adjustment {
                            id: e.adjustments.len() as u64 + 2,
                            mask: Mask {
                                components: vec![Component {
                                    shape: Shape::Radial {
                                        center: [rng.unit(), rng.unit()],
                                        radius: [0.1, 0.05],
                                        angle: 10.0,
                                        feather: 0.5,
                                    },
                                    ..Component::default()
                                }],
                                ..Mask::default()
                            },
                            ..Adjustment::default()
                        }),
                        _ => {}
                    }
                    line.record(e);
                }
                4 => {
                    s.line_mut(v.as_deref()).unwrap().undo();
                }
                5 => {
                    s.line_mut(v.as_deref()).unwrap().redo();
                }
                6 => {
                    s.turn_by(rng.below(3) as i32 + 1, None);
                }
                7 => {
                    snaps += 1;
                    s.line_mut(v.as_deref())
                        .unwrap()
                        .take_snapshot(format!("snap {snaps}"), 0);
                }
                8 => {
                    s.new_version(v.as_deref(), None);
                }
                _ => {
                    s.save_in_stamped(&raw, Placement::Beside, &stamp(step as u64 + 1, "desk"))
                        .unwrap();
                    s = Sidecar::load_as_file(&raw).unwrap().unwrap();
                    // As the window's read: the file says the shape.
                    s.frame_known(aspect_at(aspect0, s.turn));
                }
            }
            s.fill_ids();
            assert!(!s.needs_frame(), "seed {seed} step {step}");
            for (key, written, shown) in states(&s) {
                // (a) Written the same since it was first seen.
                match seen.get(&key) {
                    Some(was) => assert_eq!(*was, written, "seed {seed} step {step}: {key}"),
                    None => {
                        seen.insert(key.clone(), written.clone());
                    }
                }
                // (b) Shown as written, turned from where it was placed.
                let q = i32::from(s.turn) - i32::from(written.placed_under);
                let want = turned(&written, q, aspect_at(aspect0, written.placed_under));
                assert_eq!(shown, want, "seed {seed} step {step}: {key}");
            }
        }
    }
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A snapshot placed under a turn round-trips through the file, and one
/// made from what the file holds of it is the same snapshot.
#[test]
fn a_snapshot_placed_under_a_turn_round_trips() {
    let mut s = Sidecar::default();
    s.frame_known(1.5);
    s.turn_by(3, None);
    s.record(everything());
    s.take_snapshot("kept", 5);
    s.turn_by(1, None);
    let back = Sidecar::from_json(&s.to_json(), &crate::Edit::default()).unwrap();
    assert_eq!(back.snapshots, s.snapshots);
    assert_eq!(back.snapshots[0].written().placed_under, 3);
    let made = Snapshot::new("kept", 5, back.snapshots[0].written().into_owned());
    assert_eq!(made, back.snapshots[0]);
}

/// A file whose states wait on a shape nothing can measure, written out:
/// `everything()` recorded under no turn and snapshotted, the frame turned
/// a quarter by a window that knew its shape, and the shape taken out.
fn waiting_file(dir: &Path) -> (PathBuf, PathBuf, Sidecar) {
    let raw = dir.join("IMG_0001.CR3");
    let path = Sidecar::path_for(&raw);
    let mut s = Sidecar::default();
    s.record(everything());
    s.take_snapshot("kept", 1);
    assert!(s.turn_by(1, Some(1.5)).is_some());
    let mut file: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
    file.as_object_mut().unwrap().remove("shape");
    std::fs::write(&path, file.to_string()).unwrap();
    (raw, path, s)
}

/// A copy marked as waiting on the frame's shape is never made a state:
/// a line is not started from it, a version from the original is not
/// made from it, and an in-place change that leaves the current one is
/// put back. Read from outside the crate, the current is only lent
/// (`Line::current`).
#[test]
fn a_copy_waiting_on_the_shape_starts_no_line_and_replaces_no_state() {
    let dir = scratch("placed-marked");
    let (_, path, _) = waiting_file(&dir);
    let mut read = Sidecar::read_as_file(&path).unwrap();
    let marked = read.current().clone();
    assert!(marked.unturned);
    assert!(Line::of(marked.clone(), 1).is_none());
    let versions = read.versions.len();
    assert!(
        read.new_version_from_original(marked.clone(), None)
            .is_none()
    );
    assert_eq!(read.versions.len(), versions);
    // Over a state that does not wait: put back.
    let mut s = Sidecar::default();
    s.record(everything());
    let was = s.current().clone();
    assert!(!s.record(marked.clone()));
    assert_eq!(*s.current(), was);
    let mut e = was.clone();
    e.light.exposure = 1.0;
    assert!(s.record(e));
    assert_eq!(s.current().light.exposure, 1.0);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A wait fails closed: a state waiting on the frame's shape, changed in
/// place (a field set, a mask moved, on the current state and on a
/// snapshot), still waits and is still read-only, and is written as it
/// was; once the shape is known it shows from what was written, the
/// change not kept. Nothing is written under the turn it is shown at
/// with masks placed under another.
#[test]
fn a_waiting_state_changed_in_place_still_waits_and_is_written_as_it_was() {
    let dir = scratch("placed-fails-closed");
    let (raw, path, s) = waiting_file(&dir);
    let mut read = Sidecar::read_as_file(&path).unwrap();
    assert!(read.current_waits());
    let before: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    read.current.light.exposure = 2.0;
    let moved = read
        .current
        .adjustments
        .first_mut()
        .and_then(|a| a.mask.components.first_mut())
        .map(|c| &mut c.shape);
    let Some(Shape::Linear { from, .. }) = moved else {
        panic!("everything() starts with a linear mask");
    };
    *from = [0.9, 0.9];
    read.snapshots[0].edit.light.exposure = 3.0;
    assert!(read.current_waits(), "still waiting");
    assert_eq!(read.ready(), None);
    let mut e = read.current.clone();
    e.light.exposure = 1.0;
    assert!(!read.record(e), "still read-only");
    assert_eq!(read.current_written(), s.current_written(), "as written");
    read.save_in_stamped(&raw, Placement::Beside, &stamp(300, "desk"))
        .unwrap();
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(after["current"], before["current"]);
    assert_eq!(after["snapshots"], before["snapshots"]);
    // Told the shape: what was written, turned, and no change of the
    // copy that waited.
    read.frame_known(1.0 / 1.5);
    assert!(!read.current_waits());
    assert_eq!(read.current, s.current);
    assert_eq!(read.snapshots[0].edit, s.snapshots[0].edit);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A turn of a frame with nothing placed on it turns with no shape and
/// asks its reader nothing: the library's pass and a folder's open adopt
/// an XMP's turn this way for every frame, and their reader's ask is a
/// read of the frame's file. One with something placed asks, once, under
/// the turn the frame stands at.
#[test]
fn a_turn_with_nothing_placed_reads_no_file() {
    let mut s = Sidecar::default();
    let mut cropped = Edit::default();
    cropped.geometry.crop = Some(Crop::of([0.1, 0.2, 0.5, 0.6]));
    s.record(cropped);
    let asked = std::cell::RefCell::new(Vec::new());
    let ask = |turn: u8| {
        asked.borrow_mut().push(turn);
        Some(if turn.is_multiple_of(2) {
            1.5
        } else {
            1.0 / 1.5
        })
    };
    assert_eq!(s.turn_by_asking(1, ask), Some(None), "turned, no map");
    assert_eq!(s.turn, 1);
    assert!(asked.borrow().is_empty(), "nothing asked");
    // Something placed: the reader says the shape the turn is made by.
    s.record(everything());
    assert!(matches!(s.turn_by_asking(1, ask), Some(Some(_))));
    assert_eq!(*asked.borrow(), [1]);
    // Known now: a further turn asks nothing.
    assert!(s.turn_by_asking(1, ask).is_some());
    assert_eq!(*asked.borrow(), [1]);
}

/// A file whose turned states it has no shape for (a copy turned by a
/// build before version 10 joined with one placed under the old turn
/// here, or a file cut by hand) reads with them waiting and reads no
/// frame: the reader holding the frame is asked, under the file's turn,
/// and every state shows right by what it says. One with nothing waiting
/// is asked nothing.
#[test]
fn a_state_waiting_on_the_shape_is_shown_by_what_its_reader_says() {
    let mut s = Sidecar::default();
    s.record(everything());
    assert!(s.turn_by(1, Some(1.5)).is_some());
    let mut file: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
    assert!(file.as_object_mut().unwrap().remove("shape").is_some());
    let mut read = Sidecar::from_json(&file.to_string(), &crate::Edit::default()).unwrap();
    assert!(read.needs_frame());
    read.settle_shape(|_| None);
    assert!(
        read.needs_frame(),
        "a reader that cannot say leaves it waiting"
    );
    let mut asked = None;
    read.settle_shape(|turn| {
        asked = Some(turn);
        Some(1.0 / 1.5)
    });
    assert_eq!(asked, Some(1));
    assert!(!read.needs_frame());
    assert_eq!(read.current, s.current);
    read.settle_shape(|_| panic!("asked with nothing waiting"));
}

/// The shape is written only while a state with work placed on the
/// picture is placed under another turn than the file's (or wants
/// version 4's step): a turned frame with a crop and a history of
/// sliders writes none, however it was measured.
#[test]
fn a_turned_frame_with_nothing_placed_writes_no_shape() {
    let mut s = Sidecar::default();
    let mut cropped = Edit::default();
    cropped.geometry.crop = Some(Crop::of([0.1, 0.2, 0.5, 0.6]));
    s.record(cropped.clone());
    cropped.light.exposure = 0.5;
    s.record(cropped);
    assert!(s.turn_by(1, Some(1.5)).is_some());
    let has_shape = |s: &Sidecar| {
        serde_json::from_str::<serde_json::Value>(&s.to_json()).unwrap()["shape"]
            != serde_json::Value::Null
    };
    assert!(!has_shape(&s), "nothing placed under another turn");
    // Placed under the turn the file stands at: none still.
    s.record(everything());
    assert!(!has_shape(&s));
    // Turned again, it is placed under another: written.
    assert!(s.turn_by(1, None).is_some());
    assert!(has_shape(&s));
}

/// A line made for another turn, put on a turned sidecar, is shown
/// under the sidecar's turn before anything reads it through a mutable
/// entry: the deref onto version 0's line, and `lines_mut` over every
/// version's.
#[test]
fn a_line_put_on_a_turned_sidecar_is_shown_before_a_mutable_read() {
    let want = turned(&everything(), 1, 1.5);
    let turned_sidecar = || {
        let mut s = Sidecar::default();
        assert!(s.turn_by(1, Some(1.5)).is_some());
        s
    };
    // Through the deref.
    let mut s = turned_sidecar();
    s.line = Line::of(everything(), 0).unwrap();
    let current: &mut Edit = &mut s.current;
    assert_eq!(current.adjustments, want.adjustments, "the deref");
    // Through `lines_mut`, on a version's line.
    let mut s = turned_sidecar();
    let id = s.new_version_from_original(Edit::default(), None).unwrap();
    s.versions[0].line = Line::of(everything(), 0).unwrap();
    let shown: Vec<Edit> = s.lines_mut().map(|l| l.current.clone()).collect();
    assert_eq!(shown[1].adjustments, want.adjustments, "lines_mut");
    assert_eq!(
        s.version(&id).unwrap().line.current_written().placed_under,
        0
    );
}

/// An undone state that waits on the frame's shape is never recorded
/// again from its copy, whether or not it is still in the line, nor is
/// anything made from that copy. Undo, redo, a history click and a
/// snapshot's restore onto it move it as written: the file is written
/// as it was, and the line is read-only while it is current.
#[test]
fn a_waiting_undone_state_is_gone_to_but_never_recorded_again() {
    let dir = scratch("placed-undone");
    let raw = dir.join("IMG_0001.CR3");
    let path = Sidecar::path_for(&raw);
    // A, a slider and nothing placed; B, placed under no turn; the frame
    // turned by a window that knew its shape, the shape not written.
    let mut s = Sidecar::default();
    let mut a = Edit::default();
    a.light.exposure = 0.3;
    s.record(a);
    s.record(everything());
    s.take_snapshot("B", 1);
    assert!(s.turn_by(1, Some(1.5)).is_some());
    s.shape = None;
    std::fs::write(&path, s.to_json()).unwrap();
    let as_written = || Sidecar::read_as_file(&path).unwrap();

    // Undone to A, which takes a step; B's copy, taken before the step
    // dropped B, and an edit made from it, are refused.
    let mut read = as_written();
    assert!(read.current_waits());
    assert!(read.undo());
    assert!(!read.current_waits(), "A has nothing placed");
    let b_at = read.position() + 1;
    assert!(read.waits_at(b_at));
    let b = read.state(b_at).cloned().unwrap();
    let mut moved = read.current.clone();
    moved.light.exposure = 0.5;
    assert!(read.record(moved), "a step on A");
    assert!(read.redo.is_empty(), "B undone, and dropped by the step");
    assert!(!read.record(b.clone()), "B's copy");
    let mut made = b;
    made.light.exposure = 0.9;
    assert!(!read.record(made), "an edit made from B's copy");
    assert_eq!(read.history.len(), 2, "the step on A, and nothing after");

    // Gone to: as written, and read-only while current.
    let before = as_written();
    let mut again = as_written();
    let read_only = |s: &mut Sidecar, how: &str| {
        assert!(s.current_waits(), "{how}");
        let mut e = s.current.clone();
        e.light.exposure = 1.0;
        assert!(!s.record(e), "{how}: no step");
    };
    assert!(again.undo());
    assert!(again.redo());
    read_only(&mut again, "redo");
    assert!(again.undo());
    assert!(again.go_to(b_at));
    read_only(&mut again, "a click");
    assert!(again.undo());
    assert!(again.restore_snapshot(0));
    read_only(&mut again, "a snapshot");
    assert_eq!(again.current_written(), before.current_written());
    assert!(again.undo());
    assert!(again.redo());
    again
        .save_in_stamped(&raw, Placement::Beside, &stamp(400, "desk"))
        .unwrap();
    let back = as_written();
    assert_eq!(back.current_written(), before.current_written());
    for (k, step) in back.history.iter().enumerate() {
        assert_eq!(step.written(), before.history[k].written(), "step {k}");
    }
    greycard_core::testing::remove_dir_retry(&dir);
}

/// While a turn is owed (an XMP's, nothing able to measure the frame
/// when it was read): the file keeps the shape the window measured, so a
/// later read can make the turn by it; an XMP written beside the frame
/// says the owed turn, not the frame's own; and a turn made, by hand or
/// otherwise, ends it, so a later read does not make it over that turn.
#[test]
fn an_owed_turn_keeps_the_shape_and_the_xmp_s_turn_until_a_turn_is_made() {
    let mut s = Sidecar::default();
    s.record(everything());
    s.turn_owed = Some(1);
    assert_eq!(s.xmp_turn(), 1, "the owed turn, for the XMP");
    s.frame_known(1.5);
    assert_eq!(s.written_shape(), Measured::new(0, 1.5), "kept while owed");
    let back = Sidecar::from_json(&s.to_json(), &crate::Edit::default()).unwrap();
    assert_eq!(
        (back.turn_owed, back.shape),
        (Some(1), Measured::new(0, 1.5))
    );
    // Turned by hand to 3: owed no more, and the XMP says the frame's.
    assert!(s.turn_by(3, None).is_some());
    assert_eq!(s.turn_owed, None);
    assert_eq!(s.xmp_turn(), 3);
}
