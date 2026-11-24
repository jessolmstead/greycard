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
    BlackDeltas, Calibration, CfaColor, CfaPattern, DngTags, LevelPattern, Levels, Mosaic,
    Orientation, RawFrame, Rect, Samples, SensorLayout, Shot,
};

/// rawler's `Unsupported` prints all four of its fields whether or not
/// they have anything in them, so a file that is no camera's raw comes
/// out as `Error: No decoder found, model '', make: '', mode: ''`.
/// Say what happened instead, and name the camera only when there is
/// one to name.
fn decoder_error(e: ::rawler::RawlerError) -> Error {
    if is_nikon_high_efficiency(&e) {
        return Error::Unsupported(NIKON_HE.into());
    }
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

/// The decoder's cameras, each model as the file's EXIF spells it
/// beside the name the index gives it, read once from rawler's table
/// of cameras (its aliases among them).
fn camera_names() -> &'static [(String, String)] {
    static NAMES: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names: Vec<(String, String)> = ::rawler::global_loader()
            .get_cameras()
            .values()
            .map(|c| {
                (
                    c.model.trim().to_string(),
                    crate::raw::camera_name(&c.clean_make, &c.clean_model),
                )
            })
            .filter(|(m, n)| !m.is_empty() && !n.is_empty())
            .collect();
        names.sort();
        names.dedup();
        names
    })
}

/// A body's names in the decoder's table of cameras: the make as the
/// EXIF spells it and cleaned, and each model the same way, its own
/// first and then the names it is sold under elsewhere.
struct Names {
    make: String,
    clean_make: String,
    models: Vec<(String, String)>,
}

/// The bodies with more than one name. rawler's `Camera` keeps no
/// aliases (its loader makes each alias a camera of its own), so we
/// read them from the table it is built from, which it publishes.
fn aliased_bodies() -> &'static [Names] {
    static BODIES: std::sync::OnceLock<Vec<Names>> = std::sync::OnceLock::new();
    BODIES.get_or_init(|| {
        let table = match ::rawler::decoders::CAMERAS_TOML.parse::<toml::Table>() {
            Ok(t) => t,
            Err(e) => {
                log::warn!("the decoder's table of cameras: {e}");
                return Vec::new();
            }
        };
        let text = |t: &toml::Table, k: &str| {
            t.get(k)
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        };
        let cameras = table.get("cameras").and_then(|c| c.as_array());
        cameras
            .into_iter()
            .flatten()
            .filter_map(|c| c.as_table())
            .filter_map(|c| {
                let aliases = c.get("model_aliases")?.as_array()?;
                let mut models = vec![(text(c, "model"), text(c, "clean_model"))];
                models.extend(aliases.iter().filter_map(|a| {
                    let a = a.as_array()?;
                    let name = |i: usize| a.get(i)?.as_str().map(|s| s.trim().to_string());
                    Some((name(0)?, name(1)?))
                }));
                (models.len() > 1).then(|| Names {
                    make: text(c, "make"),
                    clean_make: text(c, "clean_make"),
                    models,
                })
            })
            .collect()
    })
}

/// The other names of the body `make` `model`, each spelled either
/// way (as the EXIF has it or cleaned): the names the decoder's table
/// gives the same camera, its own among them when `model` is one of
/// the others, each both as the EXIF spells it and cleaned. Both, so
/// a body named by its EXIF make ("NIKON CORPORATION") fits the same
/// profiles as one named by its clean make: the EXIF model carries the
/// make ("NIKON D800E") where the clean one does not. Empty for a body
/// with one name, or one the table does not know.
pub fn other_names(make: &str, model: &str) -> Vec<String> {
    let same = |a: &str, b: &str| !b.is_empty() && a.trim().eq_ignore_ascii_case(b);
    let Some(body) = aliased_bodies().iter().find(|b| {
        (same(make, &b.make) || same(make, &b.clean_make))
            && b.models
                .iter()
                .any(|(m, c)| same(model, m) || same(model, c))
    }) else {
        return Vec::new();
    };
    let mut out: Vec<String> = body
        .models
        .iter()
        .filter(|(m, c)| !same(model, m) && !same(model, c))
        .flat_map(|(m, c)| [m.clone(), c.clone()])
        .filter(|n| !n.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The camera as the index names it for a model as its EXIF spells it;
/// none for a model the decoder does not know. A model two makers share
/// (none in the table as we read it) takes the first by name.
pub fn cleaned_camera(exif_model: &str) -> Option<String> {
    let model = exif_model.trim();
    camera_names()
        .iter()
        .find(|(m, _)| m.eq_ignore_ascii_case(model))
        .map(|(_, n)| n.clone())
}

/// Every EXIF spelling the decoder knows for a camera as the index
/// names it.
pub fn exif_models(camera: &str) -> Vec<String> {
    let camera = camera.trim();
    let mut out: Vec<String> = camera_names()
        .iter()
        .filter(|(_, n)| n.eq_ignore_ascii_case(camera))
        .map(|(m, _)| m.clone())
        .collect();
    out.dedup();
    out
}

/// What a Nikon High Efficiency raw says in a build without the
/// decoder for it.
const NIKON_HE: &str = "Nikon High Efficiency raw (HE or HE*), not supported in this build";

/// rawler refuses HE and HE* with `NEF compression
/// Some(HighEfficency) is not supported`, or `HighEfficencyStar`, when
/// it has no JPEG XS decoder built in. The text is the only thing that
/// tells this refusal from any other decode failure. The match stops
/// at `HighEffic` so that rawler correcting its spelling keeps it.
fn is_nikon_high_efficiency(e: &::rawler::RawlerError) -> bool {
    matches!(e, ::rawler::RawlerError::DecoderFailed(msg)
        if msg.starts_with("NEF compression") && msg.contains("HighEffic"))
}

/// Every call into rawler goes through here. rawler panics on some
/// damaged files where it should return an error — a CR3 whose tail
/// is zeros, as a copy in progress leaves it, overflows a size in its
/// CTMD reader (dnglab/dnglab#849), and a NEF, RAF or RW2 in the same
/// state runs its bit pump off the end — and a panic on a caller's
/// thread takes the editor with it when the caller is the UI. So the
/// panic is caught here and becomes the file's decode error, the one
/// every caller already handles. `half_copy` says whether the file
/// looks half-copied, and in what words (see [`half_copied`]), to say
/// what went wrong when that is knowable.
pub(crate) fn guarded<T>(
    half_copy: impl FnOnce() -> Option<&'static str>,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let was = GUARDED.replace(true);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    GUARDED.set(was);
    caught.unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "no message".into());
        Err(Error::Decode(match half_copy() {
            Some(why) => format!("{why} (the decoder gave up: {message})"),
            None => format!("the decoder gave up on this file: {message}"),
        }))
    })
}

thread_local! {
    static GUARDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether this thread is inside a call into rawler, where a panic
/// is caught and becomes an error. For a panic hook: such a panic is
/// a damaged file, not a crash, and the hook can say so.
pub fn inside_decoder() -> bool {
    GUARDED.get()
}

/// How much of a file's end is read to tell a copy in progress.
const TAIL: usize = 64 * 1024;

/// A whole [`TAIL`] of zeros at the end. No raw ends that way: its
/// last bytes are compressed samples, a preview or a maker's records.
/// A CR3 comes closest, with a 77 KiB run of zeros in its CTMD that
/// stops some 24 KiB before the end, which is why this is the whole
/// tail and not a run in it, and not a share of zeros (a CR3's last
/// 64 KiB are 70 to 85% zeros). A copier that sets the length first
/// (Windows' CopyFile, `rsync --preallocate`, some card importers)
/// leaves exactly that until it is done. Zeros only: a preallocated
/// file reads back as zeros whatever wrote it, and nothing leaves a
/// tail of any other one byte.
fn ends_in_zeros(tail: &[u8]) -> bool {
    tail.len() >= TAIL && tail.iter().all(|&b| b == 0)
}

/// What a file that [`ends_in_zeros`] is told with, whether the decoder
/// gave up on it or read it to the end, and an uncompressed RAF whose
/// last cropped row decodes to zeros (see [`judge_rows`]).
const HALF_COPIED: &str = "the file ends in zeros where a finished one has data: \
     it looks half-copied, or still being copied";

/// What a RAF whose header points past its end is told with.
const RAF_HALF_COPIED: &str = "the file's header names data past its end: \
     it looks half-copied, or still being copied";

/// A RAF's first bytes.
pub(crate) const RAF_MAGIC: &[u8] = b"FUJIFILMCCD-RAW ";

/// How much of a RAF's head holds its header: the magic, then at
/// 0x54 six big-endian u32s, the offset and length of the preview
/// JPEG, of the metadata, and of the CFA data.
const RAF_HEAD: usize = 0x6C;

/// The big-endian u32 at `at` in `bytes`, widened.
pub(crate) fn be_u32_at(bytes: &[u8], at: usize) -> u64 {
    u64::from(u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()))
}

/// What a RAF's header says of its sections: where the last of them
/// ends, and where the CFA data ends and how long it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RafSections {
    end: u64,
    cfa_end: u64,
    cfa_len: u64,
}

/// A RAF's sections, from its header; `None` when `head` is not a
/// RAF's or is too short to hold the header.
fn raf_sections(head: &[u8]) -> Option<RafSections> {
    if head.len() < RAF_HEAD || !head.starts_with(RAF_MAGIC) {
        return None;
    }
    let section_end = |at: usize| be_u32_at(head, at) + be_u32_at(head, at + 4);
    let cfa_len = be_u32_at(head, 0x68);
    Some(RafSections {
        end: [0x54, 0x5C, 0x64].map(section_end).into_iter().max()?,
        cfa_end: section_end(0x64),
        cfa_len,
    })
}

/// Whether a file of `len` bytes that starts with `head` and ends with
/// `tail` looks half-copied, and the words to say so in, for when the
/// decoder has panicked on it. A RAF is judged by its header and its
/// sensor data: one whose sections run past its end is half-copied,
/// and so is one whose CFA data ends in [`TAIL`] bytes of zeros, as a
/// copier that sets the length first leaves it; one that holds its
/// sections and whose data does not end so is whole whatever follows
/// them (some bodies' files carry a tail of zeros past the CFA data).
/// `data_tail` reads the [`TAIL`] bytes that end at an offset. Every
/// other file, and a RAF whose header cannot be read, is judged by
/// [`ends_in_zeros`].
///
/// The CFA test stays keyed on zeros here, though up front it no
/// longer refuses an uncompressed RAF (see [`refuse_a_half_copy`]):
/// a whole uncompressed X-Trans 5 HR file ends in zeros too, but the
/// decoder reads it without panicking, so a panic on a RAF whose data
/// ends in zeros is far likelier a preallocated copy than a whole
/// file, and the header alone cannot tell that copy from a finished
/// one.
fn half_copied(
    head: &[u8],
    len: u64,
    tail: &[u8],
    data_tail: impl FnOnce(u64) -> Option<Vec<u8>>,
) -> Option<&'static str> {
    match raf_sections(head) {
        Some(raf) if raf.end > len => Some(RAF_HALF_COPIED),
        Some(raf) => (raf.cfa_len >= TAIL as u64
            && data_tail(raf.cfa_end).is_some_and(|t| ends_in_zeros(&t)))
        .then_some(HALF_COPIED),
        None => ends_in_zeros(tail).then_some(HALF_COPIED),
    }
}

/// What the up-front check leaves to the decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpFront {
    /// Nothing: the bytes say the file is whole.
    Whole,
    /// An uncompressed RAF whose CFA data ends in zeros, which a whole
    /// one may: [`judge_rows`] decides once it is decoded.
    JudgeRows,
}

/// What rawler's dummy read says of a RAF's CFA data: its size, the
/// bits a sample rawler reports (16 for every RAF, from its camera
/// table), and whether it rotates the data (a SuperCCD body).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RafShape {
    width: usize,
    height: usize,
    bps: usize,
    rotated: bool,
}

/// A RAF's [`RafShape`] from the dummy read, which reads no sample.
fn raf_shape(source: &RawSource) -> Option<RafShape> {
    let image = ::rawler::get_decoder(source)
        .ok()?
        .raw_image(source, &RawDecodeParams::default(), true)
        .ok()?;
    Some(RafShape {
        width: image.width,
        height: image.height,
        bps: image.bps,
        rotated: image.fuji_rotation_width.is_some(),
    })
}

/// Refuse a file that looks half-copied, before its samples are
/// decoded. rawler reads a file whose last [`TAIL`] bytes are zeros
/// without complaint when its format lets it: an ARW, and a CR3 once
/// dnglab/dnglab#851 is released, decode the zeros past the copied
/// part as samples, and the picture comes out blank below it. The
/// bytes are already in memory (rawler maps the whole file, or was
/// handed it), so this costs no read.
///
/// A RAF is judged by its header and its CFA data, as [`half_copied`]
/// does, with one exception. An uncompressed X-Trans 5 HR file (X-H2,
/// X-T5) stores rows below the active area that the camera writes as
/// zeros, seven of them on the X-H2, 110 KiB, so every whole one ends
/// its CFA data in more than [`TAIL`] bytes of zeros, as a
/// preallocated copy does. So when the CFA data ends in zeros, `shape`
/// (rawler's dummy read, asked for only then) tells the kind: a RAF
/// stored uncompressed and not rotated is left to [`judge_rows`]
/// after the decode. A compressed one keeps the byte rule: rawler
/// decodes its strips on a pool's threads, where a strip of zeros
/// panics outside this thread's guard, and its strips run top to
/// bottom side by side, so a copy cut short zeros the right of the
/// frame, not its last rows. A rotated one keeps it too, since its
/// rows hold zero corners of their own. A RAF is compressed when its
/// CFA data is shorter than `width * height * bps / 8`, rawler's own
/// test. rawler reports 16 bits for every RAF, so only data stored at
/// two bytes a pixel counts as uncompressed here, and a packed 12- or
/// 14-bit file keeps the byte rule, which is the conservative side.
fn refuse_a_half_copy(bytes: &[u8], shape: impl FnOnce() -> Option<RafShape>) -> Result<UpFront> {
    let refuse = |why: &str| Err(Error::Decode(why.into()));
    match raf_sections(bytes) {
        Some(raf) if raf.end > bytes.len() as u64 => refuse(RAF_HALF_COPIED),
        Some(raf) => match half_copied_bytes(bytes) {
            None => Ok(UpFront::Whole),
            Some(why) => match shape() {
                Some(s) if !s.rotated && raf.cfa_len >= (s.width * s.height * s.bps / 8) as u64 => {
                    Ok(UpFront::JudgeRows)
                }
                _ => refuse(why),
            },
        },
        None if ends_in_zeros(&bytes[bytes.len().saturating_sub(TAIL)..]) => refuse(HALF_COPIED),
        None => Ok(UpFront::Whole),
    }
}

/// The decode's half of [`refuse_a_half_copy`]: refuse a frame left
/// to it whose [`last_row_unwritten`], when every black level is well
/// above zero. Under such a black (some thousand on a 14-bit body,
/// near 256 on a 12-bit one) no sensor reads 0, so zeros are data
/// never written; with a black of 0 a dark frame could read so, and
/// the frame is let through.
fn judge_rows(frame: &RawFrame, up_front: UpFront) -> Result<()> {
    let black_above_zero = frame.levels.black.values.iter().all(|&b| b > 0.0);
    if up_front == UpFront::JudgeRows && black_above_zero && last_row_unwritten(frame) {
        return Err(Error::Decode(HALF_COPIED.into()));
    }
    Ok(())
}

/// Whether the last row inside the frame's crop (the whole frame's
/// last row without one) reads nothing but zeros, across the crop's
/// columns. The samples are the frame's, before the black level is
/// taken off: a RAF's as the file stores them (only a DNG's are mapped
/// through a `LinearizationTable` first). Rows below the crop are not looked at, since a camera
/// may write those as zeros (see [`refuse_a_half_copy`]).
fn last_row_unwritten(frame: &RawFrame) -> bool {
    // `RawFrame::validate` holds the crop inside the frame; the
    // checked subtraction and `get` only keep a bad one from panicking.
    let crop = frame.effective_crop();
    let Some(row) = (crop.y + crop.height).checked_sub(1) else {
        return false;
    };
    let (start, cpp) = (row * frame.width, frame.channels);
    let span = (start + crop.x) * cpp..(start + crop.x + crop.width) * cpp;
    match &frame.samples {
        Samples::U16(v) => v.get(span).is_some_and(|r| r.iter().all(|&s| s == 0)),
        // rawler decodes a RAF to integers; a float frame is held to
        // the same rule should one ever come.
        Samples::F32(v) => v.get(span).is_some_and(|r| r.iter().all(|&s| s == 0.0)),
    }
}

/// [`half_copied`] for a file whose bytes are all at hand.
fn half_copied_bytes(bytes: &[u8]) -> Option<&'static str> {
    let tail = &bytes[bytes.len().saturating_sub(TAIL)..];
    half_copied(bytes, bytes.len() as u64, tail, |end| {
        let end = usize::try_from(end).ok()?;
        Some(bytes.get(end.checked_sub(TAIL)?..end)?.to_vec())
    })
}

/// [`half_copied`] for the file at `path`, reading its head, its last
/// [`TAIL`] bytes, and for a RAF the last [`TAIL`] bytes of its CFA
/// data; `None` when it cannot be read.
pub(crate) fn half_copy_at(path: &Path) -> Option<&'static str> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    // The bytes from `from` up to `to`, or to the end before it.
    let mut span = |from: u64, to: u64| {
        file.seek(SeekFrom::Start(from)).ok()?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(to.saturating_sub(from))
            .read_to_end(&mut bytes)
            .ok()?;
        Some(bytes)
    };
    let head = span(0, RAF_HEAD as u64)?;
    let tail = span(len.saturating_sub(TAIL as u64), len)?;
    half_copied(&head, len, &tail, |end| {
        span(end.checked_sub(TAIL as u64)?, end)
    })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RawlerDecoder;

impl Decoder for RawlerDecoder {
    fn decode_bytes(&self, bytes: &[u8]) -> Result<RawFrame> {
        guarded(
            || half_copied_bytes(bytes),
            || Ok(decode_source(&RawSource::new_from_slice(bytes))?.0),
        )
    }

    fn decode_path(&self, path: &Path) -> Result<RawFrame> {
        guarded(
            || half_copy_at(path),
            || Ok(decode_source(&mapped(path)?)?.0),
        )
    }
}

impl RawlerDecoder {
    /// Decode a file and also return the metadata rawler reads from it, for
    /// passing on to an output that carries EXIF (see [`crate::dng`]).
    pub fn decode_path_with_metadata(&self, path: &Path) -> Result<(RawFrame, RawMetadata)> {
        guarded(|| half_copy_at(path), || decode_source(&mapped(path)?))
    }

    /// [`Decoder::decode_path`] from the whole file read into memory, not
    /// mapped: for a pass over every file of a folder that may be on a
    /// share, where a volume that goes away under a mapping is a SIGBUS.
    /// The same bytes a mapping reads, since rawler populates its map.
    pub fn decode_path_unmapped(&self, path: &Path) -> Result<RawFrame> {
        guarded(
            || half_copy_at(path),
            || Ok(decode_source(&read_whole(path)?.0)?.0),
        )
    }
}

/// The camera's own rendering of a file, when the file carries one: the
/// largest preview JPEG, else the thumbnail, decoded to 8-bit sRGB, in
/// the orientation the camera stored it with, and the orientation to
/// show it in.
///
/// Never through a memory map: the thumbnails and the local previews
/// walk every file of a folder, a share's among them, and a mapped file
/// whose volume goes away faults as a SIGBUS no Rust code catches. A
/// CR3's preview is read as its probe is, from its ends, with the
/// preview's own bytes, which its `moov` box locates; a RAF's is the
/// head through its embedded JPEG, which its header locates; every
/// other format is read whole into memory. See [`preview_counted`].
pub fn preview_path(path: &Path) -> Result<Option<(image::RgbImage, Orientation)>> {
    preview_counted(path, PreviewSize::Largest).map(|(preview, _)| preview)
}

/// [`preview_path`] for a picture to be made `long_edge` on its long
/// edge or smaller: of a file with several previews, the smallest that
/// size or past it, else the largest ([`PreviewSize::AtLeast`]). A DNG
/// and a PEF have several to pick from; every other format's is
/// `preview_path`'s.
pub fn preview_path_at_least(
    path: &Path,
    long_edge: u32,
) -> Result<Option<(image::RgbImage, Orientation)>> {
    preview_counted(path, PreviewSize::AtLeast(long_edge)).map(|(preview, _)| preview)
}

/// What [`preview_path`] answers.
type Preview = Option<(image::RgbImage, Orientation)>;

/// [`preview_path`] and how many bytes of the file it read.
///
/// From the ends only the camera's preview itself is taken: a JPEG the
/// gap left short does not decode, and rawler then falls to the
/// thumbnail, which from the whole file it would not have. So any
/// error, panic, or answer short of the preview from the ends reads the
/// whole file, which gives `preview_path`'s answer as it always did.
fn preview_counted(path: &Path, want: PreviewSize) -> Result<(Preview, u64)> {
    let mut tried = 0;
    match read_preview_ends(path) {
        Ok(Some((buf, read))) => {
            let source = RawSource::new_from_shared_vec(std::sync::Arc::new(buf)).with_path(path);
            match guarded(|| None, || preview_source(&source, false, want)) {
                Ok(Some(found)) => return Ok((Some(found), read)),
                Ok(None) => log::debug!("{}: no preview from its ends; read whole", path.display()),
                Err(e) => log::debug!(
                    "{}: its preview from its ends: {e}; read whole",
                    path.display()
                ),
            }
            tried = read;
        }
        Ok(None) => {}
        Err(e) => log::debug!("{}: its preview's ends: {e}; read whole", path.display()),
    }
    guarded(
        || half_copy_at(path),
        || {
            let (source, whole) = read_whole(path)?;
            preview_source(&source, true, want).map(|preview| (preview, tried + whole))
        },
    )
}

/// The preview from one source: the camera's preview `want` picks,
/// else, when `thumbnail` says, the thumbnail.
fn preview_source(source: &RawSource, thumbnail: bool, want: PreviewSize) -> Result<Preview> {
    let decoder = ::rawler::get_decoder(source).map_err(decoder_error)?;
    let params = RawDecodeParams::default();
    let thumb = || {
        decoder
            .thumbnail_image(source, &params)
            .ok()
            .flatten()
            .map(|i| i.to_rgb8())
    };
    // A DNG's previews are ours to pick (`dng_preview`), among the
    // ones rawler's `preview_image` looks at, so a DNG none of whose
    // previews holds up has the thumbnail and not rawler's first.
    let image = if decoder.format_hint() == ::rawler::decoders::FormatHint::DNG {
        dng_preview(decoder.as_ref(), source, want).or_else(|| thumbnail.then(thumb).flatten())
    } else if decoder.format_hint() == ::rawler::decoders::FormatHint::PEF {
        // rawler's is the maker note's; the full one is ours to find.
        let theirs = decoder
            .preview_image(source, &params)
            .ok()
            .flatten()
            .map(|i| i.to_rgb8());
        pef_preview(source.buf(), theirs, want).or_else(|| thumbnail.then(thumb).flatten())
    } else {
        match decoder.preview_image(source, &params) {
            Ok(Some(i)) => Some(i.to_rgb8()),
            _ if thumbnail => thumb(),
            _ => None,
        }
    };
    let Some(image) = image else {
        return Ok(None);
    };
    let orientation = decoder
        .raw_metadata(source, &params)
        .ok()
        .and_then(|m| m.exif.orientation)
        .and_then(Orientation::from_exif)
        .unwrap_or_default();
    Ok(Some((image, orientation)))
}

/// Which of a file's previews a caller wants, where the file has
/// several (a DNG may: a Leica writes a thumbnail first, then the
/// full size, then two between).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewSize {
    /// The largest: a picture looked at whole, or measured.
    Largest,
    /// The smallest whose long edge is this or more, else the largest:
    /// a picture made smaller, which a larger preview only costs.
    AtLeast(u32),
}

/// A JPEG's bytes decoded, when they begin as a JPEG does and its
/// header says the size its tags do: the header is read before a pixel
/// is, so a JPEG of another size costs nothing to pass over. A body
/// damaged past a sound header is no error here: the decoder fills
/// what it cannot read with grey.
fn jpeg_of_size(
    bytes: &[u8],
    size: (u32, u32),
) -> std::result::Result<image::DynamicImage, String> {
    use image::ImageDecoder;
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return Err("not a JPEG".into());
    }
    let header = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("its JPEG: {e}"))?
        .dimensions();
    if header != size {
        return Err(format!("a JPEG of {header:?}"));
    }
    image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
        .map_err(|e| format!("its JPEG: {e}"))
}

/// A PEF's preview as `want` picks it among the JPEGs its own TIFF
/// directories hold (`pef::tiff_jpegs`: the full-size one in the
/// third) and `theirs`, rawler's (the maker note's, 720x480 on a K-1),
/// by size as a DNG's are ([`preview_order`]). Ours are only those that
/// fit the raw's sensor and lie as it does (`pef::tiff_jpegs`). One
/// whose header disagrees with its tags or will not read gives way to
/// the next, each tried once; one whose header is sound but whose body
/// is damaged decodes, grey where it is damaged, and is shown, as a
/// DNG's is. `None` when none is left, and [`preview_source`] takes the
/// thumbnail then.
fn pef_preview(
    bytes: &[u8],
    theirs: Option<image::RgbImage>,
    want: PreviewSize,
) -> Option<image::RgbImage> {
    let mut ours = super::pef::tiff_jpegs(bytes, MAX_PREVIEW_PIXELS);
    let mut sizes: Vec<(u32, u32)> = ours.iter().map(|j| (j.width, j.height)).collect();
    sizes.extend(theirs.as_ref().map(|t| t.dimensions()));
    for size in preview_order(sizes, want) {
        if theirs.as_ref().is_some_and(|t| t.dimensions() == size) {
            log::debug!(
                "PEF preview {}x{} (the maker note's) for {want:?}",
                size.0,
                size.1
            );
            return theirs;
        }
        let Some(at) = ours.iter().position(|j| (j.width, j.height) == size) else {
            continue;
        };
        let jpeg = ours.swap_remove(at);
        let bytes = &bytes[jpeg.at as usize..(jpeg.at + jpeg.len) as usize];
        match jpeg_of_size(bytes, size) {
            Ok(decoded) => {
                log::debug!("PEF preview {}x{} for {want:?}", size.0, size.1);
                return Some(decoded.into_rgb8());
            }
            Err(why) => log::debug!("PEF preview {}x{} passed over: {why}", size.0, size.1),
        }
    }
    None
}

