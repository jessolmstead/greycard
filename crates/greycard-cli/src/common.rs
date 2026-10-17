//! What more than one subcommand uses: the command line's file-exists
//! policy, the decode, the denoiser tiers' names, the sizes the model
//! listing says, and the linear TIFF and sRGB preview the develop and
//! the stack both write.

use anyhow::{Context, Result};
use greycard_core::color::{CAT, WORKING_SPACE};
use greycard_core::output::OnExists;
use greycard_core::{RawFrame, RgbSpace, WorkingImage, decode};

/// A policy named on the command line.
pub(crate) fn on_exists_named(name: &str) -> Result<OnExists, String> {
    OnExists::from_name(name)
        .ok_or_else(|| format!("want increment, overwrite or skip, not {name}"))
}

/// The denoiser tiers, for a message to name.
pub(crate) fn denoiser_tiers() -> String {
    greycard_ai::DENOISERS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn load(file: &std::path::Path) -> Result<RawFrame> {
    decode::decode_path(file).with_context(|| format!("decoding {}", file.display()))
}

/// A model's size in whole megabytes, as the editor's sheet says it.
pub(crate) fn megabytes(bytes: u64) -> u64 {
    (bytes as f64 / 1e6).round() as u64
}

/// The `Software` tag on what the CLI writes.
pub(crate) const SOFTWARE: &str = concat!("greycard ", env!("CARGO_PKG_VERSION"));

pub(crate) fn write_linear_tiff(
    image: &WorkingImage,
    path: &std::path::Path,
    provenance: &greycard_core::exif::Provenance<'_>,
) -> Result<()> {
    let data: Vec<u16> = image
        .data
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16)
        .collect();
    let (w, h) = (image.width as u32, image.height as u32);
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    greycard_core::exif::write_rgb16_tiff(&mut writer, w, h, &data, None, Some(provenance))
        .with_context(|| format!("writing {}", path.display()))?;
    std::io::Write::flush(&mut writer).with_context(|| format!("writing {}", path.display()))
}

/// Render the working image to 8-bit sRGB: exposure applied, highlights
/// clipped in the working space, no tone curve.
pub(crate) fn srgb_preview(image: &WorkingImage, exposure: f64) -> Result<image::RgbImage> {
    let to_srgb = WORKING_SPACE
        .to_space_matrix(&RgbSpace::SRGB, CAT)
        .context("working space to sRGB matrix")?
        .rows
        .map(|row| row.map(|v| v as f32));
    let scale = 2f32.powf(exposure as f32);
    let mut data = Vec::with_capacity(image.width * image.height * 3);
    for px in image.pixels() {
        // Clip in the working space so blown areas go to white rather than to
        // whichever channel the gains pushed highest.
        let [r, g, b] = px.map(|v| (v * scale).min(1.0));
        for row in to_srgb {
            let lin = row[0] * r + row[1] * g + row[2] * b;
            data.push((srgb_encode(lin.clamp(0.0, 1.0)) * 255.0).round() as u8);
        }
    }
    image::ImageBuffer::<image::Rgb<u8>, _>::from_raw(image.width as u32, image.height as u32, data)
        .context("image dimensions do not match its data")
}

fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}
