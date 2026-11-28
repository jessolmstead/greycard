//! Pictures that are not raws: a JPEG, PNG or TIFF taken into the
//! working space as if it had just been developed.
//!
//! Such a file is somebody's rendering already: white balanced, color
//! matrixed, tone curved and encoded. The engine cannot undo that, so
//! it does not try. It takes the samples through the file's own
//! transfer curve and primaries (an embedded matrix/TRC ICC profile
//! when there is one, sRGB otherwise) into linear
//! [`crate::color::WORKING_SPACE`], turns the picture upright by its
//! EXIF orientation, and hands it on at the point a raw's develop
//! ends. Everything after that (lens corrections, retouch, geometry,
//! the look) applies as to any working image; everything before it
//! (white balance, highlights, demosaic, the raw denoise) has nothing
//! to act on.
//!
//! A profile with lookup tables rather than a matrix (a printer's, a
//! camera's) is not read; the samples are taken as sRGB and the
//! picture says so in its [`Picture::space`].

use std::path::Path;

use ::rawler::exif::Exif;
use ::rawler::formats::tiff::reader::TiffReader;
use ::rawler::formats::tiff::{GenericTiffReader, IFD};
use ::rawler::tags::TiffCommonTag;
use image::{DynamicImage, ImageDecoder};
use rawcolor::{Cat, Chromaticity, Mat3, RgbSpace, Vec3};
use rayon::prelude::*;

use crate::color::{CAT, WORKING_SPACE, srgb_decode};
use crate::decode::{Probe, RawMetadata};
use crate::develop::orient;
use crate::error::{Error, Result};
use crate::image::WorkingImage;
use crate::raw::{Orientation, Shot};

/// A picture that is not a raw, in the working space and upright.
#[derive(Debug, Clone)]
pub struct Picture {
    /// Linear working space, the way up the file says to show it.
    pub image: WorkingImage,
    pub make: String,
    pub model: String,
    /// What the file says the picture was shot at.
    pub shot: Shot,
    /// The file's EXIF as rawler holds it, blank where the file has
    /// none, for an export to carry.
    pub metadata: RawMetadata,
    /// What the samples were taken to be in: the embedded profile's
    /// name, or sRGB and why.
    pub space: String,
    /// Bits a sample had in the file: 8, 16, or 32 for floats.
    pub bits: u8,
    /// The white of the space the samples were in: D65 for sRGB and
    /// for a file without a profile, the profile's own for one that
    /// has it (ProPhoto's is D50). The decode adapts it to the working
    /// white; a white balance offset on the picture is from it.
    pub white: Chromaticity,
}

impl Picture {
    /// The level above which the picture's samples count as clipped:
    /// the file's white, a little under, as a raw's is.
    pub fn clip_level(&self) -> f32 {
        crate::develop::CLIP_FRACTION
    }

    /// The picture's white as a temperature and tint
    /// ([`white_temp_tint`]).
    pub fn own_white(&self) -> rawcolor::TempTint {
        white_temp_tint(self.white)
    }
}

/// A picture's white as a temperature and tint: what its white
/// balance offset is from. D65 is about 6504 K and +0.0032; a white
/// the search cannot place is taken as D65.
pub fn white_temp_tint(white: Chromaticity) -> rawcolor::TempTint {
    rawcolor::cct::chromaticity_to_temp_tint(white)
        .or_else(|| rawcolor::cct::chromaticity_to_temp_tint(rawcolor::illuminant::D65))
        .unwrap_or(rawcolor::TempTint::new(6504.0, 0.0032))
}

/// The extensions taken for a picture rather than a raw.
pub const EXTENSIONS: [&str; 5] = ["jpg", "jpeg", "png", "tif", "tiff"];

/// Whether a path's extension is one of a picture's.
pub fn is_picture_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The orientation a picture will be shown in, read exactly the way
/// [`decode_picture`] reads it and without decoding a pixel.
///
/// One reader answers, so nothing can disagree with the picture on
/// screen about which way up it is: `image`'s own `orientation()`
/// and this differ on a multi-directory TIFF, and a tag written into
/// an XMP from the wrong one would point at a picture nobody sees.
pub fn orientation_path(path: &Path) -> Result<Orientation> {
    Ok(stance_path(path)?.0)
}

/// A picture's orientation tag, and its size before that tag turns it.
type Stance = (Orientation, Option<(u32, u32)>);

/// A picture's orientation tag and its size before that tag turns it,
/// both read the way [`decode_picture`] reads them and without
/// decoding a pixel. A JPEG's or a PNG's are read from its head alone
/// ([`head_stance`]), as [`probe_path`] reads its EXIF; a TIFF's
/// directories may be anywhere in it, and it is read whole.
pub fn stance_path(path: &Path) -> Result<Stance> {
    if let Some(stance) = head_stance(path)? {
        return Ok(stance);
    }
    stance_of(&crate::decode::read_file(path)?)
}

/// [`stance_path`] of a picture held whole.
fn stance_of(bytes: &[u8]) -> Result<Stance> {
    use image::ImageDecoder;
    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| Error::Decode(e.to_string()))?;
    let format = reader.format();
    let is_tiff = format == Some(image::ImageFormat::Tiff);
    let mut decoder = reader.into_decoder().map_err(decode_error)?;
    let size = decoder.dimensions();
    // A JPEG's or PNG's chunk is a bare TIFF structure; a TIFF's
    // directories are the file's own.
    let chunk = if is_tiff {
        None
    } else {
        decoder.exif_metadata().ok().flatten()
    };
    let tiff: Option<&[u8]> = if is_tiff {
        Some(bytes)
    } else {
        chunk.as_deref()
    };
    let orientation = tiff
        .and_then(read_tags)
        .map(|t| t.orientation)
        .unwrap_or_default();
    Ok((
        orientation,
        (size.0 > 0 && size.1 > 0).then_some((size.0, size.1)),
    ))
}

/// A JPEG's or a PNG's [`stance_path`] from the segments or chunks
/// before its picture data, read [`PROBE_HEAD`] bytes first and more
/// only as a segment asks: the size its frame header (a JPEG's start of
/// frame, a PNG's `IHDR`) says, which is what the decoder reports, and
/// the orientation in the EXIF the decoder finds there. None for any
/// other file, and for a head this cannot walk, which is then read
/// whole as before.
fn head_stance(path: &Path) -> Result<Option<Stance>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(PROBE_HEAD);
    file.by_ref()
        .take(PROBE_HEAD as u64)
        .read_to_end(&mut bytes)?;
    let container = match Container::sniff(&bytes) {
        Some(c @ (Container::Jpeg | Container::Png)) => c,
        _ => return Ok(None),
    };
    loop {
        match frame_head(container, &bytes) {
            Head::Done { size, exif } => {
                let orientation = exif
                    .and_then(|r| read_tags(&bytes[r]))
                    .map(|t| t.orientation)
                    .unwrap_or_default();
                let size = size.filter(|&(w, h)| w > 0 && h > 0);
                return Ok(Some((orientation, size)));
            }
            Head::NeedMore(want) => {
                let had = bytes.len();
                crate::decode::read_rest(
                    &mut file.by_ref().take(want.saturating_sub(had) as u64),
                    &mut bytes,
                )?;
                if bytes.len() == had {
                    return Ok(None);
                }
            }
            Head::Broken => return Ok(None),
        }
    }
}

/// What a walk over a picture's head to its picture data found.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Head {
    /// The head, whole: the frame's size, and where its EXIF is.
    Done {
        size: Option<(u32, u32)>,
        exif: Option<std::ops::Range<usize>>,
    },
    /// The buffer ends inside a segment; this many bytes would hold it.
    NeedMore(usize),
    /// Not a head this walks.
    Broken,
}

/// [`Head`] of a JPEG or a PNG in `buf`.
fn frame_head(container: Container, buf: &[u8]) -> Head {
    match container {
        Container::Jpeg => jpeg_head(buf),
        Container::Png => {
            // The `IHDR` is the first chunk, whole at a known place.
            if buf.len() < 24 {
                return Head::NeedMore(24);
            }
            if &buf[12..16] != b"IHDR" {
                return Head::Broken;
            }
            let be =
                |at: usize| u32::from_be_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]);
            let size = Some((be(16), be(20)));
            match png_exif(buf) {
                Scan::Found(range) => Head::Done {
                    size,
                    exif: Some(range),
                },
                Scan::None => Head::Done { size, exif: None },
                Scan::NeedMore(want) => Head::NeedMore(want),
            }
        }
        Container::Tiff => Head::Broken,
    }
}

