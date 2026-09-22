//! Linear DNG output.
//!
//! A linear DNG stores demosaiced camera-space samples with the same color
//! tags a camera's own DNG would carry: `AsShotNeutral`, `ColorMatrix1/2`,
//! `CalibrationIlluminant1/2`. Nothing in the file is developed beyond the
//! demosaic. No white balance, no matrix, no tone: the consumer does those,
//! exactly as it would for the original file, and the DNG tags give it what
//! it needs. That makes this the pre-processor's product: our demosaic,
//! their editor.
//!
//! The container is written with rawler's DNG encoder, which is independent
//! of its develop path. The samples, levels and color tags are all ours.

use std::io::{Seek, Write};

use ::rawler::RawImage;
use ::rawler::decoders::Camera;
use ::rawler::dng::writer::DngWriter;
use ::rawler::dng::{CropMode, DNG_VERSION_V1_4, DngCompression, DngPhotometricConversion};
use ::rawler::formats::tiff::{Rational, SRational};
use ::rawler::imgop::xyz::Illuminant;
use ::rawler::pixarray::PixU16;
use ::rawler::rawimage::{BlackLevel, RawPhotometricInterpretation, WhiteLevel};
use ::rawler::tags::{DngTag, ExifTag, TiffCommonTag};
use rawcolor::CalibrationIlluminant;

use crate::decode::rawler_backend::RawMetadata;
use crate::error::{Error, Result};
use crate::image::CameraImage;
use crate::raw::{Calibration, Orientation, RawFrame};

/// How the samples are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    /// Lossless JPEG, as camera DNGs use. Roughly halves the file.
    #[default]
    Lossless,
    Uncompressed,
}

/// What to write besides the samples.
#[derive(Debug, Clone, Copy, Default)]
pub struct LinearDngOptions<'a> {
    pub compression: Compression,
    /// EXIF and lens metadata from the source file, if the decoder gave it.
    pub metadata: Option<&'a RawMetadata>,
    /// An 8-bit sRGB rendering, used for the embedded preview and the
    /// thumbnail. Without one, viewers that show the file without decoding
    /// it show nothing.
    pub preview: Option<&'a image::RgbImage>,
    /// Value for the `Software` tag.
    pub software: Option<&'a str>,
    /// The value the image's samples reach, 1.0 or above: reconstructed
    /// highlights go above 1.0 in camera space, and a file can only
    /// hold up to its white level. The samples are stored scaled by its
    /// reciprocal, and `BaselineExposure` tells a reader to brighten by
    /// as much, so the rendering is the same and nothing is cut. `None`
    /// is 1.0: stored as they are, no tag.
    pub headroom: Option<f32>,
}

/// The sample scale in the file: 16 bits, black at 0.
pub const WHITE_LEVEL: u16 = u16::MAX;

