//! Ratio Corrected Demosaicing (RCD).
//!
//! Ported from Luis Sanz Rodríguez's reference implementation, release 2.3
//! of 2017-11-25, `rcd_demosaicing.c` at
//! <https://github.com/LuisSR/RCD-Demosaicing>, GPL-3.0-or-later,
//! copyright Luis Sanz Rodríguez. The algorithm, its statistics and its
//! constants are his. The buffer layout, the border handling and the
//! threading are ours.
//!
//! Four passes over a 2x2 Bayer mosaic:
//!
//! 1. A directional statistic at every pixel from nine samples along the
//!    vertical and the horizontal, combined into one number in 0..1 that
//!    says how much the horizontal estimate should count.
//! 2. A 3x3 binomial low-pass filter at the red and blue sites.
//! 3. Green at red and blue sites. Each cardinal neighbor's green is
//!    scaled by the ratio of the low-pass values two pixels apart (the
//!    "ratio correction" that replaces Hamilton-Adams' difference), the
//!    four estimates are weighted by inverse gradients, and the vertical
//!    and horizontal results are blended by the statistic from pass 1.
//! 4. The missing chroma at red and blue sites along the better diagonal,
//!    then red and blue at green sites along the better axis, both as
//!    color differences against the now complete green.
//!
//! Where the reference leaves a four-pixel border to a separate routine
//! and reads uninitialized neighbors just inside it, this port seeds
//! every buffer with the bilinear result, so the border is bilinear and
//! the ring inside it degrades gracefully rather than to zero.

use rayon::prelude::*;

use super::demosaic_bilinear;
use crate::error::{Error, Result};
use crate::raw::{CfaColor, CfaPattern};

/// Pixels on each side the reference does not interpolate.
const BORDER: usize = 4;
/// Tolerances to avoid dividing by zero, as in the reference.
const EPS: f32 = 1e-5;
const EPS_SQ: f32 = 1e-10;

/// True for a 2x2 pattern with green on one diagonal and red and blue on
/// the other: the only layout RCD is defined for.
pub fn is_bayer(pattern: &CfaPattern) -> bool {
    use CfaColor::Green;
    if pattern.width != 2 || pattern.height != 2 || !pattern.is_rgb() {
        return false;
    }
    let c = |r, col| pattern.color_at(r, col);
    let greens_on_main = c(0, 0) == Green && c(1, 1) == Green && c(0, 1) != c(1, 0);
    let greens_on_anti = c(0, 1) == Green && c(1, 0) == Green && c(0, 0) != c(1, 1);
    greens_on_main || greens_on_anti
}

