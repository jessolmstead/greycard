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

use crate::naming::{self, Day, Fields};
use anyhow::{Context, Result, bail};
use greycard_edit::{Placement, Preset};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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
/// path.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut seen = HashSet::new();
    walk_from(dir, out, &mut seen)
}

fn walk_from(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
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
            Ok(t) if t.is_dir() => walk_from(&path, out, seen)?,
            Ok(t) if t.is_file() => out.push(path),
            _ => {}
        }
    }
    Ok(())
}

/// The frames under `source`, in the order the camera numbered them
/// (by path: `100CANON` before `101CANON`, `IMG_0001` before
/// `IMG_0002`), each raw with its companions. A picture with a raw of
/// the same name in its folder is the camera's JPEG of that raw and
/// goes along with it; a picture with no raw is a frame of its own,
/// as a JPEG-only camera writes them. An XMP goes with the frame it
/// names, `IMG.xmp` or `IMG.CR3.xmp`, and is left when there is none.
/// Two raws of one stem in one folder (`DSCF0001.RAF` and a
/// `DSCF0001.DNG` made from it) are two frames, and the companions of
/// that stem go with the first of them in path order.
pub fn scan(source: &Path) -> Result<Scan> {
    anyhow::ensure!(source.is_dir(), "{} is not a folder", source.display());
    let mut files = Vec::new();
    walk(source, &mut files).with_context(|| format!("reading {}", source.display()))?;
    files.sort();
    let key = |p: &Path| (p.parent().map(Path::to_path_buf), stem_of(p).to_lowercase());
    // The raws by folder and stem: they take the companions.
    let mut raws: HashMap<(Option<PathBuf>, String), usize> = HashMap::new();
    let mut items: Vec<Item> = Vec::new();
    for f in files
        .iter()
        .filter(|f| greycard_core::decode::is_raw_path(f))
    {
        raws.entry(key(f)).or_insert(items.len());
        items.push(Item {
            file: f.clone(),
            companions: Vec::new(),
        });
    }
    let mut left = Vec::new();
    for f in &files {
        if greycard_core::decode::is_raw_path(f) {
            continue;
        }
        let picture = greycard_core::picture::is_picture_path(f);
        let xmp = extension_is(f, "xmp");
        // `IMG.CR3.xmp` names its frame by the whole name.
        let owner = raws.get(&key(f)).copied().or_else(|| {
            xmp.then(|| {
                let inner = f.with_extension("");
                raws.get(&key(&inner)).copied()
            })
            .flatten()
        });
        match owner {
            Some(i) if picture || xmp => items[i].companions.push(f.clone()),
            None if picture => items.push(Item {
                file: f.clone(),
                companions: Vec::new(),
            }),
            _ => left.push(f.clone()),
        }
    }
    // Lone pictures joined the list after the raws; the camera's order
    // is the path's.
    items.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(Scan { items, left })
}

/// The tokens' values for `file`, the `seq`th frame of the run: the
/// day and the camera from its EXIF, the day from its modification
/// time when the EXIF has none. A file the probe cannot read (or that
/// makes the decoder give up) is named from its modification time and
/// no camera, and still imported.
pub fn fields_of(file: &Path, seq: usize) -> Fields {
    probe_file(file, seq).0
}

/// [`fields_of`], and the body (make and model) when the file says,
/// for the preset's camera-profile check.
pub fn probe_file(file: &Path, seq: usize) -> (Fields, Option<(String, String)>) {
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
            Some((p.make, p.model)),
        ),
        _ => (None, String::new(), None),
    };
    let day = taken.unwrap_or_else(|| modified_day(file));
    let fields = Fields {
        day,
        name: stem_of(file),
        camera,
        seq,
    };
    (fields, body)
}

/// The local day a file was last written: what a card's clock stamped
/// it with, near enough, when the EXIF says nothing. On a FAT card the
/// time is stored without a zone and read through the mount's idea of
/// one, so a frame taken near midnight can land on the day either side.
fn modified_day(file: &Path) -> Day {
    use chrono::Datelike;
    let when = std::fs::metadata(file)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| std::time::SystemTime::now());
    let local: chrono::DateTime<chrono::Local> = when.into();
    Day {
        year: local.year(),
        month: local.month(),
        day: local.day(),
    }
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

/// `path` with ` (n)` before its extension, the name kept within
/// [`naming::LONGEST`] less `reserve` however long it was.
fn numbered(path: &Path, n: usize, reserve: usize) -> PathBuf {
    let tail = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let suffix = format!(" ({n})");
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
    /// Frames whose name was another file's, given ` (2)` instead.
    pub renamed: usize,
    /// Companions not copied because their name was another file's.
    pub companions_left: usize,
    /// Frames given the preset, and frames the preset's camera
    /// profile was left off because it was made for another body.
    pub preset: usize,
    pub profile_left_off: usize,
    /// The stop was asked for before the last frame.
    pub canceled: bool,
    /// The file that stopped the import, and why.
    pub error: Option<(PathBuf, String)>,
    pub seconds: f64,
    /// The folders written to and how many frames each took, in the
    /// order they were first written.
    pub folders: Vec<(PathBuf, usize)>,
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
            format!(
                "import stopped: {} of {} imported",
                self.frames,
                frames(self.total)
            )
        } else if self.error.is_some() {
            format!(
                "import failed after {} of {}",
                self.frames,
                frames(self.total)
            )
        } else {
            format!("imported {}", frames(self.frames))
        };
        s.push_str(&format!(" to {}", destination.display()));
        if self.bytes > 0 {
            s.push_str(&format!(
                " ({:.0} MB at {:.0} MB/s)",
                self.bytes as f64 / 1e6,
                self.rate()
            ));
        }
        if self.backed_up > 0 {
            s.push_str(&format!(", {} backed up", self.backed_up));
        }
        if self.already > 0 {
            s.push_str(&format!(", {} already there", self.already));
        }
        if self.renamed > 0 {
            s.push_str(&format!(
                ", {} renamed with (2) (the name was another file's)",
                self.renamed
            ));
        }
        if self.companions_left > 0 {
            s.push_str(&format!(
                ", {} JPEG or XMP not copied (the name was another file's)",
                self.companions_left
            ));
        }
        if self.profile_left_off > 0 {
            s.push_str(&format!(
                ", the preset's camera profile left off {} (made for another camera)",
                frames(self.profile_left_off)
            ));
        }
        if let Some((file, why)) = &self.error {
            s.push_str(&format!(": {}: {why}", name_of(file)));
        }
        s
    }
}

