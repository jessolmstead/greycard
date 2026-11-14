//! Presets: a named part of an edit kept in a file of its own, to be
//! laid over any picture's edit.
//!
//! A preset says which [`Section`]s it carries and holds a whole
//! [`Edit`] with those sections set, the rest at their defaults; laid
//! over an edit, the carried sections replace the picture's and the
//! rest is left alone. The file is JSON, `.gcp` beside the sidecar's
//! `.gcd`, under the user's configuration directory; the edit inside
//! is migrated as a sidecar's is, so a preset outlives a schema.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::migration::refused;
use crate::{Edit, Error, Result, VERSION, migrate};

/// The parts of an edit a preset can carry. Not the geometry nor the
/// retouch, which are one picture's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Section {
    Light,
    /// The display curve, per channel or AgX: one switch for the whole
    /// rendering, shown under Light in the panel.
    DisplayCurve,
    WhiteBalance,
    Noise,
    /// The Detail section: Texture, Clarity, Structure, and Dehaze.
    Detail,
    Sharpen,
    /// The unsharp mask on the finished picture.
    OutputSharpen,
    Curves,
    Mixer,
    Color,
    #[serde(rename = "black-and-white")]
    BlackWhite,
    Grading,
    Tint,
    /// The look table the picture finishes through.
    Look,
    Lens,
    Vignette,
    Grain,
    Demosaic,
    /// The camera profile the develop starts from.
    Camera,
    Adjustments,
}

impl Section {
    /// In the panel's order.
    pub const ALL: &[Section] = &[
        Section::Look,
        Section::WhiteBalance,
        Section::Light,
        Section::DisplayCurve,
        Section::Color,
        Section::Curves,
        Section::Mixer,
        Section::BlackWhite,
        Section::Grading,
        Section::Tint,
        Section::Noise,
        Section::Lens,
        Section::Detail,
        Section::Sharpen,
        Section::OutputSharpen,
        Section::Vignette,
        Section::Grain,
        Section::Demosaic,
        Section::Camera,
        Section::Adjustments,
    ];

    /// The serialized name.
    pub fn name(self) -> &'static str {
        match self {
            Section::Light => "light",
            Section::DisplayCurve => "display-curve",
            Section::WhiteBalance => "white-balance",
            Section::Noise => "noise",
            Section::Detail => "detail",
            Section::OutputSharpen => "output-sharpen",
            Section::Sharpen => "sharpen",
            Section::Curves => "curves",
            Section::Mixer => "mixer",
            Section::Color => "color",
            Section::BlackWhite => "black-and-white",
            Section::Grading => "grading",
            Section::Tint => "tint",
            Section::Look => "look",
            Section::Lens => "lens",
            Section::Vignette => "vignette",
            Section::Grain => "grain",
            Section::Demosaic => "demosaic",
            Section::Camera => "camera",
            Section::Adjustments => "adjustments",
        }
    }

    /// The panel's word for it.
    pub fn title(self) -> &'static str {
        match self {
            Section::Light => "Light",
            Section::DisplayCurve => "Display curve",
            Section::WhiteBalance => "White balance",
            Section::Noise => "Noise",
            Section::Detail => "Detail",
            Section::OutputSharpen => "Output sharpen",
            Section::Sharpen => "Sharpen",
            Section::Curves => "Curves",
            Section::Mixer => "Color mixer",
            Section::Color => "Color",
            Section::BlackWhite => "Black and white",
            Section::Grading => "Color grading",
            Section::Tint => "Color tint",
            Section::Look => "Look",
            Section::Lens => "Lens",
            Section::Vignette => "Vignette",
            Section::Grain => "Grain",
            Section::Demosaic => "Demosaic",
            Section::Camera => "Camera profile",
            Section::Adjustments => "Masks",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.name() == name)
    }

    /// Whether a preset saved without a say carries it: the look and
    /// the finish, not what is one picture's own (its white, its lens,
    /// its demosaic) nor its masks.
    pub fn by_default(self) -> bool {
        !matches!(
            self,
            Section::WhiteBalance
                | Section::Lens
                | Section::Demosaic
                | Section::Camera
                | Section::Adjustments
        )
    }

    /// Whether a sync across a selection carries it unless told not
    /// to: all but the masks. A sync is between frames of one shoot,
    /// so the white, the lens, the demosaic and the camera profile
    /// that a preset leaves alone are what a sync is for; the masks
    /// are drawn on one picture's subject and land on nothing in the
    /// next. The geometry and the retouch are not sections at all.
    /// A named camera profile is ticked but the editor lays it only
    /// on frames of the body it was made for (`panel::sync`).
    pub fn syncs_by_default(self) -> bool {
        self != Section::Adjustments
    }

    /// Lay this section of `from` over `onto`.
    pub fn copy(self, from: &Edit, onto: &mut Edit) {
        match self {
            // Not the display curve's switch, which is a section of its
            // own: a sidecar leaves it out at its default, so a preset
            // of Light made before the switch and one made from a
            // picture per channel read alike, and carrying it with
            // Light would turn AgX off on every picture such a preset is
            // laid on.
            Section::Light => onto.light = from.light,
            Section::DisplayCurve => onto.display_curve = from.display_curve,
            Section::WhiteBalance => onto.white_balance = from.white_balance.clone(),
            // The profiled denoiser only: `learned` and
            // `learned_strength` are the file's own, defaulted from
            // its ISO (`Noise::blend_for_iso`), not a look to carry
            // from picture to picture, so a preset never touches them.
            Section::Noise => {
                onto.noise.enabled = from.noise.enabled;
                onto.noise.profiled = from.noise.profiled;
                onto.noise.strength = from.noise.strength;
            }
            Section::Detail => onto.detail = from.detail,
            Section::OutputSharpen => onto.output_sharpen = from.output_sharpen,
            Section::Sharpen => onto.sharpen = from.sharpen,
            Section::Curves => onto.curves = from.curves.clone(),
            Section::Mixer => onto.mixer = from.mixer,
            Section::Color => onto.color = from.color,
            Section::BlackWhite => onto.bw = from.bw,
            Section::Grading => onto.grading = from.grading,
            Section::Tint => onto.tint = from.tint,
            Section::Look => onto.look_lut = from.look_lut.clone(),
            Section::Lens => onto.lens = from.lens.clone(),
            Section::Vignette => onto.vignette = from.vignette,
            Section::Grain => onto.grain = from.grain,
            Section::Demosaic => onto.demosaic = from.demosaic,
            // A DCP lands with its look table (`Camera::take`).
            Section::Camera => onto.camera.take(&from.camera),
            Section::Adjustments => {
                // The preset's masks in place of the picture's, with
                // ids of their own.
                onto.adjustments = from
                    .adjustments
                    .iter()
                    .enumerate()
                    .map(|(i, a)| crate::Adjustment {
                        id: i as u64 + 1,
                        ..a.clone()
                    })
                    .collect();
            }
        }
    }

    /// Whether this section reads the same in both edits.
    pub fn same(self, a: &Edit, b: &Edit) -> bool {
        match self {
            // Not the zero an older edit keeps, which no slider shows: a
            // preset whose sliders match reads as applied.
            Section::Light => {
                let (mut a, mut b) = (a.light, b.light);
                (a.baseline, b.baseline) = (None, None);
                a == b
            }
            Section::DisplayCurve => a.display_curve == b.display_curve,
            Section::WhiteBalance => a.white_balance == b.white_balance,
            // As `copy`: the learned tier and its blend are not part
            // of what a preset carries or compares.
            Section::Noise => {
                a.noise.enabled == b.noise.enabled
                    && a.noise.profiled == b.noise.profiled
                    && a.noise.strength == b.noise.strength
            }
            Section::Detail => a.detail == b.detail,
            Section::OutputSharpen => a.output_sharpen == b.output_sharpen,
            Section::Sharpen => a.sharpen == b.sharpen,
            Section::Curves => a.curves == b.curves,
            Section::Mixer => a.mixer == b.mixer,
            Section::Color => a.color == b.color,
            Section::BlackWhite => a.bw == b.bw,
            Section::Grading => a.grading == b.grading,
            Section::Tint => a.tint == b.tint,
            Section::Look => a.look_lut == b.look_lut,
            Section::Lens => a.lens == b.lens,
            Section::Vignette => a.vignette == b.vignette,
            Section::Grain => a.grain == b.grain,
            Section::Demosaic => a.demosaic == b.demosaic,
            // As `copy` would lay it (`Camera::take`).
            Section::Camera => b.camera.has(&a.camera),
            Section::Adjustments => a.adjustments == b.adjustments,
        }
    }
}

/// A part of a section: one slider, or the few settings that make one
/// control in the panel, to carry alone, the rest of its section left
/// as the picture has it. A section with one control has none and goes
/// whole. The section's own switch is not a part: it goes only with
/// the whole section, so a part laid over a picture whose section is
/// off is there, and shows, once the section is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    LightExposure,
    LightContrast,
    LightHighlights,
    LightMidtones,
    LightShadows,
    LightWhites,
    LightBlacks,
    ColorVibrance,
    ColorSaturation,
    CurvesParametric,
    CurvesRgb,
    CurvesRed,
    CurvesGreen,
    CurvesBlue,
    CurvesRedGreen,
    CurvesBlueYellow,
    /// One hue band of the mixer, its hue, saturation and luminance
    /// together, in `mixer::Band::ALL`'s order.
    MixerRed,
    MixerOrange,
    MixerYellow,
    MixerGreen,
    MixerAqua,
    MixerBlue,
    MixerPurple,
    MixerMagenta,
    /// The eight bands' weights into the grey.
    BlackWhiteWeights,
    BlackWhiteStrength,
    GradingShadows,
    GradingMidtones,
    GradingHighlights,
    GradingBalance,
    /// The profile's switch, the lens it is of, and its distortion.
    LensProfile,
    /// The profile's vignetting.
    LensVignetting,
    /// The profile's lateral chromatic aberration, its switch.
    LensChromaticAberration,
    /// The lateral chromatic aberration set by hand: the red's and the
    /// blue's amounts, the two sliders under the profile's switches.
    LensManualChromaticAberration,
    /// The distortion set by hand.
    LensDistortion,
    LensScale,
    /// The defringe, its switch and its windows.
    LensDefringe,
    DetailTexture,
    DetailClarity,
    DetailStructure,
    DetailDehaze,
    VignetteAmount,
    VignetteMidpoint,
    VignetteFeather,
    VignetteRoundness,
    GrainAmount,
    GrainSize,
    GrainKind,
}

/// The mixer's parts, band by band.
const MIXER_PARTS: [Part; crate::mixer::BANDS] = [
    Part::MixerRed,
    Part::MixerOrange,
    Part::MixerYellow,
    Part::MixerGreen,
    Part::MixerAqua,
    Part::MixerBlue,
    Part::MixerPurple,
    Part::MixerMagenta,
];

impl Part {
    /// Every part, in the panel's order: by section as
    /// [`Section::ALL`] has them, then down the section.
    pub const ALL: &[Part] = &[
        Part::LightExposure,
        Part::LightContrast,
        Part::LightHighlights,
        Part::LightMidtones,
        Part::LightShadows,
        Part::LightWhites,
        Part::LightBlacks,
        Part::ColorVibrance,
        Part::ColorSaturation,
        Part::CurvesParametric,
        Part::CurvesRgb,
        Part::CurvesRed,
        Part::CurvesGreen,
        Part::CurvesBlue,
        Part::CurvesRedGreen,
        Part::CurvesBlueYellow,
        Part::MixerRed,
        Part::MixerOrange,
        Part::MixerYellow,
        Part::MixerGreen,
        Part::MixerAqua,
        Part::MixerBlue,
        Part::MixerPurple,
        Part::MixerMagenta,
        Part::BlackWhiteWeights,
        Part::BlackWhiteStrength,
        Part::GradingShadows,
        Part::GradingMidtones,
        Part::GradingHighlights,
        Part::GradingBalance,
        Part::LensProfile,
        Part::LensVignetting,
        Part::LensChromaticAberration,
        Part::LensManualChromaticAberration,
        Part::LensDistortion,
        Part::LensScale,
        Part::LensDefringe,
        Part::DetailTexture,
        Part::DetailClarity,
        Part::DetailStructure,
        Part::DetailDehaze,
        Part::VignetteAmount,
        Part::VignetteMidpoint,
        Part::VignetteFeather,
        Part::VignetteRoundness,
        Part::GrainAmount,
        Part::GrainSize,
        Part::GrainKind,
    ];

