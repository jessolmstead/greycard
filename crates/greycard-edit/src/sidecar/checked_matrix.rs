//! Every change a window can make against every change another writer
//! can make, saved through [`Sidecar::save_checked`]: every state carries
//! a crop, a keystone, a radial mask, a brush stroke, a retouch patch and
//! a look, with a snapshot and a virtual copy. Built from a review's
//! matrix. In every cell the outcome is one of the four, the file is
//! what the window holds, and that is exactly the window's sidecar, the
//! other writer's (its meta joined), or the window's with the other's
//! meta; when the other's file is kept, it is kept byte for byte; and
//! where the other is a window too, the two saving in turn settle. A
//! kept file is no lineage: the archive sync, holding the other's file,
//! reads ours as it reads a last-writer-wins save's.

use super::checked::KEEP_OTHERS;
use super::checked_tests::{other_saves, stamp};
use super::*;
use crate::mask::{Component, Mask, Shape};
use crate::retouch::{Method, Patch};
use crate::testing::scratch;

const ASPECT0: f32 = 1.5;
const C0: [f32; 4] = [0.1, 0.2, 0.5, 0.6];
const C1: [f32; 4] = [0.05, 0.05, 0.7, 0.4];

fn aspect_at(turn: u8) -> f32 {
    if turn.is_multiple_of(2) {
        ASPECT0
    } else {
        1.0 / ASPECT0
    }
}

/// A real turn, as the UI makes it: with the picture's aspect.
fn turn_real(s: &mut Sidecar, q: i32) {
    for _ in 0..q.rem_euclid(4) {
        let a = aspect_at(s.turn);
        s.frame_known(a);
        s.turn_by(1, None);
    }
}

fn base(ev: f32, crop: [f32; 4]) -> Edit {
    let mut e = crate::testing::exposed(ev);
    e.geometry.crop = Some(crate::geometry::Crop::of(crop));
    e.geometry.vertical = 0.1;
    e.light.tone.contrast = 0.2;
    e.look_lut.lut = crate::look::LutChoice::Named("Glow".into());
    e.adjustments.push(crate::Adjustment {
        id: 1,
        name: "Face".into(),
        mask: Mask {
            components: vec![
                Component {
                    shape: Shape::Radial {
                        center: [0.48, 0.42],
                        radius: [0.12, 0.09],
                        angle: 20.0,
                        feather: 0.5,
                    },
                    ..Component::default()
                },
                Component {
                    shape: Shape::Brush {
                        strokes: vec![crate::brush::Stroke {
                            points: vec![[0.2, 0.3], [0.25, 0.31]],
                            radius: 0.03,
                            feather: 0.5,
                            flow: 1.0,
                            op: Default::default(),
                        }],
                    },
                    ..Component::default()
                },
            ],
            ..Mask::default()
        },
        ..crate::Adjustment::default()
    });
    e.retouch.patches.push(Patch {
        id: 1,
        method: Method::Heal,
        points: vec![[0.31, 0.77]],
        radius: 0.02,
        source: Some([0.05, -0.03]),
        ..Patch::default()
    });
    e
}

/// What a state's geometry and placed work are, in numbers.
fn fp(e: &Edit) -> Vec<f32> {
    let mut v = Vec::new();
    if let Some(c) = e.geometry.crop {
        v.extend([c.x, c.y, c.w, c.h]);
    }
    v.extend([
        e.geometry.vertical,
        e.geometry.horizontal,
        e.geometry.turns as f32,
    ]);
    for a in &e.adjustments {
        for c in &a.mask.components {
            match &c.shape {
                Shape::Radial {
                    center,
                    radius,
                    angle,
                    ..
                } => {
                    v.extend(center);
                    v.extend(radius);
                    v.push(*angle);
                }
                Shape::Brush { strokes } => {
                    for s in strokes {
                        for p in &s.points {
                            v.extend(p);
                        }
                        v.push(s.radius);
                    }
                }
                _ => {}
            }
        }
    }
    for p in &e.retouch.patches {
        for q in &p.points {
            v.extend(q);
        }
        v.push(p.radius);
        if let Some(s) = p.source {
            v.extend(s);
        }
    }
    v
}

