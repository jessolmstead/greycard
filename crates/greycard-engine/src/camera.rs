//! The camera's own picture of a file, and its bytes turned as the
//! develop turns a picture: what the previews, the strip, and the
//! loupe all read, so they agree about which way up a frame stands.

use std::path::Path;
#[cfg(any(test, feature = "testing"))]
use std::path::PathBuf;
#[cfg(any(test, feature = "testing"))]
use std::sync::Mutex;

use greycard_core::raw::Orientation;

/// The files read for a camera picture, for the tests that say a frame
/// out of reach reads none: by path, since the tests run beside each
/// other.
#[cfg(any(test, feature = "testing"))]
pub static FILE_READS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// The camera's own rendering of a file, and the orientation it is
/// to be shown in: of a raw with several, the largest.
pub fn camera_picture(path: &Path) -> anyhow::Result<(image::RgbImage, Orientation)> {
    camera_picture_sized(path, None)
}

/// [`camera_picture`] for a picture made `long_edge` or smaller: of a
/// raw with several previews, the smallest that size or past it.
pub fn camera_picture_at_least(
    path: &Path,
    long_edge: u32,
) -> anyhow::Result<(image::RgbImage, Orientation)> {
    camera_picture_sized(path, Some(long_edge))
}

fn camera_picture_sized(
    path: &Path,
    long_edge: Option<u32>,
) -> anyhow::Result<(image::RgbImage, Orientation)> {
    #[cfg(any(test, feature = "testing"))]
    FILE_READS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(path.to_path_buf());
    if greycard_core::picture::is_picture_path(path) {
        // The picture module's reader for the tag: one answer a file,
        // so the loupe, the strip and the develop cannot disagree
        // about which way up a multi-directory TIFF stands. Its
        // profile too, so a file in a wider space than sRGB shows here
        // as it develops.
        let orientation = greycard_core::picture::orientation_path(path)?;
        let mut decoder = image::ImageReader::open(path)?
            .with_guessed_format()?
            .into_decoder()?;
        let icc = image::ImageDecoder::icc_profile(&mut decoder)
            .ok()
            .flatten();
        let to_srgb = greycard_core::picture::ToSrgb::of_profile(icc.as_deref())?;
        let rgb = to_srgb.rgb8(&image::DynamicImage::from_decoder(decoder)?);
        return Ok((rgb, orientation));
    }
    let preview = match long_edge {
        Some(edge) => greycard_core::decode::preview_path_at_least(path, edge)?,
        None => greycard_core::decode::preview_path(path)?,
    };
    match preview {
        Some(found) => Ok(found),
        None => anyhow::bail!("no camera preview in the file"),
    }
}

/// The size of a `w` by `h` picture once turned.
pub fn turned_size(w: u32, h: u32, orientation: Orientation) -> (u32, u32) {
    if matches!(
        orientation,
        Orientation::Transpose
            | Orientation::Rotate90
            | Orientation::Transverse
            | Orientation::Rotate270
    ) {
        (h, w)
    } else {
        (w, h)
    }
}

/// RGB bytes turned as the engine's `develop::orient` turns a
/// picture, so the loupe agrees with the strip and the develop.
pub fn turn_rgb8(w: u32, h: u32, rgb: &[u8], orientation: Orientation) -> (u32, u32, Vec<u8>) {
    if orientation == Orientation::Normal {
        return (w, h, rgb.to_vec());
    }
    let (w, h) = (w as usize, h as usize);
    let (ow, oh) = turned_size(w as u32, h as u32, orientation);
    let (ow, oh) = (ow as usize, oh as usize);
    let source = |ox: usize, oy: usize| -> (usize, usize) {
        match orientation {
            Orientation::Normal => (ox, oy),
            Orientation::FlipHorizontal => (w - 1 - ox, oy),
            Orientation::Rotate180 => (w - 1 - ox, h - 1 - oy),
            Orientation::FlipVertical => (ox, h - 1 - oy),
            Orientation::Transpose => (oy, ox),
            Orientation::Rotate90 => (oy, h - 1 - ox),
            Orientation::Transverse => (w - 1 - oy, h - 1 - ox),
            Orientation::Rotate270 => (w - 1 - oy, ox),
        }
    };
    let mut out = vec![0u8; ow * oh * 3];
    for oy in 0..oh {
        for ox in 0..ow {
            let (sx, sy) = source(ox, oy);
            let s = (sy * w + sx) * 3;
            let d = (oy * ow + ox) * 3;
            out[d..d + 3].copy_from_slice(&rgb[s..s + 3]);
        }
    }
    (ow as u32, oh as u32, out)
}
