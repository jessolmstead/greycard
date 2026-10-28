//! The fill's middle against the surface round it: a fill matched at
//! its edge ([`super::seam`]) can still drift a few percent away from
//! the picture deeper in. Over a plain sky the models' fills grow light
//! or dark, and off in hue, with distance from the edge (blue 3 percent
//! short 30 px into one, red 3 to 5 percent over in another), which the
//! edge, read a few pixels in, does not see; and a stroke a few pixels
//! wide is never read at the edge at all.
//!
//! Here the picture round the hole is read as a surface: a band of it
//! set back from the edge is low-passed, its locally smooth pixels are
//! fitted per channel with a quadratic in logs, robustly, so another
//! surface the hole crosses (a hill under the sky) falls out as an
//! outlier; what the band shows past the quadratic is carried a short
//! way in. Where that surface fits the band closely, the fill's own
//! low-pass is compared with it where the fill is smooth and near it,
//! and the difference, smoothed far wider than any detail and held to
//! [`LIMIT`], is divided out of the whole hole round those pixels. A
//! textured surround (a wall, foliage, clouds) does not fit a quadratic
//! and the fill is left as it is; so is any part of the fill that
//! differs from the surface by more than [`SAME`] (the hill the fill
//! drew, an object it kept), and the fill's grain and texture, which
//! sit far below the smoothing's scale.

use super::scale::{to_linear, to_srgb};
use super::seam::{Crop, PEDESTAL, solve};
use crate::image::Rgbf;
use rayon::prelude::*;

/// The band round the hole the surface is fitted on, in pixels out
/// from the hole's edge: set back past a removed object's rim, and as
/// far out as the surface is read.
pub const BAND: (usize, usize) = (4, 32);

/// The low-pass radius both sides are read through first: grain is
/// averaged out of the surface fit and out of the fill's texture read.
const PRE: usize = 2;

/// The radius the local texture is read over, and the most a pixel's
/// low-passed log luma may spread over it for the pixel to count as
/// smooth, on the picture's side and the fill's: a sky reads under 1
/// percent, a stone wall or a hill's scrub several.
const LOCAL: usize = 6;
const SMOOTH: f64 = 0.02;

/// How closely the surface must fit the band's smooth pixels, the
/// residuals' sigma in logs in the worst channel: wholly trusted at the
/// first, not at all at the second. A sky fits to 1.4 to 1.9 percent
/// after the low-pass, a wall 3 to 5.
const FIT: (f64, f64) = (0.02, 0.03);

/// The share of the band the fit must hold as inliers: fully trusted
/// at the second, not at all at the first. Over a mostly textured
/// surround the few smooth pixels are not the hole's surface.
const SHARE: (f64, f64) = (0.3, 0.5);

/// The least inlier pixels, and the least octants round the hole's
/// middle (of eight) that must each hold a share of them: a quadratic
/// read on one side of the hole alone is an extrapolation.
const LEAST: usize = 200;
const OCTANTS: usize = 4;

/// How far, in logs in any channel, the fill's low-pass may be from
/// the surface for it to be the same surface: past it the fill drew
/// something else.
const SAME: f64 = 0.08;

/// The share of the band's smooth pixels that may sit off the fit but
/// within [`SAME`] of it: fully trusted at the first, not at all at
/// the second. Past a few percent the band holds a second soft surface
/// (a shadow, a cloud) that a wide hole's fill would continue where the
/// band cannot show it, and the fill's departure from the quadratic is
/// no longer its own.
const SOFT: (f64, f64) = (0.04, 0.07);

/// How far the band's own departure from the quadratic is carried
/// into the hole: two box passes of this radius, fully where a quarter
/// of them is band.
const CARRY: usize = 8;

/// The correction's smoothing, two box passes of this radius over the
/// fill's own pixels: the drift spans tens of pixels, the fill's
/// detail a few.
const RADIUS: usize = 10;

/// The least share of the smoothing's boxes that must be the fill's
/// read pixels for the field to be taken at a pixel: a one-pixel line
/// along the boxes holds a fortieth of them.
const FEW: f64 = 0.02;

/// How many of its own noise's sigmas the field is shrunk by.
const SHRINK: f64 = 3.0;

/// What the smoothing does to an independent sample's sigma: the
/// fill's low-pass ([`PRE`]) is one sample a box, and the two box
/// passes of [`RADIUS`] average about `1 / kernel()²` such boxes.
fn kernel() -> f64 {
    let side = 2 * RADIUS + 1;
    let tent: Vec<f64> = (0..2 * side - 1)
        .map(|j| (side - j.abs_diff(side - 1)) as f64)
        .collect();
    let sum = tent.iter().sum::<f64>();
    let squares = tent.iter().map(|t| t * t).sum::<f64>();
    squares / (sum * sum) * (2 * PRE + 1) as f64
}

/// The most the fill is moved, in logs per channel.
const LIMIT: f64 = 0.05;

