//! The fill in camera space, before the DNG warp (notes §719 §1a): what
//! the engine's stage calls on the demosaiced (or binned) camera picture
//! and its prepared frame.
//!
//! The codes are in the unwarped mosaic's frame, so the fill runs there.
//! Its input is `x = M·rgb / (ceiling·headroom)` with `M` the matrix
//! [`finish`](crate::develop::finish) applies, read pixel by pixel as
//! the analysis asks ([`Through`]), so no second full-size picture is
//! made. The fields then go back through `M⁻¹`, and only at the pixels
//! they change; every other pixel keeps its bits. With no warp between
//! the fill and `finish`, this is the fill after the matrix to float
//! rounding: `finish`'s `M` meets the fill's `M⁻¹`.

use rayon::prelude::*;

use super::apply::{Fields, pixel};
use super::half::{PixelCodes, PixelSource};
use super::net::{FillLook, FillWeights};
use super::reduced::look_scale;
use crate::develop::Prepared;
use crate::develop::clipcodes::ClipCodes;

/// What turns the develop's camera picture into the fill's input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Camera to working space by columns (`matrix[c][r]`), as
    /// [`apply_matrix`](crate::develop::apply_matrix) takes it: the
    /// prepared frame's `white_balance.matrix_f32()`.
    pub matrix: [[f32; 3]; 3],
    /// [`Prepared::ceiling`].
    pub ceiling: f32,
    /// [`Prepared::headroom`].
    pub headroom: f32,
}

impl Camera {
    /// The prepared frame's: the matrix `finish` applies, its ceiling,
    /// and its headroom.
    pub fn of(prepared: &Prepared) -> Self {
        Self {
            matrix: prepared.white_balance.matrix_f32(),
            ceiling: prepared.ceiling,
            headroom: prepared.headroom,
        }
    }

    /// The fill's unit in the picture's: ceiling × headroom, in f32.
    pub fn scale(&self) -> f32 {
        self.ceiling * self.headroom
    }

    /// One camera pixel in the working space, as `apply_matrix` makes
    /// it, to the bit: each channel `m[0]·r + m[1]·g + m[2]·b`, left to
    /// right in f32.
    #[inline]
    pub fn working(&self, [r, g, b]: [f32; 3]) -> [f32; 3] {
        let m = &self.matrix;
        std::array::from_fn(|c| m[0][c] * r + m[1][c] * g + m[2][c] * b)
    }

    /// `M⁻¹` by columns: inverted in f64 from the f32 matrix, then
    /// rounded. None for a matrix with no finite inverse.
    fn inverse(&self) -> Option<[[f32; 3]; 3]> {
        // m[r][c], row-major, from the columns.
        let m: [[f64; 3]; 3] =
            std::array::from_fn(|r| std::array::from_fn(|c| self.matrix[c][r] as f64));
        let cof = |r: usize, c: usize| {
            let (r0, r1) = ((r + 1) % 3, (r + 2) % 3);
            let (c0, c1) = ((c + 1) % 3, (c + 2) % 3);
            m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
        };
        let det = m[0][0] * cof(0, 0) + m[0][1] * cof(0, 1) + m[0][2] * cof(0, 2);
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        // inv[r][c] = cof(c, r) / det; back to columns: out[c][r].
        let out: [[f32; 3]; 3] =
            std::array::from_fn(|c| std::array::from_fn(|r| (cof(c, r) / det) as f32));
        out.iter().flatten().all(|v| v.is_finite()).then_some(out)
    }
}

/// A camera picture read as the fill's input: each pixel through
/// [`Camera::working`] and divided by [`Camera::scale`], the bits
/// `finish`'s matrix and then a division would give, made when read.
#[derive(Debug, Clone, Copy)]
pub struct Through<'a> {
    rgb: &'a [f32],
    width: usize,
    height: usize,
    camera: Camera,
    scale: f32,
}

impl<'a> Through<'a> {
    /// # Panics
    /// If `rgb` is not `width · height · 3` long.
    pub fn new(rgb: &'a [f32], width: usize, height: usize, camera: &Camera) -> Self {
        assert_eq!(rgb.len(), width * height * 3, "rgb is RGB interleaved");
        Self {
            rgb,
            width,
            height,
            camera: *camera,
            scale: camera.scale(),
        }
    }
}

impl PixelSource for Through<'_> {
    fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    #[inline]
    fn rgb(&self, x: usize, y: usize) -> [f32; 3] {
        let p = 3 * (y * self.width + x);
        // One division by the f32 product ceiling·headroom; the
        // oracle's SPEC divides by each in turn, so a frame with
        // headroom matches the oracle to its tolerance, not to the bit.
        let s = self.scale;
        self.camera
            .working([self.rgb[p], self.rgb[p + 1], self.rgb[p + 2]])
            .map(|v| v / s)
    }
}

