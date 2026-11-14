//! The CPU reference pipeline: levels, white balance, demosaic, matrix.
//!
//! Each step is a public function so it can be tested alone and so a GPU
//! implementation has something to be checked against. [`develop`] runs
//! them in order into the working space; [`demosaic`] stops after the
//! demosaic and hands back camera-space RGB, which is what a linear DNG
//! stores.

pub mod amaze;
pub mod bad_pixels;
pub mod ca;
pub mod ca_xtrans;
pub mod defringe;
pub mod dehaze;
pub mod denoise;
pub mod dual;
pub mod gain_map;
pub mod highlights;
pub mod hotpixels;
pub mod lens;
pub mod local_contrast;
pub mod nlm;
pub mod noise;
pub mod profile;
pub mod rcd;
pub mod reduced;
pub mod refine_edge;
pub mod remap;
pub mod retouch;
pub mod segments;
pub mod sharpen;
pub mod sites;
pub(crate) mod sum;
pub mod vng4;
pub mod warp;
pub mod xtrans;

use std::sync::Arc;

use rayon::prelude::*;

use crate::color::{self, WhitePoint};
use crate::error::{Error, Result};
use crate::image::{CameraImage, WorkingImage};
use crate::opcodes::{BadPixels, MosaicOp, OpcodeList};
use crate::raw::{
    BlackDeltas, CfaPattern, Mosaic, Orientation, RawFrame, Rect, Samples, SensorLayout,
};

/// What to develop a frame with.
///
/// Not `Copy`: a camera profile carries a hue/saturation/value map,
/// which is a hundred kilobytes and is shared between the develops of
/// one file rather than copied into each.
#[derive(Debug, Clone, PartialEq)]
pub struct DevelopSettings {
    pub white_point: WhitePoint,
    /// The camera profile to develop with, in place of the frame's own
    /// calibrations: a DCP's matrix pair, its forward matrices and its
    /// hue/saturation/value map. `None` is the file's.
    pub profile: Option<Arc<color::Profile>>,
    /// Whether the profile's look table is applied after its map. On
    /// by default; an edit made before the table was applied turns it
    /// off to keep its picture.
    pub profile_look: bool,
    /// The stops the consumer's default develop brightens the picture
    /// by, its `BaselineExposure`: the look table is read that much and
    /// the profile's `BaselineExposureOffset` above the develop, where
    /// a default develop in DNG's pipeline would read it. Only where
    /// the table is read; the picture's brightness stays the consumer's.
    pub look_baseline: f32,
    pub demosaic: DemosaicMethod,
    pub highlights: HighlightMode,
    /// How [`HighlightMode::Segments`] forms and judges its segments.
    pub segments: segments::SegmentOptions,
    /// Lateral chromatic aberration correction before the demosaic; `None`
    /// leaves the mosaic as shot.
    pub chromatic_aberration: Option<ca::CaOptions>,
    /// The contrast threshold of the dual demosaics; ignored by the others.
    pub dual_contrast: dual::DualContrast,
    /// Profiled noise reduction after the demosaic, from a noise model
    /// measured on the frame; `None` leaves the noise.
    pub denoise: Option<denoise::DenoiseOptions>,
    /// Hot and dead photosite repair on the mosaic, against the noise
    /// measured on the frame; `None` leaves them. On by default: the
    /// repair takes a site only when the other colors round it sit at
    /// their background, which a glint or a catchlight never does.
    pub hot_pixels: Option<hotpixels::HotPixelOptions>,
    /// The orientation to show the frame in, in place of the tag the
    /// file carries. `None` is the file's own, which is what every
    /// path but the editor's wants; the editor passes the camera's
    /// tag with the frame's quarter turns composed onto it, for the
    /// frames a camera got wrong.
    pub orientation: Option<Orientation>,
}

// The editor's thumbnail of a raw with no preview is a develop under
// these defaults, and the thumbnail cache keeps it on disk: a change
// here that changes that picture wants `THUMB_RECIPE` in
// greycard-ui's worker raised, or old thumbnails stay.
impl Default for DevelopSettings {
    fn default() -> Self {
        Self {
            white_point: WhitePoint::AsShot,
            profile: None,
            profile_look: true,
            look_baseline: 0.0,
            demosaic: DemosaicMethod::default(),
            highlights: HighlightMode::default(),
            segments: segments::SegmentOptions::default(),
            chromatic_aberration: Some(ca::CaOptions::default()),
            dual_contrast: dual::DualContrast::default(),
            denoise: None,
            hot_pixels: Some(hotpixels::HotPixelOptions::default()),
            orientation: None,
        }
    }
}

/// What to do with photosites the sensor saturated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum HighlightMode {
    /// Clip every channel at 1.0 after white balance: blown areas go to
    /// neutral white, and everything the brighter channels still held is
    /// lost with them.
    Clip,
    /// Reconstruct clipped photosites from the other channels before the
    /// demosaic (see [`highlights`]), then clip at the brightest channel's
    /// saturation. Output above 1.0 is real scene light for the consumer's
    /// tone mapping to roll off.
    #[default]
    InpaintOpposed,
    /// Inpaint opposed, then refine each clipped region with its own
    /// chrominance measured on its border (see [`segments`]).
    Segments,
}

impl HighlightMode {
    pub const ALL: &[HighlightMode] = &[
        HighlightMode::Clip,
        HighlightMode::InpaintOpposed,
        HighlightMode::Segments,
    ];

    pub fn name(self) -> &'static str {
        match self {
            HighlightMode::Clip => "clip",
            HighlightMode::InpaintOpposed => "opposed",
            HighlightMode::Segments => "segments",
        }
    }

    /// Whether the mode reconstructs at all.
    pub fn reconstructs(self) -> bool {
        !matches!(self, HighlightMode::Clip)
    }
}

impl std::str::FromStr for HighlightMode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|m| m.name() == s)
            .ok_or_else(|| Error::Unsupported(format!("highlight mode {s:?}")))
    }
}

/// The demosaic algorithms the engine has. Every one is a CPU reference
/// with tests; the benchmark harness scores them against reference images.
///
/// Each is defined on a mosaic: bilinear on Bayer and X-Trans both, the
/// Markesteijn pair on X-Trans alone, the rest on Bayer alone.
/// [`DemosaicMethod::for_pattern`] lists a mosaic's, and
/// [`demosaic_cfa_with`] refuses a method its mosaic does not take. A
/// develop asked for a method of the other mosaic (a setting carried
/// from another camera's frame) takes the mosaic's default instead, by
/// [`DemosaicMethod::for_mosaic`], the one place that decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum DemosaicMethod {
    /// The placeholder: soft, correct, works on any RGB pattern.
    Bilinear,
    /// Ratio Corrected Demosaicing, Bayer only. See [`rcd`]. Faster than
    /// AMaZE and smoother on noisy or strongly colored edges.
    Rcd,
    /// AMaZE, Bayer only: the best detail and the least aliasing of the
    /// single algorithms. See [`amaze`].
    Amaze,
    /// VNG4, Bayer only: dcraw's variable number of gradients with the two
    /// greens kept apart. Soft, and free of the maze that noise gives the
    /// sharper algorithms. See [`vng4`].
    Vng4,
    /// RCD where there is detail, VNG4 where there is only noise. See
    /// [`dual`].
    RcdVng4,
    /// AMaZE where there is detail, VNG4 where there is only noise, the
    /// default: with the threshold from the frame's measured noise it
    /// scores above AMaZE alone at every noise level, clean included
    /// (notes §13m), for about twice the time. See [`dual`].
    #[default]
    AmazeVng4,
    /// Markesteijn's X-Trans demosaic in one pass and four directions:
    /// about half the time of three. See [`xtrans`].
    Markesteijn1,
    /// Markesteijn's X-Trans demosaic in three passes and eight
    /// directions, the X-Trans default: the sharper. See [`xtrans`].
    Markesteijn3,
}

impl DemosaicMethod {
    pub const ALL: &[DemosaicMethod] = &[
        DemosaicMethod::Bilinear,
        DemosaicMethod::Rcd,
        DemosaicMethod::Amaze,
        DemosaicMethod::Vng4,
        DemosaicMethod::RcdVng4,
        DemosaicMethod::AmazeVng4,
        DemosaicMethod::Markesteijn1,
        DemosaicMethod::Markesteijn3,
    ];

    /// A Bayer mosaic's methods, the default first.
    const BAYER: &[DemosaicMethod] = &[
        DemosaicMethod::AmazeVng4,
        DemosaicMethod::RcdVng4,
        DemosaicMethod::Amaze,
        DemosaicMethod::Rcd,
        DemosaicMethod::Vng4,
        DemosaicMethod::Bilinear,
    ];

    /// An X-Trans mosaic's methods, the default first.
    const XTRANS: &[DemosaicMethod] = &[
        DemosaicMethod::Markesteijn3,
        DemosaicMethod::Markesteijn1,
        DemosaicMethod::Bilinear,
    ];

    /// The methods defined on a `mosaic`, its default first; none for
    /// a mosaic the engine does not demosaic.
    pub fn for_kind(mosaic: Mosaic) -> &'static [DemosaicMethod] {
        match mosaic {
            Mosaic::XTrans => Self::XTRANS,
            Mosaic::Bayer => Self::BAYER,
            Mosaic::Other => &[],
        }
    }

    /// [`DemosaicMethod::for_kind`] of `pattern`'s mosaic.
    pub fn for_pattern(pattern: &CfaPattern) -> &'static [DemosaicMethod] {
        Self::for_kind(pattern.mosaic())
    }

    /// The method a `mosaic` takes when nothing else is asked:
    /// AMaZE+VNG4 on Bayer, Markesteijn's three passes on X-Trans.
    pub fn default_for_kind(mosaic: Mosaic) -> DemosaicMethod {
        Self::for_kind(mosaic).first().copied().unwrap_or_default()
    }

    /// [`DemosaicMethod::default_for_kind`] of `pattern`'s mosaic.
    pub fn default_for(pattern: &CfaPattern) -> DemosaicMethod {
        Self::default_for_kind(pattern.mosaic())
    }

    /// Whether this method is defined on a `mosaic`.
    pub fn takes_kind(self, mosaic: Mosaic) -> bool {
        Self::for_kind(mosaic).contains(&self)
    }

    /// Whether this method is defined on `pattern`'s mosaic.
    pub fn takes(self, pattern: &CfaPattern) -> bool {
        self.takes_kind(pattern.mosaic())
    }

    /// [`DemosaicMethod::for_mosaic`] by the mosaic's kind alone, for
    /// what is known of a file before its pixels are read.
    pub fn resolve(self, mosaic: Mosaic) -> DemosaicMethod {
        if self.takes_kind(mosaic) {
            self
        } else {
            Self::default_for_kind(mosaic)
        }
    }

    /// The method a develop asked for this one runs on `pattern`'s
    /// mosaic: this one where the mosaic takes it, else the mosaic's
    /// default. A Bayer method stored for an X-Trans frame (an older
    /// sidecar's default, a sync from another camera) and the reverse
    /// resolve here, and only here: the develop and the editor's menu
    /// both ask this.
    pub fn for_mosaic(self, pattern: &CfaPattern) -> DemosaicMethod {
        self.resolve(pattern.mosaic())
    }

    /// Whether this blends two demosaics by contrast (see [`dual`]).
    pub fn is_dual(self) -> bool {
        matches!(self, DemosaicMethod::RcdVng4 | DemosaicMethod::AmazeVng4)
    }

    pub fn name(self) -> &'static str {
        match self {
            DemosaicMethod::Bilinear => "bilinear",
            DemosaicMethod::Rcd => "rcd",
            DemosaicMethod::Amaze => "amaze",
            DemosaicMethod::Vng4 => "vng4",
            DemosaicMethod::RcdVng4 => "rcd-vng4",
            DemosaicMethod::AmazeVng4 => "amaze-vng4",
            DemosaicMethod::Markesteijn1 => "markesteijn-1",
            DemosaicMethod::Markesteijn3 => "markesteijn-3",
        }
    }
}

impl std::str::FromStr for DemosaicMethod {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|m| m.name() == s)
            .ok_or_else(|| Error::Unsupported(format!("demosaic method {s:?}")))
    }
}

/// Demosaic CFA samples with the chosen method. `samples` are one per
/// pixel, levels normalized, white balance applied; `pattern` is RGB only.
/// Fails when the method does not handle the pattern. The dual methods
/// measure their contrast threshold; [`demosaic_cfa_with`] sets it.
pub fn demosaic_cfa(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    method: DemosaicMethod,
) -> Result<Vec<f32>> {
    demosaic_cfa_with(
        samples,
        width,
        height,
        pattern,
        method,
        dual::DualContrast::Auto,
    )
    .map(|(rgb, _)| rgb)
}

