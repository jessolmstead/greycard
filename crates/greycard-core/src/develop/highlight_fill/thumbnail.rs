//! The context encoder's input (SPEC.md stage 7): the input box-averaged
//! to [`THUMB_LONG`](super::THUMB_LONG) px on its long edge, in five
//! channels. The editor builds it from the fill's own input, at f > 1
//! the reduced one (hl-1).

use rayon::prelude::*;

use super::half::{PixelCodes, PixelSource, Plain};

/// A thumbnail, `height` × `width` × 5, channels last: log2 of each
/// channel's mean (negatives and non-finite values as 0, floor 2⁻¹²),
/// the share of pixels with k > 0, and the share with k = 3.
#[derive(Debug, Clone, PartialEq)]
pub struct Thumbnail {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

/// The thumbnail of `x` (`width` × `height`, RGB interleaved) at a long
/// edge of `long`: an integer factor f_t = max(⌈long edge / long⌉, 1),
/// ⌈W/f_t⌉ × ⌈H/f_t⌉ blocks, the last row and column of them possibly
/// short. Each block sums in f64 in raster order, so the thumbnail is
/// the oracle's to its log2's ulp. Rows of blocks in parallel.
pub fn thumbnail(
    x: &[f32],
    width: usize,
    height: usize,
    codes: &impl PixelCodes,
    long: usize,
) -> Thumbnail {
    thumbnail_of(&Plain::new(x, width, height), codes, long)
}

/// [`thumbnail`] of any source.
pub fn thumbnail_of(x: &impl PixelSource, codes: &impl PixelCodes, long: usize) -> Thumbnail {
    let (width, height) = x.size();
    let f = width.max(height).div_ceil(long).max(1);
    let (tw, th) = (width.div_ceil(f), height.div_ceil(f));
    let mut data = vec![0.0f32; tw * th * 5];
    if tw > 0 {
        data.par_chunks_mut(tw * 5)
            .enumerate()
            .for_each(|(ty, row)| {
                for tx in 0..tw {
                    let mut acc = [0.0f64; 3];
                    let (mut any, mut core) = (0u64, 0u64);
                    let ys = ty * f..((ty + 1) * f).min(height);
                    let xs = tx * f..((tx + 1) * f).min(width);
                    let n = (ys.len() * xs.len()) as f64;
                    for y in ys {
                        for px in xs.clone() {
                            let rgb = x.rgb(px, y);
                            for (a, v) in acc.iter_mut().zip(rgb) {
                                *a += if v.is_finite() {
                                    (v as f64).max(0.0)
                                } else {
                                    0.0
                                };
                            }
                            let k = codes.code(px, y) & 0x7F;
                            any += (k > 0) as u64;
                            core += (k == 3) as u64;
                        }
                    }
                    let out = &mut row[tx * 5..tx * 5 + 5];
                    for c in 0..3 {
                        out[c] = ((acc[c] / n) as f32).max(1.0 / 4096.0).log2();
                    }
                    out[3] = (any as f64 / n) as f32;
                    out[4] = (core as f64 / n) as f32;
                }
            });
    }
    Thumbnail {
        width: tw,
        height: th,
        data,
    }
}
