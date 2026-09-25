//! Words for a step of the history: what one edit changed from the
//! one before it, as a row of the panel says it.

use crate::mask::{Component, Mask};
use crate::{Edit, Look, Snapshot};

/// What `after` changed from `before`, in a few words: the one thing
/// when one thing moved ("Exposure +0.50"), the sections' names when
/// several did, and a snapshot's name when `after` is one and more
/// than one section moved to get there.
pub fn describe(before: &Edit, after: &Edit, snapshots: &[Snapshot]) -> String {
    let changes = changes(before, after);
    match changes.as_slice() {
        [] => "No change".to_string(),
        [one] => one.clone(),
        many => match snapshots.iter().find(|s| s.edit == *after) {
            Some(s) => snapshot_label(&s.name),
            None => many.join(", "),
        },
    }
}

/// What a row of the history says for the step from `before` to
/// `after`: the words it was recorded with when it has any (see
/// [`crate::Step::label`]), and otherwise what [`describe`] makes of
/// the difference. A label that is blank reads as none.
pub fn describe_step(
    before: &Edit,
    after: &Edit,
    label: Option<&str>,
    snapshots: &[Snapshot],
) -> String {
    match label.map(str::trim) {
        Some(label) if !label.is_empty() => label.to_string(),
        _ => describe(before, after, snapshots),
    }
}

/// How many characters a row of the history panel shows at a scale
/// of one before it elides the rest (the hover's status line has it
/// whole): a line of small type in the left column, measured.
pub const ROW_CHARS: usize = 36;

// The words a named step is recorded with. Short, to fit
// [`ROW_CHARS`]: the kind of step first, in as few characters as say
// it, and the preset's, snapshot's or frame's name after, so a name of
// sixteen characters survives whole.

/// A preset laid over the frame on its own.
pub fn preset_label(name: &str) -> String {
    format!("Preset: {name}")
}

/// A preset laid over a selected set of `frames`, on each frame of
/// it that took it, the one on screen too: "Preset ×3: Faded film".
pub fn preset_over_set_label(frames: usize, name: &str) -> String {
    format!("Preset ×{frames}: {name}")
}

/// A sync onto a frame, from the frame named `from` (its file name).
pub fn sync_label(from: &str) -> String {
    format!("Sync from {from}")
}

/// A snapshot restored.
pub fn snapshot_label(name: &str) -> String {
    format!("Snapshot: {name}")
}

/// One entry per section that differs, in the panel's order, each
/// as specific as a single moved control allows.
fn changes(b: &Edit, a: &Edit) -> Vec<String> {
    let mut out = Vec::new();
    if b.white_balance != a.white_balance {
        out.push(white_balance(b, a));
    }
    out.extend(look(&b.look(), &a.look(), ""));
    if b.bw != a.bw {
        out.push(black_and_white(b, a));
    }
    if b.noise != a.noise {
        out.push(noise(b, a));
    }
    if b.lens != a.lens {
        out.push("Lens".to_string());
    }
    if b.detail != a.detail {
        out.push(detail(b, a));
    }
    if b.sharpen != a.sharpen {
        out.push(match (b.sharpen.enabled, a.sharpen.enabled) {
            (x, y) if x != y => switched("Sharpen", y),
            _ => "Sharpen".to_string(),
        });
    }
    if b.geometry != a.geometry {
        out.push(geometry(b, a));
    }
    if b.vignette != a.vignette {
        let (x, y) = (&b.vignette, &a.vignette);
        let shape_same =
            x.midpoint == y.midpoint && x.feather == y.feather && x.roundness == y.roundness;
        out.push(
            if x.enabled != y.enabled && x.amount == y.amount && shape_same {
                switched("Vignette", y.enabled)
            } else if x.amount != y.amount && x.enabled == y.enabled && shape_same {
                format!("Vignette {:+.2}", y.amount)
            } else {
                "Vignette".to_string()
            },
        );
    }
    if b.grain != a.grain {
        out.push(if b.grain.enabled != a.grain.enabled {
            switched("Grain", a.grain.enabled)
        } else {
            "Grain".to_string()
        });
    }
    if b.demosaic != a.demosaic {
        out.push("Demosaic".to_string());
    }
    if b.camera != a.camera {
        out.push(match &a.camera.profile {
            crate::camera::ProfileChoice::Embedded => "Camera profile embedded".to_string(),
            crate::camera::ProfileChoice::Named(name) => format!("Camera profile {name}"),
        });
    }
    if b.look_lut != a.look_lut {
        // The table when it changed, otherwise the strength, which is
        // the only other thing in the section.
        out.push(if b.look_lut.lut == a.look_lut.lut {
            format!("Look strength {:.0}%", a.look_lut.strength * 100.0)
        } else {
            match &a.look_lut.lut {
                crate::look::LutChoice::None => "Look none".to_string(),
                crate::look::LutChoice::Named(name) => format!("Look {name}"),
            }
        });
    }
    out.extend(adjustments(b, a));
    if b.retouch != a.retouch {
        out.push(retouch(b, a));
    }
    out
}

