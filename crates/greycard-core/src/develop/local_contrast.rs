//! Local contrast: Texture and Clarity, one op at two scales.
//!
//! Each is a band of the log luminance, scaled by the slider and put
//! back as a single gain on all three channels, so a color keeps its
//! hue and its channel ratios the way the Light sliders do. Texture
//! is the fine band: the log less an edge-preserving base of it from
//! a guided filter (He, Sun and Tang, "Guided Image Filtering", ECCV
//! 2010, self-guided) at a few pixels of radius on a full-size
//! picture, grain included. Clarity is the band between that radius
//! and a fraction of the long edge: the log low-passed at the fine
//! radius, less the guided filter of that at the coarse one. The
//! lower bound is a plain low-pass, a box twice, and not the fine
//! guided base, because a guided filter is an edge-keeper and not a
//! low-pass: at the fine radius it lets a fifth of a fine grating
//! and a twentieth of the grain through into its base, and Clarity
//! would lift them. So Clarity leaves the fine detail and the grain
//! alone, and it is weighted toward the mid-tones so a bright sky is
//! not pushed over the clip nor a shadow under the floor. Negative
//! amounts take the band away instead: Texture below zero smooths
//! skin, Clarity below zero softens the structure and keeps the
//! grain.
//!
//! The guided filter rather than a Gaussian or a bilateral: a Gaussian
//! base (an unsharp mask at a large radius) rings a strong edge with a
//! halo the width of the radius, the thing local contrast tools are
//! disliked for; a bilateral at these radii is either a grid
//! approximation or too slow; the guided filter is four box filters,
//! linear in the pixels whatever the radius, and where a window's
//! variance is well above its epsilon it passes the picture through,
//! so a hard edge comes out where it went in. Its own artifact, that
//! a window holding one strong edge protects the texture beside it
//! too, is the tradeoff taken.

use crate::image::WorkingImage;
use rayon::prelude::*;

/// Luminance weights of the working space, Rec.2020.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// Mid grey, where the stops are counted from.
const MID_GREY: f32 = 0.18;

/// Below this a luminance is taken as this, so the log has a floor.
const FLOOR: f32 = 1e-6;

/// Texture's radius as a fraction of the long edge: three pixels on a
/// 6000-pixel picture, and never under [`MIN_TEXTURE_RADIUS`].
pub const TEXTURE_RADIUS_FRACTION: f32 = 1.0 / 2000.0;
pub const MIN_TEXTURE_RADIUS: usize = 2;
/// Clarity's radius as a fraction of the long edge: 150 pixels on a
/// 6000-pixel picture.
pub const CLARITY_RADIUS_FRACTION: f32 = 1.0 / 40.0;
pub const MIN_CLARITY_RADIUS: usize = 8;

/// The guided filter's epsilon at each scale, in stops squared: the
/// window variance at which half the detail is protected as an edge.
/// Fine detail is a fraction of a stop, so Texture's is small and
/// protects any real edge. Clarity's is half a stop of standard
/// deviation, the middle of a tradeoff measured in the tests: the
/// halo a hard edge gets grows about in proportion to epsilon (a
/// third of a stop at a four-stop edge at this value, at the slider's
/// end), while the lift on a structure of a third of a stop hardly
/// depends on it (92 percent here); what a larger epsilon buys is
/// more lift on structures of a stop or two (71 and 40 percent here),
/// at the price of that halo.
const TEXTURE_EPSILON: f32 = 0.05;
const CLARITY_EPSILON: f32 = 0.25;

/// The band at an amount of one, as a multiple of itself: Texture at
/// +1 doubles the fine band; Clarity at +1 adds this much of the
/// middle one, which lost the fine energy the first cut let it keep,
/// so it is set above one to read about as it did.
const TEXTURE_GAIN: f32 = 1.0;
const CLARITY_GAIN: f32 = 1.5;

/// Where Clarity's weight fades out, in stops about mid grey: full
/// between the two inner values, nothing beyond the outer ones. Scene
/// white is 2.47 stops above mid grey, and the fade under it keeps
/// the op from pushing a near-white over the clip. The stops are the
/// scene's, before the look's exposure, so the shadow fade is set
/// deep: a candle-lit interior at base exposure lives four stops
/// under mid grey and is still detail, and what is left out is the
/// noise floor.
const CLARITY_SHADOW_FADE: (f32, f32) = (-8.0, -5.5);
const CLARITY_HIGHLIGHT_FADE: (f32, f32) = (1.5, 2.5);

