//! Remove rejects from an archive (notes §197, §216): the frames in a
//! shoot's `rejects` folder here, found on the archive by content hash
//! and confirmed on its disk, then either moved into a `rejects` folder
//! beside each copy there (as Move rejects moves a frame here, its
//! sidecars with it) or deleted from it with their sidecars through
//! `crate::delete`.
//!
//! A reject is carried as an [`Entry`]: its content key, its name, its
//! size, and what else the index knew of it (the whole file's hash, its
//! time). The same entry is what the queue keeps for a move confirmed
//! while the archive did not answer, so a run from the sheet and a run
//! of the queue are one code path with one check on the disk: a copy is
//! a file under the archive the index gives that key, there at that
//! path with that size and that head, and the same file by the whole
//! hashes where both are known, else by the time (a backup keeps it). A
//! pair that only a whole read would settle is not known, and never
//! acted on.
//!
//! The queue (`archive-queue.json` beside `roots.json`) is read loosely:
//! a malformed entry is dropped and said, the rest kept. Nothing in it
//! ever deletes.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

use greycard_library::Library;

use super::{Beat, Hooks, key_of, open_index, whole_from_row};
use crate::cull::{REJECTS, rejects_dir};
use crate::delete::{self, Deleted, How};

/// The queue's file name, beside `roots.json`.
pub(crate) const QUEUE_FILE: &str = "archive-queue.json";

/// The queue file's version.
const QUEUE_VERSION: u64 = 1;

/// Two times this close are one: a filesystem that keeps two seconds
/// (FAT, some shares) rounds a kept time, as the backup's check allows.
const SAME_TIME: Duration = Duration::from_secs(2);

/// One reject, as a run and the queue carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// Its content key (§72's head and size).
    pub(crate) hash: String,
    /// Its file name, for saying it.
    pub(crate) name: String,
    /// The size it had.
    pub(crate) size: u64,
    /// The whole file's BLAKE3, when the index kept one of the file as
    /// it was.
    pub(crate) whole: Option<String>,
    /// Its mtime, nanoseconds since the epoch.
    pub(crate) mtime: Option<i64>,
}

/// One reject a run is over: its entry, and the copies the sheet
/// listed for it. A run from the sheet acts on those paths alone, each
/// confirmed again at the moment it goes (§216: same list, same
/// confirmation); a copy the index has picked up since the sheet was
/// never shown and is left. The queue's runs have no list: the paths
/// may have changed while the archive was away, so they look the reject
/// up again by its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) entry: Entry,
    pub(crate) listed: Option<Vec<PathBuf>>,
}

impl Item {
    /// A queued reject, looked up by its hash when it runs.
    pub(crate) fn queued(entry: Entry) -> Item {
        Item {
            entry,
            listed: None,
        }
    }
}

/// The copies of `item` there now: the listed ones still its copies,
/// confirmed on the disk, with those no longer so named; or, with no
/// list, found again by hash.
fn copies_now(
    item: &Item,
    archive: &Path,
    lib: Option<&Library>,
    beat: &dyn Fn(),
) -> (Found, Vec<(PathBuf, String)>) {
    let Some(listed) = &item.listed else {
        return (
            find(&item.entry, archive, lib, true, None, beat),
            Vec::new(),
        );
    };
    let mut found = Found::default();
    let mut stale = Vec::new();
    for p in listed {
        beat();
        match seen_on_disk(&item.entry, p, lib) {
            Seen::Copy => found.copies.push(p.clone()),
            Seen::Unknown => found.unknown.push(p.clone()),
            Seen::Other => stale.push((
                p.clone(),
                format!(
                    "{} is no longer the copy the sheet listed",
                    p.file_name().unwrap_or_default().to_string_lossy()
                ),
            )),
        }
    }
    (found, stale)
}

fn nanos(t: SystemTime) -> Option<i64> {
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_nanos() as i64)
}

fn near(a: i64, b: i64) -> bool {
    a.abs_diff(b) <= SAME_TIME.as_nanos() as u64
}

/// The entry for a frame here: its key from its row when the row is of
/// the file as it is, else read from its head.
pub(crate) fn entry_of(frame: &Path, lib: Option<&Library>) -> Result<Entry, String> {
    let meta = std::fs::metadata(frame).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    let hash = key_of(frame, &meta, lib).ok_or_else(|| "its head could not be read".to_string())?;
    Ok(Entry {
        hash,
        name: frame
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size: meta.len(),
        whole: whole_from_row(frame, lib),
        mtime: meta.modified().ok().and_then(nanos),
    })
}

/// Whether a file is a reject's copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Seen {
    Copy,
    /// Its key and size, and nothing short of reading both whole would
    /// tell whether it is the same file.
    Unknown,
    Other,
}

/// Whether the file at `there` is `entry`'s copy, on the disk: there,
/// a file of that size, of that head (its row's when the row is of the
/// file as it is now, else read), and the same by the whole hashes when
/// both are known, else by the time.
pub(crate) fn seen_on_disk(entry: &Entry, there: &Path, lib: Option<&Library>) -> Seen {
    let Ok(meta) = std::fs::metadata(there) else {
        return Seen::Other;
    };
    if !meta.is_file() || meta.len() != entry.size {
        return Seen::Other;
    }
    if key_of(there, &meta, lib).as_deref() != Some(entry.hash.as_str()) {
        return Seen::Other;
    }
    if let (Some(mine), Some(theirs)) = (&entry.whole, whole_from_row(there, lib)) {
        return if *mine == theirs {
            Seen::Copy
        } else {
            Seen::Other
        };
    }
    match (entry.mtime, meta.modified().ok().and_then(nanos)) {
        (Some(a), Some(b)) if near(a, b) => Seen::Copy,
        _ => Seen::Unknown,
    }
}