/// The fingerprints a state may have under `turn`: from C0 or C1.
fn refs(turn: u8) -> Vec<Vec<f32>> {
    [C0, C1]
        .iter()
        .map(|c| {
            let mut s = Sidecar {
                line: crate::Line::of(base(0.0, *c), 0).unwrap(),
                ..Sidecar::default()
            };
            turn_real(&mut s, i32::from(turn));
            fp(&s.current)
        })
        .collect()
}

fn close(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-3)
}

/// Invariant (i): every edit's geometry and placed work agree with the
/// sidecar's turn. Lists where it does not.
fn geometry_bad(s: &Sidecar) -> Vec<String> {
    // A sidecar read from its file shows a state placed under another
    // turn once it knows the frame's shape, which the window's read
    // asks the raw for (`on_disk`); here the frame is ASPECT0's.
    let mut s = s.clone();
    if s.needs_frame() {
        s.frame_known(aspect_at(s.turn));
    }
    let s = &s;
    let want = refs(s.turn);
    let mut bad = Vec::new();
    let mut check = |what: String, e: &Edit| {
        if e.geometry.crop.is_none() {
            return;
        }
        let f = fp(e);
        if !want.iter().any(|w| close(w, &f)) {
            // Which part: crop only, masks only?
            let any_turn = (0..4u8).find(|t| refs(*t).iter().any(|w| close(w, &f)));
            let crop_t = (0..4u8).find(|t| {
                refs(*t)
                    .iter()
                    .any(|w| close(&w[..7], &f[..7.min(f.len())]))
            });
            let rest_t = (0..4u8).find(|t| {
                refs(*t)
                    .iter()
                    .any(|w| f.len() >= 7 && close(&w[7..], &f[7..]))
            });
            bad.push(format!(
                "{what}:whole={any_turn:?},geom={crop_t:?},placed={rest_t:?}"
            ));
        }
    };
    for (vi, l) in s.lines().enumerate() {
        for (i, h) in l.history.iter().enumerate() {
            check(format!("v{vi}h{i}"), &h.edit);
        }
        check(format!("v{vi}cur"), &l.current);
        for (i, h) in l.redo.iter().enumerate() {
            check(format!("v{vi}r{i}"), &h.edit);
        }
        for (i, h) in l.snapshots.iter().enumerate() {
            check(format!("v{vi}s{i}"), &h.edit);
        }
    }
    bad
}

type WinOp = fn(&mut Sidecar);
type WinCheck = fn(&Sidecar) -> bool;

fn vc(s: &Sidecar) -> Option<String> {
    s.versions
        .iter()
        .find(|v| v.name == "VC")
        .map(|v| v.id.clone())
}

fn slider(s: &mut Sidecar, ev: f32) {
    let mut e = s.current.clone();
    e.light.exposure = ev;
    s.record(e);
}

