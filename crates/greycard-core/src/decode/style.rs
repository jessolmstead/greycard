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
//! (NEF, ARW, RW2, ORF) with a small IFD reader of our own that holds
//! every count and offset to the file before it reads or allocates.
//!
//! Canon CR3, Fujifilm RAF, Nikon NEF, Sony ARW, Panasonic RW2, and
//! Olympus and OM System ORF are read: the style, the scene-adaptive
//! settings and the vignetting flag. Any other TIFF raw of those
//! makers (a Canon CR2, or a DNG, whose preview is its converter's
//! rendering rather than the camera's) is recognized and named, with
//! no style.
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
//! highlights, a Panasonic Photo Style's contrast, saturation and
//! the like, and an Olympus Picture Mode's saturation, contrast,
//! sharpness, hue and effect steps (and the Custom Saturation,
//! Contrast and Sharpness that go with them). A group may mix frames
//! that differ in them.
//!
//! The tag numbers, array indexes and the names of their values follow
//! exiv2's maker note tables, `src/canonmn_int.cpp`, `src/fujimn_int.cpp`,
//! `src/nikonmn_int.cpp`, `src/sonymn_int.cpp`,
//! `src/panasonicmn_int.cpp` and `src/olympusmn_int.cpp` at
//! <https://github.com/Exiv2/exiv2>, GPL-2.0-or-later, copyright the
//! Exiv2 authors. The names were taken from exiv2's own output on real
//! and exiv2-written files. The layout
//! of the second and third versions of Nikon's Picture Control record,
//! which exiv2 reads with the first version's offsets, follows
//! ExifTool's `lib/Image/ExifTool/Nikon.pm` (PictureControl2 and
//! PictureControl3), Panasonic's Filter Settings tag, which exiv2
//! does not list, follows `lib/Image/ExifTool/Panasonic.pm`
//! (FilterEffect), and Olympus's Tone Level and Stacked Image tags,
//! which exiv2 does not name, follow `lib/Image/ExifTool/Olympus.pm`
//! (ToneLevel, StackedImage), copyright Phil Harvey, distributed under
//! the same terms as Perl (the GPL or the Artistic License); each was
//! checked on the bytes of real files. The reading code is ours.
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

use super::rawler_backend::{RAF_MAGIC, be_u32_at, guarded, half_copy_at};
use crate::error::{Error, Result};
use crate::raw::{CameraFrame, Rect};

/// Who made the camera, as far as the reader tells makers apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Maker {
    Canon,
    Fujifilm,
    Nikon,
    Sony,
    Panasonic,
    /// Olympus, whose Make is "OLYMPUS IMAGING CORP.", "OLYMPUS
    /// CORPORATION" or "OLYMPUS OPTICAL CO.,LTD".
    Olympus,
    /// OM Digital Solutions, Olympus's camera business since 2021 and
    /// the OM System bodies' Make. The same maker note as Olympus's.
    OmDigital,
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
        } else if make.starts_with("olympus") {
            Maker::Olympus
        } else if make.starts_with("om digital") {
            Maker::OmDigital
        } else {
            Maker::Other
        }
    }

    /// The maker's name as a group key begins with it: the start of
    /// the Make the camera writes, as a look's name strips it after the
    /// body's.
    pub fn name(self) -> &'static str {
        match self {
            Maker::Canon => "Canon",
            Maker::Fujifilm => "Fujifilm",
            Maker::Nikon => "Nikon",
            Maker::Sony => "Sony",
            Maker::Panasonic => "Panasonic",
            Maker::Olympus => "Olympus",
            Maker::OmDigital => "OM Digital Solutions",
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
    /// Panasonic's Intelligent D-Range, Olympus's Gradation. A frame
    /// with one on is still of its style's group, and fills a group
    /// short of fixed frames.
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
    /// highlight and shadow tone, and Olympus's highlight, shadow and
    /// midtone, are listed only when they were moved off zero.
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
    /// file that could not be read, a CR3, RAF, NEF, ARW, RW2 or ORF whose
    /// boxes or maker note are damaged, or a file that begins like a
    /// TIFF (`II`/`MM`) and is not a valid one.
    pub fn read(path: &Path) -> Result<Option<CameraStyle>> {
        guarded(
            || half_copy_at(path),
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
pub(crate) fn read_stream(mut file: impl Read + Seek) -> Result<Option<CameraStyle>> {
    let mut head = [0u8; 16];
    let n = read_up_to(&mut file, &mut head)?;
    let head = &head[..n];
    file.seek(SeekFrom::Start(0))?;

    if head.len() >= 12 && &head[4..8] == b"ftyp" && &head[8..12] == b"crx " {
        return canon_cr3(file).map(Some);
    }
    if head.starts_with(RAF_MAGIC) {
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
            Maker::Olympus | Maker::OmDigital => match note(&mut tiff)? {
                Some(note) => olympus_from_note(&mut tiff, &note, maker)?,
                None => CameraStyle::bare(maker),
            },
            Maker::Canon | Maker::Fujifilm => CameraStyle::bare(maker),
        }));
    }
    Ok(None)
}

/// A merge's tag from a DNG's IFD0, read as the style is, with no
/// decode: `None` for a file that is not TIFF-based, or whose
/// `DNGPrivateData` is missing or another maker's. A damaged file's
/// error says "merge tag", not the style reader's "maker note", so
/// the log tells this read from the style read that follows it.
pub(crate) fn merge_tag_stream(file: impl Read + Seek) -> Result<Option<crate::merge::Tag>> {
    read_merge_tag(file).map_err(|e| match e {
        Error::Decode(m) => Error::Decode(m.replacen("maker note: ", "merge tag: ", 1)),
        e => e,
    })
}

fn read_merge_tag(file: impl Read + Seek) -> Result<Option<crate::merge::Tag>> {
    let mut tiff = Tiff::new(file)?;
    let head = tiff.read_at(0, 2.min(tiff.len))?;
    if head != b"II" && head != b"MM" {
        return Ok(None);
    }
    let (ifd0, _) = tiff_ifd0(&mut tiff)?;
    Ok(find(&ifd0, TAG_DNG_PRIVATE_DATA)
        .filter(|f| matches!(f.typ, 1 | 7))
        .and_then(|f| tiff.bytes(&f))
        .and_then(|b| crate::merge::Tag::parse(&b)))
}