    /// The section it is a part of.
    pub fn section(self) -> Section {
        use Part::*;
        match self {
            LightExposure | LightContrast | LightHighlights | LightMidtones | LightShadows
            | LightWhites | LightBlacks => Section::Light,
            ColorVibrance | ColorSaturation => Section::Color,
            CurvesParametric | CurvesRgb | CurvesRed | CurvesGreen | CurvesBlue
            | CurvesRedGreen | CurvesBlueYellow => Section::Curves,
            MixerRed | MixerOrange | MixerYellow | MixerGreen | MixerAqua | MixerBlue
            | MixerPurple | MixerMagenta => Section::Mixer,
            BlackWhiteWeights | BlackWhiteStrength => Section::BlackWhite,
            GradingShadows | GradingMidtones | GradingHighlights | GradingBalance => {
                Section::Grading
            }
            LensProfile
            | LensVignetting
            | LensChromaticAberration
            | LensManualChromaticAberration
            | LensDistortion
            | LensScale
            | LensDefringe => Section::Lens,
            DetailTexture | DetailClarity | DetailStructure | DetailDehaze => Section::Detail,
            VignetteAmount | VignetteMidpoint | VignetteFeather | VignetteRoundness => {
                Section::Vignette
            }
            GrainAmount | GrainSize | GrainKind => Section::Grain,
        }
    }

    /// The serialized name: the section's, a dot, the part's.
    pub fn name(self) -> &'static str {
        use Part::*;
        match self {
            LightExposure => "light.exposure",
            LightContrast => "light.contrast",
            LightHighlights => "light.highlights",
            LightMidtones => "light.midtones",
            LightShadows => "light.shadows",
            LightWhites => "light.whites",
            LightBlacks => "light.blacks",
            ColorVibrance => "color.vibrance",
            ColorSaturation => "color.saturation",
            CurvesParametric => "curves.parametric",
            CurvesRgb => "curves.rgb",
            CurvesRed => "curves.red",
            CurvesGreen => "curves.green",
            CurvesBlue => "curves.blue",
            CurvesRedGreen => "curves.red-green",
            CurvesBlueYellow => "curves.blue-yellow",
            MixerRed => "mixer.red",
            MixerOrange => "mixer.orange",
            MixerYellow => "mixer.yellow",
            MixerGreen => "mixer.green",
            MixerAqua => "mixer.aqua",
            MixerBlue => "mixer.blue",
            MixerPurple => "mixer.purple",
            MixerMagenta => "mixer.magenta",
            BlackWhiteWeights => "black-and-white.weights",
            BlackWhiteStrength => "black-and-white.strength",
            GradingShadows => "grading.shadows",
            GradingMidtones => "grading.midtones",
            GradingHighlights => "grading.highlights",
            GradingBalance => "grading.balance",
            LensProfile => "lens.profile",
            LensVignetting => "lens.vignetting",
            LensChromaticAberration => "lens.profile-chromatic-aberration",
            LensManualChromaticAberration => "lens.manual-chromatic-aberration",
            LensDistortion => "lens.distortion",
            LensScale => "lens.scale",
            LensDefringe => "lens.defringe",
            DetailTexture => "detail.texture",
            DetailClarity => "detail.clarity",
            DetailStructure => "detail.structure",
            DetailDehaze => "detail.dehaze",
            VignetteAmount => "vignette.amount",
            VignetteMidpoint => "vignette.midpoint",
            VignetteFeather => "vignette.feather",
            VignetteRoundness => "vignette.roundness",
            GrainAmount => "grain.amount",
            GrainSize => "grain.size",
            GrainKind => "grain.kind",
        }
    }

    /// The panel's word for it, under its section's.
    pub fn title(self) -> &'static str {
        use Part::*;
        match self {
            LightExposure => "Exposure",
            LightContrast => "Contrast",
            LightHighlights => "Highlights",
            LightMidtones => "Midtones",
            LightShadows => "Shadows",
            LightWhites => "Whites",
            LightBlacks => "Blacks",
            ColorVibrance => "Vibrance",
            ColorSaturation => "Saturation",
            CurvesParametric => "Parametric",
            CurvesRgb => "RGB",
            CurvesRed => "Red",
            CurvesGreen => "Green",
            CurvesBlue => "Blue",
            CurvesRedGreen => "R/G",
            CurvesBlueYellow => "B/Y",
            MixerRed => "Red",
            MixerOrange => "Orange",
            MixerYellow => "Yellow",
            MixerGreen => "Green",
            MixerAqua => "Aqua",
            MixerBlue => "Blue",
            MixerPurple => "Purple",
            MixerMagenta => "Magenta",
            BlackWhiteWeights => "Weights",
            BlackWhiteStrength => "Strength",
            GradingShadows => "Shadows",
            GradingMidtones => "Midtones",
            GradingHighlights => "Highlights",
            GradingBalance => "Balance",
            LensProfile => "Profile",
            LensVignetting => "Vignetting",
            LensChromaticAberration => "Chromatic aberration",
            LensManualChromaticAberration => "Manual aberration",
            LensDistortion => "Manual distortion",
            LensScale => "Scale",
            LensDefringe => "Defringe",
            DetailTexture => "Texture",
            DetailClarity => "Clarity",
            DetailStructure => crate::STRUCTURE,
            DetailDehaze => "Dehaze",
            VignetteAmount => "Amount",
            VignetteMidpoint => "Midpoint",
            VignetteFeather => "Feather",
            VignetteRoundness => "Roundness",
            GrainAmount => "Amount",
            GrainSize => "Size",
            GrainKind => "Type",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.name() == name)
    }

    /// The parts a name in a preset's file stands for: its own, or for
    /// a name an older build wrote, what that name carried then.
    /// `lens.chromatic-aberration` was the profile's switch and the
    /// amounts set by hand together, before the two were parts of their
    /// own, and a preset saved then still carries both.
    pub fn from_file_name(name: &str) -> Vec<Self> {
        match name {
            "lens.chromatic-aberration" => vec![
                Part::LensChromaticAberration,
                Part::LensManualChromaticAberration,
            ],
            _ => Self::from_name(name).into_iter().collect(),
        }
    }

    /// Lay this part of `from` over `onto`, the rest of the section
    /// and its switch left as `onto` has them.
    pub fn copy(self, from: &Edit, onto: &mut Edit) {
        use Part::*;
        let (f, o) = (from, onto);
        if let Some(b) = MIXER_PARTS.iter().position(|&p| p == self) {
            o.mixer.hue[b] = f.mixer.hue[b];
            o.mixer.saturation[b] = f.mixer.saturation[b];
            o.mixer.luminance[b] = f.mixer.luminance[b];
            return;
        }
        match self {
            // The zero the exposure was set against goes with it: none
            // from a preset, the source frame's from a paste or a sync
            // (`Preset::between_frames`).
            LightExposure => {
                o.light.exposure = f.light.exposure;
                o.light.baseline = f.light.baseline;
            }
            LightContrast => o.light.tone.contrast = f.light.tone.contrast,
            LightHighlights => o.light.tone.highlights = f.light.tone.highlights,
            LightMidtones => o.light.tone.midtones = f.light.tone.midtones,
            LightShadows => o.light.tone.shadows = f.light.tone.shadows,
            LightWhites => o.light.tone.whites = f.light.tone.whites,
            LightBlacks => o.light.tone.blacks = f.light.tone.blacks,
            ColorVibrance => o.color.vibrance = f.color.vibrance,
            ColorSaturation => o.color.saturation = f.color.saturation,
            CurvesParametric => o.curves.parametric = f.curves.parametric,
            CurvesRgb => o.curves.rgb = f.curves.rgb.clone(),
            CurvesRed => o.curves.red = f.curves.red.clone(),
            CurvesGreen => o.curves.green = f.curves.green.clone(),
            CurvesBlue => o.curves.blue = f.curves.blue.clone(),
            CurvesRedGreen => o.curves.red_green = f.curves.red_green.clone(),
            CurvesBlueYellow => o.curves.blue_yellow = f.curves.blue_yellow.clone(),
            BlackWhiteWeights => o.bw.weights = f.bw.weights,
            BlackWhiteStrength => o.bw.strength = f.bw.strength,
            GradingShadows => o.grading.shadows = f.grading.shadows,
            GradingMidtones => o.grading.midtones = f.grading.midtones,
            GradingHighlights => o.grading.highlights = f.grading.highlights,
            GradingBalance => o.grading.balance = f.grading.balance,
            LensProfile => {
                o.lens.profile = f.lens.profile;
                o.lens.chosen = f.lens.chosen.clone();
                o.lens.distortion = f.lens.distortion;
            }
            LensVignetting => o.lens.vignetting = f.lens.vignetting,
            LensChromaticAberration => {
                o.lens.chromatic_aberration = f.lens.chromatic_aberration;
            }
            LensManualChromaticAberration => {
                o.lens.ca_red = f.lens.ca_red;
                o.lens.ca_blue = f.lens.ca_blue;
            }
            LensDistortion => o.lens.manual = f.lens.manual,
            LensScale => {
                o.lens.auto_scale = f.lens.auto_scale;
                o.lens.scale = f.lens.scale;
            }
            LensDefringe => {
                o.lens.defringe = f.lens.defringe;
                o.lens.defringe_radius = f.lens.defringe_radius;
                o.lens.defringe_threshold = f.lens.defringe_threshold;
                o.lens.defringe_purple_center = f.lens.defringe_purple_center;
                o.lens.defringe_purple_width = f.lens.defringe_purple_width;
                o.lens.defringe_purple_amount = f.lens.defringe_purple_amount;
                o.lens.defringe_green_center = f.lens.defringe_green_center;
                o.lens.defringe_green_width = f.lens.defringe_green_width;
                o.lens.defringe_green_amount = f.lens.defringe_green_amount;
            }
            DetailTexture => o.detail.texture = f.detail.texture,
            DetailClarity => o.detail.clarity = f.detail.clarity,
            DetailStructure => o.detail.structure = f.detail.structure,
            DetailDehaze => o.detail.dehaze = f.detail.dehaze,
            VignetteAmount => o.vignette.amount = f.vignette.amount,
            VignetteMidpoint => o.vignette.midpoint = f.vignette.midpoint,
            VignetteFeather => o.vignette.feather = f.vignette.feather,
            VignetteRoundness => o.vignette.roundness = f.vignette.roundness,
            GrainAmount => o.grain.amount = f.grain.amount,
            GrainSize => o.grain.size = f.grain.size,
            GrainKind => o.grain.kind = f.grain.kind,
            MixerRed | MixerOrange | MixerYellow | MixerGreen | MixerAqua | MixerBlue
            | MixerPurple | MixerMagenta => unreachable!("a band, laid above"),
        }
    }

    /// Whether this part reads the same in both edits: `b` with the
    /// part of `a` laid over it is `b`.
    pub fn same(self, a: &Edit, b: &Edit) -> bool {
        let mut laid = b.clone();
        self.copy(a, &mut laid);
        // The light's zero is not a setting a preset is said to have.
        laid.light.baseline = b.light.baseline;
        laid == *b
    }
}

impl Serialize for Part {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for Part {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        Part::from_name(&name)
            .ok_or_else(|| serde::de::Error::custom(format!("no part called {name:?}")))
    }
}

/// A preset's parts as its file has them: a name this build does not
/// know (one from a newer build) is passed over with a word in the log
/// rather than the whole preset refused, and a null is none.
fn known_parts<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<Part>, D::Error> {
    let names = Option::<Vec<String>>::deserialize(d)?.unwrap_or_default();
    let mut parts = Vec::new();
    for n in &names {
        let these = Part::from_file_name(n);
        if these.is_empty() {
            log::warn!("preset: no part called {n:?}, passed over");
        }
        for p in these {
            if !parts.contains(&p) {
                parts.push(p);
            }
        }
    }
    Ok(parts)
}

/// How a preset's masks land on a picture's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Masks {
    /// In place of the picture's, as they always have.
    #[default]
    Replace,
    /// After the picture's own, which stay.
    Join,
}

impl Masks {
    pub fn is_replace(&self) -> bool {
        *self == Masks::Replace
    }
}

/// What goes across, as a sheet chooses it: whole sections, parts of
/// others, which masks, and how they land. [`Preset::from_choice`]
/// keeps it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Choice {
    /// Whole sections. [`Section::Adjustments`] among them carries the
    /// masks `masks` names.
    pub sections: Vec<Section>,
    /// Parts of sections not among `sections`.
    pub parts: Vec<Part>,
    /// The masks, by id, when the masks go: `None` for every one.
    pub masks: Option<Vec<u64>>,
    pub masks_land: Masks,
}

impl Choice {
    /// Whole sections and nothing else: what a sheet chose before it
    /// had parts.
    pub fn whole(sections: &[Section]) -> Self {
        Self {
            sections: sections.to_vec(),
            ..Self::default()
        }
    }

