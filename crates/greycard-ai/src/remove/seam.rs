//! The level under the hole: the UNet can set the whole fill a little
//! light or dark (the four-step remove about a fifth of a stop light on
//! a flat wall), which the ring match ([`super::level`]) cannot see,
//! since it compares the decoder's answer with the picture outside the
//! hole only. Here the fill is compared with the picture across the
//! hole's edge, in linear light: a band of the picture set back from
//! the edge ([`GAP`]) and a band of the fill just inside it, in each of
//! [`SECTORS`] sectors round the hole, are fitted with a tilt shared by
//! all of them, a level of the picture's in each sector, and a step on
//! the fill's side of each. The tilt is the picture's own gradient, the
//! steps are the fill's level. A level the UNet set for the whole hole
//! is the same step all round it, give or take each sector's texture,
//! and sectors over another surface or a busy texture count for less;
//! where the sectors disagree past their texture, the band holds
//! something of the picture's own (a horizon along the edge), and the
//! fill is left as it is. Otherwise their mean, each held near the
//! median, is shrunk toward no change by its error and multiplied into
//! the whole fill, which leaves its texture as it was. The error is the
//! sectors' own scatter and what the same read gives on the picture
//! alone, across pseudo-edges further out: a texture at the hole's
//! scale reads a level between the band's two radii that the scatter
//! does not show. A fill that took up a texture out of phase, or sits
//! at the picture's level under its own grain or over a patterned
//! surface, reads a mean within that error and is left alone, and so
//! is a true level that small.

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

/// The sectors round the hole's middle that each have a step. Their
/// mean is shrunk by how they scatter (see [`RESOLVED`]), so there must
/// be enough of them for the scatter to say how well the mean is
/// known: with eight, a fill that kept the picture's own texture could
/// read a level its scatter did not show for a chance one.
pub const SECTORS: usize = 16;

/// The least share of the fill's side of the band a sector must hold,
/// and of the picture's, for its step to count; and the least number
/// of sectors that must count for the fill to be matched at all.
const SHARE: f64 = 0.25 / SECTORS as f64;
const COUNTED: usize = 10;

/// How far a sector's step may be from the median of them to agree
/// with it, in its texture's sigmas (see [`TEXTURE`]), and how much of
/// the edge, by weight, must agree for the fill to be matched: a level
/// the UNet set is the same all round, give or take the texture's say
/// in a sector (on a stone wall the steps scatter about as far as the
/// texture itself), where a horizon or a wall's edge along the hole's
/// edge sets whole smooth sectors far apart. Two sigmas and three
/// quarters are what the single plane's gate had, 0.1 and 75%, kept
/// for a smooth sector.
const AGREE: (f64, f64) = (2.0, 0.75);

/// The error the edge's level is shrunk by, as a multiple: the level
/// is taken times `1 - (RESOLVED * error / level)²`, and none of it
/// within its error. The error is `sqrt(e² + mean(l²))`: `e` the
/// sectors' standard error about their mean, `l` the levels the picture
/// alone reads across the pseudo-edges of [`NULLS`].
const RESOLVED: f64 = 1.0;

/// How far out from the hole's edge the pseudo-edges are that the
/// picture is read across on its own, in pixels. Their levels enter
/// the error as a mean square, so one reading alone counts at one over
/// root three. Near the window's or the frame's edge a pseudo-edge's
/// ring is partial and fewer than three may read, which is safe (none
/// leaves the fill alone) but weakens the three.
const NULLS: &[usize] = &[8, 12, 16];

/// A pixel of the band: on the fill's side or not, its sector, its
/// place, and its linear value.
type Pixel = (bool, usize, (f64, f64), [f64; 3]);

/// The tilt's parameters, shared by every sector: the plane's slope
/// across and down the hole. A quadratic's `u² + v²` would tell a ring
/// inside the edge from one outside it, which is the step itself, so
/// it would take up part of a level the fill sits at.
const TILT: usize = 2;

/// How many times the fit is made, each after the first with the
/// band's pixels weighted down past [`HUBER`] times the scale of its
/// residuals (their median, as a normal's sigma).
const PASSES: usize = 3;
const HUBER: f64 = 1.345;