/// The same question asked of the index alone, for an archive that does
/// not answer: the row's size, the whole hashes it keeps, its time.
fn seen_in_index(entry: &Entry, row: &greycard_library::Entry, lib: &Library) -> Seen {
    if row.size != entry.size {
        return Seen::Other;
    }
    if let (Some(mine), Ok(Some(theirs))) = (&entry.whole, lib.whole_hash(&row.path)) {
        return if *mine == theirs {
            Seen::Copy
        } else {
            Seen::Other
        };
    }
    match entry.mtime {
        Some(a) if near(a, row.mtime) => Seen::Copy,
        _ => Seen::Unknown,
    }
}

/// Where `entry`'s copies are under `archive`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) copies: Vec<PathBuf>,
    /// Files of its key and size that could not be told from it.
    pub(crate) unknown: Vec<PathBuf>,
}

/// `entry`'s copies under `archive`, by hash in the index at any path,
/// each confirmed on the disk when `on_disk` (the archive answered),
/// else as the index has it. `not` is never its own copy.
pub(crate) fn find(
    entry: &Entry,
    archive: &Path,
    lib: Option<&Library>,
    on_disk: bool,
    not: Option<&Path>,
    beat: &dyn Fn(),
) -> Found {
    let mut found = Found::default();
    let Some(lib) = lib else {
        return found;
    };
    let Ok(rows) = lib.by_hash_under(&entry.hash, &[archive.to_path_buf()]) else {
        return found;
    };
    for row in rows {
        beat();
        if Some(row.path.as_path()) == not {
            continue;
        }
        let seen = if on_disk {
            seen_on_disk(entry, &row.path, Some(lib))
        } else {
            seen_in_index(entry, &row, lib)
        };
        match seen {
            Seen::Copy => found.copies.push(row.path),
            Seen::Unknown => found.unknown.push(row.path),
            Seen::Other => {}
        }
    }
    found
}

/// One frame of the rejects folder, and what was found of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reject {
    pub(crate) frame: PathBuf,
    /// Its entry, or why it could not be read.
    pub(crate) entry: Result<Entry, String>,
    pub(crate) found: Found,
}

/// What the sheet shows: each reject with its copies on the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Look {
    pub(crate) archive: PathBuf,
    /// Whether the archive answered its look; when not, the copies are
    /// as the index last saw them.
    pub(crate) answered: bool,
    pub(crate) dir: PathBuf,
    pub(crate) rejects: Vec<Reject>,
}

impl Look {
    /// The rejects with a copy found.
    pub(crate) fn with_copies(&self) -> impl Iterator<Item = &Reject> {
        self.rejects.iter().filter(|r| !r.found.copies.is_empty())
    }

    /// How many copies, and their bytes.
    pub(crate) fn copies(&self) -> (usize, u64) {
        self.with_copies().fold((0, 0), |(n, b), r| {
            let size = r.entry.as_ref().map(|e| e.size).unwrap_or(0);
            let k = r.found.copies.len();
            (n + k, b + size * k as u64)
        })
    }

    /// How many copies a move would move: those not in a rejects
    /// folder there already.
    pub(crate) fn to_move(&self) -> usize {
        self.with_copies()
            .flat_map(|r| r.found.copies.iter())
            .filter(|c| !in_rejects(c))
            .count()
    }

    /// What a run from the sheet is over: each reject with a copy
    /// found, and the copies listed.
    pub(crate) fn items(&self) -> Vec<Item> {
        self.with_copies()
            .filter_map(|r| {
                Some(Item {
                    entry: r.entry.as_ref().ok()?.clone(),
                    listed: Some(r.found.copies.clone()),
                })
            })
            .collect()
    }

    /// The rejects with no copy found and none in doubt.
    pub(crate) fn none_found(&self) -> Vec<&Path> {
        self.rejects
            .iter()
            .filter(|r| r.entry.is_ok() && r.found.copies.is_empty() && r.found.unknown.is_empty())
            .map(|r| r.frame.as_path())
            .collect()
    }

    /// The rejects with a file on the archive that could not be told
    /// from them: listed, never acted on.
    pub(crate) fn not_known(&self) -> Vec<&Path> {
        self.rejects
            .iter()
            .filter(|r| !r.found.unknown.is_empty())
            .map(|r| r.frame.as_path())
            .collect()
    }

    pub(crate) fn unreadable(&self) -> Vec<(&Path, &str)> {
        self.rejects
            .iter()
            .filter_map(|r| match &r.entry {
                Err(why) => Some((r.frame.as_path(), why.as_str())),
                Ok(_) => None,
            })
            .collect()
    }
}

/// The frames in the rejects folder `dir` here, each looked for under
/// `archive`: on its disk when it `answered`, else in the index alone.
/// A rejects folder that is a link is refused, as §190's delete refuses
/// it.
pub(crate) fn look(
    dir: &Path,
    archive: &Path,
    answered: bool,
    index: Option<&Path>,
    beat: &Beat,
) -> Result<Look, String> {
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_symlink() => {
            return Err(format!(
                "{} is a link; nothing is looked for through one",
                dir.display()
            ));
        }
        Ok(m) if m.is_dir() => {}
        _ => return Err(format!("there is no rejects folder at {}", dir.display())),
    }
    beat();
    let frames = crate::files::list_files(dir)
        .map_err(|e| format!("the rejects folder cannot be read: {e:#}"))?;
    let lib = index.and_then(|p| open_index(p, false, beat));
    let mut rejects = Vec::with_capacity(frames.len());
    for frame in frames {
        beat();
        let entry = entry_of(&frame, lib.as_ref());
        let mut found = match &entry {
            Ok(e) => find(e, archive, lib.as_ref(), answered, Some(&frame), &**beat),
            Err(_) => Found::default(),
        };
        // The folder looked at is never where its own copies are,
        // whichever way its path is spelled.
        let here = |c: &PathBuf| c.parent().is_some_and(|p| same_folder(p, dir));
        found.copies.retain(|c| !here(c));
        found.unknown.retain(|c| !here(c));
        rejects.push(Reject {
            frame,
            entry,
            found,
        });
    }
    Ok(Look {
        archive: archive.to_path_buf(),
        answered,
        dir: dir.to_path_buf(),
        rejects,
    })
}

