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
pub mod retouch;
pub mod segments;
pub mod sharpen;
pub mod vng4;

use std::sync::Arc;

use rayon::prelude::*;

use crate::color::{self, WhitePoint};
use crate::error::{Error, Result};
use crate::image::{CameraImage, WorkingImage};
use crate::raw::{CfaPattern, Orientation, RawFrame, Rect, SensorLayout};

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
    /// measured on the frame; `None` leaves them.
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
            hot_pixels: None,
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
}

impl DemosaicMethod {
    pub const ALL: &[DemosaicMethod] = &[
        DemosaicMethod::Bilinear,
        DemosaicMethod::Rcd,
        DemosaicMethod::Amaze,
        DemosaicMethod::Vng4,
        DemosaicMethod::RcdVng4,
        DemosaicMethod::AmazeVng4,
    ];

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
fn not_bayer(pattern: &CfaPattern) -> Error {
    Error::Unsupported(format!(
        "this build does not develop a {pattern} mosaic yet; it demosaics 2x2 Bayer only"
    ))
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
    // The backstop for a consumer that comes straight here: [`prepare`]
    // turns such a frame away first. Bilinear alone would run on a
    // pattern that is not Bayer and make a mess of it, which is no
    // kindness either.
    if !rcd::is_bayer(pattern) {
        return Err(not_bayer(pattern));
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
    // At the door, before the levels are normalized and the frame is
    // copied: preparing a 101 MP mosaic this build cannot demosaic
    // costs a gigabyte and several seconds to reach the same refusal.
    if let SensorLayout::Cfa(pattern) = &frame.layout
        && !rcd::is_bayer(pattern)
    {
        return Err(not_bayer(pattern));
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
        orientation: settings.orientation.unwrap_or(frame.orientation),
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
            settings.demosaic,
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
            for (x, px) in row.chunks_exact_mut(cpp).enumerate() {
                for (ch, s) in px.iter_mut().enumerate() {
                    let b = black.at(y, x, ch);
                    let range = (white.at(y, x, ch) - b).max(f32::EPSILON);
                    *s = ((*s - b) / range).clamp(0.0, 1.0);
                }
            }
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
            for (x, s) in row.iter_mut().enumerate() {
                let c = pattern
                    .color_at(y, x)
                    .rgb_index()
                    .expect("RGB pattern checked by caller");
                *s *= gains[c];
            }
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
            0.0
        }
    })
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

    /// An X-Trans frame goes no further than the demosaic, and what it
    /// is refused with is a sentence, not a 36-entry pattern dump.
    #[test]
    fn an_xtrans_mosaic_is_refused_in_words() {
        let pattern = crate::raw::xtrans();
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
            assert!(complaint.contains("X-Trans"), "{method:?}: {complaint}");
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
            "unsupported: this build does not develop a 6x6 (X-Trans) mosaic yet; \
             it demosaics 2x2 Bayer only"
        );
        // The demosaic keeps the same words for a consumer that comes
        // straight to it.
        let samples = vec![0.5f32; 144];
        let late = demosaic_cfa(&samples, 12, 12, &pattern, DemosaicMethod::Bilinear)
            .unwrap_err()
            .to_string();
        assert_eq!(late, early);
        // And the sites that say "2x2 Bayer" name the pattern the same
        // short way when they are reached on their own.
        let complaint = rcd::demosaic_rcd(&samples, 12, 12, &pattern)
            .unwrap_err()
            .to_string();
        assert_eq!(
            complaint,
            "unsupported: RCD needs a 2x2 Bayer pattern, got 6x6 (X-Trans)"
        );
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
