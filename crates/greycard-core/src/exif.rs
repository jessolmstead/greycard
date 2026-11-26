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
//!
//! Who made the picture and who holds its rights are the user's to
//! say ([`Rights`]): when said, `Artist` and `Copyright` carry them
//! over whatever the camera wrote, and the XMP says the same as
//! `dc:creator` and `dc:rights`. Unsaid, the camera's (when not blank)
//! travel as the rest of its tags do, and the XMP says nothing of
//! them. They are the one thing an export with no other metadata still
//! carries ([`rights_payload`], [`rights_xmp`]): a statement the user
//! made, not a detail of the exposure.

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
    /// The user's author and copyright, over the camera's.
    pub rights: &'a Rights,
}

/// Who made a picture and who holds its rights, as the user says
/// them: each none when unsaid, and then the camera's tag, if any,
/// stands. [`Rights::new`] reads typed text as [`statement`] does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rights {
    /// EXIF `Artist`, XMP `dc:creator`. Several people go in as one
    /// text split by `;`, as the Metadata Working Group has it: the
    /// EXIF tag keeps the text, the XMP's sequence takes each name.
    pub author: Option<String>,
    /// EXIF `Copyright`, XMP `dc:rights` in its default language.
    pub copyright: Option<String>,
}

/// No author and no copyright: a picture the user said nothing of,
/// which carries the camera's own.
pub static NO_RIGHTS: Rights = Rights {
    author: None,
    copyright: None,
};

