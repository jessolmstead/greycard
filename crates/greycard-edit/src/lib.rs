//! The edit: everything a development needs beyond the file, as a typed
//! struct in physical units (stops, kelvin, Duv), with a schema version
//! and a sidecar that keeps its history and the frame's meta.
//!
//! The engine takes no edit schema (`greycard-core` must not depend on
//! one); this crate depends on the engine and turns an [`Edit`] into the
//! engine's settings. What the viewport does with an edit (exposure, the
//! tone curve) is the consumer's, read from the same struct.
//!
//! Rules: every field has a default, so a sidecar written by an older
//! build loads and a newer build's unknown fields are ignored; a field
//! that changes meaning changes the [`VERSION`] and gets a migration in
//! [`migrate`]; a sidecar from a newer version than this build knows is
//! refused rather than half read.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub mod brush;
pub mod bw;
pub mod camera;
pub mod color;
pub mod curve;
pub mod geometry;
pub mod grading;
pub mod grain;
pub mod history;
pub mod lens;
pub mod lightroom;
pub mod look;
pub mod mask;
pub mod meta;
pub mod mixer;
pub mod preset;
pub mod retouch;
pub mod tint;
pub mod vignette;
// XMP interop for the meta section (§72): read and write the
// standard fields in an `.xmp` beside the frame.
pub mod xmp;
pub use bw::BlackWhite;
pub use camera::Camera;
pub use color::Color;
pub use curve::{CurveLut, Curves, Parametric};
pub use geometry::Geometry;
pub use grading::Grading;
pub use grain::Grain;
pub use history::describe;
pub use lens::Lens;
pub use look::LookLut;
pub use mask::Mask;
pub use meta::Meta;
pub use mixer::Mixer;
pub use preset::{Preset, Section};
pub use tint::Tint;
pub use vignette::Vignette;

use greycard_core::TempTint;
use greycard_core::color::WhitePoint;
use greycard_core::develop::dehaze::DehazeOptions;
use greycard_core::develop::denoise::{DenoiseOptions, Strength};
use greycard_core::develop::local_contrast::LocalContrastOptions;
use greycard_core::develop::sharpen::{self, Radius, SharpenOptions, Threshold};
use greycard_core::develop::{DemosaicMethod, DevelopSettings};

/// The schema version this build writes.
pub const VERSION: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sidecar is schema version {0}, this build reads up to {VERSION}")]
    Newer(u32),
    #[error("sidecar: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sidecar: {0}")]
    Io(#[from] std::io::Error),
    #[error("preset: {0}")]
    Preset(String),
    #[error("xmp: {0}")]
    Xmp(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A development, described.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Edit {
    /// Schema version of the struct as written.
    pub version: u32,
    pub light: Light,
    pub white_balance: WhiteBalance,
    pub noise: Noise,
    /// Local contrast, Texture and Clarity, on the developed picture
    /// after the retouch and before the sharpen.
    pub detail: Detail,
    pub sharpen: Sharpen,
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
}

impl Default for Adjustment {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            enabled: true,
            mask: Mask::default(),
            look: Look::default(),
        }
    }
}

impl Edit {
    /// The edit a picture that is not a raw starts from: the default,
    /// but with the capture sharpening off, since the file has been
    /// sharpened once by whatever rendered it.
    pub fn for_picture() -> Self {
        Self {
            sharpen: Sharpen {
                enabled: false,
                ..Sharpen::default()
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
        }
    }

    pub fn set_look(&mut self, look: Look) {
        self.light = look.light;
        self.curves = look.curves;
        self.mixer = look.mixer;
        self.color = look.color;
        self.grading = look.grading;
        self.tint = look.tint;
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
}

impl Default for Edit {
    fn default() -> Self {
        Self {
            version: VERSION,
            light: Light::default(),
            white_balance: WhiteBalance::default(),
            noise: Noise::default(),
            detail: Detail::default(),
            sharpen: Sharpen::default(),
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

/// Brightness and the tone curve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Light {
    /// The section's switch: off, the exposure and the tone curve do
    /// nothing, and stay set for when it is on again.
    pub enabled: bool,
    /// Exposure, in stops.
    pub exposure: f32,
    pub tone: Tone,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            enabled: true,
            exposure: 0.0,
            tone: Tone::default(),
        }
    }
}

impl Light {
    /// What acts: this light, or with the switch off, no exposure and
    /// the tone curve off. Consumers apply this, so the switch is one
    /// test in one place.
    pub fn effective(&self) -> Light {
        if self.enabled {
            *self
        } else {
            Light {
                enabled: false,
                exposure: 0.0,
                tone: Tone {
                    enabled: false,
                    ..self.tone
                },
            }
        }
    }
}

/// The display tone curve: a shoulder over the scene, or none, and
/// the shifts about it. The shifts are by luminance and scale all
/// three channels alike, so a color keeps its hue.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tone {
    pub enabled: bool,
    /// Slope at mid grey relative to the base curve's: 1 leaves it,
    /// above steepens, below flattens.
    pub contrast: f32,
    /// Stops of shift over mid grey, full two and a half stops above
    /// it, a fifth at it, none a stop under it. Read from the region
    /// the pixel sits in and not from the pixel: the tone equalizer,
    /// `finish.rs`. The slider runs ±2.
    pub highlights: f32,
    /// Stops of shift under mid grey, full three and a half stops
    /// below it, none at it. By region too, and ±2.
    pub shadows: f32,
    /// The white point, in stops, pivoted at mid grey: a luminance this
    /// many stops under scene white (2.47 over mid grey) is brought to
    /// scene white, and everything over mid grey with it, by a power on
    /// the luminance; nothing at or under mid grey moves. Positive clips
    /// earlier and stretches the top, negative puts the clip further
    /// out and compresses the top towards mid grey. Global, not by
    /// region: `white_point` in `finish.rs`. The slider runs ±2.
    pub whites: f32,
    /// The black point, ±0.3. Negative crushes: that fraction of mid
    /// grey goes to zero, with scene white held. Positive lifts the
    /// toe: the deep shadows up by as much as 1.2 stops at 0.3, fading
    /// out through the mid-tones (`black_point` in `finish.rs`). Zero
    /// is the scene's own.
    pub blacks: f32,
}

impl Default for Tone {
    fn default() -> Self {
        Self {
            enabled: true,
            contrast: 1.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
        }
    }
}

/// The scene white.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum WhiteBalance {
    /// What the camera recorded.
    #[default]
    AsShot,
    /// A correlated color temperature in kelvin and a tint as Duv,
    /// positive toward green.
    Custom { temperature: f64, tint: f64 },
}

impl WhiteBalance {
    pub fn white_point(&self) -> WhitePoint {
        match *self {
            Self::AsShot => WhitePoint::AsShot,
            Self::Custom { temperature, tint } => WhitePoint::TempTint(TempTint {
                cct: temperature,
                duv: tint,
            }),
        }
    }
}

/// Noise reduction: the engine's profiled denoiser, or the learned
/// one in place of the demosaic and the profiled pass together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Noise {
    /// The section's switch: off, neither denoiser runs.
    pub enabled: bool,
    /// The profiled denoiser; ignored while `learned` is on.
    pub profiled: bool,
    /// The engine's strength; 1.5 is what its benchmark chose.
    pub strength: f32,
    /// The learned denoiser, by tier, or off.
    pub learned: Learned,
    /// How much of the learned answer shows, 0 to 1: it is blended
    /// with the plain demosaic of the same mosaic, so the slider does
    /// not run the network again.
    pub learned_strength: f32,
}

impl Default for Noise {
    fn default() -> Self {
        Self {
            enabled: true,
            profiled: false,
            strength: 1.5,
            learned: Learned::Off,
            learned_strength: 1.0,
        }
    }
}

impl Noise {
    /// The learned tier that runs: the edit's, with the section on.
    pub fn tier(&self) -> Option<&'static str> {
        if self.enabled {
            self.learned.tier()
        } else {
            None
        }
    }

    /// Whether the profiled denoiser runs: the section on, the
    /// profiled pass chosen, and no learned tier standing in for it.
    pub fn profiled_runs(&self) -> bool {
        self.enabled && self.profiled && self.learned.tier().is_none()
    }

    /// The learned denoiser's default blend for a frame at this ISO,
    /// or the old constant (full blend) when the ISO is not known.
    ///
    /// §61: "best" at full blend takes fabric weave and surface
    /// texture off a base-ISO frame that barely needed it; a clean
    /// frame should keep more of its own detail and only lean on the
    /// network once the noise is worth the trade. So the default
    /// ramps with the frame's ISO rather than sitting at one value
    /// for every frame: 0.35 at ISO 200 and below, where the sensor's
    /// own noise is small next to real texture; 1.0 at ISO 3200 and
    /// above, where the network's answer is worth taking whole; a
    /// straight line between the two, but in stops (log2 of the ISO)
    /// rather than in ISO itself, because noise is a stops quantity
    /// the eye and the sensor both see logarithmically, and a linear
    /// ISO ramp would spend nearly its whole range under ISO 800. A
    /// file's own sidecar always overrides this; it only sets the
    /// blend a fresh edit starts with.
    pub fn blend_for_iso(iso: Option<u32>) -> f32 {
        /// The old constant, and what an unknown ISO falls back to.
        const UNKNOWN: f32 = 1.0;
        const LOW_ISO: f32 = 200.0;
        const LOW_BLEND: f32 = 0.35;
        const HIGH_ISO: f32 = 3200.0;
        const HIGH_BLEND: f32 = 1.0;
        let Some(iso) = iso else {
            return UNKNOWN;
        };
        let stops =
            ((iso as f32).max(1.0).log2() - LOW_ISO.log2()) / (HIGH_ISO.log2() - LOW_ISO.log2());
        LOW_BLEND + stops.clamp(0.0, 1.0) * (HIGH_BLEND - LOW_BLEND)
    }
}

/// The learned denoiser's tiers, by the names the model registry uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Learned {
    #[default]
    Off,
    Fast,
    Balanced,
    Best,
}

impl Learned {
    pub const ALL: &[Learned] = &[
        Learned::Off,
        Learned::Fast,
        Learned::Balanced,
        Learned::Best,
    ];

    /// The registry's name for the tier, which is also the serialized
    /// form; "off" for none.
    pub fn name(self) -> &'static str {
        match self {
            Learned::Off => "off",
            Learned::Fast => "fast",
            Learned::Balanced => "balanced",
            Learned::Best => "best",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.name() == name)
    }

    /// The tier's name when one is on.
    pub fn tier(self) -> Option<&'static str> {
        (self != Learned::Off).then(|| self.name())
    }
}

/// Local contrast: the engine's Texture and Clarity, each -1 to 1
/// (the panel shows ±100), negative smoothing instead.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Detail {
    /// The section's switch: off, the sliders do nothing and stay
    /// set for when it is on again.
    pub enabled: bool,
    /// The fine scale.
    pub texture: f32,
    /// The coarse, mid-tone weighted scale.
    pub clarity: f32,
    /// Haze removal, the engine's dark channel prior: -1 to 1 (the
    /// panel shows ±100), how much haze to take out; negative puts
    /// haze in. Zero is nothing. Runs after the local contrast and
    /// before the sharpen, and follows the section's switch.
    pub dehaze: f32,
}

impl Default for Detail {
    fn default() -> Self {
        Self {
            enabled: true,
            texture: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
        }
    }
}

impl Detail {
    /// The engine's options, or none when off or when both sliders
    /// rest, which is the same picture.
    pub fn options(&self) -> Option<LocalContrastOptions> {
        let options = LocalContrastOptions {
            texture: self.texture.clamp(-1.0, 1.0),
            clarity: self.clarity.clamp(-1.0, 1.0),
        };
        (self.enabled && !options.is_identity()).then_some(options)
    }

    /// The dehaze's options, or none when off or at zero.
    pub fn dehaze_options(&self) -> Option<DehazeOptions> {
        (self.enabled && self.dehaze != 0.0).then_some(DehazeOptions {
            amount: self.dehaze.clamp(-1.0, 1.0),
        })
    }
}

/// Capture sharpening: the engine's deconvolution of the lens and
/// demosaic's blur.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sharpen {
    pub enabled: bool,
    /// Take the point spread's width from the mosaic.
    pub auto_radius: bool,
    /// The width otherwise, pixels of standard deviation.
    pub radius: f32,
    pub iterations: u32,
    /// Set the contrast threshold from the picture's flattest patch.
    pub auto_threshold: bool,
    /// The threshold otherwise, a fraction (the panel shows a percentage).
    pub threshold: f32,
}

impl Default for Sharpen {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_radius: true,
            radius: sharpen::DEFAULT_RADIUS,
            iterations: sharpen::DEFAULT_ITERATIONS as u32,
            auto_threshold: true,
            threshold: 0.1,
        }
    }
}

