//! Lateral chromatic aberration correction on the X-Trans mosaic.
//!
//! RawTherapee's automatic correction, which [`super::ca`] ports, does
//! not run on X-Trans: `RawImageSource::preprocess`
//! (`rtengine/rawimagesource.cc`) calls `CA_correct_RT` for a Bayer
//! sensor only, under the comment "Auto CA correction disabled for
//! X-Trans, for now...", and RawPedia sends an X-Trans user to the
//! manual correction after the demosaic, which this engine has as the
//! lens correction's red and blue scale. darktable's port of the same
//! code (`src/iop/cacorrect.c`) is Bayer only too. darktable's answer
//! for X-Trans is another algorithm, its "chromatic aberrations" module
//! (`src/iop/cacorrectrgb.c`), which runs on the demosaiced picture and
//! pulls red and blue toward green's local detail through two guided
//! manifolds: it treats axial aberration and blur as much as a shift,
//! moves color everywhere a little, and darktable leaves it off by
//! default. The engine runs the lateral correction by default on Bayer,
//! and the same correction on both mosaics is what a user can reason
//! about, so this carries RawTherapee's to X-Trans rather than port the
//! other.
//!
//! Built on RawTherapee's `rtengine/CA_correct_RT.cc`, GPL-3.0-or-later:
//! copyright (c) 2008-2010 Emil Martinec <ejmartin@uchicago.edu>;
//! copyright (c) 2018 Ingo Weyrich for the iterated correction and the
//! color-shift avoidance. Theirs, through [`super::ca`] unchanged: the
//! tiles, each tile's vote for a shift of red and of blue against green,
//! the median and the gate over the votes, the polynomial fit of the
//! shift over the frame, the iterations, the guards of the resample
//! (take a correction only where it lessens the color difference, weigh
//! the color differences by how like the green is where the plain mean
//! strays, desaturate an overshoot), and the color-shift avoidance's
//! factor and its wide blur.
//!
//! Ours is how X-Trans reaches them:
//!
//! - **The votes** are cast on a Bayer mosaic made of the X-Trans one:
//!   at every site the color an RGGB pattern puts there, the X-Trans
//!   sample where the colors match and an estimate where they do not.
//!   The estimates are exact on a plane ([`super::sites`]): green at a
//!   red or blue site from its five greens, red and blue from their
//!   sites in the 3x3, or the 5x5 where the 3x3 holds two. A plain mean
//!   of a site's X-Trans neighbors reads the picture a fraction of a
//!   pixel away (a fifth, for green), in a direction that depends on
//!   the site, and the
//!   stand-in mosaic samples the 6x6 period at only some of its phases,
//!   so those offsets would not cancel: they would be measured as
//!   aberration.
//! - **The resample** works on the X-Trans sites. The reference takes,
//!   at each red or blue site, green at the point the fitted shift says
//!   the site's light came from, the color difference there, and
//!   interpolates those differences back to the site from its
//!   neighbors of the same color two photosites away, which on Bayer
//!   sit on a lattice. Here they sit where the pattern puts them, so
//!   the differences are read with a tent of radius two over the
//!   site's color round the point, green is the plane-exact estimate,
//!   and the guard's weighted mean is the reference's weights times the
//!   tent. The point is the site moved back by the whole shift; the
//!   reference moves it by the shift's fractional part, which is the
//!   same for the shifts under a pixel that nearly all frames have.
//!   Each site's color difference is taken once, by its own tile's
//!   shift; the reference's tile buffers take a site near a seam by the
//!   shift of the tile being corrected, so the two part by the
//!   difference between neighboring tiles' shifts there.
//! - **The color-shift avoidance** gathers its factors on a grid of 3x3
//!   cells, the X-Trans counterpart of the reference's 2x2 (every 3x3
//!   holds red and blue), each cell's factor the mean of its sites',
//!   and its blur's sigma is two thirds of the reference's, so that it
//!   spans the same photosites.
//!
//! The GPU's correction is Bayer only: an X-Trans frame runs this one,
//! on the CPU, in the viewport and the export alike.

