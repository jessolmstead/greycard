//! Decoder backends: bytes in, [`RawFrame`] out.
//!
//! The engine never calls a backend's own develop path. A backend's job ends
//! at sensor samples and metadata; everything after that is an engine op with
//! a reference implementation and tests. Keeping the boundary here means a
//! second backend (LibRaw, a vendor SDK, a hand-written DNG reader) is a new
//! module, not a rewrite.

use std::path::Path;

use crate::error::Result;
use crate::raw::{Orientation, RawFrame};

mod focus;
pub mod rawler_backend;
mod sparse;
pub mod style;

pub use rawler_backend::{
    CameraWhite, IndexRead, MosaicRead, Probe, RawMetadata, RawlerDecoder, inside_decoder,
};
pub use style::{Adaptive, AdaptiveKind, CameraStyle, Maker};

pub trait Decoder {
    /// Decode a whole file held in memory.
    fn decode_bytes(&self, bytes: &[u8]) -> Result<RawFrame>;

    /// Decode a file on disk. The default reads it and calls
    /// [`Decoder::decode_bytes`]; backends with a faster path override it.
    fn decode_path(&self, path: &Path) -> Result<RawFrame> {
        let bytes = std::fs::read(path)?;
        self.decode_bytes(&bytes)
    }
}

/// The camera's own JPEG rendering of a file, if it has one, and the
/// orientation to show it in, read without a memory map; see
/// `rawler_backend::preview_path`.
pub fn preview_path(path: impl AsRef<Path>) -> Result<Option<(image::RgbImage, Orientation)>> {
    rawler_backend::preview_path(path.as_ref())
}

/// [`preview_path`] for a picture made `long_edge` or smaller: a file
/// with several previews gives the smallest that size or past it; see
/// `rawler_backend::preview_path_at_least`.
pub fn preview_path_at_least(
    path: impl AsRef<Path>,
    long_edge: u32,
) -> Result<Option<(image::RgbImage, Orientation)>> {
    rawler_backend::preview_path_at_least(path.as_ref(), long_edge)
}

/// Decode with the default backend.
pub fn decode_path(path: impl AsRef<Path>) -> Result<RawFrame> {
    RawlerDecoder.decode_path(path.as_ref())
}

/// [`decode_path`] from the file read whole into memory, never mapped:
/// for a pass over a folder's files (the thumbnails' quick develop of a
/// raw with no preview), whose volume can go away under it.
pub fn decode_path_unmapped(path: impl AsRef<Path>) -> Result<RawFrame> {
    RawlerDecoder.decode_path_unmapped(path.as_ref())
}

/// The frame and the metadata rawler read beside it, for an output that
/// carries EXIF (see [`crate::exif`] and [`crate::dng`]).
pub fn decode_path_with_metadata(path: impl AsRef<Path>) -> Result<(RawFrame, RawMetadata)> {
    RawlerDecoder.decode_path_with_metadata(path.as_ref())
}

/// The camera as the index names it ([`crate::raw::camera_name`] over
/// the decoder's cleaned make and model) for a model as the file's EXIF
/// spells it, the way Lightroom names a camera: "Canon EOS R6m2" is
/// "Canon EOS R6 Mark II". Case is ignored. None for a model the
/// decoder does not know; see `rawler_backend::cleaned_camera`.
pub fn cleaned_camera(exif_model: &str) -> Option<String> {
    rawler_backend::cleaned_camera(exif_model)
}

/// Every EXIF spelling of a model the decoder knows for a camera as the
/// index names it: the other way of [`cleaned_camera`]. Empty for a
/// camera it does not know.
pub fn exif_models(camera: &str) -> Vec<String> {
    rawler_backend::exif_models(camera)
}

/// Camera, lens, ISO, exposure and date from a raw's metadata,
/// without decoding the samples; see `rawler_backend::probe_path`.
/// A picture's is [`crate::picture::probe_path`].
pub fn probe_path(path: impl AsRef<Path>) -> Result<Probe> {
    rawler_backend::probe_path(path.as_ref())
}

/// A raw's as-shot white and calibrations without its samples; see
/// `rawler_backend::white_path`.
pub fn white_path(path: impl AsRef<Path>) -> Result<CameraWhite> {
    rawler_backend::white_path(path.as_ref())
}

