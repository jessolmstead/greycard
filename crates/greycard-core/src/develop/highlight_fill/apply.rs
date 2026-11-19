//! The fields' upsample and their apply to a full-size picture (SPEC.md
//! stage 11).

use rayon::prelude::*;

use super::half::PixelCodes;
use super::harmonic::source;
use super::oklab;

/// The fields' tile, in half-size cells a side (32 px).
pub const TILE: usize = 16;

const NONE: u32 = u32::MAX;

/// The two half-size fields (SPEC stage 10's gain_h and dab_h) of one
/// fill, for a picture of `width` × `height`: computed once, applied to
/// any number of pictures of that size and orientation
/// ([`Fields::apply`]), such as both halves of the learned denoiser's
/// pair.
///
/// Both are zero off the fa cells, so they are kept by tile: a tile of
/// [`TILE`]² cells holds (gain, a, b) a cell, 12 bytes, and only the
/// tiles where some value is nonzero are stored. A cell of a tile not
/// stored reads 0. A frame whose clipped cells spread over every tile
/// costs 12 bytes a half-size cell, 3 bytes a pixel.
#[derive(Debug, Clone)]
pub struct Fields {
    width: usize,
    height: usize,
    half_width: usize,
    half_height: usize,
    tiles_wide: usize,
    /// Per tile, row-major: its slot in `data`, or [`NONE`].
    index: Vec<u32>,
    /// [`TILE`]² cells a slot, row-major within the tile.
    data: Vec<[f32; 3]>,
}

/// Fields are equal when they are for one picture size and every cell
/// reads the same (−0 as 0), however their tiles are laid out.
impl PartialEq for Fields {
    fn eq(&self, o: &Self) -> bool {
        (self.width, self.height) == (o.width, o.height)
            && (0..self.half_height)
                .all(|y| (0..self.half_width).all(|x| self.value(x, y) == o.value(x, y)))
    }
}

impl Fields {
    /// No field anywhere, for a `width` × `height` picture on its
    /// (`half_width`, `half_height`) half grid.
    pub(crate) fn empty(
        width: usize,
        height: usize,
        half_width: usize,
        half_height: usize,
    ) -> Self {
        let tiles_wide = half_width.div_ceil(TILE);
        Self {
            width,
            height,
            half_width,
            half_height,
            tiles_wide,
            index: vec![NONE; tiles_wide * half_height.div_ceil(TILE)],
            data: Vec::new(),
        }
    }

    /// Stores tile (`tx`, `ty`), [`TILE`]² cells row-major; where the
    /// tile is stored already, its nonzero cells overwrite (two windows
    /// sharing a tile write disjoint cells).
    pub(crate) fn merge(&mut self, tx: usize, ty: usize, tile: Vec<[f32; 3]>) {
        assert_eq!(tile.len(), TILE * TILE);
        let at = ty * self.tiles_wide + tx;
        match self.index[at] {
            NONE => {
                self.index[at] = (self.data.len() / (TILE * TILE)) as u32;
                self.data.extend_from_slice(&tile);
            }
            slot => {
                let kept = &mut self.data[slot as usize * TILE * TILE..][..TILE * TILE];
                for (k, v) in kept.iter_mut().zip(tile) {
                    if v != [0.0; 3] {
                        *k = v;
                    }
                }
            }
        }
    }

    /// The picture's width.
    pub fn width(&self) -> usize {
        self.width
    }

    /// The picture's height.
    pub fn height(&self) -> usize {
        self.height
    }

    /// The tiles stored.
    pub fn tiles(&self) -> usize {
        self.data.len() / (TILE * TILE)
    }

    /// The bytes the fields hold.
    pub fn bytes(&self) -> usize {
        std::mem::size_of_val(self.index.as_slice()) + std::mem::size_of_val(self.data.as_slice())
    }

    /// Half-size cell (`cx`, `cy`): gain_h and dab_h's a and b.
    #[inline]
    pub(crate) fn value(&self, cx: usize, cy: usize) -> [f32; 3] {
        match self.index[(cy / TILE) * self.tiles_wide + cx / TILE] {
            NONE => [0.0; 3],
            slot => self.data[slot as usize * TILE * TILE + (cy % TILE) * TILE + cx % TILE],
        }
    }

    /// [`Fields::value`], for other crates' tests against the oracle.
    #[cfg(feature = "testing")]
    pub fn cell(&self, cx: usize, cy: usize) -> [f32; 3] {
        self.value(cx, cy)
    }

