//! Import from a card: every frame copied to the destination under
//! its new name, read back and checked against the card by a hash of
//! the whole file, then copied again to a backup folder if one is
//! asked for, and given a preset.
//!
//! Nothing here is written on the card or deleted from it, and no
//! destination or backup on the card is taken (see [`card_root`]). A
//! file is written under a temporary name of its own in its own
//! destination folder and given its name only once its copy reads back
//! the same as the card's bytes did, by a rename that never replaces a
//! file, so a stop, a full disk or a dying card never leaves a
//! half-written file under a real name. The temporary is in the
//! destination folder and not a temp directory because a rename across
//! drives is a copy (and on Windows an error). A name that is there
//! already is never written over.
//!
//! No window here: the sheet (`panel::import`) and `--import` both
//! run it on a thread of their own and hear its progress through a
//! callback.

use crate::export_name::modified_day;
use crate::naming::{self, Day, Fields};
use crate::tr;
use anyhow::{Context, Result, bail};
use greycard_edit::{Placement, Preset};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// What an import is asked to do.
#[derive(Debug, Clone)]
pub struct Options {
    /// The card's folder (its `DCIM`, or anything under it), walked
    /// whole.
    pub source: PathBuf,
    pub destination: PathBuf,
    /// The folders under the destination, a [`naming`] pattern;
    /// empty for the destination itself.
    pub subfolder: String,
    /// The file's name before its extension, a [`naming`] pattern.
    pub name: String,
    /// Laid over each frame as one history step, "Preset: X".
    pub preset: Option<Preset>,
    /// A second copy of every file, under the same folders and names.
    pub backup: Option<PathBuf>,
    /// The library index to ask whether a file is imported already.
    pub library: Option<PathBuf>,
    /// Compare a frame already there with the card byte for byte,
    /// rather than by its size and its first and last 64 KiB (see
    /// [`same_bytes`]). Off by default, since it reads the whole card
    /// again on a re-run.
    pub verify: bool,
    /// Where a preset's sidecar is written, as the Settings sheet
    /// says (§153); none when the run writes no sidecars, and then no
    /// preset is laid.
    pub placement: Option<Placement>,
    /// The camera profiles the profile folder holds, for the preset's
    /// fit check: a profile made for another body is left off (§162).
    pub profiles: Vec<greycard_edit::camera::Entry>,
}

/// A frame on the card and the files that go with it: a raw's XMP
/// and the camera's JPEG of the same name.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub file: PathBuf,
    pub companions: Vec<PathBuf>,
}

/// What a card holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
    pub items: Vec<Item>,
    /// Files left on the card: ones the editor does not open (a
    /// video, the camera's own catalog) and XMPs with no frame.
    pub left: Vec<PathBuf>,
    /// Folders under the source that could not be read, and so were
    /// passed over with whatever is in them.
    pub unreadable: usize,
}

impl Scan {
    /// Every file that would be copied, companions included.
    pub fn files(&self) -> usize {
        self.items.iter().map(|i| 1 + i.companions.len()).sum()
    }
}

fn extension_is(p: &Path, want: &str) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(want))
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn stem_of(p: &Path) -> String {
    p.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Every file under `dir`, walked depth first, hidden files and
/// folders left out (the Mac's `._` shadows, a `.greycard` folder).
/// A symbolic link is not followed, to a folder or to a file: a link
/// out of the destination would make the card's own files look
/// imported, and a link back up a card's tree would list its frames
/// over and over. A camera writes none. A folder reached twice (a
/// bind mount, a Windows junction) is walked once, by its canonical
/// path. A folder under `dir` that cannot be read is passed over and
/// counted, the count returned; `dir` itself not read is the error.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<usize> {
    let mut seen = HashSet::new();
    let mut unreadable = 0;
    walk_from(dir, out, &mut seen, &mut unreadable)?;
    Ok(unreadable)
}

fn walk_from(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    unreadable: &mut usize,
) -> std::io::Result<()> {
    let canonical = dunce::canonicalize(dir)?;
    if !seen.insert(canonical) {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        match e.file_type() {
            Ok(t) if t.is_symlink() => {
                tracing::debug!("import: {} is a link, not followed", path.display());
            }
            Ok(t) if t.is_dir() => {
                if let Err(e) = walk_from(&path, out, seen, unreadable) {
                    tracing::warn!("import: {} not read: {e}", path.display());
                    *unreadable += 1;
                }
            }
            Ok(t) if t.is_file() => out.push(path),
            _ => {}
        }
    }
    Ok(())
}

/// How many folders are under `dir`, at any depth, as [`walk`] goes
/// through them: hidden ones and links left out, a folder reached
/// twice counted once. One that cannot be read counts, and nothing
/// under it.
pub fn subfolders(dir: &Path) -> usize {
    fn count(dir: &Path, seen: &mut HashSet<PathBuf>) -> usize {
        let Ok(canonical) = dunce::canonicalize(dir) else {
            return 0;
        };
        if !seen.insert(canonical) {
            return 0;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .filter_map(|e| e.ok())
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| 1 + count(&e.path(), seen))
            .sum()
    }
    count(dir, &mut HashSet::new())
}

/// The frames under `source`, in the order the camera numbered them
/// (by path: `100CANON` before `101CANON`, `IMG_0001` before
/// `IMG_0002`), each raw with its companions. A picture with a raw of
/// the same name in its folder is the camera's JPEG of that raw and
/// goes along with it; a picture with no raw is a frame of its own,
/// as a JPEG-only camera writes them. An XMP goes with the frame it
/// names, `IMG.xmp` or `IMG.CR3.xmp` (or `IMG.JPG.xmp`, with the JPEG
/// or the raw that takes it), and is left when there is none.
/// Two raws of one stem in one folder (`DSCF0001.RAF` and a
/// `DSCF0001.DNG` made from it) are two frames, and the companions of
/// that stem go with the one the browser pairs the camera's JPEG with
/// (`greycard_library::pairs`): the camera's own format before a DNG.
pub fn scan(source: &Path) -> Result<Scan> {
    anyhow::ensure!(source.is_dir(), tr!("{} is not a folder", source.display()));
    let mut files = Vec::new();
    let unreadable =
        walk(source, &mut files).with_context(|| tr!("reading {}", source.display()))?;
    files.sort();
    let key = |p: &Path| (p.parent().map(Path::to_path_buf), stem_of(p).to_lowercase());
    let whole = |p: &Path| (p.parent().map(Path::to_path_buf), name_of(p).to_lowercase());
    // The raws by folder and stem: they take the companions. The
    // frames, a lone picture's too, by their whole names: an XMP that
    // names one (`P1.ORI.xmp` beside `P1.ORF`) is that frame's.
    let mut raws: HashMap<(Option<PathBuf>, String), usize> = HashMap::new();
    let mut by_name: HashMap<(Option<PathBuf>, String), usize> = HashMap::new();
    let mut items: Vec<Item> = Vec::new();
    for f in files
        .iter()
        .filter(|f| greycard_core::decode::is_raw_path(f))
    {
        let at = items.len();
        by_name.insert(whole(f), at);
        // An `.ORI` takes nothing by its stem: the JPEG and the short
        // XMP are its `.ORF`'s, or not a raw's.
        if !greycard_core::decode::always_yields(f) {
            raws.entry(key(f))
                .and_modify(|best| {
                    let rank = greycard_library::pairs::raw_rank;
                    if rank(f) < rank(&items[*best].file) {
                        *best = at;
                    }
                })
                .or_insert(at);
        }
        items.push(Item {
            file: f.clone(),
            companions: Vec::new(),
        });
    }
    let mut left = Vec::new();
    let mut xmps = Vec::new();
    for f in &files {
        if greycard_core::decode::is_raw_path(f) {
            continue;
        }
        if extension_is(f, "xmp") {
            xmps.push(f);
            continue;
        }
        match raws.get(&key(f)).copied() {
            Some(i) if greycard_core::picture::is_picture_path(f) => {
                items[i].companions.push(f.clone())
            }
            None if greycard_core::picture::is_picture_path(f) => {
                by_name.insert(whole(f), items.len());
                items.push(Item {
                    file: f.clone(),
                    companions: Vec::new(),
                });
            }
            _ => left.push(f.clone()),
        }
    }
    // An XMP goes with the frame its whole name names (`IMG.CR3.xmp`,
    // a lone `IMG.JPG.xmp`), else with its stem's raw: `IMG.xmp`; the
    // `IMG.JPG.xmp` darktable writes for a raw's JPEG, which goes where
    // the JPEG goes; and one named for a raw that is not there
    // (`X.CR2.xmp` beside `X.DNG`).
    for f in xmps {
        let inner = f.with_extension("");
        let owner = by_name.get(&whole(&inner)).copied().or_else(|| {
            let named = greycard_core::picture::is_picture_path(&inner)
                || greycard_core::decode::is_raw_path(&inner);
            raws.get(&key(if named { &inner } else { f })).copied()
        });
        match owner {
            Some(i) => items[i].companions.push(f.clone()),
            None => left.push(f.clone()),
        }
    }
    // In the order they were found, the path's.
    for item in &mut items {
        item.companions.sort();
    }
    left.sort();
    // Lone pictures joined the list after the raws; the camera's order
    // is the path's.
    items.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(Scan {
        items,
        left,
        unreadable,
    })
}