/// The import's record in the log: from where to where, what came of
/// it, and what was left. A warning when anything did not go, so a
/// command-line run says it without -v.
pub fn log_report(opts: &Options, scan: &Scan, report: &Report) {
    let line = format!(
        "import: {} to {}: {} of {} imported ({} files, {} bytes in {:.2} s, {:.1} MB/s), \
         {} backed up{}, {} already there, {} renamed, {} companions not copied, \
         {} given {}, profile left off {}, {} left on the card{}{}",
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
        scan.left.len(),
        if report.canceled { "; stopped" } else { "" },
        report
            .error
            .as_ref()
            .map(|(f, why)| format!("; failed on {}: {why}", f.display()))
            .unwrap_or_default(),
    );
    if report.error.is_some() || report.canceled || report.renamed > 0 || report.companions_left > 0
    {
        tracing::warn!("{line}");
    } else {
        tracing::info!("{line}");
    }
    for f in &scan.left {
        tracing::debug!("import: left on the card: {}", f.display());
    }
}

/// "1 frame", "3 frames".
pub fn frames(n: usize) -> String {
    format!("{n} frame{}", if n == 1 { "" } else { "s" })
}

/// The status line while frame `index` (from zero) is copied.
pub fn progress_line(index: usize, total: usize, name: &str) -> String {
    format!("importing {} of {total}: {name}", index + 1)
}

const CHUNK: usize = 1 << 20;

/// BLAKE3 over every byte of a file. The index's hash is the head and
/// the size, which is enough to know a file again but cannot see a
/// copy whose tail never landed; the verify has to read it all.
pub fn hash_whole(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    hash_reader(&mut file).map(|(h, _)| h)
}