/// Demosaic a Bayer mosaic with RCD. `samples` are one per pixel, levels
/// normalized and white balance applied. Values are clamped to
/// `0..=max(1, largest sample)` so unclipped highlights survive.
///
/// Images too small for the interior fall back to bilinear.
pub fn demosaic_rcd(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
) -> Result<Vec<f32>> {
    if !is_bayer(pattern) {
        return Err(Error::Unsupported(format!(
            "RCD needs a 2x2 Bayer pattern, got {pattern}"
        )));
    }
    let bilinear = demosaic_bilinear(samples, width, height, pattern);
    if width < 2 * BORDER + 1 || height < 2 * BORDER + 1 {
        return Ok(bilinear);
    }

    let n = width * height;
    let w = width as isize;
    let ceiling = samples.iter().copied().fold(1.0f32, f32::max);
    let color = |x: usize, y: usize| pattern.color_at(y, x);
    let chroma_index = |x: usize, y: usize| {
        color(x, y)
            .rgb_index()
            .expect("Bayer pattern checked above")
    };
    let is_green = |x: usize, y: usize| color(x, y) == CfaColor::Green;
    let interior_rows = |y: usize| y >= BORDER && y < height - BORDER;
    let at = |buf: &[f32], i: usize, off: isize| buf[(i as isize + off) as usize];

    // Pass 1: vertical/horizontal discrimination at every interior pixel.
    let mut vh = vec![0.5f32; n];
    vh.par_chunks_mut(width)
        .enumerate()
        .filter(|(y, _)| interior_rows(*y))
        .for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate().take(width - BORDER).skip(BORDER) {
                let i = y * width + x;
                let v = direction_stat(line(samples, i, w));
                let h = direction_stat(line(samples, i, 1));
                *out = v / (v + h);
            }
        });

    // Pass 2: low-pass filter at red and blue sites, two pixels in.
    let mut lpf = vec![0.0f32; n];
    lpf.par_chunks_mut(width)
        .enumerate()
        .filter(|(y, _)| *y >= 2 && *y < height - 2)
        .for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate().take(width - 2).skip(2) {
                if is_green(x, y) {
                    continue;
                }
                let i = y * width + x;
                let s = |off| at(samples, i, off);
                *out = 0.25 * s(0)
                    + 0.125 * (s(-w) + s(w) + s(-1) + s(1))
                    + 0.0625 * (s(-w - 1) + s(-w + 1) + s(w - 1) + s(w + 1));
            }
        });

    // Pass 3: green at red and blue sites.
    let mut green: Vec<f32> = bilinear.iter().skip(1).step_by(3).copied().collect();
    green
        .par_chunks_mut(width)
        .enumerate()
        .filter(|(y, _)| interior_rows(*y))
        .for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate().take(width - BORDER).skip(BORDER) {
                if is_green(x, y) {
                    continue;
                }
                let i = y * width + x;
                let s = |off| at(samples, i, off);
                let l = |off| at(&lpf, i, off);
                let disc = refined(&vh, i, w);

                let n_grad = EPS
                    + (s(-w) - s(w)).abs()
                    + (s(0) - s(-2 * w)).abs()
                    + (s(-w) - s(-3 * w)).abs()
                    + (s(-2 * w) - s(-4 * w)).abs();
                let s_grad = EPS
                    + (s(w) - s(-w)).abs()
                    + (s(0) - s(2 * w)).abs()
                    + (s(w) - s(3 * w)).abs()
                    + (s(2 * w) - s(4 * w)).abs();
                let w_grad = EPS
                    + (s(-1) - s(1)).abs()
                    + (s(0) - s(-2)).abs()
                    + (s(-1) - s(-3)).abs()
                    + (s(-2) - s(-4)).abs();
                let e_grad = EPS
                    + (s(1) - s(-1)).abs()
                    + (s(0) - s(2)).abs()
                    + (s(1) - s(3)).abs()
                    + (s(2) - s(4)).abs();

                // Ratio-corrected estimates: the neighbor's green scaled by
                // how the low-pass image changes across it.
                let est = |g: f32, far: f32| g * (1.0 + (l(0) - far) / (EPS + l(0) + far));
                let n_est = est(s(-w), l(-2 * w));
                let s_est = est(s(w), l(2 * w));
                let w_est = est(s(-1), l(-2));
                let e_est = est(s(1), l(2));

                let v_est = (s_grad * n_est + n_grad * s_est) / (n_grad + s_grad);
                let h_est = (w_grad * e_est + e_grad * w_est) / (e_grad + w_grad);
                *out = (disc * h_est + (1.0 - disc) * v_est).clamp(0.0, ceiling);
            }
        });
    drop(lpf);

    // Pass 4a: diagonal discrimination at red and blue sites.
    let mut pq = vec![0.5f32; n];
    pq.par_chunks_mut(width)
        .enumerate()
        .filter(|(y, _)| interior_rows(*y))
        .for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate().take(width - BORDER).skip(BORDER) {
                if is_green(x, y) {
                    continue;
                }
                let i = y * width + x;
                let p = direction_stat(line(samples, i, w + 1));
                let q = direction_stat(line(samples, i, w - 1));
                *out = p / (p + q);
            }
        });

    // Pass 4b: the missing chroma at red and blue sites (blue at red, red
    // at blue). The diagonal neighbors carry that color in the mosaic.
    let mut other: Vec<f32> = (0..n)
        .map(|i| {
            let (x, y) = (i % width, i / width);
            if is_green(x, y) {
                0.0
            } else {
                bilinear[i * 3 + 2 - chroma_index(x, y)]
            }
        })
        .collect();
    other
        .par_chunks_mut(width)
        .enumerate()
        .filter(|(y, _)| interior_rows(*y))
        .for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate().take(width - BORDER).skip(BORDER) {
                if is_green(x, y) {
                    continue;
                }
                let i = y * width + x;
                let s = |off| at(samples, i, off);
                let g = |off| at(&green, i, off);
                let disc = refined(&pq, i, w);

                let nw_grad = EPS
                    + (s(-w - 1) - s(w + 1)).abs()
                    + (s(-w - 1) - s(-3 * w - 3)).abs()
                    + (g(0) - g(-2 * w - 2)).abs();
                let ne_grad = EPS
                    + (s(-w + 1) - s(w - 1)).abs()
                    + (s(-w + 1) - s(-3 * w + 3)).abs()
                    + (g(0) - g(-2 * w + 2)).abs();
                let sw_grad = EPS
                    + (s(w - 1) - s(-w + 1)).abs()
                    + (s(w - 1) - s(3 * w - 3)).abs()
                    + (g(0) - g(2 * w - 2)).abs();
                let se_grad = EPS
                    + (s(w + 1) - s(-w - 1)).abs()
                    + (s(w + 1) - s(3 * w + 3)).abs()
                    + (g(0) - g(2 * w + 2)).abs();

                let nw_est = s(-w - 1) - g(-w - 1);
                let ne_est = s(-w + 1) - g(-w + 1);
                let sw_est = s(w - 1) - g(w - 1);
                let se_est = s(w + 1) - g(w + 1);

                let p_est = (nw_grad * se_est + se_grad * nw_est) / (nw_grad + se_grad);
                let q_est = (ne_grad * sw_est + sw_grad * ne_est) / (ne_grad + sw_grad);
                *out = (g(0) + (1.0 - disc) * p_est + disc * q_est).clamp(0.0, ceiling);
            }
        });

    // Pass 4c: red and blue at green sites, and assembly.
    let mut out = bilinear;
    out.par_chunks_mut(width * 3)
        .enumerate()
        .filter(|(y, _)| interior_rows(*y))
        .for_each(|(y, row)| {
            for x in BORDER..width - BORDER {
                let i = y * width + x;
                let px = &mut row[x * 3..x * 3 + 3];
                if !is_green(x, y) {
                    let own = chroma_index(x, y);
                    px[own] = samples[i];
                    px[1] = green[i];
                    px[2 - own] = other[i];
                    continue;
                }
                px[1] = samples[i];
                let g = |off| at(&green, i, off);
                let disc = refined(&vh, i, w);
                for c in [0usize, 2] {
                    // A red-or-blue neighbor's value of color `c`: its own
                    // sample if that is its color, else the pass 4b result.
                    let value = |off: isize| {
                        let j = (i as isize + off) as usize;
                        if chroma_index(j % width, j / width) == c {
                            samples[j]
                        } else {
                            other[j]
                        }
                    };
                    let n_grad = EPS
                        + (g(0) - g(-2 * w)).abs()
                        + (value(-w) - value(w)).abs()
                        + (value(-w) - value(-3 * w)).abs();
                    let s_grad = EPS
                        + (g(0) - g(2 * w)).abs()
                        + (value(w) - value(-w)).abs()
                        + (value(w) - value(3 * w)).abs();
                    let w_grad = EPS
                        + (g(0) - g(-2)).abs()
                        + (value(-1) - value(1)).abs()
                        + (value(-1) - value(-3)).abs();
                    let e_grad = EPS
                        + (g(0) - g(2)).abs()
                        + (value(1) - value(-1)).abs()
                        + (value(1) - value(3)).abs();

                    let n_est = value(-w) - g(-w);
                    let s_est = value(w) - g(w);
                    let w_est = value(-1) - g(-1);
                    let e_est = value(1) - g(1);

                    let v_est = (n_grad * s_est + s_grad * n_est) / (n_grad + s_grad);
                    let h_est = (e_grad * w_est + w_grad * e_est) / (e_grad + w_grad);
                    px[c] = (g(0) + (1.0 - disc) * v_est + disc * h_est).clamp(0.0, ceiling);
                }
            }
        });
    Ok(out)
}

