//! The output sharpen ([`greycard_edit::OutputSharpen`]): an unsharp
//! mask on the finished picture's luminance. It runs between the look
//! table and the output transform ([`crate::finish`]), on the picture
//! display-referred in the working space. The luminance is read
//! encoded, through the sRGB curve the point curves work in, so a
//! step's halo is the same size in lightness on its dark and its light
//! side; sharpened in linear light instead, the dark side takes the
//! whole change in the few levels it has and goes to black in a hard
//! line (`linear_light_halos_are_lopsided` measures it). The sharpened
//! luminance goes back on as a ratio on the linear color, so the
//! channels keep their proportions: brightness only, the hue and the
//! saturation as they were.
//!
//! The blur is a Gaussian of the radius in pixels of the full-size
//! picture, scaled with the picture as an export made smaller is (and
//! as the viewport's zoom does, in `viewport.wgsl`), its edges clamped.
//! The threshold is a soft knee on the difference from it, `d³ / (d² +
//! t²)`: half the difference at the threshold, less below it as its
//! square, nearly all of it above, with no step anywhere, so no level
//! is cut off into a contour. A pixel that is not a number is left as
//! it is and gives its neighbors' blurs nothing.

use greycard_edit::OutputSharpen;
use rayon::prelude::*;

use crate::finish::{decode, encode};

/// Luminance weights of the working space, Rec.2020.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// The most taps a side the blur takes: three of the widest radius.
pub const MOST_TAPS: usize = 9;

/// The linear luminance under which a pixel is black and is left so:
/// no ratio lifts it. `viewport.wgsl`'s `OSHARP_BLACK` is this.
pub const BLACK: f32 = 1e-6;

/// The sharpen as the finish of one picture takes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub amount: f32,
    /// The blur's standard deviation in this picture's pixels.
    pub sigma: f32,
    pub threshold: f32,
}

impl Params {
    /// `s` for a picture `reach` of its pixels to a pixel of the
    /// full-size picture, or none when it does nothing.
    pub fn of(s: &OutputSharpen, reach: f32) -> Option<Self> {
        s.acting().map(|s| Params {
            amount: s.amount,
            sigma: s.radius * reach,
            threshold: s.threshold,
        })
    }

    /// Taps a side of the blur: what a band of rows needs beyond it.
    pub fn reach(&self) -> usize {
        half_width(self.sigma)
    }
}

/// Taps a side for a blur of `sigma`: three of it, one at least.
pub fn half_width(sigma: f32) -> usize {
    ((3.0 * sigma).ceil() as usize).clamp(1, MOST_TAPS)
}

/// The blur's weights from the middle out, the middle first, as the
/// shader makes them: not summed to one, since a blur divides by the
/// weights of the taps it read.
pub fn kernel(sigma: f32) -> Vec<f32> {
    let s = sigma.max(1e-3);
    (0..=half_width(sigma))
        .map(|i| (-((i * i) as f32) / (2.0 * s * s)).exp())
        .collect()
}

/// A color's luminance, encoded: what the sharpen reads. Not a number
/// when the color is not finite.
#[inline]
pub fn luminance(c: [f32; 3]) -> f32 {
    if !c.iter().all(|v| v.is_finite()) {
        return f32::NAN;
    }
    encode((LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2]).clamp(0.0, 1.0))
}

/// The share of a difference `d` that is sharpened under threshold
/// `t`: the soft knee.
#[inline]
pub fn soft(d: f32, t: f32) -> f32 {
    let d2 = d * d;
    if t <= 0.0 || d2 == 0.0 {
        d
    } else {
        d * d2 / (d2 + t * t)
    }
}

