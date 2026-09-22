//! Dual demosaic: a detail algorithm where there is detail, VNG4 where
//! there is not.
//!
//! Ported from RawTherapee's `rtengine/dual_demosaic_RT.cc` and the blend
//! mask in `rtengine/rt_algo.cc` (`buildBlendMask`, `calcBlendFactor`,
//! `calcContrastThreshold`, `tileAverage`, `tileVariance`), copyright 2018
//! Ingo Weyrich, GPL-3.0-or-later. The contrast measure, the sigmoid, the
//! automatic threshold search and its constants are his; the buffer layout
//! and the threading are ours.
//!
//! AMaZE and RCD resolve detail that VNG4 smears, and in return they turn
//! noise in flat areas into mazes and speckle that VNG4 does not. So: run
//! both, take the L* of the detail result, measure local contrast as the
//! root of the summed squared central differences at one and two pixels,
//! and blend by a sigmoid in that contrast whose midpoint is the contrast
//! threshold. Above the threshold the detail result wins, below it VNG4.
//! The mask is Gaussian-blurred (sigma 2) so the seam never shows.
//!
//! The threshold is a fraction of the reference's percent scale: RT's
//! default of 20 is [`DualContrast::Fixed`]`(0.2)`. Automatic mode finds
//! the flattest mid-tone tile in the image, assumes what contrast it has
//! is noise, and picks the threshold at which one percent of that tile
//! would still count as detail. On a clean image that is a low threshold
//! and the result is nearly all detail demosaic; on a noisy one it rises
//! with the noise.
//!
//! Lightness is kept in the reference's units, L* times 327.68, so the
//! constants read as they do in the source.

use rayon::prelude::*;

use super::noise::NoiseModel;
use super::{DemosaicMethod, demosaic_cfa, vng4};
use crate::error::{Error, Result};
use crate::raw::CfaPattern;

/// How much local contrast it takes before the detail demosaic is used.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum DualContrast {
    /// Whatever the pipeline can measure: [`super::develop`] estimates the
    /// frame's noise and uses [`DualContrast::Noise`]; called directly,
    /// [`demosaic_dual`] has no frame to measure and falls back to
    /// [`DualContrast::Tiles`].
    #[default]
    Auto,
    /// The reference's search: find the flattest mid-tone tile, take its
    /// contrast as noise, set the threshold from it.
    Tiles,
    /// From a noise model of the RGB the mask is built on (after white
    /// balance): the threshold at every pixel is the one the reference's
    /// rule would give a flat tile with that pixel's lightness and the
    /// model's noise there. Never gives up on a noisy frame, and follows
    /// the noise into the shadows.
    Noise(NoiseModel),
    /// A fixed threshold, the reference's percent scale divided by 100.
    /// Zero means the detail demosaic alone.
    Fixed(f32),
}

/// Where a dual demosaic's threshold came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdSource {
    Tiles,
    Noise,
    Fixed,
}

/// What the blend decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DualStats {
    /// The threshold used, as [`DualContrast::Fixed`] would take it; for
    /// [`DualContrast::Noise`], its value at mid grey.
    pub threshold: f32,
    pub source: ThresholdSource,
    /// The mean of `1 - blend`: the share of the image that came from VNG4.
    pub flat_fraction: f32,
}

/// The threshold over the image: one value, or one per unit of L*.
enum Threshold {
    Uniform(f32),
    ByLightness(Vec<f32>),
}

impl Threshold {
    /// The threshold at a pixel of lightness `l` in the reference's units.
    #[inline]
    fn at(&self, l: f32) -> f32 {
        match self {
            Threshold::Uniform(t) => *t,
            Threshold::ByLightness(table) => {
                let i = (l / L_SCALE).round().clamp(0.0, 100.0) as usize;
                table[i]
            }
        }
    }

    fn is_zero(&self) -> bool {
        match self {
            Threshold::Uniform(t) => *t == 0.0,
            Threshold::ByLightness(table) => table.iter().all(|t| *t == 0.0),
        }
    }
}

/// The reference's lightness scale: L* in 0..100 times this.
const L_SCALE: f32 = 327.68;
/// Contrast units: the reference divides the L* differences by 16.
const CONTRAST_SCALE: f32 = 0.0625 / L_SCALE;
/// Sigma of the blur over the blend mask.
const MASK_BLUR_SIGMA: f32 = 2.0;

