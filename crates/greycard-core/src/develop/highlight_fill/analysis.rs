//! The fill's analysis of one input: SPEC.md stages 1 to 6 and the
//! per-region half of stage 9, everything the synthesis (stage 10)
//! reads beside the network's outputs.

use super::constants::*;
use super::half::{self, Cell, PixelCodes, PixelSource, Plain};
use super::harmonic::{self, Harmonic};
use super::net::{FEATURES, frame_where, grid_point};
use super::regions::{self, Labels};

/// One row of the region table (SPEC stage 9). Counts are integers;
/// every mean is an f64 sum in raster order over its count, rounded to
/// f32 once (hl-1: the trial's f32 sums drifted on regions of a few
/// million cells, and its counts stopped at 2²⁴).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Region {
    /// 0 for the outside row.
    pub id: u32,
    /// Cells in the region.
    pub area: u64,
    /// The region's largest d0, px (0 for the outside).
    pub depth0: f32,
    /// Mean ly over the bright rim (ring cells it owns with kh = 0 and
    /// exp2(ly) > 0.25), and their count.
    pub ybr: f32,
    pub cbr: u64,
    /// Mean ly over the region's cells with kh = 1, and their count.
    pub yb1: f32,
    pub cb1: u64,
    /// Mean ly over the region's cells with kh = 2, and their count.
    pub yb2: f32,
    pub cb2: u64,
    /// The region's fc cells.
    pub ccore: u64,
    /// Mean abh over the bright rim.
    pub ar: f32,
    pub br: f32,
    /// Mean abh over the region's band cells (fa, not fc), and their
    /// count.
    pub an: f32,
    pub bn: f32,
    pub cbn: u64,
    /// The share of its fc cells within 4 px of the band (dcore ≤ 4).
    pub thin: f32,
    /// The mean of (x + 0.5)/Wh and (y + 0.5)/Hh over its cells.
    pub centroid: [f32; 2],
}

impl Region {
    /// The network's 20 features (SPEC stage 9), in the oracle's order:
    /// each count read as f32, the counts' total summed as an integer.
    pub fn features(&self, clips: [f32; 3]) -> [f32; FEATURES] {
        let tot = (self.cbr + self.cb1 + self.cb2 + self.ccore).max(1) as f32;
        let top = if self.cb2 > 0 {
            self.yb2
        } else if self.cb1 > 0 {
            self.yb1
        } else {
            self.ybr
        };
        let rise = if self.cbr > 0 && self.cb1 + self.cb2 > 0 {
            top - self.ybr
        } else {
            0.0
        };
        [
            (self.area as f32).max(1.0).log2() / 16.0,
            self.depth0.max(1.0).log2() / 8.0,
            self.ybr / 4.0,
            self.yb1 / 4.0,
            self.yb2 / 4.0,
            rise,
            self.cbr as f32 / tot,
            self.cb1 as f32 / tot,
            self.cb2 as f32 / tot,
            self.ccore as f32 / tot,
            self.ar * 10.0,
            self.br * 10.0,
            self.an * 10.0,
            self.bn * 10.0,
            (self.an * self.an + self.bn * self.bn).sqrt() * 10.0,
            (self.ar * self.ar + self.br * self.br).sqrt() * 10.0,
            self.thin,
            clips[0],
            clips[1],
            clips[2],
        ]
    }
}

/// What the synthesis and the apply (SPEC stages 10 and 11) read, at
/// half size (`half_width` Wh = ⌈W/2⌉ by `half_height` Hh = ⌈H/2⌉,
/// every plane row-major, a value a cell) unless named otherwise:
/// `cells`, `abh`, `labels.inv`, `d0`, `dc0`, `harmonic`, `regions`;
/// and the network (stage 9) reads `features` and `grid`.
///
/// The ring and yh feed only the region table and are not kept. Planes
/// the oracle keeps that follow from these are methods: fa, fc, band,
/// and kh ([`Cell`]), dcore, the label map, and the harmonic at half
/// size.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    /// The input's size.
    pub width: usize,
    pub height: usize,
    pub half_width: usize,
    pub half_height: usize,
    pub cells: Vec<Cell>,
    /// Unit-luminance Oklab a and b, clamped to ±0.5.
    pub abh: Vec<[f32; 2]>,
    /// The regions: `labels.inv` is each cell's row in [`Self::regions`].
    pub labels: Labels,
    /// px to the nearest cell not fa, quantized to 4 and capped at 512.
    pub d0: Vec<u16>,
    /// px to the nearest cell not fc, quantized to 4 and capped at 512.
    pub dc0: Vec<u16>,
    /// The continuation at a quarter of the half grid.
    pub harmonic: Harmonic,
    /// The region table, a row each of `labels.ids` (row 0 the outside
    /// when there is one). Empty when no cell is fa: the fill then
    /// leaves the input as it is.
    pub regions: Vec<Region>,
    /// Each row's features.
    pub features: Vec<[f32; FEATURES]>,
    /// SPEC's `where`: [box_x/fw, box_y/fh, 2·Wh/fw, 2·Hh/fh], the box
    /// (0, 0) and the frame the input's own size (the padded size, so
    /// (W + 1)/W for an odd W).
    pub extent: [f32; 4],
    /// Each row's point in the encoder's map, grid_sample's
    /// coordinates in [-1, 1]: (extent₀ + extent₂·cx)·2 − 1 and
    /// likewise for y.
    pub grid: Vec<[f32; 2]>,
}

