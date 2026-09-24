//! Haze removal by the dark channel prior. Haze is the atmosphere's
//! own light laid over the scene, `I = J t + A (1 - t)`: `J` the
//! scene's radiance, `A` the airlight, `t` the transmission, which
//! falls with distance. He, Sun and Tang's prior (CVPR 2009): in a
//! clear outdoor scene nearly every patch has some channel near zero,
//! so the darkest channel over a window, divided by the airlight, is
//! a read of `1 - t`. The airlight is read off the haziest, brightest
//! pixels; the transmission map is refined by a guided filter (He,
//! Sun and Tang, ECCV 2010) so it follows the picture's edges rather
//! than the window's blocks, then floored, and the scene recovered as
//! `J = (I - A) / t + A`, on all three channels alike, which keeps
//! hue.
//!
//! After darktable's haze removal module, `src/iop/hazeremoval.c`,
//! copyright 2017 Heiko Bauke, GPL-3.0-or-later: the airlight from
//! the dark channel's brightest tail, the strength scaling the dark
//! channel, and its negative putting haze in. Differences: the dark
//! channel, the airlight and the guided filter's coefficients are
//! found on a reduced copy of the picture, each reduced pixel the
//! mean of its block, and the transmission put back at full size
//! through the filter's own linear model in the full-size luminance
//! (He and Sun's fast guided filter, 2015); the guide is the
//! luminance rather than the three channels, with a window and a
//! regularization of its own; the dark channel's window is the block
//! itself, not darktable's `w1`; every pixel's transmission is
//! bounded below by what its own dark channel says, which keeps the
//! recovered radiance at or above zero; the airlight is held near
//! neutral; the floor is a constant rather than darktable's distance
//! quantile; and the units are the working space's.

use crate::image::WorkingImage;
use rayon::prelude::*;

/// Luminance weights of the working space, Rec.2020.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// About the long edge the reduced copy is brought to; the windows
/// below are in its pixels, so they cover the same share of the
/// picture whatever its size. A reduced pixel is the mean of its
/// block, four by four at 24 MP.
const REDUCED_EDGE: usize = 1536;
/// Half-width of the window the airlight's dark channel is taken
/// over, reduced pixels. The transmission's own dark channel has no
/// window beyond the reduced pixel's block: darktable's `w1` of 6
/// pixels at full size is about that block, and a wider window
/// spreads a dark object's "no haze" into the sky around it, a rim
/// that no filter after it can take back.
const AIRLIGHT_WINDOW: usize = 1;
/// Half-width of the guided filter's window, reduced pixels: 16 at
/// full size on a 24 MP frame. darktable's `w2` is 9 at full size;
/// wider here than that, since the block means have already taken
/// the pixel noise, and no wider, since a window that reaches from
/// a leaf into the sky around it lends the sky the leaf's clear
/// transmission as a halo the width of the window.
const GUIDE_WINDOW: usize = 4;
/// The guided filter's regularization against the guide's variance,
/// on a scale of one: small, so the line follows an edge between
/// sky and leaf rather than blurring across it. darktable's 0.025
/// is against a full-size pixel's noise; a block mean has little.
const GUIDE_EPS: f32 = 0.0003;
/// The strength the amount's end maps to. The strength scales the
/// dark channel into the transmission, and at one the prior is
/// believed whole: the haziest pixels are taken as pure airlight,
/// and with the per-pixel bound in [`Model::transmission`] holding
/// the radiance at or above zero, the recovered dark channel of any
/// pixel the bound bites on is `A d (1 - s) / (1 - s d)`, which is
/// zero at `s = 1`: a whole sky with its darkest channels at zero, a
/// gain of five at the floor, and every mismatch between the grid's
/// transmission and a pixel's own drawn as an outline. He, Sun and
/// Tang keep `omega = 0.95` for the same reason, aerial perspective;
/// 0.8 is where the outlines and the rim along a ridge are gone at
/// the slider's end on the frames looked at, and the darkest channel
/// keeps at least a fifth of what it had.
pub const FULL_STRENGTH: f32 = 0.8;
/// Transmission below which the recovery stops: He, Sun and Tang's
/// `t0`. Under it the model is amplifying noise, not restoring
/// radiance.
pub const MIN_TRANSMISSION: f32 = 0.2;
/// Quantile of the dark channel above which a pixel counts as hazy,
/// and of luminance among those for the brightest, darktable's.
const HAZY_QUANTILE: f32 = 0.95;
const BRIGHT_QUANTILE: f32 = 0.99;
/// The airlight can be no darker than this, so a black frame still
/// divides by something.
const MIN_AIRLIGHT: f32 = 1e-4;
/// No channel of the airlight is below this share of its brightest.
/// Haze is scattered light, near neutral or warm; a channel far
/// under the others is a bright object's color, which is where the
/// estimate lands on a picture with no haze in it, and without the
/// floor every pixel sharing that object's dark channel would read
/// as deep haze.
const AIRLIGHT_NEUTRALITY: f32 = 0.25;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DehazeOptions {
    /// -1 to 1: how much of the dark channel's haze to take out; at
    /// one the prior is believed whole, and a negative amount puts
    /// haze in instead.
    pub amount: f32,
}