/// Write `image` as a linear DNG.
///
/// `gains` are the camera-space white balance gains (RGB, green 1) that
/// neutralize the scene white; their reciprocals become `AsShotNeutral`.
/// The color matrices come from `frame`'s calibrations: the warmest and
/// coolest with a known illuminant, the same pair the engine's own profile
/// uses. The image is written in sensor orientation with the EXIF
/// `Orientation` tag set, as camera files are.
pub fn write_linear_dng<W: Write + Seek>(
    out: W,
    frame: &RawFrame,
    image: &CameraImage,
    gains: [f32; 3],
    options: &LinearDngOptions<'_>,
) -> Result<()> {
    if gains.iter().any(|g| !g.is_finite() || *g <= 0.0) {
        return Err(Error::Unsupported(format!("white balance gains {gains:?}")));
    }
    let headroom = options.headroom.unwrap_or(1.0);
    if !headroom.is_finite() || headroom < 1.0 {
        return Err(Error::Unsupported(format!("headroom {headroom}")));
    }
    let calibrations = calibration_pair(&frame.calibrations);
    if calibrations.is_empty() {
        return Err(Error::NoCalibration);
    }

    let raw = raw_image_for(frame, image, gains, headroom);
    let compression = match options.compression {
        Compression::Lossless => DngCompression::Lossless,
        Compression::Uncompressed => DngCompression::Uncompressed,
    };

    let mut dng = DngWriter::new(out, DNG_VERSION_V1_4).map_err(encode)?;

    // With a thumbnail on the root IFD the image goes in a sub-IFD, which
    // is the layout readers expect; without one it can sit on the root.
    let mut main = if options.preview.is_some() {
        dng.subframe(0)
    } else {
        dng.subframe_on_root(0)
    };
    main.raw_image(
        &raw,
        CropMode::None,
        compression,
        DngPhotometricConversion::Original,
        1,
    )
    .map_err(encode)?;
    main.finalize().map_err(encode)?;

    if let Some(preview) = options.preview {
        let preview = image::DynamicImage::ImageRgb8(preview.clone());
        let mut sub = dng.subframe(1);
        sub.preview(&preview, 0.85).map_err(encode)?;
        sub.finalize().map_err(encode)?;
        dng.thumbnail(&preview).map_err(encode)?;
    }

    dng.load_base_tags(&raw).map_err(encode)?;
    if let Some(metadata) = options.metadata {
        dng.load_metadata(metadata).map_err(encode)?;
        // rawler copies these even when the source left them blank.
        let root = dng.root_ifd_mut();
        if metadata.exif.artist.as_deref().is_none_or(str::is_empty) {
            root.remove_tag(ExifTag::Artist);
        }
        if metadata.exif.copyright.as_deref().is_none_or(str::is_empty) {
            root.remove_tag(ExifTag::Copyright);
        }
    }
    let root = dng.root_ifd_mut();
    root.add_tag(ExifTag::Orientation, orientation_code(frame.orientation));
    root.add_tag(
        TiffCommonTag::Software,
        options
            .software
            .unwrap_or(concat!("greycard ", env!("CARGO_PKG_VERSION"))),
    );
    root.add_tag(DngTag::AsShotNeutral, neutral(gains).as_slice());
    if headroom > 1.0 {
        root.add_tag(DngTag::BaselineExposure, baseline_exposure(headroom));
    }

    for (slot, calibration) in calibrations.iter().enumerate() {
        let slot = slot + 1;
        let illuminant = Illuminant::try_from(calibration.illuminant).map_err(Error::Encode)?;
        dng.color_matrix(slot, illuminant, srationals(&calibration.color_matrix));
        if let Some(forward) = &calibration.forward_matrix {
            let tag = if slot == 1 {
                DngTag::ForwardMatrix1
            } else {
                DngTag::ForwardMatrix2
            };
            dng.root_ifd_mut()
                .add_tag(tag, srationals(forward).as_slice());
        }
    }

    dng.close().map_err(encode)
}

/// The rawler image the encoder consumes: our samples at 16 bits with a
/// zero black, `headroom` at the white level, no camera definition, no
/// color tags (those are added separately so the choice of matrices
/// is ours).
fn raw_image_for(
    frame: &RawFrame,
    image: &CameraImage,
    gains: [f32; 3],
    headroom: f32,
) -> RawImage {
    let scale = WHITE_LEVEL as f32 / headroom;
    let data: Vec<u16> = image
        .data
        .iter()
        .map(|v| (v.clamp(0.0, headroom) * scale).round() as u16)
        .collect();
    let cpp = CameraImage::CHANNELS;
    let pixels = PixU16::new_with(data, image.width * cpp, image.height);

    let mut camera = Camera::new();
    camera.make = frame.make.clone();
    camera.model = frame.model.clone();
    camera.clean_make = frame.make.clone();
    camera.clean_model = frame.model.clone();

    // rawler's encoder writes AsShotNeutral from these, RGBE order; the
    // fourth is unused for three-color images. Rewritten afterwards with
    // more precision, but kept consistent here.
    let wb = [gains[0], gains[1], gains[2], f32::NAN];
    let black = BlackLevel::new(&[0u16; 3], 1, 1, cpp);
    let white = WhiteLevel::new_bits(16, cpp);
    RawImage::new(
        camera,
        pixels,
        cpp,
        wb,
        RawPhotometricInterpretation::LinearRaw,
        Some(black),
        Some(white),
        false,
    )
}

