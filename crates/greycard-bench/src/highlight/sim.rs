//! The synthetic overexposure and the develops every method shares.
//!
//! A frame whose highlights the sensor did not clip is the truth. Its
//! overexposure by `stops` is the same frame read with its white level
//! lowered by that many stops: each photosite's black-subtracted value
//! over the new range is `2^stops` times what it was, and the
//! normalization clips it at 1, in raw space, before the white balance,
//! so each channel clips at its own scene level as in a real
//! overexposure. In floating point this is the same mosaic as scaling
//! the black-subtracted samples by `2^stops` and clipping at the
//! original white; only the integers in a file differ, and these are
//! left untouched.
//!
//! Every develop here is the engine's own [`prepare`], [`demosaic_prepared`]
//! and [`finish`] with one set of settings: one demosaic, no chromatic
//! aberration correction, no hot pixel repair, no denoise, the sensor's
//! orientation. Only the highlight mode differs between methods, and the
//! truth goes through the same stages with nothing clipped.

use anyhow::{Context, Result, bail};
use greycard_core::WhitePoint;
use greycard_core::develop::segments::SegmentStats;
use greycard_core::develop::{
    DemosaicMethod, DevelopSettings, HighlightMode, Prepared, apply_dng_warps, crop_samples,
    demosaic_prepared, finish, normalize_levels, prepare, prepare_unclipped,
};
use greycard_core::raw::{CfaPattern, LevelPattern, Orientation, RawFrame, SensorLayout};

/// A frame ready to be overexposed: the decoded file, and its mosaic
/// normalized (black 0, white 1) and cropped as the develop crops it.
pub struct Scene {
    pub frame: RawFrame,
    /// Levels normalized, cropped, no white balance: the truth before
    /// the gains, in units of the original white.
    pub normalized: Vec<f32>,
    pub width: usize,
    pub height: usize,
    /// The pattern from the crop's top left photosite.
    pub pattern: CfaPattern,
}

impl Scene {
    /// Take a decoded frame. Refuses what the benchmark cannot simulate
    /// faithfully: anything but a Bayer or X-Trans mosaic, and a DNG
    /// whose black has column and row differences (its range is not a
    /// plain difference of two levels).
    pub fn new(mut frame: RawFrame) -> Result<Self> {
        let SensorLayout::Cfa(pattern) = &frame.layout else {
            bail!("not a mosaic");
        };
        let bayer = pattern.width == 2 && pattern.height == 2;
        if !(pattern.is_rgb() && (bayer || pattern.is_xtrans())) {
            bail!("a {pattern} mosaic: neither 2x2 Bayer nor X-Trans");
        }
        if frame.levels.black_deltas.is_some() {
            bail!("black level deltas: not simulated");
        }
        let crop = frame.effective_crop();
        let pattern = pattern.shifted(crop.x, crop.y);
        frame.orientation = Orientation::Normal;
        // The simulated clip is the lowered white: a saturation over
        // it would keep what the overexposure is meant to cut.
        frame.levels.saturation = None;
        let full = normalize_levels(&frame);
        let (normalized, width, height) = crop_samples(&full, frame.width, 1, crop);
        Ok(Self {
            frame,
            normalized,
            width,
            height,
            pattern,
        })
    }

    /// The share of photosites at or above `level` of the white.
    pub fn share_at_or_above(&self, level: f32) -> f64 {
        let n = self.normalized.iter().filter(|v| **v >= level).count();
        n as f64 / self.normalized.len().max(1) as f64
    }

    /// The share of photosites the overexposure by `stops` clips.
    pub fn clipped_share(&self, stops: f32) -> f64 {
        self.share_at_or_above(2f32.powf(-stops))
    }

    /// Whether the scene's mosaic is X-Trans.
    pub fn is_xtrans(&self) -> bool {
        self.pattern.is_xtrans()
    }
}

/// `frame` with its white level lowered by `stops`: black plus the range
/// over `2^stops`, at every pattern position.
pub fn lowered(frame: &RawFrame, stops: f32) -> Result<RawFrame> {
    let mut out = frame.clone();
    out.levels.white = lowered_white(&frame.levels.black, &frame.levels.white, stops)?;
    Ok(out)
}

fn lowered_white(black: &LevelPattern, white: &LevelPattern, stops: f32) -> Result<LevelPattern> {
    let width = lcm(black.width, white.width);
    let height = lcm(black.height, white.height);
    let cpp = white.cpp;
    let factor = 2f32.powf(-stops);
    let mut values = Vec::with_capacity(width * height * cpp);
    for r in 0..height {
        for c in 0..width {
            for ch in 0..cpp {
                let b = black.at(r, c, ch);
                values.push(b + (white.at(r, c, ch) - b) * factor);
            }
        }
    }
    LevelPattern::new(width, height, cpp, values).context("the lowered white")
}

fn lcm(a: usize, b: usize) -> usize {
    let gcd = |mut a: usize, mut b: usize| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    a / gcd(a, b) * b
}

/// The settings every develop of the benchmark shares, with `highlights`
/// and the white balance `white`.
pub fn settings(
    highlights: HighlightMode,
    demosaic: DemosaicMethod,
    white: WhitePoint,
) -> DevelopSettings {
    DevelopSettings {
        white_point: white,
        highlights,
        demosaic,
        chromatic_aberration: None,
        hot_pixels: None,
        denoise: None,
        orientation: Some(Orientation::Normal),
        ..Default::default()
    }
}

