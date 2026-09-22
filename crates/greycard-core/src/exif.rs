//! EXIF for the pictures the engine's consumers write.
//!
//! An export is a new picture of an old exposure. What the camera said
//! about the exposure (the lens, the shutter, the aperture, the ISO,
//! the time, the place) is still true of it and travels with it; what
//! describes the file (its size, its orientation, what wrote it, when)
//! is the export's own and is set here. What the edit did is the
//! export's own too, and goes in an XMP packet beside the EXIF: the
//! source file's name, what the output was asked to be, and the edit
//! itself as its consumer wrote it, so a picture can say where it came
//! from and be made again. The source's name is in the EXIF as well,
//! as `OriginalRawFileName`.
//!
//! The tags are laid out as a TIFF structure, which is what every
//! container carries: a JPEG in an APP1 segment, a PNG in an `eXIf`
//! chunk, a TIFF as its own directories. [`payload`] builds the first
//! two; [`write_rgb16_tiff`] writes the third whole, since the image
//! crate's TIFF encoder takes no tags of its own. The XMP goes into a
//! JPEG or PNG the image crate has written by [`with_xmp`], since its
//! encoders take none, and into the TIFF as tag 700.

use std::io::{Cursor, Seek, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use ::rawler::formats::tiff::{DirectoryWriter, TiffWriter, Value};
use ::rawler::imgop::Dim2;
use ::rawler::tags::{ExifTag, TiffCommonTag};

use crate::decode::rawler_backend::RawMetadata;
use crate::error::{Error, Result};

/// What an exported picture says about where it came from and what it is.
#[derive(Debug, Clone, Copy)]
pub struct Provenance<'a> {
    /// The source file's metadata, as rawler read it.
    pub metadata: &'a RawMetadata,
    /// The `Software` tag: what wrote the picture.
    pub software: &'a str,
    /// The picture's size as written, after any resize.
    pub width: u32,
    pub height: u32,
    /// Whether the samples are sRGB. Anything else is marked
    /// uncalibrated, and the embedded profile says which space it is.
    pub srgb: bool,
    /// When the picture was written. None leaves the modify date out.
    pub written: Option<SystemTime>,
    /// The source file's name, for `OriginalRawFileName` and the XMP.
    pub source_name: Option<&'a str>,
    /// What the output was asked to be, in words, for the XMP.
    pub output: Option<&'a str>,
    /// The edit as its consumer serializes it, for the XMP; the engine
    /// takes no schema and passes it through untouched.
    pub edit: Option<&'a str>,
}

/// The XMP namespace an export's own properties are in.
pub const NAMESPACE: &str = "https://greycard.org/ns/1.0/";

