//! A reduced develop's input (SPEC.md, "A reduced develop's input"):
//! the picture and its codes box-downsampled by an integer factor, as
//! the trial's look-scaled runs made them and the f = 3 and f = 2.5
//! fixtures were made. The fill then runs on them at the look scale f.

use rayon::prelude::*;

/// The look scale f of a reduced picture: the full frame's long edge
/// over the picture's, unrounded, from the two sizes in pixels. One
/// function, so the engine's stage and the tests take the same f.
/// A binned develop's picture drops the ragged edge, so an odd frame's
/// f is a little over the bin (4001 over 2000 at bin 2).
///
/// # Panics
/// If the picture is empty.
pub fn look_scale(full: (usize, usize), reduced: (usize, usize)) -> f64 {
    let small = reduced.0.max(reduced.1);
    assert!(small > 0, "a reduced picture of some size");
    full.0.max(full.1) as f64 / small as f64
}

/// `down(x, n)`: `x` (`width` × `height`, RGB interleaved) with the
/// ragged edge dropped (the first ⌊H/n⌋·n rows and ⌊W/n⌋·n columns
/// kept), each n×n block's mean in f32: its pixels summed in raster
/// order from the first, then divided by n², numpy's float32 mean.
/// Returns the picture and its size, ⌊W/n⌋ × ⌊H/n⌋.
///
/// # Panics
/// If `n` is 0 or `x` is not `width · height · 3` long.
pub fn down(x: &[f32], width: usize, height: usize, n: usize) -> (Vec<f32>, usize, usize) {
    assert!(n > 0, "a factor of at least 1");
    assert_eq!(x.len(), width * height * 3, "x is RGB interleaved");
    let (w, h) = (width / n, height / n);
    let count = (n * n) as f32;
    let mut out = vec![0.0f32; w * h * 3];
    if w > 0 {
        out.par_chunks_mut(w * 3).enumerate().for_each(|(by, row)| {
            for (bx, o) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let mut sum = [0.0f32; 3];
                for (k, (y, px)) in (by * n..(by + 1) * n)
                    .flat_map(|y| (bx * n..(bx + 1) * n).map(move |px| (y, px)))
                    .enumerate()
                {
                    let p = 3 * (y * width + px);
                    for c in 0..3 {
                        sum[c] = if k == 0 { x[p + c] } else { sum[c] + x[p + c] };
                    }
                }
                *o = sum.map(|s| s / count);
            }
        });
    }
    (out, w, h)
}

/// `down_codes(codes, n)`: a code a pixel (`width` × `height`,
/// row-major) pooled to ⌊W/n⌋ × ⌊H/n⌋ with k = code & 0x7F and the
/// ragged edge dropped: 3 where more than half the block's pixels have
/// k = 3, else the block's largest k capped at 2 (0 when nothing in it
/// is clipped). Bit 7 is not carried.
///
/// # Panics
/// If `n` is 0 or `codes` is not `width · height` long.
pub fn down_codes(codes: &[u8], width: usize, height: usize, n: usize) -> Vec<u8> {
    assert!(n > 0, "a factor of at least 1");
    assert_eq!(codes.len(), width * height, "a code a pixel");
    let (w, h) = (width / n, height / n);
    (0..w * h)
        .map(|b| {
            let (bx, by) = (b % w, b / w);
            let (mut core, mut most) = (0usize, 0u8);
            for y in by * n..(by + 1) * n {
                for &c in &codes[y * width + bx * n..y * width + (bx + 1) * n] {
                    let k = c & 0x7F;
                    core += (k == 3) as usize;
                    most = most.max(k);
                }
            }
            if 2 * core > n * n { 3 } else { most.min(2) }
        })
        .collect()
}
