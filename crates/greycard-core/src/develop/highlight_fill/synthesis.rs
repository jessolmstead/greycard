//! The fill's synthesis (SPEC.md stage 10): each region's 12 outputs
//! become its parameters, and with the analysis's planes two half-size
//! fields, a gain in stops and an Oklab ab offset, which
//! [`Fields`](super::apply::Fields) keeps for the apply (stage 11).
//!
//! Every expression is the oracle's (`oracle.py`, `numbers`) in its
//! order, f32 throughout, with no fused multiply-add (Rust never
//! contracts) and libm's exp, tanh, and log1p, so the bits are the same
//! on every machine. The look's numbers are rounded where SPEC's
//! "Precision with an arbitrary f" says.
//!
//! Both fields are zero off the fa cells: s, mix, and every color term
//! are multiplied by fa. So we compute them only on windows around
//! clusters of regions ([`windows`] says why that is the whole grid's
//! result to the bit), one window at a time.

use rayon::prelude::*;

use super::analysis::Analysis;
use super::apply::{Fields, TILE};
use super::blur::{blur, blur_normalized, sg};
use super::net::{FillLook, OUTPUTS, Reach};
use super::{OFF, SIGMA_DEPTH, SIGMA_MIX};

/// One region's parameters from its outputs r0 … r11 (SPEC stage 10).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Params {
    /// The peak gain, stops: 5·tanh(r0/5).
    pub h: f32,
    /// The falloff, px: clamp(depth·exp(clamp(r1, ±20)), 1, 4096).
    pub l: f32,
    /// The harmonic's share of the core's color: σ(r2).
    pub beta: f32,
    /// The band's share of the core's color: σ(r3).
    pub gamma: f32,
    /// The band color's scale: exp(0.7·tanh r4).
    pub kappa: f32,
    /// A constant ab offset, rising with s: 0.03·tanh(r5, r6).
    pub c: [f32; 2],
    /// The band's share of the gain: 1.5·σ(r7).
    pub beta_b: f32,
    /// The rim's share of the band's color: σ(r8).
    pub gamma_r: f32,
    /// The rim color's scale: exp(0.7·tanh r9).
    pub kappa_r: f32,
    /// The core's rise, stops: 5·tanh(r10/5).
    pub h2: f32,
    /// The core's falloff, px: clamp(cdepth·exp(clamp(r11, ±20)), 1, 4096).
    pub l2: f32,
}

/// torch's sigmoid on the CPU: 1 / (1 + exp(−x)).
#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + libm::expf(-x))
}

impl Params {
    /// From a row's outputs (the outside row's already zeroed) and its
    /// depth and cdepth (SPEC stage 10), in the oracle's order.
    pub fn new(r: &[f32; OUTPUTS], depth: f32, cdepth: f32) -> Self {
        let falloff = |d: f32, r: f32| (d * libm::expf(r.clamp(-20.0, 20.0))).clamp(1.0, 4096.0);
        let scale = |r: f32| libm::expf(0.7 * libm::tanhf(r));
        Self {
            h: 5.0 * libm::tanhf(r[0] / 5.0),
            l: falloff(depth, r[1]),
            beta: sigmoid(r[2]),
            gamma: sigmoid(r[3]),
            kappa: scale(r[4]),
            c: [0.03 * libm::tanhf(r[5]), 0.03 * libm::tanhf(r[6])],
            beta_b: 1.5 * sigmoid(r[7]),
            gamma_r: sigmoid(r[8]),
            kappa_r: scale(r[9]),
            h2: 5.0 * libm::tanhf(r[10] / 5.0),
            l2: falloff(cdepth, r[11]),
        }
    }

    /// The fixtures' `params` row: H, L, β, γ, κ, C_a, C_b, β_b, γ_r,
    /// κ_r, H2, L2.
    #[cfg(test)]
    pub fn row(&self) -> [f32; OUTPUTS] {
        [
            self.h,
            self.l,
            self.beta,
            self.gamma,
            self.kappa,
            self.c[0],
            self.c[1],
            self.beta_b,
            self.gamma_r,
            self.kappa_r,
            self.h2,
            self.l2,
        ]
    }
}

