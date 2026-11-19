//! Archive roots (notes §197, §216): Back up and Bring back, the one
//! copy in two directions.
//!
//! A backup takes frames here and copies to a folder under an archive
//! root the ones not there yet; a bring-back takes frames on the
//! archive and copies to a local root the ones not there yet. "There"
//! is decided by hash first: the frame's content key (§72's head and
//! size) looked up in the index under the other side's roots, at any
//! path, and every find confirmed on the disk by existence and size, so
//! a row with no file behind it is a frame not there. Only a frame
//! found nowhere is copied, to the destination folder at its path
//! relative to the source's base.
//!
//! A frame found there carries across those of its sidecars that are
//! the newer, each file judged on its own: the `.gcd` by §161's rule
//! (the save counter when both carry one and they differ, the mtime
//! otherwise), an XMP by its mtime. A sidecar whose copy over there is
//! the newer, or cannot be told from this one, is left as it is and the
//! frame named, since copying over it would lose an edit made on the
//! other side.
//!
//! Every copy is the whole file's BLAKE3 streamed as it is written to a
//! temporary name beside the destination, read back and hashed again,
//! and renamed to its name only when the two agree (the import's
//! `land_with`); a temporary left by a crash, an hour old and of no
//! process running here, is removed by the next run that writes to that
//! folder, and said. Nothing on the destination side is ever deleted or
//! written over, except an older sidecar replaced by the newer.
//!
//! Everything here asks the disk and is run off the window's thread,
//! a file at a time, with a beat at every chunk, and at every wait on
//! the library's lock, so a share that stops answering in the middle of
//! a copy can be told from a slow one or a busy index ([`supervise`]).
//! The window's side is `panel::archive`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime};

use greycard_edit::{SIDECAR_FOLDER, Sidecar};
use greycard_library::Library;

use crate::tr;

pub mod rejects;

/// The ending of a copy's temporary name, which a later run removes.
pub const TEMPORARY: &str = ".greycard-backup";

/// How old a temporary has to be before a run takes it for one a crash
/// left: a copy under way writes to its temporary at every chunk, so an
/// hour without a write is no copy under way.
const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// A job's word that it is still moving: shared, so a wait on the
/// library's lock can say it too.
pub type Beat = Arc<dyn Fn() + Send + Sync>;

/// A beat nobody listens to. Built whatever the build: the binary's
/// tests use it, and they build this library without cfg(test).
pub fn no_beat() -> Beat {
    Arc::new(|| {})
}

/// Which way the copy goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From here to an archive.
    BackUp,
    /// From an archive to a local root.
    BringBack,
}

/// The frames a copy is over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frames {
    /// These frames, as chosen.
    Chosen(Vec<PathBuf>),
    /// A folder and every folder under it: the hidden ones left out,
    /// the other side's roots, and the rejects folders unless the ask
    /// includes them.
    Folder(PathBuf),
}

/// A copy asked for: everything the look needs, taken from the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub direction: Direction,
    pub frames: Frames,
    /// Walk a folder's rejects folders too.
    pub include_rejects: bool,
    /// Carry the sidecars: false under `--no-sidecars`, which neither
    /// reads nor writes one.
    pub sidecars: bool,
    /// Where each frame's path is taken relative to.
    pub base: PathBuf,
    /// Where each frame goes: this folder, at its path under `base`.
    pub dest: PathBuf,
    /// Where a copy already there is looked for by hash: the archive
    /// for a backup, the local roots for a bring-back.
    pub other_side: Vec<PathBuf>,
    /// The roots a folder's walk never goes into: every archive for a
    /// backup (a folder of no root's with an archive inside it), the
    /// local roots for a bring-back.
    pub skip: Vec<PathBuf>,
    /// The library's database, read for the hashes, and written with
    /// the copies' rows and their whole-file hashes.
    pub index: Option<PathBuf>,
}

impl Ask {
    /// Where `frame` goes.
    pub fn dest_of(&self, frame: &Path) -> PathBuf {
        match frame.strip_prefix(&self.base) {
            Ok(rel) if !rel.as_os_str().is_empty() => self.dest.join(rel),
            _ => self
                .dest
                .join(frame.file_name().unwrap_or(frame.as_os_str())),
        }
    }
}

/// One sidecar file to copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarCopy {
    pub from: PathBuf,
    pub to: PathBuf,
    /// Over an older copy of it there, judged older file by file.
    pub replace: bool,
}

/// Two `.gcd`s of one frame that went their own ways, to be joined
/// (§233) and the join written to both: `mine` is the frame on the side
/// the copy is from, `theirs` the frame on the other side, each with
/// the sidecar path found there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarJoin {
    pub mine: PathBuf,
    pub mine_sidecar: PathBuf,
    pub theirs: PathBuf,
    pub theirs_sidecar: PathBuf,
}

/// What is to be done with one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    /// Not on the other side: the frame and its sidecars copied.
    Copy {
        to: PathBuf,
        sidecars: Vec<SidecarCopy>,
    },
    /// There, at `there`, and some of its sidecars here are the newer:
    /// those alone copied. `held` when others there are the newer and
    /// are left, which names the frame. `join` when the two `.gcd`s
    /// went their own ways and are joined, the join written to both.
    Sidecars {
        there: PathBuf,
        sidecars: Vec<SidecarCopy>,
        held: bool,
        join: Option<SidecarJoin>,
    },
    /// There, and the same: nothing to do. `elsewhere` when it is not
    /// at the path this frame would have gone to.
    Same { there: PathBuf, elsewhere: bool },
    /// There, and its sidecars there are the newer (or cannot be told
    /// from these): left as they are, and named.
    TheirsNewer { there: PathBuf },
    /// Not there, and another file has its name, or one of its
    /// sidecars' names, at the destination (`to`): left alone, and
    /// named.
    Taken { to: PathBuf },
    /// The frame could not be read here.
    Unreadable(String),
}