/// A JPEG's segments from the SOI to the start of scan, as
/// [`jpeg_exif`] walks them: the first EXIF, and the size in the start
/// of frame.
fn jpeg_head(buf: &[u8]) -> Head {
    const EXIF: &[u8] = b"Exif\0\0";
    let mut pos = 2;
    let mut size = None;
    let mut exif = None;
    loop {
        while pos < buf.len() && buf[pos] == 0xFF {
            pos += 1;
        }
        if pos >= buf.len() {
            return Head::NeedMore(pos + 4);
        }
        let marker = buf[pos];
        pos += 1;
        match marker {
            0xDA | 0xD9 => return Head::Done { size, exif },
            0xD0..=0xD7 | 0x01 => continue,
            _ => {}
        }
        if pos + 2 > buf.len() {
            return Head::NeedMore(pos + 2);
        }
        let len = u16::from_be_bytes([buf[pos], buf[pos + 1]]) as usize;
        if len < 2 {
            return Head::Broken;
        }
        let payload = pos + 2..pos + len;
        if payload.end > buf.len() {
            return Head::NeedMore(payload.end);
        }
        let body = &buf[payload.clone()];
        match marker {
            0xE1 if exif.is_none() && body.starts_with(EXIF) => {
                exif = Some(payload.start + EXIF.len()..payload.end);
            }
            // A start of frame (not a Huffman or arithmetic table, nor
            // JPG): precision, height, width.
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) && size.is_none() => {
                if body.len() < 5 {
                    return Head::Broken;
                }
                let h = u32::from(u16::from_be_bytes([body[1], body[2]]));
                let w = u32::from(u16::from_be_bytes([body[3], body[4]]));
                size = Some((w, h));
            }
            _ => {}
        }
        pos = payload.end;
    }
}

/// A picture's camera, lens, exposure and date from its EXIF chunk,
/// read exactly the way [`decode_picture`] reads it and without
/// decoding a pixel; the raw's counterpart is
/// [`crate::decode::probe_path`]. A picture with no EXIF probes as
/// a file that says nothing: an empty make and model, the rest
/// `None`.
///
/// Only the head of the file is read: a JPEG's APP1 segment and a
/// PNG's `eXIf` chunk come before the picture data, so the read
/// starts at [`PROBE_HEAD`] bytes and grows only when the EXIF is
/// not yet whole within it. A file cut off after an intact EXIF
/// block still probes. A PNG that put its `eXIf` after the picture
/// data (the standard allows it; nothing writes it so) is not read
/// past the first `IDAT`, and probes as a file that says nothing. A
/// TIFF's directories can be anywhere in the file, so a TIFF is
/// read whole.
pub fn probe_path(path: &Path) -> Result<Probe> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(PROBE_HEAD);
    file.by_ref()
        .take(PROBE_HEAD as u64)
        .read_to_end(&mut bytes)?;
    let tiff: Option<Vec<u8>> = match Container::sniff(&bytes) {
        Some(Container::Tiff) => {
            crate::decode::read_rest(&mut file, &mut bytes)?;
            Some(bytes)
        }
        Some(container) => loop {
            match container.exif(&bytes) {
                Scan::Found(range) => break Some(bytes[range].to_vec()),
                Scan::None => break None,
                Scan::NeedMore(want) => {
                    let had = bytes.len();
                    crate::decode::read_rest(
                        &mut file.by_ref().take((want - had) as u64),
                        &mut bytes,
                    )?;
                    if bytes.len() == had {
                        // The file ends inside a segment: whatever
                        // EXIF it had is not whole.
                        break None;
                    }
                }
            }
        },
        None => {
            return Err(Error::Decode(format!(
                "{} is not a JPEG, PNG or TIFF",
                path.display()
            )));
        }
    };
    Ok(tiff
        .as_deref()
        .and_then(read_tags)
        .map(|t| Probe::from_metadata(&t.metadata))
        .unwrap_or_default())
}

/// How much of a picture [`probe_path`] reads first.
pub const PROBE_HEAD: usize = 128 * 1024;

/// The containers the probe walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Jpeg,
    Png,
    Tiff,
}

/// What a walk over a file's head found.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Scan {
    /// The EXIF's TIFF structure, at this range of the buffer.
    Found(std::ops::Range<usize>),
    /// The segments before the picture data hold no EXIF.
    None,
    /// The buffer ends inside a segment; this many bytes would hold
    /// it.
    NeedMore(usize),
}

impl Container {
    fn sniff(head: &[u8]) -> Option<Container> {
        if head.starts_with(&[0xFF, 0xD8]) {
            Some(Container::Jpeg)
        } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Container::Png)
        } else if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
            Some(Container::Tiff)
        } else {
            None
        }
    }

    fn exif(self, buf: &[u8]) -> Scan {
        match self {
            Container::Jpeg => jpeg_exif(buf),
            Container::Png => png_exif(buf),
            Container::Tiff => Scan::Found(0..buf.len()),
        }
    }
}

/// Walk a JPEG's segments from the SOI to the start of scan: an
/// APP1 whose payload begins `Exif\0\0` carries the TIFF structure.
fn jpeg_exif(buf: &[u8]) -> Scan {
    const EXIF: &[u8] = b"Exif\0\0";
    let mut pos = 2;
    loop {
        // Padding between segments is allowed to be any run of FF.
        while pos < buf.len() && buf[pos] == 0xFF {
            pos += 1;
        }
        if pos >= buf.len() {
            return Scan::NeedMore(pos + 4);
        }
        let marker = buf[pos];
        pos += 1;
        match marker {
            // Start of scan: the picture data, and no more segments
            // before it. End of image likewise.
            0xDA | 0xD9 => return Scan::None,
            // Restart markers and TEM carry no length.
            0xD0..=0xD7 | 0x01 => continue,
            _ => {}
        }
        if pos + 2 > buf.len() {
            return Scan::NeedMore(pos + 2);
        }
        let len = u16::from_be_bytes([buf[pos], buf[pos + 1]]) as usize;
        if len < 2 {
            return Scan::None;
        }
        let payload = pos + 2..pos + len;
        if payload.end > buf.len() {
            return Scan::NeedMore(payload.end);
        }
        if marker == 0xE1 && buf[payload.clone()].starts_with(EXIF) {
            return Scan::Found(payload.start + EXIF.len()..payload.end);
        }
        pos = payload.end;
    }
}

/// Walk a PNG's chunks from the signature to the first `IDAT`: an
/// `eXIf` chunk's data is the TIFF structure.
fn png_exif(buf: &[u8]) -> Scan {
    let mut pos = 8;
    loop {
        if pos + 8 > buf.len() {
            return Scan::NeedMore(pos + 8);
        }
        let len = u32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
        let kind = &buf[pos + 4..pos + 8];
        let data = pos + 8..pos + 8 + len;
        match kind {
            b"IDAT" | b"IEND" => return Scan::None,
            b"eXIf" => {
                if data.end > buf.len() {
                    return Scan::NeedMore(data.end);
                }
                return Scan::Found(data);
            }
            _ => {}
        }
        // The chunk's data and its CRC.
        pos = data.end + 4;
    }
}

/// Read a JPEG, PNG or TIFF into the working space.
pub fn decode_picture_path(path: &Path) -> Result<Picture> {
    let bytes = std::fs::read(path)?;
    decode_picture(&bytes)
}

/// Read a JPEG, PNG or TIFF held in memory into the working space.
pub fn decode_picture(bytes: &[u8]) -> Result<Picture> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| Error::Decode(e.to_string()))?;
    let format = reader.format();
    let mut decoder = reader.into_decoder().map_err(decode_error)?;
    let icc = decoder.icc_profile().ok().flatten();
    // The EXIF: a JPEG's or PNG's chunk is a bare TIFF structure; a
    // TIFF's directories are the file's own.
    let exif_bytes = match format {
        Some(image::ImageFormat::Tiff) => Some(bytes.to_vec()),
        _ => decoder.exif_metadata().ok().flatten(),
    };
    let decoded = DynamicImage::from_decoder(decoder).map_err(decode_error)?;

    let tags = exif_bytes.as_deref().and_then(read_tags);
    let (metadata, orientation, make, model) = match tags {
        Some(t) => (t.metadata, t.orientation, t.make, t.model),
        None => (
            blank_metadata(),
            Orientation::Normal,
            String::new(),
            String::new(),
        ),
    };
    let shot = crate::decode::rawler_backend::shot_of(&metadata);

    let (transform, space) = transform_of(icc.as_deref())?;
    let (image, bits) = to_working(decoded, &transform)?;
    let image = orient(image, orientation);
    Ok(Picture {
        image,
        make,
        model,
        shot,
        metadata,
        space,
        bits,
        white: transform.white,
    })
}

/// The transform the samples of a file with this embedded profile go
/// through, and what that says of their space: the profile's own for
/// a matrix/TRC one, sRGB for none or one that is not.
fn transform_of(icc: Option<&[u8]>) -> Result<(Transform, String)> {
    Ok(match icc.map(MatrixTrc::parse) {
        Some(Some(profile)) => {
            let name = profile
                .description
                .clone()
                .unwrap_or_else(|| "embedded profile".into());
            (Transform::from_profile(&profile)?, name)
        }
        Some(None) => (
            Transform::srgb()?,
            "sRGB (the embedded profile is not a matrix one)".into(),
        ),
        None => (Transform::srgb()?, "sRGB (no profile)".into()),
    })
}