/// The look at scale f, rounded as SPEC's "Precision with an arbitrary
/// f" says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Scaled {
    /// σ_g = max(gain_sigma / f, 1) in f64; None when it is exactly
    /// [`SIGMA_MIX`] (2.0), where the oracle skips the wide branch and
    /// mixg is mix.
    pub gain_sigma: Option<f64>,
    /// tie_lo = depth_tie₀ / f in f64, rounded.
    pub tie_lo: f32,
    /// depth_tie₁ / f − depth_tie₀ / f in f64, rounded.
    pub tie_den: f32,
    /// boost − 1 in f64, rounded (f32 0.4 for hl-1's 1.4).
    pub boost: f32,
    /// The whitening, rounded (0.5, exact).
    pub whiten: f32,
    /// hl-2's rules at this scale; None for hl-1.
    pub reach: Option<ScaledReach>,
}

/// A [`Reach`] at look scale f: each length over f in f64, rounded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScaledReach {
    pub gate_lo: f32,
    pub gate_hi: f32,
    pub ramp: Option<f32>,
    /// The neighbors' reach in half-size cells: round(near / f / 2).
    pub near: Option<usize>,
}

impl ScaledReach {
    pub(crate) fn new(r: &Reach, f: f64) -> Self {
        assert!(
            r.gate[0] < r.gate[1] && r.ramp.is_none_or(|v| v > 0.0),
            "a gate's shoulder rises and a ramp has a length: {r:?}"
        );
        Self {
            gate_lo: (r.gate[0] / f) as f32,
            gate_hi: (r.gate[1] / f) as f32,
            ramp: r.ramp.map(|v| (v / f) as f32),
            near: r.near.map(|v| (v / f / 2.0).round() as usize),
        }
    }

    /// A region's share of its fields by its depth0, px: 1 to
    /// `gate_lo`, 0 from `gate_hi` (exactly), a smoothstep down between.
    pub fn gate(&self, depth0: f32) -> f32 {
        if depth0 >= self.gate_hi {
            0.0
        } else if depth0 <= self.gate_lo {
            1.0
        } else {
            1.0 - smoothstep((depth0 - self.gate_lo) / (self.gate_hi - self.gate_lo))
        }
    }

    /// A core cell's share by its core distance `e` (px past the
    /// core's first two layers) in a region whose deepest is `e_max`:
    /// smoothstep over min(ramp, e_max); 0 where that is 0. 1 without
    /// a ramp.
    pub fn ramp(&self, e: f32, e_max: f32) -> f32 {
        let Some(ramp) = self.ramp else {
            return 1.0;
        };
        let span = ramp.min(e_max);
        if span > 0.0 {
            smoothstep(e / span)
        } else {
            0.0
        }
    }
}

/// 3t² − 2t³ on t clamped to [0, 1]: 0 and 1 at the ends exactly, its
/// slope 0 at both.
#[inline]
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Scaled {
    pub fn new(look: &FillLook, f: f64) -> Self {
        assert!(
            f.is_finite() && f > 0.0,
            "the look scale is a positive number, not {f}"
        );
        let sigma = (look.gain_sigma / f).max(1.0);
        let (lo, hi) = (look.depth_tie[0] / f, look.depth_tie[1] / f);
        Self {
            gain_sigma: (sigma != SIGMA_MIX).then_some(sigma),
            tie_lo: lo as f32,
            tie_den: (hi - lo) as f32,
            boost: (look.core_boost - 1.0) as f32,
            whiten: look.core_whiten as f32,
            reach: look.reach.map(|r| ScaledReach::new(&r, f)),
        }
    }
}

/// A blur's radius in cells, r = int(3·sg(σ)), as [`blur`]'s kernel.
fn radius(sigma: f64) -> usize {
    (3.0 * sg(sigma)) as usize
}

/// The largest radius of the blurs the synthesis takes at this look:
/// the depth fields' σ 3, the hand-over's σ 2, and σ_g when it runs.
fn reach(look: &Scaled) -> usize {
    [Some(SIGMA_DEPTH), Some(SIGMA_MIX), look.gain_sigma]
        .into_iter()
        .flatten()
        .map(radius)
        .max()
        .unwrap_or(0)
}

/// A box of the half grid, row-major within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Window {
    pub x0: usize,
    pub y0: usize,
    pub width: usize,
    pub height: usize,
}

impl Window {
    /// The grid index of window cell `j`.
    #[inline]
    fn grid(&self, j: usize, wh: usize) -> usize {
        (self.y0 + j / self.width) * wh + self.x0 + j % self.width
    }

    /// Window cell `j` of grid cell (`x`, `y`), if inside.
    #[cfg(test)]
    fn local(&self, x: usize, y: usize) -> Option<usize> {
        let inside = (self.x0..self.x0 + self.width).contains(&x)
            && (self.y0..self.y0 + self.height).contains(&y);
        inside.then(|| (y - self.y0) * self.width + x - self.x0)
    }