fn hash_reader(r: &mut dyn Read) -> std::io::Result<(String, u64)> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    let mut n = 0u64;
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => {
                hasher.update(&buf[..k]);
                n += k as u64;
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
pub fn temporary(to: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = name_of(to);
    to.with_file_name(format!(".{name}.{}-{n}{TEMPORARY}", std::process::id()))
}

/// The process id a temporary's name carries, `.NAME.PID-N.greycard-import`.
fn temporary_pid(name: &str) -> Option<u32> {
    let rest = name.strip_suffix(TEMPORARY)?;
    let (_, last) = rest.rsplit_once('.')?;
    last.split_once('-')?.0.parse().ok()
}

/// Whether process `pid` is running. Unix asks with signal 0; elsewhere
/// it cannot be told cheaply and a temporary is judged by its age alone.
fn alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    #[cfg(unix)]
    {
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
        false
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
    let mut src = std::fs::File::open(from).context("reading it")?;
    let meta = src.metadata().context("reading it")?;
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
    let folder = to.parent().context("no folder to write in")?;
    std::fs::create_dir_all(folder).with_context(|| format!("making {}", folder.display()))?;
    let tmp = temporary(to);
    let mut made = false;
    let result = (|| -> Result<(String, u64)> {
        // Made new, so a file or a link already at the name is an
        // error rather than something followed and cut short.
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .context("writing the copy")?;
        made = true;
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; CHUNK];
        let mut bytes = 0u64;
        loop {
            let k = match src.read(&mut buf) {
                Ok(0) => break,
                Ok(k) => k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(anyhow::Error::new(e).context("reading it")),
            };
            hasher.update(&buf[..k]);
            out.write_all(&buf[..k]).context("writing the copy")?;
            bytes += k as u64;
        }
        if let Some(length) = length
            && bytes != length
        {
            bail!("read {bytes} bytes of the {length} it has");
        }
        out.sync_all().context("writing the copy")?;
        drop_cached(&out);
        drop(out);
        let hash = hasher.finalize().to_hex().to_string();
        if let Some(want) = want
            && want != hash
        {
            bail!("the copy it was made from no longer matches the card");
        }
        let (back, _) = open_uncached(&tmp)
            .and_then(|mut f| hash_reader(&mut f))
            .context("reading the copy back")?;
        if back != hash {
            bail!("the copy does not read back the same");
        }
        let how = rename_noreplace(&tmp, to).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                anyhow::anyhow!("{} appeared while it was copied", to.display())
            } else {
                anyhow::Error::new(e).context("naming the copy")
            }
        })?;
        say_rename(how);
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
            self.tail = Some(hash_tail(self.path).context("reading it")?);
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
            self.head = Some(greycard_library::hash_file(self.path).context("reading it")?);
        }
        Ok(self.head.clone().unwrap_or_default())
    }

    fn whole(&mut self) -> Result<String> {
        if self.whole.is_none() {
            self.whole = Some(hash_whole(self.path).context("reading it")?);
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
        "the source {} is not a folder",
        opts.source.display()
    );
    anyhow::ensure!(
        !opts.destination.as_os_str().is_empty(),
        "no destination is chosen"
    );
    let card = card_root(&opts.source);
    let source = resolve(&opts.source);
    let mut places = vec![("destination", resolve(&opts.destination))];
    if let Some(b) = &opts.backup {
        places.push(("backup", resolve(b)));
    }
    for (what, place) in &places {
        anyhow::ensure!(
            !source.starts_with(place),
            "the {what} {} holds the source {}",
            place.display(),
            source.display()
        );
        anyhow::ensure!(
            !place.starts_with(&card),
            "the {what} {} is on the card ({})",
            place.display(),
            card.display()
        );
    }
    if let [(_, dest), (_, backup)] = places.as_slice() {
        anyhow::ensure!(
            !backup.starts_with(dest) && !dest.starts_with(backup),
            "the backup is the destination, or inside it"
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

    /// `planned`, or ` (2)`, ` (3)` after it until the name is neither
    /// on disk nor given in this run; and whether a file that was on
    /// disk before this run moved it.
    fn free_name(&mut self, planned: &Path, reserve: usize) -> (PathBuf, bool) {
        let mut out = planned.to_path_buf();
        let mut n = 2;
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
    let (fields, body) = probe_file(&item.file, seq);
    let reserve = companion_reserve(item);
    let rel = relative_within(opts, &item.file, &fields, reserve)?;
    let planned = opts.destination.join(&rel);
    let size = std::fs::metadata(&item.file).context("reading it")?.len();
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
                bail!("the card read differently twice");
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
            .with_context(|| format!("{} (with {})", name_of(companion), name_of(&item.file)))?;
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
    if let (Some(preset), Some(placement)) = (&opts.preset, opts.placement) {
        let (preset, left_off) =
            crate::panel::sync::preset_fitted(preset, &opts.profiles, || body.clone());
        if left_off {
            tracing::warn!(
                "import: {}: the preset's camera profile was made for another camera, left off",
                name_of(&frame)
            );
            report.profile_left_off += 1;
        }
        let mut sidecar = greycard_edit::Sidecar::load(&frame)
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut seed = crate::files::never_developed(&sidecar);
        crate::panel::startup::preset_at_start(
            &mut sidecar,
            &mut seed,
            &frame,
            &preset,
            Some(placement),
        );
        if sidecar.current_label.is_some() {
            report.preset += 1;
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
/// ` (2)`, ` (3)`, stopping at the first that is free or holds these
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
    let mut n = 2;
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
            .context("the backup copy")?;
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
            .context("the backup copy")?;
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
mod tests {
    use super::*;

    /// A folder of this test's own under the temporary directory,
    /// emptied first, as the file system spells it (macOS's temporary
    /// directory is behind a link).
    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("greycard-import-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        dunce::canonicalize(&d).unwrap()
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn options(card: &Path, dest: &Path) -> Options {
        Options {
            source: card.to_path_buf(),
            destination: dest.to_path_buf(),
            subfolder: String::new(),
            name: "{name}".into(),
            preset: None,
            backup: None,
            library: None,
            placement: Some(Placement::Folder),
            profiles: Vec::new(),
            verify: false,
        }
    }

    fn go(o: &Options) -> Report {
        run(o, &scan(&o.source).unwrap(), &Job::default(), |_, _, _| {})
    }

    /// Every file under `dir`, hidden ones included, links not followed.
    fn all(d: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                out.extend(all(&p));
            } else {
                out.push(p);
            }
        }
        out.sort();
        out
    }

    /// No file in `dir` or under it has a temporary's name.
    fn no_temporaries(dir: &Path) {
        let tmp: Vec<_> = all(dir)
            .into_iter()
            .filter(|f| name_of(f).ends_with(TEMPORARY))
            .collect();
        assert!(tmp.is_empty(), "{tmp:?}");
    }

    /// A raw takes its JPEG and its XMP, a picture alone is a frame, a
    /// video and an XMP with no frame are left, and hidden files are
    /// not seen. Nothing here needs to be a real raw: the scan goes by
    /// the names.
    #[test]
    fn a_card_is_read_into_frames_and_companions() {
        let card = dir("scan").join("DCIM");
        let a = card.join("100CANON");
        let b = card.join("101CANON");
        for (f, body) in [
            (a.join("IMG_0001.CR3"), b"raw1".as_slice()),
            (a.join("IMG_0001.JPG"), b"jpg1"),
            (a.join("IMG_0001.CR3.xmp"), b"<x/>"),
            (a.join("IMG_0002.JPG"), b"jpg2"),
            (a.join("MVI_0003.MP4"), b"mov"),
            (a.join("IMG_0004.xmp"), b"<x/>"),
            (a.join("._IMG_0001.CR3"), b"mac"),
            (b.join("IMG_0001.CR3"), b"raw1b"),
            (b.join("IMG_0001.xmp"), b"<x/>"),
        ] {
            write(&f, body);
        }
        let s = scan(&card).unwrap();
        assert_eq!(
            s.items,
            vec![
                Item {
                    file: a.join("IMG_0001.CR3"),
                    companions: vec![a.join("IMG_0001.CR3.xmp"), a.join("IMG_0001.JPG")],
                },
                Item {
                    file: a.join("IMG_0002.JPG"),
                    companions: vec![],
                },
                Item {
                    file: b.join("IMG_0001.CR3"),
                    companions: vec![b.join("IMG_0001.xmp")],
                },
            ]
        );
        assert_eq!(s.left, vec![a.join("IMG_0004.xmp"), a.join("MVI_0003.MP4")]);
        assert_eq!(s.files(), 6);
        assert_eq!(
            companion_name(
                &a.join("IMG_0001.CR3"),
                Path::new("x/2026-0001.CR3"),
                &a.join("IMG_0001.CR3.xmp")
            ),
            "2026-0001.CR3.xmp"
        );
    }

    /// Every frame lands under its name, the companions beside it
    /// under the same new name, each verified, the card untouched, and
    /// a backup made of each; the same card again is all "already
    /// there", even under another name.
    #[test]
    fn an_import_copies_verifies_and_backs_up_and_a_second_is_already_there() {
        let root = dir("copy");
        let card = root.join("card/DCIM/100CANON");
        write(&card.join("IMG_0001.CR3"), &vec![7u8; 3 * CHUNK + 5]);
        write(&card.join("IMG_0001.JPG"), b"camera jpeg");
        write(&card.join("IMG_0002.CR3"), &vec![9u8; 1000]);
        let card_before: Vec<_> = all(&root.join("card"))
            .into_iter()
            .map(|p| (p.clone(), std::fs::read(&p).unwrap()))
            .collect();
        let dest = root.join("photos");
        let backup = root.join("backup");
        let mut o = options(&root.join("card/DCIM"), &dest);
        o.name = "shoot-{seq}".into();
        o.subfolder = "set".into();
        o.backup = Some(backup.clone());
        check(&o).unwrap();
        let s = scan(&o.source).unwrap();
        let mut seen = Vec::new();
        let r = run(&o, &s, &Job::default(), |i, n, name| {
            seen.push((i, n, name.to_string()))
        });
        assert_eq!(r.error, None);
        assert_eq!((r.frames, r.files, r.backed_up), (2, 3, 3));
        assert_eq!(r.bytes, (3 * CHUNK + 5 + 11 + 1000) as u64);
        assert_eq!(
            seen,
            vec![(0, 2, "IMG_0001.CR3".into()), (1, 2, "IMG_0002.CR3".into())]
        );
        for (name, from) in [
            ("shoot-0001.CR3", "IMG_0001.CR3"),
            ("shoot-0001.JPG", "IMG_0001.JPG"),
            ("shoot-0002.CR3", "IMG_0002.CR3"),
        ] {
            let want = std::fs::read(card.join(from)).unwrap();
            assert_eq!(std::fs::read(dest.join("set").join(name)).unwrap(), want);
            assert_eq!(std::fs::read(backup.join("set").join(name)).unwrap(), want);
        }
        assert_eq!(r.main_folder(), Some(dest.join("set").as_path()));
        // The card as it was, and no sidecar with no preset.
        let card_after: Vec<_> = all(&root.join("card"))
            .into_iter()
            .map(|p| (p.clone(), std::fs::read(&p).unwrap()))
            .collect();
        assert_eq!(card_after, card_before);
        assert!(
            !dest
                .join("set")
                .join(greycard_edit::SIDECAR_FOLDER)
                .exists()
        );
        no_temporaries(&root);

        // Again, under another pattern: nothing copied.
        o.name = "{name}".into();
        let again = run(&o, &s, &Job::default(), |_, _, _| {});
        assert_eq!((again.frames, again.already, again.files), (0, 2, 0));
        assert!(!dest.join("set/IMG_0001.CR3").exists());
        assert!(
            again.line(&dest).contains("2 already there"),
            "{}",
            again.line(&dest)
        );
    }

    /// A frame already there still gets its companions and its backup:
    /// a run with no backup then one with a backup leaves a full backup,
    /// and a companion missing from the destination is put back.
    #[test]
    fn a_frame_already_there_still_gets_its_companions_and_backup() {
        let root = dir("already");
        let card = root.join("card/DCIM/100CANON");
        write(&card.join("IMG_0001.CR3"), b"raw one");
        write(&card.join("IMG_0001.JPG"), b"jpeg one");
        write(&card.join("IMG_0002.CR3"), b"raw two");
        let dest = root.join("dest");
        let mut o = options(&root.join("card/DCIM"), &dest);
        let first = go(&o);
        assert_eq!((first.frames, first.files), (2, 3));
        std::fs::remove_file(dest.join("IMG_0001.JPG")).unwrap();

        let backup = root.join("backup");
        o.backup = Some(backup.clone());
        let second = go(&o);
        assert_eq!(second.error, None);
        assert_eq!((second.frames, second.already), (0, 2));
        assert_eq!(second.files, 1, "the JPEG put back");
        assert_eq!(
            std::fs::read(dest.join("IMG_0001.JPG")).unwrap(),
            b"jpeg one"
        );
        assert_eq!(second.backed_up, 3);
        for name in ["IMG_0001.CR3", "IMG_0001.JPG", "IMG_0002.CR3"] {
            assert_eq!(
                std::fs::read(backup.join(name)).unwrap(),
                std::fs::read(card.join(name)).unwrap()
            );
        }
        // A third run has nothing left to do.
        let third = go(&o);
        assert_eq!((third.files, third.backed_up, third.already), (0, 0, 2));
    }

    /// A name that is there with other bytes is never
    /// written over; the frame goes in as ` (2)` and the line says so.
    #[test]
    fn a_name_taken_by_another_file_goes_in_as_two() {
        let root = dir("taken");
        let card = root.join("card/DCIM");
        write(&card.join("IMG_0001.CR3"), b"from the card");
        write(&card.join("IMG_0001.JPG"), b"its jpeg");
        write(&card.join("IMG_0002.CR3"), b"second");
        let dest = root.join("dest");
        write(&dest.join("IMG_0001.CR3"), b"last year's frame");
        let r = go(&options(&card, &dest));
        assert_eq!((r.frames, r.renamed, r.already), (2, 1, 0));
        assert_eq!(
            std::fs::read(dest.join("IMG_0001.CR3")).unwrap(),
            b"last year's frame"
        );
        assert_eq!(
            std::fs::read(dest.join("IMG_0001 (2).CR3")).unwrap(),
            b"from the card"
        );
        // The JPEG follows its frame's new name.
        assert_eq!(
            std::fs::read(dest.join("IMG_0001 (2).JPG")).unwrap(),
            b"its jpeg"
        );
        assert!(
            r.line(&dest).contains("1 renamed with (2)"),
            "{}",
            r.line(&dest)
        );

        // A companion whose name is another file's is said too.
        let card2 = root.join("card2/DCIM");
        write(&card2.join("IMG_0009.CR3"), b"nine");
        write(&card2.join("IMG_0009.JPG"), b"nine's jpeg");
        write(&dest.join("IMG_0009.JPG"), b"some other jpeg");
        let r = go(&options(&card2, &dest));
        assert_eq!((r.frames, r.companions_left), (1, 1));
        assert!(
            r.line(&dest).contains("1 JPEG or XMP not copied"),
            "{}",
            r.line(&dest)
        );
    }

    /// Two frames of one name in one run (two folders of the card)
    /// are told apart rather than one written over the other.
    #[test]
    fn two_frames_of_one_name_in_a_run_both_land() {
        let root = dir("twice");
        let card = root.join("card/DCIM");
        write(&card.join("100CANON/IMG_0001.CR3"), b"first body");
        write(&card.join("101CANON/IMG_0001.CR3"), b"second body");
        let dest = root.join("dest");
        let r = go(&options(&card, &dest));
        assert_eq!(r.frames, 2);
        // Its own frame's name is not "another file's".
        assert_eq!(r.renamed, 0);
        assert_eq!(
            std::fs::read(dest.join("IMG_0001.CR3")).unwrap(),
            b"first body"
        );
        assert_eq!(
            std::fs::read(dest.join("IMG_0001 (2).CR3")).unwrap(),
            b"second body"
        );
    }

    /// The stop is heard between frames: the frame in hand lands whole,
    /// the rest are not begun, and nothing half-written is left.
    #[test]
    fn a_cancel_lands_the_frame_in_hand_and_leaves_no_partial_file() {
        let root = dir("cancel");
        let card = root.join("card/DCIM");
        for i in 1..=4 {
            write(
                &card.join(format!("IMG_000{i}.CR3")),
                &vec![i as u8; CHUNK + 17],
            );
        }
        let dest = root.join("dest");
        let job = Job::default();
        let r = run(
            &options(&card, &dest),
            &scan(&card).unwrap(),
            &job,
            |i, _, _| {
                if i == 1 {
                    job.cancel();
                }
            },
        );
        assert!(r.canceled);
        assert_eq!(r.frames, 2);
        let landed: Vec<_> = all(&dest).iter().map(|p| name_of(p)).collect();
        assert_eq!(landed, vec!["IMG_0001.CR3", "IMG_0002.CR3"]);
        assert_eq!(
            std::fs::read(dest.join("IMG_0002.CR3")).unwrap().len(),
            CHUNK + 17
        );
        assert!(r.line(&dest).starts_with("import stopped: 2 of 4 frames"));
        no_temporaries(&root);
    }

    /// A card that fails partway through a file: the error names the
    /// file, no file of that name is made, and the temporary is gone.
    #[test]
    fn a_read_error_leaves_nothing_behind() {
        struct Dying(usize);
        impl Read for Dying {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0 == 0 {
                    return Err(std::io::Error::other("I/O error on the card"));
                }
                let k = buf.len().min(self.0);
                buf[..k].fill(3);
                self.0 -= k;
                Ok(k)
            }
        }
        let root = dir("dying");
        let to = root.join("dest/IMG_0001.CR3");
        let e = land_from(&mut Dying(CHUNK + 100), None, None, &to, None).unwrap_err();
        assert!(format!("{e:#}").contains("I/O error on the card"), "{e:#}");
        assert!(!to.exists());
        no_temporaries(&root);
        // A read that ends short of the file's size is not a smaller
        // file.
        let e = land_from(&mut &b"short"[..], None, Some(9), &to, None).unwrap_err();
        assert!(format!("{e:#}").contains("read 5 bytes of the 9"), "{e:#}");
        assert!(!to.exists());
        no_temporaries(&root);

        // A bad file in a run stops the run there and says which.
        let card = root.join("card/DCIM");
        write(&card.join("IMG_0001.CR3"), b"fine");
        write(&card.join("IMG_0002.CR3"), b"fine too");
        let dest = root.join("run");
        let mut s = scan(&card).unwrap();
        // Gone between the scan and its copy, as a card pulled out.
        std::fs::remove_file(card.join("IMG_0001.CR3")).unwrap();
        s.items.swap(0, 1);
        let r = run(&options(&card, &dest), &s, &Job::default(), |_, _, _| {});
        assert_eq!(r.frames, 1);
        let (file, _) = r.error.clone().expect("the missing file stops it");
        assert_eq!(file, card.join("IMG_0001.CR3"));
        assert!(r.line(&dest).contains("IMG_0001.CR3"), "{}", r.line(&dest));
        no_temporaries(&root);
    }

    /// The index's hash reads the head and the size, so a copy whose
    /// tail is wrong has the same one; the verify's whole-file hash
    /// does not.
    #[test]
    fn a_copy_wrong_in_its_tail_passes_the_head_hash_and_fails_the_whole_one() {
        let root = dir("tail");
        let mut body = vec![5u8; greycard_library::hash::HEAD + 4096];
        let good = root.join("good.CR3");
        std::fs::write(&good, &body).unwrap();
        *body.last_mut().unwrap() = 6;
        let bad = root.join("bad.CR3");
        std::fs::write(&bad, &body).unwrap();
        assert_eq!(
            greycard_library::hash_file(&good).unwrap(),
            greycard_library::hash_file(&bad).unwrap()
        );
        assert_ne!(hash_whole(&good).unwrap(), hash_whole(&bad).unwrap());
        // And a backup checked against a hash it does not have is
        // refused, nothing written.
        let to = root.join("backup/good.CR3");
        let e = land(&bad, &to, Some(&hash_whole(&good).unwrap())).unwrap_err();
        assert!(format!("{e:#}").contains("no longer matches"), "{e:#}");
        assert!(!to.exists());
        no_temporaries(&root);
    }

    /// The library knows a copy by its head and size; a card file that
    /// differs from it in its last byte is not that copy. And
    /// a row on a camera card (the other slot of a two-slot body,
    /// browsed in the editor) is not an import.
    #[test]
    fn the_library_is_trusted_only_for_the_same_bytes_off_the_card() {
        let root = dir("library");
        let mut body = vec![4u8; greycard_library::hash::HEAD + 4096];
        let dest = root.join("dest");
        write(&dest.join("IMG_0001.CR3"), &body);
        let slot2 = root.join("slot2/DCIM/100CANON");
        write(&slot2.join("IMG_0002.CR3"), b"on the other card");
        let db = root.join("library.sqlite");
        {
            let mut lib = greycard_library::Library::open(&db).unwrap();
            lib.index_folder(&dest, &mut |_| {}).unwrap();
            lib.index_folder(&slot2, &mut |_| {}).unwrap();
        }
        *body.last_mut().unwrap() = 5;
        let card = root.join("card/DCIM");
        write(&card.join("IMG_0001.CR3"), &body);
        write(&card.join("IMG_0002.CR3"), b"on the other card");
        // The destination is elsewhere, so only the library can say.
        let elsewhere = root.join("elsewhere");
        let mut o = options(&card, &elsewhere);
        o.library = Some(db);
        let r = go(&o);
        assert_eq!(r.error, None);
        assert_eq!((r.frames, r.already), (2, 0), "{r:?}");
        assert_eq!(std::fs::read(elsewhere.join("IMG_0001.CR3")).unwrap(), body);

        // The same bytes, off any card: already there.
        let card3 = root.join("card3/DCIM");
        write(
            &card3.join("IMG_0001.CR3"),
            &std::fs::read(dest.join("IMG_0001.CR3")).unwrap(),
        );
        let mut o = options(&card3, &root.join("third"));
        o.library = Some(root.join("library.sqlite"));
        let r = go(&o);
        assert_eq!((r.frames, r.already), (0, 1), "{r:?}");
    }

    /// A frame already there is known by its size, its head and its
    /// tail: a copy whose middle differs passes by default, which is
    /// the price of a re-run that does not read the whole card, and
    /// `verify` catches it. The tail catches a copy cut short or
    /// changed at its end either way.
    #[test]
    fn verify_catches_a_middle_the_head_and_tail_do_not() {
        let root = dir("middle");
        let big = 3 * greycard_library::hash::HEAD;
        let body: Vec<u8> = (0..big).map(|i| (i % 251) as u8).collect();
        let card = root.join("card/DCIM");
        write(&card.join("IMG_0001.CR3"), &body);
        let mut middle = body.clone();
        middle[big / 2] ^= 0xff;
        let mut tail = body.clone();
        *tail.last_mut().unwrap() ^= 0xff;

        let dest = root.join("dest");
        write(&dest.join("IMG_0001.CR3"), &middle);
        let by_default = go(&options(&card, &dest));
        assert_eq!((by_default.already, by_default.frames), (1, 0));
        let mut o = options(&card, &dest);
        o.verify = true;
        let verified = go(&o);
        assert_eq!((verified.already, verified.frames), (0, 1), "{verified:?}");
        assert_eq!(std::fs::read(dest.join("IMG_0001 (2).CR3")).unwrap(), body);

        let dest = root.join("tail");
        write(&dest.join("IMG_0001.CR3"), &tail);
        let r = go(&options(&card, &dest));
        assert_eq!((r.already, r.frames), (0, 1), "{r:?}");
        assert!(
            !same_bytes(
                &card.join("IMG_0001.CR3"),
                &dest.join("IMG_0001.CR3"),
                false
            )
            .unwrap()
        );
    }

    /// A link out of the destination does not make the
    /// card's own files look imported, and a link back up the card's
    /// tree does not list a frame over and over.
    #[cfg(unix)]
    #[test]
    fn links_are_not_followed_in_either_walk() {
        let root = dir("links");
        let card = root.join("card/DCIM");
        write(&card.join("100CANON/IMG_0001.CR3"), b"a raw");
        std::os::unix::fs::symlink("..", card.join("100CANON/loop")).unwrap();
        let s = scan(&card).unwrap();
        assert_eq!(s.items.len(), 1, "{:?}", s.items);
        let dest = root.join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        std::os::unix::fs::symlink(&root, dest.join("loop")).unwrap();
        let r = go(&options(&card, &dest));
        assert_eq!((r.frames, r.already), (1, 0), "{r:?}");
    }

    /// The preset is one named step in the sidecar where the Settings
    /// sheet puts sidecars, as a preset laid by hand is.
    #[test]
    fn a_preset_is_one_named_step_where_the_setting_puts_it() {
        let root = dir("preset");
        let card = root.join("card/DCIM");
        std::fs::create_dir_all(&card).unwrap();
        greycard_library::fixture::write_frame(
            &card.join("IMG_0001.tif"),
            &greycard_library::fixture::R5,
            1,
        );
        let mut edit = greycard_edit::Edit::default();
        edit.light.exposure = 0.7;
        let preset = Preset::from_edit("Faded film", &edit, &[greycard_edit::Section::Light]);
        for (placement, dest) in [
            (Placement::Folder, root.join("folder")),
            (Placement::Beside, root.join("beside")),
        ] {
            let mut o = options(&card, &dest);
            o.preset = Some(preset.clone());
            o.placement = Some(placement);
            o.subfolder = "{yyyy}/{date} {camera}".into();
            let r = go(&o);
            assert_eq!((r.frames, r.preset), (1, 1), "{r:?}");
            // The EXIF's day and camera, not the file's time.
            let file = dest.join("2024/2024-08-24 Canon EOS R5/IMG_0001.tif");
            assert!(file.is_file());
            assert_eq!(
                greycard_edit::Sidecar::find(&file),
                Some(greycard_edit::Sidecar::path_in(&file, placement))
            );
            let sidecar = greycard_edit::Sidecar::load(&file).unwrap().unwrap();
            assert_eq!(sidecar.history.len(), 1);
            assert_eq!(sidecar.current_label.as_deref(), Some("Preset: Faded film"));
            assert_eq!(sidecar.current.light.exposure, 0.7);
        }
        // Sidecars off: no preset laid, nothing written.
        let mut o = options(&card, &root.join("none"));
        o.preset = Some(preset);
        o.placement = None;
        let r = go(&o);
        assert_eq!((r.frames, r.preset), (1, 0));
        assert!(greycard_edit::Sidecar::find(&root.join("none/IMG_0001.tif")).is_none());
    }

    /// A preset's camera profile made for another body is
    /// left off a frame, as §162 leaves it off a frame of a set.
    #[test]
    fn a_preset_s_camera_profile_is_left_off_another_body() {
        let root = dir("profile");
        let card = root.join("card/DCIM");
        std::fs::create_dir_all(&card).unwrap();
        greycard_library::fixture::write_frame(
            &card.join("IMG_0001.tif"),
            &greycard_library::fixture::R5,
            1,
        );
        let mut edit = greycard_edit::Edit::default();
        edit.light.exposure = 0.4;
        edit.camera.profile = greycard_edit::camera::ProfileChoice::Named("z9".into());
        let preset = Preset::from_edit(
            "Nikon look",
            &edit,
            &[
                greycard_edit::Section::Light,
                greycard_edit::Section::Camera,
            ],
        );
        let mut o = options(&card, &root.join("dest"));
        o.preset = Some(preset);
        o.profiles = vec![greycard_edit::camera::Entry {
            name: "z9".into(),
            path: root.join("z9.dcp"),
            title: None,
            unique_camera_model: Some("Nikon Z 9".into()),
            copyright: None,
        }];
        let r = go(&o);
        assert_eq!((r.frames, r.preset, r.profile_left_off), (1, 1, 1), "{r:?}");
        let sidecar = greycard_edit::Sidecar::load(&root.join("dest/IMG_0001.tif"))
            .unwrap()
            .unwrap();
        assert_eq!(sidecar.current.light.exposure, 0.4);
        assert_eq!(
            sidecar.current.camera.profile,
            greycard_edit::Edit::default().camera.profile
        );
        assert!(
            r.line(Path::new("x"))
                .contains("camera profile left off 1 frame")
        );
    }

    /// Nothing on the card is a destination or a
    /// backup, beside `DCIM` as well as under it, whether the folder is
    /// there yet or not and however the path is spelled; nor is a folder
    /// that holds the source.
    #[test]
    fn nothing_on_the_card_or_around_the_source_is_written_to() {
        let root = dir("check");
        let card = root.join("EOS_DIGITAL");
        std::fs::create_dir_all(card.join("DCIM/100CANON")).unwrap();
        let source = card.join("DCIM");
        let mut o = options(&source, &source.join("out"));
        assert!(check(&o).is_err());
        // Beside DCIM, on the card.
        o.destination = card.join("Imported");
        let e = check(&o).unwrap_err().to_string();
        assert!(e.contains("is on the card"), "{e}");
        // The card itself, which holds the source.
        o.destination = card.clone();
        assert!(check(&o).is_err());
        // Spelled with `..`, and not there yet.
        o.destination = root.join("elsewhere/../EOS_DIGITAL/new/deeper");
        assert!(check(&o).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&card, root.join("card-link")).unwrap();
            o.destination = root.join("card-link/new");
            assert!(check(&o).is_err());
        }
        // A folder of pictures imported into the folder holding it.
        let pics = root.join("pics");
        std::fs::create_dir_all(pics.join("dump")).unwrap();
        let o2 = options(&pics.join("dump"), &pics);
        let e = check(&o2).unwrap_err().to_string();
        assert!(e.contains("holds the source"), "{e}");

        o.destination = root.join("dest");
        o.backup = Some(card.join("backup"));
        assert!(check(&o).is_err());
        o.backup = Some(root.join("dest/backup"));
        assert!(check(&o).is_err());
        o.backup = Some(root.join("backup"));
        check(&o).unwrap();
        o.name = "{nope}".into();
        assert!(check(&o).is_err());
        // The card is where the DCIM is, from any folder under it.
        assert_eq!(card_root(&source.join("100CANON")), card);
        assert_eq!(
            resolve(&root.join("a/../EOS_DIGITAL/x/y")),
            card.join("x").join("y")
        );
    }

    /// The rename never replaces a file, and a temporary
    /// a killed run left more than a day ago is swept, a fresh one not.
    #[test]
    fn the_rename_never_replaces_and_stale_temporaries_go() {
        let root = dir("rename");
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::write(&a, b"new").unwrap();
        std::fs::write(&b, b"there first").unwrap();
        let e = rename_noreplace(&a, &b).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&b).unwrap(), b"there first");
        assert!(a.exists());
        rename_noreplace(&a, &root.join("c")).unwrap();
        assert!(!a.exists() && root.join("c").exists());

        let dest = root.join("dest");
        // A process id no system hands out, so taken for gone.
        let gone = 2_147_483_646u32;
        let stale = dest.join(format!(".IMG_0001.CR3.{gone}-0.greycard-import"));
        let fresh = dest.join(format!(".IMG_0002.CR3.{gone}-1.greycard-import"));
        let running = dest.join(format!(
            ".IMG_0004.CR3.{}-2.greycard-import",
            std::process::id()
        ));
        let two_days = std::time::Duration::from_secs(2 * 24 * 60 * 60);
        for (f, old) in [(&stale, true), (&fresh, false), (&running, true)] {
            write(f, b"half");
            if old {
                std::fs::File::options()
                    .write(true)
                    .open(f)
                    .unwrap()
                    .set_modified(std::time::SystemTime::now() - two_days)
                    .unwrap();
            }
        }
        assert_eq!(temporary_pid(&name_of(&stale)), Some(gone));
        let card = root.join("card/DCIM");
        write(&card.join("IMG_0003.CR3"), b"three");
        go(&options(&card, &dest));
        assert!(!stale.exists(), "old and its process gone");
        assert!(fresh.exists(), "written within the day");
        #[cfg(unix)]
        assert!(running.exists(), "its process is running");
        // The card's time goes on the file once it has its name, not on
        // the temporary a sweep would judge by it.
        let when = std::fs::metadata(card.join("IMG_0003.CR3"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            std::fs::metadata(dest.join("IMG_0003.CR3"))
                .unwrap()
                .modified()
                .unwrap(),
            when
        );
        // Two temporaries for one name are two names.
        assert_ne!(temporary(&b), temporary(&b));
    }

    /// A file of the same size but another head is not read further,
    /// and the card file is not read whole.
    #[test]
    fn a_different_head_is_not_read_whole() {
        let root = dir("heads");
        let dest = root.join("dest");
        write(&dest.join("other.CR3"), &vec![1u8; 5000]);
        let card = root.join("IMG.CR3");
        write(&card, &vec![2u8; 5000]);
        let mut existing = Existing::under(&dest);
        let mut c = CardFile::new(&card, 5000);
        assert_eq!(existing.holds(&mut c).unwrap(), None);
        assert!(existing.wholes.is_empty());
        assert!(c.whole.is_none(), "the card file was not read whole");
    }

    #[test]
    fn a_card_is_a_volume_with_a_dcim_folder() {
        let root = dir("volumes");
        std::fs::create_dir_all(root.join("BACKUP")).unwrap();
        std::fs::create_dir_all(root.join("EOS_DIGITAL/DCIM/100CANON")).unwrap();
        std::fs::create_dir_all(root.join("ZZZ/DCIM")).unwrap();
        let v = volumes_under(&root);
        assert_eq!(v.len(), 3);
        assert_eq!(find_card(&v), Some(root.join("EOS_DIGITAL/DCIM")));
        assert_eq!(find_card(&v[..1]), None);
        assert!(volumes_under(&root.join("not there")).is_empty());
        assert_eq!(
            same_device(&root.join("BACKUP"), &root.join("ZZZ")),
            Some(true)
        );
    }

    /// The card is found by its `DCIM`, looked for no higher than the
    /// source's mount and never at the home folder: a phone-sync tool's
    /// `~/DCIM` does not make everything under home a card, and a folder
    /// of pictures on a data drive may go beside itself on that drive.
    #[test]
    fn a_dcim_in_home_or_none_at_all_is_not_a_card() {
        let root = dir("home");
        let home = root.join("fh");
        std::fs::create_dir_all(home.join("DCIM")).unwrap();
        let shoot = home.join("Downloads/shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        assert_eq!(dcim_root(&shoot, Some(&home)), None);
        // Without the home folder in the way it would have been one.
        assert_eq!(dcim_root(&shoot, None), Some(home.clone()));
        let eos = root.join("EOS_DIGITAL");
        std::fs::create_dir_all(eos.join("DCIM/100CANON")).unwrap();
        assert_eq!(
            dcim_root(&eos.join("DCIM/100CANON"), Some(&home)),
            Some(eos.clone())
        );

        // A data drive's incoming folder into a folder beside it.
        let data = root.join("data");
        write(&data.join("incoming/IMG_0001.CR3"), b"raw");
        let o = options(&data.join("incoming"), &data.join("2026"));
        check(&o).unwrap();
        assert_eq!(card_root(&data.join("incoming")), data.join("incoming"));
        let r = go(&o);
        assert_eq!(r.frames, 1);
        // Never inside the source, nor holding it.
        assert!(check(&options(&data.join("incoming"), &data.join("incoming/out"))).is_err());
        assert!(check(&options(&data.join("incoming"), &data)).is_err());
    }

    /// A backup name holding other bytes moves the frame's backup on to
    /// ` (2)`, and a re-run finds that copy rather than making ` (3)`;
    /// the companion follows as `X (2).CR3.xmp`.
    #[test]
    fn a_backup_name_taken_is_numbered_once_and_found_again() {
        let root = dir("backup-names");
        let card = root.join("card/DCIM");
        write(&card.join("IMG_0002.CR3"), b"the card's raw");
        write(&card.join("IMG_0002.CR3.xmp"), b"<x/>");
        let dest = root.join("dest");
        let backup = root.join("backup");
        write(&backup.join("IMG_0002.CR3"), b"another raw");
        let mut o = options(&card, &dest);
        o.backup = Some(backup.clone());
        let first = go(&o);
        assert_eq!(first.backed_up, 2, "{first:?}");
        assert_eq!(
            std::fs::read(backup.join("IMG_0002 (2).CR3")).unwrap(),
            b"the card's raw"
        );
        assert_eq!(
            std::fs::read(backup.join("IMG_0002 (2).CR3.xmp")).unwrap(),
            b"<x/>"
        );
        for _ in 0..2 {
            let again = go(&o);
            assert_eq!((again.backed_up, again.already), (0, 1), "{again:?}");
        }
        let mut names: Vec<_> = all(&backup).iter().map(|p| name_of(p)).collect();
        names.sort();
        assert_eq!(
            names,
            vec!["IMG_0002 (2).CR3", "IMG_0002 (2).CR3.xmp", "IMG_0002.CR3"]
        );
    }

    /// A frame's name is cut to leave room for its companion's longer
    /// tail, so a raw and its `.CR3.xmp` keep one stem however long the
    /// pattern makes it.
    #[test]
    fn a_long_name_leaves_room_for_the_companion() {
        let root = dir("long-pair");
        let card = root.join("card/DCIM");
        write(&card.join("IMG_0001.CR3"), b"raw");
        write(&card.join("IMG_0001.CR3.xmp"), b"<x/>");
        let dest = root.join("dest");
        let mut o = options(&card, &dest);
        o.name = format!("{}{{seq}}", "a".repeat(196));
        let r = go(&o);
        assert_eq!(r.files, 2, "{r:?}");
        let names: Vec<_> = all(&dest).iter().map(|p| name_of(p)).collect();
        let raw = names.iter().find(|n| n.ends_with(".CR3")).unwrap();
        let xmp = names.iter().find(|n| n.ends_with(".CR3.xmp")).unwrap();
        assert_eq!(format!("{raw}.xmp"), *xmp);
        assert!(xmp.len() <= naming::LONGEST, "{}", xmp.len());
    }
}
