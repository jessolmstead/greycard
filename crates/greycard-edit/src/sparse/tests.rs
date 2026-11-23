//! A state written as what differs from its defaults, read back as it
//! was; ids made from that form whatever writes the file; and a later
//! build's differences read strictly, or lost alone (§724).

use super::*;
use crate::Sidecar;
use crate::sync::{ZERO_ID, join, state_id};
use crate::testing::exposed;

fn read(json: &str) -> Sidecar {
    Sidecar::from_json(json, &Edit::default()).unwrap()
}

/// `f` with every state written whole, as before defaults were left out.
fn whole<R>(f: impl FnOnce() -> R) -> R {
    WHOLE.set(true);
    let r = f();
    WHOLE.set(false);
    r
}

/// A sidecar with one of nearly everything: masks of every shape, a
/// brush, a repair spot with a chosen source, words, a snapshot, a turn,
/// and a version.
fn rich() -> Sidecar {
    let mut s = Sidecar::default();
    s.record(exposed(0.4));
    s.record(crate::sidecar::placed_tests::everything());
    let mut offset = exposed(0.5);
    offset.white_balance = crate::WhiteBalance::Offset {
        mired: 10.0,
        tint: 0.0,
    };
    s.record(offset);
    s.record_as(exposed(-0.2), Some("Preset: Faded film".into()));
    s.take_snapshot("Kept", 1_800_000_000);
    let mut e = s.current().clone();
    e.retouch.patches.push(retouch::Patch {
        id: e.retouch.next_id(),
        method: retouch::Method::Heal,
        points: vec![[0.4, 0.6]],
        radius: 0.03,
        ..retouch::Patch::default()
    });
    s.record(e.clone());
    e.retouch.patches[0].source = Some([0.05, -0.02]);
    s.take_sources(&e.retouch.patches);
    s.frame_known(1.5);
    s.turn_by(1, None).unwrap();
    let mut e = s.current().clone();
    e.light.tone.contrast = 12.0;
    s.record(e);
    let id = s.new_version(None, Some("Warm")).unwrap();
    s.line_mut(Some(&id)).unwrap().record(exposed(0.9));
    s.fill_ids();
    s
}

/// A state as the file has it: its text, id, parent, and words.
type Entry = (Value, Option<String>, Option<String>, Option<String>);

/// Every state's text in the file, its id, parent, and words: what an
/// id is made from.
fn entries(json: &str) -> Vec<Entry> {
    let file: Value = serde_json::from_str(json).unwrap();
    let mut out = Vec::new();
    let lines = std::iter::once(&file).chain(
        file.get("versions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.get("line")),
    );
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(String::from);
    for line in lines {
        for h in line
            .get("history")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let mut edit = h.clone();
            for k in ["id", "parent", "step", "exported"] {
                edit.as_object_mut().unwrap().remove(k);
            }
            out.push((edit, s(h, "id"), s(h, "parent"), s(h, "step")));
        }
        if let Some(c) = line.get("current") {
            out.push((c.clone(), s(line, "id"), s(line, "parent"), s(line, "step")));
        }
    }
    out
}

/// A sidecar written as what differs from its defaults reads back the
/// same, and writes again the same bytes; written whole, it reads as
/// the same sidecar.
#[test]
fn a_sparse_sidecar_reads_back_as_written_and_as_a_whole_one() {
    let s = rich();
    let sparse = s.to_json();
    let back = read(&sparse);
    assert_eq!(back.to_json(), sparse, "byte for byte");
    let whole_text = whole(|| s.to_json());
    assert!(whole_text.len() > sparse.len());
    let from_whole = read(&whole_text);
    assert_eq!(from_whole, back, "one sidecar, whichever way written");
    assert_eq!(from_whole.to_json(), sparse);
}

/// Every state written says its version, the default edit's too.
#[test]
fn a_state_always_says_its_version() {
    let mut s = Sidecar::default();
    s.record(exposed(0.1));
    for (edit, ..) in entries(&s.to_json()) {
        assert!(edit.get("version").is_some(), "{edit}");
    }
    assert_eq!(
        serde_json::to_string(&written_value(&Edit::default())).unwrap(),
        format!(
            r#"{{"process":{},"version":{}}}"#,
            crate::PROCESS,
            crate::VERSION
        )
    );
}

/// Every key the sparse form may leave out, at every path of a rich
/// edit, reads as its container's default: what serde fills a missing
/// field with is what the form takes as its default.
#[test]
fn a_field_left_out_reads_as_the_default_it_was_left_out_for() {
    let mut edits = vec![crate::sidecar::placed_tests::everything()];
    let mut all = exposed(0.7);
    all.light.tone.contrast = 10.0;
    all.vignette.amount = -20.0;
    all.grain.amount = 30.0;
    // As shot with an offset: its `mode` is the default's, and is
    // written all the same.
    all.white_balance = crate::WhiteBalance::Offset {
        mired: 10.0,
        tint: 0.0,
    };
    all.retouch.patches.push(retouch::Patch {
        id: 1,
        method: retouch::Method::Clone,
        points: vec![[0.2, 0.2]],
        radius: 0.04,
        source: Some([0.1, 0.0]),
        more: vec![retouch::Stroke {
            points: vec![[0.3, 0.3]],
            radius: 0.02,
        }],
        ..retouch::Patch::default()
    });
    edits.push(all);
    for edit in edits {
        let full = full_value(&edit);
        let mut checked = 0;
        each_removable(
            &full,
            Some(&DEFAULTS.edit),
            &mut String::new(),
            &mut Vec::new(),
            &mut |path, d| {
                let mut without = full.clone();
                let mut with = full.clone();
                remove_at(&mut without, path);
                set_at(&mut with, path, d.clone());
                let a: Edit = serde_json::from_value(without)
                    .unwrap_or_else(|e| panic!("left out at {path:?}: {e}"));
                let b: Edit = serde_json::from_value(with).unwrap();
                assert_eq!(a, b, "left out at {path:?}");
                checked += 1;
            },
        );
        assert!(checked > 100, "{checked}");
    }
}

