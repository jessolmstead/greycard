//! The export's resize: a separable Lanczos3 over float planes that
//! keeps every value, over one and under zero.
//!
//! Ported from the image crate's `imageops::resize` with
//! `FilterType::Lanczos3` (image 0.25, `src/imageops/sample.rs`:
//! `vertical_sample`, `horizontal_sample`, `lanczos3_kernel`; copyright
//! the image-rs developers, MIT OR Apache-2.0). The kernel, the window
//! of source pixels each output pixel reads, the weights' sum and their
//! division by it, the pass order (down the columns first, then along
//! the rows) and each sum's order are theirs, so a picture whose resize
//! stays inside 0..1 comes out to the bit. What is ours: no clamp (the
//! crate holds a float to 0..1, its range for one, which took a scene's
//! light over one and every negative band value to the edge), any
//! number of channels, the rows run in parallel, and a sum taken across
//! a row of the source at once rather than down one column at a time,
//! which adds each output's terms in the same order.
//!
//! The image crate's notice, taken under its MIT license:
//!
//! > MIT License
//! >
//! > Permission is hereby granted, free of charge, to any
//! > person obtaining a copy of this software and associated
//! > documentation files (the "Software"), to deal in the
//! > Software without restriction, including without
//! > limitation the rights to use, copy, modify, merge,
//! > publish, distribute, sublicense, and/or sell copies of
//! > the Software, and to permit persons to whom the Software
//! > is furnished to do so, subject to the following
//! > conditions:
//! >
//! > The above copyright notice and this permission notice
//! > shall be included in all copies or substantial portions
//! > of the Software.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
//! > ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
//! > TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
//! > PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
//! > SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
//! > CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
//! > OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
//! > IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
//! > DEALINGS IN THE SOFTWARE.

use rayon::prelude::*;

/// The Lanczos window, in source pixels at one to one.
const SUPPORT: f32 = 3.0;

fn sinc(t: f32) -> f32 {
    let a = t * std::f32::consts::PI;
    if t == 0.0 { 1.0 } else { a.sin() / a }
}

fn lanczos3(x: f32) -> f32 {
    if x.abs() < SUPPORT {
        sinc(x) * sinc(x / SUPPORT)
    } else {
        0.0
    }
}

/// For each of `to` output positions along an axis of `from` source
/// pixels: the first source pixel it reads and the weights of it and
/// those after it, summing to one.
fn taps(from: usize, to: usize) -> Vec<(usize, Vec<f32>)> {
    let ratio = from as f32 / to as f32;
    let sratio = if ratio < 1.0 { 1.0 } else { ratio };
    let reach = SUPPORT * sratio;
    (0..to)
        .map(|out| {
            let input = (out as f32 + 0.5) * ratio;
            let left = ((input - reach).floor() as i64).clamp(0, from as i64 - 1);
            let right = ((input + reach).ceil() as i64).clamp(left + 1, from as i64);
            let input = input - 0.5;
            let mut ws: Vec<f32> = (left..right)
                .map(|i| lanczos3((i as f32 - input) / sratio))
                .collect();
            let mut sum = 0.0;
            for w in &ws {
                sum += w;
            }
            for w in &mut ws {
                *w /= sum;
            }
            (left as usize, ws)
        })
        .collect()
}

/// `data`, `width` by `height` pixels of `channels` interleaved floats
/// (one or three), resampled to `w` by `h`.
pub fn resize(
    data: &[f32],
    width: usize,
    height: usize,
    channels: usize,
    w: usize,
    h: usize,
) -> Vec<f32> {
    assert_eq!(data.len(), width * height * channels, "a plane of its size");
    match channels {
        1 => resize_of::<1>(data, width, height, w, h),
        3 => resize_of::<3>(data, width, height, w, h),
        _ => panic!("{channels} channels: one or three"),
    }
}