/// `c` with luminance `l` sharpened against its blur `blurred`: the
/// encoded luminance moved, and the linear color scaled by what that
/// does to the linear luminance. Black stays black; no channel goes
/// under zero, on any pixel, moved or not, and a brightening stops where the color's largest
/// channel reaches white, so the channels' proportions hold.
#[inline]
pub fn apply(c: [f32; 3], l: f32, blurred: f32, p: &Params) -> [f32; 3] {
    if !c.iter().all(|v| v.is_finite()) {
        return c;
    }
    // Every pixel the sharpen passes over is held at zero from below,
    // moved or not, so a flat stretch outside the gamut is clipped as
    // its sharpened edge is.
    let c = c.map(|v| v.max(0.0));
    let delta = p.amount * soft(l - blurred, p.threshold);
    if delta == 0.0 || !delta.is_finite() {
        return c;
    }
    let base = decode(l);
    if base <= BLACK {
        return c;
    }
    let mut ratio = decode((l + delta).clamp(0.0, 1.0)) / base;
    let top = c[0].max(c[1]).max(c[2]);
    if ratio > 1.0 && top > 0.0 {
        ratio = ratio.min((1.0 / top).max(1.0));
    }
    c.map(|v| v * ratio)
}

/// One tap a side of `x` in a line `len` long, clamped at its ends,
/// read through `at`; the taps that are not numbers left out of the
/// sum and their weight with them.
#[inline]
fn blur_at(k: &[f32], x: usize, len: usize, at: impl Fn(usize) -> f32) -> f32 {
    let n = k.len() - 1;
    let (mut acc, mut sum) = (0.0f32, 0.0f32);
    for i in 0..=2 * n {
        let j = (x + i).saturating_sub(n).min(len - 1);
        let v = at(j);
        if v.is_finite() {
            let w = k[i.abs_diff(n)];
            acc += w * v;
            sum += w;
        }
    }
    if sum > 0.0 { acc / sum } else { f32::NAN }
}

/// `plane`, `w` wide and `h` tall, through the Gaussian of `sigma`,
/// its edges clamped: across, then down.
pub fn blur(plane: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    blur_rows(plane, w, h, 0, h, sigma)
}

/// [`blur`] of `plane`'s `rows` tall picture at rows `first` to
/// `first + count` alone: the rows outside them read for the blur down
/// and its edges clamped at `plane`'s own.
fn blur_rows(
    plane: &[f32],
    w: usize,
    rows: usize,
    first: usize,
    count: usize,
    sigma: f32,
) -> Vec<f32> {
    let k = kernel(sigma);
    let n = k.len() - 1;
    // Across only the rows the blur down reads.
    let lo = first.saturating_sub(n);
    let hi = (first + count + n).min(rows);
    let mut across = vec![0.0f32; w * (hi - lo)];
    across
        .par_chunks_mut(w)
        .zip(plane[lo * w..hi * w].par_chunks(w))
        .for_each(|(out, row)| {
            for (x, o) in out.iter_mut().enumerate() {
                *o = blur_at(&k, x, w, |j| row[j]);
            }
        });
    let mut out = vec![0.0f32; w * count];
    out.par_chunks_mut(w).enumerate().for_each(|(r, row)| {
        let y = first + r;
        for (x, o) in row.iter_mut().enumerate() {
            *o = blur_at(&k, y, rows, |j| across[(j - lo) * w + x]);
        }
    });
    out
}

/// The sharpen over a picture's colors, `w` by `h`, in place.
pub fn sharpen(colors: &mut [[f32; 3]], w: usize, h: usize, p: &Params) {
    let sharpened = sharpen_rows(colors, w, h, 0, h, p);
    colors.copy_from_slice(&sharpened);
}