/// The tokens' values for `file`, the `seq`th frame of the run: the
/// day and the camera from its EXIF, the day from its modification
/// time when the EXIF has none. A file the probe cannot read (or that
/// makes the decoder give up) is named from its modification time and
/// no camera, and still imported.
pub fn fields_of(file: &Path, seq: usize) -> Fields {
    probe_file(file, seq).0
}

/// [`fields_of`], and the probe when the file reads, for the preset's
/// camera-profile check.
pub fn probe_file(file: &Path, seq: usize) -> (Fields, Option<greycard_core::decode::Probe>) {
    let probed = std::panic::catch_unwind(|| {
        if greycard_core::picture::is_picture_path(file) {
            greycard_core::picture::probe_path(file)
        } else {
            greycard_core::decode::probe_path(file)
        }
    });
    let (taken, camera, body) = match probed {
        Ok(Ok(p)) => (
            p.taken.as_deref().and_then(Day::from_exif),
            greycard_core::raw::camera_name(&p.make, &p.model),
            Some(p),
        ),
        _ => (None, String::new(), None),
    };
    let day = taken.unwrap_or_else(|| modified_day(file));
    let fields = Fields {
        day,
        name: stem_of(file),
        camera,
        seq,
        folder: String::new(),
    };
    (fields, body)
}

/// `file`'s folder under `source`, its levels joined by `/` whatever
/// the platform: what `{folder}` fills with. Empty for a file in the
/// source itself, or one not under it.
pub fn folder_under(source: &Path, file: &Path) -> String {
    file.parent()
        .and_then(|dir| dir.strip_prefix(source).ok())
        .map(|rel| {
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default()
}

/// Where a frame goes, relative to the destination: its folders and
/// its name. The folder part is the backup's as well.
pub fn relative(opts: &Options, file: &Path, fields: &Fields) -> Result<PathBuf> {
    relative_within(opts, file, fields, 0)
}

/// [`relative`], the name leaving `reserve` bytes free for a companion's
/// longer tail (see [`naming::file_name_within`]).
fn relative_within(
    opts: &Options,
    file: &Path,
    fields: &Fields,
    reserve: usize,
) -> Result<PathBuf> {
    let folders = naming::folders(&opts.subfolder, fields)?;
    let extension = file
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = naming::file_name_within(&opts.name, fields, &extension, reserve)?;
    let mut rel = PathBuf::new();
    for f in folders {
        rel.push(f);
    }
    rel.push(name);
    Ok(rel)
}

/// What a companion is called beside its frame's new name: the new
/// stem and whatever followed the old stem, so `IMG_0001.JPG` beside
/// `IMG_0001.CR3` renamed `2026-0001.CR3` is `2026-0001.JPG`, and
/// `IMG_0001.CR3.xmp` is `2026-0001.CR3.xmp`.
pub fn companion_name(frame: &Path, new_frame: &Path, companion: &Path) -> String {
    let old_stem = stem_of(frame);
    let new_stem = stem_of(new_frame);
    let name = name_of(companion);
    let tail = name.get(old_stem.len()..).unwrap_or_default();
    naming::with_tail(&new_stem, tail)
}

/// How many bytes longer than the frame's own extension its longest
/// companion's tail is (`.CR3.xmp` against `.CR3`): room kept free in
/// the frame's name so the companion's name keeps the frame's stem.
fn companion_reserve(item: &Item) -> usize {
    let stem = stem_of(&item.file).len();
    let own = item
        .file
        .extension()
        .map_or(0, |e| e.to_string_lossy().len() + 1);
    item.companions
        .iter()
        .map(|c| name_of(c).len().saturating_sub(stem))
        .max()
        .unwrap_or(0)
        .saturating_sub(own)
}

/// `path` with `-n` before its extension, the name kept within
/// [`naming::LONGEST`] less `reserve` however long it was.
fn numbered(path: &Path, n: usize, reserve: usize) -> PathBuf {
    let tail = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let suffix = format!("-{n}");
    let room = naming::LONGEST.saturating_sub(tail.len() + suffix.len() + reserve);
    let stem = naming::with_tail(&stem_of(path), "");
    let mut cut = room.min(stem.len());
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    path.with_file_name(format!("{}{suffix}{tail}", &stem[..cut]))
}

/// Stop after the frame in hand.
#[derive(Debug, Default)]
pub struct Job {
    canceled: AtomicBool,
}

impl Job {
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::SeqCst);
    }
    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }
}

/// What an import came to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// Frames on the card.
    pub total: usize,
    /// Frames copied and verified.
    pub frames: usize,
    /// Files copied and verified, companions included; and their bytes.
    pub files: usize,
    pub bytes: u64,
    /// Files copied to the backup and verified there.
    pub backed_up: usize,
    /// Frames already imported: the same bytes under the destination,
    /// or a file the library holds with the same bytes.
    pub already: usize,
    /// Frames whose name was another file's, given `-1` instead.
    pub renamed: usize,
    /// Companions not copied because their name was another file's.
    pub companions_left: usize,
    /// Frames given the preset, and frames the preset's camera
    /// profile was left off because it was made for another body.
    pub preset: usize,
    pub profile_left_off: usize,
    /// Frames whose edit came from the Lightroom XMP beside them.
    pub lightroom: usize,
    /// The stop was asked for before the last frame.
    pub canceled: bool,
    /// The file that stopped the import, and why.
    pub error: Option<(PathBuf, String)>,
    pub seconds: f64,
    /// The folders written to and how many frames each took, in the
    /// order they were first written.
    pub folders: Vec<(PathBuf, usize)>,
    /// Folders under the source the scan could not read: frames there
    /// were not reached, so the import is not whole however the rest
    /// went (`Scan::unreadable`).
    pub unreadable: usize,
}

impl Report {
    /// The folder that took the most frames: the one to open.
    pub fn main_folder(&self) -> Option<&Path> {
        let most = self.folders.iter().map(|(_, n)| *n).max()?;
        self.folders
            .iter()
            .find(|(_, n)| *n == most)
            .map(|(f, _)| f.as_path())
    }