impl Sharpen {
    /// The engine's options, or none when off.
    pub fn options(&self) -> Option<SharpenOptions> {
        self.enabled.then_some(SharpenOptions {
            radius: if self.auto_radius {
                Radius::Auto
            } else {
                Radius::Fixed(self.radius)
            },
            iterations: self.iterations as usize,
            contrast: if self.auto_threshold {
                Threshold::Auto
            } else {
                Threshold::Fixed(self.threshold)
            },
            stop_early: true,
        })
    }
}

/// The demosaic, by the engine's names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Demosaic {
    #[default]
    AmazeVng4,
    RcdVng4,
    Amaze,
    Rcd,
    Vng4,
    Bilinear,
}

impl Demosaic {
    pub const ALL: &[Demosaic] = &[
        Demosaic::AmazeVng4,
        Demosaic::RcdVng4,
        Demosaic::Amaze,
        Demosaic::Rcd,
        Demosaic::Vng4,
        Demosaic::Bilinear,
    ];

    pub fn method(self) -> DemosaicMethod {
        match self {
            Demosaic::AmazeVng4 => DemosaicMethod::AmazeVng4,
            Demosaic::RcdVng4 => DemosaicMethod::RcdVng4,
            Demosaic::Amaze => DemosaicMethod::Amaze,
            Demosaic::Rcd => DemosaicMethod::Rcd,
            Demosaic::Vng4 => DemosaicMethod::Vng4,
            Demosaic::Bilinear => DemosaicMethod::Bilinear,
        }
    }

    /// The engine's name for it, which is also the serialized form.
    pub fn name(self) -> &'static str {
        self.method().name()
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|d| d.name() == name)
    }
}

impl Edit {
    /// The engine's settings for this edit. Exposure and the tone curve
    /// are not the engine's; see [`Light`].
    pub fn settings(&self) -> DevelopSettings {
        DevelopSettings {
            white_point: self.white_balance.white_point(),
            // Read here, where every consumer's develop passes, so the
            // viewport and the export cannot disagree about it. The
            // file is read once and kept (see [`camera`]).
            profile: self.camera.profile(),
            demosaic: self.demosaic.method(),
            denoise: self.noise.profiled_runs().then(|| DenoiseOptions {
                strength: Strength::Fixed(self.noise.strength),
                ..Default::default()
            }),
            sharpen: self.sharpen.options(),
            dehaze: self.detail.dehaze_options(),
            ..Default::default()
        }
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

    /// Whether two edits develop the same image; the rest is the viewport's.
    pub fn same_develop(&self, other: &Edit) -> bool {
        self.same_patched(other) && self.detail == other.detail && self.sharpen == other.sharpen
    }

    /// Whether the developed picture before its local contrast, its
    /// dehaze and its sharpen is the same: the base and the retouch.
    pub fn same_patched(&self, other: &Edit) -> bool {
        self.same_base(other) && self.retouch == other.retouch
    }

    /// Whether two edits develop the same image before the sharpen,
    /// which a consumer may cache and apply on its own.
    pub fn same_base(&self, other: &Edit) -> bool {
        self.white_balance == other.white_balance
            && self.noise == other.noise
            && self.demosaic == other.demosaic
            && self.camera == other.camera
            && self.lens == other.lens
    }

    /// Whether the learned denoiser would answer the same for both:
    /// the same mosaic and tier. Its strength is a blend a consumer
    /// applies on the answer it has.
    pub fn same_learned(&self, other: &Edit) -> bool {
        self.white_balance == other.white_balance
            && self.demosaic == other.demosaic
            && self.camera == other.camera
            && self.noise.learned == other.noise.learned
    }

    /// Whether this edit is still part-migrated: version 4's step
    /// for [`geometry::Aspect::Original`] wants the frame's shape,
    /// which the sidecar does not carry, so [`migrate`] leaves the
    /// edit at version 3 and this says so. An edit that answers yes
    /// must have [`Self::migrate_with_frame`] called on it before
    /// anything reads its crop's aspect — and it is safe to save
    /// meanwhile, since it writes back as the version 3 it still is.
    pub fn needs_frame(&self) -> bool {
        self.version < VERSION
    }

    /// Finish version 4's step now the frame's shape is known.
    ///
    /// `source_portrait` is whether the frame's *developed* picture
    /// stands on end — the camera's tag and the frame's own turn
    /// applied, this edit's quarter turns not — which is what
    /// `Stance::shown_size` reports and what the viewport calls the
    /// source size. The plane the crop is measured in is that turned
    /// by the edit's own quarters.
    ///
    /// On a landscape plane the flag already meant what it means
    /// now, so it is left alone. On an upright plane it did not: a
    /// ticked flag was the only way to ask for the frame's own
    /// shape, and an unticked one gave a landscape crop of an
    /// upright frame, which is the bug. Both now mean the frame's
    /// own shape, so an upright plane ends flag-false either way.
    /// That is `portrait AND NOT plane_is_portrait`, which preserves
    /// what every stored edit rendered as except the one spelling
    /// whose rendering was the bug.
    ///
    /// A no-op on an edit that does not need it, so it is safe to
    /// call on every edit in hand.
    pub fn migrate_with_frame(&mut self, source_portrait: bool) {
        if !self.needs_frame() {
            return;
        }
        if self.geometry.aspect == geometry::Aspect::Original {
            let plane_portrait = source_portrait != (self.geometry.turns % 2 == 1);
            if plane_portrait {
                self.geometry.portrait = false;
            }
        }
        self.version = VERSION;
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("an edit serializes")
    }

    pub fn from_json(json: &str) -> Result<Self> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        Ok(serde_json::from_value(migrate(value)?)?)
    }
}

/// Bring a stored edit up to [`VERSION`]. A missing version is read
/// as 1. Version 2 gave the NOISE section a switch of its own, so
/// version 1's `noise.enabled`, which was the profiled denoiser's,
/// becomes `noise.profiled`; the section's switch is on.
///
/// Version 3 is the tone equalizer and the white point. No field moved
/// and none is renamed, so the migration is the identity, but the bump
/// is not ceremony: `light.tone.highlights`, `.shadows` and `.whites`
/// changed meaning. The first two were stops of shift read from the
/// pixel's own luminance, over ramps placed at 0..3 and -3..0 stops
/// over mid grey; they are read from a smoothed luminance now — a
/// region's, not a pixel's — over 0..2.5 and -3..0. Whites was a third
/// such shift over 2..5 stops and is a white point pivoted at mid grey,
/// a global power on the top of the scale. A sidecar written before
/// this build opens with its numbers unchanged and its picture does
/// move: most of all where whites was set, which used to move the clip
/// by 7 percent of a stop at minus one and moves it by 71 now.
///
/// That is the deliberate choice, and the alternative was considered.
/// Scaling the old numbers to reproduce the old picture would need a
/// whites of about a fourteenth of what is stored, which is not a
/// setting anybody chose and would carry forward the very thing the
/// roadmap called "whites do basically nothing". The stored number is
/// what the edit asked for; this is the first build that does it.
///
/// Turn every frame in `frames` by `quarters` quarter turns
/// clockwise; the frames it moved.
///
/// No history state is recorded — a turn is no part of the edit's
/// line — but every state a sidecar holds takes the same map, so
/// that what undo, redo and a snapshot hand back means the pixels
/// the current state means (see [`Sidecar::turn_by`]). A frame past
/// the end of the list is passed over rather than a panic; a
/// selection can outlive a folder change.
pub fn turn_into(
    sidecars: &mut [Sidecar],
    frames: &[usize],
    quarters: i32,
    aspect: Option<f32>,
) -> Vec<usize> {
    if quarters.rem_euclid(4) == 0 {
        return Vec::new();
    }
    frames
        .iter()
        .copied()
        .filter(|&i| match sidecars.get_mut(i) {
            Some(sidecar) => {
                sidecar.turn_by(quarters, aspect);
                true
            }
            None => false,
        })
        .collect()
}

/// Put `change` into the meta of every frame in `frames`, settled
/// across the set first; the settled change and the frames it moved.
///
/// Nothing here touches `current`, `history` or the snapshots: the
/// sidecar holds the meta beside the edit, and this writes only the
/// one field. A frame past the end of the list is passed over rather
/// than a panic; a selection can outlive a folder change.
pub fn meta_into(
    sidecars: &mut [Sidecar],
    frames: &[usize],
    change: meta::Change,
) -> (meta::Change, Vec<usize>) {
    let change = change.settled(
        frames
            .iter()
            .filter_map(|&i| sidecars.get(i))
            .map(|s| &s.meta),
    );
    let moved = frames
        .iter()
        .copied()
        .filter(|&i| {
            sidecars
                .get_mut(i)
                .is_some_and(|sidecar| change.apply(&mut sidecar.meta))
        })
        .collect();
    (change, moved)
}

/// Lay `sections` of `from` over the current edit of every frame in
/// `frames`, each as one step of its history; the frames it moved.
///
/// This is a preset that is never written: [`Preset::from_edit`]
/// keeps the chosen sections of `from` and [`Preset::apply`] lays
/// them over each frame, so a sync and a preset cannot come to mean
/// two different things by a section, but for one thing on top: a
/// sync's Noise brings the learned denoiser and its blend as well
/// ([`sync_learned`]). A preset leaves those to each file's ISO; a
/// sync is between frames of one shoot, and a frame taken to the
/// learned tier is the one the others are meant to look like. A
/// frame that already has every
/// chosen section as `from` has it records nothing and is left out
/// of what comes back, so its sidecar is not rewritten. Nothing but
/// `current` and `history` is touched: the meta, the turn and the
/// snapshots are the frame's own. A frame past the end of the list
/// is passed over rather than a panic, as [`meta_into`] does.
pub fn sync_into(
    sidecars: &mut [Sidecar],
    from: &Edit,
    frames: &[usize],
    sections: &[Section],
) -> Vec<usize> {
    let carried = Preset::from_edit("", from, sections);
    if carried.sections.is_empty() {
        return Vec::new();
    }
    let learned = carried.sections.contains(&Section::Noise);
    frames
        .iter()
        .copied()
        .filter(|&i| {
            sidecars.get_mut(i).is_some_and(|sidecar| {
                let mut applied = carried.applied(&sidecar.current);
                if learned {
                    sync_learned(from, &mut applied);
                }
                sidecar.record(applied)
            })
        })
        .collect()
}

/// What a sync's Noise carries beyond a preset's: the learned
/// denoiser's tier and its blend.
pub fn sync_learned(from: &Edit, onto: &mut Edit) {
    onto.noise.learned = from.noise.learned;
    onto.noise.learned_strength = from.noise.learned_strength;
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
            }
            _ => {
                if before != touched {
                    *onto = touched.clone();
                }
            }
        }
    }
    let before: Value = serde_json::from_str(&before.to_json())?;
    let touched: Value = serde_json::from_str(&touched.to_json())?;
    let mut result: Value = serde_json::from_str(&onto.to_json())?;
    overlay(&before, &touched, &mut result);
    Edit::from_json(&serde_json::to_string(&result)?)
}

/// §98 moved the two ramps within version 3, highlights to -1..2.5
/// and shadows to -3.5..0, and the sliders to ±2: where the same
/// shift reaches, not what the number is, and nothing shipped in
/// between, so no bump.
///
/// Version 4 changed what [`geometry::Aspect::Original`] means. It
/// used to stand the plane up as landscape and read which way up the
/// crop went off `geometry.portrait`, which starts false, so
/// "Original" on a frame shot on end cropped it landscape; it is the
/// plane's own ratio now, and the flag turns that. The conversion
/// clears the flag on an upright plane and leaves it on a landscape
/// one, which keeps every stored edit rendering as it did but for
/// the one spelling whose rendering was the bug; and the plane is
/// the frame's developed picture turned by the edit's own quarter
/// turns, which a sidecar does not hold.
///
/// So this step cannot be done here. An edit that needs it is left
/// at version 3 and says so through [`Edit::needs_frame`]; whoever
/// knows the frame's shape finishes it with
/// [`Edit::migrate_with_frame`], and until then the edit reads,
/// renders and writes back as the version 3 it is, so nothing is
/// lost by a build that only listed a folder.
pub fn migrate(mut value: serde_json::Value) -> Result<serde_json::Value> {
    let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    if version > VERSION {
        return Err(Error::Newer(version));
    }
    if let Some(obj) = value.as_object_mut() {
        // Version 3 changed what the three tone shifts mean, not where
        // they are kept, so it has nothing to move; the doc above says
        // why the numbers are left standing.
        if version < 2
            && let Some(noise) = obj.get_mut("noise").and_then(|n| n.as_object_mut())
            && let Some(profiled) = noise.remove("enabled")
        {
            noise.insert("profiled".into(), profiled);
        }
        // The dehaze was a top-level `dehaze: {amount}` for a few
        // days of master before it joined the DETAIL section; a
        // sidecar from then keeps its amount. Within version 3: it
        // never shipped.
        if let Some(amount) = obj
            .remove("dehaze")
            .and_then(|d| d.get("amount").and_then(|a| a.as_f64()))
        {
            let detail = obj
                .entry("detail")
                .or_insert_with(|| serde_json::Value::Object(Default::default()));
            if let Some(detail) = detail.as_object_mut() {
                detail.insert("dehaze".into(), amount.into());
            }
        }
        // Version 4's step needs the frame's shape, which is not in
        // here: an edit that wants it stays at 3 until someone who
        // knows the frame calls `Edit::migrate_with_frame`. Only an
        // `Original` aspect is affected; every other edit is brought
        // up by its version number alone.
        let waiting = version < 4
            && obj
                .get("geometry")
                .and_then(|g| g.get("aspect"))
                .and_then(|a| a.get("kind"))
                .and_then(|k| k.as_str())
                == Some("original");
        obj.insert("version".into(), if waiting { 3 } else { VERSION }.into());
    }
    Ok(value)
}