/// [`probe_path`] from the file's head and tail alone, where its format
/// allows; the same answer, a fraction of the bytes read. See
/// `rawler_backend::probe_ends_path`.
pub fn probe_ends_path(path: impl AsRef<Path>) -> Result<Probe> {
    rawler_backend::probe_ends_path(path.as_ref())
}

/// What the library's index reads of a raw, its probe, its kind of
/// mosaic, and its maker's style, from the pieces of the file they are
/// in; see `rawler_backend::index_read_path`.
pub fn index_read_path(path: impl AsRef<Path>) -> Result<rawler_backend::IndexRead> {
    rawler_backend::index_read_path(path.as_ref())
}

/// The tag a merge of ours carries, read from the file's first IFD
/// with no decode; `None` for any other file. For a caller that must
/// tell a merge from a camera's frame before it reads further.
pub fn merge_tag_path(path: impl AsRef<Path>) -> Result<Option<crate::merge::Tag>> {
    style::merge_tag_stream(std::io::BufReader::new(std::fs::File::open(path.as_ref())?))
}

/// The camera's frame of a raw: the rectangle of the sensor, in
/// samples, that the photographer framed in camera (an aspect ratio
/// chosen on the body), when the file keeps more of the sensor than
/// that. `None` when the file does not say or the frame is all of it.
/// Read from the maker note with no decode; Olympus and OM System are
/// the makers read.
///
/// As the decode takes it: not where the decoder says it develops
/// another size than the note's valid area ([`Stance::size`]), so what
/// this says and what a develop frames agree. A file whose size the
/// decoder will not say cheaply keeps the note's word; its decode, if
/// it decodes, has the last one.
pub fn camera_frame_path(path: impl AsRef<Path>) -> Result<Option<crate::raw::CameraFrame>> {
    let path = path.as_ref();
    let Some(frame) = rawler_backend::guarded(
        || rawler_backend::half_copy_at(path),
        || style::frame_stream(std::io::BufReader::new(std::fs::File::open(path)?)),
    )?
    else {
        return Ok(None);
    };
    let size = stance_path_unmapped(path).ok().and_then(|s| s.size);
    Ok(frame.develops_at(size).then_some(frame))
}

/// The extensions of the raws [`camera_frame_path`] can find a frame in:
/// Olympus's and OM System's. Another maker read later adds its own.
const FRAMED_EXTENSIONS: [&str; 2] = ["orf", "ori"];

/// Whether the file at `path` may decode with a saturation level above
/// its white ([`crate::raw::Levels::saturation`]), by its name alone: a
/// Canon CR3, the one format the table covers. What the keys of the
/// pictures kept of a frame take [`HEADROOM`] in by, since they are made
/// without a decode.
pub fn may_have_headroom(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cr3"))
}

/// The saturation table's version, for the keys of whatever is kept of
/// a frame [`may_have_headroom`]: raise it when the table or how the
/// develop reads the headroom changes. The pictures, fills, and mask
/// bases kept of every CR3 are then made again (the keys know a file
/// by its name, not its body, so the R6 II's and the R5 II's and every
/// other Canon's alike); those of every other file are found again as
/// they were. The index learns of it through `keys_made_under`, which
/// takes this in: settling then clears the develop key of every row of
/// the library, of every make, not only the CR3s', and each is made
/// again from its sidecar when its folder is next read (an offline
/// root's rows, when it is back).
/// 1: the R6 II, R5 II, R5, R6, and R7 by ISO band, trusted to 0.98
/// of their range (never shipped), 2026-11-17.
/// 2: Canon's R6 II and R5 II, at the ISOs, shutters, and full frame
/// measured blown in every channel and with no capture mode that could
/// move the clip, trusted to 0.955 of their range, 2026-11-17.
/// 3: and the 850D (Rebel T8i, Kiss X10i) first curtain at ISO 200,
/// 2026-11-19.
pub const HEADROOM: &str = "headroom-3";

/// Whether the file at `path` may carry a `LinearizationTable`, by its
/// name alone: a DNG, the one format whose decoder hands one over.
/// What the keys of the pictures kept of a frame take [`LINEARIZATION`]
/// in by, since they are made without a decode.
pub fn may_have_linearization(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
}

