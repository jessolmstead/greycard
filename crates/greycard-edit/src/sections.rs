//! The sections of an edit kept in the crate root: the display curve,
//! light and tone, white balance, noise, detail, sharpen, and demosaic.

use serde::{Deserialize, Serialize};

use greycard_core::TempTint;
use greycard_core::color::WhitePoint;
use greycard_core::develop::DemosaicMethod;
use greycard_core::develop::dehaze::DehazeOptions;
use greycard_core::develop::local_contrast::LocalContrastOptions;
use greycard_core::develop::sharpen::{self, Radius, SharpenOptions, Threshold};
use greycard_core::raw::{CfaPattern, Mosaic};

/// The display curve's way with color. The first two are the same
/// curve on a neutral and differ on a color as it rolls off toward
/// white; the third is another transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DisplayCurve {
    /// The curve on each channel on its own: a bright saturated color
    /// turns as its channels reach the shoulder at different points,
    /// blue toward magenta and red toward yellow.
    ///
    /// `norm`, the curve on the mean of the channels that Hold hue to
    /// white was before it was taken out unreleased, reads as this.
    #[default]
    #[serde(alias = "norm")]
    Channels,
    /// AgX, Blender's wide-gamut formation with its Punchy look: the
    /// primaries inset toward white and rotated, a sigmoid per channel
    /// in a log encoding, part of the per-channel hue shift kept and
    /// the primaries pushed back out, so a bright saturated color goes
    /// to white by the formation's path rather than the clip's.
    Agx,
}

impl DisplayCurve {
    pub const ALL: [DisplayCurve; 2] = [DisplayCurve::Channels, DisplayCurve::Agx];

    pub fn is_channels(&self) -> bool {
        *self == DisplayCurve::Channels
    }

    /// The name the LIGHT section's choice shows, and the history.
    pub fn name(self) -> &'static str {
        match self {
            DisplayCurve::Channels => "Per channel",
            DisplayCurve::Agx => "AgX",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The name in a sentence: "fitted under per channel".
    pub fn phrase(self) -> &'static str {
        match self {
            DisplayCurve::Channels => "per channel",
            DisplayCurve::Agx => "AgX",
        }
    }

    /// The word the sidecar writes for this curve, which is also what
    /// a fitted look table declares it was made under: `channels`,
    /// `agx`. Taken from the serde names so the two cannot drift.
    pub fn key(self) -> String {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::String(s)) => s,
            _ => unreachable!("a unit variant serializes as a string"),
        }
    }

    /// A curve from the word a sidecar or a table writes for it, its
    /// aliases included (`norm` reads as per channel), or the words the
    /// editor shows for it ("Per channel", "per channel"), case aside:
    /// a table someone tags by hand is tagged in whichever they saw.
    pub fn from_key(key: &str) -> Option<Self> {
        let key = key.trim();
        serde_json::from_value(serde_json::Value::String(key.to_ascii_lowercase()))
            .ok()
            .or_else(|| {
                Self::ALL.into_iter().find(|c| {
                    c.name().eq_ignore_ascii_case(key) || c.phrase().eq_ignore_ascii_case(key)
                })
            })
    }
}

/// Brightness and the tone curve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Light {
    /// The section's switch: off, the exposure and the tone sliders do
    /// nothing, and stay set for when it is on again. The base display
    /// curve under them is not the section's and stays.
    pub enabled: bool,
    /// Exposure, in stops.
    pub exposure: f32,
    pub tone: Tone,
    /// The baseline this exposure's zero stands for, in stops, where it
    /// is not the body's own ([`baseline::for_body`]): an edit written
    /// before the bodies had baselines of their own was made against
    /// [`baseline::BEFORE_BODIES`] and keeps it ([`migrate`]), so its
    /// picture does not move. `None`, the default and what a new edit
    /// has, is the body's. The global light's alone: a mask's exposure
    /// adds to the global's and has no zero of its own. Carried with
    /// the section from frame to frame, since its numbers were set
    /// against it; never by a preset, whose exposure is meant from the
    /// default of whatever frame it lands on.
    ///
    /// [`baseline::for_body`]: crate::baseline::for_body
    /// [`baseline::BEFORE_BODIES`]: crate::baseline::BEFORE_BODIES
    /// [`migrate`]: crate::migrate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<f32>,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            enabled: true,
            exposure: 0.0,
            tone: Tone::default(),
            baseline: None,
        }
    }
}