/// What is said about a mosaic this build cannot demosaic, in one
/// place, so the gate at the door and the demosaic's own backstop say
/// the same thing.
fn unsupported_mosaic(pattern: &CfaPattern) -> Error {
    Error::Unsupported(format!(
        "this build does not develop a {pattern} mosaic; it demosaics 2x2 Bayer and 6x6 X-Trans"
    ))
}

/// Whether the engine demosaics this pattern: 2x2 Bayer, or X-Trans.
pub fn demosaics(pattern: &CfaPattern) -> bool {
    rcd::is_bayer(pattern) || pattern.is_xtrans()
}

/// [`demosaic_cfa`] with the dual methods' contrast threshold chosen, and
/// what they decided returned alongside.
pub fn demosaic_cfa_with(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    method: DemosaicMethod,
    contrast: dual::DualContrast,
) -> Result<(Vec<f32>, Option<dual::DualStats>)> {
    if !demosaics(pattern) {
        return Err(unsupported_mosaic(pattern));
    }
    // Each method on its own mosaic; a consumer resolves a choice from
    // the other one first ([`DemosaicMethod::for_mosaic`]).
    if !method.takes(pattern) {
        let kind = if pattern.is_xtrans() {
            "a Bayer"
        } else {
            "an X-Trans"
        };
        return Err(Error::Unsupported(format!(
            "{} is {kind} demosaic and this is a {pattern} mosaic",
            method.name()
        )));
    }
    let single = |rgb: Result<Vec<f32>>| rgb.map(|rgb| (rgb, None));
    let dual = |base| {
        dual::demosaic_dual(samples, width, height, pattern, base, contrast)
            .map(|(rgb, stats)| (rgb, Some(stats)))
    };
    match method {
        DemosaicMethod::Bilinear => Ok((demosaic_bilinear(samples, width, height, pattern), None)),
        DemosaicMethod::Rcd => single(rcd::demosaic_rcd(samples, width, height, pattern)),
        DemosaicMethod::Amaze => single(amaze::demosaic_amaze(samples, width, height, pattern)),
        DemosaicMethod::Vng4 => single(vng4::demosaic_vng4(samples, width, height, pattern)),
        DemosaicMethod::RcdVng4 => dual(DemosaicMethod::Rcd),
        DemosaicMethod::AmazeVng4 => dual(DemosaicMethod::Amaze),
        DemosaicMethod::Markesteijn1 => single(xtrans::demosaic_markesteijn(
            samples,
            width,
            height,
            pattern,
            xtrans::Passes::One,
        )),
        DemosaicMethod::Markesteijn3 => single(xtrans::demosaic_markesteijn(
            samples,
            width,
            height,
            pattern,
            xtrans::Passes::Three,
        )),
    }
}

/// A developed frame plus the white balance that was applied to it.
#[derive(Debug, Clone)]
pub struct Developed {
    pub image: WorkingImage,
    pub white_balance: rawcolor::WhiteBalance,
    /// What highlight reconstruction measured, when it ran.
    pub highlights: Option<highlights::HighlightStats>,
    /// What the segment pass of highlight reconstruction did, when it ran.
    pub segments: Option<segments::SegmentStats>,
    /// What chromatic aberration correction measured, when it ran.
    pub chromatic_aberration: Option<ca::CaStats>,
    /// What a dual demosaic decided, when one ran.
    pub dual: Option<dual::DualStats>,
    /// The noise measured on the frame, when denoising ran.
    pub noise: Option<noise::NoiseEstimate>,
    /// What the denoise did, when it ran.
    pub denoise: Option<denoise::DenoiseStats>,
    /// What the hot pixel repair did, when it ran.
    pub hot_pixels: Option<hotpixels::HotPixelStats>,
    /// The point spread the mosaic suggests, when it could be measured:
    /// what an automatic sharpen radius uses.
    pub sharpen_radius: Option<f32>,
    /// The working-space value at or above which a channel is taken as
    /// clipped: what a sharpen after the develop should leave alone.
    pub clip_level: f32,
    /// The demosaic that ran, resolved for the frame's mosaic; `None`
    /// for a frame with no mosaic.
    pub demosaic: Option<DemosaicMethod>,
}

/// A demosaiced frame in camera space plus the white balance it was
/// demosaiced under.
#[derive(Debug, Clone)]
pub struct Demosaiced {
    pub image: CameraImage,
    pub white_balance: rawcolor::WhiteBalance,
}

/// A frame made ready for the demosaic: levels, the noise measured,
/// hot pixels repaired, white balance gains applied, highlights
/// reconstructed, chromatic aberration corrected. What [`develop`] does
/// before the demosaic, held so a consumer can put its own demosaic and
/// denoise (a learned one, say) between [`prepare`] and [`finish`].
#[derive(Debug, Clone)]
pub struct Prepared {
    /// One sample per pixel for a CFA frame, interleaved RGB for a
    /// linear one, and for a monochrome one its plane three times;
    /// white balance applied.
    pub samples: Vec<f32>,
    pub width: usize,
    pub height: usize,
    /// The pattern as seen from the crop's origin; `None` for a linear
    /// frame.
    pub pattern: Option<CfaPattern>,
    /// The white balance gains the samples carry.
    pub gains: [f32; 3],
    pub white_balance: rawcolor::WhiteBalance,
    /// The noise measured on the frame before the gains, when it was.
    pub noise: Option<noise::NoiseEstimate>,
    /// The value above which nothing is real: 1.0, or the largest gain
    /// when highlights were reconstructed.
    pub ceiling: f32,
    pub highlights: Option<highlights::HighlightStats>,
    pub segments: Option<segments::SegmentStats>,
    pub chromatic_aberration: Option<ca::CaStats>,
    pub hot_pixels: Option<hotpixels::HotPixelStats>,
    pub sharpen_radius: Option<f32>,
    /// The demosaic the frame takes: the settings' resolved for its
    /// mosaic ([`resolve_settings`]).
    pub demosaic: DemosaicMethod,
    /// The orientation the move into the working space shows the frame
    /// in: the settings' or the file's own tag.
    pub orientation: Orientation,
    /// A monochrome sensor's: its three channels are one plane, which
    /// the denoise takes as one ([`denoise::denoise_picture`]).
    pub monochrome: bool,
}

impl Prepared {
    /// The noise model in the samples' units: measured before the
    /// gains, transformed for them. `None` when it was not measured.
    pub fn noise_model(&self) -> Option<noise::NoiseModel> {
        self.noise.map(|e| e.model.after_gains(self.gains))
    }
}

/// A lateral chromatic aberration correction with [`ca::correct_ca`]'s
/// contract, for a consumer that runs it elsewhere than this crate's
/// reference (on a GPU, say): given the balanced mosaic, its size, its
/// pattern and the options, the corrected mosaic and what was measured.
/// What it hands back must be the reference's answer to the tolerance
/// the consumer states, since the export develops on the reference. It
/// is handed X-Trans mosaics too, which the reference corrects in
/// [`ca_xtrans`]; one that has no version for them (the GPU's) runs the
/// reference's.
pub type CaCorrector<'a> = &'a dyn Fn(
    &[f32],
    usize,
    usize,
    &CfaPattern,
    &ca::CaOptions,
) -> Result<(Vec<f32>, ca::CaStats)>;

/// Everything [`develop`] does before the demosaic. With
/// `measure_noise` the frame's noise model is estimated (a consumer
/// that denoises wants it); [`develop`] measures it when its settings
/// need it.
pub fn prepare(
    frame: &RawFrame,
    settings: &DevelopSettings,
    measure_noise: bool,
) -> Result<Prepared> {
    prepare_with(frame, settings, measure_noise, None)
}

/// [`prepare`] with the chromatic aberration correction run by `ca`
/// in place of the reference's when one is given.
pub fn prepare_with(
    frame: &RawFrame,
    settings: &DevelopSettings,
    measure_noise: bool,
    ca: Option<CaCorrector<'_>>,
) -> Result<Prepared> {
    prepare_inner(frame, settings, measure_noise, ca, true)
}

/// [`prepare`] with nothing clipped after the gains: what a merge
/// reads, where a channel the sensor saturated must still read its
/// own clip after the gains are divided back out, and a red photosite
/// at half its range under a gain of two is not cut to the green's.
/// The ceiling is the largest gain, the most any channel can reach.
pub fn prepare_unclipped(
    frame: &RawFrame,
    settings: &DevelopSettings,
    measure_noise: bool,
) -> Result<Prepared> {
    prepare_inner(frame, settings, measure_noise, None, false)
}

fn prepare_inner(
    frame: &RawFrame,
    settings: &DevelopSettings,
    measure_noise: bool,
    ca: Option<CaCorrector<'_>>,
    clip: bool,
) -> Result<Prepared> {
    let settings = &resolve_settings(frame, settings);
    // At the door, before the levels are normalized and the frame is
    // copied: preparing a 101 MP mosaic this build cannot demosaic
    // costs a gigabyte and several seconds to reach the same refusal.
    if let SensorLayout::Cfa(pattern) = &frame.layout {
        if !demosaics(pattern) {
            return Err(unsupported_mosaic(pattern));
        }
        if pattern.is_xtrans() {
            // The one stage before the demosaic with no X-Trans version
            // yet, said in the log rather than passed by quietly; an
            // automatic sharpen takes its default radius.
            log::info!(
                "X-Trans mosaic: the capture sharpen's radius measurement has no X-Trans version yet and is left out"
            );
        }
    }
    let white_balance = white_balance_for(frame, settings)?;
    let gains = white_balance.coefficients_f32();

    let (mut samples, width, height, pattern, noise, hot_pixels) = balanced_samples(
        frame,
        gains,
        measure_noise || wants_noise(settings),
        settings.hot_pixels,
    )?;

    // The mosaic's point spread, from its sharpest green pair, before
    // anything smooths it. Greens clip at their gain.
    let sharpen_radius = pattern.as_ref().and_then(|p| {
        sharpen::measure_radius(
            &samples,
            width,
            height,
            p,
            RADIUS_LOWER_LIMIT,
            gains[1] * CLIP_FRACTION,
        )
    });

    let mut stats = None;
    let mut segment_stats = None;
    let mut ceiling = 1.0f32;
    if let Some(pattern) = &pattern
        && settings.highlights.reconstructs()
    {
        let clips = gains.map(|g| g * highlights::CLIP_MAGIC);
        let (reconstructed, s) =
            highlights::inpaint_opposed(&samples, width, height, pattern, clips)?;
        if settings.highlights == HighlightMode::Segments {
            let (refined, s) = segments::inpaint_segments(
                &samples,
                &reconstructed,
                width,
                height,
                pattern,
                clips,
                &settings.segments,
            )?;
            samples = refined;
            segment_stats = Some(s);
        } else {
            samples = reconstructed;
        }
        stats = Some(s);
        ceiling = gains.iter().copied().fold(1.0, f32::max);
    }
    if clip {
        clip_highlights(&mut samples, ceiling);
    } else {
        ceiling = gains.iter().copied().fold(1.0, f32::max);
    }

    let mut ca_stats = None;
    // A DNG whose warps correct each color has corrected its lateral
    // aberration itself: the warp after the demosaic is the authority.
    let file_corrects_ca = frame.dng.warps_each_color();
    if file_corrects_ca && settings.chromatic_aberration.is_some() {
        log::debug!("the CA correction left to the file's own warps");
    }
    if let (Some(pattern), Some(options)) = (&pattern, &settings.chromatic_aberration)
        && ca::corrects(pattern)
        && !file_corrects_ca
    {
        let (corrected, s) = match ca {
            Some(correct) => correct(&samples, width, height, pattern, options)?,
            None => ca::correct_ca(&samples, width, height, pattern, options)?,
        };
        samples = corrected;
        ca_stats = Some(s);
    }

    Ok(Prepared {
        samples,
        width,
        height,
        pattern,
        gains,
        white_balance,
        noise,
        ceiling,
        highlights: stats,
        segments: segment_stats,
        chromatic_aberration: ca_stats,
        hot_pixels,
        sharpen_radius,
        demosaic: settings.demosaic,
        orientation: settings.orientation.unwrap_or(frame.orientation),
        monochrome: frame.layout == SensorLayout::Monochrome,
    })
}

/// The engine's own demosaic and profiled denoise of a prepared frame:
/// interleaved camera RGB in the samples' units, and what the dual
/// demosaic and the denoise reported.
pub fn demosaic_prepared(
    prepared: &Prepared,
    settings: &DevelopSettings,
) -> Result<(
    Vec<f32>,
    Option<dual::DualStats>,
    Option<denoise::DenoiseStats>,
)> {
    let (width, height) = (prepared.width, prepared.height);
    let (mut rgb, dual_stats) = match &prepared.pattern {
        Some(pattern) => demosaic_cfa_with(
            &prepared.samples,
            width,
            height,
            pattern,
            prepared.demosaic,
            dual_contrast(settings, prepared.noise.as_ref(), prepared.gains),
        )?,
        None => (prepared.samples.clone(), None),
    };
    let denoise_stats = match (&settings.denoise, prepared.noise_model()) {
        (Some(options), Some(model)) => Some(denoise::denoise_picture(
            &mut rgb,
            width,
            height,
            &model,
            options,
            prepared.monochrome,
        )?),
        _ => None,
    };
    Ok((rgb, dual_stats, denoise_stats))
}