/// How a DNG's `LinearizationTable` is read, for the keys of whatever
/// is kept of a frame [`may_have_linearization`]: raise it when that
/// changes, and the pictures, fills, and mask bases kept of every DNG
/// are made again, those of every other file found again as they were.
/// The index learns of it through `keys_made_under`, as of
/// [`HEADROOM`].
/// 1: the table applied at decode, before the levels, 2026-11-19.
pub const LINEARIZATION: &str = "linearization-1";

/// Whether the file at `path` is of a kind [`camera_frame_path`] can
/// find a frame in, by its name alone: what spares every other file a
/// read of its head.
pub fn may_have_camera_frame(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| FRAMED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Where the camera focused, read from the file's head with no decode,
/// as fractions of the picture its decode cuts to before the file's
/// orientation turns it, with that orientation; `None` when the file
/// does not say, focused by hand, or is of a maker not read, and for
/// anything at `path` that is not a regular file (a FIFO would block
/// the open). Reads the few small pieces of the head it needs and
/// nothing else: a CR3's box headers up to its maker note, a TIFF
/// raw's IFDs and maker note, a RAF's or an RW2's embedded JPEG's EXIF.
/// Canon, Nikon, Sony, Fujifilm, Panasonic, and Olympus and OM System
/// are the makers read.
pub fn focus_point_path(
    path: impl AsRef<Path>,
) -> Result<Option<(crate::raw::FocusPoint, Orientation)>> {
    use std::io::Read;
    let path = path.as_ref();
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Ok(None);
    }
    rawler_backend::guarded(
        || rawler_backend::half_copy_at(path),
        || {
            // The head in one read: a CR3's moov (30 to 60 KiB on the
            // bodies seen), and the maker note of every other raw
            // read, lie in it.
            // Only a walk that runs past it reads the file again, a
            // piece at a time.
            let mut file = std::fs::File::open(path)?;
            let mut head = Vec::with_capacity(FOCUS_HEAD as usize);
            (&mut file).take(FOCUS_HEAD).read_to_end(&mut head)?;
            match focus::focus_in_head(head, meta.len()) {
                Some(read) => read,
                None => focus::focus_head(file),
            }
        },
    )
}

/// How much of a file's head [`focus_point_path`] reads at once.
const FOCUS_HEAD: u64 = 128 << 10;

/// [`focus_point_path`] on a file's bytes in memory, unguarded.
pub fn focus_point_bytes(bytes: &[u8]) -> Result<Option<(crate::raw::FocusPoint, Orientation)>> {
    focus::focus_head(std::io::Cursor::new(bytes))
}

/// The extensions of the raws [`focus_point_path`] can find a point
/// in.
const FOCUS_EXTENSIONS: [&str; 8] = ["cr3", "cr2", "nef", "arw", "raf", "rw2", "orf", "ori"];

/// Whether the file at `path` is of a kind [`focus_point_path`] can
/// find a point in, by its name alone.
pub fn may_have_focus_point(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| FOCUS_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// A raw's kind of mosaic alone, without a sample decoded and without a
/// memory map; `Ok(None)` for a raw that is not a mosaic.
pub fn mosaic_path(path: impl AsRef<Path>) -> Result<Option<crate::raw::Mosaic>> {
    rawler_backend::mosaic_path(path.as_ref())
}

/// The extensions taken for a raw the decoder opens. `ori` is an ORF
/// under another name: the single frame an Olympus or OM System body
/// saves beside a High Res Shot's `.ORF`, which never takes its stem's
/// short names or JPEG ([`always_yields`]). `pef` is Pentax's own
/// raw. The order is the one a camera JPEG picks its raw by among raws
/// of one stem (`greycard_library::pairs::raw_rank`).
pub const RAW_EXTENSIONS: [&str; 10] = [
    "cr3", "cr2", "dng", "nef", "arw", "raf", "orf", "rw2", "pef", "ori",
];

/// Whether a file's name begins with a dot, as a hidden file's does:
/// macOS's `._NAME` resource forks beside the raws on a share or a
/// card, and `.DS_Store`. A folder's listing passes over them, on any
/// platform, since such files travel with the folder; a file named by
/// hand still opens.
pub fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.as_encoded_bytes().starts_with(b"."))
}