/// A step of a path into a value.
#[derive(Debug, Clone)]
enum Step {
    Key(String),
    At(usize),
}

/// Every key `strip` may leave out of `v` against a default, with that
/// default, as [`strip`] walks; `at` is where `v` is, as `strip` says
/// it. What a type's reading leaves out ([`read_at`]) is read back as
/// it is left out, and needs no walk.
fn each_removable(
    v: &Value,
    d: Option<&Value>,
    at: &mut String,
    path: &mut Vec<Step>,
    f: &mut impl FnMut(&[Step], &Value),
) {
    match v {
        Value::Array(items) => {
            let e = DEFAULTS.element(at);
            let len = at.len();
            at.push_str("[]");
            for (i, x) in items.iter().enumerate() {
                path.push(Step::At(i));
                each_removable(x, e, at, path, f);
                path.pop();
            }
            at.truncate(len);
        }
        Value::Object(o) => {
            let d = if read_at(at).is_some() { None } else { d };
            let d = d.and_then(Value::as_object);
            for (k, x) in o {
                path.push(Step::Key(k.clone()));
                if let Some(dv) = d.and_then(|d| d.get(k))
                    && !(at.is_empty() && k == "version")
                {
                    f(path, dv);
                }
                let len = at.len();
                if len > 0 {
                    at.push('.');
                }
                at.push_str(k);
                each_removable(x, d.and_then(|d| d.get(k)), at, path, f);
                at.truncate(len);
                path.pop();
            }
        }
        _ => {}
    }
}

/// Edits that hold, between them, every payload a state can carry that
/// the defaults do not show: a crop and each aspect, each white
/// balance, each local white (one at the daylight a missing
/// temperature reads as), each shape (a sky with no picks, a part with
/// no person), a clone and a fill. Built from numbers, never from the
/// constants a reader fills with, so a changed constant shows.
///
/// A default a reader fills that no struct's default shows (a
/// variant's field, a wire's missing value: anything [`read_at`]
/// leaves out) is pinned only by an edit here at that default: a new
/// one needs its golden edit, added before the cut.
pub(crate) fn golden_edits() -> Vec<(&'static str, Edit)> {
    use crate::geometry::{Aspect, Crop};
    use crate::mask::{Component, LandscapeGroup, Mask, Route, Shape};
    use crate::white::LocalWhite;
    let mut out = vec![
        ("default", Edit::default()),
        ("picture", Edit::for_picture()),
        ("everything", crate::sidecar::placed_tests::everything()),
    ];
    let mut e = exposed(0.25);
    e.white_balance = crate::WhiteBalance::Custom {
        temperature: 4300.0,
        tint: 0.004,
    };
    e.geometry.crop = Some(Crop::of([0.0, 0.0, 1.0, 1.0]));
    e.geometry.aspect = Aspect::Original;
    out.push(("custom white, original aspect", e));
    let mut e = exposed(0.5);
    e.white_balance = crate::WhiteBalance::Offset {
        mired: 0.0,
        tint: 0.002,
    };
    e.geometry.crop = Some(Crop::of([0.25, 0.0, 0.5, 1.0]));
    e.geometry.aspect = Aspect::Free;
    out.push(("offset white, free aspect", e));
    let local = |white: LocalWhite, shape: Shape| Adjustment {
        id: 1,
        name: "One".into(),
        mask: Mask {
            components: vec![Component {
                shape,
                ..Component::default()
            }],
            ..Mask::default()
        },
        look: crate::Look {
            white_balance: white,
            ..crate::Look::default()
        },
        ..Adjustment::default()
    };
    let mut e = exposed(0.75);
    e.adjustments = vec![
        local(
            LocalWhite::Absolute {
                temperature: 5500.0,
                tint: 0.0,
            },
            Shape::Sky { picks: vec![] },
        ),
        local(
            LocalWhite::Relative {
                mired: -12.0,
                tint: 0.0,
            },
            Shape::Part {
                phrase: "hair".into(),
                route: Route::Eye,
                person: None,
            },
        ),
        local(
            LocalWhite::Kept {
                temperature: 3200.0,
                tint: 0.001,
            },
            Shape::Landscape {
                group: LandscapeGroup::Water,
            },
        ),
        local(
            LocalWhite::KeptRelative {
                mired: 5.0,
                tint: 0.0,
            },
            Shape::Background {},
        ),
        local(
            LocalWhite::Off,
            Shape::Color {
                hue: 200.0,
                width: 40.0,
                hue_feather: 20.0,
                chroma: 0.05,
                chroma_feather: 0.02,
            },
        ),
    ];
    e.retouch.patches.push(retouch::Patch {
        id: 2,
        method: retouch::Method::Clone,
        points: vec![[0.5, 0.5]],
        radius: 0.05,
        source: None,
        ..retouch::Patch::default()
    });
    out.push(("local whites, the other shapes, a clone", e));
    out
}

/// Each of [`golden_edits`]' written text and id, as schema 10 makes
/// them.
const GOLDEN_IDS: &str = include_str!("../../fixtures/ids-at-10.json");

fn ids_now() -> String {
    let rows: Vec<Value> = golden_edits()
        .into_iter()
        .map(|(name, edit)| {
            serde_json::json!({
                "name": name,
                "text": written_text(&edit),
                "id": state_id(ZERO_ID, &edit, None),
            })
        })
        .collect();
    serde_json::to_string_pretty(&rows).unwrap() + "\n"
}