/// Everything [`develop`] does after the demosaic: the matrix into the
/// working space, the camera profile's map, and the orientation. `rgb`
/// is interleaved camera RGB of the prepared frame's size, in its
/// units.
///
/// The dehaze and the capture sharpen are not here: they read the
/// picture once it has stopped moving, after whatever geometry a
/// consumer puts on top ([`dehaze::dehaze`], [`sharpen::sharpen`]).
pub fn finish(
    prepared: Prepared,
    rgb: Vec<f32>,
    dual: Option<dual::DualStats>,
    denoise: Option<denoise::DenoiseStats>,
    settings: &DevelopSettings,
) -> Result<Developed> {
    let mut image = WorkingImage::from_data(prepared.width, prepared.height, rgb)?;
    apply_matrix(&mut image.data, prepared.white_balance.matrix_f32());
    // The profile's map, straight after the matrix it is defined
    // against, and before anything that moves a pixel: this is the
    // last of the camera's own color. A map that does nothing is not
    // run at all.
    if let Some(profile) = &settings.profile
        && let Some(map) = profile.map_at_cct(prepared.white_balance.temp_tint.cct)?
        && !map.is_identity()
    {
        profile::MapStage::new(map)?.apply(&mut image);
    }
    // Then its look table, where DNG puts it: after the map and before
    // the tone curve, which is the consumer's. DNG reads it after the
    // exposure, the file's BaselineExposure plus the profile's offset;
    // the consumer's baseline stands for the first.
    if settings.profile_look
        && let Some(profile) = &settings.profile
        && let Some(look) = &profile.look
        && !look.is_identity()
    {
        let stops = settings.look_baseline + profile.baseline_exposure_offset.unwrap_or(0.0);
        profile::MapStage::look((**look).clone(), stops)?.apply(&mut image);
    }
    let image = orient(image, prepared.orientation);
    let clip_level = prepared.ceiling * CLIP_FRACTION;
    Ok(Developed {
        image,
        white_balance: prepared.white_balance,
        highlights: prepared.highlights,
        segments: prepared.segments,
        chromatic_aberration: prepared.chromatic_aberration,
        dual,
        noise: prepared.noise,
        denoise,
        hot_pixels: prepared.hot_pixels,
        sharpen_radius: prepared.sharpen_radius,
        clip_level,
        demosaic: prepared.pattern.as_ref().map(|_| prepared.demosaic),
    })
}

/// Decode-independent development of a frame into the working space:
/// [`prepare`], [`demosaic_prepared`], [`finish`].
pub fn develop(frame: &RawFrame, settings: &DevelopSettings) -> Result<Developed> {
    develop_with(frame, settings, None)
}

/// [`develop`] with the chromatic aberration correction run by `ca`
/// in place of the reference's when one is given; see
/// [`CaCorrector`].
pub fn develop_with(
    frame: &RawFrame,
    settings: &DevelopSettings,
    ca: Option<CaCorrector<'_>>,
) -> Result<Developed> {
    let prepared = prepare_with(frame, settings, false, ca)?;
    let (mut rgb, dual, denoise) = demosaic_prepared(&prepared, settings)?;
    apply_dng_warps(frame, &mut rgb, prepared.width, prepared.height, 1);
    finish(prepared, rgb, dual, denoise, settings)
}

/// The frame's DNG warps on its demosaiced camera RGB, binned by `bin`:
/// the lens's distortion as the file corrects it, on the camera's own
/// geometry, before the move into the working space turns it.
pub fn apply_dng_warps(
    frame: &RawFrame,
    rgb: &mut Vec<f32>,
    width: usize,
    height: usize,
    bin: usize,
) {
    if let Some(area) = warp::area_of(frame) {
        let at = warp::WarpPlacement::binned(area, bin);
        warp::apply_warps(rgb, width, height, at, &frame.dng.warps);
        log::debug!("DNG warps: {} applied", frame.dng.warps.len());
    }
}

/// Below this a sample is too dark to say how sharp the mosaic is:
/// RawTherapee's 1000 of 65535.
const RADIUS_LOWER_LIMIT: f32 = 1000.0 / 65535.0;
/// This close to the clip a value counts as clipped.
pub(crate) const CLIP_FRACTION: f32 = 0.98;

/// Demosaic a frame and stop before the color: camera-space RGB with no
/// white balance, no matrix and no orientation applied, a DNG's own warps
/// on it, since the linear DNG it is stored as carries no opcode list.
///
/// The gains are still applied before the demosaic, because demosaic
/// algorithms assume balanced channels, and divided out afterwards. Nothing
/// is clipped: a sample the sensor saturated comes out at 1.0 in its own
/// channel, which is the fact a consumer's highlight handling needs.
pub fn demosaic(frame: &RawFrame, settings: &DevelopSettings) -> Result<Demosaiced> {
    let settings = &resolve_settings(frame, settings);
    let white_balance = white_balance_for(frame, settings)?;
    let gains = white_balance.coefficients_f32();

    let (samples, width, height, pattern, noise, _) =
        balanced_samples(frame, gains, wants_noise(settings), settings.hot_pixels)?;
    let mut rgb = match &pattern {
        Some(pattern) => {
            demosaic_cfa_with(
                &samples,
                width,
                height,
                pattern,
                settings.demosaic,
                dual_contrast(settings, noise.as_ref(), gains),
            )?
            .0
        }
        None => samples,
    };
    if let (Some(options), Some(estimate)) = (&settings.denoise, &noise) {
        denoise::denoise_picture(
            &mut rgb,
            width,
            height,
            &estimate.model.after_gains(gains),
            options,
            frame.layout == SensorLayout::Monochrome,
        )?;
    }
    // A linear DNG carries no opcode list: the lens's warp goes in.
    apply_dng_warps(frame, &mut rgb, width, height, 1);
    apply_gains_rgb(&mut rgb, gains.map(|g| 1.0 / g));
    rgb.par_iter_mut().for_each(|s| *s = s.clamp(0.0, 1.0));

    let image = CameraImage::from_data(width, height, rgb)?;
    Ok(Demosaiced {
        image,
        white_balance,
    })
}

/// `settings` with the demosaic resolved for `frame`'s mosaic
/// ([`DemosaicMethod::for_mosaic`]): the one resolve of a develop, made
/// at its start, so everything after it (the noise it measures for a
/// dual demosaic, the demosaic) reads the method that runs. A method
/// that gives way is warned of, unless it is the engine's default,
/// which is what an edit stores when nobody chose (every X-Trans frame
/// under an edit from before X-Trans would warn otherwise).
pub fn resolve_settings(frame: &RawFrame, settings: &DevelopSettings) -> DevelopSettings {
    let mut resolved = settings.clone();
    if let SensorLayout::Cfa(pattern) = &frame.layout {
        let method = settings.demosaic.for_mosaic(pattern);
        if method != settings.demosaic && settings.demosaic != DemosaicMethod::default() {
            log::warn!(
                "{} is not a demosaic for a {pattern} mosaic; {}, its default",
                settings.demosaic.name(),
                method.name()
            );
        }
        resolved.demosaic = method;
    }
    resolved
}

/// Whether the pipeline needs the frame's noise measured: for denoising,
/// or for a dual demosaic left to set its own threshold.
fn wants_noise(settings: &DevelopSettings) -> bool {
    settings.denoise.is_some()
        || settings.hot_pixels.is_some()
        || (settings.demosaic.is_dual() && settings.dual_contrast == dual::DualContrast::Auto)
}

/// The dual demosaic's threshold setting with `Auto` resolved to the
/// measured noise, transformed for the gains the RGB carries: what
/// [`demosaic_prepared`] hands the dual, for a device implementation to
/// hand its own.
pub fn dual_contrast(
    settings: &DevelopSettings,
    noise: Option<&noise::NoiseEstimate>,
    gains: [f32; 3],
) -> dual::DualContrast {
    match (settings.dual_contrast, noise) {
        (dual::DualContrast::Auto, Some(estimate)) => {
            dual::DualContrast::Noise(estimate.model.after_gains(gains))
        }
        (contrast, _) => contrast,
    }
}

fn white_balance_for(
    frame: &RawFrame,
    settings: &DevelopSettings,
) -> Result<rawcolor::WhiteBalance> {
    frame.validate()?;
    let chosen;
    let profile = match &settings.profile {
        Some(profile) => &profile.camera,
        None => {
            chosen = color::profile_from_frame(frame)?;
            &chosen
        }
    };
    color::resolve_white_balance(frame, profile, settings.white_point)
}

/// Samples, width, height, the crop's pattern, the noise estimate, and
/// what the hot pixel repair did.
type BalancedSamples = (
    Vec<f32>,
    usize,
    usize,
    Option<CfaPattern>,
    Option<noise::NoiseEstimate>,
    Option<hotpixels::HotPixelStats>,
);

/// Levels normalized, cropped, white balance gains applied in camera space.
///
/// Returns the samples, the crop's dimensions and, for a CFA frame, the
/// pattern as seen from the crop's origin. Linear frames come back
/// interleaved RGB with no pattern, and monochrome ones as three equal
/// channels. With `estimate_noise`, a Bayer or
/// X-Trans frame's noise is measured before the gains go on, and with
/// `hot_pixels` the defects are repaired against it.
fn balanced_samples(
    frame: &RawFrame,
    gains: [f32; 3],
    estimate_noise: bool,
    hot_pixels: Option<hotpixels::HotPixelOptions>,
) -> Result<BalancedSamples> {
    let normalized = normalize_levels(frame);
    let crop = frame.effective_crop();
    let (mut samples, width, height) = crop_samples(&normalized, frame.width, frame.channels, crop);
    drop(normalized);
    // A DNG's first two opcode lists go on the linear samples before
    // anything reads them, as the specification runs them: the noise
    // measured and every pass after see the mosaic the file meant.
    apply_dng_mosaic(frame, &mut samples, width, height, crop);

    let mut noise = None;
    let mut hot = None;
    let pattern = match &frame.layout {
        SensorLayout::Cfa(pattern) => {
            let pattern = pattern.shifted(crop.x, crop.y);
            if !pattern.is_rgb() {
                return Err(Error::Unsupported(format!(
                    "CFA with non-RGB filters: {pattern:?}"
                )));
            }
            if estimate_noise && noise::measures(&pattern) {
                noise = Some(noise::estimate_noise(&samples, width, height, &pattern)?);
            }
            if let (Some(options), Some(estimate)) = (hot_pixels, &noise)
                && hotpixels::repairs(&pattern)
            {
                hot = Some(hotpixels::fix_hot_pixels(
                    &mut samples,
                    width,
                    height,
                    &pattern,
                    &estimate.model,
                    &options,
                )?);
            }
            apply_gains_cfa(&mut samples, width, &pattern, gains);
            Some(pattern)
        }
        SensorLayout::Linear => {
            // A linear file has no mosaic to measure. A merge of ours is
            // taken at its `NoiseProfile`, which it measured from its
            // frames; another maker's (a phone's, a denoised DNG's) says
            // something of its own pipeline and is left alone.
            if estimate_noise
                && frame.dng.merged.is_some()
                && let Some(model) = frame.dng.noise_profile
            {
                noise = Some(noise::NoiseEstimate {
                    model,
                    blocks: [0; 3],
                    bins: [0; 3],
                });
            }
            apply_gains_rgb(&mut samples, gains);
            None
        }
        SensorLayout::Monochrome => {
            // One plane, every photosite the same color: an RGGB
            // mosaic's same-color neighbors two apart are its neighbors
            // two apart, so the Bayer measure and repair read it as one.
            //
            // A merge of ours is taken at its `NoiseProfile` and left
            // unrepaired, as a linear merge is: its noise is the weighted
            // mean of several frames', and its photosites no one frame's.
            let plane = CfaPattern::rggb();
            let merged = frame.dng.merged.is_some();
            if estimate_noise && merged {
                noise = frame.dng.noise_profile.map(|model| noise::NoiseEstimate {
                    model,
                    blocks: [0; 3],
                    bins: [0; 3],
                });
            } else if estimate_noise {
                noise = Some(noise::estimate_noise(&samples, width, height, &plane)?.pooled());
            }
            if let (Some(options), Some(estimate), false) = (hot_pixels, &noise, merged) {
                hot = Some(hotpixels::fix_hot_pixels(
                    &mut samples,
                    width,
                    height,
                    &plane,
                    &estimate.model,
                    &options,
                )?);
            }
            samples = samples.par_iter().flat_map_iter(|&v| [v; 3]).collect();
            apply_gains_rgb(&mut samples, gains);
            None
        }
    };
    Ok((samples, width, height, pattern, noise, hot))
}

