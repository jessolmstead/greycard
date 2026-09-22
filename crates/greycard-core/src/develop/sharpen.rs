//! Capture sharpening: Richardson–Lucy deconvolution of the luminance
//! under a Gaussian point spread, blended in by local contrast so that
//! flat areas keep their noise as it was, with the point spread's
//! width measured on the mosaic.
//!
//! A port of RawTherapee's capture sharpening: `CaptureDeconvSharpening`
//! and `calcRadiusBayer` in `rtengine/capturesharpening.cc` and
//! `buildBlendMask`, `calcContrastThreshold` and `calcBlendFactor` in
//! `rtengine/rt_algo.cc`, copyright 2019 Ingo Weyrich, GPL-3.0-or-later.
//! Differences: the Gaussian is applied as two one-dimensional passes,
//! which its truncated square kernel is exactly; tiles at the picture's
//! edge clamp their reads so every pixel is sharpened; the clip mask is
//! taken on the working image; and units are the working space's
//! (luminance relative to one, L* out of a hundred) rather than
//! RawTherapee's scaled integers, with the thresholds converted.
//!
//! The pieces marked as shared (`LUMA`, `l_star`, `contrast`,
//! `blend_factor`, `kernel`, `tile_for`, `border_for`, `tile_index`,
//! `contrast_threshold` and the tile constants) are public so that a
//! GPU implementation reads the same numbers this reference does; a
//! difference between the two is then rounding, not a constant.

use crate::image::WorkingImage;
use crate::raw::{CfaColor, CfaPattern};
use rayon::prelude::*;

/// Luminance weights of the working space, Rec.2020. Shared.
pub const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// A sample this many times brighter than every same-color neighbor
/// two away is a spike, not an edge, and says nothing about the lens.
const SPIKE_RATIO: f32 = 4.0;

/// The narrowest and widest point spread the deconvolution will assume,
/// in pixels of standard deviation; RawTherapee's range.
pub const MIN_RADIUS: f32 = 0.4;
pub const MAX_RADIUS: f32 = 2.0;
/// The width assumed when the mosaic cannot say.
pub const DEFAULT_RADIUS: f32 = 0.75;
pub const DEFAULT_ITERATIONS: usize = 20;

/// The point spread's standard deviation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Radius {
    /// Measured on the mosaic from its sharpest point.
    Auto,
    Fixed(f32),
}

/// The local contrast below which a pixel is left alone, in
/// RawTherapee's units (a fraction; its panel shows a percentage).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Threshold {
    /// Set from the flattest patch in the picture, so that its noise
    /// falls under it.
    Auto,
    Fixed(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SharpenOptions {
    pub radius: Radius,
    /// Richardson–Lucy iterations; more is sharper and, past a point,
    /// haloed.
    pub iterations: usize,
    pub contrast: Threshold,
    /// Stop a patch's iterations when any pixel in it drops under half
    /// its blended start, before a halo goes dark.
    pub stop_early: bool,
}

impl Default for SharpenOptions {
    fn default() -> Self {
        Self {
            radius: Radius::Auto,
            iterations: DEFAULT_ITERATIONS,
            contrast: Threshold::Auto,
            stop_early: true,
        }
    }
}

/// What a sharpen did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SharpenStats {
    /// The point spread deconvolved, pixels.
    pub radius: f32,
    /// The contrast threshold used; zero means everything unclipped.
    pub threshold: f32,
    /// Mean of the blend mask: how much of the picture was sharpened.
    pub blend_mean: f32,
    /// Pixels excluded as clipped, as a fraction.
    pub clipped: f32,
    pub iterations: usize,
}

