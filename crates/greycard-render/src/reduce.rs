//! A developed picture box-averaged by a whole factor each way, in
//! linear light: from the working image, and from rows of a viewport
//! texture as the GPU gives them back, which agree.

use greycard_core::image::WorkingImage;
use rayon::prelude::*;

/// `image` box-averaged by `factor` each way, in linear light; the
/// pixels past the last whole block are left out.
pub fn reduce(image: &WorkingImage, factor: usize) -> WorkingImage {
    let factor = factor.max(1);
    let (w, h) = (
        (image.width / factor).max(1),
        (image.height / factor).max(1),
    );
    let mut data = vec![0.0f32; w * h * 3];
    let n = (factor * factor) as f32;
    data.par_chunks_mut(w * 3).enumerate().for_each(|(y, out)| {
        for (x, px) in out.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let mut sum = [0.0f32; 3];
            for sy in y * factor..((y + 1) * factor).min(image.height) {
                let row = &image.data[sy * image.width * 3..][..image.width * 3];
                for sx in x * factor..((x + 1) * factor).min(image.width) {
                    for (s, v) in sum.iter_mut().zip(&row[sx * 3..sx * 3 + 3]) {
                        *s += v;
                    }
                }
            }
            *px = sum.map(|s| s / n);
        }
    });
    WorkingImage {
        width: w,
        height: h,
        data,
    }
}

/// Rows of a viewport texture as the GPU gives them back (RGBA half
/// floats, eight bytes a texel), `width` texels each and a whole number
/// of `factor`s of them, box-averaged by `factor` into `out`, its alpha
/// dropped.
pub fn reduce_halves(rows: &[u8], width: usize, factor: usize, out: &mut Vec<f32>) {
    let line = width * 8;
    let bands = rows.len() / line / factor;
    let w = width / factor;
    let n = (factor * factor) as f32;
    let start = out.len();
    out.resize(start + bands * w * 3, 0.0);
    out[start..]
        .par_chunks_mut(w * 3)
        .enumerate()
        .for_each(|(band, o)| {
            for (x, px) in o.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let mut sum = [0.0f32; 3];
                for r in 0..factor {
                    let row = &rows[(band * factor + r) * line..][..line];
                    for sx in x * factor..(x + 1) * factor {
                        let texel = &row[sx * 8..sx * 8 + 6];
                        for (c, s) in sum.iter_mut().enumerate() {
                            let bits = u16::from_le_bytes([texel[c * 2], texel[c * 2 + 1]]);
                            *s += half::f16::from_bits(bits).to_f32();
                        }
                    }
                }
                *px = sum.map(|s| s / n);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reduction_averages_whole_blocks_and_the_halves_agree() {
        let (w, h) = (7, 5);
        let data: Vec<f32> = (0..w * h * 3).map(|i| (i % 11) as f32 * 0.125).collect();
        let image = WorkingImage {
            width: w,
            height: h,
            data,
        };
        let r = reduce(&image, 2);
        assert_eq!((r.width, r.height), (3, 2));
        let at = |x: usize, y: usize, c: usize| image.data[(y * w + x) * 3 + c];
        let want = (at(2, 2, 1) + at(3, 2, 1) + at(2, 3, 1) + at(3, 3, 1)) / 4.0;
        assert!((r.data[(3 + 1) * 3 + 1] - want).abs() < 1e-6);
        // The same picture as the viewport's texture holds it.
        let mut halves = Vec::new();
        for y in 0..4 {
            for x in 0..w {
                for c in 0..4 {
                    let v = if c == 3 { 1.0 } else { at(x, y, c) };
                    halves.extend_from_slice(&half::f16::from_f32(v).to_bits().to_le_bytes());
                }
            }
        }
        let mut out = Vec::new();
        reduce_halves(&halves, w, 2, &mut out);
        assert_eq!(out.len(), r.data.len());
        for (a, b) in out.iter().zip(&r.data) {
            assert!((a - b).abs() < 1e-3, "{a} against {b}");
        }
        assert_eq!(reduce(&image, 1).data, image.data);
    }
}
