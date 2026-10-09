//! The maker's rendering settings on a raw: the picture style or film
//! simulation the camera's own JPEG was rendered with, the
//! scene-adaptive settings that bend it frame by frame, and whether the
//! camera corrected the lens's vignetting.
//!
//! The camera match groups frames by these and fits only the frames a
//! fixed style rendered, so each value here is read from the maker note
//! as exiv2 reads it, and checked against exiv2 on real files. rawler
//! decodes the file but does not expose the maker note's contents, so
//! a CR3's boxes and a RAF's JPEG are walked here with rawler's own
//! `formats::bmff` and `formats::tiff` parsers, and the TIFF-based raws
//! (NEF, ARW, RW2) with a small IFD reader of our own that holds every
//! count and offset to the file before it reads or allocates.
//!
//! Canon CR3, Fujifilm RAF, Nikon NEF, Sony ARW and Panasonic RW2 are
//! read: the style, the scene-adaptive settings and the vignetting
//! flag. Any other TIFF raw of those makers (a Canon CR2, or a DNG,
//! whose preview is its converter's rendering rather than the camera's)
//! is recognized and named, with no style.
//!
//! A setting the reader expects and cannot find or cannot name counts
//! as on, marked `known: false`, so a frame is never called fixed on a
//! guess. What the reader does not see yet: the per-style tweaks that
//! change the rendering without adapting to the scene, Canon's
//! Contrast, Saturation and Color tone for the style, Fujifilm's Color
//! (saturation) beside a film simulation, Color Chrome Effect, Color
//! Chrome FX Blue and Clarity, a Nikon Picture Control's fixed
//! sharpening, contrast, brightness, saturation and hue steps, Sony's
//! contrast, saturation, sharpness and Creative Look fade, shadows and
//! highlights, and a Panasonic Photo Style's contrast, saturation and
//! the like. A group may mix frames that differ in them.
//!
//! The tag numbers, array indexes and the names of their values follow
//! exiv2's maker note tables, `src/canonmn_int.cpp`, `src/fujimn_int.cpp`,
//! `src/nikonmn_int.cpp`, `src/sonymn_int.cpp` and
//! `src/panasonicmn_int.cpp` at <https://github.com/Exiv2/exiv2>,
//! GPL-2.0-or-later, copyright the Exiv2 authors. The names were taken
//! from exiv2's own output on real and exiv2-written files. The layout
//! of the second and third versions of Nikon's Picture Control record,
//! which exiv2 reads with the first version's offsets, follows
//! ExifTool's `lib/Image/ExifTool/Nikon.pm` (PictureControl2 and
//! PictureControl3), and Panasonic's Filter Settings tag, which exiv2
//! does not list, follows `lib/Image/ExifTool/Panasonic.pm`
//! (FilterEffect), copyright Phil Harvey, distributed under the same
//! terms as Perl (the GPL or the Artistic License); both were checked
//! on the bytes of real files. The reading code is ours.
//!
//! A setting on that renders in place of the style (a scene mode, a
//! picture effect, an in-camera HDR) takes the frame out of its
//! style's group, as a setting not known does; one that bends the
//! style's tone per scene leaves it in, not fixed
//! ([`AdaptiveKind`]).

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use ::rawler::bits::Endian;
use ::rawler::formats::bmff::{BmffError, FileBox};
use ::rawler::formats::tiff::reader::TiffReader;
use ::rawler::formats::tiff::{IFD, Value};

use super::rawler_backend::{guarded, tail_of};
use crate::error::{Error, Result};

/// Who made the camera, as far as the reader tells makers apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Maker {
    Canon,
    Fujifilm,
    Nikon,
    Sony,
    Panasonic,
    /// Any other maker. [`CameraStyle::read`] returns `None` for one.
    Other,
}

impl Maker {
    /// The maker named by an EXIF `Make` string.
    pub fn from_make(make: &str) -> Maker {
        let make = make.trim().to_ascii_lowercase();
        if make.starts_with("canon") {
            Maker::Canon
        } else if make.starts_with("fujifilm") {
            Maker::Fujifilm
        } else if make.starts_with("nikon") {
            Maker::Nikon
        } else if make.starts_with("sony") {
            Maker::Sony
        } else if make.starts_with("panasonic") {
            Maker::Panasonic
        } else {
            Maker::Other
        }
    }

    /// The maker's name as a group key begins with it.
    pub fn name(self) -> &'static str {
        match self {
            Maker::Canon => "Canon",
            Maker::Fujifilm => "Fujifilm",
            Maker::Nikon => "Nikon",
            Maker::Sony => "Sony",
            Maker::Panasonic => "Panasonic",
            Maker::Other => "Other",
        }
    }
}

/// What an adaptive setting does to the camera's rendering when it is
/// on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveKind {
    /// Bends the style's tone per scene and leaves the style in charge:
    /// Canon's Auto Lighting Optimizer and Highlight Tone Priority,
    /// Fujifilm's Dynamic Range, Nikon's Active D-Lighting, Sony's DRO,
    /// Panasonic's Intelligent D-Range. A frame with one on is still of
    /// its style's group, and fills a group short of fixed frames.
    Tone,
    /// Renders in place of the style, or draws over it: a scene or
    /// intelligent auto mode, a picture effect or filter, an in-camera
    /// HDR. A frame with one on is of no group.
    Replaces,
}

/// One of the maker's scene-adaptive settings and whether it was on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adaptive {
    pub name: &'static str,
    pub on: bool,
    /// False when the file did not carry the setting or carried a value
    /// the reader cannot name; `on` is then true, and the frame is of
    /// no group.
    pub known: bool,
    pub kind: AdaptiveKind,
}

impl Adaptive {
    fn known(name: &'static str, on: bool) -> Adaptive {
        Adaptive {
            name,
            on,
            known: true,
            kind: AdaptiveKind::Tone,
        }
    }

    fn unknown(name: &'static str) -> Adaptive {
        Adaptive {
            name,
            on: true,
            known: false,
            kind: AdaptiveKind::Tone,
        }
    }

    /// The same setting, one that replaces the style.
    fn replacing(self) -> Adaptive {
        Adaptive {
            kind: AdaptiveKind::Replaces,
            ..self
        }
    }

    /// Whether this setting takes the frame out of its style's group:
    /// one that replaces the style and is on, or one not known.
    fn ungroups(&self) -> bool {
        !self.known || (self.on && self.kind == AdaptiveKind::Replaces)
    }
}

/// The rendering settings a raw carries for its camera JPEG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraStyle {
    pub maker: Maker,
    /// The picture style or film simulation as the maker names it:
    /// "Faithful", "Standard", "Auto", "Reala Ace", "Provia". A code
    /// the reader has no name for is `Unknown (code)`. `None` when the
    /// file does not say or the maker's tags are not read.
    pub style: Option<String>,
    /// Whether [`CameraStyle::style`] renders every frame the same
    /// way: false for a style that adapts to the scene itself (Canon's
    /// Auto, or a user style built on it) and for one the reader cannot
    /// name.
    pub style_fixed: bool,
    /// The preset a user-defined style was built on, where the maker
    /// says: Canon's User Def. 1 to 3.
    pub base: Option<String>,
    /// Each scene-adaptive setting the maker has and whether it was on,
    /// in the maker's order. One the file leaves out or gives a value
    /// the reader cannot name is listed on and not known. Fujifilm's
    /// highlight and shadow tone are listed only when they were moved
    /// off zero.
    pub adaptive: Vec<Adaptive>,
    /// The camera's own vignetting correction, where the file says.
    pub peripheral_correction: Option<bool>,
}

impl CameraStyle {
    fn bare(maker: Maker) -> CameraStyle {
        CameraStyle {
            maker,
            style: None,
            style_fixed: false,
            base: None,
            adaptive: Vec::new(),
            peripheral_correction: None,
        }
    }

    /// The rendering settings of the raw at `path`. `None` for a TIFF
    /// raw whose maker is not one of [`Maker`]'s named ones, and for a
    /// file that is neither a CR3, a RAF nor TIFF-based. An error is a
    /// file that could not be read, a CR3, RAF, NEF, ARW or RW2 whose
    /// boxes or maker note are damaged, or a file that begins like a
    /// TIFF (`II`/`MM`) and is not a valid one.
    pub fn read(path: &Path) -> Result<Option<CameraStyle>> {
        guarded(
            || tail_of(path),
            || read_stream(BufReader::new(File::open(path)?)),
        )
    }

    /// A fixed style with every adaptive setting off: frames with this
    /// can be fitted.
    pub fn is_fixed(&self) -> bool {
        self.group_key().is_some() && self.adaptive.iter().all(|a| !a.on)
    }

    /// The name frames are grouped under for a fit, the maker and the
    /// style: "Canon Faithful", "Fujifilm Reala Ace". `None` when there
    /// is no style, when the style is itself adaptive, when a setting
    /// that replaces the style is on ([`AdaptiveKind::Replaces`]), and
    /// when a setting is not known: such a frame was not rendered by
    /// its style, or may not have been, and is never sampled for the
    /// style's fit. A tone setting on leaves the key: the frame is of
    /// the group, not fixed, and fills a group short of fixed frames.
    /// The key does not name the body; a caller fitting per body adds
    /// the model.
    pub fn group_key(&self) -> Option<String> {
        if self.adaptive.iter().any(Adaptive::ungroups) {
            return None;
        }
        match &self.style {
            Some(style) if self.style_fixed => Some(format!("{} {style}", self.maker.name())),
            _ => None,
        }
    }
}

/// [`CameraStyle::read`] on any seekable source, unguarded.
fn read_stream(mut file: impl Read + Seek) -> Result<Option<CameraStyle>> {
    let mut head = [0u8; 16];
    let n = read_up_to(&mut file, &mut head)?;
    let head = &head[..n];
    file.seek(SeekFrom::Start(0))?;

    if head.len() >= 12 && &head[4..8] == b"ftyp" && &head[8..12] == b"crx " {
        return canon_cr3(file).map(Some);
    }
    if head.starts_with(b"FUJIFILMCCD-RAW") {
        return fujifilm_raf(file).map(Some);
    }
    if head.starts_with(b"II") || head.starts_with(b"MM") {
        let mut tiff = Tiff::new(file)?;
        let (ifd0, endian) = tiff_ifd0(&mut tiff)?;
        let make = find(&ifd0, TAG_MAKE)
            .and_then(|f| tiff.ascii(&f))
            .unwrap_or_default();
        let maker = Maker::from_make(&make);
        let note = |tiff: &mut Tiff<_>| maker_note(tiff, &ifd0, endian);
        return Ok(Some(match maker {
            Maker::Other => return Ok(None),
            // A DNG's preview is its converter's rendering, not the
            // camera's, so it has no style to fit even where the
            // maker note came along.
            _ if find(&ifd0, TAG_DNG_VERSION).is_some() => CameraStyle::bare(maker),
            Maker::Nikon => match note(&mut tiff)? {
                Some(note) => nikon_from_note(&mut tiff, &note)?,
                None => CameraStyle::bare(maker),
            },
            Maker::Sony => match note(&mut tiff)? {
                Some(note) => sony_from_note(&mut tiff, &note)?,
                None => CameraStyle::bare(maker),
            },
            Maker::Panasonic => match note(&mut tiff)? {
                Some(note) => panasonic_from_note(&mut tiff, &note)?,
                None => panasonic_from_jpeg(&mut tiff, &ifd0)?,
            },
            Maker::Canon | Maker::Fujifilm => CameraStyle::bare(maker),
        }));
    }
    Ok(None)
}

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

fn note_error(e: impl std::fmt::Display) -> Error {
    Error::Decode(format!("maker note: {e}"))
}

/// rawler's BMFF error prints its I/O case as a DNG write failure, so
/// say what was being read and what the I/O error was.
fn boxes_error(e: BmffError) -> Error {
    Error::Decode(match e {
        BmffError::Io(io) => format!("reading the CR3 boxes: {io}"),
        BmffError::Parse(what) => format!("reading the CR3 boxes: {what}"),
    })
}