/// One frame of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub frame: PathBuf,
    pub size: u64,
    pub what: What,
}

/// What a copy would do, before anything is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub ask: Ask,
    pub items: Vec<Item>,
    /// Frames in the rejects folders a folder's walk found, left out
    /// unless the ask includes them.
    pub rejects: usize,
}

impl Plan {
    /// The frames to copy whole, and their bytes.
    pub fn copies(&self) -> (usize, u64) {
        self.items
            .iter()
            .filter(|i| matches!(i.what, What::Copy { .. }))
            .fold((0, 0), |(n, b), i| (n + 1, b + i.size))
    }

    /// How many frames go as sidecars alone.
    pub fn sidecars_only(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i.what, What::Sidecars { .. }))
            .count()
    }

    /// How many are there already, and how many of those under
    /// another folder than this frame's would be.
    pub fn same(&self) -> (usize, usize) {
        self.items.iter().fold((0, 0), |(n, e), i| match i.what {
            What::Same { elsewhere, .. } => (n + 1, e + usize::from(elsewhere)),
            _ => (n, e),
        })
    }

    /// The frames some of whose sidecars over there are the newer, left
    /// as they are: all of them, or beside the newer ones that go.
    pub fn theirs_newer(&self) -> Vec<&Path> {
        self.items
            .iter()
            .filter(|i| {
                matches!(
                    i.what,
                    What::TheirsNewer { .. } | What::Sidecars { held: true, .. }
                )
            })
            .map(|i| i.frame.as_path())
            .collect()
    }

    pub fn taken(&self) -> Vec<&Path> {
        self.items
            .iter()
            .filter(|i| matches!(i.what, What::Taken { .. }))
            .map(|i| i.frame.as_path())
            .collect()
    }

    pub fn unreadable(&self) -> Vec<(&Path, &str)> {
        self.items
            .iter()
            .filter_map(|i| match &i.what {
                What::Unreadable(why) => Some((i.frame.as_path(), why.as_str())),
                _ => None,
            })
            .collect()
    }

    /// Whether running it would copy anything at all.
    pub fn has_work(&self) -> bool {
        self.items
            .iter()
            .any(|i| matches!(i.what, What::Copy { .. } | What::Sidecars { .. }))
    }
}

/// The frames under `dir`, every folder down, the hidden folders and
/// those at or under `skip` (the other side's roots: a folder of no
/// root's with the archive inside it) left out, and the rejects folders
/// unless `rejects`; with how many frames the rejects folders hold,
/// taken or not. By path.
pub fn walk(
    dir: &Path,
    rejects: bool,
    skip: &[PathBuf],
    beat: &dyn Fn(),
    stop: &dyn Fn() -> bool,
) -> (Vec<PathBuf>, usize) {
    let mut frames = Vec::new();
    let mut left = 0;
    let skipped = |p: &Path| skip.iter().any(|s| p.starts_with(s));
    if skipped(dir) {
        return (frames, left);
    }
    let mut stack = vec![(dir.to_path_buf(), false)];
    while let Some((d, in_rejects)) = stack.pop() {
        beat();
        if stop() {
            return (Vec::new(), 0);
        }
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name();
            let hidden = name.to_string_lossy().starts_with('.');
            let Ok(kind) = e.file_type() else {
                continue;
            };
            if hidden {
                continue;
            }
            if kind.is_dir() {
                if skipped(&e.path()) {
                    continue;
                }
                let rejected = in_rejects || name == std::ffi::OsStr::new(crate::rejects::REJECTS);
                stack.push((e.path(), rejected));
            } else if kind.is_file() && greycard_library::is_indexed_path(&e.path()) {
                left += usize::from(in_rejects);
                if !in_rejects || rejects {
                    frames.push(e.path());
                }
            }
        }
    }
    frames.sort();
    (frames, left)
}

/// The frames an ask is over: those chosen, or the folder's walk.
pub fn frames_of(ask: &Ask, beat: &dyn Fn()) -> (Vec<PathBuf>, usize) {
    match &ask.frames {
        Frames::Chosen(f) => (f.clone(), 0),
        Frames::Folder(dir) => walk(dir, ask.include_rejects, &ask.skip, beat, &|| false),
    }
}

/// The sidecars that travel with a frame: the `.gcd` the editor would
/// read ([`Sidecar::find`], beside or under the hidden folder), and the
/// frame's XMPs.
pub fn travelling(frame: &Path) -> Vec<PathBuf> {
    Sidecar::find(frame)
        .into_iter()
        .chain(greycard_edit_io::xmp::paths_of(frame))
        .collect()
}

