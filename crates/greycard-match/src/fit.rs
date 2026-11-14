//! The model: a matrix in linear light with a ridge toward identity,
//! per-channel monotone curves kept only where they help, and a
//! residual table on a lattice over the encoded domain, its
//! corrections in Oklab, solved as a regularized least squares.
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
//!
//! The script's residual was in encoded sRGB; ours is in Oklab, so
//! that the table's lightness is one of its three coordinates and can
//! be held to a fourth term the script has not got (notes §260): the
//! whole model's lightness asked not to change across the input's
//! chroma. Without it the table puts small differences of lightness,
//! a quarter of a visible step, across short distances in chroma,
//! which held out over every frame buy nothing; but it is their slope
//! a picture's chroma noise goes through, and it comes out as blotches
//! of lightness. The term holds over the common colors and fades out
//! past them ([`FLAT_CHROMA`]), so a saturated color no frame showed
//! is still what the matrix and curves make of it, the table near
//! identity there as before.
//!
//! One more term holds the greys no pair reaches. The per-channel
//! curves are fitted on whatever colors the frames had, so after one
//! shoot under warm light they tint greys the shoot never showed, and
//! the smoothness term, which costs nothing for a straight line,
//! carries the data's slope there too. So near the grey axis a node
//! with little support is pulled hard ([`LutParams::hold`]) toward the
//! input's own chroma plus the cast the data's greys show: a grey no
//! frame showed comes out as the camera renders the greys it did show,
//! grey when it showed none, at the lightness the data and the
//! smoothness give it.

use crate::LUT_SIZE;
use crate::color::{
    decode3, delta_e, encode3, from_oklab, mean_delta_e, mul3, oklab, srgb_decode, srgb_encode,
};

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

/// The residual table: `size³` corrections, indexed by the encoded
/// color, red fastest, and added to the Oklab of what the matrix and
/// curves give: lightness, a and b, so a correction to lightness is
/// one number, fitted in the units ΔE is measured in.
#[derive(Debug, Clone, PartialEq)]
pub struct Lattice {
    pub size: usize,
    pub data: Vec<[f32; 3]>,
}

/// How the table is regularized. The weights are relative to a data
/// term normalized to one vote per node on average, so they mean the
/// same thing whatever the number of pairs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LutParams {
    /// Weight on the second difference along each axis.
    pub smoothness: f32,
    /// Weight pulling each node's residual to zero.
    pub pull: f32,
    /// Weight, on top of `pull`, holding a node near the grey axis
    /// that few pairs support to the input's own chroma plus the cast
    /// the data's near-grey pairs show (its a and b; its lightness is
    /// left to the data and the smoothness). A node's
    /// support is its share of the data term, the pairs' trilinear
    /// weights on it, one vote a node on average; the hold falls
    /// linearly from all of `hold` at none to nothing at `supported`.
    pub hold: f32,
    /// The support at which a node is held by `pull` alone.
    pub supported: f32,
    /// The input chroma (Oklab) up to which the hold holds whole,
    /// fading to none at twice it.
    pub neutral: f32,
    /// Weight on the model's lightness changing across chroma: the
    /// gradient of the whole model's output lightness, along the
    /// lattice, with its part along the input's own lightness taken
    /// off (notes §260).
    pub flatness: f32,
    /// Conjugate gradient iterations.
    pub iterations: usize,
}

impl Default for LutParams {
    fn default() -> Self {
        LutParams {
            smoothness: 0.5,
            pull: 0.02,
            hold: HOLD,
            supported: SUPPORTED,
            neutral: NEUTRAL,
            flatness: 32.0,
            iterations: 300,
        }
    }
}

/// [`LutParams::hold`]'s default.
pub const HOLD: f32 = 4.0;
/// [`LutParams::supported`]'s default.
pub const SUPPORTED: f32 = 1.0;
/// [`LutParams::neutral`]'s default.
pub const NEUTRAL: f32 = 0.02;

