//! Hot and dead photosites, repaired on the mosaic.
//!
//! A photosite that reads far above every same-color neighbor is a
//! defect, not a picture: the demosaic would spread it into a colored
//! dot, and neither denoiser removes it, since nothing around it agrees
//! with it. The rule follows darktable's `hotpixels.c` (copyright
//! 2011-2024 darktable developers, GPL-3.0-or-later): compare with the
//! same-color neighbors two photosites away and replace a hot one
//! with the brightest of them, which the reference notes gives the
//! fewest artifacts when the pixel was not hot after all. Two things
//! are ours: the eight neighbors rather than four, and the threshold,
//! which is not a fixed level but a number of standard deviations of
//! the frame's measured noise at the neighbors' level, so the same
//! setting means the same thing at ISO 100 and ISO 32000.
//!
//! Off by default, and not a candidate for on. Anything at most two
//! photosites across is a hot pixel to this filter and to every filter
//! of its kind: a star, a specular glint, the catchlight in an eye (a
//! portrait in the test set lost the color of its catchlight to it),
//! a bright or dark speck of texture. What separates a defect from a
//! picture is that the defect is in the same place in every frame; the
//! design that gets this right is a per-camera defect map from dark
//! frames, applied without judgment (notes §13s).

use rayon::prelude::*;

use super::noise::NoiseModel;
pub use super::rcd::is_bayer;
use crate::error::{Error, Result};
use crate::raw::CfaPattern;

/// How far out a photosite has to be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HotPixelOptions {
    /// Standard deviations of the noise beyond the brightest (or below
    /// the darkest) same-color neighbor. This is what keeps noise
    /// from counting at high ISO.
    pub sigmas: f32,
    /// The factor a photosite must exceed its brightest same-color
    /// neighbor by (a dead one, fall below its darkest by). This is
    /// what keeps fine texture from counting at base ISO, where the
    /// noise is a fraction of a percent and detail is not.
    pub ratio: f32,
}

impl Default for HotPixelOptions {
    fn default() -> Self {
        Self {
            sigmas: DEFAULT_SIGMAS,
            ratio: DEFAULT_RATIO,
        }
    }
}

/// Six sigmas: the noise's tail never gets there over a frame, on the
/// samples here, and a defect is far beyond it.
pub const DEFAULT_SIGMAS: f32 = 6.0;
/// The reference tests at four thirds against four neighbors. Against
/// eight and with the sigma test beside it, three is where real frames
/// keep a few dozen candidates, and even those include catchlights and
/// specks of texture (notes §13s); lower is for frames known to be
/// full of defects.
pub const DEFAULT_RATIO: f32 = 3.0;

/// What the repair did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HotPixelStats {
    /// Photosites above all their neighbors, replaced by the brightest.
    pub hot: usize,
    /// Photosites below all their neighbors, replaced by the darkest.
    pub dead: usize,
}