/// Where `sidecar`, one of `frame`'s, goes for the frame at `to`: the
/// same placement, its name taken over from the frame's to `to`'s.
pub fn sidecar_at(sidecar: &Path, frame: &Path, to: &Path) -> PathBuf {
    let name = sidecar
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (from_name, to_name) = (
        frame
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        to.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    let stem = |p: &Path| {
        p.file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let renamed = if let Some(rest) = name.strip_prefix(&from_name) {
        format!("{to_name}{rest}")
    } else if let Some(rest) = name.strip_prefix(&stem(frame)) {
        format!("{}{rest}", stem(to))
    } else {
        name
    };
    let folder = to.parent().unwrap_or(Path::new("."));
    let under =
        sidecar.parent().and_then(Path::file_name) == Some(std::ffi::OsStr::new(SIDECAR_FOLDER));
    if under {
        folder.join(SIDECAR_FOLDER).join(renamed)
    } else {
        folder.join(renamed)
    }
}

fn bytes_of(p: &Path) -> Option<Vec<u8>> {
    std::fs::read(p).ok()
}

fn modified(p: &Path) -> Option<SystemTime> {
    p.metadata().and_then(|m| m.modified()).ok()
}

/// The sidecars of `mine` to copy toward the same frame at `theirs`,
/// each file judged on its own, and whether any of `theirs`' is left
/// because it is the newer (or cannot be told from mine, or mine has
/// no counterpart). The `.gcd` by §233's comparison, as the archive
/// write behind a save does it: the copy that is behind is written
/// over whole, in either direction, over the one the other side has
/// wherever that is, so the copy never ends up with two; two that went
/// their own ways are joined and the join written to both (the third
/// of the tuple); each XMP by its mtime against the one its name gives
/// it there. The same bytes on both sides is nothing to do.
pub fn compare_sidecars(
    mine: &Path,
    theirs: &Path,
) -> (Vec<SidecarCopy>, bool, Option<SidecarJoin>) {
    let mut sends = Vec::new();
    let mut held = false;
    let mut join = None;
    match (Sidecar::find(mine), Sidecar::find(theirs)) {
        (Some(m), None) => sends.push(SidecarCopy {
            to: sidecar_at(&m, mine, theirs),
            from: m,
            replace: false,
        }),
        (None, Some(_)) => held = true,
        (Some(m), Some(t)) if bytes_of(&m) != bytes_of(&t) => {
            use greycard_edit::sync::{Compared, Side, compare};
            match (Sidecar::read(&m), Sidecar::read(&t)) {
                (Ok(ours), Ok(other)) => match compare(&ours, &other) {
                    Compared::Same => {}
                    Compared::Behind(Side::Other) => sends.push(SidecarCopy {
                        from: m,
                        to: t,
                        replace: true,
                    }),
                    Compared::Behind(Side::Local) => sends.push(SidecarCopy {
                        from: t,
                        to: m,
                        replace: true,
                    }),
                    Compared::SameEdit | Compared::Diverged => {
                        join = Some(SidecarJoin {
                            mine: mine.to_path_buf(),
                            mine_sidecar: m,
                            theirs: theirs.to_path_buf(),
                            theirs_sidecar: t,
                        });
                    }
                },
                // One that will not read is left as it is, and named.
                _ => held = true,
            }
        }
        _ => {}
    }
    let theirs_xmps = greycard_edit_io::xmp::paths_of(theirs);
    let mut answered = Vec::new();
    for m in greycard_edit_io::xmp::paths_of(mine) {
        let t = sidecar_at(&m, mine, theirs);
        answered.push(t.clone());
        if std::fs::symlink_metadata(&t).is_err() {
            sends.push(SidecarCopy {
                from: m,
                to: t,
                replace: false,
            });
        } else if bytes_of(&m) == bytes_of(&t) {
        } else if modified(&m) > modified(&t) {
            sends.push(SidecarCopy {
                from: m,
                to: t,
                replace: true,
            });
        } else {
            held = true;
        }
    }
    // An XMP over there that this frame has none of is the other side's
    // word alone.
    if theirs_xmps.iter().any(|t| !answered.contains(t)) {
        held = true;
    }
    (sends, held, join)
}

/// Whether a sidecar path is a frame's `.gcd` (and not an XMP).
fn is_gcd(path: &Path) -> bool {
    path.extension() == Some(std::ffi::OsStr::new("gcd"))
}

/// Join the two `.gcd`s of one frame and write the join to both
/// (§233): the side the copy is from gets the save, which makes the
/// revision, and the other side the same bytes; the archive write
/// behind a later save then finds the copies the same.
pub fn join_sidecars(join: &SidecarJoin) -> Result<(), String> {
    let ours = Sidecar::read(&join.mine_sidecar).map_err(|e| tr!("reading it: {e}", e = e))?;
    let theirs =
        Sidecar::read(&join.theirs_sidecar).map_err(|e| tr!("reading the copy: {e}", e = e))?;
    let mut joined = greycard_edit::sync::join(&ours, &theirs);
    let placement = if join.mine_sidecar.parent().and_then(Path::file_name)
        == Some(std::ffi::OsStr::new(SIDECAR_FOLDER))
    {
        greycard_edit::Placement::Folder
    } else {
        greycard_edit::Placement::Beside
    };
    joined
        .save_in(&join.mine, placement)
        .map_err(|e| tr!("writing it: {e}", e = e))?;
    joined
        .write_to(&join.theirs_sidecar)
        .map_err(|e| tr!("writing the copy: {e}", e = e))?;
    Ok(())
}

/// The content key of `frame` (§72's), from its row when the row is of
/// the file as it is now, else read from the file's head.
fn key_of(frame: &Path, meta: &std::fs::Metadata, lib: Option<&Library>) -> Option<String> {
    if let Some(lib) = lib
        && let Ok(Some(row)) = lib.by_path(frame)
        && row.size == meta.len()
        && meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|d| d.as_nanos() as i64 == row.mtime)
    {
        return Some(row.hash);
    }
    greycard_library::hash_file(frame).ok()
}

/// Whole-file hashes read to settle whether two files are one, kept to
/// be written to the index's rows when the job is done: path, size,
/// hash.
pub type Settled = RefCell<Vec<(PathBuf, u64, String)>>;

/// The whole-file hash the index keeps for `path`, trusted only while
/// the row's size and time are the file's now: a file written again in
/// place since its hash was taken is not given its old one.
fn whole_from_row(path: &Path, lib: Option<&Library>) -> Option<String> {
    let lib = lib?;
    let row = lib.by_path(path).ok()??;
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos() as i64;
    if row.size != meta.len() || row.mtime != mtime {
        return None;
    }
    lib.whole_hash(path).ok()?
}

/// The whole-file hash of `path`: the index's when it is the file's
/// now, one settled already in this job, or read now, every byte, with
/// a beat at every chunk, and kept in `settled` when the bytes read are
/// the size the file had before the read (a file growing or shrinking
/// under the read keeps nothing).
fn whole_of(
    path: &Path,
    lib: Option<&Library>,
    beat: &dyn Fn(),
    settled: &Settled,
) -> Option<String> {
    if let Some(h) = whole_from_row(path, lib) {
        return Some(h);
    }
    if let Some((_, _, h)) = settled.borrow().iter().find(|(p, _, _)| p == path) {
        return Some(h.clone());
    }
    let size = std::fs::metadata(path).ok()?.len();
    let (h, read) = crate::import::hash_whole_beating(path, beat).ok()?;
    if read != size {
        return None;
    }
    settled
        .borrow_mut()
        .push((path.to_path_buf(), read, h.clone()));
    Some(h)
}

/// Whether two files of one key and size are one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Same,
    Other,
    /// Not settled: their times differ and no whole hash of both is on
    /// file, and this look does not read them whole.
    Unknown,
}

