//! The noise model, and how it is estimated from the frame itself.
//!
//! A sensor's noise at a level `x` (levels normalized, before white
//! balance) has variance `a * x + b`: the `a` term is photon shot noise,
//! amplified by the ISO gain; the `b` term is read noise. darktable ships a
//! database of `a` and `b` per camera and ISO, measured from test shots.
//! greycard does not have that database yet, and there is no ISO on
//! [`crate::raw::RawFrame`], so the model is measured from the frame:
//! this estimator is ours, not a port.
//!
//! The frame is cut into 16x16 blocks. In each block, per filter color,
//! the residual of every sample against the mean of its four same-color
//! neighbors two pixels away is squared and averaged: for white noise
//! that is 1.25 times the variance, and texture only adds to it. On
//! X-Trans, whose sites of a color sit at no one offset, the residual is
//! against every same-color site in the 5x5, weighted to be exact on a
//! plane ([`super::sites::planar_weights`]), and each squared residual
//! is divided by its own `1 + sum w^2`: 1.2 at a red or blue site (five
//! neighbors), 1.06 to 1.08 at a green one (twelve to sixteen). Blocks
//! are binned by mean level, evenly in the square root of the level so
//! the shadows get as many bins as the highlights, and in each bin the
//! quietest quarter of the blocks is taken as the noise-only population,
//! corrected for the bias that picking the quietest quarter introduces.
//! A weighted straight line through the bins is the model. Clipped blocks
//! are left out.
//!
//! What this cannot tell apart is fine, uniform texture at the noise's
//! scale, foliage at a distance say, from noise; a textured frame with no
//! flat area at some level overstates the noise there. The database, when
//! it comes, replaces the estimate rather than the interface.

use rayon::prelude::*;

use super::sites::{Phases, Tap, linear, of_color, planar_weights};
use crate::error::{Error, Result};
use crate::raw::CfaPattern;

pub use super::rcd::is_bayer;

/// Variance as a straight line in level, per RGB channel.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NoiseModel {
    /// Shot noise: variance per unit of level.
    pub a: [f32; 3],
    /// Read noise: variance at zero level.
    pub b: [f32; 3],
}

impl NoiseModel {
    /// The variance of channel `c` at `level`.
    pub fn variance(&self, c: usize, level: f32) -> f32 {
        (self.a[c] * level + self.b[c]).max(0.0)
    }

    /// The standard deviation of channel `c` at `level`.
    pub fn sigma(&self, c: usize, level: f32) -> f32 {
        self.variance(c, level).sqrt()
    }

    /// The model of the same data after each channel is multiplied by a
    /// gain: a level `y = g x` has variance `g^2 (a x + b) = g a y + g^2 b`.
    pub fn after_gains(&self, gains: [f32; 3]) -> NoiseModel {
        let mut out = *self;
        for (c, g) in gains.into_iter().enumerate() {
            out.a[c] = g * self.a[c];
            out.b[c] = g * g * self.b[c];
        }
        out
    }

    /// True when no channel has measurable noise.
    pub fn is_silent(&self) -> bool {
        (0..3).all(|c| self.sigma(c, MID_GREY) < SILENT_SIGMA)
    }
}

/// The level the estimate is summarized at.
pub const MID_GREY: f32 = 0.18;
/// Below this standard deviation at mid grey a channel counts as clean.
pub const SILENT_SIGMA: f32 = 1e-5;

/// An estimated model and how much evidence it rests on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoiseEstimate {
    pub model: NoiseModel,
    /// Unclipped blocks that went into each channel's line.
    pub blocks: [usize; 3],
    /// Level bins with enough blocks to vote, per channel.
    pub bins: [usize; 3],
}

const BLOCK: usize = 16;
/// Per channel, per level bin: each block's (mean level, noise variance,
/// samples).
type Bins = [Vec<Vec<(f32, f32, usize)>>; 3];
const BINS: usize = 32;
/// Blocks a bin needs before its quietest quarter means anything.
const MIN_BLOCKS_PER_BIN: usize = 8;
/// Same-color samples a block needs to be counted.
const MIN_SAMPLES: usize = 32;
/// A sample at or above this is clipped; its block is left out.
const CLIP: f32 = 0.98;
/// The residual against four neighbors has 1 + 4/16 of the variance.
const RESIDUAL_GAIN: f32 = 1.25;