/// The most pixels a DNG preview's tags are believed for: past a
/// 200 MP sensor's, so a tag past it is a damaged one.
const MAX_PREVIEW_PIXELS: u64 = 256 << 20;

/// The most DNG previews tried: a camera writes up to four.
const MAX_PREVIEWS: usize = 16;

/// A DNG preview a `SubIFDs` entry describes, by its tags.
struct DngPreview<'a> {
    ifd: &'a ::rawler::formats::tiff::IFD,
    width: u32,
    height: u32,
}

/// The order a DNG's previews are tried in for `want`: the largest
/// first, or the smallest at `AtLeast`'s edge or past it first, then
/// the rest from the largest down. A sort, so the file's order (the
/// thumbnail first, or any other) does not matter.
fn preview_order(mut sizes: Vec<(u32, u32)>, want: PreviewSize) -> Vec<(u32, u32)> {
    let area = |&(w, h): &(u32, u32)| u64::from(w) * u64::from(h);
    sizes.sort_by_key(|s| std::cmp::Reverse(area(s)));
    if let PreviewSize::AtLeast(edge) = want {
        let (mut big, small): (Vec<_>, Vec<_>) =
            sizes.into_iter().partition(|&(w, h)| w.max(h) >= edge);
        big.reverse();
        big.extend(small);
        sizes = big;
    }
    sizes
}

/// A DNG's preview as `want` picks it among the previews its root's
/// `SubIFDs` list, decoded to 8-bit RGB: rawler's `preview_image` takes
/// the first, which is a thumbnail in a file that lists that first.
/// Each candidate's tags and bytes are checked before it is decoded,
/// and one that fails ([`dng_preview_of`]) gives way to the next;
/// `None` when none decodes, and [`preview_source`] takes the root's
/// thumbnail then.
fn dng_preview(
    decoder: &dyn ::rawler::decoders::Decoder,
    source: &RawSource,
    want: PreviewSize,
) -> Option<image::RgbImage> {
    let root = decoder
        .ifd(::rawler::decoders::WellKnownIFD::Root)
        .ok()
        .flatten()?;
    let mut previews = dng_previews_of(&root);
    let order = preview_order(previews.iter().map(|p| (p.width, p.height)).collect(), want);
    for size in order {
        let at = previews.iter().position(|p| (p.width, p.height) == size)?;
        let preview = previews.swap_remove(at);
        match dng_preview_of(&preview, source) {
            Ok(rgb) => {
                log::debug!(
                    "DNG preview {}x{} for {want:?}",
                    preview.width,
                    preview.height
                );
                return Some(rgb);
            }
            Err(why) => log::debug!(
                "DNG preview {}x{} passed over: {why}",
                preview.width,
                preview.height
            ),
        }
    }
    None
}

/// The previews a DNG's root lists in its `SubIFDs`, by their tags:
/// one a strip, the first [`MAX_PREVIEWS`] of them, each of a size to
/// believe.
fn dng_previews_of(root: &::rawler::formats::tiff::IFD) -> Vec<DngPreview<'_>> {
    use ::rawler::tags::TiffCommonTag;
    let u32_of = |ifd: &::rawler::formats::tiff::IFD, tag| {
        ifd.get_entry(tag)
            .and_then(|e| e.value.get_u32(0).ok().flatten())
    };
    // One candidate a strip, and no more than a camera writes: a
    // hostile file's thousand entries at one strip would otherwise be
    // a thousand decodes of it.
    let mut seen = std::collections::HashSet::new();
    root.get_sub_ifd_all(TiffCommonTag::SubIFDs)
        .into_iter()
        .flatten()
        .filter(|ifd| u32_of(ifd, TiffCommonTag::NewSubFileType) == Some(1))
        .filter_map(|ifd| {
            let (width, height) = (
                u32_of(ifd, TiffCommonTag::ImageWidth)?,
                u32_of(ifd, TiffCommonTag::ImageLength)?,
            );
            let pixels = u64::from(width) * u64::from(height);
            (pixels > 0 && pixels <= MAX_PREVIEW_PIXELS).then_some(DngPreview {
                ifd,
                width,
                height,
            })
        })
        .filter(|p| seen.insert(u32_of(p.ifd, TiffCommonTag::StripOffsets)))
        .take(MAX_PREVIEWS)
        .collect()
}

/// One DNG preview decoded, or why not: 8 bits a sample, one or three
/// samples a pixel, its strips inside the file and not empty, and a
/// JPEG's bytes a JPEG of the size its tags say, decoded then; an
/// uncompressed one rawler decodes (as its `preview_image` would), its
/// panic caught as an error.
///
/// Passed over, where rawler would decode one listed first: a JPEG XL
/// preview, a tiled one, one of 9 to 16 bits, and one under the
/// alternate subfile type (0x10001). No camera or converter seen
/// writes one of those as its only preview: they come beside an 8-bit
/// JPEG, which is then taken, and a file with nothing else falls to
/// its thumbnail. A preview missed costs a smaller picture; one
/// misread would cost a wrong one.
fn dng_preview_of(
    preview: &DngPreview,
    source: &RawSource,
) -> std::result::Result<image::RgbImage, String> {
    use ::rawler::tags::TiffCommonTag;
    let ifd = preview.ifd;
    let entry = |tag| ifd.get_entry(tag).map(|e| &e.value);
    let values = |tag| -> Vec<u64> {
        entry(tag).map_or(Vec::new(), |v| {
            (0..v.count())
                .map_while(|i| v.get_u32(i).ok().flatten().map(u64::from))
                .collect()
        })
    };
    let spp = values(TiffCommonTag::SamplesPerPixel)
        .first()
        .copied()
        .unwrap_or(1);
    let bits = values(TiffCommonTag::BitsPerSample);
    if !matches!(spp, 1 | 3) || bits.is_empty() || bits.iter().any(|&b| b != 8) {
        return Err(format!("{spp} samples of {bits:?} bits"));
    }
    let compression = values(TiffCommonTag::Compression)
        .first()
        .copied()
        .unwrap_or(1);
    let (offsets, counts) = (
        values(TiffCommonTag::StripOffsets),
        values(TiffCommonTag::StripByteCounts),
    );
    if offsets.is_empty() || offsets.len() != counts.len() {
        return Err(format!(
            "{} strips with {} lengths",
            offsets.len(),
            counts.len()
        ));
    }
    let len = source.buf().len() as u64;
    for (&at, &n) in offsets.iter().zip(&counts) {
        if n == 0 || at.checked_add(n).is_none_or(|end| end > len) {
            return Err(format!("a strip of {n} bytes at {at} in a file of {len}"));
        }
    }
    let (w, h) = (preview.width, preview.height);
    match compression {
        // Uncompressed: the strips hold the pixels and no more.
        1 if counts.iter().sum::<u64>() < u64::from(w) * u64::from(h) * spp => {
            return Err("strips shorter than the pixels".into());
        }
        1 => {}
        // JPEG, the old form's code too: one strip, its header the size.
        6 | 7 if offsets.len() == 1 => {
            let bytes = &source.buf()[offsets[0] as usize..(offsets[0] + counts[0]) as usize];
            // Decoded with the `image` crate rawler's own JPEG strip
            // decoder calls (one version of it in the build), so the
            // pixels are those its `preview_image` gives, without its
            // copy into 16-bit samples and back: a 60 MP preview peaks
            // near a third of the memory. RGB for three samples, grey
            // for one, as rawler takes them.
            let decoded = jpeg_of_size(bytes, (w, h))?;
            return match (decoded, spp) {
                (image::DynamicImage::ImageRgb8(rgb), 3) => Ok(rgb),
                (grey @ image::DynamicImage::ImageLuma8(_), 1) => Ok(grey.to_rgb8()),
                (other, _) => Err(format!("a JPEG of {:?} for {spp} samples", other.color())),
            };
        }
        c => return Err(format!("compression {c} in {} strips", offsets.len())),
    }
    let samples = match guarded(
        || None,
        || ::rawler::decoders::plain_image_from_ifd(ifd, source).map_err(decoder_error),
    ) {
        Ok(RawImageData::Integer(v)) => v,
        Ok(RawImageData::Float(_)) => return Err("float samples".into()),
        Err(e) => return Err(e.to_string()),
    };
    let pixels = w as usize * h as usize;
    if samples.len() != pixels * spp as usize {
        return Err(format!("{} samples for {w}x{h}", samples.len()));
    }
    let rgb: Vec<u8> = if spp == 3 {
        samples.iter().map(|&s| s as u8).collect()
    } else {
        samples.iter().flat_map(|&s| [s as u8; 3]).collect()
    };
    image::RgbImage::from_raw(w, h, rgb).ok_or_else(|| "no picture".into())
}

/// The bytes of the file at `path` its preview is read from, in a
/// buffer of its length, the gap zeroed, and how many were read; `None`
/// for a format read whole. A CR3's are its ends as [`cr3_ends`] reads
/// them and the preview's sample, a RAF's its head through its JPEG.
fn read_preview_ends(path: &Path) -> std::io::Result<Option<(Vec<u8>, u64)>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    match ext.as_deref() {
        Some("cr3") => {
            let Some(head) = cr3_head(&mut file, len)? else {
                return Ok(None);
            };
            let Some((mut buf, read)) = read_ends_of(&mut file, len, head, CR3_TAIL)? else {
                return Ok(None);
            };
            let Some((at, size)) = cr3_preview_at(&buf) else {
                return Ok(None);
            };
            if at.checked_add(size).is_none_or(|end| end > len) {
                return Ok(None);
            }
            // The part of the sample not read already: the preview
            // sits past the head, and ahead of the tail but in a small
            // file.
            let (from, to) = (at.max(head), (at + size).min(len - CR3_TAIL));
            if from < to {
                use std::io::{Read, Seek, SeekFrom};
                file.seek(SeekFrom::Start(from))?;
                file.read_exact(&mut buf[from as usize..to as usize])?;
            }
            Ok(Some((buf, read + to.saturating_sub(from))))
        }
        Some("raf") => {
            let Some(end) = raf_jpeg_end(&mut file, len)? else {
                return Ok(None);
            };
            read_ends_of(&mut file, len, end, 0)
        }
        _ => Ok(None),
    }
}

/// Where a CR3's preview sample lies, as rawler's `preview_image` finds
/// it: the trak its `CCTP` names for the big preview, else the first,
/// and that trak's first sample, from the `moov` box in `buf`. `None`
/// when the boxes will not parse or say; a panic in rawler's parse is
/// the same `None`, caught as [`guarded`] catches one, so the panic hook
/// takes it for a damaged file and not a crash.
fn cr3_preview_at(buf: &[u8]) -> Option<(u64, u64)> {
    guarded(|| None, || Ok(cr3_preview_parse(buf)))
        .ok()
        .flatten()
}

/// [`cr3_preview_at`] unguarded.
fn cr3_preview_parse(buf: &[u8]) -> Option<(u64, u64)> {
    /// rawler's `Cr3ImageType::PreviewBig`.
    const PREVIEW_BIG: u64 = 2;
    let bmff = ::rawler::formats::bmff::Bmff::new_buf(buf).ok()?;
    let moov = &bmff.filebox.moov;
    let trak = moov
        .cr3desc
        .as_ref()
        .and_then(|d| {
            d.cctp
                .ccdts
                .iter()
                .find(|c| c.image_type == PREVIEW_BIG)
                .map(|c| (c.trak_index as usize).saturating_sub(1))
        })
        .unwrap_or(0);
    let stbl = &moov.traks.get(trak)?.mdia.minf.stbl;
    if stbl.stsz.sample_count == 0 {
        return None;
    }
    let (at, size) = stbl.get_sample_offset(1)?;
    Some((at as u64, size as u64))
}

/// Where a RAF's embedded JPEG ends, by the offset and the length its
/// header gives, as rawler reads them for the preview: what is read of
/// a RAF for it. Past [`raf_head`], so the probe's metadata is in it.
/// `None` for a file not laid out so.
fn raf_jpeg_end(
    file: &mut (impl std::io::Read + std::io::Seek),
    len: u64,
) -> std::io::Result<Option<u64>> {
    use std::io::SeekFrom;
    // The JPEG's offset and then its length, big-endian, at 84.
    const JPEG_AT: usize = 84;
    let Some(head) = raf_head(file, len)? else {
        return Ok(None);
    };
    let mut header = [0u8; JPEG_AT + 8];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header)?;
    let end = be_u32_at(&header, JPEG_AT) + be_u32_at(&header, JPEG_AT + 4);
    Ok(Some(end).filter(|&end| end >= head && end <= len))
}

/// Samples and metadata from one source; the frame takes what it keeps
/// of the metadata (the shot).
fn decode_source(source: &RawSource) -> Result<(RawFrame, RawMetadata)> {
    // Every file is judged by its bytes before rawler reads it, as it
    // always was; only a RAF whose CFA data ends in zeros has rawler
    // look at it first, through a dummy read of its own.
    let up_front = refuse_a_half_copy(source.buf(), || raf_shape(source))?;
    let decoder = ::rawler::get_decoder(source).map_err(decoder_error)?;
    let params = RawDecodeParams::default();
    let image = decoder
        .raw_image(source, &params, false)
        .map_err(decoder_error)?;
    let metadata = metadata_of(decoder.as_ref(), source, &params).map_err(decoder_error)?;
    let active_area = image.active_area.map(|r| Rect {
        x: r.p.x,
        y: r.p.y,
        width: r.d.w,
        height: r.d.h,
    });
    let mut dng = dng_tags(decoder.as_ref());
    if image.linearization_table.is_some()
        && image.cpp == 1
        && let RawImageData::Integer(stored) = &image.data
    {
        list_stored_constants(&mut dng.mosaic, stored, image.width);
    }
    let mut frame = frame_from_rawler(image)?;
    judge_rows(&frame, up_front)?;
    frame.dng = dng;
    if !frame.dng.mosaic.is_empty() || !frame.dng.warps.is_empty() {
        frame.dng.opcode_area = active_area;
    }
    let forward = forward_matrices(decoder.as_ref());
    with_forward_matrices(&mut frame.calibrations, &forward);
    if let Some(gains) = white_xy_gains(decoder.as_ref(), &frame.calibrations) {
        frame.as_shot_coefficients = Some(gains);
    }
    if let Some(stops) = frame.dng.baseline_exposure {
        log::info!("BaselineExposure {stops:.2} EV carried, not applied");
    }
    if !forward.is_empty() || frame.dng.opcode_area.is_some() {
        log::info!(
            "DNG tags: forward matrices for illuminant(s) {:?}, {} gain map(s), {} sample map(s), {} bad-pixel list(s), {} warp(s)",
            forward.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            crate::opcodes::gain_maps_of(&frame.dng.mosaic).count(),
            frame
                .dng
                .mosaic
                .iter()
                .filter(|op| matches!(op, crate::opcodes::MosaicOp::Remap(_)))
                .count(),
            frame
                .dng
                .mosaic
                .iter()
                .filter(|op| matches!(op, crate::opcodes::MosaicOp::BadPixels(..)))
                .count(),
            frame.dng.warps.len()
        );
    }
    // rawler leaves the image's orientation at Normal (a TODO in its
    // `RawImage::new`); the EXIF tag has the camera's word.
    if frame.orientation == Orientation::Normal
        && let Some(o) = metadata.exif.orientation.and_then(Orientation::from_exif)
    {
        frame.orientation = o;
    }
    frame.shot = shot_of(&metadata);
    frame.shot.camera_frame = camera_frame_of(source, frame.crop);
    let area = frame
        .crop
        .map_or((frame.width, frame.height), |c| (c.width, c.height));
    frame.shot.focus_point = focus_point_of(source, area);
    // After the shot: the table reads its ISO and exposure, and the
    // maker note's capture modes.
    if decoder.format_hint() == ::rawler::decoders::FormatHint::CR3 {
        let modes = super::style::canon_modes(std::io::Cursor::new(source.buf()));
        frame.levels.saturation = canon_saturation(&frame, modes.as_ref());
    }
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

/// The tags of a DNG's that rawler reads and leaves unparsed, from the
/// IFDs its DNG decoder hands out; every other format's decoder hands
/// out none, so another raw's are the default. A tag that is missing or
/// will not read is left at its default: the file still develops, as
/// it did before the tag was read.
fn dng_tags(decoder: &dyn ::rawler::decoders::Decoder) -> DngTags {
    use ::rawler::decoders::WellKnownIFD;
    use ::rawler::tags::DngTag;
    // The profile's tags (and `BaselineExposure`) live on IFD 0, which
    // in a phone's DNG is the preview's and not the raw's.
    let root = decoder.ifd(WellKnownIFD::VirtualDngRootTags).ok().flatten();
    let baseline_exposure = root
        .as_ref()
        .and_then(|ifd| ifd.get_entry(DngTag::BaselineExposure))
        .and_then(|e| e.value.get_f32(0).ok().flatten())
        // Past sixteen stops either way is a damaged tag, not a picture.
        .filter(|v| v.is_finite() && v.abs() <= 16.0);
    // The opcode lists live on the raw image's IFD.
    let raw = decoder.ifd(WellKnownIFD::VirtualDngRawTags).ok().flatten();
    let list = |tag: DngTag| -> Option<Vec<u8>> {
        let e = raw.as_ref()?.get_entry(tag)?;
        match &e.value {
            ::rawler::formats::tiff::Value::Undefined(b)
            | ::rawler::formats::tiff::Value::Byte(b) => Some(b.clone()),
            _ => None,
        }
    };
    let (one, two, three) = (
        list(DngTag::OpcodeList1),
        list(DngTag::OpcodeList2),
        list(DngTag::OpcodeList3),
    );
    let read = crate::opcodes::corrections(one.as_deref(), two.as_deref(), three.as_deref());
    // `NoiseProfile`: pairs of doubles, a pair a plane or one for all;
    // the first pair is taken for every channel. A pair that is not a
    // variance (negative, or not a number) is no profile.
    let noise_profile = raw
        .as_ref()
        .and_then(|ifd| ifd.get_entry(DngTag::NoiseProfile))
        .and_then(|e| {
            let at = |i: usize| match &e.value {
                ::rawler::formats::tiff::Value::Double(v) => v.get(i).copied(),
                v => v.get_f32(i).ok().flatten().map(f64::from),
            };
            let (s, o) = (at(0)?, at(1)?);
            (s.is_finite() && o.is_finite() && s >= 0.0 && o >= 0.0).then_some(
                crate::develop::noise::NoiseModel {
                    a: [s as f32; 3],
                    b: [o as f32; 3],
                },
            )
        });
    let merged = root
        .as_ref()
        .and_then(|ifd| ifd.get_entry(DngTag::DNGPrivateData))
        .and_then(|e| match &e.value {
            ::rawler::formats::tiff::Value::Byte(b)
            | ::rawler::formats::tiff::Value::Undefined(b) => crate::merge::Tag::parse(b),
            _ => None,
        });
    // Whose `BaselineExposure` it is: ours when we wrote the file. The
    // whole root is read only for a file that carries the tag.
    let ours = baseline_exposure.is_some()
        && decoder
            .ifd(WellKnownIFD::Root)
            .ok()
            .flatten()
            .and_then(|ifd| {
                ifd.get_entry(::rawler::tags::TiffCommonTag::Software)
                    .and_then(|e| e.value.as_string().cloned())
            })
            .is_some_and(|s| crate::raw::is_ours(&s));
    DngTags {
        baseline_exposure,
        ours,
        mosaic: read.mosaic,
        warps: read.warps,
        opcode_area: None,
        noise_profile,
        merged,
    }
}

/// A DNG's `ForwardMatrix1` and `ForwardMatrix2`, each with the
/// illuminant code of the `CalibrationIlluminant` beside it: rawler reads
/// the color matrices onto its image and leaves these as tags. The tags
/// are found as rawler finds the color matrices (IFD 0 first, then its
/// sub-IFDs) and an illuminant is read as it reads one (D65 when
/// missing or unknown), so a forward matrix lands on the calibration
/// its color matrix did. None for another format, for a matrix that
/// is not three by three, or for a file whose writer
/// [`reads_forward_matrices`] passes over.
fn forward_matrices(decoder: &dyn ::rawler::decoders::Decoder) -> Vec<(u16, Vec<f32>)> {
    use ::rawler::decoders::WellKnownIFD;
    use ::rawler::imgop::xyz::Illuminant;
    use ::rawler::tags::{DngTag, TiffCommonTag};
    let Some(root) = decoder.ifd(WellKnownIFD::Root).ok().flatten() else {
        return Vec::new();
    };
    let software = root
        .get_entry(TiffCommonTag::Software)
        .and_then(|e| e.value.as_string().cloned())
        .unwrap_or_default();
    let has_forward = [DngTag::ForwardMatrix1, DngTag::ForwardMatrix2]
        .into_iter()
        .any(|tag| root.get_entry_recursive(tag).is_some());
    if has_forward && !reads_forward_matrices(&software) {
        log::info!(
            "DNG forward matrices of {:?} left; its color matrices are used",
            software.trim()
        );
        return Vec::new();
    }
    [
        (DngTag::CalibrationIlluminant1, DngTag::ForwardMatrix1),
        (DngTag::CalibrationIlluminant2, DngTag::ForwardMatrix2),
    ]
    .into_iter()
    .filter_map(|(illuminant, matrix)| {
        let m = root.get_entry_recursive(matrix)?;
        if m.value.count() != 9 {
            return None;
        }
        let m: Vec<f32> = (0..9)
            .map(|i| m.value.get_f32(i).ok().flatten())
            .collect::<Option<_>>()?;
        if m.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let code = root
            .get_entry_recursive(illuminant)
            .and_then(|e| e.value.get_u16(0).ok().flatten())
            .and_then(|c| Illuminant::try_from(c).ok())
            .map_or(Illuminant::D65 as u16, |i| i as u16);
        Some((code, m))
    })
    .collect()
}

/// Whether we read the forward matrices of a DNG whose `Software` tag
/// is `software`. Not CHDK's: on a PowerShot's file they contradict its
/// own color matrices, putting a ColorChecker's colors at 0.4 (under
/// illuminant A) to 0.6 (D65) of the chroma the color matrices give, a
/// mean ΔE of 28 and 19, where a Pixel's agree to within 5; the picture
/// through them is faded, and through the color matrices it is the
/// scene's. No threshold on that distance tells the two apart, as an
/// Adobe profile's pair, meant for the tables after it, sits 14 to 15
/// apart; so we name the writer, and its file develops through the
/// color matrices, as every DNG did before we read forward matrices.
fn reads_forward_matrices(software: &str) -> bool {
    !software.trim_start().starts_with("CHDK")
}

/// The as-shot gains of a DNG that gives its white as `AsShotWhiteXY`
/// and no `AsShotNeutral`, through the engine's own profile of
/// `calibrations`, interpolated at that white as the specification asks.
/// rawler's gains for such a file come from one illuminant's matrix
/// alone, so the engine's two would read them back as another white,
/// some hundred kelvin off. `None` for every other file, or when the
/// profile will not make the white.
fn white_xy_gains(
    decoder: &dyn ::rawler::decoders::Decoder,
    calibrations: &[Calibration],
) -> Option<[f64; 3]> {
    use ::rawler::decoders::WellKnownIFD;
    use ::rawler::tags::DngTag;
    // The white is a root tag rawler's virtual DNG root does not copy.
    let root = decoder.ifd(WellKnownIFD::Root).ok().flatten()?;
    if root.get_entry(DngTag::AsShotNeutral).is_some() {
        return None;
    }
    let xy = root.get_entry(DngTag::AsShotWhiteXY)?;
    let x = xy.value.get_f32(0).ok().flatten()?;
    let y = xy.value.get_f32(1).ok().flatten()?;
    let profile = crate::color::profile_from_calibrations(calibrations).ok()?;
    crate::color::coefficients_of_white(&profile, f64::from(x), f64::from(y))
        .ok()
        .filter(|g| g.iter().all(|v| v.is_finite() && *v > 0.0))
}

/// Put each forward matrix on the calibration of its illuminant; one
/// whose illuminant no calibration has is dropped, as a forward matrix
/// means nothing without the color matrix it pairs with.
fn with_forward_matrices(calibrations: &mut [Calibration], forward: &[(u16, Vec<f32>)]) {
    for (illuminant, matrix) in forward {
        if let Some(c) = calibrations
            .iter_mut()
            .find(|c| c.illuminant == *illuminant)
        {
            c.forward_matrix = Some(matrix.clone());
        }
    }
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

/// The camera's frame of a raw (`style::frame_stream`), kept when its
/// area is the crop the develop cuts to, which the fractions it gives
/// are of; a file the reader cannot read has none.
fn camera_frame_of(source: &RawSource, crop: Option<Rect>) -> Option<crate::raw::CameraFrame> {
    match super::style::frame_stream(std::io::Cursor::new(source.buf())) {
        Ok(Some(c)) if Some(c.area) == crop => Some(c),
        Ok(Some(c)) => {
            log::debug!("the camera's frame {c:?} is of another area than the crop {crop:?}");
            None
        }
        Ok(None) => None,
        Err(e) => {
            log::debug!("the camera's frame: {e}");
            None
        }
    }
}

/// The camera's focus point of a raw (`focus::focus_stream`) on the
/// area its develop is cut to; a file the reader cannot read has none.
fn focus_point_of(source: &RawSource, area: (usize, usize)) -> Option<crate::raw::FocusPoint> {
    match super::focus::focus_stream(std::io::Cursor::new(source.buf()), area) {
        Ok(Some(p)) => {
            log::debug!("the camera's focus point {:?}", p.area);
            Some(p)
        }
        Ok(None) => None,
        Err(e) => {
            log::debug!("the camera's focus point: {e}");
            None
        }
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
        camera_frame: None,
        focus_point: None,
    }
}

/// The decoder's metadata for `source`, with the lens a Pentax DNG
/// names only in its maker note ([`with_maker_note_lens`]).
fn metadata_of(
    decoder: &dyn ::rawler::decoders::Decoder,
    source: &RawSource,
    params: &RawDecodeParams,
) -> ::rawler::Result<RawMetadata> {
    let mut metadata = decoder.raw_metadata(source, params)?;
    with_maker_note_lens(decoder, &mut metadata);
    Ok(metadata)
}

/// The lens of a Pentax DNG put into its metadata as rawler puts a
/// PEF's: the camera writes no `LensModel`, only the maker note's
/// `LensRec` in `DNGPrivateData`, which rawler's PEF decoder reads and
/// its DNG decoder does not. We read the tag and look it up in rawler's
/// own lens table, as its PEF decoder does, until rawler's DNG decoder
/// does the same (a change offered upstream; this goes when it lands).
/// A DNG whose EXIF names a lens, or any other file, is left as it is.
fn with_maker_note_lens(decoder: &dyn ::rawler::decoders::Decoder, metadata: &mut RawMetadata) {
    use ::rawler::decoders::{FormatHint, WellKnownIFD};
    let named = |s: &Option<String>| s.as_deref().is_some_and(|s| !s.trim().is_empty());
    if decoder.format_hint() != FormatHint::DNG
        || metadata.lens.is_some()
        || named(&metadata.exif.lens_model)
    {
        return;
    }
    let Some(root) = decoder.ifd(WellKnownIFD::Root).ok().flatten() else {
        return;
    };
    let Some(lens) = pentax_dng_lens(&root, &metadata.make, &metadata.model) else {
        return;
    };
    log::debug!("lens {} from the DNG's Pentax maker note", lens.lens_name);
    let exif = &mut metadata.exif;
    exif.lens_spec = Some([
        lens.focal_range[0],
        lens.focal_range[1],
        lens.aperture_range[0],
        lens.aperture_range[1],
    ]);
    exif.lens_make = Some(lens.lens_make.clone());
    exif.lens_model = Some(lens.lens_model.clone());
    metadata.lens = Some(lens.clone());
}

/// The lens a Pentax DNG's maker note names ([`pentax_lens_rec`]) in
/// rawler's table of K-mount lenses. A first byte of 0 to 2 is a lens
/// with no id of its own (none, a K or M lens, an A lens), as rawler's
/// PEF decoder reads it.
fn pentax_dng_lens(
    root: &::rawler::formats::tiff::IFD,
    make: &str,
    model: &str,
) -> Option<&'static ::rawler::lens::LensDescription> {
    use ::rawler::formats::tiff::Value;
    use ::rawler::tags::DngTag;
    let data = match &root.get_entry(DngTag::DNGPrivateData)?.value {
        Value::Byte(d) | Value::Undefined(d) => d,
        _ => return None,
    };
    let (kind, id) = pentax_lens_rec(data)?;
    if kind <= 2 {
        return None;
    }
    ::rawler::lens::LensResolver::new()
        .with_camera_make(make)
        .with_camera_model(model)
        .with_lens_id((u32::from(kind), u32::from(id)))
        .with_mounts(&["k-mount".into()])
        .resolve()
}

/// The most bytes a `LensRec` is taken at: Pentax writes four.
const LENS_REC_MAX: usize = 64;

/// The first two bytes of the `LensRec` (0x003f) of the Pentax maker
/// note a Pentax DNG keeps in `DNGPrivateData`: "PENTAX \0", the byte
/// order, then an IFD whose offsets count from the data's start. Read
/// by hand, not through rawler's IFD reader, which allocates what an
/// entry's count says before it finds the data short: the entries are
/// walked only as far as the data holds them, and the tag is taken only
/// as BYTE or UNDEFINED of 2 to [`LENS_REC_MAX`] bytes, inline or
/// wholly inside the data.
fn pentax_lens_rec(data: &[u8]) -> Option<(u8, u8)> {
    const HEADER: &[u8] = b"PENTAX \0";
    const LENS_REC: u16 = 0x003f;
    // The header, the byte order, and the entry count.
    const FIRST: usize = 12;
    const ENTRY: usize = 12;
    if !data.starts_with(HEADER) {
        return None;
    }
    let little = match data.get(8..10)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = data.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = data.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let count = usize::from(u16_at(10)?);
    let fit = data.len().saturating_sub(FIRST) / ENTRY;
    for i in 0..count.min(fit) {
        let at = FIRST + i * ENTRY;
        if u16_at(at)? != LENS_REC {
            continue;
        }
        let kind = u16_at(at + 2)?;
        let n = usize::try_from(u32_at(at + 4)?).ok()?;
        // BYTE or UNDEFINED.
        if !matches!(kind, 1 | 7) || !(2..=LENS_REC_MAX).contains(&n) {
            return None;
        }
        let bytes = if n <= 4 {
            data.get(at + 8..at + 8 + n)?
        } else {
            let from = usize::try_from(u32_at(at + 8)?).ok()?;
            data.get(from..from.checked_add(n)?)?
        };
        return Some((bytes[0], bytes[1]));
    }
    None
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
        _ if is_monochrome(&image) => SensorLayout::Monochrome,
        RawPhotometricInterpretation::Cfa(config) => SensorLayout::Cfa(cfa_pattern(&config.cfa)?),
        _ => SensorLayout::Linear,
    };

    let black = {
        let bl = &image.blacklevel;
        let values = bl.levels.iter().map(|l| l.as_f32()).collect();
        LevelPattern::new(bl.width, bl.height, bl.cpp, values)?
    };
    let black_deltas = black_deltas(&image);
    let white = white_pattern(&image.whitelevel.0, image.cpp)?;

    let as_shot_coefficients = as_shot_of(&image);
    let calibrations = calibrations_of(&image);

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
        linearization_table,
        ..
    } = image;
    let samples = match data {
        RawImageData::Integer(mut v) => {
            // rawler hands a DNG's table over unapplied (its develop
            // applies it, which we do not call) and clears it once
            // applied, so a table it reports is one still owed.
            if let Some(table) = linearization_table.as_deref() {
                linearize(&mut v, table);
                log::info!("LinearizationTable of {} entries applied", table.len());
            }
            Samples::U16(v)
        }
        // rawler drops a table on float samples, which the
        // specification does not allow one on.
        RawImageData::Float(v) => Samples::F32(v),
    };

    let frame = RawFrame {
        other_names: other_names(&clean_make, &clean_model),
        make: clean_make,
        model: clean_model,
        width,
        height,
        channels: cpp,
        layout,
        samples,
        levels: Levels {
            black,
            white,
            black_deltas,
            saturation: None,
        },
        as_shot_coefficients,
        calibrations,
        crop,
        orientation,
        shot: Default::default(),
        dng: DngTags::default(),
    };
    frame.validate()?;
    Ok(frame)
}

