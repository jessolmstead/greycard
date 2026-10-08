//! The camera match: the maker's picture style fitted as a look, from
//! the JPEG every raw carries, against the engine's own develop of the
//! same frame (the idea, the trial and the design are in
//! `docs/camera-match.md`).
//!
//! The pieces, in the order a fit runs them:
//!
//! - [`register`]: the camera's JPEG laid over the develop, a scale
//!   about the center and a shift found on luminance.
//! - [`pairs`]: both pictures averaged over blocks, the blocks with
//!   texture or a clipped side dropped, so what is fitted is color and
//!   tone and not the maker's sharpening or noise reduction.
//! - [`exposure`]: the frame's own brightness against the camera's,
//!   in stops, solved per frame so the shared look need not carry it.
//! - [`fit`]: the [`Model`]: a matrix with a ridge toward identity,
//!   per-channel monotone curves kept only where they help, and a
//!   residual table on a lattice, solved as a regularized least
//!   squares.
//! - [`cube`]: the model written as a `.cube` the engine's look slot
//!   reads.
//! - [`radial`]: the lightness left after the look, regressed on the
//!   radius from the frame's center: the camera's vignetting against
//!   the lens profile's, measured and kept out of the look.
//!
//! Everything is display-referred sRGB at the export's size; the
//! reference implementation is `tools/camera-match/fit.py`, whose
//! numbers on one set are the oracle the tests hold this crate to.
//! The engine is not touched: `greycard-core` supplies the transfer
//! curve, Oklab's matrices and the table reader, and a consumer joins
//! this crate to the develop and the decoded JPEG.

pub mod color;
pub mod cube;
pub mod exposure;
pub mod fit;
pub mod pairs;
pub mod radial;
pub mod register;

pub use fit::Model;
pub use pairs::{Pair, Pairs};
pub use register::{Picture, Plane, Registration};

/// A block's side in pixels at the export's size.
pub const BLOCK: usize = 24;

/// Nodes per axis of the fitted table.
pub const LUT_SIZE: usize = 33;

/// Which fit wrote a table, declared in its header (`# fit: 2`), so a
/// table an earlier fit wrote can be told apart and refitted. A table
/// the match wrote before the key was written is fit 1. Bump it with
/// any change to the fit that changes what a table does to a picture,
/// and say in the notes section what the bump was for:
///
/// 1. Everything before §260: the table's residual in encoded sRGB.
/// 2. §260: the residual in Oklab, and lightness held off chroma.
pub const FIT_VERSION: u32 = 2;