/// The robust fit's passes, the scale of its biweight in sigmas, and
/// the least sigma it is taken at (a sky's grain alone after the
/// low-pass).
const PASSES: usize = 4;
const TUKEY: f64 = 4.685;
const FLOOR: f64 = 0.004;

/// The most the fit's sigma may be, in the band's own grain read
/// [`LAG`] pixels apart. A broad shadow across the band inflates the
/// residuals' median to 2.3 to 4.3 times the grain and would be taken
/// in as inliers; held to the grain, it is soft ([`SOFT`]). A sky's
/// residuals run 1.2 to 1.8 times it, which leaves under 3 percent of
/// its band soft.
const GRAINS: f64 = 1.0;
const LAG: usize = 6;

/// What the middle's correction did, for the log.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Middle {
    /// The mean gain put into the hole, per channel.
    pub mean: [f32; 3],
    /// The largest change of any pixel, in logs.
    pub largest: f32,
    /// How much of the correction was trusted, 0 to 1.
    pub strength: f32,
}

/// `fill` (sRGB-encoded, in 0 to 1, like `image`) with its pixels
/// under `hole` (nonzero is in it) brought to the surface the picture
/// round the hole describes, where that surface is smooth and the fill
/// is on it; `fill` as it was and `None` where the surround is
/// textured, too little of it is smooth, or there is no hole.
pub fn evened(image: &Rgbf, hole: &[f32], fill: &Rgbf) -> (Rgbf, Option<Middle>) {
    let (w, h) = (image.width, image.height);
    if fill.width != w || fill.height != h || hole.len() != w * h {
        return (fill.clone(), None);
    }
    let Some(crop) = Crop::round(hole, w, h, BAND.1 + PRE + LOCAL) else {
        return (fill.clone(), None);
    };
    let (cw, ch) = (crop.width, crop.height);
    let n = cw * ch;
    let mask = crop.mask(hole);
    let inside: Vec<f64> = mask.iter().map(|&m| f64::from(u8::from(m > 0.0))).collect();
    let outside: Vec<f64> = inside.iter().map(|m| 1.0 - m).collect();
    // Each side in linear light where it is that side's, 0 elsewhere.
    let linear = |img: &Rgbf, weight: &[f64]| -> [Vec<f64>; 3] {
        let pixels: Vec<[f64; 3]> = (0..n)
            .into_par_iter()
            .map(|i| {
                let j = crop.at(i) * 3;
                if weight[i] > 0.0 {
                    std::array::from_fn(|k| f64::from(to_linear(img.data[j + k]) + PEDESTAL))
                } else {
                    [0.0; 3]
                }
            })
            .collect();
        std::array::from_fn(|k| pixels.iter().map(|p| p[k]).collect())
    };

    // Each side low-passed over its own pixels, and its texture; the
    // two at once.
    let ((picture, texture, band), (filled, low, fill_texture)) = rayon::join(
        || {
            let counts = Counts::of(&outside, cw, ch);
            let picture = counts.means(&linear(image, &outside), PRE);
            let texture = texture_of(&picture, &counts);
            let near = super::ring(&mask, cw, ch, BAND.0 - 1);
            let band: Vec<bool> = super::ring(&mask, cw, ch, BAND.1)
                .into_iter()
                .zip(&near)
                .map(|(b, n)| b && !n)
                .collect();
            (picture, texture, band)
        },
        || {
            let filled = linear(fill, &inside);
            let counts = Counts::of(&inside, cw, ch);
            let low = counts.means(&filled, PRE);
            let texture = texture_of(&low, &counts);
            (filled, low, texture)
        },
    );
    let in_band = band.iter().filter(|&&b| b).count();

    // The hole's middle and size, for the surface's coordinates.
    let (mut mx, mut my, mut count) = (0.0f64, 0.0f64, 0.0f64);
    let (mut x0, mut x1, mut y0, mut y1) = (cw, 0, ch, 0);
    for i in (0..n).filter(|&i| inside[i] > 0.0) {
        let (x, y) = (i % cw, i / cw);
        mx += x as f64;
        my += y as f64;
        count += 1.0;
        (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
    }
    if count == 0.0 {
        return (fill.clone(), None);
    }
    let (mx, my) = (mx / count, my / count);
    let size = ((x1 - x0).max(y1 - y0) as f64 / 2.0 + BAND.1 as f64).max(1.0);
    let terms = |i: usize| {
        let (u, v) = (((i % cw) as f64 - mx) / size, ((i / cw) as f64 - my) / size);
        [1.0, u, v, u * u, u * v, v * v]
    };

    // The surface: the band's smooth pixels, fitted robustly. The
    // first weights keep what is near the median in every channel, so
    // a hill along one side of the hole does not bend the first fit.
    // Every reading is per channel and takes the worst of the three: a
    // warm surface beside a blue sky can share its luma.
    let points: Vec<usize> = (0..n).filter(|&i| band[i] && texture[i] < SMOOTH).collect();
    if points.len() < LEAST {
        log::info!(
            "remove: the fill's middle left as it is: {} smooth pixels round the hole",
            points.len()
        );
        return (fill.clone(), None);
    }
    let logs: Vec<[f64; 3]> = points
        .iter()
        .map(|&i| std::array::from_fn(|k| picture[k][i].max(1e-6).ln()))
        .collect();
    let center: [f64; 3] = std::array::from_fn(|k| median(logs.iter().map(|l| l[k]).collect()));
    let mut weights: Vec<f64> = logs
        .iter()
        .map(|l| f64::from(u8::from((0..3).all(|k| (l[k] - center[k]).abs() < SAME))))
        .collect();
    let mut at = vec![usize::MAX; n];
    for (j, &i) in points.iter().enumerate() {
        at[i] = j;
    }
    let mut grain = [f64::INFINITY; 3];
    let mut surface = [[0.0f64; 6]; 3];
    let mut residuals = vec![[0.0f64; 3]; points.len()];
    for _ in 0..PASSES {
        let mut m = vec![0.0f64; 36];
        let mut r = [vec![0.0f64; 6], vec![0.0f64; 6], vec![0.0f64; 6]];
        for ((&i, l), &wt) in points.iter().zip(&logs).zip(&weights) {
            if wt <= 0.0 {
                continue;
            }
            let t = terms(i);
            for a in 0..6 {
                for b in 0..6 {
                    m[a * 6 + b] += wt * t[a] * t[b];
                }
                for k in 0..3 {
                    r[k][a] += wt * t[a] * l[k];
                }
            }
        }
        for k in 0..3 {
            let Some(c) = solve(m.clone(), r[k].clone(), 6) else {
                return (fill.clone(), None);
            };
            surface[k].copy_from_slice(&c);
        }
        for (res, (&i, l)) in residuals.iter_mut().zip(points.iter().zip(&logs)) {
            let t = terms(i);
            *res = std::array::from_fn(|k| l[k] - dot(&surface[k], &t));
        }
        // The band's grain at the fit's scale, per channel, from the
        // first fit's residuals at pixels six apart (past the
        // low-pass's reach, and too near for a soft surface to change
        // much between them): a broad second surface inflates the
        // residuals' median, which would take it in, but not this.
        if grain[0].is_infinite() {
            grain = std::array::from_fn(|k| {
                let mut d = Vec::with_capacity(2 * points.len());
                for (j, &i) in points.iter().enumerate() {
                    for o in [i + LAG, i + LAG * cw] {
                        if o < n && at[o] != usize::MAX {
                            d.push((residuals[j][k] - residuals[at[o]][k]).abs());
                        }
                    }
                }
                1.4826 * median(d) / 2f64.sqrt()
            });
        }
        let cutoff: [f64; 3] = std::array::from_fn(|k| {
            let spread = 1.4826 * median(residuals.iter().map(|r| r[k].abs()).collect());
            TUKEY * spread.min(GRAINS * grain[k]).max(FLOOR)
        });
        for (wt, r) in weights.iter_mut().zip(&residuals) {
            let u = (0..3).map(|k| r[k].abs() / cutoff[k]).fold(0.0, f64::max);
            *wt = if u < 1.0 { (1.0 - u * u).powi(2) } else { 0.0 };
        }
    }
    let total = weights.iter().sum::<f64>();
    let spread = (0..3)
        .map(|k| {
            (weights
                .iter()
                .zip(&residuals)
                .map(|(w, r)| w * r[k] * r[k])
                .sum::<f64>()
                / total.max(1e-12))
            .sqrt()
        })
        .fold(0.0, f64::max);
    let inliers = weights.iter().filter(|&&w| w > 0.0).count();
    let share = inliers as f64 / in_band.max(1) as f64;
    // The smooth band the fit threw out that is still within SAME of
    // it: a second soft surface (a shadow, a wisp of cloud) whose
    // continuation in the fill would read as the fill's own drift.
    let near_surface = |r: &[f64; 3]| r.iter().all(|r| r.abs() < SAME);
    let soft = weights
        .iter()
        .zip(&residuals)
        .filter(|(w, r)| **w <= 0.0 && near_surface(r))
        .count() as f64
        / points.len() as f64;
    let mut octants = [0usize; 8];
    for (&i, _) in points.iter().zip(&weights).filter(|(_, w)| **w > 0.0) {
        let (u, v) = ((i % cw) as f64 - mx, (i / cw) as f64 - my);
        let a = (v.atan2(u) + std::f64::consts::PI) / (2.0 * std::f64::consts::PI) * 8.0;
        octants[(a as usize).min(7)] += 1;
    }
    let held = octants
        .iter()
        // An octant counts with a fiftieth of the inliers.
        .filter(|&&c| c as f64 >= 0.02 * inliers as f64)
        .count();
    let strength = ramp(FIT.1, FIT.0, spread) * ramp(SHARE.0, SHARE.1, share);
    let unshown = ramp(SOFT.1, SOFT.0, soft);
    if strength <= 0.0 || inliers < LEAST || held < OCTANTS {
        log::info!(
            "remove: the fill's middle left as it is: the surround fits a smooth surface to {spread:.4}, {:.0}% of it, on {held} sides, {:.1}% of it a second soft surface",
            100.0 * share,
            100.0 * soft
        );
        return (fill.clone(), None);
    }

    // What the band shows past the quadratic, carried a short way into
    // the hole: a soft feature the hole cuts (a wisp across a mast)
    // is the fill's to continue, not its drift. Deep in a wide hole
    // nothing is carried and the quadratic stands.
    let mut carry: [Vec<f64>; 4] = std::array::from_fn(|_| vec![0.0f64; n]);
    for (&i, r) in points
        .iter()
        .zip(&residuals)
        .filter(|(_, r)| near_surface(r))
    {
        for k in 0..3 {
            carry[k][i] = r[k];
        }
        carry[3][i] = 1.0;
    }
    let reach = |a: &[f64]| box_sum(&box_sum(a, cw, ch, CARRY), cw, ch, CARRY);
    let [c0, c1, c2, carried_n]: [Vec<f64>; 4] = carry
        .par_iter()
        .map(|a| reach(a))
        .collect::<Vec<_>>()
        .try_into()
        .expect("four planes");
    let carried = [c0, c1, c2];
    let enough = 0.25 * ((2 * CARRY + 1) as f64).powi(4);
    let shown = |k: usize, i: usize| {
        if carried_n[i] > 0.5 {
            carried[k][i] / carried_n[i] * (carried_n[i] / enough).min(1.0)
        } else {
            0.0
        }
    };
    // Past the carry's reach the band cannot show what the fill should
    // be; there the fill's departure counts only as far as the band
    // holds no second soft surface.
    let trust = |i: usize| {
        let reached = (carried_n[i] / enough).min(1.0);
        reached + (1.0 - reached) * unshown
    };

    // The fill's side: its low-pass over its own pixels against that
    // reference, read where it is smooth and near it in every channel.
    let read: Vec<Option<[f64; 3]>> = (0..n)
        .into_par_iter()
        .map(|i| {
            if inside[i] <= 0.0 || fill_texture[i] >= SMOOTH {
                return None;
            }
            let t = terms(i);
            let q: [f64; 3] = std::array::from_fn(|k| {
                low[k][i].max(1e-6).ln() - dot(&surface[k], &t) - shown(k, i)
            });
            near_surface(&q).then(|| q.map(|q| q * trust(i)))
        })
        .collect();
    let kept = read.iter().flatten().count();
    if kept < LEAST {
        log::info!(
            "remove: the fill's middle left as it is: {kept} of its pixels on the surface round it"
        );
        return (fill.clone(), None);
    }
    let on: Vec<f64> = read
        .iter()
        .map(|q| f64::from(u8::from(q.is_some())))
        .collect();
    let off: [Vec<f64>; 3] =
        std::array::from_fn(|k| read.iter().map(|q| q.map_or(0.0, |q| q[k])).collect());
    let smooth = |a: &[f64]| box_sum(&box_sum(a, cw, ch, RADIUS), cw, ch, RADIUS);
    let [on_s, hole_s, off_0, off_1, off_2]: [Vec<f64>; 5] =
        [&on, &inside, &off[0], &off[1], &off[2]]
            .into_par_iter()
            .map(|a| smooth(a))
            .collect::<Vec<_>>()
            .try_into()
            .expect("five planes");
    let off_s = [off_0, off_1, off_2];

    // The field's own noise, from how the fill's low-pass scatters
    // round it, through the smoothing's kernel: a fill at the surface's
    // level under grain reads a field of that size by chance, and each
    // channel's field is shrunk by it (a positive-part Wiener shrink),
    // so such a fill moves by a hair and a true drift is taken nearly
    // whole.
    let field = |k: usize, i: usize| off_s[k][i] / on_s[i];
    let mut scatter = [0.0f64; 3];
    let mut samples = 0.0f64;
    for i in (0..n).filter(|&i| on[i] > 0.0 && on_s[i] > 0.5) {
        samples += 1.0;
        for (k, s) in scatter.iter_mut().enumerate() {
            *s += (off[k][i] - field(k, i)).powi(2);
        }
    }
    let full = ((2 * RADIUS + 1) as f64).powi(4);
    let noise = scatter.map(|s| (s / samples.max(1.0)).sqrt() * kernel());

    // No fade toward the edge: the surface is the picture's own across
    // it, and a stroke a few pixels wide is nothing but edge.
    let changes: Vec<Option<[f64; 3]>> = (0..n)
        .into_par_iter()
        .map(|i| {
            // A field read from a few pixels has no scatter to shrink
            // it by: a hole that small is left as it is.
            if inside[i] <= 0.0 || on_s[i] < FEW * full {
                return None;
            }
            let cover = (2.0 * on_s[i] / hole_s[i].max(1.0)).min(1.0);
            // Near the hole's edge, or along a stroke, the boxes hold
            // fewer of the fill's pixels and the field is the noisier.
            let thin = (full / on_s[i]).sqrt();
            let c: [f64; 3] = std::array::from_fn(|k| {
                let f = field(k, i);
                let kept = 1.0 - (SHRINK * noise[k] * thin / f.abs().max(1e-12)).powi(2);
                (f * kept.max(0.0) * cover).clamp(-LIMIT, LIMIT) * strength
            });
            // A pixel shrunk to no change keeps its value to the bit.
            c.iter().any(|&c| c != 0.0).then_some(c)
        })
        .collect();
    let (mut sum, mut largest, mut moved) = ([0.0f64; 3], 0.0f64, 0usize);
    for c in changes.iter().flatten() {
        moved += 1;
        for k in 0..3 {
            sum[k] += c[k];
            largest = largest.max(c[k].abs());
        }
    }
    if moved == 0 {
        return (fill.clone(), None);
    }
    let mut out = fill.clone();
    let (ox, oy) = crop.origin;
    out.data
        .par_chunks_mut(w * 3)
        .enumerate()
        .skip(oy)
        .take(ch)
        .for_each(|(y, row)| {
            for x in 0..cw {
                let i = (y - oy) * cw + x;
                if let Some(c) = changes[i] {
                    for k in 0..3 {
                        let v = filled[k][i] * (-c[k]).exp() - f64::from(PEDESTAL);
                        row[(ox + x) * 3 + k] = to_srgb(v as f32);
                    }
                }
            }
        });
    let middle = Middle {
        mean: sum.map(|s| (-s / count).exp() as f32),
        largest: largest as f32,
        strength: strength as f32,
    };
    (out, Some(middle))
}

/// 0 at `from`, 1 at `to`, linear between and held past either end.
fn ramp(from: f64, to: f64, x: f64) -> f64 {
    ((x - from) / (to - from)).clamp(0.0, 1.0)
}

fn dot(a: &[f64; 6], b: &[f64; 6]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

/// The median of `v`, 0 for none.
fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mid = v.len() / 2;
    *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1
}

/// The spread of the log luma of `planes` round its local mean, over
/// [`LOCAL`], counting only the pixels `counts` was made of.
fn texture_of(planes: &[Vec<f64>; 3], counts: &Counts) -> Vec<f64> {
    let luma: Vec<f64> = (0..planes[0].len())
        .map(|i| {
            ((planes[0][i] + planes[1][i] + planes[2][i]) / 3.0)
                .max(1e-6)
                .ln()
        })
        .collect();
    let mean = counts.mean(&luma, LOCAL);
    let square: Vec<f64> = luma
        .iter()
        .zip(&mean)
        .map(|(l, m)| (l - m).powi(2))
        .collect();
    counts
        .mean(&square, LOCAL)
        .into_iter()
        .map(|v| v.max(0.0).sqrt())
        .collect()
}

/// A mask's pixel counts in boxes of [`PRE`] and [`LOCAL`], for the
/// means over its pixels alone.
struct Counts {
    weight: Vec<f64>,
    pre: Vec<f64>,
    local: Vec<f64>,
    width: usize,
    height: usize,
}

impl Counts {
    fn of(weight: &[f64], width: usize, height: usize) -> Counts {
        let (pre, local) = rayon::join(
            || box_sum(weight, width, height, PRE),
            || box_sum(weight, width, height, LOCAL),
        );
        Counts {
            weight: weight.to_vec(),
            pre,
            local,
            width,
            height,
        }
    }

    /// The mean of `a` over a box of `radius` ([`PRE`] or [`LOCAL`])
    /// round each pixel, counting only the mask's pixels (0 where none
    /// is).
    fn mean(&self, a: &[f64], radius: usize) -> Vec<f64> {
        let counts = if radius == PRE {
            &self.pre
        } else {
            &self.local
        };
        let weighted: Vec<f64> = a.iter().zip(&self.weight).map(|(a, w)| a * w).collect();
        box_sum(&weighted, self.width, self.height, radius)
            .iter()
            .zip(counts)
            .map(|(s, c)| if *c > 0.5 { s / c } else { 0.0 })
            .collect()
    }

    /// [`Counts::mean`] of three planes at once.
    fn means(&self, planes: &[Vec<f64>; 3], radius: usize) -> [Vec<f64>; 3] {
        let [a, b, c]: [Vec<f64>; 3] = planes
            .par_iter()
            .map(|p| self.mean(p, radius))
            .collect::<Vec<_>>()
            .try_into()
            .expect("three planes");
        [a, b, c]
    }
}

/// The sum of `a` over a box of `radius` round each pixel, the box cut
/// at the edges, by a summed-area table.
fn box_sum(a: &[f64], w: usize, h: usize, radius: usize) -> Vec<f64> {
    let stride = w + 1;
    let mut table = vec![0.0f64; stride * (h + 1)];
    for y in 0..h {
        let mut row = 0.0;
        for x in 0..w {
            row += a[y * w + x];
            table[(y + 1) * stride + x + 1] = table[y * stride + x + 1] + row;
        }
    }
    let mut out = vec![0.0f64; w * h];
    for y in 0..h {
        let (top, bottom) = (y.saturating_sub(radius), (y + radius + 1).min(h));
        for x in 0..w {
            let (left, right) = (x.saturating_sub(radius), (x + radius + 1).min(w));
            out[y * w + x] = table[bottom * stride + right]
                - table[top * stride + right]
                - table[bottom * stride + left]
                + table[top * stride + left];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 256;
    const H: usize = 192;

    /// A grain of `amount` (its standard deviation, relative), from a
    /// hash of `i`.
    fn grain(i: usize, amount: f32) -> f32 {
        let mut z = (i as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        let u = (z >> 40) as f32 / (1u64 << 24) as f32 - 0.5;
        1.0 + u * amount * 12f32.sqrt()
    }

    /// A sky: a gradient in linear light, bluer than it is red, under
    /// 2 percent of grain, encoded.
    fn sky() -> Rgbf {
        let data = (0..W * H)
            .flat_map(|i| {
                let (x, y) = ((i % W) as f32, (i / W) as f32);
                let v = (0.2 + 0.05 * x / W as f32 + 0.08 * y / H as f32) * grain(i, 0.02);
                [to_srgb(v * 0.7), to_srgb(v * 0.9), to_srgb(v * 1.2)]
            })
            .collect();
        Rgbf::new(W, H, data)
    }

    /// `truth` under `hole` taken through `f(linear, index, channel)`:
    /// a fill.
    fn fill_of(truth: &Rgbf, hole: &[f32], f: impl Fn(f32, usize, usize) -> f32) -> Rgbf {
        let mut out = truth.clone();
        for (i, _) in hole.iter().enumerate().filter(|(_, m)| **m > 0.0) {
            for k in 0..3 {
                let v = to_linear(truth.data[i * 3 + k]);
                out.data[i * 3 + k] = to_srgb(f(v, i, k));
            }
        }
        out
    }

    fn round(r: f32) -> Vec<f32> {
        (0..W * H)
            .map(|i| {
                let (x, y) = ((i % W) as f32 - 128.0, (i / W) as f32 - 96.0);
                f32::from(u8::from(x * x + y * y < r * r))
            })
            .collect()
    }

    /// The mean of `a` over `b` in linear light, per channel, over the
    /// pixels where `keep` says.
    fn ratio(a: &Rgbf, b: &Rgbf, keep: impl Fn(usize) -> bool) -> [f64; 3] {
        let mut s = [[0.0f64; 2]; 3];
        for i in (0..W * H).filter(|&i| keep(i)) {
            for (k, s) in s.iter_mut().enumerate() {
                s[0] += f64::from(to_linear(a.data[i * 3 + k]));
                s[1] += f64::from(to_linear(b.data[i * 3 + k]));
            }
        }
        s.map(|[a, b]| a / b)
    }

    /// The largest relative change in linear light of `a` from `b`.
    fn largest(a: &Rgbf, b: &Rgbf) -> f32 {
        a.data
            .iter()
            .zip(&b.data)
            .map(|(&u, &v)| (to_linear(u) / to_linear(v) - 1.0).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_thin_stroke_light_in_its_middle_comes_back_to_the_sky() {
        // A mast seven pixels wide, its fill 2.5 percent light and
        // bluer still, under the model's own grain: too thin for the
        // edge to read, all of it middle.
        let truth = sky();
        let hole: Vec<f32> = (0..W * H)
            .map(|i| {
                let (x, y) = (i % W, i / W);
                f32::from(u8::from((125..132).contains(&x) && (30..170).contains(&y)))
            })
            .collect();
        let fill = fill_of(&truth, &hole, |v, i, k| {
            v * [1.02, 1.025, 1.035][k] * grain(i * 7 + 3, 0.02) / grain(i, 0.02)
        });
        assert_eq!(super::super::matched(&truth, &hole, &fill).1, None);
        let (out, middle) = evened(&truth, &hole, &fill);
        let middle = middle.expect("a smooth sky round the stroke");
        let r = ratio(&out, &truth, |i| hole[i] > 0.0);
        for k in 0..3 {
            assert!((r[k] - 1.0).abs() < 0.003, "{r:?} {middle:?}");
        }
        // Along the stroke, a stretch at a time.
        for y0 in (40..160).step_by(20) {
            let r = ratio(&out, &truth, |i| {
                hole[i] > 0.0 && (y0..y0 + 20).contains(&(i / W))
            });
            let l = r.iter().sum::<f64>() / 3.0;
            assert!((l - 1.0).abs() < 0.003, "{y0}: {r:?}");
        }
    }

    #[test]
    fn a_bulge_the_edge_cannot_see_is_taken_out() {
        // At level at the edge, 2 percent light at the middle, on a
        // round hole: what the edge match leaves.
        let truth = sky();
        let hole = round(40.0);
        let fill = fill_of(&truth, &hole, |v, i, _| {
            let (x, y) = ((i % W) as f32 - 128.0, (i / W) as f32 - 96.0);
            v * (1.0 + 0.02 * (1.0 - (x * x + y * y) / 1600.0))
        });
        let (out, middle) = evened(&truth, &hole, &fill);
        assert!(middle.is_some());
        let deep = |i: usize| {
            let (x, y) = ((i % W) as f32 - 128.0, (i / W) as f32 - 96.0);
            x * x + y * y < 400.0
        };
        let before = ratio(&fill, &truth, deep);
        let after = ratio(&out, &truth, deep);
        for k in 0..3 {
            assert!(before[k] > 1.012, "{before:?}");
            assert!((after[k] - 1.0).abs() < 0.003, "{after:?}");
        }
    }

    #[test]
    fn a_fill_at_level_under_grain_is_left_within_a_hair() {
        // A draw of the fill's grain can read a field by chance; the
        // shrink keeps what it moves to a few tenths of a percent.
        let truth = sky();
        let hole = round(40.0);
        for seed in 0..20 {
            let fill = fill_of(&truth, &hole, |v, i, _| {
                v * grain(i * 31 + seed * 977 + 7, 0.02) / grain(i, 0.02)
            });
            let (out, _) = evened(&truth, &hole, &fill);
            let moved = largest(&out, &fill);
            assert!(moved < 0.005, "seed {seed}: {moved}");
        }
    }

    #[test]
    fn a_textured_wall_with_a_darker_patch_is_left_alone() {
        // Stones 16 px across, 10 percent apart, and a fill that drew
        // a shadow in part of the hole: the surround fits no smooth
        // surface.
        let data = (0..W * H)
            .flat_map(|i| {
                let (x, y) = (i % W, i / W);
                let stone = grain((x / 16) * 1000 + y / 12 + 17, 0.1);
                let v = 0.25 * stone * grain(i, 0.03);
                [to_srgb(v); 3]
            })
            .collect();
        let truth = Rgbf::new(W, H, data);
        let hole = round(40.0);
        let fill = fill_of(
            &truth,
            &hole,
            |v, i, _| {
                if i % W < 128 { v * 0.85 } else { v }
            },
        );
        let (out, middle) = evened(&truth, &hole, &fill);
        assert_eq!(middle, None);
        assert_eq!(out.data, fill.data);
    }

    #[test]
    fn a_uniformly_light_fill_is_the_edge_matchs_and_is_not_fought() {
        let truth = sky();
        let hole = round(40.0);
        let light = fill_of(&truth, &hole, |v, i, _| {
            v * 1.05 * grain(i * 31 + 7, 0.02) / grain(i, 0.02)
        });
        let (edge, gains) = super::super::matched(&truth, &hole, &light);
        assert!(gains.is_some());
        let (out, _) = evened(&truth, &hole, &edge);
        let inside = |i: usize| hole[i] > 0.0;
        let (e, o) = (ratio(&edge, &truth, inside), ratio(&out, &truth, inside));
        for k in 0..3 {
            // Whatever the edge left, the middle takes it further
            // toward the sky, never back.
            assert!(
                (o[k] - 1.0).abs() <= (e[k] - 1.0).abs() + 0.001,
                "{e:?} {o:?}"
            );
            assert!((o[k] - 1.0).abs() < 0.003, "{o:?}");
        }
        let moved = ratio(&out, &edge, inside);
        assert!(moved.iter().all(|m| (m - 1.0).abs() < 0.01), "{moved:?}");
    }

    #[test]
    fn a_hole_over_sky_and_hill_corrects_the_sky_alone() {
        // The lower part of the window a dark, textured hill; the hole
        // crosses its ridge, and its fill is 2 percent light in the sky
        // and right on the hill.
        let ridge = |x: usize| 85.0 + 0.2 * x as f32;
        let base = sky();
        let data = (0..W * H)
            .flat_map(|i| {
                let (x, y) = (i % W, i / W);
                let px = [0, 1, 2].map(|k| base.data[i * 3 + k]);
                if (y as f32) < ridge(x) {
                    px
                } else {
                    let v = 0.04 * grain((x / 6) * 977 + y / 5, 0.25) * grain(i, 0.05);
                    [to_srgb(v * 1.1), to_srgb(v), to_srgb(v * 0.8)]
                }
            })
            .collect();
        let truth = Rgbf::new(W, H, data);
        let hole = round(44.0);
        let in_sky = |i: usize| ((i / W) as f32) < ridge(i % W) - 3.0;
        let fill = fill_of(&truth, &hole, |v, i, _| {
            v * if in_sky(i) { 1.02 } else { 1.0 }
        });
        let (out, middle) = evened(&truth, &hole, &fill);
        assert!(middle.is_some());
        let sky_deep = |i: usize| hole[i] > 0.0 && ((i / W) as f32) < ridge(i % W) - 12.0;
        let r = ratio(&out, &truth, sky_deep);
        for k in 0..3 {
            assert!((r[k] - 1.0).abs() < 0.005, "{r:?}");
        }
        let hill_deep = |i: usize| hole[i] > 0.0 && ((i / W) as f32) > ridge(i % W) + 10.0;
        let r = ratio(&out, &fill, hill_deep);
        for k in 0..3 {
            assert!((r[k] - 1.0).abs() < 0.01, "{r:?}");
        }
    }

    /// A mast seven pixels wide down the middle of the window.
    fn stroke() -> Vec<f32> {
        (0..W * H)
            .map(|i| {
                let (x, y) = (i % W, i / W);
                f32::from(u8::from((125..132).contains(&x) && (30..170).contains(&y)))
            })
            .collect()
    }

    /// `truth` with the hole's pixels taken from `other`: a fill that
    /// is right under its own grain.
    fn refill(truth: &Rgbf, hole: &[f32], other: &Rgbf) -> Rgbf {
        let mut out = truth.clone();
        for (i, _) in hole.iter().enumerate().filter(|(_, m)| **m > 0.0) {
            out.data[i * 3..i * 3 + 3].copy_from_slice(&other.data[i * 3..i * 3 + 3]);
        }
        out
    }

    /// A sky under `shade(x, y)`, with the grain drawn from `seed`.
    fn shaded(seed: usize, shade: impl Fn(usize, usize) -> f32) -> Rgbf {
        let data = (0..W * H)
            .flat_map(|i| {
                let (x, y) = (i % W, i / W);
                let v = (0.2 + 0.05 * x as f32 / W as f32 + 0.08 * y as f32 / H as f32)
                    * grain(i * 31 + seed, 0.02)
                    * shade(x, y);
                [to_srgb(v * 0.7), to_srgb(v * 0.9), to_srgb(v * 1.2)]
            })
            .collect();
        Rgbf::new(W, H, data)
    }

    #[test]
    fn a_stroke_at_level_under_grain_is_left_within_a_hair() {
        let truth = shaded(0, |_, _| 1.0);
        let hole = stroke();
        for seed in 0..20 {
            let fill = refill(&truth, &hole, &shaded(seed * 977 + 7, |_, _| 1.0));
            let (out, _) = evened(&truth, &hole, &fill);
            let moved = largest(&out, &fill);
            assert!(moved < 0.005, "seed {seed}: {moved}");
        }
    }

    #[test]
    fn a_soft_shadow_across_the_hole_is_kept() {
        // A shadow 5 percent deep and 25 px wide crosses the hole, in
        // the picture and the fill alike: a second soft surface the
        // fill rightly continues.
        let shade = |x: usize, _: usize| {
            let t = (((x as f32 - 128.0).abs() - 12.0) / 6.0).clamp(-1.0, 1.0);
            1.0 - 0.05 * (0.5 - 0.5 * t)
        };
        let truth = shaded(0, shade);
        let hole = round(40.0);
        let fill = refill(&truth, &hole, &shaded(977, shade));
        let (edge, _) = super::super::matched(&truth, &hole, &fill);
        let (out, _) = evened(&truth, &hole, &edge);
        let r = ratio(&out, &edge, |i| {
            hole[i] > 0.0 && ((i % W) as f32 - 128.0).abs() < 6.0
        });
        for k in 0..3 {
            assert!((r[k] - 1.0).abs() < 0.005, "{r:?}");
        }
    }

    #[test]
    fn a_wisp_across_a_mast_is_kept() {
        // A wisp of cloud 6 percent light and 20 px tall across the
        // stroke: what the band shows beside it is carried in.
        let cloud = |_: usize, y: usize| {
            let d = (y as f32 - 100.0).abs() / 10.0;
            1.0 + 0.06 * (1.0 - d * d).max(0.0)
        };
        let truth = shaded(0, cloud);
        let hole = stroke();
        let fill = refill(&truth, &hole, &shaded(977, cloud));
        let (out, _) = evened(&truth, &hole, &fill);
        let r = ratio(&out, &fill, |i| {
            hole[i] > 0.0 && ((i / W) as f32 - 100.0).abs() < 5.0
        });
        for k in 0..3 {
            assert!((r[k] - 1.0).abs() < 0.005, "{r:?}");
        }
    }

    #[test]
    fn a_hue_step_at_equal_luma_is_kept() {
        // A blue sky on the left, a grey surface of the same mean-log
        // luma on the right, the fill right on both.
        let picture = |seed: usize| {
            let data = (0..W * H)
                .flat_map(|i| {
                    let x = i % W;
                    let v = (0.2 + 0.08 * (i / W) as f32 / H as f32) * grain(i * 31 + seed, 0.02);
                    let tint = if x < 128 {
                        [0.7, 0.9, 1.2]
                    } else {
                        [0.756f32.cbrt(); 3]
                    };
                    tint.map(|t| to_srgb(v * t))
                })
                .collect();
            Rgbf::new(W, H, data)
        };
        let truth = picture(0);
        let hole = round(40.0);
        let fill = refill(&truth, &hole, &picture(977));
        let (out, _) = evened(&truth, &hole, &fill);
        let moved = largest(&out, &fill);
        assert!(moved < 0.005, "{moved}");
    }
}