/// An id made at schema 10 is made the same by every later build: the
/// same state recorded on two builds is one state. Each golden edit's
/// text and id are as the fixture has them, and its text reads back as
/// the edit (so a default a reader fills, a variant's too, holds).
#[test]
fn ids_made_at_schema_10_are_made_alike_by_every_build() {
    let golden: Vec<Value> = serde_json::from_str(GOLDEN_IDS).unwrap();
    let edits = golden_edits();
    assert_eq!(
        golden.len(),
        edits.len(),
        "the fixture's edits:\n{}",
        ids_now()
    );
    for (row, (name, edit)) in golden.iter().zip(&edits) {
        assert_eq!(row["name"], *name);
        let text = row["text"].as_str().unwrap();
        assert!(
            written_text(edit) == text && state_id(ZERO_ID, edit, None) == row["id"],
            "{name}: a state's id or written text has changed. Under schema 10 an \
             id is made from the written text (§724), so two builds would record one \
             state as two; a field added must be left out at its default, and a \
             change to a default needs a schema bump. Now:\n{}",
            ids_now()
        );
        let back: Edit = serde_json::from_str(text).unwrap();
        assert_eq!(back, *edit, "{name}: reads back as written");
    }
}

/// Writes the golden ids again. Only before schema 10's cut.
#[test]
#[ignore]
fn write_the_golden_ids() {
    if !crate::testing::may_write_golden() {
        return;
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/ids-at-10.json");
    std::fs::write(path, ids_now()).unwrap();
}

/// Every path of `v`, as `strip` names them.
fn paths(v: &Value, at: &mut String, out: &mut std::collections::BTreeSet<String>) {
    out.insert(at.clone());
    match v {
        Value::Array(items) => {
            let len = at.len();
            at.push_str("[]");
            for x in items {
                paths(x, at, out);
            }
            at.truncate(len);
        }
        Value::Object(o) => {
            for (k, x) in o {
                let len = at.len();
                if len > 0 {
                    at.push('.');
                }
                at.push_str(k);
                paths(x, at, out);
                at.truncate(len);
            }
        }
        _ => {}
    }
}

/// Every string a state holds, each in turn given a value no build
/// writes, read as a file a newer build made reads it: the edit reads,
/// so every enum a later build may extend has its fallback (a
/// `#[serde(other)]`, a conversion from any string, or
/// `meta::newer_loose` on its field), and a newer file opens read-only
/// rather than not at all (§724).
#[test]
fn every_enum_a_later_build_may_extend_reads_in_a_newer_file() {
    fn strings(v: &Value, path: &mut Vec<Step>, out: &mut Vec<Vec<Step>>) {
        match v {
            Value::String(_) => out.push(path.clone()),
            Value::Array(items) => {
                for (i, x) in items.iter().enumerate() {
                    path.push(Step::At(i));
                    strings(x, path, out);
                    path.pop();
                }
            }
            Value::Object(o) => {
                for (k, x) in o {
                    path.push(Step::Key(k.clone()));
                    strings(x, path, out);
                    path.pop();
                }
            }
            _ => {}
        }
    }
    let mut checked = 0;
    for (name, edit) in golden_edits() {
        let full = full_value(&edit);
        let mut at = Vec::new();
        strings(&full, &mut Vec::new(), &mut at);
        for path in at {
            let mut v = full.clone();
            set_at(&mut v, &path, Value::String("a_value_from_later".into()));
            let read = crate::meta::reading_newer(true, || serde_json::from_value::<Edit>(v));
            assert!(read.is_ok(), "{name}: {path:?}: {}", read.unwrap_err());
            checked += 1;
        }
    }
    assert!(checked > 40, "{checked}");
    // The enums a state leaves out at their default, so no string of
    // the golden edits is theirs: each set to a value no build writes,
    // and the tag of an enum written as a one-key object renamed.
    let full = full_value(&golden_edits()[2].1);
    let unknown = || Value::String("a_value_from_later".into());
    let cases: [(&str, Value); 4] = [
        ("/lens/chosen", unknown()),
        (
            "/lens/chosen",
            serde_json::json!({"a_value_from_later": "x"}),
        ),
        ("/display_curve", unknown()),
        ("/vignette/place", unknown()),
    ];
    for (at, value) in cases {
        let mut v = full.clone();
        let (parent, key) = at.rsplit_once('/').unwrap();
        v.pointer_mut(parent)
            .and_then(Value::as_object_mut)
            .unwrap_or_else(|| panic!("{at}"))
            .insert(key.into(), value);
        let read = crate::meta::reading_newer(true, || serde_json::from_value::<Edit>(v));
        assert!(read.is_ok(), "{at}: {}", read.unwrap_err());
    }
}

/// Each rule's path is one a state has: a rule misnamed would leave its
/// values to be stripped as plain structs.
#[test]
fn every_rule_names_a_path_a_state_has() {
    let mut all = std::collections::BTreeSet::new();
    for (_, edit) in golden_edits() {
        paths(&full_value(&edit), &mut String::new(), &mut all);
    }
    for path in RULED {
        assert!(all.contains(path), "no state has {path}");
    }
}

/// A field at its default inside a tagged enum or an optional struct is
/// left out as one in a struct is: a sky with no picks, a local white at
/// the daylight a missing temperature reads as, a part with no person.
#[test]
fn a_variant_s_defaults_are_left_out_too() {
    let (_, e) = golden_edits().pop().unwrap();
    let v = written_value(&e);
    let adj = |i: usize| &v["adjustments"][i];
    assert_eq!(
        adj(0)["mask"]["components"][0]["shape"],
        serde_json::json!({"kind": "sky"})
    );
    assert_eq!(
        adj(0)["look"]["white_balance"],
        serde_json::json!({"mode": "absolute"})
    );
    assert_eq!(
        adj(1)["look"]["white_balance"],
        serde_json::json!({"mode": "relative", "mired": -12.0})
    );
    assert!(
        adj(1)["mask"]["components"][0]["shape"]
            .get("person")
            .is_none()
    );
}

/// A number reads as written: an f32's shortest text reads into the
/// value an id is written from as the f64 nearest it (what `std`'s
/// parse gives), and that value prints back to text that reads the
/// same. What `serde_json`'s `float_roundtrip` gives; a build without it
/// could read a digit off and change ids.
#[test]
fn numbers_print_as_written() {
    let mut x: u32 = 0x9e37_79b9;
    for _ in 0..100_000 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let f = f32::from_bits(x);
        if !f.is_finite() {
            continue;
        }
        let text = serde_json::to_string(&f).unwrap();
        let v = full_value(&f);
        assert_eq!(v.as_f64(), Some(text.parse::<f64>().unwrap()), "{f:e}");
        let mut out = String::new();
        canonical(&v, &mut out);
        assert_eq!(serde_json::from_str::<Value>(&out).unwrap(), v, "{f:e}");
    }
}