use rayon::prelude::*;

use super::ca::{
    CaOptions, CaStats, EPS, Fit, MIN_SIDE, SHIFT_BLUR_SIGMA, STEP, fit_votes,
    gaussian_blur_approx, measure_votes, origins, shift_factor,
};
use super::sites::{Phases, Tap, interpolation};
use crate::error::{Error, Result};
use crate::raw::CfaPattern;

/// Correct lateral CA in a white-balanced X-Trans mosaic. Returns the
/// corrected mosaic (green untouched) and what was measured, as
/// [`super::ca::correct_ca`] does for Bayer.
pub fn correct_ca(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    options: &CaOptions,
) -> Result<(Vec<f32>, CaStats)> {
    if !pattern.is_xtrans() {
        return Err(Error::Unsupported(format!(
            "X-Trans CA correction needs an X-Trans pattern, got {pattern}"
        )));
    }
    if samples.len() != width * height {
        return Err(Error::Unsupported(format!(
            "{width}x{height} mosaic with {} samples",
            samples.len()
        )));
    }
    let mut current = samples.to_vec();
    let mut stats = CaStats::default();
    if width < MIN_SIDE || height < MIN_SIDE {
        return Ok((current, stats));
    }
    let table = interpolation(pattern);
    let colors = Colors::of(pattern);
    let bayer = CfaPattern::rggb();
    let (down, across) = (origins(height).len(), origins(width).len());
    for it in 0..options.iterations.max(1) {
        let (green, standin) = planes(&current, width, height, pattern, &table);
        let votes = measure_votes(&standin, width, height, &bayer)?;
        drop(standin);
        let fit = match fit_votes(&votes, down, across) {
            Ok(fit) => fit,
            Err(no) => {
                stats.corrected = false;
                stats.blocks = no.blocks();
                stats.order = 0;
                break;
            }
        };
        if it == 0 {
            stats.max_shift = fit.max_shift;
        }
        stats.corrected = true;
        stats.blocks = fit.blocks;
        stats.order = fit.order;
        current = resample(&current, &green, width, height, &colors, &fit);
    }
    if options.avoid_color_shift && stats.corrected {
        avoid_color_shift(samples, &mut current, width, height, pattern);
    }
    Ok((current, stats))
}

/// The pattern's colors as RGB indices, read with the period of six
/// known to the compiler: the resample asks a site's neighborhood for
/// its colors sixteen times a site.
struct Colors([[u8; 6]; 6]);

impl Colors {
    fn of(pattern: &CfaPattern) -> Self {
        Self(std::array::from_fn(|y| {
            std::array::from_fn(|x| {
                pattern.color_at(y, x).rgb_index().expect("X-Trans is RGB") as u8
            })
        }))
    }

    #[inline]
    fn at(&self, y: usize, x: usize) -> u8 {
        self.0[y % 6][x % 6]
    }
}

/// Color `c` at (`y`, `x`) by `taps`, its taps in the table, or by the
/// bilinear demosaic's mean within two sites of an edge, where the taps
/// would read past it.
#[inline]
fn estimate(
    samples: &[f32],
    (width, height): (usize, usize),
    pattern: &CfaPattern,
    taps: &[Tap],
    (y, x): (usize, usize),
    c: usize,
) -> f32 {
    if y < 2 || x < 2 || y + 2 >= height || x + 2 >= width {
        return super::bilinear_pixel(samples, width, height, pattern, y, x)[c];
    }
    taps.iter()
        .map(|&(dy, dx, w)| {
            w * samples[(y as isize + dy) as usize * width + (x as isize + dx) as usize]
        })
        .sum()
}