/// Demosaic with `base`, then VNG4, and blend them by local contrast.
pub fn demosaic_dual(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    base: DemosaicMethod,
    contrast: DualContrast,
) -> Result<(Vec<f32>, DualStats)> {
    if !matches!(
        base,
        DemosaicMethod::Amaze | DemosaicMethod::Rcd | DemosaicMethod::Bilinear
    ) {
        return Err(Error::Unsupported(format!(
            "{} cannot be the detail half of a dual demosaic",
            base.name()
        )));
    }
    let mut out = demosaic_cfa(samples, width, height, pattern, base)?;

    let (threshold, source) = match contrast {
        DualContrast::Fixed(t) => (Threshold::Uniform(t.max(0.0)), ThresholdSource::Fixed),
        DualContrast::Auto | DualContrast::Tiles => (
            Threshold::Uniform(auto_threshold(
                &lightness(&out, width, height),
                width,
                height,
            )),
            ThresholdSource::Tiles,
        ),
        DualContrast::Noise(model) => (
            Threshold::ByLightness(noise_thresholds(&model)),
            ThresholdSource::Noise,
        ),
    };
    let reported = threshold.at(MID_GREY_L * L_SCALE);
    if threshold.is_zero() || width < 5 || height < 5 {
        return Ok((
            out,
            DualStats {
                threshold: reported,
                source,
                flat_fraction: 0.0,
            },
        ));
    }

    let blend = blend_mask(&lightness(&out, width, height), width, height, &threshold);
    // The soft half is blended in band by band as it is made, so the
    // frame is never held twice.
    let vng = vng4::Vng4::new(samples, width, height, pattern)?;
    out.par_chunks_mut(vng4::BAND * width * 3)
        .zip(blend.par_chunks(vng4::BAND * width))
        .enumerate()
        .for_each(|(k, (rows, blend))| {
            let y0 = k * vng4::BAND;
            let mut flat = vec![0.0f32; rows.len()];
            vng.rows(y0, y0 + rows.len() / (width * 3), &mut flat);
            for ((px, soft), &b) in rows
                .as_chunks_mut::<3>()
                .0
                .iter_mut()
                .zip(flat.as_chunks::<3>().0)
                .zip(blend)
            {
                for (v, s) in px.iter_mut().zip(soft) {
                    *v = intp(b, *v, *s);
                }
            }
        });
    let flat_fraction = 1.0 - blend.iter().sum::<f32>() / blend.len() as f32;
    Ok((
        out,
        DualStats {
            threshold: reported,
            source,
            flat_fraction,
        },
    ))
}

/// L* of mid grey.
const MID_GREY_L: f32 = 49.5;

/// The threshold the reference's rule gives a flat tile whose noise, in
/// L*, is what `model` says at each lightness from 0 to 100: the smallest
/// hundredth at which no more than one percent of pure noise reads as
/// detail, plus one hundredth, as [`tile_threshold`] does.
///
/// Four central differences of independent noise sum to `2 sigma^2`
/// times a chi-squared with four degrees of freedom, so the mean blend at
/// a threshold is an integral over that density. Independence is not
/// quite true after a demosaic, which errs on the side of VNG4.
fn noise_thresholds(model: &NoiseModel) -> Vec<f32> {
    // The model is the mosaic's noise; the mask is built on demosaiced
    // RGB, where the channels at a pixel share the noise of the sample
    // that was there (a color-difference interpolation carries the green
    // sample's noise into the red and blue it fills in). So Y's deviation
    // is the weighted sum of the channel deviations, not the root of the
    // weighted sum of variances: the fully correlated case, which is the
    // noisiest site type and the one the one-percent rule has to cover.
    let weights = [0.212671f32, 0.715160, 0.072169];
    let y_variance = |y: f32| -> f32 {
        let sigma: f32 = (0..3).map(|c| weights[c] * model.sigma(c, y)).sum();
        sigma * sigma
    };
    (0..=100)
        .map(|l| {
            let l = l as f32;
            let (y, slope) = if l > 8.0 {
                let f = (l + 16.0) / 116.0;
                let y = f * f * f;
                (y, 116.0 / 3.0 * y.powf(-2.0 / 3.0))
            } else {
                (l * 27.0 / 24389.0, 24389.0 / 27.0)
            };
            let sigma_l = slope * y_variance(y).sqrt();
            threshold_for_noise(sigma_l)
        })
        .collect()
}