    /// A plane of the window from the grid's cells.
    fn gather<U: Send>(&self, a: &Analysis, f: impl Fn(usize) -> U + Sync) -> Vec<U> {
        (0..self.width * self.height)
            .into_par_iter()
            .map(|j| f(self.grid(j, a.half_width)))
            .collect()
    }
}

/// An inclusive box of cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bounds {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Bounds {
    fn union(self, o: Self) -> Self {
        Self {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// The windows the synthesis runs on (SPEC stage 10 computed on part of
/// the grid, to the bit).
///
/// Every blur's input ((d − off)₊·fa, fa, and fc) is zero off the fa
/// cells, and every value the fields keep is at an fa cell. Regions are
/// grouped into clusters, merging any two whose bounding boxes come
/// within `reach` cells of each other (Chebyshev) until no two do; a
/// cluster's window is its box widened by one cell and clipped to the
/// grid. Then, for an fa cell c of a cluster, every tap either pass of
/// a blur of radius ≤ `reach` reads lies within `reach` of c, so it is
/// in the window or a zero cell outside every cluster; and a tap the
/// window's edge replicates lands on the widened border, which holds no
/// fa cell of any cluster (another cluster's box is more than `reach`
/// ≥ 1 cells away), so it reads zero as the grid does, or on the grid's
/// own edge, which the grid replicates alike. Each tap, and so each
/// sum in its order, is the whole grid's: c's values are the whole
/// grid's to the bit.
pub(crate) fn windows(a: &Analysis, reach: usize) -> Vec<Window> {
    let (wh, hh) = (a.half_width, a.half_height);
    let mut boxes: Vec<Option<Bounds>> = vec![None; a.regions.len()];
    for (i, c) in a.cells.iter().enumerate() {
        if c.fa() {
            let (x, y) = (i % wh, i / wh);
            let b = Bounds {
                x0: x,
                y0: y,
                x1: x,
                y1: y,
            };
            let slot = &mut boxes[a.labels.inv[i] as usize];
            *slot = Some(slot.map_or(b, |s| s.union(b)));
        }
    }
    let mut boxes: Vec<Bounds> = boxes.into_iter().flatten().collect();
    // Merge until no two boxes come within reach: a sweep down the rows
    // over the boxes still in reach, repeated while it merges (a merged
    // box can reach a box neither part reached).
    loop {
        boxes.sort_by_key(|b| (b.y0, b.x0));
        let mut parent: Vec<usize> = (0..boxes.len()).collect();
        let mut active: Vec<usize> = Vec::new();
        for (i, b) in boxes.iter().enumerate() {
            active.retain(|&k| boxes[k].y1 + reach >= b.y0);
            for &k in &active {
                let o = boxes[k];
                if o.x0 <= b.x1 + reach && b.x0 <= o.x1 + reach {
                    let (ri, rk) = (root(&mut parent, i), root(&mut parent, k));
                    parent[ri.max(rk)] = ri.min(rk);
                }
            }
            active.push(i);
        }
        let mut merged: Vec<Option<Bounds>> = vec![None; boxes.len()];
        for (i, b) in boxes.iter().enumerate() {
            let r = root(&mut parent, i);
            merged[r] = Some(merged[r].map_or(*b, |m| m.union(*b)));
        }
        let merged: Vec<Bounds> = merged.into_iter().flatten().collect();
        let done = merged.len() == boxes.len();
        boxes = merged;
        if done {
            break;
        }
    }
    boxes.sort_by_key(|b| (b.y0, b.x0));
    boxes
        .into_iter()
        .map(|b| {
            let (x0, y0) = (b.x0.saturating_sub(1), b.y0.saturating_sub(1));
            let (x1, y1) = ((b.x1 + 1).min(wh - 1), (b.y1 + 1).min(hh - 1));
            Window {
                x0,
                y0,
                width: x1 + 1 - x0,
                height: y1 + 1 - y0,
            }
        })
        .collect()
}

/// Every value the synthesis makes at one cell (SPEC stage 10), named
/// as the oracle records them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct CellOut {
    pub s: f32,
    pub mix: f32,
    pub mixg: f32,
    /// g, the band's gain, before the core's rise.
    pub gain_band: f32,
    pub s2: f32,
    pub tie: f32,
    pub core_dab: [f32; 2],
    pub band_dab: [f32; 2],
    /// d, the two colors mixed, before the whitening.
    pub dab_mixed: [f32; 2],
    pub keep: f32,
    /// hl-2's share of the fields here, gate times ramp (1 for hl-1);
    /// `gain` and `dab` are taken times it.
    pub weight: f32,
    pub gain: f32,
    pub dab: [f32; 2],
}

/// One window's blurred planes: dd, dc, blur(fc, 2), and blur(fc, σ_g)
/// when the wide branch runs; and under hl-2's reach each cell's core
/// distance ([`core_distance`]).
struct Planes {
    w: Window,
    dd: Vec<f32>,
    dc: Vec<f32>,
    mix_blur: Vec<f32>,
    mixg_blur: Option<Vec<f32>>,
    core: Option<Vec<f32>>,
}

impl Planes {
    fn new(a: &Analysis, look: &Scaled, off: &[f32], w: Window) -> Self {
        let (ww, wh) = (w.width, w.height);
        let fa: Vec<bool> = w.gather(a, |i| a.cells[i].fa());
        let fc: Vec<bool> = w.gather(a, |i| a.cells[i].fc());
        // (d − off)₊, which blurN takes times m: the oracle's
        // (d − off)₊ · m (SPEC stage 10: dd and dc).
        let depth = |d: &[u16], m: &[bool]| {
            let v: Vec<f32> = w.gather(a, |i| {
                let o = off[a.labels.inv[i] as usize];
                (d[i] as f32 - o).max(0.0)
            });
            blur_normalized(&v, m, ww, wh)
        };
        let dd = depth(&a.d0, &fa);
        drop(fa);
        let dc = depth(&a.dc0, &fc);
        let core = look
            .reach
            .and_then(|r| r.ramp)
            .map(|_| core_distance(&fc, w));
        let fcf: Vec<f32> = fc.iter().map(|&b| b as u32 as f32).collect();
        drop(fc);
        let mix_blur = blur(&fcf, ww, wh, SIGMA_MIX);
        let mixg_blur = look.gain_sigma.map(|s| blur(&fcf, ww, wh, s));
        Self {
            w,
            dd,
            dc,
            mix_blur,
            mixg_blur,
            core,
        }
    }
}

/// Each window cell's core distance for hl-2's ramp, px: on an fc cell
/// 2·(√d² − √2)₊, d² the squared Euclidean distance in cells to the
/// nearest cell of the window not fc ([`squared_distance`]); 0 off fc.
/// So the core's first two layers, and every cell sharing a 2×2 block
/// with a cell not fc, read 0, and no pixel whose four upsampling cells
/// include one not fc sees a field. Exact on the whole grid: the
/// window's border off the grid's edge is not fa ([`windows`]), and a
/// cell beyond it is no nearer than the border's cell in its row or
/// column; past the grid's edge there is nothing, so the frame's edge
/// is no core edge. Infinite where the window has no cell not fc.
fn core_distance(fc: &[bool], w: Window) -> Vec<f32> {
    let not_fc: Vec<bool> = fc.iter().map(|&b| !b).collect();
    let d2 = squared_distance(&not_fc, w.width, w.height);
    fc.iter()
        .zip(d2)
        .map(|(&inside, d2)| {
            if inside {
                (2.0 * (d2.sqrt() - std::f64::consts::SQRT_2).max(0.0)) as f32
            } else {
                0.0
            }
        })
        .collect()
}

/// The exact squared Euclidean distance, in cells, from each cell of a
/// `width` × `height` grid to the nearest cell where `source` holds
/// (infinite where none does): a column pass, then the lower envelope
/// of parabolas along each row (Felzenszwalb and Huttenlocher, 2012).
/// Every value is an integer in f64 and every comparison is of exact
/// sums, so the bits are the same on any machine and thread count.
pub(crate) fn squared_distance(source: &[bool], width: usize, height: usize) -> Vec<f64> {
    assert_eq!(source.len(), width * height);
    let mut g: Vec<f64> = source
        .iter()
        .map(|&s| if s { 0.0 } else { f64::INFINITY })
        .collect();
    for y in 1..height {
        let (above, row) = g.split_at_mut(y * width);
        let above = &above[(y - 1) * width..];
        for (v, &u) in row[..width].iter_mut().zip(above) {
            *v = v.min(u + 1.0);
        }
    }
    for y in (0..height.saturating_sub(1)).rev() {
        let (row, below) = g.split_at_mut((y + 1) * width);
        let row = &mut row[y * width..];
        for (v, &u) in row.iter_mut().zip(&below[..width]) {
            *v = v.min(u + 1.0);
        }
    }
    let mut out = vec![0.0f64; width * height];
    out.par_chunks_mut(width.max(1))
        .zip(g.par_chunks(width.max(1)))
        .for_each(|(out, g)| envelope(g, out));
    out
}

/// One row's pass: out[x] = min over q of (x − q)² + g[q]²,
/// infinite where every g is.
fn envelope(g: &[f64], out: &mut [f64]) {
    let n = g.len();
    let f = |q: usize| g[q] * g[q];
    let mut v: Vec<usize> = Vec::with_capacity(n);
    let mut z: Vec<f64> = Vec::with_capacity(n);
    for q in (0..n).filter(|&q| g[q].is_finite()) {
        let fq = f(q) + (q * q) as f64;
        loop {
            let Some(&p) = v.last() else {
                v.push(q);
                z.push(f64::NEG_INFINITY);
                break;
            };
            let s = (fq - (f(p) + (p * p) as f64)) / (2.0 * (q - p) as f64);
            if s <= *z.last().unwrap_or(&f64::NEG_INFINITY) && v.len() > 1 {
                v.pop();
                z.pop();
                continue;
            }
            v.push(q);
            z.push(s);
            break;
        }
    }
    if v.is_empty() {
        out.fill(f64::INFINITY);
        return;
    }
    let mut k = 0;
    for (x, o) in out.iter_mut().enumerate() {
        while k + 1 < v.len() && z[k + 1] < x as f64 {
            k += 1;
        }
        let d = x as f64 - v[k] as f64;
        *o = d * d + f(v[k]);
    }
}

/// Each row's gate shared with its neighbors (hl-2's [`Reach::near`]):
/// the smallest gate of any region with an fa cell within `near` cells
/// (chessboard) of one of the row's own, its own included. Read on the
/// whole grid, so a neighbor in another window counts, and the windows
/// and the whole grid give the same gates. Where every gate is 1 the
/// gates are returned as they are.
///
/// [`Reach::near`]: super::net::Reach::near
pub(crate) fn shared_gates(a: &Analysis, gate: &[f32], near: usize) -> Vec<f32> {
    let (wh, hh) = (a.half_width, a.half_height);
    if near == 0 || gate.iter().all(|&g| g >= 1.0) {
        return gate.to_vec();
    }
    // Each fa cell's region's gate, 1 elsewhere (no gate is above 1, so
    // 1 is the min filter's neutral), then the smallest within a square
    // of radius `near`.
    let field: Vec<f32> = (0..wh * hh)
        .into_par_iter()
        .map(|i| {
            if a.cells[i].fa() {
                gate[a.labels.inv[i] as usize]
            } else {
                1.0
            }
        })
        .collect();
    let spread = min_square(&field, wh, hh, near);
    let mut shared = gate.to_vec();
    for (i, &s) in spread.iter().enumerate() {
        if a.cells[i].fa() {
            let r = a.labels.inv[i] as usize;
            shared[r] = shared[r].min(s);
        }
    }
    shared
}

/// The smallest value within a (2r + 1)² square of each cell, cells
/// past the grid's edge reading 1: van Herk and Gil-Werman along the
/// rows, then the columns. Minimums are exact, so the same bits on any
/// thread count.
fn min_square(v: &[f32], width: usize, height: usize, r: usize) -> Vec<f32> {
    let rows = min_lines(v, width, height, r);
    let t = transpose(&rows, width, height);
    transpose(&min_lines(&t, height, width, r), height, width)
}

fn transpose(v: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; width * height];
    out.par_chunks_mut(height.max(1))
        .enumerate()
        .for_each(|(x, col)| {
            for (y, o) in col.iter_mut().enumerate() {
                *o = v[y * width + x];
            }
        });
    out
}