/// A picture file's samples taken to sRGB's, encoded: what
/// [`decode_picture`] makes of them in the working space, brought on to
/// sRGB a pixel at a time, for a small picture of the file that has to
/// agree with its develop (a thumbnail, the culling loupe's) without
/// the working image whole. Out of sRGB's gamut a channel is clamped,
/// as a raw's thumbnail's is.
#[derive(Debug, Clone)]
pub struct ToSrgb {
    trc: [Trc; 3],
    /// Rows, the file's linear RGB to sRGB's.
    matrix: [[f32; 3]; 3],
    srgb: bool,
}

impl ToSrgb {
    /// For a file that embeds `icc` (`ImageDecoder::icc_profile`), or
    /// none. The profile is read as [`decode_picture`] reads it.
    pub fn of_profile(icc: Option<&[u8]>) -> Result<Self> {
        let (t, _) = transform_of(icc)?;
        let to_srgb = rows(
            &WORKING_SPACE
                .to_space_matrix(&RgbSpace::SRGB, CAT)
                .ok_or(Error::NoCalibration)?,
        );
        let m = t.matrix;
        let matrix = std::array::from_fn(|i| {
            std::array::from_fn(|j| (0..3).map(|k| to_srgb[i][k] * m[k][j]).sum())
        });
        let srgb = icc.is_none() || samples_are_srgb(&t.trc, &matrix);
        Ok(Self {
            trc: t.trc,
            matrix,
            srgb,
        })
    }

    /// Whether the file's encoded samples are sRGB's as they stand, to
    /// within half an 8-bit level: no profile, one [`decode_picture`]
    /// does not read, or one of sRGB's. A caller keeps such samples as
    /// they are and pays nothing for the profile.
    pub fn is_srgb(&self) -> bool {
        self.srgb
    }

    /// Encoded samples, 0 to 1, to sRGB's.
    pub fn encoded(&self, rgb: [f32; 3]) -> [f32; 3] {
        self.linear([0, 1, 2].map(|i| self.trc[i].linear(rgb[i])))
    }

    /// Linear samples to sRGB's, encoded: a float file's, which
    /// [`decode_picture`] takes as linear whatever its curve.
    pub fn linear(&self, rgb: [f32; 3]) -> [f32; 3] {
        let m = self.matrix;
        [0, 1, 2].map(|i| {
            let v = m[i][0] * rgb[0] + m[i][1] * rgb[1] + m[i][2] * rgb[2];
            crate::color::srgb_encode(v.clamp(0.0, 1.0))
        })
    }

    /// A decoded picture whole as 8-bit sRGB: `to_rgb8` itself when the
    /// samples are sRGB's already, else each through a table of its
    /// depth's curve, the matrix, and a table of sRGB's.
    pub fn rgb8(&self, decoded: &DynamicImage) -> image::RgbImage {
        let float = is_float(decoded);
        if self.srgb && !float {
            return decoded.to_rgb8();
        }
        let (w, h) = (decoded.width(), decoded.height());
        // Linear to encoded in 16-bit steps: a twentieth of a level at
        // black, where sRGB's curve is steepest. Made once a process.
        static ENCODE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        let encode = ENCODE.get_or_init(|| {
            (0..=u16::MAX)
                .map(|i| (crate::color::srgb_encode(i as f32 / 65535.0) * 255.0).round() as u8)
                .collect()
        });
        let m = self.matrix;
        let out = |lin: [f32; 3]| -> [u8; 3] {
            [0, 1, 2].map(|i| {
                let v = m[i][0] * lin[0] + m[i][1] * lin[1] + m[i][2] * lin[2];
                encode[(v.clamp(0.0, 1.0) * 65535.0).round() as usize]
            })
        };
        let mut rgb = vec![0u8; w as usize * h as usize * 3];
        if float {
            let px = decoded.to_rgb32f();
            rgb.par_chunks_mut(3)
                .zip(px.as_raw().par_chunks(3))
                .for_each(|(o, p)| o.copy_from_slice(&out([p[0], p[1], p[2]])));
        } else if is_sixteen(decoded) {
            let px = decoded.to_rgb16();
            let luts: [Vec<f32>; 3] = std::array::from_fn(|i| self.trc[i].lut(16));
            rgb.par_chunks_mut(3)
                .zip(px.as_raw().par_chunks(3))
                .for_each(|(o, p)| {
                    o.copy_from_slice(&out([0, 1, 2].map(|i| luts[i][p[i] as usize])));
                });
        } else {
            let px = decoded.to_rgb8();
            let luts: [Vec<f32>; 3] = std::array::from_fn(|i| self.trc[i].lut(8));
            rgb.par_chunks_mut(3)
                .zip(px.as_raw().par_chunks(3))
                .for_each(|(o, p)| {
                    o.copy_from_slice(&out([0, 1, 2].map(|i| luts[i][p[i] as usize])));
                });
        }
        image::RgbImage::from_raw(w, h, rgb).expect("three samples a pixel")
    }
}

/// Whether a decoded picture's samples are floats, which
/// [`decode_picture`] takes as linear.
pub fn is_float(decoded: &DynamicImage) -> bool {
    matches!(
        decoded,
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_)
    )
}

/// Whether a decoded picture's samples are 16 bits, which
/// [`decode_picture`] reads at that depth.
pub fn is_sixteen(decoded: &DynamicImage) -> bool {
    matches!(
        decoded,
        DynamicImage::ImageLuma16(_)
            | DynamicImage::ImageLumaA16(_)
            | DynamicImage::ImageRgb16(_)
            | DynamicImage::ImageRgba16(_)
    )
}

/// How far, in 8-bit levels, a curve may land from sRGB's for the
/// samples to be sRGB's as they stand.
const CURVE_LEVELS: f32 = 0.5;

/// How far, in 8-bit levels, the matrix may take a corner of the cube
/// from itself for the samples to be sRGB's as they stand. Looser than
/// the curves': the common sRGB profiles' colorants, D50 adapted and
/// rounded to the profile's fixed point, come back from the working
/// space's own D65 a little off at the steep foot of sRGB's curve (the
/// HP profile Lightroom, Photoshop, and Capture One embed by 0.55 of a
/// level), and a wider space is hundreds of levels off.
const CORNER_LEVELS: f32 = 2.0;

/// Whether samples through these curves and this matrix to sRGB come
/// back where they were ([`CURVE_LEVELS`], [`CORNER_LEVELS`]).
fn samples_are_srgb(trc: &[Trc; 3], m: &[[f32; 3]; 3]) -> bool {
    curve_levels(trc) < CURVE_LEVELS && corner_levels(m) < CORNER_LEVELS
}

/// The furthest the curves land from sRGB's, in 8-bit levels, over
/// every 8-bit level.
fn curve_levels(trc: &[Trc; 3]) -> f32 {
    use crate::color::srgb_encode;
    trc.iter()
        .flat_map(|c| {
            (0..=255).map(move |i| {
                let x = i as f32 / 255.0;
                (srgb_encode(c.linear(x)) - x).abs() * 255.0
            })
        })
        .fold(0.0, f32::max)
}

/// The furthest the matrix takes a corner of the cube from itself, in
/// 8-bit levels of sRGB's curve: there one channel's leak into another
/// meets the curve's steep foot. Unclamped: a wider space's primary
/// lands outside sRGB's cube, and a clamp would put it back on the
/// corner it came from.
fn corner_levels(m: &[[f32; 3]; 3]) -> f32 {
    use crate::color::srgb_encode;
    (0..8usize)
        .flat_map(|k| {
            let c = [k & 1, (k >> 1) & 1, (k >> 2) & 1].map(|b| b as f32);
            (0..3).map(move |i| {
                let v = m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2];
                (srgb_encode(v) - c[i]).abs() * 255.0
            })
        })
        .fold(0.0, f32::max)
}

fn decode_error(e: image::ImageError) -> Error {
    Error::Decode(e.to_string())
}

/// What the EXIF tells the picture.
struct Tags {
    metadata: RawMetadata,
    orientation: Orientation,
    make: String,
    model: String,
}