/// A state's written value is made once and kept; one changed in place
/// since is made again.
#[test]
fn a_state_written_again_is_made_again_only_when_it_changed() {
    let mut s = rich();
    let first = s.to_json();
    assert_eq!(s.to_json(), first, "kept");
    s.line.history[1].edit.light.exposure = 1.5;
    let again = s.to_json();
    assert_ne!(again, first);
    assert_eq!(read(&again).history[1].edit.light.exposure, 1.5);
}

/// The types a state is made of whose values `strip` must know the
/// shape of: each list or option of a struct, and each tagged or
/// converted enum, in the files the edit is made from. Each is ruled in
/// `sparse.rs` by its path ([`Defaults::element`], [`read_at`]) or is
/// not an object written in a state.
const SHAPES: &str = "\
brush.rs: done: Vec<Stroke>,
brush.rs: pub points: Vec<Pos>,
camera.rs: #[serde(default, from = \"Read\")] pub struct Camera
camera.rs: enum Row: struct variant Heading
camera.rs: enum Row: struct variant Profile
curve.rs: curve: Option<Channel>,
curve.rs: pub blue: Vec<Point>,
curve.rs: pub blue_yellow: Vec<Point>,
curve.rs: pub green: Vec<Point>,
curve.rs: pub red: Vec<Point>,
curve.rs: pub red_green: Vec<Point>,
curve.rs: pub rgb: Vec<Point>,
edit.rs: pub adjustments: Vec<Adjustment>,
geometry.rs: #[serde(tag = \"kind\", rename_all = \"snake_case\")] pub enum Aspect
geometry.rs: enum Aspect: struct variant Ratio
geometry.rs: pub axis: Option<Guide>,
geometry.rs: pub crop: Option<Crop>,
geometry.rs: pub first: Option<GuideStroke>,
look.rs: enum Row: struct variant Heading
look.rs: enum Row: struct variant Look
look.rs: pub variant: Option<DisplayCurve>,
mask.rs: #[serde(from = \"String\", into = \"String\")] pub enum LandscapeGroup
mask.rs: #[serde(from = \"String\", into = \"String\")] pub enum Route
mask.rs: #[serde(tag = \"kind\", rename_all = \"snake_case\")] pub enum Shape
mask.rs: Brush { strokes: Vec<Stroke> },
mask.rs: enum Shape: struct variant Background
mask.rs: enum Shape: struct variant Brush
mask.rs: enum Shape: struct variant Color
mask.rs: enum Shape: struct variant Landscape
mask.rs: enum Shape: struct variant Linear
mask.rs: enum Shape: struct variant Luminance
mask.rs: enum Shape: struct variant Object
mask.rs: enum Shape: struct variant Part
mask.rs: enum Shape: struct variant Radial
mask.rs: enum Shape: struct variant Sky
mask.rs: enum Shape: struct variant Subject
mask.rs: person: Option<Person>,
mask.rs: picks: Vec<Pick>,
mask.rs: pub components: Vec<Component>,
mask.rs: sample: Option<Sample>,
retouch.rs: pub more: Vec<Stroke>,
retouch.rs: pub patches: Vec<Patch>,
retouch.rs: pub points: Vec<Pos>,
retouch.rs: pub source: Option<Pos>,
sections.rs: #[serde(try_from = \"WhiteWire\", into = \"WhiteWire\")] pub enum WhiteBalance
sections.rs: enum WhiteBalance: struct variant Custom
sections.rs: enum WhiteBalance: struct variant Offset
sections.rs: offset: Option<WhiteOffset>,
white.rs: #[serde(from = \"Wire\", into = \"Wire\")] pub enum LocalWhite
white.rs: enum Base: struct variant Camera
white.rs: enum LocalWhite: struct variant Absolute
white.rs: enum LocalWhite: struct variant Kept
white.rs: enum LocalWhite: struct variant KeptRelative
white.rs: enum LocalWhite: struct variant Relative
white.rs: profile: Box<CameraProfile>,
";

/// The files of the crate a state is not made from.
const NOT_STATE: [&str; 19] = [
    "auto.rs",
    "baseline.rs",
    "frames.rs",
    "history.rs",
    "kept.rs",
    "key.rs",
    "layout.rs",
    "lib.rs",
    "line.rs",
    "loader.rs",
    "meta.rs",
    "migration.rs",
    "preset.rs",
    "sidecar.rs",
    "sparse.rs",
    "sync.rs",
    "testing.rs",
    "versions.rs",
    "xmp.rs",
];

/// The files under `dir`, by their path from `src`, sorted: a state's
/// (not [`NOT_STATE`], nor the sidecar's or the sparse form's own
/// folders) and no test's.
fn state_files(src: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let path = e.path();
        let rel = path
            .strip_prefix(src)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            if !["sidecar", "sparse"].contains(&rel.as_str()) {
                state_files(src, &path, out);
            }
        } else if rel.ends_with(".rs")
            && !rel.ends_with("tests.rs")
            && !NOT_STATE.contains(&rel.as_str())
        {
            out.push(rel);
        }
    }
    out.sort();
}

