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

use crate::{Edit, Error, Result, VERSION, migrate, refused};

/// The parts of an edit a preset can carry. Not the geometry nor the
/// retouch, which are one picture's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Section {
    Light,
    WhiteBalance,
    Noise,
    /// Local contrast: Texture and Clarity.
    Detail,
    Sharpen,
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
            Section::WhiteBalance => "white-balance",
            Section::Noise => "noise",
            Section::Detail => "detail",
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
            Section::WhiteBalance => "White balance",
            Section::Noise => "Noise",
            Section::Detail => "Detail",
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
            // Not the display curve's switch: a sidecar leaves it out at
            // its default, so a preset made before it and one made from
            // a picture per channel read alike, and carrying it would
            // turn the switch off on every picture such a preset is
            // laid on. The switch is the picture's.
            Section::Light => onto.light = from.light,
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
            Section::Sharpen => onto.sharpen = from.sharpen,
            Section::Curves => onto.curves = from.curves.clone(),
            Section::Mixer => onto.mixer = from.mixer,
            Section::Color => onto.color = from.color,
            Section::BlackWhite => onto.bw = from.bw,
            Section::Grading => onto.grading = from.grading,
            Section::Tint => onto.tint = from.tint,
            Section::Look => onto.look_lut = from.look_lut.clone(),
            Section::Lens => onto.lens = from.lens,
            Section::Vignette => onto.vignette = from.vignette,
            Section::Grain => onto.grain = from.grain,
            Section::Demosaic => onto.demosaic = from.demosaic,
            Section::Camera => onto.camera = from.camera.clone(),
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
            Section::WhiteBalance => a.white_balance == b.white_balance,
            // As `copy`: the learned tier and its blend are not part
            // of what a preset carries or compares.
            Section::Noise => {
                a.noise.enabled == b.noise.enabled
                    && a.noise.profiled == b.noise.profiled
                    && a.noise.strength == b.noise.strength
            }
            Section::Detail => a.detail == b.detail,
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
            Section::Camera => a.camera == b.camera,
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
    /// The profile's switch and its distortion.
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
            LightExposure | LightContrast | LightHighlights | LightShadows | LightWhites
            | LightBlacks => Section::Light,
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
            DetailTexture | DetailClarity | DetailDehaze => Section::Detail,
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
            GradingMidtones => "Mid-tones",
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
    /// is not laid twice, so a second paste adds nothing.
    pub fn apply(&self, onto: &mut Edit) {
        for s in &self.sections {
            if *s == Section::Adjustments && self.joins_masks() {
                for a in &self.edit.adjustments {
                    if !has_mask(onto, a) {
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

/// Read a preset file of either kind: Lightroom's `.xmp`, or this
/// engine's; the name from inside, or the file's stem.
pub fn import(path: &Path) -> Result<crate::lightroom::Imported> {
    let text = std::fs::read_to_string(path)?;
    if path
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("xmp"))
    {
        return crate::lightroom::read(&text, &stem_name(path));
    }
    let mut preset = Preset::from_json(&text)?;
    if preset.name.is_empty() {
        preset.name = stem_name(path);
    }
    Ok(crate::lightroom::Imported {
        preset,
        unmapped: Vec::new(),
        offset: 0.0,
    })
}

/// Why a preset file did not come into the store.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// The file would not read.
    #[error("{0}")]
    Unreadable(std::io::Error),
    /// It read, and is not a preset: not XMP with Camera Raw settings
    /// in it, not a preset's JSON, or one that carries nothing this
    /// engine has. Why, in words.
    #[error("not a preset: {0}")]
    NotAPreset(String),
    /// A preset of ours from a build newer than this one: its schema.
    #[error("from a newer greycard (schema {0})")]
    Newer(u32),
    /// A preset whose name is blank, which no row could show.
    #[error("a preset needs a name")]
    Nameless,
    /// The store has a preset of this name, in any case: the stored
    /// one's name, as the list shows it.
    #[error("there is a preset called \"{0}\" already")]
    Taken(String),
    /// It read, and could not be written into the store.
    #[error("saving {0}: {why}", why = plain(.1))]
    Unsaved(String, Error),
}

/// An error's own words: a file system's without the "sidecar: " that
/// [`Error::Io`] puts before them, which a preset is not.
pub fn plain(e: &Error) -> String {
    match e {
        Error::Io(e) => e.to_string(),
        e => e.to_string(),
    }
}

impl Store {
    /// Bring a preset file into the store: Lightroom's `.xmp` or this
    /// engine's `.gcp`, read as [`import`] reads it, with what did not
    /// come across. The editor's Import and the command line's
    /// `--import` both come here.
    ///
    /// A name the store has already is refused rather than written
    /// over, where a save from the editor writes over it: a file
    /// brought in is not necessarily a newer copy of the one there, and
    /// the one there may be the user's own. A blank name is refused, as
    /// a save refuses one. With `replace` the caller says it is, and the
    /// one there is written over in its own file; the window refuses,
    /// having a Remove, and the command line replaces on `--replace`.
    /// A new name's file is the first free one, so a different name
    /// with the same slug ("Warm, Film" beside "Warm Film") takes no
    /// other preset's file either.
    pub fn import(
        &self,
        path: &Path,
        replace: bool,
    ) -> std::result::Result<(PathBuf, crate::lightroom::Imported), ImportError> {
        let imported = import(path).map_err(|e| match e {
            Error::Io(e) => ImportError::Unreadable(e),
            Error::Newer(v) => ImportError::Newer(v),
            Error::Json(e) => ImportError::NotAPreset(e.to_string()),
            Error::Preset(why) | Error::Xmp(why) | Error::Pp3(why) => ImportError::NotAPreset(why),
        })?;
        let preset = &imported.preset;
        if preset.is_empty() {
            let why = match imported.unmapped.as_slice() {
                [] => "it carries nothing".to_string(),
                passed => format!("nothing in it has a place here ({})", passed.join(", ")),
            };
            return Err(ImportError::NotAPreset(why));
        }
        let name = preset.name.trim();
        if name.is_empty() {
            return Err(ImportError::Nameless);
        }
        let there = self
            .list()
            .into_iter()
            .find(|e| e.preset.name.trim().eq_ignore_ascii_case(name));
        let written = match there {
            Some(entry) if replace => entry.path,
            Some(entry) => return Err(ImportError::Taken(entry.preset.name)),
            None => {
                let slug = slug(name);
                std::iter::once(self.path_for(name))
                    .chain((2..).map(|n| self.dir.join(format!("{slug}-{n}.{EXTENSION}"))))
                    .find(|p| !p.exists())
                    .expect("a free file name")
            }
        };
        preset
            .save_to(&written)
            .map_err(|e| ImportError::Unsaved(name.to_string(), e))?;
        Ok((written, imported))
    }
}

/// A file's stem as a name.
fn stem_name(path: &Path) -> String {
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

/// A preset as found in the store.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub preset: Preset,
}

/// Where the presets live: a directory of `.gcp` files.
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

    /// Every preset in the store, by name; a file that will not read
    /// is passed over with a word on stderr.
    pub fn list(&self) -> Vec<Entry> {
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<Entry> = dir
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == EXTENSION))
            .filter_map(|path| match Preset::load(&path) {
                Ok(preset) => Some(Entry { path, preset }),
                Err(e) => {
                    log::warn!("preset {}: {e}", path.display());
                    None
                }
            })
            .collect();
        out.sort_by(|a, b| {
            a.preset
                .name
                .to_lowercase()
                .cmp(&b.preset.name.to_lowercase())
                .then_with(|| a.path.cmp(&b.path))
        });
        out
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

    pub fn remove(&self, entry: &Entry) -> Result<()> {
        Ok(std::fs::remove_file(&entry.path)?)
    }

    /// Put the film presets this build ships in the store, each one
    /// once.
    ///
    /// A store that was there before a preset shipped gets it too (a
    /// user with presets of their own is the common case, not the
    /// exception), so the record of what has been seeded is not the
    /// directory's existence but a `seeded` file in it listing the
    /// stems put there so far. A stem on that list is never written
    /// again: one edited stays edited and one binned stays binned.
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
            if seeded.iter().any(|s| s == stem) {
                continue;
            }
            let path = self.dir.join(format!("{stem}.{EXTENSION}"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Learned, WhiteBalance};

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("greycard-preset-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
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
        // does not change.
        for preset in SHIPPED.map(|(_, t)| Preset::from_json(t).unwrap()) {
            let bare = Edit::default();
            for s in Section::ALL {
                let changed = !s.same(&preset.edit, &bare);
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

        // Red-Filter Mono is mono, and through a red filter: the one
        // thing here that its name states outright.
        let mono = by_name("red-filter-mono");
        assert_eq!(mono.name, "Red-Filter Mono");
        assert!(mono.edit.bw.enabled);
        assert_eq!(mono.edit.bw.filter(), Some(crate::bw::Filter::Red));
        assert!(!mono.edit.grain.is_off());
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
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
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
        let _ = std::fs::remove_dir_all(&d);
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
        let _ = std::fs::remove_dir_all(&d);
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
        e.light.tone.shadows = 0.6;
        e.light.tone.whites = 0.2;
        e.light.tone.blacks = -0.1;
        e.detail.texture = 0.3;
        e.detail.clarity = 0.4;
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
        for b in 0..crate::mixer::BANDS {
            from.mixer.hue[b] = 0.1 * (b as f32 + 1.0);
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
                Section::Tint,
                Section::Noise,
                Section::Sharpen,
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
            "{\n  \"version\": 6,\n  \"name\": \"Warm\",\n  \"sections\": [\n    \"look\",\n    \"light\"\n  ],\n  \"edit\": {"
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
        assert_eq!(on.len(), 13);
        // A film preset is a look and a strength, so the look comes
        // across by default as the rest of the look does.
        assert!(on.contains(&&Section::Look));
    }

    #[test]
    fn import_reads_either_kind() {
        let d = dir("import");
        std::fs::create_dir_all(&d).unwrap();
        let xmp = d.join("Faded.XMP");
        std::fs::write(&xmp, r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="+1.0" crs:Dehaze="+20"/></rdf:RDF></x:xmpmeta>"#).unwrap();
        let got = import(&xmp).unwrap();
        assert_eq!(got.preset.name, "Faded");
        assert_eq!(got.preset.edit.light.exposure, 1.0);
        assert!((got.preset.edit.detail.dehaze - 0.2).abs() < 1e-6);
        assert_eq!(got.preset.sections, vec![Section::Light, Section::Detail]);
        assert!(got.unmapped.is_empty());
        let gcp = d.join("Mine.gcp");
        std::fs::write(
            &gcp,
            r#"{"sections": ["grain"], "edit": {"grain": {"amount": 0.2}}}"#,
        )
        .unwrap();
        let got = import(&gcp).unwrap();
        assert_eq!(got.preset.name, "Mine");
        assert_eq!(got.preset.edit.grain.amount, 0.2);
        assert!(got.unmapped.is_empty());
        assert!(import(&d.join("none.gcp")).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// The store's import: a preset in, under its slug, with what did
    /// not come across; a name the store has refused in any case, the
    /// one there untouched; a file that is no preset, or carries
    /// nothing, refused; and a name whose slug another preset's file
    /// has takes the next file, not that one.
    #[test]
    fn the_store_imports_and_refuses_a_name_it_has() {
        let d = dir("store-import");
        std::fs::create_dir_all(&d).unwrap();
        let store = Store::at(d.join("store"));
        let xmp = d.join("faded.xmp");
        std::fs::write(&xmp, r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="+1.0" crs:RedHue="+10"/></rdf:RDF></x:xmpmeta>"#).unwrap();
        let (written, got) = store.import(&xmp, false).unwrap();
        assert_eq!(written, store.dir().join("faded.gcp"));
        assert_eq!(got.preset.name, "faded");
        assert_eq!(got.unmapped, vec!["RedHue"]);
        assert_eq!(store.list().len(), 1);

        // Again, and as another case: refused, and the file the same.
        let before = std::fs::read_to_string(&written).unwrap();
        let other = d.join("FADED.gcp");
        std::fs::write(
            &other,
            r#"{"sections": ["grain"], "edit": {"grain": {"amount": 0.2}}}"#,
        )
        .unwrap();
        for path in [&xmp, &other] {
            match store.import(path, false) {
                // The stored one's name, not the incoming "FADED".
                Err(ImportError::Taken(name)) => assert_eq!(name, "faded"),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(std::fs::read_to_string(&written).unwrap(), before);

        // Asked to replace: the other case's file goes over the one
        // there, in that one's own file, and the store has one still.
        let (replaced, got) = store.import(&other, true).unwrap();
        assert_eq!(replaced, written);
        assert_eq!(got.preset.name, "FADED");
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].preset.edit.grain.amount, 0.2);
        // And back, for what follows.
        store.import(&xmp, true).unwrap();
        assert_eq!(std::fs::read_to_string(&written).unwrap(), before);

        // Not a preset, nothing in it, not there.
        let junk = d.join("junk.xmp");
        std::fs::write(&junk, "not xml at all").unwrap();
        assert!(matches!(
            store.import(&junk, false),
            Err(ImportError::NotAPreset(_))
        ));
        let masks = d.join("masks.xmp");
        std::fs::write(&masks, r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:RedHue="+10"/></rdf:RDF></x:xmpmeta>"#).unwrap();
        match store.import(&masks, false) {
            Err(ImportError::NotAPreset(why)) => assert!(why.contains("RedHue"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            store.import(&d.join("gone.xmp"), false),
            Err(ImportError::Unreadable(_))
        ));
        // A name of spaces alone, which no row could show, and one of
        // ours from a newer build.
        let blank = d.join("blank.gcp");
        std::fs::write(
            &blank,
            r#"{"name": "   ", "sections": ["grain"], "edit": {"grain": {"amount": 0.2}}}"#,
        )
        .unwrap();
        assert!(matches!(
            store.import(&blank, true),
            Err(ImportError::Nameless)
        ));
        let newer = d.join("newer.gcp");
        std::fs::write(
            &newer,
            format!(
                r#"{{"version": {}, "name": "Next", "sections": ["grain"], "edit": {{}}}}"#,
                VERSION + 1
            ),
        )
        .unwrap();
        match store.import(&newer, false) {
            Err(e @ ImportError::Newer(v)) => {
                assert_eq!(v, VERSION + 1);
                assert_eq!(
                    e.to_string(),
                    format!("from a newer greycard (schema {})", VERSION + 1)
                );
            }
            other => panic!("{other:?}"),
        }
        // A write that fails says so in the file system's words.
        let unsaved = ImportError::Unsaved(
            "Other".into(),
            Error::Io(std::io::Error::other("disk full")),
        );
        assert_eq!(unsaved.to_string(), "saving Other: disk full");
        assert_eq!(store.list().len(), 1);

        // "Faded!" is another name with the same slug: its own file.
        let bang = d.join("bang.gcp");
        std::fs::write(
            &bang,
            r#"{"name": "Faded!", "sections": ["grain"], "edit": {"grain": {"amount": 0.2}}}"#,
        )
        .unwrap();
        let (written, _) = store.import(&bang, false).unwrap();
        assert_eq!(written, store.dir().join("faded-2.gcp"));
        assert_eq!(
            std::fs::read_to_string(store.dir().join("faded.gcp")).unwrap(),
            before
        );
        assert_eq!(store.list().len(), 2);
        std::fs::remove_dir_all(&d).unwrap();
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
        std::fs::remove_dir_all(store.dir()).unwrap();
    }
}