/// Whether [`estimate_noise`] reads this pattern: 2x2 Bayer or X-Trans.
pub fn measures(pattern: &CfaPattern) -> bool {
    is_bayer(pattern) || pattern.is_xtrans()
}

/// Estimate the noise model of a Bayer or an X-Trans mosaic. `samples`
/// are levels normalized to 0..1, before white balance. A frame with no
/// unclipped blocks gives a silent model, not an error.
pub fn estimate_noise(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
) -> Result<NoiseEstimate> {
    if !measures(pattern) {
        return Err(Error::Unsupported(format!(
            "noise estimation needs a 2x2 Bayer or an X-Trans pattern, got {pattern}"
        )));
    }
    if width < BLOCK + 4 || height < BLOCK + 4 {
        return Ok(NoiseEstimate {
            model: NoiseModel::default(),
            blocks: [0; 3],
            bins: [0; 3],
        });
    }
    let at = |i: usize| samples[i];
    let rows = if is_bayer(pattern) {
        block_rows(samples, width, height, pattern, |i, v| {
            let neighbors = (at(i - 2) + at(i + 2) + at(i - 2 * width) + at(i + 2 * width)) * 0.25;
            let r = v - neighbors;
            r * r
        })
    } else {
        // Each site's taps, and the factor that puts its squared
        // residual on the scale the blocks divide by.
        let table = Phases::build(pattern, |y, x| {
            let color = pattern.color_at(y, x).rgb_index().expect("X-Trans is RGB");
            let offsets = of_color(pattern, y, x, color, 2);
            let weights = planar_weights(&offsets).expect("X-Trans sites span a plane");
            let gain = 1.0 + weights.iter().map(|w| w * w).sum::<f32>();
            let taps: Vec<Tap> = offsets
                .iter()
                .zip(weights)
                .map(|(&(dy, dx), w)| (dy, dx, w))
                .collect();
            (linear(&taps, width), RESIDUAL_GAIN / gain)
        });
        block_rows(samples, width, height, pattern, |i, v| {
            let (taps, scale) = table.at(i / width, i % width);
            let neighbors: f32 = taps
                .iter()
                .map(|&(o, w)| w * at((i as isize + o) as usize))
                .sum();
            let r = v - neighbors;
            r * r * scale
        })
    };
    Ok(fit_bins(rows))
}

/// The noise of a developed picture, on the denoiser's luma and two
/// chroma axes ([`super::denoise`]'s orthonormal Y0U0V0): a picture's
/// grain is correlated between its channels (the demosaic's color
/// differences, the matrix, a denoise that cuts chroma harder than
/// luma), and only on these axes is it close to independent. Each
/// axis's variance is a line in the pixel's mean level, `(r+g+b)/3`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PictureNoise {
    /// Per axis (luma, then the two chroma): variance at a mean level.
    pub axes: NoiseModel,
}

impl PictureNoise {
    /// The luma's standard deviation at `level`, per channel: the
    /// spread a grey's three channels share.
    pub fn luma_sigma(&self, level: f32) -> f32 {
        (self.axes.variance(0, level) / 3.0).sqrt()
    }

    pub fn is_silent(&self) -> bool {
        self.axes.is_silent()
    }
}