/// The point spread's width from a Bayer mosaic: the sharpest pair of
/// diagonal green neighbors, taken as a point source under a Gaussian,
/// gives its standard deviation as `sqrt(1 / ln(ratio))`. Samples below
/// `lower` are too dark to trust; a pair with a neighbor at or above
/// `upper` is clipped and skipped. None on a mosaic with no edge at all,
/// or that is not Bayer. A heuristic, RawTherapee's: it reads the
/// sharpest edge in the picture, and an isolated wide star would fool
/// it; the caller clamps the answer.
pub fn measure_radius(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    lower: f32,
    upper: f32,
) -> Option<f32> {
    if pattern.width != 2 || pattern.height != 2 || width < 10 || height < 10 {
        return None;
    }
    let at = |row: usize, col: usize| samples[row * width + col];
    let max_ratio = (4..height - 4)
        .into_par_iter()
        .map(|row| {
            let mut best = 1.0f32;
            for col in 5..width - 4 {
                if pattern.color_at(row, col) != CfaColor::Green {
                    continue;
                }
                let center = at(row, col);
                if center <= 0.0 {
                    continue;
                }
                for (dc, others) in [
                    (
                        -1isize,
                        [
                            (row - 1, col - 1),
                            (row - 1, col + 1),
                            (row + 1, col + 1),
                            (row, col - 2),
                            (row + 2, col - 2),
                            (row + 2, col),
                        ],
                    ),
                    (
                        1,
                        [
                            (row - 1, col - 1),
                            (row - 1, col + 1),
                            (row + 1, col - 1),
                            (row, col + 2),
                            (row + 2, col),
                            (row + 2, col + 2),
                        ],
                    ),
                ] {
                    let other = at(row + 1, (col as isize + dc) as usize);
                    if other <= 0.0 {
                        continue;
                    }
                    let (hi, lo) = if center > other {
                        (center, other)
                    } else {
                        (other, center)
                    };
                    if hi <= lower || hi <= best * lo {
                        continue;
                    }
                    // Nothing clipped in reach of the pair, and the bright
                    // one not a spike on its own: a hot photosite would
                    // otherwise set the radius for the whole picture.
                    let clipped = others.iter().any(|&(r, c)| at(r, c) >= upper) || hi >= upper;
                    let (hr, hc) = if center > other {
                        (row, col)
                    } else {
                        (row + 1, (col as isize + dc) as usize)
                    };
                    let spike = [(hr - 2, hc), (hr + 2, hc), (hr, hc - 2), (hr, hc + 2)]
                        .iter()
                        .all(|&(r, c)| at(r, c) * SPIKE_RATIO < hi);
                    if !clipped && !spike {
                        best = hi / lo;
                    }
                }
            }
            best
        })
        .reduce(|| 1.0f32, f32::max);
    if max_ratio <= 1.0 {
        return None;
    }
    let radius = (1.0 / max_ratio.ln()).sqrt();
    radius.is_finite().then_some(radius)
}

/// Sharpen the working image in place. `measured` is the mosaic's own
/// point spread when it could be measured; `clip_level` is the value at
/// or above which a channel counts as clipped, and its neighborhood is
/// left alone.
pub fn sharpen(
    image: &mut WorkingImage,
    options: &SharpenOptions,
    measured: Option<f32>,
    clip_level: f32,
) -> SharpenStats {
    sharpen_with_mask(image, options, measured, clip_level).0
}

/// [`sharpen`], and the blend mask it worked under: one plane, a
/// value in 0 to 1 a pixel, how far the sharpen went in there; zero
/// where the picture was flat or clipped. For showing where it acts.
pub fn sharpen_with_mask(
    image: &mut WorkingImage,
    options: &SharpenOptions,
    measured: Option<f32>,
    clip_level: f32,
) -> (SharpenStats, Vec<f32>) {
    let radius = match options.radius {
        Radius::Auto => measured.unwrap_or(DEFAULT_RADIUS),
        Radius::Fixed(r) => r,
    }
    .clamp(MIN_RADIUS, MAX_RADIUS);
    let (w, h) = (image.width, image.height);
    let n = w * h;

    // Luminance, its L*, and the clip mask.
    let mut luminance = vec![0f32; n];
    let mut lstar = vec![0f32; n];
    let mut clip = vec![1f32; n];
    luminance
        .par_iter_mut()
        .zip(lstar.par_iter_mut())
        .zip(clip.par_iter_mut())
        .zip(image.data.par_chunks(3))
        .for_each(|(((y, l), c), px)| {
            *y = LUMA[0] * px[0] + LUMA[1] * px[1] + LUMA[2] * px[2];
            *l = l_star(*y);
            if px[0] >= clip_level || px[1] >= clip_level || px[2] >= clip_level {
                *c = 0.0;
            }
        });
    let clipped = 1.0 - mean(&clip);
    dilate_zeros(&mut clip, w, h, 2);

    let threshold = match options.contrast {
        Threshold::Fixed(t) => t.max(0.0),
        Threshold::Auto => auto_threshold(&lstar, w, h),
    };
    let blend = blend_mask(&lstar, &clip, w, h, threshold);
    let blend_mean = mean(&blend);

    let sharpened = deconvolve(&luminance, &blend, w, h, radius, options);

    image
        .data
        .par_chunks_mut(3)
        .zip(luminance.par_iter())
        .zip(sharpened.par_iter())
        .for_each(|((px, &old), &new)| {
            let factor = new / old.max(1e-5);
            px[0] *= factor;
            px[1] *= factor;
            px[2] *= factor;
        });
    (
        SharpenStats {
            radius,
            threshold,
            blend_mean,
            clipped,
            iterations: options.iterations,
        },
        blend,
    )
}

/// The mean of a plane, summed in double so a large picture does not
/// saturate a single-precision total.
fn mean(plane: &[f32]) -> f32 {
    (plane.par_iter().map(|&v| v as f64).sum::<f64>() / plane.len() as f64) as f32
}

/// CIE L* of a luminance relative to white at one. Shared.
pub fn l_star(y: f32) -> f32 {
    let f = if y > 0.008856 {
        y.max(0.0).cbrt()
    } else {
        7.787 * y + 16.0 / 116.0
    };
    116.0 * f - 16.0
}

