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
pub mod style;

pub use rawler_backend::{
    CameraWhite, MosaicRead, Probe, RawMetadata, RawlerDecoder, inside_decoder,
};
pub use style::{Adaptive, AdaptiveKind, CameraStyle, Maker};

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

/// The camera's own JPEG rendering of a file, if it has one, and the
/// orientation to show it in, read without a memory map; see
/// `rawler_backend::preview_path`.
pub fn preview_path(path: impl AsRef<Path>) -> Result<Option<(image::RgbImage, Orientation)>> {
    rawler_backend::preview_path(path.as_ref())
}

/// Decode with the default backend.
pub fn decode_path(path: impl AsRef<Path>) -> Result<RawFrame> {
    RawlerDecoder.decode_path(path.as_ref())
}

/// [`decode_path`] from the file read whole into memory, never mapped:
/// for a pass over a folder's files (the thumbnails' quick develop of a
/// raw with no preview), whose volume can go away under it.
pub fn decode_path_unmapped(path: impl AsRef<Path>) -> Result<RawFrame> {
    RawlerDecoder.decode_path_unmapped(path.as_ref())
}

/// The frame and the metadata rawler read beside it, for an output that
/// carries EXIF (see [`crate::exif`] and [`crate::dng`]).
pub fn decode_path_with_metadata(path: impl AsRef<Path>) -> Result<(RawFrame, RawMetadata)> {
    RawlerDecoder.decode_path_with_metadata(path.as_ref())
}

/// The camera as the index names it ([`crate::raw::camera_name`] over
/// the decoder's cleaned make and model) for a model as the file's EXIF
/// spells it, the way Lightroom names a camera: "Canon EOS R6m2" is
/// "Canon EOS R6 Mark II". Case is ignored. None for a model the
/// decoder does not know; see `rawler_backend::cleaned_camera`.
pub fn cleaned_camera(exif_model: &str) -> Option<String> {
    rawler_backend::cleaned_camera(exif_model)
}

/// Every EXIF spelling of a model the decoder knows for a camera as the
/// index names it: the other way of [`cleaned_camera`]. Empty for a
/// camera it does not know.
pub fn exif_models(camera: &str) -> Vec<String> {
    rawler_backend::exif_models(camera)
}

/// Camera, lens, ISO, exposure and date from a raw's metadata,
/// without decoding the samples; see `rawler_backend::probe_path`.
/// A picture's is [`crate::picture::probe_path`].
pub fn probe_path(path: impl AsRef<Path>) -> Result<Probe> {
    rawler_backend::probe_path(path.as_ref())
}

/// A raw's as-shot white and calibrations without its samples; see
/// `rawler_backend::white_path`.
pub fn white_path(path: impl AsRef<Path>) -> Result<CameraWhite> {
    rawler_backend::white_path(path.as_ref())
}

/// [`probe_path`] from the file's head and tail alone, where its format
/// allows; the same answer, a fraction of the bytes read. See
/// `rawler_backend::probe_ends_path`.
pub fn probe_ends_path(path: impl AsRef<Path>) -> Result<Probe> {
    rawler_backend::probe_ends_path(path.as_ref())
}

/// [`probe_path`] and the raw's kind of mosaic in one read of the file,
/// from its ends where its format allows and never through a memory
/// map: see `rawler_backend::probe_and_mosaic_path`.
pub fn probe_and_mosaic_path(path: impl AsRef<Path>) -> Result<(Probe, MosaicRead)> {
    rawler_backend::probe_and_mosaic_path(path.as_ref())
}

/// A raw's kind of mosaic alone, without a sample decoded and without a
/// memory map; `Ok(None)` for a raw that is not a mosaic.
pub fn mosaic_path(path: impl AsRef<Path>) -> Result<Option<crate::raw::Mosaic>> {
    rawler_backend::mosaic_path(path.as_ref())
}

/// The extensions taken for a raw the decoder opens.
pub const RAW_EXTENSIONS: [&str; 8] = ["cr3", "cr2", "dng", "nef", "arw", "raf", "orf", "rw2"];

/// Whether a file's name begins with a dot, as a hidden file's does:
/// macOS's `._NAME` resource forks beside the raws on a share or a
/// card, and `.DS_Store`. A folder's listing passes over them, on any
/// platform, since such files travel with the folder; a file named by
/// hand still opens.
pub fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.as_encoded_bytes().starts_with(b"."))
}