/// The reference's rule for a flat tile with Gaussian noise of standard
/// deviation `sigma_l` in L*.
fn threshold_for_noise(sigma_l: f32) -> f32 {
    if sigma_l <= 0.0 {
        return 0.02;
    }
    // Chi-squared with four degrees of freedom: pdf(s) = s exp(-s/2) / 4.
    const STEPS: usize = 400;
    let ds = 60.0 / STEPS as f32;
    let mean_blend = |t: f32| -> f32 {
        (0..STEPS)
            .map(|i| {
                let s = (i as f32 + 0.5) * ds;
                let pdf = s * (-s / 2.0).exp() / 4.0;
                let contrast = sigma_l * (2.0 * s).sqrt() / 16.0;
                pdf * blend_factor(contrast, t) * ds
            })
            .sum()
    };
    let mut c = 1;
    while c < 100 {
        if mean_blend(c as f32 / 100.0) <= 0.01 {
            break;
        }
        c += 1;
    }
    (c + 1) as f32 / 100.0
}

/// CIE L* of interleaved RGB, in the reference's units. The RGB is
/// white-balanced camera space, taken as if it were sRGB, as the reference
/// does: the mask only needs something monotone in brightness.
fn lightness(rgb: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut l = vec![0.0f32; width * height];
    l.par_iter_mut().zip(rgb.par_chunks(3)).for_each(|(l, px)| {
        let y = 0.212671 * px[0] + 0.715160 * px[1] + 0.072169 * px[2];
        let f = if y > 216.0 / 24389.0 {
            y.max(0.0).cbrt()
        } else {
            (24389.0 / 27.0 * y + 16.0) / 116.0
        };
        *l = (116.0 * f - 16.0) * L_SCALE;
    });
    l
}

/// The sigmoid in contrast: 0.5 at the threshold, near 0 at zero contrast,
/// near 1 at twice the threshold.
fn blend_factor(contrast: f32, threshold: f32) -> f32 {
    let x = -16.0 + (16.0 / threshold) * contrast;
    0.5 * (1.0 + x / (1.0 + x * x).sqrt())
}

/// Local contrast two pixels in from the edge.
#[inline]
fn contrast_at(l: &[f32], width: usize, y: usize, x: usize) -> f32 {
    let at = |yy: usize, xx: usize| l[yy * width + xx];
    (sq(at(y, x + 1) - at(y, x - 1))
        + sq(at(y + 1, x) - at(y - 1, x))
        + sq(at(y, x + 2) - at(y, x - 2))
        + sq(at(y + 2, x) - at(y - 2, x)))
    .sqrt()
        * CONTRAST_SCALE
}

/// The blend mask: the sigmoid of local contrast, edges copied from the
/// nearest measured pixel, blurred.
fn blend_mask(l: &[f32], width: usize, height: usize, threshold: &Threshold) -> Vec<f32> {
    let mut blend = vec![1.0f32; width * height];
    blend
        .par_chunks_mut(width)
        .enumerate()
        .skip(2)
        .take(height - 4)
        .for_each(|(y, row)| {
            for (x, b) in row.iter_mut().enumerate().skip(2).take(width - 4) {
                let t = threshold.at(l[y * width + x]);
                *b = if t > 0.0 {
                    blend_factor(contrast_at(l, width, y, x), t)
                } else {
                    1.0
                };
            }
        });
    for x in 2..width - 2 {
        for y in 0..2 {
            blend[y * width + x] = blend[2 * width + x];
            blend[(height - 1 - y) * width + x] = blend[(height - 3) * width + x];
        }
    }
    for y in 0..height {
        let row = &mut blend[y * width..(y + 1) * width];
        row[0] = row[2];
        row[1] = row[2];
        row[width - 1] = row[width - 3];
        row[width - 2] = row[width - 3];
    }
    gaussian_blur(&mut blend, width, height, MASK_BLUR_SIGMA);
    blend
}

