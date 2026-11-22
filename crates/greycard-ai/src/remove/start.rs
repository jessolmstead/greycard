//! What the sampler starts from under the hole. RORem, like diffusers'
//! SDXL inpainting it is run as, starts at strength 0.99 from the
//! window's own latents noised: about a tenth of their signal is left
//! at the first of 25 Euler steps (√ᾱ 0.107 at timestep 921) and near
//! a quarter at the first of the 4-step sampler's (0.229 at 759), and
//! what is left is the low frequencies. Under a small hole that is the
//! surround's color anyway; under a large, smooth, strongly colored
//! object it is the object, which the model then paints back. In
//! training the noised latents were the target's, the object already
//! gone, so the window's own are a start the model never saw.
//!
//! So the start is made from the window with the hole filled smoothly
//! from round it first ([`prefilled`]): the push-pull interpolation of
//! Gortler et al., "The Lumigraph" (SIGGRAPH 1996), the known pixels
//! averaged down a pyramid by weight and the averages brought back up
//! bilinearly where a level knows nothing. Outside the hole the window
//! is its own, so the start is unchanged there.

use crate::image::Rgbf;

/// A level of the pyramid: its width and height, its colors
/// premultiplied by its weights, and the weights, at most 1.
type Level = (usize, usize, Vec<[f64; 3]>, Vec<f64>);