/// Green at every site, and the stand-in Bayer mosaic (RGGB from the
/// origin) the votes are cast on.
fn planes(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    table: &Phases<[Vec<Tap>; 3]>,
) -> (Vec<f32>, Vec<f32>) {
    let size = (width, height);
    let mut green = vec![0.0f32; width * height];
    green
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            let taps = table.row(y).iter().cycle();
            for (x, (g, taps)) in row.iter_mut().zip(taps).enumerate() {
                *g = estimate(samples, size, pattern, &taps[1], (y, x), 1);
            }
        });
    let bayer = CfaPattern::rggb();
    let mut standin = vec![0.0f32; width * height];
    standin
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            let taps = table.row(y).iter().cycle();
            for (x, (s, taps)) in row.iter_mut().zip(taps).enumerate() {
                let c = bayer.color_at(y, x).rgb_index().expect("RGGB is RGB");
                *s = if c == 1 {
                    green[y * width + x]
                } else {
                    estimate(samples, size, pattern, &taps[c], (y, x), c)
                };
            }
        });
    (green, standin)
}

/// Green at a point between sites, bilinear on the green plane, the
/// point held inside the frame.
#[inline]
fn green_at(green: &[f32], width: usize, height: usize, y: f32, x: f32) -> f32 {
    let y = y.clamp(0.0, (height - 1) as f32);
    let x = x.clamp(0.0, (width - 1) as f32);
    let (y0, x0) = (y.floor() as usize, x.floor() as usize);
    let (y1, x1) = ((y0 + 1).min(height - 1), (x0 + 1).min(width - 1));
    let (fy, fx) = (y - y0 as f32, x - x0 as f32);
    let g = |yy: usize, xx: usize| green[yy * width + xx];
    let top = g(y0, x0) + fx * (g(y0, x1) - g(y0, x0));
    let bottom = g(y1, x0) + fx * (g(y1, x1) - g(y1, x0));
    top + fy * (bottom - top)
}

/// One pass's resample of red and blue by the shifts `fit` gives each
/// tile, kept at or above zero; green as it was.
fn resample(
    samples: &[f32],
    green: &[f32],
    width: usize,
    height: usize,
    colors: &Colors,
    fit: &Fit,
) -> Vec<f32> {
    // The shift of each tile's interior, `[color][direction]`: a row's
    // block is its index over the interior plus one, as the fit counts
    // them.
    let (down, across) = (origins(height).len(), origins(width).len());
    let shifts: Vec<[[f32; 2]; 2]> = (1..=down)
        .flat_map(|v| (1..=across).map(move |h| (v, h)))
        .map(|(v, h)| fit.shift_at(v, h).map(|c| c.map(|s| s as f32)))
        .collect();
    let shift_of = |y: usize, x: usize, c: usize| shifts[(y / STEP) * across + x / STEP][c];
    // Each red and blue site's green at the point its light came from,
    // by its tile's shift, and the color difference there: read by the
    // resample of every site round it, made once.
    let mut shifted = vec![(0.0f32, 0.0f32); width * height];
    shifted
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate() {
                let c = match colors.at(y, x) {
                    0 => 0,
                    2 => 1,
                    _ => continue,
                };
                let [sv, sh] = shift_of(y, x, c);
                let gs = green_at(green, width, height, y as f32 + sv, x as f32 + sh);
                *out = (gs, gs - samples[y * width + x]);
            }
        });
    let mut out = samples.to_vec();
    out.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for (x, value) in row.iter_mut().enumerate() {
            let (c, color) = match colors.at(y, x) {
                0 => (0, 0),
                2 => (1, 2),
                _ => continue,
            };
            let site = Site {
                y,
                x,
                color,
                shift: shift_of(y, x, c),
            };
            *value = site
                .resample(samples, green, &shifted, (width, height), colors)
                .max(0.0);
        }
    });
    out
}

/// A red or blue site, its color's RGB index and its tile's shift
/// (vertical, horizontal).
struct Site {
    y: usize,
    x: usize,
    color: u8,
    shift: [f32; 2],
}