/// Each line of `width` cells: the smallest within r of each cell.
fn min_lines(v: &[f32], width: usize, height: usize, r: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; width * height];
    let k = 2 * r + 1;
    out.par_chunks_mut(width.max(1))
        .zip(v.par_chunks(width.max(1)))
        .for_each(|(out, line)| {
            // The line padded with r ones each side, cut in blocks of k
            // from its start: g the running minimum from each block's
            // start, h from each block's end; the window of k starting
            // at padded index i is min(h[i], g[i + 2r]).
            let n = line.len() + 2 * r;
            let at = |i: usize| {
                if i < r || i >= r + line.len() {
                    1.0
                } else {
                    line[i - r]
                }
            };
            let mut g = vec![0.0f32; n];
            let mut h = vec![0.0f32; n];
            for i in 0..n {
                g[i] = if i % k == 0 {
                    at(i)
                } else {
                    g[i - 1].min(at(i))
                };
            }
            for i in (0..n).rev() {
                h[i] = if i % k == k - 1 || i == n - 1 {
                    at(i)
                } else {
                    h[i + 1].min(at(i))
                };
            }
            for (x, o) in out.iter_mut().enumerate() {
                *o = h[x].min(g[x + 2 * r]);
            }
        });
    out
}

/// Per row of the region table: off, the largest dd and dc so far, and
/// the parameters and tie of each row whose window has run.
pub(crate) struct Tables {
    pub off: Vec<f32>,
    depth: Vec<f32>,
    cdepth: Vec<f32>,
    pub params: Vec<Params>,
    pub tie: Vec<f32>,
    /// Under hl-2's reach, each row's gate and its deepest core
    /// distance, px (0 without a reach).
    pub gate: Vec<f32>,
    pub core_max: Vec<f32>,
}