/// The codes of a picture binned by the codes' own cell: one picture
/// pixel is one whole cell (a Bayer bin of 2 is the 2×2 cell, laid
/// from the crop's origin as the bin is), so its code is the cell's,
/// exactly what [`down_codes`](super::down_codes) makes of a code a
/// pixel. Read in the binned picture's coordinates, and sized as it is,
/// so the fill refuses it beside a picture of another size.
#[derive(Debug, Clone, Copy)]
pub struct BinnedCodes<'a> {
    codes: &'a ClipCodes,
    width: usize,
    height: usize,
}

impl<'a> BinnedCodes<'a> {
    /// The codes of the picture binned by `bin` from the mosaic `codes`
    /// cover: ⌊W/bin⌋ × ⌊H/bin⌋, the ragged edge dropped as the bin
    /// drops it. None unless `bin` is the codes' cell (2 for Bayer, 3
    /// for X-Trans), where a pixel would be part of a cell or several.
    pub fn new(codes: &'a ClipCodes, bin: usize) -> Option<Self> {
        (bin == codes.cell).then(|| Self {
            codes,
            width: codes.width / bin,
            height: codes.height / bin,
        })
    }
}

impl PixelCodes for BinnedCodes<'_> {
    #[inline]
    fn code(&self, x: usize, y: usize) -> u8 {
        debug_assert!(
            x < self.width && y < self.height,
            "({x}, {y}) is outside the {}x{} binned picture",
            self.width,
            self.height
        );
        self.codes.codes[y * self.codes.cells_wide + x]
    }

    fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }
}

/// The codes for a picture of the develop: the full-size mosaic's, read
/// through each pixel's cell, or a binned picture's.
#[derive(Debug, Clone, Copy)]
pub enum FrameCodes<'a> {
    Full(&'a ClipCodes),
    Binned(BinnedCodes<'a>),
}

impl<'a> FrameCodes<'a> {
    /// The codes for the picture a develop made from the mosaic `codes`
    /// cover, binned by `bin` (1: not binned). None for a bin that is
    /// neither 1 nor the codes' cell, or an empty picture.
    pub fn new(codes: &'a ClipCodes, bin: usize) -> Option<Self> {
        let out = match bin {
            1 => Self::Full(codes),
            _ => Self::Binned(BinnedCodes::new(codes, bin)?),
        };
        let (w, h) = out.size();
        (w > 0 && h > 0).then_some(out)
    }

    /// The fill's look scale f for this picture: the mosaic's long edge
    /// over the picture's ([`look_scale`]); 1 at full size.
    pub fn look_scale(&self) -> f64 {
        let full = match self {
            Self::Full(c) => c,
            Self::Binned(b) => b.codes,
        };
        look_scale((full.width, full.height), self.size())
    }

    /// Each channel's clip level over the ceiling, the mosaic's.
    pub fn clips(&self) -> [f32; 3] {
        match self {
            Self::Full(c) => c.clips,
            Self::Binned(b) => b.codes.clips,
        }
    }
}

impl PixelCodes for FrameCodes<'_> {
    #[inline]
    fn code(&self, x: usize, y: usize) -> u8 {
        match self {
            Self::Full(c) => c.code(x, y),
            Self::Binned(b) => b.code(x, y),
        }
    }

    fn size(&self) -> (usize, usize) {
        match self {
            Self::Full(c) => c.size(),
            Self::Binned(b) => b.size(),
        }
    }
}

/// One fill's fields in camera space, for pictures of one size and
/// orientation: computed once ([`camera_fields`]), applied to any
/// number of them ([`CameraFill::apply`]), such as both halves of the
/// learned denoiser's pair.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraFill {
    fields: Fields,
    camera: Camera,
    /// `M⁻¹` by columns.
    inverse: [[f32; 3]; 3],
}

/// The pixels an apply changed, exactly those whose bits it changed:
/// a bit a pixel, each row its own whole `u64` words. Its size is the
/// picture's (⌈W/64⌉·8 bytes a row, 12.5 MB at 100 MP), however many
/// pixels changed and however noise scatters them. What a consumer that
/// measures the picture before the apply needs to measure it again
/// after, where it changed only (the frame's range for Highlights and
/// Whites: [`Changes::any_in`] for each of its cells).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Changes {
    width: usize,
    height: usize,
    /// Words a row: ⌈width/64⌉.
    stride: usize,
    bits: Vec<u64>,
}

impl Changes {
    /// None changed yet, for a `width` × `height` picture.
    fn new(width: usize, height: usize) -> Self {
        let stride = width.div_ceil(64);
        Self {
            width,
            height,
            stride,
            bits: vec![0; stride * height],
        }
    }