    /// Whether nothing at all is chosen.
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty() && self.parts.is_empty()
    }

    /// The words for how much goes: "3 sections", "1 part", "2
    /// sections and 4 parts".
    pub fn words(&self) -> String {
        carried_words(self.sections.len(), self.parts.len())
    }
}

/// "N sections and M parts", either left out at none.
pub fn carried_words(sections: usize, parts: usize) -> String {
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    match (sections, parts) {
        (s, 0) => format!("{s} section{}", plural(s)),
        (0, p) => format!("{p} part{}", plural(p)),
        (s, p) => format!("{s} section{} and {p} part{}", plural(s), plural(p)),
    }
}

impl Section {
    /// Its parts, in the panel's order; none for a section that is
    /// one control. The masks' parts are the masks themselves, one
    /// picture's own, and are not listed here.
    pub fn parts(self) -> impl Iterator<Item = Part> {
        Part::ALL
            .iter()
            .copied()
            .filter(move |p| p.section() == self)
    }
}

/// `parts` in [`Part::ALL`]'s order, each once, less those of a
/// section carried whole, which the whole carries already.
fn parts_in_order(parts: &[Part], whole: &[Section]) -> Vec<Part> {
    Part::ALL
        .iter()
        .copied()
        .filter(|p| parts.contains(p) && !whole.contains(&p.section()))
        .collect()
}

/// Whether `edit` has a mask that is `mask` in all but its id.
fn has_mask(edit: &Edit, mask: &crate::Adjustment) -> bool {
    edit.adjustments.iter().any(|a| {
        crate::Adjustment {
            id: mask.id,
            ..a.clone()
        } == *mask
    })
}

/// A preset file's extension.
pub const EXTENSION: &str = "gcp";
/// The file in a store that lists the shipped presets put there.
const SEEDED: &str = "seeded";

/// A named part of an edit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    /// The schema version of the edit inside.
    pub version: u32,
    pub name: String,
    /// What it carries whole, in [`Section::ALL`]'s order, each once.
    pub sections: Vec<Section>,
    /// What it carries of a section it does not carry whole, in
    /// [`Part::ALL`]'s order, each once. Left out of the file when
    /// there are none, so a preset of whole sections is written as it
    /// always was, and a file without it is whole sections.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "known_parts"
    )]
    pub parts: Vec<Part>,
    /// How the masks it carries land: in place of the picture's, or
    /// after them. Left out of the file at the first, which is what a
    /// file without it means.
    #[serde(default, skip_serializing_if = "Masks::is_replace")]
    pub masks: Masks,
    /// The carried sections' and parts' settings; the rest at their
    /// defaults. Its masks are the ones carried.
    pub edit: Edit,
}

impl Default for Preset {
    fn default() -> Self {
        Self {
            version: VERSION,
            name: String::new(),
            sections: Vec::new(),
            parts: Vec::new(),
            masks: Masks::Replace,
            edit: Edit::default(),
        }
    }
}

impl Preset {
    /// The chosen sections of `edit`, whole, under a name. What is
    /// not chosen is not written.
    pub fn from_edit(name: &str, edit: &Edit, sections: &[Section]) -> Self {
        Self::from_choice(name, edit, &Choice::whole(sections))
    }

    /// What `choice` picks of `edit`, under a name: its sections whole,
    /// its parts of the others, and of the masks the ones it names.
    /// What is not chosen is not written.
    pub fn from_choice(name: &str, edit: &Edit, choice: &Choice) -> Self {
        let sections: Vec<Section> = Section::ALL
            .iter()
            .copied()
            .filter(|s| choice.sections.contains(s))
            .collect();
        let parts = parts_in_order(&choice.parts, &sections);
        // The masks named, in the picture's order, before the copy
        // gives them ids of their own.
        let picked;
        let source = match &choice.masks {
            Some(ids) if sections.contains(&Section::Adjustments) => {
                let mut e = edit.clone();
                e.adjustments.retain(|a| ids.contains(&a.id));
                picked = e;
                &picked
            }
            _ => edit,
        };
        let mut kept = Edit::default();
        for s in &sections {
            s.copy(source, &mut kept);
        }
        for p in &parts {
            p.copy(source, &mut kept);
        }
        // A preset's exposure is from the default of whatever frame it
        // lands on, not from the zero its source frame rested at.
        kept.light.baseline = None;
        // How masks land says something only when masks go.
        let masks = if sections.contains(&Section::Adjustments) {
            choice.masks_land
        } else {
            Masks::Replace
        };
        Self {
            version: VERSION,
            name: name.trim().to_string(),
            sections,
            parts,
            masks,
            edit: kept,
        }
    }

    /// What `choice` picks of a frame's edit to lay over other frames,
    /// a paste or a sync: [`Preset::from_choice`], and where the Light
    /// section or its exposure goes, the zero the source's exposure was
    /// set against goes with it ([`crate::Light::baseline`]), so an
    /// older edit synced onto a frame of its body lands where the
    /// source sits. A new edit has none to carry, so its exposure lands
    /// from the target's body's default. A preset saved to a file is
    /// made by [`Preset::from_choice`] and carries none.
    pub fn between_frames(edit: &Edit, choice: &Choice) -> Self {
        let mut p = Self::from_choice("", edit, choice);
        if p.sections.contains(&Section::Light) || p.parts.contains(&Part::LightExposure) {
            p.edit.light.baseline = edit.light.baseline;
        }
        p
    }

    /// The mask of `edit` with id `id`, alone, to join the masks of
    /// the frames it is pasted onto: what Copy mask holds and Paste
    /// mask lays, as a sync or paste of that one mask with Add to
    /// masks lays it.
    pub fn of_mask(edit: &Edit, id: u64) -> Self {
        Self::between_frames(
            edit,
            &Choice {
                sections: vec![Section::Adjustments],
                parts: Vec::new(),
                masks: Some(vec![id]),
                masks_land: Masks::Join,
            },
        )
    }

    /// Whether it carries nothing.
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty() && self.parts.is_empty()
    }

    /// Whether the masks it carries join a picture's rather than
    /// replace them.
    fn joins_masks(&self) -> bool {
        self.masks == Masks::Join && self.sections.contains(&Section::Adjustments)
    }

    /// Lay the carried sections and parts over `onto`. Masks that
    /// join go after the picture's own, each with an id after theirs,
    /// and one the picture has already (the same in all but its id)
    /// is not laid twice, so a second paste adds nothing; none past
    /// [`crate::MAX_MASKS`], which would be listed and never drawn.
    pub fn apply(&self, onto: &mut Edit) {
        for s in &self.sections {
            if *s == Section::Adjustments && self.joins_masks() {
                for a in &self.edit.adjustments {
                    if onto.adjustments.len() < crate::MAX_MASKS && !has_mask(onto, a) {
                        let id = onto.next_id();
                        onto.adjustments.push(crate::Adjustment { id, ..a.clone() });
                    }
                }
            } else {
                s.copy(&self.edit, onto);
            }
        }
        for p in &self.parts {
            p.copy(&self.edit, onto);
        }
    }

    /// `edit` with the preset laid over it.
    pub fn applied(&self, edit: &Edit) -> Edit {
        let mut out = edit.clone();
        self.apply(&mut out);
        out
    }

    /// Whether `edit` already has every carried section and part as
    /// the preset has it; masks that join, when it has each of them
    /// among its own.
    pub fn is_applied(&self, edit: &Edit) -> bool {
        self.sections.iter().all(|s| {
            if *s == Section::Adjustments && self.joins_masks() {
                self.edit.adjustments.iter().all(|a| has_mask(edit, a))
            } else {
                s.same(&self.edit, edit)
            }
        }) && self.parts.iter().all(|p| p.same(&self.edit, edit))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a preset serializes")
    }

    /// Read a preset, the edit inside migrated to this build's schema.
    /// A fault in its JSON is said as the preset's, not the sidecar's
    /// that [`Error::Json`] names.
    pub fn from_json(json: &str) -> Result<Self> {
        Self::read_json(json).map_err(|e| match e {
            Error::Json(e) => Error::Preset(e.to_string()),
            e => e,
        })
    }

    fn read_json(json: &str) -> Result<Self> {
        let mut value: serde_json::Value = serde_json::from_str(json)?;
        // A list of the fields in order would carry an edit that no
        // migration had touched (see `migrate`).
        let Some(obj) = value.as_object_mut() else {
            return Err(refused("a preset is an object"));
        };
        let version = obj.get("version").cloned();
        if let Some(mut edit) = obj.remove("edit") {
            // The edit's own version is the file's.
            if let (Some(v), Some(e)) = (version, edit.as_object_mut())
                && !e.contains_key("version")
            {
                e.insert("version".into(), v);
            }
            obj.insert("edit".into(), migrate(edit)?);
        }
        obj.insert("version".into(), VERSION.into());
        let mut preset: Preset = serde_json::from_value(value)?;
        preset.sections = Section::ALL
            .iter()
            .copied()
            .filter(|s| preset.sections.contains(s))
            .collect();
        preset.parts = parts_in_order(&preset.parts, &preset.sections);
        if !preset.sections.contains(&Section::Adjustments) {
            preset.masks = Masks::Replace;
        }
        // The migration keeps the zero an older edit was made against;
        // a preset carries none ([`Preset::from_choice`]).
        preset.edit.light.baseline = None;
        Ok(preset)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let mut preset = Self::from_json(&std::fs::read_to_string(path)?)?;
        if preset.name.is_empty() {
            preset.name = stem_name(path);
        }
        Ok(preset)
    }

    /// Write the preset, whole, through a temporary file.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension(format!("{EXTENSION}.tmp"));
        // On the disk before it takes the old one's name, so a crash
        // before the filesystem flushes cannot leave an empty preset.
        crate::write_synced(&tmp, self.to_json().as_bytes())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// An error's own words: a file system's without the "sidecar: " that
/// [`Error::Io`] puts before them, which a preset is not.
pub fn plain(e: &Error) -> String {
    match e {
        Error::Io(e) => e.to_string(),
        e => e.to_string(),
    }
}

/// A file's stem as a name.
pub fn stem_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Preset".into())
}

/// A preset's file name from its name: the letters and digits, in
/// lower case, runs of anything else a dash.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.chars() {
        if c.is_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.extend(c.to_lowercase());
        } else {
            dash = true;
        }
    }
    if out.is_empty() {
        out.push_str("preset");
    }
    out
}

/// A group's name as a folder can have it on every platform: a slash,
/// a backslash, a control character, and any of `<>:"|?*` made a dash,
/// with spaces and dots trimmed from either end (a folder starting with
/// a dot is not read as a group). None for nothing left.
pub fn group_folder(name: &str) -> Option<String> {
    let made: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let made = made.trim_matches(|c: char| c.is_whitespace() || c == '.');
    (!made.is_empty()).then(|| made.to_string())
}

/// A preset as found in the store.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub preset: Preset,
}