impl Site {
    /// The site's value after the resample, before it is kept at or
    /// above zero: the reference's guards on color differences read
    /// from the site's color round the point its light came from.
    fn resample(
        &self,
        samples: &[f32],
        green: &[f32],
        shifted: &[(f32, f32)],
        (width, height): (usize, usize),
        colors: &Colors,
    ) -> f32 {
        let i = self.y * width + self.x;
        let [sv, sh] = self.shift;
        let g0 = green[i];
        let cur = samples[i];
        let old = g0 - cur;
        // The point this site's light came from; each site of its color
        // round it gives the color difference at its own such point.
        let py = (self.y as f32 - sv).clamp(0.0, (height - 1) as f32);
        let px = (self.x as f32 - sh).clamp(0.0, (width - 1) as f32);
        let (y0, x0) = (py.floor() as isize - 1, px.floor() as isize - 1);
        // (tent weight, color difference, green there) of each.
        let mut taps = [(0.0f32, 0.0f32, 0.0f32); 16];
        let mut n = 0;
        for jy in y0..y0 + 4 {
            for jx in x0..x0 + 4 {
                if jy < 0 || jx < 0 || jy >= height as isize || jx >= width as isize {
                    continue;
                }
                let (jy, jx) = (jy as usize, jx as usize);
                if colors.at(jy, jx) != self.color {
                    continue;
                }
                let tent = (1.0 - (jy as f32 - py).abs() * 0.5).max(0.0)
                    * (1.0 - (jx as f32 - px).abs() * 0.5).max(0.0);
                if tent <= 0.0 {
                    continue;
                }
                let (gs, d) = shifted[jy * width + jx];
                taps[n] = (tent, d, gs);
                n += 1;
            }
        }
        let taps = &taps[..n];
        let total: f32 = taps.iter().map(|t| t.0).sum();
        if total <= 0.0 {
            return cur;
        }
        let mut int = taps.iter().map(|t| t.0 * t.1).sum::<f32>() / total;
        let rbint = g0 - int;
        let mut value = cur;
        if (rbint - cur).abs() < 0.25 * (rbint + cur) {
            if old.abs() > int.abs() {
                value = rbint;
            }
        } else {
            // Weight by how far green at the shifted points is from
            // green here.
            let (mut num, mut den) = (0.0f32, 0.0f32);
            for &(tent, d, gs) in taps {
                let p = tent / (EPS + (g0 - gs).abs());
                num += p * d;
                den += p;
            }
            int = num / den;
            if old.abs() > int.abs() {
                value = g0 - int;
            }
        }
        // Overshot the correction: desaturate instead.
        if old * int < 0.0 {
            value = g0 - 0.5 * (old + int);
        }
        value
    }
}