/// A DNG's `LinearizationTable` on its stored samples, in place: each
/// sample is replaced by the table's entry at it, and one past the
/// table's end by its last entry (DNG 1.7, chapter 5). The black and
/// white levels are in the table's units, so they apply after it, as
/// does every reader of the frame's samples. An empty table leaves the
/// samples as they are.
pub(crate) fn linearize(samples: &mut [u16], table: &[u16]) {
    use rayon::prelude::*;
    let Some(&last) = table.last() else {
        return;
    };
    samples.par_chunks_mut(1 << 16).for_each(|run| {
        run.iter_mut()
            .for_each(|s| *s = *table.get(usize::from(*s)).unwrap_or(&last))
    });
}

/// A DNG's `OpcodeList1` constant bad-pixel tests made lists of the
/// photosites they find, read on the stored samples (`stored`, `width`
/// wide), for a frame whose samples a `LinearizationTable` is about to
/// change: the first list runs on the stored values, before the table
/// (DNG 1.7, chapter 7), and the develop reads the frame's samples,
/// which are the table's by then.
fn list_stored_constants(mosaic: &mut [crate::opcodes::MosaicOp], stored: &[u16], width: usize) {
    use crate::opcodes::{BadPixels, MosaicOp, OpcodeList};
    for op in mosaic {
        if let MosaicOp::BadPixels(OpcodeList::One, BadPixels::Constant { value, phase }) = op {
            let points = stored
                .iter()
                .enumerate()
                .filter(|&(_, &s)| u32::from(s) == *value)
                .map(|(i, _)| [(i / width) as i32, (i % width) as i32])
                .collect();
            *op = MosaicOp::BadPixels(
                OpcodeList::One,
                BadPixels::List {
                    phase: *phase,
                    points,
                    rects: Vec::new(),
                },
            );
        }
    }
}

/// A capture of a Canon body whose CR3 white (Canon's "specular"
/// level, which rawler reads from the color data) sits under where
/// every channel saturates: the body, one shutter, and the ISOs at
/// which a blown frame of it was measured.
pub(crate) struct CanonCell {
    /// rawler's clean model name, or one the body is also sold under
    /// ([`RawFrame::other_names`]).
    pub model: &'static str,
    /// The ShutterMode code (0 mechanical, 1 electronic first curtain,
    /// 2 electronic).
    pub shutter: i64,
    /// The ISO speeds measured, each on its own: no speed between two
    /// of them is taken on trust.
    pub isos: &'static [u32],
    /// The full frame's width and height, as AspectInfo gives them: a
    /// crop mode or another aspect reads otherwise.
    pub frame: (i64, i64),
}

const R6_II: (i64, i64) = (6000, 4000);
const R5_II: (i64, i64) = (8192, 5464);
const EOS_850D: (i64, i64) = (6000, 4000);

/// The captures with headroom over their white. An ISO is in a cell
/// only when a frame of that body, shutter, and ISO, under a second,
/// is blown in every channel: each of the four positions of the
/// pattern has 20 or more samples in the top 32 levels (16351 to
/// 16383), so each channel's clip was seen at the top. Counted over
/// the 838 CR3s measured (design.md has the evidence). A saturation set
/// too high leaves saturated photosites unflagged, which paints them
/// magenta, so nothing is in on less: the R5, R6, and R7 (no pile),
/// the R6 II's electronic shutter and the R5 II's mechanical (no
/// frame), the R5 II's electronic ISO 125 (no blue at the top), and
/// the R6 II's first curtain 640, 800, 4000, and 5000. At 640 and 800
/// green piles and red does not (at the top in 2 of 8 frames at 640,
/// by at most 11 samples at 800); at none does blue reach it. The four
/// positions share one converter, so a channel short of the top is
/// the scenes', and the rule errs toward leaving a frame out. The 850D
/// (Rebel T8i, Kiss X10i), in live view, from three frames: its white
/// 14338 at ISO 200, every channel at exactly 16383. Its ISO 100 was
/// blown in every channel too and is out: the white sits at 0.70 of
/// the range, 0.45 stops no bracket has shown linear.
pub(crate) const CANON_HEADROOM: [CanonCell; 5] = [
    CanonCell {
        model: "EOS R6 Mark II",
        shutter: 0,
        isos: &[100, 320],
        frame: R6_II,
    },
    CanonCell {
        model: "EOS R6 Mark II",
        shutter: 1,
        isos: &[
            100, 125, 160, 200, 250, 320, 400, 500, 1250, 1600, 2500, 12800,
        ],
        frame: R6_II,
    },
    CanonCell {
        model: "EOS R5 Mark II",
        shutter: 1,
        isos: &[100],
        frame: R5_II,
    },
    CanonCell {
        model: "EOS R5 Mark II",
        shutter: 2,
        isos: &[100],
        frame: R5_II,
    },
    CanonCell {
        model: "EOS 850D",
        shutter: 1,
        isos: &[200],
        frame: EOS_850D,
    },
];

/// The 14-bit top every channel of [`CANON_HEADROOM`]'s bodies clips at.
const CANON_TOP: f32 = 16383.0;

/// How far from the black to the top the response is trusted. Brackets
/// of the R6 II at ISO 100 read linear within about 1% to 0.955 of the
/// way and 1.5 to 2.5% low past 0.96; the saturation pile is the last
/// 32 levels. At 0.955 nothing past the measured linear range is read
/// as light, and opposed flags a photosite at 0.987 of it, 0.943 of
/// the way. 0.98 read greens in the low band as light, which leans
/// magenta; the margin study (design.md) has what each share costs.
pub(crate) const SATURATION_SHARE: f32 = 0.955;

/// A CR3's saturation level, for a capture in a cell of
/// [`CANON_HEADROOM`]: its body, shutter, and an ISO measured, its full frame, no
/// highlight tone priority, no dual pixel raw, no HDR, no long exposure
/// noise reduction, and under a second (where Canon would run it); a
/// white of 14 bits under the top and a black under the white. `None`
/// for anything else, and for anything the maker note does not say.
/// Taken from the highest black, so no channel's is over-stated.
pub(crate) fn canon_saturation(
    frame: &RawFrame,
    modes: Option<&super::style::CanonModes>,
) -> Option<f32> {
    if frame.make != "Canon" {
        return None;
    }
    let modes = modes?;
    let iso = frame.shot.iso?;
    let exposure = frame.shot.exposure_time?;
    let shutter = modes.shutter?;
    let measured = CANON_HEADROOM.iter().any(|cell| {
        (cell.model == frame.model || frame.other_names.iter().any(|n| n == cell.model))
            && cell.shutter == shutter
            && cell.isos.contains(&iso)
            && modes
                .aspect
                .is_some_and(|(a, w, h)| a == 0 && (w, h) == cell.frame)
    }) && exposure < 1.0
        && modes.highlight_tone_priority == Some(0)
        && modes.long_exposure_nr == Some(0)
        && modes.dual_pixel_raw == Some(0)
        && modes.hdr == Some(0);
    if !measured {
        return None;
    }
    if !matches!(frame.samples, Samples::U16(_)) || frame.levels.black_deltas.is_some() {
        return None;
    }
    let levels = &frame.levels;
    let white = levels
        .white
        .values
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min);
    let top_white = levels.white.values.iter().copied().fold(0.0f32, f32::max);
    let black = levels.black.values.iter().copied().fold(0.0f32, f32::max);
    if !(white > 8191.0 && top_white < CANON_TOP && black < white) {
        return None;
    }
    Some(black + SATURATION_SHARE * (CANON_TOP - black))
}

/// Whether the image is a monochrome sensor's: DNG's form of one is
/// LinearRaw with one sample a pixel, rawler's own BlackIsZero.
fn is_monochrome(image: &RawImage) -> bool {
    match image.photometric {
        RawPhotometricInterpretation::LinearRaw => image.cpp == 1,
        RawPhotometricInterpretation::BlackIsZero => true,
        RawPhotometricInterpretation::Cfa(_) => false,
    }
}

/// The as-shot gains: the file's, or the neutral for a monochrome
/// sensor, whose one channel has no white of its own to record.
fn as_shot_of(image: &RawImage) -> Option<[f64; 3]> {
    if is_monochrome(image) {
        Some([1.0; 3])
    } else {
        as_shot(image.wb_coeffs)
    }
}

/// A DNG's column and row differences of the black level, counted from
/// its active area (the whole frame without one); `None` when it has
/// neither.
fn black_deltas(image: &RawImage) -> Option<BlackDeltas> {
    let bl = &image.blacklevel;
    if bl.delta_h.is_empty() && bl.delta_v.is_empty() {
        return None;
    }
    let area = image.active_area.map_or(
        Rect {
            x: 0,
            y: 0,
            width: image.width,
            height: image.height,
        },
        |r| Rect {
            x: r.p.x,
            y: r.p.y,
            width: r.d.w,
            height: r.d.h,
        },
    );
    let floats = |v: &[::rawler::formats::tiff::SRational]| v.iter().map(|d| d.as_f32()).collect();
    Some(BlackDeltas {
        area,
        columns: floats(&bl.delta_h),
        rows: floats(&bl.delta_v),
    })
}

/// The color matrices of the image's primary camera profile (a DNG's
/// own, or the decoder's table's for every other raw) as the engine's
/// calibrations, one per illuminant, the first slot's kept where two
/// share one. The first two slots only (no table entry has more): a
/// third illuminant is a triangle of interpolation the engine's pair
/// does not make. A slot whose illuminant tag is missing is read as
/// D65, as rawler read it before its camera profiles; the
/// specification's unknown would leave such a file with no calibration
/// the engine can use.
///
/// Each matrix is the DNG specification's XYZ to camera, `AnalogBalance
/// x CameraCalibration x ColorMatrix`, the calibration taken when its
/// signature matches the profile's: the matrix the file's as-shot
/// neutral is measured against. The forward matrices are left as the
/// file has them, which is exact while the calibration is diagonal, as
/// it is on every file seen.
fn calibrations_of(image: &RawImage) -> Vec<Calibration> {
    use ::rawler::imgop::xyz::Illuminant;
    let Some(profile) = image.primary_camera_profile() else {
        return Vec::new();
    };
    let mut calibrations: Vec<Calibration> = Vec::new();
    for slot in profile.slots().filter(|&slot| slot < 2) {
        let Some(calibration) = profile.calibration_slot(slot, Some(&image.camera_calibration))
        else {
            continue;
        };
        let illuminant = u16::from(profile.calibration_illuminant[slot].unwrap_or(Illuminant::D65));
        if calibrations.iter().any(|c| c.illuminant == illuminant) {
            continue;
        }
        calibrations.push(Calibration {
            illuminant,
            color_matrix: xyz_to_camera(
                calibration.color_matrix,
                calibration.camera_calibration.map(Vec::as_slice),
                image.analog_balance.as_deref(),
            ),
            // rawler leaves ForwardMatrix as a tag; a DNG's are put on
            // by `with_forward_matrices`.
            forward_matrix: None,
        });
    }
    calibrations.sort_by_key(|c| c.illuminant);
    calibrations
}