/// [`SHAPES`] as the source has them now: every line of a file a state
/// is made from (its tests left out) that declares a list, an option or
/// a box of a type (`std::` spelled out too, or its type on the next
/// line), a type alias of one, a field of another crate's type, a
/// struct variant of an enum, or a tag, a conversion or `untagged` on
/// an enum or a struct.
fn shapes_now() -> String {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut names = Vec::new();
    state_files(&src, &src, &mut names);
    let wrappers = [
        "Vec<",
        "Option<",
        "Box<",
        "BTreeMap<",
        "HashMap<",
        "std::collections::BTreeMap<",
        "std::collections::HashMap<",
        "std::vec::Vec<",
        "std::option::Option<",
        "std::boxed::Box<",
    ];
    let of_a_type = |rest: &str| {
        (rest.starts_with(|c: char| c.is_ascii_uppercase()) && !rest.starts_with("String"))
            || rest.starts_with("crate::")
            || rest.starts_with("greycard_")
            || rest.starts_with("std::")
            || rest.is_empty()
    };
    let mut out = Vec::new();
    for name in names {
        let text = std::fs::read_to_string(src.join(&name)).unwrap();
        let text = text.split("\n#[cfg(test)]\nmod ").next().unwrap();
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        let mut in_enum: Option<String> = None;
        for (i, t) in lines.iter().enumerate() {
            if t.starts_with("//") {
                continue;
            }
            // A type on the next line, as rustfmt breaks a long one.
            let joined;
            let t: &str = if t.ends_with('<') {
                joined = format!("{t}{}", lines.get(i + 1).unwrap_or(&""));
                &joined
            } else {
                t
            };
            let attr = t.starts_with("#[serde(")
                && [
                    "tag = \"",
                    " from = \"",
                    "(from = \"",
                    "try_from = \"",
                    "untagged",
                ]
                .iter()
                .any(|m| t.contains(m));
            if attr {
                let target = lines[i + 1..]
                    .iter()
                    .find(|l| l.contains("enum ") || l.contains("struct "))
                    .map_or("?", |l| l.split('{').next().unwrap().trim());
                out.push(format!("{name}: {t} {target}"));
                continue;
            }
            if let Some(e) = t.split("enum ").nth(1).filter(|_| t.ends_with('{')) {
                in_enum = Some(e.trim_end_matches('{').trim().to_string());
                continue;
            }
            if t == "}" {
                in_enum = None;
            }
            if let Some(e) = &in_enum
                && let Some(v) = t
                    .strip_suffix('{')
                    .or_else(|| t.split(" {").next().filter(|_| t.contains(" {")))
                && v.trim()
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                && v.trim().starts_with(|c: char| c.is_ascii_uppercase())
            {
                out.push(format!("{name}: enum {e}: struct variant {}", v.trim()));
            }
            if format!(" {t}").contains(" fn ") || t.starts_with("let ") || t.contains("=>") {
                continue;
            }
            let alias = t.starts_with("type ") || t.contains(" type ");
            let field = wrappers.iter().any(|w| {
                let sep = if alias { " = " } else { ": " };
                t.split(&format!("{sep}{w}")).nth(1).is_some_and(of_a_type)
            }) || (!alias && t.contains(": greycard_"));
            if field {
                out.push(format!("{name}: {t}"));
            }
        }
    }
    out.sort();
    out.dedup();
    out.join("\n") + "\n"
}

/// A new list or option of a struct, or a new tagged enum, in a state
/// is given its rule. Left to the plain rule, a struct with no default
/// would be written whole (so a field added to it later changes ids),
/// and a list element stripped against a default of another type would
/// be lost.
#[test]
fn every_shape_a_state_has_is_ruled() {
    assert!(
        shapes_now() == SHAPES,
        "a list or an option of a type, or a tagged or converted enum, was added \
         to or taken from the files a state is made of. Give it its rule in \
         sparse.rs by path (`Defaults::element` for a list of structs with a \
         default, `read_at` for a tagged enum or a struct with none), or say here \
         that it is not an object a state writes; then set SHAPES to:\n{}",
        shapes_now()
    );
}

fn parent_of<'a>(v: &'a mut Value, path: &[Step]) -> &'a mut Value {
    let mut at = v;
    for s in &path[..path.len() - 1] {
        at = match s {
            Step::Key(k) => &mut at[k.as_str()],
            Step::At(i) => &mut at[*i],
        };
    }
    at
}

fn remove_at(v: &mut Value, path: &[Step]) {
    if let Some(Step::Key(k)) = path.last() {
        parent_of(v, path).as_object_mut().unwrap().remove(k);
    }
}

fn set_at(v: &mut Value, path: &[Step], to: Value) {
    if let Some(Step::Key(k)) = path.last() {
        parent_of(v, path)[k.as_str()] = to;
    }
}

/// Every state's id is made from its sparse text as the file holds it,
/// its parent, and its words; and the same history written whole gives
/// the same ids, since an id is made from the edit in memory.
#[test]
fn an_id_is_its_state_s_sparse_text_whatever_writes_the_file() {
    let s = rich();
    let json = s.to_json();
    let mut n = 0;
    for (edit, id, parent, label) in entries(&json) {
        let text = serde_json::to_string(&edit).unwrap();
        let made = crate::sync::id_of(
            parent.as_deref().unwrap_or(ZERO_ID),
            &text,
            label.as_deref(),
        );
        assert_eq!(id.as_deref(), Some(made.as_str()));
        n += 1;
    }
    assert!(n > 5);
    let ids = |s: &Sidecar| {
        s.lines()
            .flat_map(|l| {
                l.history
                    .iter()
                    .map(|h| h.id.clone())
                    .chain([l.current_id.clone()])
            })
            .collect::<Vec<_>>()
    };
    let from_whole = read(&whole(|| s.to_json()));
    assert_eq!(ids(&from_whole), ids(&s));
    // Two machines record the same step from the same parent alike.
    let (mut a, mut b) = (Sidecar::default(), Sidecar::default());
    a.record(exposed(0.3));
    b.record(exposed(0.3));
    assert_eq!(a.current_id, b.current_id);
    assert_eq!(
        a.current_id.as_deref(),
        Some(state_id(a.history[0].id.as_deref().unwrap(), &exposed(0.3), None).as_str())
    );
}