/// The EXIF from a TIFF structure, as rawler reads a raw's.
fn read_tags(tiff: &[u8]) -> Option<Tags> {
    let reader = GenericTiffReader::new_with_buffer(tiff, 0, 0, None).ok()?;
    let root: &IFD = reader.root_ifd();
    let exif = Exif::new(root).ok()?;
    let text = |tag: TiffCommonTag| {
        root.get_entry(tag)
            .and_then(|e| e.value.as_string().cloned())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let make = text(TiffCommonTag::Make);
    let model = text(TiffCommonTag::Model);
    let orientation = exif
        .orientation
        .and_then(Orientation::from_exif)
        .unwrap_or_default();
    Some(Tags {
        metadata: RawMetadata {
            exif,
            model: model.clone(),
            make: make.clone(),
            lens: None,
            unique_image_id: None,
            rating: None,
        },
        orientation,
        make,
        model,
    })
}

fn blank_metadata() -> RawMetadata {
    RawMetadata {
        exif: Exif::default(),
        model: String::new(),
        make: String::new(),
        lens: None,
        unique_image_id: None,
        rating: None,
    }
}

/// Encoded samples to the working space: a curve per channel, then a
/// matrix.
struct Transform {
    trc: [Trc; 3],
    /// Rows, the file's linear RGB to the working space's.
    matrix: [[f32; 3]; 3],
    /// The white of the space the samples were in.
    white: Chromaticity,
}

impl Transform {
    fn srgb() -> Result<Self> {
        Ok(Self {
            trc: [Trc::Srgb, Trc::Srgb, Trc::Srgb],
            matrix: rows(&space_matrix(&RgbSpace::SRGB)?),
            white: RgbSpace::SRGB.white,
        })
    }

    /// The profile's curves and its colorants, which the profile
    /// gives relative to D50 (the connection space), adapted to the
    /// working space's white.
    fn from_profile(p: &MatrixTrc) -> Result<Self> {
        let to_xyz = Mat3::from_cols(Vec3(p.red), Vec3(p.green), Vec3(p.blue));
        let d50 = Chromaticity::from_xyz(Vec3(PCS_WHITE)).ok_or(Error::NoCalibration)?;
        let adapt = rawcolor::cat::adaptation_matrix(d50, WORKING_SPACE.white, Cat::Bradford)
            .ok_or(Error::NoCalibration)?;
        let from_xyz = WORKING_SPACE
            .from_xyz_matrix()
            .ok_or(Error::NoCalibration)?;
        Ok(Self {
            trc: p.trc.clone(),
            matrix: rows(&(from_xyz * adapt * to_xyz)),
            white: p.white_point(),
        })
    }
}

/// The ICC connection space's white, D50 as the specification fixes it.
const PCS_WHITE: [f64; 3] = [0.9642, 1.0, 0.8249];

fn space_matrix(from: &RgbSpace) -> Result<Mat3> {
    from.to_space_matrix(&WORKING_SPACE, CAT)
        .ok_or(Error::NoCalibration)
}

fn rows(m: &Mat3) -> [[f32; 3]; 3] {
    m.rows.map(|r| r.map(|v| v as f32))
}

/// A channel's transfer curve, encoded to linear.
#[derive(Debug, Clone, PartialEq)]
enum Trc {
    Srgb,
    Identity,
    Gamma(f32),
    /// Evenly spaced over 0..1, linear between.
    Table(Vec<f32>),
    /// ICC's parametric curve: `(a x + b)^g + e` above `d`, `c x + f`
    /// below.
    Para {
        g: f32,
        a: f32,
        b: f32,
        c: f32,
        d: f32,
        e: f32,
        f: f32,
    },
}

impl Trc {
    fn linear(&self, x: f32) -> f32 {
        match self {
            Trc::Srgb => srgb_decode(x),
            Trc::Identity => x,
            Trc::Gamma(g) => x.max(0.0).powf(*g),
            Trc::Table(t) => {
                let n = t.len();
                if n < 2 {
                    return x;
                }
                let p = x.clamp(0.0, 1.0) * (n - 1) as f32;
                let i = (p.floor() as usize).min(n - 2);
                let f = p - i as f32;
                t[i] + (t[i + 1] - t[i]) * f
            }
            Trc::Para {
                g,
                a,
                b,
                c,
                d,
                e,
                f,
            } => {
                if x >= *d {
                    (a * x + b).max(0.0).powf(*g) + e
                } else {
                    c * x + f
                }
            }
        }
    }

    /// The curve tabulated for `bits`-bit samples.
    fn lut(&self, bits: u32) -> Vec<f32> {
        let n = 1usize << bits;
        (0..n)
            .map(|i| self.linear(i as f32 / (n - 1) as f32))
            .collect()
    }
}

/// The decoded samples through the transform, and their depth.
fn to_working(decoded: DynamicImage, t: &Transform) -> Result<(WorkingImage, u8)> {
    let (w, h) = (decoded.width() as usize, decoded.height() as usize);
    if w == 0 || h == 0 {
        return Err(Error::Unsupported("an empty picture".into()));
    }
    let m = t.matrix;
    let matrixed = |rgb: [f32; 3]| -> [f32; 3] {
        [0, 1, 2].map(|i| m[i][0] * rgb[0] + m[i][1] * rgb[1] + m[i][2] * rgb[2])
    };
    let mut out = WorkingImage::new(w, h);
    let bits = match &decoded {
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => {
            // Floats are linear already; the curve is a name only.
            let px = decoded.to_rgb32f();
            out.data
                .par_chunks_mut(3)
                .zip(px.as_raw().par_chunks(3))
                .for_each(|(o, p)| {
                    let rgb = [p[0], p[1], p[2]].map(crate::image::admit);
                    o.copy_from_slice(&matrixed(rgb).map(crate::image::admit));
                });
            32
        }
        DynamicImage::ImageLuma16(_)
        | DynamicImage::ImageLumaA16(_)
        | DynamicImage::ImageRgb16(_)
        | DynamicImage::ImageRgba16(_) => {
            let px = decoded.to_rgb16();
            let luts: [Vec<f32>; 3] = [t.trc[0].lut(16), t.trc[1].lut(16), t.trc[2].lut(16)];
            out.data
                .par_chunks_mut(3)
                .zip(px.as_raw().par_chunks(3))
                .for_each(|(o, p)| {
                    let lin = [
                        luts[0][p[0] as usize],
                        luts[1][p[1] as usize],
                        luts[2][p[2] as usize],
                    ];
                    o.copy_from_slice(&matrixed(lin));
                });
            16
        }
        _ => {
            let px = decoded.to_rgb8();
            let luts: [Vec<f32>; 3] = [t.trc[0].lut(8), t.trc[1].lut(8), t.trc[2].lut(8)];
            out.data
                .par_chunks_mut(3)
                .zip(px.as_raw().par_chunks(3))
                .for_each(|(o, p)| {
                    let lin = [
                        luts[0][p[0] as usize],
                        luts[1][p[1] as usize],
                        luts[2][p[2] as usize],
                    ];
                    o.copy_from_slice(&matrixed(lin));
                });
            8
        }
    };
    Ok((out, bits))
}

/// What a matrix/TRC ICC profile says: the colorants relative to D50
/// and a curve per channel, with its description when it has one.
#[derive(Debug, Clone, PartialEq)]
pub struct MatrixTrc {
    pub red: [f64; 3],
    pub green: [f64; 3],
    pub blue: [f64; 3],
    trc: [Trc; 3],
    pub description: Option<String>,
    /// The media white, XYZ: what the profile's chromatic adaptation
    /// (`chad`) took to D50, or its `wtpt` without one; none when it
    /// has neither.
    pub white: Option<[f64; 3]>,
}

impl MatrixTrc {
    /// The profile's matrix and curves; none for a profile that is
    /// not RGB to XYZ by matrix, or that cannot be read.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 132 || &bytes[16..20] != b"RGB " || &bytes[20..24] != b"XYZ " {
            return None;
        }
        let u32_at = |at: usize| -> Option<u32> {
            bytes
                .get(at..at + 4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        let count = u32_at(128)? as usize;
        let mut tags = std::collections::HashMap::new();
        for i in 0..count {
            let at = 132 + i * 12;
            let sig = bytes.get(at..at + 4)?;
            let offset = u32_at(at + 4)? as usize;
            let size = u32_at(at + 8)? as usize;
            let data = bytes.get(offset..offset.checked_add(size)?)?;
            tags.insert([sig[0], sig[1], sig[2], sig[3]], data);
        }
        let xyz = |sig: &[u8; 4]| -> Option<[f64; 3]> {
            let d = tags.get(sig)?;
            if d.len() < 20 || &d[0..4] != b"XYZ " {
                return None;
            }
            Some([8, 12, 16].map(|at| s15f16(&d[at..at + 4])))
        };
        let curve = |sig: &[u8; 4]| -> Option<Trc> { parse_curve(tags.get(sig)?) };
        Some(Self {
            red: xyz(b"rXYZ")?,
            green: xyz(b"gXYZ")?,
            blue: xyz(b"bXYZ")?,
            trc: [curve(b"rTRC")?, curve(b"gTRC")?, curve(b"bTRC")?],
            description: tags.get(b"desc").and_then(|d| parse_description(d)),
            white: media_white(tags.get(b"chad").copied(), xyz(b"wtpt")),
        })
    }

    /// The white of the profile's space as a chromaticity: its media
    /// white, or the connection space's D50 when it names none. A white
    /// no light could be (a channel at nothing or below, a sum that
    /// names no chromaticity) is D50 too, said in the log, so a profile
    /// with a broken `wtpt` opens as it did before the white was read.
    pub fn white_point(&self) -> Chromaticity {
        let d50 = || Chromaticity::from_xyz(Vec3(PCS_WHITE)).unwrap_or(rawcolor::illuminant::D50);
        let Some(w) = self.white else {
            return d50();
        };
        let usable = w.iter().all(|v| v.is_finite() && *v > 0.0);
        match Chromaticity::from_xyz(Vec3(w)).filter(|_| usable) {
            Some(c) => c,
            None => {
                log::warn!(
                    "{}: its profile's white {w:?} is no light; taken as D50",
                    self.description.as_deref().unwrap_or("embedded profile")
                );
                d50()
            }
        }
    }
}

/// A profile's media white. A version 4 profile's `wtpt` is D50 by
/// rule, the colorants adapted to it, and its `chad` the adaptation
/// that did it, so the white is `chad` undone on D50 (Display P3's
/// comes back D65); a version 2 profile's `wtpt` is the white itself.
fn media_white(chad: Option<&[u8]>, wtpt: Option<[f64; 3]>) -> Option<[f64; 3]> {
    let adapted = chad.and_then(|d| {
        if d.len() < 44 || &d[0..4] != b"sf32" {
            return None;
        }
        let v: [f64; 9] = std::array::from_fn(|i| s15f16(&d[8 + 4 * i..12 + 4 * i]));
        let m = Mat3::new([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]]);
        Some((m.inverse()? * Vec3(PCS_WHITE)).0)
    });
    adapted.or(wtpt)
}