fn resize_of<const C: usize>(
    data: &[f32],
    width: usize,
    height: usize,
    w: usize,
    h: usize,
) -> Vec<f32> {
    if width == 0 || height == 0 || w == 0 || h == 0 {
        return vec![0.0; w * h * C];
    }
    let row = width * C;
    // Down the columns: `h` rows of the source's width.
    let mut tall = vec![0.0f32; h * row];
    let down = taps(height, h);
    tall.par_chunks_mut(row)
        .zip(down.par_iter())
        .for_each(|(out, (top, ws))| {
            for (i, w) in ws.iter().enumerate() {
                let src = &data[(top + i) * row..][..row];
                for (o, s) in out.iter_mut().zip(src) {
                    *o += s * w;
                }
            }
        });
    // Along the rows.
    let across = taps(width, w);
    let mut out = vec![0.0f32; h * w * C];
    out.par_chunks_mut(w * C)
        .zip(tall.par_chunks(row))
        .for_each(|(out, src)| {
            let src = src.as_chunks::<C>().0;
            for (px, (left, ws)) in out.as_chunks_mut::<C>().0.iter_mut().zip(&across) {
                for (s, wt) in src[*left..].iter().zip(ws) {
                    for c in 0..C {
                        px[c] += s[c] * wt;
                    }
                }
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values well inside 0..1, so the crate's clamp never bites: a
    /// smooth field with fine detail on it, hashed so no two channels
    /// or pixels repeat.
    fn inside(width: usize, height: usize, channels: usize) -> Vec<f32> {
        (0..width * height * channels)
            .map(|i| {
                let (p, c) = (i / channels, i % channels);
                let (x, y) = ((p % width) as f32, (p / width) as f32);
                let mut z = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                z ^= z >> 29;
                let fine = (z % 1000) as f32 / 1000.0;
                0.35 + 0.1 * (x / 7.0 + c as f32).sin() * (y / 5.0).cos() + 0.1 * fine
            })
            .collect()
    }

    fn crate_rgb(data: &[f32], width: usize, height: usize, w: usize, h: usize) -> Vec<f32> {
        let src: image::ImageBuffer<image::Rgb<f32>, &[f32]> =
            image::ImageBuffer::from_raw(width as u32, height as u32, data).unwrap();
        image::imageops::resize(
            &src,
            w as u32,
            h as u32,
            image::imageops::FilterType::Lanczos3,
        )
        .into_raw()
    }

    fn crate_luma(data: &[f32], width: usize, height: usize, w: usize, h: usize) -> Vec<f32> {
        let src: image::ImageBuffer<image::Luma<f32>, &[f32]> =
            image::ImageBuffer::from_raw(width as u32, height as u32, data).unwrap();
        image::imageops::resize(
            &src,
            w as u32,
            h as u32,
            image::imageops::FilterType::Lanczos3,
        )
        .into_raw()
    }

    /// Inside 0..1 the resize is the crate's to the bit, three channels
    /// and one, down by whole and broken ratios, up, and one axis left
    /// as it is.
    #[test]
    fn inside_the_range_it_is_the_crates_to_the_bit() {
        let (width, height) = (97, 61);
        for (w, h) in [(48, 30), (31, 20), (13, 9), (97, 30), (40, 61), (150, 90)] {
            let rgb = inside(width, height, 3);
            assert_eq!(
                resize(&rgb, width, height, 3, w, h),
                crate_rgb(&rgb, width, height, w, h),
                "{w}x{h}"
            );
            let one = inside(width, height, 1);
            assert_eq!(
                resize(&one, width, height, 1, w, h),
                crate_luma(&one, width, height, w, h),
                "{w}x{h}, one channel"
            );
        }
    }

    /// Over one and under zero come through: a field lifted by two and
    /// one lowered by two are the field's resize moved by two, where the
    /// crate held each to the edge of 0..1.
    #[test]
    fn light_over_one_and_values_under_zero_are_kept() {
        let (width, height, w, h) = (64, 40, 21, 13);
        let field = inside(width, height, 3);
        let base = resize(&field, width, height, 3, w, h);
        for lift in [2.0f32, -2.0] {
            let moved: Vec<f32> = field.iter().map(|v| v + lift).collect();
            let out = resize(&moved, width, height, 3, w, h);
            let most = out
                .iter()
                .zip(&base)
                .map(|(a, b)| (a - b - lift).abs())
                .fold(0.0f32, f32::max);
            assert!(most < 1e-5, "{lift}: {most}");
            let held = crate_rgb(&moved, width, height, w, h);
            assert!(
                held.iter()
                    .all(|&v| v == if lift > 0.0 { 1.0 } else { 0.0 })
            );
        }
    }
}
