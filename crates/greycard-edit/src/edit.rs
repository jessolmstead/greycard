//! The edit and its parts: the struct, the look a mask can carry, and
//! the engine settings an edit asks for.

use serde::{Deserialize, Serialize};

use greycard_core::develop::DevelopSettings;
use greycard_core::develop::denoise::{DenoiseOptions, Strength};

use crate::{
    BlackWhite, Camera, Color, Curves, Demosaic, Detail, DisplayCurve, Geometry, Grading, Lens,
    Light, Loader, LocalWhite, LookLut, Mask, Mixer, Noise, OutputSharpen, Result, Sharpen, Tint,
    VERSION, WhiteBalance, grain, mask, migrate, retouch, vignette,
};

/// A development, described.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Edit {
    /// Schema version of the struct as written.
    pub version: u32,
    /// Version 4's step still waits on the frame's shape
    /// ([`Edit::needs_frame`]). Kept apart from `version`, so an edit
    /// written back meanwhile is at this build's version and no other
    /// step runs on it twice; left out of a sidecar when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub frame_pending: bool,
    pub light: Light,
    /// The display curve under the light: per channel, as it always has
    /// been, or AgX. Global, as the curve is; left out of a sidecar at its
    /// default, so a picture nobody switched writes what it always wrote.
    #[serde(skip_serializing_if = "DisplayCurve::is_channels")]
    pub display_curve: DisplayCurve,
    pub white_balance: WhiteBalance,
    pub noise: Noise,
    /// Local contrast, Texture, Clarity, and Structure, on the
    /// developed picture after the retouch and before the sharpen.
    pub detail: Detail,
    pub sharpen: Sharpen,
    /// The unsharp mask on the finished picture, after the tone curve,
    /// the curves, and the look table. Left out of a sidecar at its
    /// default, so every sidecar and develop key written before it is
    /// unchanged.
    #[serde(skip_serializing_if = "OutputSharpen::is_default")]
    pub output_sharpen: OutputSharpen,
    /// The parametric and point curves on the encoded picture, after
    /// the tone curve.
    pub curves: Curves,
    /// The color mixer, by hue band, on the scene after exposure.
    pub mixer: Mixer,
    /// Global vibrance and saturation, in the same pass as the mixer.
    #[serde(alias = "colour")]
    pub color: Color,
    /// The conversion to black and white, after the mixer and the
    /// global color, before the curves and the grading. Global: a
    /// mask carries a look, and a picture is mono or it is not.
    pub bw: BlackWhite,
    /// Color grading by lightness, after the curves, with them.
    pub grading: Grading,
    /// A tint toward one hue, in the same Oklab pass, last in it.
    pub tint: Tint,
    /// The look the picture finishes through: a 3D LUT by name and a
    /// strength, after the tone curve and the point curves, before
    /// the output transform (notes §78). Named `look_lut` and not
    /// `look` because [`Look`] above is already the other thing the
    /// word means here; the sidecar's key is `look`.
    #[serde(rename = "look")]
    pub look_lut: LookLut,
    /// The lens's corrections, on the developed picture before
    /// everything else that moves or shades it.
    pub lens: Lens,
    /// Straighten and crop, on the developed picture.
    pub geometry: Geometry,
    /// The vignette over the frame as shown.
    pub vignette: vignette::Vignette,
    /// The grain over the frame as shown.
    pub grain: grain::Grain,
    pub demosaic: Demosaic,
    /// The camera profile the sensor's color is taken through: the
    /// file's own calibrations, or a DCP from the profile directory.
    pub camera: Camera,
    /// Local adjustments: each a mask and a look of its own, blended
    /// into the global look where the mask is.
    pub adjustments: Vec<Adjustment>,
    /// Spots and strokes healed or cloned, on the developed picture
    /// before its look.
    pub retouch: retouch::Retouch,
}

/// The part of an edit that is a look: what the viewport and the
/// export apply after the develop, and what a mask can carry a
/// version of.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    pub light: Light,
    pub curves: Curves,
    pub mixer: Mixer,
    #[serde(alias = "colour")]
    pub color: Color,
    pub grading: Grading,
    pub tint: Tint,
    /// A mask's own white balance, on the developed picture before
    /// the rest of the look ([`white`]). Off on the global look, which
    /// never reads it: the picture's white is the edit's, in the
    /// develop. Left out of a sidecar while off with no light kept.
    ///
    /// [`white`]: crate::white
    #[serde(skip_serializing_if = "LocalWhite::is_off")]
    pub white_balance: LocalWhite,
    /// A mask's own Texture, Clarity, Structure, and Dehaze, put on at
    /// the finish by its weight from the bands the develop keeps
    /// beside the picture. Never read on the global look, whose
    /// Detail is the edit's own, before the sharpen. Left out of a
    /// sidecar at its default.
    #[serde(skip_serializing_if = "Detail::is_default")]
    pub detail: Detail,
}

/// Which of the detail bands some mask asks the develop for: the
/// texture, clarity, and structure bands, the haze, and the sharpen's
/// delta, each made only when an enabled mask with something in it
/// has that slider off zero under its section's switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DetailWants {
    pub texture: bool,
    pub clarity: bool,
    pub haze: bool,
    pub structure: bool,
    /// The sharpen's delta, made where the develop sharpens.
    pub sharpen: bool,
    /// The picture from before the Detail section, which the range
    /// masks read when that section is on: no band, and made nowhere,
    /// only kept beside the planes ([`Edit::samples_before_detail`]).
    pub sample: bool,
}

impl DetailWants {
    /// Whether any band is wanted at all.
    pub fn any(&self) -> bool {
        self.bands() || self.sharpen
    }