/// A TIFF's first IFD and its byte order, and nothing else: no chained
/// IFDs, no sub-IFDs. rawler's `GenericTiffReader` follows the
/// next-IFD chain with no cycle check when it is given no limit, so a
/// file whose IFD0 points back at itself grows the chain until memory
/// runs out, a hang the panic guard cannot catch. IFD0's Make and the
/// pointer to the Exif IFD are all that is wanted here.
fn tiff_ifd0<R: Read + Seek>(tiff: &mut Tiff<R>) -> Result<(Vec<Field>, Endian)> {
    let header = tiff.read_at(0, 8).map_err(|_| note_error("not a TIFF"))?;
    let endian = match &header[..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => return Err(note_error("not a TIFF")),
    };
    // 42, or Panasonic's 0x55 and Olympus's two, as rawler takes them.
    if !matches!(u16_of(endian, &header[2..4]), 42 | 0x55 | 0x4f52 | 0x5352) {
        return Err(note_error("not a TIFF"));
    }
    let offset = u32_of(endian, &header[4..8]) as u64;
    Ok((tiff.ifd(offset, 0, endian)?, endian))
}

/// The Exif IFD's maker note entry, if the file has both.
fn maker_note<R: Read + Seek>(
    tiff: &mut Tiff<R>,
    ifd0: &[Field],
    endian: Endian,
) -> Result<Option<Field>> {
    let Some(at) = find(ifd0, TAG_EXIF_IFD).and_then(|f| tiff.int(&f)) else {
        return Ok(None);
    };
    let exif = tiff.ifd(at as u64, 0, endian)?;
    Ok(find(&exif, TAG_MAKER_NOTE))
}

const TAG_MAKE: u16 = 0x010f;
const TAG_EXIF_IFD: u16 = 0x8769;
const TAG_MAKER_NOTE: u16 = 0x927c;
const TAG_DNG_VERSION: u16 = 0xc612;

/// More entries than any maker's IFD holds: an IFD that says more is
/// damaged, not read.
const MAX_ENTRIES: usize = 1024;
/// The largest value read: every tag the reader wants is a few bytes
/// to a few hundred, so a count that asks for more is not believed.
const MAX_VALUE: u64 = 64 * 1024;

/// One IFD entry: where its value sits and how to read it, nothing
/// read yet.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Field {
    tag: u16,
    typ: u16,
    count: u32,
    /// The value's position in the source.
    at: u64,
    endian: Endian,
}

/// The size of one element of a TIFF type; `None` for a type the
/// reader does not know.
fn type_size(typ: u16) -> Option<u64> {
    match typ {
        1 | 2 | 6 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 9 | 11 | 13 => Some(4),
        5 | 10 | 12 => Some(8),
        _ => None,
    }
}

fn u16_of(endian: Endian, b: &[u8]) -> u16 {
    let b = [b[0], b[1]];
    match endian {
        Endian::Little => u16::from_le_bytes(b),
        Endian::Big => u16::from_be_bytes(b),
    }
}

fn u32_of(endian: Endian, b: &[u8]) -> u32 {
    let b = [b[0], b[1], b[2], b[3]];
    match endian {
        Endian::Little => u32::from_le_bytes(b),
        Endian::Big => u32::from_be_bytes(b),
    }
}

fn find(fields: &[Field], tag: u16) -> Option<Field> {
    fields.iter().find(|f| f.tag == tag).copied()
}

/// A TIFF-based file read an IFD at a time, every read held to the
/// file's length before anything is allocated. rawler's IFD reader
/// allocates what an entry's count asks before it reads, so a damaged
/// count could ask for gigabytes, an abort the panic guard cannot
/// catch.
struct Tiff<R> {
    r: R,
    len: u64,
}

impl<R: Read + Seek> Tiff<R> {
    fn new(mut r: R) -> Result<Tiff<R>> {
        let len = r.seek(SeekFrom::End(0))?;
        Ok(Tiff { r, len })
    }

    /// `n` bytes at `at`, an error when they run past the end.
    fn read_at(&mut self, at: u64, n: u64) -> Result<Vec<u8>> {
        if at.checked_add(n).is_none_or(|end| end > self.len) {
            return Err(note_error(format!(
                "{n} bytes at {at} run past the end of the file"
            )));
        }
        self.r.seek(SeekFrom::Start(at))?;
        let mut buf = vec![0; n as usize];
        self.r.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// The entries of the IFD at `at`, whose value offsets count from
    /// `base`. The chain to the next IFD is not followed.
    fn ifd(&mut self, at: u64, base: u64, endian: Endian) -> Result<Vec<Field>> {
        let n = u16_of(endian, &self.read_at(at, 2)?) as usize;
        if n > MAX_ENTRIES {
            return Err(note_error(format!("an IFD of {n} entries at {at}")));
        }
        let table = self.read_at(at + 2, 12 * n as u64)?;
        Ok(table
            .as_chunks::<12>()
            .0
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let typ = u16_of(endian, &e[2..4]);
                let count = u32_of(endian, &e[4..8]);
                let inline = type_size(typ).is_some_and(|s| s * count as u64 <= 4);
                let at = match inline {
                    true => at + 2 + 12 * i as u64 + 8,
                    false => base + u32_of(endian, &e[8..12]) as u64,
                };
                Field {
                    tag: u16_of(endian, &e[0..2]),
                    typ,
                    count,
                    at,
                    endian,
                }
            })
            .collect())
    }

    /// A value's bytes; `None` for a type the reader does not know, a
    /// value over [`MAX_VALUE`] or one that runs past the end.
    fn bytes(&mut self, f: &Field) -> Option<Vec<u8>> {
        let size = type_size(f.typ)?.checked_mul(f.count as u64)?;
        if size > MAX_VALUE {
            return None;
        }
        self.read_at(f.at, size).ok()
    }

    /// An integer value's elements, signed or unsigned as stored.
    fn ints(&mut self, f: &Field) -> Option<Vec<i64>> {
        let b = self.bytes(f)?;
        let e = f.endian;
        Some(match f.typ {
            1 | 7 => b.iter().map(|&v| v as i64).collect(),
            6 => b.iter().map(|&v| v as i8 as i64).collect(),
            3 => b
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16_of(e, c) as i64)
                .collect(),
            8 => b
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16_of(e, c) as i16 as i64)
                .collect(),
            4 | 13 => b
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32_of(e, c) as i64)
                .collect(),
            9 => b
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32_of(e, c) as i32 as i64)
                .collect(),
            _ => return None,
        })
    }

    /// An integer value's first element.
    fn int(&mut self, f: &Field) -> Option<i64> {
        self.ints(f)?.first().copied()
    }

    /// An ASCII value up to its first NUL, spaces trimmed.
    fn ascii(&mut self, f: &Field) -> Option<String> {
        let b = self.bytes(f)?;
        Some(c_string(&b))
    }
}

/// Bytes up to the first NUL as text, spaces trimmed.
fn c_string(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

/// The value at `index` of an integer array, read the way exiv2 types
/// it: a 16-bit signed array (`SShort`) whatever the file declares.
fn i16_at(value: &Value, index: usize) -> Option<i64> {
    match value {
        Value::Short(v) => v.get(index).map(|&n| n as i16 as i64),
        Value::SShort(v) => v.get(index).map(|&n| n as i64),
        _ => None,
    }
}

/// As [`i16_at`], for a 32-bit signed array (`SLong`).
fn i32_at(value: &Value, index: usize) -> Option<i64> {
    match value {
        Value::Long(v) => v.get(index).map(|&n| n as i32 as i64),
        Value::SLong(v) => v.get(index).map(|&n| n as i64),
        _ => None,
    }
}

/// Any integer value's first element, unsigned or signed as stored.
fn int_first(value: &Value) -> Option<i64> {
    match value {
        Value::Byte(v) => v.first().map(|&n| n as i64),
        Value::Short(v) => v.first().map(|&n| n as i64),
        Value::Long(v) => v.first().map(|&n| n as i64),
        Value::SByte(v) => v.first().map(|&n| n as i64),
        Value::SShort(v) => v.first().map(|&n| n as i64),
        Value::SLong(v) => v.first().map(|&n| n as i64),
        _ => None,
    }
}

// Canon. The maker note tags and array indexes are exiv2's (its
// CanonPr, CanonLiOp and CanonVigCor2 groups), each checked against
// exiv2's output on CR3s from the R5, R5 II, R6, R6 II, R6 III, R7 and R8.

/// ProcessingInfo, an int16s array; PictureStyle is element 10.
const CANON_PROCESSING_INFO: u16 = 0x00a0;
const CANON_PICTURE_STYLE: usize = 10;
/// PictureStyleUserDef: the base preset of User Def. 1, 2 and 3.
const CANON_PICTURE_STYLE_USER_DEF: u16 = 0x4008;
/// VignettingCorr2, an int32s array; PeripheralLightingSetting is
/// element 5.
const CANON_VIGNETTING_CORR2: u16 = 0x4016;
const CANON_PERIPHERAL_LIGHTING: usize = 5;
/// LightingOpt, an int32s array.
const CANON_LIGHTING_OPT: u16 = 0x4018;
const CANON_PERIPHERAL_ILLUMINATION: usize = 1;
const CANON_AUTO_LIGHTING_OPTIMIZER: usize = 2;
const CANON_HIGHLIGHT_TONE_PRIORITY: usize = 3;

/// A Canon Picture Style code as exiv2 names it, and whether the style
/// renders every frame the same way.
fn canon_style_name(code: i64) -> (String, bool) {
    let name = match code {
        0x00 => "None",
        0x01 => "Standard",
        0x02 => "Portrait",
        0x03 => "High Saturation",
        0x04 => "Adobe RGB",
        0x05 => "Low Saturation",
        0x06 => "CM Set 1",
        0x07 => "CM Set 2",
        0x21 => "User Def. 1",
        0x22 => "User Def. 2",
        0x23 => "User Def. 3",
        0x41 => "PC 1",
        0x42 => "PC 2",
        0x43 => "PC 3",
        0x81 => "Standard",
        0x82 => "Portrait",
        0x83 => "Landscape",
        0x84 => "Neutral",
        0x85 => "Faithful",
        0x86 => "Monochrome",
        0x87 => "Auto",
        0x88 => "Fine Detail",
        _ => return (format!("Unknown ({code})"), false),
    };
    (name.to_string(), code != 0x87)
}

fn canon_cr3(mut file: impl Read + Seek) -> Result<CameraStyle> {
    let filebox = FileBox::parse(&mut file).map_err(boxes_error)?;
    let Some(desc) = filebox.moov.cr3desc else {
        return Ok(CameraStyle::bare(Maker::Canon));
    };
    Ok(canon_from_note(&desc.cmt3.tiff))
}

/// The settings in a Canon maker note, a CR3's CMT3 box.
fn canon_from_note(note: &impl TiffReader) -> CameraStyle {
    let mut style = CameraStyle::bare(Maker::Canon);

    let code = note
        .get_entry(CANON_PROCESSING_INFO)
        .and_then(|e| i16_at(&e.value, CANON_PICTURE_STYLE));
    if let Some(code) = code {
        let (name, mut fixed) = canon_style_name(code);
        if (0x21..=0x23).contains(&code) {
            let base = note
                .get_entry(CANON_PICTURE_STYLE_USER_DEF)
                .and_then(|e| i16_at(&e.value, (code - 0x21) as usize))
                .map(canon_style_name);
            // A user style is as fixed as the preset it was built on.
            fixed = base.as_ref().is_some_and(|(_, f)| *f);
            style.base = base.map(|(n, _)| n);
        }
        if code != 0 {
            style.style = Some(name);
            style.style_fixed = fixed;
        }
    }

    let opt = |index| {
        note.get_entry(CANON_LIGHTING_OPT)
            .and_then(|e| i32_at(&e.value, index))
    };
    // 0 Standard, 1 Low, 2 Strong, 3 Off.
    const ALO: &str = "Auto Lighting Optimizer";
    style
        .adaptive
        .push(match opt(CANON_AUTO_LIGHTING_OPTIMIZER) {
            Some(0..=2) => Adaptive::known(ALO, true),
            Some(3) => Adaptive::known(ALO, false),
            _ => Adaptive::unknown(ALO),
        });
    // 0 Off, 1 On.
    const HTP: &str = "Highlight Tone Priority";
    style
        .adaptive
        .push(match opt(CANON_HIGHLIGHT_TONE_PRIORITY) {
            Some(0) => Adaptive::known(HTP, false),
            Some(1) => Adaptive::known(HTP, true),
            _ => Adaptive::unknown(HTP),
        });

    // The R-series bodies write the lens correction they applied in
    // VignettingCorr2 and leave LightingOpt's older flag at Off; that
    // flag is the answer only where VignettingCorr2 is missing.
    let vig2 = note
        .get_entry(CANON_VIGNETTING_CORR2)
        .and_then(|e| i32_at(&e.value, CANON_PERIPHERAL_LIGHTING));
    let lio = note
        .get_entry(CANON_LIGHTING_OPT)
        .and_then(|e| i32_at(&e.value, CANON_PERIPHERAL_ILLUMINATION));
    style.peripheral_correction = match vig2.or(lio) {
        Some(0) => Some(false),
        Some(1) => Some(true),
        _ => None,
    };

    style
}

// Fujifilm. The maker note sits in the EXIF of the JPEG the RAF embeds;
// its IFD is little-endian with offsets from the note's own start. Tag
// numbers and values are exiv2's (its Fujifilm group), checked against
// exiv2's output on GFX 100S II files.

const FUJI_SHADOW_TONE: u16 = 0x1040;
const FUJI_HIGHLIGHT_TONE: u16 = 0x1041;
const FUJI_COLOR: u16 = 0x1003;
const FUJI_FILM_MODE: u16 = 0x1401;
const FUJI_DYNAMIC_RANGE_SETTING: u16 = 0x1402;
const FUJI_DEVELOPMENT_DYNAMIC_RANGE: u16 = 0x1403;
const FUJI_D_RANGE_PRIORITY: u16 = 0x1443;

/// A FilmMode code by the film's name, after exiv2's table.
fn fujifilm_film_name(code: i64) -> Option<&'static str> {
    Some(match code {
        0x000 => "Provia",
        0x100 => "Studio Portrait",
        0x110 => "Studio Portrait Enhanced Saturation",
        0x120 => "Astia",
        0x130 => "Studio Portrait Increased Sharpness",
        0x200 => "Velvia",
        0x300 => "Studio Portrait Ex",
        0x400 => "Velvia (F4)",
        0x500 => "Pro Neg. Std",
        0x501 => "Pro Neg. Hi",
        0x600 => "Classic Chrome",
        0x700 => "Eterna",
        0x800 => "Classic Neg.",
        0x900 => "Eterna Bleach Bypass",
        0xa00 => "Nostalgic Neg.",
        0xb00 => "Reala Ace",
        _ => return None,
    })
}