/// Estimate the noise of a picture of three interleaved channels a
/// pixel, a demosaiced one in its own linear values, from the pixels
/// `known` marks, on the axes of [`PictureNoise`]. The same blocks,
/// residual, and fit as [`estimate_noise`]; the neighbors are two
/// pixels away, as a mosaic's same-color ones are, past most of the
/// correlation a demosaic leaves between neighbors, so what is
/// measured is the noise's variance at high frequency: noise drawn to
/// it ([`add_picture_noise`]) matches the picture's there and is a
/// little finer than the picture's own. A block is read when it and
/// the two pixels round it are all known.
///
/// A picture has no one clip, and its highlights may be made up
/// smooth (reconstructed past 1, or a clipped camera channel mixed
/// through the matrix), which would pass for the quietest blocks at
/// their level. So a block reaching the brightest value of a channel
/// is left out, and the line is fitted twice, the second time without
/// the blocks quieter than a sixteenth of what the first gave at their
/// level.
pub fn estimate_picture_noise(
    data: &[f32],
    width: usize,
    height: usize,
    known: &[bool],
) -> PictureNoise {
    if width < BLOCK + 4 || height < BLOCK + 4 || data.len() != width * height * 3 {
        return PictureNoise::default();
    }
    // How many unknown pixels lie above and left of each corner.
    let stride = width + 1;
    let mut unknown = vec![0u32; stride * (height + 1)];
    let mut peak = [f32::MIN; 3];
    for y in 0..height {
        let mut row = 0u32;
        for x in 0..width {
            let i = y * width + x;
            row += u32::from(!known[i]);
            unknown[(y + 1) * stride + x + 1] = unknown[y * stride + x + 1] + row;
            if known[i] {
                for c in 0..3 {
                    peak[c] = peak[c].max(data[i * 3 + c]);
                }
            }
        }
    }
    let unknown_in = |x0: usize, y0: usize, x1: usize, y1: usize| {
        unknown[y1 * stride + x1] + unknown[y0 * stride + x0]
            - unknown[y0 * stride + x1]
            - unknown[y1 * stride + x0]
    };
    let near_peak = peak.map(|p| if p > 0.0 { p * PEAK_FRACTION } else { f32::MAX });
    let yuv = |i: usize| super::denoise::to_yuv([data[i * 3], data[i * 3 + 1], data[i * 3 + 2]]);
    let block_rows: Vec<usize> = (2..height - 2 - BLOCK + 1).step_by(BLOCK).collect();
    // Per block: its mean level and each axis's variance.
    let blocks: Vec<(f32, [f32; 3])> = block_rows
        .par_iter()
        .flat_map_iter(|&by| {
            let mut out = Vec::new();
            'block: for bx in (2..width - 2 - BLOCK + 1).step_by(BLOCK) {
                if unknown_in(bx - 2, by - 2, bx + BLOCK + 2, by + BLOCK + 2) > 0 {
                    continue;
                }
                let mut sum = 0.0f64;
                let mut sq = [0.0f64; 3];
                for y in by..by + BLOCK {
                    for x in bx..bx + BLOCK {
                        let i = y * width + x;
                        let p = &data[i * 3..i * 3 + 3];
                        if (0..3).any(|c| p[c] >= near_peak[c]) {
                            continue 'block;
                        }
                        sum += (p[0] + p[1] + p[2]) as f64 / 3.0;
                        let v = yuv(i);
                        let n = [
                            yuv(i - 2),
                            yuv(i + 2),
                            yuv(i - 2 * width),
                            yuv(i + 2 * width),
                        ];
                        for k in 0..3 {
                            let r = v[k] - (n[0][k] + n[1][k] + n[2][k] + n[3][k]) * 0.25;
                            sq[k] += (r * r) as f64;
                        }
                    }
                }
                let n = (BLOCK * BLOCK) as f64;
                out.push(((sum / n) as f32, sq.map(|s| (s / n) as f32 / RESIDUAL_GAIN)));
            }
            out
        })
        .collect();
    let fit = |keep: &dyn Fn(usize, f32, f32) -> bool| {
        let mut bins: Bins = std::array::from_fn(|_| vec![Vec::new(); BINS]);
        let mut counts = [0usize; 3];
        for &(level, var) in &blocks {
            let bin = ((level.max(0.0).sqrt() * BINS as f32) as usize).min(BINS - 1);
            for k in 0..3 {
                if var[k] > STILL && keep(k, level, var[k]) {
                    bins[k][bin].push((level, var[k], BLOCK * BLOCK));
                    counts[k] += 1;
                }
            }
        }
        fit_bins(vec![(bins, counts)]).model
    };
    let first = fit(&|_, _, _| true);
    let axes = fit(&|k, level, var| var * QUIET_FLOOR >= first.variance(k, level.max(0.0)));
    PictureNoise { axes }
}

