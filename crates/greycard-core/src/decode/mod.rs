//! Decoder backends: bytes in, [`RawFrame`] out.
//!
//! The engine never calls a backend's own develop path. A backend's job ends
//! at sensor samples and metadata; everything after that is an engine op with
//! a reference implementation and tests. Keeping the boundary here means a
//! second backend (LibRaw, a vendor SDK, a hand-written DNG reader) is a new
//! module, not a rewrite.

use std::path::Path;

use crate::error::Result;
use crate::raw::{Orientation, RawFrame};

pub mod rawler_backend;

pub use rawler_backend::{Probe, RawMetadata, RawlerDecoder};

pub trait Decoder {
    /// Decode a whole file held in memory.
    fn decode_bytes(&self, bytes: &[u8]) -> Result<RawFrame>;

    /// Decode a file on disk. The default reads it and calls
    /// [`Decoder::decode_bytes`]; backends with a faster path override it.
    fn decode_path(&self, path: &Path) -> Result<RawFrame> {
        let bytes = std::fs::read(path)?;
        self.decode_bytes(&bytes)
    }
}

/// Decode with the default backend.
/// The camera's own JPEG rendering of a file, if it has one, and the
/// orientation to show it in; see `rawler_backend::preview_path`.
pub fn preview_path(path: impl AsRef<Path>) -> Result<Option<(image::RgbImage, Orientation)>> {
    rawler_backend::preview_path(path.as_ref())
}

pub fn decode_path(path: impl AsRef<Path>) -> Result<RawFrame> {
    RawlerDecoder.decode_path(path.as_ref())
}

/// The frame and the metadata rawler read beside it, for an output that
/// carries EXIF (see [`crate::exif`] and [`crate::dng`]).
pub fn decode_path_with_metadata(path: impl AsRef<Path>) -> Result<(RawFrame, RawMetadata)> {
    RawlerDecoder.decode_path_with_metadata(path.as_ref())
}

/// Camera, ISO and exposure from a file's metadata, without decoding
/// the samples; see `rawler_backend::probe_path`.
pub fn probe_path(path: impl AsRef<Path>) -> Result<Probe> {
    rawler_backend::probe_path(path.as_ref())
}

/// How a file's frame stands, without decoding a pixel of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stance {
    /// The orientation tag the file carries. What a frame's quarter
    /// turns are measured from: a turn is stored as quarters on top
    /// of this, so the same edit reads the same however a later build
    /// reads the tag.
    pub orientation: Orientation,
    /// The size a develop comes out at *before* the tag turns it;
    /// `None` when the file will not say this cheaply.
    pub size: Option<(u32, u32)>,
}

impl Stance {
    /// The size the picture is shown at: the frame's own, turned by
    /// the tag and by `turn` quarter turns clockwise on top of it.
    pub fn shown_size(&self, turn: u8) -> Option<(u32, u32)> {
        let o = self.orientation.turned(i32::from(turn % 4));
        self.size.map(|(w, h)| {
            if matches!(
                o,
                Orientation::Transpose
                    | Orientation::Rotate90
                    | Orientation::Transverse
                    | Orientation::Rotate270
            ) {
                (h, w)
            } else {
                (w, h)
            }
        })
    }
}

/// How a file's frame stands, raw or picture: its orientation tag and
/// the size a develop of it comes out at, with no pixel decoded.
///
/// One reader a file, so nothing can disagree with the picture on
/// screen about which way up it is or what shape it is.
pub fn stance_path(path: impl AsRef<Path>) -> Result<Stance> {
    let path = path.as_ref();
    let (orientation, size) = if crate::picture::is_picture_path(path) {
        // The picture module's own reader, which is the one that
        // decides which way up the picture is drawn.
        crate::picture::stance_path(path)?
    } else {
        rawler_backend::stance_path(path)?
    };
    Ok(Stance { orientation, size })
}

/// The orientation tag a file carries, raw or picture, without
/// decoding a pixel of it.
pub fn orientation_path(path: impl AsRef<Path>) -> Result<Orientation> {
    Ok(stance_path(path)?.orientation)
}

#[cfg(test)]
mod tests {
    /// With `GREYCARD_SAMPLES` a directory of raws: each file's preview,
    /// its size, orientation and the time to read it, printed.
    #[test]
    #[ignore]
    fn previews_of_the_samples() {
        let Some(dir) = std::env::var_os("GREYCARD_SAMPLES") else {
            return;
        };
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let path = e.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(
                ext.to_ascii_lowercase().as_str(),
                "cr3" | "cr2" | "dng" | "nef" | "arw" | "raf"
            ) {
                continue;
            }
            let t = std::time::Instant::now();
            match super::preview_path(&path) {
                Ok(Some((img, o))) => println!(
                    "{}: {}x{} {:?} in {:.0} ms",
                    path.file_name().unwrap().to_string_lossy(),
                    img.width(),
                    img.height(),
                    o,
                    t.elapsed().as_secs_f64() * 1000.0
                ),
                Ok(None) => println!("{}: no preview", path.display()),
                Err(e) => println!("{}: {e}", path.display()),
            }
        }
    }
}
