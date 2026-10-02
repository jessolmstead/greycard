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

pub(crate) mod rejects;

/// The ending of a copy's temporary name, which a later run removes.
pub(crate) const TEMPORARY: &str = ".greycard-backup";

/// How old a temporary has to be before a run takes it for one a crash
/// left: a copy under way writes to its temporary at every chunk, so an
/// hour without a write is no copy under way.
const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// A job's word that it is still moving: shared, so a wait on the
/// library's lock can say it too.
pub(crate) type Beat = Arc<dyn Fn() + Send + Sync>;

/// A beat nobody listens to.
#[cfg(test)]
pub(crate) fn no_beat() -> Beat {
    Arc::new(|| {})
}

/// Which way the copy goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    /// From here to an archive.
    BackUp,
    /// From an archive to a local root.
    BringBack,
}

/// The frames a copy is over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Frames {
    /// These frames, as chosen.
    Chosen(Vec<PathBuf>),
    /// A folder and every folder under it: the hidden ones left out,
    /// the other side's roots, and the rejects folders unless the ask
    /// includes them.
    Folder(PathBuf),
}

/// A copy asked for: everything the look needs, taken from the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ask {
    pub(crate) direction: Direction,
    pub(crate) frames: Frames,
    /// Walk a folder's rejects folders too.
    pub(crate) include_rejects: bool,
    /// Carry the sidecars: false under `--no-sidecars`, which neither
    /// reads nor writes one.
    pub(crate) sidecars: bool,
    /// Where each frame's path is taken relative to.
    pub(crate) base: PathBuf,
    /// Where each frame goes: this folder, at its path under `base`.
    pub(crate) dest: PathBuf,
    /// Where a copy already there is looked for by hash: the archive
    /// for a backup, the local roots for a bring-back.
    pub(crate) other_side: Vec<PathBuf>,
    /// The roots a folder's walk never goes into: every archive for a
    /// backup (a folder of no root's with an archive inside it), the
    /// local roots for a bring-back.
    pub(crate) skip: Vec<PathBuf>,
    /// The library's database, read for the hashes, and written with
    /// the copies' rows and their whole-file hashes.
    pub(crate) index: Option<PathBuf>,
}

impl Ask {
    /// Where `frame` goes.
    pub(crate) fn dest_of(&self, frame: &Path) -> PathBuf {
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
pub(crate) struct SidecarCopy {
    pub(crate) from: PathBuf,
    pub(crate) to: PathBuf,
    /// Over an older copy of it there, judged older file by file.
    pub(crate) replace: bool,
}

/// What is to be done with one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum What {
    /// Not on the other side: the frame and its sidecars copied.
    Copy {
        to: PathBuf,
        sidecars: Vec<SidecarCopy>,
    },
    /// There, at `there`, and some of its sidecars here are the newer:
    /// those alone copied. `held` when others there are the newer and
    /// are left, which names the frame.
    Sidecars {
        there: PathBuf,
        sidecars: Vec<SidecarCopy>,
        held: bool,
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
pub(crate) struct Item {
    pub(crate) frame: PathBuf,
    pub(crate) size: u64,
    pub(crate) what: What,
}

/// What a copy would do, before anything is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) ask: Ask,
    pub(crate) items: Vec<Item>,
    /// Frames in the rejects folders a folder's walk found, left out
    /// unless the ask includes them.
    pub(crate) rejects: usize,
}

impl Plan {
    /// The frames to copy whole, and their bytes.
    pub(crate) fn copies(&self) -> (usize, u64) {
        self.items
            .iter()
            .filter(|i| matches!(i.what, What::Copy { .. }))
            .fold((0, 0), |(n, b), i| (n + 1, b + i.size))
    }

    /// How many frames go as sidecars alone.
    pub(crate) fn sidecars_only(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i.what, What::Sidecars { .. }))
            .count()
    }

    /// How many are there already, and how many of those under
    /// another folder than this frame's would be.
    pub(crate) fn same(&self) -> (usize, usize) {
        self.items.iter().fold((0, 0), |(n, e), i| match i.what {
            What::Same { elsewhere, .. } => (n + 1, e + usize::from(elsewhere)),
            _ => (n, e),
        })
    }

