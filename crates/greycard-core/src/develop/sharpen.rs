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
//! RawTherapee's scaled integers, with the thresholds converted. And
//! the deconvolution runs on the luminance clipped at zero and raised
//! by [`BLACK_PEDESTAL`], the channels scaled about the pedestal's
//! negative and the early stop's floor read under the pedestal
//! ([`stop_floor`]): RawTherapee divides camera values that are never
//! negative, but the working image is after the camera matrix, where
//! the noise at black goes below zero, and there Richardson–Lucy and
//! the division by the old luminance both blew single pixels up into
//! full-brightness colored dots. And what is flat (the L* that the
//! automatic threshold and the blend mask read) can be read off a
//! guide, the picture before the Detail section's Texture, Clarity,
//! and Structure, while the deconvolution runs on the picture after
//! them: RawTherapee's capture sharpening runs on the demosaiced raw,
//! before any local contrast, so the grain those lift never reaches
//! its threshold; ours runs after them, and without the guide Texture
//! lifted the flattest patch's grain until the threshold fell to zero
//! and the grain was sharpened as detail.
//!
//! The pieces marked as shared (`LUMA`, `BLACK_PEDESTAL`,
//! `FLOOR_PEDESTAL`, `stop_floor`, `l_star`, `contrast`,
//! `blend_factor`, `kernel`, `tile_for`, `border_for`, `tile_index`,
//! `contrast_threshold` and the tile constants) are public so that a
//! GPU implementation reads the same numbers this reference does; a
//! difference between the two is then rounding, not a constant.

use crate::image::WorkingImage;
use crate::raw::{CfaColor, CfaPattern};
use rayon::prelude::*;

/// Luminance weights of the working space, Rec.2020. Shared.
pub const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// What the deconvolution adds to the luminance, clipped at zero,
/// before it starts, relative to white at one. Shared.
///
/// The working image is after the camera matrix, so the noise at
/// black goes below zero. Richardson–Lucy divides by its blurred
/// estimate and multiplies by the ratio: on a luminance that is noise
/// about zero the divisor crosses zero, the ratio runs to hundreds and
/// changes sign, and the estimate with it; and the channels were then
/// scaled by the sharpened luminance over an old one of a ten
/// thousandth or less. At ISO 10000 the two together made hundreds of
/// isolated magenta, blue and white dots in the blacks, one pixel at a
/// time. On a luminance that is never under the pedestal the ratios
/// stay near one, and the channels are scaled about the pedestal's
/// negative, `(c + k) · new / old − k`, so a factor never meets a
/// luminance smaller than the pedestal.
///
/// Small, because under it the sharpen changes character: the
/// deconvolution there is nearer linear than Richardson–Lucy's
/// multiplicative steps, which go slowly in the dark, and a channel's
/// change there is nearer an equal step in every channel than a gain.
/// At a thousandth of white the frame that showed the dots shows none,
/// and no pixel in its blacks rises by a hundredth of white, at the
/// editor's settings and at the panel's strongest; it stays so with
/// the pedestal at a thirtieth of that frame's noise at black, so a
/// frame many times noisier is covered too, and the pedestal need not
/// follow the noise. A base-ISO frame's pixels well above the pedestal
/// move by under a level of 255 at the 99.9th percentile.
pub const BLACK_PEDESTAL: f32 = 0.001;

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
/// RawTherapee's units: a fraction, 0 to 2, its panel's 0 to 200
/// percent over a hundred, and ours.
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
    /// Stop a patch's iterations when any pixel's luminance in it drops
    /// under half its blended start, before a halo goes dark.
    pub stop_early: bool,
    /// Deconvolve only the tiles this part of the picture touches,
    /// every other left as it was; none for the whole picture.
    pub part: Option<Part>,
    /// What the picture's levels are multiplied by to read as the
    /// scene was exposed: what the threshold's and the blend's L* and
    /// the pedestal are taken at. One for every file but a merge of
    /// ours with headroom, which stores its samples that many times
    /// under its reference frame's ([`SharpenOptions::at_headroom`]);
    /// read at its own levels, a merge's shadows were taken for darker
    /// than its reference's and sharpened less.
    pub level_scale: f32,
}

impl SharpenOptions {
    /// These options on a file whose samples are `stops` under the
    /// scene's exposure: a merge's headroom, zero for any other file,
    /// which keeps the options as they are.
    pub fn at_headroom(self, stops: f32) -> Self {
        Self {
            level_scale: stops.exp2(),
            ..self
        }
    }

    /// The pedestal in the picture's own levels: [`BLACK_PEDESTAL`] at
    /// the exposed scale. Shared.
    pub fn pedestal(&self) -> f32 {
        BLACK_PEDESTAL / self.level_scale
    }
}

impl Default for SharpenOptions {
    fn default() -> Self {
        Self {
            radius: Radius::Auto,
            iterations: DEFAULT_ITERATIONS,
            contrast: Threshold::Auto,
            stop_early: true,
            part: None,
            level_scale: 1.0,
        }
    }
}