/// The frame's DNG opcodes of its first two lists on its cropped,
/// normalized samples, in their order. A crop that reaches outside the
/// area the second list is measured in (which no camera writes) leaves
/// that list undone, said in the log.
fn apply_dng_mosaic(
    frame: &RawFrame,
    samples: &mut [f32],
    width: usize,
    height: usize,
    crop: Rect,
) {
    if frame.dng.mosaic.is_empty() {
        return;
    }
    let area = frame.dng.opcode_area.unwrap_or(Rect {
        x: 0,
        y: 0,
        width: frame.width,
        height: frame.height,
    });
    let inside = crop.x >= area.x && crop.y >= area.y;
    if !inside {
        log::warn!("DNG OpcodeList2 left undone: the crop {crop:?} is outside {area:?}");
    }
    let at = gain_map::MapPlacement {
        x: crop.x.saturating_sub(area.x),
        y: crop.y.saturating_sub(area.y),
        image_width: area.width,
        image_height: area.height,
    };
    let (mut maps, mut held, mut curves, mut moved, mut made) = (0, 0, 0, 0, 0);
    for op in &frame.dng.mosaic {
        match op {
            MosaicOp::GainMap(map) if inside => {
                held += gain_map::apply_gain_maps(
                    samples,
                    width,
                    frame.channels,
                    at,
                    std::slice::from_ref(map),
                );
                maps += 1;
            }
            MosaicOp::Remap(map) if inside => {
                moved += remap::apply_remap(samples, width, frame.channels, at, map);
                curves += 1;
            }
            MosaicOp::BadPixels(OpcodeList::One, fix) => {
                made += fix_bad_pixels(
                    frame,
                    samples,
                    (width, height),
                    crop,
                    (crop.x, crop.y),
                    true,
                    fix,
                );
            }
            MosaicOp::BadPixels(_, fix) if inside => {
                made += fix_bad_pixels(
                    frame,
                    samples,
                    (width, height),
                    crop,
                    (at.x, at.y),
                    false,
                    fix,
                );
            }
            _ => {}
        }
    }
    log::debug!(
        "DNG mosaic opcodes: {maps} gain map(s), {held} of {} samples held at the white; {curves} sample map(s), {moved} samples moved; {made} bad photosite(s) made again",
        samples.len()
    );
}

/// One bad-pixel opcode on the cropped, normalized mosaic, its list's
/// image placed so the crop's corner is at `at`: the first list's
/// (`stored`) constant against the file's stored values, the second's
/// against the values as they stand, scaled to 0..65535 as the SDK's
/// 16-bit stage holds them. Answers how many were made again; a linear
/// frame has none made, said in the log.
fn fix_bad_pixels(
    frame: &RawFrame,
    samples: &mut [f32],
    (width, height): (usize, usize),
    crop: Rect,
    at: (usize, usize),
    stored: bool,
    fix: &BadPixels,
) -> usize {
    let SensorLayout::Cfa(pattern) = &frame.layout else {
        log::warn!("DNG bad pixels left undone: the frame is not a mosaic");
        return 0;
    };
    let pattern = pattern.shifted(crop.x, crop.y);
    let mut bad = vec![false; width * height];
    match fix {
        BadPixels::List { .. } => bad_pixels::mark_listed(&mut bad, width, height, at.0, at.1, fix),
        BadPixels::Constant { value, .. } => {
            let value = *value as f32;
            let stored_at = |i: usize| -> f32 {
                let (cx, cy) = (crop.x + i % width, crop.y + i / width);
                match &frame.samples {
                    Samples::U16(v) => f32::from(v[cy * frame.width + cx]),
                    Samples::F32(v) => v[cy * frame.width + cx],
                }
            };
            for (i, b) in bad.iter_mut().enumerate() {
                let seen = if stored {
                    stored_at(i)
                } else {
                    (samples[i] * 65535.0).round()
                };
                *b = seen == value;
            }
        }
    }
    let made = bad_pixels::repair(samples, width, height, &pattern, &bad);
    log::debug!(
        "DNG bad pixels (phase {}): {} marked, {made} made again",
        fix.phase(),
        bad.iter().filter(|b| **b).count()
    );
    made
}

/// Subtract black, divide by the range to white, clamp to 0..1.
///
/// Works on the full frame so level patterns index by sensor position.
/// Each pattern position's range runs from its own black to white. A
/// DNG's column and row differences go on the black of each sample, and
/// the range then runs from the highest black that position reaches over
/// the active area. The DNG specification and rawler scale a whole plane
/// by its one highest black; we keep a scale per pattern position, the
/// rule without differences carried over to them.
pub fn normalize_levels(frame: &RawFrame) -> Vec<f32> {
    if let Some(deltas) = &frame.levels.black_deltas {
        return normalize_with_deltas(frame, deltas);
    }
    let cpp = frame.channels;
    let width = frame.width;
    let black = &frame.levels.black;
    let white = &frame.levels.white;
    let mut out = frame.samples.to_f32();
    out.par_chunks_mut(width * cpp)
        .enumerate()
        .for_each(|(y, row)| {
            own_row(row, |row| {
                for (x, px) in row.chunks_exact_mut(cpp).enumerate() {
                    for (ch, s) in px.iter_mut().enumerate() {
                        let b = black.at(y, x, ch);
                        let range = (white.at(y, x, ch) - b).max(f32::EPSILON);
                        *s = ((*s - b) / range).clamp(0.0, 1.0);
                    }
                }
            })
        });
    out
}

/// [`normalize_levels`] for a frame whose black has column and row
/// differences.
fn normalize_with_deltas(frame: &RawFrame, deltas: &BlackDeltas) -> Vec<f32> {
    let cpp = frame.channels;
    let width = frame.width;
    let black = &frame.levels.black;
    let white = &frame.levels.white;
    let columns: Vec<f32> = (0..width).map(|x| deltas.column(x)).collect();
    let column_ceilings = deltas.column_ceilings(black.width);
    let row_ceilings = deltas.row_ceilings(black.height);
    let mut out = frame.samples.to_f32();
    out.par_chunks_mut(width * cpp)
        .enumerate()
        .for_each(|(y, row)| {
            own_row(row, |row| {
                let row_delta = deltas.row(y);
                let row_ceiling = row_ceilings[y % black.height];
                for (x, px) in row.chunks_exact_mut(cpp).enumerate() {
                    let delta = columns[x] + row_delta;
                    let ceiling = column_ceilings[x % black.width] + row_ceiling;
                    for (ch, s) in px.iter_mut().enumerate() {
                        let b = black.at(y, x, ch);
                        let range = (white.at(y, x, ch) - b - ceiling).max(f32::EPSILON);
                        *s = ((*s - b - delta) / range).clamp(0.0, 1.0);
                    }
                }
            })
        });
    out
}

/// Copy `rect` out of an interleaved buffer. Returns the data and its
/// dimensions.
pub fn crop_samples(
    samples: &[f32],
    width: usize,
    cpp: usize,
    rect: Rect,
) -> (Vec<f32>, usize, usize) {
    let mut out = Vec::with_capacity(rect.width * rect.height * cpp);
    for y in rect.y..rect.y + rect.height {
        let start = (y * width + rect.x) * cpp;
        out.extend_from_slice(&samples[start..start + rect.width * cpp]);
    }
    (out, rect.width, rect.height)
}

/// Multiply each CFA sample by the gain for its filter color.
pub fn apply_gains_cfa(samples: &mut [f32], width: usize, pattern: &CfaPattern, gains: [f32; 3]) {
    samples
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            own_row(row, |row| {
                for (x, s) in row.iter_mut().enumerate() {
                    let c = pattern
                        .color_at(y, x)
                        .rgb_index()
                        .expect("RGB pattern checked by caller");
                    *s *= gains[c];
                }
            })
        });
}

/// Multiply interleaved RGB by per-channel gains.
pub fn apply_gains_rgb(samples: &mut [f32], gains: [f32; 3]) {
    samples.par_chunks_mut(3).for_each(|px| {
        for (s, g) in px.iter_mut().zip(gains) {
            *s *= g;
        }
    });
}

/// Clip white-balanced camera samples at `ceiling`.
///
/// A saturated sensor channel carries no information, and after gains it
/// sits above 1.0 while the others do not, which the matrix then turns into
/// magenta. Clipping at 1.0 here, in camera space, sends fully blown pixels
/// to neutral. After reconstruction the ceiling is the brightest channel's
/// saturation instead, since the clipped channels have been filled in.
pub fn clip_highlights(samples: &mut [f32], ceiling: f32) {
    samples.par_iter_mut().for_each(|s| *s = s.min(ceiling));
}

/// Bilinear demosaic: each missing color is the mean of that color's
/// samples in the 3x3 neighborhood. Edges use the neighbors that exist.
///
/// The placeholder demosaic. It is correct, general over any RGB pattern,
/// and soft; a proper algorithm (RCD, AMaZE) replaces it later, tested
/// against this on flat fields where they must agree.
pub fn demosaic_bilinear(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
) -> Vec<f32> {
    let mut out = vec![0.0f32; width * height * 3];
    out.par_chunks_mut(width * 3)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                *px = bilinear_pixel(samples, width, height, pattern, y, x);
            }
        });
    out
}

/// One pixel of [`demosaic_bilinear`], for algorithms that fill their
/// border with it without wanting the whole frame.
pub(crate) fn bilinear_pixel(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    y: usize,
    x: usize,
) -> [f32; 3] {
    let own = pattern
        .color_at(y, x)
        .rgb_index()
        .expect("RGB pattern checked by caller");
    let mut sum = [0.0f32; 3];
    let mut count = [0u32; 3];
    let y0 = y.saturating_sub(1);
    let y1 = (y + 1).min(height - 1);
    let x0 = x.saturating_sub(1);
    let x1 = (x + 1).min(width - 1);
    for ny in y0..=y1 {
        for nx in x0..=x1 {
            if ny == y && nx == x {
                continue;
            }
            let c = pattern
                .color_at(ny, nx)
                .rgb_index()
                .expect("RGB pattern checked by caller");
            sum[c] += samples[ny * width + nx];
            count[c] += 1;
        }
    }
    std::array::from_fn(|c| {
        if c == own {
            samples[y * width + x]
        } else if count[c] > 0 {
            sum[c] / count[c] as f32
        } else {
            wider_mean(samples, width, height, pattern, y, x, c)
        }
    })
}

/// The mean of color `c` in the 5x5 round (`y`, `x`), for a site whose
/// 3x3 holds none of it: never in a Bayer frame of two rows and columns
/// or more, but at a corner of an X-Trans frame, which can be all
/// green. Zero where the 5x5 holds none either.
fn wider_mean(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    y: usize,
    x: usize,
    c: usize,
) -> f32 {
    let mut sum = 0.0f32;
    let mut count = 0u32;
    for ny in y.saturating_sub(2)..=(y + 2).min(height - 1) {
        for nx in x.saturating_sub(2)..=(x + 2).min(width - 1) {
            if pattern.color_at(ny, nx).rgb_index() == Some(c) {
                sum += samples[ny * width + nx];
                count += 1;
            }
        }
    }
    if count > 0 { sum / count as f32 } else { 0.0 }
}

/// `f` run on `row`, with `row` handed across a call.
///
/// A row that comes out of rayon's `enumerate` or `zip` arrives inside
/// a tuple, and a reference loaded from memory carries no promise that
/// it is distinct from anything else: every write to the row then makes
/// the compiler read again whatever the loop reads through the
/// closure's captures, a level pattern's fields or a slice's length,
/// and can keep the loop from vectorizing. As the argument of a
/// function that is not inlined the row is `noalias`, and `f`, inlined
/// into it, is compiled knowing its writes touch nothing else.
///
/// The `#[inline(never)]` is what does the work, at least for the
/// blur: with `#[inline(always)]` the levels and the gains keep their
/// gain, since LLVM turns a `noalias` argument into scope metadata when
/// it inlines, but the blur goes back to what it was without this.
/// Measured with `#[inline(always)]` only (the blur 0.128 to 0.154 s
/// at one thread); plain `#[inline]` was not measured, so leave this.
///
/// Worth it only where a measurement says so; see the notes' section
/// on the tile-write audit (§165).
#[inline(never)]
pub(crate) fn own_row<T, R>(row: &mut [T], f: impl FnOnce(&mut [T]) -> R) -> R {
    f(row)
}

/// Apply a 3x3 matrix given as columns (`cols[c][r]`) to interleaved RGB.
pub fn apply_matrix(rgb: &mut [f32], cols: [[f32; 3]; 3]) {
    rgb.par_chunks_mut(3).for_each(|px| {
        let [r, g, b] = [px[0], px[1], px[2]];
        for (ch, out) in px.iter_mut().enumerate() {
            *out = cols[0][ch] * r + cols[1][ch] * g + cols[2][ch] * b;
        }
    });
}