/// The sharpened colors of rows `first` to `first + count` of `held`,
/// a picture `w` wide and `rows` tall: a band of the frame with the
/// rows its blur reads above and below it, or the whole of it. The
/// picture's edges are `held`'s, so a band held with [`Params::reach`]
/// rows each side (fewer at the frame's own edges) comes out as the
/// same rows of the whole frame do, to the bit.
pub(crate) fn sharpen_rows(
    held: &[[f32; 3]],
    w: usize,
    rows: usize,
    first: usize,
    count: usize,
    p: &Params,
) -> Vec<[f32; 3]> {
    let l: Vec<f32> = held.par_iter().map(|c| luminance(*c)).collect();
    let b = blur_rows(&l, w, rows, first, count, p.sigma);
    held[first * w..(first + count) * w]
        .par_iter()
        .zip(
            l[first * w..(first + count) * w]
                .par_iter()
                .zip(b.par_iter()),
        )
        .map(|(c, (l, b))| apply(*c, *l, *b, p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(amount: f32, radius: f32, threshold: f32) -> OutputSharpen {
        OutputSharpen {
            enabled: true,
            amount,
            radius,
            threshold,
        }
    }

    /// A grey step across the middle of a `w` wide, `h` tall picture,
    /// `dark` to `light`, encoded values, as the finish holds colors.
    fn step(w: usize, h: usize, dark: f32, light: f32) -> Vec<[f32; 3]> {
        (0..w * h)
            .map(|i| {
                let v = decode(if i % w < w / 2 { dark } else { light });
                [v, v, v]
            })
            .collect()
    }

    #[test]
    fn nothing_at_no_amount_and_the_radius_scales_with_the_picture() {
        assert_eq!(Params::of(&OutputSharpen::default(), 1.0), None);
        assert_eq!(Params::of(&on(0.5, 1.0, 0.0).with_off(), 1.0), None);
        let p = Params::of(&on(0.5, 2.0, 0.01), 0.25).unwrap();
        assert_eq!(p.sigma, 0.5);
        let k = kernel(1.0);
        assert_eq!((k.len(), k[0]), (4, 1.0));
        // A flat line stays the value it is, the weights summed out.
        let flat = blur(&[0.37; 12 * 5], 12, 5, 1.3);
        assert!(flat.iter().all(|v| (v - 0.37).abs() < 1e-6));
        assert_eq!(half_width(3.0), MOST_TAPS);
    }

    trait Off {
        fn with_off(self) -> Self;
    }
    impl Off for OutputSharpen {
        fn with_off(self) -> Self {
            OutputSharpen {
                enabled: false,
                ..self
            }
        }
    }

    /// A step is steepened, its two sides pushed apart by the same
    /// lightness; a flat stays flat, and a grey stays grey.
    #[test]
    fn a_step_steepens_and_a_flat_keeps() {
        let (w, h) = (32, 4);
        let mut px = step(w, h, 0.3, 0.7);
        let p = Params::of(&on(1.0, 1.0, 0.0), 1.0).unwrap();
        sharpen(&mut px, w, h, &p);
        let e = |x: usize| encode(px[w + x][0]);
        assert!(e(w / 2 - 1) < 0.3 - 0.05, "{}", e(w / 2 - 1));
        assert!(e(w / 2) > 0.7 + 0.05, "{}", e(w / 2));
        // The halo is as deep on the dark side as it is high on the
        // light one.
        let (under, over) = (0.3 - e(w / 2 - 1), e(w / 2) - 0.7);
        assert!((under - over).abs() < 1e-3, "{under} {over}");
        // Far from the step, nothing.
        assert!((e(2) - 0.3).abs() < 1e-5 && (e(w - 3) - 0.7).abs() < 1e-5);
        assert!(px.iter().all(|c| c[0] == c[1] && c[1] == c[2]));
    }

    /// Under the threshold a difference is kept back, softly: half at
    /// the threshold, a ninth at a third of it, and the change never
    /// jumps as the difference grows.
    #[test]
    fn the_threshold_is_a_soft_knee() {
        let t = 0.02;
        assert!((soft(t, t) - t / 2.0).abs() < 1e-7);
        assert!((soft(t / 3.0, t) - (t / 3.0) / 10.0).abs() < 1e-7);
        assert!((soft(-t, t) + t / 2.0).abs() < 1e-7);
        assert!(soft(10.0 * t, t) > 0.99 * 10.0 * t);
        let mut last = 0.0;
        for i in 1..2000 {
            let d = i as f32 * 1e-4;
            let s = soft(d, t);
            assert!(s > last && s - last < 2e-4, "at {d}");
            last = s;
        }
        // A low-contrast step under it moves far less than with none.
        let (w, h) = (32, 2);
        let moved = |threshold: f32| {
            let mut px = step(w, h, 0.50, 0.51);
            let p = Params::of(&on(1.0, 1.0, threshold), 1.0).unwrap();
            sharpen(&mut px, w, h, &p);
            encode(px[w / 2][0]) - 0.51
        };
        assert!(
            moved(0.05) < 0.1 * moved(0.0),
            "{} {}",
            moved(0.05),
            moved(0.0)
        );
    }

    /// Why the luminance is encoded: sharpened in linear light at the
    /// amount that lifts a step's light side as much, the dark side
    /// goes down further in lightness, the more so the darker it is.
    /// Measured here, encoded halos against linear ones on steps from
    /// shadow to highlight, none of them clipped.
    #[test]
    fn linear_light_halos_are_lopsided() {
        let (w, h) = (32, 2);
        for (dark, light) in [(0.25f32, 0.6f32), (0.35, 0.75), (0.45, 0.85), (0.15, 0.4)] {
            let mut px = step(w, h, dark, light);
            sharpen(&mut px, w, h, &Params::of(&on(0.5, 1.0, 0.0), 1.0).unwrap());
            let enc = (
                dark - encode(px[w / 2 - 1][0]),
                encode(px[w / 2][0]) - light,
            );
            // Linear light: the same unsharp mask on linear luminance,
            // its amount set so the light side lifts as far.
            let lin: Vec<f32> = step(w, h, dark, light).iter().map(|c| c[0]).collect();
            let b = blur(&lin, w, h, 1.0);
            let d = |x: usize| lin[x] - b[x];
            let over = |a: f32| encode((lin[w / 2] + a * d(w / 2)).clamp(0.0, 1.0)) - light;
            let (mut lo, mut hi) = (0.0f32, 20.0f32);
            for _ in 0..60 {
                let mid = 0.5 * (lo + hi);
                if over(mid) < enc.1 {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let a = 0.5 * (lo + hi);
            let under = dark - encode((lin[w / 2 - 1] + a * d(w / 2 - 1)).clamp(0.0, 1.0));
            eprintln!(
                "step {dark}-{light}: encoded halos {:.3} under, {:.3} over; \
                 linear {under:.3} under at amount {a:.2}",
                enc.0, enc.1
            );
            assert!(under > 1.5 * enc.0, "{under} against {}", enc.0);
        }
    }

    /// The sharpened luminance goes on as a ratio: on saturated edges
    /// the channels keep their proportions, so the hue and the
    /// saturation are what they were, dark halo and light alike, and
    /// no channel goes under zero or a color past white.
    #[test]
    fn hue_and_saturation_hold_on_a_saturated_edge() {
        let (w, h) = (24, 3);
        for (dark, light) in [
            ([0.02f32, 0.004, 0.001], [0.9, 0.35, 0.05]),
            ([0.05, 0.01, 0.2], [0.2, 0.6, 0.9]),
            ([0.0, 0.03, 0.0], [0.95, 0.4, 0.0]),
        ] {
            let mut px: Vec<[f32; 3]> = (0..w * h)
                .map(|i| if i % w < w / 2 { dark } else { light })
                .collect();
            let before = px.clone();
            sharpen(&mut px, w, h, &Params::of(&on(2.0, 1.5, 0.0), 1.0).unwrap());
            let mut moved = 0;
            for (a, b) in before.iter().zip(&px) {
                assert!(b.iter().all(|v| (0.0..=1.0).contains(v)), "{b:?}");
                if a != b {
                    moved += 1;
                }
                let (ta, tb) = (a[0] + a[1] + a[2], b[0] + b[1] + b[2]);
                // A dark side taken all the way to black has no hue left
                // to keep.
                if tb == 0.0 {
                    continue;
                }
                for k in 0..3 {
                    assert!((a[k] / ta - b[k] / tb).abs() < 1e-5, "{a:?} went to {b:?}");
                }
            }
            assert!(moved >= 2 * h, "the edge moved");
        }
        // Black stays black, however bright its neighbors.
        let mut px = vec![[0.0f32; 3]; 8];
        px[4..].fill([0.8, 0.8, 0.8]);
        sharpen(&mut px, 8, 1, &Params::of(&on(2.0, 1.0, 0.0), 1.0).unwrap());
        assert_eq!(px[3], [0.0; 3]);
    }

    /// A pixel that is not a number stays as it is and gives the blur
    /// nothing: its neighbors come out as they would beside a pixel
    /// that is not there, not spread with it.
    #[test]
    fn a_pixel_that_is_not_a_number_stays_itself_and_spreads_nothing() {
        let (w, h) = (16, 16);
        let p = Params::of(&on(1.0, 2.0, 0.0), 1.0).unwrap();
        let mut px = step(w, h, 0.3, 0.6);
        px[8 * w + 3] = [f32::NAN, 0.2, 0.2];
        px[2 * w + 12] = [f32::INFINITY; 3];
        sharpen(&mut px, w, h, &p);
        assert!(px[8 * w + 3][0].is_nan() && px[8 * w + 3][1] == 0.2);
        assert_eq!(px[2 * w + 12], [f32::INFINITY; 3]);
        assert!(
            px.iter()
                .enumerate()
                .filter(|(i, _)| *i != 8 * w + 3 && *i != 2 * w + 12)
                .all(|(_, c)| c.iter().all(|v| v.is_finite()))
        );
        // A flat field with a hole sharpens to the same flat field.
        let mut flat = vec![[0.4f32; 3]; w * h];
        flat[5 * w + 5] = [f32::NAN; 3];
        sharpen(&mut flat, w, h, &p);
        assert!(
            flat.iter()
                .enumerate()
                .filter(|(i, _)| *i != 5 * w + 5)
                .all(|(_, c)| (c[0] - 0.4).abs() < 1e-6)
        );
    }

    /// A channel under zero (a color outside the working space) is held
    /// at zero on every pixel the sharpen passes over, the flat ones
    /// too, so a flat stretch is clipped as its sharpened edge is.
    #[test]
    fn every_pixel_is_held_at_zero_from_below_alike() {
        let (w, h) = (24, 2);
        let wide = [0.5f32, -0.02, 0.3];
        let mut px: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                if i % w < w / 2 {
                    wide
                } else {
                    [0.05, 0.05, 0.05]
                }
            })
            .collect();
        sharpen(
            &mut px,
            w,
            h,
            &Params::of(&on(1.0, 1.0, 0.01), 1.0).unwrap(),
        );
        assert_eq!(px[0], [0.5, 0.0, 0.3], "a flat pixel far from the edge");
        assert!(px.iter().all(|c| c.iter().all(|v| *v >= 0.0)));
        assert_ne!(px[w / 2 - 1], [0.5, 0.0, 0.3], "the edge moved");
    }

    /// A band of rows held with the blur's reach above and below comes
    /// out as those rows of the whole frame, to the bit.
    #[test]
    fn a_band_is_the_whole_frames_rows() {
        let (w, h) = (20, 41);
        let p = Params::of(&on(1.3, 1.7, 0.01), 1.0).unwrap();
        let px: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let v = ((i * 2654435761) % 1000) as f32 / 1000.0;
                [v, v * 0.7, 0.2]
            })
            .collect();
        let whole = sharpen_rows(&px, w, h, 0, h, &p);
        let n = p.reach();
        for (y0, count) in [(0, 6), (6, 6), (17, 9), (35, 6)] {
            let lo = y0 - n.min(y0);
            let hi = (y0 + count + n).min(h);
            let band = sharpen_rows(&px[lo * w..hi * w], w, hi - lo, y0 - lo, count, &p);
            assert_eq!(band, whole[y0 * w..(y0 + count) * w], "rows {y0} on");
        }
    }
}