fn win_ops() -> Vec<(&'static str, WinOp, WinCheck, bool)> {
    // (name, op, the window's own thing kept, keeps ids hashing)
    vec![
        ("nothing", |_| {}, |_| true, true),
        (
            "rating",
            |s| s.meta.rating = 3,
            |s| s.meta.rating == 3,
            true,
        ),
        (
            "slider",
            |s| slider(s, 1.0),
            |s| s.current.light.exposure == 1.0,
            true,
        ),
        (
            "undo",
            |s| {
                s.undo();
            },
            |s| s.current.light.exposure == 0.5,
            true,
        ),
        (
            "undo-slider",
            |s| {
                s.undo();
                slider(s, 1.25);
            },
            |s| s.current.light.exposure == 1.25,
            true,
        ),
        (
            "snap",
            |s| {
                s.take_snapshot("mine", 7);
            },
            |s| s.snapshots.iter().any(|x| x.name == "mine"),
            true,
        ),
        (
            "vc-edit",
            |s| {
                let id = vc(s).unwrap();
                let mut l = s.line_mut(Some(&id)).unwrap();
                let mut e = l.current.clone();
                e.light.exposure = 2.0;
                l.record(e);
            },
            |s| {
                s.versions
                    .iter()
                    .any(|v| v.line.current.light.exposure == 2.0)
            },
            true,
        ),
        ("turn", |s| turn_real(s, 1), |s| s.turn == 1, false),
        (
            "turn-slider",
            |s| {
                turn_real(s, 1);
                slider(s, 1.0);
            },
            |s| s.turn == 1 && s.current.light.exposure == 1.0,
            false,
        ),
        (
            "turn-crop",
            |s| {
                turn_real(s, 1);
                let mut e = base(1.0, C1);
                let mut t = Sidecar {
                    line: crate::Line::of(e.clone(), 0).unwrap(),
                    ..Sidecar::default()
                };
                turn_real(&mut t, 1);
                e = t.current.clone();
                s.record(e);
            },
            |s| s.turn == 1 && s.current.light.exposure == 1.0,
            false,
        ),
        ("turn2", |s| turn_real(s, 2), |s| s.turn == 2, false),
        (
            "mem-rename",
            |s| {
                s.rename_look("Glow", "Neon");
            },
            |s| !s.current.look_lut.names("Glow"),
            false,
        ),
        (
            "mem-value",
            |s| s.line.current.light.tone.contrast = 0.3,
            |s| s.current.light.tone.contrast == 0.3,
            false,
        ),
    ]
}

fn replace(p: &Path, bytes: &[u8]) {
    let _ = std::fs::remove_file(p);
    std::fs::write(p, bytes).unwrap();
}