/// Two copies read from files written sparse and whole join alike.
#[test]
fn copies_written_sparse_or_whole_join_alike() {
    let a = rich();
    let mut b = read(&a.to_json());
    b.record(exposed(2.0));
    let mut c = read(&a.to_json());
    c.record(exposed(3.0));
    let sparse = join(&read(&b.to_json()), &read(&c.to_json()));
    let whole_join = join(&read(&whole(|| b.to_json())), &read(&whole(|| c.to_json())));
    assert_eq!(sparse, whole_join);
}

/// A history written as differences, as a later build may: each
/// entry made whole against its parent, or the base it names.
fn with_history(history: Value) -> String {
    serde_json::json!({
        "history": history,
        "current": {"version": crate::VERSION, "light": {"exposure": 9.0}},
        "id": "c",
        "parent": "h2",
    })
    .to_string()
}

fn exposures(s: &Sidecar) -> Vec<f32> {
    s.history.iter().map(|h| h.edit.light.exposure).collect()
}

/// A chain of differences, one against an explicit base, a list grown
/// at its end, an element changed, and a key taken out, read whole.
#[test]
fn a_history_of_differences_reads_whole() {
    let v = crate::VERSION;
    let s = read(&with_history(serde_json::json!([
        {"version": v, "light": {"exposure": 1.0, "tone": {"contrast": 5.0}}, "id": "h0"},
        {"delta": {"light": {"exposure": 2.0}}, "id": "h1", "parent": "h0"},
        {"delta": {"light": {"$del": ["tone"]},
                   "retouch": {"patches": [{"id": 1, "points": [[0.1, 0.1]], "radius": 0.02}]}},
         "id": "h2", "parent": "h1", "step": "Words"},
        {"delta": {"retouch": {"patches": {"$+": [{"id": 2, "points": [[0.2, 0.2]], "radius": 0.03}]}}},
         "id": "h3", "parent": "h2"},
        {"delta": {"retouch": {"patches": {"$at": {"0": {"radius": 0.05}}}}},
         "id": "h4", "parent": "h3"},
        {"delta": {"light": {"exposure": 3.0}}, "id": "h5", "parent": "h4", "base": "h0"},
    ])));
    assert_eq!(s.unread(), 0);
    assert_eq!(exposures(&s), [1.0, 2.0, 2.0, 2.0, 2.0, 3.0]);
    assert_eq!(
        s.history[1].edit.light.tone.contrast, 5.0,
        "from its parent"
    );
    assert_eq!(
        s.history[2].edit.light.tone,
        Edit::default().light.tone,
        "taken out"
    );
    assert_eq!(s.history[2].label.as_deref(), Some("Words"));
    assert_eq!(s.history[3].edit.retouch.patches.len(), 2);
    assert_eq!(s.history[4].edit.retouch.patches[0].radius, 0.05);
    assert_eq!(
        s.history[5].edit.light.tone.contrast, 5.0,
        "against its base"
    );
    assert!(s.history[5].edit.retouch.patches.is_empty());
    assert_eq!(s.history[5].parent.as_deref(), Some("h4"));
}

/// A difference whose base is missing, a later entry (a cycle), or one
/// this reader does not wholly understand is that state lost, with the
/// states made from it, and nothing else: never read as a default
/// edit. The sidecar is then not written, nor is a join with it.
#[test]
fn a_difference_that_cannot_be_read_loses_that_state_alone() {
    let v = crate::VERSION;
    let s = read(&with_history(serde_json::json!([
        {"version": v, "light": {"exposure": 1.0}, "id": "h0"},
        {"delta": {"light": {"exposure": 2.0}}, "id": "h1", "parent": "gone"},
        {"delta": {"light": {"exposure": 3.0}}, "id": "h2", "parent": "h1"},
        {"delta": {"light": {"exposure": 4.0}}, "id": "h3", "parent": "h4"},
        {"delta": {"light": {"exposure": 5.0}}, "id": "h4", "parent": "h3"},
        {"delta": {"light": {"$wat": 1}}, "id": "h5", "parent": "h0"},
        {"delta": {"light": {"exposure": 6.0}}, "id": "h6", "parent": "h0", "extra": 1},
        {"delta": [1], "id": "h7", "parent": "h0"},
        {"delta": {"retouch": {"patches": {"$at": {"3": {"radius": 0.1}}}}}, "id": "h8", "parent": "h0"},
        {"delta": {"light": {"exposure": 7.0}}, "id": "h9", "parent": "h0"},
    ])));
    assert_eq!(
        exposures(&s),
        [1.0, 7.0],
        "the rest read, none as a default"
    );
    assert_eq!(s.unread(), 8);
    let dir = crate::testing::scratch("sparse-unread");
    let raw = dir.join("frame.dng");
    std::fs::write(&raw, b"").unwrap();
    let mut written = s.clone();
    assert!(matches!(
        written.save_in(&raw, crate::Placement::Beside),
        Err(crate::Error::Unread(8))
    ));
    assert!(!Sidecar::path_for(&raw).exists(), "nothing written");
    let joined = join(&s, &read(&with_history(serde_json::json!([]))));
    assert!(joined.unread() > 0, "a join keeps the loss in view");
    greycard_core::testing::remove_dir_retry(&dir);
}

/// `n` states, each a difference against the one before: their
/// history as written.
fn chained(n: usize) -> Vec<Value> {
    let mut history = vec![serde_json::json!({"id": "0", "light": {"exposure": 0.0}})];
    for i in 1..n {
        history.push(serde_json::json!({
            "delta": {"light": {"exposure": i as f64}},
            "id": i.to_string(),
            "parent": (i - 1).to_string(),
        }));
    }
    history
}

