//! What a learned mask is made from, as the mask cache keys it.
//!
//! A model sees the base: the develop up to the masks' tap (the
//! prepare, the CA, the demosaic or the bin, the denoise, the warp,
//! the working space, the learned blend, the lens, and the defringe),
//! finished under the edit's global look at the models' own 2048. A
//! raster is kept, in memory and on disk, by a print of that base
//! ([`print`]): the file (its path, length, and modification time), the
//! parts of the edit those stages read ([`base_edit`]) with the files
//! they read by name, the frame's quarter turns, and the size the base
//! is made at. The viewport, the window's export, and `greycard export`
//! work the print out alike from the edit, so they read one raster or
//! make the same one. A base the print does not match (a proxy's for
//! the full develop, the full develop's for a proxy, one from before a
//! change to the white balance or the lens) makes a fresh raster
//! rather than read that one.
//!
//! The global look the models' preview is finished under is not in the
//! print: a mask is not made again for an exposure or a curve, and
//! whichever of the viewport and an export makes it first, the other
//! reads it.

use std::hash::{Hash, Hasher};
use std::path::Path;

use greycard_edit::Edit;

/// The size a base is made at: the full develop's (the viewport's and
/// an export's), or the reduced develop's, binned, whose models see it
/// through the Detail section (`Purpose::ModelsSee`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Size {
    Full,
    Reduced,
}

/// What a base is known by in the mask cache ([`print`]): a hash of
/// what made it, and the size it was made at, which also names the
/// raster's slot on disk, so the full develop's raster and a proxy's of
/// one frame are kept side by side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Print {
    pub hash: u64,
    pub size: Size,
}

impl Print {
    /// A test's print of the full develop: `hash` itself.
    #[cfg(any(test, feature = "testing"))]
    pub fn test(hash: u64) -> Self {
        Self {
            hash,
            size: Size::Full,
        }
    }
}

/// The base a learned mask is made from or read for: what this
/// session's kept work of the picture is known by (`stamp`: the
/// preview, the scene, the embedding), and the print of what made it,
/// which the rasters are kept by ([`print`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Base {
    pub stamp: u64,
    pub print: Print,
}

impl Base {
    /// A test's base: picture `stamp`, printed as itself.
    #[cfg(any(test, feature = "testing"))]
    pub fn stamped(stamp: u64) -> Self {
        Self {
            stamp,
            print: Print::test(stamp),
        }
    }
}

/// The print of the base of the frame at `file` under `edit` at `turn`
/// quarter turns, made at `size`. `None` for no file (a test's
/// picture). See the module.
pub fn print(file: Option<&Path>, edit: &Edit, turn: u8, size: Size) -> Print {
    let base = base_edit(edit, size);
    // The lens database is read only while the edit corrects by its
    // profile, as the develop key takes it.
    let lenses = (base.lens.enabled && base.lens.profile)
        .then(crate::edited_pictures::lens_database)
        .flatten();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    "greycard mask base".hash(&mut h);
    match file {
        Some(file) => {
            // One name for the file whichever way it was reached: the
            // window's open and a command line's relative path.
            let file = canonical(file);
            file.hash(&mut h);
            let meta = std::fs::metadata(&file).ok();
            meta.as_ref().map(|m| m.len()).hash(&mut h);
            meta.and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .hash(&mut h);
        }
        None => 0u8.hash(&mut h),
    }
    greycard_edit_io::key::develop_key(&base, lenses.as_deref()).hash(&mut h);
    (turn % 4).hash(&mut h);
    size.hash(&mut h);
    Print {
        hash: h.finish(),
        size,
    }
}

/// `file` by the one name it has, whichever way it was reached (the
/// window's open, a command line's relative path), with no verbatim
/// prefix on Windows; as given when it will not resolve.
pub fn canonical(file: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(file)
        .map(|p| strip_verbatim(&p))
        .unwrap_or_else(|_| file.to_path_buf())
}

/// A Windows path without its `\\?\` prefix where the path reads the
/// same without it; any other path as it is.
fn strip_verbatim(p: &Path) -> std::path::PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => std::path::PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}

