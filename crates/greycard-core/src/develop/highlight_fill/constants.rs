//! hl-1's constants (SPEC.md, "Constants"; `oracle.py`'s top block).
//! Lengths are in half-size cells or full-size px as named; S = 1
//! throughout, so each is the same number of pixels on every frame. The
//! look constants (whitening, boost, depth tie, gain hand-over, and the
//! harmonic's brightness) belong to a version and come in its
//! [`FillLook`](super::net::FillLook).

/// The log2 floor on luminance, 2⁻¹².
pub const FLOOR: f32 = 1.0 / 4096.0;
/// Unit-luminance Oklab ab is clamped to ± this at half size.
pub const ABMAX: f32 = 0.5;
/// The distance cap, half-size cells (512 full px).
pub const REACH_HALF: u16 = 256;
/// The ring each region owns, half-size cells.
pub const RING: usize = 12;
/// The thin-core distance's cap, half-size cells.
pub const CORE_REACH: u16 = 8;
/// A core cell is thin within this many full px of the band.
pub const THIN: f32 = 4.0;
/// The rim's offset, full px, at most half the region's depth.
pub const OFF: f32 = 4.0;
/// Jacobi iterations at a quarter of the half size.
pub const HARM_ITERS: usize = 300;
/// The depth fields' normalized blur, half-size cells.
pub const SIGMA_DEPTH: f64 = 3.0;
/// The band-to-core hand-over (color, and the core's own gain).
pub const SIGMA_MIX: f64 = 2.0;
/// The thumbnail's long edge.
pub const THUMB_LONG: usize = 384;