impl Rights {
    pub fn new(author: &str, copyright: &str) -> Self {
        Self {
            // A list of no names is no author.
            author: statement(author).filter(|a| a.split(';').any(|n| !n.trim().is_empty())),
            copyright: statement(copyright),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.author.is_none() && self.copyright.is_none()
    }

    /// The names `author` lists, for `dc:creator`'s sequence.
    fn creators(&self) -> Vec<&str> {
        self.author
            .as_deref()
            .map(|a| {
                a.split(';')
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The most of a statement kept, in bytes: a notice is a line, and
/// both tags, in the EXIF and the XMP, still leave a JPEG's 64 KB
/// segments room.
pub const STATEMENT_MAX: usize = 1024;

/// Text the user gave for an author or a copyright, as it is written:
/// control characters (a newline, a tab, a NUL that would end the EXIF
/// string early) and the two XML has no place for (U+FFFE, U+FFFF) as
/// spaces, trimmed, and cut on a character's boundary
/// at [`STATEMENT_MAX`] bytes. None for nothing left. UTF-8 throughout:
/// the XMP is, and the EXIF's ASCII tags carry the same bytes, which
/// exiv2 and the image crate read back as written.
pub fn statement(text: &str) -> Option<String> {
    let spaced: String = text
        .chars()
        .map(|c| {
            if c.is_control() || !xml_char(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut s = spaced.trim();
    if s.len() > STATEMENT_MAX {
        let mut end = STATEMENT_MAX;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s = s[..end].trim_end();
    }
    (!s.is_empty()).then(|| s.to_string())
}

/// The Dublin Core namespace, `dc:creator`'s and `dc:rights`'.
const DC: &str = "http://purl.org/dc/elements/1.1/";

/// The `dc:` properties for `rights`, at the packet's indent; empty for
/// none.
fn dc_properties(rights: &Rights) -> String {
    let mut p = String::new();
    let creators = rights.creators();
    if !creators.is_empty() {
        p.push_str("   <dc:creator>\n    <rdf:Seq>\n");
        for c in creators {
            p.push_str(&format!("     <rdf:li>{}</rdf:li>\n", escape(c)));
        }
        p.push_str("    </rdf:Seq>\n   </dc:creator>\n");
    }
    if let Some(c) = &rights.copyright {
        p.push_str("   <dc:rights>\n    <rdf:Alt>\n");
        p.push_str(&format!(
            "     <rdf:li xml:lang=\"x-default\">{}</rdf:li>\n",
            escape(c)
        ));
        p.push_str("    </rdf:Alt>\n   </dc:rights>\n");
    }
    p
}

/// The XMP packet of an export that carries nothing else: the author
/// and the copyright alone. None when neither is said.
pub fn rights_xmp(rights: &Rights) -> Option<String> {
    if rights.is_empty() {
        return None;
    }
    let mut p = String::new();
    p.push_str("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    p.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n");
    p.push_str(" <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
    p.push_str("  <rdf:Description rdf:about=\"\"\n");
    p.push_str(&format!("    xmlns:dc=\"{DC}\">\n"));
    p.push_str(&dc_properties(rights));
    p.push_str("  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>");
    Some(p)
}

/// The EXIF payload of an export that carries nothing else, laid out
/// as [`payload`]'s: `Artist` and `Copyright` in the root directory and
/// no other tag. None when neither is said.
pub fn rights_payload(rights: &Rights) -> Result<Option<Vec<u8>>> {
    if rights.is_empty() {
        return Ok(None);
    }
    let mut buffer = Cursor::new(Vec::new());
    {
        let tiff = TiffWriter::new(&mut buffer).map_err(encode)?;
        let mut root = DirectoryWriter::new();
        put_rights(&mut root, rights);
        tiff.build(root).map_err(encode)?;
    }
    Ok(Some(buffer.into_inner()))
}

/// The said author and copyright into the root directory, over
/// whatever is there.
fn put_rights(root: &mut DirectoryWriter, rights: &Rights) {
    if let Some(a) = &rights.author {
        root.add_tag(ExifTag::Artist, a.as_str());
    }
    if let Some(c) = &rights.copyright {
        root.add_tag(ExifTag::Copyright, c.as_str());
    }
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
        if !provenance.rights.is_empty() {
            p.push_str(&format!("    xmlns:dc=\"{DC}\"\n"));
        }
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
        p.push_str(&dc_properties(provenance.rights));
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
/// A character XML 1.0 has no place for (a C0 control but the tab, the
/// line feed, and the carriage return; U+FFFE; U+FFFF) is a space: one
/// would make the whole packet ill-formed, and a reader drop all of it.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c if !xml_char(c) => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Whether XML 1.0's `Char` production takes `c`. A Rust `char` holds
/// no surrogate, so what it leaves out is the C0 controls but tab, line
/// feed, and carriage return, and U+FFFE and U+FFFF.
fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{FFFD}' | '\u{10000}'..)
}

/// The picture with its XMP packet in: a JPEG's APP1 segment after
/// the ones the encoder wrote, or a PNG's `iTXt` chunk after its
/// header. Anything else comes back as it was.
pub fn with_xmp(picture: Vec<u8>, provenance: &Provenance<'_>) -> Vec<u8> {
    with_packet(picture, |limit| xmp(provenance, limit))
}

/// [`with_xmp`] of the packet `packet` makes, which is handed the most
/// the container holds (a JPEG's segment) or none.
pub fn with_packet(picture: Vec<u8>, packet: impl FnOnce(Option<usize>) -> String) -> Vec<u8> {
    if picture.starts_with(&[0xFF, 0xD8]) {
        let packet = packet(Some(JPEG_PACKET_LIMIT));
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
        let packet = packet(None);
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
    let tags = match provenance {
        Some(p) => Tags::Provenance(p),
        None => Tags::Nothing,
    };
    write_rgb16_tiff_tagged(out, width, height, data, icc, tags)
}

/// What a file says beside its pixels and profile.
#[derive(Debug, Clone, Copy)]
pub enum Tags<'a> {
    /// No EXIF, no XMP.
    Nothing,
    /// The source's tags and the export's own, with the user's rights
    /// over them ([`payload`], [`xmp`]).
    Provenance(&'a Provenance<'a>),
    /// The user's author and copyright and nothing else
    /// ([`rights_payload`], [`rights_xmp`]).
    Rights(&'a Rights),
}

/// [`write_rgb16_tiff`] with the file's tags as [`Tags`] says.
pub fn write_rgb16_tiff_tagged<W: Write + Seek>(
    out: W,
    width: u32,
    height: u32,
    data: &[u16],
    icc: Option<&[u8]>,
    tags: Tags<'_>,
) -> Result<()> {
    write_16_tiff(out, (width, height), 3, data, icc, tags)
}

/// [`write_rgb16_tiff_tagged`] of one channel, black at 0.
pub fn write_grey16_tiff_tagged<W: Write + Seek>(
    out: W,
    width: u32,
    height: u32,
    data: &[u16],
    icc: Option<&[u8]>,
    tags: Tags<'_>,
) -> Result<()> {
    write_16_tiff(out, (width, height), 1, data, icc, tags)
}

/// A 16-bit TIFF of three channels, RGB, or of one.
fn write_16_tiff<W: Write + Seek>(
    out: W,
    (width, height): (u32, u32),
    channels: usize,
    data: &[u16],
    icc: Option<&[u8]>,
    tags: Tags<'_>,
) -> Result<()> {
    let (w, h) = (width as usize, height as usize);
    if data.len() != w * h * channels {
        return Err(Error::Encode(format!(
            "{}x{} {} wants {} samples, got {}",
            width,
            height,
            if channels == 1 { "grey16" } else { "RGB16" },
            w * h * channels,
            data.len()
        )));
    }
    if w == 0 || h == 0 {
        return Err(Error::Encode("an empty picture".into()));
    }
    let mut tiff = TiffWriter::new(out).map_err(encode)?;
    let mut root = DirectoryWriter::new();
    let mut exif = DirectoryWriter::new();

    let differenced = horizontal_differences(data, w * channels, channels);
    let (rows_per_strip, strips) = tiff
        .write_strips_lzw(&differenced, channels, Dim2 { w, h }, 0)
        .map_err(encode)?;
    let offsets: Vec<u32> = strips.iter().map(|s| s.0).collect();
    let counts: Vec<u32> = strips.iter().map(|s| s.1).collect();
    root.add_tag(TiffCommonTag::ImageWidth, width);
    root.add_tag(TiffCommonTag::ImageLength, height);
    root.add_tag(TiffCommonTag::BitsPerSample, &[16u16; 3][..channels]);
    // 5 is LZW; 2 is RGB, and 1 grey with black at 0.
    root.add_tag(TiffCommonTag::Compression, 5u16);
    root.add_tag(
        TiffCommonTag::PhotometricInt,
        if channels == 1 { 1u16 } else { 2 },
    );
    root.add_tag(TiffCommonTag::StripOffsets, &offsets);
    root.add_tag(TiffCommonTag::SamplesPerPixel, channels as u16);
    root.add_tag(TiffCommonTag::RowsPerStrip, rows_per_strip);
    root.add_tag(TiffCommonTag::StripByteCounts, &counts);
    root.add_tag(ExifTag::PlanarConfiguration, 1u16);
    root.add_tag(TiffCommonTag::Predictor, 2u16);
    if let Some(icc) = icc {
        root.add_tag_undefined(ExifTag::IccProfile, icc.to_vec());
    }

    match tags {
        Tags::Nothing => {}
        Tags::Provenance(provenance) => {
            fill(&mut tiff, &mut root, &mut exif, provenance)?;
            let exif_offset = exif.build(&mut tiff).map_err(encode)?;
            root.add_tag(TiffCommonTag::ExifIFDPointer, exif_offset);
            root.add_untyped_tag(XMP_TAG, Value::Byte(xmp(provenance, None).into_bytes()));
        }
        Tags::Rights(rights) => {
            put_rights(&mut root, rights);
            if let Some(packet) = rights_xmp(rights) {
                root.add_untyped_tag(XMP_TAG, Value::Byte(packet.into_bytes()));
            }
        }
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
    // The user's word over the camera's.
    put_rights(root, provenance.rights);
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
    date_time_at(secs.min(i64::MAX as u64) as i64)
}

/// [`date_time`] for a time either side of the epoch: what a capture
/// time moved by a shift is written back as.
pub fn date_time_at(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}:{m:02}:{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

/// A capture time as seconds since the epoch, the wall clock read as
/// though it were UTC: a camera's clock keeps no zone, and a shift of
/// it is a count of seconds whatever the zone was. Takes EXIF's
/// `YYYY:MM:DD HH:MM:SS`, and the index's and ISO 8601's dashes with a
/// space or a `T` before the time; whatever follows the seconds (a
/// fraction, a zone) is not read, and a caller that wants none checks
/// the length. A date with no time is its midnight. None for anything
/// that is not a date: a day the month has not got, a date spelled
/// with one colon and one dash.
pub fn seconds_of(date: &str) -> Option<i64> {
    let s = date.trim().as_bytes();
    let digits = |from: usize, len: usize| -> Option<i64> {
        let part = s.get(from..from + len)?;
        part.iter()
            .all(u8::is_ascii_digit)
            .then(|| part.iter().fold(0i64, |n, d| n * 10 + i64::from(d - b'0')))
    };
    let sep = |at: usize, allowed: &[u8]| s.get(at).is_some_and(|c| allowed.contains(c));
    // The date's two separators one spelling: EXIF's colons or the
    // dashes, never one of each.
    if !sep(4, b":-") || s.get(7) != s.get(4) {
        return None;
    }
    let (y, m, d) = (digits(0, 4)?, digits(5, 2)?, digits(8, 2)?);
    if y == 0 || !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m as u32) {
        return None;
    }
    let (hh, mm, ss) = if s.len() == 10 {
        (0, 0, 0)
    } else {
        if !sep(10, b" T") || !sep(13, b":") || !sep(16, b":") {
            return None;
        }
        (digits(11, 2)?, digits(14, 2)?, digits(17, 2)?)
    };
    if hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    Some(days_from_civil(y, m as u32, d as u32) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// `date` (as [`seconds_of`] reads it) moved by `by` seconds, in
/// EXIF's form. None when `date` is not a date.
pub fn shifted(date: &str, by: i64) -> Option<String> {
    Some(date_time_at(seconds_of(date)?.saturating_add(by)))
}

/// EXIF's `YYYY:MM:DD HH:MM:SS` as ISO 8601's `YYYY-MM-DDTHH:MM:SS`,
/// with no zone, since the camera gave none: the form XMP's dates
/// take. Anything not in EXIF's form comes back as it was.
pub fn iso_8601(exif: &str) -> String {
    let b = exif.as_bytes();
    if b.len() != 19 || b[4] != b':' || b[7] != b':' || b[10] != b' ' {
        return exif.to_string();
    }
    format!(
        "{}-{}-{}T{}",
        &exif[0..4],
        &exif[5..7],
        &exif[8..10],
        &exif[11..]
    )
}

/// `metadata` with the capture times it carries moved by `by`
/// seconds: `DateTimeOriginal` and `CreateDate` (EXIF's
/// `DateTimeDigitized`), each where it is there and is a date. What an
/// export of a frame whose camera's clock was off carries, so the
/// picture says when it was taken and not what the clock said. The
/// modify date is the export's own and [`payload`] sets it anyway.
pub fn with_capture_shifted(metadata: &RawMetadata, by: i64) -> RawMetadata {
    let mut out = metadata.clone();
    if by == 0 {
        return out;
    }
    for date in [&mut out.exif.date_time_original, &mut out.exif.create_date] {
        if let Some(moved) = date.as_deref().and_then(|d| shifted(d, by)) {
            *date = Some(moved);
        }
    }
    out
}

/// A lens as an export names it: who made it, when that is known, and
/// its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensName {
    pub make: Option<String>,
    pub model: String,
}

/// `metadata` naming `lens` where the source names no lens: the lens
/// the engine resolved for the file, or the one picked by hand, as
/// `LensModel`, and its maker as `LensMake` where the source has none.
/// A source's own `LensModel` is never written over, nor its
/// `LensMake`: the camera's word stands. Every writer of an export's
/// tags ([`payload`], [`write_rgb16_tiff`], the linear DNG) carries the
/// metadata's lens, so naming it here names it in each.
pub fn name_lens(metadata: &mut RawMetadata, lens: &LensName) {
    let blank = |s: &Option<String>| s.as_deref().is_none_or(|s| s.trim().is_empty());
    let model = lens.model.trim();
    if model.is_empty() || !blank(&metadata.exif.lens_model) {
        return;
    }
    metadata.exif.lens_model = Some(model.to_string());
    if blank(&metadata.exif.lens_make)
        && let Some(make) = lens
            .make
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
    {
        metadata.exif.lens_make = Some(make.to_string());
    }
}

/// The days in month `m` (1 to 12) of year `y`, by the Gregorian rule.
fn days_in_month(y: i64, m: u32) -> i64 {
    match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// A proleptic Gregorian date to days since 1970-01-01: the inverse
/// of [`civil_from_days`], by the same author.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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
            rights: &NO_RIGHTS,
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

    /// A source that names no lens is given the lens named: in the
    /// payload (a JPEG's, a PNG's, a WebP's) and in a TIFF, `LensModel`
    /// and `LensMake`. A source's own is never written over.
    #[test]
    fn the_lens_named_goes_where_the_source_names_none() {
        let fa = LensName {
            make: Some("Pentax".into()),
            model: "smc PENTAX-FA 35mm F2 AL".into(),
        };
        let provenance = |m| Provenance {
            metadata: m,
            software: "greycard test",
            width: 2,
            height: 1,
            srgb: true,
            written: None,
            source_name: None,
            output: None,
            edit: None,
            rights: &NO_RIGHTS,
        };
        let lens_of = |exif: &IFD| {
            (
                text(exif, ExifTag::LensMake),
                text(exif, ExifTag::LensModel),
            )
        };
        let mut none = metadata();
        none.exif.lens_model = None;
        name_lens(&mut none, &fa);
        let named = (
            Some("Pentax".into()),
            Some("smc PENTAX-FA 35mm F2 AL".into()),
        );
        let (_, exif) = read(&payload(&provenance(&none)).unwrap());
        assert_eq!(lens_of(&exif), named);
        let mut tiff = Cursor::new(Vec::new());
        write_rgb16_tiff(&mut tiff, 2, 1, &[0; 6], None, Some(&provenance(&none))).unwrap();
        let (_, exif) = read(&tiff.into_inner());
        assert_eq!(lens_of(&exif), named);

        // The source's lens stands, and its blank maker is not filled
        // from another lens's.
        let mut own = metadata();
        name_lens(&mut own, &fa);
        let (_, exif) = read(&payload(&provenance(&own)).unwrap());
        assert_eq!(lens_of(&exif), (None, Some("RF50mm F1.8 STM".into())));
        // A blank tag is no lens, and a blank name names none.
        let mut blank = metadata();
        blank.exif.lens_model = Some("  ".into());
        blank.exif.lens_make = Some("Sigma".into());
        name_lens(&mut blank, &fa);
        assert_eq!(
            (
                blank.exif.lens_make.as_deref(),
                blank.exif.lens_model.as_deref()
            ),
            (Some("Sigma"), Some("smc PENTAX-FA 35mm F2 AL"))
        );
        let mut unnamed = metadata();
        unnamed.exif.lens_model = None;
        name_lens(
            &mut unnamed,
            &LensName {
                make: Some("Pentax".into()),
                model: " ".into(),
            },
        );
        assert_eq!(
            (unnamed.exif.lens_make, unnamed.exif.lens_model),
            (None, None)
        );
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
            rights: &NO_RIGHTS,
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
                rights: &NO_RIGHTS,
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

    /// A grey TIFF is one channel, read back sample for sample with its
    /// profile and its tags.
    #[test]
    fn a_grey_tiff_is_one_channel() {
        let (w, h) = (37u32, 300u32);
        let data: Vec<u16> = (0..w * h)
            .map(|i| (i.wrapping_mul(2654435761) >> 8) as u16)
            .collect();
        let m = metadata();
        let icc = b"not really a profile".to_vec();
        let mut file = Cursor::new(Vec::new());
        let provenance = Provenance {
            metadata: &m,
            software: "greycard test",
            width: w,
            height: h,
            srgb: false,
            written: None,
            source_name: Some("a.dng"),
            output: None,
            edit: None,
            rights: &NO_RIGHTS,
        };
        let tags = Tags::Provenance(&provenance);
        write_grey16_tiff_tagged(&mut file, w, h, &data, Some(&icc), tags).unwrap();
        let bytes = file.into_inner();
        match image::load_from_memory(&bytes).unwrap() {
            image::DynamicImage::ImageLuma16(back) => {
                assert_eq!((back.width(), back.height()), (w, h));
                assert_eq!(back.as_raw(), &data);
            }
            other => panic!("{:?}", other.color()),
        }
        let (root, exif) = read(&bytes);
        let bits = root.get_entry(TiffCommonTag::BitsPerSample).unwrap();
        assert_eq!((bits.count(), bits.force_u16(0)), (1, 16));
        let kind = root.get_entry(TiffCommonTag::PhotometricInt).unwrap();
        assert_eq!(kind.force_u16(0), 1);
        match &root.get_entry(ExifTag::IccProfile).unwrap().value {
            Value::Undefined(v) => assert_eq!(v, &icc),
            other => panic!("{other:?}"),
        }
        assert!(text(&exif, ExifTag::LensModel).is_some());
        let short = Cursor::new(Vec::new());
        assert!(write_grey16_tiff_tagged(short, w, h, &data[1..], None, Tags::Nothing).is_err());
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
            rights: &NO_RIGHTS,
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

    #[test]
    fn a_capture_time_reads_in_every_spelling_and_moves_by_seconds() {
        // EXIF's, the index's and ISO 8601's spellings are one time.
        let exif = seconds_of("2026:09:01 10:20:30").unwrap();
        assert_eq!(seconds_of("2026-09-01 10:20:30"), Some(exif));
        assert_eq!(seconds_of("2026-09-01T10:20:30"), Some(exif));
        assert_eq!(seconds_of("2026-09-01T10:20:30.45+02:00"), Some(exif));
        assert_eq!(date_time_at(exif), "2026:09:01 10:20:30");
        assert_eq!(
            seconds_of("2026-09-01"),
            Some(exif - 10 * 3600 - 20 * 60 - 30)
        );
        // Round trips across the calendar, either side of the epoch.
        for secs in [0, -1, 951_782_400, 4_102_444_799, -2_208_988_800] {
            assert_eq!(seconds_of(&date_time_at(secs)), Some(secs), "{secs}");
        }
        // Five minutes on, across midnight and a leap day, and back
        // across a new year.
        assert_eq!(
            shifted("2024:02:28 23:58:00", 300).as_deref(),
            Some("2024:02:29 00:03:00")
        );
        assert_eq!(
            shifted("2026-01-01 00:02:00", -300).as_deref(),
            Some("2025:12:31 23:57:00")
        );
        // A clock that wrote nothing, or nonsense, is no date.
        for bad in [
            "",
            "    :  :     :  :  ",
            "0000:00:00 00:00:00",
            "2026:13:01 00:00:00",
            "2026:09:01 25:00:00",
            "2026:09:01 10:20",
            "2026/09/01 10:20:30",
            // No such day, and one of each separator.
            "2026-02-30 10:00:00",
            "2026-02-29",
            "2026-04-31",
            "2026:09-01 10:20:30",
            "2026-09:01",
        ] {
            assert_eq!(seconds_of(bad), None, "{bad:?}");
        }
        // A leap year's last of February is a day.
        assert!(seconds_of("2024-02-29 12:00:00").is_some());
        assert!(seconds_of("2000:02:29 12:00:00").is_some());
        assert_eq!(seconds_of("1900:02:29 12:00:00"), None);
        assert_eq!(iso_8601("2026:09:01 10:20:30"), "2026-09-01T10:20:30");
        assert_eq!(iso_8601("2026-09-01"), "2026-09-01");
    }

    #[test]
    fn a_shifted_export_carries_the_corrected_capture_time() {
        let mut m = metadata();
        m.exif.create_date = Some("2026:09:01 10:20:30".into());
        let moved = with_capture_shifted(&m, 5 * 60);
        let bytes = payload(&Provenance {
            metadata: &moved,
            software: "greycard test",
            width: 1,
            height: 1,
            srgb: true,
            written: None,
            source_name: None,
            output: None,
            edit: None,
            rights: &NO_RIGHTS,
        })
        .unwrap();
        let (_, exif) = read(&bytes);
        assert_eq!(
            text(&exif, ExifTag::DateTimeOriginal).as_deref(),
            Some("2026:09:01 10:25:30")
        );
        assert_eq!(
            text(&exif, ExifTag::CreateDate).as_deref(),
            Some("2026:09:01 10:25:30")
        );
        // No shift is the source as it was, and a missing date stays
        // missing.
        assert_eq!(with_capture_shifted(&m, 0), m);
        m.exif.create_date = None;
        assert_eq!(with_capture_shifted(&m, 60).exif.create_date, None);
    }

    /// Text given for a right: trimmed, its control characters spaces,
    /// cut on a character at the cap, and nothing for blank.
    #[test]
    fn a_statement_is_read_as_written_text() {
        assert_eq!(statement("  Jo Doe \n").as_deref(), Some("Jo Doe"));
        assert_eq!(statement("a\tb\0c").as_deref(), Some("a b c"));
        assert_eq!(
            statement("© Jo\u{FFFE}Doe\u{FFFF}\u{1}").as_deref(),
            Some("© Jo Doe")
        );
        assert_eq!(statement(""), None);
        assert_eq!(statement(" \n\t "), None);
        // Two-byte characters across the cap: cut before the one that
        // would straddle it.
        let long = "é".repeat(STATEMENT_MAX);
        let cut = statement(&long).unwrap();
        assert_eq!(cut.len(), STATEMENT_MAX);
        assert!(cut.chars().all(|c| c == 'é'));
        let odd = format!("x{long}");
        assert_eq!(statement(&odd).unwrap().len(), STATEMENT_MAX - 1);
        // A list of no names is no author; a notice is a notice.
        assert_eq!(Rights::new(" ; ;", ";").author, None);
        assert_eq!(Rights::new(" ; ;", ";").copyright.as_deref(), Some(";"));
        assert!(Rights::new("", " ").is_empty());
    }

    /// Text with characters XML 1.0 has no place for, in the rights and
    /// in the source's name, still makes a packet a strict reader parses,
    /// each of them a space; ordinary text is escaped as it always was.
    #[test]
    fn a_packet_of_text_xml_refuses_still_parses() {
        let m = metadata();
        let odd = "Jo\u{FFFE}Doe\u{FFFF} & <co>\u{1}\u{1F}";
        let r = Rights::new(odd, odd);
        let p = Provenance {
            source_name: Some("IMG\u{FFFF}\u{2}_0001.CR3"),
            rights: &r,
            ..provenance(&m, Some("{\"a\": \"\u{FFFE}\"}"))
        };
        for packet in [xmp(&p, None), rights_xmp(&r).unwrap()] {
            let doc =
                roxmltree::Document::parse(&packet).unwrap_or_else(|e| panic!("{e}: {packet}"));
            let text: Vec<&str> = doc
                .descendants()
                .filter(|n| n.has_tag_name("li"))
                .filter_map(|n| n.text())
                .collect();
            assert!(text.contains(&"Jo Doe  & <co>"), "{text:?}");
        }
        let packet = xmp(&p, None);
        assert!(packet.contains("<greycard:Source>IMG  _0001.CR3</greycard:Source>"));
        assert_eq!(escape("Tom & Jerry\té\n"), "Tom &amp; Jerry\té\n");
    }

    fn rights() -> Rights {
        Rights::new(
            "Zoë Ångström; Jo Doe",
            "© 2026 Zoë Ångström & co <all rights>",
        )
    }

    /// Said, the user's author and copyright replace the camera's in the
    /// EXIF and are the XMP's `dc:creator` and `dc:rights`; unsaid, the
    /// camera's stand and the packet is as it was.
    #[test]
    fn the_users_rights_go_over_the_cameras() {
        let mut m = metadata();
        m.exif.artist = Some("Camera Owner".into());
        m.exif.copyright = Some("camera notice".into());
        let r = rights();
        let mine = Provenance {
            rights: &r,
            ..provenance(&m, None)
        };
        let (root, _) = read(&payload(&mine).unwrap());
        assert_eq!(
            text(&root, ExifTag::Artist).as_deref(),
            Some("Zoë Ångström; Jo Doe")
        );
        assert_eq!(
            text(&root, ExifTag::Copyright).as_deref(),
            Some("© 2026 Zoë Ångström & co <all rights>")
        );
        let p = xmp(&mine, None);
        assert!(p.contains(&format!("xmlns:dc=\"{DC}\"")), "{p}");
        assert!(p.contains(
            "<dc:creator>\n    <rdf:Seq>\n     <rdf:li>Zoë Ångström</rdf:li>\n     <rdf:li>Jo Doe</rdf:li>\n    </rdf:Seq>\n   </dc:creator>"
        ));
        assert!(p.contains(
            "<rdf:li xml:lang=\"x-default\">© 2026 Zoë Ångström &amp; co &lt;all rights&gt;</rdf:li>"
        ));

        // The author alone: the camera's notice stands in the EXIF, and
        // the XMP says only what the user did.
        let author = Rights::new("Jo Doe", "");
        let (root, _) = read(
            &payload(&Provenance {
                rights: &author,
                ..provenance(&m, None)
            })
            .unwrap(),
        );
        assert_eq!(text(&root, ExifTag::Artist).as_deref(), Some("Jo Doe"));
        assert_eq!(
            text(&root, ExifTag::Copyright).as_deref(),
            Some("camera notice")
        );

        // Nothing said: the camera's, and no `dc:` in the packet.
        let theirs = provenance(&m, None);
        let (root, _) = read(&payload(&theirs).unwrap());
        assert_eq!(
            text(&root, ExifTag::Artist).as_deref(),
            Some("Camera Owner")
        );
        assert_eq!(
            text(&root, ExifTag::Copyright).as_deref(),
            Some("camera notice")
        );
        assert!(!xmp(&theirs, None).contains("dc"));
    }

    /// With nothing else to carry, the rights alone: the two tags in
    /// the root, no EXIF directory, a packet of `dc:` alone; nothing
    /// at all when none is said.
    #[test]
    fn the_rights_alone() {
        let r = rights();
        let bytes = rights_payload(&r).unwrap().expect("a payload");
        let reader = GenericTiffReader::new_with_buffer(&bytes, 0, 0, None).unwrap();
        let root = reader.root_ifd();
        assert_eq!(
            text(root, ExifTag::Artist).as_deref(),
            Some("Zoë Ångström; Jo Doe")
        );
        assert!(root.get_entry(ExifTag::Copyright).is_some());
        assert_eq!(root.entries().len(), 2, "{root:?}");
        let p = rights_xmp(&r).unwrap();
        assert!(p.contains("<rdf:li>Jo Doe</rdf:li>") && !p.contains("greycard"));
        assert!(rights_payload(&NO_RIGHTS).unwrap().is_none());
        assert!(rights_xmp(&NO_RIGHTS).is_none());

        // A TIFF of them.
        let mut file = Cursor::new(Vec::new());
        write_rgb16_tiff_tagged(&mut file, 2, 2, &[0u16; 12], None, Tags::Rights(&r)).unwrap();
        let bytes = file.into_inner();
        let root = GenericTiffReader::new_with_buffer(&bytes, 0, 0, None)
            .unwrap()
            .root_ifd()
            .clone();
        assert_eq!(
            text(&root, ExifTag::Copyright).as_deref(),
            Some("© 2026 Zoë Ångström & co <all rights>")
        );
        assert!(root.get_entry(TiffCommonTag::ExifIFDPointer).is_none());
        // The packet as tag 700 (the image crate sizes its buffers by
        // the pixels and refuses a packet this long on a 2 by 2).
        match &root.get_entry(XMP_TAG).expect("a packet").value {
            Value::Byte(b) => assert_eq!(b, &rights_xmp(&r).unwrap().into_bytes()),
            other => panic!("{other:?}"),
        }
    }
}