/// The gain fades to nothing over the top of the scale, from this
/// fraction of the clip level (four tenths of a stop under it) to the
/// clip, on the pixel's brightest channel: a clipped plateau is not
/// detail, and the band would lift its inside edge past the clip and
/// darken the ring around it.
const CLIP_FADE: f32 = 0.75;

/// How much of each band to add: -1 to 1, zero for none.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LocalContrastOptions {
    pub texture: f32,
    pub clarity: f32,
}

impl LocalContrastOptions {
    /// Whether the op would leave the picture as it is.
    pub fn is_identity(&self) -> bool {
        self.texture == 0.0 && self.clarity == 0.0
    }
}

/// What a run did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalContrastStats {
    /// Texture's radius in pixels, for the picture's size.
    pub texture_radius: usize,
    /// Clarity's.
    pub clarity_radius: usize,
}

/// The radii for a picture of this size: the same fractions of the
/// long edge whatever the picture, so a preview and an export at
/// different sizes agree in what they show.
pub fn radii(width: usize, height: usize) -> (usize, usize) {
    let long = width.max(height) as f32;
    let texture = ((long * TEXTURE_RADIUS_FRACTION).round() as usize).max(MIN_TEXTURE_RADIUS);
    let clarity = ((long * CLARITY_RADIUS_FRACTION).round() as usize).max(MIN_CLARITY_RADIUS);
    (texture, clarity)
}

/// Apply the local contrast in place. `clip_level` is the value at
/// or above which a channel counts as clipped, the develop's, as the
/// sharpen takes it; the gain fades to nothing there. Infinite for a
/// picture with no clip.
pub fn local_contrast(
    image: &mut WorkingImage,
    options: &LocalContrastOptions,
    clip_level: f32,
) -> LocalContrastStats {
    let (w, h) = (image.width, image.height);
    let (texture_radius, clarity_radius) = radii(w, h);
    let stats = LocalContrastStats {
        texture_radius,
        clarity_radius,
    };
    if options.is_identity() || w == 0 || h == 0 {
        return stats;
    }
    let n = w * h;

    // Log luminance in stops about mid grey.
    let mut log: Vec<f32> = vec![0.0; n];
    log.par_iter_mut()
        .zip(image.data.par_chunks(3))
        .for_each(|(l, px)| {
            let y = LUMA[0] * px[0] + LUMA[1] * px[1] + LUMA[2] * px[2];
            *l = (y.max(FLOOR) / MID_GREY).log2();
        });

    // Each band is a shift in stops put on as a gain, a power of two
    // of it, on all three channels, and the two gains compose. The
    // op holds the log and the three scratch planes, four in all.
    let mut scratch = Scratch::new(n);
    if options.texture != 0.0 {
        let k = options.texture.clamp(-1.0, 1.0) * TEXTURE_GAIN;
        scratch.guided_filter(&log, w, h, texture_radius, TEXTURE_EPSILON);
        apply_shift(image, &log, &scratch.a, clip_level, |_| k);
    }
    if options.clarity != 0.0 {
        // The log is done with: low-passed in place, it is the top
        // of Clarity's band, and the guided filter of it the bottom.
        for _ in 0..2 {
            box_mean(&mut log, &mut scratch.tmp, w, h, texture_radius);
        }
        let k = options.clarity.clamp(-1.0, 1.0) * CLARITY_GAIN;
        scratch.guided_filter(&log, w, h, clarity_radius, CLARITY_EPSILON);
        apply_shift(image, &log, &scratch.a, clip_level, |b| {
            k * midtone_weight(b)
        });
    }
    stats
}