    /// The fields at pixel (`x`, `y`) of the picture, for an apply that
    /// converts the picture pixel by pixel: the gain in stops and the
    /// Oklab ab offset, or None where no field reaches (all three zero,
    /// −0 included), where the apply leaves the pixel as it is.
    ///
    /// torch's bilinear interpolate with scale factor 2, align_corners
    /// false, cropped to the picture (SPEC stage 11): along each axis
    /// src = (dst + 0.5)·½ − 0.5, clamped at 0, i0 = ⌊src⌋,
    /// i1 = min(i0 + 1, n − 1); and the value (v00·w0 + v01·w1)·h0 +
    /// (v10·w0 + v11·w1)·h1, torch's documented CPU order (its bits on
    /// the fixture to 2 ulp). Each pixel from its own four cells by one
    /// expression, so any split into rows or strips gives the one-pass
    /// result to the bit.
    #[inline]
    pub fn upsampled(&self, x: usize, y: usize) -> Option<(f32, [f32; 2])> {
        let rows = source(y, self.half_height, 0.5);
        let [g, a, b] = self.lerp(source(x, self.half_width, 0.5), rows);
        (g != 0.0 || a != 0.0 || b != 0.0).then_some((g, [a, b]))
    }

    #[inline]
    fn lerp(&self, (x0, x1, w0, w1): Pair, (y0, y1, h0, h1): Pair) -> [f32; 3] {
        let (v00, v01) = (self.value(x0, y0), self.value(x1, y0));
        let (v10, v11) = (self.value(x0, y1), self.value(x1, y1));
        std::array::from_fn(|c| (v00[c] * w0 + v01[c] * w1) * h0 + (v10[c] * w0 + v11[c] * w1) * h1)
    }

    /// Applies the fields to `x` in place (SPEC stage 11). `x` and
    /// `codes` must be of the fill's size and orientation, `x` RGB
    /// interleaved, linear Rec.2020 over the ceiling as the fill's input;
    /// a picture warped, rotated, or cropped since the fill would take
    /// another pixel's field. A pixel changes only where its
    /// code & 0x7F > 0, 1e-6 < Y(x) < +inf, and its upsampled gain or
    /// either offset is nonzero (−0 counts as zero); every other pixel
    /// keeps its bits. Rows in parallel, each pixel alone, so the same
    /// on any thread count.
    ///
    /// # Panics
    /// If `x` is not `width · height · 3` long or the codes are not
    /// `width` × `height`.
    pub fn apply(&self, x: &mut [f32], codes: &impl PixelCodes) {
        let (w, h) = (self.width, self.height);
        assert_eq!(x.len(), w * h * 3, "x is the fill's size, RGB interleaved");
        assert_eq!(codes.size(), (w, h), "the codes are the fill's size");
        if self.data.is_empty() || w == 0 {
            return;
        }
        let columns: Vec<Pair> = (0..w).map(|px| source(px, self.half_width, 0.5)).collect();
        x.par_chunks_mut(w * 3).enumerate().for_each(|(py, row)| {
            let rows = source(py, self.half_height, 0.5);
            for (px, (rgb, &col)) in row
                .as_chunks_mut::<3>()
                .0
                .iter_mut()
                .zip(&columns)
                .enumerate()
            {
                if codes.code(px, py) & 0x7F == 0 {
                    continue;
                }
                let [g, da, db] = self.lerp(col, rows);
                if let Some(out) = pixel(*rgb, g, [da, db]) {
                    *rgb = out;
                }
            }
        });
    }
}

/// One axis's source pair and weights.
type Pair = (usize, usize, f32, f32);

/// One clipped pixel filled (SPEC stage 11): None where hl-1 leaves it
/// as it is (Y(x) not in (1e-6, +inf), or no field: gain and both
/// offsets zero). Else y = Y(x), u = Oklab(x / y) with the offset added
/// to a and b, rgb = its inverse, and rgb / max(Y(rgb), 1e-6) · y ·
/// 2^gain, left to right in f32.
#[inline]
pub(crate) fn pixel(x: [f32; 3], gain: f32, dab: [f32; 2]) -> Option<[f32; 3]> {
    let y = oklab::luma(x);
    if !(y > 1e-6 && y < f32::INFINITY) || (gain == 0.0 && dab == [0.0; 2]) {
        return None;
    }
    // y = max(Y, 1e-6) is Y here.
    let mut u = oklab::of(x.map(|v| v / y));
    u[1] += dab[0];
    u[2] += dab[1];
    let rgb = oklab::rgb(u);
    let yn = oklab::luma(rgb).max(1e-6);
    let lift = libm::exp2f(gain);
    Some(rgb.map(|v| v / yn * y * lift))
}