/// Nine samples centered on `i`, `step` apart.
#[inline]
fn line(samples: &[f32], i: usize, step: isize) -> [f32; 9] {
    let mut v = [0.0f32; 9];
    for (k, out) in v.iter_mut().enumerate() {
        *out = samples[(i as isize + (k as isize - 4) * step) as usize];
    }
    v
}

/// The reference's directional statistic: a quadratic form over nine
/// samples along one line, larger where the line is busier. Symmetric
/// under reversal, zero on a constant line, floored at `EPS_SQ`.
#[inline]
fn direction_stat(v: [f32; 9]) -> f32 {
    let [m4, m3, m2, m1, c, p1, p2, p3, p4] = v;
    let s = -18.0 * c * m1 - 18.0 * c * p1 - 36.0 * c * m2 - 36.0 * c * p2
        + 18.0 * c * m3
        + 18.0 * c * p3
        - 2.0 * c * m4
        - 2.0 * c * p4
        + 38.0 * c * c
        - 70.0 * m1 * p1
        - 12.0 * m1 * m2
        + 24.0 * m1 * p2
        - 38.0 * m1 * m3
        + 16.0 * m1 * p3
        + 12.0 * m1 * m4
        - 6.0 * m1 * p4
        + 46.0 * m1 * m1
        + 24.0 * p1 * m2
        - 12.0 * p1 * p2
        + 16.0 * p1 * m3
        - 38.0 * p1 * p3
        - 6.0 * p1 * m4
        + 12.0 * p1 * p4
        + 46.0 * p1 * p1
        + 14.0 * m2 * p2
        - 12.0 * m2 * p3
        - 2.0 * m2 * m4
        + 2.0 * m2 * p4
        + 11.0 * m2 * m2
        - 12.0 * p2 * m3
        + 2.0 * p2 * m4
        - 2.0 * p2 * p4
        + 11.0 * p2 * p2
        + 2.0 * m3 * p3
        - 6.0 * m3 * m4
        + 10.0 * m3 * m3
        - 6.0 * p3 * p4
        + 10.0 * p3 * p3
        + m4 * m4
        + p4 * p4;
    s.max(EPS_SQ)
}