/// Remove `key` from every object under `v` named `parent`.
fn drop_key(v: &mut serde_json::Value, parent: &str, key: &str) {
    match v {
        serde_json::Value::Object(o) => {
            for (k, c) in o.iter_mut() {
                if k == parent
                    && let Some(m) = c.as_object_mut()
                {
                    m.remove(key);
                }
                drop_key(c, parent, key);
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(|c| drop_key(c, parent, key)),
        _ => {}
    }
}

type OtherOp = fn(&Path, &[u8]) -> Option<Sidecar>;

fn other_ops() -> Vec<(&'static str, OtherOp, bool)> {
    // (name, op -> the other's live sidecar when it is a window, keeps ids hashing)
    vec![
        ("none", |_, _| None, true),
        (
            "rating",
            |raw, _| {
                Some(other_saves(raw, Placement::Beside, 150, |o| {
                    o.meta.rating = 5
                }))
            },
            true,
        ),
        (
            "slider",
            |raw, _| Some(other_saves(raw, Placement::Beside, 150, |o| slider(o, 0.9))),
            true,
        ),
        (
            "turn",
            |raw, _| {
                Some(other_saves(raw, Placement::Beside, 150, |o| {
                    turn_real(o, 1)
                }))
            },
            false,
        ),
        (
            "turn3",
            |raw, _| {
                Some(other_saves(raw, Placement::Beside, 150, |o| {
                    turn_real(o, 3)
                }))
            },
            false,
        ),
        (
            "turn-slider",
            |raw, _| {
                Some(other_saves(raw, Placement::Beside, 150, |o| {
                    turn_real(o, 1);
                    slider(o, 0.9);
                }))
            },
            false,
        ),
        (
            "turn-crop",
            |raw, _| {
                Some(other_saves(raw, Placement::Beside, 150, |o| {
                    turn_real(o, 1);
                    let mut t = Sidecar {
                        line: crate::Line::of(base(0.9, C1), 0).unwrap(),
                        ..Sidecar::default()
                    };
                    turn_real(&mut t, 1);
                    o.record(t.current.clone());
                }))
            },
            false,
        ),
        (
            "turn2",
            |raw, _| {
                Some(other_saves(raw, Placement::Beside, 150, |o| {
                    turn_real(o, 2)
                }))
            },
            false,
        ),
        (
            "look-rename",
            |raw, _| {
                Some(other_saves(raw, Placement::Beside, 150, |o| {
                    o.rename_look("Glow", "Neon");
                }))
            },
            false,
        ),
        (
            "seed",
            |raw, _| {
                Some(other_saves(raw, Placement::Beside, 150, |o| {
                    o.line.current.noise.learned_strength = 0.7
                }))
            },
            false,
        ),
        (
            "inplace-hist",
            |raw, _| {
                Some(other_saves(raw, Placement::Beside, 150, |o| {
                    o.line.history[1].edit.light.tone.contrast = 0.4
                }))
            },
            false,
        ),
        (
            "old-drop",
            |raw, _| {
                let p = Sidecar::path_for(raw);
                let mut v: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
                drop_key(&mut v, "tone", "contrast");
                replace(&p, serde_json::to_string_pretty(&v).unwrap().as_bytes());
                None
            },
            false,
        ),
        (
            "old-drop-change",
            |raw, _| {
                let p = Sidecar::path_for(raw);
                let mut v: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
                drop_key(&mut v, "tone", "contrast");
                v["current"]["light"]["exposure"] = serde_json::json!(0.66);
                replace(&p, serde_json::to_string_pretty(&v).unwrap().as_bytes());
                None
            },
            false,
        ),
        (
            "unrelated",
            |raw, _| {
                let mut o = Sidecar::default();
                o.record(base(3.0, C0));
                o.write_to(&Sidecar::path_for(raw)).unwrap();
                None
            },
            true,
        ),
        (
            "rollback",
            |raw, before| {
                replace(&Sidecar::path_for(raw), before);
                None
            },
            true,
        ),
        (
            "same-edit",
            |raw, _| Some(other_saves(raw, Placement::Beside, 150, |o| slider(o, 1.0))),
            true,
        ),
        (
            "deleted",
            |raw, _| {
                std::fs::remove_file(Sidecar::path_for(raw)).unwrap();
                None
            },
            true,
        ),
        (
            "moved",
            |raw, _| {
                let mut o = Sidecar::load_as_file(raw).unwrap().unwrap();
                o.meta.rating = 2;
                o.save_in_stamped(raw, Placement::Folder, &stamp(150))
                    .unwrap();
                Some(o)
            },
            true,
        ),
        (
            "newer",
            |raw, _| {
                let p = Sidecar::path_for(raw);
                let mut v: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
                v["current"]["version"] = serde_json::json!(crate::VERSION + 1);
                replace(&p, serde_json::to_string_pretty(&v).unwrap().as_bytes());
                None
            },
            true,
        ),
        (
            "torn",
            |raw, _| {
                let p = Sidecar::path_for(raw);
                let b = std::fs::read(&p).unwrap();
                replace(&p, &b[..b.len() / 2]);
                None
            },
            true,
        ),
    ]
}

/// The window's sidecar: four states, a redo of none, a snapshot, a
/// virtual copy, written and read.
fn start(name: &str) -> (PathBuf, PathBuf, Sidecar, Vec<u8>) {
    let dir = scratch(name);
    let raw = dir.join("IMG_0001.CR3");
    let mut s = Sidecar::default();
    s.record(base(0.25, C0));
    s.record(base(0.5, C0));
    s.record(base(0.75, C0));
    s.take_snapshot("snap", 1);
    s.new_version(None, Some("VC"));
    s.save_in_stamped(&raw, Placement::Beside, &stamp(100))
        .unwrap();
    let bytes = std::fs::read(Sidecar::path_for(&raw)).unwrap();
    let read = Sidecar::read_as_file(&Sidecar::path_for(&raw)).unwrap();
    (dir, raw, read, bytes)
}

/// `s` as the file holds it: the redo stack written as undone (§724).
fn strip(s: &Sidecar) -> Sidecar {
    let mut s = s.clone();
    for l in s.lines_mut() {
        l.leave_redo();
    }
    s
}

fn lines_eq(a: &Sidecar, b: &Sidecar) -> bool {
    let (a, b) = (strip(a), strip(b));
    a.line == b.line && a.versions == b.versions && a.turn == b.turn && a.meta == b.meta
}

fn now(k: u64) -> sync::Stamp {
    stamp(sync::Stamp::now().at + 100 + k)
}

/// The other writers' files kept beside the frame's sidecar.
fn kept(raw: &Path) -> Vec<PathBuf> {
    let path = Sidecar::path_for(raw);
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            let f = p.file_name()?.to_string_lossy().into_owned();
            (f.starts_with(&name) && f.ends_with(".other")).then_some(p)
        })
        .collect()
}

