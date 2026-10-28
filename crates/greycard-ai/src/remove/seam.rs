//! The level under the hole: the UNet can set the whole fill a little
//! light or dark (the four-step remove about a fifth of a stop light on
//! a flat wall), which the ring match ([`super::level`]) cannot see,
//! since it compares the decoder's answer with the picture outside the
//! hole only. Here the fill is compared with the picture across the
//! hole's edge: one plane, in linear light, is fitted to a band of the
//! picture set back from the edge ([`GAP`]) and a band of the fill just
//! inside it, the fill's side raised by a step in each of [`SECTORS`]
//! sectors round the hole. The plane is the picture's own gradient,
//! the steps are the fill's level. A level the UNet set for the whole
//! hole is the same step all round it, give or take the texture; where
//! the sectors disagree, the band holds something of the picture's own
//! (a horizon along the edge, a texture the fill took up out of phase)
//! and the fill is left as it is. Where they agree, their mean, each
//! held near the median, is multiplied into the whole fill, which
//! leaves its texture as it was.

use std::f64::consts::PI;

use super::scale::{to_linear, to_srgb};
use crate::fill::depth;
use crate::image::Rgbf;

/// How deep the fill's side of the band is, inside the hole's edge,
/// in pixels.
pub const BAND: usize = 4;

/// How far out from the hole's edge the picture's side of the band
/// starts, in pixels, and how deep it is past that. A removed object
/// leaves a rim round itself in the picture (an anti-aliased fringe, a
/// halo, a contact shadow) that the hole's coverage does not take in;
/// set back past it, the surface carries the picture's level over the
/// gap.
pub const GAP: usize = 8;
pub const OUTER: usize = 8;

/// The surface's terms: a plane. A quadratic's `u² + v²` tells a ring
/// inside the edge from one outside it, which is the step itself, so
/// it takes up part of a level the fill sits at.
const SURFACE: usize = 3;

/// The sectors round the hole's middle that each have a step.
pub const SECTORS: usize = 8;

/// The least share of the fill's side of the band a sector must hold,
/// and of the picture's, for its step to count; and the least number
/// of sectors that must count for the fill to be matched at all.
const SHARE: f64 = 0.25 / SECTORS as f64;
const COUNTED: usize = 5;

/// How far a sector's step may be from the median of them, in logs,
/// to agree with it, and how much of the edge must agree for the fill
/// to be matched: a level the UNet set is the same all round, give or
/// take the texture's say in a sector (up to a tenth on a stone wall),
/// where a horizon or a wall's edge along the hole's edge, or a texture
/// taken up out of phase, sets whole sectors far apart.
const AGREE: (f64, f64) = (0.1, 0.75);

/// The gains the edge may ask for; past them the fill is left as it
/// is. The four-step remove was measured a fifth of a stop light (1.14)
/// on a flat wall, the 25-step one 1.10.
pub const GAINS: (f32, f32) = (0.8, 1.25);

/// What the gain is taken about, in linear light: the fill and the
/// picture are compared as `v + PEDESTAL`, and the fill is scaled the
/// same way, so near black the match becomes an offset rather than a
/// gain on a value near zero. The window comes in at a grey gain,
/// round 0.18; this is about four stops below that.
pub const PEDESTAL: f32 = 0.01;

/// The gain put into the hole, per channel.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Gains {
    pub gain: [f32; 3],
}

