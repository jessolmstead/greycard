//! The CPU reference pipeline: levels, white balance, demosaic, matrix.
//!
//! Each step is a public function so it can be tested alone and so a GPU
//! implementation has something to be checked against. [`develop`] runs
//! them in order into the working space; [`demosaic`] stops after the
//! demosaic and hands back camera-space RGB, which is what a linear DNG
//! stores.

pub mod amaze;
pub mod ca;
pub mod defringe;
pub mod dehaze;
pub mod denoise;
pub mod dual;
pub mod highlights;
pub mod hotpixels;
pub mod lens;
pub mod local_contrast;
pub mod nlm;
pub mod noise;
pub mod profile;
pub mod rcd;
pub mod reduced;
pub mod retouch;
pub mod segments;
pub mod sharpen;
pub mod vng4;
pub mod xtrans;

use std::sync::Arc;

use rayon::prelude::*;

use crate::color::{self, WhitePoint};
use crate::error::{Error, Result};
use crate::image::{CameraImage, WorkingImage};
use crate::raw::{CfaPattern, Mosaic, Orientation, RawFrame, Rect, SensorLayout};

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
    /// Capture sharpening on the working image, last; `None` leaves the
    /// picture as the lens and the demosaic made it.
    ///
    /// Last of [`develop`], that is: [`finish`] runs it straight after
    /// the matrix and the orientation, which is before any geometry a
    /// consumer puts on top. A consumer that corrects a lens, or
    /// otherwise resamples, must leave this `None` and call
    /// [`sharpen::sharpen`] itself once the picture has stopped
    /// moving: a resample blurs what was sharpened, and the point
    /// spread the deconvolution assumes is the one measured on the
    /// mosaic, which a resample changes.
    pub sharpen: Option<sharpen::SharpenOptions>,
    /// Haze removal on the working image, before the sharpen; `None`
    /// leaves the air as it was. Placed as the sharpen is: a consumer
    /// with geometry of its own leaves it `None` and calls
    /// [`dehaze::dehaze`] after that geometry, before its sharpen, so
    /// the viewport and the export read one order.
    pub dehaze: Option<dehaze::DehazeOptions>,
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
            demosaic: DemosaicMethod::default(),
            highlights: HighlightMode::default(),
            segments: segments::SegmentOptions::default(),
            chromatic_aberration: Some(ca::CaOptions::default()),
            dual_contrast: dual::DualContrast::default(),
            denoise: None,
            hot_pixels: Some(hotpixels::HotPixelOptions::default()),
            sharpen: None,
            dehaze: None,
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
    /// What the sharpen did, when it ran.
    pub sharpen: Option<sharpen::SharpenStats>,
    /// What the dehaze did, when it ran.
    pub dehaze: Option<dehaze::DehazeStats>,
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
    /// linear one; white balance applied.
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
    orientation: Orientation,
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
/// the consumer states, since the export develops on the reference.
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
    let settings = &resolve_settings(frame, settings);
    // At the door, before the levels are normalized and the frame is
    // copied: preparing a 101 MP mosaic this build cannot demosaic
    // costs a gigabyte and several seconds to reach the same refusal.
    if let SensorLayout::Cfa(pattern) = &frame.layout {
        if !demosaics(pattern) {
            return Err(unsupported_mosaic(pattern));
        }
        if pattern.is_xtrans() {
            log_xtrans_skips(settings, measure_noise);
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
    clip_highlights(&mut samples, ceiling);

    let mut ca_stats = None;
    if let (Some(pattern), Some(options)) = (&pattern, &settings.chromatic_aberration)
        && ca::is_bayer(pattern)
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
    })
}

/// The stages of a develop under some settings that have no X-Trans
/// version yet, which an X-Trans frame passes by: each is gated on a
/// Bayer pattern where it is called.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct XtransSkips {
    /// Stages on by default, or there for another's sake: said in the
    /// log as a matter of record.
    pub defaults: Vec<&'static str>,
    /// Stages the user asked for (the profiled denoise): worth a
    /// warning, since what was asked for does not happen.
    pub requested: Vec<&'static str>,
}

/// What an X-Trans develop under `settings` leaves out; `measure_noise`
/// as [`prepare`] takes it.
pub fn xtrans_skips(settings: &DevelopSettings, measure_noise: bool) -> XtransSkips {
    let mut skips = XtransSkips::default();
    if measure_noise || wants_noise(settings) {
        skips.defaults.push("the noise measurement");
    }
    if settings.hot_pixels.is_some() {
        skips.defaults.push("the hot pixel repair");
    }
    if settings.chromatic_aberration.is_some() {
        skips.defaults.push("the chromatic aberration correction");
    }
    skips
        .defaults
        .push("the capture sharpen's radius measurement");
    if settings.denoise.is_some() {
        skips.requested.push("the profiled denoise");
    }
    skips
}