pub(super) fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
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
pub(super) fn boxes_error(e: BmffError) -> Error {
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
pub(super) fn tiff_ifd0<R: Read + Seek>(tiff: &mut Tiff<R>) -> Result<(Vec<Field>, Endian)> {
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
pub(super) fn maker_note<R: Read + Seek>(
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

pub(super) const TAG_MAKE: u16 = 0x010f;
const TAG_ORIENTATION: u16 = 0x0112;
pub(super) const TAG_IMAGE_WIDTH: u16 = 0x0100;
pub(super) const TAG_IMAGE_LENGTH: u16 = 0x0101;
pub(super) const TAG_EXIF_IFD: u16 = 0x8769;
pub(super) const TAG_MAKER_NOTE: u16 = 0x927c;
pub(super) const TAG_DNG_VERSION: u16 = 0xc612;
const TAG_DNG_PRIVATE_DATA: u16 = 0xc634;

/// More entries than any maker's IFD holds: an IFD that says more is
/// damaged, not read.
const MAX_ENTRIES: usize = 1024;
/// The largest value read: every tag the reader wants is a few bytes
/// to a few hundred, so a count that asks for more is not believed.
const MAX_VALUE: u64 = 64 * 1024;

/// One IFD entry: where its value sits and how to read it, nothing
/// read yet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Field {
    tag: u16,
    pub(super) typ: u16,
    pub(super) count: u32,
    /// The value's position in the source.
    pub(super) at: u64,
    pub(super) endian: Endian,
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

pub(super) fn u16_of(endian: Endian, b: &[u8]) -> u16 {
    let b = [b[0], b[1]];
    match endian {
        Endian::Little => u16::from_le_bytes(b),
        Endian::Big => u16::from_be_bytes(b),
    }
}

pub(super) fn u32_of(endian: Endian, b: &[u8]) -> u32 {
    let b = [b[0], b[1], b[2], b[3]];
    match endian {
        Endian::Little => u32::from_le_bytes(b),
        Endian::Big => u32::from_be_bytes(b),
    }
}

pub(super) fn find(fields: &[Field], tag: u16) -> Option<Field> {
    fields.iter().find(|f| f.tag == tag).copied()
}

/// A TIFF-based file read an IFD at a time, every read held to the
/// file's length before anything is allocated. rawler's IFD reader
/// allocates what an entry's count asks before it reads, so a damaged
/// count could ask for gigabytes, an abort the panic guard cannot
/// catch.
pub(super) struct Tiff<R> {
    pub(super) r: R,
    pub(super) len: u64,
}

impl<R: Read + Seek> Tiff<R> {
    pub(super) fn new(mut r: R) -> Result<Tiff<R>> {
        let len = r.seek(SeekFrom::End(0))?;
        Ok(Tiff { r, len })
    }

    /// `n` bytes at `at`, an error when they run past the end.
    pub(super) fn read_at(&mut self, at: u64, n: u64) -> Result<Vec<u8>> {
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
    pub(super) fn ifd(&mut self, at: u64, base: u64, endian: Endian) -> Result<Vec<Field>> {
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
    pub(super) fn bytes(&mut self, f: &Field) -> Option<Vec<u8>> {
        let size = type_size(f.typ)?.checked_mul(f.count as u64)?;
        if size > MAX_VALUE {
            return None;
        }
        self.read_at(f.at, size).ok()
    }

    /// An integer value's elements, signed or unsigned as stored.
    pub(super) fn ints(&mut self, f: &Field) -> Option<Vec<i64>> {
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
    pub(super) fn int(&mut self, f: &Field) -> Option<i64> {
        self.ints(f)?.first().copied()
    }

    /// An ASCII value up to its first NUL, spaces trimmed.
    pub(super) fn ascii(&mut self, f: &Field) -> Option<String> {
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

/// FileInfo, an int16s array; ShutterMode is element 23 (0
/// mechanical, 1 electronic first curtain, 2 electronic).
const CANON_FILE_INFO: u16 = 0x0093;
const CANON_SHUTTER_MODE: usize = 23;
/// LightingOpt's LongExposureNoiseReduction (0 off, 1 auto, 2 on) and
/// DualPixelRaw (0 off, 1 on).
const CANON_LONG_EXPOSURE_NR: usize = 4;
const CANON_DUAL_PIXEL_RAW: usize = 11;
/// HDRInfo, an int32s array; HDR is element 1 (0 off).
const CANON_HDR_INFO: u16 = 0x4025;
const CANON_HDR: usize = 1;
/// AspectInfo, an int32u array: the aspect, then the frame's width
/// and height.
const CANON_ASPECT_INFO: u16 = 0x009a;

/// The capture modes a CR3's maker note records that can change where
/// its sensor clips, each `None` when the note does not say. Indexes
/// as exiv2's CanonFi, CanonLiOp, and CanonHdr groups and its
/// AspectInfo, checked against exiv2 on R6 II and R5 II files. Public
/// only as what [`canon_modes_of_note`] answers, for the fuzz target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CanonModes {
    pub(crate) shutter: Option<i64>,
    pub(crate) highlight_tone_priority: Option<i64>,
    pub(crate) long_exposure_nr: Option<i64>,
    pub(crate) dual_pixel_raw: Option<i64>,
    pub(crate) hdr: Option<i64>,
    /// The aspect code, the width, and the height.
    pub(crate) aspect: Option<(i64, i64, i64)>,
}

/// [`CanonModes`] of a CR3 in `file`; `None` when it has no maker
/// note or one [`canon_modes_of_note`] refuses. The note's bytes are
/// read again through that checked reader, the path the fuzz target
/// takes, not through the TIFF rawler made of them on its walk.
pub(crate) fn canon_modes(mut file: impl Read + Seek) -> Option<CanonModes> {
    let filebox = FileBox::parse(&mut file).ok()?;
    canon_modes_of_note(&filebox.moov.cr3desc?.cmt3.data)
}

/// [`CanonModes`] of a CMT3 box's contents, a TIFF, checked
/// ([`note_fits`]) and then read as rawler reads the box: what
/// [`canon_modes`] reads a CR3's note by, and what the fuzz target
/// reaches.
pub fn canon_modes_of_note(bytes: &[u8]) -> Option<CanonModes> {
    if !note_fits(bytes) {
        return None;
    }
    let note =
        ::rawler::formats::tiff::GenericTiffReader::new_with_buffer(bytes, 0, 0, None).ok()?;
    Some(modes_of(&note))
}

/// Whether a TIFF's first directory and every value it points to lie
/// inside `bytes`, and it points to no other directory: rawler's
/// reader sizes a value by the count it is given, so a count past the
/// buffer asks it for gigabytes before it reads a byte. Offsets are
/// summed in u64, so none wraps on a 32-bit target.
fn note_fits(bytes: &[u8]) -> bool {
    let range = |at: u64, len: u64| {
        let start = usize::try_from(at).ok()?;
        let end = usize::try_from(at.checked_add(len)?).ok()?;
        bytes.get(start..end)
    };
    let u16_at = |at: u64, le: bool| {
        let b: [u8; 2] = range(at, 2)?.try_into().ok()?;
        Some(if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |at: u64, le: bool| {
        let b: [u8; 4] = range(at, 4)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let le = match bytes.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return false,
    };
    let fits = || -> Option<bool> {
        let ifd = u64::from(u32_at(4, le)?);
        let n = u64::from(u16_at(ifd, le)?);
        for k in 0..n {
            let e = ifd + 2 + 12 * k;
            let size: u64 = match u16_at(e + 2, le)? {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 => 8,
                _ => return Some(false),
            };
            let len = size * u64::from(u32_at(e + 4, le)?);
            if len > 4 {
                let at = u64::from(u32_at(e + 8, le)?);
                if at + len > bytes.len() as u64 {
                    return Some(false);
                }
            }
            // A directory (type 13, or a tag rawler follows: SubIFDs,
            // the Exif pointer) points to another: none in a note we
            // read, so no walk of offsets it was not given.
            if u16_at(e + 2, le)? == 13 || matches!(u16_at(e, le)?, 0x014a | 0x8769) {
                return Some(false);
            }
        }
        Some(u32_at(ifd + 2 + 12 * n, le)? == 0)
    };
    fits().unwrap_or(false)
}

/// The modes read off a Canon maker note.
fn modes_of(note: &::rawler::formats::tiff::GenericTiffReader) -> CanonModes {
    let at = |tag, index, read: fn(&Value, usize) -> Option<i64>| {
        note.get_entry(tag).and_then(|e| read(&e.value, index))
    };
    let long = |index| {
        note.get_entry(CANON_ASPECT_INFO)
            .and_then(|e| i32_at(&e.value, index))
    };
    CanonModes {
        shutter: at(CANON_FILE_INFO, CANON_SHUTTER_MODE, i16_at),
        highlight_tone_priority: at(CANON_LIGHTING_OPT, CANON_HIGHLIGHT_TONE_PRIORITY, i32_at),
        long_exposure_nr: at(CANON_LIGHTING_OPT, CANON_LONG_EXPOSURE_NR, i32_at),
        dual_pixel_raw: at(CANON_LIGHTING_OPT, CANON_DUAL_PIXEL_RAW, i32_at),
        hdr: at(CANON_HDR_INFO, CANON_HDR, i32_at),
        aspect: long(0)
            .zip(long(1))
            .zip(long(2))
            .map(|((a, w), h)| (a, w, h)),
    }
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
    let offset = be_u32_at(&header, 84);
    let len = be_u32_at(&header, 88);
    let head = jpeg_head(&mut file, offset, len)?;
    fujifilm_from_jpeg(&head)
}

/// How much of a JPEG's head its EXIF is looked for in: the EXIF
/// segment is the JPEG's first or second, and a segment is at most
/// 64 KiB.
pub(super) const JPEG_HEAD: u64 = 160 * 1024;

/// The head of the JPEG at `at`, `len` bytes long, as far as
/// [`jpeg_exif`] looks into it: its markers and lengths and its APP1
/// segments, read from the file, the other segments' bodies left as
/// zeros, up to the EXIF segment's end or [`JPEG_HEAD`] bytes, or the
/// file's end. `jpeg_exif` finds in it what it finds in the head read
/// whole; a camera JPEG's thumbnail segment, the most of that head, is
/// never read.
pub(super) fn jpeg_head(
    file: &mut (impl Read + Seek),
    at: u64,
    len: u64,
) -> std::io::Result<Vec<u8>> {
    let end = len.min(JPEG_HEAD) as usize;
    let mut head = vec![0u8; end];
    // Bytes `from..to` of the head read into place; false where the
    // file ends first, the head cut there.
    let mut fill = |head: &mut Vec<u8>, from: usize, to: usize| -> std::io::Result<bool> {
        let to = to.min(head.len());
        if from >= to {
            return Ok(true);
        }
        file.seek(SeekFrom::Start(at + from as u64))?;
        let n = read_up_to(file, &mut head[from..to])?;
        if from + n < to {
            head.truncate(from + n);
            return Ok(false);
        }
        Ok(true)
    };
    if !fill(&mut head, 0, 2)? || head.len() < 2 || head[..2] != [0xff, 0xd8] {
        return Ok(head);
    }
    let mut pos = 2;
    while pos + 4 <= head.len() {
        if !fill(&mut head, pos, pos + 4)? {
            break;
        }
        let marker = head[pos + 1];
        if head[pos] != 0xff || marker == 0xda || marker == 0xd9 {
            break;
        }
        let next = pos + 2 + u16::from_be_bytes([head[pos + 2], head[pos + 3]]) as usize;
        if marker == 0xe1 {
            if !fill(&mut head, pos + 4, next)? {
                break;
            }
            if next <= head.len() && head[pos + 4..].starts_with(b"Exif\0\0") {
                head.truncate(next);
                break;
            }
        }
        pos = next;
    }
    Ok(head)
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
pub(super) fn jpeg_exif(jpeg: &[u8]) -> Option<&[u8]> {
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
pub(super) fn int_tag<R: Read + Seek>(tiff: &mut Tiff<R>, ifd: &[Field], tag: u16) -> Option<i64> {
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
    let n = (jpeg.count as u64).min(JPEG_HEAD);
    if jpeg.at.checked_add(n).is_none_or(|end| end > tiff.len) {
        return Err(note_error(format!(
            "{n} bytes at {} run past the end of the file",
            jpeg.at
        )));
    }
    let head = jpeg_head(&mut tiff.r, jpeg.at, n)?;
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

// Olympus and OM System. An ORF's maker note is in its Exif IFD, in one
// of three forms: "OM SYSTEM", three NULs, the byte order and a version
// (16 bytes), or "OLYMPUS", a NUL, the byte order and a version (12
// bytes), each an IFD after the header with offsets from the note's
// start; or the first bodies' "OLYMP", a NUL and a version (8 bytes),
// an IFD in the file's byte order with offsets from the file's start.
// The settings are in the note's Camera Settings sub-IFD. Tag numbers
// and values are exiv2's (its Olympus2 and OlympusCs groups), checked
// against exiv2's output on OM-1 and OM-5 files; the Tone Level and
// Stacked Image layouts, which exiv2 does not name, are ExifTool's.

pub(super) const OLYMPUS_CAMERA_SETTINGS: u16 = 0x2020;
const OLYMPUS_SCENE_MODE: u16 = 0x0509;
const OLYMPUS_SHADING_COMPENSATION: u16 = 0x050c;
const OLYMPUS_GRADATION: u16 = 0x050f;
const OLYMPUS_PICTURE_MODE: u16 = 0x0520;
const OLYMPUS_BW_FILTER: u16 = 0x0525;
const OLYMPUS_PICTURE_TONE: u16 = 0x0526;
const OLYMPUS_ART_FILTER: u16 = 0x0529;
const OLYMPUS_MAGIC_FILTER: u16 = 0x052c;
const OLYMPUS_TONE_LEVEL: u16 = 0x052e;
const OLYMPUS_STACKED_IMAGE: u16 = 0x0804;

/// A Picture Mode code by exiv2's name for it, and whether it renders
/// every frame the same way. i-Enhance picks its colors by the scene
/// and e-Portrait smooths the skin it finds, as Canon's Auto adapts;
/// Color Creator and the Color and Monochrome Profiles are the user's
/// own, with no preset the file names as their base, as a Nikon custom
/// control with no base is. ExifTool's 8 (Underwater) and 17 (Art
/// Mode) exiv2 does not name.
fn olympus_mode_name(code: i64) -> (String, bool) {
    let (name, fixed) = match code {
        1 => ("Vivid", true),
        2 => ("Natural", true),
        3 => ("Muted", true),
        4 => ("Portrait", true),
        5 => ("i-Enhance", false),
        6 => ("e-Portrait", false),
        7 => ("Color Creator", false),
        9 => ("Color Profile 1", false),
        10 => ("Color Profile 2", false),
        11 => ("Color Profile 3", false),
        12 => ("Monochrome Profile 1", false),
        13 => ("Monochrome Profile 2", false),
        14 => ("Monochrome Profile 3", false),
        256 => ("Monotone", true),
        512 => ("Sepia", true),
        _ => return (format!("Unknown ({code})"), false),
    };
    (name.to_string(), fixed)
}

/// Monotone's color filter and toning by exiv2's names, 1 the neutral
/// one and 0 not applicable.
fn olympus_filter_name(code: i64) -> Option<&'static str> {
    Some(match code {
        1 => "Neutral",
        2 => "Yellow",
        3 => "Orange",
        4 => "Red",
        5 => "Green",
        _ => return None,
    })
}

fn olympus_tone_name(code: i64) -> Option<&'static str> {
    Some(match code {
        1 => "Neutral",
        2 => "Sepia",
        3 => "Blue",
        4 => "Purple",
        5 => "Green",
        _ => return None,
    })
}

/// Whether an Art Filter or Magic Filter code is one exiv2 names (0 is
/// Off).
fn olympus_filter_named(code: i64) -> bool {
    matches!(code, 0..=6 | 9 | 10 | 12..=29 | 31..=41)
}

/// Whether a Scene Mode code is one exiv2 names (0 is Standard, no
/// scene).
fn olympus_scene_named(code: i64) -> bool {
    matches!(code, 0 | 6..=20 | 22..=50)
}

/// An Olympus note's main IFD, the base its offsets count from, and its
/// byte order; `None` for a note of no header the reader knows.
pub(super) fn olympus_note<R: Read + Seek>(
    tiff: &mut Tiff<R>,
    note: &Field,
) -> Result<Option<(Vec<Field>, u64, Endian)>> {
    let head = tiff.read_at(note.at, (note.count as u64).min(16))?;
    let order = |b: &[u8]| match b {
        b"II" => Some(Endian::Little),
        b"MM" => Some(Endian::Big),
        _ => None,
    };
    // Where the IFD is, the offsets' base and the byte order.
    let (at, base, endian) = if head.len() >= 16 && head.starts_with(b"OM SYSTEM\0\0\0") {
        match order(&head[12..14]) {
            Some(e) => (note.at + 16, note.at, e),
            None => return Ok(None),
        }
    } else if head.len() >= 12 && head.starts_with(b"OLYMPUS\0") {
        match order(&head[8..10]) {
            Some(e) => (note.at + 12, note.at, e),
            None => return Ok(None),
        }
    } else if head.len() >= 8 && head.starts_with(b"OLYMP\0") {
        (note.at + 8, 0, note.endian)
    } else {
        return Ok(None);
    };
    Ok(Some((tiff.ifd(at, base, endian)?, base, endian)))
}

/// A sub-IFD of an Olympus note by its tag: an offset (IFD or LONG), or
/// the older bodies' UNDEFINED block that is the IFD itself. `None`
/// when the note has no such entry.
pub(super) fn olympus_sub<R: Read + Seek>(
    tiff: &mut Tiff<R>,
    (ifd, base, endian): &(Vec<Field>, u64, Endian),
    tag: u16,
) -> Result<Option<Vec<Field>>> {
    let at = match find(ifd, tag) {
        Some(f) if f.typ == 7 && f.count > 4 => f.at,
        Some(f) if matches!(f.typ, 4 | 13) => match tiff.int(&f) {
            Some(offset) => base + offset as u64,
            None => return Ok(None),
        },
        _ => return Ok(None),
    };
    Ok(Some(tiff.ifd(at, *base, *endian)?))
}

fn olympus_from_note<R: Read + Seek>(
    tiff: &mut Tiff<R>,
    note: &Field,
    maker: Maker,
) -> Result<CameraStyle> {
    let mut style = CameraStyle::bare(maker);
    let Some(main) = olympus_note(tiff, note)? else {
        return Ok(style);
    };
    // A note without Camera Settings is a maker with no style, as a
    // Nikon type 1 note is.
    let Some(cs) = olympus_sub(tiff, &main, OLYMPUS_CAMERA_SETTINGS)? else {
        return Ok(style);
    };
    let int = |tiff: &mut Tiff<R>, tag| int_tag(tiff, &cs, tag);
    let ints = |tiff: &mut Tiff<R>, tag| find(&cs, tag).and_then(|f| tiff.ints(&f));
    // A tag exiv2 types int16s, read as one whatever type the file
    // declares, so a SHORT's 0xffff is -1 as an SSHORT's is.
    let i16s = |tiff: &mut Tiff<R>, tag| {
        ints(tiff, tag).map(|v| v.into_iter().map(|n| n as i16 as i64).collect::<Vec<_>>())
    };

    if let Some(code) = int(tiff, OLYMPUS_PICTURE_MODE) {
        let (mut name, mut fixed) = olympus_mode_name(code);
        // Monotone's filter and toning are part of the style, as a
        // Fujifilm monochrome simulation's filter is: "Monotone + Red
        // Filter + Sepia Tone". One the reader cannot name leaves the
        // style unfixed.
        if code == 256 {
            let filter = int(tiff, OLYMPUS_BW_FILTER).and_then(olympus_filter_name);
            let tone = int(tiff, OLYMPUS_PICTURE_TONE).and_then(olympus_tone_name);
            match (filter, tone) {
                (Some(filter), Some(tone)) => {
                    if filter != "Neutral" {
                        name.push_str(&format!(" + {filter} Filter"));
                    }
                    if tone != "Neutral" {
                        name.push_str(&format!(" + {tone} Tone"));
                    }
                }
                _ => fixed = false,
            }
        }
        style.style = Some(name);
        style.style_fixed = fixed;
    }

    // Gradation: three values and, on the newer bodies, a fourth.
    // Normal is "0 -1 1", Low Key "-1 -1 1" and High Key "1 -1 1", each
    // a tone curve on the Picture Mode; the fourth is 0 for the user's
    // choice (exiv2's "User-Selected") and 1 for the camera's
    // ("Auto-Override"). Gradation Auto is "0 -1 1 1" (an OM-1's): the
    // camera bending the tone per frame, so on whatever key it names.
    // A value exiv2 does not name and a note without the tag are not
    // known.
    const GRADATION: &str = "Gradation";
    style
        .adaptive
        .push(match i16s(tiff, OLYMPUS_GRADATION).as_deref() {
            Some([a, -1, 1] | [a, -1, 1, 0]) if (-1..=1).contains(a) => {
                Adaptive::known(GRADATION, *a != 0)
            }
            Some([a, -1, 1, 1]) if (-1..=1).contains(a) => Adaptive::known(GRADATION, true),
            _ => Adaptive::unknown(GRADATION),
        });
    // The Art Filter (Pop Art, Grainy Film, Dramatic Tone and the rest)
    // and the compacts' Magic Filter, from one table, drawn over the
    // Picture Mode; the first value is the filter, 0 for Off. The
    // bodies without one do not write the tag.
    for (tag, name) in [
        (OLYMPUS_ART_FILTER, "Art Filter"),
        (OLYMPUS_MAGIC_FILTER, "Magic Filter"),
    ] {
        style.adaptive.push(
            match find(&cs, tag).map(|f| tiff.int(&f)) {
                None | Some(Some(0)) => Adaptive::known(name, false),
                Some(Some(v)) if olympus_filter_named(v) => Adaptive::known(name, true),
                Some(_) => Adaptive::unknown(name),
            }
            .replacing(),
        );
    }
    // 0 Standard; every other is a scene mode, which renders in place
    // of the Picture Mode.
    const SCENE: &str = "Scene Mode";
    style.adaptive.push(
        match int(tiff, OLYMPUS_SCENE_MODE) {
            Some(0) => Adaptive::known(SCENE, false),
            Some(v) if olympus_scene_named(v) => Adaptive::known(SCENE, true),
            _ => Adaptive::unknown(SCENE),
        }
        .replacing(),
    );
    // Two 32-bit words, "0 0" for a single frame; anything else is a
    // frame the camera merged from several (in-camera HDR, Live ND,
    // Live Composite, a focus stack, high resolution). exiv2 does not
    // name the tag; the layout is ExifTool's (StackedImage). The bodies
    // without the modes do not write it.
    const STACKED: &str = "Stacked Image";
    style.adaptive.push(
        match find(&cs, OLYMPUS_STACKED_IMAGE).map(|f| tiff.ints(&f)) {
            None => Adaptive::known(STACKED, false),
            Some(Some(v)) if v.len() == 2 => Adaptive::known(STACKED, v != [0, 0]),
            Some(_) => Adaptive::unknown(STACKED),
        }
        .replacing(),
    );
    // Highlight and shadow control: groups of four, a kind (-31999
    // highlights, -31998 shadows, -31997 midtones), the step, its least
    // and most; a step off zero is a tone tweak on the Picture Mode,
    // listed only when moved, as Fujifilm's tones are. The layout is
    // ExifTool's (ToneLevel).
    if let Some(levels) = i16s(tiff, OLYMPUS_TONE_LEVEL) {
        for (kind, name) in [
            (-31999, "Highlight Tone"),
            (-31998, "Shadow Tone"),
            (-31997, "Midtone"),
        ] {
            let moved = levels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|g| g[0] == kind && g[1] != 0);
            if moved {
                style.adaptive.push(Adaptive::known(name, true));
            }
        }
    }

    // 0 Off, 1 On.
    style.peripheral_correction = match int(tiff, OLYMPUS_SHADING_COMPENSATION) {
        Some(0) => Some(false),
        Some(1) => Some(true),
        _ => None,
    };

    Ok(style)
}

// The camera's frame: the rectangle the photographer chose in camera
// (an aspect ratio), when the raw keeps more of the sensor than that.
// Olympus and OM System bodies save the whole 4:3 sensor whatever the
// aspect, and record the choice in the note's Image Processing
// sub-IFD: AspectFrame, x0 y0 x1 y1 inclusive, in the coordinates of
// the sensor's valid area CropLeft, CropTop, CropWidth and CropHeight
// (the area rawler takes as the frame's crop). The tag numbers are
// exiv2's (its OlympusIp group); the coordinates were checked on a 3:2
// frame, whose camera picture is the area's middle 3:2 band.

pub(super) const OLYMPUS_IMAGE_PROCESSING: u16 = 0x2040;
const OLYMPUS_CROP_LEFT: u16 = 0x0612;
const OLYMPUS_CROP_TOP: u16 = 0x0613;
pub(super) const OLYMPUS_CROP_WIDTH: u16 = 0x0614;
pub(super) const OLYMPUS_CROP_HEIGHT: u16 = 0x0615;
pub(super) const OLYMPUS_ASPECT_FRAME: u16 = 0x1113;

/// The camera's frame of the raw read from `file`, in sensor samples:
/// `None` for a maker whose frame is not read, a DNG (whose converter
/// has said what it kept), a note that does not say, a frame that is
/// the whole valid area, and one that does not lie inside it.
pub(crate) fn frame_stream(mut file: impl Read + Seek) -> Result<Option<CameraFrame>> {
    // From the start, wherever a reader before this one left it.
    file.seek(SeekFrom::Start(0))?;
    let mut head = [0u8; 2];
    if read_up_to(&mut file, &mut head)? < 2 || !(head == *b"II" || head == *b"MM") {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut tiff = Tiff::new(file)?;
    let (ifd0, endian) = tiff_ifd0(&mut tiff)?;
    if find(&ifd0, TAG_DNG_VERSION).is_some() {
        return Ok(None);
    }
    let make = find(&ifd0, TAG_MAKE)
        .and_then(|f| tiff.ascii(&f))
        .unwrap_or_default();
    if !matches!(Maker::from_make(&make), Maker::Olympus | Maker::OmDigital) {
        return Ok(None);
    }
    // The image's declared size, where IFD0 says it: the valid area
    // lies inside it, or the note's numbers are of another picture.
    let declared =
        int_tag(&mut tiff, &ifd0, TAG_IMAGE_WIDTH).zip(int_tag(&mut tiff, &ifd0, TAG_IMAGE_LENGTH));
    let orientation = int_tag(&mut tiff, &ifd0, TAG_ORIENTATION)
        .and_then(|o| u16::try_from(o).ok())
        .and_then(crate::raw::Orientation::from_exif)
        .unwrap_or_default();
    let Some(note) = maker_note(&mut tiff, &ifd0, endian)? else {
        return Ok(None);
    };
    let Some(main) = olympus_note(&mut tiff, &note)? else {
        return Ok(None);
    };
    let Some(ip) = olympus_sub(&mut tiff, &main, OLYMPUS_IMAGE_PROCESSING)? else {
        return Ok(None);
    };
    let mut ints = |tag| find(&ip, tag).and_then(|f| tiff.ints(&f));
    // CropLeft and CropTop are two SHORTs, the second 0.
    let mut area = [0i64; 4];
    for (i, tag) in [
        OLYMPUS_CROP_LEFT,
        OLYMPUS_CROP_TOP,
        OLYMPUS_CROP_WIDTH,
        OLYMPUS_CROP_HEIGHT,
    ]
    .into_iter()
    .enumerate()
    {
        match ints(tag).as_deref() {
            Some([v, ..]) => area[i] = *v,
            _ => return Ok(None),
        }
    }
    let Some(&[x0, y0, x1, y1]) = ints(OLYMPUS_ASPECT_FRAME).as_deref() else {
        return Ok(None);
    };
    Ok(
        olympus_frame(area, [x0, y0, x1, y1], declared).map(|frame| CameraFrame {
            area: Rect {
                x: area[0] as usize,
                y: area[1] as usize,
                width: area[2] as usize,
                height: area[3] as usize,
            },
            frame,
            orientation,
        }),
    )
}

/// An AspectFrame (inclusive corners) inside the valid `area` (left,
/// top, width, height) as a rectangle of the sensor; `None` when it is
/// the whole area or not inside it, and when the two are not plausibly
/// of one picture: the area past the image's `declared` size (IFD0's,
/// where it says), or a frame that spans neither the area's whole width
/// nor its whole height, as every aspect a body offers does (3:2 and
/// 16:9 the width, 1:1 and 3:4 the height). A High Res Shot's note,
/// whose numbers may be of the sensor's own size, is unverified: it is
/// taken only where it passes these and the decode's own check.
fn olympus_frame(
    [left, top, width, height]: [i64; 4],
    [x0, y0, x1, y1]: [i64; 4],
    declared: Option<(i64, i64)>,
) -> Option<Rect> {
    let inside = left >= 0
        && top >= 0
        && (0..=x1).contains(&x0)
        && (0..=y1).contains(&y0)
        && x1 < width
        && y1 < height;
    let fits_declared = declared.is_none_or(|(dw, dh)| left + width <= dw && top + height <= dh);
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let spans = w == width || h == height;
    (inside && fits_declared && spans && (w, h) != (width, height)).then(|| Rect {
        x: (left + x0) as usize,
        y: (top + y0) as usize,
        width: w as usize,
        height: h as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::rawler::formats::tiff::GenericTiffReader;

    /// A JPEG said to be 0 or 1 bytes long is a head of that many bytes
    /// and no EXIF, not a panic; so is a RAF whose header says so, read
    /// for its style.
    #[test]
    fn a_jpeg_of_under_two_bytes_has_no_exif() {
        let file = [0xffu8, 0xd8, 0xff, 0xe1];
        for len in [0, 1] {
            let head = jpeg_head(&mut Cursor::new(&file), 0, len).unwrap();
            assert_eq!(head.len(), len as usize);
            assert_eq!(jpeg_exif(&head), None);
            let mut raf = RAF_MAGIC.to_vec();
            raf.resize(0x64, 0);
            raf[0x54..0x58].copy_from_slice(&0x64u32.to_be_bytes());
            raf[0x58..0x5c].copy_from_slice(&(len as u32).to_be_bytes());
            raf.extend([0xff, 0xd8, 0xff, 0xd9]);
            let style = read_stream(Cursor::new(raf)).unwrap().unwrap();
            assert_eq!(style.style, None);
        }
    }

    /// A JPEG's head read by its segments gives `jpeg_exif` what the
    /// head read whole gives it: the EXIF after a JFIF segment and an
    /// XMP one, a JPEG with no EXIF before its scan, a file cut short,
    /// a length that cuts the EXIF short, and no JPEG at all.
    #[test]
    fn a_jpegs_head_read_by_segments_finds_the_same_exif() {
        let segment = |marker: u8, body: &[u8]| {
            let mut s = vec![0xff, marker];
            s.extend_from_slice(&(body.len() as u16 + 2).to_be_bytes());
            s.extend_from_slice(body);
            s
        };
        let exif: Vec<u8> = b"Exif\0\0II*\0".iter().copied().chain(1..=200).collect();
        let jfif = segment(0xe0, &[7; 14]);
        let xmp = segment(0xe1, b"http://ns.adobe.com/xap/1.0/\0<x/>");
        let thumb = segment(0xe2, &[9; 60_000]);
        let jpeg = |parts: &[&[u8]]| {
            let mut j = vec![0xff, 0xd8];
            parts.iter().for_each(|p| j.extend_from_slice(p));
            j.extend_from_slice(&[0xff, 0xda, 0, 2]);
            j.resize(j.len() + 300_000, 5);
            j
        };
        let files = [
            jpeg(&[&jfif, &xmp, &segment(0xe1, &exif)]),
            jpeg(&[&thumb, &thumb, &segment(0xe1, &exif)]),
            jpeg(&[&jfif, &thumb]),
            jpeg(&[&jfif, &segment(0xe1, &exif)])[..40].to_vec(),
            vec![1, 2, 3],
        ];
        for jpeg in &files[..2] {
            let head = jpeg_head(&mut Cursor::new(jpeg), 0, jpeg.len() as u64).unwrap();
            assert_eq!(jpeg_exif(&head), Some(&exif[6..]));
        }
        let at = 100u64;
        for (i, jpeg) in files.iter().enumerate() {
            for len in [jpeg.len() as u64, 30, 1 << 20] {
                let mut file = vec![0u8; at as usize];
                file.extend_from_slice(jpeg);
                let end = (at + len.min(JPEG_HEAD)).min(file.len() as u64) as usize;
                let whole = file[at as usize..end].to_vec();
                let head = jpeg_head(&mut Cursor::new(&file), at, len).unwrap();
                assert_eq!(jpeg_exif(&head), jpeg_exif(&whole), "file {i}, {len} bytes");
            }
        }
    }

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

    /// A damaged DNG's merge-tag read says so, apart from the style
    /// read's "maker note".
    #[test]
    fn a_damaged_merge_tag_read_says_merge_tag() {
        let mut bytes = b"II*\0".to_vec();
        bytes.extend_from_slice(&1000u32.to_le_bytes());
        let Err(Error::Decode(message)) = merge_tag_stream(Cursor::new(bytes)) else {
            panic!("an IFD past the end is an error");
        };
        assert!(message.starts_with("merge tag: "), "{message}");
    }

    /// A CMT3 box's TIFF holding the four records the capture modes
    /// are read from, each value at the index exiv2's tables give it
    /// and every other element a marker that is never the answer.
    pub(crate) fn mode_note(shutter: i16, htp: i32, lenr: i32, dpr: i32, hdr: i32) -> Vec<u8> {
        mode_note_as(true, [shutter, 0], [htp, lenr, dpr, hdr], None, false)
    }

    /// [`mode_note`] in either byte order (`modes` HTP, LENR, DPR, and
    /// HDR), with an `extra` entry of the tag and type given, count 1,
    /// whose value is the offset of a one-entry directory after the data,
    /// and with the next-directory word pointing to that one when
    /// `next`. Every offset in it lies inside the note.
    fn mode_note_as(
        le: bool,
        [shutter, _]: [i16; 2],
        [htp, lenr, dpr, hdr]: [i32; 4],
        extra: Option<(u16, u16)>,
        next: bool,
    ) -> Vec<u8> {
        let u16b = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let u32b = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let i16b = |v: i16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let i32b = |v: i32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        // (tag, type, count, bytes)
        let mut file_info = [-7i16; 24];
        file_info[23] = shutter;
        let mut lighting = [-7i32; 12];
        lighting[3] = htp;
        lighting[4] = lenr;
        lighting[11] = dpr;
        let entries: Vec<(u16, u16, u32, Vec<u8>)> = vec![
            (
                0x0093,
                8,
                24,
                file_info.iter().flat_map(|&v| i16b(v)).collect(),
            ),
            (
                0x009a,
                4,
                5,
                [0u32, 6000, 4000, 0, 0]
                    .iter()
                    .flat_map(|&v| u32b(v))
                    .collect(),
            ),
            (
                0x4018,
                9,
                12,
                lighting.iter().flat_map(|&v| i32b(v)).collect(),
            ),
            (
                0x4025,
                9,
                2,
                [-7i32, hdr].iter().flat_map(|&v| i32b(v)).collect(),
            ),
        ];
        let n = entries.len() + usize::from(extra.is_some());
        let ifd_len = 2 + n * 12 + 4;
        let empty_at = (8 + ifd_len + entries.iter().map(|e| e.3.len()).sum::<usize>()) as u32;
        let mut out = if le { b"II*\0" } else { b"MM\0*" }.to_vec();
        out.extend_from_slice(&u32b(8));
        let mut data_at = 8 + ifd_len;
        let mut data = Vec::new();
        out.extend_from_slice(&u16b(n as u16));
        for (tag, typ, count, bytes) in &entries {
            out.extend_from_slice(&u16b(*tag));
            out.extend_from_slice(&u16b(*typ));
            out.extend_from_slice(&u32b(*count));
            out.extend_from_slice(&u32b(data_at as u32));
            data_at += bytes.len();
            data.extend_from_slice(bytes);
        }
        if let Some((tag, typ)) = extra {
            out.extend_from_slice(&u16b(tag));
            out.extend_from_slice(&u16b(typ));
            out.extend_from_slice(&u32b(1));
            out.extend_from_slice(&u32b(empty_at));
        }
        out.extend_from_slice(&u32b(if next { empty_at } else { 0 }));
        out.extend_from_slice(&data);
        assert_eq!(out.len(), empty_at as usize);
        if extra.is_none() && !next {
            return out;
        }
        // The directory pointed to: one short (rawler wants an entry),
        // no next.
        out.extend_from_slice(&u16b(1));
        out.extend_from_slice(&u16b(0x0001));
        out.extend_from_slice(&u16b(3));
        out.extend_from_slice(&u32b(1));
        out.extend_from_slice(&u32b(0));
        out.extend_from_slice(&u32b(0));
        out
    }

    /// A note rawler reads but [`note_fits`] refuses has no modes, so a
    /// note pointing anywhere but its own values is never walked; one
    /// in big-endian order is read as the little-endian one. Without
    /// the check each refused note here would be read.
    #[test]
    fn a_note_pointing_past_its_values_is_refused() {
        let read = |note: &[u8]| {
            let tiff = GenericTiffReader::new_with_buffer(note, 0, 0, None).unwrap();
            modes_of(&tiff)
        };
        let on = ([2, 0], [1, 2, 1, 1]);
        let want = canon_modes_of_note(&mode_note(2, 1, 2, 1, 1)).unwrap();
        let big = mode_note_as(false, on.0, on.1, None, false);
        assert_eq!(read(&big), want);
        assert_eq!(canon_modes_of_note(&big), Some(want));
        for (what, extra, next) in [
            ("a next directory", None, true),
            ("SubIFDs", Some((0x014a, 4)), false),
            ("the Exif pointer", Some((0x8769, 4)), false),
            ("a directory value", Some((0x7777, 13)), false),
        ] {
            for le in [true, false] {
                let note = mode_note_as(le, on.0, on.1, extra, next);
                assert_eq!(read(&note), want, "{what}: rawler reads it");
                assert_eq!(canon_modes_of_note(&note), None, "{what}");
            }
        }
    }

    /// Each mode is read at its own index, as exiv2's tables have it:
    /// a value moved to a neighbor is read as the marker, never as off.
    #[test]
    fn the_capture_modes_are_read_at_their_indexes() {
        let modes = canon_modes_of_note(&mode_note(2, 1, 2, 1, 1)).unwrap();
        assert_eq!(
            modes,
            CanonModes {
                shutter: Some(2),
                highlight_tone_priority: Some(1),
                long_exposure_nr: Some(2),
                dual_pixel_raw: Some(1),
                hdr: Some(1),
                aspect: Some((0, 6000, 4000)),
            }
        );
        let off = canon_modes_of_note(&mode_note(0, 0, 0, 0, 0)).unwrap();
        assert_eq!(off.highlight_tone_priority, Some(0));
        assert_eq!(off.hdr, Some(0));
        // Not a TIFF, no modes; a TIFF with none of the records, none said.
        assert_eq!(canon_modes_of_note(b"not a tiff"), None);
        let empty = canon_modes_of_note(&mode_note(0, 0, 0, 0, 0)[..8]);
        assert!(empty.is_none_or(|m| m.shutter.is_none() && m.aspect.is_none()));
    }

    /// The fuzz target's checks on its seed cut short at every length
    /// and with every byte flipped: never a panic, the same answer twice.
    #[test]
    fn mutated_notes_never_panic() {
        let seed = mode_note(2, 1, 2, 1, 1);
        for n in 0..seed.len() {
            let _ = canon_modes_of_note(&seed[..n]);
        }
        for i in 0..seed.len() {
            for bit in 0..8 {
                let mut m = seed.clone();
                m[i] ^= 1 << bit;
                assert_eq!(canon_modes_of_note(&m), canon_modes_of_note(&m));
            }
        }
    }

    /// Writes the fuzz target's seed (a note with every mode on) into the
    /// folder `GREYCARD_MODES_SEEDS` names.
    #[test]
    #[ignore]
    fn write_mode_seeds() {
        let Some(dir) = std::env::var_os("GREYCARD_MODES_SEEDS") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::write(dir.join("seed-note-modes-on"), mode_note(2, 1, 2, 1, 1)).unwrap();
        std::fs::write(dir.join("seed-note-modes-off"), mode_note(1, 0, 0, 0, 0)).unwrap();
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
        assert_eq!(Maker::from_make("OLYMPUS IMAGING CORP."), Maker::Olympus);
        assert_eq!(Maker::from_make("OLYMPUS CORPORATION"), Maker::Olympus);
        assert_eq!(Maker::from_make("OLYMPUS OPTICAL CO.,LTD"), Maker::Olympus);
        assert_eq!(
            Maker::from_make("OM Digital Solutions   "),
            Maker::OmDigital
        );
        assert_eq!(
            Maker::from_make("RICOH IMAGING COMPANY, LTD."),
            Maker::Other
        );
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

    /// The scrubbed heads of five OM-1 ORFs, one a Picture Mode, all
    /// with Gradation Normal, no Art Filter, no scene mode and shading
    /// compensation off: each a fixed style and a group of its own.
    /// Each head is cut at 12,632 bytes, after the last Camera Settings
    /// value read, with every byte zeroed but the TIFF header, the
    /// tag, type and count of each entry of IFD0, the Exif IFD, the
    /// maker note's IFD and its Camera Settings IFD, the value fields
    /// the reader follows or reads, the Make, the note's 16-byte header
    /// and the Camera Settings values read: about 735 bytes not zero,
    /// and no picture, thumbnail, date or serial number.
    #[test]
    fn olympus_om1_heads_read_their_picture_modes() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        for (file, name) in [
            ("monotone", "Monotone"),
            ("muted", "Muted"),
            ("natural", "Natural"),
            ("portrait", "Portrait"),
            ("vivid", "Vivid"),
        ] {
            let path = fixtures.join(format!("scrubbed-om1-{file}-head.ORF"));
            let s = CameraStyle::read(&path).unwrap().unwrap();
            assert_eq!(s.maker, Maker::OmDigital, "{file}");
            assert_eq!(s.style.as_deref(), Some(name), "{file}");
            assert_eq!(
                s.adaptive,
                vec![
                    Adaptive::known("Gradation", false),
                    Adaptive::known("Art Filter", false).replacing(),
                    Adaptive::known("Magic Filter", false).replacing(),
                    Adaptive::known("Scene Mode", false).replacing(),
                    Adaptive::known("Stacked Image", false).replacing(),
                ],
                "{file}"
            );
            assert_eq!(s.peripheral_correction, Some(false), "{file}");
            assert!(s.is_fixed(), "{file}");
            assert_eq!(
                s.group_key(),
                Some(format!("OM Digital Solutions {name}")),
                "{file}"
            );
        }
    }

    fn sshorts(tag: u16, v: &[i16]) -> (u16, u16, u32, Vec<u8>) {
        (tag, SSHORT, v.len() as u32, i16s(v))
    }

    fn shorts(tag: u16, v: &[u16]) -> (u16, u16, u32, Vec<u8>) {
        let bytes = v.iter().flat_map(|n| n.to_le_bytes()).collect();
        (tag, SHORT, v.len() as u32, bytes)
    }

    /// Camera Settings as the OM-1 heads carry them: Vivid, Gradation
    /// Normal, no Art Filter, Standard (no scene), a single frame, no
    /// highlight or shadow control, shading compensation off.
    fn olympus_base() -> Vec<(u16, u16, u32, Vec<u8>)> {
        let mut levels = vec![0i16; 28];
        for (i, kind) in [-31999i16, -31998, -31997].into_iter().enumerate() {
            levels[4 * i..4 * i + 4].copy_from_slice(&[kind, 0, -7, 7]);
        }
        vec![
            shorts(OLYMPUS_PICTURE_MODE, &[1, 2]),
            sshorts(OLYMPUS_GRADATION, &[0, -1, 1, 0]),
            shorts(OLYMPUS_ART_FILTER, &[0, 0, 0, 0]),
            short(OLYMPUS_SCENE_MODE, 0),
            short(OLYMPUS_SHADING_COMPENSATION, 0),
            (OLYMPUS_STACKED_IMAGE, LONG, 2, vec![0; 8]),
            sshorts(OLYMPUS_TONE_LEVEL, &levels),
        ]
    }

    /// An ORF with a maker note of the given header ("OM SYSTEM",
    /// "OLYMPUS" or the first bodies' "OLYMP") whose Camera Settings
    /// hold `cs`, little-endian.
    fn orf_with(make: &str, header: &'static [u8], cs: &[(u16, u16, u32, Vec<u8>)]) -> Vec<u8> {
        let cs = cs.to_vec();
        raw_file(make, move |at| {
            // OLYMP's offsets count from the file's start, the others'
            // from the note's.
            let base = if header.starts_with(b"OLYMP\0") {
                at
            } else {
                0
            };
            let main_at = header.len();
            let cs_at = main_at + 18;
            let mut note = header.to_vec();
            note.extend(ifd_bytes(
                base + main_at,
                &[long(OLYMPUS_CAMERA_SETTINGS, (base + cs_at) as u32)],
            ));
            note.extend(ifd_bytes(base + cs_at, &cs));
            note
        })
    }

    const OM_SYSTEM: &[u8] = b"OM SYSTEM\0\0\0II\x04\0";

    fn orf(cs: &[(u16, u16, u32, Vec<u8>)]) -> CameraStyle {
        read_bytes(orf_with("OM Digital Solutions", OM_SYSTEM, cs))
            .unwrap()
            .unwrap()
    }

    fn olympus_with(tag: u16, entry: Option<(u16, u16, u32, Vec<u8>)>) -> CameraStyle {
        let mut entries: Vec<_> = olympus_base().into_iter().filter(|e| e.0 != tag).collect();
        entries.extend(entry);
        orf(&entries)
    }

    #[test]
    fn olympus_vivid_with_gradation_normal_is_fixed() {
        let s = orf(&olympus_base());
        assert_eq!(s.style.as_deref(), Some("Vivid"));
        assert!(s.is_fixed());
        assert_eq!(s.group_key().as_deref(), Some("OM Digital Solutions Vivid"));
        // The same note under an Olympus header and Make.
        for header in [&b"OLYMPUS\0II\x03\0"[..], &b"OLYMP\0\x01\0"[..]] {
            let bytes = orf_with("OLYMPUS IMAGING CORP.", header, &olympus_base());
            let s = read_bytes(bytes).unwrap().unwrap();
            assert_eq!(s.maker, Maker::Olympus);
            assert_eq!(s.group_key().as_deref(), Some("Olympus Vivid"));
            assert!(s.is_fixed());
        }
    }

    #[test]
    fn olympus_gradation_bends_the_tone_and_what_it_cannot_name_is_not_known() {
        let gradation = |v: &[i16]| {
            olympus_with(OLYMPUS_GRADATION, Some(sshorts(OLYMPUS_GRADATION, v))).adaptive[0]
        };
        // Low Key and High Key: on, a tone curve; the frame keeps its
        // group and is not fixed.
        for v in [[-1, -1, 1, 0], [1, -1, 1, 0]] {
            assert_eq!(gradation(&v), Adaptive::known("Gradation", true), "{v:?}");
            let s = olympus_with(OLYMPUS_GRADATION, Some(sshorts(OLYMPUS_GRADATION, &v)));
            assert!(!s.is_fixed());
            assert_eq!(s.group_key().as_deref(), Some("OM Digital Solutions Vivid"));
        }
        // The older bodies' three values.
        assert_eq!(gradation(&[0, -1, 1]), Adaptive::known("Gradation", false));
        assert_eq!(
            gradation(&[0, -1, 1, 0]),
            Adaptive::known("Gradation", false)
        );
        // Auto (an OM-1's "0 -1 1 1") and any key the camera chose: on,
        // the frame in its group as High Key's is.
        for v in [[0, -1, 1, 1], [-1, -1, 1, 1], [1, -1, 1, 1]] {
            assert_eq!(gradation(&v), Adaptive::known("Gradation", true), "{v:?}");
            let s = olympus_with(OLYMPUS_GRADATION, Some(sshorts(OLYMPUS_GRADATION, &v)));
            assert!(!s.is_fixed());
            assert_eq!(s.group_key().as_deref(), Some("OM Digital Solutions Vivid"));
        }
        // A value exiv2 does not name, and no tag.
        for v in [
            &[2, -1, 1, 0][..],
            &[2, -1, 1, 1],
            &[0, -1, 1, 2],
            &[0, -1, 1, -1],
            &[0, 0, 0, 0],
            &[0, -1, 1, 1, 0],
        ] {
            assert_eq!(gradation(v), Adaptive::unknown("Gradation"), "{v:?}");
        }
        let none = olympus_with(OLYMPUS_GRADATION, None);
        assert_eq!(none.adaptive[0], Adaptive::unknown("Gradation"));
        assert_eq!(none.group_key(), None);
    }

    #[test]
    fn olympus_art_filters_scenes_and_stacks_take_the_frame_out_of_its_group() {
        for (tag, entry, index) in [
            (
                OLYMPUS_ART_FILTER,
                shorts(OLYMPUS_ART_FILTER, &[2, 0, 0, 0]),
                1,
            ),
            (
                OLYMPUS_ART_FILTER,
                shorts(OLYMPUS_ART_FILTER, &[39, 0, 0, 2]),
                1,
            ),
            (
                OLYMPUS_MAGIC_FILTER,
                shorts(OLYMPUS_MAGIC_FILTER, &[12, 0, 0, 0]),
                2,
            ),
            (OLYMPUS_SCENE_MODE, short(OLYMPUS_SCENE_MODE, 8), 3),
            (
                OLYMPUS_STACKED_IMAGE,
                (OLYMPUS_STACKED_IMAGE, LONG, 2, i32s(&[5, 4])),
                4,
            ),
            (
                OLYMPUS_STACKED_IMAGE,
                (OLYMPUS_STACKED_IMAGE, LONG, 2, i32s(&[3, 8])),
                4,
            ),
            // An OM-1's hand-held High Res Shot (`.ORF`); its base
            // frame (`.ORI`) says "0 0", as `olympus_base` does.
            (
                OLYMPUS_STACKED_IMAGE,
                (OLYMPUS_STACKED_IMAGE, LONG, 2, i32s(&[11, 12])),
                4,
            ),
        ] {
            let s = olympus_with(tag, Some(entry));
            let a = s.adaptive[index];
            assert!(a.on && a.known, "{tag:#x}");
            assert_eq!(a.kind, AdaptiveKind::Replaces, "{tag:#x}");
            assert_eq!(s.group_key(), None, "{tag:#x}");
        }
        // Codes exiv2 does not name, and a scene mode not written: not
        // known, so of no group.
        for (tag, entry, index) in [
            (
                OLYMPUS_ART_FILTER,
                Some(shorts(OLYMPUS_ART_FILTER, &[7, 0, 0, 0])),
                1,
            ),
            (OLYMPUS_SCENE_MODE, Some(short(OLYMPUS_SCENE_MODE, 21)), 3),
            (OLYMPUS_SCENE_MODE, None, 3),
            (
                OLYMPUS_STACKED_IMAGE,
                Some(short(OLYMPUS_STACKED_IMAGE, 0)),
                4,
            ),
        ] {
            let s = olympus_with(tag, entry);
            assert!(!s.adaptive[index].known, "{tag:#x}");
            assert_eq!(s.group_key(), None, "{tag:#x}");
        }
        // The bodies without Art Filters or the stacked modes do not
        // write them.
        assert!(olympus_with(OLYMPUS_ART_FILTER, None).is_fixed());
        assert!(olympus_with(OLYMPUS_STACKED_IMAGE, None).is_fixed());
    }

    #[test]
    fn olympus_tone_levels_are_listed_only_when_moved() {
        let mut levels = vec![0i16; 28];
        levels[..12].copy_from_slice(&[-31999, -2, -7, 7, -31998, 0, -7, 7, -31997, 3, -7, 7]);
        let s = olympus_with(
            OLYMPUS_TONE_LEVEL,
            Some(sshorts(OLYMPUS_TONE_LEVEL, &levels)),
        );
        assert_eq!(
            s.adaptive[5..],
            [
                Adaptive::known("Highlight Tone", true),
                Adaptive::known("Midtone", true),
            ]
        );
        assert!(!s.is_fixed());
        assert_eq!(s.group_key().as_deref(), Some("OM Digital Solutions Vivid"));
        assert!(olympus_with(OLYMPUS_TONE_LEVEL, None).is_fixed());
    }

    /// Tone Level and Gradation stored as SHORT rather than SSHORT read
    /// the same: a moved highlight is listed, Low Key is on.
    #[test]
    fn olympus_signed_tags_stored_unsigned_read_the_same() {
        let as_short = |tag: u16, v: &[i16]| (tag, SHORT, v.len() as u32, i16s(v));
        let mut levels = vec![0i16; 28];
        levels[..4].copy_from_slice(&[-31999, -2, -7, 7]);
        let s = olympus_with(
            OLYMPUS_TONE_LEVEL,
            Some(as_short(OLYMPUS_TONE_LEVEL, &levels)),
        );
        assert_eq!(s.adaptive[5..], [Adaptive::known("Highlight Tone", true)]);
        assert!(!s.is_fixed());
        let s = olympus_with(
            OLYMPUS_GRADATION,
            Some(as_short(OLYMPUS_GRADATION, &[-1, -1, 1, 0])),
        );
        assert_eq!(s.adaptive[0], Adaptive::known("Gradation", true));
        assert!(!s.is_fixed());
    }

    /// The older bodies' Camera Settings as an UNDEFINED block that is
    /// the IFD itself, its values' offsets from the note's start.
    #[test]
    fn olympus_camera_settings_as_an_undefined_block() {
        let bytes = raw_file("OLYMPUS IMAGING CORP.", |_| {
            let header = b"OLYMPUS\0II\x03\0";
            // The note's IFD at 12, one entry and no inline value: 18
            // bytes, the block's data right after it.
            let cs_at = header.len() + 18;
            let block = ifd_bytes(cs_at, &olympus_base());
            let mut note = header.to_vec();
            note.extend(ifd_bytes(
                header.len(),
                &[(
                    OLYMPUS_CAMERA_SETTINGS,
                    UNDEFINED,
                    block.len() as u32,
                    block,
                )],
            ));
            note
        });
        let s = read_bytes(bytes).unwrap().unwrap();
        assert_eq!(s.group_key().as_deref(), Some("Olympus Vivid"));
        assert!(s.is_fixed());
    }

    /// A big-endian "OLYMPUS" note in a little-endian file.
    #[test]
    fn olympus_big_endian_note() {
        let bytes = raw_file("OLYMPUS CORPORATION", |_| {
            let header = b"OLYMPUS\0MM\x03\0";
            let cs_at = header.len() + 18;
            let be16 = |v: &[i16]| v.iter().flat_map(|n| n.to_be_bytes()).collect::<Vec<_>>();
            let cs = [
                (OLYMPUS_PICTURE_MODE, SHORT, 2, be16(&[1, 2])),
                (OLYMPUS_GRADATION, SSHORT, 4, be16(&[0, -1, 1, 0])),
                (OLYMPUS_ART_FILTER, SHORT, 4, be16(&[0, 0, 0, 0])),
                (OLYMPUS_SCENE_MODE, SHORT, 1, be16(&[0])),
                (OLYMPUS_SHADING_COMPENSATION, SHORT, 1, be16(&[1])),
            ];
            let mut note = header.to_vec();
            note.extend(ifd_bytes_in(
                header.len(),
                &[(
                    OLYMPUS_CAMERA_SETTINGS,
                    LONG,
                    1,
                    (cs_at as u32).to_be_bytes().to_vec(),
                )],
                true,
            ));
            note.extend(ifd_bytes_in(cs_at, &cs, true));
            note
        });
        let s = read_bytes(bytes).unwrap().unwrap();
        assert_eq!(s.style.as_deref(), Some("Vivid"));
        assert_eq!(s.peripheral_correction, Some(true));
        assert!(s.is_fixed());
        assert_eq!(s.group_key().as_deref(), Some("Olympus Vivid"));
    }

    #[test]
    fn olympus_modes_that_adapt_or_are_the_users_own_are_not_fitted() {
        let mode = |code: u16| {
            olympus_with(
                OLYMPUS_PICTURE_MODE,
                Some(shorts(OLYMPUS_PICTURE_MODE, &[code, 2])),
            )
        };
        for (code, name) in [
            (5, "i-Enhance"),
            (6, "e-Portrait"),
            (7, "Color Creator"),
            (9, "Color Profile 1"),
            (14, "Monochrome Profile 3"),
            (8, "Unknown (8)"),
            (17, "Unknown (17)"),
        ] {
            let s = mode(code);
            assert_eq!(s.style.as_deref(), Some(name));
            assert_eq!(s.group_key(), None, "{name}");
        }
        for (code, name) in [
            (2, "Natural"),
            (3, "Muted"),
            (4, "Portrait"),
            (512, "Sepia"),
        ] {
            assert_eq!(
                mode(code).group_key(),
                Some(format!("OM Digital Solutions {name}"))
            );
        }
        assert_eq!(olympus_with(OLYMPUS_PICTURE_MODE, None).style, None);
    }

    #[test]
    fn olympus_monotone_takes_its_filter_and_tone() {
        let monotone = |filter: Option<i16>, tone: Option<i16>| {
            let mut entries: Vec<_> = olympus_base()
                .into_iter()
                .filter(|e| e.0 != OLYMPUS_PICTURE_MODE)
                .collect();
            entries.push(shorts(OLYMPUS_PICTURE_MODE, &[256, 2]));
            entries.extend(filter.map(|v| sshorts(OLYMPUS_BW_FILTER, &[v])));
            entries.extend(tone.map(|v| sshorts(OLYMPUS_PICTURE_TONE, &[v])));
            orf(&entries)
        };
        for (filter, tone, name) in [
            (1, 1, "Monotone"),
            (4, 1, "Monotone + Red Filter"),
            (1, 2, "Monotone + Sepia Tone"),
            (2, 3, "Monotone + Yellow Filter + Blue Tone"),
        ] {
            let s = monotone(Some(filter), Some(tone));
            assert_eq!(s.style.as_deref(), Some(name));
            assert!(s.is_fixed(), "{name}");
        }
        for (filter, tone) in [(Some(0), Some(0)), (Some(9), Some(1)), (None, Some(1))] {
            let s = monotone(filter, tone);
            assert_eq!(s.style.as_deref(), Some("Monotone"));
            assert_eq!(s.group_key(), None, "{filter:?} {tone:?}");
        }
    }

    #[test]
    fn olympus_notes_it_cannot_read_are_bare() {
        // An unknown header, and a note with no Camera Settings.
        let other = raw_file("OM Digital Solutions", |_| {
            b"SOMETHING ELSE\0\0\0\0\0\0".to_vec()
        });
        assert_eq!(
            read_bytes(other).unwrap(),
            Some(CameraStyle::bare(Maker::OmDigital))
        );
        let no_cs = raw_file("OLYMPUS CORPORATION", |_| {
            let mut note = OM_SYSTEM.to_vec();
            note.extend(ifd_bytes(16, &[short(0x0200, 0)]));
            note
        });
        assert_eq!(
            read_bytes(no_cs).unwrap(),
            Some(CameraStyle::bare(Maker::Olympus))
        );
        // A Camera Settings pointer past the end is an error.
        let past = raw_file("OM Digital Solutions", |_| {
            let mut note = OM_SYSTEM.to_vec();
            note.extend(ifd_bytes(16, &[long(OLYMPUS_CAMERA_SETTINGS, 1 << 30)]));
            note
        });
        assert!(read_bytes(past).is_err());
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

    /// An ORF whose note's Image Processing holds the valid area at
    /// (8, 8), 5184 by 3888, the aspect, and `frame` as its
    /// AspectFrame (none left out), under `header`, little-endian.
    fn orf_framed(make: &str, header: &'static [u8], frame: Option<[u16; 4]>) -> Vec<u8> {
        let mut ip = vec![
            shorts(OLYMPUS_CROP_LEFT, &[8, 0]),
            shorts(OLYMPUS_CROP_TOP, &[8, 0]),
            long(OLYMPUS_CROP_WIDTH, 5184),
            long(OLYMPUS_CROP_HEIGHT, 3888),
            (OLYMPUS_ASPECT_FRAME - 1, 1, 2, vec![3, 2]),
        ];
        ip.extend(frame.map(|f| shorts(OLYMPUS_ASPECT_FRAME, &f)));
        raw_file(make, move |at| {
            let base = if header.starts_with(b"OLYMP\0") {
                at
            } else {
                0
            };
            let main_at = header.len();
            let ip_at = main_at + 18;
            let mut note = header.to_vec();
            note.extend(ifd_bytes(
                base + main_at,
                &[long(OLYMPUS_IMAGE_PROCESSING, (base + ip_at) as u32)],
            ));
            note.extend(ifd_bytes(base + ip_at, &ip));
            note
        })
    }

    fn frame_of(bytes: Vec<u8>) -> Option<Rect> {
        let read = frame_stream(Cursor::new(bytes)).unwrap();
        if let Some(c) = read {
            let area = Rect {
                x: 8,
                y: 8,
                width: 5184,
                height: 3888,
            };
            assert_eq!(c.area, area);
        }
        read.map(|c| c.frame)
    }

    /// A frame shot at 3:2 on the 4:3 sensor is the area's middle
    /// band, 5184 by 3456, as a rectangle of the sensor, under each
    /// header; one at 1:1 the middle square.
    #[test]
    fn an_olympus_frame_shot_at_a_ratio_reads_as_that_part_of_the_sensor() {
        let band = Some(Rect {
            x: 8,
            y: 8 + 216,
            width: 5184,
            height: 3456,
        });
        let frame = Some([0, 216, 5183, 3671]);
        assert_eq!(
            frame_of(orf_framed("OM Digital Solutions", OM_SYSTEM, frame)),
            band
        );
        for header in [&b"OLYMPUS\0II\x03\0"[..], &b"OLYMP\0\x01\0"[..]] {
            assert_eq!(
                frame_of(orf_framed("OLYMPUS CORPORATION", header, frame)),
                band
            );
        }
        // On the stream the style reader read first, as the index
        // reads a raw's head once for both: from the start all the same.
        let mut stream = Cursor::new(orf_framed("OM Digital Solutions", OM_SYSTEM, frame));
        read_stream(&mut stream).unwrap();
        stream.set_position(10);
        assert_eq!(frame_stream(&mut stream).unwrap().map(|c| c.frame), band);
        let square = Some([648, 0, 4535, 3887]);
        assert_eq!(
            frame_of(orf_framed("OLYMPUS CORPORATION", OM_SYSTEM, square)),
            Some(Rect {
                x: 8 + 648,
                y: 8,
                width: 3888,
                height: 3888,
            })
        );
    }

    /// A frame is taken only where the note's numbers are plausibly of
    /// one picture: the area inside the image's declared size, and the
    /// frame spanning the area's whole width or whole height.
    #[test]
    fn an_olympus_frame_of_numbers_not_of_one_picture_is_none() {
        let area = [8, 8, 5184, 3888];
        let band = [0, 216, 5183, 3671];
        assert!(olympus_frame(area, band, None).is_some());
        assert!(olympus_frame(area, band, Some((5200, 3904))).is_some());
        // The area past the declared image: another picture's numbers.
        assert!(olympus_frame(area, band, Some((2600, 1952))).is_none());
        // A frame spanning neither side, as a frame in other units would.
        assert!(olympus_frame(area, [0, 0, 2591, 1727], None).is_none());
        // 1:1 spans the height.
        assert!(olympus_frame(area, [648, 0, 4535, 3887], Some((5200, 3904))).is_some());
    }

    /// A frame shot at the sensor's own 4:3 is the whole area and says
    /// nothing; nor does a frame outside the area, a note without the
    /// tag, another maker's file, a DNG, or a file that is no TIFF.
    #[test]
    fn an_olympus_frame_that_is_the_whole_area_or_unread_is_none() {
        let make = "OM Digital Solutions";
        for frame in [
            Some([0, 0, 5183, 3887]),
            Some([0, 216, 5184, 3671]),
            Some([10, 216, 9, 3671]),
            None,
        ] {
            assert_eq!(
                frame_of(orf_framed(make, OM_SYSTEM, frame)),
                None,
                "{frame:?}"
            );
        }
        let frame = Some([0, 216, 5183, 3671]);
        assert_eq!(
            frame_of(orf_framed("NIKON CORPORATION", OM_SYSTEM, frame)),
            None
        );
        assert_eq!(frame_of(orf_with(make, OM_SYSTEM, &olympus_base())), None);
        let dng = raw_file_with(make, &[(TAG_DNG_VERSION, 1, 4, vec![1, 6, 0, 0])], |at| {
            let mut note = OM_SYSTEM.to_vec();
            note.extend(ifd_bytes(at + OM_SYSTEM.len(), &[]));
            note
        });
        assert_eq!(frame_of(dng), None);
        assert_eq!(frame_of(b"\0\0\0\x0cftypcrx ".to_vec()), None);
    }

    /// With `GREYCARD_FRAME_FILE` a raw: its camera frame, printed, for
    /// checking the reader against exiv2 on a real file.
    #[test]
    #[ignore]
    fn the_camera_frame_of_a_file() {
        let Some(path) = std::env::var_os("GREYCARD_FRAME_FILE") else {
            return;
        };
        println!("{:?}", crate::decode::camera_frame_path(&path));
    }
}
