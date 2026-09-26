//! The model: a matrix in linear light with a ridge toward identity,
//! per-channel monotone curves kept only where they help, and a
//! residual table on a lattice in the encoded domain, solved as a
//! regularized least squares.
//!
//! The reference is `tools/camera-match/fit.py`. The matrix and the
//! curves are its stages as written. The table is not: the script
//! fits the residual with thin-plate radial basis functions and pulls
//! it to zero away from the data; here each block pair votes for the
//! eight nodes of its cell with trilinear weights, a second-difference
//! term along each axis asks the table to be smooth, and a small term
//! pulls every node toward zero residual, which is what "identity
//! where there is no data" means on a lattice. The system is sparse,
//! symmetric and positive, and conjugate gradients solves it.

use crate::LUT_SIZE;
use crate::color::{decode3, delta_e, encode3, mean_delta_e, mul3, srgb_decode, srgb_encode};

/// Knots of the per-channel curves, in linear light: zero and then
/// 24 log-spaced from 0.001 to 1.
pub const CURVE_KNOTS: usize = 25;

pub fn curve_x() -> [f32; CURVE_KNOTS] {
    let mut x = [0.0f32; CURVE_KNOTS];
    for (i, v) in x.iter_mut().enumerate().skip(1) {
        *v = 1e-3 * 1000f32.powf((i - 1) as f32 / 23.0);
    }
    x
}

/// Per-channel monotone curves, linear in and linear out, at
/// [`curve_x`].
#[derive(Debug, Clone, PartialEq)]
pub struct Curves {
    pub y: [[f32; CURVE_KNOTS]; 3],
}

/// The residual table: `size³` corrections in the encoded domain,
/// red fastest.
#[derive(Debug, Clone, PartialEq)]
pub struct Lattice {
    pub size: usize,
    pub data: Vec<[f32; 3]>,
}

/// How the table is regularized. Both are relative to a data term
/// normalized to one vote per node on average, so they mean the same
/// thing whatever the number of pairs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LutParams {
    /// Weight on the second difference along each axis.
    pub smoothness: f32,
    /// Weight pulling each node's residual to zero.
    pub pull: f32,
    /// Conjugate gradient iterations.
    pub iterations: usize,
}

impl Default for LutParams {
    fn default() -> Self {
        LutParams {
            smoothness: 0.5,
            pull: 0.02,
            iterations: 300,
        }
    }
}

/// The ridge's weight, as a fraction of the data's own energy.
pub const RIDGE: f32 = 0.02;

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// Linear sRGB in, linear out, rows.
    pub matrix: [[f32; 3]; 3],
    pub curves: Option<Curves>,
    pub lattice: Option<Lattice>,
}

impl Default for Model {
    fn default() -> Self {
        Self::identity()
    }
}

/// `np.interp`: piecewise linear through `(xs, ys)`, clamped to the
/// end values outside; `xs` ascending.
pub fn interp(x: f32, xs: &[f32], ys: &[f32]) -> f32 {
    if x <= xs[0] {
        return ys[0];
    }
    let last = xs.len() - 1;
    if x >= xs[last] {
        return ys[last];
    }
    let i = xs.partition_point(|&v| v <= x) - 1;
    let t = (x - xs[i]) / (xs[i + 1] - xs[i]);
    ys[i] + (ys[i + 1] - ys[i]) * t
}

impl Model {
    pub fn identity() -> Self {
        Model {
            matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            curves: None,
            lattice: None,
        }
    }