fn switched(name: &str, on: bool) -> String {
    format!("{name} {}", if on { "on" } else { "off" })
}

fn white_balance(b: &Edit, a: &Edit) -> String {
    use crate::WhiteBalance::{AsShot, Custom};
    match (&b.white_balance, &a.white_balance) {
        (_, AsShot) => "White balance as shot".to_string(),
        (
            Custom {
                temperature: t0,
                tint: d0,
            },
            Custom {
                temperature: t1,
                tint: d1,
            },
        ) if t0 == t1 && d0 != d1 => {
            format!("Tint {:+.3}", d1)
        }
        (_, Custom { temperature, .. }) => format!("White balance {temperature:.0} K"),
    }
}

fn noise(b: &Edit, a: &Edit) -> String {
    let (x, y) = (&b.noise, &a.noise);
    let rest_same = |skip: &str| {
        (skip == "enabled" || x.enabled == y.enabled)
            && (skip == "profiled" || x.profiled == y.profiled)
            && (skip == "strength" || x.strength == y.strength)
            && (skip == "learned" || x.learned == y.learned)
            && (skip == "learned_strength" || x.learned_strength == y.learned_strength)
    };
    if x.enabled != y.enabled && rest_same("enabled") {
        switched("Noise", y.enabled)
    } else if x.profiled != y.profiled && rest_same("profiled") {
        switched("Denoise", y.profiled)
    } else if x.strength != y.strength && rest_same("strength") {
        format!("Denoise {:.1}", y.strength)
    } else if x.learned != y.learned && rest_same("learned") {
        format!("Learned denoise {}", y.learned.name())
    } else if x.learned_strength != y.learned_strength && rest_same("learned_strength") {
        format!("Learned denoise {:.0}%", y.learned_strength * 100.0)
    } else {
        "Noise".to_string()
    }
}

fn detail(b: &Edit, a: &Edit) -> String {
    let (x, y) = (&b.detail, &a.detail);
    let moved = [
        x.enabled != y.enabled,
        x.texture != y.texture,
        x.clarity != y.clarity,
        x.dehaze != y.dehaze,
    ];
    if moved.iter().filter(|&&m| m).count() != 1 {
        return "Detail".to_string();
    }
    if moved[0] {
        switched("Detail", y.enabled)
    } else if moved[1] {
        format!("Texture {:+.0}", y.texture * 100.0)
    } else if moved[2] {
        format!("Clarity {:+.0}", y.clarity * 100.0)
    } else {
        format!("Dehaze {:+.0}", y.dehaze * 100.0)
    }
}

