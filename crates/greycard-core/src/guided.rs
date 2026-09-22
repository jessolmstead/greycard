//! The guided filter of He, Sun and Tang (ECCV 2010): an output that
//! is a local linear function of a guide image, so where the guide has
//! an edge the output snaps to it and where the guide is flat the
//! output is smoothed. Every window is a box, so the cost is the same
//! whatever the radius.
//!
//! It lives here, in the engine, because two consumers want it and
//! neither may depend on the other: the learned masks' refinement in
//! `greycard-ai` (§34) and the tone equalizer's guide plane in the
//! editor's tone equalizer. Nothing here knows about either.

use rayon::prelude::*;

/// The mean over a (2r+1)² window clamped to the image, by running
/// sums along rows then columns.
///
/// The running sums are the definition, not an optimization: a window
/// is the difference of two prefix sums, so the value a pixel gets
/// depends on the order the row (or the column) was summed in. Both
/// passes are parallel over whole rows and whole columns, which leaves
/// every sum in its own order, so the answer is the serial one's bits.
pub fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    assert_eq!(src.len(), w * h);
    let mut rows = vec![0.0f32; w * h];
    rows.par_chunks_mut(w)
        .zip(src.par_chunks(w))
        .for_each(|(out, row)| {
            let mut sum = vec![0.0f32; w + 1];
            for (x, v) in row.iter().enumerate() {
                sum[x + 1] = sum[x] + v;
            }
            for (x, o) in out.iter_mut().enumerate() {
                let a = x.saturating_sub(r);
                let b = (x + r + 1).min(w);
                *o = (sum[b] - sum[a]) / (b - a) as f32;
            }
        });
    // Down the columns. The prefix sum of a column has to be walked in
    // one piece to keep the bits, but the difference of two of them is
    // a row at a time, so the plane is read in order: `cols` holds the
    // whole prefix, row y + 1 being the sum of rows 0..=y.
    let mut cols = vec![0.0f32; w * (h + 1)];
    for y in 0..h {
        let (prev, next) = cols.split_at_mut((y + 1) * w);
        let prev = &prev[y * w..];
        next[..w]
            .iter_mut()
            .zip(prev.iter().zip(&rows[y * w..(y + 1) * w]))
            .for_each(|(o, (p, v))| *o = p + v);
    }
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, line)| {
        let a = y.saturating_sub(r);
        let b = (y + r + 1).min(h);
        let n = (b - a) as f32;
        let (lo, hi) = (&cols[a * w..(a + 1) * w], &cols[b * w..(b + 1) * w]);
        for (o, (l, u)) in line.iter_mut().zip(lo.iter().zip(hi)) {
            *o = (u - l) / n;
        }
    });
    out
}