/// `fill` (sRGB-encoded, in 0 to 1, like `image`) with its pixels under
/// `hole` (nonzero is in it) brought to `image`'s level across the
/// hole's edge, and the gains that took; `fill` as it was and `None`
/// where there is no edge to compare across, the hole is no deeper
/// than the band (a stroke a few pixels wide: its edge is all of it),
/// or the edge does not agree on one level (logged).
pub fn matched(image: &Rgbf, hole: &[f32], fill: &Rgbf) -> (Rgbf, Option<Gains>) {
    let (w, h) = (image.width, image.height);
    if fill.width != w || fill.height != h || hole.len() != w * h {
        return (fill.clone(), None);
    }
    if !hole.iter().any(|&m| m > 0.0) || depth(hole, w, h) < BAND as f32 {
        return (fill.clone(), None);
    }
    let known: Vec<f32> = hole
        .iter()
        .map(|&m| f32::from(u8::from(m <= 0.0)))
        .collect();
    let near = super::ring(hole, w, h, GAP);
    let outer: Vec<bool> = super::ring(hole, w, h, GAP + OUTER)
        .into_iter()
        .zip(&near)
        .map(|(o, n)| o && !n)
        .collect();
    let inner = super::ring(&known, w, h, BAND);

    // The hole's middle and size, for the sectors and for the surface's
    // coordinates.
    let (mut mx, mut my, mut n) = (0.0f64, 0.0f64, 0.0f64);
    let (mut x0, mut x1, mut y0, mut y1) = (w, 0, h, 0);
    for (i, _) in hole.iter().enumerate().filter(|(_, m)| **m > 0.0) {
        let (x, y) = (i % w, i / w);
        mx += x as f64;
        my += y as f64;
        n += 1.0;
        (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
    }
    let (mx, my) = (mx / n, my / n);
    let size = (((x1 - x0).max(y1 - y0) + 1) as f64 / 2.0 + (GAP + OUTER) as f64).max(1.0);
    let place = |x: usize, y: usize| ((x as f64 - mx) / size, (y as f64 - my) / size);
    let sector = |(u, v): (f64, f64)| {
        (((v.atan2(u) + PI) / (2.0 * PI) * SECTORS as f64) as usize).min(SECTORS - 1)
    };

    // Which side of the band a pixel is on, if either; and which
    // sectors hold enough of both sides.
    let side = |i: usize| {
        if outer[i] {
            Some(false)
        } else if inner[i] {
            Some(true)
        } else {
            None
        }
    };
    let mut counts = [[0usize; SECTORS]; 2];
    let mut middles = [(0.0f64, 0.0f64); SECTORS];
    for i in 0..w * h {
        if let Some(inside) = side(i) {
            let at = place(i % w, i / w);
            let s = sector(at);
            counts[usize::from(inside)][s] += 1;
            if inside {
                middles[s].0 += at.0;
                middles[s].1 += at.1;
            }
        }
    }
    let totals = counts.map(|c| c.iter().sum::<usize>() as f64);
    let counted: Vec<usize> = (0..SECTORS)
        .filter(|&s| (0..2).all(|side| counts[side][s] as f64 >= SHARE * totals[side]))
        .collect();
    if counted.len() < COUNTED {
        log::info!(
            "remove: the level under the hole left as it is: {} of its {SECTORS} sectors have band enough to compare",
            counted.len()
        );
        return (fill.clone(), None);
    }

    // One plane over the whole band in linear light, a step on the
    // fill's side by least squares: `column` names each sector's step's
    // parameter, `None` leaving that sector's fill side out.
    let fit = |column: &[Option<usize>], params: usize| -> Option<Vec<Vec<f64>>> {
        let mut normal = vec![0.0f64; params * params];
        let mut rhs = vec![[0.0f64; 3]; params];
        let mut basis = vec![0.0f64; params];
        for i in 0..w * h {
            let Some(inside) = side(i) else {
                continue;
            };
            let (u, v) = place(i % w, i / w);
            let step = column[sector((u, v))];
            if inside && step.is_none() {
                continue;
            }
            let from = if inside { fill } else { image };
            let z: [f64; 3] = std::array::from_fn(|k| f64::from(to_linear(from.data[i * 3 + k])));
            basis.fill(0.0);
            basis[..SURFACE].copy_from_slice(&[1.0, u, v]);
            if let Some(j) = step.filter(|_| inside) {
                basis[j] = 1.0;
            }
            for (a, &ba) in basis.iter().enumerate().filter(|(_, b)| **b != 0.0) {
                for (b, &bb) in basis.iter().enumerate() {
                    normal[a * params + b] += ba * bb;
                }
                for (r, z) in rhs[a].iter_mut().zip(z) {
                    *r += ba * z;
                }
            }
        }
        (0..3)
            .map(|k| solve(normal.clone(), rhs.iter().map(|r| r[k]).collect(), params))
            .collect()
    };
    // The step of `x`'s parameter `j` as a log gain about the pedestal,
    // at the plane's value at `(u, v)`; NaN where either side is not
    // above the pedestal's negative.
    let pedestal = f64::from(PEDESTAL);
    let gain_of = |x: &[Vec<f64>], j: usize, (u, v): (f64, f64)| -> [f64; 3] {
        std::array::from_fn(|k| {
            let picture = x[k][0] + x[k][1] * u + x[k][2] * v + pedestal;
            let fill = picture + x[k][j];
            if picture > 0.0 && fill > 0.0 {
                (picture / fill).ln()
            } else {
                f64::NAN
            }
        })
    };

    // A step in each counted sector, as a gain where the sector's side
    // of the fill lies.
    let columns: Vec<Option<usize>> = (0..SECTORS)
        .map(|s| counted.iter().position(|&c| c == s).map(|p| SURFACE + p))
        .collect();
    let Some(x) = fit(&columns, SURFACE + counted.len()) else {
        return (fill.clone(), None);
    };
    // A sector whose plane is not above the pedestal's negative has no
    // step; it still counts in the edge, against the agreement.
    let total: f64 = counted.iter().map(|&s| counts[1][s] as f64).sum();
    let (steps, weights): (Vec<[f64; 3]>, Vec<f64>) = counted
        .iter()
        .enumerate()
        .map(|(j, &s)| {
            let n = counts[1][s] as f64;
            let step = gain_of(&x, SURFACE + j, (middles[s].0 / n, middles[s].1 / n));
            (step, n)
        })
        .filter(|(step, _)| step.iter().all(|v| v.is_finite()))
        .unzip();

    // The steps must agree round the hole, and lie within the gains.
    let median: [f64; 3] = std::array::from_fn(|k| {
        let mut v: Vec<(f64, f64)> = steps
            .iter()
            .map(|s| s[k])
            .zip(weights.iter().copied())
            .collect();
        weighted_median(&mut v)
    });
    let agreeing: f64 = steps
        .iter()
        .zip(&weights)
        .filter(|(s, _)| s.iter().zip(&median).all(|(s, m)| (s - m).abs() <= AGREE.0))
        .map(|(_, w)| w)
        .sum();
    log::debug!(
        "remove: the edge's sectors ask for gains of {:.3?}",
        steps.iter().map(|s| s.map(f64::exp)).collect::<Vec<_>>()
    );
    let (lo, hi) = (f64::from(GAINS.0).ln(), f64::from(GAINS.1).ln());
    if agreeing < AGREE.1 * total || median.iter().any(|m| !(lo..=hi).contains(m)) {
        log::info!(
            "remove: the level under the hole left as it is: its edge asks for gains of {:.3}, {:.3}, and {:.3}, {:.0}% of it within {:.2} of that, against {:.0}% and gains of {:.2} to {:.2}",
            median[0].exp(),
            median[1].exp(),
            median[2].exp(),
            100.0 * agreeing / total.max(1.0),
            AGREE.0.exp(),
            100.0 * AGREE.1,
            GAINS.0,
            GAINS.1
        );
        return (fill.clone(), None);
    }

    // The sectors' steps each held to within the agreement of the
    // median and averaged by their share of the edge: a sector of
    // texture counts in full, one of the picture's own structure that
    // the agreement let through for at most its tolerance.
    let gain: [f32; 3] = std::array::from_fn(|k| {
        let held: f64 = steps
            .iter()
            .zip(&weights)
            .map(|(s, w)| w * s[k].clamp(median[k] - AGREE.0, median[k] + AGREE.0))
            .sum();
        (held / weights.iter().sum::<f64>()).exp() as f32
    });
    if !gain.iter().all(|g| g.is_finite()) {
        return (fill.clone(), None);
    }
    let mut out = fill.clone();
    for (i, _) in hole.iter().enumerate().filter(|(_, m)| **m > 0.0) {
        for (k, g) in gain.iter().enumerate() {
            let v = to_linear(fill.data[i * 3 + k]);
            out.data[i * 3 + k] = to_srgb((v + PEDESTAL) * g - PEDESTAL);
        }
    }
    (out, Some(Gains { gain }))
}

/// The weighted median of `(value, weight)` pairs, 0 for none.
fn weighted_median(v: &mut [(f64, f64)]) -> f64 {
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let half = v.iter().map(|p| p.1).sum::<f64>() / 2.0;
    let mut run = 0.0;
    for &(value, weight) in v.iter() {
        run += weight;
        if run >= half {
            return value;
        }
    }
    0.0
}

/// `m x = r` for an `n` × `n` `m` stored by rows, by Gaussian
/// elimination with partial pivoting; `None` when `m` is singular or
/// near it.
fn solve(mut m: Vec<f64>, mut r: Vec<f64>, n: usize) -> Option<Vec<f64>> {
    let scale = m.iter().fold(0.0f64, |a, v| a.max(v.abs()));
    for col in 0..n {
        let pivot =
            (col..n).max_by(|&a, &b| m[a * n + col].abs().total_cmp(&m[b * n + col].abs()))?;
        if m[pivot * n + col].abs() <= scale * 1e-12 {
            return None;
        }
        if pivot != col {
            for j in 0..n {
                m.swap(col * n + j, pivot * n + j);
            }
            r.swap(col, pivot);
        }
        for row in col + 1..n {
            let t = m[row * n + col] / m[col * n + col];
            if t == 0.0 {
                continue;
            }
            for j in col..n {
                m[row * n + j] -= t * m[col * n + j];
            }
            r[row] -= t * r[col];
        }
    }
    let mut x = vec![0.0f64; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|j| m[row * n + j] * x[j]).sum();
        x[row] = (r[row] - s) / m[row * n + row];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 256;
    const H: usize = 192;

    /// A `W` × `H` window of `f(x, y)` in linear light, the same in
    /// every channel, encoded.
    fn picture(f: impl Fn(f32, f32) -> f32) -> Rgbf {
        Rgbf::new(
            W,
            H,
            (0..W * H)
                .flat_map(|i| [to_srgb(f((i % W) as f32, (i / W) as f32)); 3])
                .collect(),
        )
    }

    /// A round hole of radius `r` about (120, 90).
    fn round(r: f32) -> Vec<f32> {
        (0..W * H)
            .map(|i| {
                let (x, y) = ((i % W) as f32 - 120.0, (i / W) as f32 - 90.0);
                f32::from(u8::from(x * x + y * y < r * r))
            })
            .collect()
    }

    /// `truth` under `hole` taken through `f` in linear light, the
    /// rest left as it is: a fill.
    fn fill_of(truth: &Rgbf, hole: &[f32], f: impl Fn(f32, usize) -> f32) -> Rgbf {
        let mut out = truth.clone();
        for (i, &m) in hole.iter().enumerate() {
            if m > 0.0 {
                for k in 0..3 {
                    let v = to_linear(truth.data[i * 3 + k]);
                    out.data[i * 3 + k] = to_srgb(f(v, i));
                }
            }
        }
        out
    }

    /// The largest relative difference in linear light of `a` from `b`
    /// over the pixels where `keep` says.
    fn worst(a: &Rgbf, b: &Rgbf, keep: impl Fn(usize, usize) -> bool) -> f32 {
        let mut most = 0.0f32;
        for y in 0..H {
            for x in 0..W {
                if keep(x, y) {
                    let i = (y * W + x) * 3;
                    for k in 0..3 {
                        let (u, v) = (to_linear(a.data[i + k]), to_linear(b.data[i + k]));
                        most = most.max((u / v - 1.0).abs());
                    }
                }
            }
        }
        most
    }

    /// The mean and largest relative change in linear light of `a`
    /// from `b` over the pixels where `keep` says.
    fn change(a: &Rgbf, b: &Rgbf, keep: impl Fn(usize, usize) -> bool) -> (f32, f32) {
        let (mut sum, mut n) = (0.0f32, 0.0f32);
        for y in 0..H {
            for x in 0..W {
                if keep(x, y) {
                    let i = (y * W + x) * 3;
                    for k in 0..3 {
                        let (u, v) = (to_linear(a.data[i + k]), to_linear(b.data[i + k]));
                        sum += (u / v - 1.0).abs();
                        n += 1.0;
                    }
                }
            }
        }
        (sum / n, worst(a, b, keep))
    }

    fn within(r: f32) -> impl Fn(usize, usize) -> bool {
        move |x, y| {
            let (dx, dy) = (x as f32 - 120.0, y as f32 - 90.0);
            dx * dx + dy * dy < r * r
        }
    }

    /// A grain of 2 percent (its standard deviation), from a hash of
    /// `i`.
    fn grain(i: usize) -> f32 {
        let mut z = (i as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        1.0 + ((z >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.07
    }

    #[test]
    fn a_light_fill_on_a_gradient_comes_back_to_the_picture() {
        let truth = picture(|x, y| 0.1 + 0.3 * x / W as f32 + 0.05 * y / H as f32);
        let hole = round(40.0);
        let light = fill_of(&truth, &hole, |v, _| v * 1.14);
        let (out, gains) = matched(&truth, &hole, &light);
        let gains = gains.expect("an edge to match across");
        for k in 0..3 {
            assert!(gains.gain[k] > 0.85 && gains.gain[k] < 0.91, "{gains:?}");
        }
        let middle = worst(&out, &truth, within(20.0));
        assert!(middle < 0.01, "{middle}");
        let all = worst(&out, &truth, within(40.0));
        assert!(all < 0.03, "{all}");
    }

    #[test]
    fn a_fill_at_the_pictures_level_is_left_alone() {
        let truth = picture(|x, y| 0.1 + 0.3 * x / W as f32 + 0.05 * y / H as f32);
        let truth = fill_of(&truth, &vec![1.0; W * H], |v, i| v * grain(i));
        let hole = round(40.0);
        // The model's own grain, not the picture's.
        let fill = fill_of(&truth, &hole, |v, i| v * grain(i * 31 + 7) / grain(i));
        let (out, _) = matched(&truth, &hole, &fill);
        let (mean, most) = change(&out, &fill, within(40.0));
        assert!(mean < 0.002 && most < 0.005, "{mean} {most}");
        // Without the grain, nothing to read wrong.
        let plain = picture(|x, y| 0.1 + 0.3 * x / W as f32 + 0.05 * y / H as f32);
        let (out, _) = matched(&plain, &hole, &plain);
        let changed = worst(&out, &plain, within(40.0));
        assert!(changed < 0.002, "{changed}");
    }

    #[test]
    fn a_fill_with_its_own_structure_keeps_it() {
        // Light and dark patches, the fill's the picture's: what the
        // model drew inside is not pulled to a guess from outside.
        let patches = |x: f32, y: f32| 0.2 + 0.15 * (x / 10.0).sin() * (y / 12.0).sin();
        let truth = picture(patches);
        let hole = round(40.0);
        let (out, _) = matched(&truth, &hole, &truth);
        let (mean, most) = change(&out, &truth, within(40.0));
        assert!(mean < 0.005 && most < 0.01, "{mean} {most}");
    }

    #[test]
    fn a_hole_across_a_step_gets_no_gradient_of_its_own() {
        let step = |at: f32| move |_: f32, y: f32| if y < at { 0.6 } else { 0.1 };
        let truth = picture(step(90.0));
        let hole = round(40.0);
        // The fill draws the step where the picture has it, at its
        // level or a seventh light: left alone (one smooth surface does
        // not hold the step, so the sectors disagree) or matched.
        alone_or_within(&truth, &hole, &truth, 0.01);
        let light = fill_of(&truth, &hole, |v, _| v * 1.14);
        let (out, gains) = matched(&truth, &hole, &light);
        if gains.is_some() {
            let off = worst(&out, &truth, within(40.0));
            assert!(off < 0.03, "{off}");
        }
        // The fill draws it six pixels low: left alone, or changed by a
        // percent at most.
        let low = picture(step(96.0));
        let fill = fill_of(&truth, &hole, |_, i| to_linear(low.data[i * 3]));
        let (out, gains) = matched(&truth, &hole, &fill);
        if gains.is_some() {
            let changed = worst(&out, &fill, within(40.0));
            assert!(changed < 0.01, "{changed}");
        }
    }

    #[test]
    fn near_black_is_matched_by_an_offset() {
        let truth = picture(|_, _| 0.002);
        let hole = round(40.0);
        // Twice the picture, and black: neither is scaled past what
        // the pedestal allows, and both land on the picture's level.
        for (f, near) in [(2.0f32, 0.002f32), (0.0, 0.002)] {
            let fill = fill_of(&truth, &hole, |v, _| v * f);
            let (out, gains) = matched(&truth, &hole, &fill);
            let gains = gains.expect("an edge");
            for k in 0..3 {
                assert!(gains.gain[k] >= GAINS.0 && gains.gain[k] <= GAINS.1);
            }
            let v = to_linear(out.at(120, 90)[0]);
            assert!((v - near).abs() < 2e-4, "{f}: {v}");
        }
    }

    #[test]
    fn no_edge_or_a_thin_stroke_is_left_alone() {
        let truth = picture(|_, _| 0.3);
        let light = picture(|_, _| 0.4);
        let (_, gains) = matched(&truth, &vec![0.0; W * H], &light);
        assert!(gains.is_none());
        let (_, gains) = matched(&truth, &vec![1.0; W * H], &light);
        assert!(gains.is_none());
        let mut stroke = vec![0.0; W * H];
        for x in 40..200 {
            stroke[90 * W + x] = 1.0;
            stroke[91 * W + x] = 1.0;
        }
        let (out, gains) = matched(&truth, &stroke, &light);
        assert!(gains.is_none());
        assert_eq!(out, light);
    }

    /// Either the fill is left as it is, or it comes back within
    /// `bound` of `truth` (relative, in linear light) everywhere in
    /// the hole.
    fn alone_or_within(truth: &Rgbf, hole: &[f32], fill: &Rgbf, bound: f32) -> Option<f32> {
        let (out, gains) = matched(truth, hole, fill);
        let inside = |x: usize, y: usize| hole[y * W + x] > 0.0;
        match gains {
            None => {
                assert_eq!(&out, fill);
                None
            }
            Some(_) => {
                let off = worst(&out, truth, inside);
                assert!(off < bound, "{off}");
                Some(off)
            }
        }
    }

    /// The distance of (x, y) outside the round hole of radius 40.
    fn out_of(x: f32, y: f32) -> f32 {
        ((x - 120.0).powi(2) + (y - 90.0).powi(2)).sqrt() - 40.0
    }

    #[test]
    fn a_rim_round_the_object_does_not_set_the_level() {
        // What a removed object leaves round itself and the hole does
        // not take in: a fringe or a shadow a pixel or two wide, a
        // contact shadow fading over eight.
        let hole = round(40.0);
        type Rim = Box<dyn Fn(f32, f32) -> f32>;
        let rims: [(&str, Rim); 6] = [
            (
                "1 px at 0.36",
                Box::new(|d, _| if d < 1.0 { 0.36 } else { 0.4 }),
            ),
            (
                "1 px at 0.30",
                Box::new(|d, _| if d < 1.0 { 0.30 } else { 0.4 }),
            ),
            (
                "2 px at 0.30",
                Box::new(|d, _| if d < 2.0 { 0.30 } else { 0.4 }),
            ),
            (
                "1 px at 0.05",
                Box::new(|d, _| if d < 1.0 { 0.05 } else { 0.4 }),
            ),
            (
                "a contact shadow",
                Box::new(|d, _| 0.3 + 0.1 * (d / 8.0).clamp(0.0, 1.0)),
            ),
            (
                "a contact shadow below",
                Box::new(|d, y| {
                    if y > 90.0 {
                        0.3 + 0.1 * (d / 8.0).clamp(0.0, 1.0)
                    } else {
                        0.4
                    }
                }),
            ),
        ];
        for (name, rim) in rims {
            let truth = picture(|x, y| {
                let d = out_of(x, y);
                if d < 0.0 { 0.4 } else { rim(d, y) }
            });
            // The fill the wall's own level: left alone or within 1%.
            alone_or_within(&truth, &hole, &truth, 0.01);
            // A seventh light: taken back to the wall's level.
            let light = fill_of(&truth, &hole, |v, _| v * 1.14);
            let (out, gains) = matched(&truth, &hole, &light);
            assert!(gains.is_some(), "{name}");
            let off = worst(&out, &truth, within(36.0));
            assert!(off < 0.015, "{name}: {off}");
        }
    }

    #[test]
    fn an_edge_of_the_picture_along_the_holes_is_left_to_the_fill() {
        // A wall's edge along one side of a box-shaped hole, and a
        // horizon along the top of a wide, shallow one, the fill
        // drawing each where the picture has it.
        let boxed = |x0: usize, x1: usize, y0: usize, y1: usize| -> Vec<f32> {
            (0..W * H)
                .map(|i| {
                    let (x, y) = (i % W, i / W);
                    f32::from(u8::from((x0..x1).contains(&x) && (y0..y1).contains(&y)))
                })
                .collect()
        };
        let wall = picture(|x, _| if x < 120.0 { 0.6 } else { 0.1 });
        alone_or_within(&wall, &boxed(120, 200, 60, 130), &wall, 0.01);
        let horizon = picture(|_, y| if y < 80.0 { 0.5 } else { 0.08 });
        alone_or_within(&horizon, &boxed(30, 210, 80, 110), &horizon, 0.01);
    }

    #[test]
    fn a_texture_the_fill_took_up_out_of_phase_is_left_to_it() {
        let patches =
            |dx: f32| move |x: f32, y: f32| 0.2 + 0.15 * ((x + dx) / 10.0).sin() * (y / 12.0).sin();
        let truth = picture(patches(0.0));
        let hole = round(40.0);
        for dx in [15.0, 31.0] {
            let shifted = picture(patches(dx));
            let fill = fill_of(&truth, &hole, |_, i| to_linear(shifted.data[i * 3]));
            let (out, gains) = matched(&truth, &hole, &fill);
            if gains.is_some() {
                let changed = worst(&out, &fill, within(40.0));
                assert!(changed < 0.01, "{dx}: {changed}");
            }
        }
    }

    #[test]
    fn known_gap_a_long_thin_stroke_is_never_matched() {
        // Sectors in the hole's principal-axis frame, or by position
        // along the edge, would change this: today the angular sectors
        // about the middle leave four of eight too little band.
        let truth = picture(|x, y| 0.2 + 0.1 * x / W as f32 + 0.02 * y / H as f32);
        let stroke: Vec<f32> = (0..W * H)
            .map(|i| {
                let (x, y) = (i % W, i / W);
                f32::from(u8::from((28..228).contains(&x) && (84..96).contains(&y)))
            })
            .collect();
        let light = fill_of(&truth, &stroke, |v, _| v * 1.14);
        let (out, gains) = matched(&truth, &stroke, &light);
        assert!(gains.is_none());
        assert_eq!(out, light);
    }

    #[test]
    fn known_gap_a_blotchy_wall_refuses_a_true_level() {
        // A gate that weighs a sector's scatter against the texture in
        // its band would change this: today a uniform error on blotches
        // about the hole's size leaves half the edge disagreeing.
        let blotches = |x: f32, y: f32| 0.2 * (1.0 + 0.25 * (x / 10.0).sin() * (y / 12.0).sin());
        let truth = picture(blotches);
        let hole = round(40.0);
        let light = fill_of(&truth, &hole, |v, _| v * 1.14);
        let (out, gains) = matched(&truth, &hole, &light);
        assert!(gains.is_none());
        assert_eq!(out, light);
    }
}