/// A block with a value this close to a channel's brightest is left
/// out of a picture's estimate.
const PEAK_FRACTION: f32 = 0.99;
/// A block quieter than the first fit over this is made, not seen.
const QUIET_FLOOR: f32 = 16.0;
/// A block whose variance is no more than this does not move at all.
const STILL: f32 = 1e-14;

/// Noise added to `data`, three interleaved channels a pixel `width`
/// wide whose top left lies at `origin` in the frame, where `mask` is
/// nonzero: per axis of [`PictureNoise`], at the pixel's own mean
/// level, the variance `model` gives less what `present` says it
/// carries already, drawn on those axes and rotated back. Drawn from
/// `seed` and the pixel's place in the frame alone, so the same call
/// gives the same noise wherever its window lies.
pub fn add_picture_noise(
    data: &mut [f32],
    width: usize,
    origin: (usize, usize),
    mask: &[f32],
    model: &PictureNoise,
    present: &PictureNoise,
    seed: u64,
) {
    for (i, px) in data.as_chunks_mut::<3>().0.iter_mut().enumerate() {
        if mask[i] <= 0.0 {
            continue;
        }
        let (x, y) = ((origin.0 + i % width) as u64, (origin.1 + i / width) as u64);
        let mut state = seed ^ (y << 32 | x).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let level = ((px[0] + px[1] + px[2]) / 3.0).max(0.0);
        let draw: [f32; 3] = std::array::from_fn(|k| {
            let var = model.axes.variance(k, level) - present.axes.variance(k, level);
            let g = gauss(&mut state);
            if var > 0.0 { var.sqrt() * g } else { 0.0 }
        });
        let add = super::denoise::from_yuv(draw);
        for c in 0..3 {
            px[c] += add[c];
        }
    }
}