/// A history of 100,000 chained differences reads in time linear in
/// its length: each base is found by its id, not searched for (a
/// search took some 29 s for this many, in release).
#[test]
fn a_long_chain_of_differences_reads_in_linear_time() {
    let history = chained(100_000);
    let mut budget = BUDGET;
    let t = std::time::Instant::now();
    let (made, lost) = whole_history(history, &mut budget);
    let took = t.elapsed();
    assert_eq!((made.len(), lost), (100_000, 0));
    assert_eq!(made[99_999]["light"]["exposure"], 99_999.0);
    assert!(took.as_secs() < 20, "{took:?}");
}

/// What differences make whole is held to the budget: past it, a
/// difference is lost as one this reader cannot read is, and the read
/// ends having made no more.
#[test]
fn differences_make_no_more_whole_than_the_budget() {
    let history = chained(1_000);
    let mut budget = 100 * size_of(&history[0]);
    let (made, lost) = whole_history(history, &mut budget);
    // Each costs its base's copy and its own merge: some forty fit.
    assert!((20..100).contains(&made.len()), "{}", made.len());
    assert_eq!(made.len() + lost, 1_000);
    assert_eq!(budget, 0);
}

/// A large state and many small differences against it, or a long
/// chain on it: each difference is weighed, its base and its own size,
/// before its base is copied, and once the budget is spent every later
/// one is lost unread. Both files read in bounded time (each took some
/// 18 s before).
#[test]
fn a_large_base_under_many_differences_reads_in_bounded_time() {
    for (points, n, chained) in [(20_000usize, 20_000usize, false), (200_000, 2_000, true)] {
        let pts: Vec<Value> = (0..points)
            .map(|i| serde_json::json!([i as f64 / points as f64, 0.5]))
            .collect();
        let mut h = vec![serde_json::json!({
            "version": crate::VERSION,
            "id": "h0",
            "retouch": {"patches": [{"id": 1, "method": "heal", "points": pts, "radius": 0.01}]},
        })];
        for i in 1..n {
            let base = if chained {
                format!("h{}", i - 1)
            } else {
                "h0".into()
            };
            h.push(serde_json::json!({
                "delta": {"light": {"exposure": i as f64 / 100.0}},
                "parent": base,
                "id": format!("h{i}"),
            }));
        }
        let json =
            serde_json::json!({"history": h, "current": {"version": crate::VERSION}}).to_string();
        let t = std::time::Instant::now();
        let s = Sidecar::from_json(&json, &Edit::default()).unwrap();
        let took = t.elapsed();
        assert!(s.unread() > 0, "past the budget, lost");
        assert!(s.read_only());
        assert!(
            took.as_secs() < 15,
            "{points} points, {n} differences: {took:?}"
        );
    }
}

/// Every struct of a state left empty (`{}`) reads as its default
/// there: each nested type's own default is its value in its
/// container's, so leaving out every key at once is as safe as one.
/// A value ruled by its reading ([`read_at`]) is left whole: it has no
/// default to strip against.
#[test]
fn each_struct_left_empty_reads_as_its_default_there() {
    fn paths(v: &Value, at: &mut String, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        let Value::Object(o) = v else {
            return;
        };
        if read_at(at).is_some() {
            return;
        }
        out.push(path.clone());
        for (k, x) in o {
            let len = at.len();
            if len > 0 {
                at.push('.');
            }
            at.push_str(k);
            path.push(k.clone());
            paths(x, at, path, out);
            path.pop();
            at.truncate(len);
        }
    }
    fn check<T: serde::de::DeserializeOwned + Serialize>(
        name: &str,
        prefix: &str,
        d: &Value,
        bad: &mut Vec<String>,
    ) {
        let mut ps = Vec::new();
        paths(d, &mut prefix.to_string(), &mut Vec::new(), &mut ps);
        for p in ps {
            let mut v = d.clone();
            let mut at = &mut v;
            for k in &p {
                at = at.get_mut(k).unwrap();
            }
            *at = Value::Object(Map::new());
            if p.is_empty()
                && let Some(version) = d.get("version")
            {
                v["version"] = version.clone();
            }
            match serde_json::from_value::<T>(v) {
                Ok(t) if full_value(&t) == *d => {}
                Ok(_) => bad.push(format!("{name} {p:?}")),
                Err(e) => bad.push(format!("{name} {p:?}: {e}")),
            }
        }
    }
    let d = &*DEFAULTS;
    let mut bad = Vec::new();
    check::<Edit>("edit", "", &d.edit, &mut bad);
    check::<Adjustment>("adjustment", "adjustments[]", &d.adjustment, &mut bad);
    check::<mask::Component>(
        "component",
        "adjustments[].mask.components[]",
        &d.component,
        &mut bad,
    );
    check::<brush::Stroke>(
        "stroke",
        "adjustments[].mask.components[].shape.strokes[]",
        &d.stroke,
        &mut bad,
    );
    check::<retouch::Patch>("patch", "retouch.patches[]", &d.patch, &mut bad);
    check::<retouch::Stroke>(
        "patch stroke",
        "retouch.patches[].more[]",
        &d.patch_stroke,
        &mut bad,
    );
    assert!(bad.is_empty(), "{bad:#?}");
}

/// A nested object inside a value ruled by its reading leaves out what
/// it reads the same without: an offset's zero mired.
#[test]
fn a_nested_default_inside_a_read_value_is_left_out() {
    let mut e = exposed(0.5);
    e.white_balance = crate::WhiteBalance::Offset {
        mired: 0.0,
        tint: 0.002,
    };
    let v = written_value(&e);
    assert_eq!(
        v["white_balance"],
        serde_json::json!({"mode": "as_shot", "offset": {"tint": 0.002}})
    );
    let back: Edit = serde_json::from_value(v).unwrap();
    assert_eq!(back, e);
}