/// Local contrast as RawTherapee measures it: the root of the summed
/// squares of the central differences at one and two pixels, in L*,
/// over sixteen. Shared.
#[inline]
pub fn contrast(l: &[f32], w: usize, x: usize, y: usize) -> f32 {
    let at = |xx: usize, yy: usize| l[yy * w + xx];
    let dx1 = at(x + 1, y) - at(x - 1, y);
    let dy1 = at(x, y + 1) - at(x, y - 1);
    let dx2 = at(x + 2, y) - at(x - 2, y);
    let dy2 = at(x, y + 2) - at(x, y - 2);
    (dx1 * dx1 + dy1 * dy1 + dx2 * dx2 + dy2 * dy2).sqrt() * 0.0625
}

/// RawTherapee's sigmoid: half at the threshold, near one at twice it.
/// Shared.
#[inline]
pub fn blend_factor(contrast: f32, threshold: f32) -> f32 {
    let x = -16.0 + (16.0 / threshold) * contrast;
    0.5 * (1.0 + x / (1.0 + x * x).sqrt())
}

/// The blur that softens the blend mask's edges, in pixels of sigma.
/// Shared.
pub const BLEND_BLUR_SIGMA: f32 = 2.0;

/// The blend mask: the clip mask times the sigmoid of local contrast,
/// blurred so its edges are soft. A zero threshold blends everything
/// unclipped in full.
fn blend_mask(lstar: &[f32], clip: &[f32], w: usize, h: usize, threshold: f32) -> Vec<f32> {
    if threshold <= 0.0 || w < 5 || h < 5 {
        return clip.to_vec();
    }
    let mut blend = vec![0f32; w * h];
    blend.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let yy = y.clamp(2, h - 3);
        for (x, b) in row.iter_mut().enumerate() {
            let xx = x.clamp(2, w - 3);
            *b = clip[y * w + x] * blend_factor(contrast(lstar, w, xx, yy), threshold);
        }
    });
    super::dual::gaussian_blur(&mut blend, w, h, BLEND_BLUR_SIGMA);
    blend
}

/// Widen the zeros of a mask by `by` pixels each way.
fn dilate_zeros(mask: &mut [f32], w: usize, h: usize, by: usize) {
    let source = mask.to_vec();
    mask.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, m) in row.iter_mut().enumerate() {
            if *m == 0.0 {
                continue;
            }
            let y0 = y.saturating_sub(by);
            let y1 = (y + by).min(h - 1);
            let x0 = x.saturating_sub(by);
            let x1 = (x + by).min(w - 1);
            'search: for yy in y0..=y1 {
                for xx in x0..=x1 {
                    if source[yy * w + xx] == 0.0 {
                        *m = 0.0;
                        break 'search;
                    }
                }
            }
        }
    });
}

// RawTherapee's tile statistics are in L* times 327.68; these are the
// same limits in L*. All shared.
/// A tile whose mean L* is under this is too dark to read noise from.
pub const MIN_TILE_L: f32 = 2000.0 / 327.68;
/// A tile whose mean L* is over this is too bright to read noise from.
pub const MAX_TILE_L: f32 = 20000.0 / 327.68;
/// A tile whose variance over its mean is under this is suspiciously
/// flat (a blown or a black patch) and does not count.
pub const MIN_TILE_VARIANCE: f32 = 0.5 / 327.68;
/// The coarse pass takes its flattest tile when its index is at most
/// this; otherwise the fine pass runs.
pub const FLAT_ENOUGH: f32 = 1.0 / 327.68;
/// The fine pass takes its flattest tile when its index is at most
/// this; otherwise the threshold is zero.
pub const FLAT_ENOUGH_SECOND_PASS: f32 = 8.0 / 327.68;
/// The automatic threshold's coarse first pass: tiles of this side on
/// a grid of the same step. Shared.
pub const COARSE_TILE: usize = 80;
/// The finer second pass's tile. Shared.
pub const FINE_TILE: usize = 40;
/// The step of the second pass's grid, and how far around its best
/// tile every offset is then tried. Shared.
pub const FINE_SKIP: usize = FINE_TILE / 4;

/// The variance-over-mean of a tile of L*, or infinity when the tile
/// is too dark, too bright or suspiciously flat. Shared.
pub fn tile_index(lstar: &[f32], w: usize, x0: usize, y0: usize, size: usize) -> f32 {
    let n = (size * size) as f32;
    let mut sum = 0.0f32;
    for y in y0..y0 + size {
        sum += lstar[y * w + x0..y * w + x0 + size].iter().sum::<f32>();
    }
    let avg = sum / n;
    if !(MIN_TILE_L..=MAX_TILE_L).contains(&avg) {
        return f32::INFINITY;
    }
    let mut var = 0.0f32;
    for y in y0..y0 + size {
        var += lstar[y * w + x0..y * w + x0 + size]
            .iter()
            .map(|v| (v - avg) * (v - avg))
            .sum::<f32>();
    }
    let index = var / (n * avg);
    if index < MIN_TILE_VARIANCE {
        f32::INFINITY
    } else {
        index
    }
}