/// Put `k(base) * (upper - base)` stops on the picture as a gain,
/// faded out toward the clip.
fn apply_shift(
    image: &mut WorkingImage,
    upper: &[f32],
    base: &[f32],
    clip_level: f32,
    k: impl Fn(f32) -> f32 + Sync,
) {
    let fade_from = clip_level * CLIP_FADE;
    image
        .data
        .par_chunks_mut(3)
        .zip(upper.par_iter())
        .zip(base.par_iter())
        .for_each(|((px, &u), &b)| {
            let brightest = px[0].max(px[1]).max(px[2]);
            let clip = if clip_level.is_finite() {
                1.0 - smoothstep(fade_from, clip_level, brightest)
            } else {
                1.0
            };
            let gain = (clip * k(b) * (u - b)).exp2();
            px[0] *= gain;
            px[1] *= gain;
            px[2] *= gain;
        });
}

/// Clarity's weight at a base luminance in stops about mid grey: one
/// through the mid-tones, fading to nothing at the ends of the
/// scale.
#[inline]
fn midtone_weight(stops: f32) -> f32 {
    smoothstep(CLARITY_SHADOW_FADE.0, CLARITY_SHADOW_FADE.1, stops)
        * (1.0 - smoothstep(CLARITY_HIGHLIGHT_FADE.0, CLARITY_HIGHLIGHT_FADE.1, stops))
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The planes the guided filter works in, three of the picture's
/// size: the answer is left in `a`.
struct Scratch {
    a: Vec<f32>,
    b: Vec<f32>,
    tmp: Vec<f32>,
}

impl Scratch {
    fn new(n: usize) -> Self {
        Self {
            a: vec![0.0; n],
            b: vec![0.0; n],
            tmp: vec![0.0; n],
        }
    }

    /// The guided filter of `input` by itself: in every window of
    /// radius `radius` the least-squares line `a * input + b`, `a`
    /// shrunk by `epsilon` against the window's variance, the two
    /// coefficients averaged over the windows a pixel is in. Where the
    /// variance is well above epsilon `a` is near one and the picture
    /// passes through; well below it, `a` is near zero and the
    /// window's mean comes out. The answer is left in `self.a` until
    /// the next call.
    fn guided_filter(
        &mut self,
        input: &[f32],
        width: usize,
        height: usize,
        radius: usize,
        epsilon: f32,
    ) {
        let Scratch { a, b, tmp } = self;
        // a: the mean; b: the mean of the squares.
        a.copy_from_slice(input);
        box_mean(a, tmp, width, height, radius);
        b.par_iter_mut()
            .zip(input.par_iter())
            .for_each(|(sq, &v)| *sq = v * v);
        box_mean(b, tmp, width, height, radius);
        // b: the slope; a: the intercept.
        b.par_iter_mut()
            .zip(a.par_iter_mut())
            .for_each(|(sq, mean)| {
                let var = (*sq - *mean * *mean).max(0.0);
                let slope = var / (var + epsilon);
                *sq = slope;
                *mean -= slope * *mean;
            });
        // Each averaged over the windows, then the answer.
        box_mean(b, tmp, width, height, radius);
        box_mean(a, tmp, width, height, radius);
        a.par_iter_mut()
            .zip(b.par_iter())
            .zip(input.par_iter())
            .for_each(|((o, &slope), &v)| *o += slope * v);
    }
}

/// The mean over a window of `radius` each way, in place, the window
/// cut at the picture's edge and the mean taken over what is inside:
/// two passes of running sums, so the cost is the same at any radius.
/// The row pass reads `data` and writes `tmp`, the column pass reads
/// `tmp` and writes `data`, so no third plane is needed.
fn box_mean(data: &mut [f32], tmp: &mut [f32], w: usize, h: usize, r: usize) {
    // Along the rows.
    tmp.par_chunks_mut(w)
        .zip(data.par_chunks(w))
        .for_each(|(out, row)| {
            let r = r.min(w.saturating_sub(1));
            let mut sum: f32 = row[..=r].iter().sum();
            let mut count = r + 1;
            for x in 0..w {
                out[x] = sum / count as f32;
                if x + r + 1 < w {
                    sum += row[x + r + 1];
                    count += 1;
                }
                if x >= r {
                    sum -= row[x - r];
                    count -= 1;
                }
            }
        });
    // Down the columns, in bands of rows: each band sums its first
    // window whole, then slides. A band is as tall as the window at
    // least, so the first sum costs no more than the sliding does.
    let r = r.min(h.saturating_sub(1));
    let band = (2 * r + 1).max(64).min(h);
    data.par_chunks_mut(band * w)
        .enumerate()
        .for_each(|(i, out)| {
            let y0 = i * band;
            let rows = out.len() / w;
            let row = |y: usize| &tmp[y * w..(y + 1) * w];
            let mut sum = vec![0.0f32; w];
            let top = y0.saturating_sub(r);
            let bottom = (y0 + r).min(h - 1);
            for y in top..=bottom {
                for (s, v) in sum.iter_mut().zip(row(y)) {
                    *s += v;
                }
            }
            let mut count = bottom + 1 - top;
            for (j, out_row) in out.chunks_exact_mut(w).enumerate() {
                let y = y0 + j;
                let scale = 1.0 / count as f32;
                for (o, s) in out_row.iter_mut().zip(&sum) {
                    *o = s * scale;
                }
                if j + 1 == rows {
                    break;
                }
                if y + r + 1 < h {
                    for (s, v) in sum.iter_mut().zip(row(y + r + 1)) {
                        *s += v;
                    }
                    count += 1;
                }
                if y >= r {
                    for (s, v) in sum.iter_mut().zip(row(y - r)) {
                        *s -= v;
                    }
                    count -= 1;
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No clip: what the tests run under unless they say.
    const NO_CLIP: f32 = f32::INFINITY;

    fn luminance(px: &[f32]) -> f32 {
        LUMA[0] * px[0] + LUMA[1] * px[1] + LUMA[2] * px[2]
    }

    fn texture(amount: f32) -> LocalContrastOptions {
        LocalContrastOptions {
            texture: amount,
            clarity: 0.0,
        }
    }

    fn clarity(amount: f32) -> LocalContrastOptions {
        LocalContrastOptions {
            texture: 0.0,
            clarity: amount,
        }
    }

    /// A grey picture from a luminance at each pixel.
    fn grey(w: usize, h: usize, f: impl Fn(usize, usize) -> f32) -> WorkingImage {
        let mut image = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = f(x, y);
                let i = (y * w + x) * 3;
                image.data[i] = v;
                image.data[i + 1] = v;
                image.data[i + 2] = v;
            }
        }
        image
    }

    /// The spread of log luminance over a run of a row, peak to
    /// trough: what a local contrast slider moves.
    fn spread(image: &WorkingImage, y: usize, x0: usize, x1: usize) -> f32 {
        let l: Vec<f32> = (x0..x1)
            .map(|x| luminance(&image.data[(y * image.width + x) * 3..]).log2())
            .collect();
        l.iter().cloned().fold(f32::MIN, f32::max) - l.iter().cloned().fold(f32::MAX, f32::min)
    }

    /// A plain box mean, for the running sums to be checked against.
    fn naive_box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
        let mut out = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut sum = 0.0;
                let mut n = 0;
                for yy in y.saturating_sub(r)..=(y + r).min(h - 1) {
                    for xx in x.saturating_sub(r)..=(x + r).min(w - 1) {
                        sum += src[yy * w + xx];
                        n += 1;
                    }
                }
                out[y * w + x] = sum / n as f32;
            }
        }
        out
    }

    /// The guided filter as the paper writes it: a least-squares line
    /// in every window, the coefficients averaged over the windows a
    /// pixel is in, every window cut at the picture's edge.
    fn naive_guided_filter(input: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
        let window = |x: usize, y: usize| {
            (y.saturating_sub(r)..=(y + r).min(h - 1)).flat_map(move |yy| {
                (x.saturating_sub(r)..=(x + r).min(w - 1)).map(move |xx| (xx, yy))
            })
        };
        let mut a = vec![0.0f64; w * h];
        let mut b = vec![0.0f64; w * h];
        for y in 0..h {
            for x in 0..w {
                let values: Vec<f64> = window(x, y)
                    .map(|(xx, yy)| input[yy * w + xx] as f64)
                    .collect();
                let n = values.len() as f64;
                let mean = values.iter().sum::<f64>() / n;
                let var = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n;
                let slope = var / (var + eps as f64);
                a[y * w + x] = slope;
                b[y * w + x] = mean - slope * mean;
            }
        }
        let mut out = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (mut sa, mut sb, mut n) = (0.0, 0.0, 0.0);
                for (xx, yy) in window(x, y) {
                    sa += a[yy * w + xx];
                    sb += b[yy * w + xx];
                    n += 1.0;
                }
                out[y * w + x] = ((sa / n) * input[y * w + x] as f64 + sb / n) as f32;
            }
        }
        out
    }

    #[test]
    fn the_box_mean_is_the_plain_one_at_every_size_and_radius() {
        // Sizes under, at and over the band, radii that reach past the
        // edges and ones that do not.
        for &(w, h) in &[(1usize, 1usize), (5, 3), (17, 9), (40, 70), (130, 65)] {
            let src: Vec<f32> = (0..w * h)
                .map(|i| ((i * 7919) % 101) as f32 / 101.0 - 0.3)
                .collect();
            for &r in &[0usize, 1, 2, 5, 33] {
                let want = naive_box_mean(&src, w, h, r);
                let mut got = src.clone();
                let mut tmp = vec![0.0; w * h];
                box_mean(&mut got, &mut tmp, w, h, r);
                for (i, (a, b)) in got.iter().zip(&want).enumerate() {
                    assert!((a - b).abs() < 1e-4, "{w}x{h} r={r} at {i}: {a} vs {b}");
                }
            }
        }
    }

    #[test]
    fn the_guided_filter_is_the_papers_and_passes_a_hard_edge_through() {
        // Against the plain per-window least squares on a small
        // picture with structure at every scale and a corner or two
        // of the windows cut.
        let (w, h) = (20, 20);
        let input: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                0.5 * (x * 0.7).sin() + 0.3 * (y * 0.4).cos() + ((i * 7919) % 101) as f32 / 300.0
            })
            .collect();
        let mut scratch = Scratch::new(w * h);
        for &(r, eps) in &[(2usize, 0.05f32), (3, 0.25), (7, 0.01)] {
            let want = naive_guided_filter(&input, w, h, r, eps);
            scratch.guided_filter(&input, w, h, r, eps);
            for (i, (a, b)) in scratch.a.iter().zip(&want).enumerate() {
                assert!((a - b).abs() < 1e-4, "r={r} eps={eps} at {i}: {a} vs {b}");
            }
        }
        // A four-stop step with the variance of any window across it
        // far above epsilon comes out where it went in, and a flat
        // side is exact.
        let (w, h) = (64, 8);
        let step: Vec<f32> = (0..w * h)
            .map(|i| if i % w < 32 { -2.0 } else { 2.0 })
            .collect();
        let mut scratch = Scratch::new(w * h);
        scratch.guided_filter(&step, w, h, 3, 1e-3);
        for (i, (a, b)) in scratch.a.iter().zip(&step).enumerate() {
            assert!((a - b).abs() < 1e-3, "at {i}: {a} vs {b}");
        }
    }

    #[test]
    fn zero_is_the_identity_and_a_flat_field_stays_flat() {
        let (w, h) = (64, 48);
        let mut image = grey(w, h, |x, y| 0.05 + 0.4 * (x * y % 7) as f32 / 7.0);
        let before = image.clone();
        let stats = local_contrast(&mut image, &LocalContrastOptions::default(), NO_CLIP);
        assert_eq!(image, before);
        assert_eq!(stats.texture_radius, MIN_TEXTURE_RADIUS);
        assert_eq!(stats.clarity_radius, MIN_CLARITY_RADIUS);

        let mut flat = grey(w, h, |_, _| 0.3);
        for options in [
            texture(1.0),
            clarity(-1.0),
            LocalContrastOptions {
                texture: -0.5,
                clarity: 0.7,
            },
        ] {
            local_contrast(&mut flat, &options, NO_CLIP);
            for px in flat.pixels() {
                for v in px {
                    assert!((v - 0.3).abs() < 1e-5, "{options:?}: {px:?}");
                }
            }
        }
    }

    /// A fine grating, a third of a stop peak to trough, period 6.
    fn grating(x: usize, _: usize) -> f32 {
        0.18 * (1.0 + 0.12 * (x as f32 * std::f32::consts::TAU / 6.0).sin())
    }

    #[test]
    fn texture_lifts_fine_detail_with_plus_and_smooths_it_with_minus() {
        let (w, h) = (96, 32);
        let mut image = grey(w, h, grating);
        let before = spread(&image, h / 2, 20, 76);
        local_contrast(&mut image, &texture(1.0), NO_CLIP);
        let up = spread(&image, h / 2, 20, 76);
        assert!(up > before * 1.5, "{before} -> {up}");

        let mut image = grey(w, h, grating);
        local_contrast(&mut image, &texture(-1.0), NO_CLIP);
        let down = spread(&image, h / 2, 20, 76);
        assert!(down < before * 0.5, "{before} -> {down}");
    }

    #[test]
    fn clarity_leaves_the_fine_band_alone() {
        // The grating at Clarity ±1 keeps its spread within a few
        // percent: the band is between the two radii, and the grain
        // and the fine detail are Texture's alone. So both at +1
        // double the fine band, not quadruple it. A picture wide
        // enough for the radii to be a full-size frame's, three and
        // 125 pixels, since a box's leak depends on its width.
        let (w, h) = (5000, 8);
        assert_eq!(radii(w, h), (3, 125));
        let plain = grey(w, h, grating);
        let before = spread(&plain, h / 2, 2000, 3000);
        for amount in [1.0, -1.0] {
            let mut image = plain.clone();
            local_contrast(&mut image, &clarity(amount), NO_CLIP);
            let after = spread(&image, h / 2, 2000, 3000);
            assert!(
                (after - before).abs() < before * 0.05,
                "clarity {amount}: {before} -> {after}"
            );
        }
        let mut image = plain.clone();
        local_contrast(&mut image, &texture(1.0), NO_CLIP);
        let texture_alone = spread(&image, h / 2, 2000, 3000);
        assert!(texture_alone > before * 1.5, "{before} -> {texture_alone}");
        let mut image = plain.clone();
        local_contrast(
            &mut image,
            &LocalContrastOptions {
                texture: 1.0,
                clarity: 1.0,
            },
            NO_CLIP,
        );
        let both = spread(&image, h / 2, 2000, 3000);
        assert!(
            (both - texture_alone).abs() < before * 0.05,
            "texture alone {texture_alone}, both {both}"
        );
    }

    #[test]
    fn clarity_lifts_a_soft_mid_tone_bump_and_the_edge_is_where_it_was() {
        // A picture wide enough for Clarity's radius to be sixteen
        // pixels; a soft bump a third of a stop high and six pixels
        // of sigma in the middle of a mid grey field, and a hard edge
        // of four stops at the left.
        let (w, h) = (640, 40);
        assert_eq!(radii(w, h).1, 16);
        let scene = |x: usize, _| {
            let d = (x as f32 - 400.0) / 6.0;
            let bump = 0.26 * (-d * d).exp();
            if x < 120 {
                0.18 / 16.0
            } else {
                0.18 * (1.0 + bump)
            }
        };
        let mut image = grey(w, h, scene);
        let before = spread(&image, h / 2, 360, 440);
        let edge_before = spread(&image, h / 2, 80, 160);
        local_contrast(&mut image, &clarity(1.0), NO_CLIP);
        let up = spread(&image, h / 2, 360, 440);
        assert!(up > before * 1.5, "{before} -> {up}");
        // The hard edge is protected: neither side overshoots by more
        // than a third of a stop, where the halo a plain unsharp mask
        // at this radius would leave is two stops.
        let edge_after = spread(&image, h / 2, 80, 160);
        let at = |x: usize| luminance(&image.data[((h / 2) * w + x) * 3..]).log2();
        let bright = (120..160)
            .map(|x| at(x) - (0.18f32).log2())
            .fold(0.0, f32::max);
        let dark = (80..120)
            .map(|x| (0.18f32 / 16.0).log2() - at(x))
            .fold(0.0, f32::max);
        assert!(
            bright < 0.35 && dark < 0.35,
            "halo {bright} stops bright, {dark} dark; edge {edge_before} -> {edge_after}"
        );

        let mut image = grey(w, h, scene);
        local_contrast(&mut image, &clarity(-1.0), NO_CLIP);
        let down = spread(&image, h / 2, 360, 440);
        assert!(down < before * 0.5, "{before} -> {down}");
    }

    #[test]
    fn clarity_leaves_the_ends_of_the_scale_alone() {
        // The same bump on a field near the clip, and one deep in the
        // shadows: neither moves much, so nothing is blown or crushed.
        let (w, h) = (640, 40);
        let bump = |x: usize| {
            let d = (x as f32 - 320.0) / 6.0;
            0.3 * (-d * d).exp()
        };
        let moved = |level: f32| {
            let mut image = grey(w, h, |x, _| level * (1.0 + bump(x)));
            let before = spread(&image, h / 2, 280, 360);
            local_contrast(&mut image, &clarity(1.0), NO_CLIP);
            spread(&image, h / 2, 280, 360) - before
        };
        let mid = moved(0.18);
        let bright = moved(0.9);
        let dark = moved(0.0005);
        assert!(mid > 0.15, "mid {mid}");
        assert!(bright < mid * 0.2, "bright {bright} against mid {mid}");
        assert!(dark < mid * 0.2, "dark {dark} against mid {mid}");
    }

    #[test]
    fn a_clipped_plateau_is_left_alone_and_nothing_is_pushed_over_the_clip() {
        // The grating across the picture, and a plateau at the clip
        // in the middle of it, as a blown window would be.
        let (w, h) = (192, 32);
        let clip = 1.0;
        let scene = |x: usize, y: usize| {
            if (80..112).contains(&x) {
                clip
            } else {
                grating(x, y)
            }
        };
        let plain = grey(w, h, scene);
        let mut image = plain.clone();
        local_contrast(&mut image, &texture(1.0), clip);
        for x in 80..112 {
            let (a, b) = (image.pixel(x, h / 2), plain.pixel(x, h / 2));
            assert_eq!(a, b, "at {x}: the plateau is not detail");
        }
        let brightest = image.data.iter().cloned().fold(0.0, f32::max);
        assert!(brightest <= clip, "pushed over the clip: {brightest}");
        // The grating away from it is lifted as before.
        let before = spread(&plain, h / 2, 10, 70);
        let after = spread(&image, h / 2, 10, 70);
        assert!(after > before * 1.5, "{before} -> {after}");
        // With no clip level given, the plateau's edge would move:
        // the guard is what held it.
        let mut unguarded = plain.clone();
        local_contrast(&mut unguarded, &texture(1.0), NO_CLIP);
        let brightest = unguarded.data.iter().cloned().fold(0.0, f32::max);
        assert!(
            brightest > clip,
            "the guard was not what held it: {brightest}"
        );
    }

    #[test]
    fn the_gain_keeps_every_channel_ratio() {
        let (w, h) = (80, 40);
        let mut image = grey(w, h, |x, y| 0.1 + 0.3 * ((x / 3 + y / 5) % 4) as f32 / 4.0);
        for (i, px) in image.pixels_mut().enumerate() {
            px[0] *= 1.4 + (i % 3) as f32 * 0.1;
            px[2] *= 0.6;
        }
        let before = image.clone();
        local_contrast(
            &mut image,
            &LocalContrastOptions {
                texture: 0.8,
                clarity: 0.6,
            },
            NO_CLIP,
        );
        let mut changed = 0;
        for (a, b) in image.pixels().zip(before.pixels()) {
            assert!((a[0] / a[1] - b[0] / b[1]).abs() < 1e-4, "{a:?} vs {b:?}");
            assert!((a[2] / a[1] - b[2] / b[1]).abs() < 1e-4, "{a:?} vs {b:?}");
            if (a[1] - b[1]).abs() > 1e-4 {
                changed += 1;
            }
        }
        assert!(changed > w * h / 4, "only {changed} pixels moved");
    }

    #[test]
    fn the_radii_follow_the_long_edge() {
        assert_eq!(radii(6000, 4000), (3, 150));
        assert_eq!(radii(4000, 6000), (3, 150));
        assert_eq!(radii(2000, 1333), (2, 50));
        assert_eq!(radii(100, 100), (MIN_TEXTURE_RADIUS, MIN_CLARITY_RADIUS));
    }
}