/// A develop's result: the picture in the working space (linear
/// Rec.2020, interleaved, sensor orientation) and the mosaic it was
/// demosaiced from (white balanced).
pub struct Developed {
    pub rgb: Vec<f32>,
    pub mosaic: Vec<f32>,
    pub gains: [f32; 3],
    /// What the segment pass did, when it ran.
    pub segments: Option<SegmentStats>,
}

/// The engine's develop of `frame` with `mode`, at its as-shot white:
/// what the editor does, the reconstruction cut at the develop's ceiling
/// (the largest gain).
pub fn develop_mode(
    frame: &RawFrame,
    mode: HighlightMode,
    demosaic: DemosaicMethod,
) -> Result<Developed> {
    let s = settings(mode, demosaic, WhitePoint::AsShot);
    let prepared = prepare(frame, &s, false)?;
    let segments = prepared.segments;
    let mut d = finish_prepared(frame, prepared, &s)?;
    d.segments = segments;
    Ok(d)
}

/// The frame balanced at `white` and nothing clipped or reconstructed,
/// every sample times `scale`: the truth (`scale` the overexposure's
/// factor), or the clipped input as it stands (a lowered frame, `scale`
/// 1).
pub fn prepare_scaled(
    frame: &RawFrame,
    scale: f32,
    demosaic: DemosaicMethod,
    white: WhitePoint,
) -> Result<Prepared> {
    let s = settings(HighlightMode::Clip, demosaic, white);
    let mut prepared = prepare_unclipped(frame, &s, false)?;
    if scale != 1.0 {
        prepared.samples.iter_mut().for_each(|v| *v *= scale);
        prepared.ceiling *= scale;
    }
    Ok(prepared)
}

/// [`prepare_scaled`] demosaiced and moved into the working space.
pub fn develop_scaled(
    frame: &RawFrame,
    scale: f32,
    demosaic: DemosaicMethod,
    white: WhitePoint,
) -> Result<Developed> {
    let prepared = prepare_scaled(frame, scale, demosaic, white)?;
    finish_prepared(
        frame,
        prepared,
        &settings(HighlightMode::Clip, demosaic, white),
    )
}

/// The develop's ceiling for a frame balanced by `gains`: the largest
/// gain, at least 1, where the engine cuts every reconstruction.
pub fn ceiling_of(gains: [f32; 3]) -> f32 {
    gains.iter().copied().fold(1.0f32, f32::max)
}

/// The two truths of a case: the frame times `scale`, nothing cut ("what
/// is lost"), and the same mosaic cut per photosite at the develop's
/// ceiling before the demosaic ("what a method can do within today's
/// engine"). Returns them and that ceiling.
pub fn develop_truths(
    frame: &RawFrame,
    scale: f32,
    demosaic: DemosaicMethod,
    white: WhitePoint,
) -> Result<(Developed, Developed, f32)> {
    let prepared = prepare_scaled(frame, scale, demosaic, white)?;
    let ceiling = ceiling_of(prepared.gains);
    let capped = prepared.samples.clone();
    let template = prepared.clone();
    let raw = finish_prepared(
        frame,
        prepared,
        &settings(HighlightMode::Clip, demosaic, white),
    )?;
    let capped = develop_mosaic(frame, template, capped, ceiling, demosaic)?;
    Ok((raw, capped, ceiling))
}

/// A prepared frame with its samples replaced by `mosaic` (white
/// balanced, the prepared frame's size), cut at `ceiling` as the develop
/// cuts a reconstruction, then demosaiced and finished. Every method the
/// benchmark adds enters here, with its ceiling stated.
pub fn develop_mosaic(
    frame: &RawFrame,
    mut prepared: Prepared,
    mut mosaic: Vec<f32>,
    ceiling: f32,
    demosaic: DemosaicMethod,
) -> Result<Developed> {
    mosaic.iter_mut().for_each(|v| *v = v.min(ceiling));
    prepared.samples = mosaic;
    prepared.ceiling = ceiling;
    finish_prepared(
        frame,
        prepared,
        &settings(HighlightMode::Clip, demosaic, WhitePoint::AsShot),
    )
}

fn finish_prepared(frame: &RawFrame, prepared: Prepared, s: &DevelopSettings) -> Result<Developed> {
    let mosaic = prepared.samples.clone();
    let gains = prepared.gains;
    let (w, h) = (prepared.width, prepared.height);
    let (mut rgb, dual, denoise) = demosaic_prepared(&prepared, s)?;
    apply_dng_warps(frame, &mut rgb, w, h, 1);
    let out = finish(prepared, rgb, dual, denoise, s)?;
    Ok(Developed {
        rgb: out.image.data,
        mosaic,
        gains,
        segments: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lowered_white_is_black_plus_a_fraction_of_the_range() {
        let black = LevelPattern::new(2, 2, 1, vec![100.0, 104.0, 108.0, 112.0]).unwrap();
        let white = LevelPattern::uniform(4100.0, 1);
        let out = lowered_white(&black, &white, 2.0).unwrap();
        assert_eq!((out.width, out.height), (2, 2));
        assert_eq!(out.at(0, 0, 0), 100.0 + 4000.0 / 4.0);
        assert_eq!(out.at(1, 1, 0), 112.0 + 3988.0 / 4.0);
        assert_eq!(lcm(2, 6), 6);
        assert_eq!(lcm(1, 1), 1);
    }
}