/// Why the `rejects` folder a copy in `folder` would go to will not do:
/// a link (anywhere it led, the copy would leave with it), or a folder
/// that is not inside the archive as the disk has it. Its hidden
/// sidecar folder the same.
fn destination_refused(archive: &Path, folder: &Path) -> Option<String> {
    let dest = rejects_dir(folder);
    let root = dunce::canonicalize(archive).ok()?;
    for d in [dest.clone(), dest.join(greycard_edit::SIDECAR_FOLDER)] {
        match std::fs::symlink_metadata(&d) {
            Ok(m) if m.file_type().is_symlink() => {
                return Some(format!(
                    "{} is a link; nothing is moved through one",
                    d.display()
                ));
            }
            Ok(_) => {
                if !dunce::canonicalize(&d).is_ok_and(|c| c.starts_with(&root)) {
                    return Some(format!("{} is not inside the archive", d.display()));
                }
            }
            Err(_) => {}
        }
    }
    None
}

/// Whether `copy` is in a rejects folder already.
fn in_rejects(copy: &Path) -> bool {
    copy.parent().and_then(Path::file_name) == Some(std::ffi::OsStr::new(REJECTS))
}

/// Whether two folders are one, by their spelling or as the disk has
/// them.
fn same_folder(a: &Path, b: &Path) -> bool {
    a == b
        || match (dunce::canonicalize(a), dunce::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

fn note(folders: &mut Vec<PathBuf>, d: &Path) {
    if !folders.iter().any(|f| f == d) {
        folders.push(d.to_path_buf());
    }
}

/// What a run of the moves did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Moves {
    /// Each copy moved: the reject's name, where it was, where it went.
    pub(crate) moved: Vec<(String, PathBuf, PathBuf)>,
    /// How many `.gcd`s went with them.
    pub(crate) sidecars: usize,
    /// Copies in a rejects folder there already, left where they are.
    pub(crate) already: usize,
    /// The entries done: every copy found moved or in a rejects folder.
    pub(crate) done: Vec<Entry>,
    /// The entries that could not be done, and why: kept in the queue.
    pub(crate) left: Vec<(Entry, String)>,
    /// The rejects with a copy there that could not be told from them,
    /// left alone, beside copies of theirs that were moved.
    pub(crate) not_known: Vec<String>,
    /// The entries not reached because the run was canceled.
    pub(crate) canceled: Vec<Entry>,
    /// The folders written to, for the indexer.
    pub(crate) folders: Vec<PathBuf>,
}

/// Move each entry's copies on `archive` into a `rejects` folder beside
/// each, its sidecars with it and their placement kept, by Move rejects'
/// own move (`cull::move_rejects`): a name taken there leaves that copy
/// whole where it is, said. Each copy is found and confirmed on the
/// disk at the moment it goes. One entry at a time, the cancel looked
/// at between them, `hooks.bytes` counting the entries. Nothing is
/// deleted.
pub(crate) fn run_moves(
    archive: &Path,
    items: &[Item],
    index: Option<&Path>,
    hooks: &Hooks<'_>,
    beat: &Beat,
) -> Moves {
    let lib = index.and_then(|p| open_index(p, false, beat));
    let mut out = Moves::default();
    for (n, item) in items.iter().enumerate() {
        let entry = &item.entry;
        (hooks.beat)();
        if hooks.cancel.load(Ordering::SeqCst) {
            out.canceled = items[n..].iter().map(|i| i.entry.clone()).collect();
            break;
        }
        #[cfg(test)]
        if let Some(hand) = hooks.before_frame {
            hand(Path::new(&entry.name));
        }
        let (found, stale) = copies_now(item, archive, lib.as_ref(), hooks.beat);
        let mut why_left = stale.first().map(|(_, why)| why.clone());
        if found.copies.is_empty() {
            let why = if let Some(why) = why_left {
                why
            } else if found.unknown.is_empty() {
                "no copy of it was found there".to_string()
            } else {
                "its copy there could not be told from it without reading both whole".to_string()
            };
            out.left.push((entry.clone(), why));
            hooks.bytes.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let inside = allowed_under(archive, &found.copies);
        for copy in &found.copies {
            let home = copy
                .parent()
                .and_then(|d| dunce::canonicalize(d).ok())
                .is_some_and(|d| inside.iter().any(|a| dunce::simplified(a) == d));
            if !home {
                why_left = Some(format!(
                    "{} is not inside the archive as the disk has it",
                    copy.display()
                ));
                continue;
            }
            (hooks.beat)();
            let folder = copy.parent().unwrap_or(Path::new("."));
            if in_rejects(copy) {
                out.already += 1;
                continue;
            }
            if let Some(why) = destination_refused(archive, folder) {
                why_left = Some(why);
                continue;
            }
            // An orphaned sidecar there keeps the copy whole where it
            // is: on the archive nothing is written over (§197).
            match crate::cull::move_rejects_as(
                std::slice::from_ref(copy),
                &[0],
                crate::cull::Orphans::Keep,
            ) {
                Ok(m) if m.files == [0] => {
                    let to = rejects_dir(folder).join(copy.file_name().unwrap_or_default());
                    out.moved.push((entry.name.clone(), copy.clone(), to));
                    out.sidecars += m.sidecars;
                    note(&mut out.folders, folder);
                    note(&mut out.folders, &rejects_dir(folder));
                }
                Ok(m) => {
                    let why = m
                        .skipped
                        .first()
                        .map(|(_, w)| w.clone())
                        .unwrap_or_else(|| "not moved".into());
                    why_left = Some(why);
                }
                Err(e) => why_left = Some(format!("{e:#}")),
            }
        }
        if !found.unknown.is_empty() {
            out.not_known.push(entry.name.clone());
        }
        match why_left {
            Some(why) => out.left.push((entry.clone(), why)),
            None => out.done.push(entry.clone()),
        }
        hooks.bytes.fetch_add(1, Ordering::Relaxed);
    }
    out
}

/// What a run of the deletes did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Deletes {
    /// What went, as `crate::delete` says it, over every copy.
    pub(crate) deleted: Deleted,
    /// The copies refused before anything went (a link, a folder
    /// outside the archive), and why.
    pub(crate) refused: Vec<(PathBuf, String)>,
    /// The rejects with no copy found there at the moment of the delete.
    pub(crate) not_found: Vec<String>,
    /// The rejects with a copy there that could not be told from them,
    /// left alone.
    pub(crate) not_known: Vec<String>,
    /// The entries not reached because the run was canceled, or the
    /// trash refused one before them.
    pub(crate) canceled: usize,
    /// The folders deleted from, for the indexer.
    pub(crate) folders: Vec<PathBuf>,
}

/// The folders a delete on `archive` may reach for `copies`: their own,
/// canonical, and only those inside the archive as the disk has it, so
/// a folder that is a link to somewhere else is not one of them.
fn allowed_under(archive: &Path, copies: &[PathBuf]) -> Vec<PathBuf> {
    let Ok(root) = std::fs::canonicalize(archive) else {
        return Vec::new();
    };
    let dirs: Vec<PathBuf> = copies
        .iter()
        .filter_map(|c| c.parent().map(Path::to_path_buf))
        .collect();
    delete::allowed(&dirs)
        .into_iter()
        .filter(|d| d.starts_with(&root))
        .collect()
}

/// Delete each entry's copies on `archive` with their sidecars, `how`,
/// through `delete::plan` and `delete::delete` with the copies' own
/// folders under the archive as the allowed folders: a copy that is a
/// link, or in a folder that leads out of the archive, is refused and
/// stays whole. Each copy is found and confirmed on the disk at the
/// moment it goes. The cancel is looked at between entries; the first
/// refusal of the trash stops the run, as §190's does, and never turns
/// into a permanent delete.
pub(crate) fn run_deletes(
    archive: &Path,
    items: &[Item],
    index: Option<&Path>,
    how: How,
    trash: impl Fn(&[PathBuf]) -> Result<(), String>,
    hooks: &Hooks<'_>,
    beat: &Beat,
) -> Deletes {
    let lib = index.and_then(|p| open_index(p, false, beat));
    let mut out = Deletes::default();
    for (n, item) in items.iter().enumerate() {
        let entry = &item.entry;
        (hooks.beat)();
        if hooks.cancel.load(Ordering::SeqCst) {
            out.canceled = items.len() - n;
            break;
        }
        #[cfg(test)]
        if let Some(hand) = hooks.before_frame {
            hand(Path::new(&entry.name));
        }
        let (found, stale) = copies_now(item, archive, lib.as_ref(), hooks.beat);
        out.refused.extend(stale);
        if !found.unknown.is_empty() {
            out.not_known.push(entry.name.clone());
        }
        if found.copies.is_empty() {
            if found.unknown.is_empty() {
                out.not_found.push(entry.name.clone());
            }
            hooks.bytes.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let allowed = allowed_under(archive, &found.copies);
        let plan = delete::plan(&found.copies, &allowed);
        out.refused.extend(plan.refused.iter().cloned());
        let done = delete::delete(&plan, how, &allowed, &trash);
        for f in &done.frames {
            if let Some(d) = f.parent() {
                note(&mut out.folders, d);
            }
        }
        let d = &mut out.deleted;
        d.frames.extend(done.frames);
        d.vanished += done.vanished;
        d.sidecars += done.sidecars;
        d.failed.extend(done.failed);
        hooks.bytes.fetch_add(1, Ordering::Relaxed);
        if done.trash_refused.is_some() {
            d.trash_refused = done.trash_refused;
            d.untried = done.untried;
            out.canceled = items.len() - n - 1;
            break;
        }
    }
    out
}

/// One archive's queued moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Queued {
    pub(crate) archive: PathBuf,
    pub(crate) frames: Vec<Entry>,
}

/// The queue as read: what it holds, and what was dropped from it as
/// malformed, said.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Read {
    pub(crate) queue: Vec<Queued>,
    pub(crate) dropped: Vec<String>,
}