/// Whether a path's extension is one of a raw's.
pub fn is_raw_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| RAW_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// What a raw is to the short-named files of its stem, the other
/// tools' `NAME.xmp` and `NAME.pp3`: whose they are when more than one
/// raw has the stem ([`short_names`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortNames {
    /// No other raw has its stem: they are its own.
    Alone,
    /// It is there, and the others of its stem are its lesser siblings
    /// (an `.ORF`'s `.ORI`): they are its own all the same.
    Owns,
    /// They are never its: it is an `.ORI` ([`always_yields`]), or an
    /// `.ORF` gone from beside its `.ORI`. It takes nothing from them.
    Yields,
    /// Another raw of its stem that is neither (a `.DNG` beside a
    /// `.RAF`): which of the two they are for, the names do not say.
    Shared,
}

/// Whether `other` is `raw`'s lesser sibling: the single frame an
/// Olympus or OM System body saves as `NAME.ORI` beside the High Res
/// Shot it made, `NAME.ORF`. Extensions only, so no answer about whose
/// a short name is: that is [`short_names`]'s, over a real listing.
fn is_lesser_sibling(raw: &Path, other: &Path) -> bool {
    let ext = |p: &Path| {
        p.extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
    };
    ext(raw).as_deref() == Some("orf") && ext(other).as_deref() == Some("ori")
}

/// Whether `raw` yields its stem's short names (`NAME.xmp`,
/// `NAME.pp3`, a Lightroom take, the camera's `NAME.JPG`) whatever is
/// beside it: an `.ORI`, the single frame of a High Res Shot. Its own
/// are its long names alone (`NAME.ORI.xmp`, `NAME.ORI.gcd`). Whether
/// its `.ORF` is there or not never changes this, so nothing it reads
/// or writes moves with the listing. The half of [`short_names`] that
/// needs no listing.
pub fn always_yields(raw: &Path) -> bool {
    raw.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("ori"))
}

/// [`ShortNames`] for `raw` among the names in its folder (`names`,
/// file names or paths: what is there, `raw`'s own among them when it
/// is), the stems compared without case as the pairs compare them. The
/// one rule for every reader of a stem's short names: an `.ORI` never
/// owns them; another raw owns them alone, or beside its lesser
/// siblings when it is there itself.
pub fn short_names<P: AsRef<Path>>(raw: &Path, names: impl IntoIterator<Item = P>) -> ShortNames {
    if always_yields(raw) {
        return ShortNames::Yields;
    }
    let (Some(stem), Some(name)) = (raw.file_stem(), raw.file_name()) else {
        return ShortNames::Alone;
    };
    let stem = stem.to_string_lossy().to_lowercase();
    let (mut there, mut lesser, mut rival) = (false, false, false);
    for other in names {
        let other = other.as_ref();
        if other.file_name() == Some(name) {
            there = true;
            continue;
        }
        let same_stem = other
            .file_stem()
            .is_some_and(|s| s.to_string_lossy().to_lowercase() == stem);
        if !same_stem || !is_raw_path(other) {
            continue;
        }
        // An `.ORI` is no rival: it never owns them.
        if is_lesser_sibling(raw, other) {
            lesser = true;
        } else if !always_yields(other) {
            rival = true;
        }
    }
    match (rival, lesser, there) {
        (true, _, _) => ShortNames::Shared,
        (false, false, _) => ShortNames::Alone,
        (false, true, true) => ShortNames::Owns,
        (false, true, false) => ShortNames::Yields,
    }
}

/// Whether `raw` owns its stem's short names among `names` (what is in
/// its folder): [`ShortNames::Alone`] or [`ShortNames::Owns`].
pub fn owns_short_names<P: AsRef<Path>>(raw: &Path, names: impl IntoIterator<Item = P>) -> bool {
    matches!(
        short_names(raw, names),
        ShortNames::Alone | ShortNames::Owns
    )
}

/// [`short_names`] over `raw`'s folder as it is now; [`ShortNames::Alone`]
/// for a folder that will not read.
pub fn short_names_on_disk(raw: &Path) -> ShortNames {
    let Some(entries) = raw.parent().and_then(|d| std::fs::read_dir(d).ok()) else {
        return ShortNames::Alone;
    };
    short_names(
        raw,
        entries
            .flatten()
            .map(|e| std::path::PathBuf::from(e.file_name())),
    )
}