/// The most a JPEG's APP1 segment holds of a packet, past its own
/// header: the segment's length field is sixteen bits.
const JPEG_PACKET_LIMIT: usize = 65_533 - XMP_HEADER.len();
const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// The XMP packet: the creator tool and the time, then the export's
/// own properties. With `limit`, the edit is left out when the packet
/// would not fit (a JPEG's segment), and the packet says so.
pub fn xmp(provenance: &Provenance<'_>, limit: Option<usize>) -> String {
    let build = |edit: Option<&str>, note: Option<&str>| {
        let mut p = String::new();
        p.push_str("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
        p.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n");
        p.push_str(" <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
        p.push_str("  <rdf:Description rdf:about=\"\"\n");
        p.push_str("    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n");
        p.push_str(&format!("    xmlns:greycard=\"{NAMESPACE}\">\n"));
        p.push_str(&format!(
            "   <xmp:CreatorTool>{}</xmp:CreatorTool>\n",
            escape(provenance.software)
        ));
        if let Some(t) = provenance
            .written
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        {
            let d = date_time(t.as_secs());
            // EXIF's `YYYY:MM:DD HH:MM:SS` to ISO 8601, in UTC.
            p.push_str(&format!(
                "   <xmp:ModifyDate>{}-{}-{}T{}Z</xmp:ModifyDate>\n",
                &d[0..4],
                &d[5..7],
                &d[8..10],
                &d[11..]
            ));
        }
        if let Some(name) = provenance.source_name {
            p.push_str(&format!(
                "   <greycard:Source>{}</greycard:Source>\n",
                escape(name)
            ));
        }
        if let Some(o) = provenance.output {
            p.push_str(&format!(
                "   <greycard:Output>{}</greycard:Output>\n",
                escape(o)
            ));
        }
        if let Some(e) = edit {
            p.push_str(&format!(
                "   <greycard:Edit>{}</greycard:Edit>\n",
                escape(e)
            ));
        }
        if let Some(n) = note {
            p.push_str(&format!(
                "   <greycard:Note>{}</greycard:Note>\n",
                escape(n)
            ));
        }
        p.push_str("  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>");
        p
    };
    let whole = build(provenance.edit, None);
    match limit {
        Some(limit) if whole.len() > limit && provenance.edit.is_some() => build(
            None,
            Some("the edit was too long for this container's packet"),
        ),
        _ => whole,
    }
}

/// XML's text escaping.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// The picture with its XMP packet in: a JPEG's APP1 segment after
/// the ones the encoder wrote, or a PNG's `iTXt` chunk after its
/// header. Anything else comes back as it was.
pub fn with_xmp(picture: Vec<u8>, provenance: &Provenance<'_>) -> Vec<u8> {
    if picture.starts_with(&[0xFF, 0xD8]) {
        let packet = xmp(provenance, Some(JPEG_PACKET_LIMIT));
        // Past the SOI, over every APPn segment.
        let mut at = 2;
        while at + 4 <= picture.len()
            && picture[at] == 0xFF
            && (0xE0..=0xEF).contains(&picture[at + 1])
        {
            let len = u16::from_be_bytes([picture[at + 2], picture[at + 3]]) as usize;
            at += 2 + len;
        }
        let mut segment = vec![0xFF, 0xE1];
        let len = 2 + XMP_HEADER.len() + packet.len();
        segment.extend((len as u16).to_be_bytes());
        segment.extend(XMP_HEADER);
        segment.extend(packet.as_bytes());
        let mut out = Vec::with_capacity(picture.len() + segment.len());
        out.extend(&picture[..at]);
        out.extend(segment);
        out.extend(&picture[at..]);
        out
    } else if picture.starts_with(b"\x89PNG\r\n\x1a\n") && picture.len() >= 33 {
        let packet = xmp(provenance, None);
        let mut data = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
        data.extend(packet.as_bytes());
        let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
        chunk.extend(b"iTXt");
        chunk.extend(&data);
        chunk.extend(crc32(&chunk[4..]).to_be_bytes());
        // After the IHDR chunk: the signature and its 13 bytes of data
        // with the framing.
        let at = 8 + 4 + 4 + 13 + 4;
        let mut out = Vec::with_capacity(picture.len() + chunk.len());
        out.extend(&picture[..at]);
        out.extend(chunk);
        out.extend(&picture[at..]);
        out
    } else {
        picture
    }
}

/// PNG's CRC, over the chunk's type and data.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The EXIF payload for a JPEG's APP1 segment or a PNG's `eXIf` chunk:
/// a TIFF structure with the tags in their directories, no image.
pub fn payload(provenance: &Provenance<'_>) -> Result<Vec<u8>> {
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut tiff = TiffWriter::new(&mut buffer).map_err(encode)?;
        let mut root = DirectoryWriter::new();
        let mut exif = DirectoryWriter::new();
        fill(&mut tiff, &mut root, &mut exif, provenance)?;
        let exif_offset = exif.build(&mut tiff).map_err(encode)?;
        root.add_tag(TiffCommonTag::ExifIFDPointer, exif_offset);
        tiff.build(root).map_err(encode)?;
    }
    Ok(buffer.into_inner())
}

/// Write an interleaved 16-bit RGB picture as a TIFF, LZW compressed
/// under the horizontal predictor (without it LZW makes a photograph's
/// 16-bit samples larger, not smaller), with the profile and the
/// provenance's tags when given.
///
/// The samples are as they are: the caller has already encoded them for
/// the space the profile names.
pub fn write_rgb16_tiff<W: Write + Seek>(
    out: W,
    width: u32,
    height: u32,
    data: &[u16],
    icc: Option<&[u8]>,
    provenance: Option<&Provenance<'_>>,
) -> Result<()> {
    const CHANNELS: usize = 3;
    let (w, h) = (width as usize, height as usize);
    if data.len() != w * h * CHANNELS {
        return Err(Error::Encode(format!(
            "{}x{} RGB16 wants {} samples, got {}",
            width,
            height,
            w * h * CHANNELS,
            data.len()
        )));
    }
    if w == 0 || h == 0 {
        return Err(Error::Encode("an empty picture".into()));
    }
    let mut tiff = TiffWriter::new(out).map_err(encode)?;
    let mut root = DirectoryWriter::new();
    let mut exif = DirectoryWriter::new();

    let differenced = horizontal_differences(data, w * CHANNELS, CHANNELS);
    let (rows_per_strip, strips) = tiff
        .write_strips_lzw(&differenced, CHANNELS, Dim2 { w, h }, 0)
        .map_err(encode)?;
    let offsets: Vec<u32> = strips.iter().map(|s| s.0).collect();
    let counts: Vec<u32> = strips.iter().map(|s| s.1).collect();
    root.add_tag(TiffCommonTag::ImageWidth, width);
    root.add_tag(TiffCommonTag::ImageLength, height);
    root.add_tag(TiffCommonTag::BitsPerSample, [16u16; CHANNELS].as_slice());
    // 5 is LZW; 2 is RGB.
    root.add_tag(TiffCommonTag::Compression, 5u16);
    root.add_tag(TiffCommonTag::PhotometricInt, 2u16);
    root.add_tag(TiffCommonTag::StripOffsets, &offsets);
    root.add_tag(TiffCommonTag::SamplesPerPixel, CHANNELS as u16);
    root.add_tag(TiffCommonTag::RowsPerStrip, rows_per_strip);
    root.add_tag(TiffCommonTag::StripByteCounts, &counts);
    root.add_tag(ExifTag::PlanarConfiguration, 1u16);
    root.add_tag(TiffCommonTag::Predictor, 2u16);
    if let Some(icc) = icc {
        root.add_tag_undefined(ExifTag::IccProfile, icc.to_vec());
    }

    if let Some(provenance) = provenance {
        fill(&mut tiff, &mut root, &mut exif, provenance)?;
        let exif_offset = exif.build(&mut tiff).map_err(encode)?;
        root.add_tag(TiffCommonTag::ExifIFDPointer, exif_offset);
        root.add_untyped_tag(XMP_TAG, Value::Byte(xmp(provenance, None).into_bytes()));
    }
    tiff.build(root).map_err(encode)
}

/// TIFF's tag for an XMP packet.
pub const XMP_TAG: u16 = 700;
/// DNG's tag for the source file's name, at home in any TIFF structure.
pub const ORIGINAL_RAW_FILE_NAME: u16 = 0xC68B;

/// TIFF's horizontal predictor: each sample as its difference from the
/// same channel one pixel to the left, wrapping, rows of `row` samples
/// with `channels` to a pixel.
fn horizontal_differences(data: &[u16], row: usize, channels: usize) -> Vec<u16> {
    let mut out = data.to_vec();
    for line in out.chunks_exact_mut(row) {
        for i in (channels..line.len()).rev() {
            line[i] = line[i].wrapping_sub(line[i - channels]);
        }
    }
    out
}

/// The source's tags into the directories, then the export's own over
/// them. rawler writes the GPS directory itself, into `tiff`.
fn fill<W: Write + Seek>(
    tiff: &mut TiffWriter<W>,
    root: &mut DirectoryWriter,
    exif: &mut DirectoryWriter,
    provenance: &Provenance<'_>,
) -> Result<()> {
    let metadata = provenance.metadata;
    metadata.write_exif_tags(tiff, root, exif).map_err(encode)?;

    // rawler copies these even when the source left them blank.
    if metadata.exif.artist.as_deref().is_none_or(str::is_empty) {
        root.remove_tag(ExifTag::Artist);
    }
    if metadata.exif.copyright.as_deref().is_none_or(str::is_empty) {
        root.remove_tag(ExifTag::Copyright);
    }
    if metadata
        .exif
        .owner_name
        .as_deref()
        .is_none_or(str::is_empty)
    {
        exif.remove_tag(ExifTag::OwnerName);
    }
    if !metadata.make.trim().is_empty() {
        root.add_tag(ExifTag::Make, metadata.make.trim());
    }
    if !metadata.model.trim().is_empty() {
        root.add_tag(ExifTag::Model, metadata.model.trim());
    }

    // The picture is written the way up it is shown.
    root.add_tag(ExifTag::Orientation, 1u16);
    root.add_tag(ExifTag::Software, provenance.software);
    match provenance
        .written
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
    {
        Some(since_epoch) => {
            root.add_tag(ExifTag::ModifyDate, date_time(since_epoch.as_secs()));
            exif.add_tag(ExifTag::OffsetTime, "+00:00");
        }
        None => {
            root.remove_tag(ExifTag::ModifyDate);
            exif.remove_tag(ExifTag::OffsetTime);
        }
    }

    if let Some(name) = provenance.source_name {
        root.add_untyped_tag(ORIGINAL_RAW_FILE_NAME, name);
    }
    exif.add_tag_undefined(ExifTag::ExifVersion, b"0232".to_vec());
    exif.add_tag(
        ExifTag::ColorSpace,
        if provenance.srgb { 1u16 } else { 0xFFFF },
    );
    exif.add_untyped_tag(PIXEL_X_DIMENSION, Value::Long(vec![provenance.width]));
    exif.add_untyped_tag(PIXEL_Y_DIMENSION, Value::Long(vec![provenance.height]));
    Ok(())
}

/// The EXIF tags for the picture's size, which rawler's tag table lacks.
pub const PIXEL_X_DIMENSION: u16 = 0xa002;
pub const PIXEL_Y_DIMENSION: u16 = 0xa003;

/// Seconds since the epoch as the EXIF date format, `YYYY:MM:DD
/// HH:MM:SS`, in UTC.
pub fn date_time(secs: u64) -> String {
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{y:04}:{m:02}:{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date. Howard
/// Hinnant's algorithm, exact for any day the era holds.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn encode<E: std::fmt::Display>(e: E) -> Error {
    Error::Encode(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::rawler::exif::Exif;
    use ::rawler::formats::tiff::reader::TiffReader;
    use ::rawler::formats::tiff::{GenericTiffReader, IFD, Rational};

    fn metadata() -> RawMetadata {
        RawMetadata {
            exif: Exif {
                orientation: Some(8),
                artist: Some(String::new()),
                copyright: None,
                owner_name: Some(String::new()),
                exposure_time: Some(Rational::new(1, 250)),
                fnumber: Some(Rational::new(28, 10)),
                iso_speed_ratings: Some(800),
                date_time_original: Some("2026:09:01 10:20:30".into()),
                modify_date: Some("2026:09:01 10:20:30".into()),
                focal_length: Some(Rational::new(50, 1)),
                lens_model: Some("RF50mm F1.8 STM".into()),
                color_space: Some(1),
                ..Exif::default()
            },
            model: "EOS R6m2".into(),
            make: "Canon".into(),
            lens: None,
            unique_image_id: None,
            rating: None,
        }
    }

    fn read(bytes: &[u8]) -> (IFD, IFD) {
        let reader = GenericTiffReader::new_with_buffer(bytes, 0, 0, None).unwrap();
        let root = reader.root_ifd().clone();
        let exif = root
            .get_sub_ifd(TiffCommonTag::ExifIFDPointer)
            .expect("an EXIF directory")
            .clone();
        (root, exif)
    }

    fn text(ifd: &IFD, tag: impl Into<u16>) -> Option<String> {
        ifd.get_entry(tag.into())
            .and_then(|e| e.value.as_string().cloned())
    }

    #[test]
    fn the_source_is_copied_and_the_export_says_what_it_is() {
        let m = metadata();
        let bytes = payload(&Provenance {
            metadata: &m,
            software: "greycard test",
            width: 640,
            height: 480,
            srgb: false,
            written: Some(UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000)),
            source_name: Some("IMG_0001.CR3"),
            output: None,
            edit: None,
        })
        .unwrap();
        let (root, exif) = read(&bytes);
        assert_eq!(
            text(&root, ORIGINAL_RAW_FILE_NAME).as_deref(),
            Some("IMG_0001.CR3")
        );
        // The export's own.
        assert_eq!(
            root.get_entry(ExifTag::Orientation).unwrap().force_u16(0),
            1
        );
        assert_eq!(
            text(&root, ExifTag::Software).as_deref(),
            Some("greycard test")
        );
        assert_eq!(text(&root, ExifTag::Make).as_deref(), Some("Canon"));
        assert_eq!(text(&root, ExifTag::Model).as_deref(), Some("EOS R6m2"));
        assert_eq!(
            text(&root, ExifTag::ModifyDate).as_deref(),
            Some("2027:01:15 08:00:00")
        );
        assert!(
            root.get_entry(ExifTag::Artist).is_none(),
            "a blank artist is left out"
        );
        assert!(root.get_entry(ExifTag::Copyright).is_none());
        assert!(exif.get_entry(ExifTag::OwnerName).is_none());
        assert_eq!(exif.get_entry(PIXEL_X_DIMENSION).unwrap().force_u32(0), 640);
        assert_eq!(exif.get_entry(PIXEL_Y_DIMENSION).unwrap().force_u32(0), 480);
        assert_eq!(
            exif.get_entry(ExifTag::ColorSpace).unwrap().force_u16(0),
            0xFFFF
        );
        assert_eq!(text(&exif, ExifTag::OffsetTime).as_deref(), Some("+00:00"));
        // The source's.
        assert_eq!(
            text(&exif, ExifTag::DateTimeOriginal).as_deref(),
            Some("2026:09:01 10:20:30")
        );
        assert_eq!(
            text(&exif, ExifTag::LensModel).as_deref(),
            Some("RF50mm F1.8 STM")
        );
        assert_eq!(
            exif.get_entry(ExifTag::ISOSpeedRatings)
                .unwrap()
                .force_u16(0),
            800
        );
        match &exif.get_entry(ExifTag::FNumber).unwrap().value {
            Value::Rational(r) => assert_eq!((r[0].n, r[0].d), (28, 10)),
            other => panic!("{other:?}"),
        }
        match &exif.get_entry(ExifTag::ExposureTime).unwrap().value {
            Value::Rational(r) => assert_eq!((r[0].n, r[0].d), (1, 250)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn srgb_is_said_and_no_time_leaves_the_date_out() {
        let m = metadata();
        let bytes = payload(&Provenance {
            metadata: &m,
            software: "greycard test",
            width: 1,
            height: 1,
            srgb: true,
            written: None,
            source_name: None,
            output: None,
            edit: None,
        })
        .unwrap();
        let (root, exif) = read(&bytes);
        assert!(root.get_entry(ORIGINAL_RAW_FILE_NAME).is_none());
        assert_eq!(exif.get_entry(ExifTag::ColorSpace).unwrap().force_u16(0), 1);
        assert!(root.get_entry(ExifTag::ModifyDate).is_none());
        assert!(exif.get_entry(ExifTag::OffsetTime).is_none());
    }

    #[test]
    fn a_tiff_keeps_its_bits_and_its_tags() {
        let (w, h) = (37u32, 300u32);
        let data: Vec<u16> = (0..w * h * 3)
            .map(|i| (i.wrapping_mul(2654435761) >> 8) as u16)
            .collect();
        let m = metadata();
        let icc = b"not really a profile".to_vec();
        let mut file = Cursor::new(Vec::new());
        write_rgb16_tiff(
            &mut file,
            w,
            h,
            &data,
            Some(&icc),
            Some(&Provenance {
                metadata: &m,
                software: "greycard test",
                width: w,
                height: h,
                srgb: true,
                written: None,
                source_name: Some("a.dng"),
                output: Some("TIFF 16-bit, Rec.2020"),
                edit: Some("{\"version\": 2}"),
            }),
        )
        .unwrap();
        let bytes = file.into_inner();
        // The XMP is in, and the image crate reads it back.
        use image::ImageDecoder;
        let xmp = image::ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap()
            .xmp_metadata()
            .unwrap()
            .expect("an XMP packet");
        let xmp = String::from_utf8(xmp).unwrap();
        assert!(
            xmp.contains("<greycard:Source>a.dng</greycard:Source>"),
            "{xmp}"
        );
        assert!(xmp.contains("<greycard:Edit>{\"version\": 2}</greycard:Edit>"));
        // The image crate reads it back, sample for sample.
        let back = image::load_from_memory(&bytes).unwrap().into_rgb16();
        assert_eq!((back.width(), back.height()), (w, h));
        assert_eq!(back.as_raw(), &data);
        let (root, exif) = read(&bytes);
        match &root.get_entry(ExifTag::IccProfile).unwrap().value {
            Value::Undefined(v) => assert_eq!(v, &icc),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            root.get_entry(ExifTag::Orientation).unwrap().force_u16(0),
            1
        );
        assert_eq!(
            text(&exif, ExifTag::LensModel).as_deref(),
            Some("RF50mm F1.8 STM")
        );
        // More than one strip, since the height asks for it.
        assert!(root.get_entry(TiffCommonTag::StripOffsets).unwrap().count() > 1);
        // The wrong number of samples is refused.
        assert!(write_rgb16_tiff(Cursor::new(Vec::new()), w, h, &data[1..], None, None).is_err());
    }

    #[test]
    fn the_predictor_differences_along_the_row_by_channel() {
        let row = [10u16, 20, 30, 11, 25, 28, 9, 26, 65535];
        let two_rows: Vec<u16> = row.iter().chain(row.iter()).copied().collect();
        let d = horizontal_differences(&two_rows, row.len(), 3);
        let want = [10u16, 20, 30, 1, 5, 65534, 65534, 1, 65507];
        assert_eq!(&d[..9], &want);
        assert_eq!(&d[9..], &want, "each row starts afresh");
    }

    fn provenance<'a>(m: &'a RawMetadata, edit: Option<&'a str>) -> Provenance<'a> {
        Provenance {
            metadata: m,
            software: "greycard test",
            width: 4,
            height: 3,
            srgb: true,
            written: Some(UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000)),
            source_name: Some("Tom & Jerry <1>.CR3"),
            output: Some("JPEG quality 88"),
            edit,
        }
    }

    #[test]
    fn the_packet_says_what_it_should_and_escapes_what_it_must() {
        let m = metadata();
        let p = xmp(&provenance(&m, Some("{\"a\": \"<b>\"}")), None);
        assert!(p.starts_with("<?xpacket begin=\"\u{feff}\""));
        assert!(p.contains("<xmp:CreatorTool>greycard test</xmp:CreatorTool>"));
        assert!(
            p.contains("<xmp:ModifyDate>2027-01-15T08:00:00Z</xmp:ModifyDate>"),
            "{p}"
        );
        assert!(p.contains("<greycard:Source>Tom &amp; Jerry &lt;1&gt;.CR3</greycard:Source>"));
        assert!(p.contains("<greycard:Edit>{\"a\": \"&lt;b&gt;\"}</greycard:Edit>"));
        assert!(p.contains(&format!("xmlns:greycard=\"{NAMESPACE}\"")));
        // Too long for the limit: the edit goes, and the packet says so.
        let long = "x".repeat(1000);
        let short = xmp(&provenance(&m, Some(&long)), Some(800));
        assert!(!short.contains("<greycard:Edit>") && short.contains("<greycard:Note>"));
        assert!(short.len() <= 800, "{}", short.len());
        assert!(xmp(&provenance(&m, Some(&long)), Some(4000)).contains("<greycard:Edit>"));
    }

    /// The packet goes into a JPEG and a PNG the image crate wrote, and
    /// the image crate reads it back with the EXIF and the pixels intact.
    #[test]
    fn a_jpeg_and_a_png_take_the_packet() {
        use image::codecs::{jpeg::JpegEncoder, png::PngEncoder};
        use image::{ExtendedColorType, ImageDecoder, ImageEncoder};
        let m = metadata();
        let pv = provenance(&m, Some("{\"version\": 2}"));
        let exif = payload(&pv).unwrap();
        let px: Vec<u8> = (0..4 * 3 * 3).map(|i| (i * 7) as u8).collect();
        let mut jpeg = Vec::new();
        {
            let mut enc = JpegEncoder::new_with_quality(&mut jpeg, 95);
            enc.set_icc_profile(vec![1, 2, 3, 4]).unwrap();
            enc.set_exif_metadata(exif.clone()).unwrap();
            enc.write_image(&px, 4, 3, ExtendedColorType::Rgb8).unwrap();
        }
        let mut png = Vec::new();
        {
            let mut enc = PngEncoder::new(&mut png);
            enc.set_exif_metadata(exif).unwrap();
            enc.write_image(&px, 4, 3, ExtendedColorType::Rgb8).unwrap();
        }
        for (name, bytes) in [("jpeg", with_xmp(jpeg, &pv)), ("png", with_xmp(png, &pv))] {
            let mut dec = image::ImageReader::new(Cursor::new(&bytes))
                .with_guessed_format()
                .unwrap()
                .into_decoder()
                .unwrap();
            let xmp = String::from_utf8(dec.xmp_metadata().unwrap().expect(name)).unwrap();
            assert!(
                xmp.contains("<greycard:Edit>{\"version\": 2}</greycard:Edit>"),
                "{name}"
            );
            let exif = dec.exif_metadata().unwrap().expect("the EXIF stays");
            let (root, _) = read(&exif);
            assert_eq!(text(&root, ExifTag::Make).as_deref(), Some("Canon"));
            let back = image::DynamicImage::from_decoder(dec).unwrap().to_rgb8();
            assert_eq!((back.width(), back.height()), (4, 3), "{name}");
            if name == "png" {
                assert_eq!(back.as_raw(), &px);
            }
        }
        // Something else is left alone.
        assert_eq!(with_xmp(vec![1, 2, 3], &pv), vec![1, 2, 3]);
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn dates() {
        assert_eq!(date_time(0), "1970:01:01 00:00:00");
        assert_eq!(date_time(951_782_400), "2000:02:29 00:00:00");
        assert_eq!(date_time(951_782_400 + 86_399), "2000:02:29 23:59:59");
        assert_eq!(date_time(1_757_808_000), "2025:09:14 00:00:00");
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}