fn geometry(b: &Edit, a: &Edit) -> String {
    let (x, y) = (&b.geometry, &a.geometry);
    let mut moved = Vec::new();
    if x.turns != y.turns {
        moved.push("Rotate".to_string());
    }
    if x.flip != y.flip {
        moved.push("Flip".to_string());
    }
    if x.angle != y.angle {
        moved.push(format!("Straighten {:+.1}°", y.angle));
    }
    if x.vertical != y.vertical || x.horizontal != y.horizontal {
        moved.push("Perspective".to_string());
    }
    // A turn, a straighten or a perspective refits the crop, so a
    // crop that moved with one of those is that one's doing.
    if moved.is_empty() && (x.crop != y.crop || x.aspect != y.aspect || x.portrait != y.portrait) {
        moved.push("Crop".to_string());
    }
    match moved.as_slice() {
        [one] => one.clone(),
        _ => "Geometry".to_string(),
    }
}

/// The look's sections that differ, each named with `prefix` before
/// it (an adjustment's name and a colon, or nothing for the global).
fn look(b: &Look, a: &Look, prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    if b.light != a.light {
        out.push(format!("{prefix}{}", light(b, a)));
    }
    if b.curves != a.curves {
        out.push(format!("{prefix}{}", curves(b, a)));
    }
    if b.mixer != a.mixer {
        out.push(format!(
            "{prefix}{}",
            if b.mixer.enabled != a.mixer.enabled {
                switched("Color mixer", a.mixer.enabled)
            } else {
                "Color mixer".to_string()
            }
        ));
    }
    if b.color != a.color {
        out.push(format!("{prefix}{}", color(b, a)));
    }
    if b.grading != a.grading {
        out.push(format!(
            "{prefix}{}",
            if b.grading.enabled != a.grading.enabled {
                switched("Color grading", a.grading.enabled)
            } else {
                "Color grading".to_string()
            }
        ));
    }
    if b.tint != a.tint {
        out.push(format!("{prefix}{}", tint(&b.tint, &a.tint)));
    }
    out
}

/// The tint that moved. "Color tint", not "Tint": the white
/// balance's Duv already answers to that name.
fn tint(b: &crate::Tint, a: &crate::Tint) -> String {
    // Off only when it was on: picking a hue with the amount still at
    // nothing is the ordinary way round, and turns nothing off.
    if a.amount <= 0.0 && b.amount > 0.0 {
        "Color tint off".to_string()
    } else if b.hue != a.hue && b.amount == a.amount {
        format!("Color tint {:.0}°", a.hue)
    } else if b.amount != a.amount && b.hue == a.hue {
        format!("Color tint {:.0}%", a.amount * 100.0)
    } else {
        "Color tint".to_string()
    }
}

fn light(b: &Look, a: &Look) -> String {
    let (x, y) = (&b.light, &a.light);
    let (tx, ty) = (&x.tone, &y.tone);
    let mut moved: Vec<String> = Vec::new();
    if x.enabled != y.enabled {
        moved.push(switched("Light", y.enabled));
    }
    if x.exposure != y.exposure {
        moved.push(format!("Exposure {:+.2}", y.exposure));
    }
    if tx.enabled != ty.enabled {
        moved.push(switched("Tone curve", ty.enabled));
    }
    for (name, was, is) in [
        ("Contrast", tx.contrast, ty.contrast),
        ("Highlights", tx.highlights, ty.highlights),
        ("Shadows", tx.shadows, ty.shadows),
        ("Whites", tx.whites, ty.whites),
        ("Blacks", tx.blacks, ty.blacks),
    ] {
        if was != is {
            moved.push(if name == "Contrast" {
                format!("{name} {is:.2}")
            } else {
                format!("{name} {is:+.2}")
            });
        }
    }
    match moved.as_slice() {
        [one] => one.clone(),
        _ => "Light".to_string(),
    }
}