/// `p` filtered against the guide `i`, both planes `w`×`h`, with
/// window radius `r` and regularizer `eps`. `p` of `None` filters the
/// guide against itself, which is the edge-preserving smoother: the
/// detail it removes is what varies by less than the square root of
/// `eps` inside a window.
///
/// The output is unclamped; a mask wants it clamped to 0..1 and a
/// tone guide does not.
pub fn filter(i: &[f32], p: Option<&[f32]>, w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    assert_eq!(i.len(), w * h);
    let n = w * h;
    let mean_i = box_mean(i, w, h, r);
    let ii: Vec<f32> = i.par_iter().map(|v| v * v).collect();
    let corr_i = box_mean(&ii, w, h, r);
    // Guiding by the picture itself: the mean and the correlation of
    // `p` are the guide's own, to the bit, so they are not taken twice.
    let (mean_p, corr_ip) = match p {
        None => (None, None),
        Some(p) => {
            assert_eq!(p.len(), n);
            let ip: Vec<f32> = i.par_iter().zip(p).map(|(a, b)| a * b).collect();
            (Some(box_mean(p, w, h, r)), Some(box_mean(&ip, w, h, r)))
        }
    };
    let mean_p = mean_p.as_deref().unwrap_or(&mean_i);
    let corr_ip = corr_ip.as_deref().unwrap_or(&corr_i);
    let mut a = vec![0.0f32; n];
    let mut b = vec![0.0f32; n];
    a.par_iter_mut()
        .zip(b.par_iter_mut())
        .enumerate()
        .for_each(|(k, (a, b))| {
            let var_i = corr_i[k] - mean_i[k] * mean_i[k];
            let cov_ip = corr_ip[k] - mean_i[k] * mean_p[k];
            *a = cov_ip / (var_i + eps);
            *b = mean_p[k] - *a * mean_i[k];
        });
    let mean_a = box_mean(&a, w, h, r);
    let mean_b = box_mean(&b, w, h, r);
    mean_a
        .par_iter()
        .zip(&mean_b)
        .zip(i)
        .map(|((a, b), i)| a * i + b)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serial form the running sums are defined by, so the
    /// parallel one above is held to its bits.
    fn serial_box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
        let mut rows = vec![0.0f32; w * h];
        let mut sum = vec![0.0f32; w + 1];
        for y in 0..h {
            let row = &src[y * w..(y + 1) * w];
            for (x, v) in row.iter().enumerate() {
                sum[x + 1] = sum[x] + v;
            }
            for x in 0..w {
                let a = x.saturating_sub(r);
                let b = (x + r + 1).min(w);
                rows[y * w + x] = (sum[b] - sum[a]) / (b - a) as f32;
            }
        }
        let mut out = vec![0.0f32; w * h];
        let mut col = vec![0.0f32; h + 1];
        for x in 0..w {
            for y in 0..h {
                col[y + 1] = col[y] + rows[y * w + x];
            }
            for y in 0..h {
                let a = y.saturating_sub(r);
                let b = (y + r + 1).min(h);
                out[y * w + x] = (col[b] - col[a]) / (b - a) as f32;
            }
        }
        out
    }

    #[test]
    fn a_box_mean_is_a_mean_and_keeps_a_flat_field() {
        let flat = vec![0.3f32; 20 * 10];
        assert!(
            box_mean(&flat, 20, 10, 3)
                .iter()
                .all(|v| (v - 0.3).abs() < 1e-6)
        );
        let mut one = vec![0.0f32; 5 * 5];
        one[12] = 1.0;
        let m = box_mean(&one, 5, 5, 1);
        assert!((m[12] - 1.0 / 9.0).abs() < 1e-6);
        assert!((m[0] - 0.0).abs() < 1e-6);
        // At a corner the window is 2x2: the corner value spreads over four.
        one.fill(0.0);
        one[0] = 1.0;
        assert!((box_mean(&one, 5, 5, 1)[0] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn the_parallel_box_mean_is_the_serial_ones_bits() {
        // Values with no exact sum, so an order that differed would show.
        let (w, h) = (137usize, 83usize);
        let src: Vec<f32> = (0..w * h)
            .map(|k| ((k * 2654435761usize) % 1000) as f32 / 7.0 - 60.0)
            .collect();
        for r in [1usize, 5, 40, 200] {
            assert_eq!(box_mean(&src, w, h, r), serial_box_mean(&src, w, h, r));
        }
    }

    #[test]
    fn a_mask_snaps_to_the_guides_edge() {
        // A luma step at x = 32 of 64; a mask that is a wide ramp there.
        let (w, h) = (64usize, 16usize);
        let guide: Vec<f32> = (0..w * h)
            .map(|k| if k % w < 32 { 0.2 } else { 0.8 })
            .collect();
        let ramp: Vec<f32> = (0..w * h)
            .map(|k| ((k % w) as f32 - 20.0) / 24.0)
            .map(|v| v.clamp(0.0, 1.0))
            .collect();
        let out = filter(&guide, Some(&ramp), w, h, 6, 1e-4);
        // Sharper: the values two texels either side of the edge are
        // further apart than the ramp's.
        let before = ramp[8 * w + 34] - ramp[8 * w + 29];
        let after = out[8 * w + 34] - out[8 * w + 29];
        assert!(after > before + 0.1, "before {before} after {after}");
    }

    #[test]
    fn a_flat_guide_smooths_the_mask() {
        let (w, h) = (32usize, 8usize);
        let guide = vec![0.5f32; w * h];
        let mut spiky = vec![0.0f32; w * h];
        spiky[4 * w + 16] = 1.0;
        let out = filter(&guide, Some(&spiky), w, h, 2, 1e-3);
        assert!(out[4 * w + 16] < 0.2 && out[4 * w + 16] > 0.0);
        assert!(out[4 * w + 15] > 0.0);
    }

    #[test]
    fn guiding_by_the_picture_itself_is_the_same_as_passing_it_twice() {
        let (w, h) = (48usize, 32usize);
        let i: Vec<f32> = (0..w * h)
            .map(|k| ((k % w) as f32 / 8.0).sin() + ((k / w) as f32 / 5.0).cos())
            .collect();
        assert_eq!(
            filter(&i, None, w, h, 4, 0.05),
            filter(&i, Some(&i), w, h, 4, 0.05)
        );
    }

    #[test]
    fn a_self_guided_filter_keeps_a_step_and_loses_a_speck() {
        // A step of two across the middle, with a one-texel speck of
        // three either side of it.
        let (w, h) = (128usize, 64usize);
        let mut src: Vec<f32> = (0..w * h)
            .map(|k| if k % w < 64 { 0.0 } else { 2.0 })
            .collect();
        src[20 * w + 20] = 3.0;
        let out = filter(&src, None, w, h, 12, 0.25);
        // The step survives: the two sides are still nearly two apart.
        let across = out[40 * w + 90] - out[40 * w + 30];
        assert!(across > 1.8, "{across}");
        // The speck does not: it keeps under a tenth of its height.
        assert!(out[20 * w + 20] < 0.3, "{}", out[20 * w + 20]);
    }
}