/// Rotate or flip an image into display orientation.
pub fn orient(image: WorkingImage, orientation: Orientation) -> WorkingImage {
    if orientation == Orientation::Normal {
        return image;
    }
    let (w, h) = (image.width, image.height);
    let swaps = matches!(
        orientation,
        Orientation::Transpose
            | Orientation::Rotate90
            | Orientation::Transverse
            | Orientation::Rotate270
    );
    let (ow, oh) = if swaps { (h, w) } else { (w, h) };
    // Source coordinates for each output pixel.
    let source = |ox: usize, oy: usize| -> (usize, usize) {
        match orientation {
            Orientation::Normal => (ox, oy),
            Orientation::FlipHorizontal => (w - 1 - ox, oy),
            Orientation::Rotate180 => (w - 1 - ox, h - 1 - oy),
            Orientation::FlipVertical => (ox, h - 1 - oy),
            Orientation::Transpose => (oy, ox),
            Orientation::Rotate90 => (oy, h - 1 - ox),
            Orientation::Transverse => (w - 1 - oy, h - 1 - ox),
            Orientation::Rotate270 => (w - 1 - oy, ox),
        }
    };
    let mut out = WorkingImage::new(ow, oh);
    out.data
        .par_chunks_mut(ow * 3)
        .enumerate()
        .for_each(|(oy, row)| {
            for ox in 0..ow {
                let (sx, sy) = source(ox, oy);
                let src = &image.data[(sy * w + sx) * 3..(sy * w + sx) * 3 + 3];
                row[ox * 3..ox * 3 + 3].copy_from_slice(src);
            }
        });
    out
}

