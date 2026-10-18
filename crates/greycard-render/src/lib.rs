//! A developed frame made into a picture or a file: the finish on the
//! CPU (the viewport's transform, the reference its shader is held to),
//! AgX, the masks' detail bands, the straighten and crop, the box
//! reduction, the border, the watermark and the export.
//!
//! It sits between the edit (`greycard-edit`) and the window
//! (`greycard-ui`): nothing here knows of the window's state, its
//! Slint types, its panels or its worker, so the command line and a
//! headless run can render through it without the window.

pub mod agx;
pub mod border;
pub mod detail;
pub mod export;
pub mod finish;
pub mod geometry;
pub mod reduce;
pub mod watermark;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