    fn landed_in(&mut self, folder: &Path) {
        match self.folders.iter_mut().find(|(f, _)| f == folder) {
            Some((_, n)) => *n += 1,
            None => self.folders.push((folder.to_path_buf(), 1)),
        }
    }

    /// Megabytes a second read off the card and written.
    pub fn rate(&self) -> f64 {
        if self.seconds > 0.0 {
            self.bytes as f64 / 1e6 / self.seconds
        } else {
            0.0
        }
    }

    /// The status line's words once it is done.
    pub fn line(&self, destination: &Path) -> String {
        let mut s = if self.canceled {
            tr!(
                "import stopped: {done} of {n} frame imported"
                    | "import stopped: {done} of {n} frames imported" % self.total,
                done = self.frames
            )
        } else if self.error.is_some() {
            tr!(
                "import failed after {done} of {n} frame"
                    | "import failed after {done} of {n} frames" % self.total,
                done = self.frames
            )
        } else {
            tr!("imported {n} frame" | "imported {n} frames" % self.frames)
        };
        s.push_str(&tr!(" to {}", destination.display()));
        if self.bytes > 0 {
            s.push_str(&tr!(
                " ({} MB at {} MB/s)",
                format!("{:.0}", self.bytes as f64 / 1e6),
                format!("{:.0}", self.rate())
            ));
        }
        if self.backed_up > 0 {
            s.push_str(&tr!(", {} backed up", self.backed_up));
        }
        if self.already > 0 {
            s.push_str(&tr!(", {} already there", self.already));
        }
        if self.lightroom > 0 {
            s.push_str(&tr!(", {} with a Lightroom edit", self.lightroom));
        }
        if self.renamed > 0 {
            s.push_str(&tr!(
                ", {} renamed with -1 (the name was another file's)",
                self.renamed
            ));
        }
        if self.companions_left > 0 {
            s.push_str(&tr!(
                ", {} JPEG or XMP not copied (the name was another file's)",
                self.companions_left
            ));
        }
        if self.profile_left_off > 0 {
            s.push_str(&tr!(
                ", the preset's camera profile left off {n} frame (made for another camera)"
                    | ", the preset's camera profile left off {n} frames (made for another camera)"
                        % self.profile_left_off
            ));
        }
        if self.unreadable > 0 {
            s.push_str(&tr!(
                "; {n} folder could not be read, its frames not imported"
                    | "; {n} folders could not be read, their frames not imported"
                        % self.unreadable
            ));
        }
        if let Some((file, why)) = &self.error {
            s.push_str(&format!(": {}: {why}", name_of(file)));
        }
        s
    }

    /// Whether every frame under the source was reached and the run
    /// went through: no error, no stop, no folder unread.
    pub fn whole(&self) -> bool {
        self.error.is_none() && !self.canceled && self.unreadable == 0
    }
}

/// The import's record in the log: from where to where, what came of
/// it, and what was left. A warning when anything did not go, so a
/// command-line run says it without -v.
pub fn log_report(opts: &Options, scan: &Scan, report: &Report) {
    let line = format!(
        "import: {} to {}: {} of {} imported ({} files, {} bytes in {:.2} s, {:.1} MB/s), \
         {} backed up{}, {} already there, {} renamed, {} companions not copied, \
         {} given {}, profile left off {}, {} with a Lightroom edit, {} left on the card, \
         {} folders unread{}{}",
        opts.source.display(),
        opts.destination.display(),
        report.frames,
        frames(report.total),
        report.files,
        report.bytes,
        report.seconds,
        report.rate(),
        report.backed_up,
        opts.backup
            .as_deref()
            .map(|b| format!(" to {}", b.display()))
            .unwrap_or_default(),
        report.already,
        report.renamed,
        report.companions_left,
        report.preset,
        opts.preset
            .as_ref()
            .map_or_else(|| "no preset".to_string(), |p| format!("\"{}\"", p.name)),
        report.profile_left_off,
        report.lightroom,
        scan.left.len(),
        report.unreadable,
        if report.canceled { "; stopped" } else { "" },
        report
            .error
            .as_ref()
            .map(|(f, why)| format!("; failed on {}: {why}", f.display()))
            .unwrap_or_default(),
    );
    if !report.whole() || report.renamed > 0 || report.companions_left > 0 {
        tracing::warn!("{line}");
    } else {
        tracing::info!("{line}");
    }
    for f in &scan.left {
        tracing::debug!("import: left on the card: {}", f.display());
    }
}

/// "1 frame", "3 frames", in English, for the log.
pub fn frames(n: usize) -> String {
    format!("{n} frame{}", if n == 1 { "" } else { "s" })
}

/// The status line while frame `index` (from zero) is copied.
pub fn progress_line(index: usize, total: usize, name: &str) -> String {
    tr!(
        "importing {} of {total}: {name}",
        index + 1,
        total = total,
        name = name
    )
}

/// One read of a copy, or of a hash: a beat goes after each. A share
/// on a slow link took up to 4.8 s over a 1 MB read, so a read is a
/// quarter of that; on a local disk the two read and hash alike.
const CHUNK: usize = 256 << 10;

/// How much of a copy to a network mount is written between syncs, at
/// first. A share's client takes the writes into its cache at once and
/// sends them at the sync, so a copy synced once at its end says
/// nothing for as long as the whole file takes over the link (41 s for
/// 100 MB from a laptop); synced in steps, it beats after each. The
/// step follows the link ([`next_step`]): a second or so a sync, from
/// [`STEP_LEAST`] to [`STEP_MOST`]. Local copies sync once: stepped,
/// they wrote two to four times slower on an SSD.
const STEP_FIRST: u64 = 1 << 20;
const STEP_LEAST: u64 = 256 << 10;
const STEP_MOST: u64 = 2 << 20;

/// The step after a stepped sync of `step` bytes that took `took`:
/// half when it took over 3 s, twice when under 1 s, within the
/// bounds. 256 KB is about 9 s at 0.25 Mbit/s, inside a copy's wait;
/// 2 MB, under a second at 20 Mbit/s, is still one step inside the
/// wait if the link drops fifteenfold in the middle of a file.
fn next_step(step: u64, took: Duration) -> u64 {
    if took > Duration::from_secs(3) {
        (step / 2).max(STEP_LEAST)
    } else if took < Duration::from_secs(1) {
        (step * 2).min(STEP_MOST)
    } else {
        step
    }
}

/// BLAKE3 over every byte of a file. The index's hash is the head and
/// the size, which is enough to know a file again but cannot see a
/// copy whose tail never landed; the verify has to read it all.
pub fn hash_whole(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    hash_reader(&mut file).map(|(h, _)| h)
}

/// [`hash_whole`], calling `beat` at every chunk read, with how many
/// bytes were read.
pub fn hash_whole_beating(path: &Path, beat: &dyn Fn()) -> std::io::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    hash_reader_beating(&mut file, beat)
}

fn hash_reader(r: &mut dyn Read) -> std::io::Result<(String, u64)> {
    hash_reader_beating(r, &|| {})
}

/// [`hash_reader`], calling `beat` at every chunk read: a reader on a
/// share is told from one that stopped answering by it.
fn hash_reader_beating(r: &mut dyn Read, beat: &dyn Fn()) -> std::io::Result<(String, u64)> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    let mut n = 0u64;
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => {
                hasher.update(&buf[..k]);
                n += k as u64;
                beat();
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok((hasher.finalize().to_hex().to_string(), n))
}

/// The ending every temporary this program writes has.
const TEMPORARY: &str = ".greycard-import";