/// Whether the file at `there`, of `frame`'s head and size, is the same
/// file. When the index keeps a whole hash of each, of the files as they
/// are now, they decide. When it does not and the two times agree (a
/// copy made here keeps the frame's time), the key does. Otherwise the
/// doubt (a picture written again under the same head and size, or a
/// copy that did not keep its time) is settled the hash-first way, both
/// files read whole and the hashes kept for the index, but only when
/// `settle` is given: the Back up sheet's plan, which the user asked
/// for. A count or the Delete sheet's look, which run by themselves,
/// read nothing whole and leave the pair unknown.
fn judged(
    frame: &Path,
    there: &Path,
    lib: Option<&Library>,
    beat: &dyn Fn(),
    settle: Option<&Settled>,
) -> Verdict {
    let same = |yes: bool| if yes { Verdict::Same } else { Verdict::Other };
    if let (Some(mine), Some(theirs)) = (whole_from_row(frame, lib), whole_from_row(there, lib)) {
        return same(mine == theirs);
    }
    let apart = match (modified(frame), modified(there)) {
        (Some(a), Some(b)) => {
            let d = if a > b {
                a.duration_since(b)
            } else {
                b.duration_since(a)
            };
            d.unwrap_or_default() > Duration::from_secs(2)
        }
        _ => false,
    };
    if !apart {
        return Verdict::Same;
    }
    let Some(settled) = settle else {
        return Verdict::Unknown;
    };
    match (
        whole_of(frame, lib, beat, settled),
        whole_of(there, lib, beat, settled),
    ) {
        (Some(a), Some(b)) => same(a == b),
        _ => Verdict::Other,
    }
}

/// What a look for a frame's copy found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Find {
    Copy(PathBuf),
    /// A file of its key and size that could not be told from it
    /// without reading both whole.
    Unknown,
    Absent,
}

/// Where a copy of `frame` (of `size`, with the content key `key`) is
/// under `roots`, by hash first, confirmed on the disk by existence and
/// size, and by the whole file when there is doubt ([`judged`], which
/// reads whole only with `settle`). `frame` itself is never its own copy.
pub fn find_copy(
    frame: &Path,
    size: u64,
    key: &str,
    roots: &[PathBuf],
    lib: Option<&Library>,
    beat: &dyn Fn(),
    settle: Option<&Settled>,
) -> Find {
    let Some(lib) = lib else {
        return Find::Absent;
    };
    let Ok(finds) = lib.by_hash_under(key, roots) else {
        return Find::Absent;
    };
    let mut unknown = false;
    for found in finds {
        beat();
        if found.path == frame {
            continue;
        }
        let there = std::fs::metadata(&found.path).is_ok_and(|m| m.is_file() && m.len() == size);
        if !there {
            continue;
        }
        match judged(frame, &found.path, Some(lib), beat, settle) {
            Verdict::Same => return Find::Copy(found.path),
            Verdict::Unknown => unknown = true,
            Verdict::Other => {}
        }
    }
    if unknown { Find::Unknown } else { Find::Absent }
}

/// The whole hashes a job settled, written to their rows (a row of
/// another size is not given one). The waits on the lock beat.
pub fn keep_whole(index: &Path, settled: &Settled, beat: &Beat) {
    let settled = settled.borrow();
    if settled.is_empty() {
        return;
    }
    let Some(mut lib) = open_index(index, true, beat) else {
        return;
    };
    for (p, size, h) in settled.iter() {
        beat();
        if let Err(e) = lib.set_whole_hash(p, *size, h) {
            tracing::warn!("archive: {}: {e}", p.display());
        }
    }
}

