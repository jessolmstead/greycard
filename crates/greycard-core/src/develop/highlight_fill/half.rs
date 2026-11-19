//! The half-size inputs (SPEC.md stage 1) and the Oklab planes made
//! from them (stage 2).

use rayon::prelude::*;

use super::oklab;
use crate::develop::clipcodes::ClipCodes;

/// The fill's input picture, pixel by pixel: linear Rec.2020 over the
/// ceiling and headroom (SPEC.md "Inputs"). A plain slice ([`Plain`]),
/// or a camera picture read through the matrix as it is read
/// ([`Through`](super::camera::Through)), so the fill of a 100 MP frame
/// needs no second full-size picture. The analysis and the thumbnail
/// read each pixel by this alone, so two sources that give a pixel the
/// same bits give the same fill to the bit.
pub trait PixelSource: Sync {
    /// The picture: (width, height).
    fn size(&self) -> (usize, usize);

    /// Pixel (`x`, `y`)'s three channels.
    fn rgb(&self, x: usize, y: usize) -> [f32; 3];
}

/// A picture held as it is read: RGB interleaved, row-major.
#[derive(Debug, Clone, Copy)]
pub struct Plain<'a> {
    x: &'a [f32],
    width: usize,
    height: usize,
}

impl<'a> Plain<'a> {
    /// # Panics
    /// If `x` is not `width · height · 3` long.
    pub fn new(x: &'a [f32], width: usize, height: usize) -> Self {
        assert_eq!(x.len(), width * height * 3, "x is RGB interleaved");
        Self { x, width, height }
    }
}

impl PixelSource for Plain<'_> {
    fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    #[inline]
    fn rgb(&self, x: usize, y: usize) -> [f32; 3] {
        let p = 3 * (y * self.width + x);
        [self.x[p], self.x[p + 1], self.x[p + 2]]
    }
}

/// The clip code of each pixel of the fill's input: the count of colors
/// clipped in its CFA cell, 0 to 3, bit 7 (untrusted) ignored by the
/// fill.
pub trait PixelCodes: Sync {
    fn code(&self, x: usize, y: usize) -> u8;

    /// The picture the codes are for: (width, height).
    fn size(&self) -> (usize, usize);
}

/// The full-size develop's codes, read through each pixel's cell.
impl PixelCodes for ClipCodes {
    #[inline]
    fn code(&self, x: usize, y: usize) -> u8 {
        self.at(x, y)
    }

    fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }
}

/// A code a pixel, row-major: a reduced develop's pooled codes, or the
/// fixtures'.
#[derive(Debug, Clone, Copy)]
pub struct CodePlane<'a> {
    pub codes: &'a [u8],
    pub width: usize,
}

impl PixelCodes for CodePlane<'_> {
    #[inline]
    fn code(&self, x: usize, y: usize) -> u8 {
        self.codes[y * self.width + x]
    }

    fn size(&self) -> (usize, usize) {
        (
            self.width,
            self.codes.len().checked_div(self.width).unwrap_or(0),
        )
    }
}

/// A half-size cell's masks and largest code, one byte (SPEC stage 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cell(u8);

impl Cell {
    const KH: u8 = 0b11;
    const FA: u8 = 1 << 2;
    const FC: u8 = 1 << 3;

    /// From the block's counts of pixels with k > 0 and with k = 3 (of
    /// four) and its largest k.
    ///
    /// # Panics
    /// When the counts are not a 2×2 block's: `any` over 4, or `core`
    /// over `any`.
    pub fn new(any: u8, core: u8, kh: u8) -> Self {
        assert!(
            any <= 4 && core <= any,
            "a 2x2 block's counts, not {any} and {core}"
        );
        // A share over a half: three or four of the four.
        let fa = any > 2;
        let fc = core > 2 && fa;
        // kh is only ever compared with 0, 1, and 2 (SPEC stage 9), so a
        // port may cap it: codes past 3, which the app never writes,
        // read as 3.
        Self(kh.min(3) | if fa { Self::FA } else { 0 } | if fc { Self::FC } else { 0 })
    }

    /// Any clipped: more than half the block has k > 0.
    #[inline]
    pub fn fa(self) -> bool {
        self.0 & Self::FA != 0
    }

    /// Fully clipped: more than half the block has k = 3 (and fa).
    #[inline]
    pub fn fc(self) -> bool {
        self.0 & Self::FC != 0
    }

    /// The band for color: fa and not fc.
    #[inline]
    pub fn band(self) -> bool {
        self.fa() && !self.fc()
    }

    /// The block's largest k, 3 standing for 3 or more.
    #[inline]
    pub fn kh(self) -> u8 {
        self.0 & Self::KH
    }
}

/// The half grid's size: the input padded to even, halved.
pub fn half_size(width: usize, height: usize) -> (usize, usize) {
    (width.div_ceil(2), height.div_ceil(2))
}

