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
//! that is 1.25 times the variance, and texture only adds to it. Blocks
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

/// Estimate the noise model of a Bayer mosaic. `samples` are levels
/// normalized to 0..1, before white balance. A frame with no unclipped
/// blocks gives a silent model, not an error.
pub fn estimate_noise(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
) -> Result<NoiseEstimate> {
    if !is_bayer(pattern) {
        return Err(Error::Unsupported(format!(
            "noise estimation needs a 2x2 Bayer pattern, got {pattern}"
        )));
    }
    let mut model = NoiseModel::default();
    let mut blocks = [0usize; 3];
    let mut bins_used = [0usize; 3];
    if width < BLOCK + 4 || height < BLOCK + 4 {
        return Ok(NoiseEstimate {
            model,
            blocks,
            bins: bins_used,
        });
    }

    // Per channel, per bin: (mean level, noise variance estimate) per block.
    // Block rows in parallel, each with its own bins, merged in order.
    let at = |y: usize, x: usize| samples[y * width + x];
    let block_rows: Vec<usize> = (2..height - 2 - BLOCK + 1).step_by(BLOCK).collect();
    let rows: Vec<(Bins, [usize; 3])> = block_rows
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
                        let v = at(y, x);
                        if v >= CLIP {
                            clipped = true;
                            break 'block;
                        }
                        let c = pattern
                            .color_at(y, x)
                            .rgb_index()
                            .expect("Bayer patterns are RGB");
                        let neighbors =
                            (at(y, x - 2) + at(y, x + 2) + at(y - 2, x) + at(y + 2, x)) * 0.25;
                        let r = v - neighbors;
                        sum[c] += v as f64;
                        sq[c] += (r * r) as f64;
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
        .collect();
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

    Ok(NoiseEstimate {
        model,
        blocks,
        bins: bins_used,
    })
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

    #[test]
    fn clipped_blocks_are_left_out() {
        let (w, h) = (128, 96);
        let pattern = CfaPattern::rggb();
        let cfa = vec![1.0f32; w * h];
        let est = estimate_noise(&cfa, w, h, &pattern).unwrap();
        assert_eq!(est.blocks, [0; 3]);
        assert!(est.model.is_silent());
    }
}