/// Where the presets live: a directory of `.gcp` files, and its
/// subfolders, one a group.
///
/// A group is a folder and nothing more: no record names it, so a
/// folder made or renamed by hand is a group made or renamed, and the
/// presets in it are found as they are. Groups are one level deep; a
/// folder in a group, and a folder whose name starts with a dot, are
/// not read. A name is meant to be one preset's across the groups, as
/// [`Store::save`] and the import keep it; two files that hold the
/// same name are both listed, and a lookup by name takes the first in
/// [`Store::list`]'s order: outside any group first, then the groups
/// alphabetically.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// `$XDG_CONFIG_HOME/greycard/presets` when that is set, else
    /// `greycard/presets` under the platform's configuration
    /// directory: `~/.config` on Linux, `~/Library/Application
    /// Support` on macOS, `%APPDATA%` on Windows.
    pub fn user() -> Option<Self> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(dirs::config_dir)?;
        Some(Self::at(base.join("greycard").join("presets")))
    }

    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where a preset of this name is kept.
    pub fn path_for(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{}.{EXTENSION}", slug(name)))
    }

    /// Every preset in the store: those outside any group first, then
    /// each group's, the groups alphabetically, and by name within
    /// each; a file that will not read is passed over with a word in
    /// the log.
    pub fn list(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        for group in std::iter::once(None).chain(self.groups().into_iter().map(Some)) {
            let Ok(dir) = std::fs::read_dir(self.group_dir(group.as_deref())) else {
                continue;
            };
            let mut these: Vec<Entry> = dir
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == EXTENSION) && p.is_file())
                .filter_map(|path| match Preset::load(&path) {
                    Ok(preset) => Some(Entry { path, preset }),
                    Err(e) => {
                        log::warn!("preset {}: {e}", path.display());
                        None
                    }
                })
                .collect();
            these.sort_by(|a, b| {
                a.preset
                    .name
                    .to_lowercase()
                    .cmp(&b.preset.name.to_lowercase())
                    .then_with(|| a.path.cmp(&b.path))
            });
            out.extend(these);
        }
        out
    }

    /// The groups: the store's subfolders, by name, alphabetically,
    /// case aside. An empty one is a group too, until a preset moved
    /// or removed takes it away ([`Store::move_to`]).
    pub fn groups(&self) -> Vec<String> {
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<String> = dir
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| !n.starts_with('.'))
            .collect();
        out.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
        out
    }

    /// The folder a group's presets are kept in; the store's own for
    /// none.
    pub fn group_dir(&self, group: Option<&str>) -> PathBuf {
        match group {
            Some(g) => self.dir.join(g),
            None => self.dir.clone(),
        }
    }

    /// The group a preset's file is in: the folder under the store's
    /// it sits in, or none for one in the store's own.
    pub fn group_of(&self, path: &Path) -> Option<String> {
        let parent = path.parent()?;
        if parent == self.dir || parent.parent() != Some(self.dir.as_path()) {
            return None;
        }
        parent.file_name()?.to_str().map(str::to_string)
    }

    /// A preset's file as a key the settings can keep: its path under
    /// the store, a slash between group and file whatever the
    /// platform's separator. None for a file outside the store.
    pub fn key_of(&self, path: &Path) -> Option<String> {
        let file = path.file_name()?.to_str()?;
        if path.parent()? == self.dir {
            return Some(file.to_string());
        }
        Some(format!("{}/{file}", self.group_of(path)?))
    }

    /// A group of this name already there, case aside, or the name as
    /// a folder can have it ([`group_folder`]); none for a name that is
    /// nothing once made one.
    fn group_named(&self, name: &str) -> Option<String> {
        let folder = group_folder(name)?;
        let wanted = folder.to_lowercase();
        Some(
            self.groups()
                .into_iter()
                .find(|g| g.to_lowercase() == wanted)
                .unwrap_or(folder),
        )
    }

    /// Move a preset into `group`, made if it is not there, or out of
    /// any for none, and say where its file is now. The file is moved
    /// as it is, so the preset keeps its name and what it holds; it
    /// keeps its file name too, unless the group has a file of that
    /// name already, when it takes the first free `stem-2`, `stem-3`.
    /// A group left empty is taken away.
    pub fn move_to(&self, entry: &Entry, group: Option<&str>) -> Result<PathBuf> {
        let group = match group {
            Some(name) => Some(
                self.group_named(name)
                    .ok_or_else(|| Error::Preset("a group needs a name".into()))?,
            ),
            None => None,
        };
        let from = self.group_of(&entry.path);
        if from == group {
            return Ok(entry.path.clone());
        }
        let dir = self.group_dir(group.as_deref());
        std::fs::create_dir_all(&dir)?;
        let stem = entry
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| slug(&entry.preset.name));
        let to = std::iter::once(format!("{stem}.{EXTENSION}"))
            .chain((2..).map(|n| format!("{stem}-{n}.{EXTENSION}")))
            .map(|file| dir.join(file))
            .find(|p| !p.exists())
            .expect("a free file name");
        std::fs::rename(&entry.path, &to)?;
        self.prune(from.as_deref());
        Ok(to)
    }

    /// Rename a group's folder, and say the name it has now: the new
    /// name as a folder can have it. Refused for a group that is not
    /// there, a name that is nothing once made a folder's, and a name
    /// another group has, case aside.
    pub fn rename_group(&self, old: &str, new: &str) -> Result<String> {
        let new = group_folder(new).ok_or_else(|| Error::Preset("a group needs a name".into()))?;
        let groups = self.groups();
        if !groups.iter().any(|g| g == old) {
            return Err(Error::Preset(format!("there is no group called \"{old}\"")));
        }
        if new == old {
            return Ok(new);
        }
        let wanted = new.to_lowercase();
        if let Some(taken) = groups
            .into_iter()
            .find(|g| g != old && g.to_lowercase() == wanted)
        {
            return Err(Error::Preset(format!(
                "there is a group called \"{taken}\" already"
            )));
        }
        std::fs::rename(self.group_dir(Some(old)), self.group_dir(Some(&new)))?;
        Ok(new)
    }

    /// Take a group's folder away once nothing is in it: a folder with
    /// anything in it, a file of the user's or a preset that will not
    /// read, stays.
    fn prune(&self, group: Option<&str>) {
        if let Some(g) = group {
            let _ = std::fs::remove_dir(self.group_dir(Some(g)));
        }
    }

    /// The preset called `name`, case aside; or, for a path that is a
    /// file, that file.
    pub fn find(&self, name: &str) -> Option<Entry> {
        let path = Path::new(name);
        if path.is_file() {
            return Preset::load(path).ok().map(|preset| Entry {
                path: path.to_path_buf(),
                preset,
            });
        }
        let wanted = name.trim().to_lowercase();
        self.list()
            .into_iter()
            .find(|e| e.preset.name.to_lowercase() == wanted)
    }

    /// Write `preset` under its name, over one of the same name.
    pub fn save(&self, preset: &Preset) -> Result<PathBuf> {
        if preset.name.trim().is_empty() {
            return Err(Error::Preset("a preset needs a name".into()));
        }
        // A file that holds the name already, whatever it is called.
        let path = self
            .list()
            .into_iter()
            .find(|e| e.preset.name.eq_ignore_ascii_case(preset.name.trim()))
            .map(|e| e.path)
            .unwrap_or_else(|| self.path_for(&preset.name));
        preset.save_to(&path)?;
        Ok(path)
    }

    /// Remove a preset's file, and its group's folder with it when
    /// that was the last thing in it.
    pub fn remove(&self, entry: &Entry) -> Result<()> {
        std::fs::remove_file(&entry.path)?;
        self.prune(self.group_of(&entry.path).as_deref());
        Ok(())
    }

    /// Put the film presets this build ships in the store, each one
    /// once.
    ///
    /// A store that was there before a preset shipped gets it too (a
    /// user with presets of their own is the common case, not the
    /// exception), so the record of what has been seeded is not the
    /// directory's existence but a `seeded` file in it listing the
    /// stems put there so far. A stem on that list is not written
    /// again: one edited stays edited and one binned stays binned. The
    /// one exception is a copy still exactly as an earlier build wrote
    /// it ([`SUPERSEDED`]), which takes this build's text.
    /// Nothing is overwritten, and a failure is a word in the log, not
    /// an error — a preset that could not be written is not a reason
    /// for the editor not to start.
    pub fn seed(&self) {
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            log::warn!("presets {}: {e}", self.dir.display());
            return;
        }
        let mut seeded = self.seeded();
        let mut changed = false;
        for (stem, text) in SHIPPED {
            let path = self.dir.join(format!("{stem}.{EXTENSION}"));
            if seeded.iter().any(|s| s == stem) {
                // Seeded before. A copy still as an earlier build wrote
                // it takes this build's text; one the user changed, or
                // moved to a group, or binned, is left as it is.
                if SUPERSEDED.iter().any(|(s, old)| {
                    *s == stem && std::fs::read_to_string(&path).is_ok_and(|have| have == *old)
                }) && let Err(e) = std::fs::write(&path, text)
                {
                    log::warn!("preset {}: {e}", path.display());
                }
                continue;
            }
            if path.exists() {
                // Something of that name is there already: the
                // user's, whatever it holds.
            } else if let Err(e) = std::fs::write(&path, text) {
                log::warn!("preset {}: {e}", path.display());
                continue;
            }
            seeded.push((*stem).to_string());
            changed = true;
        }
        if changed && let Err(e) = self.keep_seeded(&seeded) {
            log::warn!("presets {}: {e}", self.dir.join(SEEDED).display());
        }
    }

    /// The names of the shipped presets the store is without: none of
    /// its presets goes by the shipped name, case aside. What a file
    /// under the shipped stem holds does not count: one the user has
    /// filled with a preset of another name, or one that will not
    /// read, leaves the shipped preset missing. In [`SHIPPED`]'s order.
    pub fn missing_shipped(&self) -> Vec<String> {
        let names: Vec<String> = self
            .list()
            .into_iter()
            .map(|e| e.preset.name.to_lowercase())
            .collect();
        SHIPPED
            .iter()
            .filter_map(|(_, text)| {
                let name = Preset::from_json(text).ok()?.name;
                (!names.contains(&name.to_lowercase())).then_some(name)
            })
            .collect()
    }

    /// Put back every shipped preset the store is without
    /// ([`Store::missing_shipped`]), and say which, by name.
    ///
    /// The way back from [`Store::seed`]'s "one binned stays binned":
    /// asked for, the binned ones return. Nothing is overwritten: a
    /// shipped stem whose file is there already, whatever it holds,
    /// sends the preset to the first free `stem-2`, `stem-3`. So a
    /// second run does nothing. Every shipped stem is on the `seeded`
    /// record after, as a seed would leave it, so a later seed writes
    /// none of them again.
    pub fn restore_shipped(&self) -> Result<Vec<String>> {
        std::fs::create_dir_all(&self.dir)?;
        let missing = self.missing_shipped();
        let was = self.seeded();
        let mut seeded = was.clone();
        let mut restored = Vec::new();
        for (stem, text) in SHIPPED {
            let preset = Preset::from_json(text)?;
            if missing.contains(&preset.name) {
                let path = std::iter::once(format!("{stem}.{EXTENSION}"))
                    .chain((2..).map(|n| format!("{stem}-{n}.{EXTENSION}")))
                    .map(|file| self.dir.join(file))
                    .find(|p| !p.exists())
                    .expect("a free name");
                std::fs::write(path, text)?;
                restored.push(preset.name);
            }
            if !seeded.iter().any(|s| s == stem) {
                seeded.push((*stem).to_string());
            }
        }
        if seeded != was {
            self.keep_seeded(&seeded)?;
        }
        Ok(restored)
    }

    /// The stems on the `seeded` record; none without a record.
    fn seeded(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join(SEEDED))
            .map(|t| {
                t.lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn keep_seeded(&self, seeded: &[String]) -> std::io::Result<()> {
        std::fs::write(self.dir.join(SEEDED), seeded.join("\n") + "\n")
    }
}

/// The film presets this build ships, by file stem.
///
/// Every one of them is built from the parametric controls the editor
/// already has — the tone shifts, the parametric curve, the mixer, the
/// grading wheels, the black and white, the grain — so none needs a
/// LUT file to exist and every number in them stays a slider the user
/// can move. Honest names, not the makers' trademarks (notes §78):
///
/// - **Muted Slide**, a transparency look: saturation out of the
///   picture, a hard shoulder on the highlights, crushed blacks and
///   cyan shadows.
/// - **Warm Negative**, a color negative scanned and left warm: soft
///   contrast, a matte black from the point curve, a warm shadow and
///   highlight wheel, fine grain.
/// - **Red-Filter Mono**, a panchromatic black and white through a
///   red filter: a dark sky, light skin, coarse grain.
pub const SHIPPED: [(&str, &str); 3] = [
    ("muted-slide", include_str!("../presets/muted-slide.gcp")),
    (
        "red-filter-mono",
        include_str!("../presets/red-filter-mono.gcp"),
    ),
    (
        "warm-negative",
        include_str!("../presets/warm-negative.gcp"),
    ),
];

/// Earlier builds' texts of shipped presets, by stem. A user's copy
/// still byte for byte one of these was never touched, and the seed
/// writes this build's over it; a copy that differs is the user's.
pub const SUPERSEDED: [(&str, &str); 2] = [
    (
        "muted-slide",
        include_str!("../presets/superseded/muted-slide.gcp"),
    ),
    (
        "warm-negative",
        include_str!("../presets/superseded/warm-negative.gcp"),
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Learned, WhiteBalance};

    /// Every door that lays a Camera section naming a DCP on a frame
    /// turns its look table on, whatever the frame had and whatever the
    /// source says, so one preset over a batch lands one way; the
    /// preset then reads as applied. An undo gives the frame its switch
    /// back.
    #[test]
    fn a_dcp_through_any_door_comes_with_its_look_table() {
        use crate::Line;
        use crate::camera::{Camera, ProfileChoice};
        let named = |name: &str, look_table| Camera {
            profile: ProfileChoice::Named(name.into()),
            look_table,
        };
        let edit_on = |camera: Camera| Edit {
            camera,
            ..Edit::default()
        };
        // An old preset (no switch in its file) onto a frame on the
        // file's own profile, and onto an old frame on another DCP.
        let old_preset = Preset::from_json(
            r#"{"version": 8, "name": "Old", "sections": ["camera"],
                "edit": {"version": 8, "camera": {"profile": "Natural"}}}"#,
        )
        .unwrap();
        assert!(!old_preset.edit.camera.look_table);
        for frame in [Edit::default(), edit_on(named("Vivid", false))] {
            let after = old_preset.applied(&frame);
            assert_eq!(after.camera, named("Natural", true));
        }
        // Onto an old frame on the same profile too, and from a
        // source that has the table.
        let old_frame = edit_on(named("Natural", false));
        assert!(!old_preset.is_applied(&old_frame));
        let after = old_preset.applied(&old_frame);
        assert_eq!(after.camera, named("Natural", true));
        assert!(old_preset.is_applied(&after));
        let new_preset =
            Preset::from_edit("New", &edit_on(named("Natural", true)), &[Section::Camera]);
        assert_eq!(
            new_preset.applied(&old_frame).camera,
            named("Natural", true)
        );
        assert!(new_preset.is_applied(&after));
        // The file's own profile has no table.
        let own = Preset::from_edit("Own", &Edit::default(), &[Section::Camera]);
        assert_eq!(own.applied(&old_frame).camera, Camera::default());
        assert!(own.is_applied(&Edit::default()));

        // A paste or a sync: the section copied from another frame's
        // edit, an old one among them.
        let mut onto = Edit::default();
        let paste = Preset::from_choice("Paste", &old_frame, &Choice::whole(&[Section::Camera]));
        paste.apply(&mut onto);
        assert_eq!(onto.camera, named("Natural", true));
        let mut onto = edit_on(named("Vivid", false));
        Section::Camera.copy(&old_frame, &mut onto);
        assert_eq!(onto.camera, named("Natural", true));

        // An undo restores the switch as it was; a snapshot restores
        // the edit whole too, through the same record.
        let mut line = Line {
            current: old_frame.clone(),
            ..Line::default()
        };
        assert!(line.record(old_preset.applied(&edit_on(named("Vivid", false)))));
        assert!(line.record(edit_on(named("Vivid", false))));
        assert!(line.record(old_preset.applied(&line.current.clone())));
        assert!(line.current.camera.look_table);
        assert!(line.undo());
        assert_eq!(line.current.camera, named("Vivid", false));
        assert!(line.undo() && line.undo());
        assert_eq!(line.current.camera, named("Natural", false));
    }

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("greycard-preset-{tag}-{}", std::process::id()));
        greycard_core::testing::remove_dir_retry(&d);
        d
    }

    /// A lens picked by hand goes with the profile's part and with the
    /// whole section, as the profile's switch does.
    #[test]
    fn a_picked_lens_travels_with_the_profile() {
        let mut from = Edit::default();
        from.lens.chosen = crate::LensChoice::Lcp("a.lcp".into());
        let mut onto = Edit::default();
        Part::LensProfile.copy(&from, &mut onto);
        assert_eq!(onto.lens.chosen, from.lens.chosen);
        let mut onto = Edit::default();
        Section::Lens.copy(&from, &mut onto);
        assert_eq!(onto.lens.chosen, from.lens.chosen);
        let mut onto = Edit::default();
        Part::LensVignetting.copy(&from, &mut onto);
        assert!(onto.lens.chosen.is_from_file());
    }

    /// Every shipped preset reads, says what it carries, and changes
    /// the picture the way its name claims. They are checked here
    /// rather than trusted because they are hand-written JSON: a
    /// misspelt key is silently a default.
    #[test]
    fn the_shipped_presets_are_what_they_say() {
        let by_name = |stem: &str| {
            let (_, text) = SHIPPED.iter().find(|(s, _)| *s == stem).expect("shipped");
            Preset::from_json(text).expect("it reads")
        };

        let slide = by_name("muted-slide");
        assert_eq!(slide.name, "Muted Slide");
        assert!(slide.sections.contains(&Section::Color));
        // It carries nothing it does not list, and lists nothing it
        // does not change, but for Black and white: a color look lists
        // it off on purpose, so it turns a mono picture back.
        for preset in SHIPPED.map(|(_, t)| Preset::from_json(t).unwrap()) {
            let bare = Edit::default();
            for s in Section::ALL {
                let changed = !s.same(&preset.edit, &bare);
                if *s == Section::BlackWhite && !changed {
                    continue;
                }
                assert_eq!(
                    changed,
                    preset.sections.contains(s),
                    "{}: {} is {}",
                    preset.name,
                    s.name(),
                    if changed {
                        "set but not listed"
                    } else {
                        "listed but not set"
                    }
                );
            }
            // None of them is a LUT: a film preset here is the
            // parametric controls, so it needs no file to exist.
            assert!(preset.edit.look_lut.lut.is_none(), "{}", preset.name);
        }

        // No preset pulls the black point up into the picture. A
        // negative `blacks` is a clip, not a shade — -0.12 puts it at
        // 41 of 255 and once took half a frame to pure black — and
        // where the black sits is a per-picture decision a shipped
        // preset has no business making. A positive one opens the
        // shadows and clips nothing; a faded negative's matte black is
        // the point curve's, as Warm Negative has it.
        //
        // What each preset does to a picture beyond that is measured
        // on the render, in `greycard-ui`'s `shipped_presets`: the
        // signs of the sliders are not the thing, and reading them was
        // what let a "Muted" preset raise saturation.
        for preset in SHIPPED.map(|(_, t)| Preset::from_json(t).unwrap()) {
            assert!(
                preset.edit.light.tone.blacks >= 0.0,
                "{} pulls the black point up, to {}",
                preset.name,
                preset.edit.light.tone.blacks
            );
        }

        let negative = by_name("warm-negative");
        assert_eq!(negative.name, "Warm Negative");
        assert!(!negative.edit.grain.is_off());

        // The color looks carry Black and white, off, so one applied
        // over Red-Filter Mono brings the color back.
        for stem in ["muted-slide", "warm-negative"] {
            let p = by_name(stem);
            assert!(
                p.sections.contains(&Section::BlackWhite),
                "{} carries no Black and white",
                p.name
            );
            assert!(!p.edit.bw.enabled, "{} is mono", p.name);
        }

        // Red-Filter Mono is mono, and through a red filter: the one
        // thing here that its name states outright.
        let mono = by_name("red-filter-mono");
        assert_eq!(mono.name, "Red-Filter Mono");
        assert!(mono.edit.bw.enabled);
        assert_eq!(mono.edit.bw.filter(), Some(crate::bw::Filter::Red));
        assert!(!mono.edit.grain.is_off());
    }

    /// A seeded copy still as an earlier build wrote it takes this
    /// build's; one the user changed stands.
    #[test]
    fn an_untouched_copy_of_an_earlier_shipped_preset_is_brought_up() {
        let d = dir("seed-superseded");
        std::fs::create_dir_all(&d).unwrap();
        let store = Store::at(d.clone());
        let (stem, old) = SUPERSEDED[0];
        let (_, new) = SHIPPED.iter().find(|(s, _)| *s == stem).unwrap();
        let path = d.join(format!("{stem}.{EXTENSION}"));
        std::fs::write(&path, old).unwrap();
        store.keep_seeded(&[stem.to_string()]).unwrap();
        store.seed();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), *new);

        // Changed by the user: left as it is.
        let mine = old.replace("0.", "0.1");
        assert_ne!(mine, old);
        std::fs::write(&path, &mine).unwrap();
        store.seed();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), mine);
    }

    /// Every shipped preset is seeded once, into a new store and into
    /// one the user already had, and a binned one stays binned.
    #[test]
    fn the_shipped_presets_are_seeded_once_each() {
        let d = dir("seed");
        let store = Store::at(d.clone());
        store.seed();
        let names: Vec<String> = store.list().into_iter().map(|e| e.preset.name).collect();
        assert_eq!(names.len(), SHIPPED.len());
        assert!(names.contains(&"Muted Slide".to_string()));

        // Bin one and seed again: it is on the record, so it is not
        // put back.
        let gone = store.find("Muted Slide").unwrap();
        store.remove(&gone).unwrap();
        store.seed();
        assert_eq!(store.list().len(), SHIPPED.len() - 1);

        // A store that existed before anything shipped, with a preset
        // of the user's own in it, gets the shipped ones beside it;
        // and a user's file under a shipped name is left alone.
        let d2 = dir("seed-existing");
        let store2 = Store::at(d2.clone());
        let mut edit = Edit::default();
        edit.light.exposure = 0.3;
        store2
            .save(&Preset::from_edit("Mine", &edit, &[Section::Light]))
            .unwrap();
        std::fs::write(
            store2.dir().join("warm-negative.gcp"),
            r#"{"name": "Warm Negative", "sections": ["light"], "edit": {"light": {"exposure": 1.0}}}"#,
        )
        .unwrap();
        store2.seed();
        let names: Vec<String> = store2.list().into_iter().map(|e| e.preset.name).collect();
        assert_eq!(names.len(), SHIPPED.len() + 1);
        assert!(names.contains(&"Mine".to_string()));
        let theirs = store2.find("Warm Negative").unwrap();
        assert_eq!(
            theirs.preset.edit.light.exposure, 1.0,
            "the user's file stands"
        );
        greycard_core::testing::remove_dir_retry(&d);
        greycard_core::testing::remove_dir_retry(&d2);
    }

    /// A binned shipped preset comes back when asked for, once, and
    /// the seed's record still keeps it from being written twice.
    #[test]
    fn a_binned_shipped_preset_is_restored_when_asked() {
        let d = dir("restore");
        let store = Store::at(d.clone());
        store.seed();
        assert!(store.missing_shipped().is_empty());
        assert_eq!(store.restore_shipped().unwrap(), Vec::<String>::new());

        let gone = store.find("Muted Slide").unwrap();
        store.remove(&gone).unwrap();
        let gone = store.find("Red-Filter Mono").unwrap();
        store.remove(&gone).unwrap();
        assert_eq!(store.missing_shipped(), ["Muted Slide", "Red-Filter Mono"]);

        assert_eq!(
            store.restore_shipped().unwrap(),
            ["Muted Slide", "Red-Filter Mono"]
        );
        assert!(store.missing_shipped().is_empty());
        assert_eq!(store.list().len(), SHIPPED.len());
        let back = store.find("Muted Slide").unwrap();
        assert_eq!(back.path, d.join("muted-slide.gcp"));
        let (_, text) = SHIPPED[0];
        assert_eq!(back.preset, Preset::from_json(text).unwrap(), "as shipped");

        // Again: nothing to do, and nothing written twice by a seed.
        assert!(store.restore_shipped().unwrap().is_empty());
        store.seed();
        assert_eq!(store.list().len(), SHIPPED.len());
        let record = std::fs::read_to_string(d.join(SEEDED)).unwrap();
        for (stem, _) in SHIPPED {
            assert_eq!(record.lines().filter(|l| *l == stem).count(), 1, "{stem}");
        }
        greycard_core::testing::remove_dir_retry(&d);
    }

    /// A restore writes over nothing of the user's. A preset of a
    /// shipped name counts as the shipped one; a file under a shipped
    /// stem that holds another name, or will not read, does not, and
    /// the shipped preset goes beside it under a free stem.
    #[test]
    fn a_restore_overwrites_nothing_of_the_users() {
        let d = dir("restore-theirs");
        let store = Store::at(d.clone());
        // A store from before anything was seeded: no record at all.
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("warm-negative.gcp"),
            r#"{"name": "My Negative", "sections": ["light"], "edit": {"light": {"exposure": 1.0}}}"#,
        )
        .unwrap();
        std::fs::write(d.join("red-filter-mono.gcp"), "not a preset").unwrap();
        let mut edit = Edit::default();
        edit.light.exposure = -0.5;
        let mine = store
            .save(&Preset::from_edit("muted slide", &edit, &[Section::Light]))
            .unwrap();

        assert_eq!(
            store.missing_shipped(),
            ["Red-Filter Mono", "Warm Negative"]
        );
        assert_eq!(
            store.restore_shipped().unwrap(),
            ["Red-Filter Mono", "Warm Negative"]
        );
        assert!(store.missing_shipped().is_empty());
        let theirs = Preset::load(&d.join("warm-negative.gcp")).unwrap();
        assert_eq!(theirs.name, "My Negative", "the stem's file stands");
        assert_eq!(
            std::fs::read_to_string(d.join("red-filter-mono.gcp")).unwrap(),
            "not a preset",
            "an unreadable one stands too"
        );
        let beside = Preset::load(&d.join("warm-negative-2.gcp")).unwrap();
        assert_eq!(beside.name, "Warm Negative");
        let beside = Preset::load(&d.join("red-filter-mono-2.gcp")).unwrap();
        assert_eq!(beside.name, "Red-Filter Mono");
        let kept = Preset::load(&mine).unwrap();
        assert_eq!(kept.edit.light.exposure, -0.5, "the name's file stands");
        assert_eq!(store.list().len(), 4);

        // Nothing more to do, and every stem is on the record now, so
        // a seed adds nothing either.
        assert!(store.restore_shipped().unwrap().is_empty());
        store.seed();
        assert_eq!(store.list().len(), 4);
        greycard_core::testing::remove_dir_retry(&d);
    }

    #[test]
    fn a_preset_carries_its_sections_and_no_more() {
        let mut edit = Edit::default();
        edit.light.exposure = 1.5;
        edit.vignette.amount = -1.0;
        edit.white_balance = WhiteBalance::Custom {
            temperature: 3200.0,
            tint: 0.0,
        };
        edit.noise.learned = Learned::Best;
        let preset = Preset::from_edit(
            " Warm ",
            &edit,
            &[Section::Vignette, Section::Light, Section::Light],
        );
        assert_eq!(preset.name, "Warm");
        // In the panel's order, each once.
        assert_eq!(preset.sections, vec![Section::Light, Section::Vignette]);
        assert_eq!(preset.edit.light.exposure, 1.5);
        assert_eq!(preset.edit.vignette.amount, -1.0);
        assert_eq!(
            preset.edit.white_balance,
            WhiteBalance::AsShot,
            "not chosen"
        );
        assert_eq!(preset.edit.noise, crate::Noise::default());

        // Laid over another edit, the sections replace and the rest stays.
        let mut other = Edit::default();
        other.light.exposure = -2.0;
        other.grain.amount = 0.3;
        other.white_balance = WhiteBalance::Custom {
            temperature: 6500.0,
            tint: 0.01,
        };
        assert!(!preset.is_applied(&other));
        let out = preset.applied(&other);
        assert_eq!(out.light.exposure, 1.5);
        assert_eq!(out.vignette.amount, -1.0);
        assert_eq!(out.grain.amount, 0.3);
        assert!(
            matches!(out.white_balance, WhiteBalance::Custom { temperature, .. } if temperature == 6500.0)
        );
        assert!(preset.is_applied(&out));

        // The JSON says what is carried, and reads back.
        let json = preset.to_json();
        assert!(json.contains("\"vignette\""), "{json}");
        assert_eq!(Preset::from_json(&json).unwrap(), preset);
    }

    #[test]
    fn the_noise_section_carries_the_profiled_pass_not_the_learned_blend() {
        // A picture edited with the learned denoiser on, at whatever
        // blend its ISO gave it.
        let mut edit = Edit::default();
        edit.noise.profiled = true;
        edit.noise.strength = 1.8;
        edit.noise.learned = Learned::Best;
        edit.noise.learned_strength = 0.35;
        let preset = Preset::from_edit("Denoise", &edit, &[Section::Noise]);
        // The profiled pass comes across; the learned tier and its
        // blend do not, since they are the file's own (its ISO), not
        // the preset's.
        assert!(preset.edit.noise.profiled);
        assert_eq!(preset.edit.noise.strength, 1.8);
        assert_eq!(preset.edit.noise.learned, crate::Noise::default().learned);
        assert_eq!(
            preset.edit.noise.learned_strength,
            crate::Noise::default().learned_strength
        );

        // Laid over a file whose blend already follows its own ISO,
        // with its own tier chosen, applying the preset leaves both
        // alone.
        let mut other = Edit::default();
        other.noise.learned = Learned::Balanced;
        other.noise.learned_strength = crate::Noise::blend_for_iso(Some(100));
        let out = preset.applied(&other);
        assert!(out.noise.profiled);
        assert_eq!(out.noise.strength, 1.8);
        assert_eq!(out.noise.learned, Learned::Balanced, "the file's own tier");
        assert_eq!(
            out.noise.learned_strength,
            crate::Noise::blend_for_iso(Some(100)),
            "the file's own ISO blend, not stamped by the preset"
        );

        // The save sheet's "changed from default" check agrees: a
        // fresh file's ISO-driven blend alone does not make the
        // Noise section look worth saving.
        let mut iso_only = Edit::default();
        iso_only.noise.learned_strength = crate::Noise::blend_for_iso(Some(100));
        assert!(Section::Noise.same(&iso_only, &Edit::default()));
    }

    #[test]
    fn adjustments_come_with_fresh_ids() {
        let mut edit = Edit::default();
        for id in [7, 9] {
            edit.adjustments.push(crate::Adjustment {
                id,
                name: format!("Mask {id}"),
                ..Default::default()
            });
        }
        // A mask with a shape switched off carries the switch: the
        // preset lays down the mask as it was edited, not as it would
        // read with every shape on.
        edit.adjustments[0].mask.components = vec![
            crate::mask::Component::default(),
            crate::mask::Component {
                mode: crate::mask::Mode::Subtract,
                enabled: false,
                ..Default::default()
            },
        ];
        let preset = Preset::from_edit("Masks", &edit, &[Section::Adjustments]);
        let mut onto = Edit::default();
        onto.adjustments.push(crate::Adjustment {
            id: 3,
            name: "Old".into(),
            ..Default::default()
        });
        preset.apply(&mut onto);
        let ids: Vec<u64> = onto.adjustments.iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(onto.adjustments[1].name, "Mask 9");
        assert_eq!(onto.next_id(), 3);
        let carried = &onto.adjustments[0].mask.components;
        assert!(carried[0].enabled);
        assert!(!carried[1].enabled);
    }

    #[test]
    fn one_mask_copied_joins_another_edit_once() {
        use crate::mask::{Component, Shape};
        let mut edit = Edit::default();
        edit.light.exposure = 1.0;
        for (id, shape) in [(2, Shape::Subject {}), (5, Shape::skin())] {
            let mut look = crate::Look::default();
            look.light.exposure = -0.5;
            edit.adjustments.push(crate::Adjustment {
                id,
                name: format!("{} {id}", shape.name()),
                mask: crate::Mask {
                    components: vec![Component {
                        shape,
                        ..Default::default()
                    }],
                    invert: false,
                    ..Default::default()
                },
                look,
                ..Default::default()
            });
        }
        // The copy: that mask and nothing else of the edit.
        let copied = Preset::of_mask(&edit, 5);
        assert_eq!(copied.sections, [Section::Adjustments]);
        assert_eq!(copied.edit.light.exposure, Edit::default().light.exposure);
        assert_eq!(copied.edit.adjustments.len(), 1);
        assert_eq!(copied.edit.adjustments[0].name, "Color 5");
        // Pasted: after the other edit's own, under the next id, its
        // shapes, look and name as copied, the rest untouched.
        let mut onto = Edit::default();
        onto.light.exposure = 0.3;
        onto.adjustments.push(crate::Adjustment {
            id: 3,
            name: "Mine".into(),
            ..Default::default()
        });
        copied.apply(&mut onto);
        assert_eq!(onto.light.exposure, 0.3);
        let pasted = &onto.adjustments[1];
        assert_eq!((onto.adjustments[0].name.as_str(), pasted.id), ("Mine", 4));
        let source = &edit.adjustments[1];
        assert_eq!(
            (&pasted.name, &pasted.mask, &pasted.look),
            (&source.name, &source.mask, &source.look)
        );
        // A second paste adds nothing, nor one onto the edit it came from.
        let once = onto.clone();
        copied.apply(&mut onto);
        assert_eq!(onto, once);
        assert_eq!(copied.applied(&edit), edit);
        // An id the edit does not have carries no mask.
        assert_eq!(Preset::of_mask(&edit, 9).applied(&once), once);
        // A frame at the most masks a picture carries takes none more;
        // one short of it takes one of two.
        let mut full = Edit {
            adjustments: (1..=crate::MAX_MASKS as u64)
                .map(|id| crate::Adjustment {
                    id,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        assert_eq!(copied.applied(&full), full);
        full.adjustments.pop();
        let both = Preset::from_choice(
            "",
            &edit,
            &Choice {
                sections: vec![Section::Adjustments],
                masks_land: Masks::Join,
                ..Default::default()
            },
        );
        let took = both.applied(&full);
        assert_eq!(took.adjustments.len(), crate::MAX_MASKS);
        assert_eq!(took.adjustments.last().unwrap().name, "Subject 2");
    }

    #[test]
    fn an_old_presets_edit_is_migrated() {
        // Version 1's noise.enabled was the profiled denoiser's.
        let old = r#"{"version": 1, "name": "Clean", "sections": ["noise"],
                      "edit": {"noise": {"enabled": true, "strength": 2.0}}}"#;
        let p = Preset::from_json(old).unwrap();
        assert_eq!(p.version, VERSION);
        assert!(p.edit.noise.profiled);
        assert_eq!(p.edit.noise.strength, 2.0);
        // Unknown sections are dropped, and unknown fields ignored.
        let odd = r#"{"name": "X", "sections": ["light"], "edit": {}, "color": 3}"#;
        assert_eq!(
            Preset::from_json(odd).unwrap().sections,
            vec![Section::Light]
        );
        assert!(Preset::from_json(r#"{"version": 99, "edit": {"version": 99}}"#).is_err());
    }

    /// An edit with something set in every part of Light, Detail and
    /// Curves, so a part copied alone shows against the rest.
    fn busy() -> Edit {
        let mut e = Edit::default();
        e.light.exposure = 1.25;
        e.light.tone.contrast = 1.3;
        e.light.tone.highlights = -0.8;
        e.light.tone.midtones = 0.4;
        e.light.tone.shadows = 0.6;
        e.light.tone.whites = 0.2;
        e.light.tone.blacks = -0.1;
        e.detail.texture = 0.3;
        e.detail.clarity = 0.4;
        e.detail.structure = 0.6;
        e.detail.dehaze = 0.5;
        e.curves.red = vec![[0.0, 0.0], [0.5, 0.62], [1.0, 1.0]];
        e.curves.blue = vec![[0.0, 0.1], [1.0, 0.9]];
        e.curves.parametric.lights = 0.4;
        e
    }

    #[test]
    fn the_tint_s_switch_goes_with_its_section() {
        // Tint has no parts: the switch is carried and compared with
        // the hue and the amount, as a whole section's is.
        let from = Edit {
            tint: crate::Tint {
                enabled: false,
                ..crate::Tint::new(210.0, 0.4)
            },
            ..Edit::default()
        };
        let mut onto = Edit {
            tint: crate::Tint::new(210.0, 0.4),
            ..Edit::default()
        };
        assert!(!Section::Tint.same(&from, &onto), "only the switch differs");
        Section::Tint.copy(&from, &mut onto);
        assert!(!onto.tint.enabled);
        assert!(Section::Tint.same(&from, &onto));
    }

    #[test]
    fn a_part_carries_its_slider_and_leaves_the_rest_of_the_section() {
        let from = busy();
        let mut onto = Edit::default();
        onto.light.tone.shadows = -0.3;
        onto.detail.clarity = -0.2;
        onto.detail.enabled = false;
        onto.curves.blue = vec![[0.0, 0.0], [0.4, 0.3], [1.0, 1.0]];

        // Light's exposure alone.
        let choice = Choice {
            parts: vec![Part::LightExposure],
            ..Choice::default()
        };
        let p = Preset::from_choice("Exposure", &from, &choice);
        assert!(p.sections.is_empty());
        assert_eq!(p.parts, vec![Part::LightExposure]);
        let out = p.applied(&onto);
        assert_eq!(out.light.exposure, 1.25);
        let mut want = onto.clone();
        want.light.exposure = 1.25;
        assert_eq!(out, want, "nothing but the exposure moved");
        assert!(p.is_applied(&out));
        assert!(!p.is_applied(&onto));

        // Detail's Dehaze alone: Texture and Clarity are the frame's,
        // and so is the section's switch.
        let p = Preset::from_choice(
            "",
            &from,
            &Choice {
                parts: vec![Part::DetailDehaze],
                ..Choice::default()
            },
        );
        let out = p.applied(&onto);
        let mut want = onto.clone();
        want.detail.dehaze = 0.5;
        assert_eq!(out, want);
        assert!(!out.detail.enabled, "the switch goes only with the whole");

        // Structure alone, by its own name in the file.
        let p = Preset::from_choice(
            "",
            &from,
            &Choice {
                parts: vec![Part::DetailStructure],
                ..Choice::default()
            },
        );
        let out = p.applied(&onto);
        let mut want = onto.clone();
        want.detail.structure = 0.6;
        assert_eq!(out, want);
        assert_eq!(
            Part::from_name("detail.structure"),
            Some(Part::DetailStructure)
        );
        assert_eq!(Part::DetailStructure.title(), crate::STRUCTURE);

        // One curve: the red, and not the blue the frame has its own of.
        let p = Preset::from_choice(
            "",
            &from,
            &Choice {
                parts: vec![Part::CurvesRed],
                ..Choice::default()
            },
        );
        let out = p.applied(&onto);
        let mut want = onto.clone();
        want.curves.red = from.curves.red.clone();
        assert_eq!(out, want);
        assert_eq!(out.curves.parametric, onto.curves.parametric);

        // A part of a section chosen whole is the whole's already.
        let p = Preset::from_choice(
            "",
            &from,
            &Choice {
                sections: vec![Section::Detail],
                parts: vec![Part::DetailDehaze, Part::LightWhites, Part::LightWhites],
                ..Choice::default()
            },
        );
        assert_eq!(p.sections, vec![Section::Detail]);
        assert_eq!(p.parts, vec![Part::LightWhites]);
        let out = p.applied(&onto);
        assert_eq!(out.detail, from.detail);
        assert_eq!(out.light.tone.whites, 0.2);
        assert_eq!(out.light.exposure, onto.light.exposure);
    }

    /// Midtones goes with the whole of Light, a preset's or a sync's,
    /// and alone as its own part, through the file and back.
    #[test]
    fn midtones_goes_with_light_and_alone() {
        let from = busy();
        let mut onto = Edit::default();
        onto.light.tone.midtones = -1.0;
        let whole = Preset::from_edit("Light", &from, &[Section::Light]);
        let whole = Preset::from_json(&whole.to_json()).unwrap();
        assert_eq!(whole.applied(&onto).light, from.light);
        let p = Preset::from_choice(
            "",
            &from,
            &Choice {
                parts: vec![Part::LightMidtones],
                ..Choice::default()
            },
        );
        let json = p.to_json();
        assert!(json.contains("\"light.midtones\""), "{json}");
        let p = Preset::from_json(&json).unwrap();
        let out = p.applied(&onto);
        let mut want = onto.clone();
        want.light.tone.midtones = 0.4;
        assert_eq!(out, want);
        assert_eq!(Part::LightMidtones.title(), "Midtones");
        assert_eq!(Part::LightMidtones.section(), Section::Light);
        // A section chosen whole compares it: a frame whose midtones
        // differs has not had the preset laid.
        let laid = whole.applied(&onto);
        assert!(whole.is_applied(&laid));
        let mut moved = laid.clone();
        moved.light.tone.midtones = 0.0;
        assert!(!whole.is_applied(&moved));
    }

    #[test]
    fn every_part_moves_only_itself_and_the_parts_make_the_section() {
        // A default edit against one with every slider moved: each
        // part laid alone changes the edit, and all of a section's
        // parts together are the section, its switch aside.
        let mut from = busy();
        from.color.vibrance = 0.2;
        from.color.saturation = -0.3;
        from.curves.rgb = vec![[0.0, 0.05], [1.0, 1.0]];
        from.curves.green = vec![[0.0, 0.05], [1.0, 1.0]];
        from.curves.red_green = vec![[0.0, 0.45], [1.0, 0.5]];
        from.curves.blue_yellow = vec![[0.0, 0.55], [1.0, 0.5]];
        for (b, hue) in from.mixer.hue.iter_mut().enumerate() {
            *hue = 0.1 * (b as f32 + 1.0);
        }
        from.bw.weights = [0.3; crate::mixer::BANDS];
        from.bw.strength = 0.5;
        from.grading.shadows.hue = 200.0;
        from.grading.shadows.saturation = 0.2;
        from.grading.midtones.saturation = 0.2;
        from.grading.highlights.saturation = 0.2;
        from.grading.balance = 0.4;
        from.lens.profile = !from.lens.profile;
        from.lens.vignetting = !from.lens.vignetting;
        from.lens.chromatic_aberration = !from.lens.chromatic_aberration;
        from.lens.ca_red = 0.001;
        from.lens.manual = -0.02;
        from.lens.scale = 1.1;
        from.lens.auto_scale = !from.lens.auto_scale;
        from.lens.defringe = !from.lens.defringe;
        from.vignette.amount = -0.7;
        from.vignette.midpoint = 0.3;
        from.vignette.feather = 0.2;
        from.vignette.roundness = 0.6;
        from.grain.amount = 0.4;
        from.grain.size = 40.0;
        from.grain.kind = crate::grain::Kind::Tabular;
        let plain = Edit::default();
        for &p in Part::ALL {
            assert!(!p.same(&from, &plain), "{p:?} moves nothing");
            assert_eq!(Part::from_name(p.name()), Some(p));
            // Laid alone, it moves no other part.
            let mut laid = plain.clone();
            p.copy(&from, &mut laid);
            for &q in Part::ALL {
                if q != p {
                    assert!(q.same(&laid, &plain), "{p:?} moves {q:?}");
                }
            }
        }
        for &s in Section::ALL {
            let parts: Vec<Part> = s.parts().collect();
            if parts.is_empty() {
                continue;
            }
            let mut laid = plain.clone();
            for p in &parts {
                p.copy(&from, &mut laid);
            }
            let mut whole = plain.clone();
            s.copy(&from, &mut whole);
            assert_eq!(laid, whole, "{s:?}'s parts are not the section");
        }
        // The sections without parts are the ones that are one choice.
        let none: Vec<Section> = Section::ALL
            .iter()
            .copied()
            .filter(|s| s.parts().next().is_none())
            .collect();
        assert_eq!(
            none,
            vec![
                Section::Look,
                Section::WhiteBalance,
                Section::DisplayCurve,
                Section::Tint,
                Section::Noise,
                Section::Sharpen,
                Section::OutputSharpen,
                Section::Demosaic,
                Section::Camera,
                Section::Adjustments,
            ]
        );
    }

    #[test]
    fn a_preset_with_parts_round_trips_and_an_old_file_is_whole_sections() {
        let mut from = busy();
        from.adjustments.push(crate::Adjustment {
            id: 4,
            name: "Sky 4".into(),
            ..Default::default()
        });
        let choice = Choice {
            sections: vec![Section::Color, Section::Adjustments],
            parts: vec![Part::DetailDehaze, Part::CurvesRed],
            masks: Some(vec![4]),
            masks_land: Masks::Join,
        };
        let p = Preset::from_choice("Haze", &from, &choice);
        let json = p.to_json();
        assert!(json.contains("\"detail.dehaze\""), "{json}");
        assert!(json.contains("\"masks\": \"join\""), "{json}");
        assert_eq!(Preset::from_json(&json).unwrap(), p);

        // Whole sections write what they always wrote: no parts, no
        // masks key.
        let whole = Preset::from_edit("Old", &from, &[Section::Light]);
        let json = whole.to_json();
        assert!(!json.contains("\"parts\""), "{json}");
        assert!(!json.contains("\"masks\""), "{json}");

        // A file from before parts: whole sections, masks replacing.
        let old = r#"{"version": 3, "name": "Old", "sections": ["light", "adjustments"],
                      "edit": {"light": {"exposure": 0.5},
                               "adjustments": [{"id": 1, "name": "Mask 1"}]}}"#;
        let p = Preset::from_json(old).unwrap();
        assert_eq!(p.sections, vec![Section::Light, Section::Adjustments]);
        assert!(p.parts.is_empty());
        assert_eq!(p.masks, Masks::Replace);
        let mut onto = Edit::default();
        onto.light.tone.shadows = 0.4;
        onto.adjustments.push(crate::Adjustment {
            id: 7,
            name: "Mine".into(),
            ..Default::default()
        });
        let out = p.applied(&onto);
        assert_eq!(out.light.exposure, 0.5);
        assert_eq!(out.light.tone.shadows, 0.0, "the whole of Light, as before");
        let names: Vec<&str> = out.adjustments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Mask 1"]);

        // A part this build does not know is passed over, not the file.
        let newer = r#"{"name": "New", "sections": [], "parts": ["detail.dehaze", "light.glow"],
                        "edit": {}}"#;
        assert_eq!(
            Preset::from_json(newer).unwrap().parts,
            vec![Part::DetailDehaze]
        );
    }

    #[test]
    fn the_chromatic_aberration_of_an_older_preset_carries_both_its_halves() {
        // Saved when the profile's switch and the amounts by hand were
        // one part: both come across, as they did then.
        let old = r#"{"name": "Fringe", "sections": [], "parts": ["lens.chromatic-aberration"],
                      "edit": {"lens": {"chromatic_aberration": true, "ca_red": 0.002,
                                        "ca_blue": -0.001}}}"#;
        let p = Preset::from_json(old).unwrap();
        assert_eq!(
            p.parts,
            vec![
                Part::LensChromaticAberration,
                Part::LensManualChromaticAberration
            ]
        );
        let out = p.applied(&Edit::default());
        assert!(out.lens.chromatic_aberration);
        assert_eq!((out.lens.ca_red, out.lens.ca_blue), (0.002, -0.001));
        // Written again, each by its own name; the old one never is.
        let json = p.to_json();
        assert!(
            json.contains(r#""lens.profile-chromatic-aberration""#),
            "{json}"
        );
        assert!(
            json.contains(r#""lens.manual-chromatic-aberration""#),
            "{json}"
        );
        assert!(!json.contains(r#""lens.chromatic-aberration""#), "{json}");
        // The switch alone leaves the amounts as the picture has them.
        let mut onto = Edit::default();
        onto.lens.ca_red = -0.003;
        let mut from = Edit::default();
        from.lens.chromatic_aberration = true;
        Part::LensChromaticAberration.copy(&from, &mut onto);
        assert!(onto.lens.chromatic_aberration);
        assert_eq!(onto.lens.ca_red, -0.003);
    }

    #[test]
    fn a_whole_section_preset_writes_what_it_wrote_before_parts() {
        // The preset as it was before parts and the masks' landing:
        // these four keys, in this order.
        #[derive(Serialize)]
        struct Before<'a> {
            version: u32,
            name: &'a str,
            sections: &'a [&'a str],
            edit: &'a Edit,
        }
        let mut edit = busy();
        edit.look_lut.strength = 0.7;
        let p = Preset::from_edit("Warm", &edit, &[Section::Light, Section::Look]);
        let golden = serde_json::to_string_pretty(&Before {
            version: VERSION,
            name: "Warm",
            // The sections in the panel's order, Look first.
            sections: &["look", "light"],
            edit: &p.edit,
        })
        .unwrap();
        assert_eq!(p.to_json(), golden);
        assert!(p.to_json().starts_with(
            "{\n  \"version\": 8,\n  \"name\": \"Warm\",\n  \"sections\": [\n    \"look\",\n    \"light\"\n  ],\n  \"edit\": {"
        ));
    }

    #[test]
    fn how_masks_land_is_kept_only_when_masks_go() {
        let mut from = busy();
        from.adjustments.push(crate::Adjustment {
            id: 1,
            name: "Sky 1".into(),
            ..Default::default()
        });
        // "Add to masks" chosen with no mask ticked: nothing about
        // masks is written.
        let p = Preset::from_choice(
            "",
            &from,
            &Choice {
                sections: vec![Section::Light],
                masks_land: Masks::Join,
                ..Choice::default()
            },
        );
        assert_eq!(p.masks, Masks::Replace);
        assert!(!p.to_json().contains("\"masks\""));
        // A file that says join with no masks carried reads as replace.
        let odd = r#"{"name": "X", "sections": ["light"], "masks": "join", "edit": {}}"#;
        assert_eq!(Preset::from_json(odd).unwrap().masks, Masks::Replace);
        // With the masks it stays.
        let p = Preset::from_choice(
            "",
            &from,
            &Choice {
                sections: vec![Section::Adjustments],
                masks_land: Masks::Join,
                ..Choice::default()
            },
        );
        assert_eq!(p.masks, Masks::Join);
        // A null list of parts is none.
        let null = r#"{"name": "N", "sections": ["light"], "parts": null, "edit": {}}"#;
        assert!(Preset::from_json(null).unwrap().parts.is_empty());
    }

    #[test]
    fn chosen_masks_replace_the_frames_or_join_them() {
        let mask = |id: u64, name: &str| crate::Adjustment {
            id,
            name: name.into(),
            ..Default::default()
        };
        let from = Edit {
            adjustments: vec![mask(2, "Sky 2"), mask(5, "Face 5"), mask(6, "Linear 6")],
            ..Edit::default()
        };
        let onto = Edit {
            adjustments: vec![mask(1, "Mine 1"), mask(3, "Mine 3")],
            ..Edit::default()
        };

        // One mask, replacing: the frame's own are gone.
        let one = |land| Choice {
            sections: vec![Section::Adjustments],
            masks: Some(vec![5]),
            masks_land: land,
            ..Choice::default()
        };
        let p = Preset::from_choice("", &from, &one(Masks::Replace));
        let out = p.applied(&onto);
        let names: Vec<&str> = out.adjustments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Face 5"]);
        assert_eq!(out.adjustments[0].id, 1);

        // Joining: the frame's own kept, the mask after them with an
        // id after theirs.
        let p = Preset::from_choice("", &from, &one(Masks::Join));
        let out = p.applied(&onto);
        let names: Vec<&str> = out.adjustments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Mine 1", "Mine 3", "Face 5"]);
        let ids: Vec<u64> = out.adjustments.iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![1, 3, 4]);
        // Laid again, nothing is added twice.
        assert!(p.is_applied(&out));
        assert_eq!(p.applied(&out), out);

        // Every mask, None, is all of them in the picture's order.
        let p = Preset::from_choice(
            "",
            &from,
            &Choice {
                sections: vec![Section::Adjustments],
                masks_land: Masks::Join,
                ..Choice::default()
            },
        );
        let out = p.applied(&onto);
        let names: Vec<&str> = out.adjustments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Mine 1", "Mine 3", "Sky 2", "Face 5", "Linear 6"]
        );
        // The masks named are only read when the masks go.
        let p = Preset::from_choice(
            "",
            &from,
            &Choice {
                sections: vec![Section::Light],
                masks: Some(vec![5]),
                ..Choice::default()
            },
        );
        assert!(p.edit.adjustments.is_empty());
        assert_eq!(p.applied(&onto).adjustments, onto.adjustments);
    }

    #[test]
    fn carried_words_count_sections_and_parts() {
        assert_eq!(carried_words(3, 0), "3 sections");
        assert_eq!(carried_words(1, 0), "1 section");
        assert_eq!(carried_words(0, 1), "1 part");
        assert_eq!(carried_words(2, 4), "2 sections and 4 parts");
    }

    /// The display curve is a section of its own: checked, a paste
    /// lays the source's curve on the target; unchecked, or with Light
    /// alone, the target keeps its own; and a preset written before the
    /// section, which cannot name it, leaves every picture's as it is.
    #[test]
    fn the_display_curve_goes_as_a_section_of_its_own() {
        use crate::DisplayCurve;
        let mut agx = Edit {
            display_curve: DisplayCurve::Agx,
            ..Edit::default()
        };
        agx.light.exposure = 0.5;
        let channels = Edit::default();
        let with = Preset::between_frames(
            &agx,
            &Choice::whole(&[Section::Light, Section::DisplayCurve]),
        );
        assert_eq!(with.applied(&channels).display_curve, DisplayCurve::Agx);
        assert!(with.is_applied(&with.applied(&channels)));
        // Back the other way: per channel is laid too, not passed over
        // as a default.
        let back = Preset::between_frames(&channels, &Choice::whole(&[Section::DisplayCurve]));
        assert_eq!(back.applied(&agx).display_curve, DisplayCurve::Channels);
        assert!(!back.is_applied(&agx));
        // Unchecked: the light goes, the target's curve stays.
        let without = Preset::between_frames(&agx, &Choice::whole(&[Section::Light]));
        let out = without.applied(&channels);
        assert_eq!(out.light.exposure, 0.5);
        assert_eq!(out.display_curve, DisplayCurve::Channels);
        assert_eq!(without.applied(&agx).display_curve, DisplayCurve::Agx);
        // A file round trip keeps it, and an older file without it
        // reads and carries none.
        let read = Preset::from_json(&with.to_json()).unwrap();
        assert_eq!(read.sections, vec![Section::Light, Section::DisplayCurve]);
        assert_eq!(read.applied(&channels).display_curve, DisplayCurve::Agx);
        let old = r#"{"name": "Old", "sections": ["light"], "edit": {}}"#;
        let old = Preset::from_json(old).unwrap();
        assert_eq!(old.applied(&agx).display_curve, DisplayCurve::Agx);
    }

    #[test]
    fn section_names_round_trip() {
        for s in Section::ALL {
            assert_eq!(Section::from_name(s.name()), Some(*s));
            assert_eq!(
                serde_json::to_string(s).unwrap(),
                format!("\"{}\"", s.name())
            );
        }
        let on: Vec<_> = Section::ALL.iter().filter(|s| s.by_default()).collect();
        assert_eq!(on.len(), 15);
        // A film preset is a look and a strength, so the look comes
        // across by default as the rest of the look does.
        assert!(on.contains(&&Section::Look));
    }

    #[test]
    fn slugs_are_file_names() {
        assert_eq!(slug("Warm Film, v2"), "warm-film-v2");
        assert_eq!(slug("  --  "), "preset");
        assert_eq!(slug("Été/été"), "été-été");
    }

    #[test]
    fn the_store_lists_saves_finds_and_removes() {
        let store = Store::at(dir("store"));
        assert!(store.list().is_empty(), "no directory is no presets");
        let mut edit = Edit::default();
        edit.light.exposure = 0.3;
        let warm = Preset::from_edit("Warm", &edit, &[Section::Light]);
        let path = store.save(&warm).unwrap();
        assert!(path.ends_with("warm.gcp"), "{path:?}");
        edit.grain.amount = 0.5;
        let grainy = Preset::from_edit("a grainy one", &edit, &[Section::Grain]);
        store.save(&grainy).unwrap();
        // A file of a name of its own, with no name inside.
        std::fs::write(
            store.dir().join("odd-name.gcp"),
            r#"{"sections": ["grain"], "edit": {"grain": {"amount": 0.1}}}"#,
        )
        .unwrap();
        std::fs::write(store.dir().join("junk.gcp"), "not json").unwrap();
        std::fs::write(store.dir().join("notes.txt"), "nor a preset").unwrap();
        let names: Vec<_> = store.list().into_iter().map(|e| e.preset.name).collect();
        assert_eq!(names, vec!["a grainy one", "odd-name", "Warm"]);

        assert_eq!(store.find("warm").unwrap().preset, warm);
        assert!(store.find("cool").is_none());
        assert_eq!(store.find(path.to_str().unwrap()).unwrap().preset, warm);
        // Saved again under the name, the same file is written.
        let warmer = Preset::from_edit("WARM", &edit, &[Section::Light, Section::Grain]);
        assert_eq!(store.save(&warmer).unwrap(), path);
        assert_eq!(store.list().len(), 3);
        assert_eq!(store.find("Warm").unwrap().preset.sections.len(), 2);
        assert!(store.save(&Preset::default()).is_err(), "a name is needed");

        let entry = store.find("odd-name").unwrap();
        store.remove(&entry).unwrap();
        assert_eq!(store.list().len(), 2);
        greycard_core::testing::remove_dir_retry(store.dir());
    }

    /// The presets outside any group come first, then each group's,
    /// the groups alphabetically and case aside, by name within each.
    /// A folder in a group and a hidden folder are not groups. A name
    /// is found in whichever group holds it, and two files of one name
    /// give the first in that order.
    #[test]
    fn the_store_lists_groups_after_the_ungrouped_in_order() {
        let store = Store::at(dir("groups"));
        let save_in = |group: Option<&str>, name: &str| {
            let d = store.group_dir(group);
            std::fs::create_dir_all(&d).unwrap();
            Preset::from_edit(name, &Edit::default(), &[Section::Light])
                .save_to(&d.join(format!("{}.{EXTENSION}", slug(name))))
                .unwrap();
        };
        save_in(None, "Zed");
        save_in(None, "alpha");
        save_in(Some("film"), "Portra");
        save_in(Some("film"), "Ektar");
        save_in(Some("Black and white"), "Tri-X");
        save_in(Some(".hidden"), "Hidden");
        save_in(Some("film/deeper"), "Deeper");
        save_in(Some("Black and white"), "alpha");
        let listed: Vec<(Option<String>, String)> = store
            .list()
            .into_iter()
            .map(|e| (store.group_of(&e.path), e.preset.name))
            .collect();
        let g = |s: &str| Some(s.to_string());
        assert_eq!(
            listed,
            [
                (None, "alpha".to_string()),
                (None, "Zed".into()),
                (g("Black and white"), "alpha".into()),
                (g("Black and white"), "Tri-X".into()),
                (g("film"), "Ektar".into()),
                (g("film"), "Portra".into()),
            ]
        );
        assert_eq!(store.groups(), ["Black and white", "film"]);
        assert_eq!(
            store.group_of(&store.find("ektar").unwrap().path),
            g("film")
        );
        assert_eq!(store.group_of(&store.find("alpha").unwrap().path), None);
        let key = store.key_of(&store.find("Tri-X").unwrap().path).unwrap();
        assert_eq!(key, "Black and white/tri-x.gcp");
        assert_eq!(
            store.key_of(&store.find("Zed").unwrap().path).unwrap(),
            "zed.gcp"
        );
        // A save of a grouped name is written where it is.
        let path = store.find("Portra").unwrap().path;
        let again = Preset::from_edit("portra", &Edit::default(), &[Section::Grain]);
        assert_eq!(store.save(&again).unwrap(), path);
        greycard_core::testing::remove_dir_retry(store.dir());
    }

    /// A preset moved into a group, between groups, and out again
    /// keeps its name and what it holds. A file name the group has
    /// takes the next free one; a name typed in another case finds the
    /// group there; the group a move or a removal empties goes.
    /// Renamed, a group keeps its presets; a name another group has,
    /// and one that is nothing as a folder's, are refused.
    #[test]
    fn a_preset_moved_between_groups_keeps_its_name_and_content() {
        let store = Store::at(dir("move"));
        let mut edit = Edit::default();
        edit.grain.amount = 0.4;
        let grain = Preset::from_edit("Grain", &edit, &[Section::Grain]);
        let path = store.save(&grain).unwrap();
        let entry = |name: &str| store.find(name).unwrap();

        let moved = store.move_to(&entry("Grain"), Some("Film")).unwrap();
        assert_eq!(moved, store.dir().join("Film").join("grain.gcp"));
        assert!(!path.exists());
        assert_eq!(entry("Grain").preset, grain);
        assert_eq!(
            store.group_of(&entry("Grain").path).as_deref(),
            Some("Film")
        );

        // Into another group, typed in another case, which holds a file
        // of that name already.
        std::fs::create_dir_all(store.dir().join("Mono")).unwrap();
        Preset::from_edit("Other", &Edit::default(), &[Section::Light])
            .save_to(&store.dir().join("Mono").join("grain.gcp"))
            .unwrap();
        let moved = store.move_to(&entry("Grain"), Some(" mono ")).unwrap();
        assert_eq!(moved, store.dir().join("Mono").join("grain-2.gcp"));
        assert_eq!(entry("Grain").preset, grain);
        assert!(!store.dir().join("Film").exists(), "the empty group goes");
        assert_eq!(store.groups(), ["Mono"]);
        // Where it is already: nothing moves.
        assert_eq!(store.move_to(&entry("Grain"), Some("Mono")).unwrap(), moved);

        // Renamed: the presets go with it.
        assert!(store.rename_group("Mono", "  ").is_err());
        assert!(store.rename_group("Gone", "New").is_err());
        std::fs::create_dir_all(store.dir().join("Taken")).unwrap();
        let e = store.rename_group("Mono", "taken").unwrap_err();
        assert_eq!(
            e.to_string(),
            "preset: there is a group called \"Taken\" already"
        );
        assert_eq!(
            store.rename_group("Mono", "Black/White").unwrap(),
            "Black-White"
        );
        assert_eq!(
            store.group_of(&entry("Grain").path).as_deref(),
            Some("Black-White")
        );

        // Out of any group: the store's own folder, under its file
        // name again, and the group it leaves holds the other preset.
        let moved = store.move_to(&entry("Grain"), None).unwrap();
        assert_eq!(moved, store.dir().join("grain-2.gcp"));
        assert_eq!(entry("Grain").preset, grain);
        assert!(store.dir().join("Black-White").exists());
        store.remove(&entry("Other")).unwrap();
        assert!(
            !store.dir().join("Black-White").exists(),
            "a removal empties it"
        );
        assert!(store.move_to(&entry("Grain"), Some("..")).is_err());
        greycard_core::testing::remove_dir_retry(store.dir());
    }

    #[test]
    fn a_group_name_is_made_a_folder_s() {
        assert_eq!(group_folder("Film: Kodak").as_deref(), Some("Film- Kodak"));
        assert_eq!(group_folder(" .hidden. ").as_deref(), Some("hidden"));
        assert_eq!(group_folder("a/b\\c").as_deref(), Some("a-b-c"));
        assert_eq!(group_folder(" .. "), None);
        assert_eq!(group_folder(""), None);
    }
}
