//! The editor's modules that need no window: no Slint type, no wgpu,
//! and no `State`, so their tests run without one and a later engine
//! crate can reach them. The binary, `main.rs`, holds the window and
//! names these at the paths it always has.

pub mod archive;
pub mod carried;
pub mod collections;
pub mod cull;
pub mod delete;
pub mod display;
pub mod edit_in;
pub mod export_queue;
pub mod files;
pub mod filter;
pub mod focus_points;
pub mod gpu_marker;
pub mod grid;
pub mod i18n;
pub mod import;
pub mod instance;
pub mod log;
pub mod mounts;
pub mod outline;
pub mod placeholder;
pub mod rejects;
pub mod scrolling;
pub mod selection;
pub mod sort;
pub mod strip;
pub mod tags;
pub mod wheel;
pub mod zoom;

// The export's names and a preset laid at the start, in the engine's
// crate, which a run with no window shares; named here as they were.
pub use greycard_engine::{export_name, naming, preset_fit};

// The engine's modules these name, at the paths the binary names them
// by: the worker's thumbnail width, which the grid and the strip size
// from; the previews, the frame's key, and the edit's pictures culling
// reads; the sheet and the version an export queue's entry holds.
use greycard_engine::{edited_pictures, previews, queue, sheet, version_key, worker};

// The marker for a shown table's strings, at the path its callers use.
pub use i18n::marked;