/// The calibrations to write: warmest and coolest by illuminant temperature,
/// or the one there is. Unknown illuminants and non-3x3 matrices are skipped,
/// as [`crate::color::profile_from_frame`] skips them.
fn calibration_pair(calibrations: &[Calibration]) -> Vec<&Calibration> {
    let mut usable: Vec<(f64, &Calibration)> = calibrations
        .iter()
        .filter(|c| c.color_matrix.len() == 9)
        .filter_map(|c| {
            let illuminant = CalibrationIlluminant::from_exif_code(c.illuminant)?;
            Some((illuminant.cct(), c))
        })
        .collect();
    usable.sort_by(|a, b| a.0.total_cmp(&b.0));
    match usable.len() {
        0 => vec![],
        1 => vec![usable[0].1],
        n => vec![usable[0].1, usable[n - 1].1],
    }
}

/// `AsShotNeutral`: the camera-space color of a neutral, green at 1.
fn neutral(gains: [f32; 3]) -> [Rational; 3] {
    const DENOMINATOR: u32 = 1_000_000;
    gains.map(|g| Rational::new_f64(f64::from(gains[1]) / f64::from(g), DENOMINATOR))
}

/// `BaselineExposure`: the stops a reader brightens by, undoing the
/// headroom's scale.
fn baseline_exposure(headroom: f32) -> SRational {
    const DENOMINATOR: i32 = 10_000;
    SRational::new(
        (f64::from(headroom).log2() * f64::from(DENOMINATOR)).round() as i32,
        DENOMINATOR,
    )
}

fn srationals(matrix: &[f32]) -> Vec<SRational> {
    const DENOMINATOR: i32 = 10_000;
    matrix
        .iter()
        .map(|v| {
            SRational::new(
                (f64::from(*v) * f64::from(DENOMINATOR)).round() as i32,
                DENOMINATOR,
            )
        })
        .collect()
}

/// EXIF `Orientation` value; the enum is declared in tag order.
fn orientation_code(orientation: Orientation) -> u16 {
    orientation as u16 + 1
}