impl Tables {
    fn new(a: &Analysis, look: &Scaled) -> Self {
        let n = a.regions.len();
        let mut gate: Vec<f32> = a
            .regions
            .iter()
            .map(|r| look.reach.map_or(1.0, |reach| reach.gate(r.depth0)))
            .collect();
        if let Some(near) = look.reach.and_then(|r| r.near) {
            gate = shared_gates(a, &gate, near);
        }
        Self {
            // off = min(4, 0.5·depth0) per row.
            off: a.regions.iter().map(|r| OFF.min(0.5 * r.depth0)).collect(),
            depth: vec![0.0; n],
            cdepth: vec![0.0; n],
            params: vec![Params::default(); n],
            tie: vec![0.0; n],
            gate,
            core_max: vec![0.0; n],
        }
    }

    /// The row's parameters and tie from its depths (SPEC stage 10):
    /// depth = max(max dd, 1), likewise cdepth, and
    /// tie = clamp((cdepth − tie_lo) / (tie_hi − tie_lo), 0, 1), the
    /// pixel thresholds over f (cdepth / S with S = 1 is cdepth).
    fn settle(&mut self, r: usize, outputs: &[[f32; OUTPUTS]], look: &Scaled) {
        let (d, c) = (self.depth[r].max(1.0), self.cdepth[r].max(1.0));
        self.params[r] = Params::new(&outputs[r], d, c);
        self.tie[r] = ((c - look.tie_lo) / look.tie_den).clamp(0.0, 1.0);
    }