impl Light {
    /// What acts: this light, or with the switch off, no exposure and
    /// the sliders at neutral: the base curve alone, as for a raw with
    /// nothing done to it. Consumers apply this, so the switch is one
    /// test in one place.
    pub fn effective(&self) -> Light {
        if self.enabled {
            *self
        } else {
            Light {
                enabled: false,
                exposure: 0.0,
                tone: Tone::default(),
                // Not a slider: the zero the picture rests at.
                baseline: self.baseline,
            }
        }
    }
}

/// The shifts about the display tone curve. The curve itself, a
/// shoulder over the scene, is always there for a raw: it is part of
/// rendering one, not an edit. The shifts are by luminance and scale
/// all three channels alike, so a color keeps its hue.
///
/// Older sidecars and presets carry an `enabled` here, from when the
/// curve had a switch; it is ignored, so one saved with it off opens
/// with the curve on.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tone {
    /// Slope at mid grey relative to the base curve's: 1 leaves it,
    /// above steepens, below flattens.
    pub contrast: f32,
    /// Stops of shift on the bright side of the frame, placed by the
    /// frame's own range: a ramp over the span from the frame's median
    /// to its top, held to 1.6 to 3.5 stops and hung from the top
    /// (`highlights_span` in `finish.rs`). Read from the region the
    /// pixel sits in and not from the pixel: the tone equalizer. On a
    /// merge, its negative travel first pulls what the frame keeps
    /// over display white under it (`Kept` in `finish.rs`). The
    /// slider runs ±2.
    pub highlights: f32,
    /// Stops of shift in the middle of the frame, placed by the
    /// frame's own range: a bell full half a stop under the foot of
    /// the highlights span (the frame's median where the span is its
    /// own), nothing 1.75 stops either way from there
    /// (`midtones_weight` in `finish.rs`). By region, as highlights
    /// is, and ±2, where the middle moves 0.875 of a stop. Left out of a
    /// sidecar at zero, so every sidecar and key written before it is
    /// unchanged and an older build reads a newer file.
    #[serde(skip_serializing_if = "is_zero")]
    pub midtones: f32,
    /// Stops of shift under mid grey, full three and a half stops
    /// below it, none at it. By region too, and ±2.
    pub shadows: f32,
    /// The white point, in stops, pivoted at the foot of the
    /// highlights span (the frame's median where the span is the
    /// frame's own; under it on a frame narrower than 1.6 stops, over
    /// it on one wider than 3.5), held between 2.5 stops under mid grey
    /// and mid grey: a luminance this many stops under display white
    /// is brought to display white, and everything over the pivot with
    /// it, by a power on the luminance; nothing at or under the pivot
    /// moves. Positive clips
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
            contrast: 1.0,
            highlights: 0.0,
            midtones: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
        }
    }
}

/// The scene white.
///
/// Written as a `mode`, `as_shot` or `custom`, with a light beside
/// `custom`: `{"mode":"custom","temperature":3200.0,"tint":0.002}`. A
/// picture's offset is written beside `as_shot`,
/// `{"mode":"as_shot","offset":{"mired":10.0,"tint":0.0}}`, and not as
/// a mode of its own: every build before it refuses a sidecar whose
/// mode it does not know, and reads `as_shot` with the offset ignored,
/// so an older build opens the picture with its white balance off and
/// renders nothing wrong. A mode this build does not know reads as As
/// shot, so the next new one costs nothing.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(try_from = "WhiteWire", into = "WhiteWire")]
pub enum WhiteBalance {
    /// What the camera recorded; on a picture, its own white.
    #[default]
    AsShot,
    /// A correlated color temperature in kelvin and a tint as Duv,
    /// positive toward green. A raw's alone: a picture has no light to
    /// name.
    Custom { temperature: f64, tint: f64 },
    /// A picture's: its own white moved by `mired` toward the higher
    /// kelvin and `tint` in Duv, as a mask's offset from the global
    /// moves (`LocalWhite::Relative`). A raw's edit never holds one; a
    /// preset carrying one onto a raw is laid as the light it names at
    /// the raw's As shot, and one found on a raw anyway develops as As
    /// shot, as an older build reads it.
    Offset { mired: f64, tint: f64 },
}

/// How a [`WhiteBalance`] is written.
#[derive(Serialize, Deserialize)]
struct WhiteWire {
    mode: WhiteMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tint: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offset: Option<WhiteOffset>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WhiteMode {
    AsShot,
    Custom,
    /// A mode a later build writes: As shot here.
    #[serde(other)]
    Unknown,
}

/// An offset as written, each field none when left out.
#[derive(Serialize, Deserialize)]
struct WhiteOffset {
    #[serde(default)]
    mired: f64,
    #[serde(default)]
    tint: f64,
}

impl TryFrom<WhiteWire> for WhiteBalance {
    type Error = String;