/// Say what [`xtrans_skips`] finds, so the log says so rather than the
/// stage going quietly.
fn log_xtrans_skips(settings: &DevelopSettings, measure_noise: bool) {
    let skips = xtrans_skips(settings, measure_noise);
    if !skips.defaults.is_empty() {
        log::info!(
            "X-Trans mosaic: {} have no X-Trans version yet and are left out",
            skips.defaults.join(", ")
        );
    }
    if !skips.requested.is_empty() {
        log::warn!(
            "X-Trans mosaic: {} was asked for and does not run; it has no X-Trans version yet",
            skips.requested.join(", ")
        );
    }
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
        (Some(options), Some(model)) => Some(denoise::denoise_profiled(
            &mut rgb, width, height, &model, options,
        )?),
        _ => None,
    };
    Ok((rgb, dual_stats, denoise_stats))
}

/// Everything [`develop`] does after the demosaic: the matrix into the
/// working space, the orientation, and the dehaze and the sharpen when the settings
/// carry one. `rgb` is interleaved camera RGB of the prepared frame's
/// size, in its units.
///
/// A consumer with geometry of its own leaves
/// [`DevelopSettings::sharpen`] `None` and sharpens after it; see that
/// field.
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
    let mut image = orient(image, prepared.orientation);
    let clip_level = prepared.ceiling * CLIP_FRACTION;
    let dehaze_stats = settings
        .dehaze
        .as_ref()
        .map(|options| dehaze::dehaze(&mut image, options));
    let sharpen_stats = settings
        .sharpen
        .as_ref()
        .map(|options| sharpen::sharpen(&mut image, options, prepared.sharpen_radius, clip_level));
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
        sharpen: sharpen_stats,
        dehaze: dehaze_stats,
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
    let (rgb, dual, denoise) = demosaic_prepared(&prepared, settings)?;
    finish(prepared, rgb, dual, denoise, settings)
}

/// Below this a sample is too dark to say how sharp the mosaic is:
/// RawTherapee's 1000 of 65535.
const RADIUS_LOWER_LIMIT: f32 = 1000.0 / 65535.0;
/// This close to the clip a value counts as clipped.
pub(crate) const CLIP_FRACTION: f32 = 0.98;

/// Demosaic a frame and stop there: camera-space RGB with no white balance,
/// no matrix and no orientation applied.
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
        denoise::denoise_profiled(
            &mut rgb,
            width,
            height,
            &estimate.model.after_gains(gains),
            options,
        )?;
    }
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
/// measured noise, transformed for the gains the RGB carries.
fn dual_contrast(
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
/// interleaved RGB with no pattern. With `estimate_noise`, a Bayer
/// frame's noise is measured before the gains go on, and with
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
            if estimate_noise && noise::is_bayer(&pattern) {
                noise = Some(noise::estimate_noise(&samples, width, height, &pattern)?);
            }
            if let (Some(options), Some(estimate)) = (hot_pixels, &noise)
                && hotpixels::is_bayer(&pattern)
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
            apply_gains_rgb(&mut samples, gains);
            None
        }
        SensorLayout::Monochrome => {
            return Err(Error::Unsupported(
                "monochrome sensors are not developed yet".into(),
            ));
        }
    };
    Ok((samples, width, height, pattern, noise, hot))
}

/// Subtract black, divide by the range to white, clamp to 0..1.
///
/// Works on the full frame so level patterns index by sensor position.
pub fn normalize_levels(frame: &RawFrame) -> Vec<f32> {
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
            width: 4,
            height: 4,
            channels: 1,
            layout: SensorLayout::Cfa(pattern),
            samples: Samples::U16(samples),
            levels: Levels {
                black: LevelPattern::uniform(100.0, 1),
                white: LevelPattern::uniform(1100.0, 1),
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
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
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

    /// What an X-Trans develop leaves out: the defaults as a record, the
    /// denoise, which the user asked for, apart.
    #[test]
    fn the_xtrans_skips_put_a_requested_denoise_apart() {
        let defaults = xtrans_skips(&DevelopSettings::default(), false);
        assert!(defaults.requested.is_empty());
        assert!(defaults.defaults.contains(&"the hot pixel repair"));
        assert!(
            defaults
                .defaults
                .contains(&"the chromatic aberration correction")
        );
        assert!(defaults.defaults.contains(&"the noise measurement"));
        let bare = DevelopSettings {
            hot_pixels: None,
            chromatic_aberration: None,
            demosaic: DemosaicMethod::Amaze,
            ..Default::default()
        };
        assert_eq!(
            xtrans_skips(&bare, false).defaults,
            vec!["the capture sharpen's radius measurement"]
        );
        assert!(
            xtrans_skips(&bare, true)
                .defaults
                .contains(&"the noise measurement")
        );
        let denoised = DevelopSettings {
            denoise: Some(denoise::DenoiseOptions::default()),
            ..bare
        };
        assert_eq!(
            xtrans_skips(&denoised, false).requested,
            vec!["the profiled denoise"]
        );
    }

    /// The stages with no X-Trans version step aside, the demosaic is
    /// Markesteijn's or bilinear, and a flat grey comes out flat and
    /// grey whatever the method.
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
            assert!(d.chromatic_aberration.is_none(), "{method:?}");
            assert!(d.hot_pixels.is_none() && d.noise.is_none(), "{method:?}");
            assert!(d.denoise.is_none() && d.dual.is_none(), "{method:?}");
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
        assert!(prepared.noise.is_none());
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