/// The reference's automatic threshold: search for the flattest mid-tone
/// tile, first on an 80-pixel grid, then with 40-pixel tiles at a finer
/// step and a pixel-step scan around the best of those, and read the
/// threshold off it. Zero when nothing flat enough exists.
fn auto_threshold(l: &[f32], width: usize, height: usize) -> f32 {
    const MIN_LUMINANCE: f32 = 2000.0;
    const MAX_LUMINANCE: f32 = 20000.0;
    const MIN_TILE_VARIANCE: f32 = 0.5;

    let variance_of = |ty: usize, tx: usize, size: usize| -> f32 {
        let n = (size * size) as f32;
        let mut avg = 0.0f32;
        for y in ty..ty + size {
            avg += l[y * width + tx..y * width + tx + size].iter().sum::<f32>();
        }
        avg /= n;
        if !(MIN_LUMINANCE..=MAX_LUMINANCE).contains(&avg) {
            return f32::INFINITY;
        }
        let mut var = 0.0f32;
        for y in ty..ty + size {
            var += l[y * width + tx..y * width + tx + size]
                .iter()
                .map(|v| sq(v - avg))
                .sum::<f32>();
        }
        let var = var / (n * avg);
        if var < MIN_TILE_VARIANCE {
            f32::INFINITY
        } else {
            var
        }
    };
    let flattest = |origins: &[(usize, usize)], size: usize| -> (f32, usize, usize) {
        origins
            .par_iter()
            .map(|&(ty, tx)| (variance_of(ty, tx, size), ty, tx))
            .reduce(
                || (f32::INFINITY, 0, 0),
                |a, b| if b.0 < a.0 { b } else { a },
            )
    };

    for pass in 0..2usize {
        let size = 80 / (pass + 1);
        let skip = if pass == 0 { size } else { size / 4 };
        let tiles_w = (width / skip).saturating_sub(3 * pass);
        let tiles_h = (height / skip).saturating_sub(3 * pass);
        if tiles_w == 0 || tiles_h == 0 || width < size || height < size {
            continue;
        }
        let origins: Vec<(usize, usize)> = (0..tiles_h)
            .flat_map(|i| (0..tiles_w).map(move |j| (i * skip, j * skip)))
            .collect();
        let (minvar, min_y, min_x) = flattest(&origins, size);
        if pass == 0 {
            if minvar <= 1.0 {
                // Flat already: no second pass.
                return tile_threshold(l, width, min_y, min_x, size);
            }
            continue;
        }
        // Second pass: allow a variance of 8, and scan every origin within
        // `skip` of the best tile for a better hit.
        let y0 = min_y.saturating_sub(skip);
        let x0 = min_x.saturating_sub(skip);
        let y1 = (min_y + skip).min(height - size);
        let x1 = (min_x + skip).min(width - size);
        if y1 < y0 || x1 < x0 {
            return 0.0;
        }
        let origins: Vec<(usize, usize)> = (y0..=y1)
            .flat_map(|ty| (x0..=x1).map(move |tx| (ty, tx)))
            .collect();
        let (minvar, min_y, min_x) = flattest(&origins, size);
        return if minvar <= 8.0 {
            tile_threshold(l, width, min_y, min_x, size)
        } else {
            0.0
        };
    }
    0.0
}

/// The smallest threshold, in steps of 0.01, at which no more than one
/// percent of the tile's interior still counts as detail, plus one step.
fn tile_threshold(l: &[f32], width: usize, ty: usize, tx: usize, size: usize) -> f32 {
    let contrasts: Vec<f32> = (ty + 2..ty + size - 2)
        .flat_map(|y| (tx + 2..tx + size - 2).map(move |x| contrast_at(l, width, y, x)))
        .collect();
    let limit = contrasts.len() as f32 / 100.0;
    let mut c = 1;
    while c < 100 {
        let threshold = c as f32 / 100.0;
        let sum: f32 = contrasts.iter().map(|&v| blend_factor(v, threshold)).sum();
        if sum <= limit {
            break;
        }
        c += 1;
    }
    (c + 1) as f32 / 100.0
}