    fn try_from(w: WhiteWire) -> std::result::Result<Self, String> {
        Ok(match (w.mode, w.offset) {
            // An offset of none is As shot, so a hand edit or a preset
            // that writes one reads as the picture's own white, the
            // same edit and no step.
            (WhiteMode::AsShot, Some(o)) => Self::offset_of(o.mired, o.tint),
            (WhiteMode::AsShot | WhiteMode::Unknown, _) => Self::AsShot,
            // As before the offset: a light without its numbers is no
            // light, and the sidecar says so rather than guessing one.
            (WhiteMode::Custom, _) => Self::Custom {
                temperature: w.temperature.ok_or("missing field `temperature`")?,
                tint: w.tint.ok_or("missing field `tint`")?,
            },
        })
    }
}

impl From<WhiteBalance> for WhiteWire {
    fn from(wb: WhiteBalance) -> Self {
        let wire = |mode, temperature, tint, offset| WhiteWire {
            mode,
            temperature,
            tint,
            offset,
        };
        match wb {
            WhiteBalance::AsShot => wire(WhiteMode::AsShot, None, None, None),
            WhiteBalance::Custom { temperature, tint } => {
                wire(WhiteMode::Custom, Some(temperature), Some(tint), None)
            }
            WhiteBalance::Offset { mired, tint } => wire(
                WhiteMode::AsShot,
                None,
                None,
                Some(WhiteOffset { mired, tint }),
            ),
        }
    }
}

impl WhiteBalance {
    /// What the develop of a raw resolves: an offset, which no raw's
    /// edit holds, is As shot there, as an older build reads it.
    pub fn white_point(&self) -> WhitePoint {
        match *self {
            Self::AsShot | Self::Offset { .. } => WhitePoint::AsShot,
            Self::Custom { temperature, tint } => WhitePoint::TempTint(TempTint {
                cct: temperature,
                duv: tint,
            }),
        }
    }

    /// The offset of zero: As shot, which on a picture is its own
    /// white.
    pub fn offset_of(mired: f64, tint: f64) -> Self {
        if mired == 0.0 && tint == 0.0 {
            Self::AsShot
        } else {
            Self::Offset { mired, tint }
        }
    }
}

/// The panel's word for the learned blend's chroma slider, in one
/// place so it is renamed in one place: the history's rows read it, as
/// the panel's slider reads its constant in the Slint.
pub const LEARNED_CHROMA: &str = "Chroma";

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
    /// not run the network again. Since the chroma has a strength of
    /// its own, this one is the luminance's.
    pub learned_strength: f32,
    /// How much of the learned answer's color shows, 0 to 1: the
    /// chroma's share of the same blend, so the color noise can go
    /// while the grain stays. Full by default and left out of a
    /// sidecar there; an edit from before it had the two strengths as
    /// one, and schema version 7 writes it that edit's blend.
    #[serde(skip_serializing_if = "is_one")]
    pub learned_chroma_strength: f32,
}