/// Anything at `to` that belongs to a frame of that name: its `.gcd`
/// beside it or under the hidden folder, or an XMP of its name. A frame
/// copied in beside one would read another frame's rating, flag or
/// edit, whatever its own sidecars are.
pub fn sidecar_there(to: &Path) -> Option<PathBuf> {
    [
        Sidecar::path_in(to, greycard_edit::Placement::Beside),
        Sidecar::path_in(to, greycard_edit::Placement::Folder),
    ]
    .into_iter()
    .find(|p| std::fs::symlink_metadata(p).is_ok())
    .or_else(|| greycard_edit_io::xmp::paths_of(to).into_iter().next())
}

/// The library opened on a job's thread with a busy handler that beats
/// while it waits on the lock: a wait on the index is the index's, not
/// a share that stopped answering (§215). The handler waits 10 ms a
/// try, up to rusqlite's own five seconds.
pub fn open_index(path: &Path, write: bool, beat: &Beat) -> Option<Library> {
    thread_local! {
        static BUSY_BEAT: RefCell<Option<Beat>> = const { RefCell::new(None) };
    }
    fn busy(tries: i32) -> bool {
        BUSY_BEAT.with(|b| {
            if let Some(beat) = &*b.borrow() {
                beat();
            }
        });
        if tries >= 500 {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
        true
    }
    BUSY_BEAT.with(|b| *b.borrow_mut() = Some(beat.clone()));
    match Library::open_with_busy(path, !write, busy) {
        Ok(lib) => Some(lib),
        Err(e) => {
            tracing::warn!("archive: the index: {e}");
            None
        }
    }
}

/// What a copy would do: each frame looked for on the other side and
/// at its destination, and its sidecars compared with the copy's.
/// `beat` is called at every frame, so a share that stops answering
/// can be told from a slow one.
pub fn plan(ask: &Ask, beat: &Beat) -> Plan {
    let (frames, rejects) = frames_of(ask, &**beat);
    let lib = ask
        .index
        .as_deref()
        .and_then(|p| open_index(p, false, beat));
    let settled = Settled::default();
    let mut items = Vec::with_capacity(frames.len());
    for frame in frames {
        beat();
        let meta = match std::fs::metadata(&frame) {
            Ok(m) if m.is_file() => m,
            Ok(_) => {
                items.push(Item {
                    frame,
                    size: 0,
                    what: What::Unreadable(tr!("not a file")),
                });
                continue;
            }
            Err(e) => {
                let why = e.to_string();
                items.push(Item {
                    frame,
                    size: 0,
                    what: What::Unreadable(why),
                });
                continue;
            }
        };
        let size = meta.len();
        let to = ask.dest_of(&frame);
        let key = key_of(&frame, &meta, lib.as_ref());
        let found = key.as_deref().and_then(|k| {
            match find_copy(
                &frame,
                size,
                k,
                &ask.other_side,
                lib.as_ref(),
                &**beat,
                Some(&settled),
            ) {
                Find::Copy(p) => Some(p),
                Find::Unknown | Find::Absent => None,
            }
        });
        // The destination itself, which the index may not know yet: a
        // copy made since its last pass.
        let found = found.or_else(|| {
            let keyed = std::fs::metadata(&to).is_ok_and(|m| m.is_file() && m.len() == size)
                && key.is_some()
                && greycard_library::hash_file(&to).ok() == key;
            (keyed && judged(&frame, &to, lib.as_ref(), &**beat, Some(&settled)) == Verdict::Same)
                .then(|| to.clone())
        });
        let what = match found {
            Some(there) => {
                let (sidecars, held, join) = if ask.sidecars {
                    compare_sidecars(&frame, &there)
                } else {
                    (Vec::new(), false, None)
                };
                match (sidecars.is_empty() && join.is_none(), held) {
                    (true, false) => What::Same {
                        elsewhere: there != to,
                        there,
                    },
                    (true, true) => What::TheirsNewer { there },
                    (false, held) => What::Sidecars {
                        there,
                        sidecars,
                        held,
                        join,
                    },
                }
            }
            None if std::fs::symlink_metadata(&to).is_ok() => What::Taken { to },
            None => {
                let sidecars: Vec<SidecarCopy> = if ask.sidecars {
                    travelling(&frame)
                        .into_iter()
                        .map(|s| SidecarCopy {
                            to: sidecar_at(&s, &frame, &to),
                            from: s,
                            replace: false,
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                // A sidecar left at the destination with no frame, of any
                // placement and whatever this frame has of its own (none
                // at all, or sidecars off): the frame would land beside
                // another's edit.
                match sidecar_there(&to) {
                    Some(left) => What::Taken { to: left },
                    None => What::Copy { to, sidecars },
                }
            }
        };
        items.push(Item { frame, size, what });
    }
    drop(lib);
    if let Some(index) = &ask.index {
        keep_whole(index, &settled, beat);
    }
    Plan {
        ask: ask.clone(),
        items,
        rejects,
    }
}

/// What a run of a plan is given to say it is moving, be stopped, and
/// count its bytes; and, in a test, a hand in each file.
pub struct Hooks<'a> {
    /// Called at every chunk read or written, and between files.
    pub beat: &'a dyn Fn(),
    /// Set to stop between frames.
    pub cancel: &'a AtomicBool,
    /// The frames' bytes copied so far, for the bar.
    pub bytes: &'a AtomicU64,
    /// A test's hand before each frame is copied.
    #[cfg(test)]
    pub before_frame: Option<&'a dyn Fn(&Path)>,
    /// A test's hand on each temporary file once written, before it is
    /// read back.
    #[cfg(test)]
    pub tamper: Option<&'a dyn Fn(&Path)>,
}

impl<'a> Hooks<'a> {
    pub fn new(beat: &'a dyn Fn(), cancel: &'a AtomicBool, bytes: &'a AtomicU64) -> Self {
        Hooks {
            beat,
            cancel,
            bytes,
            #[cfg(test)]
            before_frame: None,
            #[cfg(test)]
            tamper: None,
        }
    }
}

/// What a run did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// The frames copied whole: from, to, size, the whole file's hash.
    pub copied: Vec<(PathBuf, PathBuf, u64, String)>,
    /// Frames whose sidecars alone went, and how many sidecar files
    /// were copied in all.
    pub sidecars_only: usize,
    pub sidecar_files: usize,
    /// There already and the same, and of those under another folder.
    pub same: usize,
    pub elsewhere: usize,
    /// Left, wholly or in part, because sidecars there are the newer.
    pub theirs_newer: Vec<PathBuf>,
    /// Left because another file has the name there, or a sidecar's.
    pub taken: Vec<PathBuf>,
    /// Frames that failed, and why: a mismatch on the read-back among
    /// them, whose copy is removed.
    pub failed: Vec<(PathBuf, String)>,
    /// Temporaries a run before left, removed.
    pub stale: Vec<PathBuf>,
    /// Frames not reached because the run was canceled.
    pub canceled: usize,
    /// The folders written to, for the indexer.
    pub folders: Vec<PathBuf>,
    /// Collections and stacks carried between a folder and its other
    /// side (`greycard_library::carry::across`).
    pub sets: usize,
}

/// Remove the temporaries a run before left in `folder`: an hour since
/// their last write, and not of a process running here (a copy set
/// aside, or another window's). Another machine's live copy writes to
/// its temporary at every chunk, so the hour keeps it too.
pub fn sweep(folder: &Path) -> Vec<PathBuf> {
    let mut gone = Vec::new();
    let Ok(entries) = std::fs::read_dir(folder) else {
        return gone;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.starts_with('.')
            && name.ends_with(TEMPORARY)
            && e.file_type().is_ok_and(|t| t.is_file()))
        {
            continue;
        }
        if crate::import::temporary_pid_ending(&name, TEMPORARY).is_some_and(crate::import::alive) {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age > STALE_AFTER);
        if !old {
            continue;
        }
        match std::fs::remove_file(e.path()) {
            Ok(()) => gone.push(e.path()),
            Err(err) => tracing::warn!("archive: {}: {err}", e.path().display()),
        }
    }
    gone
}

/// Copy `from` to `to` by the import's verified landing: a temporary
/// beside `to`, the whole file's BLAKE3 as it streams, read back past
/// the cache, named `to` only when the two agree, `from`'s mtime kept.
/// `replace` lets it go over a file at `to` (an older sidecar); otherwise
/// one there is an error and left. The whole hash and the size.
pub fn copy_file(
    from: &Path,
    to: &Path,
    replace: bool,
    hooks: &Hooks<'_>,
    counted: bool,
) -> Result<(String, u64), String> {
    let mut src = std::fs::File::open(from).map_err(|e| tr!("reading it: {e}", e = e))?;
    let meta = src.metadata().map_err(|e| tr!("reading it: {e}", e = e))?;
    let how = crate::import::Landing {
        ending: TEMPORARY,
        replace,
        beat: hooks.beat,
        bytes: counted.then_some(hooks.bytes),
        #[cfg(test)]
        tamper: hooks.tamper,
    };
    crate::import::land_with(
        &mut src,
        meta.modified().ok(),
        Some(meta.len()),
        to,
        None,
        &how,
    )
    .map_err(|e| format!("{e:#}"))
}

/// Run a plan: one frame at a time, its sidecars after it, the cancel
/// looked at between frames. The temporaries a run before left in the
/// folders it writes to are removed first. Never deletes anything but
/// its own temporaries.
pub fn run(plan: &Plan, hooks: &Hooks<'_>) -> Report {
    let mut report = Report::default();
    // The folders it will write to, each once.
    let mut folders: Vec<PathBuf> = Vec::new();
    let note = |p: &Path, folders: &mut Vec<PathBuf>| {
        if let Some(d) = p.parent()
            && !folders.iter().any(|f| f == d)
        {
            folders.push(d.to_path_buf());
        }
    };
    for item in &plan.items {
        match &item.what {
            What::Copy { to, sidecars } => {
                note(to, &mut folders);
                for s in sidecars {
                    note(&s.to, &mut folders);
                }
            }
            What::Sidecars { sidecars, .. } => {
                for s in sidecars {
                    note(&s.to, &mut folders);
                }
            }
            _ => {}
        }
    }
    for f in &folders {
        (hooks.beat)();
        report.stale.extend(sweep(f));
    }
    let mut written: Vec<PathBuf> = Vec::new();
    for (n, item) in plan.items.iter().enumerate() {
        (hooks.beat)();
        if hooks.cancel.load(Ordering::SeqCst) {
            report.canceled = plan.items[n..]
                .iter()
                .filter(|i| matches!(i.what, What::Copy { .. } | What::Sidecars { .. }))
                .count();
            break;
        }
        match &item.what {
            What::Copy { to, sidecars } => {
                #[cfg(test)]
                if let Some(hand) = hooks.before_frame {
                    hand(&item.frame);
                }
                // Looked at again: a sidecar that appeared at a name since
                // the plan keeps the frame out, rather than land it beside
                // another's edit.
                if sidecar_there(to).is_some() {
                    report.taken.push(item.frame.clone());
                    continue;
                }
                match copy_file(&item.frame, to, false, hooks, true) {
                    Ok((whole, size)) => {
                        report
                            .copied
                            .push((item.frame.clone(), to.clone(), size, whole));
                        note(to, &mut written);
                        for s in sidecars {
                            match copy_file(&s.from, &s.to, false, hooks, false) {
                                Ok(_) => {
                                    report.sidecar_files += 1;
                                    note(&s.to, &mut written);
                                }
                                Err(why) => report.failed.push((s.from.clone(), why)),
                            }
                        }
                    }
                    Err(why) => report.failed.push((item.frame.clone(), why)),
                }
            }
            What::Sidecars {
                there,
                sidecars,
                held,
                join,
            } => {
                // The `.gcd`s compared again now: a save made here or
                // there between the plan and the run (the sheet was up)
                // is a copy the plan would write over, or a pair that is
                // a join now. What goes is what the files say at this
                // moment; the XMPs go as planned.
                let fresh;
                let (sidecars, join) = if sidecars.iter().any(|s| is_gcd(&s.from)) || join.is_some()
                {
                    fresh = compare_sidecars(&item.frame, there);
                    let xmps: Vec<&SidecarCopy> =
                        sidecars.iter().filter(|s| !is_gcd(&s.from)).collect();
                    let mut gcds: Vec<&SidecarCopy> =
                        fresh.0.iter().filter(|s| is_gcd(&s.from)).collect();
                    gcds.extend(xmps);
                    (gcds, fresh.2.as_ref())
                } else {
                    (sidecars.iter().collect(), join.as_ref())
                };
                let mut ok = true;
                if let Some(join) = join {
                    match join_sidecars(join) {
                        Ok(()) => {
                            report.sidecar_files += 2;
                            note(&join.mine_sidecar, &mut written);
                            note(&join.theirs_sidecar, &mut written);
                        }
                        Err(why) => {
                            ok = false;
                            report.failed.push((join.mine_sidecar.clone(), why));
                        }
                    }
                }
                for s in sidecars {
                    // Over the older copy only when this file was judged
                    // the newer; a new name never goes over one that
                    // appeared since.
                    match copy_file(&s.from, &s.to, s.replace, hooks, false) {
                        Ok(_) => {
                            report.sidecar_files += 1;
                            note(&s.to, &mut written);
                        }
                        Err(why) => {
                            ok = false;
                            report.failed.push((s.from.clone(), why));
                        }
                    }
                }
                if ok {
                    report.sidecars_only += 1;
                }
                if *held {
                    report.theirs_newer.push(item.frame.clone());
                }
            }
            What::Same { elsewhere, .. } => {
                report.same += 1;
                report.elsewhere += usize::from(*elsewhere);
            }
            What::TheirsNewer { .. } => report.theirs_newer.push(item.frame.clone()),
            What::Taken { .. } => report.taken.push(item.frame.clone()),
            What::Unreadable(why) => report.failed.push((item.frame.clone(), why.clone())),
        }
    }
    // The collections and stacks carried beside the frames go with
    // them, folder by folder, as the sidecars do.
    if plan.ask.sidecars && report.canceled == 0 {
        let mut pairs: Vec<(PathBuf, PathBuf)> = Vec::new();
        for item in &plan.items {
            let other = match &item.what {
                What::Copy { to, .. } => to,
                What::Sidecars { there, .. }
                | What::Same { there, .. }
                | What::TheirsNewer { there } => there,
                _ => continue,
            };
            if let (Some(a), Some(b)) = (item.frame.parent(), other.parent()) {
                let pair = (a.to_path_buf(), b.to_path_buf());
                if !pairs.contains(&pair) {
                    pairs.push(pair);
                }
            }
        }
        for (a, b) in pairs {
            (hooks.beat)();
            let done = greycard_library::carry::across(&a, &b);
            if done.apart > 0 {
                tracing::info!(
                    "archive: {} carried sets in {} went their own ways; joined when read",
                    done.apart,
                    b.display()
                );
            }
            if done.written > 0 {
                report.sets += done.written;
                for f in [a, b] {
                    if !written.contains(&f) {
                        written.push(f);
                    }
                }
            }
        }
    }
    // A sidecar under the hidden folder is its frame's folder's.
    report.folders = written
        .into_iter()
        .map(|d| {
            if d.file_name() == Some(std::ffi::OsStr::new(SIDECAR_FOLDER)) {
                d.parent().map(Path::to_path_buf).unwrap_or(d)
            } else {
                d
            }
        })
        .fold(Vec::new(), |mut all, d| {
            if !all.contains(&d) {
                all.push(d);
            }
            all
        });
    report
}

/// The copies' rows written, each with the whole file's hash, and the
/// source's row given the same hash: what a verify of the archive will
/// want later (§216). A row is written for a copy the index has not
/// seen yet; a row of another size is not given the hash. The waits on
/// the library's lock beat.
pub fn record(index: &Path, report: &Report, beat: &Beat) {
    if report.copied.is_empty() {
        return;
    }
    let Some(mut lib) = open_index(index, true, beat) else {
        return;
    };
    for (from, to, size, whole) in &report.copied {
        beat();
        if let Err(e) = lib.index_file(to) {
            tracing::warn!("archive: {}: {e}", to.display());
        }
        beat();
        for p in [to, from] {
            if let Err(e) = lib.set_whole_hash(p, *size, whole) {
                tracing::warn!("archive: {}: {e}", p.display());
            }
        }
    }
}

/// Where one frame is, over the archives looked at: the indices of
/// those that hold a copy, and of those that hold a file of its key and
/// size that cannot be told from it without reading both whole.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Where {
    pub on: Vec<usize>,
    pub unknown: Vec<usize>,
}