    /// Whether any of the bands made from the picture before the
    /// local contrast is wanted: all but the sharpen's delta.
    pub fn bands(&self) -> bool {
        self.texture || self.clarity || self.haze || self.structure
    }

    /// Whether planes made for these hold every one `other` asks for.
    pub fn covers(&self, other: &DetailWants) -> bool {
        (self.texture || !other.texture)
            && (self.clarity || !other.clarity)
            && (self.haze || !other.haze)
            && (self.structure || !other.structure)
            && (self.sharpen || !other.sharpen)
            && (self.sample || !other.sample)
    }
}

/// A local adjustment: a look where a mask says, with a stable id so
/// the panel and the history can hold on to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adjustment {
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    pub mask: Mask,
    pub look: Look,
    /// Opacity, 0 to 1: how much of the look the mask puts on, the
    /// mask's weight scaled by it after the invert, the feather, and
    /// Refine edge, so half is the look at half strength over the same
    /// region and the overlay is unchanged. Whole, the default, is
    /// left out of the sidecar, so every sidecar written before it
    /// reads and writes as it did. Read through [`Adjustment::strength`].
    #[serde(skip_serializing_if = "is_whole")]
    pub opacity: f32,
}

fn is_whole(v: &f32) -> bool {
    *v == 1.0
}

impl Default for Adjustment {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            enabled: true,
            mask: Mask::default(),
            look: Look::default(),
            opacity: 1.0,
        }
    }
}

/// How many masks a picture may carry: what the viewport's shader and
/// the export draw. A paste that joins masks stops there.
pub const MAX_MASKS: usize = 16;

impl Adjustment {
    /// Its opacity as the finish reads it: held to 0 to 1, and whole
    /// for a value that is not a number, as a file edited by hand may
    /// hold.
    pub fn strength(&self) -> f32 {
        if self.opacity.is_nan() {
            1.0
        } else {
            self.opacity.clamp(0.0, 1.0)
        }
    }

    /// The name a mask carries from New mask until its first shape
    /// lands: "Mask 3".
    pub fn placeholder_name(id: u64) -> String {
        format!("Mask {id}")
    }

    /// The name a new mask with `mask`'s shapes and id `id` is given:
    /// its first shape's and the id ("Subject 3"), the placeholder
    /// with none.
    pub fn new_name(mask: &Mask, id: u64) -> String {
        mask.components.first().map_or_else(
            || Self::placeholder_name(id),
            |c| format!("{} {id}", c.shape.name()),
        )
    }
}

impl Edit {
    /// The edit a picture that is not a raw starts from: the default,
    /// but with the capture sharpening and the lens profile's
    /// corrections (distortion, vignetting, and chromatic aberration)
    /// off, since whatever rendered the file sharpened it once and, a
    /// camera's JPEG or an editor's export alike, most likely corrected
    /// its lens too, with nothing in the file to say so. The lens is
    /// still matched and the section still shows it, so one switch puts
    /// a correction back.
    pub fn for_picture() -> Self {
        Self {
            sharpen: Sharpen {
                enabled: false,
                ..Sharpen::default()
            },
            lens: Lens {
                distortion: false,
                vignetting: false,
                chromatic_aberration: false,
                ..Lens::default()
            },
            ..Self::default()
        }
    }

    /// Every edit a picture that is not a raw counts as untouched at:
    /// [`Edit::for_picture`], and the same with the lens profile's
    /// corrections on, which is what `for_picture` was before they
    /// started off. A sidecar an older build wrote for a picture only
    /// rated holds the second; it develops as it says, but nobody
    /// edited it. The one rule for the edited mark and the history's
    /// Original row; the schema migration takes the second alone, the
    /// only one a sidecar from before it can hold.
    pub fn picture_defaults() -> [Self; 2] {
        [
            Self::for_picture(),
            Self {
                lens: Lens::default(),
                ..Self::for_picture()
            },
        ]
    }

    /// Every edit a raw counts as untouched at: the default, and the
    /// same with the lens section off, which is what a fresh edit
    /// starts from with lens corrections off for new edits (the
    /// editor's setting). The one rule for the edited mark, the
    /// history's Original row, and a learned blend still waiting for
    /// its ISO, as [`Edit::picture_defaults`] is for a picture.
    pub fn raw_defaults() -> [Self; 2] {
        [Self::default(), Self::raw_without_lens()]
    }

    /// A raw's default with the lens section off: a fresh edit's start
    /// with lens corrections off for new edits.
    pub fn raw_without_lens() -> Self {
        Self {
            lens: Lens {
                enabled: false,
                ..Lens::default()
            },
            ..Self::default()
        }
    }

    /// The color mixer as the picture takes it: nothing at all while
    /// the black and white is on. A mono conversion has the last word
    /// on hue and chroma, so two of the mixer's three sliders could
    /// not reach the picture anyway, and the third, luminance, is a
    /// second set of band gains over the conversion's own — the same
    /// control twice, in two places, with no way to tell from the
    /// picture which one did it. So the black and white takes the
    /// section over: the switch and the sliders keep what they say
    /// and come back untouched when it goes off again, and the panel
    /// shows the section dimmed with its switch off while it is on.
    /// A local adjustment's mixer is its mask's and is not touched.
    pub fn acting_mixer(&self) -> Mixer {
        if self.bw.enabled {
            Mixer {
                enabled: false,
                ..Mixer::default()
            }
        } else {
            self.mixer
        }
    }

    /// The global look.
    pub fn look(&self) -> Look {
        Look {
            light: self.light,
            curves: self.curves.clone(),
            mixer: self.mixer,
            color: self.color,
            grading: self.grading,
            tint: self.tint,
            white_balance: LocalWhite::Off,
            detail: Detail::default(),
        }
    }