/// What a dehaze did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DehazeStats {
    /// The airlight read off the picture, working-space RGB.
    pub airlight: [f32; 3],
    /// The amount asked for, clamped to -1..1.
    pub amount: f32,
    /// The strength the dark channel was scaled by: the amount times
    /// [`FULL_STRENGTH`].
    pub strength: f32,
    /// The reduction the map was made at, a factor on each edge.
    pub factor: usize,
    /// Mean and least of the transmission applied, after the floor.
    pub transmission_mean: f32,
    pub transmission_min: f32,
}

/// The transmission map's coefficients on the reduced grid: `t = a *
/// luminance + b` at every full-size pixel, bilinearly between
/// them, and no less than the pixel's own dark channel says.
struct Model {
    width: usize,
    height: usize,
    factor: usize,
    a: Vec<f32>,
    b: Vec<f32>,
    airlight: [f32; 3],
    strength: f32,
}

/// Remove haze from the working image in place, by `options.amount`.
/// Nothing at zero.
pub fn dehaze(image: &mut WorkingImage, options: &DehazeOptions) -> DehazeStats {
    run(image, options, false).0
}

/// [`dehaze`], and the transmission it applied: one value a pixel,
/// at full size, in 0.2 to 1 for haze taken out and 1 up for haze
/// put in. For looking at what the prior made of a picture.
pub fn dehaze_with_map(
    image: &mut WorkingImage,
    options: &DehazeOptions,
) -> (DehazeStats, Vec<f32>) {
    let (stats, map) = run(image, options, true);
    (stats, map.expect("the map was asked for"))
}

fn run(
    image: &mut WorkingImage,
    options: &DehazeOptions,
    want_map: bool,
) -> (DehazeStats, Option<Vec<f32>>) {
    let amount = options.amount.clamp(-1.0, 1.0);
    let strength = amount * FULL_STRENGTH;
    let (w, h) = (image.width, image.height);
    if strength == 0.0 || w == 0 || h == 0 {
        return (
            DehazeStats {
                airlight: [0.0; 3],
                amount,
                strength,
                factor: 1,
                transmission_mean: 1.0,
                transmission_min: 1.0,
            },
            want_map.then(|| vec![1.0; w * h]),
        );
    }
    let model = fit(image, strength);
    let columns = taps(w, model.width, model.factor);
    let a = model.airlight;
    let mut map = want_map.then(|| vec![0f32; w * h]);
    // A row of the map to fill for each row of the picture, or none.
    let map_rows: Vec<Option<&mut [f32]>> = match map.as_mut() {
        Some(m) => m.chunks_mut(w).map(Some).collect(),
        None => (0..h).map(|_| None).collect(),
    };
    let (sum, min) = image
        .data
        .par_chunks_mut(w * 3)
        .zip(map_rows.into_par_iter())
        .enumerate()
        .map(|(y, (row, map_row))| clear_row(row, map_row, y, &model, &columns))
        .reduce(|| (0.0, f32::INFINITY), |x, y| (x.0 + y.0, x.1.min(y.1)));
    (
        DehazeStats {
            airlight: a,
            amount,
            strength,
            factor: model.factor,
            transmission_mean: (sum / (w * h) as f64) as f32,
            transmission_min: min,
        },
        map,
    )
}

/// One row of the picture cleared of its haze, and its transmission
/// written to `map_row` when there is one: the sum and the least of
/// the row's transmissions.
///
/// Out of line so the rows arrive as arguments. Inside the closure the
/// picture's row comes out of rayon's enumerate tuple and the map's out
/// of a vector, and a reference loaded from memory carries no promise
/// that it is distinct from anything else, so every write to a pixel
/// had the model's fields read again for the next.
#[inline(never)]
fn clear_row(
    row: &mut [f32],
    mut map_row: Option<&mut [f32]>,
    y: usize,
    model: &Model,
    columns: &[(usize, usize, f32)],
) -> (f64, f32) {
    let (y0, y1, fy) = tap(y, model.height, model.factor);
    let a = model.airlight;
    let mut sum = 0f64;
    let mut min = f32::INFINITY;
    for (x, (px, &(x0, x1, fx))) in row
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .zip(columns)
        .enumerate()
    {
        let t = model.transmission(x0, x1, fx, y0, y1, fy, px);
        for (v, &a) in px.iter_mut().zip(&a) {
            *v = (*v - a) / t + a;
        }
        if let Some(m) = map_row.as_mut() {
            m[x] = t;
        }
        sum += t as f64;
        min = min.min(t);
    }
    (sum, min)
}