    /// The matrix by least squares with a ridge toward identity: a
    /// set that does not cover a color, twenty frames of one warm room
    /// say, leaves that column free and plain least squares runs off
    /// (a blue diagonal of 0.4 was seen). The ridge is scaled to the
    /// data's own energy so it is the same pull whatever the set's
    /// size.
    pub fn fit_matrix(&mut self, x_lin: &[[f32; 3]], y_lin: &[[f32; 3]]) {
        assert_eq!(x_lin.len(), y_lin.len());
        let mut xtx = [[0.0f64; 3]; 3];
        let mut xty = [[0.0f64; 3]; 3];
        for (x, y) in x_lin.iter().zip(y_lin) {
            for i in 0..3 {
                for j in 0..3 {
                    xtx[i][j] += x[i] as f64 * x[j] as f64;
                    xty[i][j] += x[i] as f64 * y[j] as f64;
                }
            }
        }
        let lam = RIDGE as f64 * (xtx[0][0] + xtx[1][1] + xtx[2][2]) / 3.0;
        for i in 0..3 {
            xtx[i][i] += lam;
            xty[i][i] += lam;
        }
        // Solve (XᵀX + λI) B = XᵀY + λI for B (3×3); M = Bᵀ. With no
        // pairs at all the ridge is zero too and there is nothing to
        // solve: the matrix stays identity.
        let Some(inv) = invert3(&xtx) else {
            self.matrix = Model::identity().matrix;
            return;
        };
        let mut b = [[0.0f64; 3]; 3];
        for (i, row) in b.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..3).map(|k| inv[i][k] * xty[k][j]).sum();
            }
        }
        for (i, row) in self.matrix.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = b[j][i] as f32;
            }
        }
    }

    /// Per-channel monotone curves on top of the matrix, in the
    /// encoded domain where the knots are evenly useful: binned
    /// medians of the target against the matrix's output, made
    /// non-decreasing, then stored as linear values at [`curve_x`].
    /// Kept only where it lowers the mean ΔE: a curve from binned
    /// medians can be worse than none where the data is thin.
    ///
    /// A bin counts when it holds a 400th of the pairs, eight at the
    /// least, so the rule means the same at any set size; a fixed
    /// count dropped the top bins of a subsampled set and left white
    /// mapping to a grey. And the last knot is pinned to one: the
    /// clipping cut keeps every pair under 0.97, so the top of the
    /// curve is never measured, and white must stay white.
    pub fn fit_curves(&mut self, x_lin: &[[f32; 3]], y_lin: &[[f32; 3]]) {
        const BINS: usize = 33;
        self.curves = None;
        let min = (x_lin.len() / 400).max(8);
        let z: Vec<[f32; 3]> = x_lin
            .iter()
            .map(|x| mul3(&self.matrix, *x).map(|v| srgb_encode(v.clamp(0.0, 1.0))))
            .collect();
        let y: Vec<[f32; 3]> = y_lin.iter().map(|v| encode3(*v)).collect();
        let knots: Vec<f32> = (0..BINS).map(|i| i as f32 / (BINS - 1) as f32).collect();
        let cx = curve_x();
        let mut curves = Curves {
            y: [[0.0; CURVE_KNOTS]; 3],
        };
        for c in 0..3 {
            let mut bins: Vec<Vec<f32>> = vec![Vec::new(); BINS];
            for (zi, yi) in z.iter().zip(&y) {
                let k = ((zi[c] * (BINS - 1) as f32).round() as isize).clamp(0, BINS as isize - 1);
                bins[k as usize].push(yi[c]);
            }
            let (mut kx, mut ky) = (Vec::new(), Vec::new());
            for (k, b) in bins.iter_mut().enumerate() {
                if b.len() >= min {
                    kx.push(knots[k]);
                    ky.push(median(b));
                }
            }
            if kx.is_empty() {
                return;
            }
            let mut vals: Vec<f32> = knots.iter().map(|&k| interp(k, &kx, &ky)).collect();
            for i in 1..vals.len() {
                vals[i] = vals[i].max(vals[i - 1]);
            }
            for (i, &x) in cx.iter().enumerate() {
                curves.y[c][i] = srgb_decode(interp(srgb_encode(x), &knots, &vals));
            }
            curves.y[c][CURVE_KNOTS - 1] = 1.0;
        }
        let before = mean_delta_e(
            &x_lin
                .iter()
                .map(|x| mul3(&self.matrix, *x))
                .collect::<Vec<_>>(),
            y_lin,
        );
        self.curves = Some(curves);
        let after = mean_delta_e(
            &x_lin
                .iter()
                .map(|x| self.base_linear(*x))
                .collect::<Vec<_>>(),
            y_lin,
        );
        if after > before {
            self.curves = None;
        }
    }

    /// Matrix and curves, linear in and linear out. Without curves the
    /// matrix's output is not clipped, as the script leaves it.
    pub fn base_linear(&self, x_lin: [f32; 3]) -> [f32; 3] {
        let z = mul3(&self.matrix, x_lin);
        match &self.curves {
            None => z,
            Some(c) => {
                let cx = curve_x();
                [0, 1, 2].map(|k| interp(z[k].clamp(0.0, 1.0), &cx, &c.y[k]))
            }
        }
    }

    /// Matrix and curves, encoded in and encoded out.
    pub fn base(&self, x_enc: [f32; 3]) -> [f32; 3] {
        encode3(self.base_linear(decode3(x_enc)))
    }

    /// The residual table over the matrix and curves, from encoded
    /// pairs.
    pub fn fit_lut(&mut self, x_enc: &[[f32; 3]], y_enc: &[[f32; 3]], params: LutParams) {
        assert_eq!(x_enc.len(), y_enc.len());
        let n = LUT_SIZE;
        let nodes = n * n * n;
        // Each pair's cell corners and trilinear weights.
        let mut corners: Vec<[(usize, f32); 8]> = Vec::with_capacity(x_enc.len());
        let mut residual: Vec<[f32; 3]> = Vec::with_capacity(x_enc.len());
        for (x, y) in x_enc.iter().zip(y_enc) {
            corners.push(cell(*x, n));
            let b = self.base(*x);
            residual.push([y[0] - b[0], y[1] - b[1], y[2] - b[2]]);
        }
        // One vote per node on average: the data weight scales with
        // the nodes and against the pairs, so smoothness and pull mean
        // the same at any set size.
        let rho = nodes as f32 / x_enc.len().max(1) as f32;
        let mut data = vec![[0.0f32; 3]; nodes];
        for c in 0..3 {
            let rhs: Vec<f32> = {
                let mut r = vec![0.0f32; nodes];
                for (cs, res) in corners.iter().zip(&residual) {
                    for &(i, w) in cs {
                        r[i] += rho * w * res[c];
                    }
                }
                r
            };
            let apply = |v: &[f32], out: &mut [f32]| {
                out.iter_mut().for_each(|o| *o = 0.0);
                for cs in &corners {
                    let s: f32 = cs.iter().map(|&(i, w)| w * v[i]).sum();
                    for &(i, w) in cs {
                        out[i] += rho * w * s;
                    }
                }
                second_difference_gram(v, out, n, params.smoothness);
                for (o, vi) in out.iter_mut().zip(v) {
                    *o += params.pull * vi;
                }
            };
            let sol = conjugate_gradient(apply, &rhs, params.iterations);
            for (d, s) in data.iter_mut().zip(sol) {
                d[c] = s;
            }
        }
        // White stays white: the node at (1, 1, 1) is set so the whole
        // model lands there exactly, whatever the matrix and curves
        // made of it. No pair reaches that node (the clipping cut
        // stops at 0.97), so nothing measured is overridden.
        let white = self.base([1.0; 3]);
        data[nodes - 1] = [1.0 - white[0], 1.0 - white[1], 1.0 - white[2]];
        self.lattice = Some(Lattice { size: n, data });
    }

    /// The residual the table adds at an encoded color, trilinear.
    pub fn correction(&self, x_enc: [f32; 3]) -> [f32; 3] {
        let Some(l) = &self.lattice else {
            return [0.0; 3];
        };
        let mut out = [0.0f32; 3];
        for (i, w) in cell(x_enc, l.size) {
            for (o, d) in out.iter_mut().zip(&l.data[i]) {
                *o += w * d;
            }
        }
        out
    }

    /// The whole model, encoded in and encoded out, clipped to the
    /// domain.
    pub fn apply(&self, x_enc: [f32; 3]) -> [f32; 3] {
        let b = self.base(x_enc);
        let r = self.correction(x_enc);
        [0, 1, 2].map(|k| (b[k] + r[k]).clamp(0.0, 1.0))
    }

    /// The full fit: matrix, curves, table.
    pub fn fit(x_enc: &[[f32; 3]], y_enc: &[[f32; 3]], params: LutParams) -> Model {
        let x_lin: Vec<[f32; 3]> = x_enc.iter().map(|v| decode3(*v)).collect();
        let y_lin: Vec<[f32; 3]> = y_enc.iter().map(|v| decode3(*v)).collect();
        let mut m = Model::identity();
        m.fit_matrix(&x_lin, &y_lin);
        m.fit_curves(&x_lin, &y_lin);
        m.fit_lut(x_enc, y_enc, params);
        m
    }

    /// Mean ΔE of the model's output against the camera's, over
    /// encoded pairs.
    pub fn mean_delta_e(&self, x_enc: &[[f32; 3]], y_enc: &[[f32; 3]]) -> f32 {
        let n = x_enc.len().max(1) as f32;
        x_enc
            .iter()
            .zip(y_enc)
            .map(|(x, y)| delta_e(decode3(self.apply(*x)), decode3(*y)))
            .sum::<f32>()
            / n
    }
}

