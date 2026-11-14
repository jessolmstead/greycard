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
use ::rawler::dng::{
    CompressionParams, CropMode, DNG_VERSION_V1_4, DngCompression, DngPhotometricConversion,
};
use ::rawler::formats::tiff::{Rational, SRational};
use ::rawler::imgop::xyz::Illuminant;
use ::rawler::pixarray::PixU16;
use ::rawler::rawimage::{BlackLevel, RawImageData, RawPhotometricInterpretation, WhiteLevel};
use ::rawler::tags::{DngTag, ExifTag, TiffCommonTag};
use rawcolor::CalibrationIlluminant;

use crate::decode::rawler_backend::RawMetadata;
use crate::error::{Error, Result};
use crate::image::CameraImage;
use crate::raw::{Calibration, Orientation, RawFrame, SensorLayout};

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
/// `Orientation` tag set, as camera files are. A monochrome frame's is
/// one plane, with neither a white nor a matrix, as its camera's was.
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
    if !monochrome(frame) && calibration_pair(&frame.calibrations).is_empty() {
        return Err(Error::NoCalibration);
    }

    let raw = raw_image_for(frame, image, gains, headroom);
    let compression = match options.compression {
        Compression::Lossless => DngCompression::Lossless,
        Compression::Uncompressed => DngCompression::Uncompressed,
    };
    write_container(
        out,
        frame,
        &raw,
        (compression, &CompressionParams::default()),
        gains,
        options,
        None,
    )
}

/// What a merge's file carries beyond a linear DNG's tags.
#[derive(Debug, Clone, Copy)]
pub struct Merged<'a> {
    /// `NoiseProfile`'s pair, `(S, O)`, in the stored levels.
    pub noise_profile: [f64; 2],
    /// What was merged, in our `DNGPrivateData`.
    pub tag: &'a crate::merge::Tag,
}

/// The samples of a merge as they are stored: half floats with the
/// floating-point predictor and Deflate (DNG 1.4), about a third of
/// 32-bit floats uncompressed. A half keeps 11 bits of precision down
/// to 2^-14 of white and fades below; the merge's noise is coarser
/// than either well before then.
const MERGED_SAMPLES: (DngCompression, u8) = (DngCompression::Deflate, 16);

/// Write a merge as a linear DNG with float samples: `image` (camera
/// RGB on the reference's scale) stored divided by `options.headroom`,
/// white at 1, `BaselineExposure` saying the stops, and a
/// `NoiseProfile` and our tag beside the linear DNG's own. A float
/// keeps the deep shadows a 16-bit file would round away once the
/// headroom is several stops.
pub fn write_merged_dng<W: Write + Seek>(
    out: W,
    frame: &RawFrame,
    image: &CameraImage,
    gains: [f32; 3],
    options: &LinearDngOptions<'_>,
    merged: &Merged<'_>,
) -> Result<()> {
    if gains.iter().any(|g| !g.is_finite() || *g <= 0.0) {
        return Err(Error::Unsupported(format!("white balance gains {gains:?}")));
    }
    let headroom = options.headroom.unwrap_or(1.0);
    if !headroom.is_finite() || headroom < 1.0 {
        return Err(Error::Unsupported(format!("headroom {headroom}")));
    }
    if !monochrome(frame) && calibration_pair(&frame.calibrations).is_empty() {
        return Err(Error::NoCalibration);
    }
    let (compression, bits) = MERGED_SAMPLES;
    // No 16-bit samples made only to be replaced.
    let mut raw = raw_image_for(
        frame,
        &CameraImage::from_data(0, 0, Vec::new())?,
        gains,
        1.0,
    );
    raw.width = image.width;
    raw.height = image.height;
    let store = |v: f32| {
        if v.is_finite() {
            (v / headroom).max(0.0)
        } else {
            0.0
        }
    };
    raw.data = RawImageData::Float(if monochrome(frame) {
        plane(&image.data).map(store).collect()
    } else {
        image.data.iter().map(|&v| store(v)).collect()
    });
    raw.bps = 32;
    raw.whitelevel = WhiteLevel::new(vec![1; raw.cpp]);
    write_container(
        out,
        frame,
        &raw,
        (
            compression,
            &CompressionParams {
                deflate_float_bits: bits,
                ..CompressionParams::default()
            },
        ),
        gains,
        options,
        Some(merged),
    )
}