/// `image` with every pixel where `hole` is nonzero made up from the
/// pixels where it is zero, smoothly, by push-pull: the surround's
/// colors carried in, nothing of what was under the hole kept. The
/// picture unchanged where nothing is known.
pub fn prefilled(image: &Rgbf, hole: &[f32]) -> Rgbf {
    let (w, h) = (image.width, image.height);
    debug_assert_eq!(hole.len(), w * h);
    let mut levels: Vec<Level> = Vec::new();
    let weights: Vec<f64> = hole
        .iter()
        .map(|&m| f64::from(u8::from(m <= 0.0)))
        .collect();
    let colors: Vec<[f64; 3]> = (0..w * h)
        .map(|i| {
            let a = weights[i];
            std::array::from_fn(|c| f64::from(image.data[i * 3 + c]) * a)
        })
        .collect();
    levels.push((w, h, colors, weights));
    // Push: down by two until one pixel, a coarse pixel the sum of
    // the fine ones it covers, its weight held to 1.
    while let Some((lw, lh, colors, weights)) = levels.last().filter(|l| l.0 > 1 || l.1 > 1) {
        let (nw, nh) = (lw.div_ceil(2), lh.div_ceil(2));
        let mut nc = vec![[0.0f64; 3]; nw * nh];
        let mut na = vec![0.0f64; nw * nh];
        for y in 0..*lh {
            for x in 0..*lw {
                let (s, d) = (y * lw + x, (y / 2) * nw + x / 2);
                na[d] += weights[s];
                for c in 0..3 {
                    nc[d][c] += colors[s][c];
                }
            }
        }
        for (c, a) in nc.iter_mut().zip(na.iter_mut()) {
            if *a > 1.0 {
                for v in c.iter_mut() {
                    *v /= *a;
                }
                *a = 1.0;
            }
        }
        levels.push((nw, nh, nc, na));
    }
    let Some((_, _, top, a)) = levels.last() else {
        return image.clone();
    };
    if a[0] <= 0.0 {
        return image.clone();
    }
    // Pull: from the top down, each level's own colors where it knows
    // them and the coarser level's, read bilinearly, where it does not.
    let mut full: Vec<[f64; 3]> = vec![std::array::from_fn(|c| top[0][c] / a[0])];
    let (mut fw, mut fh) = (1usize, 1usize);
    for (lw, lh, colors, weights) in levels.iter().rev().skip(1) {
        let (lw, lh) = (*lw, *lh);
        let at = |x: f64, len: usize| -> (usize, usize, f64) {
            let p = (x + 0.5) / 2.0 - 0.5;
            let p = p.clamp(0.0, (len - 1) as f64);
            let i = p.floor() as usize;
            (i, (i + 1).min(len - 1), p - i as f64)
        };
        let mut next = Vec::with_capacity(lw * lh);
        for y in 0..lh {
            let (y0, y1, ty) = at(y as f64, fh);
            for x in 0..lw {
                let (x0, x1, tx) = at(x as f64, fw);
                let i = y * lw + x;
                let a = weights[i];
                next.push(std::array::from_fn(|c| {
                    let top = full[y0 * fw + x0][c] * (1.0 - tx) + full[y0 * fw + x1][c] * tx;
                    let bottom = full[y1 * fw + x0][c] * (1.0 - tx) + full[y1 * fw + x1][c] * tx;
                    colors[i][c] + (1.0 - a) * (top * (1.0 - ty) + bottom * ty)
                }));
            }
        }
        full = next;
        (fw, fh) = (lw, lh);
    }
    let mut data = image.data.clone();
    for (i, _) in hole.iter().enumerate().filter(|(_, m)| **m > 0.0) {
        for c in 0..3 {
            data[i * 3 + c] = full[i][c] as f32;
        }
    }
    Rgbf::new(w, h, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: usize, h: usize, f: impl Fn(usize, usize) -> [f32; 3]) -> Rgbf {
        let mut data = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&f(x, y));
            }
        }
        Rgbf::new(w, h, data)
    }

    fn disk(w: usize, h: usize, cx: f32, cy: f32, r: f32) -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 - cx, (i / w) as f32 - cy);
                f32::from(u8::from(x * x + y * y <= r * r))
            })
            .collect()
    }

    /// Whatever was under the hole, a flat surround fills it flat: the
    /// orange under it is gone, and the rest is the picture's own.
    #[test]
    fn a_flat_surround_fills_the_hole_flat() {
        let (w, h) = (90, 70);
        let hole = disk(w, h, 70.0, 55.0, 30.0);
        let image = picture(w, h, |x, y| {
            if hole[y * w + x] > 0.0 {
                [0.95, 0.42, 0.06]
            } else {
                [0.2, 0.4, 0.5]
            }
        });
        let out = prefilled(&image, &hole);
        for (i, &m) in hole.iter().enumerate() {
            let (got, was) = (&out.data[i * 3..i * 3 + 3], &image.data[i * 3..i * 3 + 3]);
            if m > 0.0 {
                for (g, want) in got.iter().zip([0.2, 0.4, 0.5]) {
                    assert!((g - want).abs() < 1e-5, "{i}: {got:?}");
                }
            } else {
                assert_eq!(got, was);
            }
        }
    }

    /// Across a gradient the fill is smooth and follows it: no pixel of
    /// it farther from the gradient than a tenth of its span, and none
    /// from its neighbor by more than a few of the gradient's steps.
    #[test]
    fn a_fill_follows_a_gradient_smoothly() {
        let (w, h) = (128, 96);
        let hole = disk(w, h, 60.0, 50.0, 25.0);
        let ramp = |x: usize, y: usize| [x as f32 / w as f32, y as f32 / h as f32, 0.5];
        let image = picture(w, h, |x, y| {
            if hole[y * w + x] > 0.0 {
                [1.0, 0.0, 0.0]
            } else {
                ramp(x, y)
            }
        });
        let out = prefilled(&image, &hole);
        let mut worst = 0.0f32;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if hole[i] <= 0.0 {
                    continue;
                }
                for (c, want) in ramp(x, y).into_iter().enumerate() {
                    worst = worst.max((out.data[i * 3 + c] - want).abs());
                }
                if x + 1 < w {
                    for c in 0..3 {
                        let step = (out.data[(i + 1) * 3 + c] - out.data[i * 3 + c]).abs();
                        assert!(step < 0.03, "({x}, {y}) channel {c}: a step of {step}");
                    }
                }
            }
        }
        assert!(worst < 0.1, "{worst}");
    }

    /// Nothing known, nothing to fill from: the picture comes back.
    #[test]
    fn a_hole_over_everything_is_left_alone() {
        let image = picture(20, 10, |x, y| [x as f32 / 20.0, y as f32 / 10.0, 0.3]);
        let out = prefilled(&image, &[1.0; 200]);
        assert_eq!(out.data, image.data);
    }
}