/// Where the queue is kept for the roots file at `roots`.
pub(crate) fn queue_path(roots: &Path) -> PathBuf {
    roots.with_file_name(QUEUE_FILE)
}

fn entry_from(v: &serde_json::Value) -> Option<Entry> {
    let hash = v.get("hash")?.as_str()?.to_owned();
    let name = v.get("name")?.as_str()?.to_owned();
    let size = v.get("size")?.as_u64()?;
    if hash.is_empty() {
        return None;
    }
    let whole = v.get("whole").and_then(|w| w.as_str()).map(str::to_owned);
    let mtime = v.get("mtime").and_then(|m| m.as_i64());
    Some(Entry {
        hash,
        name,
        size,
        whole,
        mtime,
    })
}

fn entry_value(e: &Entry) -> serde_json::Value {
    let mut v = serde_json::json!({
        "hash": e.hash,
        "name": e.name,
        "size": e.size,
    });
    if let Some(w) = &e.whole {
        v["whole"] = serde_json::Value::String(w.clone());
    }
    if let Some(m) = e.mtime {
        v["mtime"] = serde_json::Value::from(m);
    }
    v
}

/// The queue in `bytes`, read loosely: an archive's entry that has no
/// path, or a frame's that lacks its hash, its name or its size, is
/// dropped and said, and the rest kept. None for a file that is not a
/// queue at all.
pub(crate) fn parse_queue(bytes: &[u8]) -> Option<Read> {
    let value = serde_json::from_slice::<serde_json::Value>(bytes).ok()?;
    let list = value.get("queue")?.as_array()?;
    let mut read = Read::default();
    for item in list {
        let Some(archive) = item.get("archive").and_then(|a| a.as_str()) else {
            read.dropped
                .push("an entry with no archive's path".to_string());
            continue;
        };
        let archive = PathBuf::from(archive);
        let mut frames: Vec<Entry> = Vec::new();
        for f in item
            .get("frames")
            .and_then(|f| f.as_array())
            .into_iter()
            .flatten()
        {
            match entry_from(f) {
                Some(e) if !frames.iter().any(|k| k.hash == e.hash) => frames.push(e),
                Some(_) => {}
                None => read.dropped.push(format!(
                    "a frame for {} with no hash, name or size",
                    archive.display()
                )),
            }
        }
        match read.queue.iter_mut().find(|q| q.archive == archive) {
            Some(q) => {
                for e in frames {
                    if !q.frames.iter().any(|k| k.hash == e.hash) {
                        q.frames.push(e);
                    }
                }
            }
            None if frames.is_empty() => {}
            None => read.queue.push(Queued { archive, frames }),
        }
    }
    Some(read)
}