    /// The frames some of whose sidecars over there are the newer, left
    /// as they are: all of them, or beside the newer ones that go.
    pub(crate) fn theirs_newer(&self) -> Vec<&Path> {
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

    pub(crate) fn taken(&self) -> Vec<&Path> {
        self.items
            .iter()
            .filter(|i| matches!(i.what, What::Taken { .. }))
            .map(|i| i.frame.as_path())
            .collect()
    }

    pub(crate) fn unreadable(&self) -> Vec<(&Path, &str)> {
        self.items
            .iter()
            .filter_map(|i| match &i.what {
                What::Unreadable(why) => Some((i.frame.as_path(), why.as_str())),
                _ => None,
            })
            .collect()
    }

    /// Whether running it would copy anything at all.
    pub(crate) fn has_work(&self) -> bool {
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
pub(crate) fn walk(
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
                let rejected = in_rejects || name == std::ffi::OsStr::new(crate::cull::REJECTS);
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
pub(crate) fn frames_of(ask: &Ask, beat: &dyn Fn()) -> (Vec<PathBuf>, usize) {
    match &ask.frames {
        Frames::Chosen(f) => (f.clone(), 0),
        Frames::Folder(dir) => walk(dir, ask.include_rejects, &ask.skip, beat, &|| false),
    }
}

/// The sidecars that travel with a frame: the `.gcd` the editor would
/// read ([`Sidecar::find`], beside or under the hidden folder), and the
/// frame's XMPs.
pub(crate) fn travelling(frame: &Path) -> Vec<PathBuf> {
    Sidecar::find(frame)
        .into_iter()
        .chain(greycard_edit::xmp::paths_of(frame))
        .collect()
}

/// Where `sidecar`, one of `frame`'s, goes for the frame at `to`: the
/// same placement, its name taken over from the frame's to `to`'s.
pub(crate) fn sidecar_at(sidecar: &Path, frame: &Path, to: &Path) -> PathBuf {
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
/// no counterpart). The `.gcd` by §161's rule, going over the one
/// `theirs` has wherever that is, so the copy never ends up with two;
/// each XMP by its mtime against the one its name gives it there. The
/// same bytes on both sides is nothing to do.
pub(crate) fn compare_sidecars(mine: &Path, theirs: &Path) -> (Vec<SidecarCopy>, bool) {
    let mut sends = Vec::new();
    let mut held = false;
    match (Sidecar::find(mine), Sidecar::find(theirs)) {
        (Some(m), None) => sends.push(SidecarCopy {
            to: sidecar_at(&m, mine, theirs),
            from: m,
            replace: false,
        }),
        (None, Some(_)) => held = true,
        (Some(m), Some(t)) if bytes_of(&m) != bytes_of(&t) => {
            if Sidecar::compare_copies(&m, &t) == std::cmp::Ordering::Greater {
                sends.push(SidecarCopy {
                    from: m,
                    to: t,
                    replace: true,
                });
            } else {
                held = true;
            }
        }
        _ => {}
    }
    let theirs_xmps = greycard_edit::xmp::paths_of(theirs);
    let mut answered = Vec::new();
    for m in greycard_edit::xmp::paths_of(mine) {
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
    (sends, held)
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
pub(crate) type Settled = RefCell<Vec<(PathBuf, u64, String)>>;

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
pub(crate) enum Verdict {
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
pub(crate) enum Find {
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
pub(crate) fn find_copy(
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
pub(crate) fn keep_whole(index: &Path, settled: &Settled, beat: &Beat) {
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
pub(crate) fn sidecar_there(to: &Path) -> Option<PathBuf> {
    [
        Sidecar::path_in(to, greycard_edit::Placement::Beside),
        Sidecar::path_in(to, greycard_edit::Placement::Folder),
    ]
    .into_iter()
    .find(|p| std::fs::symlink_metadata(p).is_ok())
    .or_else(|| greycard_edit::xmp::paths_of(to).into_iter().next())
}

/// The library opened on a job's thread with a busy handler that beats
/// while it waits on the lock: a wait on the index is the index's, not
/// a share that stopped answering (§215). The handler waits 10 ms a
/// try, up to rusqlite's own five seconds.
pub(crate) fn open_index(path: &Path, write: bool, beat: &Beat) -> Option<Library> {
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
pub(crate) fn plan(ask: &Ask, beat: &Beat) -> Plan {
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
                    what: What::Unreadable("not a file".into()),
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
                let (sidecars, held) = if ask.sidecars {
                    compare_sidecars(&frame, &there)
                } else {
                    (Vec::new(), false)
                };
                match (sidecars.is_empty(), held) {
                    (true, false) => What::Same {
                        elsewhere: there != to,
                        there,
                    },
                    (true, true) => What::TheirsNewer { there },
                    (false, held) => What::Sidecars {
                        there,
                        sidecars,
                        held,
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
pub(crate) struct Hooks<'a> {
    /// Called at every chunk read or written, and between files.
    pub(crate) beat: &'a dyn Fn(),
    /// Set to stop between frames.
    pub(crate) cancel: &'a AtomicBool,
    /// The frames' bytes copied so far, for the bar.
    pub(crate) bytes: &'a AtomicU64,
    /// A test's hand before each frame is copied.
    #[cfg(test)]
    pub(crate) before_frame: Option<&'a dyn Fn(&Path)>,
    /// A test's hand on each temporary file once written, before it is
    /// read back.
    #[cfg(test)]
    pub(crate) tamper: Option<&'a dyn Fn(&Path)>,
}

impl<'a> Hooks<'a> {
    pub(crate) fn new(beat: &'a dyn Fn(), cancel: &'a AtomicBool, bytes: &'a AtomicU64) -> Self {
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
pub(crate) struct Report {
    /// The frames copied whole: from, to, size, the whole file's hash.
    pub(crate) copied: Vec<(PathBuf, PathBuf, u64, String)>,
    /// Frames whose sidecars alone went, and how many sidecar files
    /// were copied in all.
    pub(crate) sidecars_only: usize,
    pub(crate) sidecar_files: usize,
    /// There already and the same, and of those under another folder.
    pub(crate) same: usize,
    pub(crate) elsewhere: usize,
    /// Left, wholly or in part, because sidecars there are the newer.
    pub(crate) theirs_newer: Vec<PathBuf>,
    /// Left because another file has the name there, or a sidecar's.
    pub(crate) taken: Vec<PathBuf>,
    /// Frames that failed, and why: a mismatch on the read-back among
    /// them, whose copy is removed.
    pub(crate) failed: Vec<(PathBuf, String)>,
    /// Temporaries a run before left, removed.
    pub(crate) stale: Vec<PathBuf>,
    /// Frames not reached because the run was canceled.
    pub(crate) canceled: usize,
    /// The folders written to, for the indexer.
    pub(crate) folders: Vec<PathBuf>,
}

/// Remove the temporaries a run before left in `folder`: an hour since
/// their last write, and not of a process running here (a copy set
/// aside, or another window's). Another machine's live copy writes to
/// its temporary at every chunk, so the hour keeps it too.
pub(crate) fn sweep(folder: &Path) -> Vec<PathBuf> {
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
pub(crate) fn copy_file(
    from: &Path,
    to: &Path,
    replace: bool,
    hooks: &Hooks<'_>,
    counted: bool,
) -> Result<(String, u64), String> {
    let mut src = std::fs::File::open(from).map_err(|e| format!("reading it: {e}"))?;
    let meta = src.metadata().map_err(|e| format!("reading it: {e}"))?;
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
pub(crate) fn run(plan: &Plan, hooks: &Hooks<'_>) -> Report {
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
            What::Sidecars { sidecars, held, .. } => {
                let mut ok = true;
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
pub(crate) fn record(index: &Path, report: &Report, beat: &Beat) {
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
pub(crate) struct Where {
    pub(crate) on: Vec<usize>,
    pub(crate) unknown: Vec<usize>,
}

/// Which archives hold a copy of each of `frames`, by hash and confirmed
/// on the disk. What the header's count and the Delete sheet's line
/// read: these run by themselves, so nothing is read whole here, and a
/// pair only a whole read would settle is unknown (the Back up sheet's
/// plan settles it). None when `stop` said to stop, between frames: a
/// newer ask has overtaken this one.
pub(crate) fn on_archives(
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
/// copy is the one kept. Nothing is asked of the disk.
pub(crate) fn hide_local_copies<T>(
    rows: Vec<(PathBuf, T)>,
    archives: &[PathBuf],
    key: impl Fn(&T) -> &str,
) -> Vec<(PathBuf, T)> {
    if archives.is_empty() {
        return rows;
    }
    let on_archive = |p: &Path| archives.iter().any(|a| p.starts_with(a));
    let local: std::collections::HashSet<String> = rows
        .iter()
        .filter(|(p, _)| !on_archive(p))
        .map(|(_, r)| key(r).to_owned())
        .collect();
    rows.into_iter()
        .filter(|(p, r)| !on_archive(p) || !local.contains(key(r)))
        .collect()
}

/// How a supervised job ended for the window.
pub(crate) enum Heard<R> {
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
pub(crate) fn supervise<R: Send + 'static>(
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
pub(crate) fn size_words(bytes: u64) -> String {
    const MB: f64 = 1_000_000.0;
    match bytes {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.1} MB", b as f64 / MB),
        b if b >= 1_000 => format!("{} KB", b / 1_000),
        b => format!("{b} bytes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_edit::Placement;
    use greycard_library::fixture::{A7, R5, R6, write_frame};
    use std::sync::Arc;

    pub(crate) fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-ui-archive-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dunce::canonicalize(&dir).unwrap()
    }

    /// A rated sidecar on `frame`, saved `times` times, in `placement`.
    fn rate(frame: &Path, stars: u8, times: u32, placement: Placement) {
        let mut s = Sidecar::load(frame).ok().flatten().unwrap_or_default();
        s.meta.rating = stars;
        for _ in 0..times {
            s.save_in(frame, placement).unwrap();
        }
    }

    /// A shoot of three frames under `root/shoot`, the second rated
    /// beside, the third rated under the hidden folder; and a rejects
    /// folder in the shoot with one frame.
    fn shoot(root: &Path) -> Vec<PathBuf> {
        let dir = root.join("shoot");
        std::fs::create_dir_all(dir.join(crate::cull::REJECTS)).unwrap();
        let files: Vec<PathBuf> = ["a.tif", "b.tif", "c.tif"]
            .iter()
            .map(|n| dir.join(n))
            .collect();
        for (i, (f, cam)) in files.iter().zip([&R5, &R6, &A7]).enumerate() {
            write_frame(f, cam, i as u16 + 1);
        }
        rate(&files[1], 3, 1, Placement::Beside);
        rate(&files[2], 2, 1, Placement::Folder);
        write_frame(&dir.join(crate::cull::REJECTS).join("r.tif"), &R5, 9);
        files
    }

    fn quiet_hooks<'a>(cancel: &'a AtomicBool, bytes: &'a AtomicU64) -> Hooks<'a> {
        Hooks::new(&|| {}, cancel, bytes)
    }

    fn index(db: &Path, roots: &[&Path]) {
        let mut lib = Library::open(db).unwrap();
        for r in roots {
            lib.index_tree(r, &mut |_| {}).unwrap();
        }
    }

    fn backup_ask(local: &Path, nas: &Path, db: &Path, rejects: bool) -> Ask {
        Ask {
            direction: Direction::BackUp,
            frames: Frames::Folder(local.join("shoot")),
            include_rejects: rejects,
            sidecars: true,
            base: local.to_path_buf(),
            dest: nas.join("local"),
            other_side: vec![nas.to_path_buf()],
            skip: vec![nas.to_path_buf()],
            index: Some(db.to_path_buf()),
        }
    }

    #[test]
    fn a_folder_is_backed_up_whole_with_its_sidecars_in_place_and_the_rejects_left() {
        let dir = scratch("backup");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        std::fs::create_dir_all(&nas).unwrap();
        let files = shoot(&local);
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let ask = backup_ask(&local, &nas, &db, false);
        let plan = plan(&ask, &no_beat());
        assert_eq!(plan.rejects, 1);
        assert_eq!(plan.copies().0, 3);
        let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
        let report = run(&plan, &quiet_hooks(&cancel, &bytes));
        assert_eq!(report.copied.len(), 3, "{report:?}");
        assert!(report.failed.is_empty(), "{report:?}");
        assert_eq!(report.sidecar_files, 2);
        let there = nas.join("local").join("shoot");
        for f in &files {
            let copy = there.join(f.file_name().unwrap());
            assert_eq!(std::fs::read(f).unwrap(), std::fs::read(&copy).unwrap());
            // The time kept, so the index's size and mtime agree.
            assert_eq!(
                std::fs::metadata(f).unwrap().modified().unwrap(),
                std::fs::metadata(&copy).unwrap().modified().unwrap()
            );
        }
        // The placements kept: b beside, c under the hidden folder.
        assert!(there.join("b.tif.gcd").is_file());
        assert!(there.join(SIDECAR_FOLDER).join("c.tif.gcd").is_file());
        assert!(!there.join("c.tif.gcd").exists());
        // The rejects left out, and nothing temporary left behind.
        assert!(!there.join(crate::cull::REJECTS).exists());
        let temps = std::fs::read_dir(&there)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(TEMPORARY))
            .count();
        assert_eq!(temps, 0);
        assert_eq!(
            bytes.load(Ordering::Relaxed),
            files
                .iter()
                .map(|f| std::fs::metadata(f).unwrap().len())
                .sum::<u64>()
        );
        assert_eq!(report.folders, std::slice::from_ref(&there));

        // The rows written, with their whole hashes, on both sides.
        record(&db, &report, &no_beat());
        let lib = Library::open_read_only(&db).unwrap();
        let whole = crate::import::hash_whole(&files[0]).unwrap();
        assert_eq!(lib.whole_hash(&files[0]).unwrap(), Some(whole.clone()));
        assert_eq!(lib.whole_hash(&there.join("a.tif")).unwrap(), Some(whole));
        // Closed before the folder goes: Windows refuses to remove an
        // open file.
        drop(lib);

        // Asked again: everything there, nothing to do.
        let again = super::plan(&ask, &no_beat());
        assert!(!again.has_work(), "{again:?}");
        assert_eq!(again.same(), (3, 0));

        // The rejects asked for: the one frame, under its folder.
        let with = super::plan(&backup_ask(&local, &nas, &db, true), &no_beat());
        assert_eq!(with.rejects, 1);
        assert_eq!(with.copies().0, 1);
        run(&with, &quiet_hooks(&cancel, &bytes));
        assert!(there.join(crate::cull::REJECTS).join("r.tif").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A shoot renamed here after its backup, or an archive reorganized
    /// by hand, is still backed up: found by its hash, anywhere under
    /// the archive, and nothing copied a second time.
    #[test]
    fn a_frame_already_on_the_archive_under_another_folder_is_not_copied_again() {
        let dir = scratch("hash-first");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let files = shoot(&local);
        let elsewhere = nas.join("clients").join("by hand");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::copy(&files[0], elsewhere.join("renamed.tif")).unwrap();
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
        let a = plan.items.iter().find(|i| i.frame == files[0]).unwrap();
        assert_eq!(
            a.what,
            What::Same {
                there: elsewhere.join("renamed.tif"),
                elsewhere: true
            }
        );
        assert_eq!(plan.same(), (1, 1));
        assert_eq!(plan.copies().0, 2);

        // A row with no file behind it is a frame not there.
        std::fs::remove_file(elsewhere.join("renamed.tif")).unwrap();
        let plan = super::plan(&backup_ask(&local, &nas, &db, false), &no_beat());
        assert_eq!(plan.copies().0, 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_copy_that_does_not_read_back_the_same_is_removed_and_named() {
        let dir = scratch("mismatch");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        std::fs::create_dir_all(&nas).unwrap();
        let files = shoot(&local);
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
        let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
        // The temporary of a.tif turned bad between the write and the
        // read-back, as a failing disk or a cable pulled would.
        let tamper = |tmp: &Path| {
            if tmp.to_string_lossy().contains(".a.tif.") {
                let mut b = std::fs::read(tmp).unwrap();
                b[100] ^= 0xff;
                std::fs::write(tmp, b).unwrap();
            }
        };
        let hooks = Hooks {
            tamper: Some(&tamper),
            ..quiet_hooks(&cancel, &bytes)
        };
        let report = run(&plan, &hooks);
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert_eq!(report.failed[0].0, files[0]);
        assert!(report.failed[0].1.contains("does not read back the same"));
        let there = nas.join("local").join("shoot");
        assert!(!there.join("a.tif").exists());
        assert!(there.join("b.tif").is_file());
        let left: Vec<_> = std::fs::read_dir(&there)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(TEMPORARY))
            .collect();
        assert!(left.is_empty(), "the temporary removed");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_temporary_left_by_a_crash_is_removed_by_the_next_run_and_said() {
        let dir = scratch("stale");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let there = nas.join("local").join("shoot");
        std::fs::create_dir_all(&there).unwrap();
        shoot(&local);
        // A process that cannot be running here, an hour and more ago.
        let dead = u32::MAX;
        let aged = |p: &Path| {
            std::fs::write(p, b"half a frame").unwrap();
            let then = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
            std::fs::File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_modified(then)
                .unwrap();
        };
        let stale = there.join(format!(".a.tif.{dead}-0{TEMPORARY}"));
        aged(&stale);
        // Written to a moment ago: a copy under way on another machine.
        let fresh = there.join(format!(".b.tif.{dead}-1{TEMPORARY}"));
        std::fs::write(&fresh, b"being written").unwrap();
        // This process's own, set aside and still writing, however old.
        let own = there.join(format!(".c.tif.{}-2{TEMPORARY}", std::process::id()));
        aged(&own);
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
        // Never counted as the frame.
        assert_eq!(plan.copies().0, 3);
        let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
        let report = run(&plan, &quiet_hooks(&cancel, &bytes));
        assert_eq!(report.stale, std::slice::from_ref(&stale));
        assert!(!stale.exists());
        assert!(fresh.exists() && own.exists(), "a live writer's are kept");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A frame there already takes the newer sidecar across, in either
    /// direction: here's when it is the newer, nothing (and named) when
    /// the archive's is.
    #[test]
    fn a_newer_sidecar_goes_across_alone_and_an_older_one_is_left_and_named() {
        let dir = scratch("sidecars");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        std::fs::create_dir_all(&nas).unwrap();
        let files = shoot(&local);
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let ask = backup_ask(&local, &nas, &db, false);
        let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
        run(&plan(&ask, &no_beat()), &quiet_hooks(&cancel, &bytes));
        let there = nas.join("local").join("shoot");
        // Here: b culled again after the backup (saved twice more).
        rate(&files[1], 5, 2, Placement::Beside);
        // There: c edited on the archive's side, from another machine.
        rate(&there.join("c.tif"), 4, 3, Placement::Folder);
        // And a, never rated here, rated on the archive.
        rate(&there.join("a.tif"), 1, 1, Placement::Beside);
        index(&db, &[&local, &nas]);
        let plan = plan(&ask, &no_beat());
        assert_eq!(plan.copies().0, 0, "{plan:?}");
        assert_eq!(plan.sidecars_only(), 1);
        let mut named: Vec<&Path> = plan.theirs_newer();
        named.sort();
        assert_eq!(named, [files[0].as_path(), files[2].as_path()]);
        let report = run(&plan, &quiet_hooks(&cancel, &bytes));
        assert_eq!((report.sidecars_only, report.sidecar_files), (1, 1));
        let read = |f: &Path| Sidecar::load(f).unwrap().unwrap().meta.rating;
        assert_eq!(read(&there.join("b.tif")), 5);
        assert_eq!(
            read(&there.join("c.tif")),
            4,
            "the archive's newer edit kept"
        );
        assert_eq!(read(&files[2]), 2, "and nothing written here");

        // Bring back, the other way: c's newer sidecar comes home, and
        // b, the same on both sides now, is skipped.
        let back = Ask {
            direction: Direction::BringBack,
            frames: Frames::Folder(there.clone()),
            include_rejects: false,
            sidecars: true,
            base: nas.join("local"),
            dest: local.clone(),
            other_side: vec![local.clone()],
            skip: vec![local.clone()],
            index: Some(db.clone()),
        };
        index(&db, &[&local, &nas]);
        let plan = super::plan(&back, &no_beat());
        assert_eq!(plan.copies().0, 0, "{plan:?}");
        assert_eq!(plan.sidecars_only(), 2, "a and c: {plan:?}");
        assert_eq!(plan.same(), (1, 0));
        run(&plan, &quiet_hooks(&cancel, &bytes));
        assert_eq!(read(&files[2]), 4);
        assert_eq!(read(&files[0]), 1);
        // c's sidecar came back to where c keeps its own: the hidden folder.
        assert!(!files[2].with_file_name("c.tif.gcd").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Bring back skips a frame already under a local root, wherever it
    /// is there, and names it when the local sidecar is the newer.
    #[test]
    fn bring_back_skips_a_frame_already_local() {
        let dir = scratch("bring-back");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let on_nas = nas.join("2024").join("shoot");
        std::fs::create_dir_all(&on_nas).unwrap();
        let old = ["x.tif", "y.tif"].map(|n| on_nas.join(n));
        write_frame(&old[0], &R5, 1);
        write_frame(&old[1], &R6, 2);
        // y is here already, under another name and folder, and rated
        // here since.
        let mine = local.join("kept").join("y-copy.tif");
        std::fs::create_dir_all(mine.parent().unwrap()).unwrap();
        std::fs::copy(&old[1], &mine).unwrap();
        rate(&mine, 5, 1, Placement::Beside);
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let ask = Ask {
            direction: Direction::BringBack,
            frames: Frames::Chosen(old.to_vec()),
            include_rejects: false,
            sidecars: true,
            base: nas.clone(),
            dest: local.join("from-archive"),
            other_side: vec![local.clone()],
            skip: vec![local.clone()],
            index: Some(db.clone()),
        };
        let plan = plan(&ask, &no_beat());
        assert_eq!(plan.copies().0, 1);
        assert_eq!(plan.theirs_newer(), [old[1].as_path()]);
        let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
        let report = run(&plan, &quiet_hooks(&cancel, &bytes));
        assert_eq!(report.copied.len(), 1);
        assert!(
            local
                .join("from-archive")
                .join("2024")
                .join("shoot")
                .join("x.tif")
                .is_file()
        );
        assert!(
            !local
                .join("from-archive")
                .join("2024")
                .join("shoot")
                .join("y.tif")
                .exists()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Another file at the destination's name is never written over.
    #[test]
    fn a_different_file_at_the_name_is_left_and_named() {
        let dir = scratch("taken");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let files = shoot(&local);
        let there = nas.join("local").join("shoot");
        std::fs::create_dir_all(&there).unwrap();
        std::fs::write(there.join("a.tif"), b"another picture").unwrap();
        let db = dir.join("library.sqlite");
        index(&db, &[&local]);
        let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
        assert_eq!(plan.taken(), [files[0].as_path()]);
        let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
        run(&plan, &quiet_hooks(&cancel, &bytes));
        assert_eq!(
            std::fs::read(there.join("a.tif")).unwrap(),
            b"another picture"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_cancel_stops_between_frames() {
        let dir = scratch("cancel");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        std::fs::create_dir_all(&nas).unwrap();
        shoot(&local);
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
        let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
        // Canceled while the first frame is copied: it finishes, and
        // the rest are not begun.
        let hand = |_: &Path| cancel.store(true, Ordering::SeqCst);
        let hooks = Hooks {
            before_frame: Some(&hand),
            ..quiet_hooks(&cancel, &bytes)
        };
        let report = run(&plan, &hooks);
        assert_eq!(report.copied.len(), 1);
        assert_eq!(report.canceled, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The share-hang path, with a slow fake: a job that beats, then
    /// says nothing for longer than the wait, is set aside, and its end
    /// lands later, late; one that keeps beating is never set aside.
    #[test]
    fn a_job_that_goes_quiet_is_set_aside_and_lands_late() {
        let (tx, rx) = mpsc::channel();
        let held = Arc::new(AtomicBool::new(true));
        let hold = held.clone();
        supervise(
            "test hang",
            Duration::from_millis(100),
            move |beat| {
                beat();
                // The share stops answering until the test lets go.
                while hold.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                beat();
                7
            },
            move |h| {
                let _ = tx.send(match h {
                    Heard::Aside | Heard::Lost => None,
                    Heard::Done { result, late } => Some((result, late)),
                });
            },
        )
        .unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), None);
        held.store(false, Ordering::SeqCst);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            Some((7, true))
        );

        let (tx, rx) = mpsc::channel();
        supervise(
            "test slow",
            // Well over the beats' gap: a loaded runner (macOS CI) can
            // sleep 20 ms for a good deal longer, and the clock's wait
            // under load is no test's business.
            Duration::from_secs(2),
            |beat| {
                // Slow, but saying so: twenty beats 20 ms apart.
                for _ in 0..20 {
                    std::thread::sleep(Duration::from_millis(20));
                    beat();
                }
                1
            },
            move |h| {
                let _ = tx.send(match h {
                    Heard::Aside | Heard::Lost => None,
                    Heard::Done { result, late } => Some((result, late)),
                });
            },
        )
        .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            Some((1, false))
        );
    }

    /// The copy itself beats as it goes: a stall in the middle of a
    /// frame, behind a supervisor with a short wait, sets the run aside,
    /// and once let go it finishes the frame and stops there, the
    /// cancel the window set when it heard.
    #[test]
    fn a_copy_that_stalls_mid_run_is_set_aside_and_stops_after_its_frame() {
        let dir = scratch("stall");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        std::fs::create_dir_all(&nas).unwrap();
        shoot(&local);
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
        let cancel = Arc::new(AtomicBool::new(false));
        let held = Arc::new(AtomicBool::new(true));
        let (tx, rx) = mpsc::channel();
        let (job_cancel, hold) = (cancel.clone(), held.clone());
        supervise(
            "test stall",
            // Longer than a sync of the first frame on a slow disk
            // (the Windows runner's), which does not beat: the stall
            // this test means is the hold on the second frame.
            Duration::from_secs(1),
            move |beat| {
                let bytes = AtomicU64::new(0);
                let hand = |f: &Path| {
                    if f.ends_with("b.tif") {
                        while hold.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                };
                let hooks = Hooks {
                    beat: &**beat,
                    cancel: &job_cancel,
                    bytes: &bytes,
                    before_frame: Some(&hand),
                    tamper: None,
                };
                run(&plan, &hooks)
            },
            move |h| {
                let _ = tx.send(h);
            },
        )
        .unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            Heard::Aside
        ));
        // What the window does on hearing it: no more frames after this.
        cancel.store(true, Ordering::SeqCst);
        held.store(false, Ordering::SeqCst);
        match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            Heard::Done { result, late } => {
                assert!(late);
                assert_eq!(result.copied.len(), 2, "a, and b once let go");
                assert_eq!(result.canceled, 1);
            }
            Heard::Aside | Heard::Lost => panic!("set aside twice, or lost"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn all_roots_hides_the_archives_copies_of_local_frames() {
        let rows = vec![
            (PathBuf::from("/local/a.cr3"), "ha".to_string()),
            (PathBuf::from("/nas/old/z.cr3"), "hz".to_string()),
            (PathBuf::from("/nas/local/a.cr3"), "ha".to_string()),
            (
                PathBuf::from("/nas/elsewhere/a-renamed.cr3"),
                "ha".to_string(),
            ),
            (PathBuf::from("/other/b.cr3"), "hb".to_string()),
        ];
        let kept: Vec<PathBuf> =
            hide_local_copies(rows.clone(), &[PathBuf::from("/nas")], |h| h.as_str())
                .into_iter()
                .map(|(p, _)| p)
                .collect();
        assert_eq!(
            kept,
            [
                PathBuf::from("/local/a.cr3"),
                PathBuf::from("/nas/old/z.cr3"),
                PathBuf::from("/other/b.cr3")
            ]
        );
        // No archive: every row.
        assert_eq!(
            hide_local_copies(rows.clone(), &[], |h| h.as_str()).len(),
            5
        );
    }

    #[test]
    fn the_counts_find_each_frame_on_each_archive() {
        let dir = scratch("counts");
        let (local, nas, cloud) = (dir.join("local"), dir.join("nas"), dir.join("cloud"));
        std::fs::create_dir_all(&nas).unwrap();
        std::fs::create_dir_all(&cloud).unwrap();
        let files = shoot(&local);
        std::fs::copy(&files[0], nas.join("a.tif")).unwrap();
        std::fs::copy(&files[0], cloud.join("a.tif")).unwrap();
        std::fs::copy(&files[1], cloud.join("b.tif")).unwrap();
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas, &cloud]);
        let lib = Library::open_read_only(&db).unwrap();
        let on = on_archives(
            &files,
            &[nas.clone(), cloud.clone()],
            Some(&lib),
            &|| {},
            &|| false,
        )
        .unwrap();
        let ons: Vec<Vec<usize>> = on.iter().map(|w| w.on.clone()).collect();
        assert_eq!(ons, vec![vec![0, 1], vec![1], vec![]]);
        assert!(on.iter().all(|w| w.unknown.is_empty()));
        // Stopped: nothing.
        assert_eq!(
            on_archives(
                &files,
                std::slice::from_ref(&nas),
                Some(&lib),
                &|| {},
                &|| true
            ),
            None
        );
        drop(lib);
        // A copy that did not keep its time: unknown to a count, which
        // reads nothing whole, and no whole hash is taken.
        let late = nas.join("c.tif");
        std::fs::copy(&files[2], &late).unwrap();
        set_time(&late, Duration::from_secs(60 * 60));
        index(&db, &[&nas]);
        let lib = Library::open_read_only(&db).unwrap();
        let on = on_archives(
            std::slice::from_ref(&files[2]),
            std::slice::from_ref(&nas),
            Some(&lib),
            &|| {},
            &|| false,
        )
        .unwrap();
        assert_eq!(
            on,
            vec![Where {
                on: Vec::new(),
                unknown: vec![0]
            }]
        );
        assert_eq!(lib.whole_hash(&late).unwrap(), None);
        assert_eq!(lib.whole_hash(&files[2]).unwrap(), None);
        drop(lib);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_sidecar_is_named_after_the_frame_it_goes_with() {
        let frame = Path::new("/here/IMG_1.CR3");
        let to = Path::new("/there/x/renamed.CR3");
        assert_eq!(
            sidecar_at(Path::new("/here/IMG_1.CR3.gcd"), frame, to),
            PathBuf::from("/there/x/renamed.CR3.gcd")
        );
        assert_eq!(
            sidecar_at(Path::new("/here/.greycard/IMG_1.CR3.gcd"), frame, to),
            PathBuf::from("/there/x/.greycard/renamed.CR3.gcd")
        );
        assert_eq!(
            sidecar_at(Path::new("/here/IMG_1.xmp"), frame, to),
            PathBuf::from("/there/x/renamed.xmp")
        );
        assert_eq!(
            sidecar_at(Path::new("/here/IMG_1.CR3.xmp"), frame, to),
            PathBuf::from("/there/x/renamed.CR3.xmp")
        );
    }

    fn set_time(p: &Path, ago: Duration) {
        std::fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(SystemTime::now() - ago)
            .unwrap();
    }

    /// Each sidecar file is judged on its own: a `.gcd` newer here by
    /// its counter goes across, while the archive's XMP, edited later on
    /// another machine, stays as it is and names the frame.
    #[test]
    fn a_newer_xmp_over_there_is_kept_while_the_newer_gcd_goes() {
        let dir = scratch("per-file");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        std::fs::create_dir_all(&nas).unwrap();
        let files = shoot(&local);
        let b = &files[1];
        let mut meta = Sidecar::load(b).unwrap().unwrap().meta;
        greycard_edit::xmp::save(b, &meta, None).unwrap();
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let ask = backup_ask(&local, &nas, &db, false);
        let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
        run(&plan(&ask, &no_beat()), &quiet_hooks(&cancel, &bytes));
        let there = nas.join("local").join("shoot").join("b.tif");
        let xmps_there = greycard_edit::xmp::paths_of(&there);
        assert_eq!(xmps_there.len(), 1);
        // Here: two more saves of the edit, and the XMP an hour old.
        rate(b, 5, 2, Placement::Beside);
        for x in greycard_edit::xmp::paths_of(b) {
            set_time(&x, Duration::from_secs(60 * 60));
        }
        // There: the XMP written again, now, by another tool.
        meta.label = greycard_edit::meta::Label::Green;
        greycard_edit::xmp::save(&there, &meta, None).unwrap();
        let theirs_before = std::fs::read(&xmps_there[0]).unwrap();
        index(&db, &[&local, &nas]);
        let plan = plan(&ask, &no_beat());
        let item = plan.items.iter().find(|i| &i.frame == b).unwrap();
        match &item.what {
            What::Sidecars { sidecars, held, .. } => {
                assert!(*held);
                assert_eq!(sidecars.len(), 1, "{sidecars:?}");
                assert_eq!(sidecars[0].from.extension().unwrap(), "gcd");
            }
            other => panic!("{other:?}"),
        }
        assert!(plan.theirs_newer().contains(&b.as_path()));
        let report = run(&plan, &quiet_hooks(&cancel, &bytes));
        assert!(report.theirs_newer.contains(b));
        assert_eq!(Sidecar::load(&there).unwrap().unwrap().meta.rating, 5);
        assert_eq!(std::fs::read(&xmps_there[0]).unwrap(), theirs_before);

        // The other way, the same rule: the archive's newer XMP is the
        // one that would go home; the `.gcd`s are the same bytes now.
        let (back, held) = compare_sidecars(&there, b);
        assert!(!held);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].to, greycard_edit::xmp::paths_of(b)[0]);
        assert!(back[0].replace);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A sidecar left at the destination with no frame beside it keeps
    /// the frame out: it would land beside another's edit.
    #[test]
    fn a_leftover_sidecar_at_the_destination_keeps_the_frame_out() {
        let dir = scratch("leftover");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let files = shoot(&local);
        let there = nas.join("local").join("shoot");
        std::fs::create_dir_all(&there).unwrap();
        std::fs::write(there.join("b.tif.gcd"), br#"{"saved":9}"#).unwrap();
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
        let item = plan.items.iter().find(|i| i.frame == files[1]).unwrap();
        assert_eq!(
            item.what,
            What::Taken {
                to: there.join("b.tif.gcd")
            }
        );
        let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
        let report = run(&plan, &quiet_hooks(&cancel, &bytes));
        assert!(report.taken.contains(&files[1]));
        assert!(!there.join("b.tif").exists());
        assert_eq!(
            std::fs::read(there.join("b.tif.gcd")).unwrap(),
            br#"{"saved":9}"#
        );
        // The others went whole.
        assert!(there.join("a.tif").is_file() && there.join("c.tif").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The copy's rows wait on a library another writer holds without
    /// going quiet: the wait beats, and the copy is not set aside.
    #[test]
    fn writing_the_rows_beats_while_the_library_is_locked() {
        let dir = scratch("locked");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        std::fs::create_dir_all(&nas).unwrap();
        shoot(&local);
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
        let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
        let report = run(&plan, &quiet_hooks(&cancel, &bytes));
        assert_eq!(report.copied.len(), 3);
        // Another writer holds the lock for well past the wait.
        let holder = rusqlite::Connection::open(&db).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE;").unwrap();
        let (tx, rx) = mpsc::channel();
        let job_db = db.clone();
        supervise(
            "test locked",
            Duration::from_millis(150),
            move |beat| record(&job_db, &report, beat),
            move |h| {
                let _ = tx.send(match h {
                    Heard::Aside | Heard::Lost => "aside",
                    Heard::Done { late: false, .. } => "done",
                    Heard::Done { late: true, .. } => "late",
                });
            },
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(800));
        holder.execute_batch("COMMIT;").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), "done");
        drop(holder);
        let lib = Library::open_read_only(&db).unwrap();
        assert!(
            lib.whole_hash(&nas.join("local").join("shoot").join("a.tif"))
                .unwrap()
                .is_some()
        );
        drop(lib);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// §197's open case, settled the hash-first way: a file at the
    /// destination with the frame's head and size and another time is
    /// read whole on both sides. Other bytes: not the frame, and its name
    /// is taken, so the frame is left and named. The same bytes: the
    /// same frame. Either way both whole hashes are kept, so the next
    /// look reads nothing whole again.
    #[test]
    fn a_file_changed_under_the_same_head_and_size_is_settled_by_its_whole_hash() {
        let dir = scratch("same-head");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let shoot_dir = local.join("shoot");
        std::fs::create_dir_all(&shoot_dir).unwrap();
        let there = nas.join("local").join("shoot");
        std::fs::create_dir_all(&there).unwrap();
        // Past the 64 KB head, so a byte after it changes no key.
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let mine = shoot_dir.join("export.tif");
        std::fs::write(&mine, &body).unwrap();
        let mut other = body.clone();
        other[150_000] ^= 0xff;
        let copy = there.join("export.tif");
        std::fs::write(&copy, &other).unwrap();
        set_time(&copy, Duration::from_secs(60 * 60));
        // The same bytes, another time: a copy that did not keep it.
        let same = shoot_dir.join("same.tif");
        std::fs::write(&same, &body[..190_000]).unwrap();
        let same_there = there.join("same.tif");
        std::fs::write(&same_there, &body[..190_000]).unwrap();
        set_time(&same_there, Duration::from_secs(60 * 60));
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let ask = Ask {
            frames: Frames::Chosen(vec![mine.clone(), same.clone()]),
            ..backup_ask(&local, &nas, &db, false)
        };
        let first = plan(&ask, &no_beat());
        assert_eq!(first.items[0].what, What::Taken { to: copy.clone() });
        assert_eq!(first.taken(), [mine.as_path()]);
        assert!(
            matches!(first.items[1].what, What::Same { .. }),
            "{:?}",
            first.items[1].what
        );
        assert_eq!(first.same(), (1, 0), "counted once");
        // The hashes kept on the rows.
        let lib = Library::open_read_only(&db).unwrap();
        for (p, b) in [
            (&mine, &body[..]),
            (&copy, &other[..]),
            (&same, &body[..190_000]),
        ] {
            let whole = blake3::hash(b).to_hex().to_string();
            assert_eq!(lib.whole_hash(p).unwrap(), Some(whole), "{}", p.display());
        }
        drop(lib);
        let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
        run(&first, &quiet_hooks(&cancel, &bytes));
        assert_eq!(std::fs::read(&copy).unwrap(), other, "left alone");
        // Asked again: decided from the rows, the same way.
        let second = plan(&ask, &no_beat());
        assert_eq!(second.items, first.items);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A frame with no sidecar of its own is still kept out by a sidecar
    /// left at its destination name, of either placement or an XMP, and
    /// with sidecars off too.
    #[test]
    fn a_frame_with_no_sidecar_is_kept_out_by_one_left_at_the_destination() {
        let dir = scratch("stray");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let files = shoot(&local);
        let a = &files[0];
        assert!(travelling(a).is_empty(), "a has no sidecar here");
        let there = nas.join("local").join("shoot");
        std::fs::create_dir_all(there.join(SIDECAR_FOLDER)).unwrap();
        let db = dir.join("library.sqlite");
        index(&db, &[&local]);
        for (stray, sidecars) in [
            (there.join("a.tif.gcd"), true),
            (there.join(SIDECAR_FOLDER).join("a.tif.gcd"), true),
            (there.join("a.xmp"), true),
            (there.join("a.tif.gcd"), false),
        ] {
            std::fs::write(&stray, b"another frame's").unwrap();
            let ask = Ask {
                sidecars,
                ..backup_ask(&local, &nas, &db, false)
            };
            let plan = plan(&ask, &no_beat());
            let item = plan.items.iter().find(|i| &i.frame == a).unwrap();
            assert_eq!(
                item.what,
                What::Taken { to: stray.clone() },
                "{}",
                stray.display()
            );
            std::fs::remove_file(&stray).unwrap();
        }
        // One appearing between the plan and the copy keeps it out too.
        let plan = plan(&backup_ask(&local, &nas, &db, false), &no_beat());
        std::fs::write(there.join("a.tif.gcd"), b"late").unwrap();
        let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
        let report = run(&plan, &quiet_hooks(&cancel, &bytes));
        assert!(report.taken.contains(a));
        assert!(!there.join("a.tif").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A job whose thread ends without a result is said, so nothing
    /// waits on it.
    #[test]
    fn a_job_that_panics_is_heard_as_lost() {
        let (tx, rx) = mpsc::channel();
        supervise(
            "test lost",
            Duration::from_secs(10),
            |beat: &Beat| -> u32 {
                beat();
                std::panic::resume_unwind(Box::new("the test's own"))
            },
            move |h| {
                let _ = tx.send(matches!(h, Heard::Lost));
            },
        )
        .unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(10)).unwrap());
    }

    /// A walk told to stop ends at once.
    #[test]
    fn a_walk_told_to_stop_ends() {
        let dir = scratch("walk-stop");
        write_frame(&dir.join("a.tif"), &R5, 1);
        let (frames, _) = walk(&dir, false, &[], &|| {}, &|| true);
        assert!(frames.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A folder of no root's with the archive inside it: the walk never
    /// goes into the archive.
    #[test]
    fn a_walk_leaves_the_archive_out() {
        let dir = scratch("walk-skip");
        let top = dir.join("top");
        let nas = top.join("nas");
        std::fs::create_dir_all(&nas).unwrap();
        write_frame(&top.join("a.tif"), &R5, 1);
        write_frame(&nas.join("z.tif"), &R6, 2);
        let (frames, _) = walk(&top, false, std::slice::from_ref(&nas), &|| {}, &|| false);
        assert_eq!(frames, [top.join("a.tif")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Under `--no-sidecars` the frames go without them, and nothing of
    /// a sidecar is compared.
    #[test]
    fn with_sidecars_off_the_frames_go_alone() {
        let dir = scratch("no-sidecars");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        std::fs::create_dir_all(&nas).unwrap();
        shoot(&local);
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let ask = Ask {
            sidecars: false,
            ..backup_ask(&local, &nas, &db, false)
        };
        let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
        let report = run(&plan(&ask, &no_beat()), &quiet_hooks(&cancel, &bytes));
        assert_eq!((report.copied.len(), report.sidecar_files), (3, 0));
        let there = nas.join("local").join("shoot");
        assert!(!there.join("b.tif.gcd").exists());
        assert!(!there.join(SIDECAR_FOLDER).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A whole hash on a row is the file's only while the row's size and
    /// time are: a frame written again in place under the same start and
    /// size after its backup, before any pass, is read again rather than
    /// taken for its old self, and backed up when it differs.
    #[test]
    fn a_whole_hash_on_a_row_is_trusted_only_while_the_file_is_unchanged() {
        let dir = scratch("stale-whole");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let shoot_dir = local.join("shoot");
        std::fs::create_dir_all(&shoot_dir).unwrap();
        std::fs::create_dir_all(&nas).unwrap();
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 249) as u8).collect();
        let mine = shoot_dir.join("export.tif");
        std::fs::write(&mine, &body).unwrap();
        set_time(&mine, Duration::from_secs(2 * 60 * 60));
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let ask = Ask {
            frames: Frames::Chosen(vec![mine.clone()]),
            ..backup_ask(&local, &nas, &db, false)
        };
        let (cancel, bytes) = (AtomicBool::new(false), AtomicU64::new(0));
        let first = plan(&ask, &no_beat());
        let report = run(&first, &quiet_hooks(&cancel, &bytes));
        record(&db, &report, &no_beat());
        index(&db, &[&nas]);
        // Written again in place: the same start and size, other bytes,
        // a new time; no pass since, so the row still has the old hash.
        let mut again = body.clone();
        again[150_000] ^= 0xff;
        std::fs::write(&mine, &again).unwrap();
        let second = plan(&ask, &no_beat());
        let copy = nas.join("local").join("shoot").join("export.tif");
        assert_eq!(
            second.items[0].what,
            What::Taken { to: copy.clone() },
            "not the frame backed up before"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