/// A temporary name for a file on its way to `to`, beside it: hidden on
/// Linux and macOS, never a camera's name, and this process's own
/// (its id and a counter), so two imports never share one.
#[cfg(test)]
pub fn temporary(to: &Path) -> PathBuf {
    temporary_ending(to, TEMPORARY)
}

/// [`temporary`] with an ending of the caller's: a backup's temporaries
/// are its own, and its sweep never takes an import's.
pub fn temporary_ending(to: &Path, ending: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = name_of(to);
    to.with_file_name(format!(".{name}.{}-{n}{ending}", std::process::id()))
}

/// The process id a temporary's name carries, `.NAME.PID-N.greycard-import`.
fn temporary_pid(name: &str) -> Option<u32> {
    temporary_pid_ending(name, TEMPORARY)
}

/// [`temporary_pid`] for a temporary with `ending`.
pub fn temporary_pid_ending(name: &str, ending: &str) -> Option<u32> {
    let rest = name.strip_suffix(ending)?;
    let (_, last) = rest.rsplit_once('.')?;
    last.split_once('-')?.0.parse().ok()
}

/// Whether process `pid` is running. Unix asks with signal 0; elsewhere
/// it cannot be told cheaply and a temporary is judged by its age alone.
pub fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        if pid == std::process::id() {
            return true;
        }
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: signal 0 sends nothing; it only asks whether the
        // process is there.
        let r = unsafe { libc::kill(pid, 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        pid == std::process::id()
    }
}

/// Remove this program's temporaries in `folder` last written more than
/// a day ago by a process no longer running: what a process killed
/// mid-file leaves. The card's time is put on a file only after it has
/// its name, so a temporary's time is when it was written; the day and
/// the process check each keep another import's temporary, being
/// written this moment, from being taken.
pub fn sweep(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.starts_with('.') && name.ends_with(TEMPORARY)) {
            continue;
        }
        if temporary_pid(&name).is_some_and(alive) {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age > day);
        if old && e.file_type().is_ok_and(|t| t.is_file()) {
            match std::fs::remove_file(e.path()) {
                Ok(()) => tracing::info!("import: removed a stale {}", e.path().display()),
                Err(err) => tracing::warn!("import: {}: {err}", e.path().display()),
            }
        }
    }
}

/// Rename `from` to `to` only when nothing is at `to`: an error of kind
/// `AlreadyExists` otherwise, and `to` untouched. `std::fs::rename`
/// replaces on every platform, so a check before it leaves a moment in
/// which a file could appear and be written over; this has none where
/// the platform can say it at once: `renameat2(RENAME_NOREPLACE)` on
/// Linux, `renamex_np(RENAME_EXCL)` on macOS, `MoveFileExW` without
/// `MOVEFILE_REPLACE_EXISTING` on Windows. Where the file system refuses
/// the flag (a FUSE mount without rename2, a macOS volume that refuses
/// RENAME_EXCL), a hard link then an unlink, which also cannot replace;
/// and where it has no hard links either, the check and the rename, the
/// moment and all. Says which it took, for the log.
/// A path as MoveFileExW wants it, null-terminated: absolute, and
/// with the `\\?\` prefix a path over 260 characters needs. The
/// standard library puts the prefix on for its own calls and not for
/// one made by hand, so without this a long import name got "cannot
/// find the path" at the rename after the copy was made.
#[cfg(windows)]
fn verbatim(p: &Path) -> std::io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Component, Prefix};
    // Absolute resolves `.` and `..` and turns every slash, which a
    // verbatim path requires since nothing is parsed after the prefix.
    let p = std::path::absolute(p)?;
    let prefix = match p.components().next() {
        Some(Component::Prefix(c)) => c.kind(),
        _ => return Err(std::io::Error::other("no prefix on an absolute path")),
    };
    let mut wide: Vec<u16> = match prefix {
        Prefix::Verbatim(_)
        | Prefix::VerbatimUNC(..)
        | Prefix::VerbatimDisk(_)
        | Prefix::DeviceNS(_) => p.as_os_str().encode_wide().collect(),
        Prefix::UNC(..) => {
            // `\\server\share\...` is `\\?\UNC\server\share\...`.
            let rest: Vec<u16> = p.as_os_str().encode_wide().skip(2).collect();
            r"\\?\UNC\".encode_utf16().chain(rest).collect()
        }
        Prefix::Disk(_) => r"\\?\"
            .encode_utf16()
            .chain(p.as_os_str().encode_wide())
            .collect(),
    };
    wide.push(0);
    Ok(wide)
}

pub fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<&'static str> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
        let (f, t) = (verbatim(from)?, verbatim(to)?);
        // SAFETY: both are null-terminated UTF-16 paths that outlive
        // the call, which only reads them. No flags: no replacing and
        // no copying across volumes.
        let moved = unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), 0) };
        if moved != 0 {
            Ok("MoveFileExW")
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(windows))]
    {
        #[cfg(any(all(target_os = "linux", target_env = "gnu"), target_os = "macos"))]
        {
            use std::os::unix::ffi::OsStrExt;
            let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_bytes());
            let (f, t) = (c(from)?, c(to)?);
            // SAFETY: both are null-terminated paths that outlive the
            // call, which only reads them.
            #[cfg(target_os = "linux")]
            let r = unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    f.as_ptr(),
                    libc::AT_FDCWD,
                    t.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            // SAFETY: as above.
            #[cfg(target_os = "macos")]
            let r = unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) };
            if r == 0 {
                return Ok(if cfg!(target_os = "macos") {
                    "renamex_np(RENAME_EXCL)"
                } else {
                    "renameat2(RENAME_NOREPLACE)"
                });
            }
            let e = std::io::Error::last_os_error();
            let unsupported = matches!(
                e.raw_os_error(),
                Some(libc::EINVAL) | Some(libc::ENOSYS) | Some(libc::ENOTSUP)
            );
            if !unsupported {
                return Err(e);
            }
        }
        match std::fs::hard_link(from, to) {
            Ok(()) => std::fs::remove_file(from).map(|()| "a hard link and an unlink"),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
            Err(_) => {
                if std::fs::symlink_metadata(to).is_ok() {
                    return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
                }
                std::fs::rename(from, to).map(|()| "a check and a rename")
            }
        }
    }
}

/// Ask the system to let go of what it holds in memory of `file`, so
/// that a read after it comes from the drive and not from the pages
/// just written. Linux only; the macOS read turns its cache off
/// instead ([`open_uncached`]), and Windows offers nothing short of
/// unbuffered reads in sector-sized pieces, so there the read-back
/// checks the file system's copy.
fn drop_cached(file: &std::fs::File) {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: the descriptor is open for as long as `file` is, and
        // the call only advises the kernel.
        unsafe {
            libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = file;
}

/// `path` opened for a read that bypasses the cache where the system
/// allows it per file (macOS's `F_NOCACHE`).
fn open_uncached(path: &Path) -> std::io::Result<std::fs::File> {
    let file = std::fs::File::open(path)?;
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: the descriptor is open for as long as `file` is.
        unsafe {
            libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1);
        }
    }
    Ok(file)
}

/// Copy `from` to `to` through a [`temporary`], and name it only once
/// it reads back with the hash its bytes had going in; that hash is
/// returned with the byte count. `want` is the hash the bytes must
/// have (a backup is made from the destination's copy and checked
/// against the card's hash). The file keeps `from`'s modification
/// time. On any failure the temporary is gone and `to` untouched.
pub fn land(from: &Path, to: &Path, want: Option<&str>) -> Result<(String, u64)> {
    let mut src = std::fs::File::open(from).context(tr!("reading it"))?;
    let meta = src.metadata().context(tr!("reading it"))?;
    land_from(&mut src, meta.modified().ok(), Some(meta.len()), to, want)
}