impl Analysis {
    /// dcore at a cell: the distance to a cell not fc capped at 8 cells,
    /// 16 px; min(dc0, 16).
    #[inline]
    pub fn dcore(&self, i: usize) -> u16 {
        self.dc0[i].min(2 * CORE_REACH)
    }

    /// The region id at a cell, 0 outside: the oracle's `regions`.
    #[inline]
    pub fn region_id(&self, i: usize) -> u32 {
        self.labels.id(i)
    }

    /// The harmonic at a cell, resized to the half grid.
    #[inline]
    pub fn harm(&self, i: usize) -> [f32; 2] {
        let w = self.half_width;
        self.harmonic.at(i % w, i / w, w, self.half_height)
    }
}

/// The analysis of `x` (`width` × `height`, linear Rec.2020 over the
/// ceiling and headroom, RGB interleaved) with its codes and clips,
/// whole frame (SPEC stages 1 to 6 and 9's features). Non-finite
/// channels read as 0 (hl-1). Of the look, only `harmonic_bright` (the
/// version's [`FillLook::harmonic_bright`], rounded to f32) enters
/// here; the look scale does not.
///
/// [`FillLook::harmonic_bright`]: super::net::FillLook::harmonic_bright
pub fn analyze(
    x: &[f32],
    width: usize,
    height: usize,
    codes: &impl PixelCodes,
    clips: [f32; 3],
    harmonic_bright: f32,
) -> Analysis {
    analyze_traced(x, width, height, codes, clips, harmonic_bright).0
}

/// [`analyze`] of any source.
pub fn analyze_of(
    x: &impl PixelSource,
    codes: &impl PixelCodes,
    clips: [f32; 3],
    harmonic_bright: f32,
) -> Analysis {
    analyze_traced_of(x, codes, clips, harmonic_bright, None)
        .expect("no gate")
        .0
}

/// [`analyze_of`], or None when no region is shallower than
/// `gate_hi` px (its depth0, the network's depth feature): hl-2 gives
/// every such region to the classical fill, so the rest of the analysis
/// (the ring, the harmonic, the table) and the network are not run.
pub fn analyze_gated_of(
    x: &impl PixelSource,
    codes: &impl PixelCodes,
    clips: [f32; 3],
    harmonic_bright: f32,
    gate_hi: f32,
) -> Option<Analysis> {
    analyze_traced_of(x, codes, clips, harmonic_bright, Some(gate_hi)).map(|t| t.0)
}

/// [`analyze`], with the planes it drops: each cell's ring owner as a
/// row (0 off every ring), and yh.
pub(crate) fn analyze_traced(
    x: &[f32],
    width: usize,
    height: usize,
    codes: &impl PixelCodes,
    clips: [f32; 3],
    harmonic_bright: f32,
) -> (Analysis, Vec<u32>, Vec<f32>) {
    analyze_traced_of(
        &Plain::new(x, width, height),
        codes,
        clips,
        harmonic_bright,
        None,
    )
    .expect("no gate")
}