#[cfg(test)]
mod timing;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::color::tests::rec2020_camera_matrix;
    use crate::raw::{Calibration, CfaColor, LevelPattern, Levels, Samples};

    /// A 4x4 RGGB frame: black 100, white 1100, sensor values chosen so the
    /// normalized red is 0.5, green 1.0 and blue 0.25 everywhere. The camera
    /// is Rec.2020 itself and the as-shot gains (2, 1, 4) balance it.
    pub(crate) fn flat_frame() -> RawFrame {
        let pattern = CfaPattern::rggb();
        let mut samples = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                samples.push(match pattern.color_at(y, x) {
                    CfaColor::Red => 600u16,
                    CfaColor::Green => 1100,
                    CfaColor::Blue => 350,
                    CfaColor::Other(_) => unreachable!(),
                });
            }
        }
        RawFrame {
            make: "Test".into(),
            model: "Cam".into(),
            other_names: Vec::new(),
            width: 4,
            height: 4,
            channels: 1,
            layout: SensorLayout::Cfa(pattern),
            samples: Samples::U16(samples),
            levels: Levels {
                black: LevelPattern::uniform(100.0, 1),
                white: LevelPattern::uniform(1100.0, 1),
                black_deltas: None,
            },
            as_shot_coefficients: Some([2.0, 1.0, 4.0]),
            calibrations: vec![Calibration {
                illuminant: 21,
                color_matrix: rec2020_camera_matrix(),
                forward_matrix: None,
            }],
            crop: None,
            orientation: Orientation::Normal,
            shot: Default::default(),
            dng: Default::default(),
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// A monochrome sensor's frame as its DNG decodes: one channel,
    /// black 100, white 1100, no calibration, its photosites `at`.
    pub(crate) fn mono_frame(
        width: usize,
        height: usize,
        at: impl Fn(usize, usize) -> u16,
    ) -> RawFrame {
        let at = &at;
        RawFrame {
            make: "Test".into(),
            model: "Mono".into(),
            other_names: Vec::new(),
            width,
            height,
            channels: 1,
            layout: SensorLayout::Monochrome,
            samples: Samples::U16(
                (0..height)
                    .flat_map(|y| (0..width).map(move |x| at(y, x)))
                    .collect(),
            ),
            levels: Levels {
                black: LevelPattern::uniform(100.0, 1),
                white: LevelPattern::uniform(1100.0, 1),
                black_deltas: None,
            },
            as_shot_coefficients: Some([1.0; 3]),
            calibrations: Vec::new(),
            crop: None,
            orientation: Orientation::Normal,
            shot: Default::default(),
            dng: Default::default(),
        }
    }

    /// A monochrome frame develops to a neutral grey: three equal
    /// channels, each its photosite's normalized level, at the neutral
    /// white balance its as-shot is.
    #[test]
    fn a_monochrome_frame_develops_to_its_levels_in_grey() {
        let frame = mono_frame(8, 6, |y, x| 100 + ((y * 8 + x) * 17 % 1000) as u16);
        let developed = develop(&frame, &DevelopSettings::default()).unwrap();
        assert_eq!(developed.white_balance.coefficients_f32(), [1.0; 3]);
        assert!((developed.white_balance.temp_tint.cct - 6504.0).abs() < 10.0);
        assert_eq!(developed.demosaic, None);
        let levels = normalize_levels(&frame);
        for (px, want) in developed.image.pixels().zip(levels) {
            for v in px {
                assert!((v - want).abs() < 1e-6, "{px:?} for {want}");
            }
        }
        // The reduced develop has nothing to bin: the full one.
        let reduced =
            reduced::develop_reduced_by(&frame, &DevelopSettings::default(), None, 2).unwrap();
        assert_eq!(reduced.bin, 1);
        assert_eq!(reduced.developed.image.data, developed.image.data);
        // The camera space develop is the same plane, three times.
        let camera = demosaic(&frame, &DevelopSettings::default()).unwrap();
        for (a, b) in camera.image.data.iter().zip(&developed.image.data) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    /// No white tints a monochrome frame: a 3000 K light with a tint,
    /// or a color camera's profile, and it develops grey at its levels,
    /// as at As shot. A color frame under the same white still moves.
    #[test]
    fn no_white_tints_a_monochrome_frame() {
        let frame = mono_frame(8, 6, |y, x| 150 + ((y * 8 + x) * 23 % 900) as u16);
        let warm = WhitePoint::TempTint(rawcolor::TempTint {
            cct: 3000.0,
            duv: 0.01,
        });
        let shot = develop(&frame, &DevelopSettings::default()).unwrap();
        let color_profile =
            std::sync::Arc::new(crate::color::Profile::from_frame(&flat_frame()).unwrap());
        for settings in [
            DevelopSettings {
                white_point: warm,
                ..Default::default()
            },
            DevelopSettings {
                white_point: WhitePoint::Coefficients([2.0, 1.0, 1.5]),
                profile: Some(color_profile.clone()),
                ..Default::default()
            },
        ] {
            let got = develop(&frame, &settings).unwrap();
            assert_eq!(got.white_balance.coefficients_f32(), [1.0; 3]);
            assert_eq!(got.image.data, shot.image.data);
        }
        let color = flat_frame();
        let at_shot = develop(&color, &DevelopSettings::default()).unwrap();
        let at_warm = develop(
            &color,
            &DevelopSettings {
                white_point: warm,
                ..Default::default()
            },
        )
        .unwrap();
        assert_ne!(at_warm.image.data, at_shot.image.data);
    }

    /// Its noise is measured on the plane, the same in each channel at
    /// a photosite's variance; the denoise takes it down and keeps the
    /// channels equal. A hot photosite is repaired against it.
    #[test]
    fn a_monochrome_frame_is_measured_denoised_and_repaired() {
        let (w, h, level, sigma) = (128, 128, 0.2f32, 0.01f32);
        let mut g = noise::tests::Gauss(11);
        let noisy: Vec<u16> = (0..w * h)
            .map(|_| (100.0 + 1000.0 * (level + sigma * g.next())).round() as u16)
            .collect();
        let mut frame = mono_frame(w, h, |y, x| noisy[y * w + x]);
        let Samples::U16(v) = &mut frame.samples else {
            unreachable!()
        };
        v[60 * w + 60] = 1100;

        let prepared = prepare(&frame, &DevelopSettings::default(), true).unwrap();
        let model = prepared.noise.unwrap().model;
        let photosite = sigma * sigma;
        for c in 0..3 {
            let got = model.variance(c, level);
            assert!(
                (got / photosite - 1.0).abs() < 0.2,
                "channel {c}: {got} for {photosite}"
            );
        }
        assert_eq!(model.a[0], model.a[1]);
        assert_eq!(prepared.hot_pixels.unwrap().hot, 1);
        let at = (60 * w + 60) * 3;
        assert!(prepared.samples[at] < level + 6.0 * sigma);

        let plain = develop(&frame, &DevelopSettings::default()).unwrap();
        let denoised = develop(
            &frame,
            &DevelopSettings {
                denoise: Some(denoise::DenoiseOptions::default()),
                ..Default::default()
            },
        )
        .unwrap();
        let spread = |image: &WorkingImage| {
            let n = (image.width * image.height) as f32;
            let mean = image.pixels().map(|p| p[1]).sum::<f32>() / n;
            (image.pixels().map(|p| (p[1] - mean).powi(2)).sum::<f32>() / n).sqrt()
        };
        assert!(denoised.denoise.is_some());
        assert!(
            spread(&denoised.image) < 0.5 * spread(&plain.image),
            "{} of {}",
            spread(&denoised.image),
            spread(&plain.image)
        );
        for px in denoised.image.pixels() {
            assert!((px[0] - px[1]).abs() < 1e-6 && (px[2] - px[1]).abs() < 1e-6);
        }
    }

    #[test]
    fn levels_normalize_per_position() {
        let frame = flat_frame();
        let n = normalize_levels(&frame);
        assert!(close(n[0], 0.5), "red {}", n[0]);
        assert!(close(n[1], 1.0), "green {}", n[1]);
        assert!(close(n[5], 0.25), "blue {}", n[5]);

        // A per-cell black pattern is honored.
        let mut frame = flat_frame();
        frame.levels.black = LevelPattern::new(2, 2, 1, vec![100.0, 100.0, 100.0, 300.0]).unwrap();
        let n = normalize_levels(&frame);
        assert!(
            close(n[5], 50.0 / 800.0),
            "blue with its own black {}",
            n[5]
        );
    }

    /// A frame of 9x7 samples counting up, with a 2x2 black pattern and
    /// differences along an active area that starts at (1, 2).
    fn delta_frame() -> RawFrame {
        let (width, height) = (9, 7);
        let mut frame = flat_frame();
        frame.width = width;
        frame.height = height;
        frame.samples = Samples::U16((0..width * height).map(|i| 400 + 7 * i as u16).collect());
        frame.levels.black = LevelPattern::new(2, 2, 1, vec![100.0, 110.0, 120.0, 130.0]).unwrap();
        frame.levels.black_deltas = Some(BlackDeltas {
            area: Rect {
                x: 1,
                y: 2,
                width: 7,
                height: 4,
            },
            columns: vec![1.5, -2.0, 3.0, 0.5, -1.0, 4.0, 2.5],
            rows: vec![-3.0, 6.0, 0.0, 2.0],
        });
        frame
    }

    #[test]
    fn black_deltas_go_on_each_sample_and_the_range_starts_at_their_highest() {
        let frame = delta_frame();
        let n = normalize_levels(&frame);
        let deltas = frame.levels.black_deltas.as_ref().unwrap();
        let Samples::U16(raw) = &frame.samples else {
            unreachable!()
        };
        // Per sample, written out: the black is the pattern's plus the
        // column's and the row's difference (zero outside the area); the
        // range runs from the highest black that pattern position takes
        // anywhere in the area.
        for y in 0..frame.height {
            for x in 0..frame.width {
                let column = if (1..8).contains(&x) {
                    deltas.columns[x - 1]
                } else {
                    0.0
                };
                let row = if (2..6).contains(&y) {
                    deltas.rows[y - 2]
                } else {
                    0.0
                };
                let pattern = frame.levels.black.at(y, x, 0);
                let highest_column = (1..8)
                    .filter(|c| c % 2 == x % 2)
                    .map(|c| deltas.columns[c - 1])
                    .fold(f32::MIN, f32::max);
                let highest_row = (2..6)
                    .filter(|r| r % 2 == y % 2)
                    .map(|r| deltas.rows[r - 2])
                    .fold(f32::MIN, f32::max);
                let black = pattern + column + row;
                assert_eq!(frame.levels.black_at(y, x, 0), black);
                let range = 1100.0 - pattern - highest_column - highest_row;
                let want = ((f32::from(raw[y * frame.width + x]) - black) / range).clamp(0.0, 1.0);
                let got = n[y * frame.width + x];
                assert!(close(got, want), "({x}, {y}): {got} != {want}");
            }
        }
    }

    #[test]
    fn zero_black_deltas_normalize_as_none() {
        let mut frame = delta_frame();
        let none = {
            let mut f = frame.clone();
            f.levels.black_deltas = None;
            normalize_levels(&f)
        };
        let d = frame.levels.black_deltas.as_mut().unwrap();
        d.columns
            .iter_mut()
            .chain(d.rows.iter_mut())
            .for_each(|v| *v = 0.0);
        assert_eq!(normalize_levels(&frame), none);
    }

    /// With a black of one value, the highest black of each position is
    /// the plane's, so the scale is the DNG specification's exactly:
    /// rawler's own mapping is the reference.
    #[test]
    fn black_deltas_map_as_rawler_maps_them_under_one_black() {
        use ::rawler::formats::tiff::SRational;
        use ::rawler::imgop::{Dim2, Point};
        use ::rawler::rawimage::{BlackLevel, WhiteLevel};
        let mut frame = delta_frame();
        frame.levels.black = LevelPattern::uniform(100.0, 1);
        let deltas = frame.levels.black_deltas.clone().unwrap();
        let rationals = |v: &[f32]| {
            v.iter()
                .map(|d| SRational::new((d * 2.0) as i32, 2))
                .collect::<Vec<_>>()
        };
        let black = BlackLevel::new(&[100u16], 1, 1, 1)
            .with_deltas(rationals(&deltas.columns), rationals(&deltas.rows));
        let area = ::rawler::imgop::Rect::new(
            Point::new(deltas.area.x, deltas.area.y),
            Dim2::new(deltas.area.width, deltas.area.height),
        );
        let Samples::U16(raw) = &frame.samples else {
            unreachable!()
        };
        let pixels = ::rawler::pixarray::PixU16::new_with(raw.clone(), frame.width, frame.height);
        let want = ::rawler::imgop::blacklevel::rescale(
            &pixels,
            &black,
            &WhiteLevel::new(vec![1100]),
            area,
            None,
        )
        .unwrap();
        let got = normalize_levels(&frame);
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            assert!(close(*g, w.min(1.0)), "sample {i}: {g} != {w}");
        }
    }

    #[test]
    fn a_dngs_gain_maps_go_on_the_mosaic() {
        let settings = DevelopSettings {
            hot_pixels: None,
            chromatic_aberration: None,
            ..Default::default()
        };
        let plain = demosaic(&flat_frame(), &settings).unwrap();
        let mut frame = flat_frame();
        frame.dng.mosaic = vec![crate::opcodes::MosaicOp::GainMap(crate::opcodes::even_map(
            4,
            4,
            2,
            2,
            |_, _| 0.5,
        ))];
        let halved = demosaic(&frame, &settings).unwrap();
        for (a, b) in plain.image.data.iter().zip(&halved.image.data) {
            assert!(close(*a * 0.5, *b), "{a} halved is not {b}");
        }
        assert!(plain.image.data.iter().any(|v| *v > 0.1));
    }

    /// A DNG's sample maps go on the linear samples: a polynomial of
    /// half the sample develops as the frame at half its gain.
    #[test]
    fn a_dngs_sample_maps_go_on_the_mosaic() {
        use crate::opcodes::{Curve, MosaicOp, whole_remap};
        let settings = DevelopSettings {
            hot_pixels: None,
            chromatic_aberration: None,
            ..Default::default()
        };
        let plain = demosaic(&flat_frame(), &settings).unwrap();
        let mut frame = flat_frame();
        frame.dng.mosaic = vec![MosaicOp::Remap(whole_remap(
            0,
            1,
            Curve::Polynomial(vec![0.0, 0.5]),
        ))];
        let halved = demosaic(&frame, &settings).unwrap();
        for (a, b) in plain.image.data.iter().zip(&halved.image.data) {
            assert!(close(*a * 0.5, *b), "{a} halved is not {b}");
        }
        assert!(plain.image.data.iter().any(|v| *v > 0.1));
    }

    /// A linear frame's three sample maps reach their own planes: a
    /// Lightroom merge's case, one polynomial a plane, the second
    /// list's coordinates placed by the crop. A crop that starts
    /// before the list's area leaves them, said in the log.
    #[test]
    fn a_linear_frames_sample_maps_reach_their_planes() {
        use crate::opcodes::{Curve, MosaicOp, Remap};
        let settings = DevelopSettings {
            hot_pixels: None,
            chromatic_aberration: None,
            ..Default::default()
        };
        let mut frame = flat_frame();
        frame.layout = SensorLayout::Linear;
        frame.channels = 3;
        frame.levels = Levels {
            black: LevelPattern::uniform(0.0, 3),
            white: LevelPattern::uniform(1000.0, 3),
            black_deltas: None,
        };
        frame.samples = Samples::U16(vec![500; 4 * 4 * 3]);
        let plain = demosaic(&frame, &settings).unwrap();
        // Each plane its own curve, over the rows and columns from
        // (1, 1) of the list's image, which the crop starts at.
        let curve = |plane: u32, k: f64| {
            MosaicOp::Remap(Remap {
                top: 1,
                left: 1,
                bottom: u32::MAX,
                right: u32::MAX,
                plane,
                planes: 1,
                row_pitch: 1,
                col_pitch: 1,
                curve: Curve::Polynomial(vec![0.0, k]),
            })
        };
        frame.dng.mosaic = vec![curve(0, 0.5), curve(1, 1.0), curve(2, 2.0)];
        frame.dng.opcode_area = Some(Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        });
        frame.crop = Some(Rect {
            x: 1,
            y: 1,
            width: 3,
            height: 3,
        });
        let mapped = demosaic(&frame, &settings).unwrap();
        assert_eq!(mapped.image.width, 3);
        let plain_px = &plain.image.data[..3];
        for px in mapped.image.data.chunks(3) {
            for (c, (m, p)) in px.iter().zip(plain_px).enumerate() {
                let k = [0.5, 1.0, 2.0][c];
                assert!(close(*m, (p * k).min(1.0)), "plane {c}: {m} != {p} * {k}");
            }
        }
        // The crop starts before the list's area: nothing applied.
        frame.dng.opcode_area = Some(Rect {
            x: 2,
            y: 2,
            width: 2,
            height: 2,
        });
        let left = demosaic(&frame, &settings).unwrap();
        for px in left.image.data.chunks(3) {
            for (m, p) in px.iter().zip(plain_px) {
                assert!(close(*m, *p), "{m} != {p}");
            }
        }
    }

    /// A DNG's bad photosites are made again on the mosaic: a listed
    /// blue stuck at the white and a red at the list's constant develop
    /// as the frame with none stuck. The second list's points are in
    /// the active area's coordinates.
    #[test]
    fn a_dngs_bad_pixels_are_made_again() {
        use crate::opcodes::{BadPixels, MosaicOp, OpcodeList};
        let settings = DevelopSettings {
            hot_pixels: None,
            chromatic_aberration: None,
            ..Default::default()
        };
        let plain = demosaic(&flat_frame(), &settings).unwrap();
        let mut frame = flat_frame();
        let Samples::U16(samples) = &mut frame.samples else {
            unreachable!()
        };
        samples[5] = 1100;
        samples[10] = 1099;
        let listed = |row, col| BadPixels::List {
            phase: 0,
            points: vec![[row, col]],
            rects: vec![],
        };
        frame.dng.mosaic = vec![MosaicOp::BadPixels(OpcodeList::One, listed(1, 1))];
        let one = demosaic(&frame, &settings).unwrap();
        assert_ne!(plain.image.data, one.image.data);
        frame.dng.mosaic.push(MosaicOp::BadPixels(
            OpcodeList::One,
            BadPixels::Constant {
                value: 1099,
                phase: 0,
            },
        ));
        let both = demosaic(&frame, &settings).unwrap();
        for (a, b) in plain.image.data.iter().zip(&both.image.data) {
            assert!(close(*a, *b), "{a} is not {b}");
        }

        // The same red listed in the second list, its area the right
        // half of the frame: the red at sensor row 2, column 2 is at
        // the area's column 0.
        frame.dng.opcode_area = Some(Rect {
            x: 2,
            y: 0,
            width: 2,
            height: 4,
        });
        frame.crop = frame.dng.opcode_area;
        frame.dng.mosaic = vec![MosaicOp::BadPixels(OpcodeList::Two, listed(2, 0))];
        let mut clean = flat_frame();
        clean.crop = frame.crop;
        let plain = demosaic(&clean, &settings).unwrap();
        let area = demosaic(&frame, &settings).unwrap();
        for (a, b) in plain.image.data.iter().zip(&area.image.data) {
            assert!(close(*a, *b), "{a} is not {b}");
        }
    }

    /// The second list runs in its file order: a constant before a gain
    /// map is tested on the values before the gain, so a stuck red at
    /// 0.8 is found by its 0.8 and made again before the map halves it.
    #[test]
    fn the_second_list_runs_in_its_order() {
        use crate::opcodes::{BadPixels, MosaicOp, OpcodeList, even_map};
        let settings = DevelopSettings {
            hot_pixels: None,
            chromatic_aberration: None,
            ..Default::default()
        };
        let half = MosaicOp::GainMap(even_map(4, 4, 2, 2, |_, _| 0.5));
        let mut clean = flat_frame();
        clean.dng.mosaic = vec![half.clone()];
        let plain = demosaic(&clean, &settings).unwrap();
        let mut frame = flat_frame();
        let Samples::U16(samples) = &mut frame.samples else {
            unreachable!()
        };
        samples[10] = 900; // (900 - 100) / 1000 = 0.8
        let constant = MosaicOp::BadPixels(
            OpcodeList::Two,
            BadPixels::Constant {
                value: (0.8f32 * 65535.0).round() as u32,
                phase: 0,
            },
        );
        frame.dng.mosaic = vec![constant.clone(), half.clone()];
        let ordered = demosaic(&frame, &settings).unwrap();
        for (a, b) in plain.image.data.iter().zip(&ordered.image.data) {
            assert!(close(*a, *b), "{a} is not {b}");
        }
        // After the map the stuck red is at 0.4 and the constant finds
        // nothing.
        frame.dng.mosaic = vec![half, constant];
        let after = demosaic(&frame, &settings).unwrap();
        assert_ne!(plain.image.data, after.image.data);
    }

    #[test]
    fn bilinear_on_a_flat_field_is_flat() {
        let frame = flat_frame();
        let n = normalize_levels(&frame);
        let rgb = demosaic_bilinear(&n, 4, 4, &CfaPattern::rggb());
        for px in rgb.as_chunks::<3>().0 {
            assert!(
                close(px[0], 0.5) && close(px[1], 1.0) && close(px[2], 0.25),
                "{px:?}"
            );
        }
    }

    #[test]
    fn crop_shifts_the_pattern_with_it() {
        let frame = flat_frame();
        let n = normalize_levels(&frame);
        let rect = Rect {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        let (c, w, h) = crop_samples(&n, 4, 1, rect);
        assert_eq!((w, h), (2, 2));
        let pattern = CfaPattern::rggb().shifted(1, 1);
        assert_eq!(pattern.color_at(0, 0), CfaColor::Blue);
        let rgb = demosaic_bilinear(&c, w, h, &pattern);
        for px in rgb.as_chunks::<3>().0 {
            assert!(
                close(px[0], 0.5) && close(px[1], 1.0) && close(px[2], 0.25),
                "{px:?}"
            );
        }
    }

    #[test]
    fn matrix_applies_by_columns() {
        let mut rgb = vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let cols = [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]];
        apply_matrix(&mut rgb, cols);
        assert_eq!(rgb, vec![0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn orientation_rotates_clockwise_for_rotate90() {
        // 2 wide, 1 tall: [A B]
        let img = WorkingImage::from_data(2, 1, vec![1.0, 1.0, 1.0, 2.0, 2.0, 2.0]).unwrap();
        let r = orient(img.clone(), Orientation::Rotate90);
        assert_eq!((r.width, r.height), (1, 2));
        assert_eq!(
            r.pixel(0, 0)[0],
            1.0,
            "A ends up on top after a clockwise turn"
        );
        assert_eq!(r.pixel(0, 1)[0], 2.0);
        let r = orient(img.clone(), Orientation::Rotate270);
        assert_eq!(
            r.pixel(0, 0)[0],
            2.0,
            "B ends up on top after a counter-clockwise turn"
        );
        let r = orient(img.clone(), Orientation::Rotate180);
        assert_eq!(r.pixel(0, 0)[0], 2.0);
        let r = orient(img.clone(), Orientation::FlipHorizontal);
        assert_eq!(r.pixel(0, 0)[0], 2.0);
        let t = orient(img, Orientation::Transpose);
        assert_eq!((t.width, t.height), (1, 2));
        assert_eq!(t.pixel(0, 1)[0], 2.0);
    }

    /// A quarter turn on top of the tag turns the picture exactly as
    /// the tag that already means that would have: `Orientation::turned`
    /// against `orient` itself, for every tag and every turn.
    #[test]
    fn a_turn_on_top_of_the_tag_is_the_tag_that_means_it() {
        // 3 x 2, every pixel its own value, so nothing but the right
        // mapping can pass.
        let data: Vec<f32> = (0..6).flat_map(|i| [i as f32, 0.0, 0.0]).collect();
        let img = WorkingImage::from_data(3, 2, data).unwrap();
        for tag in (1..=8u16).map(|c| Orientation::from_exif(c).unwrap()) {
            let shown = orient(img.clone(), tag);
            for q in 0..4i32 {
                let turn = Orientation::from_exif(match q.rem_euclid(4) {
                    0 => 1,
                    1 => 6,
                    2 => 3,
                    _ => 8,
                })
                .unwrap();
                let twice = orient(shown.clone(), turn);
                let once = orient(img.clone(), tag.turned(q));
                assert_eq!(
                    (twice.width, twice.height),
                    (once.width, once.height),
                    "{tag:?} + {q}"
                );
                assert_eq!(twice.data, once.data, "{tag:?} + {q}");
            }
        }
    }

    /// The editor's turn: a develop with the orientation override is
    /// the develop of the same frame with the camera's tag moved on
    /// by hand.
    #[test]
    fn the_orientation_override_develops_as_a_frame_with_that_tag_would() {
        for tag in (1..=8u16).map(|c| Orientation::from_exif(c).unwrap()) {
            for q in 0..4i32 {
                let mut frame = flat_frame();
                frame.orientation = tag;
                let turned = develop(
                    &frame,
                    &DevelopSettings {
                        orientation: Some(tag.turned(q)),
                        ..Default::default()
                    },
                )
                .unwrap();
                let mut by_hand = flat_frame();
                by_hand.orientation = tag.turned(q);
                let plain = develop(&by_hand, &DevelopSettings::default()).unwrap();
                assert_eq!(
                    (turned.image.width, turned.image.height),
                    (plain.image.width, plain.image.height),
                    "{tag:?} + {q}"
                );
                assert_eq!(turned.image.data, plain.image.data, "{tag:?} + {q}");
            }
        }
    }

    #[test]
    fn develop_balances_and_lands_in_the_working_space() {
        // The camera is Rec.2020 itself, so with as-shot gains (2, 1, 4) the
        // flat field balances to (1, 1, 1) and the matrix is the identity.
        let frame = flat_frame();
        let out = develop(&frame, &DevelopSettings::default()).unwrap();
        assert_eq!((out.image.width, out.image.height), (4, 4));
        for px in out.image.pixels() {
            for (i, v) in px.iter().enumerate() {
                assert!((v - 1.0).abs() < 2e-2, "channel {i} = {v}");
            }
        }
        let g = out.white_balance.coefficients_f32();
        assert!(
            close(g[0], 2.0) && close(g[1], 1.0) && close(g[2], 4.0),
            "{g:?}"
        );
    }

    #[test]
    fn demosaic_returns_camera_native_values() {
        let frame = flat_frame();
        let out = demosaic(&frame, &DevelopSettings::default()).unwrap();
        assert_eq!((out.image.width, out.image.height), (4, 4));
        for px in out.image.pixels() {
            assert!(
                close(px[0], 0.5) && close(px[1], 1.0) && close(px[2], 0.25),
                "{px:?}"
            );
        }
        // Saturated samples stay at 1.0 in their own channel rather than at
        // 1/gain: the clipping fact survives for the consumer.
        let mut frame = flat_frame();
        frame.samples = Samples::U16(vec![1100; 16]);
        let out = demosaic(&frame, &DevelopSettings::default()).unwrap();
        for px in out.image.pixels() {
            assert!(px.iter().all(|v| close(*v, 1.0)), "{px:?}");
        }
    }

    /// A mosaic of neither layout goes no further than the door, and
    /// what it is refused with is a sentence, not a pattern dump.
    #[test]
    fn a_mosaic_of_neither_layout_is_refused_in_words() {
        use CfaColor::{Blue as B, Green as G, Red as R};
        let pattern = CfaPattern::new(3, 3, vec![G, R, G, B, G, B, G, R, G]).unwrap();
        let mut frame = flat_frame();
        frame.width = 12;
        frame.height = 12;
        frame.samples = Samples::U16(vec![600; 144]);
        frame.layout = SensorLayout::Cfa(pattern.clone());
        for method in DemosaicMethod::ALL {
            let settings = DevelopSettings {
                demosaic: *method,
                ..Default::default()
            };
            let complaint = develop(&frame, &settings).unwrap_err().to_string();
            assert!(
                complaint.contains("3x3 GRGBGBGRG"),
                "{method:?}: {complaint}"
            );
            assert!(
                complaint.contains("does not develop"),
                "{method:?}: {complaint}"
            );
            assert!(!complaint.contains("CfaColor"), "{method:?}: {complaint}");
        }
        // The refusal comes from the door, not from the demosaic:
        // `prepare` never reaches one, so its failing is the proof
        // that nothing was normalized, cropped or reconstructed first.
        let early = prepare(&frame, &DevelopSettings::default(), true)
            .unwrap_err()
            .to_string();
        assert_eq!(
            early,
            "unsupported: this build does not develop a 3x3 GRGBGBGRG mosaic; \
             it demosaics 2x2 Bayer and 6x6 X-Trans"
        );
        // The demosaic keeps the same words for a consumer that comes
        // straight to it.
        let samples = vec![0.5f32; 144];
        let late = demosaic_cfa(&samples, 12, 12, &pattern, DemosaicMethod::Bilinear)
            .unwrap_err()
            .to_string();
        assert_eq!(late, early);
        // And the sites that say "2x2 Bayer" name an X-Trans pattern the
        // short way when they are reached on their own.
        let complaint = rcd::demosaic_rcd(&samples, 12, 12, &CfaPattern::xtrans())
            .unwrap_err()
            .to_string();
        assert_eq!(
            complaint,
            "unsupported: RCD needs a 2x2 Bayer pattern, got 6x6 (X-Trans)"
        );
    }

    /// An X-Trans frame, cropped off its pattern's phase, under the
    /// default settings and every method.
    pub(crate) fn xtrans_frame() -> RawFrame {
        let pattern = CfaPattern::xtrans();
        let (w, h) = (80, 70);
        let samples = (0..h)
            .flat_map(|y| (0..w).map(move |x| (y, x)))
            .map(|(y, x)| match pattern.color_at(y, x) {
                CfaColor::Red => 350u16,
                CfaColor::Green => 600,
                CfaColor::Blue => 225,
                CfaColor::Other(_) => unreachable!(),
            })
            .collect();
        RawFrame {
            width: w,
            height: h,
            layout: SensorLayout::Cfa(pattern),
            samples: Samples::U16(samples),
            crop: Some(Rect {
                x: 5,
                y: 7,
                width: 70,
                height: 60,
            }),
            ..flat_frame()
        }
    }

    /// Each mosaic lists its own methods, its default first, and every
    /// method of the engine belongs to one of them.
    #[test]
    fn each_mosaic_lists_its_own_methods() {
        let bayer = CfaPattern::rggb();
        let xtrans = CfaPattern::xtrans().shifted(2, 5);
        assert_eq!(
            DemosaicMethod::default_for(&bayer),
            DemosaicMethod::AmazeVng4
        );
        assert_eq!(
            DemosaicMethod::default_for(&xtrans),
            DemosaicMethod::Markesteijn3
        );
        assert_eq!(
            DemosaicMethod::for_pattern(&xtrans),
            &[
                DemosaicMethod::Markesteijn3,
                DemosaicMethod::Markesteijn1,
                DemosaicMethod::Bilinear
            ]
        );
        assert_eq!(DemosaicMethod::for_pattern(&bayer).len(), 6);
        for method in DemosaicMethod::ALL {
            assert!(method.takes(&bayer) || method.takes(&xtrans), "{method:?}");
            assert_eq!(method.name().parse::<DemosaicMethod>().unwrap(), *method);
        }
        use CfaColor::{Blue as B, Green as G, Red as R};
        let neither = CfaPattern::new(3, 3, vec![G, R, G, B, G, B, G, R, G]).unwrap();
        assert!(DemosaicMethod::for_pattern(&neither).is_empty());
    }

    /// A method of the other mosaic resolves to the mosaic's default; a
    /// method the mosaic takes is kept.
    #[test]
    fn a_method_of_the_other_mosaic_resolves_to_the_default() {
        let bayer = CfaPattern::rggb().shifted(1, 0);
        let xtrans = CfaPattern::xtrans();
        use DemosaicMethod::*;
        assert_eq!(Amaze.for_mosaic(&xtrans), Markesteijn3);
        assert_eq!(AmazeVng4.for_mosaic(&xtrans), Markesteijn3);
        assert_eq!(Markesteijn1.for_mosaic(&bayer), AmazeVng4);
        assert_eq!(Markesteijn3.for_mosaic(&bayer), AmazeVng4);
        assert_eq!(Markesteijn1.for_mosaic(&xtrans), Markesteijn1);
        assert_eq!(Rcd.for_mosaic(&bayer), Rcd);
        assert_eq!(Bilinear.for_mosaic(&xtrans), Bilinear);
        assert_eq!(Bilinear.for_mosaic(&bayer), Bilinear);
    }

    /// The demosaic itself refuses a method of the other mosaic, in
    /// words, both ways; bilinear takes both.
    #[test]
    fn a_method_of_the_other_mosaic_is_refused_by_the_demosaic() {
        let samples = vec![0.5f32; 48 * 48];
        let xtrans = CfaPattern::xtrans();
        let bayer = CfaPattern::rggb();
        let err = demosaic_cfa(&samples, 48, 48, &xtrans, DemosaicMethod::Amaze)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "unsupported: amaze is a Bayer demosaic and this is a 6x6 (X-Trans) mosaic"
        );
        let err = demosaic_cfa(&samples, 48, 48, &bayer, DemosaicMethod::Markesteijn3)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "unsupported: markesteijn-3 is an X-Trans demosaic and this is a 2x2 RGGB mosaic"
        );
        for pattern in [&xtrans, &bayer] {
            assert!(demosaic_cfa(&samples, 48, 48, pattern, DemosaicMethod::Bilinear).is_ok());
            let default = DemosaicMethod::default_for(pattern);
            assert!(demosaic_cfa(&samples, 48, 48, pattern, default).is_ok());
        }
    }

    /// The profiled denoise on X-Trans: the noise is measured on the
    /// mosaic, the denoise after the demosaic reads that model, and the
    /// develop comes out quieter for it.
    #[test]
    fn an_xtrans_frame_is_measured_and_denoised() {
        let pattern = CfaPattern::xtrans().shifted(2, 3);
        let (w, h) = (192, 160);
        let level = [0.25f32, 0.4, 0.2];
        let sigma = 0.01f32;
        let mut g = noise::tests::Gauss(3);
        let mut samples = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let c = pattern.color_at(y, x).rgb_index().unwrap();
                let v = (level[c] + sigma * g.next()).clamp(0.0, 1.0);
                samples.push((v * 16383.0).round() as u16);
            }
        }
        let frame = RawFrame {
            width: w,
            height: h,
            layout: SensorLayout::Cfa(pattern),
            samples: Samples::U16(samples),
            levels: Levels {
                black: LevelPattern::uniform(0.0, 1),
                white: LevelPattern::uniform(16383.0, 1),
                black_deltas: None,
            },
            crop: None,
            ..flat_frame()
        };
        let spread = |d: &Developed| {
            let px: Vec<[f32; 3]> = d.image.pixels().copied().collect();
            let inner: Vec<f32> = (0..px.len())
                .filter(|i| (8..h - 8).contains(&(i / w)) && (8..w - 8).contains(&(i % w)))
                .map(|i| px[i][1])
                .collect();
            let mean = inner.iter().sum::<f32>() / inner.len() as f32;
            (inner.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / inner.len() as f32).sqrt()
        };
        let plain = develop(&frame, &DevelopSettings::default()).unwrap();
        let denoised = develop(
            &frame,
            &DevelopSettings {
                denoise: Some(denoise::DenoiseOptions::default()),
                ..Default::default()
            },
        )
        .unwrap();
        let estimate = denoised.noise.expect("the noise was measured");
        for (c, &at) in level.iter().enumerate() {
            let got = estimate.model.sigma(c, at);
            assert!((got / sigma - 1.0).abs() < 0.2, "channel {c}: {got}");
        }
        assert!(denoised.denoise.is_some_and(|s| s.scales > 0));
        let (before, after) = (spread(&plain), spread(&denoised));
        assert!(after < 0.5 * before, "{before} -> {after}");
    }

    /// Every stage before the demosaic runs on X-Trans, the demosaic is
    /// Markesteijn's or bilinear, and a flat grey comes out flat and
    /// grey whatever the method. The frame is flat and clean, so no
    /// stage finds anything to do.
    #[test]
    fn an_xtrans_frame_develops_under_every_method() {
        let frame = xtrans_frame();
        for method in DemosaicMethod::ALL {
            let settings = DevelopSettings {
                demosaic: *method,
                denoise: Some(denoise::DenoiseOptions::default()),
                ..Default::default()
            };
            let d = develop(&frame, &settings).unwrap();
            assert_eq!((d.image.width, d.image.height), (70, 60));
            let ca = d.chromatic_aberration.expect("the CA correction ran");
            assert!(!ca.corrected, "{method:?}: {ca:?}");
            assert_eq!(
                d.hot_pixels,
                Some(hotpixels::HotPixelStats::default()),
                "{method:?}"
            );
            assert!(
                d.noise.is_some_and(|n| n.model.is_silent()),
                "{method:?}: {:?}",
                d.noise
            );
            assert!(
                d.denoise.is_some_and(|s| s.scales == 0) && d.dual.is_none(),
                "{method:?}"
            );
            let first = d.image.pixels().next().unwrap();
            for px in d.image.pixels() {
                for c in 0..3 {
                    assert!(close(px[c], first[c]), "{method:?}: {px:?} vs {first:?}");
                }
            }
        }
        // The prepared frame carries the pattern from the crop's origin.
        let prepared = prepare(&frame, &DevelopSettings::default(), true).unwrap();
        assert_eq!(prepared.pattern, Some(CfaPattern::xtrans().shifted(5, 7)));
        assert!(prepared.noise.is_some());
        // The camera-space demosaic takes it too.
        let out = demosaic(&frame, &DevelopSettings::default()).unwrap();
        for px in out.image.pixels() {
            assert!(
                close(px[0], 0.25) && close(px[1], 0.5) && close(px[2], 0.125),
                "{px:?}"
            );
        }
    }

    #[test]
    fn blown_pixels_come_out_neutral() {
        let mut frame = flat_frame();
        frame.samples = Samples::U16(vec![1100; 16]);
        let clip = DevelopSettings {
            highlights: HighlightMode::Clip,
            ..Default::default()
        };
        let out = develop(&frame, &clip).unwrap();
        for px in out.image.pixels() {
            for v in px {
                assert!((v - 1.0).abs() < 2e-2, "{px:?}");
            }
        }
        // Reconstructed, a fully blown field is neutral too, at the
        // brightest channel's ceiling rather than at 1.0.
        let out = develop(&frame, &DevelopSettings::default()).unwrap();
        let stats = out.highlights.unwrap();
        assert_eq!(stats.clipped, 16, "{stats:?}");
        for px in out.image.pixels() {
            for v in px {
                assert!((v - 4.0).abs() < 5e-2, "{px:?} {stats:?}");
            }
        }
    }

    /// A DCP for the test frame's camera, with the map it is given.
    fn dcp_settings(map: Option<crate::dcp::HueSatMap>) -> DevelopSettings {
        let profile = crate::dcp::tests::test_dcp(map, None).profile().unwrap();
        DevelopSettings {
            profile: Some(Arc::new(profile)),
            ..Default::default()
        }
    }

    #[test]
    fn a_profile_with_an_identity_map_develops_the_same_picture() {
        let frame = flat_frame();
        let plain = develop(&frame, &DevelopSettings::default()).unwrap();

        // The DCP carries the frame's own matrices, so only the map
        // can change anything, and an identity map does not.
        let settings = dcp_settings(Some(crate::dcp::HueSatMap::identity(6, 3, 1)));
        let profiled = develop(&frame, &settings).unwrap();
        for (a, b) in plain.image.data.iter().zip(&profiled.image.data) {
            assert!((a - b).abs() < 1e-5, "{a} became {b}");
        }
        // And the gains it solved are the same ones.
        assert_eq!(
            plain.white_balance.coefficients_f32(),
            profiled.white_balance.coefficients_f32()
        );
    }

    #[test]
    fn a_map_that_darkens_darkens_the_develop() {
        let frame = flat_frame();
        let plain = develop(&frame, &DevelopSettings::default()).unwrap();
        let mut map = crate::dcp::HueSatMap::identity(6, 3, 1);
        for e in &mut map.data {
            e[2] = 0.5;
        }
        let profiled = develop(&frame, &dcp_settings(Some(map))).unwrap();
        for (a, b) in plain.image.data.iter().zip(&profiled.image.data) {
            assert!((a * 0.5 - b).abs() < 1e-5, "{a} became {b}");
        }
    }

    /// A DCP's look table goes on after its map, and the develop's
    /// switch takes it off again: an edit made before the table was
    /// applied keeps its picture.
    #[test]
    fn a_look_table_goes_on_unless_the_develop_says_not() {
        let frame = flat_frame();
        let plain = develop(&frame, &DevelopSettings::default()).unwrap();
        let mut dcp = crate::dcp::tests::test_dcp(None, None);
        let mut look = crate::dcp::HueSatMap::identity(6, 3, 1);
        for e in &mut look.data {
            e[2] = 0.5;
        }
        dcp.look = Some(look);
        let mut settings = DevelopSettings {
            profile: Some(Arc::new(dcp.profile().unwrap())),
            ..Default::default()
        };
        let looked = develop(&frame, &settings).unwrap();
        for (a, b) in plain.image.data.iter().zip(&looked.image.data) {
            assert!((a * 0.5 - b).abs() < 1e-5, "{a} became {b}");
        }
        settings.profile_look = false;
        let without = develop(&frame, &settings).unwrap();
        for (a, b) in plain.image.data.iter().zip(&without.image.data) {
            assert!((a - b).abs() < 1e-5, "{a} became {b}");
        }
    }

    /// The look table is read at the consumer's baseline plus the
    /// profile's offset: the test profile's -0.25 stops undone by a
    /// baseline of 0.25 reads it where a profile with no offset and no
    /// baseline does, and three stops down reads it elsewhere.
    #[test]
    fn a_look_table_is_read_at_the_baseline_and_the_offset() {
        let frame = flat_frame();
        // A linear table whose value scale climbs from 1 to 2 over the
        // value axis, so where it is read shows.
        let mut look = crate::dcp::HueSatMap::identity(6, 3, 5);
        for (i, e) in look.data.iter_mut().enumerate() {
            e[2] = 1.0 + (i / 18) as f32 / 4.0;
        }
        let with = |offset: Option<f32>, baseline: f32| {
            let mut dcp = crate::dcp::tests::test_dcp(None, None);
            dcp.baseline_exposure_offset = offset;
            dcp.look = Some(look.clone());
            let settings = DevelopSettings {
                profile: Some(Arc::new(dcp.profile().unwrap())),
                look_baseline: baseline,
                ..Default::default()
            };
            develop(&frame, &settings).unwrap().image.data
        };
        let level = with(None, 0.0);
        for (a, b) in level.iter().zip(&with(Some(-0.25), 0.25)) {
            assert!((a - b).abs() < 1e-5, "{a} against {b}");
        }
        let down = with(Some(-0.25), -2.75);
        assert!(
            level.iter().zip(&down).any(|(a, b)| (a - b).abs() > 1e-3),
            "three stops down read the table at the same place"
        );
    }

    /// A profile's offset that is not a number, or is far past any
    /// real one, cannot make the look table's scale infinite or zero:
    /// the develop through it stays finite. A table with an entry that
    /// is not a number is refused when the file is read.
    #[test]
    fn a_hostile_offset_or_table_cannot_poison_the_develop() {
        let frame = flat_frame();
        let mut look = crate::dcp::HueSatMap::identity(6, 3, 5);
        for e in &mut look.data {
            e[1] = 1.5;
            e[2] = 0.8;
        }
        for offset in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            128.0,
            -128.0,
            1e30,
        ] {
            let mut dcp = crate::dcp::tests::test_dcp(None, None);
            dcp.baseline_exposure_offset = Some(offset);
            dcp.look = Some(look.clone());
            // Through the file too, where the reader holds it.
            let back = crate::dcp::Dcp::from_bytes(&dcp.to_bytes()).unwrap();
            assert!(
                back.baseline_exposure_offset
                    .is_none_or(|o| o.abs() <= crate::dcp::MAX_OFFSET_STOPS),
                "{offset} read as {:?}",
                back.baseline_exposure_offset
            );
            for dcp in [dcp, back] {
                let settings = DevelopSettings {
                    profile: Some(Arc::new(dcp.profile().unwrap())),
                    ..Default::default()
                };
                let out = develop(&frame, &settings).unwrap();
                assert!(
                    out.image.data.iter().all(|v| v.is_finite()),
                    "an offset of {offset} made a pixel that is not a number"
                );
            }
        }
        // A look table with an entry that is not a number is dropped
        // alone: the profile reads, its map with it, and develops
        // finite. A hue map with one refuses the file.
        let mut nan = look.clone();
        nan.data[7][0] = f32::NAN;
        let mut map = crate::dcp::HueSatMap::identity(6, 3, 1);
        for e in &mut map.data {
            e[2] = 0.5;
        }
        let mut dcp = crate::dcp::tests::test_dcp(Some(map), None);
        dcp.look = Some(nan.clone());
        let back = crate::dcp::Dcp::from_bytes(&dcp.to_bytes()).unwrap();
        assert!(back.look.is_none());
        assert!(back.primary.hue_sat_map.is_some());
        for dcp in [dcp, back] {
            let profile = dcp.profile().unwrap();
            assert!(profile.look.is_none() && profile.maps.is_some());
            let settings = DevelopSettings {
                profile: Some(Arc::new(profile)),
                ..Default::default()
            };
            let out = develop(&frame, &settings).unwrap();
            assert!(out.image.data.iter().all(|v| v.is_finite()));
        }
        let dcp = crate::dcp::tests::test_dcp(Some(nan), None);
        assert!(crate::dcp::Dcp::from_bytes(&dcp.to_bytes()).is_err());

        // A consumer's baseline past any real one, or not a number,
        // reads the table at a finite scale.
        for baseline in [f32::NAN, f32::INFINITY, -f32::INFINITY, 1e30, -1e30] {
            let mut dcp = crate::dcp::tests::test_dcp(None, None);
            dcp.look = Some(look.clone());
            let settings = DevelopSettings {
                profile: Some(Arc::new(dcp.profile().unwrap())),
                look_baseline: baseline,
                ..Default::default()
            };
            let out = develop(&frame, &settings).unwrap();
            assert!(
                out.image.data.iter().all(|v| v.is_finite()),
                "a baseline of {baseline} made a pixel that is not a number"
            );
        }
    }

    /// A white balance read through one profile and applied through
    /// another is not the same white balance.
    ///
    /// This is what a neutral dropper does: it turns the camera gains
    /// that would neutralize the pixel under the pointer into a
    /// temperature and a tint for the panel, and the develop turns
    /// them back into gains. Both directions have to go through the
    /// profile the picture is developed with, or the patch that was
    /// clicked does not come out neutral. The consumer's dropper is
    /// wired that way; this pins the arithmetic under it.
    #[test]
    fn a_neutral_read_through_one_profile_is_not_another_profiles_neutral() {
        let frame = flat_frame();
        let embedded = crate::color::profile_from_frame(&frame).unwrap();

        // A profile for a camera whose sensitivities are not the
        // frame's: the same matrices, skewed.
        let mut dcp = crate::dcp::tests::test_dcp(None, None);
        for (i, v) in dcp.primary.color_matrix.iter_mut().enumerate() {
            *v *= 1.0 + (i % 3) as f64 * 0.15;
        }
        dcp.secondary = None;
        let chosen = dcp.profile().unwrap();

        // The gains a picked neutral would give, as a temperature and
        // tint through the chosen profile.
        let picked = [1.3f64, 1.0, 1.7];
        let read =
            color::resolve_white_balance(&frame, &chosen.camera, WhitePoint::Coefficients(picked))
                .unwrap();

        // Developed through the same profile, that temperature asks
        // for the gains that were picked.
        let back = color::resolve_white_balance(
            &frame,
            &chosen.camera,
            WhitePoint::TempTint(read.temp_tint),
        )
        .unwrap();
        let picked_normalized = {
            let g = read.coefficients_f32();
            [g[0], g[1], g[2]]
        };
        for (i, g) in back.coefficients_f32().iter().enumerate() {
            assert!(
                (g - picked_normalized[i]).abs() < 1e-3,
                "gain {i}: {g} is not {}",
                picked_normalized[i]
            );
        }

        // Developed through the file's own, it asks for other gains,
        // and the patch would not be neutral.
        let wrong =
            color::resolve_white_balance(&frame, &embedded, WhitePoint::TempTint(read.temp_tint))
                .unwrap();
        let off = wrong
            .coefficients_f32()
            .iter()
            .zip(picked_normalized)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(off > 0.01, "the two profiles agreed too well: {off}");
    }

    #[test]
    fn develop_honors_the_crop() {
        let mut frame = flat_frame();
        frame.crop = Some(Rect {
            x: 1,
            y: 0,
            width: 2,
            height: 3,
        });
        let out = develop(&frame, &DevelopSettings::default()).unwrap();
        assert_eq!((out.image.width, out.image.height), (2, 3));
    }
}