/// Repair `samples` (one per photosite, levels normalized, no gains
/// applied) in place. `model` is the noise of the samples as they are.
pub fn fix_hot_pixels(
    samples: &mut [f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    model: &NoiseModel,
    options: &HotPixelOptions,
) -> Result<HotPixelStats> {
    if !is_bayer(pattern) {
        return Err(Error::Unsupported(format!(
            "hot pixel repair needs a 2x2 Bayer pattern, got {pattern}"
        )));
    }
    if samples.len() != width * height {
        return Err(Error::Unsupported(
            "hot pixels: sample buffer size mismatch".into(),
        ));
    }
    if width < 5 || height < 5 || options.sigmas <= 0.0 {
        return Ok(HotPixelStats::default());
    }
    let k = options.sigmas;
    let ratio = options.ratio.max(1.0);
    let color: [[usize; 2]; 2] = std::array::from_fn(|y| {
        std::array::from_fn(|x| {
            pattern
                .color_at(y, x)
                .rgb_index()
                .expect("Bayer pattern is RGB")
        })
    });
    // Decisions on the values as shot, then the replacements.
    let fixes: Vec<(usize, f32, bool)> = (2..height - 2)
        .into_par_iter()
        .flat_map_iter(|y| {
            let mut row = Vec::new();
            for x in 2..width - 2 {
                let v = samples[y * width + x];
                let mut mx = f32::NEG_INFINITY;
                let mut mn = f32::INFINITY;
                for dy in [-2isize, 0, 2] {
                    for dx in [-2isize, 0, 2] {
                        if dy == 0 && dx == 0 {
                            continue;
                        }
                        let n = samples
                            [(y as isize + dy) as usize * width + (x as isize + dx) as usize];
                        mx = mx.max(n);
                        mn = mn.min(n);
                    }
                }
                let c = color[y & 1][x & 1];
                if v - mx > k * model.sigma(c, mx.max(0.0)) && v > mx * ratio {
                    row.push((y * width + x, mx, true));
                } else if mn - v > k * model.sigma(c, mn.max(0.0)) && v * ratio < mn {
                    row.push((y * width + x, mn, false));
                }
            }
            row
        })
        .collect();
    let mut stats = HotPixelStats::default();
    for &(i, value, hot) in &fixes {
        samples[i] = value;
        if hot {
            stats.hot += 1;
        } else {
            stats.dead += 1;
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::noise::tests::Gauss;

    fn field(w: usize, h: usize, level: f32, model: &NoiseModel, seed: u64) -> Vec<f32> {
        let mut g = Gauss(seed);
        (0..w * h)
            .map(|i| level + g.next() * model.sigma(i % 2, level))
            .collect()
    }

    #[test]
    fn finds_the_outliers_and_nothing_else() {
        let (w, h) = (128, 96);
        let model = NoiseModel {
            a: [2e-3; 3],
            b: [1e-5; 3],
        };
        let mut samples = field(w, h, 0.4, &model, 5);
        let sigma = model.sigma(1, 0.4);
        let hot = [(10, 11), (50, 77), (93, 3), (40, 40)];
        let dead = [(20, 64), (70, 100)];
        // Defects: several times the level, or a small fraction of it,
        // which is many sigmas either way.
        for &(y, x) in &hot {
            samples[y * w + x] = 2.0;
        }
        for &(y, x) in &dead {
            samples[y * w + x] = 0.02;
        }
        let before = samples.clone();
        let stats = fix_hot_pixels(
            &mut samples,
            w,
            h,
            &CfaPattern::rggb(),
            &model,
            &HotPixelOptions::default(),
        )
        .unwrap();
        assert_eq!(stats, HotPixelStats { hot: 4, dead: 2 });
        for &(y, x) in hot.iter().chain(&dead) {
            let v = samples[y * w + x];
            assert!((v - 0.4).abs() < 4.0 * sigma, "({x},{y}) -> {v}");
        }
        let changed = samples.iter().zip(&before).filter(|(a, b)| a != b).count();
        assert_eq!(changed, 6, "nothing else touched");
    }

    #[test]
    fn a_small_highlight_survives_and_a_dot_does_not() {
        let (w, h) = (64, 64);
        let model = NoiseModel {
            a: [1e-3; 3],
            b: [1e-5; 3],
        };
        let mut samples = field(w, h, 0.1, &model, 9);
        // A 5x5 highlight: every photosite in it has a same-color
        // neighbor two away that is also in it.
        for y in 20..25 {
            for x in 20..25 {
                samples[y * w + x] = 0.9;
            }
        }
        // A single bright photosite.
        samples[40 * w + 40] = 0.9;
        let stats = fix_hot_pixels(
            &mut samples,
            w,
            h,
            &CfaPattern::rggb(),
            &model,
            &HotPixelOptions::default(),
        )
        .unwrap();
        assert_eq!(stats, HotPixelStats { hot: 1, dead: 0 });
        assert!(samples[40 * w + 40] < 0.2);
        for y in 20..25 {
            for x in 20..25 {
                assert_eq!(samples[y * w + x], 0.9, "({x},{y})");
            }
        }
    }
}