/// The flattest tile among those at `origins`, by index, when any
/// qualifies.
fn flattest(
    lstar: &[f32],
    w: usize,
    size: usize,
    origins: &[(usize, usize)],
) -> Option<((usize, usize), f32)> {
    origins
        .par_iter()
        .map(|&(x, y)| ((x, y), tile_index(lstar, w, x, y, size)))
        .filter(|(_, v)| v.is_finite())
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// RawTherapee's automatic contrast threshold: find the flattest patch
/// of the picture in two passes (coarse tiles of 80, then tiles of 40
/// on a finer grid and around the best), and set the threshold so that
/// at most one pixel in a hundred of that patch would be sharpened.
/// Zero when no patch is flat enough to trust.
fn auto_threshold(lstar: &[f32], w: usize, h: usize) -> f32 {
    let grid = |size: usize, skip: usize, trim: usize| -> Vec<(usize, usize)> {
        let nx = (w / skip).saturating_sub(trim);
        let ny = (h / skip).saturating_sub(trim);
        (0..ny)
            .flat_map(|j| (0..nx).map(move |i| (i * skip, j * skip)))
            .filter(|&(x, y)| x + size <= w && y + size <= h)
            .collect()
    };
    // First pass: coarse.
    let size = COARSE_TILE;
    if let Some(((x, y), index)) = flattest(lstar, w, size, &grid(size, size, 0))
        && index <= FLAT_ENOUGH
    {
        return contrast_threshold(lstar, w, x, y, size);
    }
    // Second pass: finer, then every offset around the best.
    let size = FINE_TILE;
    let skip = FINE_SKIP;
    let Some(((bx, by), _)) = flattest(lstar, w, size, &grid(size, skip, 3)) else {
        return 0.0;
    };
    let x0 = bx.saturating_sub(skip);
    let y0 = by.saturating_sub(skip);
    let x1 = (bx + skip).min(w.saturating_sub(size));
    let y1 = (by + skip).min(h.saturating_sub(size));
    let around: Vec<(usize, usize)> = (y0..=y1)
        .flat_map(|y| (x0..=x1).map(move |x| (x, y)))
        .collect();
    match flattest(lstar, w, size, &around) {
        Some(((x, y), index)) if index <= FLAT_ENOUGH_SECOND_PASS => {
            contrast_threshold(lstar, w, x, y, size)
        }
        _ => 0.0,
    }
}

/// The smallest threshold, in hundredths, at which the sigmoid over a
/// tile's contrasts sums to at most one percent of its pixels. Reads
/// the tile alone, so a copy of it with `w = size` gives the same
/// answer. Shared.
pub fn contrast_threshold(lstar: &[f32], w: usize, x0: usize, y0: usize, size: usize) -> f32 {
    let inner = size - 4;
    let contrasts: Vec<f32> = (y0 + 2..y0 + size - 2)
        .flat_map(|y| (x0 + 2..x0 + size - 2).map(move |x| contrast(lstar, w, x, y)))
        .collect();
    let limit = (inner * inner) as f32 / 100.0;
    let mut c = 1;
    while c < 100 {
        let t = c as f32 / 100.0;
        let sum: f32 = contrasts.iter().map(|&v| blend_factor(v, t)).sum();
        if sum <= limit {
            break;
        }
        c += 1;
    }
    c as f32 / 100.0
}

/// A truncated Gaussian, one dimension, normalized. RawTherapee's
/// kernel sizes by sigma. Shared.
pub fn kernel(sigma: f32) -> Vec<f32> {
    let half: i32 = if sigma < 0.6 {
        1
    } else if sigma <= 0.84 {
        2
    } else if sigma <= 1.15 {
        3
    } else if sigma <= 1.5 {
        4
    } else {
        6
    };
    let mut k: Vec<f32> = (-half..=half)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    k
}

/// The square worked on at a time. A tile carries a border of context
/// it computes and throws away, so a wider tile wastes less: at 32 the
/// deconvolution computed 1.7 pixels for every one it kept, at 128 it
/// is 1.16. Shared.
pub const TILE: usize = 128;

/// The grain at which a tile decides it has gone far enough: the old
/// tile, so that one dark halo does not stop the iterations over a
/// whole wide tile. `TILE` is a multiple of it, so these blocks sit on
/// the same grid whatever the tile. Shared.
pub const BLOCK: usize = 32;

/// A block whose blend never reaches this is left as it is. Shared.
pub const SETTLED_BLEND: f32 = 0.01;

/// Rows of tiles are what the work is handed round in, so a short
/// picture — a downsized export's output sharpening — takes a narrower
/// tile rather than leaving most of the machine idle. Halving keeps
/// the tile a multiple of `BLOCK`, and it depends on the picture and
/// not on the machine, so the answer is the same on any of them.
/// Shared.
pub fn tile_for(h: usize) -> usize {
    let mut tile = TILE;
    while tile > BLOCK && h.div_ceil(tile) < 32 {
        tile /= 2;
    }
    tile
}

/// Richardson–Lucy on the luminance, in tiles with a border, each
/// blended into the old luminance by the mask.
fn deconvolve(
    luminance: &[f32],
    blend: &[f32],
    w: usize,
    h: usize,
    sigma: f32,
    options: &SharpenOptions,
) -> Vec<f32> {
    deconvolve_in_tiles(luminance, blend, w, h, sigma, options, tile_for(h))
}

/// [`deconvolve`] with the tile named, for the test that the tiling
/// does not decide the answer.
fn deconvolve_in_tiles(
    luminance: &[f32],
    blend: &[f32],
    w: usize,
    h: usize,
    sigma: f32,
    options: &SharpenOptions,
    tile: usize,
) -> Vec<f32> {
    let k = kernel(sigma);
    let half = k.len() / 2;
    let border = border_for(half, options.iterations);
    let full = tile + 2 * border;
    let mut out = luminance.to_vec();
    out.par_chunks_mut(w * tile)
        .enumerate()
        .for_each(|(band, rows)| {
            let y0 = band * tile;
            let rows_here = rows.len() / w;
            let mut estimate = vec![0f32; full * full];
            let mut original = vec![0f32; full * full];
            let mut ratio = vec![0f32; full * full];
            let mut scratch = vec![0f32; full * full];
            let mut floor = vec![0f32; tile * tile];
            let mut settled = Vec::new();
            let down = rows_here.div_ceil(BLOCK);
            for x0 in (0..w).step_by(tile) {
                let cols_here = tile.min(w - x0);
                let across = cols_here.div_ceil(BLOCK);
                // The rows and columns a block covers, and the floor
                // under which its iterations stop.
                let rows_of = |j: usize| j * BLOCK..((j + 1) * BLOCK).min(rows_here);
                let cols_of = |i: usize| i * BLOCK..((i + 1) * BLOCK).min(cols_here);
                // Anything to do in each block of the tile?
                settled.clear();
                settled.resize(across * down, false);
                let mut left = 0;
                for j in 0..down {
                    for i in 0..across {
                        let mut max_blend = 0.0f32;
                        for ty in rows_of(j) {
                            for tx in cols_of(i) {
                                let b = blend[(y0 + ty) * w + x0 + tx];
                                max_blend = max_blend.max(b);
                                floor[ty * tile + tx] =
                                    luminance[(y0 + ty) * w + x0 + tx] * b * 0.5;
                            }
                        }
                        if max_blend < SETTLED_BLEND {
                            settled[j * across + i] = true;
                        } else {
                            left += 1;
                        }
                    }
                }
                if left == 0 {
                    continue;
                }
                // Blend a block's share of the estimate into the answer.
                let commit = |rows: &mut [f32], estimate: &[f32], i: usize, j: usize| {
                    for ty in rows_of(j) {
                        for tx in cols_of(i) {
                            let at = ty * w + x0 + tx;
                            let b = blend[(y0 + ty) * w + x0 + tx];
                            let new = estimate[(ty + border) * full + tx + border];
                            rows[at] += b * (new - rows[at]);
                        }
                    }
                };
                // Fill the tile and its border, clamped at the picture's edge.
                for ty in 0..full {
                    let sy = (y0 + ty).saturating_sub(border).min(h - 1);
                    for tx in 0..full {
                        let sx = (x0 + tx).saturating_sub(border).min(w - 1);
                        let v = luminance[sy * w + sx];
                        original[ty * full + tx] = v;
                        estimate[ty * full + tx] = v;
                    }
                }
                for iteration in 0..options.iterations {
                    // ratio = original / blur(estimate); estimate *= blur(ratio).
                    blur_rows(&estimate, &mut scratch, full, &k);
                    blur_columns(&scratch, &mut ratio, full, &k);
                    for i in 0..full * full {
                        ratio[i] = original[i] / ratio[i].max(1e-6);
                    }
                    blur_rows(&ratio, &mut scratch, full, &k);
                    blur_columns(&scratch, &mut ratio, full, &k);
                    for i in 0..full * full {
                        estimate[i] *= ratio[i];
                    }
                    if options.stop_early && iteration + 1 < options.iterations {
                        for j in 0..down {
                            for i in 0..across {
                                if settled[j * across + i] {
                                    continue;
                                }
                                let cols = cols_of(i);
                                let mut stop = false;
                                for ty in rows_of(j) {
                                    let at = (ty + border) * full + border + cols.start;
                                    let here = &estimate[at..at + cols.len()];
                                    let under =
                                        &floor[ty * tile + cols.start..ty * tile + cols.end];
                                    // A minimum, not a search for the
                                    // first one under: the row vectorizes,
                                    // and a block this size seldom stops.
                                    let worst = here
                                        .iter()
                                        .zip(under)
                                        .fold(f32::INFINITY, |m, (e, f)| m.min(e - f));
                                    if worst < 0.0 {
                                        stop = true;
                                        break;
                                    }
                                }
                                if stop {
                                    commit(rows, &estimate, i, j);
                                    settled[j * across + i] = true;
                                    left -= 1;
                                }
                            }
                        }
                        if left == 0 {
                            break;
                        }
                    }
                }
                for j in 0..down {
                    for i in 0..across {
                        if !settled[j * across + i] {
                            commit(rows, &estimate, i, j);
                        }
                    }
                }
            }
        });
    out
}

/// The context a tile carries around its edge for `half` taps each
/// side of the kernel and this many iterations: RawTherapee's sizes.
/// Shared.
pub fn border_for(half: usize, iterations: usize) -> usize {
    if half <= 3 {
        if iterations <= 30 { 5 } else { 7 }
    } else {
        8
    }
}

/// One pass of a separable blur over a square tile, along its rows,
/// reads clamped at the edge.
fn blur_rows(src: &[f32], out: &mut [f32], size: usize, k: &[f32]) {
    let half = k.len() / 2;
    for (row, orow) in src.chunks_exact(size).zip(out.chunks_exact_mut(size)) {
        let clamped = |x: usize| {
            let mut acc = 0.0;
            for (i, &kv) in k.iter().enumerate() {
                let sx = (x + i).saturating_sub(half).min(size - 1);
                acc += kv * row[sx];
            }
            acc
        };
        let lo = half.min(size);
        let hi = size.saturating_sub(half).max(lo);
        for (x, o) in orow.iter_mut().enumerate().take(lo) {
            *o = clamped(x);
        }
        for (o, win) in orow[lo..hi].iter_mut().zip(row.windows(k.len())) {
            *o = k.iter().zip(win).map(|(a, b)| a * b).sum();
        }
        for (x, o) in orow.iter_mut().enumerate().skip(hi) {
            *o = clamped(x);
        }
    }
}

/// The other pass, down the columns: each output row is a weighted sum
/// of whole source rows.
fn blur_columns(src: &[f32], out: &mut [f32], size: usize, k: &[f32]) {
    let half = k.len() / 2;
    for (y, orow) in out.chunks_exact_mut(size).enumerate() {
        orow.fill(0.0);
        for (i, &kv) in k.iter().enumerate() {
            let sy = (y + i).saturating_sub(half).min(size - 1);
            let srow = &src[sy * size..(sy + 1) * size];
            for (o, s) in orow.iter_mut().zip(srow) {
                *o += kv * s;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grey picture with a vertical edge blurred by `sigma`.
    fn blurred_edge(w: usize, h: usize, sigma: f32) -> WorkingImage {
        let mut image = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // A Gaussian-blurred step: the error function.
                let d = (x as f32 - w as f32 / 2.0 + 0.5) / (sigma * std::f32::consts::SQRT_2);
                let v = 0.1 + 0.4 * (1.0 + erf(d)) / 2.0;
                let i = (y * w + x) * 3;
                image.data[i] = v;
                image.data[i + 1] = v;
                image.data[i + 2] = v;
            }
        }
        image
    }

    fn erf(x: f32) -> f32 {
        // Abramowitz and Stegun 7.1.26.
        let t = 1.0 / (1.0 + 0.3275911 * x.abs());
        let y = 1.0
            - (((((1.0614054 * t - 1.453152) * t) + 1.4214137) * t - 0.28449674) * t + 0.2548296)
                * t
                * (-x * x).exp();
        if x >= 0.0 { y } else { -y }
    }

    /// The steepest step between neighbors along a row: sharper is
    /// steeper.
    fn slope(image: &WorkingImage, y: usize) -> f32 {
        let row: Vec<f32> = (0..image.width)
            .map(|x| image.data[(y * image.width + x) * 3 + 1])
            .collect();
        row.windows(2).map(|w| w[1] - w[0]).fold(0.0, f32::max)
    }

    #[test]
    fn the_mask_handed_back_is_the_one_the_sharpen_used() {
        let (w, h) = (96, 64);
        let mut image = blurred_edge(w, h, 1.0);
        let options = SharpenOptions {
            radius: Radius::Fixed(1.0),
            iterations: 20,
            contrast: Threshold::Fixed(0.02),
            stop_early: true,
        };
        let (stats, mask) = sharpen_with_mask(&mut image, &options, None, 10.0);
        assert_eq!(mask.len(), w * h);
        assert!((mean(&mask) - stats.blend_mean).abs() < 1e-6);
        // Up at the edge, nothing on the flat far from it, and
        // between the two in the blur's soft border.
        let at = |x: usize| mask[(h / 2) * w + x];
        assert!(at(w / 2) > 0.9, "at the edge {}", at(w / 2));
        assert!(at(4) < 0.01, "on the flat {}", at(4));
        assert!(mask.iter().all(|&m| (0.0..=1.0).contains(&m)));
        // The same picture sharpened plainly is the same picture.
        let mut plain = blurred_edge(w, h, 1.0);
        let plain_stats = sharpen(&mut plain, &options, None, 10.0);
        assert_eq!(plain_stats, stats);
        assert_eq!(plain.data, image.data);
    }

    #[test]
    fn a_blurred_edge_comes_back_sharper_and_the_flat_stays() {
        let (w, h) = (96, 64);
        let mut image = blurred_edge(w, h, 1.0);
        let before = slope(&image, h / 2);
        let flat_before = image.data[(10 * w + 4) * 3];
        let options = SharpenOptions {
            radius: Radius::Fixed(1.0),
            iterations: 20,
            contrast: Threshold::Fixed(0.02),
            stop_early: true,
        };
        let stats = sharpen(&mut image, &options, None, 10.0);
        let after = slope(&image, h / 2);
        assert!(after > before * 1.4, "slope {before} -> {after}");
        // Far from the edge nothing moves, and a corner is no different
        // from the middle.
        assert!((image.data[(10 * w + 4) * 3] - flat_before).abs() < 1e-4);
        assert!((image.data[4 * 3] - flat_before).abs() < 1e-4);
        assert!(
            (slope(&image, 0) - after).abs() < 0.01,
            "the top row {}",
            slope(&image, 0)
        );
        assert!(
            stats.blend_mean > 0.0 && stats.blend_mean < 0.5,
            "{stats:?}"
        );
        assert_eq!(stats.clipped, 0.0);
        // Channel ratios hold: the sharpen is a gain.
        let mut color = blurred_edge(w, h, 1.0);
        for px in color.data.chunks_mut(3) {
            px[0] *= 1.5;
            px[2] *= 0.5;
        }
        sharpen(&mut color, &options, None, 10.0);
        for px in color.data.chunks(3) {
            assert!((px[0] / px[1] - 1.5).abs() < 1e-3 && (px[2] / px[1] - 0.5).abs() < 1e-3);
        }
    }

    #[test]
    fn clipped_and_flat_areas_are_left_alone() {
        let (w, h) = (96, 64);
        let mut image = blurred_edge(w, h, 1.0);
        let before = image.data.clone();
        let options = SharpenOptions {
            radius: Radius::Fixed(1.0),
            contrast: Threshold::Fixed(0.02),
            ..Default::default()
        };
        let center = (h / 2 * w + w / 2) * 3;
        let moved = |image: &WorkingImage| {
            image
                .data
                .iter()
                .zip(&before)
                .filter(|(a, b)| (*a - *b).abs() > 1e-4)
                .count()
        };
        let mut free = blurred_edge(w, h, 1.0);
        sharpen(&mut free, &options, None, 10.0);
        let free_moved = moved(&free);
        let free_center = (free.data[center] - before[center]).abs();
        // Everything above the edge's foot counts as clipped: the edge
        // and two pixels around it are masked out, and the blur of the
        // mask lets only a little through.
        let stats = sharpen(&mut image, &options, None, 0.15);
        assert!(stats.clipped > 0.4, "{stats:?}");
        assert!(
            moved(&image) < free_moved,
            "{} vs {free_moved}",
            moved(&image)
        );
        let center_moved = (image.data[center] - before[center]).abs();
        assert!(
            center_moved < free_center * 0.2,
            "{center_moved} vs {free_center}"
        );
        // A high threshold leaves a gentle edge alone too.
        let mut image = blurred_edge(w, h, 1.0);
        let options = SharpenOptions {
            contrast: Threshold::Fixed(50.0),
            ..options
        };
        sharpen(&mut image, &options, None, 10.0);
        assert!(
            image
                .data
                .iter()
                .zip(&before)
                .all(|(a, b)| (a - b).abs() < 1e-3)
        );
    }

    /// A blurred picture with detail in every tile, and a flat corner
    /// the blend leaves alone.
    fn textured(w: usize, h: usize, sigma: f32) -> (Vec<f32>, Vec<f32>) {
        let mut luminance = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let bars = if (x / 7 + y / 5) % 2 == 0 { 0.55 } else { 0.08 };
                let spot = if (x * x + y * y) % 97 < 11 { 0.3 } else { 0.0 };
                luminance[y * w + x] = bars + spot;
            }
        }
        super::super::dual::gaussian_blur(&mut luminance, w, h, sigma);
        let mut blend = vec![1f32; w * h];
        for y in 0..h {
            for x in 0..w {
                if x >= w - 40 && y >= h - 40 {
                    blend[y * w + x] = 0.0;
                } else if x > w / 2 {
                    blend[y * w + x] = 0.6;
                }
            }
        }
        (luminance, blend)
    }

    #[test]
    fn the_tile_is_not_what_the_answer_rests_on() {
        // A tile carries a border of context it throws away, so where
        // that border runs out the answer depends on the tiling. A tile
        // wider than the picture has no seam but the picture's own edge,
        // and is the answer the others are trying to be.
        let (w, h) = (208, 176);
        let (luminance, blend) = textured(w, h, 1.0);
        let options = SharpenOptions {
            radius: Radius::Fixed(1.0),
            iterations: 20,
            contrast: Threshold::Fixed(0.02),
            stop_early: true,
        };
        let whole = deconvolve_in_tiles(&luminance, &blend, w, h, 1.0, &options, 256);
        let old = deconvolve_in_tiles(&luminance, &blend, w, h, 1.0, &options, 32);
        let new = deconvolve_in_tiles(&luminance, &blend, w, h, 1.0, &options, TILE);
        let off = |a: &[f32], b: &[f32], i: usize| (a[i] - b[i]).abs() / a[i].abs().max(1e-3);
        let mut apart = 0f32;
        let (mut was, mut is) = (0f64, 0f64);
        let (mut was_worst, mut is_worst) = (0f32, 0f32);
        for i in 0..w * h {
            apart = apart.max(off(&old, &new, i));
            was += off(&whole, &old, i) as f64;
            is += off(&whole, &new, i) as f64;
            was_worst = was_worst.max(off(&whole, &old, i));
            is_worst = is_worst.max(off(&whole, &new, i));
        }
        // The two tilings are the same picture: nowhere a part in a
        // hundred apart, on a bar pattern harsher than any photograph.
        assert!(apart < 1e-2, "the two tilings are {apart} apart");
        // And the wider tile is the nearer of the two to the untiled
        // answer, in the mean by several times and at its worst too.
        assert!(
            is < was * 0.25,
            "against no tiling at all: was {was}, is {is}"
        );
        assert!(
            is_worst < was_worst,
            "at its worst: was {was_worst}, is {is_worst}"
        );
        // A tall picture is worked in the widest tile; a short one,
        // which would leave a machine idle a row of tiles at a time,
        // narrows down to the old tile.
        assert_eq!(tile_for(8192), 128);
        assert_eq!(tile_for(4000), 128);
        assert_eq!(tile_for(1365), 32);
        assert_eq!(tile_for(40), 32);
        // The flat corner the blend excludes is untouched by any of them.
        for y in h - 40..h {
            for x in w - 40..w {
                assert_eq!(new[y * w + x], luminance[y * w + x]);
                assert_eq!(old[y * w + x], luminance[y * w + x]);
            }
        }
    }

    #[test]
    fn the_automatic_threshold_sits_above_the_noise() {
        // Flat grey with noise: the threshold lands above the noise's
        // contrast, so that almost none of it is sharpened.
        let (w, h) = (256, 192);
        let mut image = WorkingImage::new(w, h);
        let mut seed = 0x9E3779B9u32;
        let mut rand = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32) - 0.5
        };
        // Two percent of noise at mid grey, about a modern sensor at a
        // low ISO.
        for px in image.data.chunks_mut(3) {
            let v = 0.18 + 0.014 * rand();
            px.copy_from_slice(&[v, v, v]);
        }
        let stats = sharpen(&mut image, &SharpenOptions::default(), None, 10.0);
        assert!(stats.threshold > 0.0, "{stats:?}");
        assert!(stats.blend_mean < 0.05, "{stats:?}");
    }

    #[test]
    fn the_radius_follows_the_edge_and_ignores_a_spike() {
        // A Bayer mosaic of a vertical edge under a Gaussian: the ratio
        // across the sharpest diagonal green pair falls as the blur
        // widens, so the radius rises with it. A hot photosite, or a
        // clipped one, does not take part.
        let (w, h) = (48, 32);
        let pattern = CfaPattern::rggb();
        let mosaic = |sigma: f32| -> Vec<f32> {
            let mut samples = vec![0f32; w * h];
            for y in 0..h {
                for x in 0..w {
                    let d = (x as f32 - 23.5) / (sigma * std::f32::consts::SQRT_2);
                    samples[y * w + x] = 0.05 + 0.45 * (1.0 + erf(d)) / 2.0;
                }
            }
            samples
        };
        let radius = |samples: &[f32]| measure_radius(samples, w, h, &pattern, 0.015, 0.99);
        let r: Vec<f32> = [0.5f32, 0.8, 1.2]
            .iter()
            .map(|&sigma| radius(&mosaic(sigma)).unwrap())
            .collect();
        assert!(r[0] < r[1] && r[1] < r[2], "{r:?}");
        assert!(r[0] > 0.3 && r[2] < 3.0, "{r:?}");
        // A spike far from the edge: ignored.
        let mut spiked = mosaic(0.8);
        spiked[10 * w + 7] = 0.9; // a green in RGGB, on the dark side
        assert_eq!(pattern.color_at(10, 7), CfaColor::Green);
        assert_eq!(radius(&spiked), Some(r[1]));
        // A clipped edge: its sharp part is skipped, and only the soft
        // foot is left to read, which says wider.
        let mut clipped = mosaic(0.8);
        for v in clipped.iter_mut() {
            if *v > 0.3 {
                *v = 1.0;
            }
        }
        assert!(radius(&clipped).unwrap() > r[1] * 1.5);
        // Flat: nothing to measure at all.
        assert_eq!(radius(&vec![0.2; w * h]), None);
    }
}
