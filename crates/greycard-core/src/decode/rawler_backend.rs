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
    Calibration, CfaColor, CfaPattern, LevelPattern, Levels, Mosaic, Orientation, RawFrame, Rect,
    Samples, SensorLayout, Shot,
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
/// gave up on it or read it to the end.
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
/// `tail` looks half-copied, and the words to say so in. A RAF is
/// judged by its header and its sensor data: one whose sections run
/// past its end is half-copied, and so is one whose CFA data ends in
/// [`TAIL`] bytes of zeros, as a copier that sets the length first
/// leaves it; one that holds its sections and whose data does not end
/// so is whole whatever follows them (some bodies' files carry a tail
/// of zeros past the CFA data). `data_tail` reads the [`TAIL`] bytes
/// that end at an offset. Every other file, and a RAF whose header
/// cannot be read, is judged by [`ends_in_zeros`].
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

/// Refuse a file that [`half_copied`] names, before its samples are
/// decoded. rawler reads a file whose last [`TAIL`] bytes are zeros
/// without complaint when its format lets it: an ARW, and a CR3 once
/// dnglab/dnglab#851 is released, decode the zeros past the copied
/// part as samples, and the picture comes out blank below it; so does
/// a RAF whose CFA data ends in zeros. The bytes are already in memory
/// (rawler maps the whole file, or was handed it), so this costs no
/// read.
fn refuse_a_half_copy(bytes: &[u8]) -> Result<()> {
    match half_copied_bytes(bytes) {
        Some(why) => Err(Error::Decode(why.into())),
        None => Ok(()),
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
    preview_counted(path).map(|(preview, _)| preview)
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
fn preview_counted(path: &Path) -> Result<(Preview, u64)> {
    let mut tried = 0;
    match read_preview_ends(path) {
        Ok(Some((buf, read))) => {
            let source = RawSource::new_from_shared_vec(std::sync::Arc::new(buf)).with_path(path);
            match guarded(|| None, || preview_source(&source, false)) {
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
            preview_source(&source, true).map(|preview| (preview, tried + whole))
        },
    )
}

/// The preview from one source: the largest preview JPEG, else, when
/// `thumbnail` says, the thumbnail.
fn preview_source(source: &RawSource, thumbnail: bool) -> Result<Preview> {
    let decoder = ::rawler::get_decoder(source).map_err(decoder_error)?;
    let params = RawDecodeParams::default();
    let image = match decoder.preview_image(source, &params) {
        Ok(Some(i)) => Some(i),
        _ if thumbnail => decoder.thumbnail_image(source, &params).ok().flatten(),
        _ => None,
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
    Ok(Some((image.to_rgb8(), orientation)))
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
    refuse_a_half_copy(source.buf())?;
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
        ..
    } = image;
    let samples = match data {
        RawImageData::Integer(v) => Samples::U16(v),
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

/// The image's color matrices as the engine's calibrations, by
/// illuminant.
fn calibrations_of(image: &RawImage) -> Vec<Calibration> {
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
    calibrations
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
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A RAF is judged by its header and its sensor data, not its
    /// tail: one that holds all its sections, its CFA data not ending
    /// in zeros, is whole with a tail of zeros past them; one whose
    /// sections run past its end, or whose CFA data ends in zeros, is
    /// half-copied; one too short for a header falls back to the tail.
    #[test]
    fn a_raf_is_judged_by_its_header() {
        let data = vec![0x33; 3 * TAIL];
        let padded = [data.clone(), vec![0; TAIL]].concat();
        let whole = a_raf(data.len(), &padded);
        assert_eq!(half_copied_bytes(&whole), None);
        assert!(refuse_a_half_copy(&whole).is_ok());

        let past = a_raf(data.len() + 1, &data);
        assert_eq!(half_copied_bytes(&past), Some(RAF_HALF_COPIED));
        match refuse_a_half_copy(&past) {
            Err(Error::Decode(text)) => assert_eq!(text, RAF_HALF_COPIED),
            other => panic!("{other:?}"),
        }

        // Full size, as a copier that sets the length first leaves it,
        // the CFA data's last 64 KiB zeros; one byte of data in them,
        // and it is whole.
        let preallocated = a_raf(padded.len(), &padded);
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

        // Not a RAF: the tail rule.
        let other = [b"II*\0".to_vec(), padded.clone()].concat();
        assert_eq!(half_copied_bytes(&other), Some(HALF_COPIED));
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

    /// A linear DNG of our own writing: a raw rawler reads, which no
    /// sample raw in the repo may stand in for.
    fn a_dng() -> Vec<u8> {
        let frame = crate::develop::tests::flat_frame();
        let (width, height) = (16, 12);
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
        let (white, frame) = (white.unwrap(), frame.unwrap());
        assert!(white.as_shot_coefficients.is_some());
        assert_eq!(white.as_shot_coefficients, frame.as_shot_coefficients);
        assert_eq!(white.calibrations, frame.calibrations);
        assert!(!white.calibrations.is_empty());
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
            assert_eq!(preview_counted(&path).unwrap().1, len, "{name}");
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
        assert!(probe_and_mosaic_path(&clip).is_err());
        assert!(mosaic_path(&clip).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
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
        let mut totals: std::collections::BTreeMap<String, (usize, u64, u64, u64)> =
            Default::default();
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
                    continue;
                }
                (a, b) => panic!("{}: {a:?} whole, {b:?} from the ends", path.display()),
            };
            let (preview, preview_read) = preview_counted(path).unwrap();
            let expected = guarded(|| None, || preview_source(&whole(), true)).unwrap();
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
                "{}: probe and mosaic {read}, preview {preview_read} of {len} bytes",
                path.display()
            );
            let t = totals.entry(format).or_default();
            *t = (t.0 + 1, t.1 + len, t.2 + read, t.3 + preview_read);
        }
        for (format, (n, l, r, p)) in &totals {
            println!(
                "{format}: {n} files of {:.1} MB; probe and mosaic {:.1} MB ({:.2}%), \
                 preview {:.1} MB ({:.2}%)",
                *l as f64 / 1e6,
                *r as f64 / 1e6,
                100.0 * *r as f64 / *l as f64,
                *p as f64 / 1e6,
                100.0 * *p as f64 / *l as f64
            );
        }
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
    Ok(CameraWhite {
        as_shot_coefficients: as_shot(image.wb_coeffs),
        calibrations: calibrations_of(&image),
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

/// What [`probe_and_mosaic_path`] says of a raw's mosaic.
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

/// [`probe_path`] and the kind of mosaic, on one decoder: the metadata
/// read and then the image's description without a sample, the file
/// opened and its format found once. What the library's index reads.
/// A failed metadata read fails the whole, as `probe_path` does; a
/// description that will not read costs only the mosaic
/// ([`MosaicRead::Unknown`]), so a raw keeps its camera, ISO, date and
/// lens in the index whatever its image says.
///
/// Never through a memory map, since the index walks shares: a CR3 is
/// read from its ends as [`probe_and_stance_counted`] reads it, and any
/// other format whole into memory. Ends that give no mosaic, or an
/// error, or a panic, fall back to the whole file. An error for a file
/// whose name is not a raw's, before a byte of it is read.
pub fn probe_and_mosaic_path(path: &Path) -> Result<(Probe, MosaicRead)> {
    probe_and_mosaic_with(path, cr3_ends).map(|(probe, mosaic, _)| (probe, mosaic))
}

/// [`probe_and_mosaic_path`] with the ends of the file to read given,
/// and the bytes read.
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

/// The probe and the mosaic from one decoder on `source`, the mosaic's
/// failure or panic said apart, since the probe beside it stands.
fn probe_and_mosaic_source(
    source: &RawSource,
    path: &Path,
) -> Result<(Probe, std::result::Result<MosaicRead, String>)> {
    let decoder = ::rawler::get_decoder(source).map_err(decoder_error)?;
    let params = RawDecodeParams::default();
    let metadata = decoder
        .raw_metadata(source, &params)
        .map_err(decoder_error)?;
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
/// mosaic. Never through a memory map, as [`probe_and_mosaic_path`]
/// reads: a CR3 from its ends, any other format whole, and ends that
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
    let metadata = decoder
        .raw_metadata(source, &RawDecodeParams::default())
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
    let metadata = decoder
        .raw_metadata(source, &params)
        .map_err(decoder_error)?;
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
