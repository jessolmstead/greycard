//! The `rawler` backend.
//!
//! Only rawler's decoding is used. Its develop functions (which apply one
//! matrix regardless of illuminant and fold white balance into it) are not.

use std::path::Path;

use ::rawler::decoders::RawDecodeParams;
use ::rawler::rawimage::RawPhotometricInterpretation;
use ::rawler::rawsource::RawSource;
use ::rawler::{CFA, Orientation as RawlerOrientation, RawImage, RawImageData};

/// The EXIF and lens metadata rawler reads alongside the samples. Carried
/// as rawler's own type so it can be written straight back into a DNG; the
/// engine does not interpret it.
pub use ::rawler::decoders::RawMetadata;

use super::Decoder;
use crate::error::{Error, Result};
use crate::raw::{
    Calibration, CfaColor, CfaPattern, LevelPattern, Levels, Orientation, RawFrame, Rect, Samples,
    SensorLayout, Shot,
};

/// rawler's `Unsupported` prints all four of its fields whether or not
/// they have anything in them, so a file that is no camera's raw comes
/// out as `Error: No decoder found, model '', make: '', mode: ''`.
/// Say what happened instead, and name the camera only when there is
/// one to name.
fn decoder_error(e: ::rawler::RawlerError) -> Error {
    let ::rawler::RawlerError::Unsupported {
        what,
        model,
        make,
        mode,
    } = &e
    else {
        return Error::Decode(e.to_string());
    };
    let camera = format!("{} {}", make.trim(), model.trim());
    let camera = camera.trim();
    if camera.is_empty() {
        return Error::Unsupported(
            "no decoder recognized this file: it is not a raw this build reads".into(),
        );
    }
    let mode = if mode.is_empty() {
        String::new()
    } else {
        format!(" in {mode} mode")
    };
    Error::Unsupported(format!("{camera}{mode}: {what}"))
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RawlerDecoder;

impl Decoder for RawlerDecoder {
    fn decode_bytes(&self, bytes: &[u8]) -> Result<RawFrame> {
        Ok(decode_source(&RawSource::new_from_slice(bytes))?.0)
    }

    fn decode_path(&self, path: &Path) -> Result<RawFrame> {
        Ok(decode_source(&RawSource::new(path)?)?.0)
    }
}

impl RawlerDecoder {
    /// Decode a file and also return the metadata rawler reads from it, for
    /// passing on to an output that carries EXIF (see [`crate::dng`]).
    pub fn decode_path_with_metadata(&self, path: &Path) -> Result<(RawFrame, RawMetadata)> {
        decode_source(&RawSource::new(path)?)
    }
}

/// The camera's own rendering of a file, when the file carries one: the
/// largest preview JPEG, else the thumbnail, decoded to 8-bit sRGB, in
/// the orientation the camera stored it with, and the orientation to
/// show it in.
pub fn preview_path(path: &Path) -> Result<Option<(image::RgbImage, Orientation)>> {
    let source = RawSource::new(path)?;
    let decoder = ::rawler::get_decoder(&source).map_err(decoder_error)?;
    let params = RawDecodeParams::default();
    let image = match decoder.preview_image(&source, &params) {
        Ok(Some(i)) => Some(i),
        _ => decoder.thumbnail_image(&source, &params).ok().flatten(),
    };
    let Some(image) = image else {
        return Ok(None);
    };
    let orientation = decoder
        .raw_metadata(&source, &params)
        .ok()
        .and_then(|m| m.exif.orientation)
        .and_then(Orientation::from_exif)
        .unwrap_or_default();
    Ok(Some((image.to_rgb8(), orientation)))
}

/// Samples and metadata from one source; the frame takes what it keeps
/// of the metadata (the shot).
fn decode_source(source: &RawSource) -> Result<(RawFrame, RawMetadata)> {
    let decoder = ::rawler::get_decoder(source).map_err(decoder_error)?;
    let params = RawDecodeParams::default();
    let image = decoder
        .raw_image(source, &params, false)
        .map_err(decoder_error)?;
    let metadata = decoder
        .raw_metadata(source, &params)
        .map_err(decoder_error)?;
    let mut frame = frame_from_rawler(image)?;
    // rawler leaves the image's orientation at Normal (a TODO in its
    // `RawImage::new`); the EXIF tag has the camera's word.
    if frame.orientation == Orientation::Normal
        && let Some(o) = metadata.exif.orientation.and_then(Orientation::from_exif)
    {
        frame.orientation = o;
    }
    frame.shot = shot_of(&metadata);
    log::info!(
        "decoded {} {}: {}x{} {}, black {}, white {}",
        frame.make,
        frame.model,
        frame.width,
        frame.height,
        layout_text(&frame.layout),
        level_text(&frame.levels.black),
        level_text(&frame.levels.white)
    );
    log::debug!(
        "as shot {:?}, {} calibration(s) for illuminant(s) {:?}, crop {:?}, orientation {:?}",
        frame.as_shot_coefficients,
        frame.calibrations.len(),
        frame
            .calibrations
            .iter()
            .map(|c| c.illuminant)
            .collect::<Vec<_>>(),
        frame.crop,
        frame.orientation
    );
    // Both fail the develop, which names the cause; this is the
    // detail behind it.
    if frame.as_shot_coefficients.is_none() {
        log::debug!(
            "{} {}: no white balance in the file",
            frame.make,
            frame.model
        );
    }
    if frame.calibrations.is_empty() {
        log::debug!(
            "{} {}: no color matrix in the file",
            frame.make,
            frame.model
        );
    }
    Ok((frame, metadata))
}

/// The sensor's layout, said briefly.
fn layout_text(layout: &SensorLayout) -> String {
    match layout {
        SensorLayout::Cfa(p) => format!("CFA {p}"),
        SensorLayout::Linear => "linear".into(),
        SensorLayout::Monochrome => "monochrome".into(),
    }
}

/// A level pattern as one number when it is one, else its values.
fn level_text(p: &LevelPattern) -> String {
    match p.values.first() {
        Some(&v) if p.values.iter().all(|&x| x == v) => format!("{v}"),
        _ => format!("{:?}", p.values),
    }
}

/// What the frame was shot at, from the metadata: the EXIF tags, and
/// rawler's own lens table for a lens name when the tag is empty.
pub(crate) fn shot_of(metadata: &RawMetadata) -> Shot {
    let rational = |r: &::rawler::formats::tiff::Rational| {
        (r.d != 0)
            .then(|| r.n as f32 / r.d as f32)
            .filter(|v| v.is_finite() && *v > 0.0)
    };
    fn text(s: &Option<String>) -> Option<String> {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }
    let exif = &metadata.exif;
    Shot {
        lens_make: text(&exif.lens_make)
            .or_else(|| metadata.lens.as_ref().map(|l| l.lens_make.clone())),
        lens_model: text(&exif.lens_model)
            .or_else(|| metadata.lens.as_ref().map(|l| l.lens_name.clone())),
        focal_length: exif.focal_length.as_ref().and_then(rational),
        f_number: exif.fnumber.as_ref().and_then(rational),
        focus_distance: exif.subject_distance.as_ref().and_then(rational),
        iso: iso_of(exif),
        exposure_time: exif.exposure_time.as_ref().and_then(rational),
    }
}

/// The ISO speed, from whichever tag the camera filled. `ISOSpeedRatings`
/// is sixteen bits, so a camera past 65534 writes 65535 there and means
/// it elsewhere: in `ISOSpeed`, or, when `SensitivityType` says the
/// speed recorded is the exposure index (which the Canon, Sony and
/// Nikon samples all say), in `RecommendedExposureIndex`. Zero is a
/// camera saying nothing, in any of the three.
pub(crate) fn iso_of(exif: &::rawler::exif::Exif) -> Option<u32> {
    exif.iso_speed_ratings
        .map(u32::from)
        .filter(|&iso| iso > 0 && iso < u16::MAX as u32)
        .or(exif.iso_speed)
        .or(exif.recommended_exposure_index)
        .filter(|&iso| iso > 0)
}

/// Reshape a decoded rawler image into the engine's frame.
pub fn frame_from_rawler(image: RawImage) -> Result<RawFrame> {
    let layout = match &image.photometric {
        RawPhotometricInterpretation::Cfa(config) => SensorLayout::Cfa(cfa_pattern(&config.cfa)?),
        RawPhotometricInterpretation::LinearRaw => SensorLayout::Linear,
        RawPhotometricInterpretation::BlackIsZero => SensorLayout::Monochrome,
    };

    let black = {
        let bl = &image.blacklevel;
        LevelPattern::new(bl.width, bl.height, bl.cpp, bl.as_vec())?
    };
    let white = white_pattern(&image.whitelevel.0, image.cpp)?;

    let as_shot_coefficients = as_shot(image.wb_coeffs);

    let mut calibrations: Vec<Calibration> = image
        .color_matrix
        .iter()
        .map(|(illuminant, matrix)| Calibration {
            illuminant: *illuminant as u16,
            color_matrix: matrix.clone(),
            // rawler does not surface ForwardMatrix tags on RawImage.
            forward_matrix: None,
        })
        .collect();
    calibrations.sort_by_key(|c| c.illuminant);

    let crop = image.crop_area.or(image.active_area).map(|r| Rect {
        x: r.p.x,
        y: r.p.y,
        width: r.d.w,
        height: r.d.h,
    });

    let orientation = match image.orientation {
        RawlerOrientation::Normal | RawlerOrientation::Unknown => Orientation::Normal,
        RawlerOrientation::HorizontalFlip => Orientation::FlipHorizontal,
        RawlerOrientation::Rotate180 => Orientation::Rotate180,
        RawlerOrientation::VerticalFlip => Orientation::FlipVertical,
        RawlerOrientation::Transpose => Orientation::Transpose,
        RawlerOrientation::Rotate90 => Orientation::Rotate90,
        RawlerOrientation::Transverse => Orientation::Transverse,
        RawlerOrientation::Rotate270 => Orientation::Rotate270,
    };

    let RawImage {
        clean_make,
        clean_model,
        width,
        height,
        cpp,
        data,
        ..
    } = image;
    let samples = match data {
        RawImageData::Integer(v) => Samples::U16(v),
        RawImageData::Float(v) => Samples::F32(v),
    };

    let frame = RawFrame {
        make: clean_make,
        model: clean_model,
        width,
        height,
        channels: cpp,
        layout,
        samples,
        levels: Levels { black, white },
        as_shot_coefficients,
        calibrations,
        crop,
        orientation,
        shot: Default::default(),
    };
    frame.validate()?;
    Ok(frame)
}

fn cfa_pattern(cfa: &CFA) -> Result<CfaPattern> {
    if cfa.width == 0 || cfa.height == 0 {
        return Err(Error::Unsupported("CFA image without a pattern".into()));
    }
    let colors = (0..cfa.height)
        .flat_map(|r| (0..cfa.width).map(move |c| (r, c)))
        .map(|(r, c)| match cfa.color_at(r, c) {
            0 => CfaColor::Red,
            1 => CfaColor::Green,
            2 => CfaColor::Blue,
            other => CfaColor::Other(other.min(u8::MAX as usize) as u8),
        })
        .collect();
    CfaPattern::new(cfa.width, cfa.height, colors)
}

/// rawler stores white levels as a flat list: one value, one per component,
/// or four laid out positionally over a 2x2 Bayer cell (the reading rawler's
/// own develop path gives them).
fn white_pattern(levels: &[u32], cpp: usize) -> Result<LevelPattern> {
    let values: Vec<f32> = levels.iter().map(|&v| v as f32).collect();
    match values.len() {
        0 => Err(Error::Unsupported("no white level".into())),
        1 => Ok(LevelPattern::uniform(values[0], cpp)),
        n if n == cpp => LevelPattern::new(1, 1, cpp, values),
        4 if cpp == 1 => LevelPattern::new(2, 2, 1, values),
        n => Err(Error::Unsupported(format!(
            "{n} white levels for {cpp} samples per pixel"
        ))),
    }
}

/// rawler reports `wb_coeffs` in RGBE order and signals "absent" with NaN.
fn as_shot(wb: [f32; 4]) -> Option<[f64; 3]> {
    let [r, g, b, _] = wb;
    if [r, g, b].iter().all(|c| c.is_finite() && *c > 0.0) {
        Some([(r / g) as f64, 1.0, (b / g) as f64])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_levels_take_the_shapes_rawler_produces() {
        assert_eq!(
            white_pattern(&[4095], 1).unwrap(),
            LevelPattern::uniform(4095.0, 1)
        );
        assert_eq!(
            white_pattern(&[4095], 3).unwrap(),
            LevelPattern::uniform(4095.0, 3)
        );
        let per_component = white_pattern(&[1, 2, 3], 3).unwrap();
        assert_eq!(
            (per_component.width, per_component.height, per_component.cpp),
            (1, 1, 3)
        );
        let bayer = white_pattern(&[1, 2, 3, 4], 1).unwrap();
        assert_eq!((bayer.width, bayer.height), (2, 2));
        assert_eq!(bayer.at(1, 0, 0), 3.0);
        assert!(white_pattern(&[], 1).is_err());
        assert!(white_pattern(&[1, 2], 1).is_err());
    }

    #[test]
    fn iso_comes_from_whichever_tag_the_camera_filled() {
        use ::rawler::exif::Exif;
        let iso = |ratings, speed, index| {
            iso_of(&Exif {
                iso_speed_ratings: ratings,
                iso_speed: speed,
                recommended_exposure_index: index,
                ..Exif::default()
            })
        };
        assert_eq!(iso(Some(400), None, None), Some(400));
        // Past sixteen bits the ratings tag saturates and the speed
        // says it.
        assert_eq!(iso(Some(65535), Some(102400), None), Some(102400));
        // A zero in the first tag is not an answer either.
        assert_eq!(iso(Some(0), Some(800), None), Some(800));
        assert_eq!(iso(Some(0), None, Some(800)), Some(800));
        // SensitivityType 2: the speed recorded is the exposure index.
        assert_eq!(iso(None, None, Some(1600)), Some(1600));
        assert_eq!(iso(None, Some(200), Some(1600)), Some(200));
        // Nothing, or nothing but zeros.
        assert_eq!(iso(None, None, None), None);
        assert_eq!(iso(Some(0), Some(0), Some(0)), None);
    }

    #[test]
    fn as_shot_normalizes_to_green_and_rejects_nan() {
        assert_eq!(as_shot([2.0, 1.0, 1.5, f32::NAN]), Some([2.0, 1.0, 1.5]));
        let c = as_shot([4.0, 2.0, 3.0, 0.0]).unwrap();
        assert!((c[0] - 2.0).abs() < 1e-6 && c[1] == 1.0 && (c[2] - 1.5).abs() < 1e-6);
        assert_eq!(as_shot([f32::NAN; 4]), None);
        assert_eq!(as_shot([1.0, 0.0, 1.0, 1.0]), None);
    }

    #[test]
    fn an_unreadable_file_says_so_without_rawlers_empty_fields() {
        let none = decoder_error(::rawler::RawlerError::Unsupported {
            what: "No decoder found".into(),
            model: String::new(),
            make: String::new(),
            mode: String::new(),
        });
        let text = none.to_string();
        assert!(text.contains("not a raw this build reads"), "{text}");
        assert!(!text.contains("''"), "{text}");
        // A camera rawler knows of but cannot decode keeps its name.
        let camera = decoder_error(::rawler::RawlerError::Unsupported {
            what: "Unknown camera".into(),
            model: "EOS R9".into(),
            make: "Canon".into(),
            mode: "sraw".into(),
        })
        .to_string();
        assert!(camera.contains("Canon EOS R9"), "{camera}");
        assert!(camera.contains("in sraw mode"), "{camera}");
        // Anything else is passed through as rawler wrote it.
        let other = decoder_error(::rawler::RawlerError::DecoderFailed("truncated".into()));
        assert!(other.to_string().contains("truncated"));
    }

    #[test]
    fn bayer_pattern_maps_color_indices() {
        let cfa = CFA::new("RGGB");
        let p = cfa_pattern(&cfa).unwrap();
        assert_eq!(p, CfaPattern::rggb());
    }
}

/// What a file says about itself, read without decoding the samples:
/// enough to pick files from a folder by camera and ISO, and for a
/// library to index it by.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Probe {
    pub make: String,
    pub model: String,
    /// The lens's name as the camera wrote it, or as the decoder's
    /// lens table names it; see [`Shot::lens_model`].
    pub lens_model: Option<String>,
    /// Millimeters.
    pub focal_length: Option<f64>,
    pub iso: Option<u32>,
    /// Seconds.
    pub exposure_time: Option<f64>,
    pub fnumber: Option<f64>,
    /// When the frame was taken, as the file spells it:
    /// `DateTimeOriginal`, or `CreateDate` when only that is there,
    /// in EXIF's `YYYY:MM:DD HH:MM:SS`. Not parsed here; a consumer
    /// that sorts by it normalizes it.
    pub taken: Option<String>,
    /// The EXIF orientation tag, the file's own; `Normal` when it
    /// carries none or one this build does not know.
    pub orientation: Orientation,
}

impl Probe {
    /// The probe of a file's metadata, however that was read: a
    /// raw's through its decoder, a picture's from its EXIF chunk.
    pub(crate) fn from_metadata(metadata: &RawMetadata) -> Probe {
        let shot = shot_of(metadata);
        let exif = &metadata.exif;
        let text = |s: &Option<String>| {
            s.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        // In double precision from the rational itself, not through
        // the shot's f32: an index compares these, and f/2.8 read
        // as 2.7999999 is under f/2.8.
        let rational = |r: &Option<::rawler::formats::tiff::Rational>| {
            r.as_ref()
                .filter(|r| r.d != 0)
                .map(|r| f64::from(r.n) / f64::from(r.d))
                .filter(|v| v.is_finite() && *v > 0.0)
        };
        Probe {
            make: metadata.make.trim().to_string(),
            model: metadata.model.trim().to_string(),
            lens_model: shot.lens_model,
            focal_length: rational(&exif.focal_length),
            iso: shot.iso,
            exposure_time: rational(&exif.exposure_time),
            fnumber: rational(&exif.fnumber),
            taken: text(&exif.date_time_original).or_else(|| text(&exif.create_date)),
            orientation: exif
                .orientation
                .and_then(Orientation::from_exif)
                .unwrap_or_default(),
        }
    }
}

/// The frame's orientation tag and the size a develop of it comes
/// out at before that tag turns it, without decoding a sample.
///
/// rawler's `raw_image` takes a `dummy` flag that fills in everything
/// but the pixels, which is where the crop rectangle lives; the
/// develop's picture is that rectangle, so this is the shape a mask's
/// coordinates are in (see `mask::Turned`).
pub fn stance_path(path: &Path) -> Result<(Orientation, Option<(u32, u32)>)> {
    let source = RawSource::new(path)?;
    let decoder = ::rawler::get_decoder(&source).map_err(decoder_error)?;
    let params = RawDecodeParams::default();
    let metadata = decoder
        .raw_metadata(&source, &params)
        .map_err(decoder_error)?;
    let orientation = metadata
        .exif
        .orientation
        .and_then(Orientation::from_exif)
        .unwrap_or_default();
    // A frame whose size cannot be had this cheaply is not worth
    // decoding for: the caller has other pictures of it to measure by.
    let size = decoder
        .raw_image(&source, &params, true)
        .ok()
        .map(|image| match image.crop_area.or(image.active_area) {
            Some(r) => (r.d.w as u32, r.d.h as u32),
            None => (image.width as u32, image.height as u32),
        })
        .filter(|(w, h)| *w > 0 && *h > 0);
    Ok((orientation, size))
}

pub fn probe_path(path: &Path) -> Result<Probe> {
    let source = RawSource::new(path)?;
    let decoder = ::rawler::get_decoder(&source).map_err(decoder_error)?;
    let metadata = decoder
        .raw_metadata(&source, &RawDecodeParams::default())
        .map_err(decoder_error)?;
    Ok(Probe::from_metadata(&metadata))
}