/// Which archives hold a copy of each of `frames`, by hash and confirmed
/// on the disk. What the header's count and the Delete sheet's line
/// read: these run by themselves, so nothing is read whole here, and a
/// pair only a whole read would settle is unknown (the Back up sheet's
/// plan settles it). None when `stop` said to stop, between frames: a
/// newer ask has overtaken this one.
pub fn on_archives(
    frames: &[PathBuf],
    archives: &[PathBuf],
    lib: Option<&Library>,
    beat: &dyn Fn(),
    stop: &dyn Fn() -> bool,
) -> Option<Vec<Where>> {
    let mut out = Vec::with_capacity(frames.len());
    for frame in frames {
        beat();
        if stop() {
            return None;
        }
        let mut here = Where::default();
        let Ok(meta) = std::fs::metadata(frame) else {
            out.push(here);
            continue;
        };
        let Some(key) = key_of(frame, &meta, lib) else {
            out.push(here);
            continue;
        };
        for (i, a) in archives.iter().enumerate() {
            match find_copy(
                frame,
                meta.len(),
                &key,
                std::slice::from_ref(a),
                lib,
                beat,
                None,
            ) {
                Find::Copy(_) => here.on.push(i),
                Find::Unknown => here.unknown.push(i),
                Find::Absent => {}
            }
        }
        out.push(here);
    }
    Some(out)
}