/// The monochrome simulations, which the Color tag names in place of a
/// FilmMode, after exiv2's table.
fn fujifilm_mono_name(code: i64) -> Option<&'static str> {
    Some(match code {
        0x300 => "Monochrome",
        0x301 => "Monochrome + R Filter",
        0x302 => "Monochrome + Ye Filter",
        0x303 => "Monochrome + G Filter",
        0x310 => "Sepia",
        0x500 => "Acros",
        0x501 => "Acros + R Filter",
        0x502 => "Acros + Ye Filter",
        0x503 => "Acros + G Filter",
        _ => return None,
    })
}

fn fujifilm_raf(mut file: impl Read + Seek) -> Result<CameraStyle> {
    let mut header = [0u8; 92];
    file.read_exact(&mut header)?;
    let offset = u32::from_be_bytes(header[84..88].try_into().unwrap()) as u64;
    let len = u32::from_be_bytes(header[88..92].try_into().unwrap()) as u64;
    // The EXIF segment is the JPEG's first or second; a segment is at
    // most 64 KiB, so this much holds it.
    let mut head = vec![0u8; len.min(160 * 1024) as usize];
    file.seek(SeekFrom::Start(offset))?;
    let n = read_up_to(&mut file, &mut head)?;
    head.truncate(n);
    fujifilm_from_jpeg(&head)
}

/// The settings from the head of a Fujifilm camera JPEG.
fn fujifilm_from_jpeg(jpeg: &[u8]) -> Result<CameraStyle> {
    let Some(tiff) = jpeg_exif(jpeg) else {
        return Ok(CameraStyle::bare(Maker::Fujifilm));
    };
    let root = IFD::new_root(&mut Cursor::new(tiff), 0).map_err(note_error)?;
    let note = root
        .get_sub_ifd(TAG_EXIF_IFD)
        .and_then(|exif| exif.get_entry(TAG_MAKER_NOTE))
        .and_then(|e| match &e.value {
            Value::Undefined(data) => Some(data.as_slice()),
            _ => None,
        });
    match note {
        Some(note) => fujifilm_from_note(note),
        None => Ok(CameraStyle::bare(Maker::Fujifilm)),
    }
}

/// The TIFF body of a JPEG's EXIF segment, if the head holds one.
fn jpeg_exif(jpeg: &[u8]) -> Option<&[u8]> {
    if !jpeg.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut at = 2;
    while at + 4 <= jpeg.len() {
        if jpeg[at] != 0xff {
            return None;
        }
        let marker = jpeg[at + 1];
        if marker == 0xda || marker == 0xd9 {
            return None;
        }
        let len = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        let body = jpeg.get(at + 4..at + 2 + len)?;
        if marker == 0xe1 && body.starts_with(b"Exif\0\0") {
            return Some(&body[6..]);
        }
        at += 2 + len;
    }
    None
}

/// The settings in a Fujifilm maker note, `FUJIFILM` header and all.
fn fujifilm_from_note(note: &[u8]) -> Result<CameraStyle> {
    let mut style = CameraStyle::bare(Maker::Fujifilm);
    if note.len() < 12 || !note.starts_with(b"FUJIFILM") {
        return Ok(style);
    }
    let start = u32::from_le_bytes(note[8..12].try_into().unwrap());
    let ifd =
        IFD::new(&mut Cursor::new(note), start, 0, 0, Endian::Little, &[]).map_err(note_error)?;
    let get = |tag: u16| ifd.get_entry(tag).and_then(|e| int_first(&e.value));

    if let Some(code) = get(FUJI_FILM_MODE) {
        match fujifilm_film_name(code) {
            Some(name) => {
                style.style = Some(name.to_string());
                style.style_fixed = true;
            }
            None => style.style = Some(format!("Unknown ({code})")),
        }
    } else if let Some(name) = get(FUJI_COLOR).and_then(fujifilm_mono_name) {
        style.style = Some(name.to_string());
        style.style_fixed = true;
    }

    // DynamicRangeSetting: 0 Auto, 1 Manual (DevelopmentDynamicRange
    // then says 100, 200 or 400), 256 Standard (100%), 512 Wide mode 1
    // (230%), 513 Wide mode 2 (400%), 32768 Film simulation mode.
    // Anything above 100 lifts the shadows; Auto picks by the scene.
    const DR: &str = "Dynamic Range";
    let setting = get(FUJI_DYNAMIC_RANGE_SETTING);
    let development = get(FUJI_DEVELOPMENT_DYNAMIC_RANGE);
    style.adaptive.push(match (setting, development) {
        (Some(0), _) => Adaptive::known(DR, true),
        (Some(1) | None, Some(100)) => Adaptive::known(DR, false),
        (Some(1) | None, Some(200 | 400)) => Adaptive::known(DR, true),
        (Some(256), _) => Adaptive::known(DR, false),
        (Some(512 | 513), _) => Adaptive::known(DR, true),
        _ => Adaptive::unknown(DR),
    });

    // D-Range Priority: 0 Auto, 1 Fixed (Weak or Strong), both a tone
    // compression on top of the simulation. The GFX 100S II, which has
    // the setting, leaves the tag out with it off, and older bodies
    // never write it, so absence is taken as off.
    const DRP: &str = "D-Range Priority";
    style.adaptive.push(match get(FUJI_D_RANGE_PRIORITY) {
        None => Adaptive::known(DRP, false),
        Some(0 | 1) => Adaptive::known(DRP, true),
        Some(_) => Adaptive::unknown(DRP),
    });

    // Signed, in steps of 16; nonzero is a tone tweak on top of the
    // simulation.
    let tone = |tag| {
        ifd.get_entry(tag)
            .and_then(|e| i32_at(&e.value, 0))
            .filter(|&v| v != 0)
    };
    if tone(FUJI_HIGHLIGHT_TONE).is_some() {
        style.adaptive.push(Adaptive::known("Highlight Tone", true));
    }
    if tone(FUJI_SHADOW_TONE).is_some() {
        style.adaptive.push(Adaptive::known("Shadow Tone", true));
    }

    Ok(style)
}

/// A maker note IFD's integer tag, first element.
fn int_tag<R: Read + Seek>(tiff: &mut Tiff<R>, ifd: &[Field], tag: u16) -> Option<i64> {
    find(ifd, tag).and_then(|f| tiff.int(&f))
}

// Nikon. The maker note is "Nikon\0", a version, two spare bytes and a
// TIFF of its own, whose offsets count from that TIFF's header. Tag
// numbers and values are exiv2's (its Nikon3 and NikonPc groups).

const NIKON_ACTIVE_D_LIGHTING: u16 = 0x0022;
const NIKON_PICTURE_CONTROL: u16 = 0x0023;
const NIKON_VIGNETTE_CONTROL: u16 = 0x002a;
const NIKON_SCENE_MODE: u16 = 0x008f;
const NIKON_VARI_PROGRAM: u16 = 0x00ab;

/// What the reader takes from a Picture Control record.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PictureControl {
    /// The control's name as the camera writes it: "STANDARD",
    /// "NEUTRAL", "AUTO", or a custom control's own name.
    name: String,
    /// The preset a custom control was built on; the name again for a
    /// preset.
    base: String,
    /// Whether the control's contrast or saturation is set to Auto,
    /// which the camera then picks per scene; `None` for a record too
    /// short to say.
    auto_adjust: Option<bool>,
}

/// The Picture Control record, maker note tag 0x0023. Three layouts:
/// version 1 ("0100") as exiv2's NikonPc table reads it, the name at 4,
/// the base at 24, contrast at 51 and saturation at 53; version 2
/// ("0200") with the name and base where version 1 has them and the
/// adjustments two bytes apart, contrast at 55 and saturation at 59;
/// version 3 ("0300", "0310") with the version written twice, the name
/// at 8, the base at 28, contrast at 63 and saturation at 67. An
/// adjustment's byte is its step plus 0x80, with 0x00 for Auto, 0x01 a
/// custom curve and 0xff not applicable. `None` for a version the
/// reader does not know or a record too short to hold the names.
fn nikon_picture_control(pc: &[u8]) -> Option<PictureControl> {
    let (name, base, contrast, saturation) = match pc.get(..2)? {
        b"01" => (4, 24, 51, 53),
        b"02" => (4, 24, 55, 59),
        b"03" => (8, 28, 63, 67),
        _ => return None,
    };
    let text = |at: usize| pc.get(at..at + 20).map(c_string);
    let auto_adjust = match (pc.get(contrast), pc.get(saturation)) {
        (Some(&c), Some(&s)) => Some(c == 0 || s == 0),
        _ => None,
    };
    Some(PictureControl {
        name: text(name)?,
        base: text(base)?,
        auto_adjust,
    })
}