    /// Whether any region in window `w` is short of hl-2's gate's end.
    fn any_open(&self, a: &Analysis, w: Window) -> bool {
        (0..w.height).any(|y| {
            (0..w.width).any(|x| {
                let i = (w.y0 + y) * a.half_width + w.x0 + x;
                a.cells[i].fa() && self.gate[a.labels.inv[i] as usize] > 0.0
            })
        })
    }

    /// Takes in a window's rows: each region lies whole in one window,
    /// so its depths are final here. The outside row's cells read dd and
    /// dc 0 (masked), so its depths stay 0 until [`Tables::settle`]
    /// clamps them to 1.
    fn absorb(&mut self, a: &Analysis, p: &Planes, outputs: &[[f32; OUTPUTS]], look: &Scaled) {
        let mut rows = Vec::new();
        for (j, (&d, &c)) in p.dd.iter().zip(&p.dc).enumerate() {
            let i = p.w.grid(j, a.half_width);
            if a.cells[i].fa() {
                let r = a.labels.inv[i] as usize;
                self.depth[r] = self.depth[r].max(d);
                self.cdepth[r] = self.cdepth[r].max(c);
                if let Some(core) = &p.core {
                    self.core_max[r] = self.core_max[r].max(core[j]);
                }
                rows.push(r);
            }
        }
        rows.sort_unstable();
        rows.dedup();
        for r in rows {
            self.settle(r, outputs, look);
        }
    }
}

/// mix from a blurred fc: clamp(blur · fa, 0, 1), at least 0.5 on fc.
#[inline]
fn hand_over(blurred: f32, fa: f32, fc: bool) -> f32 {
    let m = (blurred * fa).clamp(0.0, 1.0);
    if fc { m.max(0.5) } else { m }
}

