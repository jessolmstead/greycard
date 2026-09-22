//! greycard-core: a RAW development engine that gets the first steps right.
//!
//! The pipeline this crate implements, in order:
//!
//! 1. **Decode** a camera file into a [`RawFrame`]: sensor samples plus the
//!    metadata needed to interpret them. Decoding lives behind the
//!    [`decode::Decoder`] trait; the only backend today wraps `rawler`.
//! 2. **Normalize** black and white levels to a 0..1 sensor scale.
//! 3. **White balance in camera space**: per-channel gains derived
//!    colorimetrically from the camera's calibration and a scene white.
//! 4. **Demosaic** the color filter array.
//! 5. **Camera matrix** interpolated for the scene illuminant, into the
//!    working space, linear Rec.2020 ([`color::WORKING_SPACE`]).
//!
//! Every operation has a CPU reference implementation here. GPU
//! implementations, when they arrive, are tested against these.
//!
//! A picture that is not a raw (a JPEG, PNG or TIFF) enters at the end
//! of that list through [`picture`]: its samples taken through the
//! file's own curve and primaries into the working space, upright.
//!
//! One output format lives here too: [`dng`] writes the demosaiced
//! camera-space image as a linear DNG, the pre-processor's product, which
//! any RAW editor then treats as it would the original file.
//!
//! Two pieces sit beside the pipeline rather than in it: [`guided`],
//! the guided filter, which two consumers want and neither may depend
//! on the other for; and [`register`], which fits the transform that
//! puts one frame on top of another, what every stack — a handheld
//! bracket, a focus stack, a panorama's refinement — needs before it
//! can merge anything; and [`stack`], which merges a focus stack into
//! the frame that is sharp everywhere.
//!
//! What this crate deliberately does not contain: any tone mapping, display
//! transform, edit schema or UI concern. Those belong to consumers.

pub mod color;
pub mod dcp;
pub mod decode;
pub mod develop;
pub mod dng;
pub mod error;
pub mod exif;
pub mod guided;
pub mod image;
pub mod lut;
pub mod output;
pub mod picture;
pub mod raw;
pub mod register;
pub mod stack;

pub use color::{Profile, WhitePoint};
pub use dcp::Dcp;
pub use develop::{
    DemosaicMethod, Demosaiced, DevelopSettings, Developed, HighlightMode, demosaic, develop,
};
pub use error::{Error, Result};
pub use image::{CameraImage, WorkingImage};
pub use lut::Lut3d;
pub use output::{OnExists, Resolved};
pub use picture::Picture;
pub use raw::RawFrame;

// Re-exported so consumers can name the color types without depending on
// rawcolor directly.
pub use rawcolor::{CalibrationIlluminant, CameraProfile, RgbSpace, TempTint, WhiteBalance};