/// The rows of a view of every root with the archives' copies of local
/// frames left out: a row under an archive whose content key is also
/// under a local root is the same frame shown twice (§216). The local
/// copy is the one kept while it is there: a local row that `present`
/// says is not (its root offline, its folder gone) hides nothing, and
/// the frame is listed from the archive instead, where it can be
/// opened. Nothing is asked of the disk here; `present` is what the
/// caller already knows.
pub fn hide_local_copies<T>(
    rows: Vec<(PathBuf, T)>,
    archives: &[PathBuf],
    key: impl Fn(&T) -> &str,
    present: impl Fn(&Path) -> bool,
) -> Vec<(PathBuf, T)> {
    if archives.is_empty() {
        return rows;
    }
    let on_archive = |p: &Path| archives.iter().any(|a| p.starts_with(a));
    let local: std::collections::HashSet<String> = rows
        .iter()
        .filter(|(p, _)| !on_archive(p) && present(p))
        .map(|(_, r)| key(r).to_owned())
        .collect();
    rows.into_iter()
        .filter(|(p, r)| !on_archive(p) || !local.contains(key(r)))
        .collect()
}

/// How a supervised job ended for the window.
pub enum Heard<R> {
    /// Done, in time; or done after it was set aside (`late`).
    Done { result: R, late: bool },
    /// Quiet for the wait: set aside. It goes on alone, and its
    /// `Done` comes later, late.
    Aside,
    /// Its thread ended without a result (a panic): nothing more will
    /// come of it.
    Lost,
}