    /// Whether pixel (`x`, `y`) changed.
    ///
    /// # Panics
    /// Outside the picture.
    pub fn contains(&self, x: usize, y: usize) -> bool {
        assert!(x < self.width && y < self.height, "({x}, {y}) is outside");
        self.bits[y * self.stride + x / 64] >> (x % 64) & 1 == 1
    }

    /// Whether any pixel in columns `xs` of rows `ys` changed: a cell
    /// of the consumer's own grid, clamped to the picture.
    pub fn any_in(&self, xs: std::ops::Range<usize>, ys: std::ops::Range<usize>) -> bool {
        let (x0, x1) = (xs.start, xs.end.min(self.width));
        if x0 >= x1 {
            return false;
        }
        (ys.start..ys.end.min(self.height)).any(|y| {
            let row = &self.bits[y * self.stride..][..self.stride];
            (x0 / 64..=(x1 - 1) / 64).any(|k| {
                let lo = if k == x0 / 64 { x0 % 64 } else { 0 };
                let hi = if k == (x1 - 1) / 64 {
                    (x1 - 1) % 64
                } else {
                    63
                };
                let mask = (u64::MAX >> (63 - hi)) & (u64::MAX << lo);
                row[k] & mask != 0
            })
        })
    }

    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|&w| w == 0)
    }

    /// How many pixels changed.
    pub fn count(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// The bytes it holds.
    pub fn bytes(&self) -> usize {
        std::mem::size_of_val(self.bits.as_slice())
    }

    /// Each changed pixel, (x, y), by row and then column.
    pub fn pixels(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let stride = self.stride.max(1);
        self.bits.iter().enumerate().flat_map(move |(i, &word)| {
            let (y, base) = (i / stride, (i % stride) * 64);
            let mut w = word;
            std::iter::from_fn(move || {
                (w != 0).then(|| {
                    let b = w.trailing_zeros() as usize;
                    w &= w - 1;
                    (base + b, y)
                })
            })
        })
    }
}

/// The fill's fields for a camera picture `rgb` (`width` × `height`,
/// RGB interleaved, in the prepared frame's units): its input is
/// `x = M·rgb / (ceiling·headroom)` read as [`Through`] reads it, the
/// bits `finish`'s matrix and a division would make, so the fields are
/// [`fill_fields`](super::fill_fields)' on that `x` to the bit. With
/// `codes` for the picture ([`FrameCodes`]), their `clips`, the
/// version's weights and look, and the look scale `f`
/// ([`FrameCodes::look_scale`]).
///
/// None, never a panic, when there are no codes, the picture is empty,
/// nothing is clipped enough to make a region, or the matrix has no
/// inverse. Codes of X-Trans's 3×3 cells run as any others do (the
/// oracle's `xtrans` scene): each pixel reads its cell's code and the
/// half grid pools 2×2 blocks of them. Whether X-Trans is offered is
/// the engine's decision.
///
/// # Panics
/// If `rgb` is not `width · height · 3` long, the codes are not
/// `width` × `height`, or `f` is not a positive number.
#[allow(clippy::too_many_arguments)]
pub fn camera_fields<C: PixelCodes>(
    rgb: &[f32],
    width: usize,
    height: usize,
    camera: &Camera,
    codes: Option<&C>,
    clips: [f32; 3],
    weights: &FillWeights,
    look: &FillLook,
    f: f64,
) -> Option<CameraFill> {
    match camera_fields_until(
        rgb,
        width,
        height,
        camera,
        codes,
        clips,
        weights,
        look,
        f,
        &|| false,
    ) {
        Ok(fill) => fill,
        Err(super::GaveWay) => unreachable!("never asked to give way"),
    }
}

/// [`camera_fields`], asking `give_way` between the fill's parts
/// ([`fill_fields_until`](super::fill_fields_until)): the same fields
/// to the bit when it runs through, [`GaveWay`](super::GaveWay) and
/// nothing made when it is told to stop.
///
/// # Panics
/// As [`camera_fields`].
#[allow(clippy::too_many_arguments)]
pub fn camera_fields_until<C: PixelCodes>(
    rgb: &[f32],
    width: usize,
    height: usize,
    camera: &Camera,
    codes: Option<&C>,
    clips: [f32; 3],
    weights: &FillWeights,
    look: &FillLook,
    f: f64,
    give_way: &dyn Fn() -> bool,
) -> Result<Option<CameraFill>, super::GaveWay> {
    if width == 0 || height == 0 {
        return Ok(None);
    }
    let Some(codes) = codes else {
        return Ok(None);
    };
    let source = Through::new(rgb, width, height, camera);
    let scale = camera.scale();
    if !(scale.is_finite() && scale > 0.0) {
        return Ok(None);
    }
    let Some(inverse) = camera.inverse() else {
        return Ok(None);
    };
    let fields = super::fill_fields_until(&source, codes, clips, weights, look, f, give_way)?;
    Ok(fields.map(|fields| CameraFill {
        fields,
        camera: *camera,
        inverse,
    }))
}