fn encode(e: ::rawler::formats::tiff::TiffError) -> Error {
    Error::Encode(e.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::decode::{Decoder, RawlerDecoder};
    use crate::develop::tests::flat_frame;
    use crate::develop::{DevelopSettings, demosaic, develop};
    use crate::raw::SensorLayout;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn round_trip(compression: Compression) -> RawFrame {
        let frame = flat_frame();
        let out = demosaic(&frame, &DevelopSettings::default()).unwrap();
        let mut buf = Cursor::new(Vec::new());
        let options = LinearDngOptions {
            compression,
            ..Default::default()
        };
        write_linear_dng(
            &mut buf,
            &frame,
            &out.image,
            out.white_balance.coefficients_f32(),
            &options,
        )
        .unwrap();
        RawlerDecoder.decode_bytes(&buf.into_inner()).unwrap()
    }

    #[test]
    fn linear_dng_round_trips_through_the_decoder() {
        for compression in [Compression::Uncompressed, Compression::Lossless] {
            let back = round_trip(compression);
            assert_eq!(back.layout, SensorLayout::Linear, "{compression:?}");
            assert_eq!((back.width, back.height, back.channels), (4, 4, 3));
            assert_eq!((back.make.as_str(), back.model.as_str()), ("Test", "Cam"));
            assert_eq!(back.levels.black.values, vec![0.0; 3]);
            assert_eq!(back.levels.white.values, vec![65535.0; 3]);

            // Camera-native samples, levels applied, no white balance.
            let want = [0.5f32, 1.0, 0.25].map(|v| (v * 65535.0).round());
            for px in back.samples.to_f32().as_chunks::<3>().0 {
                assert_eq!(*px, want, "{compression:?}");
            }

            // The color tags describe the same camera and white.
            let gains = back.as_shot_coefficients.unwrap();
            assert!(
                (gains[0] - 2.0).abs() < 1e-4 && (gains[2] - 4.0).abs() < 1e-4,
                "{gains:?}"
            );
            assert_eq!(back.calibrations.len(), 1);
            let cal = &back.calibrations[0];
            assert_eq!(cal.illuminant, 21);
            for (a, b) in cal
                .color_matrix
                .iter()
                .zip(flat_frame().calibrations[0].color_matrix.iter())
            {
                assert!(close(*a, *b), "{a} vs {b}");
            }

            // Developing the DNG gives what developing the original gives.
            let developed = develop(&back, &DevelopSettings::default()).unwrap();
            for px in developed.image.pixels() {
                assert!(px.iter().all(|v| (v - 1.0).abs() < 2e-2), "{px:?}");
            }
        }
    }

    #[test]
    fn orientation_survives() {
        let mut frame = flat_frame();
        frame.orientation = Orientation::Rotate270;
        let out = demosaic(&frame, &DevelopSettings::default()).unwrap();
        let mut buf = Cursor::new(Vec::new());
        write_linear_dng(
            &mut buf,
            &frame,
            &out.image,
            [2.0, 1.0, 4.0],
            &LinearDngOptions::default(),
        )
        .unwrap();
        let back = RawlerDecoder.decode_bytes(&buf.into_inner()).unwrap();
        assert_eq!(back.orientation, Orientation::Rotate270);
        assert_eq!(orientation_code(Orientation::Normal), 1);
        assert_eq!(orientation_code(Orientation::Rotate90), 6);
    }

    #[test]
    fn headroom_keeps_values_above_one() {
        let frame = flat_frame();
        let image = CameraImage::from_data(4, 4, vec![2.0; 48]).unwrap();
        let mut buf = Cursor::new(Vec::new());
        let options = LinearDngOptions {
            headroom: Some(2.0),
            ..Default::default()
        };
        write_linear_dng(&mut buf, &frame, &image, [2.0, 1.0, 4.0], &options).unwrap();
        let back = RawlerDecoder.decode_bytes(&buf.into_inner()).unwrap();
        assert!(back.samples.to_f32().iter().all(|v| *v == 65535.0));
        let be = baseline_exposure(2.0);
        assert_eq!((be.n, be.d), (10_000, 10_000));
        assert_eq!(baseline_exposure(1.0).n, 0);
        let options = LinearDngOptions {
            headroom: Some(0.5),
            ..Default::default()
        };
        let mut buf = Cursor::new(Vec::new());
        assert!(write_linear_dng(&mut buf, &frame, &image, [2.0, 1.0, 4.0], &options).is_err());
    }

    #[test]
    fn calibration_pair_is_warmest_and_coolest() {
        let cal = |illuminant, n| Calibration {
            illuminant,
            color_matrix: vec![0.1; n],
            forward_matrix: None,
        };
        // A (2856 K), D50, D65, plus one unknown and one 4x3.
        let cals = vec![cal(23, 9), cal(21, 9), cal(17, 9), cal(0, 9), cal(21, 12)];
        let pair = calibration_pair(&cals);
        assert_eq!(
            pair.iter().map(|c| c.illuminant).collect::<Vec<_>>(),
            vec![17, 21]
        );
        assert_eq!(calibration_pair(&cals[..1]).len(), 1);
        assert!(calibration_pair(&[cal(0, 9)]).is_empty());
    }

    #[test]
    fn neutral_is_the_reciprocal_of_the_gains() {
        let n = neutral([2.0, 1.0, 4.0]);
        assert_eq!(n[0].n * 2, n[0].d);
        assert_eq!(n[1].n, n[1].d);
        assert_eq!(n[2].n * 4, n[2].d);
        let m = srationals(&[1.0, -0.5]);
        assert_eq!((m[0].n, m[0].d), (10_000, 10_000));
        assert_eq!(m[1].n, -5_000);
    }
}
