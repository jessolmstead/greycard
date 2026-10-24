//! A window past [`MAX`] a side, taken down to it for the remove and
//! its fill brought back up: the window and its hole are taken down by
//! the mean of what each new pixel covers (a box, or area, filter), so
//! grain and fine detail average rather than alias and nothing rings,
//! the model regenerating detail inside the hole anyway; the answer is
//! brought back by Lanczos 3, sharper than a bilinear or bicubic step,
//! its overshoot clamped. Both in linear light: the window comes in
//! sRGB-encoded, as the model sees it, and is decoded first, so an
//! edge between a bright and a dark area keeps its mean.

use rayon::prelude::*;

use super::MAX;
use crate::image::Rgbf;
use crate::runtime::{Error, Result};

/// The most a window is taken down by for the remove: past it, a side
/// of more than four times [`MAX`], LaMa's tiles make the fill.
pub const FACTOR: usize = 4;

/// How far a window of `width` × `height` is taken down for the
/// remove: 1 when it fits [`MAX`], its longer side over [`MAX`] up to
/// [`FACTOR`], and `None` past that or for nothing.
pub fn factor_for(width: usize, height: usize) -> Option<f32> {
    let side = width.max(height);
    if width == 0 || height == 0 || side > FACTOR * MAX {
        None
    } else if side <= MAX {
        Some(1.0)
    } else {
        Some(side as f32 / MAX as f32)
    }
}

/// The size a `width` × `height` window is taken to so that its
/// longer side is `side`.
pub fn scaled_size(width: usize, height: usize, side: usize) -> (usize, usize) {
    let long = width.max(height) as f64;
    let at = |v: usize| (((v as f64) * side as f64 / long).round() as usize).clamp(1, side);
    (at(width), at(height))
}

/// `image` (in 0 to 1, sRGB-encoded) with the pixels where `hole` is
/// nonzero made up by `run`, at a size whose longer side is `side`:
/// the window and the hole taken down to it ([`shrink`], a small pixel
/// in the hole when any of what it covers is), `run` given those, and
/// its answer brought back up ([`grow`]) and put in under `hole`
/// alone, weighted by its values as [`super::composite`] puts the
/// decoder's, so every pixel outside the hole is the picture's own.
pub fn scaled(
    image: &Rgbf,
    hole: &[f32],
    side: usize,
    run: impl FnOnce(&Rgbf, &[f32]) -> Result<Rgbf>,
) -> Result<Rgbf> {
    let (w, h) = (image.width, image.height);
    if hole.len() != w * h || w == 0 || h == 0 {
        return Err(Error::Shape(format!(
            "a hole of {} for a window of {w}x{h}",
            hole.len()
        )));
    }
    let (sw, sh) = scaled_size(w, h, side);
    let small = shrink(&image.data, 3, w, h, sw, sh, to_linear);
    let small = Rgbf::new(sw, sh, small.into_iter().map(to_srgb).collect());
    let small_hole: Vec<f32> = shrink(hole, 1, w, h, sw, sh, |m| f32::from(u8::from(m > 0.0)))
        .into_iter()
        .map(|c| f32::from(u8::from(c > 0.0)))
        .collect();
    let answer = run(&small, &small_hole)?;
    if (answer.width, answer.height) != (sw, sh) {
        return Err(Error::Shape(format!(
            "an answer of {}x{} for {sw}x{sh}",
            answer.width, answer.height
        )));
    }
    // Brought back over the hole's box alone: the rest is the picture.
    let mut data: Vec<f32> = image.data.iter().map(|v| v.clamp(0.0, 1.0)).collect();
    let Some((xs, ys)) = bounds(hole, w) else {
        return Ok(Rgbf::new(w, h, data));
    };
    let linear: Vec<f32> = answer.data.iter().map(|&v| to_linear(v)).collect();
    let back = grow_box(&linear, 3, (sw, sh), (w, h), xs.clone(), ys.clone());
    let bw = xs.len();
    for (j, y) in ys.enumerate() {
        for (k, x) in xs.clone().enumerate() {
            let i = y * w + x;
            let m = hole[i];
            if m <= 0.0 {
                continue;
            }
            let a = m.clamp(0.0, 1.0);
            for c in 0..3 {
                let p = data[i * 3 + c];
                data[i * 3 + c] = p + (to_srgb(back[(j * bw + k) * 3 + c]) - p) * a;
            }
        }
    }
    Ok(Rgbf::new(w, h, data))
}