/// The reference's refinement of a discrimination value: take the mean of
/// the four diagonal neighbors instead when it is more decisive.
#[inline]
fn refined(dir: &[f32], i: usize, w: isize) -> f32 {
    let at = |off: isize| dir[(i as isize + off) as usize];
    let central = at(0);
    let neighborhood = 0.25 * (at(-w - 1) + at(-w + 1) + at(w - 1) + at(w + 1));
    if (0.5 - central).abs() < (0.5 - neighborhood).abs() {
        neighborhood
    } else {
        central
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(width: usize, height: usize, pattern: &CfaPattern, rgb: [f32; 3]) -> Vec<f32> {
        (0..width * height)
            .map(|i| rgb[pattern.color_at(i / width, i % width).rgb_index().unwrap()])
            .collect()
    }

    #[test]
    fn recognizes_bayer_layouts() {
        for name in ["RGGB", "BGGR", "GRBG", "GBRG"] {
            let colors = name
                .chars()
                .map(|c| match c {
                    'R' => CfaColor::Red,
                    'G' => CfaColor::Green,
                    _ => CfaColor::Blue,
                })
                .collect();
            assert!(is_bayer(&CfaPattern::new(2, 2, colors).unwrap()), "{name}");
        }
        use CfaColor::*;
        assert!(!is_bayer(
            &CfaPattern::new(2, 2, vec![Red, Green, Blue, Green]).unwrap()
        ));
        assert!(!is_bayer(
            &CfaPattern::new(2, 2, vec![Green, Green, Green, Green]).unwrap()
        ));
        let xtrans = CfaPattern::new(6, 6, vec![Green; 36]).unwrap();
        assert!(demosaic_rcd(&[0.0; 36], 6, 6, &xtrans).is_err());
    }

    #[test]
    fn direction_stat_is_symmetric_and_zero_on_a_constant_line() {
        assert_eq!(direction_stat([0.3; 9]), EPS_SQ);
        let v = [0.1, 0.5, 0.2, 0.9, 0.4, 0.3, 0.8, 0.6, 0.7];
        let mut r = v;
        r.reverse();
        let (a, b) = (direction_stat(v), direction_stat(r));
        assert!((a - b).abs() < 1e-4 * a.abs(), "{a} vs {b}");
        assert!(a > 0.0);
    }

    #[test]
    fn flat_field_is_reproduced_exactly_for_every_layout() {
        for name in ["RGGB", "BGGR", "GRBG", "GBRG"] {
            let colors = name
                .chars()
                .map(|c| match c {
                    'R' => CfaColor::Red,
                    'G' => CfaColor::Green,
                    _ => CfaColor::Blue,
                })
                .collect();
            let pattern = CfaPattern::new(2, 2, colors).unwrap();
            let want = [0.5, 1.0, 0.25];
            let cfa = flat(20, 16, &pattern, want);
            let out = demosaic_rcd(&cfa, 20, 16, &pattern).unwrap();
            for (i, px) in out.as_chunks::<3>().0.iter().enumerate() {
                for c in 0..3 {
                    assert!((px[c] - want[c]).abs() < 1e-5, "{name} pixel {i}: {px:?}");
                }
            }
        }
    }

    #[test]
    fn grey_ramp_stays_grey() {
        // A horizontal ramp in all three channels: every estimate should
        // land on the ramp, inside the border, to well under a code value.
        let (w, h) = (32, 16);
        let pattern = CfaPattern::rggb();
        let cfa: Vec<f32> = (0..w * h)
            .map(|i| (i % w) as f32 / (w - 1) as f32)
            .collect();
        let out = demosaic_rcd(&cfa, w, h, &pattern).unwrap();
        for y in BORDER..h - BORDER {
            for x in BORDER..w - BORDER {
                let want = x as f32 / (w - 1) as f32;
                let px = &out[(y * w + x) * 3..(y * w + x) * 3 + 3];
                for v in px {
                    assert!((v - want).abs() < 2e-3, "({x},{y}) {px:?} want {want}");
                }
            }
        }
    }

    #[test]
    fn highlights_above_one_survive() {
        let pattern = CfaPattern::rggb();
        let cfa = flat(20, 16, &pattern, [2.0, 1.0, 1.5]);
        let out = demosaic_rcd(&cfa, 20, 16, &pattern).unwrap();
        let px = &out[(8 * 20 + 8) * 3..(8 * 20 + 8) * 3 + 3];
        assert!(
            (px[0] - 2.0).abs() < 1e-4 && (px[2] - 1.5).abs() < 1e-4,
            "{px:?}"
        );
    }

    #[test]
    fn small_images_fall_back_to_bilinear() {
        let pattern = CfaPattern::rggb();
        let cfa = flat(6, 6, &pattern, [0.5, 1.0, 0.25]);
        let out = demosaic_rcd(&cfa, 6, 6, &pattern).unwrap();
        assert_eq!(out, demosaic_bilinear(&cfa, 6, 6, &pattern));
    }
}