/// How a file's frame stands, without decoding a pixel of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stance {
    /// The orientation tag the file carries. What a frame's quarter
    /// turns are measured from: a turn is stored as quarters on top
    /// of this, so the same edit reads the same however a later build
    /// reads the tag.
    pub orientation: Orientation,
    /// The size a develop comes out at *before* the tag turns it;
    /// `None` when the file will not say this cheaply.
    pub size: Option<(u32, u32)>,
    /// The kind of mosaic, read from the same description as the size
    /// (rawler's metadata alone does not carry the pattern); `None`
    /// for a picture, a raw that is not a mosaic, or one that will not
    /// say. What the editor's demosaic menu and a sync's demosaic
    /// check go by before the pixels are read.
    pub mosaic: Option<crate::raw::Mosaic>,
}

impl Stance {
    /// The size the picture is shown at: the frame's own, turned by
    /// the tag and by `turn` quarter turns clockwise on top of it.
    pub fn shown_size(&self, turn: u8) -> Option<(u32, u32)> {
        let o = self.orientation.turned(i32::from(turn % 4));
        self.size.map(|(w, h)| {
            if matches!(
                o,
                Orientation::Transpose
                    | Orientation::Rotate90
                    | Orientation::Transverse
                    | Orientation::Rotate270
            ) {
                (h, w)
            } else {
                (w, h)
            }
        })
    }
}

/// How a file's frame stands, raw or picture: its orientation tag and
/// the size a develop of it comes out at, with no pixel decoded.
///
/// One reader a file, so nothing can disagree with the picture on
/// screen about which way up it is or what shape it is.
pub fn stance_path(path: impl AsRef<Path>) -> Result<Stance> {
    let path = path.as_ref();
    let (orientation, size, mosaic) = if crate::picture::is_picture_path(path) {
        // The picture module's own reader, which is the one that
        // decides which way up the picture is drawn.
        let (orientation, size) = crate::picture::stance_path(path)?;
        (orientation, size, None)
    } else {
        rawler_backend::stance_path(path)?
    };
    Ok(Stance {
        orientation,
        size,
        mosaic,
    })
}

/// A raw's [`probe_path`] and its [`stance_path`] in one read of the
/// file, from its ends where its format allows (a CR3's), and how many
/// bytes that read; never through a memory map, so a volume that goes
/// away under it is an error and not a SIGBUS. An error when the
/// metadata will not read; a size
/// or mosaic the image will not give is `None` in the stance. A raw's
/// alone: a picture's two readers are its own. See
/// `rawler_backend::probe_and_stance_counted`.
pub fn probe_and_stance_counted(path: impl AsRef<Path>) -> Result<(Probe, Stance, u64)> {
    let (probe, (orientation, size, mosaic), read) =
        rawler_backend::probe_and_stance_counted(path.as_ref())?;
    Ok((
        probe,
        Stance {
            orientation,
            size,
            mosaic,
        },
        read,
    ))
}

/// [`stance_path`] never through a memory map: a raw's from its ends
/// where its format allows, else read whole ([`probe_and_stance_counted`]),
/// a picture's from its own reader. For a pass over many files, the
/// thumbnails' develop and the lightbox's among them, whose volume can go
/// away under a mapping.
pub fn stance_path_unmapped(path: impl AsRef<Path>) -> Result<Stance> {
    let path = path.as_ref();
    if is_raw_path(path) && !crate::picture::is_picture_path(path) {
        return Ok(probe_and_stance_counted(path)?.1);
    }
    stance_path(path)
}

/// Count from now, on every thread, the raws under `dir` memory-mapped:
/// for a test that a read meant for a volume that can go away
/// ([`probe_and_stance_counted`], [`probe_ends_path`], what the index
/// and the thumbnails read) never maps one.
pub fn watch_maps_under(dir: &Path) {
    rawler_backend::watch_maps_under(dir)
}

/// How many raws under `dir` were memory-mapped since
/// [`watch_maps_under`].
pub fn maps_under(dir: &Path) -> u64 {
    rawler_backend::maps_under(dir)
}