/// The columns and rows of the box round the pixels where `hole`, `w`
/// wide, is nonzero; `None` for none.
fn bounds(hole: &[f32], w: usize) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, _) in hole.iter().enumerate().filter(|(_, m)| **m > 0.0) {
        let (x, y) = (i % w, i / w);
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x + 1);
        y1 = y1.max(y + 1);
    }
    (x0 < x1).then_some((x0..x1, y0..y1))
}

/// The sRGB decoding, of a value clamped to 0 to 1.
fn to_linear(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// The sRGB encoding, of a value clamped to 0 to 1.
fn to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// For each of `to` new pixels along an axis of `from`, the old pixels
/// it reads and their weights, summing to one.
type Taps = Vec<Vec<(usize, f32)>>;

/// A box filter's taps from `from` down to `to`: each new pixel the
/// mean of the span of old ones it covers, the end pixels by the part
/// of them it does.
fn box_taps(from: usize, to: usize) -> Taps {
    let s = from as f64 / to as f64;
    (0..to)
        .map(|j| {
            let (a, b) = (j as f64 * s, (j + 1) as f64 * s);
            let mut taps: Vec<(usize, f32)> = (a.floor() as usize..(b.ceil() as usize).min(from))
                .map(|i| {
                    let part = (b.min(i as f64 + 1.0) - a.max(i as f64)).max(0.0);
                    (i, (part / s) as f32)
                })
                .filter(|&(_, w)| w > 0.0)
                .collect();
            let sum: f32 = taps.iter().map(|t| t.1).sum();
            taps.iter_mut().for_each(|t| t.1 /= sum);
            taps
        })
        .collect()
}

/// Lanczos 3's taps from `from` up to `to`, pixel centers aligned, the
/// edges clamped.
fn lanczos_taps(from: usize, to: usize) -> Taps {
    let sinc = |x: f64| {
        if x.abs() < 1e-9 {
            1.0
        } else {
            let p = std::f64::consts::PI * x;
            p.sin() / p
        }
    };
    let s = from as f64 / to as f64;
    (0..to)
        .map(|j| {
            let u = (j as f64 + 0.5) * s - 0.5;
            let base = u.floor() as i64;
            let mut taps: Vec<(usize, f32)> = Vec::with_capacity(6);
            let mut sum = 0.0f64;
            for t in base - 2..=base + 3 {
                let d = u - t as f64;
                if d.abs() >= 3.0 {
                    continue;
                }
                let wt = sinc(d) * sinc(d / 3.0);
                let i = t.clamp(0, from as i64 - 1) as usize;
                sum += wt;
                match taps.iter_mut().find(|p| p.0 == i) {
                    Some(p) => p.1 += wt as f32,
                    None => taps.push((i, wt as f32)),
                }
            }
            taps.iter_mut().for_each(|p| p.1 /= sum as f32);
            taps
        })
        .collect()
}

/// Interleaved `channels` × `w` × `h` values, each taken through
/// `read` first, resampled by `xs` along rows and `ys` down columns:
/// as many columns and rows out as they hold taps for.
fn separable(
    data: &[f32],
    channels: usize,
    w: usize,
    h: usize,
    xs: &[Vec<(usize, f32)>],
    ys: &[Vec<(usize, f32)>],
    read: impl Fn(f32) -> f32 + Sync,
) -> Vec<f32> {
    let (tw, th) = (xs.len(), ys.len());
    let mut rows = vec![0.0f32; tw * h * channels];
    rows.par_chunks_mut(tw * channels)
        .enumerate()
        .for_each(|(y, out)| {
            let src: Vec<f32> = data[y * w * channels..(y + 1) * w * channels]
                .iter()
                .map(|&v| read(v))
                .collect();
            for (x, taps) in xs.iter().enumerate() {
                for c in 0..channels {
                    out[x * channels + c] =
                        taps.iter().map(|&(i, wt)| src[i * channels + c] * wt).sum();
                }
            }
        });
    let mut out = vec![0.0f32; tw * th * channels];
    out.par_chunks_mut(tw * channels)
        .zip(ys.par_iter())
        .for_each(|(row, taps)| {
            for &(i, wt) in taps {
                let src = &rows[i * tw * channels..(i + 1) * tw * channels];
                for (o, s) in row.iter_mut().zip(src) {
                    *o += s * wt;
                }
            }
        });
    out
}

/// Taken down from `w` × `h` to `tw` × `th` by a box filter, the
/// mean kept, each value taken through `read` first.
pub(crate) fn shrink(
    data: &[f32],
    channels: usize,
    w: usize,
    h: usize,
    tw: usize,
    th: usize,
    read: impl Fn(f32) -> f32 + Sync,
) -> Vec<f32> {
    separable(
        data,
        channels,
        w,
        h,
        &box_taps(w, tw),
        &box_taps(h, th),
        read,
    )
}

/// Brought up from `w` × `h` to `tw` × `th` by Lanczos 3, nothing
/// under zero.
#[cfg(test)]
fn grow(data: &[f32], channels: usize, w: usize, h: usize, tw: usize, th: usize) -> Vec<f32> {
    grow_box(data, channels, (w, h), (tw, th), 0..tw, 0..th)
}

/// [`grow`] for the columns `xs` and rows `ys` of the `to` size alone,
/// so a fill brought back up costs its box, not the window.
fn grow_box(
    data: &[f32],
    channels: usize,
    (w, h): (usize, usize),
    (tw, th): (usize, usize),
    xs: std::ops::Range<usize>,
    ys: std::ops::Range<usize>,
) -> Vec<f32> {
    let mut out = separable(
        data,
        channels,
        w,
        h,
        &lanczos_taps(w, tw)[xs],
        &lanczos_taps(h, th)[ys],
        |v| v,
    );
    out.iter_mut().for_each(|v| *v = v.max(0.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_factor_takes_the_longer_side_to_the_removes() {
        assert_eq!(factor_for(1024, 1024), Some(1.0));
        assert_eq!(factor_for(300, 700), Some(1.0));
        assert_eq!(factor_for(1500, 900), Some(1500.0 / 1024.0));
        assert_eq!(factor_for(900, 1500), Some(1500.0 / 1024.0));
        assert_eq!(factor_for(4096, 4096), Some(4.0));
        assert_eq!(factor_for(4097, 100), None);
        assert_eq!(factor_for(0, 100), None);
        assert_eq!(scaled_size(1500, 900, MAX), (1024, 614));
        assert_eq!(scaled_size(4096, 3000, MAX), (1024, 750));
    }

    /// A gradient with a hard edge, w × h, linear values.
    fn gradient_and_edge(w: usize, h: usize) -> Vec<f32> {
        (0..w * h)
            .flat_map(|i| {
                let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                let edge = if x > 0.6 { 0.5 } else { 0.0 };
                [
                    0.1 + 0.3 * x + edge,
                    0.2 + 0.4 * y,
                    0.05 + 0.2 * x * y + edge,
                ]
            })
            .collect()
    }

    #[test]
    fn down_and_up_keeps_the_picture_and_its_mean() {
        for (w, h, f) in [
            (1500, 900, 1500.0 / 1024.0),
            (2048, 1024, 2.0),
            (4000, 3000, 4000.0 / 1024.0),
        ] {
            let image = gradient_and_edge(w, h);
            let (sw, sh) = scaled_size(w, h, MAX);
            assert!((w as f32 / sw as f32 - f).abs() < 0.01);
            let small = shrink(&image, 3, w, h, sw, sh, |v| v);
            let back = grow(&small, 3, sw, sh, w, h);
            let mean = |v: &[f32]| v.iter().map(|&x| f64::from(x)).sum::<f64>() / v.len() as f64;
            assert!(
                (mean(&small) - mean(&image)).abs() < 1e-4,
                "{w}: the small mean"
            );
            assert!(
                (mean(&back) - mean(&image)).abs() < 1e-3,
                "{w}: the mean back"
            );
            // Away from the edge the gradient comes back to within a
            // hundredth; at the edge it is softened, not more than the
            // step itself.
            let edge = (w as f32 * 0.6) as usize;
            for y in (0..h).step_by(7) {
                for x in (0..w).step_by(5) {
                    for c in 0..3 {
                        let i = (y * w + x) * 3 + c;
                        let off = (back[i] - image[i]).abs();
                        let near = x.abs_diff(edge) <= 4 * (w / sw + 1);
                        assert!(off < if near { 0.6 } else { 0.01 }, "{w}: {x},{y}: {off}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_hole_through_the_round_trip_keeps_its_area() {
        let (w, h) = (3000, 2000);
        // A disk 400 across and a bar, the hole of a person and a pole.
        let hole: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let disk = (x - 1200.0).powi(2) + (y - 900.0).powi(2) < 200.0f32.powi(2);
                let bar = (2000.0..2060.0).contains(&x) && (300.0..1700.0).contains(&y);
                f32::from(u8::from(disk || bar))
            })
            .collect();
        let (sw, sh) = scaled_size(w, h, MAX);
        let small: Vec<f32> = shrink(&hole, 1, w, h, sw, sh, |v| v)
            .into_iter()
            .map(|c| f32::from(u8::from(c > 0.0)))
            .collect();
        let back = grow(&small, 1, sw, sh, w, h);
        let area = |v: &[f32]| v.iter().filter(|&&m| m > 0.5).count() as f64;
        let ratio = area(&back) / area(&hole);
        assert!((1.0..1.05).contains(&ratio), "{ratio}");
        // Every pixel of the hole is still in it.
        assert!(hole.iter().zip(&back).all(|(&a, &b)| a <= 0.0 || b > 0.5));
    }

    /// A run refused at the size it ran at is the window's refusal,
    /// said the same way.
    #[test]
    fn a_refused_run_is_the_windows_refusal() {
        let image = Rgbf::new(2048, 100, vec![0.5; 2048 * 100 * 3]);
        let hole = vec![1.0; 2048 * 100];
        let got = scaled(&image, &hole, MAX, |_, _| {
            Err(Error::Implausible("off by a mile".into()))
        });
        assert!(matches!(got, Err(Error::Implausible(why)) if why == "off by a mile"));
    }

    #[test]
    fn a_scaled_fill_leaves_the_outside_as_it_was() {
        let (w, h) = (2000, 1200);
        let image = Rgbf::new(
            w,
            h,
            gradient_and_edge(w, h)
                .into_iter()
                .map(|v| to_srgb(v) * 0.9 + 0.03)
                .collect(),
        );
        let hole: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                f32::from(u8::from(
                    (800..1100).contains(&x) && (400..900).contains(&y),
                ))
            })
            .collect();
        let mut seen = (0, 0);
        let out = scaled(&image, &hole, MAX, |small, small_hole| {
            seen = (small.width, small.height);
            assert_eq!(small_hole.len(), small.width * small.height);
            // A fill of flat grey under the hole.
            let mut out = small.clone();
            for (i, _) in small_hole.iter().enumerate().filter(|(_, m)| **m > 0.0) {
                out.data[i * 3..i * 3 + 3].copy_from_slice(&[0.5; 3]);
            }
            Ok(out)
        })
        .unwrap();
        assert_eq!(seen, (1024, 614));
        for (i, &m) in hole.iter().enumerate() {
            for c in 0..3 {
                let (a, b) = (out.data[i * 3 + c], image.data[i * 3 + c]);
                if m <= 0.0 {
                    assert_eq!(a.to_bits(), b.to_bits(), "{i}");
                }
            }
        }
        // And the hole holds the fill.
        let i = (650 * w + 950) * 3;
        assert!((out.data[i] - 0.5).abs() < 1e-3, "{}", out.data[i]);
    }
}