/// Separable Gaussian blur, edges clamped.
pub(crate) fn gaussian_blur(data: &mut [f32], width: usize, height: usize, sigma: f32) {
    let radius = (3.0 * sigma).ceil() as usize;
    let mut kernel: Vec<f32> = (0..=radius)
        .map(|i| (-(i as f32 * i as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let norm = kernel[0] + 2.0 * kernel[1..].iter().sum::<f32>();
    for k in &mut kernel {
        *k /= norm;
    }
    let taps = |i: isize, n: usize| i.clamp(0, n as isize - 1) as usize;

    // Both passes take the taps in the same order as a clamped index
    // would, so the sums are to the bit what the plain loop gave; what
    // changes is that the interior of a row never clamps, and the column
    // pass walks whole rows instead of a column at a time.
    let clamped = |src: &[f32], x: usize| {
        let mut acc = kernel[0] * src[x];
        for (k, &w) in kernel.iter().enumerate().skip(1) {
            let k = k as isize;
            acc += w * (src[taps(x as isize - k, width)] + src[taps(x as isize + k, width)]);
        }
        acc
    };

    let mut tmp = vec![0.0f32; width * height];
    tmp.par_chunks_mut(width)
        .zip(data.par_chunks(width))
        .for_each(|(dst, src)| {
            if width < 2 * radius + 1 {
                for (x, d) in dst.iter_mut().enumerate() {
                    *d = clamped(src, x);
                }
                return;
            }
            for (x, d) in dst.iter_mut().enumerate().take(radius) {
                *d = clamped(src, x);
            }
            for (d, win) in dst[radius..width - radius]
                .iter_mut()
                .zip(src.windows(2 * radius + 1))
            {
                let mut acc = kernel[0] * win[radius];
                for (k, &w) in kernel.iter().enumerate().skip(1) {
                    acc += w * (win[radius - k] + win[radius + k]);
                }
                *d = acc;
            }
            for (x, d) in dst.iter_mut().enumerate().skip(width - radius) {
                *d = clamped(src, x);
            }
        });
    data.par_chunks_mut(width).enumerate().for_each(|(y, dst)| {
        let row = |i: isize| {
            let r = taps(i, height);
            &tmp[r * width..r * width + width]
        };
        for (d, s) in dst.iter_mut().zip(row(y as isize)) {
            *d = kernel[0] * s;
        }
        for (k, &w) in kernel.iter().enumerate().skip(1) {
            let (above, below) = (row(y as isize - k as isize), row(y as isize + k as isize));
            for ((d, a), b) in dst.iter_mut().zip(above).zip(below) {
                *d += w * (a + b);
            }
        }
    });
}

#[inline]
fn sq(x: f32) -> f32 {
    x * x
}

/// `a * b + (1 - a) * c`.
#[inline]
fn intp(a: f32, b: f32, c: f32) -> f32 {
    a * (b - c) + c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::CfaColor;

    fn mosaic_of(rgb: &[f32], w: usize, h: usize, pattern: &CfaPattern) -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                let c = pattern.color_at(i / w, i % w).rgb_index().unwrap();
                rgb[i * 3 + c]
            })
            .collect()
    }

    /// Deterministic noise in -1..1.
    fn noise(seed: &mut u64) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
    }

    #[test]
    fn sigmoid_sits_on_the_threshold() {
        assert!((blend_factor(0.2, 0.2) - 0.5).abs() < 1e-6);
        assert!(blend_factor(0.0, 0.2) < 0.001);
        assert!(blend_factor(0.4, 0.2) > 0.998);
        assert!(blend_factor(0.1, 0.2) < blend_factor(0.15, 0.2));
    }

    /// The blur takes its taps in the order a clamped index would, so
    /// splitting the interior from the edges and walking whole rows in the
    /// column pass may not move a bit. These are the hashes the plain
    /// double loop gave: one image wider than the kernel, one narrower.
    #[test]
    fn the_blur_is_bit_for_bit_the_clamped_loop() {
        let fnv = |v: &[f32]| {
            let mut h = 0xcbf29ce484222325u64;
            for x in v {
                h ^= x.to_bits() as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            h
        };
        let (w, h) = (97, 71);
        let mut data: Vec<f32> = (0..w * h)
            .map(|i| ((i * 7919) % 1000) as f32 / 1000.0)
            .collect();
        gaussian_blur(&mut data, w, h, 2.0);
        assert_eq!(fnv(&data), 0x0e4dbf112561c5de);
        let mut narrow: Vec<f32> = (0..8 * 40).map(|i| (i % 5) as f32 * 0.25).collect();
        gaussian_blur(&mut narrow, 8, 40, 2.0);
        assert_eq!(fnv(&narrow), 0x18bad06bc5706187);
    }

    #[test]
    fn blur_keeps_a_constant_and_spreads_a_spike() {
        let (w, h) = (20, 15);
        let mut flat = vec![0.7f32; w * h];
        gaussian_blur(&mut flat, w, h, 2.0);
        assert!(flat.iter().all(|v| (v - 0.7).abs() < 1e-5));
        let mut spike = vec![0.0f32; w * h];
        spike[7 * w + 10] = 1.0;
        gaussian_blur(&mut spike, w, h, 2.0);
        let total: f32 = spike.iter().sum();
        assert!((total - 1.0).abs() < 1e-4, "{total}");
        assert!(spike[7 * w + 10] < 0.05 && spike[7 * w + 10] > spike[7 * w + 12]);
    }

    #[test]
    fn lightness_is_l_star() {
        let l = lightness(&[1.0, 1.0, 1.0, 0.18, 0.18, 0.18, 0.0, 0.0, 0.0], 3, 1);
        assert!((l[0] / L_SCALE - 100.0).abs() < 0.01, "{}", l[0]);
        assert!((l[1] / L_SCALE - 49.5).abs() < 0.1, "{}", l[1]);
        assert!(l[2].abs() < 1e-3);
    }

    #[test]
    fn zero_threshold_is_the_base_alone() {
        let (w, h) = (48, 40);
        let pattern = CfaPattern::rggb();
        let cfa: Vec<f32> = (0..w * h)
            .map(|i| ((i * 7919) % 1000) as f32 / 1000.0)
            .collect();
        let base = demosaic_cfa(&cfa, w, h, &pattern, DemosaicMethod::Rcd).unwrap();
        let (out, stats) = demosaic_dual(
            &cfa,
            w,
            h,
            &pattern,
            DemosaicMethod::Rcd,
            DualContrast::Fixed(0.0),
        )
        .unwrap();
        assert_eq!(out, base);
        assert_eq!(stats.flat_fraction, 0.0);
        assert!(
            demosaic_dual(
                &cfa,
                w,
                h,
                &pattern,
                DemosaicMethod::Vng4,
                DualContrast::Auto
            )
            .is_err()
        );
    }

    #[test]
    fn detail_where_there_is_detail_and_vng4_where_there_is_noise() {
        // Left half: mid grey with noise. Right half: hard stripes. The
        // measured threshold should come from the noise, so the left is
        // VNG4 and the right the base.
        let (w, h) = (240, 120);
        let pattern = CfaPattern::rggb();
        let mut seed = 0x9E3779B97F4A7C15u64;
        let rgb: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let x = i % w;
                if x < w / 2 {
                    let v = 0.18 + 0.01 * noise(&mut seed);
                    [v, v, v]
                } else if (x / 2) % 2 == 0 {
                    [0.05, 0.05, 0.05]
                } else {
                    [0.6, 0.6, 0.6]
                }
            })
            .collect();
        let cfa = mosaic_of(&rgb, w, h, &pattern);
        let base = demosaic_cfa(&cfa, w, h, &pattern, DemosaicMethod::Amaze).unwrap();
        let soft = vng4::demosaic_vng4(&cfa, w, h, &pattern).unwrap();
        let (out, stats) = demosaic_dual(
            &cfa,
            w,
            h,
            &pattern,
            DemosaicMethod::Amaze,
            DualContrast::Auto,
        )
        .unwrap();
        assert!(
            stats.source == ThresholdSource::Tiles && stats.threshold > 0.0,
            "{stats:?}"
        );
        assert!(
            stats.flat_fraction > 0.4 && stats.flat_fraction < 0.6,
            "{stats:?}"
        );
        let diff = |a: &[f32], b: &[f32], x0: usize, x1: usize| -> f32 {
            let mut d = 0.0f32;
            for y in 10..h - 10 {
                for x in x0..x1 {
                    for c in 0..3 {
                        d = d.max((a[(y * w + x) * 3 + c] - b[(y * w + x) * 3 + c]).abs());
                    }
                }
            }
            d
        };
        assert!(diff(&out, &soft, 10, w / 2 - 20) < 1e-3, "left is not VNG4");
        assert!(
            diff(&out, &base, w / 2 + 20, w - 10) < 1e-3,
            "right is not the base"
        );
        assert!(
            diff(&base, &soft, 10, w / 2 - 20) > 1e-3,
            "the two halves must differ for the test to mean anything"
        );
    }

    #[test]
    fn noise_thresholds_follow_the_noise() {
        // No noise: the floor. More noise: a higher threshold. The rule
        // reproduces the tile search on a flat noisy tile.
        assert_eq!(threshold_for_noise(0.0), 0.02);
        let quiet = threshold_for_noise(1.0);
        let loud = threshold_for_noise(4.0);
        assert!(quiet < loud, "{quiet} {loud}");
        let table = noise_thresholds(&NoiseModel {
            a: [1e-4; 3],
            b: [1e-6; 3],
        });
        assert_eq!(table.len(), 101);
        assert!(
            table[20] > table[80],
            "shadows are noisier in L*: {table:?}"
        );

        // A flat tile of Gaussian noise at mid grey through the reference's
        // search must land within a step or two of the rule.
        let (w, h) = (120, 120);
        let sigma_l = 2.0f32;
        let mut seed = 0x1234_5678_9ABC_DEF0u64;
        let l: Vec<f32> = (0..w * h)
            .map(|_| (MID_GREY_L + sigma_l * gaussian(&mut seed)) * L_SCALE)
            .collect();
        let searched = tile_threshold(&l, w, 0, 0, 80);
        let ruled = threshold_for_noise(sigma_l);
        assert!(
            (searched - ruled).abs() <= 0.02 + 1e-6,
            "search {searched} vs rule {ruled}"
        );
    }

    #[test]
    fn noise_model_threshold_splits_the_scene_too() {
        let (w, h) = (240, 120);
        let pattern = CfaPattern::rggb();
        let model = NoiseModel {
            a: [4e-4; 3],
            b: [4e-6; 3],
        };
        let mut seed = 0x9E3779B97F4A7C15u64;
        let rgb: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let x = i % w;
                if x < w / 2 {
                    let v = 0.18 + model.sigma(1, 0.18) * gaussian(&mut seed);
                    [v, v, v]
                } else if (x / 2) % 2 == 0 {
                    [0.05, 0.05, 0.05]
                } else {
                    [0.6, 0.6, 0.6]
                }
            })
            .collect();
        let cfa = mosaic_of(&rgb, w, h, &pattern);
        let base = demosaic_cfa(&cfa, w, h, &pattern, DemosaicMethod::Amaze).unwrap();
        let soft = vng4::demosaic_vng4(&cfa, w, h, &pattern).unwrap();
        {
            // How the demosaiced flat field's L* noise compares with what
            // the table assumed.
            let l = lightness(&base, w, h);
            let flat: Vec<f32> = (10..h - 10)
                .flat_map(|y| (10..w / 2 - 20).map(move |x| (y, x)))
                .map(|(y, x)| l[y * w + x] / L_SCALE)
                .collect();
            let mean = flat.iter().sum::<f32>() / flat.len() as f32;
            let std = (flat.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>()
                / flat.len() as f32)
                .sqrt();
            let ruled = noise_thresholds(&model)[mean.round() as usize];
            let searched = tile_threshold(&l, w, 10, 10, 80);
            assert!(
                (ruled - searched).abs() <= 0.05,
                "L* std {std}: rule {ruled} vs search {searched}"
            );
        }
        let (out, stats) = demosaic_dual(
            &cfa,
            w,
            h,
            &pattern,
            DemosaicMethod::Amaze,
            DualContrast::Noise(model),
        )
        .unwrap();
        assert_eq!(stats.source, ThresholdSource::Noise);
        assert!(
            stats.flat_fraction > 0.4 && stats.flat_fraction < 0.6,
            "{stats:?}"
        );
        let diff = |a: &[f32], b: &[f32], x0: usize, x1: usize| -> f32 {
            let mut d = 0.0f32;
            for y in 10..h - 10 {
                for x in x0..x1 {
                    for c in 0..3 {
                        d = d.max((a[(y * w + x) * 3 + c] - b[(y * w + x) * 3 + c]).abs());
                    }
                }
            }
            d
        };
        assert!(diff(&out, &soft, 10, w / 2 - 20) < 1e-3, "left is not VNG4");
        assert!(
            diff(&out, &base, w / 2 + 20, w - 10) < 1e-3,
            "right is not the base"
        );
    }

    /// Deterministic standard normal.
    fn gaussian(seed: &mut u64) -> f32 {
        let (u1, u2) = ((noise(seed) + 1.0) * 0.5, (noise(seed) + 1.0) * 0.5);
        let u1 = u1.max(1e-7);
        ((-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()).clamp(-5.0, 5.0)
    }

    #[test]
    fn rejects_non_bayer() {
        let p = CfaPattern::new(2, 2, vec![CfaColor::Red; 4]).unwrap();
        assert!(
            demosaic_dual(
                &[0.5; 16],
                4,
                4,
                &p,
                DemosaicMethod::Amaze,
                DualContrast::Auto
            )
            .is_err()
        );
    }
}