#[inline]
fn finite(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// One half-size cell (`cx`, `cy`): the 2×2 mean of x with each
/// non-finite channel read as 0, the edge replicated where the input is
/// odd, ((a + b) + c) + d over 4 (numpy's order is within an ulp); and
/// the block's counts of k > 0 and k = 3, and its largest k, with
/// k = code & 0x7F.
#[inline]
pub fn half_cell(
    x: &[f32],
    width: usize,
    height: usize,
    codes: &impl PixelCodes,
    cx: usize,
    cy: usize,
) -> ([f32; 3], u8, u8, u8) {
    half_cell_of(&Plain::new(x, width, height), codes, cx, cy)
}

/// [`half_cell`] of any source.
#[inline]
pub fn half_cell_of(
    x: &impl PixelSource,
    codes: &impl PixelCodes,
    cx: usize,
    cy: usize,
) -> ([f32; 3], u8, u8, u8) {
    let (width, height) = x.size();
    let xs = [2 * cx, (2 * cx + 1).min(width - 1)];
    let ys = [2 * cy, (2 * cy + 1).min(height - 1)];
    let mut sum = [0.0f32; 3];
    let (mut any, mut core, mut kh) = (0u8, 0u8, 0u8);
    for (n, (py, px)) in [
        (ys[0], xs[0]),
        (ys[0], xs[1]),
        (ys[1], xs[0]),
        (ys[1], xs[1]),
    ]
    .into_iter()
    .enumerate()
    {
        let rgb = x.rgb(px, py);
        for c in 0..3 {
            let v = finite(rgb[c]);
            sum[c] = if n == 0 { v } else { sum[c] + v };
        }
        let k = codes.code(px, py) & 0x7F;
        any += (k > 0) as u8;
        core += (k == 3) as u8;
        kh = kh.max(k);
    }
    (sum.map(|s| s / 4.0), any, core, kh)
}

/// The half-size picture alone, (Hh, Wh) of RGB: what [`planes`] reads
/// and drops, for the tests.
#[cfg(test)]
pub(crate) fn half_picture(
    x: &[f32],
    width: usize,
    height: usize,
    codes: &impl PixelCodes,
) -> Vec<[f32; 3]> {
    let (wh, hh) = half_size(width, height);
    (0..wh * hh)
        .map(|i| half_cell(x, width, height, codes, i % wh, i / wh).0)
        .collect()
}

/// unit_ab of one half-size pixel: its Oklab a and b at unit luminance,
/// y = max(Y, 1e-6), clamped to ±0.5 (SPEC stage 2); and Y unclamped.
#[inline]
pub fn unit_ab(xh: [f32; 3]) -> ([f32; 2], f32) {
    let yh = oklab::luma(xh);
    let y = yh.max(1e-6);
    let lab = oklab::of(xh.map(|v| v / y));
    (
        [
            lab[1].clamp(-super::ABMAX, super::ABMAX),
            lab[2].clamp(-super::ABMAX, super::ABMAX),
        ],
        yh,
    )
}

/// The half-size planes the analysis keeps: cells, abh, and yh, rows in
/// parallel (each cell alone, so the same on any thread count).
pub fn planes(
    x: &[f32],
    width: usize,
    height: usize,
    codes: &impl PixelCodes,
) -> (Vec<Cell>, Vec<[f32; 2]>, Vec<f32>) {
    planes_of(&Plain::new(x, width, height), codes)
}

/// [`planes`] of any source.
pub fn planes_of(
    x: &impl PixelSource,
    codes: &impl PixelCodes,
) -> (Vec<Cell>, Vec<[f32; 2]>, Vec<f32>) {
    let (width, height) = x.size();
    let (wh, hh) = half_size(width, height);
    let mut cells = vec![Cell::default(); wh * hh];
    let mut abh = vec![[0.0f32; 2]; wh * hh];
    let mut yh = vec![0.0f32; wh * hh];
    if wh == 0 || hh == 0 {
        return (cells, abh, yh);
    }
    cells
        .par_chunks_mut(wh)
        .zip(abh.par_chunks_mut(wh))
        .zip(yh.par_chunks_mut(wh))
        .enumerate()
        .for_each(|(cy, ((cells, abh), yh))| {
            for cx in 0..wh {
                let (xh, any, core, kh) = half_cell_of(x, codes, cx, cy);
                cells[cx] = Cell::new(any, core, kh);
                (abh[cx], yh[cx]) = unit_ab(xh);
            }
        });
    (cells, abh, yh)
}

/// ly = log2(max(yh, 2⁻¹²)) (SPEC stage 2).
#[inline]
pub fn ly(yh: f32) -> f32 {
    yh.max(super::FLOOR).log2()
}