/// What sits beside a file: the current edit, the ones before it,
/// the states given a name, and the meta.
///
/// The meta is in the file and outside the edit, both on purpose.
/// In the file, because a rating belongs to the frame and one file
/// beside a frame is the whole library (§72); outside the edit,
/// because the history is the edit's and a star is not a step in
/// developing a picture. Nothing in [`Sidecar::record`],
/// [`Sidecar::undo`], [`Sidecar::redo`] or a snapshot touches it,
/// and setting it is not a state to undo.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Sidecar {
    pub current: Edit,
    /// The rating, the flag, the label, the keywords and the words:
    /// see [`meta`]. Absent from every sidecar written before it
    /// existed, and the default there; left out again when it says
    /// nothing, so developing a frame does not grow an empty block.
    /// Read through [`meta::loose_meta`], so that nothing written
    /// under it can cost the edit beside it.
    #[serde(
        skip_serializing_if = "Meta::is_empty",
        deserialize_with = "meta::loose_meta"
    )]
    pub meta: Meta,
    /// Quarter turns clockwise on top of the camera's orientation
    /// tag, 0 to 3: the frames a camera got wrong (shot straight
    /// down, a body with no sensor for it, a scan).
    ///
    /// Beside the edit for the same reason the meta is (§117): it is
    /// a fact about the picture the file got wrong, not a step in
    /// developing one, so undo never takes it back, a preset never
    /// carries one frame's turn onto the next, and a snapshot
    /// restores an exposure without restoring which way up the
    /// picture was. Read loosely and left out when it is 0, so a
    /// sidecar written before it existed loads unchanged and
    /// developing a frame nobody has turned does not grow a field.
    #[serde(
        default,
        skip_serializing_if = "is_no_turn",
        deserialize_with = "meta::loose_turn"
    )]
    pub turn: u8,
    /// What was last taken from an XMP beside the frame, so a load
    /// can ask whether that file has changed rather than compare two
    /// files' clocks: see [`xmp::Adopted`]. Read loosely, as the
    /// meta is, and left out when there is nothing to say.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "meta::loose"
    )]
    pub xmp: Option<xmp::Adopted>,
    /// How many times this sidecar, or the one it grew out of, has
    /// been written: [`Self::save`] and [`Self::save_in`] increment
    /// it before every write. [`Self::find`] reads it from both
    /// copies when a frame has one in each place and takes the
    /// higher, falling back to mtime only on a tie, since a copy
    /// tool, a backup restore or two clocks can make one file's
    /// mtime lie about which save is newer but cannot make it lie
    /// about how many saves it has seen. Defaulted, so a sidecar
    /// from before this field reads as 0 and both the old file and
    /// an older build opening a sidecar this build wrote are
    /// unaffected: an unknown field is silently ignored by serde,
    /// and 0 is what an old build already assumes.
    pub saved: u64,
    /// Earlier states, oldest first, each one a whole edit.
    pub history: Vec<Edit>,
    /// States undone, newest first; kept for the session, not written.
    #[serde(skip)]
    pub redo: Vec<Edit>,
    /// States kept by name, in the order they were taken, outside
    /// the history's cap.
    pub snapshots: Vec<Snapshot>,
}

fn is_no_turn(turn: &u8) -> bool {
    turn.rem_euclid(4) == 0
}

/// A whole edit kept under a name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub name: String,
    /// When it was taken, seconds since the Unix epoch.
    pub taken: u64,
    pub edit: Edit,
}

/// How many earlier states a sidecar keeps. The earliest is kept
/// whatever the count, so the state the file was opened in stays.
pub const HISTORY: usize = 50;

/// Where a frame's sidecar is kept: beside the frame, which is the
/// default and the rule of §72 and §117, or in a hidden folder
/// inside the frame's own folder, for a tester who finds a `.gcd`
/// between every pair of raws cluttering. Either way the sidecar
/// travels with the shoot's folder; a parallel tree elsewhere would
/// not, and a copied shoot would arrive without its edits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    /// `IMG_0001.CR3.gcd` beside `IMG_0001.CR3`.
    #[default]
    Beside,
    /// `.greycard/IMG_0001.CR3.gcd` under the frame's folder.
    Folder,
}

impl Placement {
    /// The other place.
    pub fn other(self) -> Self {
        match self {
            Self::Beside => Self::Folder,
            Self::Folder => Self::Beside,
        }
    }

    /// The placement `raw`'s sidecar has now, beside when it has
    /// none: for a tool without the setting, which writes a sidecar
    /// back where it found it.
    pub fn of(raw: &Path) -> Self {
        match Sidecar::find(raw) {
            Some(p) if under_folder(&p) => Self::Folder,
            _ => Self::Beside,
        }
    }
}

/// Whether a sidecar's path is under the hidden folder.
fn under_folder(sidecar: &Path) -> bool {
    sidecar.parent().and_then(Path::file_name) == Some(std::ffi::OsStr::new(SIDECAR_FOLDER))
}

/// Just [`Sidecar::saved`], read without paying for the rest of the
/// file: a struct of one field, so [`Sidecar::find`] does not parse
/// the edit, the history and the snapshots twice over to compare two
/// counters. Anything that keeps it from reading as a number — the
/// file gone, not JSON, an object with no such field, a sidecar from
/// before the field existed — reads as 0, which is exactly what
/// [`Sidecar::find`] wants: fall back to mtime.
#[derive(Deserialize, Default)]
struct SavedCount {
    #[serde(default)]
    saved: u64,
}

fn saved_count(path: &Path) -> u64 {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SavedCount>(&bytes).ok())
        .map_or(0, |s| s.saved)
}

/// The hidden folder's name under a shoot's folder.
pub const SIDECAR_FOLDER: &str = ".greycard";

impl Sidecar {
    /// The sidecar's path for a RAW: the file's name with `.gcd` on the end,
    /// so `IMG_0001.CR3` keeps its edit in `IMG_0001.CR3.gcd` (JSON inside).
    /// The `Beside` placement; `path_in` is the general form.
    pub fn path_for(raw: &Path) -> PathBuf {
        Self::path_in(raw, Placement::Beside)
    }

    /// Where `raw`'s sidecar goes under `placement`: beside it, or
    /// under the `.greycard` folder in its own folder, with the same
    /// name either way.
    pub fn path_in(raw: &Path, placement: Placement) -> PathBuf {
        let mut name = raw
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        name.push(".gcd");
        match placement {
            Placement::Beside => raw.with_file_name(name),
            Placement::Folder => raw.with_file_name(SIDECAR_FOLDER).join(name),
        }
    }

    /// The sidecar `raw` has, wherever it is: both places are read
    /// whatever the setting says, so flipping the setting never
    /// loses an edit. When both are there, the one with the higher
    /// [`Self::saved`] wins; a copy tool, a backup restore or two
    /// clocks can leave the older save with the newer mtime, but not
    /// with the higher count. Only when the counts are equal — both
    /// 0 for a pair from before the field existed, or a genuine tie —
    /// does mtime decide, beside on a tie there too. The counters are
    /// read only in this both-exist case, so the common one-copy
    /// case costs nothing beyond the metadata call it already made.
    pub fn find(raw: &Path) -> Option<PathBuf> {
        let modified = |p: &Path| p.metadata().and_then(|m| m.modified()).ok();
        let (beside, under) = (
            Self::path_in(raw, Placement::Beside),
            Self::path_in(raw, Placement::Folder),
        );
        match (modified(&beside), modified(&under)) {
            (None, None) => None,
            (Some(_), None) => Some(beside),
            (None, Some(_)) => Some(under),
            (Some(b), Some(u)) => match saved_count(&beside).cmp(&saved_count(&under)) {
                std::cmp::Ordering::Less => Some(under),
                std::cmp::Ordering::Greater => Some(beside),
                std::cmp::Ordering::Equal if u > b => Some(under),
                std::cmp::Ordering::Equal => Some(beside),
            },
        }
    }

    /// The sidecar `raw` has, if there is one, wherever it is.
    pub fn load(raw: &Path) -> Result<Option<Self>> {
        let Some(path) = Self::find(raw) else {
            return Ok(None);
        };
        let json = std::fs::read_to_string(&path)?;
        let mut value: serde_json::Value = serde_json::from_str(&json)?;
        if let Some(obj) = value.as_object_mut() {
            if let Some(current) = obj.remove("current") {
                obj.insert("current".into(), migrate(current)?);
            }
            if let Some(serde_json::Value::Array(history)) = obj.remove("history") {
                let history: Result<Vec<_>> = history.into_iter().map(migrate).collect();
                obj.insert("history".into(), serde_json::Value::Array(history?));
            }
            if let Some(serde_json::Value::Array(mut snapshots)) = obj.remove("snapshots") {
                for snapshot in snapshots.iter_mut() {
                    if let Some(edit) = snapshot.get_mut("edit") {
                        *edit = migrate(edit.take())?;
                    }
                }
                obj.insert("snapshots".into(), serde_json::Value::Array(snapshots));
            }
        }
        Ok(Some(serde_json::from_value(value)?))
    }

    /// Record `edit` as the current state, keeping the one it replaces;
    /// false when it was the current state already.
    pub fn record(&mut self, edit: Edit) -> bool {
        if edit == self.current {
            return false;
        }
        let previous = std::mem::replace(&mut self.current, edit);
        self.history.push(previous);
        if self.history.len() > HISTORY {
            // The earliest stays; what follows it goes.
            let extra = self.history.len() - HISTORY;
            self.history.drain(1..1 + extra);
        }
        self.redo.clear();
        true
    }

    /// Whether any state here is still waiting on the frame's shape:
    /// see [`Edit::needs_frame`]. A sidecar that answers yes reads
    /// and renders, and saves back as the version 3 it is, but its
    /// crop must not be refitted until the frame is known.
    pub fn needs_frame(&self) -> bool {
        std::iter::once(&self.current)
            .chain(self.history.iter())
            .chain(self.redo.iter())
            .chain(self.snapshots.iter().map(|s| &s.edit))
            .any(Edit::needs_frame)
    }

    /// Finish version 4's step on every state here, now the frame's
    /// shape is known. `source_portrait` is as
    /// [`Edit::migrate_with_frame`] has it, and includes this
    /// sidecar's own [`Self::turn`], since that is already in the
    /// developed picture. A no-op when nothing was waiting.
    pub fn migrate_with_frame(&mut self, source_portrait: bool) {
        for edit in self.edits_mut() {
            edit.migrate_with_frame(source_portrait);
        }
    }

    /// How many states there are to walk: the history, the current
    /// one, and what was undone.
    pub fn states(&self) -> usize {
        self.history.len() + 1 + self.redo.len()
    }

    /// Where the current state sits among [`Self::states`], oldest
    /// first.
    pub fn position(&self) -> usize {
        self.history.len()
    }

    /// The index among [`Self::states`] of the state a list showing
    /// the newest first has at `row`. None for a row past the end,
    /// or a negative one, which is what a list with nothing chosen
    /// hands back.
    pub fn state_at_row(&self, row: i32) -> Option<usize> {
        let n = self.states();
        usize::try_from(row)
            .ok()
            .filter(|&r| r < n)
            .map(|r| n - 1 - r)
    }

    /// The state at `index` among [`Self::states`], oldest first.
    pub fn state(&self, index: usize) -> Option<&Edit> {
        let position = self.position();
        if index < position {
            self.history.get(index)
        } else if index == position {
            Some(&self.current)
        } else {
            // The redo stack is newest first.
            let past = index - position - 1;
            self.redo.len().checked_sub(past + 1).map(|i| &self.redo[i])
        }
    }

    /// Make the state at `index` current by undoing or redoing up to
    /// it, so what lies past it is still there to redo; false when
    /// nothing moved.
    pub fn go_to(&mut self, index: usize) -> bool {
        let from = self.position();
        while self.position() > index && self.undo() {}
        while self.position() < index && self.redo() {}
        self.position() != from
    }