    pub fn set_look(&mut self, look: Look) {
        // The zero the exposure rests at is the edit's, not the look's:
        // a look read off the panel's sliders knows nothing of it.
        let baseline = self.light.baseline;
        self.light = look.light;
        self.light.baseline = baseline;
        self.curves = look.curves;
        self.mixer = look.mixer;
        self.color = look.color;
        self.grading = look.grading;
        self.tint = look.tint;
        // `look.white_balance` is a mask's alone: the picture's white
        // is `white_balance`, in the develop. So is `look.detail`: the
        // picture's is `detail`, before the sharpen.
    }

    /// The detail bands the masks ask for ([`DetailWants`]). A mask at
    /// no opacity asks for none, as one switched off does.
    pub fn detail_wants(&self) -> DetailWants {
        let mut wants = DetailWants::default();
        for a in self.adjustments.iter().filter(|a| {
            a.enabled && a.strength() > 0.0 && !a.mask.is_empty() && a.look.detail.enabled
        }) {
            let d = &a.look.detail;
            wants.texture |= d.texture != 0.0;
            wants.clarity |= d.clarity != 0.0;
            wants.structure |= d.structure != 0.0;
            wants.haze |= d.dehaze != 0.0;
            // A share of the sharpen, never less than none: with the
            // picture's own off, only a positive one has a delta to
            // read.
            wants.sharpen |= if self.sharpen.enabled {
                d.sharpen != 0.0
            } else {
                d.sharpen > 0.0
            };
        }
        wants.sample = self.samples_before_detail();
        wants
    }

    /// Whether the capture sharpen reads what is flat from a guide, the
    /// picture before the Detail section's local contrast: where
    /// Texture, Clarity, or Structure runs and a sharpen follows, the
    /// picture's own or a mask's.
    pub fn sharpen_reads_guide(&self) -> bool {
        self.detail.options().is_some()
            && (self.sharpen.options().is_some() || self.detail_wants().sharpen)
    }

    /// Whether Midtones moves anything: the picture's own slider or a
    /// mask's set.
    pub fn sets_midtones(&self) -> bool {
        self.light.tone.midtones != 0.0
            || self
                .adjustments
                .iter()
                .any(|a| a.look.light.tone.midtones != 0.0)
    }

    /// Whether the range masks read a picture of their own, the one
    /// from before the Detail section: some mask has a Luminance or
    /// Color shape and the section does something. Switched off or on,
    /// since a switched-off adjustment's mask can still be shown.
    /// Otherwise they read the developed picture, which with the
    /// section doing nothing differs from it by the capture sharpen
    /// alone.
    pub fn samples_before_detail(&self) -> bool {
        (self.detail.options().is_some() || self.detail.dehaze_options().is_some())
            && self
                .adjustments
                .iter()
                .take(MAX_MASKS)
                .any(|a| a.mask.reads_picture())
    }

    /// The look of a target: an adjustment by index, or the global
    /// look for none (or one that is gone).
    pub fn target_look(&self, target: Option<usize>) -> Look {
        match target.and_then(|i| self.adjustments.get(i)) {
            Some(a) => a.look.clone(),
            None => self.look(),
        }
    }

    pub fn set_target_look(&mut self, target: Option<usize>, look: Look) {
        match target.and_then(|i| self.adjustments.get_mut(i)) {
            Some(a) => a.look = look,
            None => self.set_look(look),
        }
    }

    /// An id no adjustment has.
    pub fn next_id(&self) -> u64 {
        self.adjustments.iter().map(|a| a.id).max().unwrap_or(0) + 1
    }

    /// A copy of mask `i` just below it, its shapes and its look the
    /// same, under an id of its own and the name a new mask with its
    /// shapes is given; where the copy went. None for no such mask.
    pub fn duplicate_mask(&mut self, i: usize) -> Option<usize> {
        let source = self.adjustments.get(i)?;
        let id = self.next_id();
        let copy = Adjustment {
            id,
            name: Adjustment::new_name(&source.mask, id),
            ..source.clone()
        };
        self.adjustments.insert(i + 1, copy);
        Some(i + 1)
    }
}

impl Default for Edit {
    fn default() -> Self {
        Self {
            version: VERSION,
            frame_pending: false,
            light: Light::default(),
            display_curve: DisplayCurve::default(),
            white_balance: WhiteBalance::default(),
            noise: Noise::default(),
            detail: Detail::default(),
            sharpen: Sharpen::default(),
            output_sharpen: OutputSharpen::default(),
            curves: Curves::default(),
            mixer: Mixer::default(),
            color: Color::default(),
            bw: BlackWhite::default(),
            grading: Grading::default(),
            tint: Tint::default(),
            look_lut: LookLut::default(),
            lens: Lens::default(),
            geometry: Geometry::default(),
            vignette: vignette::Vignette::default(),
            grain: grain::Grain::default(),
            adjustments: Vec::new(),
            demosaic: Demosaic::default(),
            camera: Camera::default(),
            retouch: retouch::Retouch::default(),
        }
    }
}