#[test]
fn every_cell_keeps_both_sides_whole() {
    let mut bad = Vec::new();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for (wn, wop, wcheck, _) in win_ops() {
        for (on, oop, _) in other_ops() {
            let (dir, raw, mut s, bytes) = start(&format!("matrix-{wn}-{on}"));
            wop(&mut s);
            let ours = s.clone();
            let mut other = oop(&raw, &bytes);
            let path = Sidecar::path_for(&raw);
            let file_before = std::fs::read(&path)
                .ok()
                .or_else(|| std::fs::read(Sidecar::path_in(&raw, Placement::Folder)).ok());
            let theirs = file_before.as_ref().and_then(|b| {
                Sidecar::from_json(std::str::from_utf8(b).ok()?, &crate::Edit::default()).ok()
            });
            let at = now(0);
            let res = s.save_checked_said(&raw, Placement::Beside, &at);
            let mut say = |what: String| bad.push(format!("{wn} x {on}: {what}"));
            let saved = match res {
                Ok(saved) => saved,
                Err(e) => {
                    if on != "newer" {
                        say(format!("not saved: {}", e.error));
                    }
                    *counts.entry("refused".into()).or_default() += 1;
                    greycard_core::testing::remove_dir_retry(&dir);
                    continue;
                }
            };
            *counts.entry(format!("{:?}", saved.checked)).or_default() += 1;
            let file = Sidecar::load_as_file(&raw).unwrap().unwrap();
            if file.to_json() != s.to_json() {
                say("the file is not what the window holds".into());
            }
            let same = |a: &Sidecar, b: &Sidecar| lines_eq(a, b);
            let lines_of = |a: &Sidecar, b: &Sidecar| {
                strip(a).line == strip(b).line && a.versions == b.versions && a.turn == b.turn
            };
            match saved.checked {
                Checked::Written | Checked::Kept => {
                    if !same(&s, &ours) {
                        say(format!("{:?} but not the window's sidecar", saved.checked));
                    }
                }
                Checked::Took => {
                    if !theirs.as_ref().is_some_and(|t| same(&s, t)) {
                        say("taken but not the other's sidecar".into());
                    }
                }
                Checked::Joined => {
                    let theirs_lines = theirs.as_ref().is_some_and(|t| lines_of(&s, t));
                    if !lines_of(&s, &ours) && !theirs_lines {
                        say("joined, but the lines are neither side's".into());
                    }
                }
            }
            if !wcheck(&s) && saved.checked != Checked::Took {
                say("the window's change".into());
            }
            if saved.checked == Checked::Kept {
                let kept = kept(&raw);
                let kept_bytes: Vec<Vec<u8>> =
                    kept.iter().map(|k| std::fs::read(k).unwrap()).collect();
                if !file_before.as_ref().is_some_and(|b| kept_bytes.contains(b)) {
                    say("kept, but not the other's bytes".into());
                }
                // The archive sync, holding the other's file, reads ours
                // as it reads a last-writer-wins save's.
                if let Some(t) = &theirs {
                    let mut last_writer = ours.clone();
                    last_writer.advance(&at);
                    let (branch, master) =
                        (sync::compare(&file, t), sync::compare(&last_writer, t));
                    if branch != master {
                        say(format!("the archive reads {branch:?}, not {master:?}"));
                    }
                    *counts.entry("archive as master".into()).or_default() += 1;
                }
            }
            if !geometry_bad(&s).is_empty() {
                say(format!(
                    "a state's geometry not for the turn: {:?}",
                    geometry_bad(&s)
                ));
            }
            // Two windows saving in turn settle.
            if let Some(o) = other.as_mut() {
                let size = |x: &Sidecar| {
                    x.lines()
                        .map(|l| l.history.len() + l.redo.len() + l.snapshots.len())
                        .sum::<usize>()
                };
                let mut sizes = Vec::new();
                let mut outcomes = Vec::new();
                for k in 0..40u64 {
                    let a = o.save_checked(&raw, Placement::Beside, &now(300 + 2 * k));
                    let b = s.save_checked(&raw, Placement::Beside, &now(301 + 2 * k));
                    outcomes.push((a.ok(), b.ok()));
                    sizes.push((size(o), size(&s), s.versions.len(), o.versions.len()));
                }
                let settled = outcomes[2..].iter().all(|(a, b)| {
                    matches!(a, Some(Checked::Written | Checked::Took))
                        && matches!(b, Some(Checked::Written | Checked::Took))
                });
                if !settled || sizes[2..].windows(2).any(|w| w[0] != w[1]) || !same(o, &s) {
                    say(format!("in turn, no settling: {:?}", &outcomes[..4]));
                }
                if kept(&raw).len() > KEEP_OTHERS {
                    say("more files kept than the bound".into());
                }
                if !geometry_bad(&s).is_empty() {
                    say("a state's geometry not for the turn, after saving in turn".into());
                }
            }
            greycard_core::testing::remove_dir_retry(&dir);
        }
    }
    println!("outcomes: {counts:?}");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// A frame with no geometry: a turn leaves the lines' bytes as they
/// were, so the print alone cannot tell a turn, and the save's turn
/// checks are what keep a turn from being taken for nothing.
#[test]
fn a_turn_without_geometry_leaves_the_lines_print() {
    use super::checked::lines_print;
    let mut s = Sidecar::default();
    s.record(crate::testing::exposed(0.25));
    s.fill_ids();
    let mut t = s.clone();
    t.turn_by(1, Some(ASPECT0));
    assert_eq!(
        lines_print(s.to_json().as_bytes()),
        lines_print(t.to_json().as_bytes())
    );
}

/// The turn cells with no geometry: a turn on one side, a rating, a
/// slider, a turn, or nothing on the other. Each the outcome, turn, and
/// rating the file ends with.
#[test]
fn turn_cells_without_geometry() {
    let act = |x: &mut Sidecar, what: &str| match what {
        "turn" => {
            x.turn_by(1, Some(aspect_at(x.turn)));
        }
        "rating" => x.meta.rating = 3,
        "slider" => {
            x.record(crate::testing::exposed(-1.0));
        }
        _ => {}
    };
    for (there, here, want, turn, rating) in [
        ("turn", "nothing", Checked::Took, 1, 0),
        ("turn", "rating", Checked::Joined, 1, 3),
        ("rating", "turn", Checked::Joined, 1, 3),
        ("slider", "turn", Checked::Kept, 1, 0),
        ("turn", "slider", Checked::Kept, 0, 0),
        ("turn", "turn", Checked::Kept, 1, 0),
        ("nothing", "turn", Checked::Written, 1, 0),
    ] {
        let dir = scratch(&format!("matrix-flat-{there}-{here}"));
        let raw = dir.join("IMG_0001.CR3");
        let mut s = Sidecar::default();
        s.record(crate::testing::exposed(0.25));
        s.save_in_stamped(&raw, Placement::Beside, &stamp(100))
            .unwrap();
        let mut s = Sidecar::read_as_file(&Sidecar::path_for(&raw)).unwrap();
        if there != "nothing" {
            other_saves(&raw, Placement::Beside, 150, |o| act(o, there));
        }
        act(&mut s, here);
        let c = s
            .save_checked(&raw, Placement::Beside, &stamp(200))
            .unwrap();
        let file = Sidecar::load_as_file(&raw).unwrap().unwrap();
        greycard_core::testing::remove_dir_retry(&dir);
        assert_eq!(
            (c, file.turn, file.meta.rating),
            (want, turn, rating),
            "{there} there, {here} here"
        );
    }
}