/// The current state and a snapshot are written whole: one written as
/// a difference refuses the file.
#[test]
fn a_current_or_a_snapshot_written_as_a_difference_refuses_the_file() {
    let current = serde_json::json!({"current": {"delta": {}}, "id": "c"}).to_string();
    assert!(Sidecar::from_json(&current, &Edit::default()).is_err());
    let snapshot = serde_json::json!({
        "current": {"version": crate::VERSION},
        "id": "c",
        "snapshots": [{"name": "s", "taken": 1, "edit": {"delta": {}}}],
    })
    .to_string();
    assert!(Sidecar::from_json(&snapshot, &Edit::default()).is_err());
}

/// The defaults a state is written against, as schema 10 has them.
const GOLDEN_DEFAULTS: &str = include_str!("../../fixtures/defaults-at-10.json");

/// Every default a state is written against, the edit's and each list
/// element's, as one value.
fn defaults_now() -> String {
    let d = &*DEFAULTS;
    // The process is always written and moves with the build: no
    // default a state is written against.
    let mut edit = d.edit.clone();
    edit.as_object_mut().unwrap().remove("process");
    let v = serde_json::json!({
        "edit": edit,
        "adjustment": d.adjustment,
        "component": d.component,
        "stroke": d.stroke,
        "patch": d.patch,
        "patch_stroke": d.patch_stroke,
    });
    serde_json::to_string_pretty(&v).unwrap() + "\n"
}

/// No default changes within a schema version: a state leaves out what
/// is at its default, so a changed default changes every picture an
/// earlier build wrote, and what an earlier build shows of this one's.
#[test]
fn no_default_changes_within_schema_10() {
    assert!(
        defaults_now() == GOLDEN_DEFAULTS,
        "a default a state is written against has changed. Under schema 10 a \
         field left out reads as its default (§724), so a change needs a schema \
         bump and a migration that writes the old value where it was left out \
         (or a new field in place of the old). The defaults now:\n{}",
        defaults_now()
    );
}

/// Writes the golden defaults again. Only before schema 10's cut.
#[test]
#[ignore]
fn write_the_golden_defaults() {
    if !crate::testing::may_write_golden() {
        return;
    }
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/defaults-at-10.json");
    std::fs::write(path, defaults_now()).unwrap();
}

/// The sparse form's size and time against the whole one, on a
/// 200-state sidecar, and on measure_724's files when `MEASURE_724`
/// names their folder: printed for step2.md, by hand.
#[test]
#[ignore]
fn sparse_against_whole_by_hand() {
    if let Ok(dir) = std::env::var("MEASURE_724") {
        measure_files(std::path::Path::new(&dir));
    }
    let mut s = Sidecar::default();
    let base = crate::sidecar::placed_tests::everything();
    for i in 0..199 {
        let mut e = base.clone();
        e.light.exposure = i as f32 / 100.0;
        s.record(e);
    }
    s.fill_ids();
    let time = |f: &dyn Fn() -> String| {
        let mut ms: Vec<f64> = (0..9)
            .map(|_| {
                let t = std::time::Instant::now();
                std::hint::black_box(f());
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        ms.sort_by(f64::total_cmp);
        ms[4]
    };
    let sparse = s.to_json();
    let whole_text = whole(|| s.to_json());
    let write_sparse = time(&|| s.to_json());
    let write_whole = time(&|| whole(|| s.to_json()));
    let read_sparse = time(&|| read(&sparse).saved.to_string());
    let read_whole = time(&|| read(&whole_text).saved.to_string());
    eprintln!(
        "200 states: whole {} B, sparse {} B; write {write_whole:.1} / {write_sparse:.1} ms; read {read_whole:.1} / {read_sparse:.1} ms",
        whole_text.len(),
        sparse.len()
    );
}

/// The median of nine runs of `f`, in ms.
fn median_ms(f: &dyn Fn()) -> f64 {
    let mut ms: Vec<f64> = (0..9)
        .map(|_| {
            let t = std::time::Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    ms.sort_by(f64::total_cmp);
    ms[4]
}

/// measure_724's turned 200-state file and its 2,000-sidecar folder,
/// each read, then written whole and sparse: sizes, and read and write
/// times of both.
fn measure_files(dir: &std::path::Path) {
    let one = std::fs::read_to_string(dir.join("turned200.gcd")).unwrap();
    let s = read(&one);
    let sparse = s.to_json();
    let whole_text = whole(|| s.to_json());
    eprintln!(
        "turned 200: whole {} B, sparse {} B; read {:.1} / {:.1} ms; write {:.1} / {:.1} ms",
        whole_text.len(),
        sparse.len(),
        median_ms(&|| drop(read(&whole_text))),
        median_ms(&|| drop(read(&sparse))),
        median_ms(&|| drop(whole(|| s.to_json()))),
        median_ms(&|| drop(s.to_json())),
    );
    let mut wholes = Vec::new();
    let mut sparses = Vec::new();
    for entry in std::fs::read_dir(dir.join("folder2000")).unwrap() {
        let path = entry.unwrap().path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let text = if name.ends_with(".gcd") {
            std::fs::read_to_string(&path).unwrap()
        } else if path.is_dir() {
            match std::fs::read_dir(&path).unwrap().find_map(|e| {
                let p = e.ok()?.path();
                p.extension().is_some_and(|x| x == "gcd").then_some(p)
            }) {
                Some(p) => std::fs::read_to_string(p).unwrap(),
                None => continue,
            }
        } else {
            continue;
        };
        let s = read(&text);
        wholes.push(whole(|| s.to_json()));
        sparses.push(s.to_json());
    }
    let size = |v: &[String]| v.iter().map(String::len).sum::<usize>();
    let read_all = |v: &[String]| {
        for t in v {
            drop(read(t));
        }
    };
    eprintln!(
        "folder of {}: whole {} B, sparse {} B; read {:.0} / {:.0} ms",
        wholes.len(),
        size(&wholes),
        size(&sparses),
        median_ms(&|| read_all(&wholes)),
        median_ms(&|| read_all(&sparses)),
    );
}