/// The curve control that moved: the switch, one of the parametric
/// curve's amounts (shown as the panel shows them, ±100) or splits,
/// or, for the point curves, the section.
fn curves(b: &Look, a: &Look) -> String {
    let (x, y) = (&b.curves, &a.curves);
    let (px, py) = (&x.parametric, &y.parametric);
    let mut moved: Vec<String> = Vec::new();
    if x.enabled != y.enabled {
        moved.push(switched("Curves", y.enabled));
    }
    for (name, was, is) in [
        ("Curve highlights", px.highlights, py.highlights),
        ("Curve lights", px.lights, py.lights),
        ("Curve darks", px.darks, py.darks),
        ("Curve shadows", px.shadows, py.shadows),
    ] {
        if was != is {
            moved.push(format!("{name} {:+.0}", is * 100.0));
        }
    }
    for (name, was, is) in [
        ("Shadow split", px.splits[0], py.splits[0]),
        ("Midtone split", px.splits[1], py.splits[1]),
        ("Highlight split", px.splits[2], py.splits[2]),
    ] {
        if was != is {
            moved.push(format!("{name} {:.0}%", is * 100.0));
        }
    }
    let points = |c: &crate::Curves| {
        (
            c.rgb.clone(),
            c.red.clone(),
            c.green.clone(),
            c.blue.clone(),
            c.red_green.clone(),
            c.blue_yellow.clone(),
        )
    };
    if points(x) != points(y) {
        moved.push("Curves".to_string());
    }
    match moved.as_slice() {
        [one] => one.clone(),
        _ => "Curves".to_string(),
    }
}

fn color(b: &Look, a: &Look) -> String {
    let (x, y) = (&b.color, &a.color);
    let mut moved: Vec<String> = Vec::new();
    if x.enabled != y.enabled {
        moved.push(switched("Color", y.enabled));
    }
    for (name, was, is) in [
        ("Vibrance", x.vibrance, y.vibrance),
        ("Saturation", x.saturation, y.saturation),
    ] {
        if was != is {
            moved.push(format!("{name} {is:+.2}"));
        }
    }
    match moved.as_slice() {
        [one] => one.clone(),
        _ => "Color".to_string(),
    }
}

/// Black and white: its switch, the filter its weights are, the
/// strength, or a band's weight when one moved.
fn black_and_white(b: &Edit, a: &Edit) -> String {
    let (x, y) = (&b.bw, &a.bw);
    if x.enabled != y.enabled && x.weights == y.weights && x.strength == y.strength {
        return switched("Black and white", y.enabled);
    }
    if x.strength != y.strength && x.weights == y.weights && x.enabled == y.enabled {
        return format!("Black and white strength {:.2}x", y.strength);
    }
    let moved: Vec<usize> = (0..crate::mixer::BANDS)
        .filter(|&i| x.weights[i] != y.weights[i])
        .collect();
    match (y.filter(), moved.as_slice()) {
        (Some(f), _) if f != crate::bw::Filter::None => format!("{} filter", f.name()),
        (_, [i]) if x.enabled == y.enabled => format!(
            "Black and white {} {:+.2}",
            crate::mixer::Band::ALL[*i].name().to_lowercase(),
            y.weights[*i]
        ),
        _ => "Black and white".to_string(),
    }
}

fn adjustments(b: &Edit, a: &Edit) -> Vec<String> {
    if b.adjustments == a.adjustments {
        return Vec::new();
    }
    let find = |edit: &Edit, id: u64| edit.adjustments.iter().find(|x| x.id == id).cloned();
    let added: Vec<_> = a
        .adjustments
        .iter()
        .filter(|x| find(b, x.id).is_none())
        .collect();
    let removed: Vec<_> = b
        .adjustments
        .iter()
        .filter(|x| find(a, x.id).is_none())
        .collect();
    let changed: Vec<_> = a
        .adjustments
        .iter()
        .filter_map(|y| find(b, y.id).filter(|x| x != y).map(|x| (x, y)))
        .collect();
    match (added.as_slice(), removed.as_slice(), changed.as_slice()) {
        ([one], [], []) => vec![format!("{} added", one.name)],
        ([], [one], []) => vec![format!("{} removed", one.name)],
        ([], [], [(x, y)]) => {
            if x.enabled != y.enabled && x.mask == y.mask && x.look == y.look && x.name == y.name {
                vec![switched(&y.name, y.enabled)]
            } else if x.name != y.name && x.mask == y.mask && x.look == y.look {
                vec![format!("{} renamed {}", x.name, y.name)]
            } else if x.mask != y.mask && x.look == y.look {
                vec![mask(&y.name, &x.mask, &y.mask)]
            } else if x.look != y.look && x.mask == y.mask {
                let parts = look(&x.look, &y.look, &format!("{}: ", y.name));
                if parts.len() == 1 {
                    parts
                } else {
                    vec![format!("{}: Light", y.name)]
                }
            } else {
                vec![y.name.clone()]
            }
        }
        _ => vec!["Adjustments".to_string()],
    }
}