/// The queue at `path`: empty when there is none. A file that is not a
/// queue is set aside as `archive-queue.json.unreadable` and said, so
/// the next write does not put an empty queue over something a person
/// might want back.
pub(crate) fn read_queue(path: &Path) -> std::io::Result<Read> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Read::default()),
        Err(e) => return Err(e),
    };
    match parse_queue(&bytes) {
        Some(read) => Ok(read),
        None => {
            let aside = path.with_extension("json.unreadable");
            let _ = std::fs::rename(path, &aside);
            Ok(Read {
                queue: Vec::new(),
                dropped: vec![format!(
                    "the whole file, which is not a queue (set aside as {})",
                    aside.display()
                )],
            })
        }
    }
}

/// Write the queue to `path` through a file beside it renamed over it;
/// an empty queue removes the file.
pub(crate) fn write_queue(path: &Path, queue: &[Queued]) -> std::io::Result<()> {
    let queue: Vec<&Queued> = queue.iter().filter(|q| !q.frames.is_empty()).collect();
    if queue.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let list: Vec<serde_json::Value> = queue
        .iter()
        .map(|q| {
            serde_json::json!({
                "archive": q.archive.to_string_lossy(),
                "frames": q.frames.iter().map(entry_value).collect::<Vec<_>>(),
            })
        })
        .collect();
    let text = serde_json::to_string_pretty(&serde_json::json!({
        "version": QUEUE_VERSION,
        "queue": list,
    }))
    .map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let part = path.with_extension(format!("json.{}.part", std::process::id()));
    std::fs::write(&part, text)?;
    std::fs::rename(&part, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })
}

/// Change the queue at `path` as it is on disk now and write it back:
/// what was dropped from it as malformed comes back to be said.
pub(crate) fn edit_queue<T>(
    path: &Path,
    change: impl FnOnce(&mut Vec<Queued>) -> T,
) -> std::io::Result<(Read, T)> {
    let mut read = read_queue(path)?;
    let out = change(&mut read.queue);
    write_queue(path, &read.queue)?;
    Ok((read, out))
}

/// `entries` added to `archive`'s queue, each frame once by its hash.
pub(crate) fn enqueue(queue: &mut Vec<Queued>, archive: &Path, entries: &[Entry]) {
    let q = match queue.iter_mut().position(|q| q.archive == archive) {
        Some(i) => &mut queue[i],
        None => {
            queue.push(Queued {
                archive: archive.to_path_buf(),
                frames: Vec::new(),
            });
            queue.last_mut().expect("just pushed")
        }
    };
    for e in entries {
        if !q.frames.iter().any(|k| k.hash == e.hash) {
            q.frames.push(e.clone());
        }
    }
}