/// The orientation tag a file carries, raw or picture, without
/// decoding a pixel of it. A raw's is its probe's, read from its ends
/// where its format allows ([`probe_ends_path`]) and never mapped: the
/// tag the stance reads, from the same metadata, at a fraction of the
/// bytes. A picture's, or a file named as neither, is the stance's.
pub fn orientation_path(path: impl AsRef<Path>) -> Result<Orientation> {
    let path = path.as_ref();
    if is_raw_path(path) && !crate::picture::is_picture_path(path) {
        return Ok(probe_ends_path(path)?.orientation);
    }
    Ok(stance_path(path)?.orientation)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// A stem's short names are a lone raw's, an ORF's over its ORI,
    /// and nobody's between two raws that are not siblings; an ORI's
    /// never, whatever is beside it. Pictures and sidecars claim
    /// nothing.
    #[test]
    fn a_stems_short_names_go_to_the_raw_alone_or_the_greater_sibling() {
        use super::{ShortNames, short_names};
        let of =
            |raw: &str, names: &[&str]| short_names(Path::new(raw), names.iter().map(Path::new));
        let both = ["P1.ORF", "P1.ORI", "P1.JPG", "P1.xmp", "P1.ORF.gcd"];
        assert_eq!(of("P1.ORF", &both), ShortNames::Owns);
        assert_eq!(of("P1.ORI", &both), ShortNames::Yields);
        assert_eq!(of("p1.ori", &["P1.orf"]), ShortNames::Yields, "case aside");
        assert_eq!(
            of("P1.ORI", &["P1.ORI", "P1.JPG", "P1.pp3"]),
            ShortNames::Yields
        );
        assert_eq!(of("P1.ORI", &[] as &[&str]), ShortNames::Yields);
        assert!(super::always_yields(Path::new("x.Ori")));
        assert!(!super::always_yields(Path::new("x.ORF")));
        assert_eq!(of("P1.ORF", &["P1.ORF", "P2.ORI"]), ShortNames::Alone);
        assert_eq!(of("A.RAF", &["A.RAF", "A.DNG"]), ShortNames::Shared);
        assert_eq!(of("A.DNG", &["A.RAF", "A.DNG"]), ShortNames::Shared);
        // An ORF gone from beside its ORI: they are neither's.
        let gone = ["P1.ORI", "P1.xmp", "P1.ORF.gcd"];
        assert_eq!(of("P1.ORF", &gone), ShortNames::Yields);
        assert_eq!(of("P1.ORI", &gone), ShortNames::Yields);
        assert!(!super::owns_short_names(
            Path::new("P1.ORF"),
            gone.iter().map(Path::new)
        ));
        // A lone raw gone keeps its claim, as it always had.
        assert_eq!(of("Q.CR3", &["Q.xmp"]), ShortNames::Alone);
        // A third raw makes the ORF's nobody's; the ORI still yields.
        let three = ["P1.ORF", "P1.ORI", "P1.DNG"];
        assert_eq!(of("P1.ORF", &three), ShortNames::Shared);
        assert_eq!(of("P1.ORI", &three), ShortNames::Yields);
        assert_eq!(of("P1.DNG", &three), ShortNames::Shared);
        // A DNG beside an ORI and no ORF owns them, as on master: the
        // ORI is no rival.
        let dng = ["P1.DNG", "P1.ORI", "P1.xmp"];
        assert_eq!(of("P1.DNG", &dng), ShortNames::Alone);
        assert_eq!(of("P1.ORI", &dng), ShortNames::Yields);
    }

    /// A dot at the head of a file's name hides it; a dot inside a
    /// name, or a hidden folder above it, does not. Hidden or not, the
    /// extension still says which decoder opens it.
    #[test]
    fn a_hidden_file_is_one_whose_name_begins_with_a_dot() {
        for name in ["._IMG.CR3", ".IMG.CR3", "._IMG.xmp", ".DS_Store"] {
            assert!(
                super::is_hidden(&Path::new("/share/day").join(name)),
                "{name}"
            );
        }
        for p in [
            "/share/day/IMG.CR3",
            "/share/day/a._b.cr3",
            "/share/.day/IMG.CR3",
            "/share/day/IMG.CR3.gcd",
        ] {
            assert!(!super::is_hidden(Path::new(p)), "{p}");
        }
        assert!(super::is_raw_path(Path::new("/share/day/._IMG.CR3")));
        assert!(crate::picture::is_picture_path(Path::new(
            "/share/day/.x.jpg"
        )));
    }

    /// Pentax's `.PEF` is a raw in any case, and an ordinary one: it
    /// owns its stem's short names alone and shares them beside a DNG
    /// of its stem, as a RAF does. Neither its focus point nor its
    /// camera frame is read, so its name spares it those reads.
    #[test]
    fn a_pef_is_a_raw_of_its_own() {
        use super::{ShortNames, short_names};
        for name in ["K1.PEF", "k1.pef", "K1.Pef"] {
            let p = Path::new("/shoot").join(name);
            assert!(super::is_raw_path(&p), "{name}");
            assert!(!super::always_yields(&p), "{name}");
            assert!(!super::may_have_focus_point(&p), "{name}");
            assert!(!super::may_have_camera_frame(&p), "{name}");
        }
        let of = |raw: &str, names: &[&str]| short_names(Path::new(raw), names);
        assert_eq!(
            of("K1.PEF", &["K1.PEF", "K1.JPG", "K1.xmp"]),
            ShortNames::Alone
        );
        assert_eq!(of("K1.PEF", &["K1.PEF", "K1.DNG"]), ShortNames::Shared);
        assert!(!super::is_raw_path(Path::new("/shoot/K1.PEF.xmp")));
    }

    /// A merge of ours is told by its tag with no decode; a camera's
    /// frame and a file that is no TIFF have none.
    #[test]
    fn a_merge_is_told_by_its_tag() {
        use crate::merge::testing::{write_dng, write_merged};
        let dir = std::env::temp_dir().join(format!("greycard-merge-tag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let frame = dir.join("IMG_0.dng");
        write_dng(&frame, vec![1000; 64 * 48], 64, 48, (1, 100), "Bracket");
        let merged = dir.join("IMG_0-HDR.dng");
        write_merged(&frame, &merged);
        let text = dir.join("notes.txt");
        std::fs::write(&text, "not a raw").unwrap();
        let tag = super::merge_tag_path(&merged)
            .unwrap()
            .expect("a merge's tag");
        assert_eq!((tag.kind.as_str(), tag.headroom_stops), ("hdr", 1.0));
        assert_eq!(super::merge_tag_path(&frame).unwrap(), None);
        assert_eq!(super::merge_tag_path(&text).unwrap(), None);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A focus point read refuses what is not a regular file without
    /// opening it: a FIFO named as a raw would block the open for ever.
    #[cfg(unix)]
    #[test]
    fn a_focus_read_opens_no_fifo_or_folder() {
        let dir = std::env::temp_dir().join(format!("greycard-focus-fifo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("IMG_0001.CR3");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if made.is_ok_and(|s| s.success()) {
            assert_eq!(super::focus_point_path(&fifo).unwrap(), None);
        }
        let folder = dir.join("IMG_0002.CR3");
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(super::focus_point_path(&folder).unwrap(), None);
        assert!(super::focus_point_path(dir.join("IMG_0003.CR3")).is_err());
        crate::testing::remove_dir_retry(&dir);
    }

    /// With `GREYCARD_SAMPLES` a directory of raws: each file's preview,
    /// its size, orientation and the time to read it, printed.
    #[test]
    #[ignore]
    fn previews_of_the_samples() {
        let Some(dir) = std::env::var_os("GREYCARD_SAMPLES") else {
            return;
        };
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let path = e.path();
            if !super::is_raw_path(&path) {
                continue;
            }
            let t = std::time::Instant::now();
            match super::preview_path(&path) {
                Ok(Some((img, o))) => println!(
                    "{}: {}x{} {:?} in {:.0} ms",
                    path.file_name().unwrap().to_string_lossy(),
                    img.width(),
                    img.height(),
                    o,
                    t.elapsed().as_secs_f64() * 1000.0
                ),
                Ok(None) => println!("{}: no preview", path.display()),
                Err(e) => println!("{}: {e}", path.display()),
            }
        }
    }
}