/// The least texture a sector is taken to have, as a share of its
/// level, in logs: a sector's sigma is this and its band's own texture
/// (the residuals' spread, picture side and fill side) together, so a
/// smooth sector may stray 0.1 from the median, as it could before the
/// sigmas, and one over another surface or a busy texture further, for
/// a weight that much smaller.
const TEXTURE: f64 = 0.05;

/// A counted sector's step, in logs, its sigma, and its weight.
struct Sector {
    step: [f64; 3],
    sigma: f64,
    weight: f64,
}

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
/// the edge does not agree on one level, there is too little picture
/// round it to read the picture's own texture across, or the level it
/// agrees on is within its error of none (all logged). A level past its
/// error is shrunk the more the nearer it is to it.
pub fn matched(image: &Rgbf, hole: &[f32], fill: &Rgbf) -> (Rgbf, Option<Gains>) {
    let (w, h) = (image.width, image.height);
    if fill.width != w || fill.height != h || hole.len() != w * h {
        return (fill.clone(), None);
    }
    // Every read stays within the furthest pseudo-edge's band of the
    // hole, so the reads see only the hole's box grown by that much:
    // the rest of a large window is never in a band.
    let reach = NULLS.iter().copied().max().unwrap_or(0) + GAP + OUTER;
    let Some(crop) = Crop::round(hole, w, h, reach) else {
        return (fill.clone(), None);
    };
    let (image_c, fill_c, hole_c) = (crop.rgb(image), crop.rgb(fill), crop.mask(hole));
    let Reading {
        mean,
        level,
        error,
        agreeing,
        asks,
        sigmas,
    } = match read(&image_c, &hole_c, &fill_c, crop.origin) {
        Ok(r) => r,
        Err(why) => {
            if let Some(why) = why {
                log::info!("remove: the level under the hole left as it is: {why}");
            }
            return (fill.clone(), None);
        }
    };
    log::debug!(
        "remove: the edge's sectors ask for gains of {asks:.3?}, their textures {sigmas:.3?}"
    );
    // The same read on the picture alone, across pseudo-edges further
    // out (the hole grown by each of [`NULLS`]): a texture at the
    // hole's scale differs between the band's two radii coherently all
    // round the ring, which the sectors' scatter does not see, and
    // reads the same way on the picture's own. The readings' mean
    // square is added to the error's square.
    let mut nulls: Vec<f64> = Vec::with_capacity(NULLS.len());
    for &d in NULLS {
        let grown: Vec<f32> = hole_c
            .iter()
            .zip(super::ring(&hole_c, crop.width, crop.height, d))
            .map(|(&m, r)| if m > 0.0 || r { 1.0 } else { 0.0 })
            .collect();
        if let Ok(null) = read(&image_c, &grown, &image_c, crop.origin) {
            nulls.push(null.level);
        }
    }
    if nulls.is_empty() {
        log::info!(
            "remove: the level under the hole left as it is: too little picture round it to read its own texture against"
        );
        return (fill.clone(), None);
    }
    let bias = nulls.iter().map(|l| l * l).sum::<f64>() / nulls.len() as f64;
    let error = (error * error + bias).sqrt();
    let (lo, hi) = (f64::from(GAINS.0).ln(), f64::from(GAINS.1).ln());
    if agreeing < AGREE.1 || mean.iter().any(|m| !(lo..=hi).contains(m)) {
        log::info!(
            "remove: the level under the hole left as it is: its edge asks for gains of {:.3}, {:.3}, and {:.3}, {:.0}% of it within its texture of that, against {:.0}% and gains of {:.2} to {:.2}",
            mean[0].exp(),
            mean[1].exp(),
            mean[2].exp(),
            100.0 * agreeing,
            100.0 * AGREE.1,
            GAINS.0,
            GAINS.1
        );
        return (fill.clone(), None);
    }
    let kept = if level.abs() > RESOLVED * error {
        1.0 - (RESOLVED * error / level.abs()).powi(2)
    } else {
        0.0
    };
    if kept <= 0.0 {
        log::info!(
            "remove: the level under the hole left as it is: its edge asks for a gain of {:.3}, within {RESOLVED} times its error of {:.3} from none",
            level.exp(),
            error
        );
        return (fill.clone(), None);
    }
    log::debug!(
        "remove: the edge asks for a gain of {:.3}, its error {:.3}; {:.0}% of it taken",
        level.exp(),
        error,
        100.0 * kept
    );
    let gain: [f32; 3] = mean.map(|m| (m * kept).exp() as f32);
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

/// What the edge reads: the gain it asks for, in logs, per channel,
/// the luma of that, its standard error, the share of the edge that
/// agrees, and each sector's ask and sigma, for the log.
struct Reading {
    mean: [f64; 3],
    level: f64,
    error: f64,
    agreeing: f64,
    asks: Vec<[f64; 3]>,
    sigmas: Vec<f64>,
}

/// The edge of `hole` read across from `image` to `fill`; an error,
/// with why where it is worth a line in the log, where there is
/// nothing to read.
///
/// `origin` is where the window read sits in the one the hole was
/// drawn in, so the sectors and the surface's coordinates are the same
/// for any crop.
fn read(
    image: &Rgbf,
    hole: &[f32],
    fill: &Rgbf,
    origin: (usize, usize),
) -> Result<Reading, Option<String>> {
    let (w, h) = (image.width, image.height);
    if fill.width != w || fill.height != h || hole.len() != w * h {
        return Err(None);
    }
    if !hole.iter().any(|&m| m > 0.0) || depth(hole, w, h) < BAND as f32 {
        return Err(None);
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
        let (x, y) = (origin.0 + i % w, origin.1 + i / w);
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
            let at = place(origin.0 + i % w, origin.1 + i / w);
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
        .filter(|&s| {
            (0..2).all(|side| counts[side][s] > 0 && counts[side][s] as f64 >= SHARE * totals[side])
        })
        .collect();
    if counted.len() < COUNTED {
        return Err(Some(format!(
            "{} of its {SECTORS} sectors have band enough to compare",
            counted.len()
        )));
    }

    // One tilt over the whole band in linear light, a level of the
    // picture's in each sector, and a step on the fill's side of each,
    // by least squares. The tilt alone is shared: one plane over the
    // whole band, level and all, bends to meet a sector that crosses
    // a horizon or a dark hill, and the sectors at the hole's sides
    // then read their step against a plane halfway between the sky
    // and the hill. The fit is made [`PASSES`] times, each after the
    // first with every pixel weighted by its last residual (Huber), and
    // once more with each sector weighted by its texture, so such a
    // sector's own curve does not set the tilt either.
    let band: Vec<Pixel> = (0..w * h)
        .filter_map(|i| {
            let inside = side(i)?;
            let at = place(origin.0 + i % w, origin.1 + i / w);
            let from = if inside { fill } else { image };
            let z = std::array::from_fn(|k| f64::from(to_linear(from.data[i * 3 + k])));
            Some((inside, sector(at), at, z))
        })
        .collect();
    // Each counted sector's level and step are parameters `TILT +
    // place` and `TILT + counted + place`; a sector not counted is out
    // of the fit on both sides.
    let place_of: Vec<Option<usize>> = (0..SECTORS)
        .map(|s| counted.iter().position(|&c| c == s))
        .collect();
    let params = TILT + 2 * counted.len();
    let model = |x: &[Vec<f64>], k: usize, inside: bool, p: usize, (u, v): (f64, f64)| {
        let mut m = x[k][0] * u + x[k][1] * v + x[k][TILT + p];
        if inside {
            m += x[k][TILT + counted.len() + p];
        }
        m
    };
    let fit_with = |weights: &[f64]| -> Option<Vec<Vec<f64>>> {
        let mut normal = vec![0.0f64; params * params];
        let mut rhs = vec![[0.0f64; 3]; params];
        for (&(inside, s, (u, v), z), &weight) in band.iter().zip(weights) {
            let Some(p) = place_of[s] else {
                continue;
            };
            // The tilt, the sector's level, and on the fill's side its
            // step: the only terms a pixel has.
            let basis = [
                (0, u),
                (1, v),
                (TILT + p, 1.0),
                (TILT + counted.len() + p, 1.0),
            ];
            let basis = &basis[..if inside { 4 } else { 3 }];
            for &(a, ba) in basis {
                for &(b, bb) in basis {
                    normal[a * params + b] += weight * ba * bb;
                }
                for (r, z) in rhs[a].iter_mut().zip(z) {
                    *r += weight * ba * z;
                }
            }
        }
        (0..3)
            .map(|k| solve(normal.clone(), rhs.iter().map(|r| r[k]).collect(), params))
            .collect()
    };
    let mut weights = vec![1.0f64; band.len()];
    let mut fit: Option<Vec<Vec<f64>>> = None;
    for pass in 0..PASSES {
        fit = fit_with(&weights);
        let Some(x) = fit.as_ref() else {
            break;
        };
        if pass + 1 == PASSES {
            break;
        }
        // Each pixel's residual, the channels summed, against the
        // band's scale of them; past it a pixel weighs less.
        let residuals: Vec<f64> = band
            .iter()
            .map(|&(inside, s, at, z)| match place_of[s] {
                Some(p) => (0..3)
                    .map(|k| z[k] - model(x, k, inside, p, at))
                    .sum::<f64>()
                    .abs(),
                None => 0.0,
            })
            .collect();
        let mut sorted: Vec<f64> = residuals.iter().copied().filter(|r| *r > 0.0).collect();
        sorted.sort_by(f64::total_cmp);
        let Some(&mid) = sorted.get(sorted.len() / 2) else {
            break;
        };
        let scale = HUBER * 1.4826 * mid;
        for (weight, r) in weights.iter_mut().zip(&residuals) {
            *weight = if *r > scale { scale / r } else { 1.0 };
        }
    }
    let Some(x) = fit else {
        return Err(None);
    };

    // Each counted sector's sigma: [`TEXTURE`] and the spread of its
    // residuals from the fit, on the picture's side and the fill's, as
    // a share of its level, together. A sector over another surface (a
    // dark hill under a sky), or over grain and structure the fit does
    // not follow, reads its step no better than that.
    let pedestal = f64::from(PEDESTAL);
    let mut spread = [[0.0f64; 4]; SECTORS];
    for &(inside, s, at, z) in &band {
        let Some(p) = place_of[s] else {
            continue;
        };
        let r = (0..3)
            .map(|k| z[k] - model(&x, k, inside, p, at))
            .sum::<f64>()
            / 3.0;
        let o = 2 * usize::from(inside);
        spread[s][o] += 1.0;
        spread[s][o + 1] += r * r;
    }
    let sigmas: [f64; SECTORS] = std::array::from_fn(|s| {
        let Some(p) = place_of[s] else {
            return f64::INFINITY;
        };
        let n = counts[1][s] as f64;
        let at = (middles[s].0 / n, middles[s].1 / n);
        let texture: f64 = [false, true]
            .into_iter()
            .map(|inside| {
                let o = 2 * usize::from(inside);
                let level =
                    (0..3).map(|k| model(&x, k, inside, p, at)).sum::<f64>() / 3.0 + pedestal;
                // Each side has its own fitted level, so one degree
                // of freedom fewer than its count.
                spread[s][o + 1] / (spread[s][o] - 1.0).max(1.0) / level.max(pedestal).powi(2)
            })
            .sum();
        (TEXTURE * TEXTURE + texture).sqrt()
    });
    // The fit once more, each sector weighted by its sigma, so a sector
    // of another surface has that much less say in the shared tilt.
    let weights: Vec<f64> = weights
        .iter()
        .zip(&band)
        .map(|(w, b)| w / sigmas[b.1].powi(2))
        .collect();
    let Some(x) = fit_with(&weights) else {
        return Err(None);
    };

    // The step in each counted sector as a log gain about the pedestal,
    // where the sector's side of the fill lies; how far its texture
    // lets it stray (`sigma`); and its weight, its share of the edge
    // over that variance. NaN, and no step, where either side is not
    // above the pedestal's negative; such a sector still counts in the
    // edge, against the agreement.
    let mut total = 0.0;
    let mut sectors: Vec<Sector> = Vec::new();
    for (p, &s) in counted.iter().enumerate() {
        let n = counts[1][s] as f64;
        let at = (middles[s].0 / n, middles[s].1 / n);
        let level = |k: usize, inside: bool| model(&x, k, inside, p, at) + pedestal;
        let sigma = sigmas[s];
        let weight = n / (sigma * sigma);
        total += weight;
        let step: [f64; 3] = std::array::from_fn(|k| {
            let (picture, fill) = (level(k, false), level(k, true));
            if picture > 0.0 && fill > 0.0 {
                (picture / fill).ln()
            } else {
                f64::NAN
            }
        });
        if step.iter().all(|v| v.is_finite()) {
            sectors.push(Sector {
                step,
                sigma,
                weight,
            });
        }
    }
    let weight_sum = sectors.iter().map(|s| s.weight).sum::<f64>();
    if sectors.is_empty() || weight_sum <= 0.0 {
        return Err(None);
    }

    // The steps must agree round the hole: a sector agrees when its
    // step's luma, the three logs' mean, is within its texture's say
    // of the median's (one channel's grain in one sector of a textured
    // wall would otherwise turn the whole edge away). Each step is then
    // held to within that of the median, channel by channel, and they
    // are averaged by their weights, edge share over sigma²: a smooth
    // sector counts most, one of another surface or a busy texture
    // little, and one of the picture's own structure that the agreement
    // let through for at most its tolerance. The gates read luma by
    // design: an error of tint alone is left as it is, and each
    // channel's gain is shrunk with the luma's below.
    let luma = |s: &[f64; 3]| s.iter().sum::<f64>() / 3.0;
    let median_of = |f: &dyn Fn(&Sector) -> f64| {
        let mut v: Vec<(f64, f64)> = sectors.iter().map(|s| (f(s), s.weight)).collect();
        weighted_median(&mut v)
    };
    let median: [f64; 3] = std::array::from_fn(|k| median_of(&|s| s.step[k]));
    let middle = median_of(&|s| luma(&s.step));
    let agreeing: f64 = sectors
        .iter()
        .filter(|s| (luma(&s.step) - middle).abs() <= AGREE.0 * s.sigma)
        .map(|s| s.weight)
        .sum();
    let held = |s: &Sector, k: usize| {
        let reach = AGREE.0 * s.sigma;
        s.step[k].clamp(median[k] - reach, median[k] + reach)
    };
    let mean: [f64; 3] = std::array::from_fn(|k| {
        sectors.iter().map(|s| s.weight * held(s, k)).sum::<f64>() / weight_sum
    });

    // And their mean is shrunk toward no change by its own standard
    // error, read from how the held steps scatter about it: taken times
    // one less the square of [`RESOLVED`] errors over the level, all
    // three channels by the luma's share. On a texture the fill took up
    // out of phase, or a fill already at the picture's level under its
    // own grain, the steps agree within the texture's say and still
    // scatter round a mean of nothing, and a level read from them by
    // chance moves the fill by little. A level well clear of its
    // scatter is taken nearly whole.
    let lumas: Vec<f64> = sectors
        .iter()
        .map(|s| (0..3).map(|k| held(s, k)).sum::<f64>() / 3.0)
        .collect();
    let level = luma(&mean);
    let scatter = sectors
        .iter()
        .zip(&lumas)
        .map(|(s, l)| s.weight * (l - level).powi(2))
        .sum::<f64>()
        / weight_sum;
    let independent =
        weight_sum * weight_sum / sectors.iter().map(|s| s.weight.powi(2)).sum::<f64>();
    let error = (scatter / (independent - 1.0).max(1.0)).sqrt();

    Ok(Reading {
        mean,
        level,
        error,
        agreeing: agreeing / total,
        asks: sectors.iter().map(|s| s.step.map(f64::exp)).collect(),
        sigmas: sectors.iter().map(|s| s.sigma).collect(),
    })
}

/// A box of a window: the hole's box grown by a reach, held to the
/// window.
pub(super) struct Crop {
    pub(super) origin: (usize, usize),
    pub(super) width: usize,
    pub(super) height: usize,
    stride: usize,
}

impl Crop {
    /// The box round `hole`'s pixels (nonzero), grown by `reach`, in a
    /// `width` × `height` window; `None` for no hole.
    pub(super) fn round(hole: &[f32], width: usize, height: usize, reach: usize) -> Option<Crop> {
        let (mut x0, mut x1, mut y0, mut y1) = (width, 0, height, 0);
        for (i, _) in hole.iter().enumerate().filter(|(_, m)| **m > 0.0) {
            let (x, y) = (i % width, i / width);
            (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
        }
        if x0 > x1 {
            return None;
        }
        let (x0, y0) = (x0.saturating_sub(reach), y0.saturating_sub(reach));
        let (x1, y1) = ((x1 + reach).min(width - 1), (y1 + reach).min(height - 1));
        Some(Crop {
            origin: (x0, y0),
            width: x1 - x0 + 1,
            height: y1 - y0 + 1,
            stride: width,
        })
    }

    /// The window's index of the box's pixel `i`.
    pub(super) fn at(&self, i: usize) -> usize {
        (self.origin.1 + i / self.width) * self.stride + self.origin.0 + i % self.width
    }

    pub(super) fn mask(&self, m: &[f32]) -> Vec<f32> {
        (0..self.width * self.height)
            .map(|i| m[self.at(i)])
            .collect()
    }

    fn rgb(&self, image: &Rgbf) -> Rgbf {
        let data = (0..self.width * self.height)
            .flat_map(|i| {
                let j = self.at(i) * 3;
                [image.data[j], image.data[j + 1], image.data[j + 2]]
            })
            .collect();
        Rgbf::new(self.width, self.height, data)
    }
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
pub(super) fn solve(mut m: Vec<f64>, mut r: Vec<f64>, n: usize) -> Option<Vec<f64>> {
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

    /// A grain of `amount` (its standard deviation, relative) from a
    /// hash of `i` and `seed`.
    fn grain_of(i: usize, seed: usize, amount: f32) -> f32 {
        let u = grain(i.wrapping_add(seed.wrapping_mul(0x51_7cc1_b727)));
        // `grain`'s spread is uniform, 0.07 wide; rescaled to `amount`.
        1.0 + (u - 1.0) / 0.07 * amount * 12f32.sqrt()
    }

    /// A gradient with `amount` of grain of its own, and a hole of
    /// radius 40, the fill the gradient times `light` under the model's
    /// own grain.
    fn grainy(seed: usize, amount: f32, light: f32) -> (Rgbf, Vec<f32>, Rgbf) {
        let truth = picture(|x, y| 0.1 + 0.3 * x / W as f32 + 0.05 * y / H as f32);
        let truth = fill_of(&truth, &vec![1.0; W * H], |v, i| {
            v * grain_of(i, seed, amount)
        });
        let hole = round(40.0);
        let fill = fill_of(&truth, &hole, |v, i| {
            v * light * grain_of(i * 31 + 7, seed, amount) / grain_of(i, seed, amount)
        });
        (truth, hole, fill)
    }

    #[test]
    fn a_fill_at_the_pictures_level_under_grain_moves_little() {
        // The model's own grain, not the picture's, over many draws:
        // the sectors scatter round a level of nothing, and when they
        // happen to read one near their error, the fill takes little
        // of it.
        let mut worst_mean = 0.0f32;
        for seed in 0..60 {
            let amount = [0.02, 0.05, 0.1][seed % 3];
            let (truth, hole, fill) = grainy(seed, amount, 1.0);
            let (out, _) = matched(&truth, &hole, &fill);
            let (mean, _) = change(&out, &fill, within(40.0));
            worst_mean = worst_mean.max(mean);
        }
        // A sample maximum: 0.69% over 300 draws like these; more
        // draws or more grain raise it.
        assert!(worst_mean < 0.008, "{worst_mean}");
        // Without the grain, nothing to read wrong.
        let plain = picture(|x, y| 0.1 + 0.3 * x / W as f32 + 0.05 * y / H as f32);
        let hole = round(40.0);
        let (out, gains) = matched(&plain, &hole, &plain);
        assert!(gains.is_none(), "{gains:?}");
        assert_eq!(out, plain);
    }

    #[test]
    fn a_small_level_under_grain_is_taken() {
        // What a tester sees: a fill three to eight percent light under
        // grain of two to ten percent.
        let mut worst = 0.0f32;
        let mut alone = 0;
        for seed in 0..60 {
            let light = [1.03, 1.05, 1.08][seed % 3];
            let amount = [0.02, 0.05, 0.1][(seed / 3) % 3];
            let (truth, hole, fill) = grainy(seed, amount, light);
            let (_, gains) = matched(&truth, &hole, &fill);
            match gains {
                None => alone += 1,
                Some(g) => {
                    for g in g.gain {
                        worst = worst.max((g * light - 1.0).abs());
                    }
                }
            }
        }
        // Measured: all taken, within 1.04% of the picture.
        assert_eq!(alone, 0);
        assert!(worst < 0.02, "{worst}");
    }

    #[test]
    fn a_fill_with_its_own_structure_keeps_it() {
        // Light and dark patches, the fill's the picture's: what the
        // model drew inside is not pulled to a guess from outside.
        // Through every phase of the patches' period: a phase at which
        // they differ between the band's two radii all round the ring
        // reads as a level, and the picture's own reading across the
        // pseudo-edges outside says it is the texture's.
        let hole = round(40.0);
        for dx in 0..63 {
            let patches = move |x: f32, y: f32| {
                0.2 + 0.15 * ((x + dx as f32) / 10.0).sin() * (y / 12.0).sin()
            };
            let truth = picture(patches);
            let (out, _) = matched(&truth, &hole, &truth);
            let most = worst(&out, &truth, within(40.0));
            assert!(most < 0.002, "{dx}: {most}");
        }
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
        // Every shift of the period: the shifts near a whole period
        // read a level of about a tenth, with neighboring sectors
        // reading it together, which the picture's own reading outside
        // shows to be the texture's: at most a few shifts are moved,
        // measured 0.6% at most.
        for dx in 0..63 {
            let shifted = picture(patches(dx as f32));
            let fill = fill_of(&truth, &hole, |_, i| to_linear(shifted.data[i * 3]));
            let (out, _) = matched(&truth, &hole, &fill);
            let changed = worst(&out, &fill, within(40.0));
            assert!(changed < 0.01, "{dx}: {changed}");
        }
    }

    #[test]
    fn a_sky_over_a_dark_hill_takes_the_skys_level() {
        // A hole whose bottom crosses a dark, rough hillside under a
        // plain sky: the fill a twentieth light on the sky and its hill
        // drawn lighter and out of phase. The hill's sectors are
        // another surface and read rough; the sky's decide.
        let hill = |x: f32, y: f32| y > 105.0 + 0.3 * (x - 120.0);
        let rough =
            |x: f32, y: f32, dx: f32| 0.04 * (1.0 + 0.6 * ((x + dx) / 3.0).sin() * (y / 4.0).sin());
        let truth = picture(|x, y| {
            if hill(x, y) {
                rough(x, y, 0.0)
            } else {
                0.3 + 0.02 * x / W as f32
            }
        });
        let hole = round(40.0);
        let fill = fill_of(&truth, &hole, |v, i| {
            let (x, y) = ((i % W) as f32, (i / W) as f32);
            if hill(x, y) {
                1.4 * rough(x, y, 5.0)
            } else {
                v * 1.05
            }
        });
        let (out, gains) = matched(&truth, &hole, &fill);
        let gains = gains.expect("the sky's sectors agree");
        for g in gains.gain {
            assert!((g - 1.0 / 1.05).abs() < 0.01, "{gains:?}");
        }
        let sky = |x: usize, y: usize| within(36.0)(x, y) && !hill(x as f32, y as f32);
        let off = worst(&out, &truth, sky);
        assert!(off < 0.015, "{off}");
    }

    #[test]
    fn two_levels_near_each_other_are_averaged_and_far_apart_left_alone() {
        // The fill light on its left half and at the picture's level on
        // its right. Within a smooth sector's reach (0.1 in logs) the
        // halves agree and the fill takes a gain between them; a
        // seventh apart, no one level is the fill's.
        let truth = picture(|x, y| 0.1 + 0.3 * x / W as f32 + 0.05 * y / H as f32);
        let hole = round(40.0);
        for light in [1.03f32, 1.05, 1.08] {
            let fill = fill_of(
                &truth,
                &hole,
                |v, i| if i % W < 120 { v * light } else { v },
            );
            let (_, gains) = matched(&truth, &hole, &fill);
            let gains = gains.expect("near levels agree");
            for g in gains.gain {
                assert!(g > 1.0 / light && g < 1.0, "{light}: {g}");
            }
        }
        let fill = fill_of(&truth, &hole, |v, i| if i % W < 120 { v * 1.14 } else { v });
        let (out, gains) = matched(&truth, &hole, &fill);
        assert!(gains.is_none(), "{gains:?}");
        assert_eq!(out, fill);
    }

    #[test]
    fn known_gap_a_long_thin_stroke_is_never_matched() {
        // Sectors in the hole's principal-axis frame, or by position
        // along the edge, would change this: today the angular sectors
        // about the middle leave half of them too little band.
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
    fn known_gap_a_finer_pattern_can_still_move_a_correct_fill() {
        // Patches of a 31 px period, a perfect fill, every phase: the
        // pseudo-edges sample 8 to 16 px out, a quarter of that period,
        // so the bias at the true edge's phase can fall between them,
        // and the reads' reach of 32 px keeps them from going further:
        // a correct fill can still move by about a tenth.
        let hole = round(40.0);
        let mut most = 0.0f32;
        for dx in 0..31 {
            let patches =
                move |x: f32, y: f32| 0.2 + 0.15 * ((x + dx as f32) / 5.0).sin() * (y / 6.0).sin();
            let truth = picture(patches);
            let (out, _) = matched(&truth, &hole, &truth);
            most = most.max(worst(&out, &truth, within(40.0)));
        }
        // Measured 12.2% at the worst phase.
        assert!(most > 0.02 && most < 0.15, "{most}");
    }

    #[test]
    fn a_window_past_the_reads_reach_does_not_change_the_match() {
        // The same scene in a window grown right and down, with other
        // content past the reads' reach: the reads crop to the hole's
        // box and their reach, so the gains come out to the bit.
        let scene = |x: f32, y: f32| 0.2 * (1.0 + 0.25 * (x / 10.0).sin() * (y / 12.0).sin());
        let hole = round(40.0);
        let truth = picture(scene);
        let light = fill_of(&truth, &hole, |v, _| v * 1.08);
        let (small, gains) = matched(&truth, &hole, &light);
        let (bw, bh) = (W + 300, H + 200);
        let grow = |img: &Rgbf| {
            let mut data = Vec::with_capacity(bw * bh * 3);
            for y in 0..bh {
                for x in 0..bw {
                    if x < W && y < H {
                        data.extend(img.at(x, y));
                    } else {
                        data.extend([0.9, 0.1, 0.5]);
                    }
                }
            }
            Rgbf::new(bw, bh, data)
        };
        let big_hole: Vec<f32> = (0..bw * bh)
            .map(|i| {
                let (x, y) = (i % bw, i / bw);
                if x < W && y < H { hole[y * W + x] } else { 0.0 }
            })
            .collect();
        let (big, big_gains) = matched(&grow(&truth), &big_hole, &grow(&light));
        assert!(gains.is_some());
        assert_eq!(gains, big_gains);
        assert_eq!(grow(&small), big);
    }

    #[test]
    fn a_blotchy_wall_takes_a_true_level() {
        // A uniform error on blotches about the hole's size: each
        // sector's level is its own, and each may stray as far as its
        // band's texture, so the edge agrees on the error, though the
        // blotches' phase across the gap still takes a part of it.
        let blotches = |dx: f32| {
            move |x: f32, y: f32| 0.2 * (1.0 + 0.25 * ((x + dx) / 10.0).sin() * (y / 12.0).sin())
        };
        let hole = round(40.0);
        // A fill at the wall's level is moved by little at any phase:
        // the picture's own reading outside, taken as an RMS over three
        // pseudo-edges, can fall a little short of the bias at the
        // edge, and the fill then takes part of it (measured 1.35% at
        // most).
        for dx in 0..63 {
            let truth = picture(blotches(dx as f32));
            let (out, _) = matched(&truth, &hole, &truth);
            let most = worst(&out, &truth, within(40.0));
            assert!(most < 0.02, "{dx}: {most}");
        }
        let truth = picture(blotches(0.0));
        let light = fill_of(&truth, &hole, |v, _| v * 1.14);
        let (out, gains) = matched(&truth, &hole, &light);
        let gains = gains.expect("the edge agrees");
        // How near the gain comes depends on the blotches' phase: at
        // this one it reads 0.862 against 0.877, with 76% of the edge
        // agreeing against the 75% it needs. Across the 63 phases the
        // fill is left alone at 38, where the picture's own reading
        // outside is as large as the ask, and taken within 6.4% at the
        // rest.
        for g in gains.gain {
            assert!((g - 1.0 / 1.14).abs() < 0.02, "{g}");
        }
        let (before, _) = change(&light, &truth, within(40.0));
        let (after, _) = change(&out, &truth, within(40.0));
        assert!(after < before / 2.0, "{after} against {before}");
    }
}
