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
use super::net::{FillLook, OUTPUTS};
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
    pub gain: f32,
    pub dab: [f32; 2],
}

/// One window's blurred planes: dd, dc, blur(fc, 2), and blur(fc, σ_g)
/// when the wide branch runs.
struct Planes {
    w: Window,
    dd: Vec<f32>,
    dc: Vec<f32>,
    mix_blur: Vec<f32>,
    mixg_blur: Option<Vec<f32>>,
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
        }
    }
}

/// Per row of the region table: off, the largest dd and dc so far, and
/// the parameters and tie of each row whose window has run.
pub(crate) struct Tables {
    pub off: Vec<f32>,
    depth: Vec<f32>,
    cdepth: Vec<f32>,
    pub params: Vec<Params>,
    pub tie: Vec<f32>,
}

impl Tables {
    fn new(a: &Analysis) -> Self {
        let n = a.regions.len();
        Self {
            // off = min(4, 0.5·depth0) per row.
            off: a.regions.iter().map(|r| OFF.min(0.5 * r.depth0)).collect(),
            depth: vec![0.0; n],
            cdepth: vec![0.0; n],
            params: vec![Params::default(); n],
            tie: vec![0.0; n],
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
    let (dd, dc, mix_blur, mixg_blur) = match at {
        Some((pl, j)) => (
            pl.dd[j],
            pl.dc[j],
            pl.mix_blur[j],
            pl.mixg_blur.as_ref().map(|b| b[j]),
        ),
        None => (0.0, 0.0, 0.0, look.gain_sigma.map(|_| 0.0)),
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
        gain,
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
        out.dab[c] = ((big_a[c] + mixed) * keep - big_a[c]) * fb;
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
    let mut t = Tables::new(a);
    let mut out = Fields::empty(a.width, a.height, a.half_width, a.half_height);
    for w in windows(a, reach(&look)) {
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
        let mut tables = Tables::new(a);
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