/// A Picture Control name in the case the other makers' styles are
/// written in: "STANDARD" to "Standard", "RICH TONE PORTRAIT" to "Rich
/// Tone Portrait".
fn nikon_title(name: &str) -> String {
    name.split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn nikon_from_note<R: Read + Seek>(tiff: &mut Tiff<R>, note: &Field) -> Result<CameraStyle> {
    let mut style = CameraStyle::bare(Maker::Nikon);
    if note.count < 18 {
        return Ok(style);
    }
    let head = tiff.read_at(note.at, 18)?;
    // The type 3 note; the older bodies' type 1 ("Nikon\0\x01\0" and an
    // IFD with no TIFF header, the first Coolpix raws) has none of
    // these tags, and is a maker with no style like any other not read.
    let endian = match (&head[..6], &head[10..12]) {
        (b"Nikon\0", b"II") => Endian::Little,
        (b"Nikon\0", b"MM") => Endian::Big,
        _ => return Ok(style),
    };
    let base = note.at + 10;
    let ifd = tiff.ifd(base + u32_of(endian, &head[14..18]) as u64, base, endian)?;

    let pc = find(&ifd, NIKON_PICTURE_CONTROL)
        .and_then(|f| tiff.bytes(&f))
        .and_then(|b| nikon_picture_control(&b));
    if let Some(pc) = pc.as_ref().filter(|pc| !pc.name.is_empty()) {
        // Auto adapts the hue and tone to the scene, so it and a
        // custom control built on it are not fixed. A custom control
        // is as fixed as the preset it was built on, and one whose
        // base is not written cannot be told.
        let auto = |s: &str| s.eq_ignore_ascii_case("AUTO");
        if pc.name == pc.base {
            style.style = Some(nikon_title(&pc.name));
            style.style_fixed = !auto(&pc.name);
        } else {
            style.style = Some(pc.name.clone());
            style.style_fixed = !pc.base.is_empty() && !auto(&pc.base);
            style.base = (!pc.base.is_empty()).then(|| nikon_title(&pc.base));
        }
    }

    // 0 Off, 1 Low, 3 Normal, 5 High, 7 Extra High, 65535 Auto.
    const ADL: &str = "Active D-Lighting";
    style
        .adaptive
        .push(match int_tag(tiff, &ifd, NIKON_ACTIVE_D_LIGHTING) {
            Some(0) => Adaptive::known(ADL, false),
            Some(1 | 3 | 5 | 7 | 65535) => Adaptive::known(ADL, true),
            _ => Adaptive::unknown(ADL),
        });
    // Contrast or saturation at Auto bends the control per scene the
    // way Active D-Lighting does.
    const AUTO_ADJUST: &str = "Auto Contrast or Saturation";
    style.adaptive.push(match pc.and_then(|pc| pc.auto_adjust) {
        Some(on) => Adaptive::known(AUTO_ADJUST, on),
        None => Adaptive::unknown(AUTO_ADJUST),
    });
    // The scene mode, and the program the consumer bodies write when
    // the dial is on Auto, a scene or an effect ("AUTO", "PORTRAIT"):
    // either renders in place of the Picture Control. Text, empty or
    // left out when the dial is on P, A, S or M.
    for (tag, name) in [
        (NIKON_SCENE_MODE, "Scene Mode"),
        (NIKON_VARI_PROGRAM, "Auto, Scene or Effects Program"),
    ] {
        let on = match find(&ifd, tag) {
            None => Some(false),
            Some(f) => tiff.ascii(&f).map(|t| !t.is_empty()),
        };
        style.adaptive.push(
            match on {
                Some(on) => Adaptive::known(name, on),
                None => Adaptive::unknown(name),
            }
            .replacing(),
        );
    }

    // 0 Off, 1 Low, 3 Normal, 5 High.
    style.peripheral_correction = match int_tag(tiff, &ifd, NIKON_VIGNETTE_CONTROL) {
        Some(0) => Some(false),
        Some(1 | 3 | 5) => Some(true),
        _ => None,
    };

    Ok(style)
}

// Sony. An ARW's maker note is a bare IFD in the file's byte order with
// offsets from the file's start; a compact's begins "SONY DSC " and
// three NULs first. Tag numbers and values are exiv2's (its Sony1 and
// Sony2 groups, and the scene modes of its Minolta tables).

const SONY_AUTO_HDR: u16 = 0x200a;
const SONY_PICTURE_EFFECT: u16 = 0x200e;
const SONY_VIGNETTING_CORRECTION: u16 = 0x2011;
const SONY_CREATIVE_STYLE: u16 = 0xb020;
const SONY_SCENE_MODE: u16 = 0xb023;
const SONY_DYNAMIC_RANGE_OPTIMIZER: u16 = 0xb025;
const SONY_INTELLIGENT_AUTO: u16 = 0xb052;

/// A Creative Style as the camera writes it, by exiv2's name for it.
/// The Creative Look bodies write their looks' short names, ST as
/// "Standard", FL, IN, SH and VV2 as themselves.
fn sony_style_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "AdobeRGB" => "Adobe RGB",
        "Autumnleaves" => "Autumn leaves",
        "BW" => "Black and White",
        "Clear" => "Clear",
        "Deep" => "Deep",
        "FL" => "FL",
        "IN" => "IN",
        "Landscape" => "Landscape",
        "Light" => "Light",
        "Neutral" => "Neutral",
        "None" => "None",
        "Portrait" => "Portrait",
        "Real" => "Real",
        "SH" => "SH",
        "Sepia" => "Sepia",
        "Standard" => "Standard",
        "Sunset" => "Sunset",
        "Vivid" => "Vivid",
        "VV2" => "VV2",
        _ => return None,
    })
}

fn sony_from_note<R: Read + Seek>(tiff: &mut Tiff<R>, note: &Field) -> Result<CameraStyle> {
    let mut style = CameraStyle::bare(Maker::Sony);
    let head = tiff.read_at(note.at, 12)?;
    let at = match head.starts_with(b"SONY ") {
        true => note.at + 12,
        false => note.at,
    };
    let ifd = tiff.ifd(at, 0, note.endian)?;
    let int = |tiff: &mut Tiff<R>, tag| int_tag(tiff, &ifd, tag);

    let code = find(&ifd, SONY_CREATIVE_STYLE).and_then(|f| tiff.ascii(&f));
    match code.as_deref().map(|c| (c, sony_style_name(c))) {
        // Written where no Creative Style applies.
        Some((_, Some("None"))) | None => {}
        Some((_, Some(name))) => {
            style.style = Some(name.to_string());
            style.style_fixed = true;
        }
        Some((code, None)) => style.style = Some(format!("Unknown ({code})")),
    }

    // 0 Off; 1 Standard, 2 Advanced Auto, 3 Auto, 8 to 12 Advanced Lv1
    // to Lv5, 16 to 20 Lv1 to Lv5, every one a tone curve bent to the
    // scene or a fixed lift of the shadows.
    const DRO: &str = "Dynamic Range Optimizer";
    style
        .adaptive
        .push(match int(tiff, SONY_DYNAMIC_RANGE_OPTIMIZER) {
            Some(0) => Adaptive::known(DRO, false),
            Some(1..=3 | 8..=12 | 16..=20) => Adaptive::known(DRO, true),
            _ => Adaptive::unknown(DRO),
        });
    // The low byte: 0 Off, 1 Auto, 0x10 to 0x1a 1.0 to 6.0 EV. The
    // bodies without it do not write the tag, so absence is off.
    const HDR: &str = "Auto HDR";
    style.adaptive.push(match int(tiff, SONY_AUTO_HDR) {
        None => Adaptive::known(HDR, false),
        Some(v) => match v & 0xff {
            0 => Adaptive::known(HDR, false),
            0x01 | 0x10..=0x1a => Adaptive::known(HDR, true),
            _ => Adaptive::unknown(HDR),
        },
    });
    // 0 Standard; the scene modes and Auto, Auto+ and Superior Auto
    // render by the scene in place of the Creative Style. 21 is SLT's
    // Continuous Priority AE, a drive mode that leaves the style alone.
    const SCENE: &str = "Scene Mode";
    style.adaptive.push(match int(tiff, SONY_SCENE_MODE) {
        None | Some(0 | 21) => Adaptive::known(SCENE, false),
        Some(1..=9 | 16..=20 | 22..=28 | 33) => Adaptive::known(SCENE, true),
        _ => Adaptive::unknown(SCENE),
    });
    // 0 Off, 1 On, 2 Advanced.
    const IA: &str = "Intelligent Auto";
    style.adaptive.push(match int(tiff, SONY_INTELLIGENT_AUTO) {
        None | Some(0) => Adaptive::known(IA, false),
        Some(1 | 2) => Adaptive::known(IA, true),
        _ => Adaptive::unknown(IA),
    });
    // 0 Off; every other value of exiv2's table is an effect (Toy
    // Camera, Posterization, HDR Painting) drawn over the style.
    const EFFECT: &str = "Picture Effect";
    style.adaptive.push(match int(tiff, SONY_PICTURE_EFFECT) {
        None | Some(0) => Adaptive::known(EFFECT, false),
        Some(1..=10 | 13 | 16..=20 | 32..=34 | 48..=54 | 64..=66 | 80 | 97 | 98 | 112..=114) => {
            Adaptive::known(EFFECT, true)
        }
        _ => Adaptive::unknown(EFFECT),
    });
    // Every one after the DRO renders in place of the Creative Style
    // or over it.
    for a in &mut style.adaptive[1..] {
        *a = a.replacing();
    }

    // 0 Off, 2 Auto, 0xffffffff n/a.
    style.peripheral_correction = match int(tiff, SONY_VIGNETTING_CORRECTION) {
        Some(0) => Some(false),
        Some(2) => Some(true),
        _ => None,
    };

    Ok(style)
}

// Panasonic. An RW2's own Exif IFD has no maker note; the note is in the
// EXIF of the JPEG the RW2 embeds (IFD0's JpgFromRaw), "Panasonic" and
// three NULs, then an IFD with offsets from that EXIF's TIFF header.
// Tag numbers and values are exiv2's (its Panasonic and PanasonicRaw
// groups).

const PANASONIC_JPG_FROM_RAW: u16 = 0x002e;
const PANASONIC_SHOOTING_MODE: u16 = 0x001f;
const PANASONIC_INTELLIGENT_EXPOSURE: u16 = 0x005d;
const PANASONIC_INTELLIGENT_D_RANGE: u16 = 0x0079;
const PANASONIC_PHOTO_STYLE: u16 = 0x0089;
const PANASONIC_SHADING_COMPENSATION: u16 = 0x008a;
const PANASONIC_HDR: u16 = 0x009e;
const PANASONIC_FILTER_SETTINGS: u16 = 0x00a1;

/// A Photo Style code by exiv2's name for it, and whether it renders
/// every frame the same way. exiv2 calls 0 "NoAuto"; it is the style
/// the intelligent auto modes write, which picks by the scene, and is
/// named Auto here as ExifTool names it. 1 is the camera's Standard and
/// its custom styles alike, which the tag does not tell apart.
fn panasonic_style_name(code: i64) -> (String, bool) {
    let name = match code {
        0 => "Auto",
        1 => "Standard or Custom",
        2 => "Vivid",
        3 => "Natural",
        4 => "Monochrome",
        5 => "Scenery",
        6 => "Portrait",
        _ => return (format!("Unknown ({code})"), false),
    };
    (name.to_string(), code != 0)
}

/// Whether a ShootingMode code is a scene mode or an intelligent auto
/// mode, which render by the scene in place of the Photo Style: every
/// code in exiv2's table but Program, Aperture priority, Shutter-speed
/// priority and Manual. `None` for a code the table does not name.
fn panasonic_scene_mode(code: i64) -> Option<bool> {
    match code {
        6 | 7 | 8 | 11 => Some(false),
        0..=37 | 39 | 41..=46 | 51 | 55 | 57 | 59 | 62..=64 | 66..=90 => Some(true),
        _ => None,
    }
}

/// The settings from the maker note in the EXIF of the JPEG an RW2
/// embeds.
fn panasonic_from_jpeg<R: Read + Seek>(tiff: &mut Tiff<R>, ifd0: &[Field]) -> Result<CameraStyle> {
    let Some(jpeg) = find(ifd0, PANASONIC_JPG_FROM_RAW) else {
        return Ok(CameraStyle::bare(Maker::Panasonic));
    };
    // The EXIF segment is the JPEG's first; a segment is at most
    // 64 KiB, so this much holds it.
    let head = tiff.read_at(jpeg.at, (jpeg.count as u64).min(160 * 1024))?;
    let Some(exif) = jpeg_exif(&head) else {
        return Ok(CameraStyle::bare(Maker::Panasonic));
    };
    let mut inner = Tiff::new(Cursor::new(exif))?;
    let (ifd0, endian) = tiff_ifd0(&mut inner)?;
    match maker_note(&mut inner, &ifd0, endian)? {
        Some(note) => panasonic_from_note(&mut inner, &note),
        None => Ok(CameraStyle::bare(Maker::Panasonic)),
    }
}