fn mask(name: &str, x: &Mask, y: &Mask) -> String {
    // One shape switched, the rest of the mask as it was: name the
    // shape and which way the switch went.
    let switched_shape = || {
        let mut it = x
            .components
            .iter()
            .zip(&y.components)
            .filter(|(a, b)| a != b);
        let (a, b) = it.next()?;
        if it.next().is_some() || a.enabled == b.enabled {
            return None;
        }
        (Component {
            enabled: b.enabled,
            ..a.clone()
        } == *b)
            .then(|| switched(b.shape.name(), b.enabled))
    };
    if x.invert != y.invert && x.components == y.components {
        format!("{name} mask inverted")
    } else if x.components.len() == y.components.len()
        && x.invert == y.invert
        && let Some(s) = switched_shape()
    {
        format!("{name} {s}")
    } else if y.components.len() > x.components.len() {
        format!(
            "{name} {} added",
            y.components
                .last()
                .map(|c| c.shape.name())
                .unwrap_or("shape")
        )
    } else if y.components.len() < x.components.len() {
        format!("{name} shape removed")
    } else {
        format!("{name} mask")
    }
}

fn retouch(b: &Edit, a: &Edit) -> String {
    let (x, y) = (&b.retouch, &a.retouch);
    if x.enabled != y.enabled && x.patches == y.patches {
        return switched("Retouch", y.enabled);
    }
    let has = |patches: &[crate::retouch::Patch], id: u64| patches.iter().any(|p| p.id == id);
    let added: Vec<_> = y
        .patches
        .iter()
        .filter(|p| !has(&x.patches, p.id))
        .collect();
    let removed: Vec<_> = x
        .patches
        .iter()
        .filter(|p| !has(&y.patches, p.id))
        .collect();
    match (added.as_slice(), removed.as_slice()) {
        ([one], []) => one.name(),
        ([], [one]) => format!("{} removed", one.name()),
        ([], []) => {
            let moved: Vec<_> = y
                .patches
                .iter()
                .filter(|p| x.patches.iter().any(|q| q.id == p.id && q != *p))
                .collect();
            match moved.as_slice() {
                [one] => format!("{} moved", one.name()),
                _ => "Retouch".to_string(),
            }
        }
        _ => "Retouch".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask::Shape;
    use crate::retouch::{Method, Patch};
    use crate::{Adjustment, WhiteBalance};

    fn snap(name: &str, edit: &Edit) -> Snapshot {
        Snapshot {
            name: name.into(),
            taken: 0,
            edit: edit.clone(),
        }
    }

    #[test]
    fn one_moved_control_is_named_with_its_value() {
        let base = Edit::default();
        let mut e = base.clone();
        e.light.exposure = 0.5;
        assert_eq!(describe(&base, &e, &[]), "Exposure +0.50");
        let mut e = base.clone();
        e.light.tone.blacks = -0.1;
        assert_eq!(describe(&base, &e, &[]), "Blacks -0.10");
        let mut e = base.clone();
        e.light.tone.contrast = 1.2;
        assert_eq!(describe(&base, &e, &[]), "Contrast 1.20");
        let mut e = base.clone();
        e.white_balance = WhiteBalance::Custom {
            temperature: 3200.0,
            tint: 0.0,
        };
        assert_eq!(describe(&base, &e, &[]), "White balance 3200 K");
        let mut f = e.clone();
        f.white_balance = WhiteBalance::Custom {
            temperature: 3200.0,
            tint: 0.004,
        };
        assert_eq!(describe(&e, &f, &[]), "Tint +0.004");
        assert_eq!(describe(&f, &base, &[]), "White balance as shot");
        let mut e = base.clone();
        e.noise.strength = 2.5;
        assert_eq!(describe(&base, &e, &[]), "Denoise 2.5");
        let mut e = base.clone();
        e.detail.texture = 0.5;
        assert_eq!(describe(&base, &e, &[]), "Texture +50");
        let mut f = e.clone();
        f.detail.clarity = -0.25;
        assert_eq!(describe(&e, &f, &[]), "Clarity -25");
        f.detail.enabled = false;
        assert_eq!(describe(&e, &f, &[]), "Detail");
        assert_eq!(describe(&f, &e, &[]), "Detail");
        let mut g = f.clone();
        g.detail.enabled = true;
        assert_eq!(describe(&f, &g, &[]), "Detail on");
        let mut h = g.clone();
        h.detail.dehaze = 0.5;
        assert_eq!(describe(&g, &h, &[]), "Dehaze +50");
        h.detail.texture = 0.0;
        assert_eq!(describe(&g, &h, &[]), "Detail");
        let mut e = base.clone();
        e.sharpen.enabled = false;
        assert_eq!(describe(&base, &e, &[]), "Sharpen off");
        let mut e = base.clone();
        e.sharpen.iterations = 30;
        assert_eq!(describe(&base, &e, &[]), "Sharpen");
        let mut e = base.clone();
        e.geometry.angle = 1.25;
        assert_eq!(describe(&base, &e, &[]), "Straighten +1.2°");
        let mut e = base.clone();
        e.geometry.turns = 1;
        assert_eq!(describe(&base, &e, &[]), "Rotate");
        // A straighten refits the crop; the step is the straighten.
        let mut e = base.clone();
        e.geometry.angle = -0.5;
        e.geometry.crop = Some(crate::geometry::Crop {
            x: 0.1,
            y: 0.1,
            w: 0.8,
            h: 0.8,
        });
        assert_eq!(describe(&base, &e, &[]), "Straighten -0.5°");
        let mut e = base.clone();
        e.vignette.amount = -1.0;
        assert_eq!(describe(&base, &e, &[]), "Vignette -1.00");
        let mut e = base.clone();
        e.grain.enabled = !e.grain.enabled;
        assert!(describe(&base, &e, &[]).starts_with("Grain o"));
        let mut e = base.clone();
        e.mixer.saturation[0] = 0.3;
        assert_eq!(describe(&base, &e, &[]), "Color mixer");
        let mut e = base.clone();
        e.color.vibrance = 0.2;
        assert_eq!(describe(&base, &e, &[]), "Vibrance +0.20");
        let mut e = base.clone();
        e.color.saturation = -0.1;
        assert_eq!(describe(&base, &e, &[]), "Saturation -0.10");
        let mut e = base.clone();
        e.color.enabled = false;
        assert_eq!(describe(&base, &e, &[]), "Color off");
        let mut e = base.clone();
        e.tint.amount = 0.5;
        assert_eq!(describe(&base, &e, &[]), "Color tint 50%");
        let mut e = base.clone();
        e.tint = crate::Tint {
            hue: 210.0,
            amount: 0.5,
        };
        let mut back = e.clone();
        back.tint.hue = 40.0;
        assert_eq!(describe(&e, &back, &[]), "Color tint 40\u{b0}");
        back.tint.amount = 0.0;
        assert_eq!(describe(&e, &back, &[]), "Color tint off");
        // A hue picked before the amount is moved turns nothing off:
        // the amount was at nothing already.
        let mut picked = base.clone();
        picked.tint.hue = 130.0;
        assert_eq!(describe(&base, &picked, &[]), "Color tint 130\u{b0}");
        let mut undone = picked.clone();
        undone.tint.hue = 0.0;
        assert_eq!(describe(&picked, &undone, &[]), "Color tint 0\u{b0}");
        // And a mask's own tint is named after the mask.
        let mut e = base.clone();
        e.adjustments.push(Adjustment {
            id: 1,
            name: "Sky".into(),
            ..Adjustment::default()
        });
        e.adjustments[0].look.tint.hue = 210.0;
        let mut after = e.clone();
        after.adjustments[0].look.tint.amount = 0.4;
        assert_eq!(describe(&e, &after, &[]), "Sky: Color tint 40%");
        let mut e = base.clone();
        e.bw.enabled = true;
        assert_eq!(describe(&base, &e, &[]), "Black and white on");
        let mut e = base.clone();
        e.bw = crate::BlackWhite::with_filter(crate::bw::Filter::Red, true);
        assert_eq!(describe(&base, &e, &[]), "Red filter");
        let mut e = base.clone();
        e.bw.weights[4] = -0.5;
        assert_eq!(describe(&base, &e, &[]), "Black and white aqua -0.50");
        let mut e = base.clone();
        e.bw.strength = 2.0;
        assert_eq!(describe(&base, &e, &[]), "Black and white strength 2.00x");
        assert_eq!(describe(&base, &base, &[]), "No change");
    }

    #[test]
    fn several_sections_are_listed_in_the_panels_order() {
        let base = Edit::default();
        let mut e = base.clone();
        e.grain.amount = 0.5;
        e.light.exposure = 1.0;
        e.light.tone.whites = 0.2;
        e.curves.enabled = false;
        assert_eq!(describe(&base, &e, &[]), "Light, Curves off, Grain");
        // The same step to a snapshot's state is the snapshot's.
        assert_eq!(
            describe(&base, &e, &[snap("Evening", &e)]),
            "Snapshot: Evening"
        );
        // Unless one control did it, which is more to the point.
        let mut one = base.clone();
        one.light.exposure = 1.0;
        assert_eq!(
            describe(&base, &one, &[snap("Bright", &one)]),
            "Exposure +1.00"
        );
    }

    #[test]
    fn the_parametric_curves_controls_are_named_with_their_values() {
        let base = Edit::default();
        let mut e = base.clone();
        e.curves.parametric.lights = -0.35;
        assert_eq!(describe(&base, &e, &[]), "Curve lights -35");
        let mut e = base.clone();
        e.curves.parametric.splits[2] = 0.8;
        assert_eq!(describe(&base, &e, &[]), "Highlight split 80%");
        // A point moved is the section, as are two controls at once.
        let mut e = base.clone();
        e.curves.rgb = vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]];
        assert_eq!(describe(&base, &e, &[]), "Curves");
        let mut e = base.clone();
        e.curves.parametric.shadows = 0.2;
        e.curves.parametric.darks = 0.2;
        assert_eq!(describe(&base, &e, &[]), "Curves");
        // In an adjustment, with its name.
        let mut e = base.clone();
        let mut look = Look::default();
        look.curves.parametric.highlights = 0.5;
        e.adjustments.push(Adjustment {
            id: 1,
            name: "Sky".into(),
            look,
            ..Default::default()
        });
        let mut after = e.clone();
        after.adjustments[0].look.curves.parametric.highlights = 0.6;
        assert_eq!(describe(&e, &after, &[]), "Sky: Curve highlights +60");
    }

    /// A look step says what the section became: the table when the
    /// table changed, the strength when only it moved.
    #[test]
    fn a_look_step_names_the_table_or_the_strength() {
        use crate::look::{LookLut, LutChoice};
        let base = Edit::default();
        let mut chosen = base.clone();
        chosen.look_lut = LookLut {
            lut: LutChoice::Named("Test Look".into()),
            strength: 1.0,
        };
        assert_eq!(describe(&base, &chosen, &[]), "Look Test Look");
        assert_eq!(describe(&chosen, &base, &[]), "Look none");
        let mut weaker = chosen.clone();
        weaker.look_lut.strength = 0.4;
        assert_eq!(describe(&chosen, &weaker, &[]), "Look strength 40%");
        // The table changing outranks a strength that moved with it.
        let mut other = weaker.clone();
        other.look_lut.lut = LutChoice::Named("Other".into());
        other.look_lut.strength = 0.9;
        assert_eq!(describe(&weaker, &other, &[]), "Look Other");
        // And no change is no step.
        assert_ne!(describe(&chosen, &chosen.clone(), &[]), "Look none");
    }

    #[test]
    fn adjustments_and_patches_are_named_by_what_came_or_went() {
        let base = Edit::default();
        let mut e = base.clone();
        let mut look = Look::default();
        look.light.exposure = -1.0;
        e.adjustments.push(Adjustment {
            id: 1,
            name: "Linear 1".into(),
            mask: Mask {
                components: vec![Component {
                    shape: Shape::Linear {
                        from: [0.0, 0.0],
                        to: [1.0, 1.0],
                    },
                    ..Default::default()
                }],
                invert: false,
            },
            look,
            ..Default::default()
        });
        assert_eq!(describe(&base, &e, &[]), "Linear 1 added");
        assert_eq!(describe(&e, &base, &[]), "Linear 1 removed");
        let mut f = e.clone();
        f.adjustments[0].look.light.exposure = -0.5;
        assert_eq!(describe(&e, &f, &[]), "Linear 1: Exposure -0.50");
        let mut f = e.clone();
        f.adjustments[0].mask.invert = true;
        assert_eq!(describe(&e, &f, &[]), "Linear 1 mask inverted");
        let mut f = e.clone();
        f.adjustments[0].enabled = false;
        assert_eq!(describe(&e, &f, &[]), "Linear 1 off");
        // A shape's own switch is named for the shape, not the mask.
        let mut f = e.clone();
        f.adjustments[0].mask.components[0].enabled = false;
        assert_eq!(describe(&e, &f, &[]), "Linear 1 Linear off");
        assert_eq!(describe(&f, &e, &[]), "Linear 1 Linear on");
        // A shape switched and something else moved with it is just
        // the mask.
        let mut g = f.clone();
        g.adjustments[0].mask.components[0].invert = true;
        assert_eq!(describe(&e, &g, &[]), "Linear 1 mask");
        let mut f = e.clone();
        f.adjustments[0].name = "Sky".into();
        assert_eq!(describe(&e, &f, &[]), "Linear 1 renamed Sky");

        let mut p = base.clone();
        p.retouch.patches.push(Patch {
            id: 3,
            method: Method::Heal,
            points: vec![[0.5, 0.5]],
            radius: 0.02,
            feather: 0.5,
            opacity: 1.0,
            source: None,
        });
        assert_eq!(describe(&base, &p, &[]), "Heal 3");
        assert_eq!(describe(&p, &base, &[]), "Heal 3 removed");
        let mut q = p.clone();
        q.retouch.patches[0].radius = 0.05;
        assert_eq!(describe(&p, &q, &[]), "Heal 3 moved");
    }

    #[test]
    fn a_step_with_words_is_named_by_them_and_one_without_as_before() {
        let base = Edit::default();
        let mut after = base.clone();
        after.light.exposure = 0.5;
        after.color.saturation = 0.2;
        let label = preset_label("Faded film");
        assert_eq!(
            describe_step(&base, &after, Some(&label), &[]),
            "Preset: Faded film"
        );
        assert_eq!(
            describe_step(&base, &after, None, &[]),
            describe(&base, &after, &[])
        );
        // Blank words are none.
        assert_eq!(
            describe_step(&base, &after, Some("  "), &[]),
            describe(&base, &after, &[])
        );
        // A name of sixteen characters survives whole in a row, even
        // over a set in the hundreds; a camera's file name does too.
        let name = "Kodachrome 64 Su";
        assert_eq!(name.chars().count(), 16);
        for words in [
            preset_label(name),
            preset_over_set_label(250, name),
            sync_label("5M0A3021.CR3"),
            snapshot_label(name),
        ] {
            assert!(words.chars().count() <= ROW_CHARS, "{words}");
        }
        assert_eq!(
            preset_over_set_label(3, "Faded film"),
            "Preset ×3: Faded film"
        );
    }
}