/// Grid cell `i`'s values (SPEC stage 10), in the oracle's orders, from
/// its window's planes (all 0 off every window, as the whole grid's are
/// there after the masking by fa). The oracle's `· on` factors multiply
/// by exactly 1 and are left out.
fn cell(
    a: &Analysis,
    look: &Scaled,
    t: &Tables,
    at: Option<(&Planes, usize)>,
    i: usize,
) -> CellOut {
    let cell = a.cells[i];
    let (fb, fc) = (cell.fa() as u32 as f32, cell.fc());
    let r = a.labels.inv[i] as usize;
    let p = &t.params[r];
    let (dd, dc, mix_blur, mixg_blur, core) = match at {
        Some((pl, j)) => (
            pl.dd[j],
            pl.dc[j],
            pl.mix_blur[j],
            pl.mixg_blur.as_ref().map(|b| b[j]),
            pl.core.as_ref().map_or(0.0, |c| c[j]),
        ),
        None => (0.0, 0.0, 0.0, look.gain_sigma.map(|_| 0.0), 0.0),
    };
    let weight = match look.reach {
        None => 1.0,
        Some(reach) if reach.ramp.is_none() => t.gate[r],
        Some(reach) if fc => t.gate[r] * reach.ramp(core, t.core_max[r]),
        Some(_) => 0.0,
    };
    let s = (1.0 - libm::expf(-dd / p.l)) * fb;
    let mix = hand_over(mix_blur, fb, fc);
    let mixg = match mixg_blur {
        Some(b) => hand_over(b, fb, fc),
        None => mix,
    };
    let gain_band = p.h * s * (mixg + (1.0 - mixg) * p.beta_b);
    let cdepth = t.cdepth[r].max(1.0);
    let s2 = libm::log1pf(dc / p.l2) / libm::log1pf(cdepth / p.l2);
    let tie = t.tie[r];
    let gain = gain_band + mix * p.h2 * s2 * (1.0 + look.boost * tie);

    let region = &a.regions[r];
    let big_a = a.abh[i];
    let harm = a.harm(i);
    let has_band = (region.cbn > 0) as u32 as f32;
    let has_rim = (region.cbr > 0) as u32 as f32;
    let band_ab = [region.an * p.kappa, region.bn * p.kappa];
    let rim_ab = [region.ar * p.kappa_r, region.br * p.kappa_r];
    let cmix = if fc { 1.0 } else { mix };
    let keep = 1.0 - look.whiten * mix * s2 * tie;
    let mut out = CellOut {
        s,
        mix,
        mixg,
        gain_band,
        s2,
        tie,
        keep,
        weight,
        gain: gain * weight,
        ..CellOut::default()
    };
    for c in 0..2 {
        let core = p.beta * (harm[c] - big_a[c])
            + has_band * p.gamma * (band_ab[c] - big_a[c])
            + p.c[c] * s;
        let band = has_rim * p.gamma_r * (rim_ab[c] - big_a[c]) * s;
        let mixed = (core * cmix + band * (1.0 - cmix)) * fb;
        out.core_dab[c] = core;
        out.band_dab[c] = band;
        out.dab_mixed[c] = mixed;
        out.dab[c] = ((big_a[c] + mixed) * keep - big_a[c]) * fb * weight;
    }
    out
}

/// Writes a window's fa cells' gain and ab offset into `fields`, by
/// tile. Only fa cells are computed: every value elsewhere is a product
/// with fa = 0, a zero whose sign neither the apply's test nor its sums
/// can see. Tile rows in parallel, each cell alone, so the same on any
/// thread count.
fn write(a: &Analysis, look: &Scaled, t: &Tables, p: &Planes, fields: &mut Fields) {
    let (w, wh) = (p.w, a.half_width);
    let (tx0, tx1) = (w.x0 / TILE, (w.x0 + w.width - 1) / TILE);
    let (ty0, ty1) = (w.y0 / TILE, (w.y0 + w.height - 1) / TILE);
    let tiles: Vec<Vec<(usize, Vec<[f32; 3]>)>> = (ty0..=ty1)
        .into_par_iter()
        .map(|ty| {
            let mut row = Vec::new();
            for tx in tx0..=tx1 {
                let mut tile = vec![[0.0f32; 3]; TILE * TILE];
                let mut any = false;
                let ys = (ty * TILE).max(w.y0)..((ty + 1) * TILE).min(w.y0 + w.height);
                let xs = (tx * TILE).max(w.x0)..((tx + 1) * TILE).min(w.x0 + w.width);
                for y in ys {
                    for x in xs.clone() {
                        let i = y * wh + x;
                        if !a.cells[i].fa() {
                            continue;
                        }
                        let j = (y - w.y0) * w.width + x - w.x0;
                        let c = cell(a, look, t, Some((p, j)), i);
                        if c.weight == 0.0 {
                            continue;
                        }
                        let v = [c.gain, c.dab[0], c.dab[1]];
                        any |= v.iter().any(|&v| v != 0.0);
                        tile[(y - ty * TILE) * TILE + x - tx * TILE] = v;
                    }
                }
                if any {
                    row.push((tx, tile));
                }
            }
            row
        })
        .collect();
    for (ty, row) in (ty0..=ty1).zip(tiles) {
        for (tx, tile) in row {
            fields.merge(tx, ty, tile);
        }
    }
}