/// `done` taken out of `archive`'s queue.
pub(crate) fn dequeue(queue: &mut Vec<Queued>, archive: &Path, done: &[Entry]) {
    for q in queue.iter_mut().filter(|q| q.archive == archive) {
        q.frames.retain(|e| !done.iter().any(|d| d.hash == e.hash));
    }
    queue.retain(|q| !q.frames.is_empty());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::no_beat;
    use greycard_edit::{Placement, SIDECAR_FOLDER, Sidecar};
    use greycard_library::fixture::{A7, R5, R6, write_frame};
    use std::sync::atomic::{AtomicBool, AtomicU64};

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-ui-rejects-{what}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dunce::canonicalize(&dir).unwrap()
    }

    fn index(db: &Path, roots: &[&Path]) {
        let mut lib = Library::open(db).unwrap();
        for r in roots {
            lib.index_tree(r, &mut |_| {}).unwrap();
        }
    }

    fn rate(frame: &Path, placement: Placement) {
        let mut s = Sidecar::default();
        s.meta.rating = 1;
        s.save_in(frame, placement).unwrap();
    }

    /// A copy of `from` at `to`, its time kept as a backup keeps it.
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(from, to).unwrap();
        let t = std::fs::metadata(from).unwrap().modified().unwrap();
        std::fs::File::options()
            .write(true)
            .open(to)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    /// A shoot culled here, `local/shoot/rejects` holding a, b, c, d;
    /// on the archive under another folder than the mirror: a's copy
    /// with its `.gcd` beside, b's with its `.gcd` under the hidden
    /// folder and an XMP, c's copy written again since (another time),
    /// d nowhere.
    struct Culled {
        dir: PathBuf,
        nas: PathBuf,
        db: PathBuf,
        rejects: PathBuf,
        there: PathBuf,
        frames: Vec<PathBuf>,
    }

    fn culled(what: &str) -> Culled {
        let dir = scratch(what);
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let rejects = rejects_dir(&local.join("shoot"));
        std::fs::create_dir_all(&rejects).unwrap();
        let frames: Vec<PathBuf> = ["a.tif", "b.tif", "c.tif", "d.tif"]
            .iter()
            .map(|n| rejects.join(n))
            .collect();
        for (i, (f, cam)) in frames.iter().zip([&R5, &R6, &A7, &R5]).enumerate() {
            write_frame(f, cam, 300 + i as u16);
        }
        let there = nas.join("clients").join("by hand");
        copy(&frames[0], &there.join("a.tif"));
        rate(&there.join("a.tif"), Placement::Beside);
        copy(&frames[1], &there.join("b.tif"));
        rate(&there.join("b.tif"), Placement::Folder);
        std::fs::write(there.join("b.xmp"), b"<x/>").unwrap();
        copy(&frames[2], &there.join("c.tif"));
        let later = SystemTime::now() + Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(there.join("c.tif"))
            .unwrap()
            .set_modified(later)
            .unwrap();
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        Culled {
            dir,
            nas,
            db,
            rejects,
            there,
            frames,
        }
    }

    fn hooks<'a>(cancel: &'a AtomicBool, count: &'a AtomicU64) -> Hooks<'a> {
        Hooks::new(&|| {}, cancel, count)
    }

    #[test]
    fn the_rejects_copies_are_found_by_hash_at_another_path_and_confirmed() {
        let c = culled("find");
        let look = look(&c.rejects, &c.nas, true, Some(&c.db), &no_beat()).unwrap();
        let of = |n: usize| {
            look.rejects
                .iter()
                .find(|r| r.frame == c.frames[n])
                .unwrap()
        };
        assert_eq!(of(0).found.copies, [c.there.join("a.tif")]);
        assert_eq!(of(1).found.copies, [c.there.join("b.tif")]);
        // Another time and no whole hashes: not known, never acted on.
        assert!(of(2).found.copies.is_empty());
        assert_eq!(of(2).found.unknown, [c.there.join("c.tif")]);
        assert_eq!(look.not_known(), [c.frames[2].as_path()]);
        assert_eq!(look.none_found(), [c.frames[3].as_path()]);
        assert_eq!(look.copies().0, 2);
        assert_eq!(look.items().len(), 2);

        // With both whole hashes kept, c is settled by them.
        let whole = crate::import::hash_whole(&c.frames[2]).unwrap();
        let size = std::fs::metadata(&c.frames[2]).unwrap().len();
        {
            let mut lib = Library::open(&c.db).unwrap();
            lib.set_whole_hash(&c.frames[2], size, &whole).unwrap();
            lib.set_whole_hash(&c.there.join("c.tif"), size, &whole)
                .unwrap();
        }
        let look2 = super::look(&c.rejects, &c.nas, true, Some(&c.db), &no_beat()).unwrap();
        let r = look2
            .rejects
            .iter()
            .find(|r| r.frame == c.frames[2])
            .unwrap();
        assert_eq!(r.found.copies, [c.there.join("c.tif")]);
        std::fs::remove_dir_all(&c.dir).unwrap();
    }

    #[test]
    fn a_copy_whose_size_changed_on_disk_is_not_confirmed() {
        let c = culled("size");
        // a's copy grew after the index saw it.
        let mut bytes = std::fs::read(c.there.join("a.tif")).unwrap();
        bytes.extend_from_slice(b"more");
        std::fs::write(c.there.join("a.tif"), bytes).unwrap();
        let look = look(&c.rejects, &c.nas, true, Some(&c.db), &no_beat()).unwrap();
        let a = look
            .rejects
            .iter()
            .find(|r| r.frame == c.frames[0])
            .unwrap();
        assert!(a.found.copies.is_empty() && a.found.unknown.is_empty());
        // And a run refuses it the same way, leaving it in place.
        let entry = a.entry.clone().unwrap();
        let (cancel, count) = (AtomicBool::new(false), AtomicU64::new(0));
        let moves = run_moves(
            &c.nas,
            &[Item::queued(entry)],
            Some(&c.db),
            &hooks(&cancel, &count),
            &no_beat(),
        );
        assert!(moves.moved.is_empty());
        assert_eq!(moves.left.len(), 1);
        assert!(c.there.join("a.tif").is_file());
        std::fs::remove_dir_all(&c.dir).unwrap();
    }

    #[test]
    fn a_move_takes_each_copy_into_a_rejects_folder_beside_it_with_its_sidecars() {
        let c = culled("move");
        let look = look(&c.rejects, &c.nas, true, Some(&c.db), &no_beat()).unwrap();
        let (cancel, count) = (AtomicBool::new(false), AtomicU64::new(0));
        let moves = run_moves(
            &c.nas,
            &look.items(),
            Some(&c.db),
            &hooks(&cancel, &count),
            &no_beat(),
        );
        assert_eq!(moves.moved.len(), 2, "{moves:?}");
        assert_eq!(moves.done.len(), 2);
        assert!(moves.left.is_empty());
        assert_eq!(moves.sidecars, 2);
        assert_eq!(count.load(Ordering::Relaxed), 2);
        let out = rejects_dir(&c.there);
        // Placement kept: a's beside, b's under the hidden folder; the
        // XMP beside.
        assert!(out.join("a.tif").is_file() && out.join("a.tif.gcd").is_file());
        assert!(out.join("b.tif").is_file());
        assert!(out.join(SIDECAR_FOLDER).join("b.tif.gcd").is_file());
        assert!(out.join("b.xmp").is_file());
        for gone in ["a.tif", "a.tif.gcd", "b.tif", "b.xmp"] {
            assert!(!c.there.join(gone).exists(), "{gone}");
        }
        assert!(!c.there.join(SIDECAR_FOLDER).join("b.tif.gcd").exists());
        // c, not known, left where it was; nothing deleted anywhere.
        assert!(c.there.join("c.tif").is_file());
        assert!(c.frames.iter().all(|f| f.is_file()));
        assert_eq!(moves.folders, [c.there.clone(), out.clone()]);
        std::fs::remove_dir_all(&c.dir).unwrap();
    }

    #[test]
    fn a_name_taken_in_the_rejects_folder_there_leaves_the_copy_whole() {
        let c = culled("taken");
        let out = rejects_dir(&c.there);
        std::fs::create_dir_all(&out).unwrap();
        // Another file of b's sidecar's name there already.
        std::fs::write(out.join("b.xmp"), b"someone else's").unwrap();
        let look = look(&c.rejects, &c.nas, true, Some(&c.db), &no_beat()).unwrap();
        let (cancel, count) = (AtomicBool::new(false), AtomicU64::new(0));
        let moves = run_moves(
            &c.nas,
            &look.items(),
            Some(&c.db),
            &hooks(&cancel, &count),
            &no_beat(),
        );
        assert_eq!(moves.moved.len(), 1);
        assert_eq!(moves.left.len(), 1);
        assert_eq!(moves.left[0].0.name, "b.tif");
        assert!(moves.left[0].1.contains("b.xmp"), "{:?}", moves.left);
        // b whole where it was, the other file untouched.
        assert!(c.there.join("b.tif").is_file());
        assert!(c.there.join(SIDECAR_FOLDER).join("b.tif.gcd").is_file());
        assert!(c.there.join("b.xmp").is_file());
        assert_eq!(std::fs::read(out.join("b.xmp")).unwrap(), b"someone else's");
        std::fs::remove_dir_all(&c.dir).unwrap();
    }

    #[test]
    fn a_delete_goes_through_the_plan_with_the_archives_folders_alone() {
        let c = culled("delete");
        // A link on the archive with a's hash is listed as a's copy: the
        // plan refuses it, and it stays, as does what it points at.
        #[cfg(unix)]
        {
            let linked = c.nas.join("linked");
            std::fs::create_dir_all(&linked).unwrap();
            std::os::unix::fs::symlink(&c.frames[0], linked.join("a.tif")).unwrap();
            index(&c.db, &[&c.nas]);
        }
        let look = look(&c.rejects, &c.nas, true, Some(&c.db), &no_beat()).unwrap();
        let entries = look.items();
        let (cancel, count) = (AtomicBool::new(false), AtomicU64::new(0));
        let never = |_: &[PathBuf]| -> Result<(), String> { panic!("no trash in a test") };
        let done = run_deletes(
            &c.nas,
            &entries,
            Some(&c.db),
            How::Permanent,
            never,
            &hooks(&cancel, &count),
            &no_beat(),
        );
        assert_eq!(done.deleted.frames.len(), 2, "{done:?}");
        // a's .gcd beside, b's under the hidden folder and its XMP.
        assert_eq!(done.deleted.sidecars, 3);
        assert!(!c.there.join("a.tif").exists() && !c.there.join("b.tif").exists());
        assert!(!c.there.join("b.xmp").exists());
        assert!(!c.there.join(SIDECAR_FOLDER).join("b.tif.gcd").exists());
        // c, not known, untouched; the rejects here untouched.
        assert!(c.there.join("c.tif").is_file());
        assert!(c.frames.iter().all(|f| f.is_file()));
        #[cfg(unix)]
        {
            assert!(
                done.refused
                    .iter()
                    .any(|(p, why)| p.ends_with("linked/a.tif") && why.contains("link")),
                "{:?}",
                done.refused
            );
            assert!(std::fs::symlink_metadata(c.nas.join("linked").join("a.tif")).is_ok());
        }
        std::fs::remove_dir_all(&c.dir).unwrap();
    }

    /// The run acts on the copies the sheet listed and no others: one
    /// the index picked up after the look stays, and a listed one that
    /// changed since is named and left.
    #[test]
    fn a_run_acts_on_the_listed_copies_alone() {
        let c = culled("listed");
        let look = look(&c.rejects, &c.nas, true, Some(&c.db), &no_beat()).unwrap();
        let items = look.items();
        // After the sheet: a second copy of a, indexed; b's copy grown.
        let late = c.nas.join("later").join("a.tif");
        copy(&c.frames[0], &late);
        index(&c.db, &[&c.nas]);
        let mut grown = std::fs::read(c.there.join("b.tif")).unwrap();
        grown.extend_from_slice(b"more");
        std::fs::write(c.there.join("b.tif"), grown).unwrap();
        let (cancel, count) = (AtomicBool::new(false), AtomicU64::new(0));
        let never = |_: &[PathBuf]| -> Result<(), String> { panic!("no trash in a test") };
        let done = run_deletes(
            &c.nas,
            &items,
            Some(&c.db),
            How::Permanent,
            never,
            &hooks(&cancel, &count),
            &no_beat(),
        );
        assert_eq!(done.deleted.frames, [c.there.join("a.tif")], "{done:?}");
        assert!(late.is_file(), "never on the sheet, never deleted");
        assert!(c.there.join("b.tif").is_file());
        assert!(
            done.refused
                .iter()
                .any(|(p, why)| *p == c.there.join("b.tif") && why.contains("no longer")),
            "{:?}",
            done.refused
        );
        // The move the same way.
        let look = super::look(&c.rejects, &c.nas, true, Some(&c.db), &no_beat()).unwrap();
        let items = look.items();
        let later2 = c.nas.join("later2").join("a.tif");
        copy(&c.frames[0], &later2);
        index(&c.db, &[&c.nas]);
        let moves = run_moves(
            &c.nas,
            &items,
            Some(&c.db),
            &hooks(&cancel, &count),
            &no_beat(),
        );
        assert!(later2.is_file(), "{moves:?}");
        assert!(rejects_dir(&c.nas.join("later")).join("a.tif").is_file());
        std::fs::remove_dir_all(&c.dir).unwrap();
    }

    /// The rejects folder a copy would go to, a link or leading out of
    /// the archive: the copy is left where it is, said.
    #[cfg(unix)]
    #[test]
    fn a_destination_leading_out_of_the_archive_is_refused() {
        let c = culled("dest-link");
        let outside = c.dir.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, rejects_dir(&c.there)).unwrap();
        let look = look(&c.rejects, &c.nas, true, Some(&c.db), &no_beat()).unwrap();
        let (cancel, count) = (AtomicBool::new(false), AtomicU64::new(0));
        let moves = run_moves(
            &c.nas,
            &look.items(),
            Some(&c.db),
            &hooks(&cancel, &count),
            &no_beat(),
        );
        assert!(moves.moved.is_empty(), "{moves:?}");
        assert_eq!(moves.left.len(), 2);
        assert!(moves.left[0].1.contains("is a link"), "{:?}", moves.left);
        assert!(c.there.join("a.tif").is_file() && c.there.join("b.tif").is_file());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        // A real folder under the archive, but its hidden folder a link
        // out: the same.
        std::fs::remove_file(rejects_dir(&c.there)).unwrap();
        std::fs::create_dir_all(rejects_dir(&c.there)).unwrap();
        std::os::unix::fs::symlink(&outside, rejects_dir(&c.there).join(SIDECAR_FOLDER)).unwrap();
        let moves = run_moves(
            &c.nas,
            &look.items(),
            Some(&c.db),
            &hooks(&cancel, &count),
            &no_beat(),
        );
        assert!(moves.moved.is_empty(), "{moves:?}");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        std::fs::remove_dir_all(&c.dir).unwrap();
    }

    /// A folder on the archive that is a link out of it is not one the
    /// delete may reach.
    #[cfg(unix)]
    #[test]
    fn a_folder_leading_out_of_the_archive_is_not_allowed() {
        let c = culled("out");
        let outside = c.dir.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, c.nas.join("door")).unwrap();
        let allowed = allowed_under(
            &c.nas,
            &[c.nas.join("door").join("x.tif"), c.there.join("a.tif")],
        );
        assert_eq!(allowed, [std::fs::canonicalize(&c.there).unwrap()]);
        std::fs::remove_dir_all(&c.dir).unwrap();
    }

    #[test]
    fn cancel_stops_between_frames() {
        let c = culled("cancel");
        let look = look(&c.rejects, &c.nas, true, Some(&c.db), &no_beat()).unwrap();
        let (cancel, count) = (AtomicBool::new(false), AtomicU64::new(0));
        let hand = |_: &Path| cancel.store(true, Ordering::SeqCst);
        let hooks = Hooks {
            before_frame: Some(&hand),
            ..hooks(&cancel, &count)
        };
        let moves = run_moves(&c.nas, &look.items(), Some(&c.db), &hooks, &no_beat());
        assert_eq!(moves.moved.len(), 1, "the frame begun finishes");
        assert_eq!(moves.canceled.len(), 1);
        assert_eq!(moves.canceled[0].name, "b.tif");
        assert!(c.there.join("b.tif").is_file());
        std::fs::remove_dir_all(&c.dir).unwrap();
    }

    /// The share-hang path with a slow fake: a run held in the middle
    /// of its first frame, behind a supervisor with a short wait, is set
    /// aside; told then to stop, it finishes that frame once let go and
    /// lands late, the second not begun.
    #[test]
    fn a_run_that_stalls_is_set_aside_and_lands_late() {
        use crate::archive::{Heard, supervise};
        use std::sync::Arc;
        let c = culled("stall");
        let look = look(&c.rejects, &c.nas, true, Some(&c.db), &no_beat()).unwrap();
        let entries = look.items();
        let cancel = Arc::new(AtomicBool::new(false));
        let held = Arc::new(AtomicBool::new(true));
        let (tx, rx) = std::sync::mpsc::channel();
        let (job_cancel, hold, nas, db) =
            (cancel.clone(), held.clone(), c.nas.clone(), c.db.clone());
        supervise(
            "test rejects stall",
            Duration::from_millis(100),
            move |beat| {
                let count = AtomicU64::new(0);
                let hand = |_: &Path| {
                    while hold.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                };
                let hooks = Hooks {
                    beat: &**beat,
                    cancel: &job_cancel,
                    bytes: &count,
                    before_frame: Some(&hand),
                    tamper: None,
                };
                run_moves(&nas, &entries, Some(&db), &hooks, beat)
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
        cancel.store(true, Ordering::SeqCst);
        held.store(false, Ordering::SeqCst);
        match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            Heard::Done { result, late } => {
                assert!(late);
                assert_eq!(result.moved.len(), 1, "{result:?}");
                assert_eq!(result.canceled.len(), 1);
            }
            Heard::Aside | Heard::Lost => panic!("set aside twice, or lost"),
        }
        assert!(c.there.join("b.tif").is_file());
        std::fs::remove_dir_all(&c.dir).unwrap();
    }

    /// The archive not answering: the copies as the index has them,
    /// the time and size compared on the rows.
    #[test]
    fn offline_the_copies_are_as_the_index_has_them() {
        let c = culled("offline");
        let away = c.dir.join("nas-away");
        std::fs::rename(&c.nas, &away).unwrap();
        let look = look(&c.rejects, &c.nas, false, Some(&c.db), &no_beat()).unwrap();
        assert!(!look.answered);
        assert_eq!(look.copies().0, 2);
        assert_eq!(look.not_known().len(), 1);
        std::fs::remove_dir_all(&c.dir).unwrap();
    }

    #[test]
    fn the_queue_is_read_loosely_and_written_back() {
        let dir = scratch("queue");
        let path = dir.join(QUEUE_FILE);
        std::fs::write(
            &path,
            r#"{"version": 1, "queue": [
                {"archive": "/mnt/nas", "frames": [
                    {"hash": "h1", "name": "a.cr3", "size": 10, "mtime": 5},
                    {"hash": "h2", "name": "b.cr3"},
                    {"name": "c.cr3", "size": 3},
                    {"hash": "h1", "name": "a.cr3", "size": 10}
                ]},
                {"frames": []},
                "not even an object",
                {"archive": "/mnt/cloud", "frames": [{"hash": "h9", "name": "z.cr3", "size": 1}]}
            ]}"#,
        )
        .unwrap();
        let read = read_queue(&path).unwrap();
        assert_eq!(read.queue.len(), 2);
        assert_eq!(read.queue[0].archive, Path::new("/mnt/nas"));
        assert_eq!(read.queue[0].frames.len(), 1);
        assert_eq!(read.queue[0].frames[0].mtime, Some(5));
        assert_eq!(read.dropped.len(), 4, "{:?}", read.dropped);
        // Written and read again: the same, nothing dropped.
        write_queue(&path, &read.queue).unwrap();
        let again = read_queue(&path).unwrap();
        assert_eq!(again.queue, read.queue);
        assert!(again.dropped.is_empty());
        // Emptied: the file goes.
        let (_, ()) = edit_queue(&path, |q| {
            dequeue(q, Path::new("/mnt/nas"), &read.queue[0].frames);
            dequeue(q, Path::new("/mnt/cloud"), &read.queue[1].frames);
        })
        .unwrap();
        assert!(!path.exists());
        // Not a queue at all: set aside, said, and nothing queued.
        std::fs::write(&path, b"{ not json").unwrap();
        let read = read_queue(&path).unwrap();
        assert!(read.queue.is_empty());
        assert_eq!(read.dropped.len(), 1);
        assert!(path.with_extension("json.unreadable").is_file());
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