/// The reduction a picture of this size gets: the smallest factor
/// that brings its long edge under [`REDUCED_EDGE`].
pub fn reduce_factor(width: usize, height: usize) -> usize {
    width.max(height).div_ceil(REDUCED_EDGE).max(1)
}

impl Model {
    /// The transmission at a full-size pixel whose bilinear taps on
    /// the reduced grid are given, from the pixel itself: the model's
    /// line in its luminance, but no less than the pixel's own dark
    /// channel reads, floored, and held under one when haze is taken
    /// out.
    ///
    /// The bound is the model's own consistency: a pixel of `I` under
    /// airlight `A` can only have `I >= A (1 - t)`, so `t >= 1 -
    /// I_c / A_c` on every channel, and the recovered radiance stays
    /// at or above zero. Without it a leaf thinner than a reduced
    /// pixel takes the sky's transmission from the grid around it and
    /// comes out below black, as a color the sky never had.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn transmission(
        &self,
        x0: usize,
        x1: usize,
        fx: f32,
        y0: usize,
        y1: usize,
        fy: f32,
        px: &[f32; 3],
    ) -> f32 {
        let w = self.width;
        let lerp = |p: &[f32]| {
            let top = p[y0 * w + x0] + (p[y0 * w + x1] - p[y0 * w + x0]) * fx;
            let bottom = p[y1 * w + x0] + (p[y1 * w + x1] - p[y1 * w + x0]) * fx;
            top + (bottom - top) * fy
        };
        let lum = LUMA[0] * px[0] + LUMA[1] * px[1] + LUMA[2] * px[2];
        let t = lerp(&self.a) * lum + lerp(&self.b);
        if self.strength > 0.0 {
            let own = 1.0 - self.strength * dark_ratio(px, &self.airlight);
            t.max(own).clamp(MIN_TRANSMISSION, 1.0)
        } else {
            t.clamp(1.0, 1.0 - self.strength)
        }
    }
}

/// The dark channel of one pixel over the airlight: the least of its
/// channels' ratios, held to 0..1. Above the airlight a pixel is
/// brighter than any haze could make it, and says nothing about the
/// haze.
#[inline]
fn dark_ratio(px: &[f32; 3], airlight: &[f32; 3]) -> f32 {
    (px[0] / airlight[0])
        .min(px[1] / airlight[1])
        .min(px[2] / airlight[2])
        .clamp(0.0, 1.0)
}

/// The bilinear taps on the reduced grid of full-size coordinate
/// `i`: the two reduced indices and the weight of the second.
#[inline]
fn tap(i: usize, reduced: usize, factor: usize) -> (usize, usize, f32) {
    let u = (i as f32 + 0.5) / factor as f32 - 0.5;
    if u <= 0.0 {
        return (0, 0, 0.0);
    }
    let i0 = (u.floor() as usize).min(reduced - 1);
    let i1 = (i0 + 1).min(reduced - 1);
    (i0, i1, u - i0 as f32)
}

fn taps(full: usize, reduced: usize, factor: usize) -> Vec<(usize, usize, f32)> {
    (0..full).map(|i| tap(i, reduced, factor)).collect()
}

