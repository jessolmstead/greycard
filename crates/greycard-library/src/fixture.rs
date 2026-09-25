//! Frames for tests: small TIFFs carrying real EXIF, written through
//! the engine's export writer, so the probe reads them the way it
//! reads any JPEG's. This crate's own tests use them, and so do the
//! editor's, through the `fixtures` feature; nothing else should.

use std::path::Path;

use greycard_core::exif::{Provenance, write_rgb16_tiff};
use rawler::decoders::RawMetadata;
use rawler::exif::Exif as RawExif;
use rawler::formats::tiff::Rational;

/// A frame's worth of EXIF.
pub struct Frame<'a> {
    pub make: &'a str,
    pub model: &'a str,
    pub lens: &'a str,
    pub iso: u16,
    pub focal: u32,
    pub aperture: (u32, u32),
    pub shutter: (u32, u32),
    pub taken: &'a str,
}

/// A small TIFF carrying real EXIF, as an export writes one:
/// a picture the probe reads the way it reads any JPEG's.
pub fn write_frame(path: &Path, frame: &Frame<'_>, seed: u16) {
    let metadata = RawMetadata {
        exif: RawExif {
            exposure_time: Some(Rational::new(frame.shutter.0, frame.shutter.1)),
            fnumber: Some(Rational::new(frame.aperture.0, frame.aperture.1)),
            iso_speed_ratings: Some(frame.iso),
            date_time_original: Some(frame.taken.into()),
            focal_length: Some(Rational::new(frame.focal, 1)),
            lens_model: Some(frame.lens.into()),
            ..RawExif::default()
        },
        model: frame.model.into(),
        make: frame.make.into(),
        lens: None,
        unique_image_id: None,
        rating: None,
    };
    let (w, h) = (4u32, 3u32);
    let data: Vec<u16> = (0..w * h * 3)
        .map(|i| (i as u16).wrapping_mul(2654).wrapping_add(seed))
        .collect();
    let mut file = std::io::Cursor::new(Vec::new());
    write_rgb16_tiff(
        &mut file,
        w,
        h,
        &data,
        None,
        Some(&Provenance {
            metadata: &metadata,
            software: "greycard-library test",
            width: w,
            height: h,
            srgb: true,
            written: None,
            source_name: None,
            output: None,
            edit: None,
        }),
    )
    .unwrap();
    std::fs::write(path, file.into_inner()).unwrap();
}

pub const R5: Frame<'static> = Frame {
    make: "Canon",
    model: "Canon EOS R5",
    lens: "RF35mm F1.4 L VCM",
    iso: 100,
    focal: 35,
    aperture: (2, 1),
    shutter: (1, 2500),
    taken: "2024:08:24 15:32:39",
};
pub const R6: Frame<'static> = Frame {
    make: "Canon",
    model: "Canon EOS R6m2",
    lens: "RF50mm F1.8 STM",
    iso: 6400,
    focal: 50,
    aperture: (18, 10),
    shutter: (1, 60),
    taken: "2026:09:21 20:05:00",
};
pub const A7: Frame<'static> = Frame {
    make: "SONY",
    model: "ILCE-7M4",
    lens: "FE 24-70mm F2.8 GM II",
    iso: 3200,
    focal: 24,
    aperture: (28, 10),
    shutter: (1, 320),
    taken: "2026:09:02 08:00:00",
};