fn analyze_traced_of(
    x: &impl PixelSource,
    codes: &impl PixelCodes,
    clips: [f32; 3],
    harmonic_bright: f32,
    gate_hi: Option<f32>,
) -> Option<(Analysis, Vec<u32>, Vec<f32>)> {
    let (width, height) = x.size();
    let (wh, hh) = half::half_size(width, height);
    let (cells, abh, yh) = half::planes_of(x, codes);
    let fa: Vec<bool> = cells.iter().map(|c| c.fa()).collect();
    let labels = regions::labels(&fa, wh, hh);
    let (d0, dc0) = rayon::join(
        || regions::distance(&fa, wh, hh, REACH_HALF),
        || {
            let fc: Vec<bool> = cells.iter().map(|c| c.fc()).collect();
            regions::distance(&fc, wh, hh, REACH_HALF)
        },
    );
    if let Some(hi) = gate_hi {
        // Each region's depth0 is its largest d0 (the table's), whole
        // regions only: the outside row has no fa cell.
        let mut depth0 = vec![0u16; labels.ids.len()];
        for (i, _) in fa.iter().enumerate().filter(|(_, f)| **f) {
            let r = labels.inv[i] as usize;
            depth0[r] = depth0[r].max(d0[i]);
        }
        let shallow = fa
            .iter()
            .enumerate()
            .any(|(i, &f)| f && (depth0[labels.inv[i] as usize] as f32) < hi);
        if !shallow {
            return None;
        }
    }
    let ring = regions::ring(&fa, &labels, wh, hh, RING);
    let fixed = harmonic::fixed(&cells, &yh, harmonic_bright);
    drop(fa);
    let mut harmonic = harmonic::start(&abh, &fixed, wh, hh);
    drop(fixed);
    harmonic::iterate(&mut harmonic, HARM_ITERS);
    let mut a = Analysis {
        width,
        height,
        half_width: wh,
        half_height: hh,
        cells,
        abh,
        labels,
        d0,
        dc0,
        harmonic,
        regions: Vec::new(),
        features: Vec::new(),
        extent: frame_where([0, 0], [width as u32, height as u32], [wh, hh]),
        grid: Vec::new(),
    };
    a.regions = region_table(&a, &ring, &yh);
    a.features = a.regions.iter().map(|r| r.features(clips)).collect();
    a.grid = a
        .regions
        .iter()
        .map(|r| grid_point(a.extent, r.centroid))
        .collect();
    Some((a, ring, yh))
}

#[derive(Default, Clone, Copy)]
struct Sums {
    area: u64,
    depth0: f32,
    ybr: f64,
    cbr: u64,
    yb1: f64,
    cb1: u64,
    yb2: f64,
    cb2: u64,
    ccore: u64,
    ar: f64,
    br: f64,
    an: f64,
    bn: f64,
    cbn: u64,
    thin: u64,
    cx: f64,
    cy: f64,
}

/// The region table (SPEC stage 9): one pass in raster order, integer
/// counts and f64 sums, each mean rounded to f32 once. Single threaded
/// and in raster order, so the sums, and the table, are the same on any
/// thread count.
pub(crate) fn region_table(a: &Analysis, ring: &[u32], yh: &[f32]) -> Vec<Region> {
    if !a.cells.iter().any(|c| c.fa()) {
        return Vec::new();
    }
    let (wh, hh) = (a.half_width, a.half_height);
    let n = a.labels.ids.len();
    let mut s = vec![Sums::default(); n];
    let xx: Vec<f32> = (0..wh).map(|x| (x as f32 + 0.5) / wh as f32).collect();
    for y in 0..hh {
        let yy = (y as f32 + 0.5) / hh as f32;
        for (x, &xx) in xx.iter().enumerate() {
            let i = y * wh + x;
            let c = a.cells[i];
            if c.fa() {
                let r = &mut s[a.labels.inv[i] as usize];
                r.area += 1;
                r.depth0 = r.depth0.max(a.d0[i] as f32);
                let ly = half::ly(yh[i]) as f64;
                match c.kh() {
                    1 => {
                        r.yb1 += ly;
                        r.cb1 += 1;
                    }
                    2 => {
                        r.yb2 += ly;
                        r.cb2 += 1;
                    }
                    _ => {}
                }
                if c.fc() {
                    r.ccore += 1;
                    r.thin += (a.dcore(i) as f32 <= THIN) as u64;
                } else {
                    r.an += a.abh[i][0] as f64;
                    r.bn += a.abh[i][1] as f64;
                    r.cbn += 1;
                }
                r.cx += xx as f64;
                r.cy += yy as f64;
            } else if ring[i] > 0 && c.kh() == 0 {
                let ly = half::ly(yh[i]);
                if ly.exp2() > 0.25 {
                    let r = &mut s[ring[i] as usize];
                    r.ybr += ly as f64;
                    r.cbr += 1;
                    r.ar += a.abh[i][0] as f64;
                    r.br += a.abh[i][1] as f64;
                }
            }
        }
    }
    let mean = |v: f64, c: u64| (v / c.max(1) as f64) as f32;
    s.iter()
        .zip(&a.labels.ids)
        .map(|(s, &id)| Region {
            id,
            area: s.area,
            depth0: s.depth0,
            ybr: mean(s.ybr, s.cbr),
            cbr: s.cbr,
            yb1: mean(s.yb1, s.cb1),
            cb1: s.cb1,
            yb2: mean(s.yb2, s.cb2),
            cb2: s.cb2,
            ccore: s.ccore,
            ar: mean(s.ar, s.cbr),
            br: mean(s.br, s.cbr),
            an: mean(s.an, s.cbn),
            bn: mean(s.bn, s.cbn),
            cbn: s.cbn,
            thin: mean(s.thin as f64, s.ccore),
            centroid: [mean(s.cx, s.area), mean(s.cy, s.area)],
        })
        .collect()
}
