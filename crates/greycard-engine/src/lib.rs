//! The develop below the window: one list of stages for every develop
//! of an edit, derived from the edit and what the develop is for
//! (`stages`), the walk that runs it and keeps what it passes for the
//! next (`walk`), the one finish both ends read (`finish`), and the
//! worker that takes the window's jobs and runs them (`worker`), with
//! what it holds: the reduced develop, the thumbnail pool, the
//! previews, the edit's pictures, the models, and the export queue.
//! Over the export's develop of a frame not open, the camera match's
//! runner (`camera_match`) and the measurements of it and of the
//! Lightroom import (`match_compare`, `measure_baseline`,
//! `measure_crop`, `measure_lightroom`, and `fit_lightroom`). The
//! tripod HDR merge walks its frames through the same list (`merge`).
//!
//! It stands between the render crate and the window, and depends on
//! core, edit, edit-io, render, gpu, ai, lens, match, and the library
//! alone: never on the window, Slint, or anything that names a
//! window's state. The GPU is lent to it as an optional context by
//! whoever owns a device, never made here; without one, every stage
//! runs on the CPU.

pub mod ai;
pub mod camera;
pub mod camera_match;
pub mod edited_pictures;
pub mod export_name;
pub mod finish;
pub mod fit_lightroom;
pub mod headless;
pub mod highlight_fill;
pub mod lens;
pub mod made;
pub mod mask_base;
pub mod match_compare;
pub mod measure_baseline;
pub mod measure_crop;
pub mod measure_lightroom;
pub mod merge;
pub mod naming;
pub mod preset_fit;
pub mod previews;
pub mod queue;
pub mod reduced;
pub mod settings;
pub mod sheet;
pub mod shown;
pub mod stages;
pub mod thumbpool;
pub mod version_key;
pub mod walk;
pub mod worker;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