/// The eight lattice corners of an encoded color's cell and their
/// trilinear weights; index red fastest.
pub fn cell(x_enc: [f32; 3], n: usize) -> [(usize, f32); 8] {
    let last = (n - 1) as f32;
    let p = x_enc.map(|v| (v.clamp(0.0, 1.0) * last).min(last));
    let i = p.map(|v| (v.floor() as usize).min(n - 2));
    let f = [0, 1, 2].map(|k| p[k] - i[k] as f32);
    let mut out = [(0usize, 0.0f32); 8];
    for (k, o) in out.iter_mut().enumerate() {
        let (dr, dg, db) = (k & 1, (k >> 1) & 1, (k >> 2) & 1);
        let w = (if dr == 1 { f[0] } else { 1.0 - f[0] })
            * (if dg == 1 { f[1] } else { 1.0 - f[1] })
            * (if db == 1 { f[2] } else { 1.0 - f[2] });
        let idx = (i[0] + dr) + n * ((i[1] + dg) + n * (i[2] + db));
        *o = (idx, w);
    }
    out
}

/// `out += λ DᵀD v` for the second difference along each axis of an
/// `n³` lattice, red fastest.
fn second_difference_gram(v: &[f32], out: &mut [f32], n: usize, lambda: f32) {
    if lambda == 0.0 {
        return;
    }
    let strides = [1usize, n, n * n];
    for &s in &strides {
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let idx = r + n * (g + n * b);
                    let along = match s {
                        1 => r,
                        x if x == n => g,
                        _ => b,
                    };
                    if along == 0 || along == n - 1 {
                        continue;
                    }
                    let d = v[idx - s] - 2.0 * v[idx] + v[idx + s];
                    out[idx - s] += lambda * d;
                    out[idx] -= 2.0 * lambda * d;
                    out[idx + s] += lambda * d;
                }
            }
        }
    }
}

