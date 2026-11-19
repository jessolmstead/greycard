//! The harmonic continuation of the rim's color into the regions
//! (SPEC.md stage 6): Jacobi at a quarter of the half size.

use rayon::prelude::*;

/// The continuation at a quarter of the half grid (an eighth of full
/// size): `width` = ⌊Wh/4⌋ by `height` = ⌊Hh/4⌋, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct Harmonic {
    pub width: usize,
    pub height: usize,
    /// More than a quarter of the 4×4 block was fixed (five or more of
    /// its sixteen cells).
    pub known: Vec<bool>,
    /// The block's mean ab over its fixed cells.
    pub val: Vec<[f32; 2]>,
    /// The iteration's start: val where known, elsewhere the mean of
    /// val over the known cells (0 when none is known).
    pub start: Vec<[f32; 2]>,
    /// After the iterations.
    pub cur: Vec<[f32; 2]>,
}

/// The cells the continuation is fixed on: outside every region and
/// brighter than `bright` (the look's `harmonic_bright`, 0.25 in hl-1)
/// by the raw yh, not by exp2(ly) as the features judge brightness (SPEC
/// stage 6, and its trial behavior 1).
pub fn fixed(cells: &[super::half::Cell], yh: &[f32], bright: f32) -> Vec<bool> {
    cells
        .iter()
        .zip(yh)
        .map(|(c, &y)| !c.fa() && y > bright)
        .collect()
}

/// The quarter grid's known cells, values, and start from abh and the
/// fixed cells on the (`wh`, `hh`) half grid: 4×4 average pools that
/// drop the last Hh % 4 rows and Wh % 4 columns, each a sum of sixteen
/// in raster order over 16, as torch's CPU `avg_pool2d`.
pub fn start(abh: &[[f32; 2]], fixed: &[bool], wh: usize, hh: usize) -> Harmonic {
    let (w, h) = (wh / 4, hh / 4);
    let mut known = vec![false; w * h];
    let mut val = vec![[0.0f32; 2]; w * h];
    let mut sum = [0.0f64; 2];
    let mut count = 0u64;
    for qy in 0..h {
        for qx in 0..w {
            let mut q = [0.0f32; 2];
            let mut n = 0u32;
            for y in 4 * qy..4 * qy + 4 {
                for x in 4 * qx..4 * qx + 4 {
                    let i = y * wh + x;
                    let f = fixed[i] as u32 as f32;
                    q[0] += abh[i][0] * f;
                    q[1] += abh[i][1] * f;
                    n += fixed[i] as u32;
                }
            }
            let fq = n as f32 / 16.0;
            let j = qy * w + qx;
            known[j] = fq > 0.25;
            let q = q.map(|s| s / 16.0);
            val[j] = q.map(|s| s / fq.max(1e-6));
            if known[j] {
                sum[0] += val[j][0] as f64;
                sum[1] += val[j][1] as f64;
                count += 1;
            }
        }
    }
    // torch's f32 sum over the plane is within an ulp of the exact one,
    // which f64 gives here; then divided in f32 by the count.
    let mean = sum.map(|s| s as f32 / count.max(1) as f32);
    let start: Vec<[f32; 2]> = known
        .iter()
        .zip(&val)
        .map(|(&k, &v)| if k { v } else { mean })
        .collect();
    Harmonic {
        width: w,
        height: h,
        known,
        val,
        cur: start.clone(),
        start,
    }
}

/// `iters` Jacobi rounds: a cell not known takes the mean of the 3×3
/// around it, the edge replicated, summed in raster order over 9 (torch's
/// `avg_pool2d` on the padded plane); a known cell keeps val. Rows in
/// parallel, each cell alone, so the same on any thread count.
pub fn iterate(h: &mut Harmonic, iters: usize) {
    let (w, ht) = (h.width, h.height);
    if w == 0 || ht == 0 {
        return;
    }
    let mut next = h.cur.clone();
    for _ in 0..iters {
        let cur = &h.cur;
        next.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let ys = [y.saturating_sub(1), y, (y + 1).min(ht - 1)];
            for (x, out) in row.iter_mut().enumerate() {
                let i = y * w + x;
                if h.known[i] {
                    *out = h.val[i];
                    continue;
                }
                let xs = [x.saturating_sub(1), x, (x + 1).min(w - 1)];
                let mut s = [0.0f32; 2];
                for &yy in &ys {
                    for &xx in &xs {
                        let v = cur[yy * w + xx];
                        s[0] += v[0];
                        s[1] += v[1];
                    }
                }
                *out = s.map(|v| v / 9.0);
            }
        });
        std::mem::swap(&mut h.cur, &mut next);
    }
}

/// One output index's source pair and weights under torch's bilinear
/// resize by size, align_corners False: src = (dst + 0.5)·(n_in/n_out)
/// − 0.5 in f32, clamped at 0; i0 = ⌊src⌋, i1 = min(i0 + 1, n_in − 1).
#[inline]
pub fn source(dst: usize, n_in: usize, scale: f32) -> (usize, usize, f32, f32) {
    let src = ((dst as f32 + 0.5) * scale - 0.5).max(0.0);
    let i0 = (src.floor() as usize).min(n_in - 1);
    let l1 = (src - i0 as f32).clamp(0.0, 1.0);
    let i1 = if i0 < n_in - 1 { i0 + 1 } else { i0 };
    (i0, i1, 1.0 - l1, l1)
}

impl Harmonic {
    /// The continuation at half-size cell (`x`, `y`) of the (`wh`,
    /// `hh`) grid: the bilinear resize of [`Harmonic::cur`], as
    /// (w0·v00 + w1·v01)·h0 + (w0·v10 + w1·v11)·h1, torch's CPU order.
    /// 0 when the quarter grid is empty (fewer than four cells across
    /// or down, where the oracle's pool would refuse).
    pub fn at(&self, x: usize, y: usize, wh: usize, hh: usize) -> [f32; 2] {
        if self.width == 0 || self.height == 0 {
            return [0.0; 2];
        }
        let (y0, y1, h0, h1) = source(y, self.height, self.height as f32 / hh as f32);
        let (x0, x1, w0, w1) = source(x, self.width, self.width as f32 / wh as f32);
        let v = |yy: usize, xx: usize| self.cur[yy * self.width + xx];
        let mut out = [0.0f32; 2];
        for (c, o) in out.iter_mut().enumerate() {
            let top = v(y0, x0)[c] * w0 + v(y0, x1)[c] * w1;
            let bottom = v(y1, x0)[c] * w0 + v(y1, x1)[c] * w1;
            *o = top * h0 + bottom * h1;
        }
        out
    }
}