/// The fields (SPEC stage 10) of an analysis with the network's outputs
/// (a row of the region table each, the outside row's zeroed:
/// [`FillWeights::region_outputs`]) at the scaled look: window by
/// window ([`windows`]), each window's planes dropped before the next's
/// are made.
/// The analysis's d0 and dc0 (2 bytes a cell each) are read by every
/// window, so they live to the last: freeing them sooner would mean
/// holding every window's dd and dc (8 bytes a cell) at once.
///
/// [`FillWeights::region_outputs`]: super::net::FillWeights::region_outputs
pub(crate) fn fields(a: &Analysis, outputs: &[[f32; OUTPUTS]], look: Scaled) -> Fields {
    assert_eq!(outputs.len(), a.regions.len(), "an output row a region");
    let mut t = Tables::new(a, &look);
    let mut out = Fields::empty(a.width, a.height, a.half_width, a.half_height);
    for w in windows(a, reach(&look)) {
        // A window of regions all past hl-2's gate writes nothing:
        // its planes are not made.
        if look.reach.is_some() && !t.any_open(a, w) {
            continue;
        }
        let p = Planes::new(a, &look, &t.off, w);
        t.absorb(a, &p, outputs, &look);
        write(a, &look, &t, &p, &mut out);
    }
    out
}

/// The synthesis with every window's planes kept, for the tests: any
/// cell's values, and the per-row tables.
#[cfg(test)]
pub(crate) struct Synthesis<'a> {
    a: &'a Analysis,
    look: Scaled,
    planes: Vec<Planes>,
    pub tables: Tables,
}

#[cfg(test)]
impl<'a> Synthesis<'a> {
    /// On [`windows`], as [`fields`] runs.
    pub fn new(a: &'a Analysis, outputs: &[[f32; OUTPUTS]], look: Scaled) -> Self {
        Self::on(a, outputs, look, windows(a, reach(&look)))
    }

    /// On one window over the whole grid: the reference the windows
    /// must equal.
    pub fn whole(a: &'a Analysis, outputs: &[[f32; OUTPUTS]], look: Scaled) -> Self {
        let w = Window {
            x0: 0,
            y0: 0,
            width: a.half_width,
            height: a.half_height,
        };
        let any = a.cells.iter().any(|c| c.fa());
        Self::on(a, outputs, look, if any { vec![w] } else { Vec::new() })
    }

    fn on(a: &'a Analysis, outputs: &[[f32; OUTPUTS]], look: Scaled, ws: Vec<Window>) -> Self {
        let mut tables = Tables::new(a, &look);
        let planes: Vec<Planes> = ws
            .into_iter()
            .map(|w| {
                let p = Planes::new(a, &look, &tables.off, w);
                tables.absorb(a, &p, outputs, &look);
                p
            })
            .collect();
        // The rows no window settled (the outside row), as the oracle
        // computes them.
        for r in 0..outputs.len() {
            tables.settle(r, outputs, &look);
        }
        Self {
            a,
            look,
            planes,
            tables,
        }
    }

    pub fn windows(&self) -> Vec<Window> {
        self.planes.iter().map(|p| p.w).collect()
    }

    fn at(&self, i: usize) -> Option<(&Planes, usize)> {
        let (x, y) = (i % self.a.half_width, i / self.a.half_width);
        self.planes
            .iter()
            .find_map(|p| p.w.local(x, y).map(|j| (p, j)))
    }

    /// Grid cell `i`'s values.
    pub fn cell(&self, i: usize) -> CellOut {
        cell(self.a, &self.look, &self.tables, self.at(i), i)
    }

    /// dd and dc at grid cell `i` (0 off every window).
    pub fn depths(&self, i: usize) -> (f32, f32) {
        self.at(i).map_or((0.0, 0.0), |(p, j)| (p.dd[j], p.dc[j]))
    }

    /// Each row's depth and cdepth, clamped to at least 1.
    pub fn depth(&self) -> (Vec<f32>, Vec<f32>) {
        let one = |v: &[f32]| v.iter().map(|d| d.max(1.0)).collect();
        (one(&self.tables.depth), one(&self.tables.cdepth))
    }

    /// The fields, written window by window from the kept planes.
    pub fn fields(&self) -> Fields {
        let a = self.a;
        let mut out = Fields::empty(a.width, a.height, a.half_width, a.half_height);
        for p in &self.planes {
            write(a, &self.look, &self.tables, p, &mut out);
        }
        out
    }
}