/// Conjugate gradients on a symmetric positive operator, from zero.
fn conjugate_gradient<F>(apply: F, rhs: &[f32], iterations: usize) -> Vec<f32>
where
    F: Fn(&[f32], &mut [f32]),
{
    let n = rhs.len();
    let mut x = vec![0.0f32; n];
    let mut r = rhs.to_vec();
    let mut p = r.clone();
    let mut ap = vec![0.0f32; n];
    let mut rr: f64 = r.iter().map(|v| (*v as f64) * (*v as f64)).sum();
    let tol = rr * 1e-12;
    for _ in 0..iterations {
        if rr <= tol {
            break;
        }
        apply(&p, &mut ap);
        let pap: f64 = p
            .iter()
            .zip(&ap)
            .map(|(a, b)| (*a as f64) * (*b as f64))
            .sum();
        if pap <= 0.0 {
            break;
        }
        let alpha = (rr / pap) as f32;
        for i in 0..n {
            x[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
        }
        let rr_new: f64 = r.iter().map(|v| (*v as f64) * (*v as f64)).sum();
        let beta = (rr_new / rr) as f32;
        for i in 0..n {
            p[i] = r[i] + beta * p[i];
        }
        rr = rr_new;
    }
    x
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(f32::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn invert3(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-18 {
        return None;
    }
    let inv = [
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) / det,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / det,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / det,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) / det,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / det,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / det,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) / det,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / det,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / det,
        ],
    ];
    Some(inv)
}