/// Fit the model: reduce, read the airlight, take the dark channel
/// for a transmission, refine it by the guided filter and keep the
/// filter's coefficients.
fn fit(image: &WorkingImage, strength: f32) -> Model {
    let (w, h) = (image.width, image.height);
    let factor = reduce_factor(w, h);
    let (rw, rh, rgb) = reduce(image, factor);
    let n = rw * rh;
    let luminance: Vec<f32> = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .map(|px| LUMA[0] * px[0] + LUMA[1] * px[1] + LUMA[2] * px[2])
        .collect();

    let airlight = airlight(&rgb, &luminance, rw, rh);

    // The transmission the prior reads: one minus the strength's
    // share of the dark channel of the picture over its airlight,
    // each reduced pixel's block its own window.
    let t0: Vec<f32> = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .map(|px| 1.0 - strength * dark_ratio(px, &airlight))
        .collect();

    // The guided filter (He, Sun and Tang): in each window the
    // transmission is taken as a line in the guide, `a g + b`, with
    // the slope regularized toward zero where the guide is flat, and
    // each pixel gets the mean of the lines of the windows it is in.
    let mean_g = box_mean(&luminance, rw, rh, GUIDE_WINDOW);
    let mean_t = box_mean(&t0, rw, rh, GUIDE_WINDOW);
    let gg: Vec<f32> = luminance.iter().map(|g| g * g).collect();
    let gt: Vec<f32> = luminance.iter().zip(&t0).map(|(g, t)| g * t).collect();
    let corr_gg = box_mean(&gg, rw, rh, GUIDE_WINDOW);
    let corr_gt = box_mean(&gt, rw, rh, GUIDE_WINDOW);
    let mut a = vec![0f32; n];
    let mut b = vec![0f32; n];
    for i in 0..n {
        let var = (corr_gg[i] - mean_g[i] * mean_g[i]).max(0.0);
        let cov = corr_gt[i] - mean_g[i] * mean_t[i];
        a[i] = cov / (var + GUIDE_EPS);
        b[i] = mean_t[i] - a[i] * mean_g[i];
    }
    let a = box_mean(&a, rw, rh, GUIDE_WINDOW);
    let b = box_mean(&b, rw, rh, GUIDE_WINDOW);
    Model {
        width: rw,
        height: rh,
        factor,
        a,
        b,
        airlight,
        strength,
    }
}

/// The picture reduced by `factor` on each edge, each reduced pixel
/// the mean of its block; a block cut by the edge is the mean of what
/// is there. Interleaved RGB, and the reduced size.
fn reduce(image: &WorkingImage, factor: usize) -> (usize, usize, Vec<f32>) {
    let (w, h) = (image.width, image.height);
    if factor == 1 {
        return (w, h, image.data.clone());
    }
    let rw = w.div_ceil(factor);
    let rh = h.div_ceil(factor);
    let mut out = vec![0f32; rw * rh * 3];
    out.par_chunks_mut(rw * 3)
        .zip(image.data.par_chunks(w * 3 * factor))
        .for_each(|(out, rows)| {
            let rows_here = rows.len() / (w * 3);
            for row in rows.chunks_exact(w * 3) {
                for (i, px) in row.as_chunks::<3>().0.iter().enumerate() {
                    let o = &mut out[(i / factor) * 3..(i / factor) * 3 + 3];
                    o[0] += px[0];
                    o[1] += px[1];
                    o[2] += px[2];
                }
            }
            for (i, o) in out.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let cols_here = (w - i * factor).min(factor);
                let count = (rows_here * cols_here) as f32;
                o[0] /= count;
                o[1] /= count;
                o[2] /= count;
            }
        });
    (rw, rh, out)
}

/// The airlight: the mean color of the brightest of the haziest
/// pixels, where hazy is the top of the dark channel and bright the
/// top of the luminance among those, darktable's estimate; then held
/// near neutral, see [`AIRLIGHT_NEUTRALITY`].
fn airlight(rgb: &[f32], luminance: &[f32], w: usize, h: usize) -> [f32; 3] {
    let mut dark: Vec<f32> = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .map(|px| px[0].min(px[1]).min(px[2]))
        .collect();
    min_filter(&mut dark, w, h, AIRLIGHT_WINDOW);
    let hazy_from = quantile(&dark, HAZY_QUANTILE);
    let hazy: Vec<usize> = (0..w * h).filter(|&i| dark[i] >= hazy_from).collect();
    let bright_from = quantile(
        &hazy.iter().map(|&i| luminance[i]).collect::<Vec<_>>(),
        BRIGHT_QUANTILE,
    );
    let mut sum = [0f64; 3];
    let mut count = 0usize;
    for &i in &hazy {
        if luminance[i] >= bright_from {
            for c in 0..3 {
                sum[c] += rgb[i * 3 + c] as f64;
            }
            count += 1;
        }
    }
    let count = count.max(1) as f64;
    let a = [
        (sum[0] / count) as f32,
        (sum[1] / count) as f32,
        (sum[2] / count) as f32,
    ]
    .map(|v| {
        if v.is_finite() {
            v.max(MIN_AIRLIGHT)
        } else {
            MIN_AIRLIGHT
        }
    });
    let floor = a[0].max(a[1]).max(a[2]) * AIRLIGHT_NEUTRALITY;
    a.map(|v| v.max(floor))
}

/// The value at quantile `q` of `values`, by selection.
fn quantile(values: &[f32], q: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v: Vec<f32> = values.to_vec();
    let k = ((v.len() - 1) as f32 * q).round() as usize;
    let (_, at, _) = v.select_nth_unstable_by(k, |a, b| a.total_cmp(b));
    *at
}