/// `analog_balance x camera_calibration x color_matrix` for a color
/// matrix of one row per camera plane and three columns; a calibration
/// or a balance not of the matrix's planes is left out, with a warning.
fn xyz_to_camera(
    color_matrix: &[f32],
    camera_calibration: Option<&[f32]>,
    analog_balance: Option<&[f32]>,
) -> Vec<f32> {
    let planes = color_matrix.len() / 3;
    let mut m = color_matrix.to_vec();
    if let Some(cc) = camera_calibration {
        if cc.len() == planes * planes && color_matrix.len() == planes * 3 {
            m = (0..planes)
                .flat_map(|i| (0..3).map(move |j| (i, j)))
                .map(|(i, j)| (0..planes).map(|k| cc[i * planes + k] * m[k * 3 + j]).sum())
                .collect();
        } else {
            log::warn!(
                "CameraCalibration of {} values for {planes} planes left out",
                cc.len()
            );
        }
    }
    if let Some(ab) = analog_balance {
        if ab.len() == planes && color_matrix.len() == planes * 3 {
            for (row, gain) in m.as_chunks_mut::<3>().0.iter_mut().zip(ab) {
                row.iter_mut().for_each(|v| *v *= gain);
            }
        } else {
            log::warn!(
                "AnalogBalance of {} values for {planes} planes left out",
                ab.len()
            );
        }
    }
    m
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
    fn a_body_s_other_names_come_from_the_decoder_s_table() {
        // Cleaned, as a decoded frame names it, and as the EXIF does.
        for (make, model) in [("Canon", "EOS 850D"), ("Canon", "Canon EOS 850D")] {
            assert_eq!(
                other_names(make, model),
                [
                    "Canon EOS Kiss X10i",
                    "Canon EOS Rebel T8i",
                    "EOS Kiss X10i",
                    "EOS Rebel T8i"
                ],
                "{make} {model}"
            );
        }
        // An alias's other names are the body's own and the rest.
        assert_eq!(
            other_names("Canon", "EOS Rebel T8i"),
            [
                "Canon EOS 850D",
                "Canon EOS Kiss X10i",
                "EOS 850D",
                "EOS Kiss X10i"
            ]
        );
        // A body whose EXIF make is not its clean make, either way.
        for (make, model) in [("Nikon", "D800"), ("NIKON CORPORATION", "NIKON D800")] {
            assert_eq!(
                other_names(make, model),
                ["D800E", "NIKON D800E"],
                "{make} {model}"
            );
        }
        assert!(other_names("Canon", "EOS R5").is_empty());
        assert!(other_names("Nikon", "EOS 850D").is_empty());
        assert!(other_names("", "").is_empty());
    }

    #[test]
    fn an_exif_model_cleans_to_the_index_s_camera_and_back() {
        assert_eq!(
            cleaned_camera("Canon EOS R6m2").as_deref(),
            Some("Canon EOS R6 Mark II")
        );
        assert_eq!(
            cleaned_camera("canon eos r6m2").as_deref(),
            Some("Canon EOS R6 Mark II"),
            "case ignored"
        );
        assert_eq!(cleaned_camera("No Such Camera 9000"), None);
        assert!(
            exif_models("Canon EOS R6 Mark II")
                .iter()
                .any(|m| m == "Canon EOS R6m2")
        );
        assert!(exif_models("No Such Camera 9000").is_empty());
    }

    /// A description that will not read, or a decoder that panics on
    /// it, is an error's words and nothing worse; the index stores it
    /// as an unknown mosaic.
    #[test]
    fn a_mosaic_read_that_fails_is_an_error_not_a_panic() {
        assert_eq!(
            described(|| Err(Error::Decode("no".into()))),
            Err("decode failed: no".into())
        );
        assert_eq!(
            described(|| panic!("rawler")),
            Err("the decoder panicked".into())
        );
        assert_eq!(described(|| Ok(None)), Ok(None));
        assert_eq!(
            described(|| Ok(Some(Mosaic::XTrans))),
            Ok(Some(Mosaic::XTrans))
        );
    }

    /// A DNG's CFA tags out of shape are a mosaic of no kind the engine
    /// takes, decided before a value past the repeat is read.
    #[test]
    fn a_dngs_cfa_tags_are_taken_only_in_shape() {
        let rggb = [0, 1, 1, 2];
        assert_eq!(mosaic_from_tags(&[2, 2], &rggb, None), Mosaic::Bayer);
        let xtrans: Vec<u32> = "GGRGGBGGBGGRBRGRBGGGBGGRGGRGGBRBGBRG"
            .chars()
            .map(|c| match c {
                'R' => 0,
                'G' => 1,
                _ => 2,
            })
            .collect();
        assert_eq!(mosaic_from_tags(&[6, 6], &xtrans, None), Mosaic::XTrans);
        // The planes map the pattern's values to colors: here blue's
        // and red's are swapped, which is still Bayer; a value past the
        // planes is out of shape.
        assert_eq!(
            mosaic_from_tags(&[2, 2], &rggb, Some(&[2, 1, 0])),
            Mosaic::Bayer
        );
        assert_eq!(
            mosaic_from_tags(&[2, 2], &[0, 1, 1, 3], Some(&[0, 1, 2])),
            Mosaic::Other
        );
        // Out of shape: oversized, empty, short of the repeat, one
        // dimension only.
        assert_eq!(
            mosaic_from_tags(&[65535, 65535], &rggb, None),
            Mosaic::Other
        );
        assert_eq!(mosaic_from_tags(&[17, 1], &rggb, None), Mosaic::Other);
        assert_eq!(mosaic_from_tags(&[0, 0], &rggb, None), Mosaic::Other);
        assert_eq!(mosaic_from_tags(&[0, 2], &rggb, None), Mosaic::Other);
        assert_eq!(mosaic_from_tags(&[2, 2], &[0, 1, 1], None), Mosaic::Other);
        assert_eq!(mosaic_from_tags(&[2], &rggb, None), Mosaic::Other);
        assert_eq!(mosaic_from_tags(&[], &[], None), Mosaic::Other);
    }

    /// A panic inside the guard is the file's decode error, and says
    /// a copy in progress when the tail is zeros; the thread knows it
    /// is inside only while it is.
    #[test]
    fn a_panic_in_the_decoder_is_an_error_that_names_a_half_copy() {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let zeros = || half_copied(&[], TAIL as u64, &[0; TAIL], |_| None);
        let data = || half_copied(&[], TAIL as u64, &[7; TAIL], |_| None);
        let seen = std::cell::Cell::new(false);
        let copying = guarded::<()>(zeros, || {
            seen.set(inside_decoder());
            panic!("capacity overflow")
        });
        let other = guarded::<()>(data, || panic!("capacity overflow"));
        std::panic::set_hook(hook);
        assert!(seen.get(), "inside while it runs");
        assert!(!inside_decoder(), "and not after");
        let copying = copying.unwrap_err().to_string();
        assert!(copying.contains("half-copied"), "{copying}");
        assert!(copying.contains("capacity overflow"), "{copying}");
        let other = other.unwrap_err().to_string();
        assert!(!other.contains("half-copied"), "{other}");
        assert!(other.contains("gave up"), "{other}");
        // No panic, nothing changed.
        assert_eq!(guarded(zeros, || Ok(3)).unwrap(), 3);
    }

    /// A RAF head: the magic, then the six words, the offset and the
    /// length of the preview, the metadata, and the CFA data, then
    /// filler to 0x400, where the CFA data of [`a_raf`] starts.
    fn a_raf_head(words: [u32; 6]) -> Vec<u8> {
        let mut head = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        head.resize(0x54, 0);
        for v in words {
            head.extend_from_slice(&u32::to_be_bytes(v));
        }
        head.resize(0x400, 0x55);
        head
    }

    /// A RAF of `body` after the head, its header naming `cfa` bytes
    /// of CFA data from 0x400.
    fn a_raf(cfa: usize, body: &[u8]) -> Vec<u8> {
        let words = [0x100, 0x200, 0x300, 0x100, 0x400, cfa as u32];
        [a_raf_head(words), body.to_vec()].concat()
    }

    /// A folder of a test's own, removed when the test ends, passed or
    /// not.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "greycard-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            crate::testing::remove_dir_retry(&self.0);
        }
    }

    /// A RAF is judged by its header and its sensor data, not its
    /// tail: one that holds all its sections, its CFA data not ending
    /// in zeros, is whole with a tail of zeros past them; one whose
    /// sections run past its end is refused up front; one whose CFA
    /// data ends in zeros is refused up front when it is compressed,
    /// rotated, or of a shape the dummy read cannot give, and when it
    /// is none of those is left to its decoded rows; a panic on it
    /// names a half copy either way. One too short for a header falls
    /// back to the tail.
    #[test]
    fn a_raf_is_judged_by_its_header() {
        let data = vec![0x33; 3 * TAIL];
        let padded = [data.clone(), vec![0; TAIL]].concat();
        let whole = a_raf(data.len(), &padded);
        assert_eq!(half_copied_bytes(&whole), None);
        // The dummy read is asked for only when the data ends in zeros.
        let unasked = || -> Option<RafShape> { panic!("the shape is not needed") };
        assert_eq!(refuse_a_half_copy(&whole, unasked).unwrap(), UpFront::Whole);

        let past = a_raf(data.len() + 1, &data);
        assert_eq!(half_copied_bytes(&past), Some(RAF_HALF_COPIED));
        match refuse_a_half_copy(&past, unasked) {
            Err(Error::Decode(text)) => assert_eq!(text, RAF_HALF_COPIED),
            other => panic!("{other:?}"),
        }

        // Full size, as a copier that sets the length first leaves it,
        // the CFA data's last 64 KiB zeros, as a whole uncompressed
        // X-Trans 5 HR file's are too. Stored uncompressed (the data
        // as long as width * height * bps / 8) and not rotated, it is
        // left to its rows; compressed, rotated, or with no shape to
        // be had, it is refused. A panic on it names a half copy; one
        // byte of data in them, and it is whole.
        let preallocated = a_raf(padded.len(), &padded);
        let shape = |width, rotated| {
            move || {
                Some(RafShape {
                    width,
                    height: 512,
                    bps: 16,
                    rotated,
                })
            }
        };
        assert_eq!(
            refuse_a_half_copy(&preallocated, shape(256, false)).unwrap(),
            UpFront::JudgeRows
        );
        for refused in [
            refuse_a_half_copy(&preallocated, shape(257, false)),
            refuse_a_half_copy(&preallocated, shape(256, true)),
            refuse_a_half_copy(&preallocated, || None),
        ] {
            match refused {
                Err(Error::Decode(text)) => assert_eq!(text, HALF_COPIED),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(half_copied_bytes(&preallocated), Some(HALF_COPIED));
        let mut one = preallocated.clone();
        one[0x400 + padded.len() - TAIL] = 1;
        assert_eq!(half_copied_bytes(&one), None);
        // CFA data shorter than the span: the header alone.
        assert_eq!(half_copied_bytes(&a_raf(TAIL - 1, &[0; TAIL])), None);

        // An offset at u32::MAX lies past the end, without overflow;
        // a section of no length ends where it starts.
        let far = [0x100, 0x200, u32::MAX, 0x100, 0x400, 0x100];
        let far = [a_raf_head(far), vec![0x33; 0x100]].concat();
        assert_eq!(half_copied_bytes(&far), Some(RAF_HALF_COPIED));
        let empty = [0x100, 0, 0x300, 0, 0x400, 0x100];
        let empty = [a_raf_head(empty), vec![0x33; 0x100]].concat();
        assert_eq!(half_copied_bytes(&empty), None);

        // Not a RAF: the tail rule, up front too.
        let other = [b"II*\0".to_vec(), padded.clone()].concat();
        assert_eq!(half_copied_bytes(&other), Some(HALF_COPIED));
        match refuse_a_half_copy(&other, unasked) {
            Err(Error::Decode(text)) => assert_eq!(text, HALF_COPIED),
            other => panic!("{other:?}"),
        }
        let mut wrong = whole.clone();
        wrong[0] = b'X';
        assert_eq!(half_copied_bytes(&wrong), Some(HALF_COPIED));
        // A head too short for a RAF's header: the tail rule too.
        let short = &a_raf(0, &[])[..RAF_HEAD - 1];
        let len = TAIL as u64;
        let none = |_| None;
        assert_eq!(half_copied(short, len, &[0; TAIL], none), Some(HALF_COPIED));
        assert_eq!(half_copied(short, len, &[1; TAIL], none), None);
        assert_eq!(half_copied_bytes(short), None, "too short to say");

        // From a path, and in a panic's words.
        let dir = Scratch::new("raf-head");
        let read = |bytes: &[u8], name: &str| {
            let path = dir.0.join(name);
            std::fs::write(&path, bytes).unwrap();
            half_copy_at(&path)
        };
        assert_eq!(read(&whole, "whole.RAF"), None);
        assert_eq!(read(&past, "past.RAF"), Some(RAF_HALF_COPIED));
        assert_eq!(read(&preallocated, "copying.RAF"), Some(HALF_COPIED));
        assert_eq!(read(&one, "one.RAF"), None);
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let gave_up = guarded::<()>(|| half_copied_bytes(&past), || panic!("off the end"));
        std::panic::set_hook(hook);
        let gave_up = gave_up.unwrap_err().to_string();
        assert!(gave_up.contains(RAF_HALF_COPIED), "{gave_up}");
    }

    /// A copy that stopped short leaves the last row inside the crop
    /// zeros; dead rows below the crop may be zeros in a whole file,
    /// and are not looked at, nor are columns beside the crop. Without
    /// a crop, the frame's last row. Only a frame the up-front check
    /// left to its rows, with a black above zero, is refused.
    #[test]
    fn a_raf_is_judged_by_its_last_cropped_row() {
        let frame = |crop, zero_rows: std::ops::Range<usize>, f32: bool| {
            let mut f = crate::develop::tests::flat_frame();
            (f.width, f.height, f.crop) = (6, 6, crop);
            let v: Vec<u16> = (0..36)
                .map(|i| {
                    if zero_rows.contains(&(i / 6)) {
                        0
                    } else {
                        1020 + i as u16
                    }
                })
                .collect();
            f.samples = if f32 {
                Samples::F32(v.iter().map(|&s| f32::from(s)).collect())
            } else {
                Samples::U16(v)
            };
            f
        };
        let cropped = Rect {
            x: 0,
            y: 0,
            width: 6,
            height: 5,
        };
        let crop = Some(cropped);
        for f32 in [false, true] {
            assert!(!last_row_unwritten(&frame(crop, 0..0, f32)));
            assert!(!last_row_unwritten(&frame(crop, 5..6, f32)), "dead row");
            assert!(last_row_unwritten(&frame(crop, 4..6, f32)));
            assert!(last_row_unwritten(&frame(crop, 4..5, f32)));
            assert!(last_row_unwritten(&frame(None, 5..6, f32)));
            assert!(!last_row_unwritten(&frame(None, 4..5, f32)));
        }
        // One sample of data in the row is enough; a column beside the
        // crop is not looked at.
        let inset = Some(Rect {
            x: 2,
            width: 3,
            ..cropped
        });
        let with = |crop, at: usize| {
            let mut f = frame(crop, 4..6, false);
            if let Samples::U16(v) = &mut f.samples {
                v[4 * 6 + at] = 1;
            }
            f
        };
        assert!(!last_row_unwritten(&with(crop, 3)));
        assert!(!last_row_unwritten(&with(inset, 2)));
        assert!(!last_row_unwritten(&with(inset, 4)));
        assert!(last_row_unwritten(&with(inset, 1)), "left of the crop");
        assert!(last_row_unwritten(&with(inset, 5)), "right of the crop");
        // A crop that `validate` would refuse says nothing, and does
        // not panic.
        let past = Some(Rect {
            height: 7,
            ..cropped
        });
        assert!(!last_row_unwritten(&frame(past, 0..6, false)));
        let empty = Some(Rect {
            height: 0,
            ..cropped
        });
        assert!(!last_row_unwritten(&frame(empty, 0..6, false)));

        // What `decode_source` asks: only a frame left to its rows is
        // judged, and only under a black above zero.
        let unwritten = frame(crop, 4..6, false);
        match judge_rows(&unwritten, UpFront::JudgeRows) {
            Err(Error::Decode(text)) => assert_eq!(text, HALF_COPIED),
            other => panic!("{other:?}"),
        }
        assert!(judge_rows(&unwritten, UpFront::Whole).is_ok());
        assert!(judge_rows(&frame(crop, 5..6, false), UpFront::JudgeRows).is_ok());
        let mut no_black = unwritten.clone();
        no_black.levels.black = LevelPattern::uniform(0.0, 1);
        assert!(judge_rows(&no_black, UpFront::JudgeRows).is_ok());
    }

    /// The wiring, on a real RAF named by `GREYCARD_RAF_SAMPLE` (none
    /// may sit in the repo): it decodes whole, and with the second half
    /// of its CFA data zeroed, as a copier that sets the length first
    /// leaves it, it is refused as a half copy and does not panic. An
    /// uncompressed X-H2 or X-T5 file is the case this pins: refused
    /// by its rows after the decode, where a compressed one is refused
    /// by its bytes before it. It swaps the process's panic hook, as
    /// the header test does, so run it on its own, by name.
    #[test]
    #[ignore = "needs a RAF in GREYCARD_RAF_SAMPLE"]
    fn a_half_copied_raf_sample_is_refused() {
        let path = std::env::var_os("GREYCARD_RAF_SAMPLE").expect("GREYCARD_RAF_SAMPLE");
        let whole = std::fs::read(path).unwrap();
        assert!(whole.starts_with(RAF_MAGIC), "not a RAF");
        RawlerDecoder.decode_bytes(&whole).unwrap();
        let (at, len) = (be_u32_at(&whole, 0x64), be_u32_at(&whole, 0x68));
        let (at, len) = (at as usize, len as usize);
        let mut copying = whole.clone();
        copying[at + len / 2..at + len].fill(0);
        let hook = std::panic::take_hook();
        let panicked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = panicked.clone();
        std::panic::set_hook(Box::new(move |_| {
            seen.store(true, std::sync::atomic::Ordering::SeqCst)
        }));
        let refused = RawlerDecoder.decode_bytes(&copying);
        std::panic::set_hook(hook);
        match refused {
            Err(Error::Decode(text)) => assert_eq!(text, HALF_COPIED),
            other => panic!("{other:?}"),
        }
        assert!(
            !panicked.load(std::sync::atomic::Ordering::SeqCst),
            "no panic on the way"
        );
    }

    #[test]
    fn only_a_whole_tail_of_zeros_is_a_copy_in_progress() {
        assert!(ends_in_zeros(&vec![0; TAIL]));
        assert!(
            !ends_in_zeros(&vec![0; TAIL - 1]),
            "a file too short to say"
        );
        let mut one = vec![0; TAIL];
        one[TAIL / 2] = 1;
        assert!(!ends_in_zeros(&one));
    }

    /// The real thing: rawler's RW2 decoder divides by zero on the
    /// scrubbed fixture the library's tests keep (see its note there),
    /// and every way in comes back an error instead of a panic.
    #[test]
    fn rawler_panicking_on_a_damaged_file_comes_back_an_error() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../greycard-library/tests/fixtures/rawler-rw2-divide-by-zero.RW2");
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let probe = probe_path(&fixture);
        let stance = stance_path(&fixture);
        let decode = RawlerDecoder.decode_path(&fixture);
        let bytes = RawlerDecoder.decode_bytes(&std::fs::read(&fixture).unwrap());
        std::panic::set_hook(hook);
        for e in [
            probe.map(|_| ()),
            stance.map(|_| ()),
            decode.map(|_| ()),
            bytes.map(|_| ()),
        ] {
            let e = e.expect_err("the fixture does not decode").to_string();
            assert!(e.contains("gave up"), "{e}");
        }
    }

    /// A 12x10 RGGB mosaic of rawler's, to be written as a DNG by
    /// [`written_dng`].
    fn a_mosaic(black: ::rawler::rawimage::BlackLevel) -> RawImage {
        use ::rawler::cfa::PlaneColor;
        use ::rawler::decoders::Camera;
        use ::rawler::pixarray::PixU16;
        use ::rawler::rawimage::{CFAConfig, WhiteLevel};

        let (w, h) = (12usize, 10usize);
        let data: Vec<u16> = (0..w * h).map(|i| 600 + (i as u16 * 37) % 900).collect();
        let cfa = CFA::new("RGGB");
        let mut camera = Camera::new();
        camera.make = "Test".into();
        camera.model = "Mosaic".into();
        camera.clean_make = camera.make.clone();
        camera.clean_model = camera.model.clone();
        camera.cfa = cfa.clone();
        RawImage::new(
            camera,
            PixU16::new_with(data, w, h),
            1,
            [1.0, 1.0, 1.0, f32::NAN],
            RawPhotometricInterpretation::Cfa(CFAConfig::new(&cfa, &PlaneColor::default())),
            Some(black),
            Some(WhiteLevel::new(vec![4000])),
            false,
        )
    }

    /// `raw` written as an uncompressed DNG through rawler's writer,
    /// `root` given the root directory last.
    fn dng_bytes(
        raw: &RawImage,
        root: impl FnOnce(&mut ::rawler::formats::tiff::writer::DirectoryWriter),
    ) -> Vec<u8> {
        use ::rawler::dng::writer::DngWriter;
        use ::rawler::dng::{
            CompressionParams, CropMode, DNG_VERSION_V1_4, DngCompression, DngPhotometricConversion,
        };
        let mut file = std::io::Cursor::new(Vec::new());
        let mut dng = DngWriter::new(&mut file, DNG_VERSION_V1_4).unwrap();
        let mut main = dng.subframe_on_root(0);
        main.raw_image(
            raw,
            CropMode::ActiveArea,
            DngCompression::Uncompressed,
            DngPhotometricConversion::Original,
            &CompressionParams::default(),
        )
        .unwrap();
        main.finalize().unwrap();
        dng.load_base_tags(raw).unwrap();
        root(dng.root_ifd_mut());
        dng.close().unwrap();
        file.into_inner()
    }

    /// [`dng_bytes`] decoded back by rawler.
    fn written_dng(
        raw: &RawImage,
        root: impl FnOnce(&mut ::rawler::formats::tiff::writer::DirectoryWriter),
    ) -> RawImage {
        let bytes = dng_bytes(raw, root);
        let source = RawSource::new_from_slice(&bytes);
        let decoder = ::rawler::get_decoder(&source).unwrap();
        decoder
            .raw_image(&source, &RawDecodeParams::default(), false)
            .unwrap()
    }

    /// A Pentax maker note as a Pentax DNG keeps it in
    /// `DNGPrivateData`, in byte order `ii` (or big-endian): the header,
    /// the order, and one IFD of a `LensRec` of `rec` and a long kept
    /// out of line, so the note's offsets count from the data's start.
    fn pentax_note(rec: [u8; 4], ii: bool) -> Vec<u8> {
        let u16b = |v: u16| if ii { v.to_le_bytes() } else { v.to_be_bytes() };
        let u32b = |v: u32| if ii { v.to_le_bytes() } else { v.to_be_bytes() };
        let mut note = b"PENTAX \0".to_vec();
        note.extend(if ii { b"II" } else { b"MM" });
        note.extend(u16b(2));
        // 0x003f, BYTE, four of them, in the entry.
        note.extend(u16b(0x003f));
        note.extend(u16b(1));
        note.extend(u32b(4));
        note.extend(rec);
        // 0x0200, LONG, two of them, out of line at the end.
        note.extend(u16b(0x0200));
        note.extend(u16b(4));
        note.extend(u32b(2));
        note.extend(u32b(10 + 2 + 2 * 12 + 4));
        note.extend(u32b(0));
        note.extend(u32b(64));
        note.extend(u32b(64));
        note
    }

    /// A mosaic DNG with `note` as its `DNGPrivateData` and `lens` as
    /// its `LensModel` when given, through rawler's metadata and ours.
    fn pentax_dng_metadata(note: Option<Vec<u8>>, lens: Option<&str>) -> RawMetadata {
        use ::rawler::formats::tiff::Value;
        use ::rawler::tags::{DngTag, ExifTag};
        let raw = a_mosaic(::rawler::rawimage::BlackLevel::new(&[100u16], 1, 1, 1));
        let bytes = dng_bytes(&raw, |root| {
            if let Some(note) = note {
                root.add_tag(DngTag::DNGPrivateData, Value::Byte(note));
            }
            if let Some(lens) = lens {
                root.add_tag(ExifTag::LensModel, lens);
            }
        });
        let source = RawSource::new_from_slice(&bytes);
        let decoder = ::rawler::get_decoder(&source).unwrap();
        metadata_of(decoder.as_ref(), &source, &RawDecodeParams::default()).unwrap()
    }

    /// A Pentax DNG's lens is the one its maker note's `LensRec` names
    /// in rawler's table, as a PEF's is: the EXIF names none.
    #[test]
    fn a_pentax_dngs_lens_is_read_from_its_maker_note() {
        for ii in [true, false] {
            let m = pentax_dng_metadata(Some(pentax_note([7, 58, 0, 0], ii)), None);
            assert_eq!(
                m.exif.lens_model.as_deref(),
                Some("smc PENTAX-D FA Macro 100mm F2.8 WR"),
                "little-endian {ii}"
            );
            assert_eq!(m.exif.lens_make.as_deref(), Some("Pentax"));
            let shot = shot_of(&m);
            assert_eq!(
                shot.lens_model.as_deref(),
                Some("smc PENTAX-D FA Macro 100mm F2.8 WR")
            );
            assert_eq!(shot.lens_make.as_deref(), Some("Pentax"));
        }
    }

    /// The EXIF's lens stands over the note's; a lens with no id of its
    /// own (a first byte of 0 to 2), an id the table lacks, a note of
    /// another maker's, or one cut short, names none and fails nothing.
    #[test]
    fn a_dngs_maker_note_lens_yields_to_the_exif_and_to_nonsense() {
        let named = |m: &RawMetadata| m.exif.lens_model.clone();
        let m = pentax_dng_metadata(Some(pentax_note([7, 58, 0, 0], true)), Some("My Lens 50mm"));
        assert_eq!(named(&m).as_deref(), Some("My Lens 50mm"));
        assert!(m.lens.is_none());
        for rec in [[0, 0, 0, 0], [1, 7, 0, 0], [2, 58, 0, 0], [7, 255, 255, 0]] {
            let m = pentax_dng_metadata(Some(pentax_note(rec, true)), None);
            assert_eq!(named(&m), None, "{rec:?}");
        }
        let mut other = pentax_note([7, 58, 0, 0], true);
        other[..6].copy_from_slice(b"RICOH\0");
        assert_eq!(named(&pentax_dng_metadata(Some(other), None)), None);
        let mut cut = pentax_note([7, 58, 0, 0], true);
        cut.truncate(14);
        assert_eq!(named(&pentax_dng_metadata(Some(cut), None)), None);
        assert_eq!(named(&pentax_dng_metadata(None, None)), None);
    }

    /// A maker note's entries as a hostile file may give them, read
    /// without a large allocation and without a panic: a count of four
    /// billion bytes, a value past the data's end, more entries counted
    /// than the data holds, a type that is not bytes, too short a tag.
    /// A well-formed note with its tag out of line or after another
    /// entry reads.
    #[test]
    fn a_maker_notes_lens_rec_is_read_within_its_data() {
        // One entry of `tag`, `kind`, `count`, `value`, big-endian, then
        // `tail`, after an entry count of `entries`.
        let note = |entries: u16, tag: u16, kind: u16, count: u32, value: [u8; 4], tail: &[u8]| {
            let mut n = b"PENTAX \0MM".to_vec();
            n.extend(entries.to_be_bytes());
            n.extend(tag.to_be_bytes());
            n.extend(kind.to_be_bytes());
            n.extend(count.to_be_bytes());
            n.extend(value);
            n.extend(tail);
            n
        };
        let rec = [7, 58, 0, 0];
        assert_eq!(
            pentax_lens_rec(&note(1, 0x3f, 1, 4, rec, &[])),
            Some((7, 58))
        );
        assert_eq!(
            pentax_lens_rec(&note(1, 0x3f, 7, 4, rec, &[])),
            Some((7, 58))
        );
        // Out of line, at 28, past the entry and four bytes more.
        let at = 28u32.to_be_bytes();
        assert_eq!(
            pentax_lens_rec(&note(1, 0x3f, 1, 6, at, &[0, 0, 0, 0, 7, 58, 1, 2, 3, 4])),
            Some((7, 58))
        );
        // The hostile ones.
        for (what, n) in [
            ("a LONG of 4G", note(1, 0x3f, 4, u32::MAX, rec, &[])),
            ("BYTEs of 4G", note(1, 0x3f, 1, u32::MAX, rec, &[])),
            (
                "past the end",
                note(1, 0x3f, 1, 8, 24u32.to_be_bytes(), &[0; 4]),
            ),
            (
                "at u32::MAX",
                note(1, 0x3f, 1, 8, u32::MAX.to_be_bytes(), &[]),
            ),
            ("one byte", note(1, 0x3f, 1, 1, rec, &[])),
            ("a SHORT", note(1, 0x3f, 3, 2, rec, &[])),
            ("another tag", note(1, 0x40, 1, 4, rec, &[])),
            ("counted, not there", note(u16::MAX, 0x40, 1, 4, rec, &[])),
        ] {
            assert_eq!(pentax_lens_rec(&n), None, "{what}");
        }
        // A count past the data, the tag in the one entry there is.
        assert_eq!(
            pentax_lens_rec(&note(u16::MAX, 0x3f, 1, 4, rec, &[])),
            Some((7, 58))
        );
        // Cut anywhere, nothing panics.
        let whole = pentax_note([7, 58, 0, 0], false);
        for end in 0..whole.len() {
            let _ = pentax_lens_rec(&whole[..end]);
        }
        assert_eq!(pentax_lens_rec(&whole), Some((7, 58)));
    }

    /// A linear DNG's image, three samples a pixel, 8 by 6.
    fn a_linear() -> RawImage {
        use ::rawler::decoders::Camera;
        use ::rawler::pixarray::PixU16;
        use ::rawler::rawimage::{BlackLevel, WhiteLevel};

        let (w, h) = (8usize, 6usize);
        let data: Vec<u16> = (0..w * h * 3)
            .map(|i| 700 + (i as u16 * 53) % 1300)
            .collect();
        let mut camera = Camera::new();
        camera.make = "Test".into();
        camera.model = "Linear".into();
        camera.clean_make = camera.make.clone();
        camera.clean_model = camera.model.clone();
        RawImage::new(
            camera,
            PixU16::new_with(data, w * 3, h),
            3,
            [1.0, 1.0, 1.0, f32::NAN],
            RawPhotometricInterpretation::LinearRaw,
            Some(BlackLevel::new(&[100u16, 100, 100], 1, 1, 3)),
            Some(WhiteLevel::new(vec![4000; 3])),
            false,
        )
    }

    /// `raw` with `table` written as its `LinearizationTable`, decoded
    /// whole as a file is.
    fn decoded_with_table(mut raw: RawImage, table: Option<Vec<u16>>) -> RawFrame {
        raw.linearization_table = table;
        RawlerDecoder
            .decode_bytes(&dng_bytes(&raw, |_| {}))
            .unwrap()
    }

    fn u16_samples(frame: &RawFrame) -> &[u16] {
        match &frame.samples {
            Samples::U16(v) => v,
            Samples::F32(_) => panic!("float samples"),
        }
    }

    /// A DNG's `LinearizationTable` maps every stored sample, a mosaic's
    /// and a linear one's: an identity table changes nothing, a curve
    /// gives its entries, and a sample past a short table's end takes
    /// its last entry. The levels stay as the file has them, in the
    /// table's units.
    #[test]
    fn a_dngs_linearization_table_maps_its_samples() {
        // The fixtures' samples 0 and 7 are 600 and 859 (the mosaic),
        // 700 and 1071 (the linear), so three halves of them are these.
        for (raw, by_hand) in [
            (
                a_mosaic(::rawler::rawimage::BlackLevel::new(&[100u16], 1, 1, 1)),
                [900, 1288],
            ),
            (a_linear(), [1050, 1606]),
        ] {
            let stored = u16_samples(&decoded_with_table(raw.clone(), None)).to_vec();
            assert!(stored.iter().all(|&s| (600..2000).contains(&s)));

            let identity: Vec<u16> = (0..=4095).collect();
            let same = decoded_with_table(raw.clone(), Some(identity));
            assert_eq!(u16_samples(&same), &stored[..]);

            let curve: Vec<u16> = (0..4096u32).map(|i| (i * 3 / 2) as u16).collect();
            let mapped = decoded_with_table(raw.clone(), Some(curve));
            let got = u16_samples(&mapped);
            assert_eq!([got[0], got[7]], by_hand);
            assert!(got.iter().zip(&stored).all(|(&g, &s)| g == s * 3 / 2));
            assert_eq!(mapped.levels.black.values[0], 100.0);
            assert_eq!(mapped.levels.white.values[0], 4000.0);

            // A table of 1000 entries: every sample at or past 1000
            // takes entry 999.
            let short: Vec<u16> = (0..1000u16).map(|i| i * 2).collect();
            let clamped = decoded_with_table(raw.clone(), Some(short));
            for (&g, &s) in u16_samples(&clamped).iter().zip(&stored) {
                assert_eq!(g, if s < 1000 { s * 2 } else { 1998 }, "stored {s}");
            }
            assert!(stored.iter().any(|&s| s >= 1000) && stored.iter().any(|&s| s < 1000));
        }
    }

    /// The levels apply after the table: a stored value the table maps
    /// to the white normalizes to 1, and one it maps to the black to 0,
    /// whatever the stored values themselves.
    #[test]
    fn a_dngs_levels_apply_after_its_table() {
        let raw = a_linear();
        let stored = u16_samples(&decoded_with_table(raw.clone(), None)).to_vec();
        let (low, high) = (stored[0], stored[1]);
        assert!(low < high);
        let table: Vec<u16> = (0..4096u16)
            .map(|i| match i {
                i if i == low => 100,
                i if i == high => 4000,
                _ => 2050,
            })
            .collect();
        let frame = decoded_with_table(raw, Some(table));
        let norm = crate::develop::normalize_levels(&frame);
        for (&n, &s) in norm.iter().zip(&stored) {
            let want = match s {
                s if s == low => 0.0,
                s if s == high => 1.0,
                _ => 0.5,
            };
            assert!((n - want).abs() < 1e-6, "stored {s}: {n}");
        }
    }

    /// `linearize` on its own: an empty table leaves the samples, and a
    /// table of one entry gives every sample that entry.
    #[test]
    fn linearize_holds_past_the_tables_end() {
        let mut v = vec![0u16, 1, 65535];
        linearize(&mut v, &[]);
        assert_eq!(v, [0, 1, 65535]);
        linearize(&mut v, &[7]);
        assert_eq!(v, [7, 7, 7]);
        let mut v = vec![0u16, 2, 3, 65535];
        linearize(&mut v, &[10, 20, 30]);
        assert_eq!(v, [10, 30, 30, 30]);
    }

    /// The first opcode list's constant bad-pixel tests, which run on
    /// the stored values, are made lists of the photosites found while
    /// those values are at hand; every other opcode stays as it was.
    #[test]
    fn a_first_list_constant_is_listed_on_the_stored_values() {
        use crate::opcodes::{BadPixels, MosaicOp, OpcodeList};
        let stored = [5u16, 0, 9, 0, 0, 3];
        let constant = |list| MosaicOp::BadPixels(list, BadPixels::Constant { value: 0, phase: 1 });
        let mut ops = vec![constant(OpcodeList::One), constant(OpcodeList::Two)];
        list_stored_constants(&mut ops, &stored, 3);
        assert_eq!(
            ops,
            vec![
                MosaicOp::BadPixels(
                    OpcodeList::One,
                    BadPixels::List {
                        phase: 1,
                        points: vec![[0, 1], [1, 0], [1, 1]],
                        rects: Vec::new(),
                    }
                ),
                constant(OpcodeList::Two),
            ]
        );
    }

    /// A DNG with a first-list constant bad-pixel test and a table:
    /// the decode lists the photosites whose stored value is the
    /// constant (none of the table's values is), the develop repairs
    /// them, and with an identity table the picture is the one the
    /// constant gives with no table.
    #[test]
    fn a_first_list_constant_with_a_table_is_listed_and_repaired() {
        use crate::opcodes::{BadPixels, MosaicOp, OpcodeList};
        use ::rawler::dng::opcodes::{FixBadPixelsConstant, Opcode, OpcodeList as RList};
        let mut raw = a_mosaic(::rawler::rawimage::BlackLevel::new(&[100u16], 1, 1, 1));
        let RawImageData::Integer(v) = &mut raw.data else {
            panic!("integer samples")
        };
        // Under every other sample, and three halves of it is none.
        let constant = 555u16;
        for i in [13usize, 50, 51, 119] {
            v[i] = constant;
        }
        let stored = v.clone();
        raw.dng_opcodes_1 = Some(RList::new(vec![Opcode::FixBadPixelsConstant(
            FixBadPixelsConstant {
                version: 0x01030000,
                flags: 0,
                constant: u32::from(constant),
                bayer_phase: 0,
            },
        )]));
        let identity: Vec<u16> = (0..=4095).collect();
        let curve: Vec<u16> = (0..4096u32).map(|i| (i * 3 / 2) as u16).collect();
        let none = decoded_with_table(raw.clone(), None);
        let same = decoded_with_table(raw.clone(), Some(identity));
        let mapped = decoded_with_table(raw.clone(), Some(curve));
        let MosaicOp::BadPixels(OpcodeList::One, BadPixels::List { points, .. }) =
            &mapped.dng.mosaic[0]
        else {
            panic!("not listed: {:?}", mapped.dng.mosaic)
        };
        let w = mapped.width;
        assert_eq!(
            points,
            &[13usize, 50, 51, 119].map(|i| [(i / w) as i32, (i % w) as i32])
        );
        assert!(stored.iter().filter(|&&s| s == constant).count() == 4);
        let s = crate::develop::DevelopSettings::default();
        // The fixture has no matrix: the camera's own space will do.
        let develop = |f: &RawFrame| {
            let mut f = f.clone();
            f.calibrations = vec![Calibration {
                illuminant: 21,
                color_matrix: vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
                forward_matrix: None,
            }];
            crate::develop::develop(&f, &s).unwrap().image.data
        };
        assert_eq!(
            develop(&none),
            develop(&same),
            "listed as the constant finds them"
        );
        let mut unrepaired = mapped.clone();
        unrepaired.dng.mosaic.clear();
        assert_ne!(
            develop(&mapped),
            develop(&unrepaired),
            "the listed sites repaired"
        );
    }

    /// How a test preview is written wrong, or not.
    #[derive(Clone, Copy, PartialEq)]
    enum Spoil {
        None,
        /// Its strip past the file's end.
        Outside,
        /// A strip of no bytes.
        Empty,
        /// A strip of four billion bytes.
        Absurd,
        /// Bytes that are not a JPEG.
        NotJpeg,
        /// Its tags say twice the JPEG's width.
        Lying,
    }

    /// A mosaic DNG with a thumbnail on its root, as Adobe writes one
    /// (rawler's writer makes it 180 by 120), the raw in its first `SubIFDs` entry, and a JPEG
    /// preview in one after it of each size given, in that order,
    /// spoiled as given.
    fn dng_with_previews(previews: &[(u32, u32, Spoil)]) -> Vec<u8> {
        use ::rawler::dng::writer::DngWriter;
        use ::rawler::dng::{
            CompressionParams, CropMode, DNG_VERSION_V1_4, DngCompression, DngPhotometricConversion,
        };
        use ::rawler::formats::tiff::Value;
        use ::rawler::tags::TiffCommonTag;
        let raw = a_mosaic(::rawler::rawimage::BlackLevel::new(&[100u16], 1, 1, 1));
        let mut file = std::io::Cursor::new(Vec::new());
        let mut dng = DngWriter::new(&mut file, DNG_VERSION_V1_4).unwrap();
        let picture = |w: u32, h: u32| {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
                image::Rgb([(x * 7) as u8, (y * 5) as u8, 128])
            }))
        };
        dng.thumbnail(&picture(48, 32)).unwrap();
        let mut main = dng.subframe(0);
        main.raw_image(
            &raw,
            CropMode::ActiveArea,
            DngCompression::Uncompressed,
            DngPhotometricConversion::Original,
            &CompressionParams::default(),
        )
        .unwrap();
        main.finalize().unwrap();
        for &(w, h, spoil) in previews {
            let mut sub = dng.subframe(1);
            sub.preview(&picture(w, h), 0.9).unwrap();
            let ifd = sub.ifd_mut();
            match spoil {
                Spoil::None => {}
                Spoil::Outside => {
                    ifd.add_tag(TiffCommonTag::StripOffsets, Value::Long(vec![1 << 30]))
                }
                Spoil::Empty => ifd.add_tag(TiffCommonTag::StripByteCounts, Value::Long(vec![0])),
                Spoil::Absurd => {
                    ifd.add_tag(TiffCommonTag::StripByteCounts, Value::Long(vec![u32::MAX]))
                }
                // The raw's own samples, which are not a JPEG.
                Spoil::NotJpeg => ifd.add_tag(TiffCommonTag::StripOffsets, Value::Long(vec![8])),
                Spoil::Lying => ifd.add_tag(TiffCommonTag::ImageWidth, Value::Long(vec![w * 2])),
            }
            sub.finalize().unwrap();
        }
        dng.load_base_tags(&raw).unwrap();
        dng.close().unwrap();
        file.into_inner()
    }

    /// The size of the preview `want` gets of `bytes`.
    fn preview_size(bytes: &[u8], want: PreviewSize) -> Option<(u32, u32)> {
        let source = RawSource::new_from_slice(bytes);
        preview_source(&source, true, want)
            .unwrap()
            .map(|(rgb, _)| rgb.dimensions())
    }

    /// Of a DNG's several previews, the largest is taken whatever their
    /// order in the file, and for a picture made smaller the smallest at
    /// its size or past it, else the largest; rawler alone takes the
    /// first, a thumbnail when the file lists that first.
    #[test]
    fn a_dngs_preview_is_picked_by_size_not_order() {
        use PreviewSize::{AtLeast, Largest};
        let ok = |w, h| (w, h, Spoil::None);
        for order in [
            [ok(40, 30), ok(400, 300), ok(200, 150), ok(96, 64)],
            [ok(400, 300), ok(96, 64), ok(40, 30), ok(200, 150)],
            [ok(96, 64), ok(200, 150), ok(400, 300), ok(40, 30)],
        ] {
            let bytes = dng_with_previews(&order);
            assert_eq!(preview_size(&bytes, Largest), Some((400, 300)));
            assert_eq!(preview_size(&bytes, AtLeast(1)), Some((40, 30)));
            assert_eq!(preview_size(&bytes, AtLeast(41)), Some((96, 64)));
            assert_eq!(preview_size(&bytes, AtLeast(96)), Some((96, 64)));
            assert_eq!(preview_size(&bytes, AtLeast(150)), Some((200, 150)));
            assert_eq!(preview_size(&bytes, AtLeast(401)), Some((400, 300)));
        }
        // A portrait preview is measured on its long edge too.
        let bytes = dng_with_previews(&[ok(30, 40), ok(150, 200)]);
        assert_eq!(preview_size(&bytes, AtLeast(41)), Some((150, 200)));
        assert_eq!(preview_size(&bytes, AtLeast(40)), Some((30, 40)));
        // rawler's own pick, the first listed.
        let first = dng_with_previews(&[ok(40, 30), ok(400, 300)]);
        let source = RawSource::new_from_slice(&first);
        let decoder = ::rawler::get_decoder(&source).unwrap();
        let theirs = decoder
            .preview_image(&source, &RawDecodeParams::default())
            .unwrap()
            .unwrap();
        assert_eq!((theirs.width(), theirs.height()), (40, 30));
    }

    /// Of a PEF's JPEGs, its directories' and rawler's (the maker
    /// note's, here 72x48), the largest for a picture looked at whole,
    /// and the smallest at the size or past it for one made smaller: a
    /// grid's picture under the maker note's size takes it, decoded
    /// already, and nothing of the full one is decoded.
    #[test]
    fn a_pefs_preview_is_picked_by_size() {
        use super::super::pef::testing::{Spoil, pef_like};
        use PreviewSize::{AtLeast, Largest};
        let theirs = || Some(image::RgbImage::new(72, 48));
        let size = |bytes: &[u8], theirs: Option<image::RgbImage>, want| {
            pef_preview(bytes, theirs, want).map(|rgb| rgb.dimensions())
        };
        let ok = |w, h| (w, h, Spoil::None);
        for strips in [false, true] {
            let bytes = pef_like(&[ok(16, 12), ok(400, 300)], strips, false);
            assert_eq!(size(&bytes, theirs(), Largest), Some((400, 300)));
            assert_eq!(size(&bytes, theirs(), AtLeast(1)), Some((16, 12)));
            assert_eq!(size(&bytes, theirs(), AtLeast(17)), Some((72, 48)));
            assert_eq!(size(&bytes, theirs(), AtLeast(72)), Some((72, 48)));
            assert_eq!(size(&bytes, theirs(), AtLeast(73)), Some((400, 300)));
            assert_eq!(size(&bytes, theirs(), AtLeast(4000)), Some((400, 300)));
            assert_eq!(size(&bytes, None, AtLeast(17)), Some((400, 300)));
        }
        // The full one spoiled, each way: the next down, then rawler's.
        for spoil in [
            Spoil::Outside,
            Spoil::Truncated,
            Spoil::NotJpeg,
            Spoil::Lying,
            Spoil::Huge,
            Spoil::Bomb,
        ] {
            let bytes = pef_like(&[ok(16, 12), (400, 300, spoil)], false, false);
            assert_eq!(size(&bytes, theirs(), Largest), Some((72, 48)), "{spoil:?}");
            assert_eq!(size(&bytes, None, Largest), Some((16, 12)), "{spoil:?}");
            let none_good = pef_like(&[(400, 300, spoil)], false, false);
            assert_eq!(size(&none_good, theirs(), Largest), Some((72, 48)));
            assert_eq!(size(&none_good, None, Largest), None, "{spoil:?}");
        }
        // No TIFF at all: rawler's, else nothing.
        assert_eq!(size(b"not a tiff", theirs(), Largest), Some((72, 48)));
        assert_eq!(size(b"", None, AtLeast(10)), None);
        // Two of one tagged size, the first's header another size: the
        // first is tried once and the second taken.
        let twins = pef_like(&[(64, 48, Spoil::Lying), ok(128, 48)], false, false);
        assert_eq!(size(&twins, None, Largest), Some((128, 48)));
        let twins = pef_like(&[ok(128, 48), (64, 48, Spoil::Lying)], false, false);
        assert_eq!(size(&twins, None, Largest), Some((128, 48)));
    }

    /// A preview whose strip lies outside the file, is empty or absurdly
    /// long, is not a JPEG, or is a JPEG of another size than its tags
    /// say gives way to the next; with none left, the root's thumbnail,
    /// as rawler reads it, and with no previews at all the same.
    #[test]
    fn a_dngs_spoiled_previews_give_way_to_the_next() {
        use PreviewSize::{AtLeast, Largest};
        for spoil in [
            Spoil::Outside,
            Spoil::Empty,
            Spoil::Absurd,
            Spoil::NotJpeg,
            Spoil::Lying,
        ] {
            let bytes = dng_with_previews(&[
                (40, 30, Spoil::None),
                (400, 300, spoil),
                (200, 150, Spoil::None),
            ]);
            assert_eq!(preview_size(&bytes, Largest), Some((200, 150)));
            assert_eq!(preview_size(&bytes, AtLeast(300)), Some((200, 150)));
            assert_eq!(preview_size(&bytes, AtLeast(10)), Some((40, 30)));
            let none_good = dng_with_previews(&[(400, 300, spoil)]);
            assert_eq!(preview_size(&none_good, Largest), Some((180, 120)));
            // Passed over for its own fault, by the check meant for it.
            let source = RawSource::new_from_slice(&none_good);
            let decoder = ::rawler::get_decoder(&source).unwrap();
            let root = decoder
                .ifd(::rawler::decoders::WellKnownIFD::Root)
                .unwrap()
                .unwrap();
            let subs = root
                .get_sub_ifd_all(::rawler::tags::TiffCommonTag::SubIFDs)
                .unwrap();
            let width = if spoil == Spoil::Lying { 800 } else { 400 };
            let why = dng_preview_of(
                &DngPreview {
                    ifd: &subs[1],
                    width,
                    height: 300,
                },
                &source,
            )
            .unwrap_err();
            let said = match spoil {
                Spoil::Outside | Spoil::Empty | Spoil::Absurd => "a strip of",
                Spoil::NotJpeg => "not a JPEG",
                Spoil::Lying => "a JPEG of (400, 300)",
                Spoil::None => unreachable!(),
            };
            assert!(why.starts_with(said), "{why}");
        }
        let bare = dng_with_previews(&[]);
        assert_eq!(preview_size(&bare, Largest), Some((180, 120)));
        assert_eq!(preview_size(&bare, AtLeast(100)), Some((180, 120)));
    }

    /// No more than [`MAX_PREVIEWS`] previews are tried, the first
    /// listed, and entries that share a strip are tried once.
    #[test]
    fn a_dngs_previews_are_capped_and_one_a_strip() {
        let candidates = |bytes: &[u8]| -> Vec<(u32, u32)> {
            let source = RawSource::new_from_slice(bytes);
            let decoder = ::rawler::get_decoder(&source).unwrap();
            let root = decoder
                .ifd(::rawler::decoders::WellKnownIFD::Root)
                .unwrap()
                .unwrap();
            dng_previews_of(&root)
                .iter()
                .map(|p| (p.width, p.height))
                .collect()
        };
        let many: Vec<_> = (1..=40).map(|i| (8 * i, 6 * i, Spoil::None)).collect();
        let bytes = dng_with_previews(&many);
        let tried = candidates(&bytes);
        assert_eq!(tried.len(), MAX_PREVIEWS);
        assert_eq!(tried[15], (128, 96), "the first listed");
        // Of the sixteen the largest, not the 40th listed.
        assert_eq!(preview_size(&bytes, PreviewSize::Largest), Some((128, 96)));
        // Forty entries at one strip (all past the file's end): one.
        let shared: Vec<_> = (1..=40).map(|i| (8 * i, 6 * i, Spoil::Outside)).collect();
        assert_eq!(candidates(&dng_with_previews(&shared)), [(8, 6)]);
    }

    /// [`preview_path_at_least`] reads the file as [`preview_path`] does
    /// and picks by the size.
    #[test]
    fn a_dngs_preview_by_size_from_its_file() {
        let dir = std::env::temp_dir().join(format!("greycard-previews-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("several.dng");
        std::fs::write(
            &path,
            dng_with_previews(&[
                (40, 30, Spoil::None),
                (400, 300, Spoil::None),
                (96, 64, Spoil::None),
            ]),
        )
        .unwrap();
        let largest = preview_path(&path);
        let small = preview_path_at_least(&path, 50);
        crate::testing::remove_dir_retry(&dir);
        assert_eq!(largest.unwrap().unwrap().0.dimensions(), (400, 300));
        assert_eq!(small.unwrap().unwrap().0.dimensions(), (96, 64));
    }

    /// A mosaic DNG's `BlackLevelDeltaH` and `BlackLevelDeltaV` come
    /// through as the frame's differences, counted from its active
    /// area, and its black pattern stays as rawler gives it.
    #[test]
    fn a_dngs_black_deltas_are_read_from_its_active_area() {
        use ::rawler::formats::tiff::SRational;
        use ::rawler::imgop::{Dim2, Point};
        use ::rawler::rawimage::BlackLevel;

        let columns: Vec<i32> = vec![2, -4, 6, 0, 8, -2, 4, 10];
        let rows: Vec<i32> = vec![-6, 12, 0, 4, 2, -8];
        let halves = |v: &[i32]| v.iter().map(|&d| SRational::new(d, 2)).collect::<Vec<_>>();
        let black = BlackLevel::new(&[100u16, 104, 108, 112], 2, 2, 1)
            .with_deltas(halves(&columns), halves(&rows));
        let mut raw = a_mosaic(black);
        raw.active_area = Some(::rawler::imgop::Rect::new(
            Point::new(2, 2),
            Dim2::new(8, 6),
        ));
        let frame = frame_from_rawler(written_dng(&raw, |_| {})).unwrap();
        let half = |v: &[i32]| v.iter().map(|&d| d as f32 / 2.0).collect::<Vec<_>>();
        assert_eq!(
            frame.levels.black_deltas,
            Some(BlackDeltas {
                area: Rect {
                    x: 2,
                    y: 2,
                    width: 8,
                    height: 6
                },
                columns: half(&columns),
                rows: half(&rows),
            })
        );
        assert_eq!(frame.levels.black.values, vec![100.0, 104.0, 108.0, 112.0]);
        // Row 3 and column 4: the pattern's 108, the area's second row
        // (+6) and third column (+3).
        assert_eq!(frame.levels.black_at(3, 4, 0), 108.0 + 6.0 + 3.0);
        // Above the area its column's goes on and no row's, as rawler
        // adds them; above and left of it, neither.
        assert_eq!(frame.levels.black_at(1, 4, 0), 108.0 + 3.0);
        assert_eq!(frame.levels.black_at(1, 1, 0), 112.0);
    }

    /// A DNG's `CameraCalibration` and `AnalogBalance` go into its
    /// color matrix, as the specification builds XYZ to camera, so the
    /// as-shot white a phone gives as `AsShotWhiteXY` reads back as that
    /// white: the same temperature and tint with a calibration as
    /// without one.
    #[test]
    fn a_dngs_camera_calibration_goes_into_its_matrix() {
        use ::rawler::camera_profile::CameraProfile;
        use ::rawler::formats::tiff::Rational;
        use ::rawler::imgop::xyz::Illuminant;
        use ::rawler::rawimage::BlackLevel;
        use ::rawler::tags::DngTag;

        let cm = vec![
            0.7107, -0.0861, -0.1265, -0.5989, 1.571, -0.0016, -0.2112, 0.4405, 0.4113,
        ];
        let decoded = |calibrate: bool| {
            let mut raw = a_mosaic(BlackLevel::new(&[100u16], 1, 1, 1));
            let mut profile = CameraProfile::default();
            profile.calibration_illuminant[0] = Some(Illuminant::D65);
            profile.color_matrix[0] = Some(cm.clone());
            raw.camera_profiles = vec![profile];
            if calibrate {
                raw.camera_calibration.matrices[0] =
                    Some(vec![1.0251, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.9713]);
                raw.analog_balance = Some(vec![1.05, 1.0, 0.98]);
            }
            frame_from_rawler(written_dng(&raw, |root| {
                root.remove_tag(DngTag::AsShotNeutral);
                root.add_tag(
                    DngTag::AsShotWhiteXY,
                    [
                        Rational::new(360849, 1_000_000),
                        Rational::new(378146, 1_000_000),
                    ],
                );
            }))
            .unwrap()
        };
        let plain = decoded(false);
        let calibrated = decoded(true);
        assert_eq!(plain.calibrations[0].color_matrix, cm);
        let want: Vec<f32> = cm
            .as_chunks::<3>()
            .0
            .iter()
            .zip([1.0251 * 1.05, 1.0, 0.9713 * 0.98])
            .flat_map(|(row, g)| row.iter().map(move |v| v * g))
            .collect();
        for (got, want) in calibrated.calibrations[0].color_matrix.iter().zip(&want) {
            assert!((got - want).abs() < 1e-5, "{got} != {want}");
        }
        let temp_tint = |frame: &RawFrame| {
            let profile = crate::color::profile_from_frame(frame).unwrap();
            crate::color::as_shot_temp_tint(frame, &profile).unwrap()
        };
        let file = rawcolor::cct::chromaticity_to_temp_tint(rawcolor::Chromaticity {
            x: 0.360849,
            y: 0.378146,
        })
        .unwrap();
        assert_ne!(plain.as_shot_coefficients, calibrated.as_shot_coefficients);
        for got in [temp_tint(&plain), temp_tint(&calibrated)] {
            assert!((got.cct - file.cct).abs() < 1.0, "{got:?} != {file:?}");
            assert!((got.duv - file.duv).abs() < 1e-4, "{got:?} != {file:?}");
        }
    }

    /// A DNG with a white for each of two illuminants reads its
    /// `AsShotWhiteXY` back as that white through both, where rawler's
    /// gains, made with one illuminant's matrix, read back hundreds of
    /// kelvin off; [`white_path`]'s gains are the frame's.
    #[test]
    fn a_dngs_white_xy_reads_back_through_both_its_matrices() {
        use ::rawler::camera_profile::CameraProfile;
        use ::rawler::formats::tiff::Rational;
        use ::rawler::imgop::xyz::Illuminant;
        use ::rawler::rawimage::BlackLevel;
        use ::rawler::tags::DngTag;

        let mut raw = a_mosaic(BlackLevel::new(&[100u16], 1, 1, 1));
        raw.camera_profiles = vec![CameraProfile {
            calibration_illuminant: [Some(Illuminant::A), Some(Illuminant::D65), None],
            color_matrix: [
                Some(vec![
                    1.0426, -0.4594, -0.0588, -0.3576, 1.2977, 0.0571, -0.03, 0.1922, 0.6598,
                ]),
                Some(vec![
                    0.7107, -0.0861, -0.1265, -0.5989, 1.571, -0.0016, -0.2112, 0.4405, 0.4113,
                ]),
                None,
            ],
            ..Default::default()
        }];
        let white_xy = |root: &mut ::rawler::formats::tiff::writer::DirectoryWriter| {
            root.remove_tag(DngTag::AsShotNeutral);
            root.add_tag(
                DngTag::AsShotWhiteXY,
                [
                    Rational::new(360849, 1_000_000),
                    Rational::new(378146, 1_000_000),
                ],
            );
        };
        let rawlers = as_shot(written_dng(&raw, white_xy).wb_coeffs).unwrap();
        let dir = std::env::temp_dir().join(format!("greycard-white-xy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("white.dng");
        std::fs::write(&path, dng_bytes(&raw, white_xy)).unwrap();
        let frame = RawlerDecoder.decode_path(&path);
        let white = white_path(&path);
        crate::testing::remove_dir_retry(&dir);
        let (frame, white) = (frame.unwrap(), white.unwrap());
        assert_eq!(white.as_shot_coefficients, frame.as_shot_coefficients);

        let file = rawcolor::cct::chromaticity_to_temp_tint(rawcolor::Chromaticity {
            x: 0.360849,
            y: 0.378146,
        })
        .unwrap();
        let profile = crate::color::profile_from_frame(&frame).unwrap();
        let got = crate::color::as_shot_temp_tint(&frame, &profile).unwrap();
        assert!((got.cct - file.cct).abs() < 1.0, "{got:?} != {file:?}");
        assert!((got.duv - file.duv).abs() < 1e-4, "{got:?} != {file:?}");
        let theirs = crate::color::temp_tint_of_gains(&profile, rawlers).unwrap();
        assert!((theirs.cct - file.cct).abs() > 50.0, "{theirs:?}");
    }

    /// A raw without differences has none, and its black is the
    /// pattern's alone.
    #[test]
    fn a_dng_without_black_deltas_has_none() {
        let frame = RawlerDecoder.decode_bytes(&a_dng()).unwrap();
        assert_eq!(frame.levels.black_deltas, None);
    }

    /// A linear DNG of our own writing: a raw rawler reads, which no
    /// sample raw in the repo may stand in for.
    fn a_dng() -> Vec<u8> {
        a_dng_of(16, 12)
    }

    /// A linear DNG `width` by `height`, its directory written last.
    fn a_dng_of(width: usize, height: usize) -> Vec<u8> {
        let frame = crate::develop::tests::flat_frame();
        let data = (0..width * height * 3)
            .map(|i| 0.05 + (i % 97) as f32 / 100.0)
            .collect();
        let image = crate::image::CameraImage::from_data(width, height, data).unwrap();
        let mut buf = std::io::Cursor::new(Vec::new());
        crate::dng::write_linear_dng(
            &mut buf,
            &frame,
            &image,
            [2.0, 1.0, 4.0],
            &Default::default(),
        )
        .unwrap();
        buf.into_inner()
    }

    /// A file rawler reads to the end without complaint, as it does a
    /// half-copied ARW, is refused when its tail is zeros, with the
    /// same words as a panic on one; one byte short of a whole tail of
    /// zeros, or none at all, and it decodes as before. The DNG writes
    /// its directory last, so the zeros go after it: rawler finds
    /// everything it needs ahead of them, which is the ARW's case.
    #[test]
    fn a_decode_that_reads_zeros_to_the_end_is_refused_as_a_half_copy() {
        let dng = a_dng();
        let padded = |zeros: usize| [dng.clone(), vec![0; zeros]].concat();
        let copying = padded(TAIL);
        let dir = std::env::temp_dir().join(format!("greycard-half-copy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("copying.dng");
        std::fs::write(&path, &copying).unwrap();

        // rawler on its own takes it for a finished file.
        let source = RawSource::new_from_slice(&copying);
        let decoder = ::rawler::get_decoder(&source).unwrap();
        assert!(
            decoder
                .raw_image(&source, &RawDecodeParams::default(), false)
                .is_ok()
        );

        let refusals = [
            RawlerDecoder.decode_bytes(&copying).map(|_| ()),
            RawlerDecoder.decode_path(&path).map(|_| ()),
            RawlerDecoder.decode_path_with_metadata(&path).map(|_| ()),
        ];
        // What sits at the head of the file still reads; a browser
        // shows the frame while it copies.
        let probe = probe_path(&path);
        let stance = stance_path(&path);
        crate::testing::remove_dir_retry(&dir);
        for e in refusals {
            match e {
                Err(Error::Decode(text)) => assert_eq!(text, HALF_COPIED),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(probe.unwrap().make, "Test");
        assert!(stance.is_ok());

        // The DNG ends in a few zeros of its own; one data byte in the
        // last 64 KiB is enough.
        let own = dng.len() - 1 - dng.iter().rposition(|&b| b != 0).unwrap();
        for finished in [dng.clone(), padded(TAIL - 1 - own)] {
            let frame = RawlerDecoder.decode_bytes(&finished).unwrap();
            assert_eq!((frame.width, frame.height), (16, 12));
        }
    }

    /// The white read without a sample is the decoded frame's.
    #[test]
    fn the_white_without_the_samples_is_the_frames() {
        let dir = std::env::temp_dir().join(format!("greycard-white-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("white.dng");
        std::fs::write(&path, a_dng()).unwrap();
        let white = white_path(&path);
        let frame = RawlerDecoder.decode_path(&path);
        crate::testing::remove_dir_retry(&dir);
        let (white, frame) = (white.unwrap(), frame.unwrap());
        assert!(white.as_shot_coefficients.is_some());
        assert_eq!(white.as_shot_coefficients, frame.as_shot_coefficients);
        assert_eq!(white.calibrations, frame.calibrations);
        assert!(!white.calibrations.is_empty());
    }

    /// A monochrome sensor's DNG (LinearRaw, one sample a pixel) is a
    /// monochrome frame, neutral as shot with no matrix, whether read
    /// whole, for its white alone, or for the index.
    #[test]
    fn a_monochrome_dng_is_a_monochrome_frame() {
        let frame = crate::develop::tests::mono_frame(16, 12, |y, x| 100 + (y + x) as u16 * 30);
        let out = crate::develop::demosaic(&frame, &Default::default()).unwrap();
        let mut dng = std::io::Cursor::new(Vec::new());
        crate::dng::write_linear_dng(&mut dng, &frame, &out.image, [1.0; 3], &Default::default())
            .unwrap();
        let dir = std::env::temp_dir().join(format!("greycard-mono-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mono.dng");
        std::fs::write(&path, dng.into_inner()).unwrap();
        let decoded = RawlerDecoder.decode_path(&path);
        let white = white_path(&path);
        let index = index_read_path(&path);
        crate::testing::remove_dir_retry(&dir);
        let decoded = decoded.unwrap();
        assert_eq!(decoded.layout, SensorLayout::Monochrome);
        assert_eq!(decoded.as_shot_coefficients, Some([1.0; 3]));
        let white = white.unwrap();
        assert_eq!(white.as_shot_coefficients, Some([1.0; 3]));
        assert!(white.calibrations.is_empty() && white.monochrome);
        let profile = white.profile().unwrap();
        let tt = crate::color::temp_tint_of_gains(&profile, [1.0; 3]).unwrap();
        assert!((tt.cct - 6504.0).abs() < 10.0);
        assert_eq!(index.unwrap().1, MosaicRead::NotMosaic);
    }

    /// The table is the ISOs of each body and shutter measured blown in
    /// every channel, and its margin is SHARE of the way from the black
    /// to the 14-bit top: an ISO, a cell, or a margin added or changed
    /// fails here, on purpose.
    #[test]
    fn the_headroom_table_is_the_isos_measured() {
        let cells: Vec<_> = CANON_HEADROOM
            .iter()
            .map(|c| (c.model, c.shutter, c.isos, c.frame))
            .collect();
        let (r6, r5) = ((6000, 4000), (8192, 5464));
        let first_curtain: &[u32] = &[
            100, 125, 160, 200, 250, 320, 400, 500, 1250, 1600, 2500, 12800,
        ];
        assert_eq!(
            cells,
            [
                ("EOS R6 Mark II", 0, &[100u32, 320][..], r6),
                ("EOS R6 Mark II", 1, first_curtain, r6),
                ("EOS R5 Mark II", 1, &[100u32][..], r5),
                ("EOS R5 Mark II", 2, &[100u32][..], r5),
                ("EOS 850D", 1, &[200u32][..], (6000, 4000)),
            ]
        );
        assert_eq!(SATURATION_SHARE, SHARE);
        assert_eq!(CANON_TOP, 16383.0);
    }

    /// The margin, written out: what the table's share is pinned to.
    const SHARE: f32 = 0.955;

    /// The modes of a full-frame R6 II capture with every mode said and
    /// off, first curtain.
    fn plain(frame: (i64, i64)) -> super::super::style::CanonModes {
        super::super::style::CanonModes {
            shutter: Some(1),
            highlight_tone_priority: Some(0),
            long_exposure_nr: Some(0),
            dual_pixel_raw: Some(0),
            hdr: Some(0),
            aspect: Some((0, frame.0, frame.1)),
        }
    }

    /// A Canon frame of `model` at `black`, `white`, `iso`, and an
    /// exposure of `t`.
    fn canon_frame(model: &str, black: f32, white: f32, iso: u32, t: f32) -> RawFrame {
        let mut frame = crate::develop::tests::flat_frame();
        frame.make = "Canon".into();
        frame.model = model.into();
        frame.levels.black = LevelPattern::uniform(black, 1);
        frame.levels.white = LevelPattern::uniform(white, 1);
        frame.shot.iso = Some(iso);
        frame.shot.exposure_time = Some(t);
        frame
    }

    /// A measured capture gets its saturation; another body, ISO,
    /// shutter, frame, a white of fewer bits, and a white at the top get
    /// none.
    #[test]
    fn a_measured_canon_saturates_under_its_top() {
        use super::super::style::CanonModes;
        let r6 = plain((6000, 4000));
        let canon = |model: &str, black: f32, white: f32, iso: u32, modes: &CanonModes| {
            canon_saturation(&canon_frame(model, black, white, iso, 0.01), Some(modes))
        };
        let top = |black: f32| black + SHARE * (16383.0 - black);
        assert_eq!(
            canon("EOS R6 Mark II", 512.0, 14008.0, 100, &r6),
            Some(top(512.0))
        );
        assert_eq!(
            canon("EOS R6 Mark II", 2048.0, 14888.0, 12800, &r6),
            Some(top(2048.0))
        );
        // Each ISO of each cell in, the ones between out.
        for cell in &CANON_HEADROOM {
            let modes = CanonModes {
                shutter: Some(cell.shutter),
                aspect: Some((0, cell.frame.0, cell.frame.1)),
                ..r6
            };
            for iso in 50..=25600u32 {
                let got = canon(cell.model, 512.0, 14888.0, iso, &modes);
                assert_eq!(
                    got.is_some(),
                    cell.isos.contains(&iso),
                    "{} {} {iso}",
                    cell.model,
                    cell.shutter
                );
            }
        }
        for unmeasured in ["EOS R5", "EOS R6", "EOS R7", "EOS R8", "EOS R3"] {
            assert_eq!(
                canon(unmeasured, 512.0, 14008.0, 100, &r6),
                None,
                "{unmeasured}"
            );
        }
        let with = |shutter| CanonModes {
            shutter: Some(shutter),
            ..r6
        };
        assert_eq!(
            canon("EOS R6 Mark II", 512.0, 14888.0, 400, &with(0)),
            None,
            "mechanical 400"
        );
        assert_eq!(
            canon("EOS R6 Mark II", 512.0, 14008.0, 100, &with(2)),
            None,
            "R6 II electronic"
        );
        let r5 = |shutter| CanonModes {
            shutter: Some(shutter),
            ..plain((8192, 5464))
        };
        assert_eq!(
            canon("EOS R5 Mark II", 512.0, 14008.0, 100, &r5(0)),
            None,
            "R5 II mechanical"
        );
        assert_eq!(
            canon("EOS R5 Mark II", 512.0, 14888.0, 125, &r5(2)),
            None,
            "R5 II 125"
        );
        assert!(canon("EOS R5 Mark II", 512.0, 14008.0, 100, &r5(2)).is_some());
        assert_eq!(
            canon("EOS R6 Mark II", 128.0, 3500.0, 100, &r6),
            None,
            "12-bit"
        );
        assert_eq!(
            canon("EOS R6 Mark II", 512.0, 16383.0, 100, &r6),
            None,
            "at the top"
        );
        let mut sony = canon_frame("EOS R6 Mark II", 512.0, 14008.0, 100, 0.01);
        sony.make = "Sony".into();
        assert_eq!(canon_saturation(&sony, Some(&r6)), None);
    }

    /// The 850D is in under each name it is sold by, as rawler reads
    /// them: first curtain at ISO 200, and no other ISO or shutter. A
    /// name with no other names behind it is only itself.
    #[test]
    fn the_850d_is_in_under_each_of_its_names() {
        use super::super::style::CanonModes;
        let fc = plain((6000, 4000));
        let named = |model: &str, white: f32, iso: u32| {
            let mut frame = canon_frame(model, 511.0, white, iso, 0.5);
            frame.other_names = other_names("Canon", model);
            frame
        };
        let top = 511.0 + SHARE * (16383.0 - 511.0);
        for model in ["EOS 850D", "EOS Rebel T8i", "EOS Kiss X10i"] {
            assert_eq!(
                canon_saturation(&named(model, 14338.0, 200), Some(&fc)),
                Some(top),
                "{model} 200"
            );
            // ISO 100's white sits at 0.70 of the range: 0.45 stops
            // unverified, lavender in that band on one frame; it waits
            // on a bracket.
            assert_eq!(
                canon_saturation(&named(model, 11606.0, 100), Some(&fc)),
                None,
                "{model} 100"
            );
            for iso in [125, 160, 250, 400, 800] {
                assert_eq!(
                    canon_saturation(&named(model, 14338.0, iso), Some(&fc)),
                    None,
                    "{model} {iso}"
                );
            }
            for shutter in [0, 2] {
                let modes = CanonModes {
                    shutter: Some(shutter),
                    ..fc
                };
                assert_eq!(
                    canon_saturation(&named(model, 14338.0, 200), Some(&modes)),
                    None,
                    "{model} shutter {shutter}"
                );
            }
        }
        let alone = canon_frame("EOS Rebel T8i", 511.0, 14338.0, 200, 0.5);
        assert_eq!(canon_saturation(&alone, Some(&fc)), None);
    }

    /// Never a wrong yes: a frame that is in the table in every other
    /// way gets no saturation when its maker note is missing, when any
    /// mode that can move the clip is on or not said, when the frame is
    /// cropped or of another aspect or not said, when its exposure is a
    /// second or more or not said, and when its ISO is not said.
    #[test]
    fn a_mode_on_or_unsaid_gets_no_saturation() {
        use super::super::style::CanonModes;
        let r6 = plain((6000, 4000));
        let frame = canon_frame("EOS R6 Mark II", 512.0, 14008.0, 100, 0.01);
        assert!(
            canon_saturation(&frame, Some(&r6)).is_some(),
            "the frame is in"
        );
        assert_eq!(canon_saturation(&frame, None), None, "no maker note");
        let off: [(&str, CanonModes); 16] = [
            (
                "HTP on",
                CanonModes {
                    highlight_tone_priority: Some(1),
                    ..r6
                },
            ),
            (
                "HTP unsaid",
                CanonModes {
                    highlight_tone_priority: None,
                    ..r6
                },
            ),
            (
                "LENR auto",
                CanonModes {
                    long_exposure_nr: Some(1),
                    ..r6
                },
            ),
            (
                "LENR on",
                CanonModes {
                    long_exposure_nr: Some(2),
                    ..r6
                },
            ),
            (
                "LENR unsaid",
                CanonModes {
                    long_exposure_nr: None,
                    ..r6
                },
            ),
            (
                "dual pixel raw",
                CanonModes {
                    dual_pixel_raw: Some(1),
                    ..r6
                },
            ),
            (
                "dual pixel raw unsaid",
                CanonModes {
                    dual_pixel_raw: None,
                    ..r6
                },
            ),
            ("HDR", CanonModes { hdr: Some(1), ..r6 }),
            ("HDR unsaid", CanonModes { hdr: None, ..r6 }),
            (
                "shutter unsaid",
                CanonModes {
                    shutter: None,
                    ..r6
                },
            ),
            (
                "crop",
                CanonModes {
                    aspect: Some((0, 3744, 2496)),
                    ..r6
                },
            ),
            (
                "another aspect",
                CanonModes {
                    aspect: Some((1, 6000, 4000)),
                    ..r6
                },
            ),
            ("aspect unsaid", CanonModes { aspect: None, ..r6 }),
            (
                "the R5 II's frame",
                CanonModes {
                    aspect: Some((0, 8192, 5464)),
                    ..r6
                },
            ),
            (
                "a narrower frame",
                CanonModes {
                    aspect: Some((0, 5999, 4000)),
                    ..r6
                },
            ),
            (
                "a shorter frame",
                CanonModes {
                    aspect: Some((0, 6000, 3999)),
                    ..r6
                },
            ),
        ];
        for (what, modes) in off {
            assert_eq!(canon_saturation(&frame, Some(&modes)), None, "{what}");
        }
        let second = canon_frame("EOS R6 Mark II", 512.0, 14008.0, 100, 1.0);
        assert_eq!(canon_saturation(&second, Some(&r6)), None, "a second");
        let mut no_time = frame.clone();
        no_time.shot.exposure_time = None;
        assert_eq!(
            canon_saturation(&no_time, Some(&r6)),
            None,
            "exposure unsaid"
        );
        let mut no_iso = frame.clone();
        no_iso.shot.iso = None;
        assert_eq!(canon_saturation(&no_iso, Some(&r6)), None, "ISO unsaid");
    }

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

    /// A forward matrix goes to the calibration of its own illuminant,
    /// and one for an illuminant with no color matrix is dropped.
    #[test]
    fn forward_matrices_land_on_their_illuminants_calibration() {
        let cal = |illuminant| Calibration {
            illuminant,
            color_matrix: vec![1.0; 9],
            forward_matrix: None,
        };
        let mut cals = vec![cal(17), cal(21)];
        let a = vec![0.5; 9];
        let d65 = vec![0.25; 9];
        with_forward_matrices(
            &mut cals,
            &[(21, d65.clone()), (17, a.clone()), (23, vec![0.0; 9])],
        );
        assert_eq!(cals[0].forward_matrix.as_ref(), Some(&a));
        assert_eq!(cals[1].forward_matrix.as_ref(), Some(&d65));
        let mut none = vec![cal(21)];
        with_forward_matrices(&mut none, &[]);
        assert_eq!(none[0].forward_matrix, None);
    }

    /// A DNG's forward matrices land on its calibrations, except in a
    /// file CHDK wrote, which develops through its color matrices alone.
    #[test]
    fn a_chdk_dngs_forward_matrices_are_left() {
        use ::rawler::camera_profile::CameraProfile;
        use ::rawler::imgop::xyz::Illuminant;
        use ::rawler::rawimage::BlackLevel;
        use ::rawler::tags::TiffCommonTag;

        let color = |d: f32| vec![1.0 + d, -0.4, -0.1, -0.3, 1.3, 0.05, -0.03, 0.2, 0.6];
        let forward = |d: f32| vec![0.6 + d, 0.2, 0.1643 - d, 0.25, 0.7, 0.05, 0.03, 0.0, 0.7949];
        let mut raw = a_mosaic(BlackLevel::new(&[100u16], 1, 1, 1));
        raw.camera_profiles = vec![CameraProfile {
            calibration_illuminant: [Some(Illuminant::A), Some(Illuminant::D65), None],
            color_matrix: [Some(color(0.1)), Some(color(0.0)), None],
            forward_matrix: [Some(forward(0.05)), Some(forward(0.0)), None],
            ..Default::default()
        }];
        let decoded = |software: Option<&str>| {
            let bytes = dng_bytes(&raw, |root| {
                if let Some(s) = software {
                    root.add_tag(TiffCommonTag::Software, s);
                }
            });
            RawlerDecoder.decode_bytes(&bytes).unwrap()
        };
        let forwards = |frame: &RawFrame| {
            let mut f: Vec<_> = frame
                .calibrations
                .iter()
                .map(|c| (c.illuminant, c.forward_matrix.clone()))
                .collect();
            f.sort_by_key(|(illuminant, _)| *illuminant);
            f
        };
        let near = |got: &Option<Vec<f32>>, want: Vec<f32>| {
            let got = got.as_ref().expect("a forward matrix");
            got.iter().zip(&want).all(|(g, w)| (g - w).abs() < 1e-3)
        };
        for software in [None, Some("Adobe DNG Converter 17.0"), Some("HDR+ 1.0")] {
            let f = forwards(&decoded(software));
            assert_eq!(f.len(), 2, "{software:?}");
            assert!(near(&f[0].1, forward(0.05)), "{software:?}: {f:?}");
            assert!(near(&f[1].1, forward(0.0)), "{software:?}: {f:?}");
        }
        for software in ["CHDK ver. 1.6.1", "CHDK_DE ver. 1.6.1"] {
            let frame = decoded(Some(software));
            assert_eq!(frame.calibrations.len(), 2, "{software}");
            assert!(
                frame
                    .calibrations
                    .iter()
                    .all(|c| c.forward_matrix.is_none()),
                "{software}: {:?}",
                forwards(&frame)
            );
        }
        assert!(reads_forward_matrices("Adobe DNG Converter 17.0"));
        assert!(reads_forward_matrices(""));
        assert!(!reads_forward_matrices(" CHDK 1.5.1"));
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
    fn a_nikon_high_efficiency_raw_says_this_build_does_not_read_it() {
        // rawler's words for HE and HE* without its JPEG XS decoder,
        // spelled as rawler spells them and as it might one day.
        for mode in [
            "HighEfficency",
            "HighEfficencyStar",
            "HighEfficiency",
            "HighEfficiencyStar",
        ] {
            let e = decoder_error(::rawler::RawlerError::DecoderFailed(format!(
                "NEF compression Some({mode}) is not supported"
            )));
            assert!(matches!(e, Error::Unsupported(_)), "{e}");
            let text = e.to_string();
            assert!(text.contains("Nikon High Efficiency"), "{text}");
            assert!(text.contains("not supported in this build"), "{text}");
        }
        // Any other NEF failure is passed through.
        let other = decoder_error(::rawler::RawlerError::DecoderFailed(
            "NEF: JPEG XS decoded a 2x2 mosaic but the IFD says 4x4".into(),
        ));
        assert!(matches!(other, Error::Decode(_)), "{other}");
    }

    /// The head of a sample CR3 through its `mdat` box's header with
    /// every byte zeroed but the top-level boxes' headers and the `ftyp`
    /// box, which is the brands: 82 bytes of 204,288 not zero, and no
    /// picture, maker note or serial number in it. For the walk only:
    /// rawler given a `moov` box that zeros cut short asked for 6 GiB.
    fn cr3_head_fixture() -> Vec<u8> {
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scrubbed-cr3-head.CR3"),
        )
        .unwrap()
    }

    /// The walk over a CR3's top-level boxes ends with the `mdat` box's
    /// header, whether that box's size is 32 or 64 bits, whatever the
    /// rest of the file; a file that is not boxes, or whose boxes do not
    /// hold together or run out first, has no head to read.
    #[test]
    fn the_box_walk_ends_with_the_mdat_boxs_header() {
        use std::io::Cursor;
        let head = cr3_head_fixture();
        // The camera's file ran 15,570,066 bytes; the walk reads none
        // past the header.
        let walk = |bytes: &[u8], len: u64| cr3_head(&mut Cursor::new(bytes), len).unwrap();
        assert_eq!(walk(&head, 15_570_066), Some(204_288));
        assert_eq!(walk(&head, head.len() as u64), Some(204_288));
        // The camera wrote `mdat` with a 64-bit size; the same box with
        // a 32-bit one has an 8-byte header.
        let at = head.len() - 16;
        assert_eq!(&head[at + 4..at + 8], b"mdat");
        let mut short = head[..at].to_vec();
        short.extend_from_slice(&[0, 0, 0, 8]);
        short.extend_from_slice(b"mdat");
        assert_eq!(walk(&short, 1 << 24), Some(at as u64 + 8));

        // Not a CR3: no `ftyp` first.
        let mut other = head.clone();
        other[4..8].copy_from_slice(b"free");
        assert_eq!(walk(&other, 15_570_066), None);
        // A box smaller than its own header.
        let mut broken = head.clone();
        broken[24..28].copy_from_slice(&4u32.to_be_bytes());
        assert_eq!(walk(&broken, 15_570_066), None);
        // A file that ends before its `mdat`.
        assert_eq!(walk(&head[..at], at as u64), None);
        // A first box whose size reaches past the end of the file.
        assert_eq!(walk(&head[..24], 20), None);
    }

    /// A RAF's head runs through its JPEG's EXIF segment, by the length
    /// that segment gives; a file with another magic, or a JPEG that
    /// does not open with EXIF, has none.
    #[test]
    fn a_rafs_head_is_its_jpegs_exif_segment() {
        use std::io::Cursor;
        let raf = |magic: &[u8], app: u8| {
            let mut bytes = vec![0u8; 148];
            bytes[..magic.len()].copy_from_slice(magic);
            bytes[84..88].copy_from_slice(&148u32.to_be_bytes());
            bytes.extend_from_slice(&[0xff, 0xd8, 0xff, app, 0xff, 0xa8]);
            bytes.resize(1 << 20, 0);
            bytes
        };
        let head = |bytes: &[u8]| raf_head(&mut Cursor::new(bytes), bytes.len() as u64).unwrap();
        assert_eq!(
            head(&raf(b"FUJIFILMCCD-RAW 0201", 0xe1)),
            Some(148 + 4 + 0xffa8)
        );
        assert_eq!(head(&raf(b"FUJIFILMCCD-RAX 0201", 0xe1)), None);
        assert_eq!(head(&raf(b"FUJIFILMCCD-RAW 0201", 0xdb)), None);
        assert_eq!(head(&raf(b"FUJIFILMCCD-RAW 0201", 0xe1)[..100]), None);
    }

    /// Ends that do not parse fall back to the whole file and give its
    /// answer; ends that do give the same answer from fewer bytes; a
    /// file that is not of its extension's format is read whole.
    #[test]
    fn ends_that_do_not_parse_fall_back_to_the_whole_file() {
        let dng = a_dng();
        // Named as the picture probe's tests name theirs, so a run beside
        // another, or after one that stopped short, has its own.
        let dir = std::env::temp_dir().join(format!(
            "greycard-ends-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("frame.dng");
        std::fs::write(&path, &dng).unwrap();
        let len = dng.len() as u64;
        let whole = probe_path(&path).unwrap();
        assert_eq!(whole.make, "Test");

        // Eight bytes of head: the TIFF header and nothing it points to.
        let (probe, read) = probe_ends_with(&path, |_, _, _| Ok(Some((8, 0)))).unwrap();
        assert_eq!(
            (probe, read),
            (whole.clone(), len + 8),
            "the ends, then the whole"
        );
        // The DNG writes its directory last, so its samples' first byte
        // is not wanted for the probe.
        let (probe, read) = probe_ends_with(&path, |_, _, len| Ok(Some((8, len - 9)))).unwrap();
        assert_eq!((probe, read), (whole.clone(), len - 1));
        // Called a CR3 or a RAF, it has neither one's head.
        for name in ["frame.CR3", "frame.RAF"] {
            let named = dir.join(name);
            std::fs::write(&named, &dng).unwrap();
            assert_eq!(probe_ends_counted(&named).unwrap(), (whole.clone(), len));
        }
        // Not a raw at all: the whole file's error.
        let junk = dir.join("junk.CR3");
        std::fs::write(&junk, b"not a raw").unwrap();
        assert_eq!(
            probe_ends_path(&junk).unwrap_err().to_string(),
            probe_path(&junk).unwrap_err().to_string()
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// The probe and the stance read together give what the two calls
    /// apart give, from the whole file for a format read whole and when
    /// the ends will not parse.
    #[test]
    fn the_probe_and_the_stance_together_are_the_two_apart() {
        let dng = a_dng();
        let dir = std::env::temp_dir().join(format!(
            "greycard-both-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        watch_maps_under(&dir);
        let len = dng.len() as u64;
        let mut maps = 0;
        for name in ["frame.dng", "frame.CR3", "frame.RAF"] {
            let path = dir.join(name);
            std::fs::write(&path, &dng).unwrap();
            let apart = (probe_path(&path).unwrap(), stance_path(&path).unwrap(), len);
            // The calls apart map; together, from the ends or whole,
            // never.
            assert!(maps_under(&dir) > maps, "{name}");
            maps = maps_under(&dir);
            assert_eq!(probe_and_stance_counted(&path).unwrap(), apart, "{name}");
            // Ends that cut the directory off: the whole file's answer,
            // the ends counted with it.
            let short = probe_and_stance_with(&path, |_, _, _| Ok(Some((8, 0)))).unwrap();
            assert_eq!(short, (apart.0.clone(), apart.1, len + 8), "{name}");
            assert_eq!(probe_ends_counted(&path).unwrap(), (apart.0.clone(), len));
            assert_eq!(maps_under(&dir), maps, "{name} mapped");
        }
        // Not a raw by its name: nothing read, none of it probed.
        let clip = dir.join("clip.mov");
        std::fs::write(&clip, &dng).unwrap();
        assert!(probe_and_stance_counted(&clip).is_err());
        assert!(probe_ends_counted(&clip).is_err());
        assert_eq!(maps_under(&dir), maps);
        let junk = dir.join("junk.CR3");
        std::fs::write(&junk, b"not a raw").unwrap();
        assert_eq!(
            probe_and_stance_counted(&junk).unwrap_err().to_string(),
            probe_path(&junk).unwrap_err().to_string()
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// With `GREYCARD_TEST_RAWS` a folder of raws: each raw's probe and
    /// stance read together against the two read apart, required to
    /// agree, and the bytes printed by format. Ignored, since it wants
    /// the samples.
    #[test]
    #[ignore]
    fn the_probe_and_the_stance_together_agree_on_the_samples() {
        let Some(dir) = std::env::var_os("GREYCARD_TEST_RAWS") else {
            eprintln!("GREYCARD_TEST_RAWS names no folder; nothing measured");
            return;
        };
        let mut raws = Vec::new();
        let mut folders = vec![std::path::PathBuf::from(dir)];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                let path = entry.path();
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    folders.push(path);
                } else if crate::decode::is_raw_path(&path) {
                    raws.push(path);
                }
            }
        }
        raws.sort();
        assert!(!raws.is_empty(), "no raws under GREYCARD_TEST_RAWS");
        let mut totals: std::collections::BTreeMap<String, (usize, u64, u64)> = Default::default();
        for path in &raws {
            let len = std::fs::metadata(path).unwrap().len();
            let format = path
                .extension()
                .unwrap()
                .to_string_lossy()
                .to_ascii_uppercase();
            match (probe_path(path), probe_and_stance_counted(path)) {
                (Ok(probe), Ok((together, stance, read))) => {
                    assert_eq!(together, probe, "{}", path.display());
                    assert_eq!(stance, stance_path(path).unwrap(), "{}", path.display());
                    let t = totals.entry(format).or_default();
                    *t = (t.0 + 1, t.1 + read, t.2 + len);
                }
                (Err(a), Err(b)) => {
                    assert_eq!(a.to_string(), b.to_string(), "{}", path.display());
                }
                (whole, together) => {
                    panic!("{}: {whole:?} apart, {together:?} together", path.display())
                }
            }
        }
        for (format, (n, r, l)) in &totals {
            println!(
                "{format}: {n} files, {:.1} MB read of {:.1} MB ({:.2}%)",
                *r as f64 / 1e6,
                *l as f64 / 1e6,
                100.0 * *r as f64 / *l as f64
            );
        }
    }

    /// With `GREYCARD_TEST_RAWS` a folder of raws: every raw in it and
    /// its subfolders probed from its ends and whole, the two required
    /// to agree, and the bytes each read printed, a file and a format
    /// at a time. One file at a time, its buffers dropped before the
    /// next. Ignored, since it wants the samples.
    #[test]
    #[ignore]
    fn the_ends_give_the_whole_files_probe_on_the_samples() {
        let Some(dir) = std::env::var_os("GREYCARD_TEST_RAWS") else {
            eprintln!("GREYCARD_TEST_RAWS names no folder; nothing measured");
            return;
        };
        let mut raws = Vec::new();
        let mut folders = vec![std::path::PathBuf::from(dir)];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                let path = entry.path();
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    folders.push(path);
                } else if crate::decode::is_raw_path(&path) {
                    raws.push(path);
                }
            }
        }
        raws.sort();
        assert!(!raws.is_empty(), "no raws under GREYCARD_TEST_RAWS");
        // By format: files, bytes the ends read, bytes in the files.
        let mut totals: std::collections::BTreeMap<String, (usize, u64, u64)> = Default::default();
        for path in &raws {
            let len = std::fs::metadata(path).unwrap().len();
            let whole = probe_path(path);
            let ends = probe_ends_counted(path);
            let format = path
                .extension()
                .unwrap()
                .to_string_lossy()
                .to_ascii_uppercase();
            match (&whole, &ends) {
                (Ok(whole), Ok((probe, read))) => {
                    assert_eq!(probe, whole, "{}", path.display());
                    println!(
                        "{}: {read} of {len} bytes ({:.1}%)",
                        path.display(),
                        100.0 * *read as f64 / len as f64
                    );
                    let t = totals.entry(format).or_default();
                    *t = (t.0 + 1, t.1 + read, t.2 + len);
                }
                (Err(a), Err(b)) => {
                    assert_eq!(a.to_string(), b.to_string(), "{}", path.display());
                    println!("{}: {a}, both ways", path.display());
                }
                _ => panic!(
                    "{}: {whole:?} whole, {ends:?} from the ends",
                    path.display()
                ),
            }
        }
        let (mut read, mut all) = (0, 0);
        for (format, (n, r, l)) in &totals {
            println!(
                "{format}: {n} files, {:.1} MB read of {:.1} MB ({:.2}%)",
                *r as f64 / 1e6,
                *l as f64 / 1e6,
                100.0 * *r as f64 / *l as f64
            );
            (read, all) = (read + r, all + l);
        }
        println!(
            "all: {} files, {:.1} MB read of {:.1} MB",
            raws.len(),
            read as f64 / 1e6,
            all as f64 / 1e6
        );
    }

    /// What the library's index and the thumbnails read of a raw (its
    /// probe and mosaic, its mosaic alone, its orientation, its preview,
    /// and the quick develop of one with none) gives what the mapped
    /// reads give, and maps nothing: a volume that goes away under a
    /// mapped raw is a SIGBUS nothing catches. A format read whole reads
    /// its length once; ends that will not parse cost the whole file
    /// besides. Our DNG named a CR3 or a RAF has neither one's ends, so
    /// the reads from a CR3's or a RAF's ends are covered by the ignored
    /// test on the samples alone.
    #[test]
    fn the_index_and_thumbnail_reads_map_nothing() {
        let dng = a_dng();
        let dir = std::env::temp_dir().join(format!(
            "greycard-unmapped-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let len = dng.len() as u64;
        for name in ["frame.dng", "frame.CR3", "frame.RAF"] {
            // A folder a file, each watched once its mapped reads'
            // answers are in hand.
            let folder = dir.join(name.replace('.', "-"));
            std::fs::create_dir_all(&folder).unwrap();
            let path = folder.join(name);
            std::fs::write(&path, &dng).unwrap();
            let probe = probe_path(&path).unwrap();
            let stance = stance_path(&path).unwrap();
            let frame = RawlerDecoder.decode_path(&path).unwrap();
            watch_maps_under(&folder);

            let whole = mosaic_source(&read_whole(&path).unwrap().0, &path).unwrap();
            let mosaic = whole.map_or(MosaicRead::NotMosaic, MosaicRead::Mosaic);
            assert_eq!(
                probe_and_mosaic_with(&path, cr3_ends).unwrap(),
                (probe.clone(), mosaic, len),
                "{name}"
            );
            let ((p, m, style, _), _) = index_read_counted(&path).unwrap();
            assert_eq!((p, m), (probe.clone(), mosaic), "{name}");
            assert_eq!(
                format!("{style:?}"),
                format!("{:?}", super::super::style::CameraStyle::read(&path)),
                "{name}"
            );
            let short = probe_and_mosaic_with(&path, |_, _, _| Ok(Some((8, 0)))).unwrap();
            assert_eq!(short, (probe.clone(), mosaic, len + 8), "{name}");
            assert_eq!(
                mosaic_with(&path, cr3_ends).unwrap(),
                (whole, len),
                "{name}"
            );
            assert_eq!(
                crate::decode::orientation_path(&path).unwrap(),
                stance.0,
                "{name}"
            );
            let unmapped = crate::decode::stance_path_unmapped(&path).unwrap();
            assert_eq!(
                (unmapped.orientation, unmapped.size, unmapped.mosaic),
                stance,
                "{name}"
            );
            // Our DNG carries no preview; asked for one, the file is
            // read whole once and says so.
            assert_eq!(
                preview_counted(&path, PreviewSize::Largest).unwrap().1,
                len,
                "{name}"
            );
            assert!(preview_path(&path).unwrap().is_none(), "{name}");
            let unmapped = RawlerDecoder.decode_path_unmapped(&path).unwrap();
            assert_eq!(
                (
                    unmapped.width,
                    unmapped.height,
                    unmapped.as_shot_coefficients
                ),
                (frame.width, frame.height, frame.as_shot_coefficients),
                "{name}"
            );
            assert_eq!(maps_under(&folder), 0, "{name} mapped");
            // The decode the editor opens a frame with still maps.
            let _ = RawlerDecoder.decode_path(&path);
            assert_eq!(maps_under(&folder), 1, "{name}");
        }
        // Not a raw by its name: the probe reads nothing.
        let clip = dir.join("clip.mov");
        std::fs::write(&clip, &dng).unwrap();
        assert!(probe_and_mosaic_with(&clip, cr3_ends).is_err());
        assert!(index_read_path(&clip).is_err());
        assert!(mosaic_path(&clip).is_err());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A DNG past the head the hash reads is read in its pieces, its
    /// directory at the end and not its samples, with the answers the
    /// whole file gives; pieces rawler will not read, and a CR3 head on
    /// which rawler's box parser panics, fall back to the whole file
    /// instead.
    #[test]
    fn the_index_reads_a_raws_pieces_and_falls_back_to_the_whole() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-pieces-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dng = a_dng_of(160, 120);
        let len = dng.len() as u64;
        assert!(len > 2 * super::super::sparse::HEAD, "{len}");
        let path = dir.join("frame.dng");
        std::fs::write(&path, &dng).unwrap();
        let (probe, mosaic) =
            probe_and_mosaic_source(&read_whole(&path).unwrap().0, &path).unwrap();
        let whole = (
            probe,
            mosaic.unwrap(),
            format!("{:?}", super::super::style::CameraStyle::read(&path)),
        );
        let ((p, m, style, _), read) = index_read_counted(&path).unwrap();
        assert_eq!((p, m, format!("{style:?}")), whole);
        assert!(read < len / 2, "{read} of {len}");

        // Every compression tag in the pieces turned to one no raw
        // has: rawler finds no raw image in them, and the whole file
        // answers. (A DNG's metadata and its mosaic pick the raw image
        // by the same tags, so the two fail together here; a mosaic
        // failing alone falls through to the same whole read.)
        let unknown: Pieces = |sparse| {
            super::super::sparse::fetch_metadata(sparse)?;
            let compression = [0x03, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00];
            let buf = sparse.buf_mut();
            let mut at = 0;
            while let Some(i) = buf[at..].windows(8).position(|w| w == compression) {
                buf[at + i + 8] = 99;
                at += i + 8;
            }
            assert!(at > 0, "no compression tag");
            Ok(true)
        };
        let ((p, m, style, _), read) = index_read_with(&path, unknown).unwrap();
        assert_eq!((p, m, format!("{style:?}")), whole);
        assert!(read > len, "{read} of {len}");

        // A CR3 whose media header has a version rawler asserts on.
        let boxed = |kind: &[u8], body: &[u8]| {
            let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            b.extend_from_slice(kind);
            b.extend_from_slice(body);
            b
        };
        let mdhd = boxed(b"mdhd", &[[2, 0, 0, 0].as_slice(), &[0; 20]].concat());
        let moov = boxed(b"moov", &boxed(b"trak", &boxed(b"mdia", &mdhd)));
        let mut cr3 = boxed(b"ftyp", b"crx \0\0\0\x01crx isom");
        cr3.extend_from_slice(&moov);
        cr3.extend_from_slice(&boxed(b"mdat", &vec![7; 200_000]));
        let path = dir.join("frame.CR3");
        std::fs::write(&path, &cr3).unwrap();
        assert!(index_read_path(&path).is_err());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A RAF's preview is read through its embedded JPEG's end, by the
    /// offset and the length its header gives; a length that ends
    /// before the EXIF the probe wants, or past the file, or a file
    /// that is no RAF, has none.
    #[test]
    fn a_rafs_preview_ends_where_its_jpeg_does() {
        use std::io::Cursor;
        let raf = |magic: &[u8], jpeg_len: u32| {
            let mut bytes = vec![0u8; 148];
            bytes[..magic.len()].copy_from_slice(magic);
            bytes[84..88].copy_from_slice(&148u32.to_be_bytes());
            bytes[88..92].copy_from_slice(&jpeg_len.to_be_bytes());
            bytes.extend_from_slice(&[0xff, 0xd8, 0xff, 0xe1, 0x10, 0x00]);
            bytes.resize(1 << 20, 0);
            bytes
        };
        let end = |bytes: &[u8]| raf_jpeg_end(&mut Cursor::new(bytes), bytes.len() as u64).unwrap();
        assert_eq!(end(&raf(b"FUJIFILMCCD-RAW 0201", 300_000)), Some(300_148));
        // The EXIF segment alone runs to 148 + 4 + 0x1000.
        assert_eq!(end(&raf(b"FUJIFILMCCD-RAW 0201", 0x1000)), None);
        assert_eq!(end(&raf(b"FUJIFILMCCD-RAW 0201", 1 << 20)), None);
        assert_eq!(end(&raf(b"FUJIFILMCCD-RAX 0201", 300_000)), None);
    }

    /// With `GREYCARD_TEST_RAWS` a folder of raws: what the index and
    /// the thumbnails read of each raw against the same read of the
    /// whole file, required to agree (the preview pixel for pixel), and
    /// the bytes printed by format. Ignored, since it wants the samples.
    #[test]
    #[ignore]
    fn the_index_and_thumbnail_reads_agree_on_the_samples() {
        let Some(dir) = std::env::var_os("GREYCARD_TEST_RAWS") else {
            eprintln!("GREYCARD_TEST_RAWS names no folder; nothing measured");
            return;
        };
        let mut raws = Vec::new();
        let mut folders = vec![std::path::PathBuf::from(dir)];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                let path = entry.path();
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    folders.push(path);
                } else if crate::decode::is_raw_path(&path) {
                    raws.push(path);
                }
            }
        }
        raws.sort();
        assert!(!raws.is_empty(), "no raws under GREYCARD_TEST_RAWS");
        // Files, their bytes, and what the probe and the mosaic, and
        // the preview, read of them.
        let mut totals: std::collections::BTreeMap<String, (usize, u64, u64, u64, u64)> =
            Default::default();
        let mut framed = 0usize;
        for path in &raws {
            let len = std::fs::metadata(path).unwrap().len();
            let format = path
                .extension()
                .unwrap()
                .to_string_lossy()
                .to_ascii_uppercase();
            let whole = || read_whole(path).unwrap().0;
            let apart = guarded(|| None, || probe_and_mosaic_source(&whole(), path));
            let together = probe_and_mosaic_with(path, cr3_ends);
            let read = match (apart, together) {
                (Ok((probe, mosaic)), Ok((p, m, read))) => {
                    assert_eq!(p, probe, "{}", path.display());
                    assert_eq!(
                        m,
                        mosaic.unwrap_or(MosaicRead::Unknown),
                        "{}",
                        path.display()
                    );
                    let alone = match m {
                        MosaicRead::Mosaic(k) => Some(Some(k)),
                        MosaicRead::NotMosaic => Some(None),
                        MosaicRead::Unknown => None,
                    };
                    assert_eq!(mosaic_path(path).ok(), alone, "{}", path.display());
                    assert_eq!(
                        crate::decode::orientation_path(path).unwrap(),
                        stance_path(path).unwrap().0,
                        "{}",
                        path.display()
                    );
                    read
                }
                (Err(a), Err(b)) => {
                    assert_eq!(a.to_string(), b.to_string(), "{}", path.display());
                    assert_eq!(
                        index_read_counted(path).unwrap_err().to_string(),
                        a.to_string(),
                        "{}",
                        path.display()
                    );
                    continue;
                }
                (a, b) => panic!("{}: {a:?} whole, {b:?} from the ends", path.display()),
            };
            // The index's read: the probe and the mosaic as the whole
            // file gives them, and the style as the file read whole.
            let ((probe, mosaic, style, frame), index_read) = index_read_counted(path).unwrap();
            let (whole_probe, whole_mosaic) = probe_and_mosaic_source(&whole(), path).unwrap();
            assert_eq!(probe, whole_probe, "{}", path.display());
            assert_eq!(
                mosaic,
                whole_mosaic.unwrap_or(MosaicRead::Unknown),
                "{}",
                path.display()
            );
            assert_eq!(
                format!("{style:?}"),
                format!("{:?}", super::super::style::CameraStyle::read(path)),
                "{}",
                path.display()
            );
            // The camera's frame read from the same pieces after the
            // style as read alone: a folder with a frame shot at a
            // ratio says so (the count below), so this compares a frame
            // and not two nothings.
            assert_eq!(
                frame,
                super::super::camera_frame_path(path).ok().flatten(),
                "{}",
                path.display()
            );
            if frame.is_some() {
                framed += 1;
            }
            // The pieces alone, the blocks not read filled with 0x00 and
            // then 0xFF: rawler's metadata the same both ways, so it
            // looked at no byte it was not given.
            let file = std::fs::File::open(path).unwrap();
            let mut sparse = super::super::sparse::Sparse::new(file, len).unwrap();
            let _ = super::super::style::read_stream(&mut sparse);
            if super::super::sparse::fetch_metadata(&mut sparse).unwrap() {
                for byte in [0x00, 0xff] {
                    let source =
                        RawSource::new_from_shared_vec(std::sync::Arc::new(sparse.filled(byte)));
                    let filled = guarded(|| None, || probe_source(&source));
                    assert_eq!(
                        filled.ok().as_ref(),
                        Some(&whole_probe),
                        "{} filled with {byte:#x}",
                        path.display()
                    );
                }
            }
            let (preview, preview_read) = preview_counted(path, PreviewSize::Largest).unwrap();
            let expected = guarded(
                || None,
                || preview_source(&whole(), true, PreviewSize::Largest),
            )
            .unwrap();
            assert_eq!(
                preview.as_ref().map(|(i, o)| (i.dimensions(), *o)),
                expected.as_ref().map(|(i, o)| (i.dimensions(), *o)),
                "{}",
                path.display()
            );
            assert!(
                preview.map(|(i, _)| i.into_raw()) == expected.map(|(i, _)| i.into_raw()),
                "{}: the preview's pixels",
                path.display()
            );
            println!(
                "{}: probe and mosaic {read}, the index's read {index_read}, \
                 preview {preview_read} of {len} bytes",
                path.display()
            );
            let t = totals.entry(format).or_default();
            *t = (
                t.0 + 1,
                t.1 + len,
                t.2 + read,
                t.3 + preview_read,
                t.4 + index_read,
            );
        }
        for (format, (n, l, r, p, i)) in &totals {
            println!(
                "{format}: {n} files of {:.1} MB; probe and mosaic {:.1} MB ({:.2}%), \
                 the index's read {:.2} MB ({:.2}%), preview {:.1} MB ({:.2}%)",
                *l as f64 / 1e6,
                *r as f64 / 1e6,
                100.0 * *r as f64 / *l as f64,
                *i as f64 / 1e6,
                100.0 * *i as f64 / *l as f64,
                *p as f64 / 1e6,
                100.0 * *p as f64 / *l as f64
            );
        }
        println!("{framed} of {} framed by the camera", raws.len());
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
    /// The body's other names; see [`RawFrame::other_names`].
    pub other_names: Vec<String>,
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
        let (make, model) = (metadata.make.trim(), metadata.model.trim());
        Probe {
            make: make.to_string(),
            model: model.to_string(),
            other_names: other_names(make, model),
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
/// The orientation tag, the size and the kind of mosaic.
pub type RawStance = (Orientation, Option<(u32, u32)>, Option<Mosaic>);

pub fn stance_path(path: &Path) -> Result<RawStance> {
    guarded(|| half_copy_at(path), || stance_unguarded(path))
}

fn stance_unguarded(path: &Path) -> Result<RawStance> {
    let source = mapped(path)?;
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
    let image = decoder.raw_image(&source, &params, true).ok();
    let size = image.as_ref().and_then(size_of);
    let mosaic = image.as_ref().and_then(mosaic_of);
    Ok((orientation, size, mosaic))
}

/// What a frame's camera recorded of the light, and the calibrations a
/// profile is made from: a [`RawFrame`]'s `as_shot_coefficients` and
/// `calibrations`, without its samples.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CameraWhite {
    pub as_shot_coefficients: Option<[f64; 3]>,
    pub calibrations: Vec<Calibration>,
    /// A monochrome sensor's, whose calibrations are none.
    pub monochrome: bool,
}

impl CameraWhite {
    /// The profile its white is read through: the calibrations', or a
    /// monochrome sensor's own ([`crate::color::monochrome_profile`]),
    /// as [`crate::color::profile_from_frame`] gives the decoded frame.
    pub fn profile(&self) -> Result<rawcolor::CameraProfile> {
        if self.monochrome {
            return Ok(crate::color::monochrome_profile());
        }
        crate::color::profile_from_calibrations(&self.calibrations)
    }
}

/// The camera's as-shot white and its calibrations, from rawler's
/// dummy image as [`stance_path`] reads the crop: what a frame that is
/// not open needs for the kelvin its As shot means. The same answer
/// [`frame_from_rawler`] gives the decoded frame.
pub fn white_path(path: &Path) -> Result<CameraWhite> {
    guarded(|| half_copy_at(path), || white_unguarded(path))
}

fn white_unguarded(path: &Path) -> Result<CameraWhite> {
    let source = mapped(path)?;
    let decoder = ::rawler::get_decoder(&source).map_err(decoder_error)?;
    let image = decoder
        .raw_image(&source, &RawDecodeParams::default(), true)
        .map_err(decoder_error)?;
    let mut calibrations = calibrations_of(&image);
    with_forward_matrices(&mut calibrations, &forward_matrices(decoder.as_ref()));
    Ok(CameraWhite {
        as_shot_coefficients: if is_monochrome(&image) {
            as_shot_of(&image)
        } else {
            white_xy_gains(decoder.as_ref(), &calibrations).or(as_shot(image.wb_coeffs))
        },
        calibrations,
        monochrome: is_monochrome(&image),
    })
}

/// The kind of mosaic in an image's description, which rawler's dummy
/// read fills in without a sample; `None` for a raw that is not a
/// mosaic.
fn mosaic_of(image: &RawImage) -> Option<Mosaic> {
    match &image.photometric {
        RawPhotometricInterpretation::Cfa(config) => {
            cfa_pattern(&config.cfa).ok().map(|p| p.mosaic())
        }
        _ => None,
    }
}

/// What [`index_read_path`] says of a raw's mosaic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MosaicRead {
    /// A mosaic of this kind.
    Mosaic(Mosaic),
    /// A raw that is not a mosaic (a linear DNG, a monochrome sensor).
    NotMosaic,
    /// The image's description would not read (a format the build's
    /// decoder takes the metadata of and not the image, a damaged
    /// file); the EXIF stands all the same.
    Unknown,
}

/// [`probe_path`] and the kind of mosaic, on one decoder, with the
/// ends of the file to read given, and the bytes read: the metadata
/// read and then the image's description without a sample, the file
/// opened and its format found once. The library's index reads these
/// and the maker's style from fewer bytes: [`index_read_path`]. A
/// failed metadata read fails the whole, as `probe_path` does; a
/// description that will not read costs only the mosaic
/// ([`MosaicRead::Unknown`]), so a raw keeps its camera, ISO, date and
/// lens in the index whatever its image says.
///
/// Never through a memory map, since the index walks shares: a CR3 is
/// read from its ends as [`probe_and_stance_counted`] reads it, and any
/// other format whole into memory. Ends that give no mosaic, or an
/// error, or a panic, fall back to the whole file. An error for a file
/// whose name is not a raw's, before a byte of it is read.
fn probe_and_mosaic_with(path: &Path, ends: Ends) -> Result<(Probe, MosaicRead, u64)> {
    not_raw(path)?;
    let mut tried = 0;
    match read_ends(path, ends) {
        Ok(Some((buf, read))) => {
            let source = RawSource::new_from_shared_vec(std::sync::Arc::new(buf)).with_path(path);
            match guarded(|| None, || probe_and_mosaic_source(&source, path)) {
                Ok((probe, Ok(m))) => return Ok((probe, m, read)),
                Ok((_, Err(e))) => {
                    log::debug!(
                        "{}: no mosaic from its ends: {e}; read whole",
                        path.display()
                    )
                }
                Err(e) => log::debug!("{}: from its ends: {e}; read whole", path.display()),
            }
            tried = read;
        }
        Ok(None) => {}
        Err(e) => log::debug!("{}: its ends: {e}; read whole", path.display()),
    }
    guarded(
        || half_copy_at(path),
        || {
            let (source, whole) = read_whole(path)?;
            let (probe, described) = probe_and_mosaic_source(&source, path)?;
            let mosaic = described.unwrap_or_else(|e| {
                log::warn!("{}: its mosaic: {e}", path.display());
                MosaicRead::Unknown
            });
            Ok((probe, mosaic, tried + whole))
        },
    )
}

/// What the library's index reads of a raw: its probe, its kind of
/// mosaic, the maker's style ([`CameraStyle::read`]), the style's failure
/// apart, since the probe beside it stands, and the camera's frame
/// (`decode::camera_frame_path`), none where it will not read.
pub type IndexRead = (
    Probe,
    MosaicRead,
    Result<Option<super::style::CameraStyle>>,
    Option<crate::raw::CameraFrame>,
);

/// [`IndexRead`] from the pieces of the file its metadata is in (see
/// `decode::sparse`): the style read from the file's own bytes as it
/// asks for them, and rawler given the file's length in zeros with the
/// metadata's pieces in place. The same answers as the whole file
/// gives, a fraction of the bytes: on the samples about 200 KB a CR3,
/// where its ends were 1.5 MB, under 300 KB a NEF, ARW, RAF, DNG, or
/// RW2, which were read whole, and 1.1 MB an ORF, whose maker note
/// holds a preview. Any error, or panic, or mosaic that will not read
/// from the pieces falls back to the whole file. Never through a
/// memory map.
pub fn index_read_path(path: &Path) -> Result<IndexRead> {
    index_read_counted(path).map(|(read, _)| read)
}

/// [`index_read_path`] and the bytes it read.
fn index_read_counted(path: &Path) -> Result<(IndexRead, u64)> {
    index_read_with(path, super::sparse::fetch_metadata)
}

/// How [`index_read_counted`] reads a file's pieces into place.
type Pieces = fn(&mut super::sparse::Sparse<std::fs::File>) -> std::io::Result<bool>;

/// [`index_read_counted`] with the pieces read by `fetch`.
fn index_read_with(path: &Path, fetch: Pieces) -> Result<(IndexRead, u64)> {
    not_raw(path)?;
    let file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let Some(mut sparse) = super::sparse::Sparse::new(file, len) else {
        let (probe, mosaic, read) = probe_and_mosaic_with(path, cr3_ends)?;
        let frame = super::camera_frame_path(path).ok().flatten();
        return Ok((
            (probe, mosaic, super::style::CameraStyle::read(path), frame),
            read,
        ));
    };
    // The head first, in one read: the style and the pieces both start
    // there.
    let _ = sparse.fetch(0, super::sparse::HEAD);
    let style = guarded(
        || half_copy_at(path),
        || super::style::read_stream(&mut sparse),
    );
    let frame = guarded(
        || half_copy_at(path),
        || super::style::frame_stream(&mut sparse),
    )
    .unwrap_or_else(|e| {
        log::debug!("{}: the camera's frame: {e}", path.display());
        None
    });
    // rawler's box parser, which the CR3's walk runs, panics on some
    // damaged heads; that is a file read whole, as an error is.
    let pieces = match guarded(|| None, || Ok(fetch(&mut sparse)?)) {
        Ok(known) => known,
        Err(e) => {
            log::debug!("{}: its pieces: {e}; read whole", path.display());
            false
        }
    };
    let (buf, tried) = sparse.into_parts();
    let source = RawSource::new_from_shared_vec(std::sync::Arc::new(buf)).with_path(path);
    // As the decode takes the frame: not where the decoder says it
    // develops another size than the note's valid area
    // (`decode::camera_frame_path` says why).
    let frame = frame.filter(|f| {
        let size = guarded(|| None, || probe_and_stance_source(&source, path))
            .ok()
            .and_then(|(_, stance)| stance.1);
        f.develops_at(size)
    });
    // A file small enough that its pieces were all of it is whole.
    let whole = tried >= len;
    if pieces && !whole {
        match guarded(|| None, || probe_and_mosaic_source(&source, path)) {
            Ok((probe, Ok(m))) => return Ok(((probe, m, style, frame), tried)),
            // Told by the compression tag, which the pieces hold: the
            // whole file would end the same way.
            Ok((probe, Err(e))) if e.contains(NIKON_HE) => {
                log::warn!("{}: its mosaic: {e}", path.display());
                return Ok(((probe, MosaicRead::Unknown, style, frame), tried));
            }
            Ok((_, Err(e))) => {
                log::debug!(
                    "{}: no mosaic from its pieces: {e}; read whole",
                    path.display()
                )
            }
            Err(e) => log::debug!("{}: from its pieces: {e}; read whole", path.display()),
        }
    }
    guarded(
        || half_copy_at(path),
        || {
            let (source, read) = if whole {
                (source, 0)
            } else {
                read_whole(path)?
            };
            let (probe, described) = probe_and_mosaic_source(&source, path)?;
            let mosaic = described.unwrap_or_else(|e| {
                log::warn!("{}: its mosaic: {e}", path.display());
                MosaicRead::Unknown
            });
            Ok(((probe, mosaic, style, frame), tried + read))
        },
    )
}

/// The probe and the mosaic from one decoder on `source`, the mosaic's
/// failure or panic said apart, since the probe beside it stands.
fn probe_and_mosaic_source(
    source: &RawSource,
    path: &Path,
) -> Result<(Probe, std::result::Result<MosaicRead, String>)> {
    let decoder = ::rawler::get_decoder(source).map_err(decoder_error)?;
    let params = RawDecodeParams::default();
    let metadata = metadata_of(decoder.as_ref(), source, &params).map_err(decoder_error)?;
    let probe = Probe::from_metadata(&metadata);
    let mosaic = described(|| {
        if is_dng(path) {
            dng_mosaic(source)
        } else {
            decoder
                .raw_image(source, &params, true)
                .map(|image| mosaic_of(&image))
                .map_err(decoder_error)
        }
    })
    .map(|m| m.map_or(MosaicRead::NotMosaic, MosaicRead::Mosaic));
    Ok((probe, mosaic))
}

/// What a read of the image's description says, its failure and its
/// panic both an error's words.
fn described(
    read: impl FnOnce() -> Result<Option<Mosaic>>,
) -> std::result::Result<Option<Mosaic>, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(read)) {
        Ok(Ok(m)) => Ok(m),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("the decoder panicked".into()),
    }
}

fn is_dng(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
}

/// A DNG's kind of mosaic from its raw IFD's tags. rawler's dummy read
/// of a DNG decodes every sample (its DNG decoder reads the image data
/// whatever the flag), 20 ms and more a file, where the tags are a
/// millisecond. The raw IFD is the one rawler takes: compressed as a
/// raw is and not a reduced copy.
fn dng_mosaic(source: &RawSource) -> Result<Option<Mosaic>> {
    use ::rawler::formats::tiff::reader::{GenericTiffReader, TiffReader};
    use ::rawler::tags::TiffCommonTag;
    let tiff = GenericTiffReader::new(&mut source.reader(), 0, 0, None, &[])
        .map_err(|e| Error::Decode(e.to_string()))?;
    let raw = tiff
        .find_ifds_with_tag(TiffCommonTag::Compression)
        .into_iter()
        .find(|ifd| {
            let reduced = ifd
                .get_entry(TiffCommonTag::NewSubFileType)
                .is_some_and(|e| e.force_u32(0) & 1 != 0);
            let compression = ifd
                .get_entry(TiffCommonTag::Compression)
                .map_or(0, |e| e.force_u32(0));
            !reduced && matches!(compression, 1 | 7 | 8 | 0x884c | 52546)
        })
        .ok_or_else(|| Error::Decode("DNG with no raw image".into()))?;
    let photometric = raw
        .get_entry(TiffCommonTag::PhotometricInt)
        .map_or(0, |e| e.force_u32(0));
    if photometric != 32803 {
        return Ok(None);
    }
    // Every value read through `get_u32`, which says nothing of an
    // index past the end, where rawler's `force_*` log a backtrace.
    let values = |e: Option<&::rawler::formats::tiff::Entry>| -> Option<Vec<u32>> {
        let e = e?;
        (0..e.value.count())
            .map(|i| e.value.get_u32(i).ok().flatten())
            .collect()
    };
    let dims = raw.get_entry(TiffCommonTag::CFARepeatPatternDim);
    let pattern = raw.get_entry(TiffCommonTag::CFAPattern);
    // Counted before a value is read: a file's word for how many
    // values there are is not taken on trust.
    let (Some(dims), Some(pattern)) = (dims, pattern) else {
        return Ok(Some(Mosaic::Other));
    };
    if dims.value.count() != 2 || pattern.value.count() > MAX_CFA * MAX_CFA {
        return Ok(Some(Mosaic::Other));
    }
    let planes = raw
        .get_entry(::rawler::tags::DngTag::CFAPlaneColor)
        .filter(|e| e.value.count() <= 16);
    Ok(Some(mosaic_from_tags(
        &values(Some(dims)).unwrap_or_default(),
        &values(Some(pattern)).unwrap_or_default(),
        values(planes).as_deref(),
    )))
}

/// The largest repeat a DNG's CFA is taken to have either way; X-Trans
/// is 6, and DNG's own limit for a pattern is 16 a side, past which a
/// file is damaged or not a mosaic this engine reads.
const MAX_CFA: usize = 16;

/// The kind of mosaic a DNG's tags describe: `dims` the
/// `CFARepeatPatternDim` (rows, columns), `pattern` the `CFAPattern`
/// row by row, `planes` the `CFAPlaneColor` when the file has one,
/// mapping each value of the pattern to a color (0 red, 1 green, 2
/// blue), the order of the planes otherwise. Anything out of shape,
/// a repeat of nothing or past [`MAX_CFA`] a side, fewer values than
/// the repeat, a color past the planes, is [`Mosaic::Other`].
fn mosaic_from_tags(dims: &[u32], pattern: &[u32], planes: Option<&[u32]>) -> Mosaic {
    let &[rows, cols] = dims else {
        return Mosaic::Other;
    };
    let (rows, cols) = (rows as usize, cols as usize);
    if !(1..=MAX_CFA).contains(&rows) || !(1..=MAX_CFA).contains(&cols) {
        return Mosaic::Other;
    }
    let Some(n) = rows.checked_mul(cols).filter(|&n| pattern.len() >= n) else {
        return Mosaic::Other;
    };
    let color = |v: u32| -> Option<u32> {
        match planes {
            Some(planes) => planes.get(v as usize).copied(),
            None => Some(v),
        }
    };
    let colors: Option<Vec<CfaColor>> = pattern[..n]
        .iter()
        .map(|&v| {
            color(v).map(|c| match c {
                0 => CfaColor::Red,
                1 => CfaColor::Green,
                2 => CfaColor::Blue,
                other => CfaColor::Other(other.min(255) as u8),
            })
        })
        .collect();
    colors
        .and_then(|colors| CfaPattern::new(cols, rows, colors).ok())
        .map_or(Mosaic::Other, |p| p.mosaic())
}

/// The kind of mosaic alone: the image's description without a sample
/// and without the metadata read. `Ok(None)` for a raw that is not a
/// mosaic. Never through a memory map: a CR3 from its ends, any other format whole, and ends that
/// give no mosaic, or an error, fall back to the whole file.
pub fn mosaic_path(path: &Path) -> Result<Option<Mosaic>> {
    mosaic_with(path, cr3_ends).map(|(mosaic, _)| mosaic)
}

/// [`mosaic_path`] with the ends of the file to read given, and the
/// bytes read.
fn mosaic_with(path: &Path, ends: Ends) -> Result<(Option<Mosaic>, u64)> {
    not_raw(path)?;
    let mut tried = 0;
    match read_ends(path, ends) {
        Ok(Some((buf, read))) => {
            let source = RawSource::new_from_shared_vec(std::sync::Arc::new(buf)).with_path(path);
            match guarded(|| None, || mosaic_source(&source, path)) {
                Ok(Some(m)) => return Ok((Some(m), read)),
                Ok(None) => log::debug!("{}: no mosaic from its ends; read whole", path.display()),
                Err(e) => log::debug!(
                    "{}: its mosaic from its ends: {e}; read whole",
                    path.display()
                ),
            }
            tried = read;
        }
        Ok(None) => {}
        Err(e) => log::debug!("{}: its ends: {e}; read whole", path.display()),
    }
    guarded(
        || half_copy_at(path),
        || {
            let (source, whole) = read_whole(path)?;
            mosaic_source(&source, path).map(|m| (m, tried + whole))
        },
    )
}

/// The kind of mosaic from one source: a DNG's from its tags (see
/// [`dng_mosaic`]), any other raw's from rawler's dummy image.
fn mosaic_source(source: &RawSource, path: &Path) -> Result<Option<Mosaic>> {
    if is_dng(path) {
        return dng_mosaic(source);
    }
    let decoder = ::rawler::get_decoder(source).map_err(decoder_error)?;
    let image = decoder
        .raw_image(source, &RawDecodeParams::default(), true)
        .map_err(decoder_error)?;
    Ok(mosaic_of(&image))
}

/// The file at `path` as rawler's source, memory-mapped: what a decode
/// and the probes that read a whole file take, for the speed of a
/// mapping. A mapped file whose volume goes away (a share that drops,
/// a disk asleep) faults on its next page as a SIGBUS no Rust code
/// catches, so a read that walks many files on such a volume, as the
/// catalog import, the library's index, and the thumbnails do, takes
/// [`read_whole`] or the ends instead.
fn mapped(path: &Path) -> Result<RawSource> {
    let mut watched = WATCHED.lock().unwrap_or_else(|e| e.into_inner());
    for (dir, n) in watched.iter_mut() {
        if path.starts_with(dir) {
            *n += 1;
        }
    }
    drop(watched);
    Ok(RawSource::new(path)?)
}

/// The folders whose files' mappings are counted, and the counts: on
/// every thread, and apart from the files of a test running beside.
static WATCHED: std::sync::Mutex<Vec<(std::path::PathBuf, u64)>> =
    std::sync::Mutex::new(Vec::new());

/// Count from now every raw under `dir` memory-mapped for rawler, on
/// any thread: for a test that a path never maps one.
pub fn watch_maps_under(dir: &Path) {
    let mut watched = WATCHED.lock().unwrap_or_else(|e| e.into_inner());
    if !watched.iter().any(|(d, _)| d == dir) {
        watched.push((dir.to_path_buf(), 0));
    }
}

/// How many raws under `dir` were mapped since [`watch_maps_under`].
pub fn maps_under(dir: &Path) -> u64 {
    let watched = WATCHED.lock().unwrap_or_else(|e| e.into_inner());
    watched
        .iter()
        .find(|(d, _)| d == dir)
        .map_or(0, |(_, n)| *n)
}

/// The whole file at `path` read into memory, not mapped, as rawler's
/// source, and its length: a volume that goes away under it is an
/// error from `read`, not a SIGBUS.
fn read_whole(path: &Path) -> Result<(RawSource, u64)> {
    let buf = std::fs::read(path)?;
    let len = buf.len() as u64;
    Ok((
        RawSource::new_from_shared_vec(std::sync::Arc::new(buf)).with_path(path),
        len,
    ))
}

pub fn probe_path(path: &Path) -> Result<Probe> {
    guarded(|| half_copy_at(path), || probe_unguarded(path))
}

fn probe_unguarded(path: &Path) -> Result<Probe> {
    probe_source(&mapped(path)?)
}

fn probe_source(source: &RawSource) -> Result<Probe> {
    let decoder = ::rawler::get_decoder(source).map_err(decoder_error)?;
    let metadata = metadata_of(decoder.as_ref(), source, &RawDecodeParams::default())
        .map_err(decoder_error)?;
    Ok(Probe::from_metadata(&metadata))
}

/// [`probe_path`] from the file's two ends, not the whole of it.
///
/// rawler's `RawSource::new` maps a file with `populate()` and advises
/// `WillNeed`, so `probe_path` reads every byte of it: 27 MB for a CR3
/// whose metadata sits in a few hundred KB at its head and a record
/// near its end, which over a share is a quarter of a second a frame
/// on gigabit and more over Wi-Fi. Here the head and the tail are read
/// into a buffer of the file's length, the gap left as zeros (which the
/// allocator hands out as pages nobody has touched), and rawler is given
/// that. What each format reads, against what the sample set's files
/// needed for the whole file's probe:
///
/// | Format | Head | Tail | The samples needed |
/// |---|---|---|---|
/// | CR3 | through the `mdat` box's header (200 KB to 1 MB) | 1 MiB | that head and 115 to 120 KiB of tail |
/// | RAF | through the embedded JPEG's EXIF segment (64 KiB) | none | 4 to 5 KiB of head |
/// | NEF, ARW, RW2, DNG, the rest | the whole file | | |
///
/// A head ends where the format's own structure says the metadata
/// does, never at a size cut through it: rawler reading a structure the
/// gap has cut short does not always fail. On a DNG whose directory
/// sits at its end and points into the middle, a gap of zeros has it
/// ask for 3.6 GB at once, and an allocation that fails is an abort no
/// guard catches. A CR3's head is its boxes up to the samples and a
/// RAF's the header and the JPEG's EXIF; what the gap leaves rawler
/// past them reads as an empty directory (the RAF's own tags and its
/// sensor's directory) or a record of size nought, on which rawler
/// panics (the CR3's CTMD, should it lie further from the end than the
/// tail). The TIFF formats would want their directories walked for a
/// head of the same kind; the samples' NEFs needed 255 to 272 KiB, the
/// ARW 135 KiB, the RW2 5 KiB, and the DNGs every byte.
///
/// Any error, or a panic, from the ends falls back to the whole file,
/// so a file this does not suit costs a little more than `probe_path`
/// alone and never gives another answer.
pub fn probe_ends_path(path: &Path) -> Result<Probe> {
    probe_ends_counted(path).map(|(probe, _)| probe)
}

/// [`probe_ends_path`] and how many bytes of the file it read.
fn probe_ends_counted(path: &Path) -> Result<(Probe, u64)> {
    probe_ends_with(path, ends_of)
}

/// [`probe_ends_counted`] with the ends of the file to read given.
fn probe_ends_with(path: &Path, ends: Ends) -> Result<(Probe, u64)> {
    not_raw(path)?;
    let mut tried = 0;
    match read_ends(path, ends) {
        Ok(Some((buf, read))) => {
            let source = RawSource::new_from_shared_vec(std::sync::Arc::new(buf)).with_path(path);
            match guarded(|| None, || probe_source(&source)) {
                Ok(probe) => return Ok((probe, read)),
                Err(e) => log::debug!("{}: from its ends: {e}; read whole", path.display()),
            }
            tried = read;
        }
        Ok(None) => {}
        Err(e) => log::debug!("{}: its ends: {e}; read whole", path.display()),
    }
    guarded(
        || half_copy_at(path),
        || {
            let (source, whole) = read_whole(path)?;
            probe_source(&source).map(|probe| (probe, tried + whole))
        },
    )
}

/// An error for a file whose name is not a raw's, before a byte of it
/// is read: a catalog's clips and layered files are no raws, and one
/// of some gigabytes read whole to find so would be a long way round.
fn not_raw(path: &Path) -> Result<()> {
    if super::is_raw_path(path) {
        Ok(())
    } else {
        Err(Error::Decode(format!(
            "{}: not a raw by its name",
            path.display()
        )))
    }
}

/// A CR3's tail: the CTMD record rawler reads sits near the end of the
/// `mdat` box, 115 to 120 KiB from the end on the samples.
const CR3_TAIL: u64 = 1 << 20;

/// How much of each end of the file at a path, open and `len` bytes
/// long, to read: the head and the tail; `None` to read it whole.
type Ends = fn(&Path, &mut std::fs::File, u64) -> std::io::Result<Option<(u64, u64)>>;

/// The head and the tail of `path` in a buffer of its length, the gap
/// zeroed, and the bytes read; `None` for a format read whole, or a
/// file whose ends would be the whole of it anyway.
fn read_ends(path: &Path, ends: Ends) -> std::io::Result<Option<(Vec<u8>, u64)>> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let Some((head, tail)) = ends(path, &mut file, len)? else {
        return Ok(None);
    };
    read_ends_of(&mut file, len, head, tail)
}

/// [`probe_path`] and [`stance_path`] in one read of the file, from its
/// two ends where the format allows, and the bytes read.
///
/// A CR3's stance is its `moov` box (the image's size in `CMP1`, its
/// crop in `IAD1`) and the CTMD record the probe reads already, so its
/// ends give the stance as they give the probe. A RAF's size is in its
/// own directory past the JPEG, which its head stops short of, and the
/// TIFF formats are read whole for the probe anyway: those are read
/// whole, once, for both. A DNG's dummy image decodes its samples (see
/// [`dng_mosaic`]), as [`stance_path`]'s does; that costs time, not
/// bytes.
///
/// An image that will not describe itself leaves the stance's size and
/// mosaic `None` and the probe standing, as the two calls apart would
/// answer. Any error, panic, or stance without a size from the ends
/// falls back to the whole file.
pub fn probe_and_stance_counted(path: &Path) -> Result<(Probe, RawStance, u64)> {
    probe_and_stance_with(path, cr3_ends)
}

/// [`probe_and_stance_counted`] with the ends of the file to read given.
fn probe_and_stance_with(path: &Path, ends: Ends) -> Result<(Probe, RawStance, u64)> {
    not_raw(path)?;
    let mut tried = 0;
    match read_ends(path, ends) {
        Ok(Some((buf, read))) => {
            let source = RawSource::new_from_shared_vec(std::sync::Arc::new(buf)).with_path(path);
            match guarded(|| None, || probe_and_stance_source(&source, path)) {
                Ok((probe, stance)) if stance.1.is_some() => return Ok((probe, stance, read)),
                Ok(_) => log::debug!("{}: no size from its ends; read whole", path.display()),
                Err(e) => log::debug!("{}: from its ends: {e}; read whole", path.display()),
            }
            tried = read;
        }
        Ok(None) => {}
        Err(e) => log::debug!("{}: its ends: {e}; read whole", path.display()),
    }
    guarded(
        || half_copy_at(path),
        || {
            let (source, whole) = read_whole(path)?;
            probe_and_stance_source(&source, path)
                .map(|(probe, stance)| (probe, stance, tried + whole))
        },
    )
}

/// A CR3's ends as [`probe_ends_path`] reads them; every other format
/// whole. See [`probe_and_stance_counted`].
fn cr3_ends(
    path: &Path,
    file: &mut std::fs::File,
    len: u64,
) -> std::io::Result<Option<(u64, u64)>> {
    let cr3 = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cr3"));
    Ok(if cr3 {
        cr3_head(file, len)?.map(|head| (head, CR3_TAIL))
    } else {
        None
    })
}

/// The probe and the stance from one decoder on `source`: the metadata
/// read, then the image's description without a sample. A description
/// that fails or panics costs the stance's size and mosaic alone.
fn probe_and_stance_source(source: &RawSource, path: &Path) -> Result<(Probe, RawStance)> {
    let decoder = ::rawler::get_decoder(source).map_err(decoder_error)?;
    let params = RawDecodeParams::default();
    let metadata = metadata_of(decoder.as_ref(), source, &params).map_err(decoder_error)?;
    let probe = Probe::from_metadata(&metadata);
    let described = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        decoder.raw_image(source, &params, true)
    }));
    let image = match described {
        Ok(Ok(image)) => Some(image),
        Ok(Err(e)) => {
            log::debug!("{}: its stance: {e}", path.display());
            None
        }
        Err(_) => {
            log::debug!("{}: its stance: the decoder panicked", path.display());
            None
        }
    };
    let size = image.as_ref().and_then(size_of);
    let mosaic = image.as_ref().and_then(mosaic_of);
    let orientation = probe.orientation;
    Ok((probe, (orientation, size, mosaic)))
}

/// The size a develop of `image` comes out at before its tag turns it:
/// its crop, else its active area, else the whole.
fn size_of(image: &RawImage) -> Option<(u32, u32)> {
    Some(match image.crop_area.or(image.active_area) {
        Some(r) => (r.d.w as u32, r.d.h as u32),
        None => (image.width as u32, image.height as u32),
    })
    .filter(|(w, h)| *w > 0 && *h > 0)
}

/// The ends of each format, by the path's extension: see
/// [`probe_ends_path`].
fn ends_of(path: &Path, file: &mut std::fs::File, len: u64) -> std::io::Result<Option<(u64, u64)>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    Ok(match ext.as_deref() {
        Some("cr3") => cr3_head(file, len)?.map(|head| (head, CR3_TAIL)),
        Some("raf") => raf_head(file, len)?.map(|head| (head, 0)),
        _ => None,
    })
}

/// The ends read: see [`read_ends`].
fn read_ends_of(
    file: &mut (impl std::io::Read + std::io::Seek),
    len: u64,
    head: u64,
    tail: u64,
) -> std::io::Result<Option<(Vec<u8>, u64)>> {
    use std::io::SeekFrom;
    if head.saturating_add(tail) >= len {
        return Ok(None);
    }
    let Ok(size) = usize::try_from(len) else {
        return Ok(None);
    };
    let mut buf = vec![0u8; size];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut buf[..head as usize])?;
    file.seek(SeekFrom::Start(len - tail))?;
    file.read_exact(&mut buf[(len - tail) as usize..])?;
    Ok(Some((buf, head + tail)))
}

/// Where a CR3's `mdat` box's header ends: the top-level boxes walked
/// from the start, each header read and its body skipped. rawler's
/// file box is not finished without that header. `None` for a file
/// that does not start with `ftyp`, or whose boxes run out or do not
/// hold together before an `mdat`.
fn cr3_head(
    file: &mut (impl std::io::Read + std::io::Seek),
    len: u64,
) -> std::io::Result<Option<u64>> {
    use std::io::SeekFrom;
    let mut at = 0u64;
    while at.saturating_add(8) <= len {
        let mut header = [0u8; 8];
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(&mut header)?;
        let size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let kind = &header[4..8];
        if at == 0 && kind != b"ftyp" {
            return Ok(None);
        }
        // A size of 1 says a 64-bit size follows; 0, that the box runs
        // to the end of the file.
        let (size, header_len) = match size {
            1 => {
                if at.saturating_add(16) > len {
                    return Ok(None);
                }
                let mut large = [0u8; 8];
                file.read_exact(&mut large)?;
                (u64::from_be_bytes(large), 16)
            }
            0 => (len - at, 8),
            n => (u64::from(n), 8),
        };
        if kind == b"mdat" {
            return Ok(Some(at + header_len));
        }
        if size < header_len {
            return Ok(None);
        }
        at = at.saturating_add(size);
    }
    Ok(None)
}

/// Where a RAF's metadata ends: the embedded JPEG's EXIF segment, which
/// a Fujifilm camera writes first in the JPEG, its length in its own
/// header. rawler takes the camera, the EXIF and the maker note from
/// it; the RAF's own tags and the sensor's directory, past the JPEG,
/// are not in the probe. `None` for a file not laid out so.
fn raf_head(
    file: &mut (impl std::io::Read + std::io::Seek),
    len: u64,
) -> std::io::Result<Option<u64>> {
    use std::io::SeekFrom;
    // Where the header keeps the JPEG's offset, big-endian.
    const JPEG_AT: usize = 84;
    let mut header = [0u8; JPEG_AT + 4];
    if len < header.len() as u64 {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header)?;
    if !header.starts_with(RAF_MAGIC) {
        return Ok(None);
    }
    let jpeg = be_u32_at(&header, JPEG_AT);
    // The start of the image, then APP1 and its length, which counts
    // its own two bytes.
    let mut start = [0u8; 6];
    if jpeg.saturating_add(6) > len {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(jpeg))?;
    file.read_exact(&mut start)?;
    if start[..4] != [0xff, 0xd8, 0xff, 0xe1] {
        return Ok(None);
    }
    let segment = u64::from(u16::from_be_bytes([start[4], start[5]]));
    Ok(Some(jpeg + 4 + segment).filter(|&head| head <= len))
}
