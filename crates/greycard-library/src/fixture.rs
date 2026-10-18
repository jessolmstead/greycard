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

/// A small synthetic mosaic DNG whose filters are `pattern`, rawler's
/// spelling: four letters for a 2x2 Bayer cell (`RGGB`), thirty-six for
/// a 6x6 X-Trans one. Samples are made up, not a camera's: what the
/// index reads a raw's kind of mosaic from without any real raw in the
/// tree.
pub fn write_mosaic_dng(path: &Path, pattern: &str) {
    write_mosaic_dng_for(path, pattern, "Synthetic", "Mosaic");
}

/// [`write_mosaic_dng`] naming `make` and `model` as its body, for
/// what reads a body off a raw (the Lightroom mapper's offset).
pub fn write_mosaic_dng_for(path: &Path, pattern: &str, make: &str, model: &str) {
    use rawler::RawImage;
    use rawler::cfa::{CFA, PlaneColor};
    use rawler::decoders::Camera;
    use rawler::dng::writer::DngWriter;
    use rawler::dng::{CropMode, DNG_VERSION_V1_4, DngCompression, DngPhotometricConversion};
    use rawler::pixarray::PixU16;
    use rawler::rawimage::{BlackLevel, CFAConfig, RawPhotometricInterpretation, WhiteLevel};

    let (w, h) = (24usize, 18usize);
    let data: Vec<u16> = (0..w * h).map(|i| (i as u16).wrapping_mul(97)).collect();
    let cfa = CFA::new(pattern);
    let mut camera = Camera::new();
    camera.make = make.into();
    camera.model = model.into();
    camera.clean_make = camera.make.clone();
    camera.clean_model = camera.model.clone();
    camera.cfa = cfa.clone();
    let raw = RawImage::new(
        camera,
        PixU16::new_with(data, w, h),
        1,
        [1.0, 1.0, 1.0, f32::NAN],
        RawPhotometricInterpretation::Cfa(CFAConfig::new(&cfa, &PlaneColor::default())),
        Some(BlackLevel::new(&[0u16], 1, 1, 1)),
        Some(WhiteLevel::new_bits(16, 1)),
        false,
    );
    let mut file = std::io::Cursor::new(Vec::new());
    let mut dng = DngWriter::new(&mut file, DNG_VERSION_V1_4).unwrap();
    let mut main = dng.subframe_on_root(0);
    main.raw_image(
        &raw,
        CropMode::None,
        DngCompression::Uncompressed,
        DngPhotometricConversion::Original,
        1,
    )
    .unwrap();
    main.finalize().unwrap();
    dng.load_base_tags(&raw).unwrap();
    dng.close().unwrap();
    std::fs::write(path, file.into_inner()).unwrap();
}

/// Overwrite the inline SHORT values of every entry for `tag` in a
/// TIFF's IFDs (the root chain and its SubIFDs): how a test damages a
/// DNG [`write_mosaic_dng`] wrote, a CFA repeat of 65535 a side, a raw
/// image compressed as nothing reads. Only entries of type SHORT whose
/// values fit inline (two at most) are touched; answers how many.
pub fn patch_short_tag(path: &Path, tag: u16, values: &[u16]) -> usize {
    let mut b = std::fs::read(path).unwrap();
    let little = &b[..2] == b"II";
    let u16_at = |b: &[u8], o: usize| {
        let x = [b[o], b[o + 1]];
        if little {
            u16::from_le_bytes(x)
        } else {
            u16::from_be_bytes(x)
        }
    };
    let u32_at = |b: &[u8], o: usize| {
        let x = [b[o], b[o + 1], b[o + 2], b[o + 3]];
        if little {
            u32::from_le_bytes(x)
        } else {
            u32::from_be_bytes(x)
        }
    };
    let put16 = |b: &mut Vec<u8>, o: usize, v: u16| {
        let x = if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        b[o..o + 2].copy_from_slice(&x);
    };
    let mut patched = 0;
    let mut queue = vec![u32_at(&b, 4) as usize];
    let mut seen = std::collections::HashSet::new();
    while let Some(mut off) = queue.pop() {
        while off != 0 && off + 2 <= b.len() && seen.insert(off) {
            let n = u16_at(&b, off) as usize;
            for i in 0..n {
                let p = off + 2 + 12 * i;
                let (t, typ, count) = (u16_at(&b, p), u16_at(&b, p + 2), u32_at(&b, p + 4));
                if t == tag && typ == 3 && count as usize == values.len() && values.len() <= 2 {
                    for (k, v) in values.iter().enumerate() {
                        put16(&mut b, p + 8 + 2 * k, *v);
                    }
                    patched += 1;
                }
                if t == 0x14A {
                    if count == 1 {
                        queue.push(u32_at(&b, p + 8) as usize);
                    } else {
                        let at = u32_at(&b, p + 8) as usize;
                        for k in 0..count as usize {
                            queue.push(u32_at(&b, at + 4 * k) as usize);
                        }
                    }
                }
            }
            off = u32_at(&b, off + 2 + 12 * n) as usize;
        }
    }
    std::fs::write(path, b).unwrap();
    patched
}
