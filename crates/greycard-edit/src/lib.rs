//! The edit: everything a development needs beyond the file, as a typed
//! struct in physical units (stops, kelvin, Duv), with a schema version
//! and a sidecar that keeps its history and the frame's meta.
//!
//! The engine takes no edit schema (`greycard-core` must not depend on
//! one); this crate depends on the engine and turns an [`Edit`] into the
//! engine's settings. What the viewport does with an edit (exposure, the
//! tone curve) is the consumer's, read from the same struct.
//!
//! Rules: every field has a default, so a sidecar written by an older
//! build loads and a newer build's unknown fields are ignored; a field
//! that changes meaning changes the [`VERSION`] and gets a migration in
//! [`migrate`]; a sidecar from a newer version than this build knows is
//! refused rather than half read.

pub mod auto;
pub mod baseline;
pub mod brush;
pub mod bw;
pub mod camera;
pub mod color;
pub mod curve;
pub mod geometry;
pub mod grading;
pub mod grain;
pub mod history;
pub mod kept;
pub mod key;
pub mod lens;
pub mod look;
pub mod mask;
pub mod meta;
pub mod mixer;
pub mod output_sharpen;
pub mod preset;
pub mod retouch;
pub mod sync;
pub mod tint;
pub mod versions;
pub mod vignette;
pub mod white;
// The mark of the XMP beside a frame that a sidecar keeps; the XMP's
// own reading and writing are greycard-edit-io's.
pub mod xmp;

mod edit;
mod frames;
mod layout;
mod line;
mod loader;
mod migration;
mod sections;
mod sidecar;
mod sparse;
#[cfg(test)]
mod testing;

pub use bw::BlackWhite;
pub use camera::Camera;
pub use color::Color;
pub use curve::{CurveLut, Curves, Parametric};
pub use geometry::Geometry;
pub use grading::Grading;
pub use grain::Grain;
pub use history::{describe, describe_step};
pub use lens::{Lens, LensChoice};
pub use look::LookLut;
pub use mask::Mask;
pub use meta::Meta;
pub use mixer::Mixer;
pub use output_sharpen::OutputSharpen;
pub use preset::{Preset, Section};
pub use tint::Tint;
pub use versions::{LineMut, Version};
pub use vignette::Vignette;
pub use white::{LocalWhite, WhiteShift};

pub use edit::{Adjustment, DetailWants, Edit, Look, MAX_MASKS, overlay_changes};
pub use frames::{
    apply_preset_into, apply_preset_to_line, meta_into, sync_into, sync_learned, turn_into,
};
pub use line::{
    Exported, HISTORY, HeadAt, Line, Measured, Order, Row, Snapshot, Step, TOMBSTONES, Written,
};
pub use loader::{Loader, NoStores};
pub use migration::migrate;
pub use sections::{
    Demosaic, Detail, DisplayCurve, LEARNED_CHROMA, Learned, Light, Noise, STRUCTURE, Sharpen,
    Tone, WhiteBalance,
};
pub use sidecar::{
    Asides, Bytes, Checked, CopyIs, FileStat, Floors, NotSaved, OnDisk, Placement, SIDECAR_FOLDER,
    Saved, Settled, Sidecar, Staged, aside_at, asides_among, asides_of, replace_synced,
    sync_is_unsupported, write_synced,
};
#[cfg(feature = "testing")]
pub use sidecar::{changed_reads, synced_writes};

/// The schema version this build writes.
pub const VERSION: u32 = 10;

/// The rendering this build makes of a state: what a state the user
/// makes is stamped with ([`Edit::process`]). Before 1.0 it only
/// records; nothing renders by it, and no old rendering is kept. A
/// state read keeps its own, a later build's too, and renders as this
/// build renders.
///
/// Not in the pictures' key while one process exists: raising it must
/// re-key them (`key::DEVELOP_RECIPE`, or for a change to the finish
/// alone the engine's `edited_pictures::EDITED_RECIPE`, and
/// `key::PROCESS_KEYED` with it, which a test holds to this).
///
/// 2: the gamut op at the output in the clamp's place (the floor before
/// the per channel curve, the look table read at its cube, the rail, the
/// bound, and the roll at held Oklab hue), 2026-11-22. AgX's rail with
/// no camera profile's curve renders as at 1.
pub const PROCESS: u32 = 2;

/// The process a state the user makes now takes: [`PROCESS`], or in a
/// test the one it sets for its thread.
pub(crate) fn process_now() -> u32 {
    #[cfg(test)]
    if let Some(p) = testing::PROCESS.get() {
        return p;
    }
    PROCESS
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sidecar is schema version {0}, this build reads up to {VERSION}")]
    Newer(u32),
    #[error("sidecar: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sidecar: {0}")]
    Io(#[from] std::io::Error),
    #[error("preset: {0}")]
    Preset(String),
    #[error("xmp: {0}")]
    Xmp(String),
    #[error("pp3: {0}")]
    Pp3(String),
    /// A sidecar with states it could not read is never written: the
    /// write would hide their loss (§724).
    #[error("sidecar: {0} of its states could not be read, so it is not written")]
    Unread(usize),
    /// A sidecar a newer build made, whose write floor is past this
    /// build's schema, is read and never written (§724). The plain line
    /// a command says for a frame it holds: "read-only: made with a
    /// newer greycard", the window's words lower-cased.
    #[error("read-only: made with a newer greycard (it asks for schema {0} to write it)")]
    ReadOnly(u32),
}

pub type Result<T> = std::result::Result<T, Error>;