/// Run `job` on a thread of its own with a beat it calls as it goes,
/// and hand what happens to `heard`, on a second thread that watches
/// the beats: `Aside` once if it goes quiet for `wait` (a share that
/// answered its look and then stopped), and `Done` when it ends, late
/// or not. As §215's lanes do for the indexer's passes, a hung job
/// holds its own thread and nothing of the window's.
pub fn supervise<R: Send + 'static>(
    name: &str,
    wait: Duration,
    job: impl FnOnce(&Beat) -> R + Send + 'static,
    heard: impl Fn(Heard<R>) + Send + 'static,
) -> std::io::Result<()> {
    enum Back<R> {
        Beat,
        Done(R),
    }
    let (tx, rx) = mpsc::channel::<Back<R>>();
    let beats = tx.clone();
    std::thread::Builder::new()
        .name(format!("greycard {name}"))
        .spawn(move || {
            let beat: Beat = Arc::new(move || {
                let _ = beats.send(Back::Beat);
            });
            let out = job(&beat);
            let _ = tx.send(Back::Done(out));
        })?;
    std::thread::Builder::new()
        .name(format!("greycard {name} watch"))
        .spawn(move || {
            let tick = wait.min(Duration::from_millis(200));
            let mut last = Instant::now();
            let mut aside = false;
            loop {
                match rx.recv_timeout(tick) {
                    Ok(Back::Beat) => last = Instant::now(),
                    Ok(Back::Done(result)) => {
                        heard(Heard::Done {
                            result,
                            late: aside,
                        });
                        return;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if !aside && last.elapsed() >= wait {
                            aside = true;
                            heard(Heard::Aside);
                        }
                    }
                    // The job's thread died without a word (a panic):
                    // said, so the window lets go of it.
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        heard(Heard::Lost);
                        return;
                    }
                }
            }
        })?;
    Ok(())
}

/// A size in words: "12.3 MB", "512 KB", "40 bytes".
pub fn size_words(bytes: u64) -> String {
    const MB: f64 = 1_000_000.0;
    match bytes {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.1} MB", b as f64 / MB),
        b if b >= 1_000 => format!("{} KB", b / 1_000),
        b => tr!("{n} byte" | "{n} bytes" % b),
    }
}

#[cfg(test)]
mod tests;