    /// Keep the current state under `name`; its index.
    pub fn take_snapshot(&mut self, name: impl Into<String>, taken: u64) -> usize {
        self.snapshots.push(Snapshot {
            name: name.into(),
            taken,
            edit: self.current.clone(),
        });
        self.snapshots.len() - 1
    }

    /// Make snapshot `index` the current state as a new step, so the
    /// history stays a line and undo goes back to before; false when
    /// it is the current state already, or there is no such snapshot.
    pub fn restore_snapshot(&mut self, index: usize) -> bool {
        match self.snapshots.get(index) {
            Some(s) => {
                let edit = s.edit.clone();
                self.record(edit)
            }
            None => false,
        }
    }

    /// Back to the previous state; false at the beginning.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.history.pop() else {
            return false;
        };
        let undone = std::mem::replace(&mut self.current, previous);
        self.redo.push(undone);
        true
    }

    /// Forward again; false when nothing was undone.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.current, next);
        self.history.push(current);
        true
    }

    /// Turn the frame `quarters` more quarter turns clockwise, and
    /// carry every state here round with it: the crop, the keystone
    /// and a held aspect, and the masks and the repair patches when
    /// `aspect` says what shape the picture is (see [`Edit::turn`]).
    ///
    /// *Every* state, not only the current one. A turn is not a step
    /// in developing the picture — no state is recorded for it, and
    /// an undo that took the crop back but left the frame on its
    /// side would be worse than one that does neither — and that
    /// promise only holds if what undo, redo and a snapshot hand
    /// back means the same pixels the current state does. So the
    /// history, the redo stack and the snapshots take the same map.
    ///
    /// `aspect` is the picture's width over its height *before* the
    /// turn, and is `None` when the caller cannot say. The geometry
    /// turns either way; anything placed on the picture is left
    /// where it is, which [`Sidecar::placed`] warns the caller
    /// about.
    pub fn turn_by(&mut self, quarters: i32, aspect: Option<f32>) {
        if quarters.rem_euclid(4) == 0 {
            return;
        }
        self.turn = (i32::from(self.turn) + quarters).rem_euclid(4) as u8;
        let positions = aspect.map(|a| mask::Turned::new(quarters, a));
        for edit in self.edits_mut() {
            edit.turn(quarters, positions);
        }
    }

    /// Every edit here: the current one, the history, what was
    /// undone, and the snapshots.
    fn edits_mut(&mut self) -> impl Iterator<Item = &mut Edit> {
        std::iter::once(&mut self.current)
            .chain(self.history.iter_mut())
            .chain(self.redo.iter_mut())
            .chain(self.snapshots.iter_mut().map(|s| &mut s.edit))
    }

    /// Whether any state here has work placed on the picture itself;
    /// see [`Edit::placed`].
    pub fn placed(&self) -> bool {
        std::iter::once(&self.current)
            .chain(self.history.iter())
            .chain(self.redo.iter())
            .chain(self.snapshots.iter().map(|s| &s.edit))
            .any(Edit::placed)
    }

    /// The hidden folder under `shoot`, made and hidden: every time
    /// and not only the first, so a folder unhidden by hand, or
    /// copied to a disk that dropped the attribute, goes hidden
    /// again. The edit matters more than the attribute, so a failure
    /// to hide is said and not returned.
    pub fn folder_under(shoot: &Path) -> std::io::Result<PathBuf> {
        let folder = shoot.join(SIDECAR_FOLDER);
        std::fs::create_dir_all(&folder)?;
        if let Err(e) = hide_folder(&folder) {
            log::warn!("{}: not hidden: {e}", folder.display());
        }
        Ok(folder)
    }

    /// Write the sidecar beside `raw`, whole, through a temporary file.
    pub fn save(&mut self, raw: &Path) -> Result<()> {
        self.save_in(raw, Placement::Beside)
    }

    /// Write the sidecar where `placement` says, whole, through a
    /// temporary file; the hidden folder made, and hidden, when it
    /// is not there yet. A copy in the other place goes: what was
    /// loaded was the newer of the two and what is written came from
    /// it, so the other is superseded either way, and a frame keeps
    /// one sidecar after a save. That is how a folder migrates when
    /// the setting flips, a frame at its next edit; the rest wait for
    /// a move asked for by name.
    ///
    /// [`Self::saved`] is incremented before the write, so the copy
    /// that lands is always the one [`Self::find`] will prefer over
    /// whatever it replaces.
    pub fn save_in(&mut self, raw: &Path, placement: Placement) -> Result<()> {
        self.saved += 1;
        let path = Self::path_in(raw, placement);
        if placement == Placement::Folder
            && let Some(folder) = path.parent()
        {
            Self::folder_under(folder.parent().unwrap_or(Path::new("")))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        let other = Self::path_in(raw, placement.other());
        if other.exists()
            && let Err(e) = std::fs::remove_file(&other)
        {
            log::warn!("{}: superseded but not removed: {e}", other.display());
        }
        Ok(())
    }

    /// Whether `raw` has a sidecar in the place `placement` does not
    /// name: one a [`Sidecar::settle`] would move or remove.
    pub fn misplaced(raw: &Path, placement: Placement) -> bool {
        Self::path_in(raw, placement.other()).exists()
    }

    /// Put `raw`'s sidecar where `placement` says, without reading or
    /// rewriting it: the move a folder asks for by name when the
    /// setting has flipped, rather than waiting for each frame's next
    /// save. The copy in the other place is renamed into this one
    /// when it is the one [`Sidecar::find`] would pick — by
    /// [`Sidecar::saved`], falling back to mtime the same way — and
    /// removed when it is not, since a save would have superseded it
    /// the same way. The hidden folder is made, and hidden, first.
    pub fn settle(raw: &Path, placement: Placement) -> std::io::Result<Settled> {
        let (here, other) = (
            Self::path_in(raw, placement),
            Self::path_in(raw, placement.other()),
        );
        if !other.exists() {
            return Ok(Settled::InPlace);
        }
        if Self::find(raw).as_deref() == Some(other.as_path()) {
            if placement == Placement::Folder {
                let shoot = raw.parent().unwrap_or(Path::new(""));
                Self::folder_under(shoot)?;
            }
            std::fs::rename(&other, &here)?;
            Ok(Settled::Moved)
        } else {
            std::fs::remove_file(&other)?;
            Ok(Settled::Dropped)
        }
    }
}

/// What [`Sidecar::settle`] did with a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// Nothing was in the other place.
    InPlace,
    /// The copy in the other place, the one read, was renamed into
    /// this one.
    Moved,
    /// The copy in the other place was the older of two and was
    /// removed; the one read was already in place.
    Dropped,
}