/// A rectangle of the picture, pixels, its far ends exclusive: what a
/// sharpen deconvolves when a view shows only so much of the picture.
///
/// The deconvolution works in tiles that read only their own padded
/// square of the luminance and the blend mask, so a tile the part
/// touches is the whole picture's sharpen of that tile, to the bit.
/// The luminance, the clip mask, the threshold, and the blend mask are
/// still the whole picture's, and so are the stats. Every tile the part
/// misses is left unsharpened. Shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl Part {
    /// The columns and the rows (bands) of `tile`-pixel tiles of a
    /// `w` by `h` picture that the part touches: none for a part off
    /// the picture or of no area.
    pub fn tiles(
        &self,
        w: usize,
        h: usize,
        tile: usize,
    ) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
        let (x1, y1) = (self.x1.min(w), self.y1.min(h));
        if self.x0 >= x1 || self.y0 >= y1 {
            return (0..0, 0..0);
        }
        (
            self.x0 / tile..x1.div_ceil(tile),
            self.y0 / tile..y1.div_ceil(tile),
        )
    }

    /// Whether the part touches every tile of a `w` by `h` picture, so
    /// that it is the whole picture's sharpen.
    pub fn covers(&self, w: usize, h: usize, tile: usize) -> bool {
        let (cols, bands) = self.tiles(w, h, tile);
        cols == (0..w.div_ceil(tile)) && bands == (0..h.div_ceil(tile))
    }
}

/// How small, at the finest detail a view shows, the part of the blur
/// a shortened deconvolution has not yet taken back may be: the
/// residual of [`iterations_seen_at`]'s model.
const SEEN_RESIDUAL: f64 = 0.004;

/// The fewest iterations [`iterations_seen_at`] gives: under it the
/// early stop and the halos it guards against, which the linear model
/// does not see, show at a fit view (measured by greycard-gpu's
/// `held_apart` example: at six, fine synthetic texture at a 0.3 view
/// had 0.12 % of its samples a level or more apart; at eight, about
/// 0.001 %).
const SEEN_FLOOR: usize = 8;

/// How many of `iterations` a view at `zoom` (view pixels a picture
/// pixel) needs to show what all of them would, for a point spread of
/// `sigma`: all of them at 1:1 and above; below it, the finest detail
/// the view shows is `zoom / 2` cycles a picture pixel, where the
/// Gaussian keeps `H = exp(-2 pi^2 sigma^2 f^2)` of it, and
/// Richardson–Lucy takes back the rest at about `1 - H^2` an iteration
/// left; enough for [`SEEN_RESIDUAL`] of it to be left, and never
/// fewer than [`SEEN_FLOOR`].
///
/// Measured on four frames from 24 to 102 MP and on fine synthetic
/// texture, the viewport's minify at 0.15 to 0.7 against twenty
/// iterations in 8-bit sRGB levels: at the counts this gives, about
/// 0.01 % of the samples or fewer a level or more apart.
pub fn iterations_seen_at(zoom: f32, sigma: f32, iterations: usize) -> usize {
    if !(zoom > 0.0 && zoom < 1.0) {
        return iterations;
    }
    let f = f64::from(zoom) / 2.0;
    let s = f64::from(sigma.clamp(MIN_RADIUS, MAX_RADIUS));
    let pi = std::f64::consts::PI;
    let kept = (-2.0 * pi * pi * s * s * f * f).exp();
    let left = 1.0 - kept * kept;
    if left <= 0.0 {
        return SEEN_FLOOR.min(iterations);
    }
    let needed = (SEEN_RESIDUAL.ln() / left.ln()).ceil();
    let needed = if needed.is_finite() && needed < iterations as f64 {
        needed as usize
    } else {
        iterations
    };
    needed.max(SEEN_FLOOR).min(iterations)
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
    sharpen_with_mask(image, None, options, measured, clip_level).0
}

/// [`sharpen`], and the blend mask it worked under: one plane, a
/// value in 0 to 1 a pixel, how far the sharpen went in there; zero
/// where the picture was flat or clipped. For showing where it acts.
///
/// `guide`, a picture of `image`'s size, is what the sharpen reads
/// what is flat from, in place of `image` (see [`deconvolved`]); the
/// deconvolution and the clip mask stay `image`'s.
pub fn sharpen_with_mask(
    image: &mut WorkingImage,
    guide: Option<&WorkingImage>,
    options: &SharpenOptions,
    measured: Option<f32>,
    clip_level: f32,
) -> (SharpenStats, Vec<f32>) {
    let d = deconvolved(image, guide, options, measured, clip_level);
    scale(image, &d.luminance, &d.sharpened, d.pedestal);
    (d.stats, d.blend)
}

/// [`sharpen_with_mask`], and what the sharpen did as a plane:
/// [`delta`] at each pixel, in stops, which [`apply_delta`] puts on a
/// pixel once more or takes off it.
pub fn sharpen_with_delta(
    image: &mut WorkingImage,
    guide: Option<&WorkingImage>,
    options: &SharpenOptions,
    measured: Option<f32>,
    clip_level: f32,
) -> (SharpenStats, Vec<f32>, Vec<f32>) {
    let d = deconvolved(image, guide, options, measured, clip_level);
    scale(image, &d.luminance, &d.sharpened, d.pedestal);
    let deltas = d.deltas();
    (d.stats, d.blend, deltas)
}