/// [`land`] from any reader, so a read that fails halfway is tested.
/// `length`, when given, is how many bytes the file says it has: a read
/// that ends short of it is an error, not a smaller file.
pub fn land_from(
    src: &mut dyn Read,
    modified: Option<std::time::SystemTime>,
    length: Option<u64>,
    to: &Path,
    want: Option<&str>,
) -> Result<(String, u64)> {
    land_with(src, modified, length, to, want, &Landing::default())
}

/// How [`land_with`] lands a file beyond the import's way.
pub struct Landing<'a> {
    /// The temporary's ending: [`TEMPORARY`] for an import.
    pub ending: &'a str,
    /// Go over a file already at the name (a backup's older sidecar);
    /// otherwise one there is an error and left.
    pub replace: bool,
    /// Called at every chunk written or read back, and after each sync.
    pub beat: &'a dyn Fn(),
    /// Counts the bytes written, for a bar.
    pub bytes: Option<&'a AtomicU64>,
    /// The copy goes to a network mount: synced in steps as it is
    /// written ([`next_step`]), a beat after each.
    pub far: bool,
    /// A test's hand on the temporary once written, before it is read
    /// back.
    #[cfg(test)]
    pub tamper: Option<&'a dyn Fn(&Path)>,
    /// A test's hand before each sync, with the bytes written since the
    /// last: a share's slow upload. What it returns, when anything, is
    /// how long the sync is taken to have been, in place of the clock.
    #[cfg(test)]
    pub on_sync: Option<&'a dyn Fn(u64) -> Option<Duration>>,
}

impl Default for Landing<'_> {
    fn default() -> Self {
        Landing {
            ending: TEMPORARY,
            replace: false,
            beat: &|| {},
            bytes: None,
            far: false,
            #[cfg(test)]
            tamper: None,
            #[cfg(test)]
            on_sync: None,
        }
    }
}

/// [`land_from`], the one copy the import and a backup share: a
/// temporary beside `to`, the bytes hashed as they stream, the
/// temporary read back past the cache and hashed again, and named `to`
/// only when the two agree; `to` takes `modified`.
pub fn land_with(
    src: &mut dyn Read,
    modified: Option<std::time::SystemTime>,
    length: Option<u64>,
    to: &Path,
    want: Option<&str>,
    how: &Landing<'_>,
) -> Result<(String, u64)> {
    let folder = to.parent().context(tr!("no folder to write in"))?;
    std::fs::create_dir_all(folder).with_context(|| tr!("making {}", folder.display()))?;
    let tmp = temporary_ending(to, how.ending);
    let mut made = false;
    let result = (|| -> Result<(String, u64)> {
        // Made new, so a file or a link already at the name is an
        // error rather than something followed and cut short.
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .context(tr!("writing the copy"))?;
        made = true;
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; CHUNK];
        let mut bytes = 0u64;
        let mut unsynced = 0u64;
        let mut step = STEP_FIRST;
        // Each test's hand gets the bytes a sync has to send, and may
        // say how long it took.
        #[cfg(test)]
        let before_sync = |n: u64| how.on_sync.and_then(|hand| hand(n));
        #[cfg(not(test))]
        let before_sync = |_: u64| None::<Duration>;
        loop {
            let k = match src.read(&mut buf) {
                Ok(0) => break,
                Ok(k) => k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(anyhow::Error::new(e).context(tr!("reading it"))),
            };
            hasher.update(&buf[..k]);
            out.write_all(&buf[..k]).context(tr!("writing the copy"))?;
            bytes += k as u64;
            unsynced += k as u64;
            if let Some(counted) = how.bytes {
                counted.fetch_add(k as u64, Ordering::Relaxed);
            }
            (how.beat)();
            // Only pushes the bytes along: the sync that makes the
            // file whole on the disk is the one below. The beat after
            // it says the sync is over before the next read begins.
            if how.far && unsynced >= step {
                let began = Instant::now();
                let said = before_sync(unsynced);
                out.sync_data().context(tr!("writing the copy"))?;
                step = next_step(step, said.unwrap_or_else(|| began.elapsed()));
                unsynced = 0;
                (how.beat)();
            }
        }
        if let Some(length) = length
            && bytes != length
        {
            bail!(tr!(
                "read {bytes} bytes of the {length} it has",
                bytes = bytes,
                length = length
            ));
        }
        // On the disk before it is read back and named: what a copy
        // that has landed means.
        let _ = before_sync(unsynced);
        out.sync_all().context(tr!("writing the copy"))?;
        (how.beat)();
        drop_cached(&out);
        drop(out);
        let hash = hasher.finalize().to_hex().to_string();
        if let Some(want) = want
            && want != hash
        {
            bail!(tr!("the copy it was made from no longer matches the card"));
        }
        #[cfg(test)]
        if let Some(tamper) = how.tamper {
            tamper(&tmp);
        }
        let (back, _) = open_uncached(&tmp)
            .and_then(|mut f| hash_reader_beating(&mut f, how.beat))
            .context(tr!("reading the copy back"))?;
        if back != hash {
            bail!(tr!("the copy does not read back the same"));
        }
        if how.replace {
            std::fs::rename(&tmp, to).context(tr!("naming the copy"))?;
        } else {
            let how = rename_noreplace(&tmp, to).map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    anyhow::anyhow!(tr!("{} appeared while it was copied", to.display()))
                } else {
                    anyhow::Error::new(e).context(tr!("naming the copy"))
                }
            })?;
            say_rename(how);
        }
        // The card's time, now that the file has its name: on the
        // temporary it would make a file being written look days old
        // to another import's sweep.
        if let Some(when) = modified
            && let Ok(file) = std::fs::File::options().write(true).open(to)
        {
            let _ = file.set_modified(when);
        }
        Ok((hash, bytes))
    })();
    if result.is_err() && made {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The hash of a file's last 64 KiB (all of it, when it is shorter)
/// and its size: with the head hash, what a frame already there is
/// known by. The tail catches a copy preallocated and never finished,
/// and one whose last bytes changed.
pub fn hash_tail(path: &Path) -> std::io::Result<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let n = size.min(greycard_library::hash::HEAD as u64);
    file.seek(SeekFrom::Start(size - n))?;
    let mut tail = vec![0u8; n as usize];
    file.read_exact(&mut tail)?;
    Ok(greycard_library::hash::hash_bytes(&tail, size))
}

/// Whether `copy` holds `card`'s bytes, as a frame already there is
/// judged: the same size, head and tail (64 KiB each), or with
/// `verify`, every byte. A re-run over a card imported before reads
/// 128 KiB a frame off the card rather than the whole card; a
/// corruption in the middle of a file that was verified when it was
/// first imported is what `verify` is for, not what a re-run looks for.
pub fn same_bytes(card: &Path, copy: &Path, verify: bool) -> std::io::Result<bool> {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len());
    if size(card)? != size(copy)? {
        return Ok(false);
    }
    if greycard_library::hash_file(card)? != greycard_library::hash_file(copy)? {
        return Ok(false);
    }
    Ok(if verify {
        hash_whole(card)? == hash_whole(copy)?
    } else {
        hash_tail(card)? == hash_tail(copy)?
    })
}

/// Log the rename a copy took, once a process for each kind.
fn say_rename(how: &'static str) {
    static SAID: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if !said.contains(&how) {
        said.push(how);
        tracing::info!("import: files are named by {how}");
    }
}

