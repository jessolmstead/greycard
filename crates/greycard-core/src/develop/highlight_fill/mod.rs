//! The learned highlight fill, hl-1 and its successors (notes §719): a
//! CPU reference of `scripts/highlight-fill-oracle/oracle.py`, stage by
//! stage as its `SPEC.md` pins them, tested against the oracle's
//! fixtures.
//!
//! [`fill_fields`] runs the whole fill on a picture up to its two
//! half-size fields, which [`Fields::apply`] applies to it or to
//! another picture of its size; [`fill`] does both. [`camera_fields`]
//! and [`CameraFill::apply`] run it on the develop's camera picture,
//! before the DNG warp, reading the input through the matrix
//! ([`camera`]).

pub mod analysis;
pub mod apply;
pub mod blur;
pub mod camera;
pub mod constants;
pub mod half;
pub mod harmonic;
pub mod net;
pub mod oklab;
pub mod reduced;
pub mod regions;
pub mod synthesis;
pub mod thumbnail;

pub use apply::Fields;
pub use camera::{
    BinnedCodes, Camera, CameraFill, Changes, FrameCodes, Through, camera_fields,
    camera_fields_until,
};
pub use constants::*;
pub use reduced::{down, down_codes, look_scale};

use half::{PixelCodes, PixelSource, Plain};
use net::{FillLook, FillWeights, RegionRow};

/// The fill's fields for `x` (`width` × `height`, RGB interleaved,
/// linear Rec.2020 over the ceiling and headroom; SPEC.md "Inputs"),
/// with its codes, its clips, the version's weights and look, and the
/// look scale `f` (the frame's long edge over the input's; 1 at full
/// size). The whole frame: the box (0, 0), the frame the input's size,
/// the thumbnail the input's own (SPEC stages 1 to 10).
///
/// None when no half-size cell is fa (nothing clipped enough to make a
/// region, or an empty picture): the oracle's fields are then zero
/// everywhere and it returns x, as applying nothing does. The analysis
/// is dropped before this returns; only the fields outlive it.
///
/// # Panics
/// If `x` is not `width · height · 3` long, the codes are not
/// `width` × `height`, or `f` is not a positive number.
#[allow(clippy::too_many_arguments)]
pub fn fill_fields(
    x: &[f32],
    width: usize,
    height: usize,
    codes: &impl PixelCodes,
    clips: [f32; 3],
    weights: &FillWeights,
    look: &FillLook,
    f: f64,
) -> Option<Fields> {
    fill_fields_of(
        &Plain::new(x, width, height),
        codes,
        clips,
        weights,
        look,
        f,
    )
}

/// [`fill_fields`] of any source: the same fields from any two sources
/// that give each pixel the same bits.
///
/// # Panics
/// If the codes are not the source's size, or `f` is not a positive
/// number.
pub fn fill_fields_of(
    x: &impl PixelSource,
    codes: &impl PixelCodes,
    clips: [f32; 3],
    weights: &FillWeights,
    look: &FillLook,
    f: f64,
) -> Option<Fields> {
    match fill_fields_until(x, codes, clips, weights, look, f, &|| false) {
        Ok(fields) => fields,
        Err(GaveWay) => unreachable!("never asked to give way"),
    }
}

/// The fill asked to stop by its caller, between two of its parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GaveWay;

/// [`fill_fields_of`], asking `give_way` between its parts: after the
/// region analysis, after the network, and after the synthesis, where
/// what follows is the caller's apply. A yes drops what was made and
/// returns [`GaveWay`]; nothing is half made, since only the fields
/// leave here. The fields, when made, are [`fill_fields_of`]'s to the
/// bit: the asks read nothing of the fill.
///
/// # Panics
/// As [`fill_fields_of`].
pub fn fill_fields_until(
    x: &impl PixelSource,
    codes: &impl PixelCodes,
    clips: [f32; 3],
    weights: &FillWeights,
    look: &FillLook,
    f: f64,
    give_way: &dyn Fn() -> bool,
) -> Result<Option<Fields>, GaveWay> {
    let (width, height) = x.size();
    assert_eq!(
        codes.size(),
        (width, height),
        "the codes are the picture's size"
    );
    let look_scaled = synthesis::Scaled::new(look, f);
    if width == 0 || height == 0 {
        return Ok(None);
    }
    let bright = look.harmonic_bright as f32;
    let a = match look_scaled.reach {
        None => analysis::analyze_of(x, codes, clips, bright),
        // hl-2: every region gated out is the classical fill's, to the
        // bit; the network is not asked.
        Some(reach) => match analysis::analyze_gated_of(x, codes, clips, bright, reach.gate_hi) {
            Some(a) => a,
            None => return Ok(None),
        },
    };
    if give_way() {
        return Err(GaveWay);
    }
    let Some(outputs) = region_outputs_of(&a, x, codes, weights) else {
        return Ok(None);
    };
    if give_way() {
        return Err(GaveWay);
    }
    let fields = synthesis::fields(&a, &outputs, look_scaled);
    if give_way() {
        return Err(GaveWay);
    }
    Ok(Some(fields))
}

/// [`region_outputs_of`] of a slice, for the tests.
#[cfg(test)]
pub(crate) fn region_outputs(
    a: &analysis::Analysis,
    x: &[f32],
    codes: &impl PixelCodes,
    weights: &FillWeights,
) -> Option<Vec<[f32; net::OUTPUTS]>> {
    region_outputs_of(a, &Plain::new(x, a.width, a.height), codes, weights)
}

/// The network's outputs a row of the analysis's region table (SPEC
/// stages 7 to 9): the input's thumbnail, the encoder, and the MLP at
/// each row's point, the outside row's zeroed. None when the table is
/// empty.
fn region_outputs_of(
    a: &analysis::Analysis,
    x: &impl PixelSource,
    codes: &impl PixelCodes,
    weights: &FillWeights,
) -> Option<Vec<[f32; net::OUTPUTS]>> {
    if a.regions.is_empty() {
        return None;
    }
    let t = thumbnail::thumbnail_of(x, codes, THUMB_LONG);
    let map = weights.encode(&t.data, t.height, t.width);
    let rows: Vec<RegionRow> = a
        .regions
        .iter()
        .zip(&a.features)
        .zip(&a.grid)
        .map(|((r, &features), &point)| RegionRow {
            outside: r.id == 0,
            features,
            point,
        })
        .collect();
    Some(weights.region_outputs(&map, &rows))
}

/// [`fill_fields`], then [`Fields::apply`] to `x` itself. Returns
/// whether any field was made (false: `x` is untouched).
#[allow(clippy::too_many_arguments)]
pub fn fill(
    x: &mut [f32],
    width: usize,
    height: usize,
    codes: &impl PixelCodes,
    clips: [f32; 3],
    weights: &FillWeights,
    look: &FillLook,
    f: f64,
) -> bool {
    match fill_fields(x, width, height, codes, clips, weights, look, f) {
        Some(fields) => {
            fields.apply(x, codes);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod fill_tests;
#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod reach_tests;
#[cfg(test)]
mod tests;