impl Edit {
    /// The engine's settings for this edit, with the camera profile it
    /// names read through `loader`. Exposure and the tone curve are not
    /// the engine's; see [`Light`].
    pub fn settings(&self, loader: &dyn Loader) -> DevelopSettings {
        DevelopSettings {
            white_point: self.white_balance.white_point(),
            // Read here, where every consumer's develop passes, so the
            // viewport and the export cannot disagree about it. The
            // loader reads the file once and keeps it (see [`camera`]).
            profile: self.camera.profile(loader),
            profile_look: self.camera.applies_look_table(),
            demosaic: self.demosaic.method(),
            denoise: self.noise.profiled_runs().then(|| DenoiseOptions {
                strength: Strength::Fixed(self.noise.strength),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Where a DCP's look table is read, in stops over the develop
    /// ([`DevelopSettings::look_baseline`]): what this edit's default
    /// develop brightens a frame from this make and model by, the
    /// baseline it was made against else the body's
    /// ([`crate::baseline`]), and a merge's `headroom` on top
    /// ([`crate::baseline::headroom`]), as the render's own baseline
    /// has it.
    pub fn look_baseline(&self, make: &str, model: &str, headroom: f32) -> f32 {
        self.light
            .baseline
            .unwrap_or_else(|| crate::baseline::for_make_model(make, model))
            + headroom
    }

    /// This edit for a frame turned `quarters` quarter turns
    /// clockwise under it, so every part of it still means the same
    /// pixels.
    ///
    /// Two different spaces move, and they move differently. The
    /// geometry is in fractions of the leveled plane and needs no
    /// aspect, so it always turns
    /// ([`geometry::Geometry::under_turned_source`]). The masks and
    /// the repair patches are in units of the developed picture's
    /// *width*, both ways, so their map depends on the picture's
    /// shape: `positions` is [`mask::Turned`] for it, and is `None`
    /// when the caller does not know the shape. Then they are left
    /// where they are and the caller says so; [`Edit::placed`] tells
    /// it whether there was anything to leave.
    ///
    /// [`geometry::Geometry::under_turned_source`]: crate::geometry::Geometry::under_turned_source
    pub fn turn(&mut self, quarters: i32, positions: Option<mask::Turned>) {
        self.geometry = self.geometry.under_turned_source(quarters);
        let Some(t) = positions.filter(|t| !t.is_identity()) else {
            return;
        };
        for a in &mut self.adjustments {
            a.mask.turn(t);
        }
        self.retouch.turn(t);
    }

    /// Whether anything here is placed on the picture itself — a
    /// mask's shape, a repair patch — and so has to be carried round
    /// by a turn of the frame rather than merely surviving it.
    pub fn placed(&self) -> bool {
        self.adjustments
            .iter()
            .any(|a| !a.mask.components.is_empty())
            || !self.retouch.patches.is_empty()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("an edit serializes")
    }

    pub fn from_json(json: &str) -> Result<Self> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        Ok(serde_json::from_value(migrate(value)?)?)
    }
}

/// `onto` with every leaf that differs between `before` and
/// `touched` taking `touched`'s value: the one slider moved, and
/// nothing else of `touched`. Arrays are leaves. Through the edit's
/// JSON, which is the one flat view of every field there is.
pub fn overlay_changes(before: &Edit, touched: &Edit, onto: &Edit) -> Result<Edit> {
    use serde_json::Value;
    fn overlay(before: &Value, touched: &Value, onto: &mut Value) {
        match (before, touched) {
            (Value::Object(b), Value::Object(t)) if onto.is_object() => {
                for (key, tv) in t {
                    match b.get(key) {
                        Some(bv) if bv == tv => {}
                        Some(bv) if bv.is_object() && tv.is_object() => {
                            let slot = onto
                                .as_object_mut()
                                .expect("checked")
                                .entry(key.clone())
                                .or_insert_with(|| Value::Object(Default::default()));
                            overlay(bv, tv, slot);
                        }
                        _ => {
                            onto.as_object_mut()
                                .expect("checked")
                                .insert(key.clone(), tv.clone());
                        }
                    }
                }
                // A key `before` has and `touched` does not is a field
                // set back to what its type leaves out of the file (a
                // slider to zero, an option to none, a choice to its
                // default), so `onto` takes that too by leaving it
                // out. Removed whole: every such field outside the
                // arrays is one value (a number, a switch, an option,
                // a choice), not a section of its own fields that
                // `onto` might hold apart from `before`. A key absent
                // from both is `onto`'s own and stays.
                let onto = onto.as_object_mut().expect("checked");
                for key in b.keys().filter(|k| !t.contains_key(*k)) {
                    onto.remove(key);
                }
            }
            _ => {
                if before != touched {
                    *onto = touched.clone();
                }
            }
        }
    }
    // A section left out of the file whole at its default is written
    // out in all three, so that one of its fields set back to the
    // default is a leaf like any other and not the whole section of
    // `onto` taken away with it.
    let json = |e: &Edit| -> Result<Value> {
        let mut v: Value = serde_json::from_str(&e.to_json())?;
        if let Value::Object(m) = &mut v {
            m.insert(
                "output_sharpen".into(),
                serde_json::to_value(e.output_sharpen)?,
            );
        }
        Ok(v)
    };
    let before = json(before)?;
    let touched = json(touched)?;
    let mut result = json(onto)?;
    overlay(&before, &touched, &mut result);
    Edit::from_json(&serde_json::to_string(&result)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Learned, Tone};
    use greycard_core::color::WhitePoint;
    use greycard_core::develop::dehaze::DehazeOptions;

    fn masked(detail: Detail) -> Edit {
        let mut edit = Edit::default();
        edit.adjustments.push(Adjustment {
            id: 1,
            name: "Skin".into(),
            mask: mask::Mask {
                components: vec![mask::Component::default()],
                ..Default::default()
            },
            ..Default::default()
        });
        edit.adjustments[0].look.detail = detail;
        edit
    }

    /// A mask's Opacity in the sidecar: whole is left out, so the file
    /// and the develop key are what they were, and a sidecar from
    /// before it reads whole; set, it is written, read back, and
    /// another develop; and a value a hand put out of range reads held.
    #[test]
    fn a_masks_opacity_round_trips_absent_whole_and_set() {
        let mut e = masked(Detail::default());
        e.adjustments[0].look.light.exposure = 1.0;
        let json: serde_json::Value = serde_json::from_str(&e.to_json()).unwrap();
        assert!(
            json["adjustments"][0].get("opacity").is_none(),
            "whole is left out: {json}"
        );
        let read = Edit::from_json(&json.to_string()).unwrap();
        assert_eq!(read.adjustments[0].opacity, 1.0);
        assert_eq!(read, e);
        let before = e.develop_key_with(&[], None);
        e.adjustments[0].opacity = 0.7;
        let json: serde_json::Value = serde_json::from_str(&e.to_json()).unwrap();
        let written = json["adjustments"][0]["opacity"].as_f64();
        assert!(written.is_some_and(|v| (v - 0.7).abs() < 1e-6), "{json}");
        assert_eq!(Edit::from_json(&e.to_json()).unwrap(), e);
        assert_ne!(e.develop_key_with(&[], None), before);
        e.adjustments[0].opacity = 1.0;
        assert_eq!(e.develop_key_with(&[], None), before);
        for (stored, read) in [(1.5, 1.0), (-0.2, 0.0), (0.25, 0.25)] {
            e.adjustments[0].opacity = stored;
            assert_eq!(e.adjustments[0].strength(), read);
        }
        e.adjustments[0].opacity = f32::NAN;
        assert_eq!(e.adjustments[0].strength(), 1.0);
    }

    /// A mask's Detail at no opacity asks for no bands, as one switched
    /// off does, so the develop is the one it would be.
    #[test]
    fn a_mask_at_no_opacity_asks_for_no_detail() {
        let mut e = masked(Detail {
            enabled: true,
            texture: 0.5,
            ..Default::default()
        });
        assert!(e.detail_wants().texture);
        e.adjustments[0].opacity = 0.0;
        let mut off = e.clone();
        off.adjustments[0].opacity = 1.0;
        off.adjustments[0].enabled = false;
        assert_eq!(e.detail_wants(), off.detail_wants());
        assert!(!e.detail_wants().texture);
    }

    /// A picture starts with the capture sharpen and the lens
    /// profile's three corrections off, everything else a raw's:
    /// the section and its profile switch stay on, so one correction
    /// switched on is all it takes, and that is kept through a save.
    #[test]
    fn a_picture_starts_with_its_lens_corrections_off() {
        let picture = Edit::for_picture();
        let lens = &picture.lens;
        assert!(!lens.distortion && !lens.vignetting && !lens.chromatic_aberration);
        assert!(lens.enabled && lens.profile && lens.chosen.is_from_file());
        assert!(!lens.wants_profile());
        assert!(!picture.sharpen.enabled);
        // Nothing else differs from a raw's default, and a raw's own is
        // unchanged.
        let raw = Edit::default();
        assert!(raw.lens.distortion && raw.lens.vignetting && raw.lens.chromatic_aberration);
        assert_eq!(
            Edit {
                lens: raw.lens.clone(),
                sharpen: raw.sharpen,
                ..picture.clone()
            },
            raw
        );
        // One switched on is what the edit then holds, read back.
        let mut on = picture;
        on.lens.vignetting = true;
        assert!(on.lens.wants_profile());
        assert_eq!(Edit::from_json(&on.to_json()).unwrap(), on);
    }

    /// A mask's Detail is written only when it is set, reads back
    /// whole, and a sidecar without it reads as none.
    #[test]
    fn a_masks_detail_is_in_the_sidecar_only_when_set() {
        let plain = masked(Detail::default());
        let look = serde_json::to_value(&plain.adjustments[0].look).unwrap();
        assert!(look.get("detail").is_none(), "{look}");
        let set = masked(Detail {
            texture: -0.5,
            dehaze: 0.25,
            ..Detail::default()
        });
        let read = Edit::from_json(&set.to_json()).unwrap();
        assert_eq!(read, set);
        let old = Edit::from_json(
            r#"{"version":4,"adjustments":[{"id":1,"look":{"light":{"exposure":0.5}}}]}"#,
        )
        .unwrap();
        assert_eq!(old.adjustments[0].look.detail, Detail::default());
        assert_eq!(old.adjustments[0].look.light.exposure, 0.5);
    }

    /// A build from before reads a look with a Detail in it as the
    /// look without: the look has no `deny_unknown_fields`.
    #[test]
    fn a_build_before_reads_a_masks_detail_as_nothing() {
        #[derive(Debug, Deserialize)]
        #[serde(default)]
        #[allow(dead_code)]
        #[derive(Default)]
        struct Before {
            light: Light,
            curves: Curves,
            mixer: Mixer,
            color: Color,
            grading: Grading,
            tint: Tint,
            white_balance: LocalWhite,
        }
        let set = masked(Detail {
            texture: -0.5,
            ..Detail::default()
        });
        let json = serde_json::to_value(&set.adjustments[0].look).unwrap();
        assert!(json.get("detail").is_some(), "{json}");
        let before: Before = serde_json::from_value(json).unwrap();
        assert_eq!(before.light, set.adjustments[0].look.light);
        assert_eq!(before.tint, set.adjustments[0].look.tint);
    }

    /// The global look never carries one: the picture's Detail is the
    /// edit's own, and setting the global look leaves it alone.
    #[test]
    fn the_global_look_never_reads_a_detail() {
        let mut edit = Edit::default();
        edit.detail.texture = 0.4;
        assert_eq!(edit.look().detail, Detail::default());
        let mut look = edit.look();
        look.detail.texture = -1.0;
        edit.set_look(look);
        assert_eq!(edit.detail.texture, 0.4);
    }

    /// Which bands the masks ask for.
    #[test]
    fn the_bands_wanted_follow_the_masks_that_ask() {
        assert!(!Edit::default().detail_wants().any());
        let quiet = masked(Detail::default());
        assert!(!quiet.detail_wants().any());
        let texture = masked(Detail {
            texture: -0.5,
            ..Detail::default()
        });
        assert_eq!(
            texture.detail_wants(),
            DetailWants {
                texture: true,
                ..Default::default()
            }
        );
        // Off by the section's switch, by the adjustment's, or with
        // nothing in the mask: nothing asked.
        for off in [
            {
                let mut e = texture.clone();
                e.adjustments[0].look.detail.enabled = false;
                e
            },
            {
                let mut e = texture.clone();
                e.adjustments[0].enabled = false;
                e
            },
            {
                let mut e = texture.clone();
                e.adjustments[0].mask.components.clear();
                e
            },
        ] {
            assert!(!off.detail_wants().any(), "{off:?}");
        }
        let all = masked(Detail {
            texture: 0.1,
            clarity: 0.2,
            structure: 0.4,
            dehaze: -0.3,
            ..Detail::default()
        });
        assert_eq!(
            all.detail_wants(),
            DetailWants {
                texture: true,
                clarity: true,
                haze: true,
                structure: true,
                sharpen: false,
                sample: false,
            }
        );
        let structure = masked(Detail {
            structure: -0.2,
            ..Detail::default()
        });
        assert_eq!(
            structure.detail_wants(),
            DetailWants {
                structure: true,
                ..Default::default()
            }
        );
        assert!(all.detail_wants().covers(&structure.detail_wants()));
        assert!(!quiet.detail_wants().covers(&structure.detail_wants()));
        // A mask's Sharpen asks for the sharpen's delta, and no band.
        let sharpen = masked(Detail {
            sharpen: -1.0,
            ..Detail::default()
        });
        let wants = sharpen.detail_wants();
        assert_eq!(
            wants,
            DetailWants {
                sharpen: true,
                ..Default::default()
            }
        );
        assert!(wants.any() && !wants.bands());
        assert!(!all.detail_wants().covers(&wants));
        // With the picture's sharpen off, a negative share asks for
        // nothing, a positive one for the delta.
        let mut off = sharpen.clone();
        off.sharpen.enabled = false;
        assert!(!off.detail_wants().any());
        off.adjustments[0].look.detail.sharpen = 0.5;
        assert!(off.detail_wants().sharpen);
    }

    /// The range masks read the picture from before the Detail section
    /// when one of them is there and the section does something; a
    /// switched-off adjustment's counts, since its mask can be shown.
    #[test]
    fn the_sample_is_asked_for_by_a_range_shape_under_the_detail_section() {
        let mut edit = masked(Detail::default());
        edit.adjustments[0].mask.components[0].shape = mask::Shape::LUMINANCE;
        assert!(!edit.detail_wants().sample);
        edit.detail.dehaze = 0.4;
        assert!(edit.detail_wants().sample);
        assert!(!edit.detail_wants().any());
        edit.adjustments[0].enabled = false;
        assert!(edit.samples_before_detail());
        edit.detail.enabled = false;
        assert!(!edit.samples_before_detail());
        edit.detail.enabled = true;
        edit.adjustments[0].mask.components[0] = mask::Component::default();
        assert!(!edit.samples_before_detail());
    }

    /// A control reached in culling: its change alone goes over the
    /// culled frame's edit, and nothing else of the panel's, which
    /// was another frame's.
    #[test]
    fn a_control_moved_in_culling_goes_over_the_frames_own_edit() {
        // The panel showed the previous frame's edit: warm, with a
        // contrast and a curve of its own.
        let mut before = Edit::default();
        before.light.exposure = 1.0;
        before.light.tone.contrast = 1.4;
        before.white_balance = WhiteBalance::Custom {
            temperature: 3200.0,
            tint: 0.01,
        };
        before.curves.rgb = vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]];
        // The culled frame's own edit: cool, its own exposure, a mask.
        let mut frame = Edit::default();
        frame.light.exposure = -0.5;
        frame.white_balance = WhiteBalance::Custom {
            temperature: 6500.0,
            tint: -0.02,
        };
        frame.adjustments.push(Adjustment {
            id: 1,
            name: "Sky".into(),
            enabled: true,
            mask: mask::Mask::default(),
            look: Look::default(),
            opacity: 1.0,
        });
        // One slider moved: the shadows.
        let mut touched = before.clone();
        touched.light.tone.shadows = 0.7;
        let merged = overlay_changes(&before, &touched, &frame).unwrap();
        let mut want = frame.clone();
        want.light.tone.shadows = 0.7;
        assert_eq!(merged, want);
        // A whole array moved (a curve's points): the array comes
        // across, and only it.
        let mut touched = before.clone();
        touched.curves.rgb = vec![[0.0, 0.1], [1.0, 1.0]];
        let merged = overlay_changes(&before, &touched, &frame).unwrap();
        let mut want = frame.clone();
        want.curves.rgb = vec![[0.0, 0.1], [1.0, 1.0]];
        assert_eq!(merged, want);
        // Nothing moved: the frame's edit as it was.
        assert_eq!(overlay_changes(&before, &before, &frame).unwrap(), frame);
    }

    /// A field left out of the file at its default and set back to it
    /// in culling (a double-click, a step to zero) goes over the frame's
    /// own value, as any other change does; a field neither side
    /// writes is the frame's own and stays.
    #[test]
    fn a_reset_to_what_the_file_leaves_out_goes_over_the_frame() {
        let mut frame = Edit::default();
        frame.light.tone.midtones = 0.8;
        frame.detail.structure = 0.4;
        frame.detail.sharpen = -0.3;
        frame.noise.learned_chroma_strength = 0.5;
        frame.lens.chosen = crate::lens::LensChoice::Database("Frame's lens".into());
        frame.tint.enabled = false;
        frame.output_sharpen.amount = 0.8;
        frame.output_sharpen.radius = 2.0;
        let mut before = Edit::default();
        before.output_sharpen.amount = 0.5;
        before.light.tone.midtones = 0.5;
        before.detail.structure = 0.6;
        before.detail.sharpen = 0.2;
        before.noise.learned_chroma_strength = 0.7;
        before.lens.chosen = crate::lens::LensChoice::Database("Panel's lens".into());
        before.tint.enabled = false;
        for (what, reset) in [
            (
                "midtones",
                (|e: &mut Edit| e.light.tone.midtones = 0.0) as fn(&mut Edit),
            ),
            ("structure", |e| e.detail.structure = 0.0),
            ("sharpen", |e| e.detail.sharpen = 0.0),
            ("chroma", |e| e.noise.learned_chroma_strength = 1.0),
            ("lens", |e| {
                e.lens.chosen = crate::lens::LensChoice::FromFile
            }),
            ("tint", |e| e.tint.enabled = true),
            ("output sharpen", |e| e.output_sharpen.amount = 0.0),
        ] {
            let mut touched = before.clone();
            reset(&mut touched);
            let merged = overlay_changes(&before, &touched, &frame).unwrap();
            let mut want = frame.clone();
            reset(&mut want);
            assert_eq!(merged, want, "{what}");
        }
        // Absent from both: the panel's edit never set them, a shadows
        // step moves only the shadows, and the frame keeps its own.
        let plain = Edit::default();
        let mut touched = plain.clone();
        touched.light.tone.shadows = 0.3;
        let merged = overlay_changes(&plain, &touched, &frame).unwrap();
        let mut want = frame.clone();
        want.light.tone.shadows = 0.3;
        assert_eq!(merged, want);
        // A white balance's offset taken off, As shot kept.
        let before = Edit {
            white_balance: WhiteBalance::Offset {
                mired: 10.0,
                tint: 0.0,
            },
            ..Edit::default()
        };
        let frame = Edit {
            white_balance: WhiteBalance::Offset {
                mired: -20.0,
                tint: 0.01,
            },
            ..Edit::default()
        };
        let merged = overlay_changes(&before, &Edit::default(), &frame).unwrap();
        assert_eq!(merged.white_balance, WhiteBalance::AsShot);
    }

    #[test]
    fn adjustments_round_trip_and_the_target_look_is_theirs() {
        use mask::{Component, Shape};
        let mut edit = Edit::default();
        edit.light.exposure = 0.5;
        let mut local = Look::default();
        local.light.exposure = -1.0;
        local.mixer.saturation[2] = 0.4;
        edit.adjustments.push(Adjustment {
            id: edit.next_id(),
            name: "Linear 1".into(),
            mask: Mask {
                components: vec![Component {
                    shape: Shape::Linear {
                        from: [0.5, 0.0],
                        to: [0.5, 0.4],
                    },
                    ..Default::default()
                }],
                invert: false,
                ..Default::default()
            },
            look: local.clone(),
            ..Default::default()
        });
        assert_eq!(edit.adjustments[0].id, 1);
        assert_eq!(edit.next_id(), 2);
        let back = Edit::from_json(&edit.to_json()).unwrap();
        assert_eq!(back, edit);
        assert_eq!(back.target_look(Some(0)), local);
        assert_eq!(back.target_look(None).light.exposure, 0.5);
        // A target that is gone is the global look; setting it sets that.
        assert_eq!(back.target_look(Some(3)), back.look());
        let mut moved = back.clone();
        moved.set_target_look(Some(0), Look::default());
        assert_eq!(moved.adjustments[0].look, Look::default());
        moved.set_target_look(None, local.clone());
        assert_eq!(moved.light.exposure, -1.0);
        // Old sidecars have none.
        let old: Edit = serde_json::from_str(r#"{"version":1}"#).unwrap();
        assert!(old.adjustments.is_empty());
    }

    fn masked_twice() -> Edit {
        use mask::{Component, Shape};
        let mut edit = Edit::default();
        for (id, name, shape) in [
            (1, "Subject 1", Shape::Subject {}),
            (4, "Lamp", Shape::LUMINANCE),
        ] {
            let mut look = Look::default();
            look.light.exposure = id as f32 / 10.0;
            edit.adjustments.push(Adjustment {
                id,
                name: name.into(),
                enabled: id == 1,
                mask: Mask {
                    components: vec![Component {
                        shape,
                        ..Default::default()
                    }],
                    invert: id == 4,
                    ..Default::default()
                },
                look,
                opacity: 1.0,
            });
        }
        edit
    }

    #[test]
    fn a_duplicate_lands_below_its_mask_under_a_new_id_and_name() {
        let mut edit = masked_twice();
        assert_eq!(edit.duplicate_mask(0), Some(1));
        let names: Vec<&str> = edit.adjustments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Subject 1", "Subject 5", "Lamp"]);
        let (source, copy) = (&edit.adjustments[0], &edit.adjustments[1]);
        assert_eq!(copy.id, 5);
        assert_eq!((&copy.mask, &copy.look), (&source.mask, &source.look));
        assert_eq!(copy.enabled, source.enabled);
        // A typed name is not carried: the copy is named as a new mask
        // with its shapes is, its invert and switch kept.
        assert_eq!(edit.duplicate_mask(2), Some(3));
        let copy = &edit.adjustments[3];
        assert_eq!((copy.id, copy.name.as_str()), (6, "Luminance 6"));
        assert!(copy.mask.invert && !copy.enabled);
        // With no shape yet, the placeholder; past the end, nothing.
        edit.adjustments[0].mask.components.clear();
        edit.duplicate_mask(0);
        assert_eq!(edit.adjustments[1].name, "Mask 7");
        let before = edit.clone();
        assert_eq!(edit.duplicate_mask(9), None);
        assert_eq!(edit, before);
    }

    #[test]
    fn round_trips_and_defaults() {
        let mut edit = Edit::default();
        edit.light.exposure = 0.7;
        edit.white_balance = WhiteBalance::Custom {
            temperature: 3200.0,
            tint: -0.004,
        };
        edit.noise = Noise {
            enabled: true,
            profiled: true,
            strength: 2.0,
            learned: Learned::Best,
            learned_strength: 0.8,
            learned_chroma_strength: 0.25,
        };
        edit.demosaic = Demosaic::Rcd;
        let json = edit.to_json();
        assert!(json.contains("\"mode\": \"custom\""), "{json}");
        assert!(json.contains("\"demosaic\": \"rcd\""), "{json}");
        assert!(json.contains("\"learned\": \"best\""), "{json}");
        assert_eq!(Edit::from_json(&json).unwrap(), edit);
        // The output sharpen is left out at its default, and kept when set.
        assert!(!json.contains("output_sharpen"), "{json}");
        let mut sharpened = edit.clone();
        sharpened.output_sharpen.amount = 0.6;
        let json = sharpened.to_json();
        assert!(json.contains("\"output_sharpen\""), "{json}");
        assert_eq!(Edit::from_json(&json).unwrap(), sharpened);
        // An empty document is the default edit; unknown fields are ignored.
        assert_eq!(Edit::from_json("{}").unwrap(), Edit::default());
        let partial = r#"{"light": {"exposure": -1}, "future_field": 3}"#;
        let e = Edit::from_json(partial).unwrap();
        assert_eq!(e.light.exposure, -1.0);
        assert_eq!(e.light.tone, Tone::default());
        assert_eq!(e.version, VERSION);
    }

    #[test]
    fn the_black_and_white_takes_the_mixer_over_and_gives_it_back() {
        let mut edit = Edit::default();
        edit.mixer.saturation[5] = -0.6;
        edit.mixer.luminance[5] = 0.4;
        assert_eq!(edit.acting_mixer(), edit.mixer);
        edit.bw.enabled = true;
        let acting = edit.acting_mixer();
        assert!(!acting.enabled);
        assert!(acting.is_identity(), "no band reaches the picture");
        assert_eq!(acting.luminance[5], 0.0);
        // The settings are kept, and come back as they were.
        assert_eq!(edit.mixer.saturation[5], -0.6);
        assert!(edit.mixer.enabled);
        edit.bw.enabled = false;
        assert_eq!(edit.acting_mixer(), edit.mixer);
        // And a mixer switched off by hand stays off under it.
        edit.mixer.enabled = false;
        edit.bw.enabled = true;
        assert!(!edit.acting_mixer().enabled);
        edit.bw.enabled = false;
        assert!(!edit.acting_mixer().enabled);
    }

    #[test]
    fn settings_follow_the_edit() {
        let mut edit = Edit::default();
        assert!(edit.settings(&crate::NoStores).denoise.is_none());
        assert!(matches!(
            edit.settings(&crate::NoStores).white_point,
            WhitePoint::AsShot
        ));
        edit.noise.profiled = true;
        edit.white_balance = WhiteBalance::Custom {
            temperature: 5000.0,
            tint: 0.0,
        };
        let s = edit.settings(&crate::NoStores);
        assert!(
            matches!(s.denoise, Some(DenoiseOptions { strength: Strength::Fixed(v), .. }) if v == 1.5)
        );
        assert!(matches!(s.white_point, WhitePoint::TempTint(_)));
        let mut other = edit.clone();
        other.detail.dehaze = 0.5;
        // The engine's settings stop before the dehaze, which is a
        // stage of its own after the lens.
        assert_eq!(
            edit.settings(&crate::NoStores),
            other.settings(&crate::NoStores)
        );
        assert!(edit.detail.dehaze_options().is_none());
        assert_eq!(
            other.detail.dehaze_options(),
            Some(DehazeOptions { amount: 0.5 })
        );
        // The section's switch takes the dehaze with it.
        other.detail.enabled = false;
        assert!(other.detail.dehaze_options().is_none());
        assert!(other.detail.options().is_none());
    }

    /// Found by fuzzing: without serde_json's `float_roundtrip`, these
    /// white balance tints (from a Lightroom XMP, a catalog's table,
    /// and a RawTherapee profile) read back a unit in the last place
    /// off what was written.
    #[test]
    fn fuzz_a_white_balance_reads_back_to_the_last_bit() {
        for tint in [
            0.014999999664723873,
            0.025333333760499954,
            0.0013249980323305319,
        ] {
            let edit = Edit {
                white_balance: WhiteBalance::Custom {
                    temperature: 4800.0,
                    tint,
                },
                ..Edit::default()
            };
            assert_eq!(Edit::from_json(&edit.to_json()).unwrap(), edit, "{tint}");
        }
    }

    #[test]
    fn an_edit_written_before_the_american_spelling_still_loads() {
        // "colour" was the field name before the 2026-09-19 rename; an
        // alias keeps sidecars written before it readable.
        let json = r#"{"colour": {"enabled": true, "saturation": 0.4, "vibrance": 0.1}}"#;
        let edit: Edit = serde_json::from_str(json).unwrap();
        assert_eq!(edit.color.saturation, 0.4);
        assert_eq!(edit.color.vibrance, 0.1);
        let look: Look = serde_json::from_str(json).unwrap();
        assert_eq!(look.color.saturation, 0.4);
    }
}
