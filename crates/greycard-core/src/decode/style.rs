//! The maker's rendering settings on a raw: the picture style or film
//! simulation the camera's own JPEG was rendered with, the
//! scene-adaptive settings that bend it frame by frame, and whether the
//! camera corrected the lens's vignetting.
//!
//! The camera match groups frames by these and fits only the frames a
//! fixed style rendered, so each value here is read from the maker note
//! as exiv2 reads it, and checked against exiv2 on real files. rawler
//! decodes the file but does not expose the maker note's contents, so
//! the boxes and IFDs are walked here with rawler's own `formats::bmff`
//! and `formats::tiff` parsers.
//!
//! Canon CR3 and Fujifilm RAF are read in full. Nikon, Sony and
//! Panasonic files are recognized and named, with no style: their tag
//! tables are not read until they can be checked against files.
//!
//! A setting the reader expects and cannot find or cannot name counts
//! as on, marked `known: false`, so a frame is never called fixed on a
//! guess. What the reader does not see yet: the per-style tweaks that
//! change the rendering without adapting to the scene, Canon's
//! Contrast, Saturation and Color tone for the style, and Fujifilm's
//! Color (saturation) beside a film simulation, Color Chrome Effect,
//! Color Chrome FX Blue and Clarity. A group may mix frames that differ
//! in them.
//!
//! The tag numbers, array indexes and the names of their values follow
//! exiv2's maker note tables, `src/canonmn_int.cpp` and
//! `src/fujimn_int.cpp` at <https://github.com/Exiv2/exiv2>,
//! GPL-2.0-or-later, copyright the Exiv2 authors. The names were taken
//! from exiv2's own output on real and exiv2-written files; the reading
//! code is ours.

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

/// One of the maker's scene-adaptive settings and whether it was on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adaptive {
    pub name: &'static str,
    pub on: bool,
    /// False when the file did not carry the setting or carried a value
    /// the reader cannot name; `on` is then true, so the frame is not
    /// taken as fixed.
    pub known: bool,
}

impl Adaptive {
    fn known(name: &'static str, on: bool) -> Adaptive {
        Adaptive {
            name,
            on,
            known: true,
        }
    }

    fn unknown(name: &'static str) -> Adaptive {
        Adaptive {
            name,
            on: true,
            known: false,
        }
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
    /// file that could not be read, a CR3 or RAF whose boxes or maker
    /// note are damaged, or a file that begins like a TIFF (`II`/`MM`)
    /// and is not a valid one.
    pub fn read(path: &Path) -> Result<Option<CameraStyle>> {
        guarded(|| tail_of(path), || read_unguarded(path))
    }

    /// A fixed style with every adaptive setting off: frames with this
    /// can be fitted.
    pub fn is_fixed(&self) -> bool {
        self.group_key().is_some() && self.adaptive.iter().all(|a| !a.on)
    }

    /// The name frames are grouped under for a fit, the maker and the
    /// style: "Canon Faithful", "Fujifilm Reala Ace". `None` when there
    /// is no style or the style is itself adaptive. The key does not
    /// name the body; a caller fitting per body adds the model.
    pub fn group_key(&self) -> Option<String> {
        match &self.style {
            Some(style) if self.style_fixed => Some(format!("{} {style}", self.maker.name())),
            _ => None,
        }
    }
}

fn read_unguarded(path: &Path) -> Result<Option<CameraStyle>> {
    let mut file = BufReader::new(File::open(path)?);
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
        let ifd0 = tiff_ifd0(&mut file)?;
        let make = ifd0
            .get_entry(TAG_MAKE)
            .and_then(|e| e.value.as_string().cloned())
            .unwrap_or_default();
        return Ok(match Maker::from_make(&make) {
            Maker::Other => None,
            maker => Some(CameraStyle::bare(maker)),
        });
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

/// A TIFF's first IFD and nothing else: no chained IFDs, no sub-IFDs.
/// rawler's `GenericTiffReader` follows the next-IFD chain with no
/// cycle check when it is given no limit, so a file whose IFD0 points
/// back at itself grows the chain until memory runs out, a hang the
/// panic guard cannot catch. Only IFD0's Make is wanted here.
fn tiff_ifd0(r: &mut (impl Read + Seek)) -> Result<IFD> {
    let mut header = [0u8; 8];
    r.seek(SeekFrom::Start(0))?;
    r.read_exact(&mut header)?;
    let endian = match &header[..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => return Err(note_error("not a TIFF")),
    };
    let word = |b: [u8; 2]| match endian {
        Endian::Little => u16::from_le_bytes(b),
        Endian::Big => u16::from_be_bytes(b),
    };
    // 42, or Panasonic's 0x55 and Olympus's two, as rawler takes them.
    if !matches!(word([header[2], header[3]]), 42 | 0x55 | 0x4f52 | 0x5352) {
        return Err(note_error("not a TIFF"));
    }
    let bytes = [header[4], header[5], header[6], header[7]];
    let offset = match endian {
        Endian::Little => u32::from_le_bytes(bytes),
        Endian::Big => u32::from_be_bytes(bytes),
    };
    IFD::new(r, offset, 0, 0, endian, &[]).map_err(note_error)
}

const TAG_MAKE: u16 = 0x010f;
const TAG_EXIF_IFD: u16 = 0x8769;
const TAG_MAKER_NOTE: u16 = 0x927c;

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
        let mut entries = entries.to_vec();
        entries.sort_by_key(|e| e.0);
        let table = 2 + 12 * entries.len() + 4;
        let mut out = Vec::new();
        let mut data = Vec::new();
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, typ, count, bytes) in &entries {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&typ.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            if bytes.len() <= 4 {
                let mut inline = bytes.clone();
                inline.resize(4, 0);
                out.extend_from_slice(&inline);
            } else {
                let at = start + table + data.len();
                out.extend_from_slice(&(at as u32).to_le_bytes());
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
        assert_eq!(s.group_key().as_deref(), Some("Canon Faithful"));
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
            fuji(&[short(FUJI_COLOR, 0x300)]).group_key().as_deref(),
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
        let ifd = tiff_ifd0(&mut Cursor::new(&bytes)).unwrap();
        assert_eq!(ifd.entries.len(), 1);
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
        assert!(tiff_ifd0(&mut Cursor::new(b"II\x01\x02garbage")).is_err());
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
}