/// The plane [`sharpen_with_delta`] makes, with the picture left as
/// it was: what the sharpen would have done.
pub fn sharpen_delta(
    image: &WorkingImage,
    guide: Option<&WorkingImage>,
    options: &SharpenOptions,
    measured: Option<f32>,
    clip_level: f32,
) -> (SharpenStats, Vec<f32>, Vec<f32>) {
    let d = deconvolved(image, guide, options, measured, clip_level);
    let deltas = d.deltas();
    (d.stats, d.blend, deltas)
}

/// The sharpen's change to a pixel, in stops: the sharpened luminance
/// over the old, both on the pedestal. Shared.
#[inline]
pub fn delta(old: f32, new: f32) -> f32 {
    (new / old).log2()
}

/// `stops` of a sharpen's [`delta`] put on a pixel as the sharpen puts
/// its factor on, every channel scaled about the negative of
/// `pedestal`, the sharpen's ([`SharpenOptions::pedestal`]): one of
/// them on the picture before the sharpen is the sharpen, minus one on
/// the sharpened picture is the picture before it. Shared.
#[inline]
pub fn apply_delta(px: [f32; 3], stops: f32, pedestal: f32) -> [f32; 3] {
    let k = pedestal;
    let factor = stops.exp2();
    px.map(|c| (c + k) * factor - k)
}

/// The deconvolution's inputs and answer, before any channel moves.
struct Deconvolved {
    stats: SharpenStats,
    blend: Vec<f32>,
    /// The luminance on its pedestal, before and after.
    luminance: Vec<f32>,
    sharpened: Vec<f32>,
    /// The pedestal they are on, in the picture's levels.
    pedestal: f32,
}

impl Deconvolved {
    fn deltas(&self) -> Vec<f32> {
        self.luminance
            .par_iter()
            .zip(self.sharpened.par_iter())
            .map(|(&old, &new)| delta(old, new))
            .collect()
    }
}

