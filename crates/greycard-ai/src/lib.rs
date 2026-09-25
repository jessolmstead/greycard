//! Learned models for greycard: the runtime, the model store, and the
//! masks. Nothing in `greycard-core` depends on this crate; the editor
//! wires it in.
//!
//! The runtime is ONNX Runtime through `ort`, on the first execution
//! provider that runs a given model (see [`runtime`]). Models are data,
//! fetched on first use into the [`store`] after their license has
//! been shown, and are never redistributed by greycard (see
//! [`registry`]). The design and its reasons are in the notes, §34.

pub mod cache;
pub mod denoise;
pub mod fill;
pub mod image;
pub mod refine;
pub mod registry;
pub mod runtime;
pub mod sam;
pub mod sky;
pub mod store;
pub mod subject;

pub use cache::Cache as DenoiseCache;
pub use denoise::Denoiser;
pub use fill::Fill;
pub use image::{Mask, Rgb8, Rgbf};
pub use refine::refine;
pub use registry::{
    DENOISE_BALANCED, DENOISE_BEST, DENOISE_FAST, DENOISERS, FILL, MODELS, Model, SAM, SKY,
    SUBJECT, SUBJECT_WEBGPU, denoiser, model, tier_of,
};
pub use runtime::Provider;
pub use sam::{Prompt, Sam};
pub use sky::Sky;
pub use store::{Progress, Store};
pub use subject::Subject;