/// The minimum over a square window of half-width `r`, clipped at the
/// edges, in place: a row pass then a column pass, which for a
/// minimum is exact.
fn min_filter(plane: &mut [f32], w: usize, h: usize, r: usize) {
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w)
        .zip(plane.par_chunks(w))
        .for_each(|(out, row)| {
            for (x, o) in out.iter_mut().enumerate() {
                let lo = x.saturating_sub(r);
                let hi = (x + r + 1).min(w);
                *o = row[lo..hi].iter().copied().fold(f32::INFINITY, f32::min);
            }
        });
    plane.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        let lo = y.saturating_sub(r);
        let hi = (y + r + 1).min(h);
        out.copy_from_slice(&tmp[lo * w..lo * w + w]);
        for row in tmp[lo * w..hi * w].chunks_exact(w).skip(1) {
            for (o, &v) in out.iter_mut().zip(row) {
                *o = o.min(v);
            }
        }
    });
}

/// The mean over a square window of half-width `r`, clipped at the
/// edges and normalized by what is inside them: running sums along
/// rows, then down columns.
fn box_mean(plane: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    // Rows.
    let mut rows = vec![0f32; w * h];
    rows.par_chunks_mut(w)
        .zip(plane.par_chunks(w))
        .for_each(|(out, row)| {
            let mut prefix = vec![0f64; w + 1];
            for (i, &v) in row.iter().enumerate() {
                prefix[i + 1] = prefix[i] + v as f64;
            }
            for (x, o) in out.iter_mut().enumerate() {
                let lo = x.saturating_sub(r);
                let hi = (x + r + 1).min(w);
                *o = ((prefix[hi] - prefix[lo]) / (hi - lo) as f64) as f32;
            }
        });
    // Columns: a running sum down the rows, then each output row a
    // difference of two of them.
    let mut prefix = vec![0f64; w * (h + 1)];
    for y in 0..h {
        let (above, here) = prefix.split_at_mut((y + 1) * w);
        let above = &above[y * w..];
        for x in 0..w {
            here[x] = above[x] + rows[y * w + x] as f64;
        }
    }
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        let lo = y.saturating_sub(r);
        let hi = (y + r + 1).min(h);
        let count = (hi - lo) as f64;
        for (x, o) in out.iter_mut().enumerate() {
            *o = ((prefix[hi * w + x] - prefix[lo * w + x]) / count) as f32;
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small deterministic generator, so the scenes are the same
    /// run to run.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as f32) / (1u64 << 31) as f32
        }
    }

    /// A clear scene: blocks of colored texture with hard edges,
    /// each block with one channel near zero as an outdoor scene's
    /// patches have, brightness varying across the frame so there is
    /// contrast at every scale.
    fn clear_scene(w: usize, h: usize) -> WorkingImage {
        let mut rng = Lcg(7);
        let block = 8;
        let bw = w.div_ceil(block);
        let bh = h.div_ceil(block);
        let colors: Vec<[f32; 3]> = (0..bw * bh)
            .map(|_| {
                let mut c = [
                    0.2 + 0.7 * rng.next(),
                    0.2 + 0.7 * rng.next(),
                    0.02 * rng.next(),
                ];
                // The near-zero channel moves around.
                let k = (rng.next() * 3.0) as usize % 3;
                c.swap(2, k);
                c
            })
            .collect();
        let mut image = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let c = colors[(y / block) * bw + x / block];
                let shade = 0.5 + 0.5 * ((x + y) as f32 / (w + h) as f32);
                let noise = 0.9 + 0.2 * rng.next();
                let px = &mut image.data[(y * w + x) * 3..][..3];
                for i in 0..3 {
                    px[i] = c[i] * shade * noise;
                }
            }
        }
        image
    }

    /// The scene under haze: a band of sky along the top at zero
    /// transmission, pure airlight, as a far horizon is, and under it
    /// the radiance mixed with the airlight by a transmission that
    /// falls across the frame.
    fn hazed(scene: &WorkingImage, airlight: [f32; 3]) -> (WorkingImage, Vec<f32>) {
        let (w, h) = (scene.width, scene.height);
        let mut out = scene.clone();
        let mut t = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let tt = if y < sky(h) {
                    0.0
                } else {
                    0.2 + 0.5 * (x as f32 / w as f32)
                };
                t[y * w + x] = tt;
                let px = &mut out.data[(y * w + x) * 3..][..3];
                for c in 0..3 {
                    px[c] = px[c] * tt + airlight[c] * (1.0 - tt);
                }
            }
        }
        (out, t)
    }

    /// A scene with no channel near zero anywhere: blocks of grey
    /// with at most a tenth of color on them, as skin, walls and
    /// cloth have. What the prior has no way to tell from haze.
    fn neutral_scene(w: usize, h: usize) -> WorkingImage {
        let mut rng = Lcg(11);
        let block = 8;
        let bw = w.div_ceil(block);
        let bh = h.div_ceil(block);
        let colors: Vec<[f32; 3]> = (0..bw * bh)
            .map(|_| {
                let grey = 0.3 + 0.5 * rng.next();
                [
                    grey * (0.9 + 0.2 * rng.next()),
                    grey * (0.9 + 0.2 * rng.next()),
                    grey * (0.9 + 0.2 * rng.next()),
                ]
            })
            .collect();
        let mut image = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let c = colors[(y / block) * bw + x / block];
                let shade = 0.6 + 0.4 * ((x + y) as f32 / (w + h) as f32);
                let noise = 0.95 + 0.1 * rng.next();
                let px = &mut image.data[(y * w + x) * 3..][..3];
                for i in 0..3 {
                    px[i] = c[i] * shade * noise;
                }
            }
        }
        image
    }

    /// Rows of sky at the top of a hazed scene.
    fn sky(h: usize) -> usize {
        h / 8
    }

    /// The rows of a hazed scene that hold the scene, clear of the
    /// sky and of the edges where the windows are clipped.
    fn body(w: usize, h: usize) -> impl Iterator<Item = usize> {
        let margin = (AIRLIGHT_WINDOW + GUIDE_WINDOW) * 4;
        (sky(h) + margin..h - margin)
            .flat_map(move |y| (margin..w - margin).map(move |x| y * w + x))
    }

    fn pick<T: Copy>(v: &[T], at: impl Iterator<Item = usize>) -> Vec<T> {
        at.map(|i| v[i]).collect()
    }

    fn luminance(image: &WorkingImage) -> Vec<f32> {
        image
            .data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|px| LUMA[0] * px[0] + LUMA[1] * px[1] + LUMA[2] * px[2])
            .collect()
    }

    fn std_dev(v: &[f32]) -> f32 {
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        (v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn zero_is_the_identity() {
        let scene = clear_scene(96, 64);
        let mut image = scene.clone();
        let stats = dehaze(&mut image, &DehazeOptions { amount: 0.0 });
        assert_eq!(image.data, scene.data);
        assert_eq!(stats.transmission_mean, 1.0);
        let (_, t) = dehaze_with_map(&mut scene.clone(), &DehazeOptions { amount: 0.0 });
        assert!(t.iter().all(|&v| v == 1.0));
    }

    #[test]
    fn a_clear_scene_barely_changes() {
        let scene = clear_scene(192, 128);
        let mut image = scene.clone();
        let stats = dehaze(&mut image, &DehazeOptions { amount: 0.5 });
        // Every window has a channel near zero, so the prior reads
        // almost no haze and the transmission stays near one.
        assert!(
            stats.transmission_mean > 0.95,
            "transmission {} under airlight {:?}",
            stats.transmission_mean,
            stats.airlight
        );
        let before = luminance(&scene);
        let after = luminance(&image);
        let worst = before
            .iter()
            .zip(&after)
            .map(|(b, a)| (a - b).abs() / b.max(1e-3))
            .fold(0.0, f32::max);
        assert!(worst < 0.1, "a clear pixel moved by {worst:.3}");
        let mean_change = before
            .iter()
            .zip(&after)
            .map(|(b, a)| (a - b).abs() / b.max(1e-3))
            .sum::<f32>()
            / before.len() as f32;
        assert!(
            mean_change < 0.03,
            "clear pixels moved by {mean_change:.3} on average"
        );
    }

    #[test]
    fn a_hazy_scene_gets_its_contrast_back() {
        let scene = clear_scene(256, 160);
        // Clearly colored, so a channel swap in the estimate would
        // show; above the neutrality floor.
        let airlight = [0.95, 0.85, 0.70];
        let (hazy, known) = hazed(&scene, airlight);
        let (w, h) = (hazy.width, hazy.height);
        let (stats, found) = dehaze_with_map(&mut hazy.clone(), &DehazeOptions { amount: 1.0 });
        let a = stats.airlight;
        let s = FULL_STRENGTH;
        for c in 0..3 {
            assert!(
                (a[c] - airlight[c]).abs() < 0.03,
                "airlight {a:?} for {airlight:?}"
            );
        }
        // The prior reads 1 - s (1 - t (1 - m)) with m the scene's
        // own dark channel, which is small here, and s the strength
        // the amount maps to; away from the sky and the edges, where
        // the window is clipped, it is within a few percent.
        let err = pick(&found, body(w, h))
            .iter()
            .zip(pick(&known, body(w, h)))
            .map(|(f, k)| (f - (1.0 - s + s * k)).abs())
            .sum::<f32>()
            / body(w, h).count() as f32;
        assert!(err < 0.05, "transmission off by {err:.3} on average");

        let mut image = hazy.clone();
        let stats = dehaze(&mut image, &DehazeOptions { amount: 1.0 });
        assert!(stats.transmission_min >= MIN_TRANSMISSION);
        let clear = std_dev(&pick(&luminance(&scene), body(w, h)));
        let veiled = std_dev(&pick(&luminance(&hazy), body(w, h)));
        let restored = std_dev(&pick(&luminance(&image), body(w, h)));
        assert!(
            veiled < 0.8 * clear,
            "the haze took no contrast: {veiled} of {clear}"
        );
        assert!(
            restored > 0.7 * clear && restored > 1.2 * veiled,
            "contrast {restored} restored from {veiled}, the scene's was {clear}"
        );
        // And the recovered picture is what the model says it is
        // for that transmission, `A + t (J - A) / (1 - s + s t)`:
        // the scene itself as s goes to one, and a known share of
        // the way there at the strength the slider's end maps to.
        let mean_err = body(w, h)
            .map(|i| {
                let t = known[i];
                (0..3)
                    .map(|c| {
                        let want = airlight[c]
                            + t * (scene.data[i * 3 + c] - airlight[c]) / (1.0 - s + s * t);
                        (image.data[i * 3 + c] - want).abs()
                    })
                    .sum::<f32>()
                    / 3.0
            })
            .sum::<f32>()
            / body(w, h).count() as f32;
        assert!(
            mean_err < 0.03,
            "recovered scene off the model by {mean_err:.3} on average"
        );
    }

    /// At the slider's end no channel goes to zero: the strength
    /// stops at [`FULL_STRENGTH`], and the bound then leaves every
    /// pixel's darkest channel at least `1 - s` of what it had. At a
    /// strength of one it would be exactly zero wherever the bound
    /// bites, which on a real frame is the whole sky.
    #[test]
    fn the_full_amount_keeps_a_share_of_every_channel() {
        let scene = neutral_scene(256, 160);
        let airlight = [0.95, 0.85, 0.70];
        let (hazy, _) = hazed(&scene, airlight);
        let mut image = hazy.clone();
        let stats = dehaze(&mut image, &DehazeOptions { amount: 1.0 });
        assert_eq!(stats.strength, FULL_STRENGTH);
        let least_share = image
            .data
            .iter()
            .zip(&hazy.data)
            .map(|(j, i)| j / i.max(1e-6))
            .fold(f32::INFINITY, f32::min);
        assert!(
            least_share >= 1.0 - FULL_STRENGTH - 1e-3,
            "a channel fell to {least_share:.3} of its input"
        );
        assert!(
            image.data.iter().all(|&v| v >= 0.0),
            "a channel went negative"
        );
        // The sky band, pure airlight, keeps at least half of it.
        let (w, h) = (hazy.width, hazy.height);
        for i in 0..sky(h) * w {
            for (c, &a) in airlight.iter().enumerate() {
                assert!(
                    image.data[i * 3 + c] > 0.5 * a,
                    "sky channel {c} at {}",
                    image.data[i * 3 + c]
                );
            }
        }
    }

    /// What the prior does to a picture with no haze and no dark
    /// channel to say so: a portrait, an interior. The airlight is
    /// read off the brightest neutral patch and everything near it
    /// in the dark channel is taken as veiled, so +50 is not a
    /// no-op; this records how far it is from one.
    #[test]
    fn a_neutral_scene_is_read_as_hazy() {
        let scene = neutral_scene(192, 128);
        let mut image = scene.clone();
        let stats = dehaze(&mut image, &DehazeOptions { amount: 0.5 });
        let before = luminance(&scene);
        let after = luminance(&image);
        let mean_change = before
            .iter()
            .zip(&after)
            .map(|(b, a)| (a - b) / b)
            .sum::<f32>()
            / before.len() as f32;
        let worst = before
            .iter()
            .zip(&after)
            .map(|(b, a)| ((a - b) / b).abs())
            .fold(0.0, f32::max);
        eprintln!(
            "neutral scene at +50: airlight {:?}, mean transmission {:.3}, mean luminance change {:+.3}, worst {:.3}",
            stats.airlight, stats.transmission_mean, mean_change, worst
        );
        assert!(stats.transmission_mean < 0.95, "the prior read no haze");
        assert!(
            mean_change < -0.02,
            "a neutral scene should darken as its veil is taken"
        );
        assert!(
            mean_change > -0.30 && mean_change < -0.10 && worst < 0.5,
            "the record moved: {mean_change:+.3}, worst {worst:.3}"
        );
    }

    #[test]
    fn a_negative_amount_puts_haze_in() {
        // Haze goes in by the dark channel, so where there is haze
        // already: a clear scene has next to none to deepen.
        let scene = clear_scene(128, 96);
        let (hazy, _) = hazed(&scene, [0.9, 0.9, 0.9]);
        let (w, h) = (hazy.width, hazy.height);
        let mut image = hazy.clone();
        let stats = dehaze(&mut image, &DehazeOptions { amount: -0.5 });
        assert!(stats.transmission_mean > 1.0);
        let before = std_dev(&pick(&luminance(&hazy), body(w, h)));
        let after = std_dev(&pick(&luminance(&image), body(w, h)));
        assert!(
            after < 0.9 * before,
            "no haze went in: {after} from {before}"
        );
        assert!(image.data.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn black_and_clipped_white_stay_finite_and_where_they_are() {
        for amount in [1.0, -1.0, 0.3] {
            let mut black = WorkingImage::new(64, 48);
            let stats = dehaze(&mut black, &DehazeOptions { amount });
            assert!(black.data.iter().all(|&v| v.is_finite() && v.abs() < 1e-3));
            assert!(stats.airlight.iter().all(|v| v.is_finite()));

            let mut white = WorkingImage::new(64, 48);
            white.data.fill(8.0);
            dehaze(&mut white, &DehazeOptions { amount });
            assert!(
                white
                    .data
                    .iter()
                    .all(|&v| v.is_finite() && (v - 8.0).abs() < 1e-3)
            );

            // Half black, half clipped white: the boundary is the
            // filter's to blend, and nothing may go non-finite.
            let mut split = WorkingImage::new(64, 48);
            for y in 0..48 {
                for x in 32..64 {
                    split.data[(y * 64 + x) * 3..][..3].fill(8.0);
                }
            }
            dehaze(&mut split, &DehazeOptions { amount });
            assert!(split.data.iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn the_reduced_map_agrees_with_the_full_one() {
        // A picture past the reduced edge is worked at a factor; the
        // transmission it gets must be near what the same scene gets
        // at full size, scaled.
        let scene = clear_scene(200, 120);
        let (hazy, known) = hazed(&scene, [0.9, 0.9, 0.9]);
        const UP: usize = 8;
        let mut big = {
            let (w, h) = (hazy.width * UP, hazy.height * UP);
            let mut big = WorkingImage::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    let src = ((y / UP) * hazy.width + x / UP) * 3;
                    big.data[(y * w + x) * 3..][..3].copy_from_slice(&hazy.data[src..src + 3]);
                }
            }
            big
        };
        assert!(reduce_factor(big.width, big.height) > 1);
        let options = DehazeOptions { amount: 1.0 };
        let (_, small) = dehaze_with_map(&mut hazy.clone(), &options);
        let (_, large) = dehaze_with_map(&mut big, &options);
        let sampled: Vec<f32> = (0..hazy.height)
            .flat_map(|y| (0..hazy.width).map(move |x| (x, y)))
            .map(|(x, y)| large[(y * UP + UP / 2) * big.width + x * UP + UP / 2])
            .collect();
        let between = pick(&small, body(hazy.width, hazy.height))
            .iter()
            .zip(pick(&sampled, body(hazy.width, hazy.height)))
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / body(hazy.width, hazy.height).count() as f32;
        assert!(
            between < 0.03,
            "reduced map off the full one by {between:.3}"
        );
        let err = pick(&sampled, body(hazy.width, hazy.height))
            .iter()
            .zip(pick(&known, body(hazy.width, hazy.height)))
            .map(|(a, b)| (a - (1.0 - FULL_STRENGTH + FULL_STRENGTH * b)).abs())
            .sum::<f32>()
            / body(hazy.width, hazy.height).count() as f32;
        assert!(err < 0.06, "reduced transmission off by {err:.3}");
    }

    #[test]
    fn the_box_mean_and_min_filter_handle_their_edges() {
        let (w, h) = (7, 5);
        let plane: Vec<f32> = (0..w * h).map(|i| i as f32).collect();
        let mean = box_mean(&plane, w, h, 1);
        // The corner's window holds four values: 0, 1, 7, 8.
        assert!((mean[0] - 4.0).abs() < 1e-5);
        // An interior pixel's holds nine, centered on itself.
        assert!((mean[w + 1] - 8.0).abs() < 1e-5);
        let mut plane = plane;
        min_filter(&mut plane, w, h, 1);
        assert_eq!(plane[0], 0.0);
        assert_eq!(plane[2 * w + 3], 9.0);
        assert_eq!(plane[w * h - 1], (h - 2) as f32 * w as f32 + (w - 2) as f32);
    }
}