fn deconvolved(
    image: &WorkingImage,
    guide: Option<&WorkingImage>,
    options: &SharpenOptions,
    measured: Option<f32>,
    clip_level: f32,
) -> Deconvolved {
    let radius = match options.radius {
        Radius::Auto => measured.unwrap_or(DEFAULT_RADIUS),
        Radius::Fixed(r) => r,
    }
    .clamp(MIN_RADIUS, MAX_RADIUS);
    let (w, h) = (image.width, image.height);
    let n = w * h;

    // Luminance on its pedestal, which is what the deconvolution works
    // on; the L* of the luminance itself, which is what the threshold
    // and the blend read; and the clip mask. The L* is the guide's
    // when there is one: what is flat is decided there, the rest here.
    if let Some(g) = guide {
        assert_eq!(
            (g.width, g.height),
            (w, h),
            "a guide of another size than the picture"
        );
    }
    let k = options.pedestal();
    let e = options.level_scale;
    let mut luminance = vec![0f32; n];
    let mut lstar = vec![0f32; n];
    let mut clip = vec![1f32; n];
    let reads = guide.unwrap_or(image);
    luminance
        .par_iter_mut()
        .zip(lstar.par_iter_mut())
        .zip(clip.par_iter_mut())
        .zip(image.data.par_chunks(3))
        .zip(reads.data.par_chunks(3))
        .for_each(|((((y, l), c), px), at)| {
            let raw = LUMA[0] * px[0] + LUMA[1] * px[1] + LUMA[2] * px[2];
            *y = raw.max(0.0) + k;
            *l = l_star((LUMA[0] * at[0] + LUMA[1] * at[1] + LUMA[2] * at[2]) * e);
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
    Deconvolved {
        stats: SharpenStats {
            radius,
            threshold,
            blend_mean,
            clipped,
            iterations: options.iterations,
        },
        blend,
        luminance,
        sharpened,
        pedestal: k,
    }
}

/// Every channel scaled about the pedestal's negative: a gain, so a
/// color keeps its ratios, wherever the pedestal is small beside the
/// channel, and an equal step in each channel where it is not. A
/// channel under the pedestal's negative stays there, scaled away from
/// it by a factor that the pedestal keeps near one.
fn scale(image: &mut WorkingImage, luminance: &[f32], sharpened: &[f32], k: f32) {
    image
        .data
        .par_chunks_mut(3)
        .zip(luminance.par_iter())
        .zip(sharpened.par_iter())
        .for_each(|((px, &old), &new)| {
            let factor = new / old;
            px[0] = (px[0] + k) * factor - k;
            px[1] = (px[1] + k) * factor - k;
            px[2] = (px[2] + k) * factor - k;
        });
}

/// The mean of a plane, summed in double so a large picture does not
/// saturate a single-precision total, and in a fixed order so a busy
/// machine gets the same threshold out of it.
fn mean(plane: &[f32]) -> f32 {
    (super::sum::sum_f64(plane, |&v| v as f64) / plane.len() as f64) as f32
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

/// How much of the pedestal the early stop's floor keeps under a
/// pixel; see [`stop_floor`]. Shared.
pub const FLOOR_PEDESTAL: f32 = 0.9;

/// The estimate under which a pixel stops its block's iterations, for
/// a start of `luminance` on the pedestal and a blend of `blend`.
///
/// RawTherapee's rule is half the blended start. Here it is read
/// nearly on the luminance rather than on the pedestal under it: where
/// the blend is full, a pixel stops its block when its estimate's
/// luminance falls under half its start less a tenth of the pedestal;
/// where the blend is nothing it stops nothing, as before. Taken on
/// the pedestal whole, half of it would come off the floor too, and a
/// pixel at black would have to fall half the pedestal under black:
/// the deconvolution on a pedestal, nearer linear in the dark than it
/// was, then left two thirds more dark halos on a base-ISO frame than
/// before, and moved the frame's export twice as much. Held to the
/// luminance exactly, a pixel resting at black (a flat patch whose
/// luminance is under zero, its estimate the pedestal to the last few
/// bits) would sit on its floor, and which side of it is the rounding:
/// the viewport and the export could stop a block an iteration apart.
/// A tenth of the pedestal is far past any rounding and leaves a
/// quarter more halos than before. Shared.
#[inline]
pub fn stop_floor(luminance: f32, blend: f32, pedestal: f32) -> f32 {
    ((luminance - pedestal) * 0.5 + pedestal * FLOOR_PEDESTAL) * blend
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
    let pedestal = options.pedestal();
    let mut out = luminance.to_vec();
    // The tiles worked: every one, or those the part touches.
    let (cols, bands) = match options.part {
        Some(part) => part.tiles(w, h, tile),
        None => (0..w.div_ceil(tile), 0..h.div_ceil(tile)),
    };
    out.par_chunks_mut(w * tile)
        .enumerate()
        .filter(|(band, _)| bands.contains(band))
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
            for x0 in (0..w)
                .step_by(tile)
                .filter(|x0| cols.contains(&(x0 / tile)))
            {
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
                                    stop_floor(luminance[(y0 + ty) * w + x0 + tx], b, pedestal);
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
            part: None,
            level_scale: 1.0,
        };
        let (stats, mask) = sharpen_with_mask(&mut image, None, &options, None, 10.0);
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
            part: None,
            level_scale: 1.0,
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
        // Channel ratios hold about the pedestal's negative: the sharpen
        // is a gain there, and so, to within the pedestal over the
        // level, a gain on the channels themselves.
        let mut color = blurred_edge(w, h, 1.0);
        for px in color.data.chunks_mut(3) {
            px[0] *= 1.5;
            px[2] *= 0.5;
        }
        let before = color.data.clone();
        sharpen(&mut color, &options, None, 10.0);
        let k = BLACK_PEDESTAL;
        for (px, was) in color.data.chunks(3).zip(before.chunks(3)) {
            for c in [0, 2] {
                let now = (px[c] + k) / (px[1] + k);
                let then = (was[c] + k) / (was[1] + k);
                assert!((now - then).abs() < 1e-4, "{now} against {then}");
            }
            assert!((px[0] / px[1] - 1.5).abs() < 0.02 && (px[2] / px[1] - 0.5).abs() < 0.02);
        }
    }

    #[test]
    fn noise_about_black_is_not_blown_up_into_dots() {
        // A high-ISO black after the camera matrix: every channel noise
        // about zero, the luminance negative at about half the pixels,
        // beside a blurred edge up to a mid grey. Sharpened everywhere
        // (a threshold of zero, which is where the automatic one lands
        // on a frame with no flat patch to read), with the editor's
        // other settings. Richardson–Lucy on that luminance, and the
        // division by it, made single pixels hundreds of times their
        // noise; the noise at black must stay noise.
        let (w, h) = (128, 96);
        let mut image = WorkingImage::new(w, h);
        let mut seed = 0x2545_F491_u32;
        let mut rand = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32) - 0.5
        };
        for y in 0..h {
            for x in 0..w {
                let d = (x as f32 - 80.5) / (0.8 * std::f32::consts::SQRT_2);
                let level = 0.3 * (1.0 + erf(d)) / 2.0;
                let i = (y * w + x) * 3;
                for c in 0..3 {
                    image.data[i + c] = level + 0.006 * rand();
                }
            }
        }
        let before = image;
        let negative = before
            .data
            .chunks(3)
            .filter(|px| LUMA[0] * px[0] + LUMA[1] * px[1] + LUMA[2] * px[2] < 0.0)
            .count();
        assert!(negative > w * h / 4, "{negative} pixels under black");
        // The edge's steepest step, summed down the rows: sharper is
        // steeper.
        let slope = |im: &WorkingImage| -> f32 {
            (0..h)
                .map(|y| {
                    (70..90)
                        .map(|x| {
                            let i = (y * w + x) * 3;
                            let sum = |j: usize| im.data[j..j + 3].iter().sum::<f32>();
                            sum(i + 3) - sum(i)
                        })
                        .fold(0.0, f32::max)
                })
                .sum()
        };
        // With the early stop, the black's own noise sharpened under the
        // floor ends the iterations of the blocks it is in, the edge's
        // among them; without it the edge is sharpened in full.
        for (stop_early, sharper) in [(true, 1.05), (false, 1.3)] {
            let mut image = before.clone();
            let options = SharpenOptions {
                contrast: Threshold::Fixed(0.0),
                radius: Radius::Fixed(0.6),
                stop_early,
                ..Default::default()
            };
            sharpen(&mut image, &options, None, 10.0);
            // In the black, well clear of the edge, no channel moves by
            // more than the noise's own spread.
            let mut worst = 0f32;
            for y in 0..h {
                for x in 0..64 {
                    let i = (y * w + x) * 3;
                    for c in 0..3 {
                        worst = worst.max((image.data[i + c] - before.data[i + c]).abs());
                    }
                }
            }
            assert!(
                worst < 0.006,
                "stopping early {stop_early}: a channel in the black moved by {worst}"
            );
            let (was, is) = (slope(&before), slope(&image));
            assert!(
                is > was * sharper,
                "stopping early {stop_early}: the edge's slope {was} -> {is}"
            );
        }
    }

    /// The delta is the sharpen: put on the picture before it, it is
    /// the sharpened picture; taken off the sharpened, it is the
    /// picture before; and made without the sharpen it is the same.
    #[test]
    fn the_delta_is_the_sharpen_on_and_off() {
        let (w, h) = (96, 64);
        let before = blurred_edge(w, h, 1.0);
        let options = SharpenOptions {
            radius: Radius::Fixed(1.0),
            contrast: Threshold::Fixed(0.0),
            ..Default::default()
        };
        let mut sharpened = before.clone();
        let (stats, mask, delta) = sharpen_with_delta(&mut sharpened, None, &options, None, 10.0);
        let mut plain = before.clone();
        let (stats_plain, mask_plain) = sharpen_with_mask(&mut plain, None, &options, None, 10.0);
        assert_eq!((stats, &mask), (stats_plain, &mask_plain));
        assert_eq!(sharpened, plain, "the same sharpen");
        let (_, _, again) = sharpen_delta(&before, None, &options, None, 10.0);
        assert_eq!(again, delta);
        assert!(
            delta.iter().any(|d| d.abs() > 0.05),
            "the sharpen did something"
        );
        let (mut on, mut off) = (0f32, 0f32);
        for (i, &d) in delta.iter().enumerate() {
            let px = |im: &WorkingImage| [im.data[i * 3], im.data[i * 3 + 1], im.data[i * 3 + 2]];
            let a = apply_delta(px(&before), d, BLACK_PEDESTAL);
            let b = apply_delta(px(&sharpened), -d, BLACK_PEDESTAL);
            for c in 0..3 {
                on = on.max((a[c] - sharpened.data[i * 3 + c]).abs());
                off = off.max((b[c] - before.data[i * 3 + c]).abs());
            }
        }
        assert!(on < 1e-5 && off < 1e-5, "on {on}, off {off}");
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

    /// The threshold's units are RawTherapee's: its panel's percentage
    /// over a hundred, against a contrast taken on L* scaled to 32768
    /// times `0.0625 / 327.68` (`buildBlendMask` in `rt_algo.cc`). So
    /// our 0, 50, and 200 percent are its 0, 50, and 200, the same
    /// blend at every contrast; at 0 both blend everything in full.
    #[test]
    fn the_threshold_is_rawtherapees_at_0_50_and_200_percent() {
        // RawTherapee's, in its units: L 0 to 32768 and the panel's
        // integer percentage.
        let rt_blend = |l: &[f32; 5], percent: f32| -> f32 {
            let t = percent / 100.0;
            if t == 0.0 {
                return 1.0;
            }
            let scale = 0.0625 / 327.68;
            let d1 = l[3] - l[1];
            let d2 = l[4] - l[0];
            let c = (d1 * d1 + d2 * d2).sqrt() * scale;
            let x = -16.0 + (16.0 / t) * c;
            0.5 * (1.0 + x / (1.0 + x * x).sqrt())
        };
        // A 5x5 patch, a ramp along x, flat along y: the contrast at
        // the center reads only the row.
        for slope in [0.0f32, 0.5, 2.0, 8.0, 15.0, 30.0, 60.0] {
            let row: [f32; 5] = std::array::from_fn(|x| 50.0 + slope * x as f32);
            let lstar: Vec<f32> = (0..25).map(|i| row[i % 5]).collect();
            let ours_c = contrast(&lstar, 5, 2, 2);
            let theirs = row.map(|v| v * 327.68);
            for percent in [50.0f32, 200.0] {
                let ours = blend_factor(ours_c, percent / 100.0);
                let rt = rt_blend(&theirs, percent);
                assert!(
                    (ours - rt).abs() < 1e-5,
                    "{slope} at {percent}%: {ours} vs {rt}"
                );
            }
            assert_eq!(rt_blend(&theirs, 0.0), 1.0);
        }
        // Half at the threshold, on both: a contrast of 0.5 at 50
        // percent, of 2 at 200.
        assert!((blend_factor(0.5, 0.5) - 0.5).abs() < 1e-6);
        assert!((blend_factor(2.0, 2.0) - 0.5).abs() < 1e-6);
        // Zero blends every unclipped pixel in full, as RawTherapee's.
        let lstar: Vec<f32> = (0..100).map(|i| (i % 7) as f32 * 9.0).collect();
        let clip = vec![1f32; 100];
        assert!(
            blend_mask(&lstar, &clip, 10, 10, 0.0)
                .iter()
                .all(|&b| b == 1.0)
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
            part: None,
            level_scale: 1.0,
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

    /// A noisy flat field, the grain of a high ISO, with a block of
    /// bars blurred as a lens blurs them: the flattest patch for the
    /// threshold, and detail for the sharpen.
    fn noisy_with_detail(w: usize, h: usize) -> WorkingImage {
        let mut seed = 0x1234_5678_u32;
        let mut rand = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32) - 0.5
        };
        let mut lum = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let bars = x < w / 3 && ((x / 4) + (y / 6)) % 2 == 0;
                lum[y * w + x] = if bars { 0.45 } else { 0.18 };
            }
        }
        super::super::dual::gaussian_blur(&mut lum, w, h, 1.0);
        let mut image = WorkingImage::new(w, h);
        for (px, l) in image.data.chunks_mut(3).zip(&lum) {
            let v = l + 0.012 * rand();
            px.copy_from_slice(&[v * 1.1, v, v * 0.9]);
        }
        image
    }

    /// Texture lifts the flattest patch's grain, which moved the
    /// automatic threshold (to zero, at a high ISO: everything
    /// sharpened) and the blend with it. Read off the picture before
    /// it, neither moves with Texture, and the bars are still
    /// sharpened where they were.
    #[test]
    fn texture_does_not_move_what_the_sharpen_calls_flat() {
        use super::super::local_contrast::{LocalContrastOptions, local_contrast};
        let (w, h) = (320, 240);
        let before = noisy_with_detail(w, h);
        let options = SharpenOptions::default();
        let (base, base_mask) = sharpen_with_mask(&mut before.clone(), None, &options, None, 10.0);
        assert!(base.threshold > 0.0, "a flat patch to read: {base:?}");
        assert!(base.blend_mean < 0.6, "the flat left alone: {base:?}");
        let mut moved = false;
        for texture in [0.5f32, 1.0, -1.0] {
            let mut textured = before.clone();
            let lc = LocalContrastOptions {
                texture,
                ..Default::default()
            };
            local_contrast(&mut textured, &lc, 10.0);
            let unguided = sharpen_with_mask(&mut textured.clone(), None, &options, None, 10.0).0;
            moved |= unguided.threshold != base.threshold;
            let (guided, mask) =
                sharpen_with_mask(&mut textured, Some(&before), &options, None, 10.0);
            assert_eq!(guided.threshold, base.threshold, "Texture {texture}");
            assert_eq!(guided.blend_mean, base.blend_mean, "Texture {texture}");
            assert_eq!(mask, base_mask, "Texture {texture}");
            // The bars are sharpened, the flat is not.
            let at = |x: usize, y: usize| mask[y * w + x];
            assert!(at(w / 6, h / 2) > 0.5, "the bars, Texture {texture}");
            assert!(at(3 * w / 4, h / 2) < 0.1, "the flat, Texture {texture}");
        }
        assert!(moved, "the picture after Texture moves the threshold");
        // A fixed threshold is the slider's and stays it; the blend
        // reads the guide.
        let fixed = SharpenOptions {
            contrast: Threshold::Fixed(0.3),
            ..options
        };
        let mut textured = before.clone();
        let lc = LocalContrastOptions {
            texture: 1.0,
            ..Default::default()
        };
        local_contrast(&mut textured, &lc, 10.0);
        let (plain, plain_mask) = sharpen_with_mask(&mut before.clone(), None, &fixed, None, 10.0);
        let (guided, mask) = sharpen_with_mask(&mut textured, Some(&before), &fixed, None, 10.0);
        assert_eq!((guided.threshold, plain.threshold), (0.3, 0.3));
        assert_eq!(mask, plain_mask);
    }

    /// A guide that is the picture is no guide: the same sharpen, to
    /// the bit, mask, delta, and stats.
    #[test]
    fn the_picture_as_its_own_guide_is_the_unguided_sharpen() {
        let before = noisy_with_detail(200, 150);
        let options = SharpenOptions::default();
        let (mut a, mut b) = (before.clone(), before.clone());
        let plain = sharpen_with_delta(&mut a, None, &options, Some(0.8), 10.0);
        let guided = sharpen_with_delta(&mut b, Some(&before), &options, Some(0.8), 10.0);
        assert_eq!(plain, guided);
        assert_eq!(a, b);
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

    /// A picture with detail and grain in every tile, flat enough in
    /// a corner for the automatic threshold to read.
    fn busy(w: usize, h: usize) -> WorkingImage {
        let mut image = WorkingImage::new(w, h);
        let mut seed = 0x9E37_79B9_u32;
        for y in 0..h {
            for x in 0..w {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let grain = (seed as f32 / u32::MAX as f32 - 0.5) * 0.01;
                let (fx, fy) = (x as f32, y as f32);
                let detail = if x < w / 5 && y < h / 5 {
                    0.0
                } else {
                    0.15 * (fx * 0.9).sin() * (fy * 0.7).cos()
                        + if (x / 9 + y / 7) % 2 == 0 { 0.1 } else { -0.1 }
                };
                let v = 0.3 + detail + grain;
                let i = (y * w + x) * 3;
                image.data[i] = v * 1.1;
                image.data[i + 1] = v;
                image.data[i + 2] = v * 0.8;
            }
        }
        image
    }
    /// A scene from deep shadow to highlight: steps of a stop and a
    /// half, 80 pixels wide, with a grain on them, crossed by bands of blurred bars, so the
    /// automatic threshold has flat patches to read and the blend has
    /// edges at every level.
    fn ramp_scene(w: usize, h: usize) -> WorkingImage {
        let mut seed = 0x9E37_79B9_u32;
        let mut grain = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32) - 0.5
        };
        let mut lum: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if y % 96 < 24 && (x / 5) % 2 == 0 {
                    0.35
                } else {
                    1.0
                }
            })
            .collect();
        // Bars blurred across, by a small box.
        let sharp = lum.clone();
        for y in 0..h {
            for x in 1..w - 1 {
                lum[y * w + x] =
                    (sharp[y * w + x - 1] + 2.0 * sharp[y * w + x] + sharp[y * w + x + 1]) / 4.0;
            }
        }
        let mut image = WorkingImage::new(w, h);
        for (i, px) in image.data.chunks_mut(3).enumerate() {
            let level = 0.002 * (1.5 * ((i % w) / 80) as f32).exp2();
            for (c, v) in px.iter_mut().enumerate() {
                let tint = [1.1, 1.0, 0.8][c];
                *v = level * lum[i] * tint * (1.0 + 0.1 * grain());
            }
        }
        image
    }

    /// A part's tiles are the whole picture's sharpen of them, to the
    /// bit, and every other pixel is left as it was; the stats are the
    /// whole picture's; and a part over every tile is the whole
    /// picture's sharpen everywhere.
    #[test]
    fn a_part_sharpens_its_tiles_as_the_whole_picture_does() {
        let (w, h) = (300, 200);
        let image = busy(w, h);
        let tile = tile_for(h);
        assert_eq!(tile, 32);
        let whole = SharpenOptions::default();
        let mut full = image.clone();
        let (full_stats, full_mask) = sharpen_with_mask(&mut full, None, &whole, Some(0.8), 2.0);
        let part = Part {
            x0: 70,
            y0: 40,
            x1: 150,
            y1: 90,
        };
        let (cols, bands) = part.tiles(w, h, tile);
        assert_eq!((cols.clone(), bands.clone()), (2..5, 1..3));
        let mut partial = image.clone();
        let options = SharpenOptions {
            part: Some(part),
            ..whole
        };
        let (stats, mask) = sharpen_with_mask(&mut partial, None, &options, Some(0.8), 2.0);
        assert_eq!(stats, full_stats);
        assert_eq!(mask, full_mask);
        let k = BLACK_PEDESTAL;
        let (mut inside, mut moved) = (0, 0);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                let touched = cols.contains(&(x / tile)) && bands.contains(&(y / tile));
                for c in 0..3 {
                    let (got, was) = (partial.data[i + c], image.data[i + c]);
                    if touched {
                        assert_eq!(got.to_bits(), full.data[i + c].to_bits(), "{x},{y}");
                        inside += 1;
                    } else {
                        assert_eq!(got, (was + k) - k, "{x},{y}");
                    }
                    moved += usize::from(touched && full.data[i + c] != was);
                }
            }
        }
        assert_eq!(inside, 3 * 32 * 2 * 32 * 3);
        assert!(moved > inside / 2, "the part's tiles were sharpened");
        // Over every tile: the whole picture's sharpen.
        let all = SharpenOptions {
            part: Some(Part {
                x0: 0,
                y0: 0,
                x1: w,
                y1: h,
            }),
            ..whole
        };
        assert!(all.part.unwrap().covers(w, h, tile));
        assert!(!part.covers(w, h, tile));
        let mut every = image.clone();
        sharpen_with_mask(&mut every, None, &all, Some(0.8), 2.0);
        assert_eq!(every.data, full.data);
        // Off the picture, or of no area: nothing deconvolved.
        for off in [
            Part {
                x0: w,
                y0: 0,
                x1: w + 10,
                y1: h,
            },
            Part {
                x0: 10,
                y0: 10,
                x1: 10,
                y1: 50,
            },
        ] {
            assert_eq!(off.tiles(w, h, tile), (0..0, 0..0));
        }
    }

    /// The iterations a view needs: all of them at 1:1 and above and
    /// for no zoom at all; below it, fewer the further out, never
    /// under the floor, and more for a wider point spread.
    #[test]
    fn a_view_further_out_needs_fewer_iterations() {
        let n = DEFAULT_ITERATIONS;
        for zoom in [1.0, 2.0, 0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(iterations_seen_at(zoom, 0.74, n), n, "{zoom}");
        }
        let at = |zoom: f32| iterations_seen_at(zoom, 0.74, n);
        assert_eq!(
            [0.1, 0.2, 0.3, 0.4, 0.5, 0.7].map(at),
            [8, 8, 8, 11, 19, 20]
        );
        let mut last = 0;
        for i in 1..100 {
            let k = at(i as f32 / 100.0);
            assert!(k >= last && (SEEN_FLOOR..=n).contains(&k), "{i}: {k}");
            last = k;
        }
        assert!(iterations_seen_at(0.2, 1.5, n) > at(0.2));
        // Never more than the edit asks for.
        assert_eq!(iterations_seen_at(0.4, 0.74, 8), 8);
        assert_eq!(iterations_seen_at(0.2, 0.74, 3), 3);
    }
    /// `scene` stored `stops` under itself, as a merge with that much
    /// headroom stores its samples.
    fn stored_under(scene: &WorkingImage, stops: f32) -> WorkingImage {
        let mut merged = scene.clone();
        let e = stops.exp2();
        merged.data.iter_mut().for_each(|v| *v /= e);
        merged
    }

    /// A merge with headroom sharpens as its reference frame does: the
    /// threshold, the blend, and the picture read at the exposed scale.
    /// Two stops is a power of two, under which every step of the
    /// sharpen scales exactly, so the two are the same to the bit; at
    /// a stop and a half, to the rounding. Read at its own levels, the
    /// merge's shadows were sharpened less.
    #[test]
    fn a_merge_with_headroom_sharpens_as_its_reference_frame() {
        let (w, h) = (480, 288);
        let scene = ramp_scene(w, h);
        let options = SharpenOptions {
            radius: Radius::Fixed(0.8),
            ..Default::default()
        };
        let mut reference = scene.clone();
        let want = sharpen(&mut reference, &options, None, f32::INFINITY);
        assert!(want.blend_mean > 0.05 && want.blend_mean < 0.95, "{want:?}");

        let mut merged = stored_under(&scene, 2.0);
        let got = sharpen(&mut merged, &options.at_headroom(2.0), None, f32::INFINITY);
        assert_eq!(got, want);
        let back: Vec<f32> = merged.data.iter().map(|v| v * 4.0).collect();
        assert_eq!(back, reference.data, "two stops of headroom, to the bit");

        let stops = 1.55f32;
        let e = stops.exp2();
        let mut merged = stored_under(&scene, stops);
        let got = sharpen(
            &mut merged,
            &options.at_headroom(stops),
            None,
            f32::INFINITY,
        );
        assert_eq!(got.threshold, want.threshold);
        assert!((got.blend_mean - want.blend_mean).abs() < 1e-5, "{got:?}");
        let worst = merged
            .data
            .iter()
            .zip(&reference.data)
            .map(|(m, r)| (m * e - r).abs() / r.abs().max(BLACK_PEDESTAL))
            .fold(0f32, f32::max);
        assert!(worst < 1e-4, "{worst:.3e} apart at {stops} stops");

        // At its own levels the same merge is sharpened less where it
        // is dark: what this test exists for.
        let mut unread = stored_under(&scene, stops);
        let wrong = sharpen(&mut unread, &options, None, f32::INFINITY);
        assert!(
            wrong.blend_mean < want.blend_mean - 0.01,
            "{wrong:?} against {want:?}"
        );
    }

    /// A file with no headroom keeps the options and the pedestal as
    /// they were, to the bit, and so its picture: a scale of one is a
    /// multiply and a divide by one.
    #[test]
    fn no_headroom_leaves_the_sharpen_as_it_was() {
        let options = SharpenOptions::default();
        let at = options.at_headroom(0.0);
        assert_eq!(at, options);
        assert_eq!(at.level_scale.to_bits(), 1.0f32.to_bits());
        assert_eq!(at.pedestal().to_bits(), BLACK_PEDESTAL.to_bits());
        let scene = ramp_scene(240, 192);
        let (mut a, mut b) = (scene.clone(), scene.clone());
        let sa = sharpen_with_delta(&mut a, None, &options, Some(0.7), 2.0);
        let sb = sharpen_with_delta(&mut b, None, &at, Some(0.7), 2.0);
        assert_eq!(sa, sb);
        assert_eq!(a.data, b.data);
        assert_ne!(a.data, scene.data, "it sharpened");
    }
}