/// Whether a path's extension is one of a raw's.
pub fn is_raw_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| RAW_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
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
    /// The kind of mosaic, read from the same description as the size
    /// (rawler's metadata alone does not carry the pattern); `None`
    /// for a picture, a raw that is not a mosaic, or one that will not
    /// say. What the editor's demosaic menu and a sync's demosaic
    /// check go by before the pixels are read.
    pub mosaic: Option<crate::raw::Mosaic>,
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
    let (orientation, size, mosaic) = if crate::picture::is_picture_path(path) {
        // The picture module's own reader, which is the one that
        // decides which way up the picture is drawn.
        let (orientation, size) = crate::picture::stance_path(path)?;
        (orientation, size, None)
    } else {
        rawler_backend::stance_path(path)?
    };
    Ok(Stance {
        orientation,
        size,
        mosaic,
    })
}

/// A raw's [`probe_path`] and its [`stance_path`] in one read of the
/// file, from its ends where its format allows (a CR3's), and how many
/// bytes that read; never through a memory map, so a volume that goes
/// away under it is an error and not a SIGBUS. An error when the
/// metadata will not read; a size
/// or mosaic the image will not give is `None` in the stance. A raw's
/// alone: a picture's two readers are its own. See
/// `rawler_backend::probe_and_stance_counted`.
pub fn probe_and_stance_counted(path: impl AsRef<Path>) -> Result<(Probe, Stance, u64)> {
    let (probe, (orientation, size, mosaic), read) =
        rawler_backend::probe_and_stance_counted(path.as_ref())?;
    Ok((
        probe,
        Stance {
            orientation,
            size,
            mosaic,
        },
        read,
    ))
}

/// [`stance_path`] never through a memory map: a raw's from its ends
/// where its format allows, else read whole ([`probe_and_stance_counted`]),
/// a picture's from its own reader. For a pass over many files, the
/// thumbnails' develop and the lightbox's among them, whose volume can go
/// away under a mapping.
pub fn stance_path_unmapped(path: impl AsRef<Path>) -> Result<Stance> {
    let path = path.as_ref();
    if is_raw_path(path) && !crate::picture::is_picture_path(path) {
        return Ok(probe_and_stance_counted(path)?.1);
    }
    stance_path(path)
}

/// Count from now, on every thread, the raws under `dir` memory-mapped:
/// for a test that a read meant for a volume that can go away
/// ([`probe_and_stance_counted`], [`probe_ends_path`], what the index
/// and the thumbnails read) never maps one.
pub fn watch_maps_under(dir: &Path) {
    rawler_backend::watch_maps_under(dir)
}

/// How many raws under `dir` were memory-mapped since
/// [`watch_maps_under`].
pub fn maps_under(dir: &Path) -> u64 {
    rawler_backend::maps_under(dir)
}

/// The orientation tag a file carries, raw or picture, without
/// decoding a pixel of it. A raw's is its probe's, read from its ends
/// where its format allows ([`probe_ends_path`]) and never mapped: the
/// tag the stance reads, from the same metadata, at a fraction of the
/// bytes. A picture's, or a file named as neither, is the stance's.
pub fn orientation_path(path: impl AsRef<Path>) -> Result<Orientation> {
    let path = path.as_ref();
    if is_raw_path(path) && !crate::picture::is_picture_path(path) {
        return Ok(probe_ends_path(path)?.orientation);
    }
    Ok(stance_path(path)?.orientation)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// A dot at the head of a file's name hides it; a dot inside a
    /// name, or a hidden folder above it, does not. Hidden or not, the
    /// extension still says which decoder opens it.
    #[test]
    fn a_hidden_file_is_one_whose_name_begins_with_a_dot() {
        for name in ["._IMG.CR3", ".IMG.CR3", "._IMG.xmp", ".DS_Store"] {
            assert!(
                super::is_hidden(&Path::new("/share/day").join(name)),
                "{name}"
            );
        }
        for p in [
            "/share/day/IMG.CR3",
            "/share/day/a._b.cr3",
            "/share/.day/IMG.CR3",
            "/share/day/IMG.CR3.gcd",
        ] {
            assert!(!super::is_hidden(Path::new(p)), "{p}");
        }
        assert!(super::is_raw_path(Path::new("/share/day/._IMG.CR3")));
        assert!(crate::picture::is_picture_path(Path::new(
            "/share/day/.x.jpg"
        )));
    }

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