/// A frame on the card, its hashes read at most once each: the head's
/// (the index's key, 64 KiB and the size), the tail's, and the whole
/// file's, which a frame already there needs only with `verify`.
struct CardFile<'a> {
    path: &'a Path,
    size: u64,
    head: Option<String>,
    tail: Option<String>,
    whole: Option<String>,
}

impl<'a> CardFile<'a> {
    fn new(path: &'a Path, size: u64) -> Self {
        Self {
            path,
            size,
            head: None,
            tail: None,
            whole: None,
        }
    }

    fn tail(&mut self) -> Result<String> {
        if self.tail.is_none() {
            self.tail = Some(hash_tail(self.path).context(tr!("reading it"))?);
        }
        Ok(self.tail.clone().unwrap_or_default())
    }

    /// The rest of the comparison once the head matched: the tail, or
    /// with `verify` the whole file, against `copy`'s.
    fn rest_matches(&mut self, copy: &Path, verify: bool) -> Result<bool> {
        Ok(if verify {
            hash_whole(copy).ok() == Some(self.whole()?)
        } else {
            hash_tail(copy).ok() == Some(self.tail()?)
        })
    }

    fn head(&mut self) -> Result<String> {
        if self.head.is_none() {
            self.head = Some(greycard_library::hash_file(self.path).context(tr!("reading it"))?);
        }
        Ok(self.head.clone().unwrap_or_default())
    }

    fn whole(&mut self) -> Result<String> {
        if self.whole.is_none() {
            self.whole = Some(hash_whole(self.path).context(tr!("reading it"))?);
        }
        Ok(self.whole.clone().unwrap_or_default())
    }
}

/// What is under the destination already, by size, so a frame is only
/// hashed against the files that could be it: first by the head, which
/// reads 64 KiB, and only on a head that matches by the tail (or with
/// `verify` the whole file). Each file's hashes are read at most once.
struct Existing {
    by_size: HashMap<u64, Vec<PathBuf>>,
    heads: HashMap<PathBuf, String>,
    wholes: HashMap<PathBuf, String>,
    verify: bool,
}

impl Existing {
    fn under(root: &Path) -> Self {
        let mut files = Vec::new();
        if root.is_dir()
            && let Err(e) = walk(root, &mut files)
        {
            tracing::warn!("import: {}: {e}", root.display());
        }
        let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
        for f in files {
            if let Ok(m) = std::fs::metadata(&f) {
                by_size.entry(m.len()).or_default().push(f);
            }
        }
        Self {
            by_size,
            heads: HashMap::new(),
            wholes: HashMap::new(),
            verify: false,
        }
    }

    fn files(&self) -> usize {
        self.by_size.values().map(Vec::len).sum()
    }

    /// A file here with the card file's bytes, if any.
    fn holds(&mut self, card: &mut CardFile<'_>) -> Result<Option<PathBuf>> {
        let candidates = self.by_size.get(&card.size).cloned().unwrap_or_default();
        for f in candidates {
            let head = match self.heads.get(&f) {
                Some(h) => h.clone(),
                None => match greycard_library::hash_file(&f) {
                    Ok(h) => {
                        self.heads.insert(f.clone(), h.clone());
                        h
                    }
                    Err(_) => continue,
                },
            };
            if head != card.head()? {
                continue;
            }
            if !self.verify {
                if card.rest_matches(&f, false)? {
                    return Ok(Some(f));
                }
                continue;
            }
            let whole = match self.wholes.get(&f) {
                Some(h) => h.clone(),
                None => match hash_whole(&f) {
                    Ok(h) => {
                        self.wholes.insert(f.clone(), h.clone());
                        h
                    }
                    Err(_) => continue,
                },
            };
            if whole == card.whole()? {
                return Ok(Some(f));
            }
        }
        Ok(None)
    }

    fn add(&mut self, path: PathBuf, size: u64, whole: String) {
        self.by_size.entry(size).or_default().push(path.clone());
        self.wholes.insert(path, whole);
    }
}

/// `p` as the file system has it: absolute, its nearest ancestor that
/// is there made canonical (links and `..` resolved, no Windows `\\?\`
/// prefix) and the part that is not there yet put back after it. So a
/// destination that does not exist yet is compared with the card the
/// same way as one that does.
pub fn resolve(p: &Path) -> PathBuf {
    let absolute = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    };
    let mut missing = Vec::new();
    let mut at = absolute.as_path();
    loop {
        if let Ok(mut there) = dunce::canonicalize(at) {
            for name in missing.iter().rev() {
                there.push(name);
            }
            return there;
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_os_string());
                at = parent;
            }
            // A `..` after a folder that is not there: nothing on disk
            // to resolve it by, so it is taken as written.
            _ => {
                let plain = lexical(&absolute);
                return if plain == absolute {
                    absolute
                } else {
                    resolve(&plain)
                };
            }
        }
    }
}

/// `p` with `.` dropped and each `..` taking the folder before it, by
/// the letters alone.
fn lexical(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// What of the source's disk the import keeps from: the card, the
/// nearest folder at or above the source that holds a `DCIM` folder (a
/// camera card's root, so a destination beside `DCIM` is on the card as
/// much as one under it); with none, the source folder itself. The
/// search stops at the mount point the source is on, and never looks
/// at the home folder or above it: a phone-sync tool's `~/DCIM` must
/// not make everything under home a card. A folder of pictures on a
/// data drive or an external SSD is not a card, and a destination
/// beside it on the same drive is taken.
pub fn card_root(source: &Path) -> PathBuf {
    let source = resolve(source);
    let home = dirs::home_dir().map(|h| resolve(&h));
    dcim_root(&source, home.as_deref()).unwrap_or(source)
}

/// The nearest folder at or above `p` holding a `DCIM` folder, looking
/// no higher than `p`'s mount point and never at `home` or above it.
pub fn dcim_root(p: &Path, home: Option<&Path>) -> Option<PathBuf> {
    for a in p.ancestors() {
        if home.is_some_and(|h| h.starts_with(a)) {
            return None;
        }
        if a.join("DCIM").is_dir() {
            return Some(a.to_path_buf());
        }
        if is_mount_point(a) {
            return None;
        }
    }
    None
}

/// Whether `a` is where a file system is mounted: its device differs
/// from its parent's (the root of a drive on Windows).
#[cfg(unix)]
fn is_mount_point(a: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let dev = |p: &Path| std::fs::metadata(p).map(|m| m.dev()).ok();
    match a.parent() {
        None => true,
        Some(parent) => dev(a) != dev(parent),
    }
}

#[cfg(not(unix))]
fn is_mount_point(a: &Path) -> bool {
    a.parent().is_none()
}

/// Whether `p` is a file on a camera card, whichever card.
fn on_a_card(p: &Path) -> bool {
    let home = dirs::home_dir().map(|h| resolve(&h));
    p.parent()
        .is_some_and(|parent| dcim_root(parent, home.as_deref()).is_some())
}

/// Whether `a` and `b` (or the nearest folders above them that are
/// there) are on one drive: a backup there is no backup when the drive
/// fails. None when it cannot be told.
pub fn same_device(a: &Path, b: &Path) -> Option<bool> {
    let there = |p: &Path| {
        let p = resolve(p);
        p.ancestors().find(|a| a.exists()).map(Path::to_path_buf)
    };
    let (a, b) = (there(a)?, there(b)?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let dev = |p: &Path| std::fs::metadata(p).ok().map(|m| m.dev());
        Some(dev(&a)? == dev(&b)?)
    }
    #[cfg(not(unix))]
    {
        let root = |p: &Path| {
            p.components()
                .next()
                .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        };
        Some(root(&a)? == root(&b)?)
    }
}

/// Refuse what would write on the card or into the source, and a
/// pattern that makes no name. Nothing is made or written by it.
pub fn check(opts: &Options) -> Result<()> {
    anyhow::ensure!(
        opts.source.is_dir(),
        tr!("the source {} is not a folder", opts.source.display())
    );
    anyhow::ensure!(
        !opts.destination.as_os_str().is_empty(),
        tr!("no destination is chosen")
    );
    let card = card_root(&opts.source);
    let source = resolve(&opts.source);
    // The destination, then the backup: each sentence whole, a place
    // a case.
    let mut places = vec![(false, resolve(&opts.destination))];
    if let Some(b) = &opts.backup {
        places.push((true, resolve(b)));
    }
    for (backup, place) in &places {
        anyhow::ensure!(
            !source.starts_with(place),
            if *backup {
                tr!(
                    "the backup {} holds the source {}",
                    place.display(),
                    source.display()
                )
            } else {
                tr!(
                    "the destination {} holds the source {}",
                    place.display(),
                    source.display()
                )
            }
        );
        anyhow::ensure!(
            !place.starts_with(&card),
            if *backup {
                tr!(
                    "the backup {} is on the card ({})",
                    place.display(),
                    card.display()
                )
            } else {
                tr!(
                    "the destination {} is on the card ({})",
                    place.display(),
                    card.display()
                )
            }
        );
    }
    if let [(_, dest), (_, backup)] = places.as_slice() {
        anyhow::ensure!(
            !backup.starts_with(dest) && !dest.starts_with(backup),
            tr!("the backup is the destination, or inside it")
        );
    }
    naming::folders(&opts.subfolder, &sample())?;
    naming::file_name(&opts.name, &sample(), "CR3")?;
    Ok(())
}

fn sample() -> Fields {
    Fields {
        day: Day {
            year: 2026,
            month: 1,
            day: 1,
        },
        name: "IMG_0001".into(),
        camera: String::new(),
        seq: 1,
        folder: "100CANON".into(),
    }
}

/// What a run keeps between frames.
struct Run<'a> {
    opts: &'a Options,
    card: PathBuf,
    existing: Existing,
    library: Option<greycard_library::Library>,
    /// The names this run has given, case-blind as two of the three
    /// platforms' file systems are, so two frames never meet.
    given: HashSet<String>,
    /// The folders this run has swept of stale temporaries.
    swept: HashSet<PathBuf>,
}

fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

impl Run<'_> {
    /// [`land`], the folder swept of stale temporaries first.
    fn land(&mut self, from: &Path, to: &Path, want: Option<&str>) -> Result<(String, u64)> {
        if let Some(folder) = to.parent()
            && self.swept.insert(folder.to_path_buf())
        {
            sweep(folder);
        }
        land(from, to, want)
    }

    /// `planned`, or `-1`, `-2` after it until the name is neither
    /// on disk nor given in this run; and whether a file that was on
    /// disk before this run moved it.
    fn free_name(&mut self, planned: &Path, reserve: usize) -> (PathBuf, bool) {
        let mut out = planned.to_path_buf();
        let mut n = 1;
        let mut moved = false;
        loop {
            let on_disk = std::fs::symlink_metadata(&out).is_ok();
            let ours = self.given.contains(&key(&out));
            if !on_disk && !ours {
                break;
            }
            // A name this run gave another frame is not "another
            // file's"; one that was there before it is.
            moved |= on_disk && !ours;
            out = numbered(planned, n, reserve);
            n += 1;
        }
        self.given.insert(key(&out));
        (out, moved)
    }

    /// The library's copy of the card file, if it holds one with the
    /// same bytes: found by the head hash, then compared by the tail
    /// (or with `verify` the whole file), so a copy cut short or changed
    /// at its end is not taken for the frame. A row on a camera card (this one, or the
    /// other slot of a two-slot body, browsed in the editor) is not an
    /// import.
    fn in_library(&mut self, card: &mut CardFile<'_>) -> Result<Option<PathBuf>> {
        let Some(lib) = &self.library else {
            return Ok(None);
        };
        let rows = lib.by_hash(&card.head()?).unwrap_or_default();
        for row in rows {
            if row.missing || row.size != card.size || !row.path.is_file() {
                continue;
            }
            let path = resolve(&row.path);
            if path.starts_with(&self.card) || on_a_card(&path) {
                continue;
            }
            if card.rest_matches(&path, self.opts.verify)? {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }
}

/// Run the import over `scan`'s frames. `progress` hears each frame as
/// it begins, by its index and name. Stops at the first error, which
/// the report names; a cancel takes effect before the next frame.
pub fn run(
    opts: &Options,
    scan: &Scan,
    job: &Job,
    mut progress: impl FnMut(usize, usize, &str),
) -> Report {
    let started = std::time::Instant::now();
    let total = scan.items.len();
    let mut report = Report {
        total,
        unreadable: scan.unreadable,
        ..Report::default()
    };
    // Compared as the file system has them from here on: a library
    // row's path is canonical.
    let mut opts = opts.clone();
    opts.destination = resolve(&opts.destination);
    opts.backup = opts.backup.as_deref().map(resolve);
    let looked = std::time::Instant::now();
    let mut existing = Existing::under(&opts.destination);
    existing.verify = opts.verify;
    tracing::info!(
        "import: {} files under {} looked over in {:.2} s",
        existing.files(),
        opts.destination.display(),
        looked.elapsed().as_secs_f64()
    );
    let library = opts
        .library
        .as_deref()
        .filter(|p| p.exists())
        .and_then(|p| {
            greycard_library::Library::open_read_only(p)
                .map_err(|e| tracing::warn!("import: the library at {}: {e}", p.display()))
                .ok()
        });
    let mut run = Run {
        opts: &opts,
        card: card_root(&opts.source),
        existing,
        library,
        given: HashSet::new(),
        swept: HashSet::new(),
    };
    for (i, item) in scan.items.iter().enumerate() {
        if job.is_canceled() {
            report.canceled = true;
            break;
        }
        progress(i, total, &name_of(&item.file));
        if let Err(e) = import_one(&mut run, item, i + 1, &mut report) {
            report.error = Some((item.file.clone(), format!("{e:#}")));
            break;
        }
    }
    report.seconds = started.elapsed().as_secs_f64();
    report
}

fn import_one(run: &mut Run<'_>, item: &Item, seq: usize, report: &mut Report) -> Result<()> {
    let opts = run.opts;
    let (mut fields, body) = probe_file(&item.file, seq);
    fields.folder = folder_under(&opts.source, &item.file);
    let reserve = companion_reserve(item);
    let rel = relative_within(opts, &item.file, &fields, reserve)?;
    let planned = opts.destination.join(&rel);
    let size = std::fs::metadata(&item.file)
        .context(tr!("reading it"))?
        .len();
    let mut card = CardFile::new(&item.file, size);

    // Imported before: the same bytes under the destination, under any
    // name, or a copy the library knows of elsewhere.
    let there = match run.existing.holds(&mut card)? {
        Some(p) => Some(p),
        None => run.in_library(&mut card)?,
    };
    let (frame, hash, new) = match there {
        Some(there) => {
            tracing::info!(
                "import: {} is there already as {}",
                item.file.display(),
                there.display()
            );
            report.already += 1;
            if !there.starts_with(&opts.destination) {
                // Somewhere this import does not write: its JPEG, its
                // XMP and its backup are that copy's business.
                return Ok(());
            }
            run.given.insert(key(&there));
            // A backup made from it is checked against the card's hash
            // when Verify read the card whole, else against its own
            // bytes as they are read; nothing is read here for it.
            (there, card.whole.clone(), false)
        }
        None => {
            let (to, moved) = run.free_name(&planned, reserve);
            if moved {
                tracing::warn!(
                    "import: {} is another file; {} goes in as {}",
                    planned.display(),
                    item.file.display(),
                    name_of(&to)
                );
                report.renamed += 1;
            }
            let (hash, bytes) = run.land(&item.file, &to, None)?;
            if let Some(before) = &card.whole
                && *before != hash
            {
                // Read twice and different: the card is not giving the
                // same bytes back.
                let _ = std::fs::remove_file(&to);
                bail!(tr!("the card read differently twice"));
            }
            run.existing.add(to.clone(), bytes, hash.clone());
            report.files += 1;
            report.bytes += bytes;
            (to, Some(hash), true)
        }
    };
    // What the backup takes: the frame, and each companion that is
    // there now with the card's bytes.
    let mut copies = vec![(frame.clone(), hash)];

    for companion in &item.companions {
        let c_to = frame.with_file_name(companion_name(&item.file, &frame, companion));
        if std::fs::symlink_metadata(&c_to).is_ok() {
            if same_bytes(companion, &c_to, opts.verify).unwrap_or(false) {
                copies.push((c_to, None));
            } else {
                tracing::warn!(
                    "import: {} not copied: {} is another file",
                    companion.display(),
                    c_to.display()
                );
                report.companions_left += 1;
            }
            continue;
        }
        if !run.given.insert(key(&c_to)) {
            report.companions_left += 1;
            continue;
        }
        let (h, b) = run
            .land(companion, &c_to, None)
            .with_context(|| tr!("{} (with {})", name_of(companion), name_of(&item.file)))?;
        run.existing.add(c_to.clone(), b, h.clone());
        report.files += 1;
        report.bytes += b;
        copies.push((c_to, Some(h)));
    }

    if let Some(backup) = &opts.backup {
        back_up(run, backup, &copies, reserve, report)?;
    }

    if !new {
        return Ok(());
    }
    // A Lightroom develop in the XMP that came with it, taken before the
    // preset, which goes over it.
    if let Some(placement) = opts.placement {
        let mut sidecar = greycard_edit::Sidecar::load_as_file(&frame)
            .ok()
            .flatten()
            .unwrap_or_default();
        if greycard_edit_io::lightroom::beside::take_and_save(&frame, &mut sidecar, placement)
            .is_some_and(|t| t.saved)
        {
            report.lightroom += 1;
        }
    }
    if let (Some(preset), Some(placement)) = (&opts.preset, opts.placement) {
        let (preset, left_off) =
            crate::preset_fit::preset_fitted(preset, &opts.profiles, || body.clone());
        if left_off {
            tracing::warn!(
                "import: {}: the preset's camera profile was made for another camera, left off",
                name_of(&frame)
            );
            report.profile_left_off += 1;
        }
        // A `.gcd` that will not read is never written over, nor a newer
        // build's (§724): the preset is not laid on it.
        match greycard_edit::Sidecar::load_as_file(&frame) {
            Ok(sidecar) => {
                let mut sidecar = sidecar.unwrap_or_default();
                crate::preset_fit::preset_at_start(
                    &mut sidecar,
                    None,
                    &frame,
                    &preset,
                    Some(placement),
                );
                if sidecar.current_label().is_some() && !sidecar.read_only() {
                    report.preset += 1;
                }
            }
            Err(e) => {
                tracing::warn!(
                    "import: {}: sidecar: {e}; the preset is not laid",
                    name_of(&frame)
                );
            }
        }
    }
    report.frames += 1;
    if let Some(folder) = frame.parent() {
        report.landed_in(folder);
    }
    Ok(())
}

/// The frame and its companions (`copies`, the frame first, each with
/// the card's hash when it is known) copied to the backup under the
/// same folders and names, from the destination's copies: the card is
/// the slow disk and may be the dying one. A name the backup already
/// holds with these bytes (by size, head and tail, or every byte with
/// Verify) is done; one holding other bytes moves the frame on to
/// `-1`, `-2`, stopping at the first that is free or holds these
/// bytes, so a re-run never makes another copy. The companions follow
/// the frame's backup name, as they follow its name in the destination.
fn back_up(
    run: &mut Run<'_>,
    backup: &Path,
    copies: &[(PathBuf, Option<String>)],
    reserve: usize,
    report: &mut Report,
) -> Result<()> {
    let opts = run.opts;
    let Some((frame, frame_hash)) = copies.first() else {
        return Ok(());
    };
    let rel = frame.strip_prefix(&opts.destination).unwrap_or(frame);
    let planned = backup.join(rel);
    let mut b_frame = planned.clone();
    let mut n = 1;
    let held = loop {
        if std::fs::symlink_metadata(&b_frame).is_err() {
            break false;
        }
        if same_bytes(frame, &b_frame, opts.verify).unwrap_or(false) {
            break true;
        }
        b_frame = numbered(&planned, n, reserve);
        n += 1;
    };
    if b_frame != planned {
        tracing::warn!(
            "import: the backup's {} is another file; {} is backed up as {}",
            planned.display(),
            name_of(frame),
            name_of(&b_frame)
        );
    }
    if !held {
        run.land(frame, &b_frame, frame_hash.as_deref())
            .context(tr!("the backup copy"))?;
        report.backed_up += 1;
    }
    for (file, hash) in &copies[1..] {
        let b_to = b_frame.with_file_name(companion_name(frame, &b_frame, file));
        if std::fs::symlink_metadata(&b_to).is_ok() {
            if !same_bytes(file, &b_to, opts.verify).unwrap_or(false) {
                tracing::warn!(
                    "import: the backup's {} is another file; {} is not backed up",
                    b_to.display(),
                    name_of(file)
                );
                report.companions_left += 1;
            }
            continue;
        }
        run.land(file, &b_to, hash.as_deref())
            .context(tr!("the backup copy"))?;
        report.backed_up += 1;
    }
    Ok(())
}

/// The folders a mounted volume may be under, as each desktop mounts
/// them. On Windows each drive is a volume of its own.
/// `GREYCARD_MOUNTS`, a list of folders in the platform's `PATH`
/// form, is looked under instead when it is set: a card for a test
/// or a snapshot, with nothing mounted.
pub fn volumes() -> Vec<PathBuf> {
    if let Some(list) = std::env::var_os("GREYCARD_MOUNTS") {
        return std::env::split_paths(&list)
            .flat_map(|p| volumes_under(&p))
            .collect();
    }
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        // A and B are floppies, and asking one that is not there can
        // wait on the drive.
        for letter in b'C'..=b'Z' {
            out.push(PathBuf::from(format!("{}:\\", letter as char)));
        }
    }
    #[cfg(not(windows))]
    {
        let mut parents = Vec::new();
        if cfg!(target_os = "macos") {
            parents.push(PathBuf::from("/Volumes"));
        } else {
            if let Some(user) = std::env::var_os("USER") {
                parents.push(Path::new("/run/media").join(&user));
                parents.push(Path::new("/media").join(&user));
            }
            parents.push(PathBuf::from("/media"));
        }
        for parent in parents {
            out.extend(volumes_under(&parent));
        }
    }
    out
}

/// The folders in `parent`, sorted: the volumes mounted there.
pub fn volumes_under(parent: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(parent)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// The first of `volumes` with a camera's `DCIM` folder on it, that
/// folder. Each check is a stat or two, but a stat of a sleeping or
/// network drive can wait: never call this on the window's thread.
pub fn find_card(volumes: &[PathBuf]) -> Option<PathBuf> {
    volumes.iter().find_map(|v| {
        let dcim = v.join("DCIM");
        dcim.is_dir().then_some(dcim)
    })
}

#[cfg(test)]
mod tests;