impl CameraFill {
    /// The fields, in the picture's frame.
    pub fn fields(&self) -> &Fields {
        &self.fields
    }

    /// The same fields applied through `camera`: another white balance
    /// of the same frame, where the fields made under the last one
    /// stand in until the fill is made again (notes §719 §3). None for
    /// a camera whose matrix has no finite inverse or whose scale is
    /// not a positive number.
    ///
    /// The fields are a gain in stops and an Oklab ab offset at half
    /// size, both read where they are applied as offsets from the
    /// pixel's own luminance and chroma, so they mean the same lift
    /// under any white balance. What a fill made again would change,
    /// and this keeps:
    /// - the network's answers, which read the clipped regions' rim
    ///   and their context through the old white balance: the rim's
    ///   color, the harmonic continuation's, and the clip levels
    ///   (`clips`, each gain over the largest) it was given;
    /// - the color hand-over's target, fitted to the old rim, so the
    ///   ab offset is added to a chroma that moved with the white;
    /// - the depth and the reach, which read only the codes and so do
    ///   not change (the codes count photosites against their own
    ///   white, not against a gain).
    ///
    /// The pixels it changes are those a fill made again would read:
    /// the same codes, the same support.
    pub fn through(&self, camera: &Camera) -> Option<CameraFill> {
        let scale = camera.scale();
        if !(scale.is_finite() && scale > 0.0) {
            return None;
        }
        Some(CameraFill {
            fields: self.fields.clone(),
            camera: *camera,
            inverse: camera.inverse()?,
        })
    }

    /// The bytes the fill holds: its fields' tiles and their index.
    pub fn bytes(&self) -> usize {
        self.fields.bytes() + std::mem::size_of::<Self>()
    }

    /// Applies the fill to `rgb`, a camera picture of the fill's size
    /// and orientation (the one it was made from, or the other half of
    /// a pair), in place. A pixel the fields reach, by
    /// [`Fields::apply`]'s rule (code & 0x7F > 0, an upsampled field,
    /// and the fill's guards on its own luminance), is read into the
    /// working space as the fill's input is, filled there, and written
    /// back as `M⁻¹·(x'·ceiling·headroom)`, `M⁻¹` inverted in f64 and
    /// rounded. Only a pixel the fill changes in the working space, and
    /// whose camera bits that then changes, is written; every
    /// other pixel keeps its bits, so no pixel takes the round trip
    /// through `M⁻¹` without the fill. Rows in parallel, each pixel
    /// alone, so the same on any thread count.
    ///
    /// Returns the pixels it changed.
    ///
    /// # Panics
    /// If `rgb` is not the fill's `width · height · 3` long or the codes
    /// are not its size.
    pub fn apply(&self, rgb: &mut [f32], codes: &impl PixelCodes) -> Changes {
        let (w, h) = (self.fields.width(), self.fields.height());
        assert_eq!(
            rgb.len(),
            w * h * 3,
            "rgb is the fill's size, RGB interleaved"
        );
        assert_eq!(codes.size(), (w, h), "the codes are the fill's size");
        let mut changes = Changes::new(w, h);
        if w == 0 {
            return changes;
        }
        let scale = self.camera.scale();
        let inv = &self.inverse;
        let stride = changes.stride;
        rgb.par_chunks_mut(w * 3)
            .zip(changes.bits.par_chunks_mut(stride))
            .enumerate()
            .for_each(|(py, (row, bits))| {
                for (px, out) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    if codes.code(px, py) & 0x7F == 0 {
                        continue;
                    }
                    let Some((gain, dab)) = self.fields.upsampled(px, py) else {
                        continue;
                    };
                    let x = self.camera.working(*out).map(|v| v / scale);
                    // A fill that leaves x as it was writes nothing: the
                    // round trip alone would move the bits.
                    let Some(filled) =
                        pixel(x, gain, dab).filter(|f| f.map(f32::to_bits) != x.map(f32::to_bits))
                    else {
                        continue;
                    };
                    let [r, g, b] = filled.map(|v| v * scale);
                    let back: [f32; 3] =
                        std::array::from_fn(|c| inv[0][c] * r + inv[1][c] * g + inv[2][c] * b);
                    if back.map(f32::to_bits) == out.map(f32::to_bits) {
                        continue;
                    }
                    *out = back;
                    bits[px / 64] |= 1 << (px % 64);
                }
            });
        changes
    }
}

#[cfg(test)]
mod tests;