/// Leave one frame out at a time: each pair carries its frame's
/// index in `ids`; for each frame in `frames` the model is fitted
/// without it and measured on it. Returns the mean ΔE per chosen
/// frame, in `frames`' order.
pub fn leave_one_out(
    x_enc: &[[f32; 3]],
    y_enc: &[[f32; 3]],
    ids: &[usize],
    frames: &[usize],
    params: LutParams,
) -> Vec<f32> {
    frames
        .iter()
        .map(|&f| {
            let (mut tx, mut ty, mut hx, mut hy) = (vec![], vec![], vec![], vec![]);
            for ((x, y), &id) in x_enc.iter().zip(y_enc).zip(ids) {
                if id == f {
                    hx.push(*x);
                    hy.push(*y);
                } else {
                    tx.push(*x);
                    ty.push(*y);
                }
            }
            if hx.is_empty() {
                return 0.0;
            }
            Model::fit(&tx, &ty, params).mean_delta_e(&hx, &hy)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors(n: usize) -> Vec<[f32; 3]> {
        // A spread over the cube, deterministic.
        (0..n)
            .map(|i| {
                let t = i as f32 * 0.618_034;
                let u = i as f32 * 0.414_214;
                let v = i as f32 * 0.267_949;
                [
                    0.05 + 0.9 * (t - t.floor()),
                    0.05 + 0.9 * (u - u.floor()),
                    0.05 + 0.9 * (v - v.floor()),
                ]
            })
            .collect()
    }

    #[test]
    fn curve_knots_start_at_zero_and_end_at_one() {
        let x = curve_x();
        assert_eq!(x[0], 0.0);
        assert!((x[1] - 1e-3).abs() < 1e-9);
        assert!((x[CURVE_KNOTS - 1] - 1.0).abs() < 1e-6);
        assert!(x.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn interp_matches_numpy_at_the_ends_and_between() {
        let xs = [0.0, 1.0, 2.0];
        let ys = [0.0, 10.0, 0.0];
        assert_eq!(interp(-1.0, &xs, &ys), 0.0);
        assert_eq!(interp(3.0, &xs, &ys), 0.0);
        assert_eq!(interp(0.5, &xs, &ys), 5.0);
        assert_eq!(interp(1.0, &xs, &ys), 10.0);
        assert_eq!(interp(1.5, &xs, &ys), 5.0);
    }

    #[test]
    fn the_matrix_recovers_a_known_mix_with_full_coverage() {
        let truth = [[1.1, -0.05, -0.05], [0.0, 1.0, 0.0], [0.02, 0.03, 0.95]];
        let x: Vec<[f32; 3]> = colors(2000).into_iter().map(decode3).collect();
        let y: Vec<[f32; 3]> = x.iter().map(|v| mul3(&truth, *v)).collect();
        let mut m = Model::identity();
        m.fit_matrix(&x, &y);
        for (i, row) in truth.iter().enumerate() {
            for (j, t) in row.iter().enumerate() {
                // The ridge pulls a little toward identity; on a
                // well-covered set that is under a percent.
                assert!((m.matrix[i][j] - t).abs() < 0.01, "{:?}", m.matrix);
            }
        }
    }

    #[test]
    fn the_ridge_holds_an_uncovered_channel_near_identity() {
        // No blue in the data at all: plain least squares could put
        // anything in blue's column; the ridge keeps it at identity.
        let x: Vec<[f32; 3]> = colors(500)
            .into_iter()
            .map(|c| decode3([c[0], c[1], 0.0]))
            .collect();
        let y: Vec<[f32; 3]> = x.iter().map(|v| [v[0] * 1.2, v[1], v[2]]).collect();
        let mut m = Model::identity();
        m.fit_matrix(&x, &y);
        assert!((m.matrix[2][2] - 1.0).abs() < 0.05, "{:?}", m.matrix);
        assert!((m.matrix[0][0] - 1.2).abs() < 0.03, "{:?}", m.matrix);
    }

    #[test]
    fn curves_recover_a_gamma_and_are_dropped_when_useless() {
        let x_enc = colors(4000);
        let x: Vec<[f32; 3]> = x_enc.iter().map(|v| decode3(*v)).collect();
        // A per-channel gamma the matrix cannot express.
        let y: Vec<[f32; 3]> = x.iter().map(|v| v.map(|c| c.powf(0.8))).collect();
        let mut m = Model::identity();
        m.fit_matrix(&x, &y);
        m.fit_curves(&x, &y);
        assert!(m.curves.is_some());
        let err = mean_delta_e(&x.iter().map(|v| m.base_linear(*v)).collect::<Vec<_>>(), &y);
        let plain = mean_delta_e(
            &x.iter().map(|v| mul3(&m.matrix, *v)).collect::<Vec<_>>(),
            &y,
        );
        // The matrix, with no offset term, mimics the gamma's lift
        // with cross terms of a few percent that a per-channel curve
        // cannot undo, so the curves take a third off rather than all.
        assert!(err < plain * 0.7, "{err} against {plain}");
        let c = m.curves.as_ref().unwrap();
        assert!(
            c.y.iter().all(|ch| ch.windows(2).all(|w| w[1] >= w[0])),
            "monotone"
        );
        // Pure matrix data: the matrix alone is within 0.0005, and
        // the curves stay only if they take that lower still (the
        // ridge leaves a hair off the gain that a curve can take, so
        // either outcome is right; what is held is the rule).
        let y2: Vec<[f32; 3]> = x
            .iter()
            .map(|v| mul3(&[[1.1, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.9]], *v))
            .collect();
        let mut m2 = Model::identity();
        m2.fit_matrix(&x, &y2);
        let plain2 = mean_delta_e(
            &x.iter().map(|v| mul3(&m2.matrix, *v)).collect::<Vec<_>>(),
            &y2,
        );
        m2.fit_curves(&x, &y2);
        let with2 = mean_delta_e(
            &x.iter().map(|v| m2.base_linear(*v)).collect::<Vec<_>>(),
            &y2,
        );
        assert!(plain2 < 0.0005, "{plain2}");
        assert!(with2 <= plain2, "{with2} against {plain2}");
        // And on data the curves can only spoil, noise about the
        // matrix, they are dropped.
        let y3: Vec<[f32; 3]> = x
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let noise = ((i * 7919) % 101) as f32 / 101.0 - 0.5;
                v.map(|c| (c * 1.05 + 0.02 * noise).clamp(0.0, 1.0))
            })
            .collect();
        let mut m3 = Model::identity();
        m3.fit_matrix(&x, &y3);
        m3.fit_curves(&x, &y3);
        let plain3 = mean_delta_e(
            &x.iter().map(|v| mul3(&m3.matrix, *v)).collect::<Vec<_>>(),
            &y3,
        );
        let with3 = mean_delta_e(
            &x.iter().map(|v| m3.base_linear(*v)).collect::<Vec<_>>(),
            &y3,
        );
        assert!(with3 <= plain3, "{with3} against {plain3}");
    }

    #[test]
    fn cell_weights_sum_to_one_and_a_node_is_exact() {
        let w: f32 = cell([0.3, 0.6, 0.9], 33).iter().map(|c| c.1).sum();
        assert!((w - 1.0).abs() < 1e-5);
        let c = cell([0.5, 0.5, 0.5], 33);
        let (i, w) = c[0];
        assert_eq!(i, 16 + 33 * (16 + 33 * 16));
        assert!((w - 1.0).abs() < 1e-5, "{c:?}");
        let (i, _) = cell([1.0, 1.0, 1.0], 33)[7];
        assert_eq!(i, 33 * 33 * 33 - 1);
    }

    #[test]
    fn the_lattice_learns_a_smooth_residual_and_stays_zero_off_data() {
        // A hue twist the matrix and curves cannot express: red gains
        // a little where green is high.
        let x_enc = colors(6000);
        let y_enc: Vec<[f32; 3]> = x_enc
            .iter()
            .map(|v| [(v[0] + 0.05 * v[1] * v[1]).min(1.0), v[1], v[2]])
            .collect();
        let m = Model::fit(&x_enc, &y_enc, LutParams::default());
        assert!(m.lattice.is_some());
        let err = m.mean_delta_e(&x_enc, &y_enc);
        let base_err = mean_delta_e(
            &x_enc
                .iter()
                .map(|v| decode3(m.base(*v)))
                .collect::<Vec<_>>(),
            &y_enc.iter().map(|v| decode3(*v)).collect::<Vec<_>>(),
        );
        assert!(err < base_err * 0.5, "{err} against {base_err}");
        // Solid black is nowhere in the data (the set starts at 0.05):
        // the correction there is near zero, pulled by the prior.
        let c = m.correction([0.0, 0.0, 0.0]);
        assert!(c.iter().all(|v| v.abs() < 0.01), "{c:?}");
    }

    #[test]
    fn an_empty_set_fits_to_identity_without_panicking() {
        let m = Model::fit(&[], &[], LutParams::default());
        assert_eq!(m.matrix, Model::identity().matrix);
        assert!(m.curves.is_none());
        let x = [0.3, 0.6, 0.9];
        let out = m.apply(x);
        assert!((0..3).all(|k| (out[k] - x[k]).abs() < 1e-6), "{out:?}");
        // And a frame held out against nothing.
        let held = leave_one_out(&[[0.5; 3]], &[[0.4; 3]], &[0], &[0], LutParams::default());
        assert!(held[0].is_finite());
    }

    #[test]
    fn white_stays_white_and_a_reused_model_drops_stale_curves() {
        let x_enc = colors(4000);
        // A look that darkens everything, so the matrix alone would
        // not send white to white.
        let y_enc: Vec<[f32; 3]> = x_enc.iter().map(|v| v.map(|c| c * 0.9)).collect();
        let m = Model::fit(&x_enc, &y_enc, LutParams::default());
        let w = m.apply([1.0; 3]);
        assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-6), "{w:?}");
        if let Some(c) = &m.curves {
            assert!(c.y.iter().all(|ch| ch[CURVE_KNOTS - 1] == 1.0));
        }
        // Curves fitted once, then refitted on a set too thin for any
        // bin: the old curves must not survive.
        let mut m2 = m.clone();
        m2.fit_curves(&[[0.5; 3]; 3], &[[0.4; 3]; 3]);
        assert!(m2.curves.is_none());
    }

    #[test]
    fn conjugate_gradient_solves_a_small_system() {
        // A = [[4,1],[1,3]], b = [1,2] → x = [1/11, 7/11].
        let apply = |v: &[f32], out: &mut [f32]| {
            out[0] = 4.0 * v[0] + v[1];
            out[1] = v[0] + 3.0 * v[1];
        };
        let x = conjugate_gradient(apply, &[1.0, 2.0], 10);
        assert!(
            (x[0] - 1.0 / 11.0).abs() < 1e-5 && (x[1] - 7.0 / 11.0).abs() < 1e-5,
            "{x:?}"
        );
    }

    #[test]
    fn second_difference_gram_is_symmetric_and_zero_on_a_plane() {
        let n = 5;
        let nodes = n * n * n;
        // A linear ramp has no second difference.
        let ramp: Vec<f32> = (0..nodes).map(|i| (i % n) as f32 * 0.1).collect();
        let mut out = vec![0.0; nodes];
        second_difference_gram(&ramp, &mut out, n, 1.0);
        assert!(out.iter().all(|v| v.abs() < 1e-5));
        // Symmetry: uᵀ(Av) == vᵀ(Au).
        let u: Vec<f32> = (0..nodes).map(|i| ((i * 7) % 11) as f32 / 11.0).collect();
        let v: Vec<f32> = (0..nodes).map(|i| ((i * 3) % 13) as f32 / 13.0).collect();
        let (mut au, mut av) = (vec![0.0; nodes], vec![0.0; nodes]);
        second_difference_gram(&u, &mut au, n, 1.0);
        second_difference_gram(&v, &mut av, n, 1.0);
        let uav: f32 = u.iter().zip(&av).map(|(a, b)| a * b).sum();
        let vau: f32 = v.iter().zip(&au).map(|(a, b)| a * b).sum();
        assert!((uav - vau).abs() < 1e-3, "{uav} {vau}");
    }
}