impl Default for Noise {
    fn default() -> Self {
        Self {
            enabled: true,
            profiled: false,
            strength: 1.5,
            learned: Learned::Off,
            learned_strength: 1.0,
            learned_chroma_strength: 1.0,
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

/// The panel's word for the Detail section's widest band, in one place
/// so it is renamed in one place: the history's rows and the copy
/// settings sheet's part read it, as the panel's slider reads its
/// constant in the Slint.
pub const STRUCTURE: &str = "Structure";

/// Local contrast: the engine's Texture, Clarity, and Structure, each
/// -1 to 1 (the panel shows ±100), negative smoothing instead.
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
    /// The widest scale, soft-kneed. Left out of a sidecar at zero, so
    /// every sidecar and develop key written before it is unchanged.
    #[serde(skip_serializing_if = "is_zero")]
    pub structure: f32,
    /// Haze removal, the engine's dark channel prior: -1 to 1 (the
    /// panel shows ±100), how much haze to take out; negative puts
    /// haze in. Zero is nothing. Runs after the local contrast and
    /// before the sharpen, and follows the section's switch.
    pub dehaze: f32,
    /// A mask's share of the capture sharpen, -1 to 1 (the panel shows
    /// ±100): the sharpen's change taken off under the mask at -1, put
    /// on once more at 1, by the global's radius, iterations, and
    /// threshold. Read only on a mask's look; the picture's sharpen is
    /// its own section. Left out of a sidecar at zero.
    #[serde(skip_serializing_if = "is_zero")]
    pub sharpen: f32,
}

impl Default for Detail {
    fn default() -> Self {
        Self {
            enabled: true,
            texture: 0.0,
            clarity: 0.0,
            structure: 0.0,
            dehaze: 0.0,
            sharpen: 0.0,
        }
    }
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

impl Detail {
    /// Whether this is the default: on, every slider at zero.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The engine's options, or none when off or when the three
    /// sliders rest, which is the same picture.
    pub fn options(&self) -> Option<LocalContrastOptions> {
        let options = LocalContrastOptions {
            texture: self.texture.clamp(-1.0, 1.0),
            clarity: self.clarity.clamp(-1.0, 1.0),
            structure: self.structure.clamp(-1.0, 1.0),
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
    /// The threshold otherwise, a fraction, 0 to 2 (the panel shows 0
    /// to 200 percent, RawTherapee's range and units). Not clamped on
    /// reading: a build whose panel stopped at 0.5 develops a higher
    /// one as written.
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
        self.enabled.then(|| self.settings())
    }

    /// The engine's options whether the section is on or off: what a
    /// mask's Sharpen deconvolves by when the picture is not sharpened.
    pub fn settings(&self) -> SharpenOptions {
        SharpenOptions {
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
        }
    }
}

/// The demosaic, by the engine's names.
///
/// What is stored is what was chosen, for whatever frame it was chosen
/// on; a choice the open frame's mosaic does not take (a Bayer method
/// on an X-Trans frame, from an older sidecar's default or a sync) is
/// resolved to the mosaic's default by the engine at the develop, shown
/// so in the editor's menu through [`Demosaic::resolve`], and never
/// written back on its own. The Markesteijn names are schema 5's, which
/// no released build reads (0.6 reads up to 4), so they took no bump of
/// their own.
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
    #[serde(rename = "markesteijn-1")]
    Markesteijn1,
    #[serde(rename = "markesteijn-3")]
    Markesteijn3,
}

impl Demosaic {
    pub const ALL: &[Demosaic] = &[
        Demosaic::AmazeVng4,
        Demosaic::RcdVng4,
        Demosaic::Amaze,
        Demosaic::Rcd,
        Demosaic::Vng4,
        Demosaic::Bilinear,
        Demosaic::Markesteijn1,
        Demosaic::Markesteijn3,
    ];

    pub fn method(self) -> DemosaicMethod {
        match self {
            Demosaic::AmazeVng4 => DemosaicMethod::AmazeVng4,
            Demosaic::RcdVng4 => DemosaicMethod::RcdVng4,
            Demosaic::Amaze => DemosaicMethod::Amaze,
            Demosaic::Rcd => DemosaicMethod::Rcd,
            Demosaic::Vng4 => DemosaicMethod::Vng4,
            Demosaic::Bilinear => DemosaicMethod::Bilinear,
            Demosaic::Markesteijn1 => DemosaicMethod::Markesteijn1,
            Demosaic::Markesteijn3 => DemosaicMethod::Markesteijn3,
        }
    }

    /// The edit's name for an engine method.
    pub fn from_method(method: DemosaicMethod) -> Self {
        Self::from_name(method.name()).unwrap_or_default()
    }

    /// The choices for a frame of `pattern`'s mosaic, its default
    /// first: what a menu offers.
    pub fn for_pattern(pattern: &CfaPattern) -> Vec<Demosaic> {
        DemosaicMethod::for_pattern(pattern)
            .iter()
            .map(|m| Self::from_method(*m))
            .collect()
    }

    /// The choices for a frame of a `mosaic`, its default first.
    pub fn for_kind(mosaic: Mosaic) -> Vec<Demosaic> {
        DemosaicMethod::for_kind(mosaic)
            .iter()
            .map(|m| Self::from_method(*m))
            .collect()
    }

    /// What a develop of a frame of a `mosaic` runs for this choice
    /// ([`DemosaicMethod::resolve`]).
    pub fn resolve(self, mosaic: Mosaic) -> Self {
        Self::from_method(self.method().resolve(mosaic))
    }

    /// What a develop of a `pattern` frame runs for this choice: the
    /// engine's resolve ([`DemosaicMethod::for_mosaic`]), for a menu to
    /// show what will run.
    pub fn for_mosaic(self, pattern: &CfaPattern) -> Self {
        Self::from_method(self.method().for_mosaic(pattern))
    }

    /// The engine's name for it, which is also the serialized form.
    pub fn name(self) -> &'static str {
        self.method().name()
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|d| d.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::WhiteBalance;

    /// Midtones came in with no new version: at zero it is not
    /// written, so a tone serializes as it did before it, byte for
    /// byte, and an edit from before it reads with it at zero and
    /// writes back the same; set, it is written and read back.
    #[test]
    fn midtones_at_zero_is_not_written_and_the_tone_is_as_before() {
        let tone = super::Tone {
            contrast: 1.2,
            highlights: -0.5,
            shadows: 0.25,
            ..super::Tone::default()
        };
        assert_eq!(
            serde_json::to_string(&tone).unwrap(),
            r#"{"contrast":1.2,"highlights":-0.5,"shadows":0.25,"whites":0.0,"blacks":0.0}"#
        );
        let old = r#"{"version": 8, "light": {"exposure": 0.5, "tone": {"contrast": 1.2, "highlights": -0.5}}}"#;
        let e = crate::Edit::from_json(old).unwrap();
        assert_eq!(e.light.tone.midtones, 0.0);
        let json = e.to_json();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value["light"]["tone"].get("midtones").is_none(), "{json}");
        assert_eq!(crate::Edit::from_json(&json).unwrap().to_json(), json);
        assert_eq!(
            e.develop_key_with(&[], None),
            crate::Edit::from_json(&json)
                .unwrap()
                .develop_key_with(&[], None)
        );

        let mut set = e.clone();
        set.light.tone.midtones = 0.75;
        let json = set.to_json();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["light"]["tone"]["midtones"].as_f64(), Some(0.75));
        assert_eq!(crate::Edit::from_json(&json).unwrap(), set);
        assert_eq!(
            serde_json::to_string(&set.light.tone).unwrap(),
            r#"{"contrast":1.2,"highlights":-0.5,"midtones":0.75,"shadows":0.0,"whites":0.0,"blacks":0.0}"#
        );
    }

    /// As shot and a light are written as they always were; an offset
    /// rides beside As shot; a mode this build does not know is As
    /// shot; and a light without its numbers is refused, as before.
    #[test]
    fn a_white_balance_is_written_as_it_was_and_an_offset_beside_as_shot() {
        let cases = [
            (WhiteBalance::AsShot, r#"{"mode":"as_shot"}"#),
            (
                WhiteBalance::Custom {
                    temperature: 3200.0,
                    tint: 0.002,
                },
                r#"{"mode":"custom","temperature":3200.0,"tint":0.002}"#,
            ),
            (
                WhiteBalance::Offset {
                    mired: 10.0,
                    tint: 0.0,
                },
                r#"{"mode":"as_shot","offset":{"mired":10.0,"tint":0.0}}"#,
            ),
        ];
        for (wb, json) in cases {
            assert_eq!(serde_json::to_string(&wb).unwrap(), json);
            assert_eq!(serde_json::from_str::<WhiteBalance>(json).unwrap(), wb);
        }
        let later = r#"{"mode":"from_a_card","temperature":1.0}"#;
        assert_eq!(
            serde_json::from_str::<WhiteBalance>(later).unwrap(),
            WhiteBalance::AsShot
        );
        assert!(serde_json::from_str::<WhiteBalance>(r#"{"mode":"custom","tint":0.0}"#).is_err());
        let none = r#"{"mode":"as_shot","offset":{"mired":0.0,"tint":0.0}}"#;
        assert_eq!(
            serde_json::from_str::<WhiteBalance>(none).unwrap(),
            WhiteBalance::AsShot
        );
        let half = r#"{"mode":"as_shot","offset":{"tint":0.004}}"#;
        assert_eq!(
            serde_json::from_str::<WhiteBalance>(half).unwrap(),
            WhiteBalance::Offset {
                mired: 0.0,
                tint: 0.004
            }
        );
    }

    /// A sidecar with an offset, read the way every build before it
    /// reads the white balance (its enum, tagged by mode, as_shot or
    /// custom and nothing else), opens with As shot and the rest of
    /// the edit kept.
    #[test]
    fn an_older_build_reads_an_offset_as_as_shot() {
        #[derive(Debug, PartialEq, serde::Deserialize)]
        #[serde(tag = "mode", rename_all = "snake_case")]
        enum Before {
            AsShot,
            Custom { temperature: f64, tint: f64 },
        }
        #[derive(serde::Deserialize)]
        struct BeforeEdit {
            white_balance: Before,
            light: serde_json::Value,
        }
        let mut edit = crate::Edit::for_picture();
        edit.white_balance = WhiteBalance::Offset {
            mired: -35.0,
            tint: 0.003,
        };
        edit.light.exposure = 0.5;
        let json = edit.to_json();
        let old: BeforeEdit = serde_json::from_str(&json).unwrap();
        assert_eq!(old.white_balance, Before::AsShot);
        assert_eq!(old.light["exposure"], 0.5);
        // And this build reads it back whole, at the version it was.
        let back = crate::Edit::from_json(&json).unwrap();
        assert_eq!(back, edit);
        assert_eq!(back.version, crate::VERSION);
    }

    use super::*;
    use crate::{Edit, Error, Section, VERSION, describe};

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

    /// The display curve's switch is left out of a sidecar at its
    /// default, so a picture nobody switched writes what it always
    /// did, and read back either way; a preset or a copy of the light
    /// leaves it as it is on the picture, and the history names it. A
    /// sidecar from before Hold hue to white was taken out, `norm`,
    /// reads as per channel.
    #[test]
    fn the_display_curve_switch_is_written_only_when_thrown() {
        let edit = Edit::default();
        assert!(!edit.to_json().contains("display_curve"));
        let mut agx = edit.clone();
        agx.display_curve = DisplayCurve::Agx;
        let json = agx.to_json();
        assert!(json.contains(r#""display_curve": "agx""#), "{json}");
        assert_eq!(Edit::from_json(&json).unwrap(), agx);
        let mut onto = agx.clone();
        Section::Light.copy(&edit, &mut onto);
        assert_eq!(onto.display_curve, DisplayCurve::Agx);
        let mut onto = Edit::default();
        Section::Light.copy(&agx, &mut onto);
        assert_eq!(onto.display_curve, DisplayCurve::Channels);
        assert!(Section::Light.same(&edit, &agx));
        assert_eq!(describe(&edit, &agx, &[]), "Display curve: AgX");
        assert_eq!(describe(&agx, &edit, &[]), "Display curve: Per channel");
        let old = agx.to_json().replace(r#""agx""#, r#""norm""#);
        assert_eq!(
            Edit::from_json(&old).unwrap().display_curve,
            DisplayCurve::Channels
        );
        for c in DisplayCurve::ALL {
            assert_eq!(DisplayCurve::from_name(c.name()), Some(c));
            assert_eq!(DisplayCurve::from_key(&c.key()), Some(c));
        }
        // The key is the sidecar's word, which a fitted table also
        // declares.
        assert_eq!(DisplayCurve::Channels.key(), "channels");
        assert_eq!(DisplayCurve::Agx.key(), "agx");
        assert_eq!(DisplayCurve::from_key(" AgX "), Some(DisplayCurve::Agx));
        assert_eq!(DisplayCurve::from_key("norm"), Some(DisplayCurve::Channels));
        assert_eq!(DisplayCurve::from_key("aces"), None);
        assert_eq!(
            DisplayCurve::from_key("Per Channel"),
            Some(DisplayCurve::Channels)
        );
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
        assert_eq!(Edit::from_json(&e.to_json()).unwrap(), e);
    }

    #[test]
    fn structure_is_in_the_sidecar_and_the_key_only_when_set() {
        // At zero the section writes what it always wrote, so a
        // sidecar and a develop key from before are unchanged.
        let mut e = Edit::default();
        e.detail.clarity = 0.3;
        let json: serde_json::Value = serde_json::from_str(&e.to_json()).unwrap();
        assert!(json["detail"].get("structure").is_none(), "{json}");
        let before = e.develop_key_with(&[], None);
        // Set, it is written, read back, asked of the engine, and
        // another picture.
        e.detail.structure = -0.4;
        let json: serde_json::Value = serde_json::from_str(&e.to_json()).unwrap();
        assert!(json["detail"].get("structure").is_some(), "{json}");
        assert_eq!(Edit::from_json(&e.to_json()).unwrap(), e);
        assert_eq!(e.detail.options().unwrap().structure, -0.4);
        assert_ne!(e.develop_key_with(&[], None), before);
        e.detail.structure = 0.0;
        assert_eq!(e.develop_key_with(&[], None), before);
        // Alone it asks the engine for the op; past the end it is held.
        let mut e = Edit::default();
        e.detail.structure = 2.0;
        assert_eq!(e.detail.options().unwrap().structure, 1.0);
        // A sidecar from a build before it reads as none.
        let old = r#"{"version": 3, "detail": {"texture": 0.2, "clarity": 0.1}}"#;
        assert_eq!(Edit::from_json(old).unwrap().detail.structure, 0.0);
    }

    /// The learned denoiser's chroma blend: full it is left out, so a
    /// sidecar and a develop key at the default are what they were;
    /// below full it is written and read back.
    #[test]
    fn the_learned_chroma_blend_is_in_the_sidecar_only_below_full() {
        let mut e = Edit::default();
        e.noise.learned = crate::Learned::Best;
        e.noise.learned_strength = 0.4;
        let json: serde_json::Value = serde_json::from_str(&e.to_json()).unwrap();
        assert!(
            json["noise"].get("learned_chroma_strength").is_none(),
            "{json}"
        );
        let before = e.develop_key_with(&[], None);
        e.noise.learned_chroma_strength = 0.7;
        let json: serde_json::Value = serde_json::from_str(&e.to_json()).unwrap();
        assert!(
            json["noise"].get("learned_chroma_strength").is_some(),
            "{json}"
        );
        assert_eq!(Edit::from_json(&e.to_json()).unwrap(), e);
        assert_ne!(e.develop_key_with(&[], None), before);
        let fresh = r#"{"version": 7, "noise": {"learned": "best", "learned_strength": 0.4}}"#;
        assert_eq!(
            Edit::from_json(fresh)
                .unwrap()
                .noise
                .learned_chroma_strength,
            1.0
        );
    }

    /// A mask's Sharpen in the sidecar: absent it reads as zero, at
    /// zero it is left out so the file and the develop key are what
    /// they were, and set it is written and read back as it was.
    #[test]
    fn a_masks_sharpen_round_trips_absent_zero_and_set() {
        let mut e = Edit::default();
        e.adjustments.push(crate::Adjustment {
            id: 1,
            ..Default::default()
        });
        e.adjustments[0].look.detail.texture = -0.3;
        let json: serde_json::Value = serde_json::from_str(&e.to_json()).unwrap();
        let detail = &json["adjustments"][0]["look"]["detail"];
        assert!(detail.get("texture").is_some(), "{json}");
        assert!(detail.get("sharpen").is_none(), "zero is left out: {json}");
        let before = e.develop_key_with(&[], None);
        // Absent: a sidecar from before the field.
        let mut old = json.clone();
        old["adjustments"][0]["look"]["detail"]
            .as_object_mut()
            .unwrap()
            .remove("sharpen");
        let read = Edit::from_json(&old.to_string()).unwrap();
        assert_eq!(read.adjustments[0].look.detail.sharpen, 0.0);
        assert_eq!(read, e);
        // Set: written, read back, and another develop.
        e.adjustments[0].look.detail.sharpen = -0.85;
        let json: serde_json::Value = serde_json::from_str(&e.to_json()).unwrap();
        let written = json["adjustments"][0]["look"]["detail"]["sharpen"].as_f64();
        assert!(written.is_some_and(|v| (v + 0.85).abs() < 1e-6), "{json}");
        assert_eq!(Edit::from_json(&e.to_json()).unwrap(), e);
        assert_ne!(e.develop_key_with(&[], None), before);
        e.adjustments[0].look.detail.sharpen = 0.0;
        assert_eq!(e.develop_key_with(&[], None), before);
        // The picture's own Detail never writes one.
        let json: serde_json::Value = serde_json::from_str(&Edit::default().to_json()).unwrap();
        assert!(json["detail"].get("sharpen").is_none(), "{json}");
    }

    /// The sharpen's threshold at the panel's top, 200 percent: written
    /// as the fraction, read back as it was, and handed to the engine
    /// unclamped; and one from a sidecar written at the old top of 50
    /// percent reads and develops as it did.
    #[test]
    fn a_sharpen_threshold_of_200_percent_round_trips() {
        let mut e = Edit::default();
        e.sharpen.auto_threshold = false;
        e.sharpen.threshold = 2.0;
        let json: serde_json::Value = serde_json::from_str(&e.to_json()).unwrap();
        assert_eq!(json["sharpen"]["threshold"].as_f64(), Some(2.0), "{json}");
        let read = Edit::from_json(&e.to_json()).unwrap();
        assert_eq!(read, e);
        assert_eq!(read.sharpen.settings().contrast, Threshold::Fixed(2.0));
        let old = r#"{"version": 5, "sharpen": {"auto_threshold": false, "threshold": 0.5}}"#;
        let old = Edit::from_json(old).unwrap();
        assert_eq!(old.sharpen.settings().contrast, Threshold::Fixed(0.5));
        assert_ne!(
            old.develop_key_with(&[], None),
            e.develop_key_with(&[], None)
        );
    }

    #[test]
    fn a_sections_switch_off_is_the_section_undone() {
        let mut edit = Edit::default();
        edit.light.exposure = 2.0;
        edit.light.tone.contrast = 1.5;
        edit.light.enabled = false;
        let acting = edit.light.effective();
        assert_eq!(acting.exposure, 0.0);
        assert_eq!(acting.tone, Tone::default(), "the sliders at neutral");
        assert_eq!(edit.light.exposure, 2.0, "and the edit's are kept");
        assert_eq!(edit.light.tone.contrast, 1.5);

        edit.noise.learned = Learned::Fast;
        edit.noise.profiled = true;
        assert!(edit.noise.tier().is_some());
        assert!(!edit.noise.profiled_runs(), "the tier stands in for it");
        edit.noise.enabled = false;
        assert!(edit.noise.tier().is_none());
        assert!(edit.settings(&crate::NoStores).denoise.is_none());

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
    fn demosaic_names_are_the_engines() {
        for d in Demosaic::ALL {
            assert_eq!(Demosaic::from_name(d.name()), Some(*d));
            let json = serde_json::to_string(d).unwrap();
            assert_eq!(json, format!("\"{}\"", d.name()));
        }
        assert_eq!(DemosaicMethod::ALL.len(), Demosaic::ALL.len());
    }

    /// The Markesteijn choices go into a sidecar by the engine's names
    /// and come back; a Bayer frame's stored choice and an X-Trans
    /// frame's show what the develop will run.
    #[test]
    fn x_trans_choices_round_trip_and_resolve_by_the_mosaic() {
        for d in [Demosaic::Markesteijn1, Demosaic::Markesteijn3] {
            let edit = Edit {
                demosaic: d,
                ..Edit::default()
            };
            let json = serde_json::to_string_pretty(&edit).unwrap();
            assert!(
                json.contains(&format!("\"demosaic\": \"{}\"", d.name())),
                "{json}"
            );
            let back = Edit::from_json(&json).unwrap();
            assert_eq!(back.demosaic, d);
            assert_eq!(back.version, VERSION);
        }
        let xtrans = CfaPattern::xtrans();
        let bayer = CfaPattern::rggb();
        assert_eq!(
            Demosaic::for_pattern(&xtrans),
            vec![
                Demosaic::Markesteijn3,
                Demosaic::Markesteijn1,
                Demosaic::Bilinear
            ]
        );
        assert_eq!(Demosaic::for_pattern(&bayer)[0], Demosaic::AmazeVng4);
        assert_eq!(
            Demosaic::AmazeVng4.for_mosaic(&xtrans),
            Demosaic::Markesteijn3
        );
        assert_eq!(
            Demosaic::Markesteijn1.for_mosaic(&bayer),
            Demosaic::AmazeVng4
        );
        assert_eq!(
            Demosaic::Markesteijn1.for_mosaic(&xtrans),
            Demosaic::Markesteijn1
        );
        assert_eq!(Demosaic::Rcd.for_mosaic(&bayer), Demosaic::Rcd);
        // The engine is handed what was stored; it resolves at the
        // develop, so the stored choice is never rewritten by a read.
        let edit = Edit::default();
        assert_eq!(
            edit.settings(&crate::NoStores).demosaic,
            DemosaicMethod::AmazeVng4
        );
    }

    /// A Markesteijn name is written at schema 5 and read back; an
    /// unknown name is an error rather than a wrong demosaic. That a
    /// released build refuses the file is not tested here: 0.6 reads up
    /// to schema 4, and every write of this build is at 5.
    #[test]
    fn a_markesteijn_sidecar_is_schema_five() {
        const { assert!(VERSION >= 5) };
        let json = r#"{"version": 5, "demosaic": "markesteijn-3"}"#;
        assert_eq!(
            Edit::from_json(json).unwrap().demosaic,
            Demosaic::Markesteijn3
        );
        let unknown = Edit::from_json(r#"{"version": 5, "demosaic": "markesteijn-9"}"#);
        assert!(matches!(unknown, Err(Error::Json(_))));
    }
}