/// `edit` with only what the base made at `size` reads of it: the
/// white balance, the camera profile, the demosaic, the noise, and the
/// lens; and for the reduced develop the geometry, which picks its bin,
/// and the Detail section, which its models' picture is run through.
pub fn base_edit(edit: &Edit, size: Size) -> Edit {
    let mut base = Edit {
        white_balance: edit.white_balance.clone(),
        camera: edit.camera.clone(),
        demosaic: edit.demosaic,
        noise: edit.noise.clone(),
        lens: edit.lens.clone(),
        ..Edit::default()
    };
    // Where a DCP's look table is read follows the baseline an older
    // edit keeps (`Edit::look_baseline`). Carried only where a table is
    // on, so no base made before the table moves.
    if base.camera.applies_look_table() {
        base.light.baseline = edit.light.baseline;
    }
    if size == Size::Reduced {
        base.geometry = edit.geometry.clone();
        base.detail = edit.detail;
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stages::{Facts, Identity, InputKind, Purpose, Step, stages};
    use greycard_core::raw::{CfaPattern, Orientation, Shot};

    fn facts(turn: u8) -> Facts {
        Facts {
            input: InputKind::Raw {
                pattern: Some(CfaPattern::rggb()),
                crop: (6000, 4000),
                tag: Orientation::Normal,
                warp: None,
                merged: false,
                headroom: 0.0,
                lens_corrected: false,
                monochrome: false,
            },
            identity: Identity {
                make: "Canon".into(),
                model: "EOS R5".into(),
                shot: Shot::default(),
            },
            turn,
        }
    }

    /// An edit with every section moved from its default, a learned
    /// mask among its adjustments.
    fn everything() -> Edit {
        let mut edit = Edit {
            white_balance: greycard_edit::WhiteBalance::Custom {
                temperature: 4200.0,
                tint: 0.004,
            },
            ..Edit::default()
        };
        edit.noise.enabled = true;
        edit.noise.profiled = true;
        edit.noise.strength = 0.6;
        edit.lens.enabled = true;
        edit.lens.manual = 0.2;
        edit.lens.ca_red = 0.1;
        edit.lens.defringe = true;
        edit.light.exposure = 0.7;
        edit.light.tone.contrast = 20.0;
        edit.detail.clarity = 0.4;
        edit.detail.dehaze = 0.25;
        edit.sharpen.radius = 1.5;
        edit.geometry.crop = Some(greycard_edit::geometry::Crop {
            x: 0.1,
            y: 0.0,
            w: 0.7,
            h: 1.0,
        });
        edit.vignette.amount = -0.3;
        edit.grain.amount = 0.2;
        edit.output_sharpen.amount = 0.5;
        edit.color.saturation = 15.0;
        let mut a = greycard_edit::Adjustment::default();
        a.mask.components.push(greycard_edit::mask::Component {
            shape: greycard_edit::mask::Shape::Sky { picks: Vec::new() },
            ..Default::default()
        });
        edit.adjustments.push(a);
        edit
    }

    /// What a list's ops are, up to its base or to its end.
    fn ops(steps: &crate::stages::Steps, to_base: bool) -> Vec<String> {
        let mut out = Vec::new();
        for s in &steps.steps {
            match s {
                Step::Keep(crate::stages::Keep::Base) if to_base => break,
                Step::Op(op) => out.push(format!("{op:?}")),
                _ => {}
            }
        }
        out
    }

    /// The base reads a DCP's look table where the shown picture does,
    /// an older edit's own baseline included; with no table on, the
    /// base is the one it always was.
    #[test]
    fn the_base_reads_the_look_table_where_the_picture_does() {
        use greycard_edit::camera::{Camera, ProfileChoice};
        let mut edit = Edit::default();
        edit.light.baseline = Some(0.3);
        edit.camera = Camera {
            profile: ProfileChoice::Named("Natural".into()),
            look_table: false,
        };
        let off = base_edit(&edit, Size::Full);
        assert_eq!(off.light, Edit::default().light);
        // A hand-set switch on the file's own profile is no table.
        let mut own = edit.clone();
        own.camera = Camera {
            profile: ProfileChoice::Embedded,
            look_table: true,
        };
        assert_eq!(base_edit(&own, Size::Full).light, Edit::default().light);
        edit.camera.look_table = true;
        let on = base_edit(&edit, Size::Full);
        assert_eq!(on.light.baseline, Some(0.3));
        assert_eq!(
            on.look_baseline("Canon", "EOS R5", 0.0),
            edit.look_baseline("Canon", "EOS R5", 0.0)
        );
    }

    /// The trimmed edit makes the base the whole edit makes, op for op,
    /// for the full develop and for what the reduced develop's models
    /// see: nothing a base reads is left out of the print.
    #[test]
    fn the_base_edit_makes_the_edit_s_base() {
        let edit = everything();
        for turn in 0..4 {
            let full = |e: &Edit| ops(&stages(e, &Purpose::Viewport, &facts(turn), None), true);
            assert_eq!(full(&base_edit(&edit, Size::Full)), full(&edit));
            let models = |e: &Edit| ops(&stages(e, &Purpose::ModelsSee, &facts(turn), None), false);
            assert_eq!(models(&base_edit(&edit, Size::Reduced)), models(&edit));
        }
    }

    /// Every leaf of the edit's JSON moved in turn (a number up and to
    /// zero, a switch flipped, a word to each of the edit's words), as
    /// `overlay_changes` sees an edit: where the print stays, the ops up
    /// to the base stay too, for the full develop and for what the
    /// reduced develop's models see, at a turn and without one. A field
    /// added to the edit that a base reads and the print leaves out
    /// fails here, wherever it is.
    #[test]
    fn a_leaf_the_print_leaves_out_never_reaches_the_base() {
        fn leaves(v: &serde_json::Value, at: String, out: &mut Vec<String>) {
            match v {
                serde_json::Value::Object(m) => {
                    for (k, v) in m {
                        let k = k.replace('~', "~0").replace('/', "~1");
                        leaves(v, format!("{at}/{k}"), out);
                    }
                }
                serde_json::Value::Array(a) => {
                    for (i, v) in a.iter().enumerate() {
                        leaves(v, format!("{at}/{i}"), out);
                    }
                }
                _ => out.push(at),
            }
        }
        let edit = everything();
        let json = serde_json::to_value(&edit).unwrap();
        let mut paths = Vec::new();
        leaves(&json, String::new(), &mut paths);
        // The words an edit's enums are written in, for a word to move to.
        let mut words: Vec<serde_json::Value> = greycard_edit::Demosaic::ALL
            .iter()
            .map(|d| serde_json::to_value(d).unwrap())
            .collect();
        words.extend(["as_shot", "custom", "off", "on", "auto", ""].map(serde_json::Value::from));
        let nudges = |v: &serde_json::Value| -> Vec<serde_json::Value> {
            match v {
                serde_json::Value::Number(n) => {
                    let f = n.as_f64().unwrap();
                    let mut out = vec![serde_json::Value::from(0), serde_json::Value::from(0.5)];
                    if let Some(i) = n.as_u64() {
                        out.push(serde_json::Value::from(i + 1));
                    }
                    out.push(serde_json::json!(f + 1.0));
                    out
                }
                serde_json::Value::Bool(b) => vec![serde_json::Value::from(!b)],
                serde_json::Value::String(_) => words.clone(),
                _ => Vec::new(),
            }
        };
        let base_ops = |e: &Edit, size: Size, turn: u8| match size {
            Size::Full => ops(&stages(e, &Purpose::Viewport, &facts(turn), None), true),
            Size::Reduced => ops(&stages(e, &Purpose::ModelsSee, &facts(turn), None), false),
        };
        let (mut tried, mut reached) = (0, 0);
        for path in &paths {
            let leaf = json.pointer(path).unwrap();
            for nudged in nudges(leaf) {
                if &nudged == leaf {
                    continue;
                }
                let mut moved = json.clone();
                *moved.pointer_mut(path).unwrap() = nudged.clone();
                let Ok(e) = serde_json::from_value::<Edit>(moved) else {
                    continue;
                };
                tried += 1;
                for size in [Size::Full, Size::Reduced] {
                    for turn in [0, 1] {
                        let same = base_ops(&e, size, turn) == base_ops(&edit, size, turn);
                        if !same {
                            reached += 1;
                        }
                        assert!(
                            same || print(None, &e, turn, size) != print(None, &edit, turn, size),
                            "{path} moved to {nudged} reaches the {size:?} base, and the print \
                             leaves it out"
                        );
                    }
                }
            }
        }
        assert!(tried > 200, "{tried} edits tried");
        assert!(reached > 20, "{reached} reached a base");
    }

    /// The print moves with what the base is made from: the white
    /// balance, the lens, the noise, the turn, the size, the file; and
    /// the reduced base's with its crop and Detail section. It stays
    /// for what only the finish reads.
    #[test]
    fn the_print_moves_with_the_base_and_only_with_it() {
        let edit = everything();
        let at = |e: &Edit| print(None, e, 0, Size::Full);
        let p = at(&edit);
        let moved = |what: &str, change: &dyn Fn(&mut Edit)| {
            let mut e = edit.clone();
            change(&mut e);
            (what.to_string(), at(&e) != p)
        };
        let base = [
            moved("white balance", &|e| {
                e.white_balance = greycard_edit::WhiteBalance::Custom {
                    temperature: 5000.0,
                    tint: 0.004,
                }
            }),
            moved("lens", &|e| e.lens.manual += 0.1),
            moved("defringe", &|e| e.lens.defringe = false),
            moved("denoise", &|e| e.noise.strength += 0.1),
            moved("demosaic", &|e| e.demosaic = greycard_edit::Demosaic::Vng4),
        ];
        for (what, moved) in base {
            assert!(moved, "{what} moves the print");
        }
        let finish = [
            moved("exposure", &|e| e.light.exposure += 1.0),
            moved("contrast", &|e| e.light.tone.contrast -= 10.0),
            moved("saturation", &|e| e.color.saturation -= 10.0),
            moved("sharpen", &|e| e.sharpen.radius = 0.5),
            moved("clarity", &|e| e.detail.clarity = 0.0),
            moved("crop", &|e| e.geometry.crop = None),
            moved("vignette", &|e| e.vignette.amount = 0.0),
            moved("grain", &|e| e.grain.amount = 0.0),
            moved("output sharpen", &|e| e.output_sharpen.amount = 0.0),
            moved("a mask's adjustment", &|e| {
                e.adjustments[0].look.light.exposure = 1.0
            }),
        ];
        for (what, moved) in finish {
            assert!(!moved, "{what} leaves the print");
        }
        assert_ne!(print(None, &edit, 1, Size::Full), p, "a turn");
        assert_ne!(print(None, &edit, 0, Size::Reduced), p, "the reduced size");
        let reduced = |e: &Edit| print(None, e, 0, Size::Reduced);
        let mut cropped = edit.clone();
        cropped.geometry.crop = Some(greycard_edit::geometry::Crop {
            x: 0.1,
            y: 0.1,
            w: 0.3,
            h: 0.3,
        });
        assert_ne!(reduced(&cropped), reduced(&edit), "a crop picks the bin");
        let mut flat = edit.clone();
        flat.detail.clarity = 0.0;
        assert_ne!(reduced(&flat), reduced(&edit), "the models see Detail");

        let dir = crate::testing::scratch_dir("mask-base-file");
        let (a, b) = (dir.join("a.dng"), dir.join("b.dng"));
        std::fs::write(&a, b"one").unwrap();
        std::fs::write(&b, b"one").unwrap();
        let of = |f: &Path| print(Some(f), &edit, 0, Size::Full);
        assert_eq!(of(&a), of(&a));
        assert_ne!(of(&a), of(&b), "another file");
        assert_ne!(of(&a), p, "a file is not no file");
        // The same file by another name for it.
        assert_eq!(of(&dir.join(".").join("a.dng")), of(&a));
        std::fs::write(&a, b"another").unwrap();
        assert_ne!(of(&a), of(&b), "a file written again");
        greycard_core::testing::remove_dir_retry(&dir);
    }
}