fn s15f16(b: &[u8]) -> f64 {
    i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64 / 65536.0
}

fn parse_curve(d: &[u8]) -> Option<Trc> {
    let u32_at = |at: usize| -> Option<u32> {
        d.get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    match d.get(0..4)? {
        b"curv" => {
            let n = u32_at(8)? as usize;
            match n {
                0 => Some(Trc::Identity),
                1 => {
                    let g = d.get(12..14)?;
                    Some(Trc::Gamma(u16::from_be_bytes([g[0], g[1]]) as f32 / 256.0))
                }
                _ => {
                    let table = d.get(12..12 + 2 * n)?;
                    Some(Trc::Table(
                        table
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|b| u16::from_be_bytes(*b) as f32 / 65535.0)
                            .collect(),
                    ))
                }
            }
        }
        b"para" => {
            let kind = d.get(8..10).map(|b| u16::from_be_bytes([b[0], b[1]]))?;
            let wanted = match kind {
                0 => 1,
                1 => 3,
                2 => 4,
                3 => 5,
                4 => 7,
                _ => return None,
            };
            let p: Vec<f32> = (0..wanted)
                .map(|i| d.get(12 + i * 4..16 + i * 4).map(|b| s15f16(b) as f32))
                .collect::<Option<_>>()?;
            let at = |i: usize| p.get(i).copied().unwrap_or(0.0);
            // The forms without a break are the general one with the
            // break at zero and nothing below it.
            Some(match kind {
                0 => Trc::Gamma(at(0)),
                1 => Trc::Para {
                    g: at(0),
                    a: at(1),
                    b: at(2),
                    c: 0.0,
                    d: -at(2) / at(1),
                    e: 0.0,
                    f: 0.0,
                },
                2 => Trc::Para {
                    g: at(0),
                    a: at(1),
                    b: at(2),
                    c: 0.0,
                    d: -at(2) / at(1),
                    e: at(3),
                    f: at(3),
                },
                3 => Trc::Para {
                    g: at(0),
                    a: at(1),
                    b: at(2),
                    c: at(3),
                    d: at(4),
                    e: 0.0,
                    f: 0.0,
                },
                _ => Trc::Para {
                    g: at(0),
                    a: at(1),
                    b: at(2),
                    c: at(3),
                    d: at(4),
                    e: at(5),
                    f: at(6),
                },
            })
        }
        _ => None,
    }
}