/// Keep the local average of each of red and blue what it was: the
/// ratio of old to new at every red and blue site, gathered on 3x3
/// cells, blurred widely, multiplies the corrected value. What
/// [`correct_ca`] does after its passes when asked to avoid a color
/// shift.
pub fn avoid_color_shift(
    original: &[f32],
    corrected: &mut [f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
) {
    let colors = Colors::of(pattern);
    let (cw, ch) = (width.div_ceil(3), height.div_ceil(3));
    let mut factor = [vec![1.0f32; cw * ch], vec![1.0f32; cw * ch]];
    // A factor row covers three mosaic rows; rows in parallel.
    let [red, blue] = &mut factor;
    red.par_chunks_mut(cw)
        .zip(blue.par_chunks_mut(cw))
        .enumerate()
        .for_each(|(fy, (fr, fb))| {
            let mut sum = [vec![0.0f32; cw], vec![0.0f32; cw]];
            let mut count = [vec![0u32; cw], vec![0u32; cw]];
            for y in (3 * fy)..(3 * fy + 3).min(height) {
                for x in 0..width {
                    let c = match colors.at(y, x) {
                        0 => 0,
                        2 => 1,
                        _ => continue,
                    };
                    let i = y * width + x;
                    sum[c][x / 3] += shift_factor(original[i], corrected[i]);
                    count[c][x / 3] += 1;
                }
            }
            for (c, f) in [fr, fb].into_iter().enumerate() {
                for (k, v) in f.iter_mut().enumerate() {
                    if count[c][k] > 0 {
                        *v = sum[c][k] / count[c][k] as f32;
                    }
                }
            }
        });
    for f in factor.iter_mut() {
        gaussian_blur_approx(f, cw, ch, SHIFT_BLUR_SIGMA * 2.0 / 3.0);
    }
    corrected
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, v) in row.iter_mut().enumerate() {
                let c = match colors.at(y, x) {
                    0 => 0,
                    2 => 1,
                    _ => continue,
                };
                *v *= factor[c][(y / 3) * cw + x / 3];
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::ca::tests::{sample, texture};
    use crate::raw::CfaColor;

    /// Mosaic a grey texture on `pattern` with lateral CA: red
    /// magnified about the center by `1 + red`, blue by `1 + blue`.
    fn aberrated(
        width: usize,
        height: usize,
        pattern: &CfaPattern,
        red: f32,
        blue: f32,
    ) -> (Vec<f32>, Vec<f32>) {
        let tex = texture(width, height);
        let (cx, cy) = ((width - 1) as f32 / 2.0, (height - 1) as f32 / 2.0);
        let mut shifted = vec![0.0f32; width * height];
        for y in 0..height {
            for x in 0..width {
                let scale = match pattern.color_at(y, x) {
                    CfaColor::Red => 1.0 + red,
                    CfaColor::Blue => 1.0 + blue,
                    _ => 1.0,
                };
                let sx = cx + (x as f32 - cx) / scale;
                let sy = cy + (y as f32 - cy) / scale;
                shifted[y * width + x] = sample(&tex, width, height, sx, sy);
            }
        }
        (tex, shifted)
    }

    fn rms_error(
        a: &[f32],
        b: &[f32],
        width: usize,
        height: usize,
        pattern: &CfaPattern,
        color: CfaColor,
    ) -> f32 {
        let (mut sum, mut n) = (0.0f64, 0usize);
        for y in 32..height - 32 {
            for x in 32..width - 32 {
                if pattern.color_at(y, x) != color {
                    continue;
                }
                let d = f64::from(a[y * width + x] - b[y * width + x]);
                sum += d * d;
                n += 1;
            }
        }
        (sum / n as f64).sqrt() as f32
    }

    const PHASES: [(usize, usize); 3] = [(0, 0), (2, 2), (5, 1)];

    #[test]
    fn an_unaberrated_mosaic_is_left_nearly_alone() {
        let (w, h) = (400, 320);
        for (dx, dy) in PHASES {
            let p = CfaPattern::xtrans().shifted(dx, dy);
            let (clean, _) = aberrated(w, h, &p, 0.0, 0.0);
            let (out, stats) = correct_ca(&clean, w, h, &p, &CaOptions::default()).unwrap();
            assert!(
                stats.max_shift[0] < 0.15 && stats.max_shift[1] < 0.15,
                "({dx}, {dy}): {stats:?}"
            );
            for color in [CfaColor::Red, CfaColor::Blue] {
                let err = rms_error(&clean, &out, w, h, &p, color);
                assert!(err < 2e-3, "({dx}, {dy}) {color:?}: {err} {stats:?}");
            }
        }
    }

    #[test]
    fn radial_aberration_is_measured_and_removed() {
        // 0.5% magnification of red and -0.4% of blue: about 1.3 and 1.0
        // pixels of displacement at the corners of a 400x320 frame.
        let (w, h) = (400, 320);
        for (dx, dy) in PHASES {
            let p = CfaPattern::xtrans().shifted(dx, dy);
            let (clean, shifted) = aberrated(w, h, &p, 0.005, -0.004);
            let (out, stats) = correct_ca(&shifted, w, h, &p, &CaOptions::default()).unwrap();
            assert!(stats.corrected, "({dx}, {dy}): {stats:?}");
            assert!(
                stats.max_shift[0] > 0.4 && stats.max_shift[0] < 2.0,
                "({dx}, {dy}): {stats:?}"
            );
            for color in [CfaColor::Red, CfaColor::Blue] {
                let before = rms_error(&clean, &shifted, w, h, &p, color);
                let after = rms_error(&clean, &out, w, h, &p, color);
                assert!(
                    after < 0.5 * before,
                    "({dx}, {dy}) {color:?}: {before} -> {after} {stats:?}"
                );
            }
            for (i, (a, b)) in out.iter().zip(&shifted).enumerate() {
                if p.color_at(i / w, i % w) == CfaColor::Green {
                    assert_eq!(a, b, "green at {i} untouched");
                }
            }
        }
    }

    /// The stand-in Bayer mosaic of a plane is the plane: every
    /// estimate is exact on one, away from the edges, so the votes see
    /// no shift that the X-Trans layout made.
    #[test]
    fn the_stand_in_of_a_plane_is_the_plane() {
        let (w, h) = (48, 40);
        let plane = |y: usize, x: usize| 0.2 + 0.004 * y as f32 + 0.003 * x as f32;
        for (dx, dy) in PHASES {
            let p = CfaPattern::xtrans().shifted(dx, dy);
            // A grey plane with each color at its own level.
            let level = [0.8f32, 1.0, 0.6];
            let samples: Vec<f32> = (0..w * h)
                .map(|i| {
                    let c = p.color_at(i / w, i % w).rgb_index().unwrap();
                    level[c] * plane(i / w, i % w)
                })
                .collect();
            let (green, standin) = planes(&samples, w, h, &p, &interpolation(&p));
            let bayer = CfaPattern::rggb();
            for y in 2..h - 2 {
                for x in 2..w - 2 {
                    let c = bayer.color_at(y, x).rgb_index().unwrap();
                    let want = level[c] * plane(y, x);
                    let got = standin[y * w + x];
                    assert!((got - want).abs() < 1e-5, "({y}, {x}): {got} vs {want}");
                    let g = green[y * w + x];
                    assert!((g - plane(y, x)).abs() < 1e-5, "green ({y}, {x})");
                }
            }
        }
    }

    /// The avoidance puts each color's local average back: a red and a
    /// blue the correction moved uniformly come back to their level,
    /// and green is not touched.
    #[test]
    fn the_color_shift_avoidance_restores_the_local_average() {
        let (w, h) = (90, 75);
        let p = CfaPattern::xtrans().shifted(1, 3);
        let level = [0.4f32, 0.5, 0.3];
        let original: Vec<f32> = (0..w * h)
            .map(|i| level[p.color_at(i / w, i % w).rgb_index().unwrap()])
            .collect();
        let moved = [1.25f32, 1.0, 0.8];
        let mut corrected: Vec<f32> = (0..w * h)
            .map(|i| {
                let c = p.color_at(i / w, i % w).rgb_index().unwrap();
                level[c] * moved[c]
            })
            .collect();
        avoid_color_shift(&original, &mut corrected, w, h, &p);
        for (i, (a, b)) in corrected.iter().zip(&original).enumerate() {
            assert!((a - b).abs() < 1e-4, "{i}: {a} vs {b}");
        }
    }

    #[test]
    fn refuses_bayer_and_skips_small_images() {
        let p = CfaPattern::xtrans();
        assert!(correct_ca(&[0.0; 16], 4, 4, &CfaPattern::rggb(), &CaOptions::default()).is_err());
        assert!(correct_ca(&[0.0; 16], 5, 3, &p, &CaOptions::default()).is_err());
        let small = vec![0.5f32; 200 * 300];
        let (out, stats) = correct_ca(&small, 300, 200, &p, &CaOptions::default()).unwrap();
        assert_eq!(out, small);
        assert!(!stats.corrected);
        // Through the engine's entry point, too.
        let (out, _) =
            crate::develop::ca::correct_ca(&small, 300, 200, &p, &CaOptions::default()).unwrap();
        assert_eq!(out, small);
    }
}