fn panasonic_from_note<R: Read + Seek>(tiff: &mut Tiff<R>, note: &Field) -> Result<CameraStyle> {
    let mut style = CameraStyle::bare(Maker::Panasonic);
    let head = tiff.read_at(note.at, 12)?;
    if !head.starts_with(b"Panasonic\0") {
        return Ok(style);
    }
    let ifd = tiff.ifd(note.at + 12, 0, note.endian)?;
    let int = |tiff: &mut Tiff<R>, tag| int_tag(tiff, &ifd, tag);

    if let Some(code) = int(tiff, PANASONIC_PHOTO_STYLE) {
        let (name, fixed) = panasonic_style_name(code);
        style.style = Some(name);
        style.style_fixed = fixed;
    }

    // 0 Off, 1 Low, 2 Standard, 3 High. The bodies before Intelligent
    // D-Range called the same tone lift Intelligent Exposure; a body
    // writes one of them, and one that writes neither is not known.
    let level = |name, v: Option<i64>| match v {
        Some(0) => Adaptive::known(name, false),
        Some(1..=3) => Adaptive::known(name, true),
        _ => Adaptive::unknown(name),
    };
    const IDR: &str = "Intelligent D-Range";
    const IEX: &str = "Intelligent Exposure";
    let idr = int(tiff, PANASONIC_INTELLIGENT_D_RANGE);
    let iex = int(tiff, PANASONIC_INTELLIGENT_EXPOSURE);
    match (idr, iex) {
        (None, Some(_)) => style.adaptive.push(level(IEX, iex)),
        (_, None) => style.adaptive.push(level(IDR, idr)),
        (Some(_), Some(_)) => {
            style.adaptive.push(level(IDR, idr));
            style.adaptive.push(level(IEX, iex));
        }
    }
    // 0 Off; 100, 200, 300 the EV, and 32868, 32968, 33068 the same
    // with Auto. The bodies without it do not write the tag.
    const HDR: &str = "HDR";
    style.adaptive.push(
        match int(tiff, PANASONIC_HDR) {
            None | Some(0) => Adaptive::known(HDR, false),
            Some(100 | 200 | 300 | 32868 | 32968 | 33068) => Adaptive::known(HDR, true),
            _ => Adaptive::unknown(HDR),
        }
        .replacing(),
    );
    const SCENE: &str = "Scene or Intelligent Auto Mode";
    style.adaptive.push(
        match int(tiff, PANASONIC_SHOOTING_MODE).and_then(panasonic_scene_mode) {
            Some(on) => Adaptive::known(SCENE, on),
            None => Adaptive::unknown(SCENE),
        }
        .replacing(),
    );
    // The Filter Settings, a creative filter (Expressive, Retro, Toy
    // Effect and the rest) drawn over the Photo Style, usable in P, A,
    // S and M too: two 32-bit words, both zero with no filter. exiv2's
    // table has no entry for the tag; its layout and the zero for off
    // are ExifTool's (FilterEffect), and every body sampled writes
    // zeros but one.
    const FILTER: &str = "Filter Settings";
    let filter = find(&ifd, PANASONIC_FILTER_SETTINGS);
    let words = filter.and_then(|f| {
        let b = tiff.bytes(&f)?;
        (b.len() == 8).then(|| [u32_of(f.endian, &b[..4]), u32_of(f.endian, &b[4..])])
    });
    style.adaptive.push(
        match (filter, words) {
            (None, _) | (_, Some([0, 0])) => Adaptive::known(FILTER, false),
            (Some(_), Some(_)) => Adaptive::known(FILTER, true),
            (Some(_), None) => Adaptive::unknown(FILTER),
        }
        .replacing(),
    );

    // 0 Off, 1 On.
    style.peripheral_correction = match int(tiff, PANASONIC_SHADING_COMPENSATION) {
        Some(0) => Some(false),
        Some(1) => Some(true),
        _ => None,
    };

    Ok(style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::rawler::formats::tiff::GenericTiffReader;

    const SHORT: u16 = 3;
    const LONG: u16 = 4;
    const UNDEFINED: u16 = 7;
    const SSHORT: u16 = 8;
    const SLONG: u16 = 9;

    /// A little-endian IFD at `start` within a buffer that begins at
    /// `base` bytes before it, entries sorted by tag, values that do
    /// not fit in four bytes stored after the IFD. Offsets are counted
    /// from the buffer's start.
    fn ifd_bytes(start: usize, entries: &[(u16, u16, u32, Vec<u8>)]) -> Vec<u8> {
        ifd_bytes_in(start, entries, false)
    }

    /// As [`ifd_bytes`], big-endian when `big`; the values' own bytes
    /// are written as given.
    fn ifd_bytes_in(start: usize, entries: &[(u16, u16, u32, Vec<u8>)], big: bool) -> Vec<u8> {
        let u16b = |v: u16| {
            if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let u32b = |v: u32| {
            if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let mut entries = entries.to_vec();
        entries.sort_by_key(|e| e.0);
        let table = 2 + 12 * entries.len() + 4;
        let mut out = Vec::new();
        let mut data = Vec::new();
        out.extend_from_slice(&u16b(entries.len() as u16));
        for (tag, typ, count, bytes) in &entries {
            out.extend_from_slice(&u16b(*tag));
            out.extend_from_slice(&u16b(*typ));
            out.extend_from_slice(&u32b(*count));
            if bytes.len() <= 4 {
                let mut inline = bytes.clone();
                inline.resize(4, 0);
                out.extend_from_slice(&inline);
            } else {
                let at = start + table + data.len();
                out.extend_from_slice(&u32b(at as u32));
                data.extend_from_slice(bytes);
                if data.len() % 2 == 1 {
                    data.push(0);
                }
            }
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&data);
        out
    }

    fn tiff(entries: &[(u16, u16, u32, Vec<u8>)]) -> Vec<u8> {
        let mut out = b"II*\0".to_vec();
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend(ifd_bytes(8, entries));
        out
    }

    fn i16s(v: &[i16]) -> Vec<u8> {
        v.iter().flat_map(|n| n.to_le_bytes()).collect()
    }

    fn i32s(v: &[i32]) -> Vec<u8> {
        v.iter().flat_map(|n| n.to_le_bytes()).collect()
    }

    /// A Canon maker note with a Picture Style, the LightingOpt flags
    /// and optionally VignettingCorr2.
    fn canon_note(
        style: i16,
        alo: i32,
        htp: i32,
        lio_peripheral: i32,
        vig2: Option<i32>,
    ) -> CameraStyle {
        let mut processing = vec![0i16; 14];
        processing[0] = 28;
        processing[9] = 5200;
        processing[CANON_PICTURE_STYLE] = style;
        let mut opt = vec![0i32; 12];
        opt[0] = 48;
        opt[CANON_PERIPHERAL_ILLUMINATION] = lio_peripheral;
        opt[CANON_AUTO_LIGHTING_OPTIMIZER] = alo;
        opt[CANON_HIGHLIGHT_TONE_PRIORITY] = htp;
        opt[10] = 1;
        let mut entries = vec![
            (CANON_PROCESSING_INFO, SSHORT, 14, i16s(&processing)),
            (
                CANON_PICTURE_STYLE_USER_DEF,
                SHORT,
                3,
                i16s(&[0x81, 0x87, 0x85]),
            ),
            (CANON_LIGHTING_OPT, SLONG, 12, i32s(&opt)),
        ];
        if let Some(v) = vig2 {
            let mut vig = vec![0i32; 10];
            vig[0] = 40;
            vig[CANON_PERIPHERAL_LIGHTING] = v;
            vig[6] = 1;
            entries.push((CANON_VIGNETTING_CORR2, SLONG, 10, i32s(&vig)));
        }
        let bytes = tiff(&entries);
        let reader = GenericTiffReader::new_with_buffer(&bytes, 0, 0, None).unwrap();
        canon_from_note(&reader)
    }

    #[test]
    fn canon_faithful_with_alo_off_is_fixed() {
        let s = canon_note(133, 3, 0, 0, Some(1));
        assert_eq!(s.maker, Maker::Canon);
        assert_eq!(s.style.as_deref(), Some("Faithful"));
        assert_eq!(
            s.adaptive,
            vec![
                Adaptive::known("Auto Lighting Optimizer", false),
                Adaptive::known("Highlight Tone Priority", false),
            ]
        );
        assert_eq!(s.peripheral_correction, Some(true));
        assert!(s.is_fixed());
        assert_eq!(s.group_key().as_deref(), Some("Canon Faithful"));
    }

    #[test]
    fn canon_alo_standard_or_htp_is_not_fixed() {
        let alo = canon_note(129, 0, 0, 0, None);
        assert_eq!(alo.style.as_deref(), Some("Standard"));
        assert!(alo.adaptive[0].on);
        assert!(!alo.is_fixed());
        assert_eq!(alo.group_key().as_deref(), Some("Canon Standard"));

        let htp = canon_note(129, 3, 1, 0, None);
        assert!(htp.adaptive[1].on);
        assert!(!htp.is_fixed());
    }

    #[test]
    fn canon_values_it_cannot_name_are_not_fixed() {
        let alo = canon_note(133, 7, 0, 0, None);
        assert_eq!(
            alo.adaptive[0],
            Adaptive::unknown("Auto Lighting Optimizer")
        );
        assert!(!alo.is_fixed());
        let htp = canon_note(133, 3, 2, 0, None);
        assert_eq!(
            htp.adaptive[1],
            Adaptive::unknown("Highlight Tone Priority")
        );
        assert!(!htp.is_fixed());
    }

    #[test]
    fn canon_without_lighting_opt_is_not_fixed() {
        let mut processing = vec![0i16; 14];
        processing[CANON_PICTURE_STYLE] = 133;
        let bytes = tiff(&[(CANON_PROCESSING_INFO, SSHORT, 14, i16s(&processing))]);
        let reader = GenericTiffReader::new_with_buffer(&bytes, 0, 0, None).unwrap();
        let s = canon_from_note(&reader);
        // Not known, so of no group: never sampled for Faithful's fit.
        assert_eq!(s.style.as_deref(), Some("Faithful"));
        assert_eq!(s.group_key(), None);
        assert_eq!(
            s.adaptive,
            vec![
                Adaptive::unknown("Auto Lighting Optimizer"),
                Adaptive::unknown("Highlight Tone Priority"),
            ]
        );
        assert!(!s.is_fixed());
        assert_eq!(s.peripheral_correction, None);
    }

    #[test]
    fn canon_auto_has_no_group() {
        let s = canon_note(135, 3, 0, 0, None);
        assert_eq!(s.style.as_deref(), Some("Auto"));
        assert_eq!(s.group_key(), None);
        assert!(!s.is_fixed());
    }

    #[test]
    fn canon_user_style_takes_its_base() {
        let one = canon_note(0x21, 3, 0, 0, None);
        assert_eq!(one.style.as_deref(), Some("User Def. 1"));
        assert_eq!(one.base.as_deref(), Some("Standard"));
        assert!(one.is_fixed());
        let two = canon_note(0x22, 3, 0, 0, None);
        assert_eq!(two.base.as_deref(), Some("Auto"));
        assert_eq!(two.group_key(), None);
    }

    #[test]
    fn canon_unknown_style_is_named_by_code_and_not_fitted() {
        let s = canon_note(0x99, 3, 0, 0, None);
        assert_eq!(s.style.as_deref(), Some("Unknown (153)"));
        assert_eq!(s.group_key(), None);
    }

    #[test]
    fn canon_peripheral_prefers_vignetting_corr2() {
        assert_eq!(
            canon_note(133, 3, 0, 0, Some(1)).peripheral_correction,
            Some(true)
        );
        assert_eq!(
            canon_note(133, 3, 0, 1, Some(0)).peripheral_correction,
            Some(false)
        );
        assert_eq!(
            canon_note(133, 3, 0, 1, None).peripheral_correction,
            Some(true)
        );
        assert_eq!(
            canon_note(133, 3, 0, 0, None).peripheral_correction,
            Some(false)
        );
    }

    #[test]
    fn canon_names_follow_exiv2() {
        let names: Vec<_> = (0x81..=0x88).map(|c| canon_style_name(c).0).collect();
        assert_eq!(
            names,
            [
                "Standard",
                "Portrait",
                "Landscape",
                "Neutral",
                "Faithful",
                "Monochrome",
                "Auto",
                "Fine Detail"
            ]
        );
    }

    /// A Fujifilm maker note: the header, then the IFD at 12 with
    /// offsets from the note's start.
    fn fuji_note(entries: &[(u16, u16, u32, Vec<u8>)]) -> Vec<u8> {
        let mut note = b"FUJIFILM".to_vec();
        note.extend_from_slice(&12u32.to_le_bytes());
        note.extend(ifd_bytes(12, entries));
        note
    }

    fn short(tag: u16, v: u16) -> (u16, u16, u32, Vec<u8>) {
        (tag, SHORT, 1, v.to_le_bytes().to_vec())
    }

    fn slong(tag: u16, v: i32) -> (u16, u16, u32, Vec<u8>) {
        (tag, SLONG, 1, v.to_le_bytes().to_vec())
    }

    fn fuji(entries: &[(u16, u16, u32, Vec<u8>)]) -> CameraStyle {
        fujifilm_from_note(&fuji_note(entries)).unwrap()
    }

    #[test]
    fn fujifilm_reala_ace_dr100_is_fixed() {
        let s = fuji(&[
            short(FUJI_COLOR, 0),
            slong(FUJI_SHADOW_TONE, 0),
            slong(FUJI_HIGHLIGHT_TONE, 0),
            short(FUJI_FILM_MODE, 2816),
            short(FUJI_DYNAMIC_RANGE_SETTING, 1),
            short(FUJI_DEVELOPMENT_DYNAMIC_RANGE, 100),
        ]);
        assert_eq!(s.maker, Maker::Fujifilm);
        assert_eq!(s.style.as_deref(), Some("Reala Ace"));
        assert_eq!(
            s.adaptive,
            vec![
                Adaptive::known("Dynamic Range", false),
                Adaptive::known("D-Range Priority", false),
            ]
        );
        assert!(s.is_fixed());
        assert_eq!(s.group_key().as_deref(), Some("Fujifilm Reala Ace"));
        assert_eq!(s.peripheral_correction, None);
    }

    #[test]
    fn fujifilm_dr_it_cannot_decide_is_not_fixed() {
        for setting in [None, Some(32768), Some(2), Some(1)] {
            let mut entries = vec![short(FUJI_FILM_MODE, 0xb00)];
            if let Some(v) = setting {
                entries.push(short(FUJI_DYNAMIC_RANGE_SETTING, v));
            }
            let s = fuji(&entries);
            assert_eq!(
                s.adaptive[0],
                Adaptive::unknown("Dynamic Range"),
                "{setting:?}"
            );
            assert!(!s.is_fixed(), "{setting:?}");
        }
        for wide in [512, 513] {
            let s = fuji(&[
                short(FUJI_FILM_MODE, 0xb00),
                short(FUJI_DYNAMIC_RANGE_SETTING, wide),
            ]);
            assert_eq!(s.adaptive[0], Adaptive::known("Dynamic Range", true));
        }
        let standard = fuji(&[
            short(FUJI_FILM_MODE, 0xb00),
            short(FUJI_DYNAMIC_RANGE_SETTING, 256),
        ]);
        assert!(standard.is_fixed());
    }

    #[test]
    fn fujifilm_d_range_priority_is_adaptive() {
        let dr100 = [
            short(FUJI_FILM_MODE, 0xb00),
            short(FUJI_DYNAMIC_RANGE_SETTING, 1),
            short(FUJI_DEVELOPMENT_DYNAMIC_RANGE, 100),
        ];
        for (value, expected) in [
            (0, Adaptive::known("D-Range Priority", true)),
            (1, Adaptive::known("D-Range Priority", true)),
            (5, Adaptive::unknown("D-Range Priority")),
        ] {
            let mut entries = dr100.to_vec();
            entries.push(short(FUJI_D_RANGE_PRIORITY, value));
            let s = fuji(&entries);
            assert_eq!(s.adaptive[1], expected);
            assert!(!s.is_fixed());
        }
    }

    #[test]
    fn fujifilm_dr400_and_auto_are_adaptive() {
        let dr400 = fuji(&[
            short(FUJI_FILM_MODE, 0),
            short(FUJI_DYNAMIC_RANGE_SETTING, 1),
            short(FUJI_DEVELOPMENT_DYNAMIC_RANGE, 400),
        ]);
        assert_eq!(dr400.style.as_deref(), Some("Provia"));
        assert!(dr400.adaptive[0].on);
        assert!(!dr400.is_fixed());
        assert_eq!(dr400.group_key().as_deref(), Some("Fujifilm Provia"));

        let auto = fuji(&[
            short(FUJI_FILM_MODE, 0),
            short(FUJI_DYNAMIC_RANGE_SETTING, 0),
        ]);
        assert!(auto.adaptive[0].on);
    }

    #[test]
    fn fujifilm_tones_are_listed_only_when_moved() {
        let s = fuji(&[
            short(FUJI_FILM_MODE, 0x600),
            slong(FUJI_HIGHLIGHT_TONE, -16),
            slong(FUJI_SHADOW_TONE, 0),
            short(FUJI_DYNAMIC_RANGE_SETTING, 256),
        ]);
        assert_eq!(s.style.as_deref(), Some("Classic Chrome"));
        assert_eq!(
            s.adaptive,
            vec![
                Adaptive::known("Dynamic Range", false),
                Adaptive::known("D-Range Priority", false),
                Adaptive::known("Highlight Tone", true),
            ]
        );
        assert!(!s.is_fixed());
    }

    #[test]
    fn fujifilm_monochrome_comes_from_color() {
        let s = fuji(&[
            short(FUJI_COLOR, 0x501),
            short(FUJI_DYNAMIC_RANGE_SETTING, 256),
        ]);
        assert_eq!(s.style.as_deref(), Some("Acros + R Filter"));
        assert!(s.is_fixed());
        // Canon's Monochrome is another group.
        assert_eq!(
            fuji(&[
                short(FUJI_COLOR, 0x300),
                short(FUJI_DYNAMIC_RANGE_SETTING, 256)
            ])
            .group_key()
            .as_deref(),
            Some("Fujifilm Monochrome")
        );
        assert_eq!(
            canon_note(0x86, 3, 0, 0, None).group_key().as_deref(),
            Some("Canon Monochrome")
        );
        let unknown = fuji(&[short(FUJI_FILM_MODE, 0xc00)]);
        assert_eq!(unknown.style.as_deref(), Some("Unknown (3072)"));
        assert_eq!(unknown.group_key(), None);
    }

    #[test]
    fn fujifilm_note_is_found_through_the_jpeg_exif() {
        let note = fuji_note(&[short(FUJI_FILM_MODE, 0xb00)]);
        // IFD0 at 8 with one entry, the ExifIFD pointer; the Exif IFD
        // right after it, holding the maker note.
        let exif_at = 8 + 2 + 12 + 4;
        let mut body = b"II*\0".to_vec();
        body.extend_from_slice(&8u32.to_le_bytes());
        body.extend(ifd_bytes(
            8,
            &[(
                TAG_EXIF_IFD,
                LONG,
                1,
                (exif_at as u32).to_le_bytes().to_vec(),
            )],
        ));
        body.extend(ifd_bytes(
            exif_at,
            &[(TAG_MAKER_NOTE, UNDEFINED, note.len() as u32, note)],
        ));
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(body);
        let mut jpeg = vec![0xff, 0xd8];
        // An APP0 first, as a camera writes one.
        jpeg.extend_from_slice(&[0xff, 0xe0, 0x00, 0x04, 0x00, 0x00]);
        jpeg.extend_from_slice(&[0xff, 0xe1]);
        jpeg.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        jpeg.extend(app1);
        jpeg.extend_from_slice(&[0xff, 0xda, 0x00, 0x02]);

        let s = fujifilm_from_jpeg(&jpeg).unwrap();
        assert_eq!(s.style.as_deref(), Some("Reala Ace"));
    }

    #[test]
    fn a_jpeg_without_exif_gives_no_style() {
        let s = fujifilm_from_jpeg(&[0xff, 0xd8, 0xff, 0xda, 0x00, 0x02]).unwrap();
        assert_eq!(s, CameraStyle::bare(Maker::Fujifilm));
    }

    #[test]
    fn a_tiff_whose_ifd_chains_to_itself_is_read_once() {
        // 26 bytes: the header, one IFD at 8 with one entry, and a
        // next-IFD pointer back to 8. An unbounded chain walk never ends.
        let mut bytes = b"II*\0".to_vec();
        bytes.extend_from_slice(&8u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&0x0100u16.to_le_bytes());
        bytes.extend_from_slice(&SHORT.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&[64, 0, 0, 0]);
        bytes.extend_from_slice(&8u32.to_le_bytes());
        assert_eq!(bytes.len(), 26);
        let start = std::time::Instant::now();
        let mut t = Tiff::new(Cursor::new(&bytes)).unwrap();
        let (ifd, _) = tiff_ifd0(&mut t).unwrap();
        assert_eq!(ifd.len(), 1);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));

        let path =
            std::env::temp_dir().join(format!("greycard-style-cycle-{}.tif", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let read = CameraStyle::read(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(read.unwrap(), None);
    }

    #[test]
    fn a_file_that_looks_like_tiff_and_is_not_is_an_error() {
        let mut t = Tiff::new(Cursor::new(b"II\x01\x02garbage")).unwrap();
        assert!(tiff_ifd0(&mut t).is_err());
        let mut t = Tiff::new(Cursor::new(b"II*")).unwrap();
        assert!(tiff_ifd0(&mut t).is_err());
    }

    #[test]
    fn a_truncated_cr3_says_what_was_read() {
        let mut bytes = 24u32.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"ftypcrx \0\0\0\x01crx isom");
        bytes.extend_from_slice(&1000u32.to_be_bytes());
        bytes.extend_from_slice(b"moov");
        let Err(Error::Decode(message)) = canon_cr3(Cursor::new(bytes)) else {
            panic!("a truncated CR3 is an error");
        };
        assert!(message.starts_with("reading the CR3 boxes: "), "{message}");
        assert!(!message.contains("DNG"), "{message}");
    }

    #[test]
    fn a_file_that_is_no_raw_has_no_style() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert_eq!(CameraStyle::read(&path).unwrap(), None);
    }

    #[test]
    fn makers_from_make_strings() {
        assert_eq!(Maker::from_make("Canon"), Maker::Canon);
        assert_eq!(Maker::from_make("NIKON CORPORATION"), Maker::Nikon);
        assert_eq!(Maker::from_make("SONY"), Maker::Sony);
        assert_eq!(Maker::from_make("Panasonic"), Maker::Panasonic);
        assert_eq!(Maker::from_make("FUJIFILM"), Maker::Fujifilm);
        assert_eq!(Maker::from_make("OM Digital Solutions"), Maker::Other);
    }

    const ASCII: u16 = 2;

    fn ascii(tag: u16, text: &str) -> (u16, u16, u32, Vec<u8>) {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        (tag, ASCII, bytes.len() as u32, bytes)
    }

    fn long(tag: u16, v: u32) -> (u16, u16, u32, Vec<u8>) {
        (tag, LONG, 1, v.to_le_bytes().to_vec())
    }

    /// A little-endian TIFF raw: IFD0 with the Make and a pointer to an
    /// Exif IFD that holds one maker note, `note(at)` built for its
    /// place `at` in the file.
    fn raw_file(make: &str, note: impl Fn(usize) -> Vec<u8>) -> Vec<u8> {
        raw_file_with(make, &[], note)
    }

    /// [`raw_file`] with more entries in IFD0.
    fn raw_file_with(
        make: &str,
        more: &[(u16, u16, u32, Vec<u8>)],
        note: impl Fn(usize) -> Vec<u8>,
    ) -> Vec<u8> {
        let ifd0 = |exif: u32| {
            let mut entries = vec![ascii(TAG_MAKE, make), long(TAG_EXIF_IFD, exif)];
            entries.extend_from_slice(more);
            ifd_bytes(8, &entries)
        };
        let exif_at = 8 + ifd0(0).len();
        let note = note(exif_at + 2 + 12 + 4);
        let mut out = b"II*\0".to_vec();
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend(ifd0(exif_at as u32));
        out.extend(ifd_bytes(
            exif_at,
            &[(TAG_MAKER_NOTE, UNDEFINED, note.len() as u32, note)],
        ));
        out
    }

    fn read_bytes(bytes: Vec<u8>) -> Result<Option<CameraStyle>> {
        read_stream(Cursor::new(bytes))
    }

    /// A Picture Control record of `version` with a name, a base, and
    /// contrast and saturation bytes, every other adjustment at 0x80.
    fn picture_control(version: &str, name: &str, base: &str, contrast: u8, sat: u8) -> Vec<u8> {
        let (name_at, base_at, c_at, s_at, len) = match &version[..2] {
            "01" => (4, 24, 51, 53, 58),
            "02" => (4, 24, 55, 59, 68),
            _ => (8, 28, 63, 67, 76),
        };
        let mut pc = vec![0x80u8; len];
        pc[..4].copy_from_slice(version.as_bytes());
        if version.starts_with("03") {
            pc[4..8].copy_from_slice(version.as_bytes());
        }
        for (at, text) in [(name_at, name), (base_at, base)] {
            pc[at..at + 20].fill(0);
            pc[at..at + text.len()].copy_from_slice(text.as_bytes());
        }
        pc[c_at] = contrast;
        pc[s_at] = sat;
        pc
    }

    /// A NEF with a Nikon maker note, its TIFF big-endian as the
    /// cameras write it, holding the tags given (values big-endian).
    fn nef(entries: &[(u16, u16, u32, Vec<u8>)]) -> Vec<u8> {
        let entries = entries.to_vec();
        raw_file("NIKON CORPORATION", move |_| {
            let mut note = b"Nikon\0\x02\x10\0\0MM\0*".to_vec();
            note.extend_from_slice(&8u32.to_be_bytes());
            note.extend(ifd_bytes_in(8, &entries, true));
            note
        })
    }

    fn be_short(tag: u16, v: u16) -> (u16, u16, u32, Vec<u8>) {
        (tag, SHORT, 1, v.to_be_bytes().to_vec())
    }

    fn nikon(pc: Option<Vec<u8>>, adl: Option<u16>, vignette: Option<u16>) -> CameraStyle {
        let mut entries = Vec::new();
        if let Some(pc) = pc {
            entries.push((NIKON_PICTURE_CONTROL, UNDEFINED, pc.len() as u32, pc));
        }
        if let Some(v) = adl {
            entries.push(be_short(NIKON_ACTIVE_D_LIGHTING, v));
        }
        if let Some(v) = vignette {
            entries.push(be_short(NIKON_VIGNETTE_CONTROL, v));
        }
        read_bytes(nef(&entries)).unwrap().unwrap()
    }

    #[test]
    fn nikon_picture_control_in_its_three_layouts() {
        for version in ["0100", "0200", "0310"] {
            let pc = picture_control(version, "NEUTRAL", "NEUTRAL", 0x80, 0x80);
            assert_eq!(
                nikon_picture_control(&pc),
                Some(PictureControl {
                    name: "NEUTRAL".into(),
                    base: "NEUTRAL".into(),
                    auto_adjust: Some(false),
                }),
                "{version}"
            );
            for (contrast, sat) in [(0x00, 0x80), (0x80, 0x00)] {
                let auto = picture_control(version, "STANDARD", "STANDARD", contrast, sat);
                assert_eq!(
                    nikon_picture_control(&auto).unwrap().auto_adjust,
                    Some(true)
                );
            }
            // A custom curve (0x01) and n/a (0xff) are not Auto.
            let user = picture_control(version, "STANDARD", "STANDARD", 0x01, 0xff);
            assert_eq!(
                nikon_picture_control(&user).unwrap().auto_adjust,
                Some(false)
            );
        }
        let mut short = picture_control("0310", "FLAT", "FLAT", 0x80, 0x80);
        short.truncate(60);
        assert_eq!(nikon_picture_control(&short).unwrap().auto_adjust, None);
        assert_eq!(nikon_picture_control(b"0400STANDARD"), None);
        assert_eq!(nikon_picture_control(b"01"), None);
    }

    #[test]
    fn nikon_neutral_with_adl_off_is_fixed() {
        let s = nikon(
            Some(picture_control("0310", "NEUTRAL", "NEUTRAL", 0x80, 0x80)),
            Some(0),
            Some(3),
        );
        assert_eq!(s.maker, Maker::Nikon);
        assert_eq!(s.style.as_deref(), Some("Neutral"));
        assert_eq!(s.base, None);
        assert_eq!(
            s.adaptive,
            vec![
                Adaptive::known("Active D-Lighting", false),
                Adaptive::known("Auto Contrast or Saturation", false),
                Adaptive::known("Scene Mode", false).replacing(),
                Adaptive::known("Auto, Scene or Effects Program", false).replacing(),
            ]
        );
        assert_eq!(s.peripheral_correction, Some(true));
        assert!(s.is_fixed());
        assert_eq!(s.group_key().as_deref(), Some("Nikon Neutral"));
    }

    #[test]
    fn nikon_scene_and_auto_programs_take_the_frame_out_of_its_group() {
        let pc = picture_control("0310", "NEUTRAL", "NEUTRAL", 0x80, 0x80);
        let with = |tag: u16, text: &str| {
            let mut entries = vec![
                (
                    NIKON_PICTURE_CONTROL,
                    UNDEFINED,
                    pc.len() as u32,
                    pc.clone(),
                ),
                be_short(NIKON_ACTIVE_D_LIGHTING, 0),
            ];
            let mut bytes = text.as_bytes().to_vec();
            bytes.push(0);
            entries.push((tag, ASCII, bytes.len() as u32, bytes));
            read_bytes(nef(&entries)).unwrap().unwrap()
        };
        for (tag, index) in [(NIKON_SCENE_MODE, 2), (NIKON_VARI_PROGRAM, 3)] {
            // Spaces only, as the cameras write it on P, A, S and M.
            let off = with(tag, "                ");
            assert!(!off.adaptive[index].on, "{tag:#x}");
            assert!(off.is_fixed(), "{tag:#x}");
            let on = with(tag, "PORTRAIT");
            assert_eq!(on.adaptive[index].kind, AdaptiveKind::Replaces);
            assert!(on.adaptive[index].on && on.adaptive[index].known);
            assert_eq!(on.group_key(), None, "{tag:#x}");
        }
    }

    #[test]
    fn a_type_one_nikon_note_is_a_maker_with_no_style() {
        let bytes = raw_file("NIKON", |_| {
            let mut note = b"Nikon\0\x01\0".to_vec();
            note.extend(ifd_bytes(8, &[short(0x0002, 1)]));
            note.resize(40, 0);
            note
        });
        assert_eq!(
            read_bytes(bytes).unwrap(),
            Some(CameraStyle::bare(Maker::Nikon))
        );
    }

    #[test]
    fn nikon_auto_and_what_is_built_on_it_are_not_fixed() {
        let auto = nikon(
            Some(picture_control("0200", "AUTO", "AUTO", 0xff, 0xff)),
            Some(0),
            Some(0),
        );
        assert_eq!(auto.style.as_deref(), Some("Auto"));
        assert_eq!(auto.group_key(), None);
        assert!(!auto.is_fixed());
        assert_eq!(auto.peripheral_correction, Some(false));

        let custom = nikon(
            Some(picture_control("0300", "MY-LOOK", "AUTO", 0x80, 0x80)),
            Some(0),
            None,
        );
        assert_eq!(custom.style.as_deref(), Some("MY-LOOK"));
        assert_eq!(custom.base.as_deref(), Some("Auto"));
        assert!(!custom.is_fixed());
        assert_eq!(custom.peripheral_correction, None);

        let vivid = nikon(
            Some(picture_control("0300", "VIVID-KR", "VIVID", 0x80, 0x8c)),
            Some(0),
            None,
        );
        assert_eq!(vivid.base.as_deref(), Some("Vivid"));
        assert!(vivid.is_fixed());
        assert_eq!(vivid.group_key().as_deref(), Some("Nikon VIVID-KR"));

        let unbased = nikon(
            Some(picture_control("0100", "MINE", "", 0x80, 0x80)),
            Some(0),
            None,
        );
        assert!(!unbased.is_fixed());
    }

    #[test]
    fn nikon_adaptive_settings_on_or_unnamed_are_not_fixed() {
        let pc = || Some(picture_control("0310", "STANDARD", "STANDARD", 0x80, 0x80));
        for (adl, expected) in [
            (Some(1), Adaptive::known("Active D-Lighting", true)),
            (Some(3), Adaptive::known("Active D-Lighting", true)),
            (Some(7), Adaptive::known("Active D-Lighting", true)),
            (Some(65535), Adaptive::known("Active D-Lighting", true)),
            (Some(2), Adaptive::unknown("Active D-Lighting")),
            (None, Adaptive::unknown("Active D-Lighting")),
        ] {
            let s = nikon(pc(), adl, Some(3));
            assert_eq!(s.adaptive[0], expected, "{adl:?}");
            assert!(!s.is_fixed(), "{adl:?}");
            // A tone setting on keeps the group; one not known does not.
            let group = expected.known.then_some("Nikon Standard");
            assert_eq!(s.group_key().as_deref(), group, "{adl:?}");
        }
        let auto_contrast = nikon(
            Some(picture_control("0310", "STANDARD", "STANDARD", 0x00, 0x80)),
            Some(0),
            None,
        );
        assert_eq!(
            auto_contrast.adaptive[1],
            Adaptive::known("Auto Contrast or Saturation", true)
        );
        assert!(!auto_contrast.is_fixed());
        // No Picture Control at all: no style, the adjustment not known.
        let none = nikon(None, Some(0), None);
        assert_eq!(none.style, None);
        assert_eq!(
            none.adaptive[1],
            Adaptive::unknown("Auto Contrast or Saturation")
        );
    }

    #[test]
    fn a_dng_keeps_its_maker_and_has_no_style() {
        let note = |_| {
            let pc = picture_control("0310", "NEUTRAL", "NEUTRAL", 0x80, 0x80);
            let mut note = b"Nikon\0\x02\x10\0\0MM\0*".to_vec();
            note.extend_from_slice(&8u32.to_be_bytes());
            note.extend(ifd_bytes_in(
                8,
                &[
                    (NIKON_PICTURE_CONTROL, UNDEFINED, pc.len() as u32, pc),
                    be_short(NIKON_ACTIVE_D_LIGHTING, 0),
                ],
                true,
            ));
            note
        };
        let nef = read_bytes(raw_file("NIKON CORPORATION", note))
            .unwrap()
            .unwrap();
        assert!(nef.is_fixed());
        let dng = raw_file_with(
            "NIKON CORPORATION",
            &[(TAG_DNG_VERSION, 1, 4, vec![1, 6, 0, 0])],
            note,
        );
        assert_eq!(
            read_bytes(dng).unwrap(),
            Some(CameraStyle::bare(Maker::Nikon))
        );
    }

    #[test]
    fn nikon_without_its_note_header_is_bare() {
        let bytes = raw_file("NIKON CORPORATION", |_| {
            b"Olympus\0\0\0\0\0\0\0\0\0\0\0".to_vec()
        });
        assert_eq!(
            read_bytes(bytes).unwrap(),
            Some(CameraStyle::bare(Maker::Nikon))
        );
        let no_exif = tiff(&[ascii(TAG_MAKE, "NIKON CORPORATION")]);
        assert_eq!(
            read_bytes(no_exif).unwrap(),
            Some(CameraStyle::bare(Maker::Nikon))
        );
    }

    /// An ARW: a bare little-endian IFD for the note, offsets from the
    /// file's start.
    fn arw(entries: &[(u16, u16, u32, Vec<u8>)]) -> CameraStyle {
        let entries = entries.to_vec();
        read_bytes(raw_file("SONY", move |at| ifd_bytes(at, &entries)))
            .unwrap()
            .unwrap()
    }

    fn sony_base() -> Vec<(u16, u16, u32, Vec<u8>)> {
        vec![
            ascii(SONY_CREATIVE_STYLE, "Neutral"),
            long(SONY_DYNAMIC_RANGE_OPTIMIZER, 0),
            long(SONY_AUTO_HDR, 0),
            long(SONY_SCENE_MODE, 0),
            short(SONY_INTELLIGENT_AUTO, 0),
            short(SONY_PICTURE_EFFECT, 0),
            long(SONY_VIGNETTING_CORRECTION, 2),
        ]
    }

    fn sony_with(tag: u16, entry: Option<(u16, u16, u32, Vec<u8>)>) -> CameraStyle {
        let mut entries: Vec<_> = sony_base().into_iter().filter(|e| e.0 != tag).collect();
        entries.extend(entry);
        arw(&entries)
    }

    #[test]
    fn sony_neutral_with_dro_off_is_fixed() {
        let s = arw(&sony_base());
        assert_eq!(s.maker, Maker::Sony);
        assert_eq!(s.style.as_deref(), Some("Neutral"));
        assert_eq!(
            s.adaptive,
            vec![
                Adaptive::known("Dynamic Range Optimizer", false),
                Adaptive::known("Auto HDR", false).replacing(),
                Adaptive::known("Scene Mode", false).replacing(),
                Adaptive::known("Intelligent Auto", false).replacing(),
                Adaptive::known("Picture Effect", false).replacing(),
            ]
        );
        assert_eq!(s.peripheral_correction, Some(true));
        assert!(s.is_fixed());
        assert_eq!(s.group_key().as_deref(), Some("Sony Neutral"));
        // The bodies without Auto HDR, a scene mode tag, Intelligent
        // Auto or Picture Effect write none of them.
        let older: Vec<_> = sony_base()
            .into_iter()
            .filter(|e| [SONY_CREATIVE_STYLE, SONY_DYNAMIC_RANGE_OPTIMIZER].contains(&e.0))
            .collect();
        let older = arw(&older);
        assert!(older.is_fixed());
        assert_eq!(older.peripheral_correction, None);
    }

    #[test]
    fn sony_dro_and_the_auto_modes_are_adaptive() {
        for (v, expected) in [
            (Some(3), Adaptive::known("Dynamic Range Optimizer", true)),
            (Some(1), Adaptive::known("Dynamic Range Optimizer", true)),
            (Some(20), Adaptive::known("Dynamic Range Optimizer", true)),
            (Some(4), Adaptive::unknown("Dynamic Range Optimizer")),
            (None, Adaptive::unknown("Dynamic Range Optimizer")),
        ] {
            let s = sony_with(
                SONY_DYNAMIC_RANGE_OPTIMIZER,
                v.map(|v| long(SONY_DYNAMIC_RANGE_OPTIMIZER, v)),
            );
            assert_eq!(s.adaptive[0], expected, "{v:?}");
            assert!(!s.is_fixed(), "{v:?}");
            // DRO bends the tone and keeps the frame in Neutral's group,
            // where it can fill a group short of fixed frames; a DRO
            // not known does not.
            let group = expected.known.then_some("Sony Neutral");
            assert_eq!(s.group_key().as_deref(), group, "{v:?}");
        }
        // Each of these renders in place of the style: of no group.
        for (tag, entry, index) in [
            (SONY_AUTO_HDR, long(SONY_AUTO_HDR, 0x0001_0012), 1),
            (SONY_SCENE_MODE, long(SONY_SCENE_MODE, 16), 2),
            (SONY_SCENE_MODE, long(SONY_SCENE_MODE, 24), 2),
            (SONY_INTELLIGENT_AUTO, short(SONY_INTELLIGENT_AUTO, 1), 3),
            (SONY_PICTURE_EFFECT, short(SONY_PICTURE_EFFECT, 1), 4),
        ] {
            let s = sony_with(tag, Some(entry));
            assert!(s.adaptive[index].on, "{tag:#x}");
            assert!(s.adaptive[index].known, "{tag:#x}");
            assert_eq!(s.adaptive[index].kind, AdaptiveKind::Replaces);
            assert!(!s.is_fixed(), "{tag:#x}");
            assert_eq!(s.group_key(), None, "{tag:#x}");
        }
        for (tag, entry, index) in [
            (SONY_SCENE_MODE, long(SONY_SCENE_MODE, 0xffff), 2),
            (SONY_PICTURE_EFFECT, short(SONY_PICTURE_EFFECT, 11), 4),
        ] {
            let s = sony_with(tag, Some(entry));
            assert!(!s.adaptive[index].known, "{tag:#x}");
            assert!(!s.is_fixed(), "{tag:#x}");
            assert_eq!(s.group_key(), None, "{tag:#x}");
        }
        // SLT's Continuous Priority AE leaves the style alone.
        assert!(sony_with(SONY_SCENE_MODE, Some(long(SONY_SCENE_MODE, 21))).is_fixed());
    }

    #[test]
    fn sony_style_names_follow_exiv2() {
        let named =
            |code: &str| sony_with(SONY_CREATIVE_STYLE, Some(ascii(SONY_CREATIVE_STYLE, code)));
        let bw = named("BW");
        assert_eq!(bw.style.as_deref(), Some("Black and White"));
        assert!(bw.is_fixed());
        let unknown = named("XX");
        assert_eq!(unknown.style.as_deref(), Some("Unknown (XX)"));
        assert_eq!(unknown.group_key(), None);
        assert_eq!(named("None").style, None);
        assert_eq!(sony_with(SONY_CREATIVE_STYLE, None).style, None);
    }

    #[test]
    fn sony_note_with_its_header_is_read() {
        let bytes = raw_file("SONY", |at| {
            let mut note = b"SONY DSC \0\0\0".to_vec();
            note.extend(ifd_bytes(at + 12, &sony_base()));
            note
        });
        let s = read_bytes(bytes).unwrap().unwrap();
        assert_eq!(s.group_key().as_deref(), Some("Sony Neutral"));
    }

    /// An RW2: IFD0 with the Make and the camera's JPEG, whose EXIF
    /// holds the Panasonic maker note with these tags.
    fn rw2(entries: &[(u16, u16, u32, Vec<u8>)]) -> CameraStyle {
        let entries = entries.to_vec();
        let exif = raw_file("Panasonic", move |at| {
            let mut note = b"Panasonic\0\0\0".to_vec();
            note.extend(ifd_bytes(at + 12, &entries));
            note
        });
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(exif);
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        jpeg.extend(app1);
        jpeg.extend_from_slice(&[0xff, 0xda, 0x00, 0x02]);
        let mut out = b"IIU\0".to_vec();
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend(ifd_bytes(
            8,
            &[
                (PANASONIC_JPG_FROM_RAW, UNDEFINED, jpeg.len() as u32, jpeg),
                ascii(TAG_MAKE, "Panasonic"),
            ],
        ));
        read_bytes(out).unwrap().unwrap()
    }

    fn panasonic_base() -> Vec<(u16, u16, u32, Vec<u8>)> {
        vec![
            short(PANASONIC_SHOOTING_MODE, 11),
            short(PANASONIC_INTELLIGENT_D_RANGE, 0),
            short(PANASONIC_PHOTO_STYLE, 3),
            short(PANASONIC_SHADING_COMPENSATION, 1),
            short(PANASONIC_HDR, 0),
        ]
    }

    fn panasonic_with(tag: u16, entry: Option<(u16, u16, u32, Vec<u8>)>) -> CameraStyle {
        let mut entries: Vec<_> = panasonic_base()
            .into_iter()
            .filter(|e| e.0 != tag)
            .collect();
        entries.extend(entry);
        rw2(&entries)
    }

    #[test]
    fn panasonic_natural_with_idr_off_is_fixed() {
        let s = rw2(&panasonic_base());
        assert_eq!(s.maker, Maker::Panasonic);
        assert_eq!(s.style.as_deref(), Some("Natural"));
        assert_eq!(
            s.adaptive,
            vec![
                Adaptive::known("Intelligent D-Range", false),
                Adaptive::known("HDR", false).replacing(),
                Adaptive::known("Scene or Intelligent Auto Mode", false).replacing(),
                Adaptive::known("Filter Settings", false).replacing(),
            ]
        );
        assert_eq!(s.peripheral_correction, Some(true));
        assert!(s.is_fixed());
        assert_eq!(s.group_key().as_deref(), Some("Panasonic Natural"));
    }

    #[test]
    fn panasonic_idr_hdr_and_the_auto_modes_are_adaptive() {
        let idr = panasonic_with(
            PANASONIC_INTELLIGENT_D_RANGE,
            Some(short(PANASONIC_INTELLIGENT_D_RANGE, 2)),
        );
        assert_eq!(
            idr.adaptive[0],
            Adaptive::known("Intelligent D-Range", true)
        );
        assert!(!idr.is_fixed());
        // A tone lift: the frame stays in the group.
        assert_eq!(idr.group_key().as_deref(), Some("Panasonic Natural"));
        // An older body's Intelligent Exposure in its place.
        let iex = panasonic_with(
            PANASONIC_INTELLIGENT_D_RANGE,
            Some(short(PANASONIC_INTELLIGENT_EXPOSURE, 0)),
        );
        assert_eq!(
            iex.adaptive[0],
            Adaptive::known("Intelligent Exposure", false)
        );
        assert!(iex.is_fixed());
        let neither = panasonic_with(PANASONIC_INTELLIGENT_D_RANGE, None);
        assert_eq!(
            neither.adaptive[0],
            Adaptive::unknown("Intelligent D-Range")
        );
        assert!(!neither.is_fixed());
        let hdr = panasonic_with(PANASONIC_HDR, Some(short(PANASONIC_HDR, 32968)));
        assert!(hdr.adaptive[1].on && hdr.adaptive[1].known);
        assert_eq!(hdr.group_key(), None);
        assert!(panasonic_with(PANASONIC_HDR, None).is_fixed());
        let ia = panasonic_with(
            PANASONIC_SHOOTING_MODE,
            Some(short(PANASONIC_SHOOTING_MODE, 37)),
        );
        assert!(ia.adaptive[2].on && ia.adaptive[2].known);
        assert_eq!(ia.group_key(), None);
        let unnamed = panasonic_with(
            PANASONIC_SHOOTING_MODE,
            Some(short(PANASONIC_SHOOTING_MODE, 60)),
        );
        assert_eq!(
            unnamed.adaptive[2],
            Adaptive::unknown("Scene or Intelligent Auto Mode").replacing()
        );
        assert_eq!(unnamed.group_key(), None);
        for mode in [6, 7, 8, 11] {
            let s = panasonic_with(
                PANASONIC_SHOOTING_MODE,
                Some(short(PANASONIC_SHOOTING_MODE, mode)),
            );
            assert!(s.is_fixed(), "{mode}");
        }
    }

    #[test]
    fn panasonic_filter_settings_take_the_frame_out_of_its_group() {
        const RATIONAL: u16 = 5;
        let words = |a: u32, b: u32| {
            let mut v = a.to_le_bytes().to_vec();
            v.extend_from_slice(&b.to_le_bytes());
            (PANASONIC_FILTER_SETTINGS, RATIONAL, 1, v)
        };
        let off = panasonic_with(PANASONIC_FILTER_SETTINGS, Some(words(0, 0)));
        assert_eq!(
            off.adaptive[3],
            Adaptive::known("Filter Settings", false).replacing()
        );
        assert!(off.is_fixed());
        for (a, b) in [(0, 1), (0, 512), (1, 0)] {
            let on = panasonic_with(PANASONIC_FILTER_SETTINGS, Some(words(a, b)));
            assert_eq!(
                on.adaptive[3],
                Adaptive::known("Filter Settings", true).replacing()
            );
            assert_eq!(on.group_key(), None, "{a} {b}");
        }
        // A value of another size is not known.
        let odd = panasonic_with(
            PANASONIC_FILTER_SETTINGS,
            Some(short(PANASONIC_FILTER_SETTINGS, 0)),
        );
        assert!(!odd.adaptive[3].known);
        assert_eq!(odd.group_key(), None);
    }

    #[test]
    fn panasonic_auto_and_unnamed_styles_are_not_fitted() {
        let style = |code| {
            panasonic_with(
                PANASONIC_PHOTO_STYLE,
                Some(short(PANASONIC_PHOTO_STYLE, code)),
            )
        };
        let auto = style(0);
        assert_eq!(auto.style.as_deref(), Some("Auto"));
        assert_eq!(auto.group_key(), None);
        let unknown = style(20);
        assert_eq!(unknown.style.as_deref(), Some("Unknown (20)"));
        assert_eq!(unknown.group_key(), None);
        assert_eq!(
            style(1).group_key().as_deref(),
            Some("Panasonic Standard or Custom")
        );
    }

    #[test]
    fn damaged_counts_and_offsets_are_held_to_the_file() {
        // A DRO whose count asks for 16 GiB: not read, and not known.
        let s = sony_with(
            SONY_DYNAMIC_RANGE_OPTIMIZER,
            Some((SONY_DYNAMIC_RANGE_OPTIMIZER, LONG, u32::MAX, vec![0; 8])),
        );
        assert_eq!(s.adaptive[0], Adaptive::unknown("Dynamic Range Optimizer"));
        // A Creative Style whose bytes run past the end: no style.
        let mut bytes = raw_file("SONY", |at| ifd_bytes(at, &sony_base()));
        let len = bytes.len();
        bytes.truncate(len - 4);
        assert_eq!(read_bytes(bytes).unwrap().unwrap().style, None);

        // An IFD that says it has 60000 entries is an error.
        let mut bytes = b"II*\0".to_vec();
        bytes.extend_from_slice(&8u32.to_le_bytes());
        bytes.extend_from_slice(&60000u16.to_le_bytes());
        bytes.resize(64, 0);
        assert!(read_bytes(bytes).is_err());

        // An Exif IFD pointer past the end is an error, as is a maker
        // note IFD that runs past it.
        let past = tiff(&[ascii(TAG_MAKE, "SONY"), long(TAG_EXIF_IFD, 1 << 30)]);
        assert!(read_bytes(past).is_err());
        let nef_bytes = nef(&[be_short(NIKON_ACTIVE_D_LIGHTING, 0)]);
        let cut = nef_bytes[..nef_bytes.len() - 10].to_vec();
        assert!(read_bytes(cut).is_err());

        // Through the guard, as files.
        for (i, bytes) in [
            tiff(&[ascii(TAG_MAKE, "SONY"), long(TAG_EXIF_IFD, 1 << 30)]),
            b"IIU\0\x08\0\0\0\xff\xff".to_vec(),
        ]
        .into_iter()
        .enumerate()
        {
            let path = std::env::temp_dir().join(format!(
                "greycard-style-damaged-{}-{i}.tif",
                std::process::id()
            ));
            std::fs::write(&path, &bytes).unwrap();
            let read = CameraStyle::read(&path);
            std::fs::remove_file(&path).unwrap();
            assert!(read.is_err(), "{i}");
        }
    }
}
