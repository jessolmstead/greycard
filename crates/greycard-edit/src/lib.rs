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
pub use versions::Version;
pub use vignette::Vignette;
pub use white::{LocalWhite, WhiteShift};

pub use edit::{Adjustment, DetailWants, Edit, Look, MAX_MASKS, overlay_changes};
pub use frames::{
    apply_preset_into, apply_preset_to_line, meta_into, sync_into, sync_learned, turn_into,
};
pub use line::{Exported, HISTORY, Line, Order, Row, Snapshot, Step};
pub use loader::{Loader, NoStores};
pub use migration::migrate;
pub use sections::{
    Demosaic, Detail, DisplayCurve, LEARNED_CHROMA, Learned, Light, Noise, STRUCTURE, Sharpen,
    Tone, WhiteBalance,
};
#[cfg(feature = "testing")]
pub use sidecar::synced_writes;
pub use sidecar::{
    Placement, SIDECAR_FOLDER, Settled, Sidecar, Staged, replace_synced, sync_is_unsupported,
    write_synced,
};

/// The schema version this build writes.
pub const VERSION: u32 = 8;

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
}

pub type Result<T> = std::result::Result<T, Error>;