/// One standard normal sample from `state`, moved on (splitmix64 and
/// Box-Muller).
fn gauss(state: &mut u64) -> f32 {
    let mut next = || {
        *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        ((z >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    let (u1, u2) = (next(), next());
    ((-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()) as f32
}

/// Per row of blocks, each channel's bins and its count of blocks.
type BlockRows = Vec<(Bins, [usize; 3])>;

/// Per channel, per bin: (mean level, noise variance estimate) per
/// block. Block rows in parallel, each with its own bins.
/// `residual(i, v)` is the squared residual of the sample `v` at index
/// `i` against its same-color neighbors, on the scale where white noise
/// reads [`RESIDUAL_GAIN`] times its variance; it reads up to two sites
/// out either way.
fn block_rows(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    residual: impl Fn(usize, f32) -> f32 + Sync,
) -> BlockRows {
    let block_rows: Vec<usize> = (2..height - 2 - BLOCK + 1).step_by(BLOCK).collect();
    block_rows
        .par_iter()
        .map(|&by| {
            let mut bins: Bins = std::array::from_fn(|_| vec![Vec::new(); BINS]);
            let mut blocks = [0usize; 3];
            for bx in (2..width - 2 - BLOCK + 1).step_by(BLOCK) {
                let mut sum = [0.0f64; 3];
                let mut sq = [0.0f64; 3];
                let mut n = [0usize; 3];
                let mut clipped = false;
                'block: for y in by..by + BLOCK {
                    for x in bx..bx + BLOCK {
                        let i = y * width + x;
                        let v = samples[i];
                        if v >= CLIP {
                            clipped = true;
                            break 'block;
                        }
                        let c = pattern
                            .color_at(y, x)
                            .rgb_index()
                            .expect("measured patterns are RGB");
                        sum[c] += v as f64;
                        sq[c] += residual(i, v) as f64;
                        n[c] += 1;
                    }
                }
                if clipped {
                    continue;
                }
                for c in 0..3 {
                    if n[c] < MIN_SAMPLES {
                        continue;
                    }
                    let mean = (sum[c] / n[c] as f64) as f32;
                    let var = (sq[c] / n[c] as f64) as f32 / RESIDUAL_GAIN;
                    let bin = ((mean.max(0.0).sqrt() * BINS as f32) as usize).min(BINS - 1);
                    bins[c][bin].push((mean, var, n[c]));
                    blocks[c] += 1;
                }
            }
            (bins, blocks)
        })
        .collect()
}

/// The model from the blocks: the rows' bins merged in order, then per
/// channel the quietest quarter of each bin and a weighted line through
/// the bins.
fn fit_bins(rows: BlockRows) -> NoiseEstimate {
    let mut model = NoiseModel::default();
    let mut blocks = [0usize; 3];
    let mut bins_used = [0usize; 3];
    let mut bins: Bins = std::array::from_fn(|_| vec![Vec::new(); BINS]);
    for (row_bins, row_blocks) in rows {
        for c in 0..3 {
            blocks[c] += row_blocks[c];
            for (bin, more) in bins[c].iter_mut().zip(row_bins[c].iter()) {
                bin.extend_from_slice(more);
            }
        }
    }

    for c in 0..3 {
        // Weighted least squares of variance on level over the bins.
        let (mut sw, mut swx, mut swy, mut swxx, mut swxy) =
            (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for bin in bins[c].iter_mut() {
            if bin.len() < MIN_BLOCKS_PER_BIN {
                continue;
            }
            bin.sort_by(|a, b| a.1.total_cmp(&b.1));
            let quiet = &bin[..(bin.len() / 4).max(2)];
            let count = quiet.len() as f64;
            let mean = quiet.iter().map(|q| q.0 as f64).sum::<f64>() / count;
            let var = quiet.iter().map(|q| q.1 as f64).sum::<f64>() / count;
            let n = quiet.iter().map(|q| q.2).sum::<usize>() as f64 / count;
            // The quietest quarter of unbiased variance estimates with
            // relative spread sqrt(2/n) averages 1 - 1.271 sqrt(2/n) of
            // the truth (the mean of a normal's lowest quartile).
            let bias = 1.0 - 1.271 * (2.0 / n).sqrt();
            let var = var / bias.max(0.5);
            let w = bin.len() as f64;
            sw += w;
            swx += w * mean;
            swy += w * var;
            swxx += w * mean * mean;
            swxy += w * mean * var;
            bins_used[c] += 1;
        }
        if bins_used[c] == 0 {
            continue;
        }
        let det = sw * swxx - swx * swx;
        let (mut a, mut b) = if bins_used[c] >= 2 && det.abs() > 1e-18 {
            (
                (sw * swxy - swx * swy) / det,
                (swxx * swy - swx * swxy) / det,
            )
        } else {
            (0.0, swy / sw)
        };
        if a < 0.0 {
            a = 0.0;
            b = swy / sw;
        }
        if b < 0.0 {
            b = 0.0;
            a = if swxx > 0.0 { swxy / swxx } else { 0.0 };
        }
        model.a[c] = a as f32;
        model.b[c] = b as f32;
    }

    NoiseEstimate {
        model,
        blocks,
        bins: bins_used,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Deterministic standard normal samples.
    pub(crate) struct Gauss(pub u64);

    impl Gauss {
        fn uniform(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }

        pub(crate) fn uniform_public(&mut self) -> f64 {
            self.uniform()
        }

        pub(crate) fn next(&mut self) -> f32 {
            let (u1, u2) = (self.uniform(), self.uniform());
            ((-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()) as f32
        }
    }

    /// A mosaic of `truth` (one value per pixel) with the model's noise.
    pub(crate) fn noisy(
        truth: &[f32],
        pattern: &CfaPattern,
        w: usize,
        model: &NoiseModel,
        seed: u64,
    ) -> Vec<f32> {
        let mut g = Gauss(seed);
        truth
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let c = pattern.color_at(i / w, i % w).rgb_index().unwrap();
                (v + g.next() * model.sigma(c, v)).max(0.0)
            })
            .collect()
    }

    fn ramp(w: usize, h: usize) -> Vec<f32> {
        (0..w * h)
            .map(|i| 0.02 + 0.8 * (i % w) as f32 / w as f32)
            .collect()
    }

    #[test]
    fn model_follows_gains() {
        let m = NoiseModel {
            a: [1e-4, 2e-4, 3e-4],
            b: [1e-6, 2e-6, 3e-6],
        };
        let g = m.after_gains([2.0, 1.0, 4.0]);
        assert_eq!(g.a, [2e-4, 2e-4, 12e-4]);
        assert_eq!(g.b, [4e-6, 2e-6, 48e-6]);
        // The variance of a gained level agrees with the gained variance.
        let level = 0.3;
        let want = 4.0 * m.variance(0, level);
        assert!((g.variance(0, 2.0 * level) - want).abs() < 1e-9);
        assert!(NoiseModel::default().is_silent() && !m.is_silent());
    }

    #[test]
    fn recovers_a_known_model_from_a_ramp() {
        let (w, h) = (512, 384);
        let pattern = CfaPattern::rggb();
        let truth = NoiseModel {
            a: [4e-4, 2e-4, 6e-4],
            b: [4e-6, 2e-6, 8e-6],
        };
        let cfa = noisy(&ramp(w, h), &pattern, w, &truth, 7);
        let est = estimate_noise(&cfa, w, h, &pattern).unwrap();
        for c in 0..3 {
            assert!(est.bins[c] >= 8, "{est:?}");
            for level in [0.02, 0.1, 0.3, 0.7] {
                let (got, want) = (est.model.sigma(c, level), truth.sigma(c, level));
                assert!(
                    (got / want - 1.0).abs() < 0.12,
                    "channel {c} at {level}: sigma {got} vs {want}; {est:?}"
                );
            }
        }
    }

    #[test]
    fn a_clean_frame_is_silent_and_texture_is_survivable() {
        let (w, h) = (256, 192);
        let pattern = CfaPattern::rggb();
        let est = estimate_noise(&ramp(w, h), w, h, &pattern).unwrap();
        assert!(est.model.is_silent(), "{est:?}");

        // Fine texture on a third of the frame, noise everywhere: the
        // quiet blocks still carry the estimate.
        let truth = NoiseModel {
            a: [3e-4; 3],
            b: [3e-6; 3],
        };
        let textured: Vec<f32> = ramp(w, h)
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let (x, y) = (i % w, i / w);
                if y < h / 3 {
                    v * (1.0 + 0.3 * (((x / 2) + (y / 2)) % 2) as f32)
                } else {
                    v
                }
            })
            .collect();
        let cfa = noisy(&textured, &pattern, w, &truth, 11);
        let est = estimate_noise(&cfa, w, h, &pattern).unwrap();
        for c in 0..3 {
            let (got, want) = (est.model.sigma(c, 0.3), truth.sigma(c, 0.3));
            assert!(
                (got / want - 1.0).abs() < 0.25,
                "channel {c}: sigma {got} vs {want}; {est:?}"
            );
        }
    }

    /// The X-Trans estimate at several of the pattern's phases: a known
    /// model comes back from a noisy ramp, and the ramp without noise is
    /// silent, which a plain mean of a site's lopsided neighbors would
    /// not give (it reads the ramp a fraction of a pixel away).
    #[test]
    fn recovers_a_known_model_on_x_trans() {
        let (w, h) = (512, 384);
        let truth = NoiseModel {
            a: [4e-4, 2e-4, 6e-4],
            b: [4e-6, 2e-6, 8e-6],
        };
        for (dx, dy) in [(0, 0), (2, 2), (5, 1), (3, 4)] {
            let pattern = CfaPattern::xtrans().shifted(dx, dy);
            let clean = estimate_noise(&ramp(w, h), w, h, &pattern).unwrap();
            assert!(clean.model.is_silent(), "({dx}, {dy}): {clean:?}");
            let cfa = noisy(&ramp(w, h), &pattern, w, &truth, 7 + dx as u64);
            let est = estimate_noise(&cfa, w, h, &pattern).unwrap();
            // Red and blue are two sites in nine, against one in four
            // on Bayer, so the line's intercept, which carries the
            // estimate at the bottom of the ramp, scatters more there:
            // up to 20% over these phases and seeds, within 6% from
            // 0.1 up.
            for c in 0..3 {
                assert!(est.bins[c] >= 8, "{est:?}");
                for (level, within) in [(0.02, 0.25), (0.1, 0.12), (0.3, 0.12), (0.7, 0.12)] {
                    let (got, want) = (est.model.sigma(c, level), truth.sigma(c, level));
                    assert!(
                        (got / want - 1.0).abs() < within,
                        "({dx}, {dy}) channel {c} at {level}: sigma {got} vs {want}; {est:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn other_patterns_are_refused() {
        let quad = CfaPattern::new(
            4,
            4,
            [0, 0, 1, 1, 0, 0, 1, 1, 1, 1, 2, 2, 1, 1, 2, 2]
                .map(|c| match c {
                    0 => crate::raw::CfaColor::Red,
                    1 => crate::raw::CfaColor::Green,
                    _ => crate::raw::CfaColor::Blue,
                })
                .to_vec(),
        )
        .unwrap();
        assert!(!measures(&quad));
        assert!(estimate_noise(&vec![0.5; 64 * 64], 64, 64, &quad).is_err());
    }

    #[test]
    fn clipped_blocks_are_left_out() {
        let (w, h) = (128, 96);
        let pattern = CfaPattern::rggb();
        let cfa = vec![1.0f32; w * h];
        let est = estimate_noise(&cfa, w, h, &pattern).unwrap();
        assert_eq!(est.blocks, [0; 3]);
        assert!(est.model.is_silent());
    }

    /// A picture of `level` (one value a pixel, the same in its three
    /// channels) with noise drawn on the luma and chroma axes to
    /// `truth`: grain correlated between the channels.
    fn noisy_picture(level: &[f32], truth: &PictureNoise, seed: u64) -> Vec<f32> {
        let mut g = Gauss(seed);
        level
            .iter()
            .flat_map(|&v| {
                let n: [f32; 3] = std::array::from_fn(|k| g.next() * truth.axes.sigma(k, v));
                let add = super::super::denoise::from_yuv(n);
                [v + add[0], v + add[1], v + add[2]]
            })
            .collect()
    }

    /// Luma well above chroma, as a denoised frame has it.
    fn picture_truth() -> PictureNoise {
        PictureNoise {
            axes: NoiseModel {
                a: [9e-4, 1e-4, 2e-4],
                b: [9e-6, 1e-6, 2e-6],
            },
        }
    }

    /// The noise on each axis, as a straight variance over the pixels
    /// `pick` names.
    pub(crate) fn axis_spread(data: &[f32], pick: impl Fn(usize) -> bool) -> [f32; 3] {
        let yuv: Vec<[f32; 3]> = (0..data.len() / 3)
            .filter(|&i| pick(i))
            .map(|i| super::super::denoise::to_yuv([data[i * 3], data[i * 3 + 1], data[i * 3 + 2]]))
            .collect();
        let n = yuv.len() as f32;
        std::array::from_fn(|k| {
            let mean = yuv.iter().map(|p| p[k]).sum::<f32>() / n;
            yuv.iter().map(|p| (p[k] - mean).powi(2)).sum::<f32>() / n
        })
    }

    /// A picture's model comes back on each axis from a noisy ramp
    /// whose grain is correlated between its channels, read round a
    /// hole that is left out; the clean ramp is silent.
    #[test]
    fn recovers_a_known_model_from_a_picture_round_a_hole() {
        let (w, h) = (384, 256);
        let truth = picture_truth();
        let level = ramp(w, h);
        let mut data = noisy_picture(&level, &truth, 3);
        // The hole holds nonsense, which must not be read.
        let known: Vec<bool> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 - 192.0, (i / w) as f32 - 128.0);
                x * x + y * y > 60.0 * 60.0
            })
            .collect();
        for (i, k) in known.iter().enumerate() {
            if !k {
                data[i * 3..i * 3 + 3].copy_from_slice(&[5.0, -1.0, 0.0]);
            }
        }
        let est = estimate_picture_noise(&data, w, h, &known);
        for k in 0..3 {
            for level in [0.1, 0.3, 0.7] {
                let (got, want) = (est.axes.sigma(k, level), truth.axes.sigma(k, level));
                assert!(
                    (got / want - 1.0).abs() < 0.12,
                    "axis {k} at {level}: sigma {got} vs {want}; {est:?}"
                );
            }
        }
        let clean: Vec<f32> = level.iter().flat_map(|&v| [v; 3]).collect();
        assert!(estimate_picture_noise(&clean, w, h, &known).is_silent());
    }

    /// Smooth highlights past 1 in a third of the window, as a
    /// reconstruction makes them, do not pass for the quietest blocks
    /// and pull the mid tones' noise down.
    #[test]
    fn smooth_highlights_past_one_leave_the_mid_tones_alone() {
        let (w, h) = (384, 256);
        let truth = picture_truth();
        let level: Vec<f32> = (0..w * h)
            .map(|i| {
                let x = (i % w) as f32 / w as f32;
                if x < 2.0 / 3.0 {
                    0.02 + 1.3 * x
                } else {
                    // From 0.9 to 3, curved, so its residual is not 0.
                    let t = (x - 2.0 / 3.0) * 3.0;
                    0.9 + 2.1 * t * t
                }
            })
            .collect();
        let mut data = noisy_picture(&level, &truth, 5);
        for (i, &v) in level.iter().enumerate() {
            if v >= 0.9 && (i % w) as f32 / w as f32 >= 2.0 / 3.0 {
                data[i * 3..i * 3 + 3].copy_from_slice(&[v, v * 0.98, v * 1.01]);
            }
        }
        let est = estimate_picture_noise(&data, w, h, &vec![true; w * h]);
        for k in 0..3 {
            for level in [0.2, 0.5] {
                let (got, want) = (est.axes.sigma(k, level), truth.axes.sigma(k, level));
                assert!(
                    (got / want - 1.0).abs() < 0.15,
                    "axis {k} at {level}: sigma {got} vs {want}; {est:?}"
                );
            }
        }
    }

    /// The noise added has the model's spread less what is there on
    /// each axis, only under the mask, and is the same on a second
    /// call whatever the window.
    #[test]
    fn added_noise_has_the_models_spread_and_repeats() {
        let (w, h) = (200, 200);
        let model = PictureNoise {
            axes: NoiseModel {
                a: [0.0; 3],
                b: [9e-4, 1e-4, 2e-4],
            },
        };
        let present = PictureNoise {
            axes: NoiseModel {
                a: [0.0; 3],
                b: [3e-4, 0.0, 5e-4],
            },
        };
        let mask: Vec<f32> = (0..w * h).map(|i| (i % w >= 100) as u8 as f32).collect();
        let mut data = vec![0.5f32; w * h * 3];
        add_picture_noise(&mut data, w, (10, 20), &mask, &model, &present, 9);
        let var = axis_spread(&data, |i| mask[i] > 0.0);
        for k in 0..3 {
            let want = (model.axes.b[k] - present.axes.b[k]).max(0.0);
            assert!(
                (var[k] - want).abs() <= 0.05 * want + 1e-9,
                "axis {k}: {var:?}"
            );
        }
        assert!(
            (0..w * h)
                .filter(|&i| mask[i] == 0.0)
                .all(|i| data[i * 3..i * 3 + 3] == [0.5; 3])
        );
        // The right half again, as a window of its own.
        let mut half = vec![0.5f32; 100 * h * 3];
        add_picture_noise(
            &mut half,
            100,
            (110, 20),
            &vec![1.0; 100 * h],
            &model,
            &present,
            9,
        );
        for y in 0..h {
            for x in 0..100 {
                let (a, b) = ((y * w + 100 + x) * 3, (y * 100 + x) * 3);
                assert_eq!(data[a..a + 3], half[b..b + 3]);
            }
        }
    }
}