/// The container around `raw`: the preview, the base tags, the
/// metadata, the orientation, the white, the baseline, the color
/// matrices, and a merge's own tags. `packing` is how the samples are
/// stored; the headroom is the options', as the caller checked it.
fn write_container<W: Write + Seek>(
    out: W,
    frame: &RawFrame,
    raw: &RawImage,
    packing: (DngCompression, &CompressionParams),
    gains: [f32; 3],
    options: &LinearDngOptions<'_>,
    merged: Option<&Merged<'_>>,
) -> Result<()> {
    let (compression, params) = packing;
    let headroom = options.headroom.unwrap_or(1.0);
    // A monochrome sensor's file has no color to describe: DNG's form
    // of it carries neither a white nor a matrix.
    let mono = monochrome(frame);
    let calibrations = if mono {
        Vec::new()
    } else {
        calibration_pair(&frame.calibrations)
    };
    if calibrations.is_empty() && !mono {
        return Err(Error::NoCalibration);
    }
    let mut dng = DngWriter::new(out, DNG_VERSION_V1_4).map_err(encode)?;

    // With a thumbnail on the root IFD the image goes in a sub-IFD, which
    // is the layout readers expect; without one it can sit on the root.
    let mut main = if options.preview.is_some() {
        dng.subframe(0)
    } else {
        dng.subframe_on_root(0)
    };
    main.raw_image(
        raw,
        CropMode::None,
        compression,
        DngPhotometricConversion::Original,
        params,
    )
    .map_err(encode)?;
    if let Some(m) = merged {
        main.ifd_mut()
            .add_tag(DngTag::NoiseProfile, m.noise_profile.as_slice());
    }
    main.finalize().map_err(encode)?;

    if let Some(preview) = options.preview {
        let preview = image::DynamicImage::ImageRgb8(preview.clone());
        let mut sub = dng.subframe(1);
        sub.preview(&preview, 0.85).map_err(encode)?;
        sub.finalize().map_err(encode)?;
        dng.thumbnail(&preview).map_err(encode)?;
    }

    dng.load_base_tags(raw).map_err(encode)?;
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
    if !mono {
        root.add_tag(DngTag::AsShotNeutral, neutral(gains).as_slice());
    }
    if let Some(m) = merged {
        root.add_tag(DngTag::DNGPrivateData, m.tag.to_bytes().as_slice());
    }
    // The headroom's stops, and the source's own tag carried on top
    // when it had one: a phone's DNG written out linear keeps its word.
    let carried = frame.dng.baseline_exposure;
    if carried.is_some() || headroom > 1.0 {
        let stops = carried.unwrap_or(0.0) + headroom.log2();
        root.add_tag(DngTag::BaselineExposure, baseline_exposure(stops));
    }

    for (slot, calibration) in calibrations.iter().enumerate() {
        let slot = slot + 1;
        let illuminant = Illuminant::try_from(calibration.illuminant).map_err(Error::Encode)?;
        let (illuminant_tag, matrix_tag, forward_tag) = if slot == 1 {
            (
                DngTag::CalibrationIlluminant1,
                DngTag::ColorMatrix1,
                DngTag::ForwardMatrix1,
            )
        } else {
            (
                DngTag::CalibrationIlluminant2,
                DngTag::ColorMatrix2,
                DngTag::ForwardMatrix2,
            )
        };
        let root = dng.root_ifd_mut();
        root.add_tag(illuminant_tag, u16::from(illuminant));
        root.add_tag(matrix_tag, srationals(&calibration.color_matrix).as_slice());
        if let Some(forward) = &calibration.forward_matrix {
            root.add_tag(forward_tag, srationals(forward).as_slice());
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
    let store = |v: f32| (v.clamp(0.0, headroom) * scale).round() as u16;
    // A monochrome frame's three channels are one, and stored as one.
    let (data, cpp): (Vec<u16>, usize) = if monochrome(frame) {
        (plane(&image.data).map(store).collect(), 1)
    } else {
        (
            image.data.iter().map(|&v| store(v)).collect(),
            CameraImage::CHANNELS,
        )
    };
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
    let black = BlackLevel::new(&vec![0u16; cpp], 1, 1, cpp);
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

/// Whether `frame` is a monochrome sensor's, written as one.
fn monochrome(frame: &RawFrame) -> bool {
    frame.layout == SensorLayout::Monochrome
}

/// Interleaved RGB as the one plane its equal channels are: the
/// green, which is each of them.
fn plane(rgb: &[f32]) -> impl Iterator<Item = f32> + '_ {
    rgb.as_chunks::<3>().0.iter().map(|px| px[1])
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

/// `BaselineExposure`: the stops a reader brightens by.
fn baseline_exposure(stops: f32) -> SRational {
    const DENOMINATOR: i32 = 10_000;
    SRational::new(
        (f64::from(stops) * f64::from(DENOMINATOR)).round() as i32,
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
        // The headroom's stop is read back as the file's baseline.
        assert_eq!(back.dng.baseline_exposure, Some(1.0));
        let be = baseline_exposure(1.0);
        assert_eq!((be.n, be.d), (10_000, 10_000));
        assert_eq!(baseline_exposure(0.0).n, 0);
        let options = LinearDngOptions {
            headroom: Some(0.5),
            ..Default::default()
        };
        let mut buf = Cursor::new(Vec::new());
        assert!(write_linear_dng(&mut buf, &frame, &image, [2.0, 1.0, 4.0], &options).is_err());
    }

    /// The headroom's stops are written, with the source's own tag
    /// carried on top when it had one (a phone's DNG re-saved keeps its
    /// word); a zero is a tag and is written; a source with neither
    /// writes none.
    #[test]
    fn the_sources_baseline_is_carried_into_the_file() {
        let mut frame = flat_frame();
        frame.dng.baseline_exposure = Some(0.5);
        let image = CameraImage::from_data(4, 4, vec![0.5; 48]).unwrap();
        let write = |frame: &RawFrame, headroom| {
            let mut buf = Cursor::new(Vec::new());
            let options = LinearDngOptions {
                headroom,
                ..Default::default()
            };
            write_linear_dng(&mut buf, frame, &image, [2.0, 1.0, 4.0], &options).unwrap();
            RawlerDecoder.decode_bytes(&buf.into_inner()).unwrap()
        };
        assert_eq!(write(&frame, None).dng.baseline_exposure, Some(0.5));
        let back = write(&frame, Some(2.0));
        assert_eq!(back.dng.baseline_exposure, Some(1.5));
        let shown = |f: &RawFrame, sample: f32| sample * f.dng.baseline_exposure.unwrap().exp2();
        let stored = back.samples.to_f32()[0] / f32::from(WHITE_LEVEL);
        assert!((shown(&back, stored) - shown(&frame, 0.5)).abs() < 1e-4);
        frame.dng.baseline_exposure = Some(0.0);
        assert_eq!(write(&frame, None).dng.baseline_exposure, Some(0.0));
        frame.dng.baseline_exposure = None;
        assert_eq!(write(&frame, None).dng.baseline_exposure, None);
        assert_eq!(write(&frame, Some(2.0)).dng.baseline_exposure, Some(1.0));
    }

    /// A forward matrix written into a DNG is read back onto the
    /// calibration of its illuminant: the decoder's own reading of the
    /// tag, through a file of ours.
    #[test]
    fn a_forward_matrix_is_read_back_from_the_file() {
        let mut frame = flat_frame();
        let forward = vec![0.6, 0.2, 0.15, 0.25, 0.7, 0.05, 0.0, 0.1, 0.72];
        frame.calibrations[0].forward_matrix = Some(forward.clone());
        let image = CameraImage::from_data(4, 4, vec![0.5; 48]).unwrap();
        let mut buf = Cursor::new(Vec::new());
        write_linear_dng(
            &mut buf,
            &frame,
            &image,
            [2.0, 1.0, 4.0],
            &Default::default(),
        )
        .unwrap();
        let back = RawlerDecoder.decode_bytes(&buf.into_inner()).unwrap();
        let cal = &back.calibrations[0];
        assert_eq!(cal.illuminant, frame.calibrations[0].illuminant);
        let read = cal.forward_matrix.as_ref().expect("a forward matrix");
        for (a, b) in read.iter().zip(&forward) {
            assert!((a - b).abs() < 1e-4, "{read:?}");
        }
        // Without one in the file there is none.
        let plain = flat_frame();
        let mut buf = Cursor::new(Vec::new());
        write_linear_dng(
            &mut buf,
            &plain,
            &image,
            [2.0, 1.0, 4.0],
            &Default::default(),
        )
        .unwrap();
        let back = RawlerDecoder.decode_bytes(&buf.into_inner()).unwrap();
        assert_eq!(back.calibrations[0].forward_matrix, None);
    }

    /// `v` (not negative, under 65504) rounded to a half float, ties
    /// to even, and back.
    fn to_half(v: f32) -> f32 {
        if v < 2f32.powi(-14) {
            // A subnormal half: a whole number of 2^-24.
            return (v * 2f32.powi(24)).round_ties_even() / 2f32.powi(24);
        }
        let bits = v.to_bits();
        let odd = (bits >> 13) & 1;
        f32::from_bits((bits + 0xfff + odd) & !0x1fff)
    }

    /// A monochrome frame is written as DNG writes one, a single plane
    /// of LinearRaw with no white and no matrix, and reads back as the
    /// monochrome frame it was, its plane the developed picture's.
    #[test]
    fn a_monochrome_frame_round_trips_as_one_plane() {
        let frame = crate::develop::tests::mono_frame(16, 12, |y, x| 100 + (y * 16 + x) as u16 * 5);
        let out = demosaic(&frame, &DevelopSettings::default()).unwrap();
        for compression in [Compression::Uncompressed, Compression::Lossless] {
            let mut buf = Cursor::new(Vec::new());
            let options = LinearDngOptions {
                compression,
                ..Default::default()
            };
            write_linear_dng(&mut buf, &frame, &out.image, [1.0; 3], &options).unwrap();
            let back = RawlerDecoder.decode_bytes(&buf.into_inner()).unwrap();
            assert_eq!(back.layout, SensorLayout::Monochrome, "{compression:?}");
            assert_eq!((back.width, back.height, back.channels), (16, 12, 1));
            assert!(back.calibrations.is_empty());
            assert_eq!(back.as_shot_coefficients, Some([1.0; 3]));
            assert_eq!(back.levels.white.values, vec![65535.0]);
            let stored = back.samples.to_f32();
            for (got, px) in stored.iter().zip(out.image.pixels()) {
                assert_eq!(*got, (px[1] * 65535.0).round(), "{compression:?}");
            }
            // And develops as the source does.
            let again = develop(&back, &DevelopSettings::default()).unwrap();
            let first = develop(&frame, &DevelopSettings::default()).unwrap();
            for (a, b) in again.image.data.iter().zip(&first.image.data) {
                assert!((a - b).abs() < 1e-4);
            }
        }
        // A merge of monochrome frames is a plane of floats.
        let mut buf = Cursor::new(Vec::new());
        let tag = crate::merge::Tag {
            kind: "hdr".into(),
            frames: vec!["a.dng".into(), "b.dng".into()],
            reference: 0,
            denoised: false,
            headroom_stops: 1.0,
            lens_corrected: false,
            heads: vec![None, None],
            wholes: vec![None, None],
            unread: None,
            recipe: None,
        };
        let options = LinearDngOptions {
            headroom: Some(2.0),
            ..Default::default()
        };
        let merged = Merged {
            noise_profile: [3e-6, 2e-9],
            tag: &tag,
        };
        write_merged_dng(&mut buf, &frame, &out.image, [1.0; 3], &options, &merged).unwrap();
        let back = RawlerDecoder.decode_bytes(&buf.into_inner()).unwrap();
        assert_eq!(
            (&back.layout, back.channels),
            (&SensorLayout::Monochrome, 1)
        );
        assert_eq!(back.levels.white.values, vec![1.0]);
        for (got, px) in back.samples.to_f32().iter().zip(out.image.pixels()) {
            assert_eq!(*got, to_half(px[1] / 2.0));
        }
        // And develops as a merge: its noise is its NoiseProfile, not
        // measured, and no photosite of it is repaired.
        let prepared = crate::develop::prepare(&back, &DevelopSettings::default(), true).unwrap();
        let model = prepared.noise.expect("the file's noise").model;
        assert_eq!((model.a, model.b), ([3e-6f32; 3], [2e-9f32; 3]));
        assert!(prepared.hot_pixels.is_none());
        assert!(prepared.monochrome);
    }

    /// A merge's file: half-float samples that read back exactly, the
    /// headroom's stops, the `NoiseProfile` and our tag, the body, and
    /// the source's lens kept.
    #[test]
    fn a_merged_file_carries_its_samples_and_tags() {
        let frame = flat_frame();
        let data: Vec<f32> = (0..48).map(|i| i as f32 * 0.137 + 1e-6).collect();
        let image = CameraImage::from_data(4, 4, data.clone()).unwrap();
        let headroom = 8.0;
        let tag = crate::merge::Tag {
            kind: "hdr".into(),
            frames: vec!["a.dng".into(), "b.dng".into()],
            reference: 1,
            denoised: false,
            headroom_stops: 3.0,
            lens_corrected: true,
            heads: vec![Some("ab".repeat(32)), None],
            wholes: vec![None, None],
            unread: None,
            recipe: None,
        };
        let metadata = RawMetadata {
            exif: ::rawler::exif::Exif {
                lens_model: Some("Test 50mm".into()),
                ..Default::default()
            },
            make: "Test".into(),
            model: "Cam".into(),
            ..Default::default()
        };
        let options = LinearDngOptions {
            headroom: Some(headroom),
            metadata: Some(&metadata),
            ..Default::default()
        };
        let mut buf = Cursor::new(Vec::new());
        write_merged_dng(
            &mut buf,
            &frame,
            &image,
            [2.0, 1.0, 4.0],
            &options,
            &Merged {
                noise_profile: [3e-6, 2e-9],
                tag: &tag,
            },
        )
        .unwrap();
        let bytes = buf.into_inner();
        let back = RawlerDecoder.decode_bytes(&bytes).unwrap();
        assert_eq!(back.layout, SensorLayout::Linear);
        assert_eq!((back.make.as_str(), back.model.as_str()), ("Test", "Cam"));
        assert_eq!(back.levels.white.values, vec![1.0; 3]);
        let stored = back.samples.to_f32();
        // Exact to a half: what was stored, rounded once to 16 bits.
        for (got, want) in stored.iter().zip(&data) {
            let half = to_half(want / headroom);
            assert_eq!(*got, half, "{want}");
        }
        assert_eq!(back.dng.baseline_exposure, Some(3.0));
        let noise = back.dng.noise_profile.expect("a noise profile");
        assert!((noise.a[0] - 3e-6).abs() < 1e-12 && (noise.b[2] - 2e-9).abs() < 1e-15);
        assert_eq!(back.dng.merged.as_ref(), Some(&tag));
        assert_eq!(back.shot.lens_model.as_deref(), Some("Test 50mm"));
        // A linear DNG of ours that is no merge says neither.
        let plain = round_trip(Compression::Uncompressed);
        assert_eq!((plain.dng.noise_profile, plain.dng.merged), (None, None));
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