/// The ridge's weight, as a fraction of the data's own energy.
pub const RIDGE: f32 = 0.02;

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// Linear sRGB in, linear out, rows.
    pub matrix: [[f32; 3]; 3],
    pub curves: Option<Curves>,
    pub lattice: Option<Lattice>,
    /// Fitted on a monochrome set ([`crate::mono`]): the matrix, curves
    /// and table are fitted as for color, on the camera's greys, and
    /// every output is the grey at their lightness, R = G = B.
    pub monochrome: bool,
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
            monochrome: false,
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
            let (b, t) = (self.base_lab(*x), oklab(decode3(*y)));
            residual.push([t[0] - b[0], t[1] - b[1], t[2] - b[2]]);
        }
        // One vote per node on average: the data weight scales with
        // the nodes and against the pairs, so smoothness and pull mean
        // the same at any set size.
        let rho = nodes as f32 / x_enc.len().max(1) as f32;
        // Each node's support: its votes in the data term.
        let mut support = vec![0.0f32; nodes];
        for cs in &corners {
            for &(i, w) in cs {
                support[i] += rho * w;
            }
        }
        // The hold's weight at each node, and the a and b of the
        // residual it holds the node to: the one that gives the input's
        // own chroma and the cast the data's greys show.
        let cast = GreyCast::of(x_enc, y_enc, params.neutral);
        let (held, targets): (Vec<f32>, Vec<[f32; 2]>) = (0..nodes)
            .map(|i| {
                let x = node_color(i, n);
                let input = oklab(decode3(x));
                let weight = params.hold
                    * held_share(support[i], params.supported)
                    * near_grey(input[1].hypot(input[2]), params.neutral);
                if weight == 0.0 {
                    return (0.0, [0.0; 2]);
                }
                let b = self.base_lab(x);
                let c = cast.at(input[0]);
                (
                    weight,
                    [
                        input[1] + c[0] * input[0] - b[1],
                        input[2] + c[1] * input[0] - b[2],
                    ],
                )
            })
            .unzip();
        // The flatness term, on lightness alone: at each node, the way
        // the input's own lightness rises, along which the output's may
        // change freely; the lightness the matrix and curves give it,
        // which the table is asked to flatten along with its own; and
        // the term's weight there, whole over the common colors and
        // fading to none past them (`flat_reach`), so a saturated color
        // no frame showed keeps what the matrix and curves make of it.
        let (rises, base_lightness, weights) = if params.flatness > 0.0 {
            let input: Vec<[f32; 3]> = (0..nodes)
                .map(|i| oklab(decode3(node_color(i, n))))
                .collect();
            let lightness: Vec<f32> = input.iter().map(|lab| lab[0]).collect();
            let base = (0..nodes)
                .map(|i| self.base_lab(node_color(i, n))[0])
                .collect();
            let weights = input
                .iter()
                .map(|lab| params.flatness * flat_reach(lab[1].hypot(lab[2])))
                .collect();
            (rises(&lightness, n), base, weights)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
        let mut data = vec![[0.0f32; 3]; nodes];
        for c in 0..3 {
            let flat = c == 0 && params.flatness > 0.0;
            let mut rhs = vec![0.0f32; nodes];
            for (cs, res) in corners.iter().zip(&residual) {
                for &(i, w) in cs {
                    rhs[i] += rho * w * res[c];
                }
            }
            // The hold is on a and b alone: lightness is left to the
            // data and the smoothness, since the matrix and curves' own
            // can fall back on itself between the greys the data has.
            let holds = c > 0;
            if holds {
                for ((r, h), t) in rhs.iter_mut().zip(&held).zip(&targets) {
                    *r += h * t[c - 1];
                }
            }
            if flat {
                let mut pushed = vec![0.0f32; nodes];
                across_chroma_gram(&base_lightness, &mut pushed, n, &rises, &weights);
                for (r, p) in rhs.iter_mut().zip(pushed) {
                    *r -= p;
                }
            }
            let apply = |v: &[f32], out: &mut [f32]| {
                out.iter_mut().for_each(|o| *o = 0.0);
                for cs in &corners {
                    let s: f32 = cs.iter().map(|&(i, w)| w * v[i]).sum();
                    for &(i, w) in cs {
                        out[i] += rho * w * s;
                    }
                }
                second_difference_gram(v, out, n, params.smoothness);
                if flat {
                    across_chroma_gram(v, out, n, &rises, &weights);
                }
                for ((o, vi), h) in out.iter_mut().zip(v).zip(&held) {
                    *o += (params.pull + if holds { *h } else { 0.0 }) * vi;
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
        let (white, to) = (self.base_lab([1.0; 3]), oklab([1.0; 3]));
        data[nodes - 1] = [0, 1, 2].map(|k| to[k] - white[k]);
        self.lattice = Some(Lattice { size: n, data });
    }

    /// Matrix and curves, encoded in, Oklab out.
    fn base_lab(&self, x_enc: [f32; 3]) -> [f32; 3] {
        oklab(self.base_linear(decode3(x_enc)))
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
    /// domain. A monochrome model's output is the grey at the
    /// lightness of what its stages give, exactly R = G = B.
    pub fn apply(&self, x_enc: [f32; 3]) -> [f32; 3] {
        let out = self.apply_stages(x_enc);
        if self.monochrome {
            let l = oklab(decode3(out))[0];
            return [srgb_encode(crate::mono::grey(l).clamp(0.0, 1.0)); 3];
        }
        out
    }

    /// Matrix, curves and table, encoded in and out, clipped.
    fn apply_stages(&self, x_enc: [f32; 3]) -> [f32; 3] {
        let out = match &self.lattice {
            None => self.base(x_enc),
            Some(_) => {
                let b = self.base_lab(x_enc);
                let r = self.correction(x_enc);
                encode3(from_oklab([0, 1, 2].map(|k| b[k] + r[k])))
            }
        };
        out.map(|v| v.clamp(0.0, 1.0))
    }

    /// The full fit: matrix, curves, table. A monochrome set
    /// ([`crate::mono::is_monochrome`]) is fitted on its camera blocks'
    /// greys (a camera's slight cast dropped) and renders every color
    /// as a grey: where no pair reaches, at the lightness the stages
    /// give, and with no color whatever the develop's was.
    pub fn fit(x_enc: &[[f32; 3]], y_enc: &[[f32; 3]], params: LutParams) -> Model {
        if crate::mono::is_monochrome(x_enc, y_enc) {
            let greys: Vec<[f32; 3]> = y_enc
                .iter()
                .map(|y| encode3([crate::mono::grey(oklab(decode3(*y))[0]); 3]))
                .collect();
            return Model {
                monochrome: true,
                ..Model::fit_stages(x_enc, &greys, params)
            };
        }
        Model::fit_stages(x_enc, y_enc, params)
    }

    /// Matrix, curves and table, as for color.
    fn fit_stages(x_enc: &[[f32; 3]], y_enc: &[[f32; 3]], params: LutParams) -> Model {
        let x_lin: Vec<[f32; 3]> = x_enc.iter().map(|v| decode3(*v)).collect();
        let y_lin: Vec<[f32; 3]> = y_enc.iter().map(|v| decode3(*v)).collect();
        let mut m = Model::identity();
        m.fit_matrix(&x_lin, &y_lin);
        m.fit_curves(&x_lin, &y_lin);
        m.fit_lut(x_enc, y_enc, params);
        m
    }

    /// How much the model's output lightness moves with its input's
    /// chroma: at each color of a grid over the common colors (Oklab
    /// lightness 0.3 to 0.92, chroma under 0.12), the size of
    /// ∂L_out/∂(a, b)_in, in Oklab on both sides. A look whose
    /// lightness follows only its input's lightness scores zero, as
    /// identity does. What a high score costs a picture is noise:
    /// chroma noise the look is given comes out as lightness noise
    /// this many times its size. It measures the model, base and table
    /// as fitted; the `.cube` written from it is the model at the
    /// nodes, interpolated in encoded sRGB, so scores close to this.
    pub fn coupling(&self) -> Coupling {
        let mut at: Vec<f32> = coupling_probes()
            .into_iter()
            .map(|e| {
                let j = lab_jacobian(|x| self.apply(x), e);
                (j[0][1] * j[0][1] + j[0][2] * j[0][2]).sqrt() as f32
            })
            .collect();
        at.sort_by(f32::total_cmp);
        let mean = at.iter().sum::<f32>() / at.len() as f32;
        let p95 = at[(at.len() * 95 / 100).min(at.len() - 1)];
        Coupling { mean, p95 }
    }

    /// What the model does to the grey ramp ([`crate::grey`]).
    pub fn grey_axis(&self) -> crate::grey::GreyAxis {
        crate::grey::grey_axis(|x| self.apply(x))
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

/// Lightness bins the greys' cast is read in.
const CAST_BINS: usize = 8;
/// The weight of near-grey pairs (one a pair, less as its chroma nears
/// the edge of [`LutParams::neutral`]'s reach) a bin, or the whole
/// set, needs before its cast is trusted.
const CAST_MIN: f32 = 8.0;

/// The cast the camera puts on the develop's greys, as the data shows
/// it: over the pairs whose input is near grey, the camera's a and b
/// less the input's, fitted per unit of the input's lightness (a
/// channel gain gives a cast in proportion to lightness in Oklab) with
/// the input's own chroma as a second term, so that a camera that
/// scales the near-greys' chroma, over a set whose near-greys lean one
/// way (a warm room's), does not read as a cast. Read per lightness
/// bin, with the whole set's where a bin has too few, and none where
/// the set has too few.
struct GreyCast {
    bins: [Option<[f32; 2]>; CAST_BINS],
    all: [f32; 2],
}

/// Sums for the cast's least squares on one set of pairs: per channel
/// (a, b), the camera's less the input's (`d`) against the input's
/// lightness (`l`) and its own chroma in that channel (`i`), so a camera
/// that only scales chroma (`d` in proportion to `i`) shows no cast.
#[derive(Clone, Copy, Default)]
struct CastSums {
    w: f64,
    ll: f64,
    li: [f64; 2],
    ii: [f64; 2],
    ld: [f64; 2],
    id: [f64; 2],
}

impl CastSums {
    fn add(&mut self, w: f64, l: f64, i: [f64; 2], d: [f64; 2]) {
        self.w += w;
        self.ll += w * l * l;
        for c in 0..2 {
            self.li[c] += w * l * i[c];
            self.ii[c] += w * i[c] * i[c];
            self.ld[c] += w * l * d[c];
            self.id[c] += w * i[c] * d[c];
        }
    }

    fn merge(&mut self, o: &CastSums) {
        self.w += o.w;
        self.ll += o.ll;
        for c in 0..2 {
            self.li[c] += o.li[c];
            self.ii[c] += o.ii[c];
            self.ld[c] += o.ld[c];
            self.id[c] += o.id[c];
        }
    }

    /// The cast per unit of lightness, when there is weight enough.
    fn cast(&self) -> Option<[f32; 2]> {
        if self.w < CAST_MIN as f64 || self.ll <= 0.0 {
            return None;
        }
        Some([0, 1].map(|c| {
            let det = self.ll * self.ii[c] - self.li[c] * self.li[c];
            if det > 1e-9 * self.ll * self.ii[c].max(1e-12) {
                ((self.ld[c] * self.ii[c] - self.li[c] * self.id[c]) / det) as f32
            } else {
                (self.ld[c] / self.ll) as f32
            }
        }))
    }
}

impl GreyCast {
    fn of(x_enc: &[[f32; 3]], y_enc: &[[f32; 3]], neutral: f32) -> GreyCast {
        let mut sums = [CastSums::default(); CAST_BINS];
        for (x, y) in x_enc.iter().zip(y_enc) {
            let (i, o) = (oklab(decode3(*x)), oklab(decode3(*y)));
            let w = near_grey(i[1].hypot(i[2]), neutral);
            if w == 0.0 {
                continue;
            }
            let k = ((i[0] * CAST_BINS as f32) as usize).min(CAST_BINS - 1);
            sums[k].add(
                w as f64,
                i[0] as f64,
                [i[1] as f64, i[2] as f64],
                [(o[1] - i[1]) as f64, (o[2] - i[2]) as f64],
            );
        }
        let mut all = CastSums::default();
        for s in &sums {
            all.merge(s);
        }
        GreyCast {
            bins: sums.map(|s| s.cast()),
            all: all.cast().unwrap_or([0.0; 2]),
        }
    }

    /// The cast per unit of lightness at input lightness `l`.
    fn at(&self, l: f32) -> [f32; 2] {
        let k = ((l * CAST_BINS as f32) as usize).min(CAST_BINS - 1);
        self.bins[k].unwrap_or(self.all)
    }
}

/// How much of [`LutParams::hold`] a node of `support` votes takes:
/// all at none, falling linearly to none at `supported`.
fn held_share(support: f32, supported: f32) -> f32 {
    if supported <= 0.0 {
        return 0.0;
    }
    1.0 - (support / supported).min(1.0)
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

/// [`Model::coupling`]: over the grid, the mean and the 95th
/// percentile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coupling {
    pub mean: f32,
    pub p95: f32,
}

/// The grid [`Model::coupling`] is measured on: encoded colors eleven
/// steps a side, those in the common range of lightness and chroma.
fn coupling_probes() -> Vec<[f32; 3]> {
    let n = 12;
    let mut out = Vec::new();
    for b in 1..n {
        for g in 1..n {
            for r in 1..n {
                let e = [r, g, b].map(|v| v as f32 / n as f32);
                let lab = oklab(decode3(e));
                let chroma = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
                if chroma < FLAT_CHROMA && lab[0] > 0.3 && lab[0] < 0.92 {
                    out.push(e);
                }
            }
        }
    }
    out
}

/// The Jacobian of `f` (encoded in, encoded out) at `e`, taken into
/// Oklab on both sides: rows the output's L, a, b, columns the
/// input's.
fn lab_jacobian(f: impl Fn([f32; 3]) -> [f32; 3], e: [f32; 3]) -> [[f64; 3]; 3] {
    let jac = |f: &dyn Fn([f32; 3]) -> [f32; 3], at: [f32; 3], h: f32| {
        let mut j = [[0.0f64; 3]; 3];
        for k in 0..3 {
            let (mut a, mut b) = (at, at);
            a[k] += h;
            b[k] -= h;
            let (fa, fb) = (f(a), f(b));
            for (i, row) in j.iter_mut().enumerate() {
                row[k] = ((fa[i] - fb[i]) / (2.0 * h)) as f64;
            }
        }
        j
    };
    let lab = |x: [f32; 3]| oklab(decode3(x));
    let mul = |a: [[f64; 3]; 3], b: [[f64; 3]; 3]| {
        let mut o = [[0.0f64; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
            }
        }
        o
    };
    // A step of a third of a lattice cell: across the table's own
    // interpolation, not inside one cell's linear piece alone.
    let through = jac(&f, e, 0.01);
    let into_in = jac(&lab, e, 1e-3);
    let into_out = jac(&lab, f(e), 1e-3);
    let Some(from_in) = invert3(&into_in) else {
        return [[0.0; 3]; 3];
    };
    mul(mul(into_out, through), from_in)
}

/// The encoded color at a lattice node, red fastest.
fn node_color(i: usize, n: usize) -> [f32; 3] {
    let last = (n - 1) as f32;
    [i % n, (i / n) % n, i / (n * n)].map(|v| v as f32 / last)
}

/// The way `v` rises at each node of an `n³` lattice, red fastest,
/// as a unit vector of its forward differences: the same differences
/// [`across_chroma_gram`] takes, so `v` itself costs nothing there and
/// a smooth function of it next to nothing, where the analytic
/// gradient would leave the lattice's own discretization to be
/// penalized (and Oklab's cube root bends hard enough near black for
/// that to matter). Zero at the last node along any axis, which the
/// penalty does not reach.
fn rises(v: &[f32], n: usize) -> Vec<[f32; 3]> {
    let strides = [1usize, n, n * n];
    (0..v.len())
        .map(|i| {
            let at = [i % n, (i / n) % n, i / (n * n)];
            if at.iter().any(|&a| a == n - 1) {
                return [0.0; 3];
            }
            let d = strides.map(|s| v[i + s] - v[i]);
            let m = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if m > 0.0 { d.map(|x| x / m) } else { d }
        })
        .collect()
}

/// The input chroma (Oklab) up to which the flatness term holds
/// whole: the range [`Model::coupling`] measures, where the fitted
/// sets' blocks are (their 99th percentile was 0.11).
pub const FLAT_CHROMA: f32 = 0.12;
/// The input chroma past which it holds not at all.
pub const FLAT_CHROMA_END: f32 = 0.15;

/// The flatness term's share at a node of input chroma `c`: one up to
/// [`FLAT_CHROMA`], none past [`FLAT_CHROMA_END`], a smoothstep between.
fn flat_reach(c: f32) -> f32 {
    fade(c, FLAT_CHROMA, FLAT_CHROMA_END)
}

/// The hold's share at a node of input chroma `c`: one up to
/// `neutral`, none past twice it; none at all with no `neutral`.
fn near_grey(c: f32, neutral: f32) -> f32 {
    if neutral <= 0.0 {
        return 0.0;
    }
    fade(c, neutral, 2.0 * neutral)
}

/// One up to `from`, none past `to`, a smoothstep between.
fn fade(c: f32, from: f32, to: f32) -> f32 {
    let t = ((c - from) / (to - from)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// `out += DᵀWQD v`: D the forward difference along each axis of an
/// `n³` lattice, red fastest, giving a gradient at every node with
/// three forward neighbors; Q the projection off the node's own
/// direction of rising lightness in `rises`; W each node's weight in
/// `weights`. So it is the penalty on `v` changing across chroma, with
/// its change along lightness left free. Q is symmetric and
/// idempotent and the weights are not negative, so the operator is
/// symmetric and positive semidefinite, as conjugate gradients needs.
fn across_chroma_gram(v: &[f32], out: &mut [f32], n: usize, rises: &[[f32; 3]], weights: &[f32]) {
    let strides = [1usize, n, n * n];
    for b in 0..n - 1 {
        for g in 0..n - 1 {
            for r in 0..n - 1 {
                let idx = r + n * (g + n * b);
                let (u, lambda) = (rises[idx], weights[idx]);
                if lambda == 0.0 {
                    continue;
                }
                let d = strides.map(|s| v[idx + s] - v[idx]);
                let along = u[0] * d[0] + u[1] * d[1] + u[2] * d[2];
                for (k, s) in strides.into_iter().enumerate() {
                    let q = lambda * (d[k] - along * u[k]);
                    out[idx + s] += q;
                    out[idx] -= q;
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

    /// A portrait shoot's colors: skin across its lightness, and
    /// every hue muted to a fifth, so nothing saturated is sampled.
    fn portrait(n: usize) -> Vec<[f32; 3]> {
        colors(n)
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                let g = (c[0] + c[1] + c[2]) / 3.0;
                if i % 2 == 0 {
                    let t = 0.15 + 0.8 * c[0];
                    [t, t * (0.80 + 0.06 * c[1]), t * (0.68 + 0.08 * c[2])]
                } else {
                    c.map(|v| g + 0.2 * (v - g))
                }
            })
            .collect()
    }

    /// The camera's monochrome: its own channel weights in linear
    /// light, then an S-curve in the encoding, grey out.
    fn mono_camera(x: [f32; 3]) -> [f32; 3] {
        let lin = decode3(x);
        let e = srgb_encode((0.45 * lin[0] + 0.45 * lin[1] + 0.10 * lin[2]).clamp(0.0, 1.0));
        [e - 0.3 * e * (1.0 - e) * (1.0 - 2.0 * e); 3]
    }

    /// Saturated colors no portrait shows.
    const SATURATED: [[f32; 3]; 6] = [
        [0.9, 0.15, 0.15],
        [0.15, 0.15, 0.9],
        [0.15, 0.8, 0.15],
        [0.85, 0.2, 0.85],
        [0.9, 0.8, 0.1],
        [0.1, 0.8, 0.85],
    ];

    fn chroma_of(e: [f32; 3]) -> f32 {
        let l = oklab(decode3(e));
        l[1].hypot(l[2])
    }

    fn lightness(e: [f32; 3]) -> f32 {
        oklab(decode3(e))[0]
    }

    /// Issue 144: a monochrome style fitted on a portrait shoot, whose
    /// muted colors say nothing of a saturated red or blue. Fitted as
    /// color, those kept much of the develop's own color (the matrix's
    /// ridge toward identity, and the table's pull to the matrix and
    /// curves where no pair reaches); fitted as monochrome, every node
    /// is a grey, at the camera's lightness where the shoot has data.
    #[test]
    fn a_monochrome_set_renders_every_color_grey() {
        let x = portrait(6000);
        let y: Vec<[f32; 3]> = x.iter().map(|c| mono_camera(*c)).collect();
        let reading = crate::mono::Reading::of(&x, &y);
        eprintln!("the portrait shoot reads {reading:?}");
        assert!(reading.monochrome());
        let before = Model::fit_stages(&x, &y, LutParams::default());
        let leak: Vec<f32> = SATURATED
            .iter()
            .map(|s| chroma_of(before.apply(*s)))
            .collect();
        eprintln!("fitted as color, saturated colors keep chroma {leak:.4?}");
        assert!(leak.iter().all(|c| *c > 0.04), "{leak:?}");

        let m = Model::fit(&x, &y, LutParams::default());
        assert!(m.monochrome);
        let n = LUT_SIZE;
        for i in 0..n * n * n {
            let o = m.apply(node_color(i, n));
            assert!(o[0] == o[1] && o[1] == o[2], "node {i}: {o:?}");
        }
        assert!(m.apply([1.0; 3]).iter().all(|v| (v - 1.0).abs() < 1e-6));
        assert!(SATURATED.iter().all(|s| chroma_of(m.apply(*s)) < 1e-4));
        // Lightness where the shoot has data: the camera's, as closely
        // as the color fit holds it, under a fifth of a visible step on
        // average and under one at the 99th percentile.
        let errors = |m: &Model| {
            let mut err: Vec<f32> = x
                .iter()
                .zip(&y)
                .map(|(c, t)| (lightness(m.apply(*c)) - lightness(*t)).abs())
                .collect();
            err.sort_by(f32::total_cmp);
            let mean = err.iter().sum::<f32>() / err.len() as f32;
            (mean, err[err.len() * 99 / 100])
        };
        let ((mean, p99), (was, was99)) = (errors(&m), errors(&before));
        eprintln!(
            "lightness on the data: mean {mean:.4}, 99th percentile {p99:.4}; \
             as color {was:.4}, {was99:.4}"
        );
        assert!(mean < 0.004 && p99 < 0.02, "{mean} {p99}");
        assert!(mean < was + 0.0005 && p99 < was99 + 0.002, "{mean} {p99}");
        // Where it has none, the lightness the color fit would have
        // given: the monochrome fit changes the color there, not the
        // lightness.
        for s in SATURATED {
            let (o, c) = (lightness(m.apply(s)), lightness(before.apply(s)));
            eprintln!(
                "{s:?}: {o:.4}, as color {c:.4}, the camera's {:.4}",
                lightness(mono_camera(s))
            );
            assert!((o - c).abs() < 0.002, "{s:?}: {o} against {c}");
        }
        // The grey check: no cast to drift, and the same lightness
        // along the axis as fitted as color (whose fall at black, where
        // this shoot has nothing, is the synthetic set's own).
        let (axis, was) = (m.grey_axis(), before.grey_axis());
        assert!(axis.drift < 1e-5, "{axis:?}");
        assert!(
            (axis.fall - was.fall).abs() < 1e-4,
            "{axis:?} against {was:?}"
        );
    }

    /// A camera's slight cast on its greys is still a monochrome set,
    /// fitted to the same lightness and rendered neutral.
    #[test]
    fn a_slight_cast_is_monochrome_and_dropped() {
        let x = portrait(3000);
        let warm: Vec<[f32; 3]> = x
            .iter()
            .map(|c| {
                let l = lightness(mono_camera(*c));
                encode3(from_oklab([l, 0.002, 0.004]))
            })
            .collect();
        assert!(crate::mono::is_monochrome(&x, &warm));
        let m = Model::fit(&x, &warm, LutParams::default());
        assert!(m.monochrome);
        let o = m.apply([0.5, 0.4, 0.35]);
        assert!(o[0] == o[1] && o[1] == o[2], "{o:?}");
    }

    /// A color set takes the color fit, exactly as before.
    #[test]
    fn a_color_set_fits_as_before() {
        let x = portrait(3000);
        let y: Vec<[f32; 3]> = x
            .iter()
            .map(|v| [(v[0] * 1.04).min(1.0), v[1], v[2] * 0.95])
            .collect();
        assert!(!crate::mono::is_monochrome(&x, &y));
        let m = Model::fit(&x, &y, LutParams::default());
        assert!(!m.monochrome);
        assert_eq!(m, Model::fit_stages(&x, &y, LutParams::default()));
    }

    /// A set grey on both sides takes the color fit, as before.
    #[test]
    fn a_set_grey_on_both_sides_fits_as_color() {
        // A color style on fog: the develop's blocks near grey too, and
        // the camera's a little brighter. Nothing says the style drops
        // color, so it is not fitted as a grey.
        let x: Vec<[f32; 3]> = colors(3000)
            .into_iter()
            .map(|c| {
                let g = (c[0] + c[1] + c[2]) / 3.0;
                c.map(|v| g + 0.02 * (v - g))
            })
            .collect();
        let y: Vec<[f32; 3]> = x.iter().map(|c| c.map(|v| (v * 1.05).min(1.0))).collect();
        let reading = crate::mono::Reading::of(&x, &y);
        assert!(
            reading.camera.unwrap() < crate::mono::MONO_CHROMA,
            "{reading:?}"
        );
        let m = Model::fit(&x, &y, LutParams::default());
        assert!(!m.monochrome, "{reading:?}");
        assert_eq!(m, Model::fit_stages(&x, &y, LutParams::default()));
    }

    /// A sepia set, its chroma a function of lightness along one hue,
    /// is not monochrome, and fits as a color set does.
    #[test]
    fn a_sepia_set_is_not_monochrome() {
        let x = portrait(3000);
        let y: Vec<[f32; 3]> = x
            .iter()
            .map(|c| {
                let l = lightness(mono_camera(*c));
                let s = 4.0 * l * (1.0 - l) * 0.03;
                encode3(from_oklab([l, 0.4 * s, s]))
            })
            .collect();
        let p = crate::mono::chroma_percentile(&y);
        assert!(!crate::mono::is_monochrome(&x, &y), "{p:?}");
        let m = Model::fit(&x, &y, LutParams::default());
        assert!(!m.monochrome);
        assert_eq!(m, Model::fit_stages(&x, &y, LutParams::default()));
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
        // the correction there, in Oklab, is near zero, held by the
        // pull and, in lightness, the flatness term.
        let c = m.correction([0.0, 0.0, 0.0]);
        assert!(c.iter().all(|v| v.abs() < 0.01), "{c:?}");
    }

    /// One shoot under warm light: every pair in an orange cluster,
    /// and the camera's green lower there than the develop's. Nothing
    /// says what a grey does. Without the hold the curves and the
    /// table carry the cluster's green onto the greys, which go
    /// magenta; with it the greys from mid-grey up stay neutral at the
    /// matrix and curves' lightness, and the cluster fits as well.
    #[test]
    fn greys_no_pair_reaches_stay_neutral() {
        let x: Vec<[f32; 3]> = colors(4000)
            .into_iter()
            .map(|c| [0.5 + 0.4 * c[0], 0.3 + 0.25 * c[1], 0.1 + 0.2 * c[2]])
            .collect();
        let y: Vec<[f32; 3]> = x
            .iter()
            .map(|c| [c[0], c[1] * 0.88, (c[2] + 0.04).min(1.0)])
            .collect();
        let held = Model::fit(&x, &y, LutParams::default());
        let free = Model::fit(
            &x,
            &y,
            LutParams {
                hold: 0.0,
                ..LutParams::default()
            },
        );
        let spread = |m: &Model| {
            (16..33)
                .map(|k| {
                    let o = m.apply([k as f32 / 32.0; 3]);
                    o[0].max(o[1]).max(o[2]) - o[0].min(o[1]).min(o[2])
                })
                .fold(0.0f32, f32::max)
        };
        eprintln!(
            "grey spread from mid-grey up: {} held, {} free",
            spread(&held),
            spread(&free)
        );
        assert!(spread(&free) > 0.05, "without the hold {}", spread(&free));
        assert!(spread(&held) < 0.02, "with it {}", spread(&held));
        // The greys come out at the matrix and curves' own lightness,
        // the hold leaving lightness to them where no data says
        // otherwise; white itself (step 32) is pinned to white, so it is
        // left out.
        for k in 16..32 {
            let g = [k as f32 / 32.0; 3];
            let (out, base) = (oklab(decode3(held.apply(g)))[0], held.base_lab(g)[0]);
            assert!((out - base).abs() < 0.01, "{g:?}: {out} against {base}");
        }
        assert!(held.grey_axis().holds(), "{:?}", held.grey_axis());
        let (a, b) = (held.mean_delta_e(&x, &y), free.mean_delta_e(&x, &y));
        assert!(a < b + 0.001, "fitted {a} against {b}");
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

    /// The flatness operator on a 9³ lattice with the input's own
    /// lightness: symmetric, never negative, and nothing on that
    /// lightness itself, so a model whose lightness is its input's is
    /// not penalized; `rises` is zero on the last face, which the
    /// operator does not reach.
    #[test]
    fn the_flatness_operator_is_symmetric_positive_and_free_along_lightness() {
        let n = 9;
        let nodes = n * n * n;
        let lightness: Vec<f32> = (0..nodes)
            .map(|i| oklab(decode3(node_color(i, n)))[0])
            .collect();
        let r = rises(&lightness, n);
        let weights: Vec<f32> = (0..nodes).map(|i| 1.0 + (i % 5) as f32).collect();
        let apply = |v: &[f32]| {
            let mut out = vec![0.0; nodes];
            across_chroma_gram(v, &mut out, n, &r, &weights);
            out
        };
        let u: Vec<f32> = (0..nodes).map(|i| ((i * 7) % 11) as f32 / 11.0).collect();
        let v: Vec<f32> = (0..nodes).map(|i| ((i * 3) % 13) as f32 / 13.0).collect();
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        let (au, av) = (apply(&u), apply(&v));
        assert!((dot(&u, &av) - dot(&v, &au)).abs() < 1e-3);
        assert!(dot(&u, &au) >= 0.0 && dot(&v, &av) >= 0.0);
        let along = apply(&lightness);
        assert!(
            along.iter().all(|x| x.abs() < 1e-5),
            "{:?}",
            along.iter().fold(0.0f32, |m, x| m.max(x.abs()))
        );
        for (i, ri) in r.iter().enumerate() {
            let at = [i % n, (i / n) % n, i / (n * n)];
            if at.contains(&(n - 1)) {
                assert_eq!(*ri, [0.0; 3]);
            }
        }
    }

    /// The flatness term's share: whole up to [`FLAT_CHROMA`], none
    /// past [`FLAT_CHROMA_END`], falling between.
    #[test]
    fn the_flatness_fades_out_past_the_common_colors() {
        assert_eq!(flat_reach(0.0), 1.0);
        assert_eq!(flat_reach(FLAT_CHROMA), 1.0);
        assert_eq!(flat_reach(FLAT_CHROMA_END), 0.0);
        assert_eq!(flat_reach(0.3), 0.0);
        let mid = flat_reach((FLAT_CHROMA + FLAT_CHROMA_END) / 2.0);
        assert!((mid - 0.5).abs() < 1e-5, "{mid}");
        let steps: Vec<f32> = (0..=20)
            .map(|k| flat_reach(0.1 + k as f32 * 0.005))
            .collect();
        assert!(steps.windows(2).all(|w| w[1] <= w[0]));
    }
}