/// Mark a folder hidden where a leading dot does not do it. Windows
/// keeps that in a file attribute rather than in the name, and the
/// attribute set is written whole, so the ones the folder has are
/// read first and kept. Nothing to do elsewhere.
fn hide_folder(dir: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_HIDDEN, GetFileAttributesW, INVALID_FILE_ATTRIBUTES, SetFileAttributesW,
        };
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: `wide` is a null-terminated UTF-16 path that outlives
        // both calls, which read it and nothing else.
        unsafe {
            let have = GetFileAttributesW(wide.as_ptr());
            if have == INVALID_FILE_ATTRIBUTES {
                return Err(std::io::Error::last_os_error());
            }
            if have & FILE_ATTRIBUTE_HIDDEN == 0
                && SetFileAttributesW(wide.as_ptr(), have | FILE_ATTRIBUTE_HIDDEN) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Version 4 changed what `Original` means, so every sidecar
    /// written before it has to go on rendering as it did. The old
    /// build stood the plane up as landscape and read the way up off
    /// `portrait`; the new one reads the plane's own ratio. The
    /// migration is `portrait AND NOT plane_is_portrait` — an
    /// upright plane ends flag-false, a landscape one keeps the flag
    /// it had — and the plane is the developed frame turned by the
    /// edit's own quarters, so it needs the frame and cannot be done
    /// in `migrate`.
    ///
    /// The one row that must *not* come back the same is the bug:
    /// an upright frame with the flag false cropped landscape, and
    /// now crops the whole frame. That row is the only one where
    /// this differs from `portrait XOR plane_is_portrait`, which
    /// would have preserved the bug along with everything else.
    #[test]
    fn a_version_three_original_keeps_its_shape_once_the_frame_is_known() {
        use geometry::Aspect;

        // The landscape source the editor develops, and the same
        // frame shot on end.
        const LAND: (f32, f32) = (6000.0, 4000.0);
        const UP: (f32, f32) = (4000.0, 6000.0);

        // What version 3 would have cropped: the plane stood up as
        // landscape, then turned by the flag.
        fn as_version_three(turns: u8, portrait: bool, source: (f32, f32)) -> f32 {
            let (w, h) = if turns % 2 == 1 {
                (source.1, source.0)
            } else {
                source
            };
            let landscape = if w / h >= 1.0 { w / h } else { h / w };
            if portrait { 1.0 / landscape } else { landscape }
        }

        let stored = |turns: u8, portrait: bool| {
            format!(
                r#"{{"version":3,"geometry":{{"turns":{turns},"portrait":{portrait},
                   "aspect":{{"kind":"original"}}}}}}"#
            )
        };

        for (name, turns, portrait, source, old_is_right) in [
            ("landscape, flag false", 0u8, false, LAND, true),
            ("landscape, flag true", 0, true, LAND, true),
            ("upright, flag false", 0, false, UP, false),
            ("upright, flag true", 0, true, UP, true),
            // What the old build itself wrote for "pick Original,
            // then Turn left" on a landscape frame: it flipped the
            // flag on every quarter turn. The common case.
            ("landscape turned, flag true", 1, true, LAND, true),
        ] {
            let edit = Edit::from_json(&stored(turns, portrait)).unwrap();
            // It reads, and it says it is not finished with.
            assert!(edit.needs_frame(), "{name}");
            assert_eq!(edit.version, 3, "{name}");
            // Saved back meanwhile it is still a version 3 file, so a
            // build that only listed the folder loses nothing.
            assert!(edit.to_json().contains("\"version\": 3"), "{name}");

            let mut edit = edit;
            edit.migrate_with_frame(source.1 > source.0);
            assert!(!edit.needs_frame(), "{name}");
            assert_eq!(edit.version, VERSION, "{name}");

            let (pw, ph) = edit.geometry.plane_size(source.0, source.1);
            let now = edit
                .geometry
                .aspect
                .ratio(pw, ph, edit.geometry.portrait)
                .unwrap();
            let then = as_version_three(turns, portrait, source);
            if old_is_right {
                assert!((now - then).abs() < 1e-6, "{name}: {then} became {now}");
            } else {
                // The bug: it cropped landscape and must now be the
                // whole upright frame.
                assert!((then - 1.5).abs() < 1e-6, "{name}: {then}");
                assert!((now - pw / ph).abs() < 1e-6, "{name}: {now}");
                assert!(now < 1.0, "{name}: {now} is not upright");
            }
        }

        // Nothing but `Original` is touched, and a free or named
        // aspect comes up to version 4 in `migrate` alone.
        for aspect in [r#"{"kind":"free"}"#, r#"{"kind":"ratio","w":3.0,"h":2.0}"#] {
            let json =
                format!(r#"{{"version":3,"geometry":{{"portrait":true,"aspect":{aspect}}}}}"#);
            let edit = Edit::from_json(&json).unwrap();
            assert!(!edit.needs_frame(), "{aspect}");
            assert_eq!(edit.version, VERSION, "{aspect}");
            assert!(edit.geometry.portrait, "{aspect}");
        }
        // And an edit this build wrote is never migrated twice: the
        // flag would flip back.
        let mut fresh = Edit {
            geometry: geometry::Geometry {
                aspect: Aspect::Original,
                portrait: true,
                ..geometry::Geometry::default()
            },
            ..Edit::default()
        };
        assert!(!fresh.needs_frame());
        fresh.migrate_with_frame(true);
        assert!(fresh.geometry.portrait, "a version 4 edit is left alone");
    }

    /// A sidecar migrates every state it holds, not only the current
    /// one, since undo, redo and a snapshot each put one of the
    /// others back on the panel; and one already saved and reloaded
    /// is not migrated a second time.
    #[test]
    fn a_sidecar_migrates_all_its_states_once_and_only_once() {
        let json = r#"{
            "current": {"version":3,"geometry":{"aspect":{"kind":"original"}}},
            "history": [{"version":3,"geometry":{"aspect":{"kind":"original"}}}],
            "snapshots": [{"name":"a","taken":0,
                "edit":{"version":3,"geometry":{"aspect":{"kind":"original"}}}}]
        }"#;
        let mut sidecar: Sidecar = serde_json::from_str(json).unwrap();
        assert!(sidecar.needs_frame());
        // Written back before the frame is known, it is version 3
        // still, and reads back wanting the same step.
        let written = serde_json::to_string(&sidecar).unwrap();
        let again: Sidecar = serde_json::from_str(&written).unwrap();
        assert!(again.needs_frame());

        // The frame stands on end: every state's flag turns.
        sidecar.migrate_with_frame(true);
        assert!(!sidecar.needs_frame());
        for edit in [
            &sidecar.current,
            &sidecar.history[0],
            &sidecar.snapshots[0].edit,
        ] {
            assert_eq!(edit.version, VERSION);
            assert!(!edit.geometry.portrait, "an upright plane ends flag-false");
        }
        // A round trip of the migrated sidecar does not migrate it
        // again: the flag stays where the first pass put it.
        let written = serde_json::to_string(&sidecar).unwrap();
        let again: Sidecar = serde_json::from_str(&written).unwrap();
        assert!(!again.needs_frame());
        again.clone().migrate_with_frame(true);
        assert!(!again.current.geometry.portrait);
        assert_eq!(again, sidecar);
    }

    /// The history list shows the newest step first, and undo leaves
    /// the undone ones below the current one rather than dropping
    /// them, so the mapping is over the whole line and not over what
    /// is still in force.
    #[test]
    fn a_row_of_the_newest_first_list_names_a_state() {
        let mut sidecar = Sidecar::default();
        for exposure in [1.0, 2.0, 3.0] {
            let mut edit = Edit::default();
            edit.light.exposure = exposure;
            assert!(sidecar.record(edit));
        }
        assert_eq!(sidecar.states(), 4);
        // Row 0 is the newest state, the last row the oldest.
        assert_eq!(sidecar.state_at_row(0), Some(3));
        assert_eq!(sidecar.state_at_row(3), Some(0));
        assert_eq!(sidecar.state_at_row(4), None);
        // A list with nothing chosen reports -1.
        assert_eq!(sidecar.state_at_row(-1), None);
        // Undone steps stay in the list, so the rows do not move.
        assert!(sidecar.undo());
        assert_eq!(sidecar.states(), 4);
        assert_eq!(sidecar.state_at_row(0), Some(3));
        assert_eq!(
            sidecar.state(sidecar.state_at_row(1).unwrap()).unwrap(),
            sidecar.state(sidecar.position()).unwrap()
        );
    }

    #[test]
    fn a_culling_key_moves_a_whole_set_and_leaves_their_edits_alone() {
        use crate::meta::{Change, Flag, Label};
        let mut sidecars = vec![Sidecar::default(); 3];
        let mut edit = Edit::default();
        edit.light.exposure = 1.0;
        assert!(sidecars[0].record(edit));
        let before = sidecars[0].clone();

        // Two of the three frames: the third is nobody's business.
        let (settled, moved) = meta_into(&mut sidecars, &[0, 1], Change::Rating(4));
        assert_eq!(settled, Change::Rating(4));
        assert_eq!(moved, [0, 1]);
        assert_eq!(sidecars[0].meta.rating, 4);
        assert_eq!(sidecars[1].meta.rating, 4);
        assert!(sidecars[2].meta.is_empty());
        // The edit, its history and its snapshots are where they were.
        assert_eq!(sidecars[0].current, before.current);
        assert_eq!(sidecars[0].history, before.history);
        assert_eq!(sidecars[0].snapshots, before.snapshots);
        // The same key again writes nothing, so no sidecar is
        // rewritten for a key that said what was there already.
        let (_, moved) = meta_into(&mut sidecars, &[0, 1], Change::Rating(4));
        assert!(moved.is_empty());

        // A label key sets it, and the same key over a set that all
        // carries it takes it off.
        meta_into(&mut sidecars, &[0, 1], Change::Label(Label::Red));
        assert_eq!(sidecars[0].meta.label, Label::Red);
        let (settled, moved) = meta_into(&mut sidecars, &[0, 1], Change::Label(Label::Red));
        assert_eq!(settled, Change::Label(Label::None));
        assert_eq!(moved, [0, 1]);
        assert_eq!(sidecars[1].meta.label, Label::None);
        // The rating is none of the label's business, nor the flag's.
        meta_into(&mut sidecars, &[0], Change::Flag(Flag::Reject));
        assert_eq!(sidecars[0].meta.rating, 4);
        assert_eq!(sidecars[0].meta.flag, Flag::Reject);

        // A frame that is not there any more is passed over.
        let (_, moved) = meta_into(&mut sidecars, &[9], Change::Rating(1));
        assert!(moved.is_empty());
    }

    #[test]
    fn a_sync_lays_the_chosen_sections_over_the_set_as_one_step_each() {
        use crate::meta::Change;
        // The frame synced from: a look, a white balance, a crop, a
        // mask and a patch of its own.
        let mut from = Edit::default();
        from.light.exposure = 0.7;
        from.light.tone.contrast = 1.3;
        from.color.vibrance = 0.4;
        from.white_balance = WhiteBalance::Custom {
            temperature: 3200.0,
            tint: 0.002,
        };
        from.geometry.crop = Some(geometry::Crop {
            x: 0.1,
            y: 0.1,
            w: 0.5,
            h: 0.5,
        });
        from.adjustments.push(Adjustment {
            id: 9,
            name: "Sky".into(),
            ..Adjustment::default()
        });
        from.retouch.patches.push(retouch::Patch::default());

        // Three others: one with an edit and a history of its own,
        // one fresh, one with a star.
        let mut sidecars = vec![Sidecar::default(); 4];
        let mut own = Edit::default();
        own.light.exposure = -1.0;
        own.sharpen.radius = 2.0;
        own.geometry.crop = Some(geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.9,
            h: 0.9,
        });
        assert!(sidecars[1].record(own.clone()));
        meta_into(&mut sidecars, &[3], Change::Rating(3));
        sidecars[3].turn = 1;
        let before: Vec<Sidecar> = sidecars.clone();

        let sections = [Section::Light, Section::Color, Section::WhiteBalance];
        let moved = sync_into(&mut sidecars, &from, &[1, 2, 3, 9], &sections);
        assert_eq!(moved, [1, 2, 3], "the frame past the end is passed over");
        for &i in &moved {
            let s = &sidecars[i];
            // The chosen sections are the source's.
            assert_eq!(s.current.light, from.light, "frame {i}");
            assert_eq!(s.current.color, from.color, "frame {i}");
            assert_eq!(s.current.white_balance, from.white_balance, "frame {i}");
            // The rest is the frame's own: its sharpen, its crop,
            // and no masks or patches of anybody else's.
            let was = &before[i].current;
            assert_eq!(s.current.sharpen, was.sharpen, "frame {i}");
            assert_eq!(s.current.geometry, was.geometry, "frame {i}");
            assert_eq!(s.current.adjustments, was.adjustments, "frame {i}");
            assert_eq!(s.current.retouch, was.retouch, "frame {i}");
            // One step on the history, the state before it kept.
            assert_eq!(s.history.len(), before[i].history.len() + 1, "frame {i}");
            assert_eq!(s.history.last(), Some(was), "frame {i}");
            // The meta and the turn are not the edit's.
            assert_eq!(s.meta, before[i].meta, "frame {i}");
            assert_eq!(s.turn, before[i].turn, "frame {i}");
        }
        // The frame not in the set is untouched.
        assert_eq!(sidecars[0], before[0]);
        // Undo on a synced frame takes the sync back whole.
        let mut undone = sidecars[1].clone();
        assert!(undone.undo());
        assert_eq!(undone.current, own);

        // Once more: every frame has it already, so nothing moves
        // and no sidecar needs writing.
        let again = sync_into(&mut sidecars, &from, &[1, 2, 3], &sections);
        assert!(again.is_empty());
        assert_eq!(sidecars[1].history.len(), before[1].history.len() + 1);

        // The masks, asked for, come across with ids of their own,
        // exactly as a preset's do.
        let moved = sync_into(&mut sidecars, &from, &[2], &[Section::Adjustments]);
        assert_eq!(moved, [2]);
        assert_eq!(sidecars[2].current.adjustments.len(), 1);
        assert_eq!(sidecars[2].current.adjustments[0].name, "Sky");
        assert_eq!(sidecars[2].current.adjustments[0].id, 1);

        // Noise carries the learned denoiser in a sync, which a
        // preset of the same section does not.
        let mut learned = from.clone();
        learned.noise.strength = 0.8;
        learned.noise.learned = Learned::Balanced;
        learned.noise.learned_strength = 0.6;
        let moved = sync_into(&mut sidecars, &learned, &[3], &[Section::Noise]);
        assert_eq!(moved, [3]);
        assert_eq!(sidecars[3].current.noise, learned.noise);
        let preset = Preset::from_edit("", &learned, &[Section::Noise]);
        let laid = preset.applied(&Edit::default());
        assert_eq!(laid.noise.strength, 0.8);
        assert_eq!(laid.noise.learned, Learned::Off, "a preset leaves it");
        // Without Noise chosen, the learned tier stays the frame's.
        let moved = sync_into(&mut sidecars, &from, &[3], &[Section::Light]);
        assert!(moved.is_empty(), "light was the source's already");
        assert_eq!(sidecars[3].current.noise.learned, Learned::Balanced);

        // Nothing chosen is nothing done.
        assert!(sync_into(&mut sidecars, &from, &[0], &[]).is_empty());
        assert_eq!(sidecars[0], before[0]);
    }

    #[test]
    fn a_turn_key_turns_every_frame_in_the_set_and_leaves_the_rest() {
        let mut sidecars = vec![Sidecar::default(); 3];
        sidecars[1].current.geometry.crop = Some(geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        });
        sidecars[1].current.light.exposure = 1.0;
        let moved = turn_into(&mut sidecars, &[0, 1], 1, Some(1.5));
        assert_eq!(moved, vec![0, 1]);
        assert_eq!(sidecars[0].turn, 1);
        assert_eq!(sidecars[1].turn, 1);
        assert_eq!(sidecars[2].turn, 0, "a frame not in the set is not turned");
        // The crop went round with the picture; the exposure did not
        // move, and no history state was made for any of it.
        assert_eq!(
            sidecars[1].current.geometry.crop,
            Some(geometry::Crop {
                x: 0.5,
                y: 0.0,
                w: 0.5,
                h: 0.5
            })
        );
        assert_eq!(sidecars[1].current.light.exposure, 1.0);
        assert!(sidecars[1].history.is_empty());
        // Left again is where it started.
        turn_into(&mut sidecars, &[0, 1], -1, Some(1.0 / 1.5));
        assert_eq!(sidecars[1].turn, 0);
        assert_eq!(
            sidecars[1].current.geometry.crop,
            Some(geometry::Crop {
                x: 0.0,
                y: 0.0,
                w: 0.5,
                h: 0.5
            })
        );
        // Four quarters are none, and a frame off the end is passed
        // over rather than a panic.
        assert!(turn_into(&mut sidecars, &[0], 4, Some(1.5)).is_empty());
        assert_eq!(sidecars[0].turn, 0);
        assert!(turn_into(&mut sidecars, &[9], 1, Some(1.5)).is_empty());
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

    #[test]
    fn learned_blend_ramps_with_iso_in_stops() {
        // At and below ISO 200: the low end.
        assert_eq!(Noise::blend_for_iso(Some(200)), 0.35);
        assert_eq!(Noise::blend_for_iso(Some(50)), 0.35);
        // At and above ISO 3200: the high end.
        assert_eq!(Noise::blend_for_iso(Some(3200)), 1.0);
        assert_eq!(Noise::blend_for_iso(Some(51200)), 1.0);
        // Halfway in stops (ISO 800 is 2 of the 4 stops from 200 to
        // 3200) is halfway in blend.
        assert!((Noise::blend_for_iso(Some(800)) - 0.675).abs() < 1e-5);
        // Unknown ISO: the old constant, full blend.
        assert_eq!(Noise::blend_for_iso(None), 1.0);
        assert_eq!(
            Noise::blend_for_iso(None),
            Noise::default().learned_strength
        );
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
        };
        edit.demosaic = Demosaic::Rcd;
        let json = edit.to_json();
        assert!(json.contains("\"mode\": \"custom\""), "{json}");
        assert!(json.contains("\"demosaic\": \"rcd\""), "{json}");
        assert!(json.contains("\"learned\": \"best\""), "{json}");
        assert_eq!(Edit::from_json(&json).unwrap(), edit);
        // An empty document is the default edit; unknown fields are ignored.
        assert_eq!(Edit::from_json("{}").unwrap(), Edit::default());
        let partial = r#"{"light": {"exposure": -1}, "future_field": 3}"#;
        let e = Edit::from_json(partial).unwrap();
        assert_eq!(e.light.exposure, -1.0);
        assert_eq!(e.light.tone, Tone::default());
        assert_eq!(e.version, VERSION);
    }

    #[test]
    fn the_detail_section_rests_off_the_engine_and_reads_from_an_old_sidecar() {
        // A sidecar from before the section has it at its default: on,
        // both sliders at rest, which asks the engine for nothing.
        let old = r#"{"version": 3, "sharpen": {"enabled": false}}"#;
        let e = Edit::from_json(old).unwrap();
        assert_eq!(e.detail, Detail::default());
        assert!(e.detail.options().is_none());
        assert!(!e.sharpen.enabled, "the rest of the file still reads");
        // A slider set asks for it; the switch off takes it away and
        // keeps the slider; a value past the end is held to it.
        let mut e = Edit::default();
        e.detail.clarity = 0.5;
        let o = e.detail.options().expect("clarity is set");
        assert_eq!((o.texture, o.clarity), (0.0, 0.5));
        e.detail.enabled = false;
        assert!(e.detail.options().is_none());
        assert_eq!(e.detail.clarity, 0.5);
        e.detail.enabled = true;
        e.detail.texture = -3.0;
        assert_eq!(e.detail.options().unwrap().texture, -1.0);
        // It is part of the develop, not of the base: the worker's
        // cache of the develop before it stays good, the develop does
        // not.
        let base = Edit::default();
        assert!(base.same_patched(&e));
        assert!(!base.same_develop(&e));
        assert_eq!(Edit::from_json(&e.to_json()).unwrap(), e);
    }

    #[test]
    fn a_version_one_noise_switch_is_the_profiled_denoisers() {
        let old = r#"{"version": 1, "noise": {"enabled": true, "strength": 2.0}}"#;
        let e = Edit::from_json(old).unwrap();
        assert!(e.noise.enabled, "the section is on");
        assert!(
            e.noise.profiled,
            "and the profiled pass is what it asked for"
        );
        assert_eq!(e.noise.strength, 2.0);
        assert!(e.settings().denoise.is_some());
        // Written back, it is version 2 and reads the same.
        let again = Edit::from_json(&e.to_json()).unwrap();
        assert_eq!(again, e);
        // A version 2 file's `enabled` is the section's.
        let new = r#"{"version": 2, "noise": {"enabled": false, "profiled": true}}"#;
        let e = Edit::from_json(new).unwrap();
        assert!(!e.noise.enabled && e.noise.profiled);
        assert!(e.settings().denoise.is_none());
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
    fn a_sections_switch_off_is_the_section_undone() {
        let mut edit = Edit::default();
        edit.light.exposure = 2.0;
        edit.light.tone.contrast = 1.5;
        edit.light.enabled = false;
        let acting = edit.light.effective();
        assert_eq!(acting.exposure, 0.0);
        assert!(!acting.tone.enabled);
        assert_eq!(acting.tone.contrast, 1.5, "the setting is kept");
        assert_eq!(edit.light.exposure, 2.0, "and so is the edit's");

        edit.noise.learned = Learned::Fast;
        edit.noise.profiled = true;
        assert!(edit.noise.tier().is_some());
        assert!(!edit.noise.profiled_runs(), "the tier stands in for it");
        edit.noise.enabled = false;
        assert!(edit.noise.tier().is_none());
        assert!(edit.settings().denoise.is_none());

        edit.vignette.amount = -1.0;
        assert!(!edit.vignette.is_off());
        edit.vignette.enabled = false;
        assert!(edit.vignette.is_off());

        edit.grain.amount = 0.5;
        assert!(!edit.grain.is_off());
        edit.grain.enabled = false;
        assert!(edit.grain.is_off());

        edit.lens.manual = 0.1;
        assert!(!edit.lens.is_identity(false));
        edit.lens.enabled = false;
        assert!(edit.lens.is_identity(true));
        assert!(!edit.lens.wants_profile());
        assert!(edit.lens.corrections(None).is_empty());
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let err = Edit::from_json(r#"{"version": 99}"#).unwrap_err();
        assert!(matches!(err, Error::Newer(99)));
    }

    #[test]
    fn demosaic_names_are_the_engines() {
        for d in Demosaic::ALL {
            assert_eq!(Demosaic::from_name(d.name()), Some(*d));
            let json = serde_json::to_string(d).unwrap();
            assert_eq!(json, format!("\"{}\"", d.name()));
        }
        assert_eq!(DemosaicMethod::ALL.len(), Demosaic::ALL.len());
    }

    #[test]
    fn settings_follow_the_edit() {
        let mut edit = Edit::default();
        assert!(edit.settings().denoise.is_none());
        assert!(matches!(edit.settings().white_point, WhitePoint::AsShot));
        edit.noise.profiled = true;
        edit.white_balance = WhiteBalance::Custom {
            temperature: 5000.0,
            tint: 0.0,
        };
        let s = edit.settings();
        assert!(
            matches!(s.denoise, Some(DenoiseOptions { strength: Strength::Fixed(v), .. }) if v == 1.5)
        );
        assert!(matches!(s.white_point, WhitePoint::TempTint(_)));
        let mut other = edit.clone();
        other.light.exposure = 2.0;
        assert!(edit.same_develop(&other));
        other.demosaic = Demosaic::Vng4;
        assert!(!edit.same_develop(&other));
        let mut other = edit.clone();
        other.detail.dehaze = 0.5;
        assert!(edit.same_patched(&other), "the dehaze is after the base");
        assert!(!edit.same_develop(&other));
        assert!(edit.settings().dehaze.is_none());
        assert_eq!(other.settings().dehaze, Some(DehazeOptions { amount: 0.5 }));
        // The section's switch takes the dehaze with it.
        other.detail.enabled = false;
        assert!(other.settings().dehaze.is_none());
        assert!(other.detail.options().is_none());
    }

    #[test]
    fn a_sidecar_without_a_dehaze_reads_as_zero() {
        let edit = Edit::from_json(r#"{"version": 3, "light": {"exposure": 1.0}}"#).unwrap();
        assert_eq!(edit.detail.dehaze, 0.0);
        let edit = Edit::from_json(r#"{"version": 3, "detail": {"texture": 0.2, "clarity": 0.1}}"#)
            .unwrap();
        assert_eq!(edit.detail.dehaze, 0.0);
        assert_eq!(edit.detail.texture, 0.2);
    }

    /// A sidecar from the days the dehaze sat at the top level of the
    /// edit keeps its amount, in the Detail section now, whether or
    /// not the sidecar had a Detail section of its own.
    #[test]
    fn a_top_level_dehaze_moves_into_the_detail_section() {
        let edit = Edit::from_json(r#"{"version": 3, "dehaze": {"amount": 0.5}}"#).unwrap();
        assert_eq!(edit.detail.dehaze, 0.5);
        assert!(edit.detail.enabled);
        let edit = Edit::from_json(
            r#"{"version": 3, "dehaze": {"amount": -0.25}, "detail": {"texture": 0.3}}"#,
        )
        .unwrap();
        assert_eq!(edit.detail.dehaze, -0.25);
        assert_eq!(edit.detail.texture, 0.3);
        // And it is written where it now lives, not where it was.
        let json = edit.to_json();
        let tree: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(tree.get("dehaze").is_none(), "{json}");
        assert_eq!(tree["detail"]["dehaze"], -0.25);
        assert_eq!(Edit::from_json(&json).unwrap(), edit);
    }

    /// A fresh folder a test can write into, its own by name and pid.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-edit-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn set_modified(path: &Path, secs: u64) {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("the file opens")
            .set_modified(t)
            .expect("the time is set");
    }

    #[test]
    fn the_sidecar_has_one_name_in_two_places() {
        let raw = Path::new("/shoot/IMG_0001.CR3");
        assert_eq!(
            Sidecar::path_in(raw, Placement::Beside),
            Path::new("/shoot/IMG_0001.CR3.gcd")
        );
        assert_eq!(
            Sidecar::path_in(raw, Placement::Beside),
            Sidecar::path_for(raw)
        );
        assert_eq!(
            Sidecar::path_in(raw, Placement::Folder),
            Path::new("/shoot/.greycard/IMG_0001.CR3.gcd")
        );
        // A bare name has a folder too: the current one.
        assert_eq!(
            Sidecar::path_in(Path::new("IMG_0001.CR3"), Placement::Folder),
            Path::new(".greycard/IMG_0001.CR3.gcd")
        );
    }

    #[test]
    fn find_reads_both_places_and_takes_the_newer() {
        let dir = scratch("find");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        assert_eq!(Sidecar::find(&raw), None);

        let beside = Sidecar::path_in(&raw, Placement::Beside);
        let under = Sidecar::path_in(&raw, Placement::Folder);
        std::fs::write(&beside, b"{}").unwrap();
        assert_eq!(Sidecar::find(&raw), Some(beside.clone()));

        std::fs::remove_file(&beside).unwrap();
        std::fs::create_dir_all(under.parent().unwrap()).unwrap();
        std::fs::write(&under, b"{}").unwrap();
        assert_eq!(Sidecar::find(&raw), Some(under.clone()));

        // Both there, neither with a `saved` count (both read as 0,
        // an equal tie): mtime decides, the one written last,
        // whichever place it is in.
        std::fs::write(&beside, b"{}").unwrap();
        set_modified(&beside, 1_000_000_000);
        set_modified(&under, 2_000_000_000);
        assert_eq!(Sidecar::find(&raw), Some(under.clone()));
        set_modified(&beside, 3_000_000_000);
        assert_eq!(Sidecar::find(&raw), Some(beside.clone()));
        // A tie is beside: the older rule.
        set_modified(&under, 3_000_000_000);
        assert_eq!(Sidecar::find(&raw), Some(beside));
    }

    #[test]
    fn saving_three_times_counts_three() {
        let dir = scratch("counter");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let mut sidecar = Sidecar::default();
        for n in 0..3 {
            let mut e = Edit::default();
            e.light.exposure = n as f32;
            sidecar.record(e);
            sidecar.save(&raw).unwrap();
        }
        assert_eq!(sidecar.saved, 3);
        assert_eq!(Sidecar::load(&raw).unwrap().unwrap().saved, 3);
    }

    #[test]
    fn find_prefers_the_higher_count_over_a_newer_mtime() {
        let dir = scratch("count-over-mtime");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let (beside, under) = (
            Sidecar::path_in(&raw, Placement::Beside),
            Sidecar::path_in(&raw, Placement::Folder),
        );
        std::fs::create_dir_all(under.parent().unwrap()).unwrap();
        // The beside copy has the higher count but the older mtime —
        // a copy tool or a restore could produce exactly this. It
        // still wins: the count says it is the later save.
        std::fs::write(&beside, r#"{"saved":5}"#).unwrap();
        std::fs::write(&under, r#"{"saved":2}"#).unwrap();
        set_modified(&beside, 1_000_000_000);
        set_modified(&under, 2_000_000_000);
        assert_eq!(Sidecar::find(&raw), Some(beside));
    }

    #[test]
    fn find_falls_back_to_mtime_when_counts_are_equal() {
        let dir = scratch("count-tie");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let (beside, under) = (
            Sidecar::path_in(&raw, Placement::Beside),
            Sidecar::path_in(&raw, Placement::Folder),
        );
        std::fs::create_dir_all(under.parent().unwrap()).unwrap();
        // Equal, non-zero counts: not before the field existed, a
        // genuine tie (a `settle` that renamed rather than wrote,
        // for instance). mtime is what is left to decide with.
        std::fs::write(&beside, r#"{"saved":4}"#).unwrap();
        std::fs::write(&under, r#"{"saved":4}"#).unwrap();
        set_modified(&beside, 1_000_000_000);
        set_modified(&under, 2_000_000_000);
        assert_eq!(Sidecar::find(&raw), Some(under.clone()));
        set_modified(&beside, 3_000_000_000);
        assert_eq!(Sidecar::find(&raw), Some(beside));
    }

    #[test]
    fn a_sidecar_without_the_saved_field_loads_as_zero() {
        let dir = scratch("no-field");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let beside = Sidecar::path_in(&raw, Placement::Beside);
        // What this build's own sidecars looked like before this
        // field existed, and what an older build still writes.
        std::fs::write(&beside, r#"{"current":{"version":4}}"#).unwrap();
        let loaded = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(loaded.saved, 0);
    }

    #[test]
    fn settle_agrees_with_find_when_the_count_beats_mtime() {
        let dir = scratch("settle-count");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let (beside, under) = (
            Sidecar::path_in(&raw, Placement::Beside),
            Sidecar::path_in(&raw, Placement::Folder),
        );
        std::fs::create_dir_all(under.parent().unwrap()).unwrap();
        // Same shape as `find_prefers_the_higher_count_over_a_newer_mtime`:
        // the beside copy is the higher count but the older mtime.
        std::fs::write(&beside, r#"{"saved":5}"#).unwrap();
        std::fs::write(&under, r#"{"saved":2}"#).unwrap();
        set_modified(&beside, 1_000_000_000);
        set_modified(&under, 2_000_000_000);

        assert_eq!(Sidecar::find(&raw), Some(beside.clone()));
        // Settling into `Folder` should keep the copy `find` picked,
        // beside, and drop the other: they must never disagree.
        assert_eq!(
            Sidecar::settle(&raw, Placement::Folder).unwrap(),
            Settled::Moved
        );
        assert!(under.exists());
        assert!(!beside.exists());
        assert_eq!(std::fs::read_to_string(&under).unwrap(), r#"{"saved":5}"#);
    }

    #[test]
    fn a_sidecar_under_the_folder_loads_and_saves() {
        let dir = scratch("folder");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let mut sidecar = Sidecar::default();
        let mut e = Edit::default();
        e.light.exposure = 1.0;
        sidecar.record(e);

        // Saved under the folder: the folder is made, nothing lands
        // beside the raw, and a load finds it without being told.
        sidecar.save_in(&raw, Placement::Folder).unwrap();
        let under = dir.join(SIDECAR_FOLDER).join("IMG_0001.CR3.gcd");
        assert!(under.exists(), "{}", under.display());
        assert!(!Sidecar::path_for(&raw).exists());
        assert!(
            !dir.join(SIDECAR_FOLDER)
                .join("IMG_0001.CR3.json.tmp")
                .exists()
        );
        assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), sidecar);

        // Saved beside afterwards: the folder's copy is superseded
        // and goes, so the frame has one sidecar, and it loads.
        let mut later = sidecar.clone();
        let mut e = Edit::default();
        e.light.exposure = 2.0;
        later.record(e);
        later.save(&raw).unwrap();
        assert!(!under.exists(), "the folder's copy is superseded");
        assert!(Sidecar::path_for(&raw).exists());
        assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), later);

        // And back under the folder: the beside copy goes the same way.
        later.save_in(&raw, Placement::Folder).unwrap();
        assert!(under.exists());
        assert!(
            !Sidecar::path_for(&raw).exists(),
            "the beside copy is superseded"
        );
        assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), later);
    }

    #[test]
    fn settle_moves_the_newer_copy_and_drops_the_stale_one() {
        let dir = scratch("settle");
        let (a, b, c, d) = (
            dir.join("A.CR3"),
            dir.join("B.CR3"),
            dir.join("C.CR3"),
            dir.join("D.CR3"),
        );
        let place = |raw: &Path, placement, text: &str| {
            let path = Sidecar::path_in(raw, placement);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        };
        // A beside only; B under the folder only; C in both, the one
        // beside newer; D with none.
        place(&a, Placement::Beside, "a");
        place(&b, Placement::Folder, "b");
        let c_beside = place(&c, Placement::Beside, "c new");
        let c_under = place(&c, Placement::Folder, "c old");
        set_modified(&c_under, 1_000_000_000);
        set_modified(&c_beside, 2_000_000_000);

        let to = Placement::Folder;
        assert!(Sidecar::misplaced(&a, to));
        assert!(!Sidecar::misplaced(&b, to));
        assert!(Sidecar::misplaced(&c, to));
        assert!(!Sidecar::misplaced(&d, to));

        assert_eq!(Sidecar::settle(&a, to).unwrap(), Settled::Moved);
        assert_eq!(Sidecar::settle(&b, to).unwrap(), Settled::InPlace);
        assert_eq!(Sidecar::settle(&c, to).unwrap(), Settled::Moved);
        assert_eq!(Sidecar::settle(&d, to).unwrap(), Settled::InPlace);

        let read =
            |raw: &Path, placement| std::fs::read_to_string(Sidecar::path_in(raw, placement)).ok();
        assert_eq!(read(&a, to).as_deref(), Some("a"));
        assert_eq!(read(&a, Placement::Beside), None);
        assert_eq!(read(&b, to).as_deref(), Some("b"));
        // The newer copy won, over the one that was in place.
        assert_eq!(read(&c, to).as_deref(), Some("c new"));
        assert_eq!(read(&c, Placement::Beside), None);
        assert!(!Sidecar::path_in(&d, to).exists());

        // And back: a stale copy in the other place is dropped, not
        // moved over the one that is read.
        let stale = place(&a, Placement::Beside, "a stale");
        set_modified(&stale, 1_000_000_000);
        assert_eq!(Sidecar::settle(&a, to).unwrap(), Settled::Dropped);
        assert_eq!(read(&a, to).as_deref(), Some("a"));
        assert!(!stale.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_sidecar_keeps_history_and_survives_a_round_trip() {
        let dir = std::env::temp_dir().join(format!("greycard-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0001.CR3");
        assert!(Sidecar::load(&raw).unwrap().is_none());
        let mut sidecar = Sidecar::default();
        let mut e = Edit::default();
        e.light.exposure = 1.0;
        assert!(sidecar.record(e.clone()));
        assert!(!sidecar.record(e.clone())); // the same again is not a new state
        e.light.exposure = 2.0;
        sidecar.record(e.clone());
        assert_eq!(sidecar.history.len(), 2);
        sidecar.save(&raw).unwrap();
        assert!(Sidecar::path_for(&raw).ends_with("IMG_0001.CR3.gcd"));
        let back = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(back, sidecar);
        assert_eq!(back.current.light.exposure, 2.0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_sidecar_carries_the_meta_beside_the_edit_and_the_history_leaves_it_alone() {
        use meta::{Flag, Label};
        let dir = std::env::temp_dir().join(format!("greycard-meta-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0002.CR3");
        let mut sidecar = Sidecar::default();
        // An edit, then a rating: the rating is not a state.
        let mut e = Edit::default();
        e.light.exposure = 1.0;
        assert!(sidecar.record(e.clone()));
        let states = sidecar.states();
        sidecar.meta.set_rating(4);
        sidecar.meta.flag = Flag::Pick;
        sidecar.meta.label = Label::Green;
        sidecar.meta.add_keyword("harbor");
        sidecar.meta.title = "Low tide".into();
        assert_eq!(sidecar.states(), states);
        assert!(!sidecar.history.is_empty());
        sidecar.save(&raw).unwrap();
        let back = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(back, sidecar);
        assert_eq!(back.meta.rating, 4);
        assert_eq!(back.meta.flag, Flag::Pick);
        assert_eq!(back.meta.label, Label::Green);
        assert_eq!(back.meta.keywords, ["harbor"]);
        assert_eq!(back.meta.title, "Low tide");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_meta_that_says_nothing_is_not_written_and_a_bad_one_costs_no_edit() {
        // A developed frame nobody has rated: no meta block at all.
        let mut sidecar = Sidecar::default();
        sidecar.current.light.exposure = 1.0;
        let json = serde_json::to_string(&sidecar).unwrap();
        assert!(!json.contains("meta"), "{json}");
        assert_eq!(serde_json::from_str::<Sidecar>(&json).unwrap(), sidecar);
        sidecar.meta.set_rating(1);
        assert!(serde_json::to_string(&sidecar).unwrap().contains("meta"));

        // A meta this build cannot read at all does not take the edit
        // and the history down with it: that would open the frame as
        // a fresh one and let the next slider write over both.
        let json = r#"{"current":{"version":3,"light":{"exposure":0.5}},"meta":"picked"}"#;
        let back: Sidecar = serde_json::from_str(json).unwrap();
        assert_eq!(back.current.light.exposure, 0.5);
        assert!(back.meta.is_empty());
        // Nor does one field of it: a label from a later build leaves
        // the rating, and the edit, where they are.
        let json = r#"{"current":{"version":3,"light":{"exposure":0.5}},
                       "meta":{"rating":3,"label":"grey"}}"#;
        let back: Sidecar = serde_json::from_str(json).unwrap();
        assert_eq!(back.current.light.exposure, 0.5);
        assert_eq!(back.meta.rating, 3);
        assert_eq!(back.meta.label, meta::Label::None);
    }

    #[test]
    fn undo_after_a_rating_takes_back_the_edit_and_not_the_rating() {
        let mut sidecar = Sidecar::default();
        let mut e = Edit::default();
        e.light.exposure = 1.0;
        sidecar.record(e.clone());
        sidecar.meta.set_rating(3);
        e.light.exposure = 2.0;
        sidecar.record(e);
        sidecar.meta.set_rating(5);
        assert!(sidecar.undo());
        assert_eq!(sidecar.current.light.exposure, 1.0);
        assert_eq!(sidecar.meta.rating, 5);
        assert!(sidecar.undo());
        assert_eq!(sidecar.current, Edit::default());
        assert_eq!(sidecar.meta.rating, 5);
        // And a redo, and a snapshot restored, leave it where it is.
        assert!(sidecar.redo());
        assert_eq!(sidecar.meta.rating, 5);
        sidecar.take_snapshot("one", 0);
        assert!(sidecar.undo());
        assert!(sidecar.restore_snapshot(0));
        assert_eq!(sidecar.meta.rating, 5);
    }

    #[test]
    fn the_turn_round_trips_beside_the_edit_and_is_absent_at_none() {
        let dir = std::env::temp_dir().join(format!("greycard-turn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0002.CR3");
        let mut sidecar = Sidecar::default();
        sidecar.current.light.exposure = 0.25;

        // Nothing to say: no field in the file at all, so a sidecar
        // written before the turn existed and one written now for an
        // unturned frame are the same bytes.
        let json = serde_json::to_string(&sidecar).unwrap();
        assert!(!json.contains(r#""turn":"#), "{json}");

        sidecar.turn_by(-1, Some(1.5));
        assert_eq!(sidecar.turn, 3);
        sidecar.save(&raw).unwrap();
        let back = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(back.turn, 3);
        assert_eq!(back.current.light.exposure, 0.25);
        std::fs::remove_dir_all(&dir).ok();

        // Four turns are none, and the field goes again.
        let mut sidecar = Sidecar::default();
        for _ in 0..4 {
            sidecar.turn_by(1, Some(1.5));
        }
        assert_eq!(sidecar.turn, 0);
        assert!(
            !serde_json::to_string(&sidecar)
                .unwrap()
                .contains(r#""turn":"#)
        );

        // A turn is no part of an edit: a preset made from this one
        // cannot carry it onto the next frame.
        let mut sidecar = Sidecar::default();
        sidecar.turn_by(1, Some(1.5));
        assert!(!sidecar.current.to_json().contains(r#""turn":"#));
    }

    #[test]
    fn a_turn_this_build_cannot_read_is_no_turn_and_costs_no_edit() {
        // Loose, as the meta is (§117): a word, a number past three,
        // a list. None of them takes the edit down with it.
        for value in ["\"left\"", "[1]", "null", "-2", "true", "1.5"] {
            let json = format!(
                r#"{{"current":{{"version":3,"light":{{"exposure":0.5}}}},"turn":{value}}}"#
            );
            let back: Sidecar = serde_json::from_str(&json).unwrap();
            assert_eq!(back.turn, 0, "{value}");
            assert_eq!(back.current.light.exposure, 0.5, "{value}");
        }
        // A turn past three is the turn it comes to, not a refusal.
        let back: Sidecar = serde_json::from_str(r#"{"turn":6}"#).unwrap();
        assert_eq!(back.turn, 2);
        let back: Sidecar = serde_json::from_str(r#"{"turn":7}"#).unwrap();
        assert_eq!(back.turn, 3);
    }

    #[test]
    fn undo_after_a_turn_takes_back_the_edit_and_not_the_turn() {
        let mut sidecar = Sidecar::default();
        // A crop on the state that undo will hand back: every state
        // has to mean the same pixels as the current one, or undo
        // puts a pre-turn rectangle onto a turned frame.
        sidecar.current.geometry.crop = Some(geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        });
        let mut edit = sidecar.current.clone();
        edit.light.exposure = 1.0;
        sidecar.record(edit);
        sidecar.turn_by(1, Some(1.5));
        assert!(sidecar.undo());
        assert_eq!(sidecar.current.light.exposure, 0.0);
        assert_eq!(sidecar.turn, 1, "the frame is still the way up it was");
        assert_eq!(
            sidecar.current.geometry.crop,
            Some(geometry::Crop {
                x: 0.5,
                y: 0.0,
                w: 0.5,
                h: 0.5
            }),
            "the state undo handed back is the turned frame's"
        );
        // And a snapshot restored leaves it alone too.
        sidecar.take_snapshot("before", 0);
        sidecar.turn_by(1, Some(1.5));
        let mut later = Edit::default();
        later.light.exposure = 2.0;
        sidecar.record(later);
        assert!(sidecar.restore_snapshot(0));
        assert_eq!(sidecar.current.light.exposure, 0.0);
        assert_eq!(sidecar.turn, 2);
    }

    /// A mask and a repair patch cover the same pixels through every
    /// turn, on a landscape frame and on a portrait one, and in every
    /// state the sidecar holds. Positions are in units of the
    /// picture's *width*, both ways, so leaving them alone would
    /// slide them down the frame and change their size.
    #[test]
    fn a_mask_and_a_patch_cover_the_same_pixels_through_a_turn() {
        use mask::{Component, Mask, Shape};
        use retouch::{Method, Patch};

        // The reference: a pixel of a `w` by `h` picture, turned `q`
        // quarters clockwise about the picture, and the size it is
        // then in.
        fn turn_pixel(p: (f32, f32), w: f32, h: f32, q: i32) -> ((f32, f32), (f32, f32)) {
            let (mut p, mut w, mut h) = (p, w, h);
            for _ in 0..q.rem_euclid(4) {
                p = (h - p.1, p.0);
                std::mem::swap(&mut w, &mut h);
            }
            (p, (w, h))
        }

        let face = [0.48, 0.42];
        let spot = [0.31, 0.77];
        let make = || {
            let mut edit = Edit::default();
            edit.adjustments.push(Adjustment {
                id: 1,
                name: "Face".into(),
                mask: Mask {
                    components: vec![Component {
                        shape: Shape::Radial {
                            center: face,
                            radius: [0.12, 0.09],
                            angle: 20.0,
                            feather: 0.5,
                        },
                        ..Component::default()
                    }],
                    ..Mask::default()
                },
                ..Adjustment::default()
            });
            edit.retouch.patches.push(Patch {
                id: 1,
                method: Method::Heal,
                points: vec![spot],
                radius: 0.02,
                source: Some([0.05, -0.03]),
                ..Patch::default()
            });
            edit
        };

        for (w, h) in [(6000.0f32, 4000.0f32), (4000.0f32, 6000.0f32)] {
            for q in 1..4i32 {
                let mut sidecar = Sidecar {
                    current: make(),
                    ..Sidecar::default()
                };
                // One state behind it, one undone in front of it and
                // one kept by name: every state `edits_mut` covers
                // has to land on the same pixels.
                sidecar.history.push(make());
                sidecar.redo.push(make());
                sidecar.snapshots.push(Snapshot {
                    name: "before".into(),
                    taken: 0,
                    edit: make(),
                });
                sidecar.turn_by(q, Some(w / h));

                let (_, (nw, _)) = turn_pixel((0.0, 0.0), w, h, q);
                let want = |p: [f32; 2]| {
                    let (px, _) = turn_pixel((p[0] * w, p[1] * w), w, h, q);
                    [px.0 / nw, px.1 / nw]
                };
                let (wanted_face, wanted_spot) = (want(face), want(spot));
                let scale = w / nw;

                for edit in std::iter::once(&sidecar.current)
                    .chain(sidecar.history.iter())
                    .chain(sidecar.redo.iter())
                    .chain(sidecar.snapshots.iter().map(|s| &s.edit))
                {
                    let Shape::Radial {
                        center,
                        radius,
                        angle,
                        ..
                    } = &edit.adjustments[0].mask.components[0].shape
                    else {
                        panic!("the shape is still a radial");
                    };
                    assert!(
                        (center[0] - wanted_face[0]).abs() < 1e-4
                            && (center[1] - wanted_face[1]).abs() < 1e-4,
                        "{w}x{h} + {q}: {center:?} vs {wanted_face:?}"
                    );
                    assert!(
                        (radius[0] - 0.12 * scale).abs() < 1e-4
                            && (radius[1] - 0.09 * scale).abs() < 1e-4,
                        "{w}x{h} + {q}: {radius:?}"
                    );
                    // The ellipse turns with the picture: the angle
                    // grows clockwise on the screen, as `Shape::at`
                    // reads it, and stays within a half turn either
                    // way of level.
                    let wanted = {
                        let a = (20.0 + 90.0 * q as f32).rem_euclid(360.0);
                        if a > 180.0 { a - 360.0 } else { a }
                    };
                    assert!(
                        (angle - wanted).abs() < 1e-4,
                        "{w}x{h} + {q}: {angle} vs {wanted}"
                    );
                    let patch = &edit.retouch.patches[0];
                    assert!(
                        (patch.points[0][0] - wanted_spot[0]).abs() < 1e-4
                            && (patch.points[0][1] - wanted_spot[1]).abs() < 1e-4,
                        "{w}x{h} + {q}: {:?} vs {wanted_spot:?}",
                        patch.points[0]
                    );
                    assert!((patch.radius - 0.02 * scale).abs() < 1e-5);
                    // The source is an offset, so it turns without
                    // being moved: the healed pixels keep their
                    // source pixels.
                    let d = patch.source.expect("the source is kept");
                    let turned = mask::Turned::new(q, w / h).offset([0.05, -0.03]);
                    assert!((d[0] - turned[0]).abs() < 1e-5 && (d[1] - turned[1]).abs() < 1e-5);
                }
            }
        }

        // And all the way round is where it started.
        let mut sidecar = Sidecar {
            current: make(),
            ..Sidecar::default()
        };
        let (w, h) = (6000.0f32, 4000.0f32);
        let mut aspect = w / h;
        for _ in 0..4 {
            sidecar.turn_by(1, Some(aspect));
            aspect = 1.0 / aspect;
        }
        assert_eq!(sidecar.turn, 0);
        let Shape::Radial {
            center,
            radius,
            angle,
            ..
        } = &sidecar.current.adjustments[0].mask.components[0].shape
        else {
            panic!("a radial");
        };
        assert!((center[0] - face[0]).abs() < 1e-4 && (center[1] - face[1]).abs() < 1e-4);
        assert!((radius[0] - 0.12).abs() < 1e-5 && (radius[1] - 0.09).abs() < 1e-5);
        assert!((angle - 20.0).abs() < 1e-4);
    }

    /// With no picture to measure by, the geometry still turns — a
    /// crop is in fractions of the plane and needs no aspect — and
    /// anything placed on the picture stays where it is, which the
    /// caller is told about rather than left to find out.
    #[test]
    fn a_turn_with_no_aspect_moves_the_crop_and_says_what_it_left() {
        use mask::{Component, Mask};
        let mut sidecar = Sidecar::default();
        sidecar.current.geometry.crop = Some(geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        });
        assert!(!sidecar.placed(), "a crop is not placed on the picture");
        sidecar.current.adjustments.push(Adjustment {
            id: 1,
            mask: Mask {
                components: vec![Component::default()],
                ..Mask::default()
            },
            ..Adjustment::default()
        });
        assert!(sidecar.placed());
        let before = sidecar.current.adjustments[0].mask.clone();
        sidecar.turn_by(1, None);
        assert_eq!(sidecar.turn, 1);
        assert_eq!(
            sidecar.current.geometry.crop,
            Some(geometry::Crop {
                x: 0.5,
                y: 0.0,
                w: 0.5,
                h: 0.5
            })
        );
        assert_eq!(sidecar.current.adjustments[0].mask, before);
    }

    #[test]
    fn a_turn_carries_the_crop_with_the_frame() {
        let mut sidecar = Sidecar::default();
        sidecar.current.geometry.crop = Some(geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        });
        sidecar.turn_by(1, Some(1.5));
        assert_eq!(
            sidecar.current.geometry.crop,
            Some(geometry::Crop {
                x: 0.5,
                y: 0.0,
                w: 0.5,
                h: 0.5
            })
        );
        assert_eq!(sidecar.current.geometry.turns, 0);
    }

    #[test]
    fn a_sidecar_can_hold_meta_with_no_edit_and_an_edit_with_no_meta() {
        // Rated before it was ever developed: the frame opens at the
        // default edit.
        let only_meta: Sidecar = serde_json::from_str(r#"{"meta":{"rating":2}}"#).unwrap();
        assert_eq!(only_meta.meta.rating, 2);
        assert_eq!(only_meta.current, Edit::default());
        assert!(only_meta.history.is_empty());
        // Written before there was a meta section at all.
        let only_edit: Sidecar =
            serde_json::from_str(r#"{"current":{"version":3,"light":{"exposure":0.5}}}"#).unwrap();
        assert_eq!(only_edit.current.light.exposure, 0.5);
        assert!(only_edit.meta.is_empty());
        // And the meta is no part of an edit: a preset or a look
        // never sees one, since the field is not on `Edit`.
        let json = Edit::default().to_json();
        assert!(!json.contains("meta"), "{json}");
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

    #[test]
    fn undo_and_redo_walk_the_history() {
        let mut sidecar = Sidecar::default();
        for exposure in [1.0, 2.0, 3.0] {
            let mut e = Edit::default();
            e.light.exposure = exposure;
            sidecar.record(e);
        }
        assert!(sidecar.undo());
        assert_eq!(sidecar.current.light.exposure, 2.0);
        assert!(sidecar.undo());
        assert_eq!(sidecar.current.light.exposure, 1.0);
        assert!(sidecar.redo());
        assert_eq!(sidecar.current.light.exposure, 2.0);
        // A new state after an undo forgets what was undone.
        let mut e = Edit::default();
        e.light.exposure = 5.0;
        sidecar.record(e);
        assert!(!sidecar.redo());
        assert_eq!(sidecar.history.len(), 3);
        for _ in 0..3 {
            assert!(sidecar.undo());
        }
        assert!(!sidecar.undo());
        assert_eq!(sidecar.current, Edit::default());
    }

    #[test]
    fn go_to_walks_either_way_and_keeps_what_lies_past() {
        let mut sidecar = Sidecar::default();
        for exposure in [1.0, 2.0, 3.0] {
            let mut e = Edit::default();
            e.light.exposure = exposure;
            sidecar.record(e);
        }
        // Four states: the default, then 1, 2, 3; the current is last.
        assert_eq!(sidecar.states(), 4);
        assert_eq!(sidecar.position(), 3);
        assert!(sidecar.go_to(1));
        assert_eq!(sidecar.current.light.exposure, 1.0);
        assert_eq!(sidecar.position(), 1);
        assert_eq!(sidecar.states(), 4);
        // Every state is still there to read, in order.
        let exposures: Vec<f32> = (0..4)
            .map(|i| sidecar.state(i).unwrap().light.exposure)
            .collect();
        assert_eq!(exposures, [0.0, 1.0, 2.0, 3.0]);
        assert!(sidecar.state(4).is_none());
        assert!(sidecar.go_to(3));
        assert_eq!(sidecar.current.light.exposure, 3.0);
        assert!(!sidecar.go_to(3));
        // Past the end stops at the end.
        assert!(sidecar.go_to(0));
        assert!(sidecar.go_to(9));
        assert_eq!(sidecar.position(), 3);
    }

    #[test]
    fn the_cap_keeps_the_earliest_state() {
        let mut sidecar = Sidecar::default();
        let mut first = Edit::default();
        first.light.exposure = -9.0;
        sidecar.record(first.clone());
        for i in 0..(HISTORY + 20) {
            let mut e = Edit::default();
            e.light.exposure = i as f32;
            sidecar.record(e);
        }
        assert_eq!(sidecar.history.len(), HISTORY);
        assert_eq!(sidecar.history[0], Edit::default());
        assert_eq!(sidecar.history[1].light.exposure, 20.0);
    }

    #[test]
    fn snapshots_are_kept_named_restored_as_a_step_and_written() {
        let mut sidecar = Sidecar::default();
        let mut e = Edit::default();
        e.light.exposure = 1.0;
        sidecar.record(e.clone());
        assert_eq!(sidecar.take_snapshot("Bright", 1_700_000_000), 0);
        let mut e2 = Edit::default();
        e2.light.exposure = -1.0;
        sidecar.record(e2);
        // Restoring is a step forward, not a step back.
        assert!(sidecar.restore_snapshot(0));
        assert_eq!(sidecar.current, e);
        assert_eq!(sidecar.history.len(), 3);
        assert!(!sidecar.restore_snapshot(0));
        assert!(!sidecar.restore_snapshot(7));
        assert!(sidecar.undo());
        assert_eq!(sidecar.current.light.exposure, -1.0);
        // The snapshot is written and read back; an old sidecar
        // without any reads as none.
        let dir = std::env::temp_dir().join(format!("greycard-snap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0002.CR3");
        sidecar.save(&raw).unwrap();
        let back = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(back.snapshots, sidecar.snapshots);
        assert_eq!(back.snapshots[0].name, "Bright");
        std::fs::write(
            Sidecar::path_for(&raw),
            r#"{"current":{"version":1},"history":[]}"#,
        )
        .unwrap();
        let old = Sidecar::load(&raw).unwrap().unwrap();
        assert!(old.snapshots.is_empty());
        // A snapshot's edit is migrated as the current one is.
        std::fs::write(
            Sidecar::path_for(&raw),
            r#"{"current":{"version":1},"snapshots":[{"name":"x","taken":0,"edit":{"version":1,"noise":{"enabled":true}}}]}"#,
        )
        .unwrap();
        let migrated = Sidecar::load(&raw).unwrap().unwrap();
        assert!(migrated.snapshots[0].edit.noise.profiled);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