/// A profile's name from its `desc` tag: version 2's ASCII, or
/// version 4's first unicode record.
fn parse_description(d: &[u8]) -> Option<String> {
    let u32_at = |at: usize| -> Option<u32> {
        d.get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let s = match d.get(0..4)? {
        b"desc" => {
            let n = u32_at(8)? as usize;
            String::from_utf8_lossy(d.get(12..12 + n)?).into_owned()
        }
        b"mluc" => {
            // The first record: language, country, then its length
            // and offset from the tag's start.
            let len = u32_at(20)? as usize;
            let offset = u32_at(24)? as usize;
            let units: Vec<u16> = d
                .get(offset..offset + len)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_be_bytes(*b))
                .collect();
            String::from_utf16_lossy(&units)
        }
        _ => return None,
    };
    let s = s.trim_end_matches('\0').trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::rawler::formats::tiff::{DirectoryWriter, Rational, TiffWriter};
    use ::rawler::tags::ExifTag;
    use image::ImageEncoder;
    use std::io::Cursor;

    /// A float picture's NaN comes in as zero and its infinity as the
    /// half float's largest, each channel on its own, before the matrix
    /// mixes it into the others; a channel under zero is carried.
    #[test]
    fn a_float_pictures_samples_come_in_finite() {
        let px = [f32::NAN, 0.2, 0.3, f32::INFINITY, 0.2, 0.3, -0.1, 0.2, 0.3];
        let decoded =
            DynamicImage::ImageRgb32F(image::Rgb32FImage::from_raw(3, 1, px.to_vec()).unwrap());
        let t = Transform::srgb().unwrap();
        let (image, bits) = to_working(decoded, &t).unwrap();
        assert_eq!(bits, 32);
        assert!(image.data.iter().all(|v| v.is_finite()), "{:?}", image.data);
        let m = t.matrix;
        let want = |rgb: [f32; 3]| {
            [0, 1, 2].map(|i| {
                crate::image::admit(m[i][0] * rgb[0] + m[i][1] * rgb[1] + m[i][2] * rgb[2])
            })
        };
        let limit = crate::image::SAMPLE_LIMIT;
        assert_eq!(image.pixel(0, 0), want([0.0, 0.2, 0.3]));
        assert_eq!(image.pixel(1, 0), want([limit, 0.2, 0.3]));
        assert_eq!(image.pixel(2, 0), want([-0.1, 0.2, 0.3]));
    }

    /// A matrix/TRC profile built by hand: the colorants given, a
    /// parametric sRGB curve, and a version 2 description.
    fn profile(red: [f64; 3], green: [f64; 3], blue: [f64; 3], name: &str) -> Vec<u8> {
        profile_with(red, green, blue, name, Vec::new())
    }

    /// An ICC `XYZ ` tag's data.
    fn xyz_tag(v: [f64; 3]) -> Vec<u8> {
        let mut d = b"XYZ \0\0\0\0".to_vec();
        for c in v {
            d.extend(((c * 65536.0).round() as i32).to_be_bytes());
        }
        d
    }

    /// [`profile`] with more tags.
    fn profile_with(
        red: [f64; 3],
        green: [f64; 3],
        blue: [f64; 3],
        name: &str,
        extra: Vec<([u8; 4], Vec<u8>)>,
    ) -> Vec<u8> {
        let mut tags: Vec<([u8; 4], Vec<u8>)> = extra;
        let xyz = xyz_tag;
        tags.push((*b"rXYZ", xyz(red)));
        tags.push((*b"gXYZ", xyz(green)));
        tags.push((*b"bXYZ", xyz(blue)));
        let mut para = b"para\0\0\0\0\0\x03\0\0".to_vec();
        for p in [2.4f64, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045] {
            para.extend(((p * 65536.0).round() as i32).to_be_bytes());
        }
        for sig in [b"rTRC", b"gTRC", b"bTRC"] {
            if !tags.iter().any(|(s, _)| s == sig) {
                tags.push((*sig, para.clone()));
            }
        }
        let mut desc = b"desc\0\0\0\0".to_vec();
        desc.extend((name.len() as u32 + 1).to_be_bytes());
        desc.extend(name.as_bytes());
        desc.push(0);
        tags.push((*b"desc", desc));

        let mut out = vec![0u8; 128];
        out[16..20].copy_from_slice(b"RGB ");
        out[20..24].copy_from_slice(b"XYZ ");
        out.extend((tags.len() as u32).to_be_bytes());
        let mut offset = 132 + tags.len() * 12;
        let mut table: Vec<u8> = Vec::new();
        let mut data: Vec<u8> = Vec::new();
        for (sig, d) in &tags {
            table.extend(sig);
            table.extend((offset as u32).to_be_bytes());
            table.extend((d.len() as u32).to_be_bytes());
            data.extend(d);
            offset += d.len();
        }
        out.extend(table);
        out.extend(data);
        let size = (out.len() as u32).to_be_bytes();
        out[0..4].copy_from_slice(&size);
        out
    }

    /// sRGB's colorants as its ICC profile carries them, D50 adapted.
    const SRGB_RED: [f64; 3] = [0.4360, 0.2225, 0.0139];
    const SRGB_GREEN: [f64; 3] = [0.3851, 0.7169, 0.0971];
    const SRGB_BLUE: [f64; 3] = [0.1431, 0.0606, 0.7141];

    /// A curve of `n` evenly spaced entries, sRGB's, as the HP profile's
    /// 1024 are.
    fn srgb_table(n: usize) -> Vec<u8> {
        let mut d = b"curv\0\0\0\0".to_vec();
        d.extend((n as u32).to_be_bytes());
        for i in 0..n {
            let v = srgb_decode(i as f32 / (n - 1) as f32) as f64;
            d.extend(((v * 65535.0).round() as u16).to_be_bytes());
        }
        d
    }

    /// The three curves the same.
    fn curves(curve: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        [*b"rTRC", *b"gTRC", *b"bTRC"]
            .map(|s| (s, curve.to_vec()))
            .to_vec()
    }

    /// A profile's colorants in the s15Fixed16 steps it holds them in.
    fn fixed(v: [i32; 3]) -> [f64; 3] {
        v.map(|x| x as f64 / 65536.0)
    }

    /// The sRGB profiles files are tagged with count as sRGB, so their
    /// thumbnails and views take the cheap way; the curve within half a
    /// level and the matrix's corners within [`CORNER_LEVELS`]. The HP
    /// "sRGB IEC61966-2.1" profile (Lightroom's, Photoshop's, Capture
    /// One's) with its 1024-entry table, and lcms's built-in one
    /// (`cmsCreate_sRGBProfile`, colord's too) with its parametric
    /// curve, both built here with their own colorants. A wider space
    /// is hundreds of levels out at a corner, whatever its curve.
    #[test]
    fn the_common_srgb_profiles_are_srgb_and_wider_ones_are_not() {
        let levels = |icc: &[u8]| {
            let to = ToSrgb::of_profile(Some(icc)).unwrap();
            let p = MatrixTrc::parse(icc).unwrap();
            (
                to.is_srgb(),
                curve_levels(&p.trc),
                corner_levels(&to.matrix),
            )
        };
        let hp = profile_with(
            [0.4360657, 0.2224884, 0.0139160],
            [0.3851471, 0.7168732, 0.0970764],
            [0.1430664, 0.0606079, 0.7140961],
            "sRGB IEC61966-2.1",
            curves(&srgb_table(1024)),
        );
        // lcms's parametric sRGB curve, its own fixed-point steps.
        let mut para = b"para\0\0\0\0\0\x03\0\0".to_vec();
        for p in [0x26666i32, 0xf2a7, 0x0d59, 0x13d0, 0x0a5b] {
            para.extend(p.to_be_bytes());
        }
        let lcms = profile_with(
            fixed([28576, 14581, 912]),
            fixed([25239, 46983, 6361]),
            fixed([9375, 3972, 46787]),
            "sRGB built-in",
            curves(&para),
        );
        let here = profile(SRGB_RED, SRGB_GREEN, SRGB_BLUE, "sRGB built here");
        for (name, icc) in [("HP", &hp), ("lcms", &lcms), ("built here", &here)] {
            let (srgb, curve, corner) = levels(icc);
            eprintln!("{name}: the curve {curve:.3} levels off, a corner {corner:.3}");
            assert!(
                srgb,
                "{name}: the curve {curve} levels off, a corner {corner}"
            );
        }
        assert!(ToSrgb::of_profile(None).unwrap().is_srgb());

        let mut adobe_gamma = b"curv\0\0\0\0\0\0\0\x01".to_vec();
        adobe_gamma.extend(563u16.to_be_bytes());
        let mut pro_gamma = b"curv\0\0\0\0\0\0\0\x01".to_vec();
        pro_gamma.extend(461u16.to_be_bytes());
        let wide = [
            // sRGB's curve: the matrix alone says no.
            (
                "Display P3",
                profile(
                    [0.5151, 0.2412, -0.0011],
                    [0.2920, 0.6922, 0.0419],
                    [0.1571, 0.0666, 0.7841],
                    "Display P3",
                ),
            ),
            (
                "Rec.2020",
                profile(
                    [0.6734, 0.2790, -0.0019],
                    [0.1656, 0.6753, 0.0300],
                    [0.1251, 0.0457, 0.7973],
                    "Rec.2020",
                ),
            ),
            (
                "Adobe RGB",
                profile_with(
                    [0.6097, 0.3111, 0.0195],
                    [0.2053, 0.6257, 0.0609],
                    [0.1492, 0.0632, 0.7446],
                    "Adobe RGB",
                    curves(&adobe_gamma),
                ),
            ),
            (
                "ProPhoto",
                profile_with(
                    [0.7977, 0.2880, 0.0],
                    [0.1352, 0.7119, 0.0],
                    [0.0313, 0.0001, 0.8249],
                    "ProPhoto",
                    curves(&pro_gamma),
                ),
            ),
        ];
        for (name, icc) in wide {
            let (srgb, _, corner) = levels(&icc);
            eprintln!("{name}: a corner {corner:.3} levels off");
            assert!(!srgb && corner > 10.0 * CORNER_LEVELS, "{name}: {corner}");
        }
    }

    /// A picture's samples taken to sRGB a pixel at a time agree with
    /// the decode into the working space taken on to sRGB: in ProPhoto
    /// (its own colorants and a gamma of 1.8), at 8 bits and 16, and as
    /// floats.
    #[test]
    fn samples_to_srgb_agree_with_the_decode() {
        use crate::color::srgb_encode;
        let mut gamma = b"curv\0\0\0\0\0\0\0\x01".to_vec();
        gamma.extend(((1.8f32 * 256.0).round() as u16).to_be_bytes());
        let curves = [b"rTRC", b"gTRC", b"bTRC"].map(|s| (*s, gamma.clone()));
        let prophoto = profile_with(
            [0.7977, 0.2880, 0.0],
            [0.1352, 0.7119, 0.0],
            [0.0313, 0.0001, 0.8249],
            "ProPhoto built here",
            curves.to_vec(),
        );
        let to = ToSrgb::of_profile(Some(&prophoto)).unwrap();
        assert!(!to.is_srgb());
        let to_srgb = rows(&WORKING_SPACE.to_space_matrix(&RgbSpace::SRGB, CAT).unwrap());
        let developed = |p: [f32; 3]| -> [f32; 3] {
            to_srgb.map(|r| srgb_encode((r[0] * p[0] + r[1] * p[1] + r[2] * p[2]).clamp(0.0, 1.0)))
        };
        let samples: [[u16; 3]; 4] = [
            [20000, 45000, 9000],
            [50000, 12000, 15000],
            [30000, 30000, 30000],
            [65535, 65535, 65535],
        ];
        let mut tiff = Cursor::new(Vec::new());
        // Rows enough that the `image` crate reads the profile: its
        // TIFF decoder refuses a tag whose values outweigh the picture's
        // bytes.
        let height = 1024;
        crate::exif::write_rgb16_tiff(
            &mut tiff,
            4,
            height,
            &samples.as_flattened().repeat(height as usize),
            Some(&prophoto),
            None,
        )
        .unwrap();
        let bytes = tiff.into_inner();
        let pic = decode_picture(&bytes).unwrap();
        assert_eq!(pic.space, "ProPhoto built here");
        let decoded = image::load_from_memory(&bytes).unwrap();
        let whole = to.rgb8(&decoded);
        for (x, s) in samples.iter().enumerate() {
            let want = developed(pic.image.pixel(x, 0));
            let got = to.encoded(s.map(|v| v as f32 / 65535.0));
            for i in 0..3 {
                assert!((got[i] - want[i]).abs() < 1e-3, "{got:?} vs {want:?}");
                let level = whole.get_pixel(x as u32, 0)[i] as f32;
                assert!(
                    (level - want[i] * 255.0).abs() <= 0.6,
                    "{x}: {level} vs {want:?}"
                );
            }
        }
        // At 8 bits, through the 8-bit table.
        let eight = DynamicImage::ImageRgb8(decoded.to_rgb8());
        let whole = to.rgb8(&eight);
        for x in 0..4 {
            let p = eight.as_rgb8().unwrap().get_pixel(x, 0).0;
            let want = to.encoded(p.map(|v| v as f32 / 255.0));
            for i in 0..3 {
                let level = whole.get_pixel(x, 0)[i] as f32;
                assert!(
                    (level - want[i] * 255.0).abs() <= 0.6,
                    "{level} vs {want:?}"
                );
            }
        }
        // Floats are linear, whatever the curve says.
        let lin = [0.1f32, 0.5, 0.02];
        let floats =
            DynamicImage::ImageRgb32F(image::Rgb32FImage::from_raw(1, 1, lin.to_vec()).unwrap());
        let got = to.rgb8(&floats).get_pixel(0, 0).0;
        let want = to.linear(lin);
        for i in 0..3 {
            assert!((got[i] as f32 - want[i] * 255.0).abs() <= 0.6);
        }
    }

    #[test]
    fn a_profile_is_read_and_srgbs_lands_on_the_working_matrix() {
        let p = MatrixTrc::parse(&profile(SRGB_RED, SRGB_GREEN, SRGB_BLUE, "sRGB built here"))
            .expect("a matrix profile");
        assert_eq!(p.description.as_deref(), Some("sRGB built here"));
        assert!((p.red[1] - 0.2225).abs() < 1e-4);
        let t = Transform::from_profile(&p).unwrap();
        let s = Transform::srgb().unwrap();
        for i in 0..3 {
            for j in 0..3 {
                assert!(
                    (t.matrix[i][j] - s.matrix[i][j]).abs() < 2e-3,
                    "{i}{j}: {} vs {}",
                    t.matrix[i][j],
                    s.matrix[i][j]
                );
            }
        }
        // The parametric curve is sRGB's.
        for x in [0.0f32, 0.02, 0.2, 0.5, 1.0] {
            assert!((t.trc[0].linear(x) - srgb_decode(x)).abs() < 1e-4, "{x}");
        }
        // Not a matrix profile: refused, not misread.
        let mut lut = profile(SRGB_RED, SRGB_GREEN, SRGB_BLUE, "x");
        lut[20..24].copy_from_slice(b"Lab ");
        assert!(MatrixTrc::parse(&lut).is_none());
        assert!(MatrixTrc::parse(b"short").is_none());
    }

    /// The picture's white: D65 for sRGB and a file without a profile,
    /// a version 2 profile's `wtpt`, a version 4 profile's `chad`
    /// undone, and D50 for a profile that names none.
    #[test]
    fn a_profiles_white_is_read() {
        let near = |c: Chromaticity, want: Chromaticity| {
            (c.x - want.x).abs() < 2e-4 && (c.y - want.y).abs() < 2e-4
        };
        assert!(near(
            Transform::srgb().unwrap().white,
            rawcolor::illuminant::D65
        ));
        let white_of = |extra| {
            let p = MatrixTrc::parse(&profile_with(SRGB_RED, SRGB_GREEN, SRGB_BLUE, "w", extra))
                .unwrap();
            Transform::from_profile(&p).unwrap().white
        };
        // ProPhoto's white, a version 2 profile's own.
        let d50 = [0.9642, 1.0, 0.8249];
        assert!(near(
            white_of(vec![(*b"wtpt", xyz_tag(d50))]),
            rawcolor::illuminant::D50
        ));
        let d65 = [0.9505, 1.0, 1.089];
        assert!(near(
            white_of(vec![(*b"wtpt", xyz_tag(d65))]),
            rawcolor::illuminant::D65
        ));
        // Version 4: `wtpt` is D50 and `chad` (Bradford, D65 to D50)
        // says the white was D65.
        let chad = rawcolor::cat::adaptation_matrix(
            rawcolor::illuminant::D65,
            rawcolor::illuminant::D50,
            Cat::Bradford,
        )
        .unwrap();
        let mut sf32 = b"sf32\0\0\0\0".to_vec();
        for v in chad.rows.iter().flatten() {
            sf32.extend(((v * 65536.0).round() as i32).to_be_bytes());
        }
        assert!(near(
            white_of(vec![(*b"wtpt", xyz_tag(d50)), (*b"chad", sf32)]),
            rawcolor::illuminant::D65
        ));
        assert!(near(white_of(Vec::new()), rawcolor::illuminant::D50));
        // A white of nothing, as a broken profile writes one: D50, and the
        // picture still opens.
        for broken in [[0.0, 0.0, 0.0], [0.9642, 0.0, 0.8249], [-1.0, 1.0, 1.0]] {
            assert!(near(
                white_of(vec![(*b"wtpt", xyz_tag(broken))]),
                rawcolor::illuminant::D50
            ));
        }
    }

    #[test]
    fn a_version_four_description_is_read() {
        let mut d = b"mluc\0\0\0\0".to_vec();
        d.extend(1u32.to_be_bytes());
        d.extend(12u32.to_be_bytes());
        d.extend(b"enUS");
        let name: Vec<u8> = "P3 wide"
            .encode_utf16()
            .flat_map(|u| u.to_be_bytes())
            .collect();
        d.extend((name.len() as u32).to_be_bytes());
        d.extend(28u32.to_be_bytes());
        d.extend(name);
        assert_eq!(parse_description(&d).as_deref(), Some("P3 wide"));
    }

    #[test]
    fn curves_of_every_kind() {
        assert_eq!(parse_curve(b"curv\0\0\0\0\0\0\0\0"), Some(Trc::Identity));
        let g = parse_curve(b"curv\0\0\0\0\0\0\0\x01\x02\x33").unwrap();
        assert!(matches!(g, Trc::Gamma(v) if (v - 2.2).abs() < 0.01));
        let mut table = b"curv\0\0\0\0\0\0\0\x03".to_vec();
        table.extend([0u8, 0, 0x40, 0, 0xff, 0xff]);
        let t = parse_curve(&table).unwrap();
        assert!((t.linear(0.5) - 0.25).abs() < 1e-3);
        assert!((t.linear(0.25) - 0.125).abs() < 1e-3);
        assert!((Trc::Gamma(2.0).linear(0.5) - 0.25).abs() < 1e-6);
    }

    /// A PNG with an EXIF chunk saying it is on its side, in a wide
    /// space, opens upright in the working space with its tags kept.
    #[test]
    fn a_png_opens_upright_in_the_working_space_with_its_tags() {
        // Three by two, a red pixel at the top left, in Display P3.
        let mut px = image::RgbImage::new(3, 2);
        px.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        px.put_pixel(2, 1, image::Rgb([128, 128, 128]));
        let p3 = profile(
            [0.5151, 0.2412, -0.0011],
            [0.2920, 0.6922, 0.0419],
            [0.1571, 0.0666, 0.7841],
            "Display P3",
        );
        let mut exif = Cursor::new(Vec::new());
        {
            let mut tiff = TiffWriter::new(&mut exif).unwrap();
            let mut root = DirectoryWriter::new();
            let mut sub = DirectoryWriter::new();
            root.add_tag(ExifTag::Make, "Canon");
            root.add_tag(ExifTag::Model, "EOS R6m2");
            // Rotate90: the file's top is the picture's right.
            root.add_tag(ExifTag::Orientation, 6u16);
            sub.add_tag(ExifTag::LensModel, "RF50mm F1.8 STM");
            sub.add_tag(ExifTag::ISOSpeedRatings, 400u16);
            sub.add_tag(ExifTag::ExposureTime, Rational::new(1, 250));
            let off = sub.build(&mut tiff).unwrap();
            root.add_tag(TiffCommonTag::ExifIFDPointer, off);
            tiff.build(root).unwrap();
        }
        let mut png = Cursor::new(Vec::new());
        let mut enc = image::codecs::png::PngEncoder::new(&mut png);
        enc.set_icc_profile(p3.clone()).unwrap();
        enc.set_exif_metadata(exif.into_inner()).unwrap();
        enc.write_image(px.as_raw(), 3, 2, image::ExtendedColorType::Rgb8)
            .unwrap();
        let pic = decode_picture(&png.into_inner()).unwrap();
        assert_eq!((pic.image.width, pic.image.height), (2, 3));
        assert_eq!(pic.make, "Canon");
        assert_eq!(pic.model, "EOS R6m2");
        assert_eq!(pic.shot.lens_model.as_deref(), Some("RF50mm F1.8 STM"));
        assert_eq!(pic.shot.iso, Some(400));
        assert_eq!(pic.shot.exposure_time, Some(1.0 / 250.0));
        assert_eq!(pic.space, "Display P3");
        assert_eq!(pic.bits, 8);
        // Turned a quarter clockwise, the top left is the top right.
        let red = pic.image.pixel(1, 0);
        assert!(
            red[0] > 0.5 && red[1].abs() < 0.05 && red[2].abs() < 0.05,
            "{red:?}"
        );
        // A P3 red is redder than an sRGB red in Rec.2020: less green.
        let srgb_red = {
            let m = Transform::srgb().unwrap().matrix;
            [m[0][0], m[1][0], m[2][0]]
        };
        assert!(red[1] < srgb_red[1], "{red:?} vs {srgb_red:?}");
        // The grey is neutral and linear: 128 is 21.4% in sRGB's curve.
        let grey = pic.image.pixel(0, 2);
        assert!(
            (grey[0] - 0.2140).abs() < 0.01 && (grey[0] - grey[1]).abs() < 0.01,
            "{grey:?}"
        );
        // Without a profile, sRGB is assumed and said.
        let mut plain = Cursor::new(Vec::new());
        image::codecs::png::PngEncoder::new(&mut plain)
            .write_image(px.as_raw(), 3, 2, image::ExtendedColorType::Rgb8)
            .unwrap();
        let pic = decode_picture(&plain.into_inner()).unwrap();
        assert_eq!(pic.space, "sRGB (no profile)");
        assert_eq!((pic.image.width, pic.image.height), (3, 2));
        assert!(pic.make.is_empty());
        let red = pic.image.pixel(0, 0);
        assert!((red[0] - srgb_red[0]).abs() < 1e-3);
    }

    #[test]
    fn a_sixteen_bit_tiff_keeps_its_depth() {
        let data: Vec<u16> = vec![65535, 32768, 0, 65535, 32768, 0];
        let mut tiff = Cursor::new(Vec::new());
        image::codecs::tiff::TiffEncoder::new(&mut tiff)
            .write_image(
                &bytemuck_bytes(&data),
                2,
                1,
                image::ExtendedColorType::Rgb16,
            )
            .unwrap();
        let pic = decode_picture(&tiff.into_inner()).unwrap();
        assert_eq!(pic.bits, 16);
        assert_eq!((pic.image.width, pic.image.height), (2, 1));
        let px = pic.image.pixel(0, 0);
        // A yellow: sRGB's red and green at full, decoded and matrixed.
        let m = Transform::srgb().unwrap().matrix;
        let want = [0, 1, 2].map(|i| m[i][0] + m[i][1] * srgb_decode(32768.0 / 65535.0));
        for i in 0..3 {
            assert!((px[i] - want[i]).abs() < 1e-3, "{px:?} vs {want:?}");
        }
    }

    fn bytemuck_bytes(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|s| s.to_ne_bytes()).collect()
    }

    #[test]
    fn extensions() {
        assert!(is_picture_path(Path::new("a/b.JPG")));
        assert!(is_picture_path(Path::new("x.tiff")));
        assert!(!is_picture_path(Path::new("x.cr3")));
        assert!(!is_picture_path(Path::new("x")));
    }

    /// An EXIF payload with a camera in it, as an export writes one.
    fn exif_payload() -> Vec<u8> {
        use ::rawler::formats::tiff::Rational;
        let metadata = RawMetadata {
            exif: Exif {
                fnumber: Some(Rational::new(28, 10)),
                iso_speed_ratings: Some(800),
                date_time_original: Some("2026:09:01 10:20:30".into()),
                lens_model: Some("RF50mm F1.8 STM".into()),
                ..Exif::default()
            },
            model: "EOS R6m2".into(),
            make: "Canon".into(),
            lens: None,
            unique_image_id: None,
            rating: None,
        };
        crate::exif::payload(&crate::exif::Provenance {
            metadata: &metadata,
            software: "probe test",
            width: 4,
            height: 3,
            srgb: true,
            written: None,
            source_name: None,
            output: None,
            edit: None,
            rights: &crate::exif::NO_RIGHTS,
        })
        .unwrap()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// A JPEG's and a PNG's stance read from the head alone is the one
    /// the whole file gives: past the first read, with a long tail, and
    /// cut off after the head.
    #[test]
    fn the_stance_from_the_head_is_the_whole_file_s() {
        use image::codecs::{jpeg::JpegEncoder, png::PngEncoder};
        use image::{ExtendedColorType, ImageEncoder};
        let (w, h) = (5u32, 3u32);
        let px: Vec<u8> = (0..w * h * 3).map(|i| (i * 7) as u8).collect();
        let mut jpeg = Vec::new();
        {
            let mut enc = JpegEncoder::new_with_quality(&mut jpeg, 95);
            enc.set_exif_metadata(exif_payload()).unwrap();
            enc.write_image(&px, w, h, ExtendedColorType::Rgb8).unwrap();
        }
        let mut png = Vec::new();
        {
            let mut enc = PngEncoder::new(&mut png);
            enc.set_exif_metadata(exif_payload()).unwrap();
            enc.write_image(&px, w, h, ExtendedColorType::Rgb8).unwrap();
        }
        // A comment run past the first read in front of the JPEG's
        // segments, and a long tail behind either.
        let mut padded = vec![0xFF, 0xD8];
        for _ in 0..3 {
            let len = 60_000u16;
            padded.extend_from_slice(&[0xFF, 0xFE]);
            padded.extend_from_slice(&len.to_be_bytes());
            padded.extend(std::iter::repeat_n(b' ', len as usize - 2));
        }
        padded.extend_from_slice(&jpeg[2..]);
        let tail = |b: &[u8]| {
            let mut t = b.to_vec();
            t.extend(std::iter::repeat_n(0u8, 3 * PROBE_HEAD));
            t
        };
        let path = scratch("stance.jpg");
        for (name, bytes) in [
            ("a.jpg", jpeg.clone()),
            ("far.jpg", padded.clone()),
            ("tail.jpg", tail(&jpeg)),
            ("a.png", png.clone()),
            ("tail.png", tail(&png)),
        ] {
            let at = path.with_file_name(name);
            std::fs::write(&at, &bytes).unwrap();
            let whole = stance_of(&bytes).unwrap();
            assert_eq!(whole.1, Some((w, h)), "{name}");
            assert_eq!(head_stance(&at).unwrap(), Some(whole), "{name}");
            assert_eq!(stance_path(&at).unwrap(), whole, "{name}");
        }
        // Cut off after the head: the head still says it.
        let Head::Done { .. } = jpeg_head(&jpeg) else {
            panic!("a whole head");
        };
        let sos = jpeg.windows(2).position(|m| m == [0xFF, 0xDA]).unwrap();
        let cut = path.with_file_name("cut.jpg");
        std::fs::write(&cut, &jpeg[..sos + 2]).unwrap();
        assert_eq!(stance_path(&cut).unwrap(), stance_of(&jpeg).unwrap());
        // A TIFF is not walked.
        std::fs::write(&cut, b"II*\0........").unwrap();
        assert_eq!(head_stance(&cut).unwrap(), None);
        crate::testing::remove_dir_retry(path.parent().unwrap());
    }

    /// The probe reads a picture's head and no more: a JPEG's EXIF
    /// before its scan, whole even when the file is cut off after it
    /// or the EXIF sits past the first read.
    #[test]
    fn the_probe_reads_the_head_and_survives_a_cut_file() {
        use image::codecs::{jpeg::JpegEncoder, png::PngEncoder};
        use image::{ExtendedColorType, ImageEncoder};
        let px: Vec<u8> = (0..4 * 3 * 3).map(|i| (i * 7) as u8).collect();
        let mut jpeg = Vec::new();
        {
            let mut enc = JpegEncoder::new_with_quality(&mut jpeg, 95);
            enc.set_exif_metadata(exif_payload()).unwrap();
            enc.write_image(&px, 4, 3, ExtendedColorType::Rgb8).unwrap();
        }
        let path = scratch("a.jpg");
        std::fs::write(&path, &jpeg).unwrap();
        let p = probe_path(&path).unwrap();
        assert_eq!((p.make.as_str(), p.model.as_str()), ("Canon", "EOS R6m2"));
        assert_eq!(p.iso, Some(800));
        assert_eq!(p.lens_model.as_deref(), Some("RF50mm F1.8 STM"));
        assert_eq!(p.taken.as_deref(), Some("2026:09:01 10:20:30"));
        // The walk found it where the decoder does.
        let Scan::Found(range) = jpeg_exif(&jpeg) else {
            panic!("no EXIF in the walk");
        };
        assert!(jpeg[range.clone()].starts_with(b"II") || jpeg[range.clone()].starts_with(b"MM"));

        // Cut off right after the EXIF segment: still probes.
        let cut = path.with_file_name("cut.jpg");
        std::fs::write(&cut, &jpeg[..range.end + 3]).unwrap();
        assert_eq!(probe_path(&cut).unwrap().make, "Canon");
        // Cut off inside the EXIF segment: says nothing, and is not
        // an error.
        std::fs::write(&cut, &jpeg[..range.start + 10]).unwrap();
        assert_eq!(probe_path(&cut).unwrap(), Probe::default());

        // A comment segment past the first read's size in front of
        // the EXIF: the read grows to it. Segments are at most 64 KB
        // each, so several of them.
        let mut padded = vec![0xFF, 0xD8];
        for _ in 0..3 {
            let len = 60_000u16;
            padded.extend_from_slice(&[0xFF, 0xFE]);
            padded.extend_from_slice(&len.to_be_bytes());
            padded.extend(std::iter::repeat_n(b' ', len as usize - 2));
        }
        padded.extend_from_slice(&jpeg[2..]);
        assert!(padded.len() > PROBE_HEAD);
        let far = path.with_file_name("far.jpg");
        std::fs::write(&far, &padded).unwrap();
        assert_eq!(probe_path(&far).unwrap().model, "EOS R6m2");
        assert!(matches!(
            jpeg_exif(&padded[..PROBE_HEAD]),
            Scan::NeedMore(_)
        ));

        // A PNG's chunk, before its picture data, and one without.
        let mut png = Vec::new();
        {
            let mut enc = PngEncoder::new(&mut png);
            enc.set_exif_metadata(exif_payload()).unwrap();
            enc.write_image(&px, 4, 3, ExtendedColorType::Rgb8).unwrap();
        }
        let png_path = path.with_file_name("a.png");
        std::fs::write(&png_path, &png).unwrap();
        assert_eq!(probe_path(&png_path).unwrap().make, "Canon");
        let mut bare = Vec::new();
        PngEncoder::new(&mut bare)
            .write_image(&px, 4, 3, ExtendedColorType::Rgb8)
            .unwrap();
        std::fs::write(&png_path, &bare).unwrap();
        assert_eq!(probe_path(&png_path).unwrap(), Probe::default());
        assert_eq!(png_exif(&bare), Scan::None);

        // Not a picture at all.
        std::fs::write(&cut, b"nothing like a picture").unwrap();
        assert!(probe_path(&cut).is_err());
        crate::testing::remove_dir_retry(path.parent().unwrap());
    }
}
