//! Indexing: a folder's files against the rows the index holds for
//! it, as little read as the rules allow.
//!
//! The rules, in the order they are tried for each file on disk:
//!
//! 1. A row at this path with the same size and mtime: the file is
//!    not read. Its sidecar is looked for, read (it is small) and
//!    hashed, and when that is not the sidecar the row knows — a
//!    different place or different bytes, or none where there was
//!    one — the meta is written again and only the meta.
//! 2. A row at this path with another size or mtime: the file
//!    changed on disk, and is hashed and probed again.
//! 3. No row at this path: the file is hashed. A row with that hash
//!    whose file is gone from a folder that is still there is this
//!    file moved, and keeps its id and its EXIF; only its path and
//!    its meta are written. A row whose whole folder is gone is not
//!    a move's other end — the folder is on a drive that is not
//!    mounted, as likely as not — so the file is a copy, and a new
//!    row. Otherwise the file is probed and a row is added.
//!
//! What was in the folder's rows and not on disk is marked missing,
//! with the time, and kept: a move to a folder not yet indexed is
//! found when that folder is, from the missing row's hash. A folder
//! that is gone marks its rows missing the same way, from its parent
//! or from a pass over the tree above it.
//!
//! A file the probe cannot read is still a row, with no EXIF, so it
//! is listed by name and rating with the rest; the error is in the
//! report. It is not retried until the file changes. A file whose
//! data makes the decoder panic is the same case: the panic is
//! caught at the file and never takes the folder with it.
//!
//! The pass works in batches of [`BATCH`] files, or of a second,
//! each in two phases: the files are stat'd, hashed and probed with
//! no transaction open, then the batch's rows are written in one
//! short write transaction taken as a write from the start. So a
//! listing never waits, and a second writer — another pass, or the
//! editor's [`Library::index_file`] after a save — gets in between
//! batches rather than polling a lock that is never free. A folder
//! found empty on disk with rows in the index under it is left as it
//! was and counted unavailable: an unmounted drive leaves its mount
//! point behind, and marking a shoot missing because its disk is in
//! a drawer would be Lightroom's exclamation mark.
//!
//! A folder on a network mount ([`Library::set_network`]) has its
//! first phase read ahead, a few files at once and each read under a
//! limit (the `far` module): a file whose read does not answer is an
//! error in the report and keeps its row as it was, for the next pass.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use greycard_edit::meta::Meta;
use greycard_edit::{Placement, Sidecar};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use crate::far::Far;
use crate::{
    Error, Exif, FileMosaic, Library, Result, StyleTags, canonical, canonical_file, filter, hash,
    mtime_of, now_secs, path_bytes, path_from_bytes, path_text,
};

/// Files a transaction holds before it is committed.
pub const BATCH: usize = 50;

/// The longest a transaction is held.
const BATCH_TIME: Duration = Duration::from_secs(1);

/// Where an index run is, for a progress bar: `done` of `total`
/// files in this folder, the one about to be looked at.
#[derive(Debug, Clone, Copy)]
pub struct Progress<'a> {
    pub done: usize,
    pub total: usize,
    pub path: &'a Path,
    /// Rows this folder's pass has written so far, added, moved,
    /// changed, refreshed or back: whether there is anything new for
    /// a window to read.
    pub written: usize,
}

/// What an index run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Files not in the index before.
    pub added: usize,
    /// Files found under a new path by their hash.
    pub moved: usize,
    /// Those files, by the path the row had and the one it has now: a
    /// stack whose member was at the old path follows it.
    pub moved_files: Vec<(PathBuf, PathBuf)>,
    /// Folders found renamed, by the folder the rows had and the one
    /// their files are in now: a folder gone, and one whose rows came
    /// live (added, or back from missing) holding more than half its
    /// files by name and content hash, the two found by this pass or
    /// one a few minutes before it on the same connection
    /// ([`Renames`]). The index keeps such files as rows of their own,
    /// the old ones missing, since a folder gone is what an unmounted
    /// drive looks like too; a stack in the old folder follows to the
    /// new.
    pub moved_folders: Vec<(PathBuf, PathBuf)>,
    /// Of those, the ones whose sidecar was left behind at the old
    /// path and carried to the new one by the pass.
    pub carried: usize,
    /// Files whose size or mtime changed, read again whole.
    pub changed: usize,
    /// Those files, by path: whose pictures are to be made again.
    pub changed_files: Vec<PathBuf>,
    /// Of those, the ones whose content hash changed with them: another
    /// tool rewrote the file in place (a metadata write moves the head
    /// and the size). A collection that holds the old hash takes the
    /// new one from here, by the path.
    pub rehashed: Vec<Rehash>,
    /// Files unchanged whose sidecar changed: the meta written again.
    pub meta_refreshed: usize,
    /// Files whose row was right already.
    pub unchanged: usize,
    /// Files unchanged whose maker's tags were read for the first
    /// time: rows from a library made before the tags were indexed.
    /// Counted among `unchanged` as well.
    pub styled: usize,
    /// Frames whose preview was scored after the pass
    /// ([`crate::score`]); a pass itself scores none.
    pub scored: usize,
    /// Files back at a path the index had marked missing.
    pub returned: usize,
    /// Rows whose file, or whose whole folder, is gone.
    pub missing: usize,
    /// Folders found empty on disk with rows in the index under
    /// them: a drive not mounted, its mount point left behind, as
    /// likely as a shoot deleted, so left as they were.
    pub unavailable: Vec<PathBuf>,
    /// Files that could not be read, and why; each has a row anyway
    /// when it could be hashed. A folder a tree pass could not read is
    /// here too, and in `unreadable`.
    pub errors: Vec<(PathBuf, String)>,
    /// Folders under the pass's own that it could not read (no
    /// permission, a mount gone bad), left as they were.
    pub unreadable: Vec<PathBuf>,
    /// Folders a tree pass did not go into: symbolic links, which
    /// could lead back up the tree.
    pub skipped: Vec<PathBuf>,
    /// The collections, smart collections, and stacks carried in the
    /// folders passed over (`carry`), each file once on a connection
    /// until it changes: what the editor joins into its own.
    pub sets: Vec<crate::carry::File>,
    /// The leftovers in those folders' carried sets, every pass: part
    /// files a write left and deletions past their keep
    /// (`carry::listing`), for the editor's writer to clear.
    pub sets_stale: Vec<PathBuf>,
    /// The pass was asked to stop and did, between two batches: what
    /// it wrote is written, and nothing was marked missing, since the
    /// files it did not reach were not looked for.
    pub stopped: bool,
}

/// A file at a path the index knew, rewritten in place so its content
/// hash is another: the path, the hash the row had and the one it has
/// now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rehash {
    pub path: PathBuf,
    pub old: String,
    pub new: String,
}

impl Report {
    /// Files looked at.
    pub fn seen(&self) -> usize {
        self.added + self.moved + self.changed + self.meta_refreshed + self.unchanged
    }

    /// `other`'s counts and lists added to these: two passes said as
    /// one.
    pub fn add(&mut self, other: Report) {
        self.added += other.added;
        self.moved += other.moved;
        self.moved_files.extend(other.moved_files);
        self.moved_folders.extend(other.moved_folders);
        self.carried += other.carried;
        self.changed += other.changed;
        self.changed_files.extend(other.changed_files);
        self.rehashed.extend(other.rehashed);
        self.meta_refreshed += other.meta_refreshed;
        self.unchanged += other.unchanged;
        self.styled += other.styled;
        self.scored += other.scored;
        self.returned += other.returned;
        self.missing += other.missing;
        self.unavailable.extend(other.unavailable);
        self.errors.extend(other.errors);
        self.unreadable.extend(other.unreadable);
        self.skipped.extend(other.skipped);
        self.sets.extend(other.sets);
        self.sets_stale.extend(other.sets_stale);
        self.stopped |= other.stopped;
    }
}

/// A tree pass in steps: where [`Library::walk_until`] got to. See
/// [`Library::tree_walk`].
#[derive(Debug)]
pub struct TreeWalk {
    root: PathBuf,
    exists: bool,
    /// The folders still to look at, the next on the end.
    dirs: Vec<PathBuf>,
    visited: HashSet<Vec<u8>>,
    /// Folders found empty with rows under them, or unreadable:
    /// nothing under them is marked.
    shielded: Vec<Vec<u8>>,
    /// Folders under the root walked by a pass of their own (a network
    /// mount below a local root): not gone into, and nothing in or
    /// under them marked or asked of the disk.
    left_out: Vec<PathBuf>,
    report: Report,
    /// What a pass over the folder on top of `dirs` did before it
    /// stopped partway, to be counted with the pass that finishes it.
    partial: Option<Report>,
    /// The reads of a root on a network mount, each under a limit;
    /// one walk's, so a read gone unanswered in one folder holds the
    /// next folders' reads too.
    far: Option<Far>,
}

impl TreeWalk {
    /// The root, canonical.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The walk with each of `folders` strictly under its root left out:
    /// not gone into, and nothing in or under them marked missing or
    /// looked for on disk. For a network mount below a local root, which
    /// is passed over as its own.
    pub fn leaving_out(mut self, folders: &[PathBuf]) -> TreeWalk {
        self.left_out = folders
            .iter()
            .filter(|f| f.starts_with(&self.root) && **f != self.root)
            .cloned()
            .collect();
        // Taken up again: the folders already queued under one are not
        // gone into either.
        let left_out = std::mem::take(&mut self.left_out);
        let out = |d: &PathBuf| left_out.iter().any(|l| d.starts_with(l));
        // The folder it stopped inside, left out now: what it did there
        // is kept with the walk's report, not counted with another's.
        if self.dirs.last().is_some_and(out)
            && let Some(partial) = self.partial.take()
        {
            self.report.add(partial);
        }
        self.dirs.retain(|d| !out(d));
        self.left_out = left_out;
        self
    }

    fn leaves_out(&self, dir: &Path) -> bool {
        self.left_out.iter().any(|l| dir.starts_with(l))
    }

    fn stopped(&self) -> Report {
        Report {
            stopped: true,
            ..self.report.clone()
        }
    }
}

/// A folder's row as the pass needs it.
#[derive(Clone)]
struct Row {
    id: i64,
    size: u64,
    mtime: i64,
    sidecar: Option<Vec<u8>>,
    sidecar_hash: Option<String>,
    missing: bool,
    /// Whether the maker's tags have been read into the row.
    style_read: bool,
    /// Whether the file's kind of mosaic has been read into the row.
    mosaic_read: bool,
    /// The row says its sidecar holds a develop.
    edited: bool,
    /// The develop key of its edit, as last read; `None` until a pass
    /// reads the sidecar for it (a row from before schema 9).
    develop_key: Option<i64>,
    /// The content hash the row holds.
    hash: String,
}

/// Whether a path is one the index holds: a raw or a picture, as
/// the browser lists a folder, and not a hidden file
/// ([`greycard_core::decode::is_hidden`]), such as macOS's `._NAME`
/// resource fork beside a raw on a share.
pub fn is_indexed_path(path: &Path) -> bool {
    !greycard_core::decode::is_hidden(path)
        && (greycard_core::decode::is_raw_path(path)
            || greycard_core::picture::is_picture_path(path))
}

/// The files of one folder, sorted, not descending.
#[cfg(test)]
fn list_folder(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    list_folder_marked(dir).map(|(files, _)| files)
}

/// [`list_folder`], and whether the sidecars' folder is in it: where it
/// is not, there are no carried sets to look for.
fn list_folder_marked(dir: &Path) -> std::io::Result<(Vec<PathBuf>, bool)> {
    let mut hidden = false;
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .inspect(|e| {
            hidden |= e.file_name() == greycard_edit::SIDECAR_FOLDER
                && e.file_type().is_ok_and(|t| t.is_dir());
        })
        .filter(|e| is_indexed_path(&e.path()))
        // Regular files, or links to them: a named pipe's read would
        // never return. The entry's own type, from the listing, so a
        // folder over a share costs no round trip a file; only a link
        // is followed.
        .filter(|e| match e.file_type() {
            Ok(t) if t.is_symlink() => e.path().is_file(),
            Ok(t) => t.is_file(),
            Err(_) => false,
        })
        .map(|e| e.path())
        .collect();
    files.sort();
    Ok((files, hidden))
}

/// Whether a folder has no entries at all, hidden ones included.
fn is_empty_dir(dir: &Path) -> std::io::Result<bool> {
    Ok(std::fs::read_dir(dir)?.next().is_none())
}

/// The folders in `dir` a tree pass goes into, not hidden, each with
/// whether it is a link to a folder (named in the report, not gone
/// into).
fn subfolders(dir: &Path) -> std::io::Result<Vec<(PathBuf, bool)>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir)?.filter_map(|e| e.ok()) {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if kind.is_symlink() && entry.path().is_dir() {
            found.push((entry.path(), true));
        } else if kind.is_dir() {
            found.push((entry.path(), false));
        }
    }
    Ok(found)
}

/// `read` of `path`, under a network mount's limit when `far` says it
/// is on one, else in place.
fn read_far<T: Send + 'static>(
    far: Option<&Far>,
    path: &Path,
    read: fn(&Path) -> std::io::Result<T>,
) -> std::io::Result<T> {
    match far {
        Some(f) => {
            let path = path.to_path_buf();
            f.one(move || read(&path))?
        }
        None => read(path),
    }
}

/// Whether the index holds rows in this folder or under it.
fn has_rows_under(conn: &Connection, folder: &[u8]) -> Result<bool> {
    let prefix = under_prefix(folder);
    Ok(conn
        .prepare_cached("SELECT 1 FROM files WHERE folder = ? OR substr(folder, 1, ?) = ? LIMIT 1")?
        .exists(params![folder, prefix.len() as i64, prefix])?)
}

/// The folder's bytes with a separator on the end: what a folder
/// under it starts with. A folder that ends in one already, `/` or a
/// drive's own root on Windows, keeps the one it has, or nothing
/// would ever be under it.
pub(crate) fn under_prefix(folder: &[u8]) -> Vec<u8> {
    let mut prefix = folder.to_vec();
    let text = path_from_bytes(folder);
    let text = text.to_string_lossy();
    if !text.ends_with(std::path::MAIN_SEPARATOR) && !text.ends_with('/') {
        prefix.extend_from_slice(&path_bytes(Path::new(std::path::MAIN_SEPARATOR_STR)));
    }
    prefix
}

/// A folder that does not exist and that the index has nothing under
/// is a name mistyped, not a folder deleted.
fn no_such_folder(dir: &Path) -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("there is no {}", dir.display()),
    ))
}

/// The folder as the index keys it, and whether it is there. A
/// folder that is gone is keyed through its parent, which has to be
/// there: what the caller does with a gone folder depends on whether
/// the index has rows under it, and a parent gone too is an error
/// either way.
fn resolve_folder(dir: &Path) -> Result<(PathBuf, bool)> {
    match canonical(dir) {
        Ok(d) => Ok((d, true)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
                return Err(Error::Io(e));
            };
            let parent = if parent.as_os_str().is_empty() {
                canonical(Path::new("."))?
            } else {
                canonical(parent)?
            };
            Ok((parent.join(name), false))
        }
        Err(e) => Err(Error::Io(e)),
    }
}

impl Library {
    /// Index one folder's files, not its subfolders: see the module
    /// for the rules. `progress` is called before each file. A folder
    /// that is gone, its parent still there and the index holding
    /// rows under it, marks its rows missing; one the index knows
    /// nothing of is an error, since it is a name mistyped. A folder
    /// found empty with rows under it is left as it was and counted
    /// unavailable.
    pub fn index_folder(
        &mut self,
        dir: &Path,
        progress: &mut dyn FnMut(Progress<'_>),
    ) -> Result<Report> {
        self.index_folder_until(dir, progress, &|| false)
    }

    /// [`Library::index_folder`], asking `stop` after each batch is
    /// written and ending there when it says so, with
    /// [`Report::stopped`] set: a pass over a folder the editor has
    /// since left, or one a save's `index_file` should not wait
    /// behind. The rows written stand; nothing is marked missing,
    /// and the next pass takes up where this one left off, since
    /// the files already written are unchanged to it.
    pub fn index_folder_until(
        &mut self,
        dir: &Path,
        progress: &mut dyn FnMut(Progress<'_>),
        stop: &dyn Fn() -> bool,
    ) -> Result<Report> {
        let far = self.far_for(dir);
        self.index_folder_far(dir, progress, stop, far.as_ref())
    }

    /// [`Library::index_folder_until`] with its reads over a network
    /// mount's limit, when `far` says it is on one.
    fn index_folder_far(
        &mut self,
        dir: &Path,
        progress: &mut dyn FnMut(Progress<'_>),
        stop: &dyn Fn() -> bool,
        far: Option<&Far>,
    ) -> Result<Report> {
        let (dir, exists) = match far {
            Some(f) => {
                let asked = dir.to_path_buf();
                f.one(move || resolve_folder(&asked))??
            }
            None => resolve_folder(dir)?,
        };
        let folder = path_bytes(&dir);
        if !exists && !has_rows_under(self.conn_mut(), &folder)? {
            return Err(no_such_folder(&dir));
        }
        let (files, hidden) = if exists {
            read_far(far, &dir, list_folder_marked)?
        } else {
            (Vec::new(), false)
        };
        if exists
            && files.is_empty()
            && read_far(far, &dir, is_empty_dir)?
            && has_rows_under(self.conn_mut(), &folder)?
        {
            log::info!("{}: empty on disk, left as it was", dir.display());
            return Ok(Report {
                unavailable: vec![dir],
                ..Report::default()
            });
        }
        let existing = rows_in_folder(self.conn_mut(), &folder)?;
        let live_before: HashSet<Vec<u8>> = existing
            .iter()
            .filter(|(_, r)| !r.missing)
            .map(|(k, _)| k.clone())
            .collect();
        let gone: Files = if exists {
            Files::new()
        } else {
            existing
                .iter()
                .filter(|(_, r)| !r.missing)
                .filter_map(|(k, r)| Some((name_of(k)?, r.hash.clone())))
                .collect()
        };
        let takes = self.lightroom;
        let mut report = index_paths(
            self.conn_mut(),
            &dir,
            &files,
            true,
            existing,
            progress,
            stop,
            far,
            takes,
        )?;
        // One half of a rename: this folder gone with its files, or
        // files come live in it.
        if !exists && !report.stopped && !gone.is_empty() {
            let pairs = self.renames.gone(vec![(dir.clone(), gone)], Instant::now());
            follow_missing(self.conn_mut(), &pairs)?;
            report.moved_folders.extend(pairs);
        } else if exists && report.added + report.returned + report.moved > 0 {
            let came = live_files(self.conn_mut(), &folder, Some(&live_before))?;
            let pairs: Vec<_> = self
                .renames
                .live(dir.clone(), came, Instant::now())
                .into_iter()
                .collect();
            follow_missing(self.conn_mut(), &pairs)?;
            report.moved_folders.extend(pairs);
        }
        if hidden && !report.stopped {
            (report.sets, report.sets_stale) = self.read_sets(&dir, far);
        }
        Ok(report)
    }

    /// The carried sets in `dir` not read on this connection at the
    /// size and time they have now: only those are read. One that
    /// cannot be read is said in the log and tried again by the next
    /// pass. With them, the leftovers there to clear, every pass.
    fn read_sets(
        &mut self,
        dir: &Path,
        far: Option<&Far>,
    ) -> (Vec<crate::carry::File>, Vec<PathBuf>) {
        let sets = crate::carry::sets_dir(dir);
        let listing = match read_far(far, &sets, crate::carry::listing) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), Vec::new()),
            Err(e) => {
                log::warn!("carried sets {}: {e}", sets.display());
                return (Vec::new(), Vec::new());
            }
        };
        let stale = listing.stale.iter().map(|n| sets.join(n)).collect();
        let mut found = Vec::new();
        for (name, stamp) in listing.copies {
            let path = sets.join(&name);
            if self.sets_seen.get(&path) == Some(&stamp) {
                continue;
            }
            match read_far(far, &path, |p| std::fs::read(p)) {
                Ok(bytes) => {
                    self.sets_seen.insert(path, stamp);
                    found.push(crate::carry::File {
                        folder: dir.to_path_buf(),
                        name,
                        bytes,
                    });
                }
                Err(e) => log::warn!("carried set {}: {e}", path.display()),
            }
        }
        (found, stale)
    }

    /// Index a folder and every folder under it, hidden ones (a
    /// leading dot, the sidecar folder among them) left alone and
    /// symbolic links to folders not followed but named in the
    /// report. Rows in folders under the root that are no longer
    /// there are marked missing, unless the folder they are under is
    /// empty on disk, which is a mount point with nothing mounted.
    pub fn index_tree(
        &mut self,
        root: &Path,
        progress: &mut dyn FnMut(Progress<'_>),
    ) -> Result<Report> {
        self.index_tree_until(root, progress, &|| false)
    }

    /// [`Library::index_tree`], asking `stop` after each batch as
    /// [`Library::index_folder_until`] does, and ending there with
    /// [`Report::stopped`] set: the launch pass over the roots, which
    /// gives way to a folder the window has opened or a save's row.
    /// Nothing is marked missing by a pass that stopped, since the
    /// folders it did not reach were not looked for.
    pub fn index_tree_until(
        &mut self,
        root: &Path,
        progress: &mut dyn FnMut(Progress<'_>),
        stop: &dyn Fn() -> bool,
    ) -> Result<Report> {
        let mut walk = self.tree_walk(root)?;
        self.walk_until(&mut walk, progress, stop)
    }

    /// A tree pass to be taken in steps: [`Library::walk_until`] walks
    /// it until it is asked to stop, and the next call takes up where
    /// that one stopped rather than from the root. The launch pass
    /// over a large root stops for every save while someone culls;
    /// started again from the root each time it re-walked the part it
    /// had done, a stat and a sidecar read a file, and at two or three
    /// keys a second it could make no headway at all.
    pub fn tree_walk(&mut self, root: &Path) -> Result<TreeWalk> {
        let far = self.far_for(root);
        let (root, exists) = match &far {
            Some(f) => {
                let asked = root.to_path_buf();
                f.one(move || resolve_folder(&asked))??
            }
            None => resolve_folder(root)?,
        };
        if !exists && !has_rows_under(self.conn_mut(), &path_bytes(&root))? {
            return Err(no_such_folder(&root));
        }
        Ok(TreeWalk {
            dirs: vec![root.clone()],
            root,
            exists,
            visited: HashSet::new(),
            shielded: Vec::new(),
            left_out: Vec::new(),
            report: Report::default(),
            partial: None,
            far,
        })
    }

    /// Walk `walk` on until it is done, or until `stop` says so after
    /// a batch or between folders, when the report comes back with
    /// [`Report::stopped`] set and `walk` holds where it got to: the
    /// folder it was in the middle of is done again, its finished
    /// files unchanged to the second look. At least one folder is
    /// looked at a call. The report is the whole walk's so far.
    pub fn walk_until(
        &mut self,
        walk: &mut TreeWalk,
        progress: &mut dyn FnMut(Progress<'_>),
        stop: &dyn Fn() -> bool,
    ) -> Result<Report> {
        let root = walk.root.clone();
        let mut first = true;
        while let Some(dir) = walk.dirs.pop() {
            // Between folders as well as between batches: a tree of
            // small folders never fills a batch.
            if !first && stop() {
                walk.dirs.push(dir);
                return Ok(walk.stopped());
            }
            first = false;
            // A folder under the root that cannot be read (no
            // permission, a mount gone bad) is said in the report and
            // left as it was, rows and all; the rest of the tree is
            // walked. The first cut let it end the pass, and a root
            // with one locked folder in it was never indexed past it.
            let far = walk.far.clone();
            let one = match self.index_folder_far(&dir, progress, stop, far.as_ref()) {
                Ok(one) => one,
                Err(Error::Io(e)) if dir != root => {
                    log::warn!("{}: {e}; left as it was", dir.display());
                    walk.report.errors.push((dir.clone(), e.to_string()));
                    walk.report.unreadable.push(dir.clone());
                    walk.visited.insert(path_bytes(&dir));
                    walk.shielded.push(under_prefix(&path_bytes(&dir)));
                    continue;
                }
                Err(e) => return Err(e),
            };
            // The files a stopped pass over this folder already did are
            // looked at again, and unchanged to the second look: counted
            // once, as whatever the first look found them.
            let mut one = one;
            if let Some(partial) = walk.partial.take() {
                one.unchanged = one.unchanged.saturating_sub(partial.seen());
                one.add(partial);
            }
            if one.stopped {
                // Taken up again from this folder.
                one.stopped = false;
                walk.partial = Some(one);
                walk.dirs.push(dir);
                return Ok(walk.stopped());
            }

            if !one.unavailable.is_empty() {
                walk.shielded.push(under_prefix(&path_bytes(&dir)));
            }
            walk.report.add(one);
            walk.visited.insert(path_bytes(&dir));
            if !walk.exists {
                continue;
            }
            let mut under = Vec::new();
            let listing = match read_far(walk.far.as_ref(), &dir, subfolders) {
                Ok(l) => l,
                Err(e) if dir != root => {
                    walk.report.errors.push((dir.clone(), e.to_string()));
                    walk.report.unreadable.push(dir.clone());
                    walk.shielded.push(under_prefix(&path_bytes(&dir)));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            for (path, link) in listing {
                if link {
                    walk.report.skipped.push(path);
                } else if !walk.leaves_out(&path) {
                    under.push(path);
                }
            }
            // Popped from the end, so reversed to walk in name order.
            under.sort();
            under.reverse();
            walk.dirs.extend(under);
        }
        // Folders the index holds under the root that the walk did
        // not reach and that are not there: gone, with their files.
        let prefix = under_prefix(&path_bytes(&root));
        let gone: Vec<Vec<u8>> = {
            let mut stmt = self.conn_mut().prepare_cached(
                "SELECT DISTINCT folder FROM files \
                 WHERE substr(folder, 1, ?) = ? AND missing_since IS NULL",
            )?;
            let rows = stmt.query_map(params![prefix.len() as i64, prefix], |r| {
                r.get::<_, Vec<u8>>(0)
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let now = now_secs();
        let mut went = Vec::new();
        for folder in gone {
            // The cheap questions first: a left-out folder (a share below
            // the root) is not asked of the disk at all.
            if walk.visited.contains(&folder)
                || walk.leaves_out(&path_from_bytes(&folder))
                || walk.shielded.iter().any(|s| folder.starts_with(s))
                // A share that does not answer has not said it is gone.
                || read_far(walk.far.as_ref(), &path_from_bytes(&folder), |d| Ok(d.is_dir()))
                    .unwrap_or(true)
            {
                continue;
            }
            let had = live_files(self.conn_mut(), &folder, None)?;
            let marked = self.conn_mut().execute(
                "UPDATE files SET missing_since = ? WHERE folder = ? AND missing_since IS NULL",
                params![now, folder],
            )?;
            log::info!(
                "{}: gone, {marked} files marked missing",
                path_from_bytes(&folder).display()
            );
            walk.report.missing += marked;
            went.push((path_from_bytes(&folder), had));
        }
        // All at once: a folder gone in this pass is no folder another
        // was renamed to, whatever order they were found in.
        let pairs = self.renames.gone(went, Instant::now());
        follow_missing(self.conn_mut(), &pairs)?;
        walk.report.moved_folders.extend(pairs);
        Ok(walk.report.clone())
    }

    /// Bring one file's row up to date, under the same rules as its
    /// folder: after a sidecar is saved, for the row to say what the
    /// sidecar says without a pass over the folder. A file gone from
    /// its path is marked missing.
    pub fn index_file(&mut self, path: &Path) -> Result<Report> {
        let path = canonical_file(path);
        let folder = path.parent().unwrap_or(Path::new("")).to_path_buf();
        let mut existing = HashMap::new();
        if let Some(row) = row_at(self.conn_mut(), &path_bytes(&path))? {
            existing.insert(path_bytes(&path), row);
        }
        // A hidden file is no row, as its folder's listing has it.
        let files: Vec<PathBuf> = if path.is_file() && !greycard_core::decode::is_hidden(&path) {
            vec![path]
        } else {
            Vec::new()
        };
        let takes = self.lightroom;
        index_paths(
            self.conn_mut(),
            &folder,
            &files,
            false,
            existing,
            &mut |_| {},
            &|| false,
            None,
            takes,
        )
    }
}

fn row_from(r: &rusqlite::Row<'_>, from: usize) -> rusqlite::Result<Row> {
    Ok(Row {
        id: r.get(from)?,
        size: r.get::<_, i64>(from + 1)? as u64,
        mtime: r.get(from + 2)?,
        sidecar: r.get(from + 3)?,
        sidecar_hash: r.get(from + 4)?,
        missing: r.get::<_, Option<i64>>(from + 5)?.is_some(),
        style_read: r.get::<_, i64>(from + 6)? != 0,
        edited: r.get::<_, i64>(from + 7)? != 0,
        develop_key: r.get(from + 8)?,
        hash: r.get(from + 9)?,
        mosaic_read: r.get::<_, i64>(from + 10)? != 0,
    })
}

fn rows_in_folder(conn: &Connection, folder: &[u8]) -> Result<HashMap<Vec<u8>, Row>> {
    let mut stmt = conn.prepare_cached(
        "SELECT path, id, size, mtime, sidecar, sidecar_hash, missing_since, style_read, \
         edited, develop_key, hash, mosaic IS NOT NULL FROM files WHERE folder = ?",
    )?;
    let rows = stmt.query_map(params![folder], |r| {
        Ok((r.get::<_, Vec<u8>>(0)?, row_from(r, 1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

fn row_at(conn: &Connection, path: &[u8]) -> Result<Option<Row>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .prepare_cached(
            "SELECT id, size, mtime, sidecar, sidecar_hash, missing_since, style_read, \
             edited, develop_key, hash, mosaic IS NOT NULL FROM files WHERE path = ?",
        )?
        .query_row(params![path], |r| row_from(r, 0))
        .optional()?)
}

/// What a file has beside it now that the row mirrors: its `.gcd`,
/// where it is, and the XMP another tool wrote, with one hash over
/// both so a change to either has the row written again, and the
/// bytes themselves for the meta to be read from once. `None` when
/// the frame has neither.
struct SidecarNow {
    /// The `.gcd`, wherever it was found; none when the frame has only
    /// an XMP.
    path: Option<PathBuf>,
    hash: String,
    /// The `.gcd`'s mtime, else the XMP's.
    mtime: i64,
    json: Option<Vec<u8>>,
    /// The XMP beside the frame, as `xmp::path_of` picks it.
    xmp: Option<PathBuf>,
    /// A culling tool's `IMG.xmp` the raw shares with its JPEG, read
    /// and parsed once here for the summary's adoption to reuse.
    shared: Option<greycard_edit_io::xmp::Shared>,
}

/// A raw found moved from `old` to `new` with no sidecar at the new
/// place: the sidecar it left behind, beside the old path or under the
/// old folder's hidden `.greycard`, carried to the new folder in the
/// same placement, so a raw dragged by hand keeps its edits and its
/// flag. The hidden placement is the one a hand move forgets, since a
/// file manager shows nothing beside the raw to take along. Nothing is
/// written over: a sidecar at the new place under either placement
/// leaves the old one where it is, and so does a name taken there by
/// the time the move is made. The XMP, a visible file beside the raw,
/// is left to the hand that moved the raw. Done in the first phase,
/// before the new place's sidecar is read, so the row's meta is the
/// carried sidecar's. True when a sidecar was carried. The asides are
/// found in `listings`, the batch's one listing of the old folder.
fn carry_sidecar(old: &Path, new: &Path, listings: &mut Listings) -> bool {
    disk_call();
    let Some(from) = Sidecar::find(old) else {
        return false;
    };
    if Sidecar::find(new).is_some() {
        return false;
    }
    let placement = if from.parent().and_then(Path::file_name)
        == Some(std::ffi::OsStr::new(greycard_edit::SIDECAR_FOLDER))
    {
        Placement::Folder
    } else {
        Placement::Beside
    };
    let to = Sidecar::path_in(new, placement);
    if placement == Placement::Folder
        && let Some(folder) = new.parent()
        && let Err(e) = Sidecar::folder_under(folder)
    {
        log::warn!(
            "{}: sidecar not carried from {}: {e}",
            new.display(),
            from.display()
        );
        return false;
    }
    // What saves put aside beside it goes along under the new name, so
    // no later frame of the old name takes it for its own.
    let asides = listings.asides_of(&from);
    match move_noreplace(&from, &to) {
        Ok(()) => {
            log::info!("{}: sidecar carried from {}", to.display(), from.display());
            listings.moved(&from, &to);
            for aside in asides {
                let there = greycard_edit::aside_at(&aside, &from, &to);
                match move_noreplace(&aside, &there) {
                    Ok(()) => listings.moved(&aside, &there),
                    Err(e) => log::warn!(
                        "{}: not carried to {}: {e}",
                        aside.display(),
                        there.display()
                    ),
                }
            }
            true
        }
        Err(e) => {
            log::warn!(
                "{}: sidecar not carried from {}: {e}",
                to.display(),
                from.display()
            );
            false
        }
    }
}

/// The names in the folders a batch carries sidecars out of, each
/// listed once a batch: a folder of raws moved by hand is a carry a
/// frame, and a listing a carry would list the folder once a frame,
/// slow on a share. Kept to what the folder holds as the batch's own
/// moves change it, so it answers as a fresh listing would. Owned by
/// one batch and dropped at its end, since another hand may change the
/// folder meanwhile; `move_noreplace` stays the judge of a name taken.
#[derive(Default)]
struct Listings(HashMap<PathBuf, Vec<OsString>>);

impl Listings {
    /// What saves put aside beside the sidecar at `sidecar`
    /// ([`greycard_edit::asides_among`]): none when its folder will not
    /// list, and a folder that would not list is asked again next time.
    fn asides_of(&mut self, sidecar: &Path) -> Vec<PathBuf> {
        let Some(folder) = sidecar.parent() else {
            return Vec::new();
        };
        if !self.0.contains_key(folder) {
            #[cfg(test)]
            LISTED.with(|l| l.set(l.get() + 1));
            let Ok(names) = std::fs::read_dir(folder)
                .and_then(|d| d.map(|e| e.map(|e| e.file_name())).collect())
            else {
                return Vec::new();
            };
            self.0.insert(folder.to_path_buf(), names);
        }
        greycard_edit::asides_among(sidecar, &self.0[folder])
    }

    /// `from` moved to `to` by this batch: gone from its folder's
    /// names and there in the other's, for each listed.
    fn moved(&mut self, from: &Path, to: &Path) {
        if let (Some(folder), Some(name)) = (from.parent(), from.file_name())
            && let Some(names) = self.0.get_mut(folder)
        {
            names.retain(|n| n != name);
        }
        if let (Some(folder), Some(name)) = (to.parent(), to.file_name())
            && let Some(names) = self.0.get_mut(folder)
        {
            names.push(name.to_os_string());
        }
    }
}

/// `from` moved to `to`, never over a file there: a hard link, which
/// fails where `to` exists, then the unlink; across devices or on a
/// file system without links, a copy into a file that must be new,
/// then the unlink.
fn move_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::hard_link(from, to) {
        Ok(()) => return std::fs::remove_file(from),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(e),
        Err(_) => {}
    }
    let bytes = std::fs::read(from)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)?;
    std::io::Write::write_all(&mut file, &bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::remove_file(from)
}

/// Folded into the hash of a frame with an XMP beside it by a pass that
/// takes Lightroom's develops: a row that holds it has had the XMP
/// looked at for one, and a row from a pass that did not look (one
/// that takes nothing, or a build before the take) is read again once.
const LOOKED: &[u8] = b"an XMP looked at for a Lightroom develop";
/// What goes into a row's hash before a culling tool's shared XMP, so
/// its bytes are never taken for the frame's own.
const SHARED: &[u8] = b"a culling tool's XMP shared with a JPEG";

/// A sidecar is small and is read whole: its hash is what says
/// whether it changed, since a save that changes one digit of a
/// rating changes neither its length nor, within one timestamp
/// tick, its mtime. The XMP beside the frame is read the same way
/// and folded into the hash, so a rating given in another tool is
/// seen by the next pass as a save here is.
fn sidecar_of(raw: &Path, looks: bool) -> Option<SidecarNow> {
    disk_call();
    let path = Sidecar::find(raw);
    let greycard_edit_io::xmp::Beside { own: xmp, shared } = greycard_edit_io::xmp::beside(raw);
    if path.is_none() && xmp.is_none() && shared.is_none() {
        return None;
    }
    let json = path.as_deref().and_then(|p| std::fs::read(p).ok());
    let xmp_bytes = xmp
        .as_deref()
        .and_then(|p| greycard_edit_io::xmp::bytes_of(p).ok());
    if json.is_none() && xmp_bytes.is_none() && shared.is_none() {
        return None;
    }
    let mtime = path
        .as_deref()
        .or(xmp.as_deref())
        .or(shared.as_ref().map(|s| s.path.as_path()))
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| mtime_of(&m))
        .unwrap_or(0);
    // A frame with a `.gcd` alone hashes as it always did; the XMP,
    // when there is one, goes in after it with its length, so that a
    // byte moved from one file to the other is a change too. An XMP
    // a pass that takes looked at is marked so.
    let mut hasher = blake3::Hasher::new();
    hasher.update(json.as_deref().unwrap_or_default());
    if looks && xmp_bytes.is_some() {
        hasher.update(LOOKED);
    }
    if let Some(x) = &xmp_bytes {
        hasher.update(&(x.len() as u64).to_le_bytes());
        hasher.update(x);
    }
    // A culling tool's shared XMP after both, marked, so a rating
    // given there is seen too.
    if let Some(s) = &shared {
        hasher.update(SHARED);
        hasher.update(&(s.bytes.len() as u64).to_le_bytes());
        hasher.update(&s.bytes);
    }
    Some(SidecarNow {
        path,
        hash: hasher.finalize().to_hex().to_string(),
        mtime,
        json,
        xmp,
        shared,
    })
}

impl Row {
    /// Whether the sidecar beside the frame is the one the row mirrors,
    /// so nothing of it need be read again. An edited row with no
    /// develop key yet (from before schema 9) is read again whatever
    /// its hash, once, to fill it.
    fn same_sidecar(&self, now: Option<&SidecarNow>) -> bool {
        if now.is_some() && self.edited && self.develop_key.is_none() {
            return false;
        }
        match now {
            None => self.sidecar.is_none() && self.sidecar_hash.is_none(),
            Some(s) => {
                self.sidecar.as_deref() == s.path.as_deref().map(path_bytes).as_deref()
                    && self.sidecar_hash.as_deref() == Some(s.hash.as_str())
            }
        }
    }
}

/// A write transaction, taken as one from the start. A deferred
/// transaction begins as a read and asks for the write lock at its
/// first write, and SQLite answers that upgrade with "busy" at once
/// rather than waiting, so two passes on two folders, or a pass and
/// the editor's `index_file`, collided instead of taking turns.
/// Immediate takes the lock up front, under the connection's busy
/// timeout.
fn begin(conn: &mut Connection) -> Result<Transaction<'_>> {
    Ok(conn.transaction_with_behavior(TransactionBehavior::Immediate)?)
}

/// What the first phase found out about a file, for the second to
/// write.
enum Plan {
    /// The row's size and mtime match the file's; with the maker's
    /// tags when the row had not had them read, and its kind of mosaic
    /// when the row had not had that read (the inner `None`: the file
    /// would not say).
    Same(Row, Option<StyleTags>, Option<Option<FileMosaic>>),
    /// The file changed on disk: hashed and probed again.
    Changed { row: Row, hash: String, exif: Exif },
    /// No row at this path when the folder's rows were read: hashed,
    /// and probed unless a move looked likely, since a move keeps
    /// its EXIF.
    Fresh { hash: String, exif: Option<Exif> },
}

/// One file looked at, with everything the disk had to say of it:
/// the file itself, its sidecar and XMP as they were then and what the
/// row mirrors of them, and for a file new to the folder the gone row
/// it most likely moved from. The second phase asks the disk nothing
/// more, so a share that stops answering mid-pass stops it in the first
/// phase, holding no lock. A save made between the phases has already
/// written its own row (`index_file`), and the second phase leaves that
/// row's meta as the save wrote it; a save after the commit writes it
/// itself.
struct Looked<'a> {
    path: &'a Path,
    key: Vec<u8>,
    size: u64,
    mtime: i64,
    plan: Plan,
    sidecar: Option<SidecarNow>,
    summary: Summary,
    /// A row with this file's hash whose file is gone from a folder
    /// still in use: the move's other end, as the first phase found it.
    moved: Option<(i64, Vec<u8>)>,
}

#[cfg(test)]
thread_local! {
    /// While set, the second phase of a batch is under way and holds
    /// the write lock: a disk call counted then is one made under it.
    static UNDER_LOCK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Disk calls made by a pass on this thread: all of them, and
    /// those under the write lock.
    static DISK_CALLS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    /// While set, `probe` is under way: its disk calls are the one kind
    /// the second phase may make.
    static IN_PROBE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Folders listed for the asides of the sidecars carried on this
    /// thread.
    static LISTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A test's count of a disk call the pass makes. Under the write lock
/// one outside `probe` is a panic, so a disk call added to the second
/// phase later fails the tests rather than slipping past the count.
#[inline]
fn disk_call() {
    #[cfg(test)]
    {
        let locked = UNDER_LOCK.with(|u| u.get());
        assert!(
            !locked || IN_PROBE.with(|p| p.get()),
            "a disk call under the write lock outside probe"
        );
        DISK_CALLS.with(|c| {
            let (all, under) = c.get();
            c.set((all + 1, under + usize::from(locked)));
        });
    }
}

/// What the row says of a file, for a read ahead to decide what to
/// read of it as the pass would.
#[derive(Clone, Copy)]
struct Hint {
    size: u64,
    mtime: i64,
    style_read: bool,
    mosaic_read: bool,
}

impl Hint {
    fn of(row: &Row) -> Hint {
        Hint {
            size: row.size,
            mtime: row.mtime,
            style_read: row.style_read,
            mosaic_read: row.mosaic_read,
        }
    }
}

/// A file's first phase read ahead on a thread of its own: whatever
/// the pass would read of it, given what its row says. A fresh file is
/// probed with its hash, though a move would not need it: a probe
/// wasted on a rare move costs less than a second round trip on every
/// file. Each field the pass takes is taken out, so it is used once.
struct Ahead {
    stat: std::result::Result<(u64, i64), String>,
    hash: Option<std::result::Result<String, String>>,
    /// The EXIF and the probe's lines for the report.
    exif: Option<(Exif, Vec<(PathBuf, String)>)>,
    /// The maker's tags read ahead, when they were: `Some(None)` for a
    /// read that failed, which the pass does not make again.
    style: Option<Option<StyleTags>>,
    mosaic: Option<Option<FileMosaic>>,
    sidecar: Option<SidecarNow>,
    summary: Summary,
}

fn read_ahead(path: &Path, hint: Option<Hint>, looks: bool) -> Ahead {
    #[cfg(test)]
    tests::held_read(path);
    let stat = std::fs::metadata(path)
        .map(|m| (m.len(), mtime_of(&m)))
        .map_err(|e| e.to_string());
    let mut ahead = Ahead {
        stat: stat.clone(),
        hash: None,
        exif: None,
        style: None,
        mosaic: None,
        sidecar: None,
        summary: Summary::default(),
    };
    let Ok((size, mtime)) = stat else {
        return ahead;
    };
    match hint {
        Some(h) if h.size == size && h.mtime == mtime => {
            ahead.style = (!h.style_read).then(|| read_style(path));
            ahead.mosaic = (!h.mosaic_read).then(|| read_mosaic(path));
        }
        _ => {
            let hash = hash::hash_file(path).map_err(|e| e.to_string());
            if hash.is_ok() {
                let mut report = Report::default();
                let exif = probe(path, &mut report);
                ahead.exif = Some((exif, report.errors));
            }
            ahead.hash = Some(hash);
        }
    }
    ahead.sidecar = sidecar_of(path, looks);
    ahead.summary = summary_of(ahead.sidecar.as_ref(), path);
    ahead
}

/// The pass over `files`, which are all in `folder`, against
/// `existing`, the folder's rows by path; what is left of `existing`
/// at the end is marked missing.
///
/// Two phases a batch. The first stats, hashes and probes the files
/// with no transaction open, which is where the time goes — a cold
/// raw is tens of milliseconds — and the second writes the batch's
/// rows in one short write transaction. The first cut held the
/// write lock through the probing and took the next transaction the
/// moment it committed, and a second writer polling for the lock
/// never once found it free and gave up at its timeout. `existing`
/// was read before the pass and another writer may have added a row
/// since, so a path it did not hold is looked up again inside the
/// transaction before anything is inserted.
// The connection, the folder's files and rows, progress, stop, and the
// channel the far reads are asked on.
#[allow(clippy::too_many_arguments)]
fn index_paths(
    conn: &mut Connection,
    folder: &Path,
    files: &[PathBuf],
    whole_folder: bool,
    mut existing: HashMap<Vec<u8>, Row>,
    progress: &mut dyn FnMut(Progress<'_>),
    stop: &dyn Fn() -> bool,
    far: Option<&Far>,
    takes: Option<Placement>,
) -> Result<Report> {
    let looks = takes.is_some();
    let mut report = Report::default();
    let total = files.len();
    let folder_bytes = path_bytes(folder);
    let folder_text = path_text(folder);
    let mut looked: Vec<Looked<'_>> = Vec::with_capacity(BATCH);
    let mut disk = Disk {
        folder: &folder_bytes,
        listed: whole_folder.then(|| files.iter().map(|p| path_bytes(p)).collect()),
        in_use: HashMap::new(),
        far,
        unanswered: None,
    };
    // Over a network mount the first phase's reads of each file are
    // made ahead, several at once and each under the limit, with what
    // the row says to decide what to read; the pass below takes them in
    // order and reads in place only what they could not know.
    let mut ahead = far.map(|f| {
        let items: Vec<(PathBuf, Option<Hint>)> = files
            .iter()
            .map(|p| (p.clone(), existing.get(&path_bytes(p)).map(Hint::of)))
            .collect();
        let read: Arc<dyn Fn((PathBuf, Option<Hint>)) -> Ahead + Send + Sync> =
            Arc::new(move |(path, hint)| read_ahead(&path, hint, looks));
        f.many(items, read)
    });
    let mut since = Instant::now();
    let mut listings = Listings::default();
    for (done, path) in files.iter().enumerate() {
        progress(Progress {
            done,
            total,
            path,
            written: report.added
                + report.moved
                + report.changed
                + report.meta_refreshed
                + report.returned
                + report.styled,
        });
        let key = path_bytes(path);
        let mut early = match ahead.as_mut().and_then(Iterator::next) {
            None => None,
            Some(Ok(a)) => Some(a),
            Some(Err(e)) => {
                // Not answered: its row is kept as it is, not marked
                // missing, and the next pass asks again.
                existing.remove(&key);
                report.errors.push((path.clone(), e.to_string()));
                continue;
            }
        };
        let stat = match early.as_ref() {
            Some(a) => a.stat.clone(),
            None => {
                disk_call();
                std::fs::metadata(path)
                    .map(|m| (m.len(), mtime_of(&m)))
                    .map_err(|e| e.to_string())
            }
        };
        let (size, mtime) = match stat {
            Ok(s) => s,
            Err(e) => {
                report.errors.push((path.clone(), e));
                continue;
            }
        };
        let hashed = |report: &mut Report, early: &mut Option<Ahead>| {
            let got = match early.as_mut().and_then(|a| a.hash.take()) {
                Some(got) => got,
                None => {
                    disk_call();
                    hash::hash_file(path).map_err(|e| e.to_string())
                }
            };
            match got {
                Ok(h) => Some(h),
                Err(e) => {
                    report.errors.push((path.clone(), e));
                    None
                }
            }
        };
        let probed = |report: &mut Report, early: &mut Option<Ahead>| match early
            .as_mut()
            .and_then(|a| a.exif.take())
        {
            Some((exif, errors)) => {
                report.errors.extend(errors);
                exif
            }
            None => probe(path, report),
        };
        let mut moved = None;
        let mut carried = false;
        let plan = match existing.remove(&key) {
            Some(row) if row.size == size && row.mtime == mtime => {
                // A read ahead that failed is not made again here: on
                // a share that would be a second read for nothing.
                let style = match early.as_mut().and_then(|a| a.style.take()) {
                    Some(read) => read,
                    None => (!row.style_read).then(|| read_style(path)).flatten(),
                };
                let mosaic = match early.as_mut().and_then(|a| a.mosaic.take()) {
                    Some(mosaic) => Some(mosaic),
                    None => (!row.mosaic_read).then(|| read_mosaic(path)),
                };
                Plan::Same(row, style, mosaic)
            }
            Some(row) => {
                let Some(hash) = hashed(&mut report, &mut early) else {
                    continue;
                };
                let exif = probed(&mut report, &mut early);
                Plan::Changed { row, hash, exif }
            }
            None => {
                let Some(hash) = hashed(&mut report, &mut early) else {
                    continue;
                };
                let found = gone_by_hash(conn, &hash, &mut disk)?;
                // A move keeps its EXIF, so a file found moved is not
                // probed; over a network mount it was probed ahead
                // anyway, and that is kept, so a move that turns out
                // not to be one under the lock asks the share nothing
                // there.
                let exif = if found.is_some() && early.is_none() {
                    None
                } else {
                    Some(probed(&mut report, &mut early))
                };
                // A read that did not answer decides nothing: taken for a
                // file gone or a folder empty, it would make a move a
                // copy, and the next pass would find the copy current.
                // The file is left to the next pass, which asks again.
                if let Some(e) = disk.unanswered.take() {
                    report.errors.push((path.clone(), e));
                    continue;
                }
                if let Some((_, old)) = &found {
                    let (old, new) = (path_from_bytes(old), path.clone());
                    carried = match far {
                        Some(f) => {
                            let mut held = std::mem::take(&mut listings);
                            let carry = f.one(move || {
                                #[cfg(test)]
                                tests::held_read(&old);
                                let carried = carry_sidecar(&old, &new, &mut held);
                                (carried, held)
                            });
                            match carry {
                                Ok((carried, held)) => {
                                    listings = held;
                                    carried
                                }
                                // Not recorded as a move, or its sidecar
                                // would be left at the old path for good:
                                // the next pass finds the move again and
                                // carries what is still to carry.
                                Err(e) => {
                                    report.errors.push((path.clone(), e.to_string()));
                                    continue;
                                }
                            }
                        }
                        None => carry_sidecar(&old, path, &mut listings),
                    };
                    if carried {
                        report.carried += 1;
                    }
                }
                moved = found;
                Plan::Fresh { hash, exif }
            }
        };
        // The sidecar as it is now: read ahead, unless one was carried
        // here since.
        let (sidecar, summary) = match early.take().filter(|_| !carried) {
            Some(a) => (a.sidecar, a.summary),
            None => {
                let read = move |p: &Path| {
                    let sidecar = sidecar_of(p, looks);
                    let summary = summary_of(sidecar.as_ref(), p);
                    (sidecar, summary)
                };
                match far {
                    Some(f) => {
                        let p = path.clone();
                        match f.one(move || read(&p)) {
                            Ok(got) => got,
                            Err(e) => {
                                report.errors.push((path.clone(), e.to_string()));
                                continue;
                            }
                        }
                    }
                    None => read(path),
                }
            }
        };
        // A raw with no edit of ours (no `.gcd`, or one with meta alone)
        // and a Lightroom develop beside it: taken as an open takes it,
        // and written, after any carry above, so the row says what an
        // open will find and the next pass finds an edit and takes
        // nothing. An XMP the row has looked at already
        // (its hash, [`LOOKED`]) is not looked at again until it
        // changes.
        let looked_at = match &plan {
            Plan::Same(row, ..) => row.same_sidecar(sidecar.as_ref()),
            _ => false,
        };
        let (sidecar, summary) = if let Some(placement) = takes
            && !looked_at
            && !summary.edited
            && summary.versions.is_empty()
        {
            let take = move |p: &Path| {
                disk_call();
                if !greycard_edit_io::lightroom::beside::waits(p) {
                    return None;
                }
                // The setting for new edits does not matter here: a
                // Lightroom take starts its line at the schema's
                // default (`on_disk::starting_of`) whatever the fresh
                // sidecar held, as the window's own take does.
                greycard_edit_io::on_disk::load_sidecar_said(
                    p,
                    Some(placement),
                    greycard_edit_io::on_disk::Fresh::default(),
                    greycard_edit_io::on_disk::frame_shape,
                );
                let sidecar = sidecar_of(p, true);
                let summary = summary_of(sidecar.as_ref(), p);
                Some((sidecar, summary))
            };
            let taken = match far {
                Some(f) => {
                    let p = path.clone();
                    f.one(move || take(&p)).ok().flatten()
                }
                None => take(path),
            };
            taken.unwrap_or((sidecar, summary))
        } else {
            (sidecar, summary)
        };
        looked.push(Looked {
            path,
            key,
            size,
            mtime,
            plan,
            sidecar,
            summary,
            moved,
        });
        if looked.len() >= BATCH || since.elapsed() >= BATCH_TIME {
            write_batch(
                conn,
                &folder_bytes,
                &folder_text,
                &mut looked,
                &mut existing,
                &mut report,
            )?;
            since = Instant::now();
            listings = Listings::default();
            if stop() {
                report.stopped = true;
                return Ok(report);
            }
        }
    }
    write_batch(
        conn,
        &folder_bytes,
        &folder_text,
        &mut looked,
        &mut existing,
        &mut report,
    )?;
    // What was not on disk. By id and path both: another writer may
    // have moved the row on since the folder's rows were read. A hidden
    // file's row, from before the listing passed over them, was never
    // a frame: it goes rather than waits under missing.
    let hidden = |key: &[u8]| greycard_core::decode::is_hidden(&path_from_bytes(key));
    if existing.iter().any(|(k, r)| !r.missing || hidden(k)) {
        let tx = begin(conn)?;
        let now = now_secs();
        for (key, row) in &existing {
            if hidden(key) {
                tx.prepare_cached("DELETE FROM files WHERE id = ? AND path = ?")?
                    .execute(params![row.id, key])?;
                tx.prepare_cached("DELETE FROM pending WHERE frame = ?")?
                    .execute(params![key])?;
                continue;
            }
            if row.missing {
                continue;
            }
            report.missing += tx
                .prepare_cached(
                    "UPDATE files SET missing_since = ? WHERE id = ? AND path = ? \
                     AND missing_since IS NULL",
                )?
                .execute(params![now, row.id, key])?;
        }
        tx.commit()?;
    }
    Ok(report)
}

/// The second phase: the batch's rows, in one write transaction.
fn write_batch(
    conn: &mut Connection,
    folder_bytes: &[u8],
    folder_text: &str,
    looked: &mut Vec<Looked<'_>>,
    existing: &mut HashMap<Vec<u8>, Row>,
    report: &mut Report,
) -> Result<()> {
    if looked.is_empty() {
        return Ok(());
    }
    let tx = begin(conn)?;
    #[cfg(test)]
    UNDER_LOCK.with(|u| u.set(true));
    let wrote = write_looked(&tx, folder_bytes, folder_text, looked, existing, report);
    #[cfg(test)]
    UNDER_LOCK.with(|u| u.set(false));
    wrote?;
    tx.commit()?;
    Ok(())
}

/// The second phase's writes, under the lock, from what the first phase
/// found. The disk is asked nothing here but a probe, in two places, both
/// for a new file the first phase took for a move's other end and so did
/// not probe (a move keeps its EXIF), when another writer changed things
/// between the phases: a row it added at this path for a file of another
/// size or time, or the move's row it took (`still_gone` fails), leaving
/// the file a new one. Any other disk call under the lock fails the
/// tests (`disk_call`).
fn write_looked(
    tx: &Transaction<'_>,
    folder_bytes: &[u8],
    folder_text: &str,
    looked: &mut Vec<Looked<'_>>,
    existing: &mut HashMap<Vec<u8>, Row>,
    report: &mut Report,
) -> Result<()> {
    for Looked {
        path,
        key,
        size,
        mtime,
        plan,
        sidecar,
        summary,
        moved,
    } in looked.drain(..)
    {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let seen = Seen {
            sidecar: sidecar.as_ref(),
            summary: &summary,
        };
        match plan {
            Plan::Same(row, style, mosaic) => {
                let snapshot = row.clone();
                let row = current(tx, &key, row)?;
                let mut reframed = false;
                if let Some(style) = style.filter(|_| !row.style_read) {
                    reframed = write_style(tx, row.id, &style)?;
                    report.styled += 1;
                }
                if let Some(mosaic) = mosaic.filter(|_| !row.mosaic_read) {
                    tx.prepare_cached("UPDATE files SET mosaic = ? WHERE id = ?")?
                        .execute(params![FileMosaic::text(mosaic), row.id])?;
                }
                // A camera's frame read anew changes the key of an edit
                // with no crop: the row's keys are written again.
                if reframed && seen.sidecar.is_some() && !wrote_since(&snapshot, &row) {
                    write_meta(tx, row.id, seen)?;
                }
                settle_same(tx, &snapshot, row, seen, report)?;
            }
            Plan::Changed { row, hash, exif } => {
                let snapshot = row.clone();
                let row = current(tx, &key, row)?;
                let reframed = camera_frame_now(tx, row.id)? != exif.style.camera_frame_text();
                update_file(tx, row.id, size, mtime, &hash, &exif)?;
                if !wrote_since(&snapshot, &row) && (reframed || !row.same_sidecar(seen.sidecar)) {
                    write_meta(tx, row.id, seen)?;
                }
                rehashed(report, path, &row.hash, &hash);
                report.changed += 1;
                report.changed_files.push(path.to_path_buf());
            }
            Plan::Fresh { hash, exif } => {
                if let Some(row) = row_at(tx, &key)? {
                    // Added by another writer since the folder's rows
                    // were read: its row is current, its meta included,
                    // as newer than the sidecar the first phase read.
                    if row.size == size && row.mtime == mtime {
                        if row.missing {
                            tx.prepare_cached(
                                "UPDATE files SET missing_since = NULL WHERE id = ?",
                            )?
                            .execute(params![row.id])?;
                            report.returned += 1;
                        }
                        report.unchanged += 1;
                    } else {
                        let exif = exif.unwrap_or_else(|| probe(path, report));
                        update_file(tx, row.id, size, mtime, &hash, &exif)?;
                        rehashed(report, path, &row.hash, &hash);
                        report.changed += 1;
                        report.changed_files.push(path.to_path_buf());
                    }
                } else if let Some((id, old_path)) =
                    moved.filter(|(id, old)| still_gone(tx, *id, old, &hash).unwrap_or(false))
                {
                    tx.prepare_cached(
                        "UPDATE files SET path = ?, folder = ?, folder_text = ?, name = ?, \
                         size = ?, mtime = ?, missing_since = NULL WHERE id = ?",
                    )?
                    .execute(params![
                        key,
                        folder_bytes,
                        folder_text,
                        name,
                        size as i64,
                        mtime,
                        id
                    ])?;
                    write_meta(tx, id, seen)?;
                    // A rename within the folder: the old row is this
                    // one, and is not to be marked missing.
                    existing.remove(&old_path);
                    log::info!(
                        "{}: moved from {}",
                        path.display(),
                        path_from_bytes(&old_path).display()
                    );
                    report.moved += 1;
                    report
                        .moved_files
                        .push((path_from_bytes(&old_path), path_from_bytes(&key)));
                } else {
                    let exif = exif.unwrap_or_else(|| probe(path, report));
                    tx.prepare_cached(
                        "INSERT INTO files (path, folder, folder_text, name, size, mtime, hash, \
                         make, model, camera, lens, iso, focal, aperture, shutter, taken, \
                         maker, style, style_fixed, peripheral, style_read, mosaic, \
                         camera_frame) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
                    )?
                    .execute(params![
                        key,
                        folder_bytes,
                        folder_text,
                        name,
                        size as i64,
                        mtime,
                        hash,
                        exif.make,
                        exif.model,
                        exif.camera,
                        exif.lens,
                        exif.iso,
                        exif.focal,
                        exif.aperture,
                        exif.shutter,
                        exif.taken,
                        exif.style.maker,
                        exif.style.style,
                        exif.style.fixed,
                        exif.style.peripheral,
                        FileMosaic::text(exif.mosaic),
                        exif.style.camera_frame_text(),
                    ])?;
                    let id = tx.last_insert_rowid();
                    write_meta(tx, id, seen)?;
                    report.added += 1;
                }
            }
        }
    }
    Ok(())
}

/// A changed file's hash, said in the report when it is not the one the
/// row had.
fn rehashed(report: &mut Report, path: &Path, old: &str, new: &str) {
    if old != new {
        report.rehashed.push(Rehash {
            path: path.to_path_buf(),
            old: old.to_string(),
            new: new.to_string(),
        });
    }
}

/// The sidecar as the first phase read it, and what the row mirrors of
/// it.
#[derive(Clone, Copy)]
struct Seen<'a> {
    sidecar: Option<&'a SidecarNow>,
    summary: &'a Summary,
}

/// Whether another writer (a save's `index_file`) wrote the row's meta
/// since the folder's rows were read: its sidecar is not the snapshot's.
/// Its meta is newer than what the first phase read, and stays.
fn wrote_since(snapshot: &Row, now: &Row) -> bool {
    snapshot.sidecar != now.sidecar || snapshot.sidecar_hash != now.sidecar_hash
}

/// Whether the row the first phase took for a move's other end is still
/// that: the same hash, at the old path, and not taken by another
/// writer since. Asks the index alone.
fn still_gone(tx: &Transaction<'_>, id: i64, old: &[u8], hash: &str) -> Result<bool> {
    use rusqlite::OptionalExtension;
    Ok(tx
        .prepare_cached("SELECT 1 FROM files WHERE id = ? AND path = ? AND hash = ?")?
        .query_row(params![id, old, hash], |_| Ok(()))
        .optional()?
        .is_some())
}

/// A row whose file is as it was: back if it was missing, and its
/// meta refreshed if its sidecar is not the one it knew.
fn settle_same(
    tx: &Transaction<'_>,
    snapshot: &Row,
    row: Row,
    seen: Seen<'_>,
    report: &mut Report,
) -> Result<()> {
    if row.missing {
        tx.prepare_cached("UPDATE files SET missing_since = NULL WHERE id = ?")?
            .execute(params![row.id])?;
        report.returned += 1;
    }
    if wrote_since(snapshot, &row) || row.same_sidecar(seen.sidecar) {
        report.unchanged += 1;
    } else {
        write_meta(tx, row.id, seen)?;
        report.meta_refreshed += 1;
    }
    Ok(())
}

/// A changed file's row: its new size, mtime, hash and EXIF. The whole
/// file's hash a backup took is the old file's, and goes.
fn update_file(
    tx: &Transaction<'_>,
    id: i64,
    size: u64,
    mtime: i64,
    hash: &str,
    exif: &Exif,
) -> Result<()> {
    tx.prepare_cached(
        "UPDATE files SET size = ?, mtime = ?, hash = ?, make = ?, model = ?, \
         camera = ?, lens = ?, iso = ?, focal = ?, aperture = ?, shutter = ?, \
         taken = ?, maker = ?, style = ?, style_fixed = ?, peripheral = ?, style_read = 1, \
         camera_frame = ?, mosaic = ?, focus = NULL, missing_since = NULL, whole_hash = NULL \
         WHERE id = ?",
    )?
    .execute(params![
        size as i64,
        mtime,
        hash,
        exif.make,
        exif.model,
        exif.camera,
        exif.lens,
        exif.iso,
        exif.focal,
        exif.aperture,
        exif.shutter,
        exif.taken,
        exif.style.maker,
        exif.style.style,
        exif.style.fixed,
        exif.style.peripheral,
        exif.style.camera_frame_text(),
        FileMosaic::text(exif.mosaic),
        id
    ])?;
    Ok(())
}

/// The row's camera frame as its column holds it.
fn camera_frame_now(tx: &Transaction<'_>, id: i64) -> Result<Option<String>> {
    Ok(tx
        .prepare_cached("SELECT camera_frame FROM files WHERE id = ?")?
        .query_row(params![id], |r| r.get(0))?)
}

/// The maker's tags into a row that had not had them read; whether the
/// camera's frame is another than the row held.
fn write_style(tx: &Transaction<'_>, id: i64, style: &StyleTags) -> Result<bool> {
    let reframed = camera_frame_now(tx, id)? != style.camera_frame_text();
    tx.prepare_cached(
        "UPDATE files SET maker = ?, style = ?, style_fixed = ?, peripheral = ?, \
         camera_frame = ?, style_read = 1 WHERE id = ?",
    )?
    .execute(params![
        style.maker,
        style.style,
        style.fixed,
        style.peripheral,
        style.camera_frame_text(),
        id
    ])?;
    Ok(reframed)
}

/// What one pass knows about the disk, so that a question is asked
/// of it once: the paths listed in the folder being indexed, which
/// are on disk without a stat, and whether each folder asked about
/// is there with something in it. A pass over one file
/// (`index_file`) has no listing — its one path would say every
/// other file in the folder is gone, and a copy would take the
/// original's row — and stats instead.
struct Disk<'a> {
    folder: &'a [u8],
    listed: Option<HashSet<Vec<u8>>>,
    in_use: HashMap<Vec<u8>, bool>,
    /// A network mount's reads.
    far: Option<&'a Far>,
    /// A network mount's read that did not answer, while the file in
    /// hand was looked for as a move: the file is left to the next pass.
    unanswered: Option<String>,
}

impl Disk<'_> {
    /// Whether a row's file is on disk: for a row in the folder being
    /// indexed whole, by the listing; for any other, by a stat.
    fn has(&mut self, row_folder: &[u8], row_path: &[u8]) -> bool {
        match &self.listed {
            Some(listed) if row_folder == self.folder => listed.contains(row_path),
            _ => {
                disk_call();
                let path = path_from_bytes(row_path);
                let Some(f) = self.far else {
                    return path.exists();
                };
                match f.one(move || path.exists()) {
                    Ok(there) => there,
                    // Taken for there, which claims no move; the caller
                    // drops the file for this pass.
                    Err(e) => {
                        self.unanswered.get_or_insert(e.to_string());
                        true
                    }
                }
            }
        }
    }

    /// Whether a folder is there with something in it, asked of the
    /// disk once a folder a pass.
    fn folder_in_use(&mut self, folder: &[u8]) -> bool {
        if let Some(&known) = self.in_use.get(folder) {
            return known;
        }
        disk_call();
        let dir = path_from_bytes(folder);
        let look = move || dir.is_dir() && !is_empty_dir(&dir).unwrap_or(true);
        let in_use = match self.far {
            Some(f) => match f.one(look) {
                Ok(in_use) => in_use,
                // Not kept for the pass: the caller drops the file, and
                // the next file asks again.
                Err(e) => {
                    self.unanswered.get_or_insert(e.to_string());
                    return false;
                }
            },
            None => look(),
        };
        self.in_use.insert(folder.to_vec(), in_use);
        in_use
    }
}

/// Files by name and content hash: what the two halves of a rename
/// are matched by.
type Files = HashSet<(OsString, String)>;

/// For each folder found renamed, old and new, the rows still missing
/// in the old one taken to the new one at their names, where no row
/// is: the files that were away when the folder was renamed. A folder
/// gone keeps its rows, since an unmounted drive looks the same, but
/// a rename matched by its files is not that, and a file of it found
/// later in the new folder, under any name, is then a move's other
/// end: its row kept, its sidecar carried, and the move said, so a
/// stack or a collection that holds it follows. A row whose name the
/// new folder has a row at stays where it is, missing. How many went.
fn follow_missing(conn: &mut Connection, pairs: &[(PathBuf, PathBuf)]) -> Result<usize> {
    use rusqlite::OptionalExtension;
    if pairs.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut n = 0;
    for (from, to) in pairs {
        let rows = tx
            .prepare_cached(
                "SELECT id, path FROM files WHERE folder = ? AND missing_since IS NOT NULL",
            )?
            .query_map(params![path_bytes(from)], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, path) in rows {
            let Some(name) = name_of(&path) else {
                continue;
            };
            let key = path_bytes(&to.join(&name));
            let taken = tx
                .prepare_cached("SELECT 1 FROM files WHERE path = ?")?
                .query_row(params![key], |_| Ok(()))
                .optional()?
                .is_some();
            if taken {
                continue;
            }
            tx.prepare_cached(
                "UPDATE files SET path = ?, folder = ?, folder_text = ?, name = ? WHERE id = ?",
            )?
            .execute(params![
                key,
                path_bytes(to),
                path_text(to),
                name.to_string_lossy(),
                id
            ])?;
            n += 1;
        }
    }
    tx.commit()?;
    if n > 0 {
        log::info!("{n} files away from a renamed folder now wait in its new place");
    }
    Ok(n)
}

fn name_of(path: &[u8]) -> Option<OsString> {
    path_from_bytes(path).file_name().map(OsString::from)
}

/// The live rows of `folder` by name and hash, those of paths in
/// `except` left out.
fn live_files(
    conn: &Connection,
    folder: &[u8],
    except: Option<&HashSet<Vec<u8>>>,
) -> Result<Files> {
    let mut stmt = conn.prepare_cached(
        "SELECT path, hash FROM files WHERE folder = ? AND missing_since IS NULL",
    )?;
    let rows = stmt.query_map(params![folder], |r| {
        Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = Files::new();
    for row in rows {
        let (path, hash) = row?;
        if except.is_some_and(|e| e.contains(&path)) {
            continue;
        }
        if let Some(name) = name_of(&path) {
            out.insert((name, hash));
        }
    }
    Ok(out)
}

/// How long one half of a rename waits for the other: the watcher
/// turns a rename into two passes, the old folder's and the new one's,
/// in either order and not always at once.
const RENAME_WAIT: Duration = Duration::from_secs(600);

/// The most halves of each kind kept waiting; the oldest go first.
const RENAME_MOST: usize = 64;

/// One half of a rename: a folder and its files.
#[derive(Debug)]
struct Half {
    folder: PathBuf,
    files: Files,
    at: Instant,
}

/// Folders lately gone and folders whose rows lately came live (added,
/// or back from missing), each waiting a few minutes for the other
/// half of a rename. A folder gone is renamed to the one come live
/// sharing more than half of each one's files by name and hash, the
/// best such alone. A tie is no rename, and the halves it weighed
/// against the asking one are dropped: a copy made twice is not later
/// taken for the one renamed. Each half is matched once.
#[derive(Debug, Default)]
pub(crate) struct Renames {
    gone: Vec<Half>,
    live: Vec<Half>,
}

impl Renames {
    /// Folders `gone`, each with its files, gone in one pass: the
    /// renames they complete, old folder and new. None of them is a
    /// folder another was renamed to, whatever order they come in;
    /// they are matched in the order of their paths.
    pub(crate) fn gone(
        &mut self,
        mut gone: Vec<(PathBuf, Files)>,
        now: Instant,
    ) -> Vec<(PathBuf, PathBuf)> {
        gone.retain(|(_, files)| !files.is_empty());
        if gone.is_empty() {
            return Vec::new();
        }
        self.prune(now);
        self.live
            .retain(|h| !gone.iter().any(|(f, _)| *f == h.folder));
        gone.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out = Vec::new();
        for (folder, files) in gone {
            let at = add(&mut self.gone, folder, files, now);
            let g = &self.gone[at];
            match best(&self.live, |l| renamed(g, l)) {
                Best::One(to) => {
                    let (g, l) = (self.gone.remove(at), self.live.remove(to));
                    out.push(said(g, l));
                }
                Best::Tie(tied) => drop_all(&mut self.live, &tied),
                Best::None => {}
            }
        }
        out
    }

    /// Files `files` come live in folder `folder`: the rename it
    /// completes, or none and it waits.
    pub(crate) fn live(
        &mut self,
        folder: PathBuf,
        files: Files,
        now: Instant,
    ) -> Option<(PathBuf, PathBuf)> {
        if files.is_empty() {
            return None;
        }
        self.prune(now);
        // Back, it is no folder gone.
        self.gone.retain(|h| h.folder != folder);
        let at = add(&mut self.live, folder, files, now);
        let l = &self.live[at];
        match best(&self.gone, |g| renamed(g, l)) {
            Best::One(from) => {
                let (g, l) = (self.gone.remove(from), self.live.remove(at));
                Some(said(g, l))
            }
            Best::Tie(tied) => {
                drop_all(&mut self.gone, &tied);
                None
            }
            Best::None => None,
        }
    }

    fn prune(&mut self, now: Instant) {
        for halves in [&mut self.gone, &mut self.live] {
            halves.retain(|h| now.saturating_duration_since(h.at) < RENAME_WAIT);
        }
    }
}

/// `files` added to `folder`'s half, or a half of its own, the oldest
/// dropped past [`RENAME_MOST`]: its place.
fn add(halves: &mut Vec<Half>, folder: PathBuf, files: Files, now: Instant) -> usize {
    if let Some(i) = halves.iter().position(|h| h.folder == folder) {
        halves[i].files.extend(files);
        halves[i].at = now;
        return i;
    }
    if halves.len() >= RENAME_MOST {
        halves.remove(0);
    }
    halves.push(Half {
        folder,
        files,
        at: now,
    });
    halves.len() - 1
}

/// The halves at `at` taken out of `halves`.
fn drop_all(halves: &mut Vec<Half>, at: &[usize]) {
    let mut i = 0;
    halves.retain(|_| {
        i += 1;
        !at.contains(&(i - 1))
    });
}

/// How many files folder `gone` and folder `live` share, when it is
/// more than half of each one's: a rename of the one to the other.
fn renamed(gone: &Half, live: &Half) -> Option<usize> {
    if gone.folder == live.folder {
        return None;
    }
    let n = gone
        .files
        .iter()
        .filter(|f| live.files.contains(*f))
        .count();
    (2 * n > gone.files.len() && 2 * n > live.files.len()).then_some(n)
}

/// What [`best`] found.
enum Best {
    One(usize),
    /// Two or more at the highest score, by their places.
    Tie(Vec<usize>),
    None,
}

/// The half of `among` with the highest `score`.
fn best(among: &[Half], score: impl Fn(&Half) -> Option<usize>) -> Best {
    let mut top = 0;
    let mut at: Vec<usize> = Vec::new();
    for (i, h) in among.iter().enumerate() {
        let Some(n) = score(h) else {
            continue;
        };
        if n > top {
            (top, at) = (n, vec![i]);
        } else if n == top {
            at.push(i);
        }
    }
    match at.len() {
        0 => Best::None,
        1 => Best::One(at[0]),
        _ => Best::Tie(at),
    }
}

fn said(gone: Half, live: Half) -> (PathBuf, PathBuf) {
    log::info!(
        "{}: renamed to {}",
        gone.folder.display(),
        live.folder.display()
    );
    (gone.folder, live.folder)
}

/// A row with this hash whose file is gone from a folder that is
/// still there with other things in it: a move's other end. Its id
/// and its old path. A row whose folder is gone, or is there but
/// empty, is not one — its drive may simply not be mounted, its
/// mount point left behind — and the file in hand is a copy. A
/// folder of four thousand links to twenty raws asks this of two
/// hundred rows a file, so the cheap question comes first and the
/// folder's answer is kept for the pass.
fn gone_by_hash(
    tx: &Connection,
    hash: &str,
    disk: &mut Disk<'_>,
) -> Result<Option<(i64, Vec<u8>)>> {
    let mut stmt =
        tx.prepare_cached("SELECT id, path, folder FROM files WHERE hash = ? ORDER BY id")?;
    let candidates = stmt.query_map(params![hash], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Vec<u8>>(1)?,
            r.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    for candidate in candidates {
        let (id, old, old_folder) = candidate?;
        if disk.has(&old_folder, &old) {
            continue;
        }
        if disk.folder_in_use(&old_folder) {
            return Ok(Some((id, old)));
        }
    }
    Ok(None)
}

/// The row as it is now under the lock, rather than as the folder's
/// rows were read before the pass: another writer — the editor
/// saving a rating and calling `index_file` between the phases — may
/// have written it since, and the row keeps the meta it wrote, which
/// is newer than the sidecar the first phase read. A row gone meanwhile (a prune)
/// is settled as the snapshot, and its updates by id touch nothing.
fn current(tx: &Transaction<'_>, key: &[u8], snapshot: Row) -> Result<Row> {
    Ok(row_at(tx, key)?.unwrap_or(snapshot))
}

/// The file's EXIF, a raw's through its decoder and a picture's
/// from its chunk; a file that will not say is an empty EXIF and a
/// line in the report. A decoder that panics on the file's data is
/// the same case, caught here so one file never ends the folder.
fn probe(path: &Path, report: &mut Report) -> Exif {
    #[cfg(test)]
    let was = IN_PROBE.with(|p| p.replace(true));
    let exif = probe_now(path, report);
    #[cfg(test)]
    IN_PROBE.with(|p| p.set(was));
    exif
}

fn probe_now(path: &Path, report: &mut Report) -> Exif {
    disk_call();
    let probed = std::panic::catch_unwind(|| {
        if greycard_core::picture::is_picture_path(path) {
            greycard_core::picture::probe_path(path)
                .map(|p| (p, Some(FileMosaic::NotMosaic), StyleTags::default()))
        } else {
            // The kind of mosaic on the same decoder as the EXIF: the
            // image's description, no samples. A description that will
            // not read costs the mosaic alone (stored unknown), and the
            // maker's tags that will not read the style alone; the
            // EXIF stands. All three from the pieces of the file they
            // are in.
            greycard_core::decode::index_read_path(path).map(|(p, m, style, frame)| {
                use greycard_core::decode::MosaicRead;
                let mosaic = match m {
                    MosaicRead::Mosaic(m) => Some(FileMosaic::Mosaic(m)),
                    MosaicRead::NotMosaic => Some(FileMosaic::NotMosaic),
                    MosaicRead::Unknown => None,
                };
                // A new row with tags that will not read has none, and
                // the frame if that read.
                let style = style_tags(path, style, frame).unwrap_or(StyleTags {
                    camera_frame: frame,
                    ..StyleTags::default()
                });
                (p, mosaic, style)
            })
        }
    });
    let failed = match probed {
        Ok(Ok((p, mosaic, style))) => {
            return Exif {
                style,
                mosaic,
                ..Exif::from_probe(&p)
            };
        }
        Ok(Err(e)) => e.to_string(),
        Err(panic) => {
            let what = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "the decoder gave up".to_string());
            format!("the decoder panicked: {what}")
        }
    };
    log::warn!("{}: {failed}", path.display());
    report.errors.push((path.to_path_buf(), failed));
    Exif::default()
}

/// A raw's maker tags, as the index keeps them, for a row whose tags
/// are not read; the default for a picture, which has no camera JPEG to
/// be fitted against. `None` for a file the reader cannot read: not an
/// error in the report, and nothing written, so the row keeps what it
/// held and stays unread, and the next pass over its folder reads its
/// head again. The log says so once a session for each file
/// ([`tags_unread`]), and at debug after.
fn read_style(path: &Path) -> Option<StyleTags> {
    disk_call();
    if !greycard_core::decode::is_raw_path(path) {
        return Some(StyleTags::default());
    }
    let frame = match greycard_core::decode::camera_frame_path(path) {
        Ok(frame) => frame,
        Err(e) => {
            tags_unread(path, &format!("the camera's frame: {e}"));
            return None;
        }
    };
    style_tags(path, greycard_core::decode::CameraStyle::read(path), frame)
}

/// A file's maker's tags would not read: a warning the first time this
/// session, and at debug after, since a row left unread is read again
/// at every pass over its folder.
fn tags_unread(path: &Path, why: &str) {
    static SAID: std::sync::LazyLock<std::sync::Mutex<HashSet<PathBuf>>> =
        std::sync::LazyLock::new(Default::default);
    let first = SAID
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(path.to_path_buf());
    if first {
        log::warn!("{}: {why}; read again at the next pass", path.display());
    } else {
        log::debug!("{}: {why}", path.display());
    }
}

/// A raw's maker's tags as the index keeps them, the camera's frame
/// with them; `None` for a reader's failure, a line in the log: a row
/// read again keeps what it held, unread.
fn style_tags(
    path: &Path,
    read: greycard_core::Result<Option<greycard_core::decode::CameraStyle>>,
    frame: Option<greycard_core::raw::CameraFrame>,
) -> Option<StyleTags> {
    match read {
        Ok(Some(style)) => Some(StyleTags::from_style(&style, frame)),
        Ok(None) => Some(StyleTags {
            camera_frame: frame,
            ..StyleTags::default()
        }),
        Err(e) => {
            tags_unread(path, &format!("the maker's tags: {e}"));
            None
        }
    }
}

/// A file's kind of mosaic alone, for a row from before the index kept
/// it (schema 10): a picture is none; a raw's is its image's
/// description, read without a sample; `None`, said in the log, for
/// one that would not say. Not an error in the report, as the style's
/// is not.
fn read_mosaic(path: &Path) -> Option<FileMosaic> {
    disk_call();
    if greycard_core::picture::is_picture_path(path) {
        return Some(FileMosaic::NotMosaic);
    }
    match std::panic::catch_unwind(|| greycard_core::decode::mosaic_path(path)) {
        Ok(Ok(m)) => Some(m.map_or(FileMosaic::NotMosaic, FileMosaic::Mosaic)),
        Ok(Err(e)) => {
            log::warn!("{}: its mosaic: {e}", path.display());
            None
        }
        Err(_) => {
            log::warn!("{}: its mosaic: the decoder panicked", path.display());
            None
        }
    }
}

/// The sidecar's meta section and nothing else of it: the edit and
/// its history are not read, and a sidecar whose edit this build
/// cannot read still gives up its stars.
pub fn read_meta(sidecar: &Path) -> Meta {
    match std::fs::read(sidecar) {
        Ok(json) => meta_from_json(&json, sidecar),
        Err(e) => {
            log::warn!("{}: {e}", sidecar.display());
            Meta::default()
        }
    }
}

fn meta_from_json(json: &[u8], sidecar: &Path) -> Meta {
    parsed(Some(json), sidecar).meta
}

/// What a row mirrors of a sidecar: its meta, whether it holds a
/// develop, and the quarter turns its picture is shown at.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Summary {
    pub(crate) meta: Meta,
    pub(crate) edited: bool,
    /// Packed as [`crate::pack_turns`] packs them.
    pub(crate) turns: u8,
    /// The sidecar records an export.
    pub(crate) exported: bool,
    /// The edit's learned blend when the sidecar has an edit to read it
    /// from, judged against the frame's ISO when the row is written
    /// (`edited` leaves it out).
    pub(crate) blend: Option<f32>,
    /// The develop key of the edit, when there is an edit to read it
    /// from, made with the lens database as named
    /// ([`crate::lens_database`]); 0 for a develop whose edit could
    /// not be read, so the row is not read again for it every pass.
    pub(crate) develop_key: Option<u64>,
    /// The edit stores no crop, so the camera's frame, where the row
    /// has one, frames it and is taken into the key at the write
    /// (`greycard_edit::key::framed_key`).
    pub(crate) uncropped: bool,
    /// The edit's aspect is a ratio: over no crop and a centered camera
    /// frame it frames as with none, and its key stays the edit's own.
    pub(crate) by_ratio: bool,
    /// The sidecar's versions beside the file's own, in its order: what
    /// the `versions` table mirrors of each.
    pub(crate) versions: Vec<VersionSummary>,
}

/// What a row of the `versions` table mirrors of one version in a
/// sidecar: its id, name and time made, and its own marks and key,
/// read under the rules version 0's are.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct VersionSummary {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) made: u64,
    pub(crate) edited: bool,
    pub(crate) exported: bool,
    /// The version's learned blend, judged at the write as version 0's.
    pub(crate) blend: Option<f32>,
    pub(crate) develop_key: Option<u64>,
    /// As [`Summary::uncropped`], of the version's edit.
    pub(crate) uncropped: bool,
    /// As [`Summary::by_ratio`], of the version's edit.
    pub(crate) by_ratio: bool,
}

/// The `.gcd`'s JSON read for what the row mirrors of it, before the
/// XMP beside the frame has its say.
#[derive(Default)]
struct Parsed {
    meta: Meta,
    turn: u8,
    /// What the sidecar last took from an XMP, so the same XMP is not
    /// taken again.
    mark: Option<greycard_edit_io::xmp::Adopted>,
    /// The same for a culling tool's `IMG.xmp` shared with a JPEG.
    shared_mark: Option<greycard_edit_io::xmp::Culled>,
    /// The edit, brought up to this build's shape; none when there is
    /// no `current` in the file.
    current: Option<greycard_edit::Edit>,
    /// A snapshot is kept. History alone is not a develop: a reset
    /// leaves its steps behind.
    stepped: bool,
    /// An export is recorded on the current state or on a step of the
    /// history.
    exported: bool,
    /// The file, or its edit, could not be read: there is something
    /// in it, whatever it is.
    unreadable: bool,
    /// Each version's line, read as version 0's is.
    versions: Vec<ParsedVersion>,
}

/// One version of a sidecar as [`parsed`] reads it.
struct ParsedVersion {
    id: String,
    name: String,
    made: u64,
    current: Option<greycard_edit::Edit>,
    stepped: bool,
    exported: bool,
    unreadable: bool,
}

/// A line's `current` brought up to this build's shape: none when the
/// line has none, and an edit that will not read said in the log.
fn line_edit(
    line: &serde_json::Value,
    sidecar: &Path,
    which: &str,
) -> (Option<greycard_edit::Edit>, bool) {
    match line.get("current").cloned().map(|c| {
        greycard_edit::migrate(c)
            .and_then(|c| Ok(serde_json::from_value::<greycard_edit::Edit>(c)?))
    }) {
        None => (None, false),
        Some(Ok(edit)) => (Some(edit), false),
        Some(Err(e)) => {
            log::warn!("{}: {which} was not read: {e}", sidecar.display());
            (None, true)
        }
    }
}

/// The sidecar's versions, loosely as the edit crate reads them: a
/// `versions` that is not a list is none, and an entry with no id, or
/// an id already read, is left out.
fn parsed_versions(value: &serde_json::Value, sidecar: &Path, edits: bool) -> Vec<ParsedVersion> {
    let Some(items) = value.get("versions").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<ParsedVersion> = Vec::with_capacity(items.len());
    for item in items {
        let Some(id) = item
            .get("id")
            .and_then(|i| i.as_str())
            .filter(|i| !i.trim().is_empty())
        else {
            continue;
        };
        if out.iter().any(|v| v.id == id) {
            continue;
        }
        let line = item.get("line").unwrap_or(&serde_json::Value::Null);
        let (current, unreadable) = if edits {
            line_edit(line, sidecar, &format!("version {id}'s edit"))
        } else {
            (has_edit(line), false)
        };
        out.push(ParsedVersion {
            id: id.to_string(),
            name: item
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default()
                .to_string(),
            made: item.get("made").and_then(|m| m.as_u64()).unwrap_or(0),
            current,
            stepped: line
                .get("snapshots")
                .and_then(|v| v.as_array())
                .is_some_and(|a| !a.is_empty()),
            exported: records_export(line),
            unreadable,
        });
    }
    out
}

/// The meta is read as `meta` alone, loosely, so a sidecar whose edit
/// this build cannot read still gives up its stars. The edit is read
/// for two things only: whether it is a develop at all, and which way
/// up it shows the picture. `current` is brought up to this build's
/// shape and parsed for that; the history and the snapshots are not
/// parsed, only counted.
fn parsed(json: Option<&[u8]>, sidecar: &Path) -> Parsed {
    let Some(json) = json else {
        return Parsed::default();
    };
    match serde_json::from_slice(json) {
        Ok(value) => parsed_value(&value, sidecar, true),
        Err(e) => {
            log::warn!("{}: {e}", sidecar.display());
            Parsed {
                unreadable: true,
                ..Parsed::default()
            }
        }
    }
}

/// A line's edit as [`parsed`] stands one in when the edits are left to
/// the full read ([`summary_of`]): an empty one where the line has a
/// `current`, for that read to fill in, and none where it has not.
fn has_edit(line: &serde_json::Value) -> Option<greycard_edit::Edit> {
    line.get("current").map(|_| greycard_edit::Edit::default())
}

/// [`parsed`] of the JSON `value`, each line's edit read with `edits`,
/// and stood in for otherwise ([`has_edit`]).
fn parsed_value(value: &serde_json::Value, sidecar: &Path, edits: bool) -> Parsed {
    let meta: Meta = value
        .get("meta")
        .cloned()
        .and_then(|m| serde_json::from_value(m).ok())
        .unwrap_or_default();
    let mark: Option<greycard_edit_io::xmp::Adopted> = value
        .get("xmp")
        .cloned()
        .and_then(|m| serde_json::from_value(m).ok());
    let shared_mark: Option<greycard_edit_io::xmp::Culled> = value
        .get("xmp_shared")
        .cloned()
        .and_then(|m| serde_json::from_value(m).ok());
    let count = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_array())
            .map_or(0, Vec::len)
    };
    let (current, unreadable) = if edits {
        line_edit(value, sidecar, "the edit")
    } else {
        (has_edit(value), false)
    };
    Parsed {
        meta,
        turn: value
            .get("turn")
            .and_then(|t| t.as_u64())
            .map_or(0, |t| (t % 4) as u8),
        mark,
        shared_mark,
        current,
        stepped: count("snapshots") > 0,
        exported: records_export(value),
        unreadable,
        versions: parsed_versions(value, sidecar, edits),
    }
}

/// Whether a sidecar's JSON records an export: the frame's
/// `last_export`, or, for a sidecar from before it, `exported` on the
/// current state or on a step of `history` (the redo stack is not
/// written).
fn records_export(value: &serde_json::Value) -> bool {
    let some =
        |v: Option<&serde_json::Value>| v.and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty());
    value.get("last_export").is_some_and(|v| v.is_object())
        || some(value.get("exported"))
        || value
            .get("history")
            .and_then(|v| v.as_array())
            .is_some_and(|steps| steps.iter().any(|s| some(s.get("exported"))))
}

/// What the row mirrors of what `raw` has beside it: the `.gcd`'s
/// meta and turn with the XMP's word taken over them exactly as the
/// editor takes it when it opens the frame (`xmp::adopt`: an XMP the
/// sidecar has not already taken from, by its mark, has its fields
/// laid over the meta and its orientation over the turn), whether the
/// sidecar holds a develop, and the quarter turns the picture is
/// shown at. `now` is `None` for a frame with nothing beside it.
///
/// A develop is a step in the history, a snapshot, or a current edit
/// that is not the frame's default (a picture's is not a raw's); a
/// `.gcd` this build cannot read counts as one, since there is
/// something in it. The XMP's orientation goes onto the frame's turn,
/// not into its edit, as it does in the editor: it moves the turns the
/// picture is shown at, and a frame only turned in another tool does
/// not count as developed.
fn summary_of(now: Option<&SidecarNow>, raw: &Path) -> Summary {
    let Some(now) = now else {
        return Summary::default();
    };
    let said = now
        .path
        .clone()
        .or_else(|| now.xmp.clone())
        .unwrap_or_else(|| raw.to_path_buf());
    // The edit as the editor reads it (§724): every state shown under
    // the file's turn from the turn it was placed under, by the shape
    // the file writes or the frame's own header, read unmapped and only
    // for a state that waits on it. The JSON is parsed
    // once: what the row counts is looked at in it, and the edit crate
    // reads the same value. A file whose edit this build cannot read is
    // read again loosely and summed up from its parts, the meta kept.
    let full = now
        .json
        .as_deref()
        .and_then(|j| serde_json::from_slice::<serde_json::Value>(j).ok())
        .and_then(|value| {
            let loose = parsed_value(&value, &said, false);
            // A line nobody developed holds no state and writes none
            // (§724): its edit is the frame's start, by what the file is,
            // and the row says of it what it says of a frame with no
            // sidecar. One a build before version 10 wrote untouched at
            // that start holds none from here, its blend kept.
            Some((
                loose,
                Sidecar::from_value(value, &Sidecar::start_of(raw)).ok()?,
            ))
        });
    let (p, mut sidecar) = match full {
        Some((mut p, mut read)) => {
            read.settle_shape(|turn| {
                disk_call();
                greycard_edit_io::on_disk::frame_shape(raw, turn)
            });
            if p.current.is_some() {
                p.current = Some(read.current().clone());
            }
            for v in &mut p.versions {
                if v.current.is_some() {
                    // A version the edit crate passed over has something
                    // in it this build does not read.
                    v.current = read.line(Some(&v.id)).map(|l| l.current().clone());
                    v.unreadable |= v.current.is_none();
                }
            }
            (p, read)
        }
        None => {
            let p = parsed(now.json.as_deref(), &said);
            let default = if greycard_core::picture::is_picture_path(raw) {
                greycard_edit::Edit::for_picture()
            } else {
                greycard_edit::Edit::default()
            };
            let sidecar = Sidecar {
                line: greycard_edit::Line::of(p.current.clone().unwrap_or(default), p.turn)
                    .expect("a fresh edit waits on no shape"),
                meta: p.meta.clone(),
                turn: p.turn,
                xmp: p.mark.clone(),
                xmp_shared: p.shared_mark.clone(),
                ..Sidecar::default()
            };
            (p, sidecar)
        }
    };
    if now.xmp.is_some() || now.shared.is_some() {
        // The camera's tag is asked for only when the packet says
        // something about the orientation, as the editor asks. What
        // the pass found beside the frame is the adoption's, so the
        // shared XMP is not read and parsed twice.
        let found = greycard_edit_io::xmp::Beside {
            own: now.xmp.clone(),
            shared: now.shared.clone(),
        };
        greycard_edit_io::xmp::adopt_beside(
            &found,
            &mut sidecar,
            || {
                disk_call();
                greycard_core::decode::orientation_path(raw)
                    .inspect_err(|e| log::warn!("{}: orientation: {e}", raw.display()))
                    .ok()
            },
            |turn| {
                disk_call();
                greycard_edit_io::on_disk::frame_shape(raw, turn)
            },
        );
    }
    // The blend is judged at the write, where the row's ISO is.
    let edited = p.stepped || p.unreadable || !crate::is_default_edit(sidecar.current(), raw, None);
    // The key is of any edit in the file, so a frame counted as edited
    // at the write (by its blend) has one too; a develop with no edit
    // to read gets the mark that says so.
    // Keyed with the headroom's version where the raw may decode with
    // one, and the DNG decode's for a DNG, as the
    // engine keys what it keeps (`decode_key`). A state whose masks wait
    // on the frame's shape is no picture to key.
    let develop_key = if p.current.is_some()
        && let Some(current) = sidecar.ready()
    {
        Some(greycard_edit::key::decode_key(
            greycard_edit_io::key::develop_key(current, crate::lens_database().as_deref()),
            raw,
        ))
    } else if edited {
        Some(0)
    } else {
        None
    };
    // Each version by the rules version 0 is read by.
    let lenses = crate::lens_database();
    let versions = p
        .versions
        .into_iter()
        .map(|mut v| {
            // Each version as the editor shows it, as version 0 is.
            // One whose masks wait on the frame's shape is no picture to
            // key, and something is in it.
            if v.current.is_some()
                && let Some(line) = sidecar.line(Some(&v.id))
            {
                v.current = line.ready().cloned();
                v.unreadable |= v.current.is_none();
            }
            let edited = v.stepped
                || v.unreadable
                || v.current
                    .as_ref()
                    .is_some_and(|c| !crate::is_default_edit(c, raw, None));
            VersionSummary {
                develop_key: match &v.current {
                    Some(c) => Some(greycard_edit::key::decode_key(
                        greycard_edit_io::key::develop_key(c, lenses.as_deref()),
                        raw,
                    )),
                    None => edited.then_some(0),
                },
                blend: v.current.as_ref().map(|c| c.noise.learned_strength),
                uncropped: v
                    .current
                    .as_ref()
                    .is_some_and(|c| c.geometry.crop.is_none()),
                by_ratio: v.current.as_ref().is_some_and(|c| {
                    matches!(
                        c.geometry.aspect,
                        greycard_edit::geometry::Aspect::Ratio { .. }
                    )
                }),
                id: v.id,
                name: v.name,
                made: v.made,
                edited,
                exported: v.exported,
            }
        })
        .collect();
    // Said of an edit read; the mark of one unread takes no frame.
    let uncropped = p.current.is_some() && sidecar.current().geometry.crop.is_none();
    let by_ratio = matches!(
        sidecar.current().geometry.aspect,
        greycard_edit::geometry::Aspect::Ratio { .. }
    );
    Summary {
        meta: sidecar.meta,
        edited,
        exported: p.exported,
        blend: p.current.as_ref().map(|c| c.noise.learned_strength),
        turns: crate::pack_turns(sidecar.line.current().geometry.shown_turns(sidecar.turn)),
        develop_key,
        uncropped,
        by_ratio,
        versions,
    }
}

/// [`summary_of`] over a `.gcd`'s bytes alone, with no XMP beside the
/// frame; `raw` says whose default the edit is measured against, a
/// raw's when `None`.
#[cfg(test)]
pub(crate) fn summary_from_json(json: &[u8], sidecar: &Path, raw: Option<&Path>) -> Summary {
    let now = SidecarNow {
        path: Some(sidecar.to_path_buf()),
        hash: String::new(),
        mtime: 0,
        json: Some(json.to_vec()),
        xmp: None,
        shared: None,
    };
    summary_of(Some(&now), raw.unwrap_or(Path::new("frame.cr3")))
}

/// The row's meta from its sidecar as the first phase summed it up, or
/// the empty meta when it has none, and the sidecar's hash so the next
/// pass can tell.
fn write_meta(tx: &Transaction<'_>, id: i64, seen: Seen<'_>) -> Result<()> {
    let (sidecar, summary) = (seen.sidecar, seen.summary);
    let meta = &summary.meta;
    let (iso, camera): (Option<u32>, _) = tx.query_row(
        "SELECT iso, camera_frame FROM files WHERE id = ?",
        params![id],
        |r| {
            Ok((
                r.get::<_, Option<i64>>(0)?.map(|v| v as u32),
                crate::camera_frame_of_column(r.get(1)?),
            ))
        },
    )?;
    let edited = summary.edited
        || summary
            .blend
            .is_some_and(|b| !crate::blend_is_default(b, iso));
    // An edited row always has a key, so it is not read again for one:
    // the edit's, or the mark for an edit that could not be read.
    let develop_key = summary
        .develop_key
        .map(|k| framed(k, summary.uncropped, summary.by_ratio, camera.as_ref()))
        .or(edited.then_some(0))
        .map(|k| k as i64);
    tx.prepare_cached(
        "UPDATE files SET sidecar = ?, sidecar_hash = ?, sidecar_mtime = ?, rating = ?, \
         flag = ?, label = ?, keywords = ?, edited = ?, turns = ?, exported = ?, \
         develop_key = ?, shift = ? WHERE id = ?",
    )?
    .execute(params![
        sidecar.and_then(|s| s.path.as_deref().map(path_bytes)),
        sidecar.map(|s| s.hash.as_str()),
        sidecar.map(|s| s.mtime),
        i64::from(meta.rating),
        filter::flag_name(meta.flag),
        filter::label_name(meta.label),
        serde_json::to_string(&meta.keywords).unwrap_or_else(|_| "[]".into()),
        i64::from(edited),
        i64::from(summary.turns),
        i64::from(summary.exported),
        develop_key,
        meta.time_shift,
        id
    ])?;
    tx.prepare_cached("DELETE FROM keywords WHERE file = ?")?
        .execute(params![id])?;
    let mut insert =
        tx.prepare_cached("INSERT OR IGNORE INTO keywords (file, word) VALUES (?, ?)")?;
    for word in &meta.keywords {
        insert.execute(params![id, word])?;
    }
    write_versions(tx, id, &summary.versions, iso, camera.as_ref())
}

/// A key as the row stores it: the edit's own, with the row's camera
/// frame taken in where it frames the edit; the mark of an edit that
/// could not be read (0) as it is.
fn framed(
    key: u64,
    uncropped: bool,
    by_ratio: bool,
    camera: Option<&greycard_core::raw::CameraFrame>,
) -> u64 {
    if key == 0 {
        return 0;
    }
    greycard_edit::key::framed_key(key, uncropped, by_ratio, camera)
}

/// The row's versions as the sidecar holds them, in its order: the
/// table's rows of the file replaced whole, so a version deleted from
/// the sidecar drops its row.
fn write_versions(
    tx: &Transaction<'_>,
    id: i64,
    versions: &[VersionSummary],
    iso: Option<u32>,
    camera: Option<&greycard_core::raw::CameraFrame>,
) -> Result<()> {
    tx.prepare_cached("DELETE FROM versions WHERE file = ?")?
        .execute(params![id])?;
    let mut insert = tx.prepare_cached(
        "INSERT OR IGNORE INTO versions (file, version, name, made, position, edited, \
         exported, develop_key) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )?;
    for (position, v) in versions.iter().enumerate() {
        let edited = v.edited || v.blend.is_some_and(|b| !crate::blend_is_default(b, iso));
        insert.execute(params![
            id,
            v.id,
            v.name,
            v.made.min(i64::MAX as u64) as i64,
            position as i64,
            i64::from(edited),
            i64::from(v.exported),
            v.develop_key
                .map(|k| framed(k, v.uncropped, v.by_ratio, camera) as i64),
        ])?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::fixture::{A7, R5, R6, write_frame};
    use crate::{ArchiveWrite, Entry, Filter, Library, PendingCount};
    use greycard_edit::meta::{Flag, Label};
    use std::time::SystemTime;

    /// A scratch folder of this run's own.
    pub(crate) fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-library-{what}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // Canonical, so paths the tests build under it compare equal
        // to the ones the index stores: on macOS the temporary
        // directory is a link (`/var` to `/private/var`).
        crate::canonical(&dir).unwrap()
    }

    /// Three frames and their sidecars: the R5 rated and picked
    /// beside the file, the R6 labeled under the hidden folder,
    /// the A7 bare.
    fn shoot(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let (r5, r6, a7) = (dir.join("r5.tif"), dir.join("r6.tif"), dir.join("a7.tif"));
        write_frame(&r5, &R5, 1);
        write_frame(&r6, &R6, 2);
        write_frame(&a7, &A7, 3);
        let mut s = Sidecar::default();
        s.meta.rating = 4;
        s.meta.flag = Flag::Pick;
        s.meta.set_keywords(vec!["Wedding".into(), "Harbor".into()]);
        s.save(&r5).unwrap();
        let mut s = Sidecar::default();
        s.meta.rating = 2;
        s.meta.label = Label::Red;
        s.save_in(&r6, Placement::Folder).unwrap();
        (r5, r6, a7)
    }

    fn names(lib: &Library, filter: &str) -> Vec<String> {
        lib.query(&Filter::parse(filter).unwrap())
            .unwrap()
            .iter()
            .map(Entry::name)
            .collect()
    }

    fn quiet() -> impl FnMut(Progress<'_>) {
        |_| {}
    }

    /// A read ahead's hand for a test: called on the read's thread
    /// before it asks the disk anything, for a path under its folder.
    type Hold = Arc<dyn Fn(&Path) + Send + Sync>;

    /// The holds, by the folder they are for: tests run side by side,
    /// each in its own folder.
    static HOLDS: std::sync::Mutex<Vec<(PathBuf, Hold)>> = std::sync::Mutex::new(Vec::new());

    pub(super) fn held_read(path: &Path) {
        let hold = HOLDS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|(dir, _)| path.starts_with(dir))
            .map(|(_, h)| h.clone());
        if let Some(hold) = hold {
            hold(path);
        }
    }

    /// `hold` on every read ahead under `dir` until the result is dropped.
    struct Holding(PathBuf);

    fn hold(dir: &Path, hold: Hold) -> Holding {
        let dir = canonical(dir).unwrap();
        HOLDS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((dir.clone(), hold));
        Holding(dir)
    }

    impl Drop for Holding {
        fn drop(&mut self) {
            HOLDS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|(d, _)| *d != self.0);
        }
    }

    /// A library that reads everything under `dir` as a network mount.
    /// The decoder's first probe in a process is slow, its tables read,
    /// so a pass on another library has it done before a test's short
    /// limit counts.
    fn over_network(dir: &Path, limit: Duration, at_once: usize) -> Library {
        Library::open_in_memory()
            .unwrap()
            .index_folder(dir, &mut quiet())
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let dir = canonical(dir).unwrap();
        lib.set_network(
            move |p| p.starts_with(&dir),
            crate::NetworkReads { limit, at_once },
        );
        lib
    }

    /// A pass over a network mount, its reads made ahead several at
    /// once, writes the rows a pass over a local disk writes: the
    /// EXIF, the sidecars' meta, a change, and a move with its sidecar
    /// carried.
    #[test]
    fn a_pass_over_a_network_mount_writes_what_a_local_one_does() {
        let near = scratch("net-near");
        let far = scratch("net-far");
        shoot(&near);
        shoot(&far);
        let mut local = Library::open_in_memory().unwrap();
        let mut remote = over_network(&far, Duration::from_secs(20), 2);
        let both = |local: &mut Library, remote: &mut Library| {
            let a = local.index_folder(&near, &mut quiet()).unwrap();
            let b = remote.index_folder(&far, &mut quiet()).unwrap();
            let counts = |r: &Report| {
                (
                    r.added,
                    r.moved,
                    r.carried,
                    r.changed,
                    r.unchanged,
                    r.meta_refreshed,
                    r.missing,
                    r.errors.len(),
                )
            };
            assert_eq!(counts(&a), counts(&b), "{a:?}\n{b:?}");
            for name in ["r5.tif", "r6.tif", "a7.tif", "moved.tif"] {
                let a = local.by_path(&near.join(name)).unwrap();
                let b = remote.by_path(&far.join(name)).unwrap();
                assert_eq!(a.is_some(), b.is_some(), "{name}");
                if let (Some(a), Some(b)) = (a, b) {
                    assert_eq!(a.hash, b.hash, "{name}");
                    assert_eq!(a.exif, b.exif, "{name}");
                    assert_eq!(a.meta, b.meta, "{name}");
                    assert_eq!(a.missing, b.missing, "{name}");
                    assert_eq!(a.sidecar.is_some(), b.sidecar.is_some(), "{name}");
                }
            }
        };
        both(&mut local, &mut remote);
        for dir in [&near, &far] {
            write_frame(&dir.join("a7.tif"), &A7, 9);
            std::fs::rename(dir.join("r5.tif"), dir.join("moved.tif")).unwrap();
        }
        both(&mut local, &mut remote);
        assert_eq!(
            remote
                .by_path(&far.join("moved.tif"))
                .unwrap()
                .unwrap()
                .meta
                .rating,
            4,
            "the sidecar came with the move"
        );
        greycard_core::testing::remove_dir_retry(&near);
        greycard_core::testing::remove_dir_retry(&far);
    }

    /// Until the reads given up on under `dir` have come back, which a
    /// pass over it waits for before it asks the share anything.
    fn settled(dir: &Path) {
        let until = Instant::now() + Duration::from_secs(5);
        while Far::new(crate::NetworkReads::default(), dir).stalled() {
            assert!(Instant::now() < until, "the stuck read came back");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A move whose sidecar carry does not answer is not recorded: the
    /// old row and the sidecar stay where they were, and the next pass
    /// finds the move again and finishes it. Recorded, the sidecar would
    /// be left at the old path with nothing to look for it there.
    #[test]
    fn a_carry_that_stalls_leaves_the_move_to_the_next_pass() {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let dir = scratch("net-carry");
        let (r5, _, _) = shoot(&dir);
        let r5 = canonical(&r5).unwrap();
        let mut lib = over_network(&dir, Duration::from_secs(1), 2);
        assert_eq!(lib.index_folder(&dir, &mut quiet()).unwrap().added, 3);
        let moved = dir.join("moved.tif");
        std::fs::rename(&r5, &moved).unwrap();
        let asleep = Arc::new(AtomicBool::new(true));
        let (a, old) = (asleep.clone(), r5.clone());
        let _holding = hold(
            &dir,
            Arc::new(move |p: &Path| {
                while a.load(SeqCst) && p == old {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }),
        );
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.moved, 0, "{report:?}");
        assert_eq!(report.carried, 0, "{report:?}");
        assert!(
            report.errors.iter().any(|(p, _)| p.ends_with("moved.tif")),
            "{report:?}"
        );
        assert!(lib.by_path(&moved).unwrap().is_none(), "no copy's row");
        assert!(lib.by_path(&r5).unwrap().is_some(), "the old row kept");
        assert!(Sidecar::find(&r5).is_some(), "the sidecar where it was");
        // Awake: the next pass makes the move, the sidecar with it.
        asleep.store(false, SeqCst);
        settled(&dir);
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.moved, 1, "{report:?}");
        assert!(report.errors.is_empty(), "{report:?}");
        let row = lib.by_path(&moved).unwrap().unwrap();
        assert_eq!(row.meta.rating, 4);
        assert!(lib.by_path(&r5).unwrap().is_none());
        assert!(Sidecar::find(&r5).is_none());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A read over a network mount that does not answer within the limit
    /// is an error in the report, and the pass finishes: the file's row
    /// is kept as it was, not marked missing, and the next pass, the
    /// share awake, reads it.
    #[test]
    fn a_read_that_stalls_past_the_limit_is_an_error_and_the_pass_finishes() {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let dir = scratch("net-stall");
        let (_, r6, a7) = shoot(&dir);
        let r6 = canonical(&r6).unwrap();
        let mut lib = over_network(&dir, Duration::from_secs(1), 2);
        // The file asleep, if any, and whether the share is asleep.
        let stalled = Arc::new(std::sync::Mutex::new(r6.clone()));
        let asleep = Arc::new(AtomicBool::new(true));
        let (s, a) = (stalled.clone(), asleep.clone());
        let _holding = hold(
            &dir,
            Arc::new(move |p: &Path| {
                while a.load(SeqCst) && *s.lock().unwrap() == p {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }),
        );
        let started = Instant::now();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the pass went on"
        );
        assert_eq!(report.added, 2, "{report:?}");
        assert_eq!(report.errors.len(), 1, "{report:?}");
        assert_eq!(report.errors[0].0, r6);
        assert!(lib.by_path(&r6).unwrap().is_none());
        // Awake: the next pass reads it.
        asleep.store(false, SeqCst);
        settled(&dir);
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.added, 1, "{report:?}");
        assert!(report.errors.is_empty(), "{report:?}");
        // Changed on disk and asleep again, the first file read: its
        // row is kept, neither changed nor missing. The file already
        // out beside it is read; the one after is not asked, and its
        // row is kept too.
        write_frame(&a7, &A7, 9);
        *stalled.lock().unwrap() = canonical(&a7).unwrap();
        asleep.store(true, SeqCst);
        let before = lib.by_path(&a7).unwrap().unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.missing, 0, "{report:?}");
        assert_eq!(report.changed, 0, "{report:?}");
        assert_eq!(report.unchanged, 1, "{report:?}");
        let errors: Vec<&str> = report.errors.iter().map(|(_, e)| e.as_str()).collect();
        assert_eq!(errors.len(), 2, "{report:?}");
        assert!(errors[0].starts_with("no answer"), "{errors:?}");
        assert!(errors[1].starts_with("not read"), "{errors:?}");
        assert_eq!(lib.by_path(&a7).unwrap().unwrap(), before);
        assert!(!lib.by_path(&r6).unwrap().unwrap().missing);
        asleep.store(false, SeqCst);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A first pass over files on a share, timed at several widths, for
    /// the choice of [`crate::READS_AT_ONCE`]. `GREYCARD_NETWORK_ROOT`
    /// names a folder of raws on one; each width takes its own slice of
    /// them (`GREYCARD_NETWORK_FILES` a width, 24 by default), so none
    /// is read from this machine's cache, and `GREYCARD_NETWORK_WIDTHS`
    /// the widths in the order run (0 is the local read, one file after
    /// another, with no limit). Bytes read are the process's `rchar`.
    #[test]
    #[ignore = "measures a share named by GREYCARD_NETWORK_ROOT"]
    fn network_reads_measured() {
        let Ok(root) = std::env::var("GREYCARD_NETWORK_ROOT") else {
            eprintln!("GREYCARD_NETWORK_ROOT is not set");
            return;
        };
        let per: usize = std::env::var("GREYCARD_NETWORK_FILES")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(24);
        let widths: Vec<usize> = std::env::var("GREYCARD_NETWORK_WIDTHS")
            .unwrap_or("0,1,2,4,8,16".into())
            .split(',')
            .map(|w| w.trim().parse().unwrap())
            .collect();
        let rchar = || {
            std::fs::read_to_string("/proc/self/io")
                .ok()
                .and_then(|io| {
                    io.lines()
                        .find_map(|l| l.strip_prefix("rchar: ").map(|n| n.parse::<u64>().ok()))
                        .flatten()
                })
                .unwrap_or(0)
        };
        let root = canonical(Path::new(&root)).unwrap();
        let files: Vec<PathBuf> = list_folder(&root)
            .unwrap()
            .into_iter()
            .filter(|p| greycard_core::decode::is_raw_path(p))
            .collect();
        assert!(files.len() >= per * widths.len(), "{} raws", files.len());
        for (i, &width) in widths.iter().enumerate() {
            let slice = &files[i * per..(i + 1) * per];
            let size: u64 = slice
                .iter()
                .map(|p| std::fs::metadata(p).unwrap().len())
                .sum();
            let mut lib = Library::open_in_memory().unwrap();
            let far = (width > 0).then(|| {
                Far::new(
                    crate::NetworkReads {
                        limit: crate::READ_LIMIT,
                        at_once: width,
                    },
                    &root,
                )
            });
            let (read_before, started) = (rchar(), Instant::now());
            let report = index_paths(
                lib.conn_mut(),
                &root,
                slice,
                false,
                HashMap::new(),
                &mut quiet(),
                &|| false,
                far.as_ref(),
                None,
            )
            .unwrap();
            let took = started.elapsed();
            let read = rchar() - read_before;
            assert_eq!(report.added, per, "{report:?}");
            // Again, the rows current: a stat and a sidecar a file, the
            // timer's pass over an archive indexed already.
            let existing = rows_in_folder(lib.conn_mut(), &path_bytes(&root)).unwrap();
            // Past the attribute cache of an SMB mount (`actimeo=1`).
            std::thread::sleep(Duration::from_millis(1500));
            let again = Instant::now();
            let report = index_paths(
                lib.conn_mut(),
                &root,
                slice,
                false,
                existing,
                &mut quiet(),
                &|| false,
                far.as_ref(),
                None,
            )
            .unwrap();
            let again = again.elapsed();
            assert_eq!(report.unchanged, per, "{report:?}");
            eprintln!(
                "width {width:2}: {per} files ({:.0} MB) in {:6.2} s, {:4.0} ms a file, \
                 {:.0} KB read a file; again {:4.0} ms a file",
                size as f64 / 1e6,
                took.as_secs_f64(),
                took.as_secs_f64() * 1000.0 / per as f64,
                read as f64 / 1024.0 / per as f64,
                again.as_secs_f64() * 1000.0 / per as f64,
            );
        }
    }

    /// A folder over a network mount is read a few files at a time,
    /// never more than asked.
    #[test]
    fn a_network_roots_reads_are_bounded() {
        let dir = scratch("net-bounded");
        for i in 0..12 {
            write_frame(&dir.join(format!("f{i:02}.tif")), &R5, i);
        }
        let out = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let most = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (o, m) = (out.clone(), most.clone());
        let _holding = hold(
            &dir,
            Arc::new(move |_: &Path| {
                use std::sync::atomic::Ordering::SeqCst;
                m.fetch_max(o.fetch_add(1, SeqCst) + 1, SeqCst);
                std::thread::sleep(Duration::from_millis(40));
                o.fetch_sub(1, SeqCst);
            }),
        );
        let mut lib = over_network(&dir, Duration::from_secs(20), 3);
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.added, 12, "{report:?}");
        assert_eq!(
            most.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "three at once, no more"
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The lens database's name is one value for the whole process
    /// (`crate::set_lens_database`), and a develop key takes it in
    /// whenever the lens profile is on, which it is by default. A test
    /// that sets the name, and a test that compares a key the index
    /// made with one it makes itself, hold this for their whole run,
    /// or they race each other across threads: the index reads the
    /// name at one moment and the test at another.
    static LENS_NAME: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lens_name_held() -> std::sync::MutexGuard<'static, ()> {
        LENS_NAME.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Names the lens database for the test, and names none again when
    /// the test ends, however it ends.
    struct LensNamed {
        _held: std::sync::MutexGuard<'static, ()>,
    }

    impl LensNamed {
        fn new(name: &str) -> Self {
            let held = lens_name_held();
            crate::set_lens_database(Some(name.into()));
            Self { _held: held }
        }
    }

    impl Drop for LensNamed {
        fn drop(&mut self) {
            crate::set_lens_database(None);
        }
    }

    #[test]
    fn a_pass_reports_a_carried_set_once_until_it_changes() {
        let dir = scratch("carried-sets");
        let (r5, _r6, _a7) = shoot(&dir);
        let mut c = crate::collections::Collection::new("Picks");
        c.add(&[(hash::hash_file(&r5).unwrap(), r5.clone(), None)]);
        c.stamp(1, "laptop");
        let item = crate::carry::Item::Collection(c.clone());
        crate::carry::put(&dir, &item).unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let first = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(first.sets.len(), 1, "{first:?}");
        assert_eq!(first.added, 3, "the sets' folder holds no frame");
        assert!(lib.index_tree(&dir, &mut quiet()).unwrap().sets.is_empty());
        c.name = "Renamed".into();
        c.stamp(2, "laptop");
        crate::carry::put(&dir, &crate::carry::Item::Collection(c)).unwrap();
        let again = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert_eq!(again.sets.len(), 1);
        let Ok(crate::carry::Parsed::Item(crate::carry::Item::Collection(back))) =
            again.sets[0].parse()
        else {
            panic!("{:?}", again.sets);
        };
        assert_eq!(back.name, "Renamed");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A part file a write left a day ago is named by every pass, with
    /// the copy it was to replace read as before; a fresh one is not.
    #[test]
    fn a_pass_names_the_leftovers_in_a_sets_folder_every_time() {
        let dir = scratch("carried-leftovers");
        let (r5, _r6, _a7) = shoot(&dir);
        let mut c = crate::collections::Collection::new("Picks");
        c.add(&[(hash::hash_file(&r5).unwrap(), r5.clone(), None)]);
        c.stamp(1, "laptop");
        crate::carry::put(&dir, &crate::carry::Item::Collection(c.clone())).unwrap();
        let sets = crate::carry::sets_dir(&dir);
        let (old, fresh) = (
            sets.join(format!("{}.json.41-0.part", c.id)),
            sets.join(format!("{}.json.41-1.part", c.id)),
        );
        for p in [&old, &fresh] {
            std::fs::write(p, "{").unwrap();
        }
        let two_days = std::time::SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(two_days)
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let first = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(first.sets.len(), 1, "{first:?}");
        assert_eq!(first.sets_stale, std::slice::from_ref(&old));
        let again = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert!(again.sets.is_empty());
        assert_eq!(again.sets_stale, [old]);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_folder_is_indexed_and_a_second_pass_reads_nothing() {
        let dir = scratch("index");
        let (r5, r6, _a7) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        let mut seen = Vec::new();
        let report = lib
            .index_folder(&dir, &mut |p| {
                seen.push((p.done, p.total, p.path.to_path_buf()))
            })
            .unwrap();
        assert_eq!(report.added, 3, "{report:?}");
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].0, 0);
        assert_eq!(seen[2], (2, 3, canonical(&r6).unwrap()));
        assert_eq!(lib.len().unwrap(), 3);

        // The EXIF came through the probe.
        let e = lib.by_path(&r5).unwrap().expect("the R5's row");
        assert_eq!(e.exif.camera, "Canon EOS R5");
        assert_eq!(e.exif.lens.as_deref(), Some("RF35mm F1.4 L VCM"));
        assert_eq!(e.exif.iso, Some(100));
        assert_eq!(e.exif.focal, Some(35.0));
        assert_eq!(e.exif.aperture, Some(2.0));
        assert!((e.exif.shutter.unwrap() - 1.0 / 2500.0).abs() < 1e-9);
        assert_eq!(e.exif.taken.as_deref(), Some("2024-08-24 15:32:39"));
        assert_eq!(e.hash, hash::hash_file(&r5).unwrap());
        assert_eq!(e.size, std::fs::metadata(&r5).unwrap().len());
        // And the meta, from wherever the sidecar was.
        assert_eq!(e.meta.rating, 4);
        assert_eq!(e.meta.flag, Flag::Pick);
        assert_eq!(e.meta.keywords, ["Wedding", "Harbor"]);
        assert_eq!(
            e.sidecar,
            Some(canonical(&r5).unwrap().with_extension("tif.gcd"))
        );
        let e = lib.by_path(&r6).unwrap().unwrap();
        assert_eq!(e.meta.label, Label::Red);
        assert!(e.sidecar.unwrap().to_string_lossy().contains(".greycard"));

        // The filters, against the seeded rows.
        assert_eq!(names(&lib, ""), ["a7.tif", "r5.tif", "r6.tif"]);
        assert_eq!(names(&lib, "camera:R6"), ["r6.tif"]);
        assert_eq!(names(&lib, "make=canon"), ["r5.tif", "r6.tif"]);
        assert_eq!(names(&lib, "make!=canon"), ["a7.tif"]);
        assert_eq!(names(&lib, "lens:\"24-70\""), ["a7.tif"]);
        assert_eq!(names(&lib, "iso>=3200"), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "iso>=3200 camera:sony"), ["a7.tif"]);
        assert_eq!(names(&lib, "focal:50mm"), ["r6.tif"]);
        assert_eq!(names(&lib, "focal<=35"), ["a7.tif", "r5.tif"]);
        assert_eq!(names(&lib, "focal>=24"), ["a7.tif", "r5.tif", "r6.tif"]);
        assert_eq!(names(&lib, "focal>24"), ["r5.tif", "r6.tif"]);
        assert_eq!(names(&lib, "aperture:f/1.8"), ["r6.tif"]);
        assert_eq!(names(&lib, "aperture<2.8"), ["r5.tif", "r6.tif"]);
        assert_eq!(names(&lib, "aperture<=2.8"), ["a7.tif", "r5.tif", "r6.tif"]);
        assert_eq!(names(&lib, "aperture>2"), ["a7.tif"]);
        assert_eq!(names(&lib, "aperture>=2"), ["a7.tif", "r5.tif"]);
        assert_eq!(names(&lib, "aperture:2.8"), ["a7.tif"]);
        assert_eq!(names(&lib, "aperture!=2.8"), ["r5.tif", "r6.tif"]);
        assert_eq!(names(&lib, "shutter:1/60"), ["r6.tif"]);
        assert_eq!(names(&lib, "shutter<1/1000"), ["r5.tif"]);
        assert_eq!(names(&lib, "date:2026-09"), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "date:2026-09-21"), ["r6.tif"]);
        assert_eq!(names(&lib, "date<=2026-09-02"), ["a7.tif", "r5.tif"]);
        assert_eq!(names(&lib, "date>2024"), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "date:2024"), ["r5.tif"]);
        assert_eq!(names(&lib, "rating>=3"), ["r5.tif"]);
        assert_eq!(names(&lib, "rating:0"), ["a7.tif"]);
        assert_eq!(names(&lib, "rating<=2 flag!=pick"), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "flag:pick"), ["r5.tif"]);
        assert_eq!(names(&lib, "flag:none"), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "label:red"), ["r6.tif"]);
        assert_eq!(names(&lib, "label!=none"), ["r6.tif"]);
        assert_eq!(names(&lib, "keyword:wedding"), ["r5.tif"]);
        assert_eq!(names(&lib, "keyword=harbor"), ["r5.tif"]);
        assert_eq!(names(&lib, "keyword=harb"), Vec::<String>::new());
        assert_eq!(names(&lib, "keyword!=wedding"), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "harbor"), ["r5.tif"]);
        assert_eq!(names(&lib, "R6"), ["r6.tif"]);
        // Two words both have to land, in the name or a keyword.
        assert_eq!(names(&lib, "r 6"), ["r6.tif"]);
        assert_eq!(names(&lib, "r6 harbor"), Vec::<String>::new());
        assert_eq!(names(&lib, "r5 harbor"), ["r5.tif"]);
        assert_eq!(names(&lib, "name:.tif"), ["a7.tif", "r5.tif", "r6.tif"]);
        assert_eq!(
            names(
                &lib,
                &format!("folder=\"{}\"", canonical(&dir).unwrap().display())
            ),
            ["a7.tif", "r5.tif", "r6.tif"]
        );
        assert_eq!(lib.count(&Filter::parse("iso>=3200").unwrap()).unwrap(), 2);
        assert_eq!(lib.folders().unwrap(), vec![canonical(&dir).unwrap()]);

        // Again: nothing added, nothing read.
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(
            report,
            Report {
                unchanged: 3,
                ..Report::default()
            }
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The Keywords field's vocabulary: every spelling the library
    /// holds, each with how many files hold it, and a missing file's
    /// words gone with it. Spellings that differ in case are kept
    /// apart here; the window folds them.
    #[test]
    fn the_library_counts_its_keywords_for_the_keywords_field() {
        let dir = scratch("words");
        let (_r5, _r6, a7) = shoot(&dir);
        let mut s = Sidecar::default();
        s.meta
            .set_keywords(vec!["HARBOR".into(), "harvest".into(), "50%_off\\".into()]);
        s.save(&a7).unwrap();
        let gone = dir.join("gone.tif");
        write_frame(&gone, &R6, 9);
        let mut s = Sidecar::default();
        s.meta
            .set_keywords(vec!["Hartford".into(), "harvest".into()]);
        s.save(&gone).unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let pairs = |want: &[(&str, usize)]| -> Vec<(String, usize)> {
            want.iter().map(|(w, n)| (w.to_string(), *n)).collect()
        };
        assert_eq!(
            lib.keyword_counts().unwrap(),
            pairs(&[
                ("50%_off\\", 1),
                ("HARBOR", 1),
                ("Harbor", 1),
                ("Hartford", 1),
                ("Wedding", 1),
                ("harvest", 2),
            ])
        );

        // The file gone from the disk: its words go with it.
        std::fs::remove_file(&gone).unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(
            lib.keyword_counts().unwrap(),
            pairs(&[
                ("50%_off\\", 1),
                ("HARBOR", 1),
                ("Harbor", 1),
                ("Wedding", 1),
                ("harvest", 1),
            ])
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The filter bar's chips: a facet's values counted with one
    /// `GROUP BY`, and a chip pressed matches exactly the files its
    /// count counted, whatever the facet.
    #[test]
    fn a_facet_counts_its_values_and_a_chip_matches_what_it_counted() {
        use crate::filter::{Facet, Term};
        let dir = scratch("facets");
        let (r5, r6, a7) = shoot(&dir);
        // A second R6 frame, its keyword in another case.
        let r6b = dir.join("r6b.tif");
        write_frame(&r6b, &R6, 40);
        let mut s = Sidecar::default();
        s.meta.set_keywords(vec!["HARBOR".into()]);
        s.save(&r6b).unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let sony = lib.by_path(&a7).unwrap().unwrap().exif.camera;
        let counts = |facet, within: Option<&[i64]>, filter: &Filter| -> Vec<(String, usize)> {
            lib.facet_counts(facet, within, filter)
                .unwrap()
                .into_iter()
                .map(|c| (c.value, c.count))
                .collect()
        };
        let pairs = |want: &[(&str, usize)]| -> Vec<(String, usize)> {
            want.iter().map(|(v, n)| (v.to_string(), *n)).collect()
        };
        let all = Filter::default();
        assert_eq!(
            counts(Facet::Camera, None, &all),
            pairs(&[("Canon EOS R6m2", 2), ("Canon EOS R5", 1), (&sony, 1)])
        );
        assert_eq!(
            counts(Facet::Lens, None, &all),
            pairs(&[
                ("RF50mm F1.8 STM", 2),
                ("FE 24-70mm F2.8 GM II", 1),
                ("RF35mm F1.4 L VCM", 1)
            ])
        );
        // Numbers in their own order, not the text's: 100 before 3200.
        assert_eq!(
            counts(Facet::Iso, None, &all),
            pairs(&[("100", 1), ("3200", 1), ("6400", 2)])
        );
        assert_eq!(
            counts(Facet::Focal, None, &all),
            pairs(&[("24", 1), ("35", 1), ("50", 2)])
        );
        assert_eq!(
            counts(Facet::Date, None, &all),
            pairs(&[("2024-08-24", 1), ("2026-09-02", 1), ("2026-09-21", 2)])
        );
        // A keyword in two cases is one chip, counted once a file.
        assert_eq!(
            counts(Facet::Keyword, None, &all),
            pairs(&[("harbor", 2), ("wedding", 1)])
        );
        let words = lib.facet_counts(Facet::Keyword, None, &all).unwrap();
        assert_eq!(words[1].label, "Wedding", "the case it was written in");

        // Every chip of every facet, pressed alone, lists what it
        // counted: the match is the grouping's own expression.
        for facet in Facet::ALL {
            for chip in lib.facet_counts(facet, None, &all).unwrap() {
                let pressed = Filter {
                    terms: vec![Term::Facet {
                        facet,
                        values: vec![chip.value.clone()],
                    }],
                };
                assert_eq!(
                    lib.count(&pressed).unwrap(),
                    chip.count,
                    "{facet:?} {}",
                    chip.value
                );
            }
        }
        // Two chips of one facet are either, and none is everything.
        let either = Filter {
            terms: vec![Term::Facet {
                facet: Facet::Iso,
                values: vec!["100".into(), "3200".into()],
            }],
        };
        assert_eq!(lib.count(&either).unwrap(), 2);
        let none = Filter {
            terms: vec![Term::Facet {
                facet: Facet::Iso,
                values: Vec::new(),
            }],
        };
        assert_eq!(lib.count(&none).unwrap(), 4);
        // A chip held out lists every file it did not count, a file
        // with no value for the facet among them.
        for facet in Facet::ALL {
            for chip in lib.facet_counts(facet, None, &all).unwrap() {
                let held_out = Filter {
                    terms: vec![Term::NotFacet {
                        facet,
                        values: vec![chip.value.clone()],
                    }],
                };
                assert_eq!(
                    lib.count(&held_out).unwrap(),
                    4 - chip.count,
                    "{facet:?} {}",
                    chip.value
                );
            }
        }

        // The other tests narrow the count; `within` narrows the rows.
        assert_eq!(
            counts(Facet::Iso, None, &Filter::parse("camera:R6").unwrap()),
            pairs(&[("6400", 2)])
        );
        let ids = lib
            .ids_of(&[r5.clone(), r6.clone(), dir.join("not-there.tif")])
            .unwrap();
        assert!(ids[0].is_some() && ids[1].is_some());
        assert_eq!(ids[2], None, "no row, no id");
        let two: Vec<i64> = ids.iter().flatten().copied().collect();
        assert_eq!(
            counts(Facet::Camera, Some(&two), &all),
            pairs(&[("Canon EOS R5", 1), ("Canon EOS R6m2", 1)])
        );
        assert_eq!(counts(Facet::Camera, Some(&[]), &all), pairs(&[]));
        let every: Vec<i64> = lib
            .ids_of(&[r5, r6, a7, r6b])
            .unwrap()
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(every.len(), 4);
        let passing = lib
            .ids_passing(&every, &Filter::parse("camera:R6").unwrap())
            .unwrap();
        assert_eq!(passing.len(), 2);
        assert!(passing.contains(&every[1]) && passing.contains(&every[3]));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The terms a sidecar and a path can answer are answered by
    /// them as the index answers them from the row, so the editor can
    /// ask its sidecars — fresher than a row still being written —
    /// and get the index's answer.
    #[test]
    fn a_meta_term_answers_from_the_sidecar_as_the_index_does() {
        let dir = scratch("onmeta");
        shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let entries = lib.query(&Filter::default()).unwrap();
        let folder = format!("folder:\"{}\"", canonical(&dir).unwrap().display());
        for text in [
            "rating>=3",
            "rating:0",
            "rating!=2",
            "rating<4",
            "rating>2",
            "rating<=2",
            "flag:pick",
            "flag!=pick",
            "flag:none",
            "label:red",
            "label!=none",
            "keyword:wed",
            "keyword=harbor",
            "keyword=HARBOR",
            "keyword!=wedding",
            "harbor",
            "r6",
            "TIF",
            "name:r5",
            "name=a7.tif",
            "name!=a7.tif",
            "missing:no",
            "missing:yes",
        ] {
            let filter = Filter::parse(text).unwrap();
            let term = &filter.terms[0];
            assert!(term.is_meta(), "{text}");
            let got: Vec<String> = entries
                .iter()
                .filter(|e| term.on_meta(&e.path, &e.meta).unwrap())
                .map(Entry::name)
                .collect();
            assert_eq!(got, names(&lib, text), "{text}");
        }
        for text in [
            "camera:R6",
            "make=canon",
            "iso>=100",
            "lens:RF",
            "date:2026",
            "focal:50",
            "aperture:2",
            "shutter<1",
            // The folder as the index keys it, canonical: `/var` on
            // macOS is `/private/var` there, which a path in hand
            // need not say.
            &folder,
        ] {
            let filter = Filter::parse(text).unwrap();
            assert!(!filter.terms[0].is_meta(), "{text} is the index's");
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A file's kind is a facet and a term, both read from its name
    /// through the one function: the chips count raws and pictures
    /// by format, a chip lists what it counted, and `kind:` answered
    /// from a path in hand is the index's answer from the row.
    #[test]
    fn a_files_kind_is_counted_and_matched_by_one_reading_of_its_name() {
        use crate::filter::{Facet, Term};
        use crate::fixture::write_mosaic_dng;
        let dir = scratch("kinds");
        shoot(&dir);
        write_mosaic_dng(&dir.join("d1.dng"), "RGGB");
        write_mosaic_dng(&dir.join("d2.DNG"), "RGGB");
        for name in ["SHOT.JPG", "shot.jpeg", "flat.png"] {
            image::RgbImage::from_pixel(4, 3, image::Rgb([90, 120, 150]))
                .save(dir.join(name))
                .unwrap();
        }
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.added, 8, "{report:?}");
        let all = Filter::default();
        let counts = |filter: &Filter| -> Vec<(String, usize)> {
            lib.facet_counts(Facet::Kind, None, filter)
                .unwrap()
                .into_iter()
                .map(|c| (c.value, c.count))
                .collect()
        };
        let pairs = |want: &[(&str, usize)]| -> Vec<(String, usize)> {
            want.iter().map(|(v, n)| (v.to_string(), *n)).collect()
        };
        // Most held first, as the cameras are; two spellings of JPEG
        // and two cases of DNG are one chip each.
        assert_eq!(
            counts(&all),
            pairs(&[("tiff", 3), ("jpeg", 2), ("raw", 2), ("png", 1)])
        );
        // The other tests narrow it: only the R5 is rated.
        assert_eq!(
            counts(&Filter::parse("rating>=3").unwrap()),
            pairs(&[("tiff", 1)])
        );
        // A chip pressed lists what it counted, on the rows and on
        // the paths in hand, and so does the term typed for it.
        let entries = lib.query(&all).unwrap();
        for chip in lib.facet_counts(Facet::Kind, None, &all).unwrap() {
            let pressed = Filter {
                terms: vec![Term::Facet {
                    facet: Facet::Kind,
                    values: vec![chip.value.clone()],
                }],
            };
            assert_eq!(lib.count(&pressed).unwrap(), chip.count, "{}", chip.value);
            let term = &pressed.terms[0];
            assert!(term.is_meta());
            let in_hand = entries
                .iter()
                .filter(|e| term.on_meta(&e.path, &e.meta).unwrap())
                .count();
            assert_eq!(in_hand, chip.count, "{}", chip.value);
            let typed = Filter::parse(&format!("kind={}", chip.value)).unwrap();
            assert_eq!(lib.count(&typed).unwrap(), chip.count, "{}", chip.value);
        }
        let mut raws = names(&lib, "kind:raw");
        raws.sort();
        assert_eq!(raws, ["d1.dng", "d2.DNG"]);

        // The term on a path in hand agrees with the index's row.
        for text in [
            "kind:raw",
            "kind!=raw",
            "kind=jpeg",
            "kind:jpg",
            "kind!=tif",
            "kind:pngs",
            "kind:TIFF",
        ] {
            let filter = Filter::parse(text).unwrap();
            let term = &filter.terms[0];
            assert!(term.is_meta(), "{text}");
            let got: Vec<String> = entries
                .iter()
                .filter(|e| term.on_meta(&e.path, &e.meta).unwrap())
                .map(Entry::name)
                .collect();
            assert!(!got.is_empty(), "{text}");
            assert_eq!(got, names(&lib, text), "{text}");
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A camera's JPEG beside its raw is the raw's companion: both keep
    /// rows, and every listing and count leaves the JPEG out unless the
    /// JPEGs are listed apart, by the same rule the folder's paths are
    /// paired by in hand. A raw gone from the disk frees its JPEG.
    #[test]
    fn a_jpeg_beside_its_raw_is_left_out_of_listings_unless_apart() {
        use crate::filter::Facet;
        use crate::fixture::write_mosaic_dng;
        let dir = scratch("pairs");
        write_mosaic_dng(&dir.join("IMG_1.dng"), "RGGB");
        write_mosaic_dng(&dir.join("IMG_3.DNG"), "RGGB");
        // Not a raw the decoder reads, and a row all the same.
        std::fs::write(dir.join("IMG_3.CR3"), b"not a raw").unwrap();
        for name in ["IMG_1.JPG", "IMG_1.png", "IMG_2.JPG", "img_3.jpeg"] {
            image::RgbImage::from_pixel(4, 3, image::Rgb([90, 120, 150]))
                .save(dir.join(name))
                .unwrap();
        }
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.seen(), 7, "{report:?}");
        let kinds = |lib: &Library| -> Vec<(String, usize)> {
            lib.facet_counts(Facet::Kind, None, &Filter::default())
                .unwrap()
                .into_iter()
                .map(|c| (c.value, c.count))
                .collect()
        };
        let listed = |lib: &Library| -> Vec<String> {
            let mut got: Vec<String> = lib
                .paths_under(std::slice::from_ref(&dir))
                .unwrap()
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            got.sort();
            got
        };
        let frames = [
            "IMG_1.dng",
            "IMG_1.png",
            "IMG_2.JPG",
            "IMG_3.CR3",
            "IMG_3.DNG",
        ];
        assert_eq!(listed(&lib), frames);
        // The rule in SQL is the rule in hand.
        let mut on_disk: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        on_disk.sort();
        let mut by_hand: Vec<PathBuf> = crate::pairs::pair(on_disk, false).frames;
        by_hand.sort();
        let mut by_index: Vec<PathBuf> = lib
            .query(&Filter::default())
            .unwrap()
            .into_iter()
            .map(|e| e.path)
            .collect();
        by_index.sort();
        assert_eq!(by_index, by_hand);
        assert_eq!(lib.count(&Filter::default()).unwrap(), 5);
        assert_eq!(names(&lib, "kind:jpeg"), ["IMG_2.JPG"]);
        let one = |v: &str, n: usize| (v.to_string(), n);
        assert_eq!(kinds(&lib), [one("raw", 3), one("jpeg", 1), one("png", 1)]);
        assert_eq!(lib.count_under(&dir).unwrap(), 5);
        assert_eq!(lib.count_in_canonical(&dir).unwrap(), 5);
        assert_eq!(
            lib.rows_in_canonical(&dir, crate::Order::NAME)
                .unwrap()
                .len(),
            5
        );
        assert_eq!(
            lib.rows_under_canonical(std::slice::from_ref(&dir), crate::Order::NAME)
                .unwrap()
                .len(),
            5
        );
        assert_eq!(
            lib.folder_counts_under_canonical(&dir).unwrap(),
            [(dir.clone(), 5)]
        );
        // Each raw's companions, the camera's raw taking its JPEG over
        // the DNG of its name.
        assert_eq!(
            lib.companions_in(std::slice::from_ref(&dir)).unwrap(),
            [
                (dir.join("IMG_1.dng"), vec![dir.join("IMG_1.JPG")]),
                (dir.join("IMG_3.CR3"), vec![dir.join("img_3.jpeg")]),
            ]
        );
        // Rows asked about by id answer for themselves, companion or not.
        let ids: Vec<i64> = lib
            .ids_of(&[dir.join("IMG_1.JPG")])
            .unwrap()
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(ids.len(), 1);
        assert_eq!(lib.ids_passing(&ids, &Filter::default()).unwrap().len(), 1);

        // Apart, every file is a frame.
        lib.set_jpegs_apart(true);
        assert_eq!(listed(&lib).len(), 7);
        assert_eq!(lib.count(&Filter::default()).unwrap(), 7);
        let mut jpegs = names(&lib, "kind:jpeg");
        jpegs.sort();
        assert_eq!(jpegs, ["IMG_1.JPG", "IMG_2.JPG", "img_3.jpeg"]);
        assert_eq!(kinds(&lib), [one("jpeg", 3), one("raw", 3), one("png", 1)]);
        assert_eq!(lib.count_in_canonical(&dir).unwrap(), 7);
        assert!(
            lib.companions_in(std::slice::from_ref(&dir))
                .unwrap()
                .is_empty()
        );

        // A raw gone from the disk leaves its JPEG a frame of its own.
        lib.set_jpegs_apart(false);
        std::fs::remove_file(dir.join("IMG_1.dng")).unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let mut jpegs = names(&lib, "kind:jpeg");
        jpegs.sort();
        assert_eq!(jpegs, ["IMG_1.JPG", "IMG_2.JPG"]);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// An Olympus or OM System High Res Shot: the merge as `.ORF`, one
    /// of its frames as `.ORI`. Two raw rows, each with its own hash
    /// and sidecar; the camera's JPEG is the ORF's; and the one gone
    /// is forgotten alone.
    #[test]
    fn a_high_res_shot_and_its_single_frame_are_two_frames() {
        let dir = scratch("ori");
        let (orf, ori, jpg) = (dir.join("P1.ORF"), dir.join("P1.ORI"), dir.join("P1.JPG"));
        // Not raws the decoder reads, and rows all the same.
        std::fs::write(&orf, b"the merge").unwrap();
        std::fs::write(&ori, b"one frame").unwrap();
        image::RgbImage::from_pixel(4, 3, image::Rgb([90, 120, 150]))
            .save(&jpg)
            .unwrap();
        for (raw, rating) in [(&orf, 5), (&ori, 2)] {
            let mut s = Sidecar::default();
            s.meta.rating = rating;
            s.save(raw).unwrap();
        }
        assert_ne!(Sidecar::path_for(&orf), Sidecar::path_for(&ori));
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.seen(), 3, "{report:?}");
        let sorted = |mut v: Vec<String>| {
            v.sort();
            v
        };
        assert_eq!(sorted(names(&lib, "kind:raw")), ["P1.ORF", "P1.ORI"]);
        assert_eq!(sorted(names(&lib, "")), ["P1.ORF", "P1.ORI"]);
        assert_eq!(
            lib.companions_in(std::slice::from_ref(&dir)).unwrap(),
            [(orf.clone(), vec![jpg.clone()])]
        );
        let (a, b) = (
            lib.by_path(&orf).unwrap().unwrap(),
            lib.by_path(&ori).unwrap().unwrap(),
        );
        assert_ne!(a.hash, b.hash);
        assert_eq!((a.meta.rating, b.meta.rating), (5, 2));

        std::fs::remove_file(&ori).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.missing, 1, "{report:?}");
        assert_eq!(lib.prune_missing().unwrap(), 1);
        assert_eq!(names(&lib, "kind:raw"), ["P1.ORF"]);
        assert_eq!(lib.by_path(&orf).unwrap().unwrap().meta.rating, 5);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A Pentax `.PEF` is indexed as a raw, with its camera's JPEG
    /// beside it as its companion and not a row of its own.
    #[test]
    fn a_pef_is_indexed_as_a_raw_with_its_jpeg() {
        let dir = scratch("pef");
        let (pef, jpg) = (dir.join("IMGP0001.PEF"), dir.join("IMGP0001.JPG"));
        // Not a raw the decoder reads, and a row all the same.
        std::fs::write(&pef, b"a pentax raw").unwrap();
        image::RgbImage::from_pixel(4, 3, image::Rgb([90, 120, 150]))
            .save(&jpg)
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.seen(), 2, "{report:?}");
        assert_eq!(names(&lib, "kind:raw"), ["IMGP0001.PEF"]);
        assert_eq!(names(&lib, ""), ["IMGP0001.PEF"]);
        assert_eq!(
            lib.companions_in(std::slice::from_ref(&dir)).unwrap(),
            [(pef.clone(), vec![jpg.clone()])]
        );
        assert!(lib.by_path(&pef).unwrap().is_some());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The index keeps each file's kind of mosaic, read on the same
    /// decoder as its EXIF: a Bayer DNG, an X-Trans DNG, a picture;
    /// and a row from before the column (schema 10) gets it on the next
    /// pass without its file being read again for anything else.
    #[test]
    fn the_index_keeps_each_files_mosaic() {
        use crate::fixture::write_mosaic_dng;
        use greycard_core::raw::Mosaic;
        let dir = scratch("mosaic");
        let (bayer, xtrans, picture) = (
            dir.join("bayer.dng"),
            dir.join("xtrans.dng"),
            dir.join("r5.tif"),
        );
        write_mosaic_dng(&bayer, "RGGB");
        write_mosaic_dng(&xtrans, "GGRGGBGGBGGRBRGRBGGGBGGRGGRGGBRBGBRG");
        write_frame(&picture, &R5, 1);
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.added, 3, "{report:?}");
        let mosaic = |lib: &Library, p: &Path| lib.mosaic(p).unwrap();
        assert_eq!(
            mosaic(&lib, &bayer),
            Some(FileMosaic::Mosaic(Mosaic::Bayer))
        );
        assert_eq!(
            mosaic(&lib, &xtrans),
            Some(FileMosaic::Mosaic(Mosaic::XTrans))
        );
        assert_eq!(mosaic(&lib, &picture), Some(FileMosaic::NotMosaic));
        assert_eq!(mosaic(&lib, &dir.join("absent.RAF")), None);
        // A row from before the column: unread, and filled by the next
        // pass, which otherwise finds nothing changed.
        lib.conn
            .execute("UPDATE files SET mosaic = NULL", [])
            .unwrap();
        assert_eq!(mosaic(&lib, &xtrans), None);
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!((report.added, report.changed), (0, 0), "{report:?}");
        assert_eq!(
            mosaic(&lib, &xtrans),
            Some(FileMosaic::Mosaic(Mosaic::XTrans))
        );
        assert_eq!(
            mosaic(&lib, &bayer),
            Some(FileMosaic::Mosaic(Mosaic::Bayer))
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The pass reads each raw into memory and never maps one: its
    /// EXIF and mosaic, the mosaic a row from before the column is owed,
    /// and the camera's orientation an XMP's turn is measured from. The
    /// index walks shares, and a mapped raw whose volume drops (a Mac
    /// asleep on an SMB share) is a SIGBUS nothing catches.
    #[test]
    fn a_pass_maps_no_raw() {
        use crate::fixture::write_mosaic_dng;
        use greycard_core::raw::Mosaic;
        let dir = scratch("unmapped");
        let (turned, plain) = (dir.join("turned.dng"), dir.join("plain.DNG"));
        write_mosaic_dng(&turned, "RGGB");
        write_mosaic_dng(&plain, "GGRGGBGGBGGRBRGRBGGGBGGRGGRGGBRBGBRG");
        let xmp = greycard_edit_io::xmp::fresh(&Meta::default(), None).replace(
            "<rdf:Description rdf:about=\"\"",
            "<rdf:Description rdf:about=\"\" \
             xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\" tiff:Orientation=\"6\"",
        );
        std::fs::write(greycard_edit_io::xmp::short_path(&turned), xmp).unwrap();
        // A sidecar whose mask waits on the frame's shape (§724): the
        // summary asks the frame's header for it, unmapped too.
        let mut waiting = Sidecar::default();
        let mut placed = greycard_edit::Edit::default();
        placed
            .retouch
            .patches
            .push(greycard_edit::retouch::Patch::default());
        waiting.record(placed);
        assert!(waiting.turn_by(1, Some(1.5)).is_some());
        let mut file: serde_json::Value = serde_json::from_str(&waiting.to_json()).unwrap();
        file.as_object_mut().unwrap().remove("shape");
        std::fs::write(Sidecar::path_for(&plain), file.to_string()).unwrap();
        greycard_core::decode::watch_maps_under(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.added, 2, "{report:?}");
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let row = |p: &Path| lib.by_path(p).unwrap().unwrap();
        assert_eq!(row(&plain).exif.make, "Synthetic");
        assert_ne!(row(&turned).turns, 0, "the XMP's turn, on the camera's tag");
        assert_eq!(
            lib.mosaic(&plain).unwrap(),
            Some(FileMosaic::Mosaic(Mosaic::XTrans))
        );
        // A row from before the column: its mosaic read alone.
        lib.conn
            .execute("UPDATE files SET mosaic = NULL", [])
            .unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(
            lib.mosaic(&turned).unwrap(),
            Some(FileMosaic::Mosaic(Mosaic::Bayer))
        );
        assert_eq!(greycard_core::decode::maps_under(&dir), 0, "raws mapped");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Two bodies at one event, the second's clock five minutes slow,
    /// one of its frames shot just after midnight by the first's clock.
    /// Returned with the four frames: the first body's two, the second
    /// body's two.
    fn two_bodies(dir: &Path) -> [PathBuf; 4] {
        use crate::fixture::Frame;
        let at = |frame: &Frame<'static>, taken: &'static str| Frame { taken, ..*frame };
        let frames = [
            (dir.join("a1.tif"), at(&R5, "2026:09:19 23:57:00")),
            (dir.join("a2.tif"), at(&R5, "2026:09:20 00:04:00")),
            (dir.join("b1.tif"), at(&R6, "2026:09:19 23:56:00")),
            (dir.join("b2.tif"), at(&R6, "2026:09:20 00:00:00")),
        ];
        for (i, (path, frame)) in frames.iter().enumerate() {
            write_frame(path, frame, i as u16 + 1);
        }
        frames.map(|(path, _)| path)
    }

    /// The second body's frames given a shift in their sidecars' meta.
    fn shift_by(paths: &[PathBuf], seconds: i64) {
        for path in paths {
            let mut s = Sidecar::default();
            s.meta.set_time_shift(seconds);
            s.save(path).unwrap();
        }
    }

    /// A shift kept in a sidecar's meta is the frame's time for the
    /// index: a list by capture time interleaves the two bodies, the
    /// date filter and the Day chips go by it, and the EXIF's own
    /// time is kept beside it. Taken off, the frames go back.
    #[test]
    fn a_shift_in_the_sidecar_moves_the_frame_for_the_sort_and_the_days() {
        let dir = scratch("shift");
        let [a1, _, b1, b2] = two_bodies(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        let order = |lib: &Library| -> Vec<String> {
            lib.rows_under_canonical(std::slice::from_ref(&dir), crate::Order::TAKEN)
                .unwrap()
                .iter()
                .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        let days = |lib: &Library| -> Vec<(String, usize)> {
            lib.facet_counts(crate::Facet::Date, None, &Filter::parse("").unwrap())
                .unwrap()
                .into_iter()
                .map(|c| (c.value, c.count))
                .collect()
        };
        let day = |d: &str| d.to_string();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        // By the clocks as they were set: the slow body's frames early.
        assert_eq!(order(&lib), ["b1.tif", "a1.tif", "b2.tif", "a2.tif"]);
        assert_eq!(days(&lib), [(day("2026-09-19"), 2), (day("2026-09-20"), 2)]);

        shift_by(&[b1.clone(), b2.clone()], 300);
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.meta_refreshed, 2, "{report:?}");
        assert_eq!(order(&lib), ["a1.tif", "b1.tif", "a2.tif", "b2.tif"]);
        // b1 crossed midnight: the 20th has three frames now.
        assert_eq!(days(&lib), [(day("2026-09-19"), 1), (day("2026-09-20"), 3)]);
        let mut on_20th = names(&lib, "date:2026-09-20");
        on_20th.sort();
        assert_eq!(on_20th, ["a2.tif", "b1.tif", "b2.tif"]);
        // The row's time is the shifted one; the EXIF's is kept.
        let e = lib.by_path(&b1).unwrap().unwrap();
        assert_eq!(e.exif.taken.as_deref(), Some("2026-09-19 23:56:00"));
        assert_eq!(e.taken_at.as_deref(), Some("2026-09-20 00:01:00"));
        assert_eq!(e.meta.time_shift, 300);
        // A collection's reference, shown from its hash, sorts by the
        // same time.
        let by_hash = lib.rows_by_hash(&e.hash).unwrap();
        assert_eq!(by_hash[0].taken.as_deref(), Some("2026-09-20 00:01:00"));
        let rows = lib
            .rows_under_canonical(std::slice::from_ref(&dir), crate::Order::TAKEN)
            .unwrap();
        let (_, row) = rows.iter().find(|(p, _)| p.ends_with("b1.tif")).unwrap();
        assert_eq!(row.taken.as_deref(), Some("2026-09-20 00:01:00"));
        assert_eq!(row.meta.time_shift, 300);
        // A frame with no shift reads its EXIF's time as it is.
        let e = lib.by_path(&a1).unwrap().unwrap();
        assert_eq!(e.taken_at, e.exif.taken);
        // The Rust order over the rows agrees with the SQL's.
        let mut keys: Vec<_> = rows.iter().map(|(p, r)| r.sort_key(p)).collect();
        keys.reverse();
        keys.sort_by(|a, b| crate::Order::TAKEN.compare(a, b));
        let by_rust: Vec<&Path> = keys.iter().map(|k| k.path).collect();
        let by_sql: Vec<&Path> = rows.iter().map(|(p, _)| p.as_path()).collect();
        assert_eq!(by_rust, by_sql);

        // Taken off: back to the clocks as they were set.
        shift_by(&[b1, b2], 0);
        lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(order(&lib), ["b1.tif", "a1.tif", "b2.tif", "a2.tif"]);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A checker of `contrast` either side of mid grey, `w` by `h`.
    fn checker(path: &Path, contrast: u16) {
        let (w, h) = (64u32, 48u32);
        let data: Vec<u16> = (0..w * h * 3)
            .map(|k| {
                let (x, y) = ((k / 3) % w, (k / 3) / w);
                if (x + y) % 2 == 0 {
                    30000 + contrast
                } else {
                    30000 - contrast
                }
            })
            .collect();
        crate::fixture::write_frame_of(path, &crate::fixture::R5, w, h, &data);
    }

    /// Score what the passes left NULL, every frame at once.
    fn score_all(lib: &mut Library) -> Report {
        crate::score::score(lib, 2, &|| false, &mut |_| {}).unwrap()
    }

    /// The `focus` column itself, NULL included.
    fn focus_column(lib: &Library, p: &Path) -> Option<f64> {
        lib.conn
            .query_row(
                "SELECT focus FROM files WHERE path = ?",
                params![path_bytes(p)],
                |r| r.get(0),
            )
            .unwrap()
    }

    /// A pass writes its rows unscored and reads no preview; the scorer
    /// after it scores each from its preview, and a raw with none is
    /// read once and left unscored. A schema 13 library comes up with
    /// the column NULL, the next pass reads nothing for it, and the
    /// scorer scores those rows.
    #[test]
    fn each_frame_is_scored_after_its_pass_and_a_schema_13_library_too() {
        let dir = scratch("focus");
        let db = dir.join("library.sqlite");
        let shoot = dir.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        let (hard, soft) = (shoot.join("hard.tif"), shoot.join("soft.tif"));
        checker(&hard, 24000);
        checker(&soft, 3000);
        let bare = shoot.join("bare.dng");
        crate::fixture::write_mosaic_dng(&bare, "RGGB");
        let focus = |lib: &Library, p: &Path| lib.by_path(p).unwrap().unwrap().exif.focus;
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&shoot, &mut quiet()).unwrap();
            assert_eq!(focus_column(&lib, &hard), None, "the pass scores nothing");
            assert_eq!(score_all(&mut lib).scored, 3);
            let (a, b) = (focus(&lib, &hard).unwrap(), focus(&lib, &soft).unwrap());
            assert!(a > b && (0.0..=1.0).contains(&a), "{a} against {b}");
            assert_eq!(focus(&lib, &bare), None, "no preview");
            assert!(focus_column(&lib, &bare).is_some_and(|v| v < 0.0));
            lib.index_folder(&shoot, &mut quiet()).unwrap();
            assert_eq!(
                score_all(&mut lib).scored,
                0,
                "the bare raw is not read again"
            );
            // A file changed is scored again.
            checker(&soft, 2000);
            let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
            std::fs::File::options()
                .write(true)
                .open(&soft)
                .unwrap()
                .set_modified(later)
                .unwrap();
            lib.index_folder(&shoot, &mut quiet()).unwrap();
            assert_eq!(focus(&lib, &soft), None);
            assert_eq!(score_all(&mut lib).scored, 1);
            lib.conn
                .execute_batch("ALTER TABLE files DROP COLUMN focus; PRAGMA user_version = 13;")
                .unwrap();
        }
        let mut lib = Library::open(&db).unwrap();
        let version: i32 = lib
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, crate::SCHEMA_VERSION);
        assert_eq!(focus(&lib, &hard), None, "not scored yet");
        let report = lib.index_folder(&shoot, &mut quiet()).unwrap();
        assert_eq!(report.unchanged, 3, "nothing read");
        assert_eq!(score_all(&mut lib).scored, 3);
        assert!(focus(&lib, &hard).unwrap() > focus(&lib, &soft).unwrap());
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// An ORF indexed before the style reader read Olympus's maker note
    /// has its tags read and no maker; a schema 14 library marks it
    /// unread, so the next pass over its folder reads its tags, and
    /// leaves every other row alone.
    #[test]
    fn a_schema_14_library_marks_its_orfs_unread() {
        let dir = scratch("migrate-14");
        let db = dir.join("library.sqlite");
        let shoot_dir = dir.join("shoot");
        std::fs::create_dir_all(&shoot_dir).unwrap();
        let (r5, r6, a7) = shoot(&shoot_dir);
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
            // As a schema 14 build left them, named as raws: an ORF of
            // each Make with no maker, and a Canon raw.
            for (path, name, make) in [
                (&r5, "a.ORF", "OM Digital Solutions"),
                (&r6, "b.ORF", "OLYMPUS IMAGING CORP."),
                (&a7, "c.CR3", "Canon"),
            ] {
                lib.conn
                    .execute(
                        "UPDATE files SET name = ?, make = ?, maker = NULL, style_read = 1 \
                         WHERE path = ?",
                        params![name, make, path_bytes(path)],
                    )
                    .unwrap();
            }
            lib.conn.execute_batch("PRAGMA user_version = 14;").unwrap();
        }
        let lib = Library::open(&db).unwrap();
        let version: i32 = lib
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, crate::SCHEMA_VERSION);
        assert_eq!(lib.styles_unread(None).unwrap(), 2);
        let unread: Vec<String> = lib
            .conn
            .prepare("SELECT name FROM files WHERE style_read = 0 ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(unread, ["a.ORF", "b.ORF"]);
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// An ORF a schema 15 build read before it knew Gradation Auto is
    /// held as no style; a schema 15 library marks every Olympus and
    /// OM System row with no style unread, so the next pass reads its
    /// tags again, and leaves every other maker's row alone. The step
    /// from 16 after it marks the one with a style too, for its camera
    /// frame.
    #[test]
    fn a_schema_15_library_marks_its_olympus_rows_unread() {
        let dir = scratch("migrate-15");
        let db = dir.join("library.sqlite");
        let shoot_dir = dir.join("shoot");
        std::fs::create_dir_all(&shoot_dir).unwrap();
        let (r5, r6, a7) = shoot(&shoot_dir);
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
            // As a schema 15 build left them: an Auto frame with its
            // maker and no style, an ORF of the older Make with a style,
            // a Canon raw with none, every one read.
            for (path, name, make, maker, style) in [
                (
                    &r5,
                    "a.ORF",
                    "OM Digital Solutions",
                    "OM Digital Solutions",
                    None,
                ),
                (
                    &r6,
                    "b.ORF",
                    "OLYMPUS IMAGING CORP.",
                    "Olympus",
                    Some("Olympus Vivid"),
                ),
                (&a7, "c.CR3", "Canon", "Canon", None),
            ] {
                lib.conn
                    .execute(
                        "UPDATE files SET name = ?, make = ?, maker = ?, style = ?, \
                         style_read = 1 WHERE path = ?",
                        params![name, make, maker, style, path_bytes(path)],
                    )
                    .unwrap();
            }
            lib.conn.execute_batch("PRAGMA user_version = 15;").unwrap();
        }
        let lib = Library::open(&db).unwrap();
        let version: i32 = lib
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, crate::SCHEMA_VERSION);
        assert_eq!(lib.styles_unread(None).unwrap(), 2);
        let unread: Vec<String> = lib
            .conn
            .prepare("SELECT name FROM files WHERE style_read = 0 ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(unread, ["a.ORF", "b.ORF"]);
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A row marked unread whose maker's tags will not read now keeps
    /// what it held: its style, its frame, and the mark, to be read at
    /// a later pass. Nothing it held is written over with nothing.
    #[test]
    fn a_row_whose_tags_will_not_read_again_keeps_its_style() {
        let dir = scratch("style-kept");
        let db = dir.join("library.sqlite");
        let shoot_dir = dir.join("shoot");
        std::fs::create_dir_all(&shoot_dir).unwrap();
        // A TIFF header whose first directory is past the end: the tags
        // will not read.
        let broken = shoot_dir.join("P.ORF");
        let mut bytes = b"II*\0\x00\x00\x00\x7f".to_vec();
        bytes.resize(4096, 0);
        std::fs::write(&broken, &bytes).unwrap();
        assert!(read_style(&broken).is_none(), "the read fails");
        // Read ahead (a share's pass), the failure is carried as a read
        // made, so the pass does not read the share a second time.
        let m = std::fs::metadata(&broken).unwrap();
        let hint = Hint {
            size: m.len(),
            mtime: mtime_of(&m),
            style_read: false,
            mosaic_read: true,
        };
        assert!(matches!(
            read_ahead(&broken, Some(hint), false).style,
            Some(None)
        ));
        let held = "8 8 5184 3888 8 224 5184 3456 1";
        let mut lib = Library::open(&db).unwrap();
        lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
        lib.conn
            .execute(
                "UPDATE files SET maker = 'Olympus', style = 'Olympus Vivid', style_read = 0, \
                 camera_frame = ? WHERE path = ?",
                params![held, path_bytes(&broken)],
            )
            .unwrap();
        lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
        let row: (Option<String>, Option<String>, Option<String>, i64) = lib
            .conn
            .query_row(
                "SELECT maker, style, camera_frame, style_read FROM files WHERE path = ?",
                params![path_bytes(&broken)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (
                Some("Olympus".into()),
                Some("Olympus Vivid".into()),
                Some(held.into()),
                0
            )
        );
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A schema 16 library gains the camera's frame: every Olympus and
    /// OM System row is marked unread, and the next pass reads its tags
    /// (the maker note, from the file's head) and fills the column, a
    /// frame shot at 3:2 with its frame and one at 4:3 with none. An
    /// edit with no crop on the 3:2 frame has its key taken anew with
    /// the frame in it. The step met again on a library that has the
    /// column adds nothing twice.
    #[test]
    fn a_schema_16_library_reads_the_cameras_frame_of_its_olympus_rows() {
        use greycard_core::raw::CameraFrame;
        use greycard_edit::Edit;
        let held = lens_name_held();
        let dir = scratch("migrate-16");
        let db = dir.join("library.sqlite");
        let shoot_dir = dir.join("shoot");
        std::fs::create_dir_all(&shoot_dir).unwrap();
        let area = [8, 8, 5184, 3888];
        let (framed, whole) = (shoot_dir.join("P1.ORF"), shoot_dir.join("P2.ORF"));
        let head = greycard_core::testing::olympus_head;
        std::fs::write(&framed, head(1, area, [0, 216, 5183, 3671])).unwrap();
        std::fs::write(&whole, head(1, area, [0, 0, 5183, 3887])).unwrap();
        let mut brightened = Edit::default();
        brightened.light.exposure = 0.7;
        Sidecar {
            line: greycard_edit::Line::of(brightened.clone(), 0).unwrap(),
            ..Sidecar::default()
        }
        .save(&framed)
        .unwrap();
        let unframed = greycard_edit_io::key::develop_key(&brightened, None);
        let row = |lib: &Library, p: &Path| -> (Option<String>, Option<i64>) {
            lib.conn
                .query_row(
                    "SELECT camera_frame, develop_key FROM files WHERE path = ?",
                    params![path_bytes(p)],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap()
        };
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
            // As a schema 16 build left them: read, a maker of theirs
            // (the head is no raw the decoder opens, so the make is set
            // here), no column, and the edit's key without the frame.
            lib.conn
                .execute(
                    "UPDATE files SET make = 'OM Digital Solutions', style_read = 1, \
                     develop_key = CASE WHEN develop_key IS NULL THEN NULL ELSE ? END",
                    params![unframed as i64],
                )
                .unwrap();
            lib.conn
                .execute_batch(
                    "ALTER TABLE files DROP COLUMN camera_frame; PRAGMA user_version = 16;",
                )
                .unwrap();
        }
        let mut lib = Library::open(&db).unwrap();
        let version: i32 = lib
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, crate::SCHEMA_VERSION);
        let unread: Vec<String> = lib
            .conn
            .prepare("SELECT name FROM files WHERE style_read = 0 ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(unread, ["P1.ORF", "P2.ORF"]);
        assert_eq!(row(&lib, &framed), (None, Some(unframed as i64)));
        lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
        let frame = greycard_core::decode::camera_frame_path(&framed)
            .unwrap()
            .expect("a 3:2 frame");
        let (text, key) = row(&lib, &framed);
        assert_eq!(
            text.as_deref().and_then(CameraFrame::from_text),
            Some(frame)
        );
        assert_eq!(
            key,
            Some(greycard_edit::key::framed_key(unframed, true, false, Some(&frame)) as i64),
            "the key taken anew with the frame"
        );
        assert_ne!(key, Some(unframed as i64));
        assert_eq!(row(&lib, &whole), (None, None), "4:3: no frame, no edit");
        // A library of this schema marked back to 16 comes up again,
        // the column kept.
        lib.conn.execute_batch("PRAGMA user_version = 16;").unwrap();
        drop(lib);
        let lib = Library::open(&db).unwrap();
        assert_eq!(row(&lib, &framed).0, text);
        drop(lib);
        drop(held);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A schema 17 library forgets its DNGs' focus scores and looks,
    /// made from the first preview listed, and keeps every other file's.
    #[test]
    fn a_schema_17_library_scores_its_dngs_again() {
        let dir = scratch("migrate-17");
        let db = dir.join("library.sqlite");
        let shoot = dir.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        for name in ["a.DNG", "b.dng", "c.CR3"] {
            std::fs::write(shoot.join(name), b"not a raw").unwrap();
        }
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&shoot, &mut quiet()).unwrap();
            lib.conn
                .execute_batch(
                    "UPDATE files SET focus = 0.5;
                     INSERT INTO looks (file, mtime, signature, focus)
                         SELECT id, mtime, x'00', 0.5 FROM files;
                     PRAGMA user_version = 17;",
                )
                .unwrap();
        }
        let lib = Library::open(&db).unwrap();
        let rows: Vec<(String, Option<f64>, bool)> = lib
            .conn
            .prepare(
                "SELECT name, focus, EXISTS (SELECT 1 FROM looks WHERE looks.file = files.id)
                 FROM files ORDER BY name",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            [
                ("a.DNG".to_string(), None, false),
                ("b.dng".to_string(), None, false),
                ("c.CR3".to_string(), Some(0.5), true),
            ]
        );
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// On a network mount only a CR3's or a RAF's preview is read, from
    /// the file's ends: a NEF stays unscored after the pass and the
    /// scorer, and a CR3 is read (this one, a picture under a raw's
    /// name, has no preview to find, and is stored so).
    #[test]
    fn on_a_network_mount_only_a_preview_from_the_ends_is_scored() {
        let dir = scratch("focus-far");
        let shoot = dir.join("share");
        std::fs::create_dir_all(&shoot).unwrap();
        let (nef, cr3, tif) = (
            shoot.join("a.NEF"),
            shoot.join("b.CR3"),
            shoot.join("c.tif"),
        );
        for p in [&nef, &cr3, &tif] {
            checker(p, 20000);
        }
        let mut lib = Library::open(&dir.join("library.sqlite")).unwrap();
        let on = shoot.clone();
        lib.set_network(move |p| p.starts_with(&on), Default::default());
        lib.index_folder(&shoot, &mut quiet()).unwrap();
        let report = score_all(&mut lib);
        assert_eq!(report.scored, 1, "{report:?}");
        assert_eq!(focus_column(&lib, &nef), None, "left for good");
        assert_eq!(focus_column(&lib, &tif), None, "a picture is read whole");
        assert!(focus_column(&lib, &cr3).is_some(), "read from its ends");
        // Asked again, nothing more.
        assert_eq!(score_all(&mut lib).scored, 0);
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A network read that does not answer holds its share: the next
    /// sweep asks it nothing, the reader not called again, and once the
    /// read comes back the sweep after scores the file.
    #[test]
    fn a_share_whose_score_read_hangs_is_asked_nothing_until_it_answers() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let dir = scratch("focus-hung");
        let shoot = dir.join("share").join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        let cr3 = shoot.join("a.CR3");
        checker(&cr3, 20000);
        let mut lib = Library::open(&dir.join("library.sqlite")).unwrap();
        let on = dir.join("share");
        lib.set_network(move |p| p.starts_with(&on), Default::default());
        lib.index_folder(&shoot, &mut quiet()).unwrap();
        let (held, calls) = (
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicUsize::new(0)),
        );
        let reader: crate::score::Reader = {
            let (held, calls) = (held.clone(), calls.clone());
            Arc::new(move |_: &Path| {
                calls.fetch_add(1, Ordering::SeqCst);
                while held.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Some(0.5)
            })
        };
        let sweep = |lib: &mut Library| {
            crate::score::score_with(
                lib,
                2,
                &|| false,
                &mut |_| {},
                &reader,
                Duration::from_millis(100),
            )
            .unwrap()
            .scored
        };
        assert_eq!(sweep(&mut lib), 0, "given up on");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(sweep(&mut lib), 0, "held");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the share asked nothing");
        // Back: the next sweep asks it again and scores the file.
        held.store(false, Ordering::SeqCst);
        let mut scored = 0;
        for _ in 0..200 {
            scored = sweep(&mut lib);
            if scored > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(scored, 1);
        assert_eq!(focus_column(&lib, &cr3), Some(0.5));
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A schema 11 library comes up with the shift's columns and its
    /// rows kept, and the next pass reads a shift a sidecar already
    /// carried.
    #[test]
    fn a_schema_11_library_is_brought_up_and_reads_the_shifts_already_there() {
        let dir = scratch("migrate-11");
        let db = dir.join("library.sqlite");
        let shoot_dir = dir.join("shoot");
        std::fs::create_dir_all(&shoot_dir).unwrap();
        let [_, _, b1, _] = two_bodies(&shoot_dir);
        shift_by(std::slice::from_ref(&b1), 300);
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
            // Back to schema 11, as an older build left it: the shift
            // never read.
            lib.conn_mut()
                .execute_batch(
                    "DROP INDEX files_taken_at;
                     ALTER TABLE files DROP COLUMN taken_at;
                     ALTER TABLE files DROP COLUMN shift; ALTER TABLE files DROP COLUMN focus;
                     PRAGMA user_version = 11;",
                )
                .unwrap();
        }
        let mut lib = Library::open(&db).unwrap();
        let version: i32 = lib
            .conn_mut()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, crate::SCHEMA_VERSION);
        assert_eq!(lib.len().unwrap(), 4);
        let e = lib.by_path(&b1).unwrap().unwrap();
        assert_eq!((e.meta.time_shift, e.taken_at.clone()), (0, e.exif.taken));
        let report = lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
        assert_eq!(report.added + report.changed, 0, "{report:?}");
        let e = lib.by_path(&b1).unwrap().unwrap();
        assert_eq!(e.meta.time_shift, 300);
        assert_eq!(e.taken_at.as_deref(), Some("2026-09-20 00:01:00"));
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The upgrade's pass over rows from before the column reads each
    /// file's description once, and stops between batches like the rest
    /// of a pass: rows past the stop stay unread, and the next pass
    /// takes them up.
    #[test]
    fn the_upgrades_mosaic_reads_stop_with_the_pass() {
        use crate::fixture::write_mosaic_dng;
        let dir = scratch("mosaic-upgrade");
        for i in 0..(BATCH + 10) {
            write_mosaic_dng(&dir.join(format!("f{i:03}.dng")), "RGGB");
        }
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        lib.conn
            .execute("UPDATE files SET mosaic = NULL", [])
            .unwrap();
        let unread = |lib: &Library| -> i64 {
            lib.conn
                .query_row("SELECT count(*) FROM files WHERE mosaic IS NULL", [], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        let first = lib
            .index_folder_until(&dir, &mut quiet(), &|| true)
            .unwrap();
        assert!(first.stopped, "{first:?}");
        assert_eq!(unread(&lib), 10, "a batch read, the rest left for later");
        lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(unread(&lib), 0);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A DNG whose CFA repeat is out of shape (65535 a side, or none)
    /// is a mosaic of no kind the engine takes, read without allocating
    /// the repeat and without an error; its EXIF stands. (A raw whose
    /// metadata reads and whose image description does not keeps its
    /// EXIF too: `decode::MosaicRead::Unknown`, tested in the engine,
    /// since no synthetic file breaks the one and not the other: a DNG
    /// whose raw IFD rawler cannot find fails its metadata as well.)
    #[test]
    fn a_dng_with_a_cfa_out_of_shape_is_other() {
        use crate::fixture::{patch_short_tag, write_mosaic_dng};
        use greycard_core::raw::Mosaic;
        let dir = scratch("mosaic-damaged");
        let (huge, zero) = (dir.join("huge.dng"), dir.join("zero.dng"));
        for p in [&huge, &zero] {
            write_mosaic_dng(p, "RGGB");
        }
        assert!(patch_short_tag(&huge, 33421, &[65535, 65535]) >= 1);
        assert!(patch_short_tag(&zero, 33421, &[0, 0]) >= 1);
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.added, 2, "{report:?}");
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        for p in [&huge, &zero] {
            assert_eq!(lib.by_path(p).unwrap().unwrap().exif.make, "Synthetic");
            assert_eq!(
                lib.mosaic(p).unwrap(),
                Some(FileMosaic::Mosaic(Mosaic::Other)),
                "{}",
                p.display()
            );
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// `!=` leaves a file that does not say out, on every kind of
    /// field: a JPEG with no EXIF is not "not ISO 100", it is
    /// unknown, and a listing of "everything but ISO 100" that
    /// carried it would surprise.
    #[test]
    fn not_equals_leaves_the_unknown_out_on_every_field() {
        let dir = scratch("unknown");
        shoot(&dir);
        // A picture with no EXIF at all.
        let bare = dir.join("bare.png");
        image::RgbImage::new(2, 2).save(&bare).unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.added, 4, "{report:?}");
        // Nothing of the EXIF; a picture is no mosaic.
        assert_eq!(
            lib.by_path(&bare).unwrap().unwrap().exif,
            Exif {
                mosaic: Some(FileMosaic::NotMosaic),
                ..Exif::default()
            }
        );
        assert_eq!(names(&lib, "iso!=100"), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "aperture!=2.8"), ["r5.tif", "r6.tif"]);
        assert_eq!(names(&lib, "focal!=50"), ["a7.tif", "r5.tif"]);
        assert_eq!(names(&lib, "date!=2024"), ["a7.tif", "r6.tif"]);
        assert_eq!(
            names(&lib, "lens!=\"RF50mm F1.8 STM\""),
            ["a7.tif", "r5.tif"]
        );
        // An empty camera is an empty string, which `make!=canon`
        // could see as "not Canon"; it is unknown and stays out.
        assert_eq!(names(&lib, "make!=canon"), ["a7.tif"]);
        assert_eq!(
            names(&lib, "camera!=\"Canon EOS R5\""),
            ["a7.tif", "r6.tif"]
        );
        // No keywords is known, and `keyword!=` keeps such a file.
        assert_eq!(
            names(&lib, "keyword!=wedding"),
            ["a7.tif", "bare.png", "r6.tif"]
        );
        // A chip held out is the other way: "everything but the R5"
        // keeps what has no camera, or no ISO, to say.
        let held_out = |facet, value: &str| -> Vec<String> {
            let filter = Filter {
                terms: vec![crate::filter::Term::NotFacet {
                    facet,
                    values: vec![value.to_string()],
                }],
            };
            lib.query(&filter)
                .unwrap()
                .iter()
                .map(Entry::name)
                .collect()
        };
        use crate::filter::Facet;
        assert_eq!(
            held_out(Facet::Camera, "Canon EOS R5"),
            ["a7.tif", "bare.png", "r6.tif"]
        );
        assert_eq!(
            held_out(Facet::Iso, "100"),
            ["a7.tif", "bare.png", "r6.tif"]
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The browser's box lowercases both sides with Unicode rules;
    /// SQLite's LIKE folds ASCII only. The index folds the same way
    /// the browser does.
    #[test]
    fn a_name_is_found_whatever_its_case_in_any_script() {
        let dir = scratch("unicode");
        let odd = dir.join("Ärger_50%.tif");
        write_frame(&odd, &R5, 4);
        let plain = dir.join("plain.tif");
        write_frame(&plain, &R6, 5);
        let mut s = Sidecar::default();
        s.meta.set_keywords(vec!["Straße".into(), "ÉTÉ".into()]);
        s.save(&plain).unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        for word in ["Ärger", "ärger", "ÄRGER", "rger_50%", "Ärger_50%.tif"] {
            assert_eq!(names(&lib, word), ["Ärger_50%.tif"], "{word}");
            assert_eq!(
                names(&lib, &format!("name:{word}")),
                ["Ärger_50%.tif"],
                "name:{word}"
            );
        }
        assert_eq!(names(&lib, "name=\"ärger_50%.TIF\""), ["Ärger_50%.tif"]);
        assert_eq!(names(&lib, "name!=\"ärger_50%.TIF\""), ["plain.tif"]);
        for word in ["straße", "STRASSE", "été", "Été"] {
            let want: Vec<&str> = if word == "STRASSE" {
                // ß has no single-character upper case; a search
                // for the two-letter spelling does not find it, as
                // the browser's box does not either.
                vec![]
            } else {
                vec!["plain.tif"]
            };
            assert_eq!(names(&lib, word), want, "{word}");
            assert_eq!(
                names(&lib, &format!("keyword:{word}")),
                want,
                "keyword:{word}"
            );
        }
        assert_eq!(names(&lib, "keyword=été"), ["plain.tif"]);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_changed_sidecar_refreshes_the_meta_and_nothing_else() {
        let dir = scratch("meta");
        let (r5, _r6, a7) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let before = lib.by_path(&r5).unwrap().unwrap();

        // The R5 loses a star; the A7 gains a sidecar.
        let mut s = Sidecar::load_as_file(&r5).unwrap().unwrap();
        s.meta.rating = 3;
        s.save(&r5).unwrap();
        let mut s = Sidecar::default();
        s.meta.set_keywords(vec!["sea".into()]);
        s.save(&a7).unwrap();

        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(
            report,
            Report {
                meta_refreshed: 2,
                unchanged: 1,
                ..Report::default()
            }
        );
        let after = lib.by_path(&r5).unwrap().unwrap();
        assert_eq!(after.meta.rating, 3);
        assert_eq!(after.meta.flag, Flag::Pick);
        assert_eq!(after.id, before.id);
        assert_eq!(after.hash, before.hash);
        assert_eq!(after.exif, before.exif);
        assert_eq!(names(&lib, "keyword:sea"), ["a7.tif"]);

        // The sidecar taken away: the meta goes with it.
        std::fs::remove_file(Sidecar::find(&a7).unwrap()).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.meta_refreshed, 1, "{report:?}");
        assert!(names(&lib, "keyword:sea").is_empty());
        assert_eq!(lib.by_path(&a7).unwrap().unwrap().sidecar, None);

        // One file on its own, after a save: the same rules.
        let mut s = Sidecar::load_as_file(&r5).unwrap().unwrap();
        s.meta.rating = 5;
        s.save(&r5).unwrap();
        let report = lib.index_file(&r5).unwrap();
        assert_eq!(report.meta_refreshed, 1, "{report:?}");
        assert_eq!(lib.by_path(&r5).unwrap().unwrap().meta.rating, 5);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A write that changes one digit changes neither the sidecar's
    /// length nor, within one filesystem tick, its mtime. The hash
    /// of its bytes still sees it. The digit is changed in place: a
    /// save by this build adds a revision line (§233) and so grows
    /// the file, but a build from before, or a hand, does not.
    #[test]
    fn a_same_size_save_in_the_same_tick_is_still_seen() {
        let dir = scratch("tick");
        let (r5, _, _) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let gcd = Sidecar::find(&r5).unwrap();
        let before = std::fs::metadata(&gcd).unwrap();
        let text = std::fs::read_to_string(&gcd).unwrap();
        assert!(text.contains("\"rating\": 4"), "{text}");
        std::fs::write(&gcd, text.replacen("\"rating\": 4", "\"rating\": 3", 1)).unwrap();
        // The same mtime as before, to the nanosecond, and the same
        // length.
        std::fs::File::options()
            .write(true)
            .open(&gcd)
            .unwrap()
            .set_modified(before.modified().unwrap())
            .unwrap();
        let after = std::fs::metadata(&gcd).unwrap();
        assert_eq!(after.len(), before.len());
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
        let report = lib.index_file(&r5).unwrap();
        assert_eq!(report.meta_refreshed, 1, "{report:?}");
        assert_eq!(lib.by_path(&r5).unwrap().unwrap().meta.rating, 3);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_file_changed_on_disk_is_read_again() {
        let dir = scratch("changed");
        let (r5, _, _) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let before = lib.by_path(&r5).unwrap().unwrap();
        // Overwritten by another frame, a second later.
        write_frame(&r5, &A7, 9);
        std::fs::File::options()
            .write(true)
            .open(&r5)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        let after = lib.by_path(&r5).unwrap().unwrap();
        assert_eq!(after.id, before.id);
        assert_ne!(after.hash, before.hash);
        assert_eq!(after.exif.camera, "SONY ILCE-7M4");
        // The sidecar beside it still counts.
        assert_eq!(after.meta.rating, 4);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_renamed_file_is_a_move_and_keeps_its_row() {
        let dir = scratch("move");
        let (r5, _r6, a7) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let before = lib.by_path(&r5).unwrap().unwrap();

        // Renamed within the folder, the sidecar with it.
        let renamed = dir.join("z_renamed.tif");
        let was = canonical(&r5).unwrap();
        std::fs::rename(&r5, &renamed).unwrap();
        std::fs::rename(Sidecar::path_for(&r5), Sidecar::path_for(&renamed)).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(
            report,
            Report {
                moved: 1,
                moved_files: vec![(was, canonical(&renamed).unwrap())],
                unchanged: 2,
                ..Report::default()
            }
        );
        assert_eq!(lib.len().unwrap(), 3);
        let after = lib
            .by_path(&renamed)
            .unwrap()
            .expect("the row at the new path");
        assert_eq!(after.id, before.id);
        assert_eq!(after.hash, before.hash);
        assert_eq!(after.exif, before.exif);
        assert_eq!(after.meta.rating, 4);
        assert!(lib.by_path(&r5).unwrap().is_none());
        let found = lib.by_hash(&before.hash).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, canonical(&renamed).unwrap());
        assert!(!found[0].missing);

        // Moved to a folder indexed later: the old folder marks it
        // missing, the new one claims it.
        let sub = dir.join("picks");
        std::fs::create_dir(&sub).unwrap();
        let moved = sub.join("a7.tif");
        let was = canonical(&a7).unwrap();
        std::fs::rename(&a7, &moved).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.missing, 1, "{report:?}");
        assert!(lib.by_path(&moved).unwrap().is_none());
        assert!(names(&lib, "").iter().all(|n| n != "a7.tif"));
        assert_eq!(names(&lib, "missing:yes"), ["a7.tif"]);
        let report = lib.index_folder(&sub, &mut quiet()).unwrap();
        assert_eq!(report.moved, 1, "{report:?}");
        assert_eq!(
            report.moved_files,
            [(was, canonical(&moved).unwrap())],
            "both paths as the index keys them"
        );
        assert_eq!(report.added, 0);
        assert_eq!(lib.len().unwrap(), 3);
        let e = lib.by_path(&moved).unwrap().unwrap();
        assert_eq!(e.exif.camera, "SONY ILCE-7M4");
        assert!(!e.missing);
        assert!(names(&lib, "missing:yes").is_empty());
        // And the old folder, indexed again, has nothing to say.
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.missing, 0, "{report:?}");
        assert_eq!(report.unchanged, 2);

        // Moved the other way round: the new folder indexed first
        // finds the old path gone and claims the row before the old
        // folder ever notices. The old folder keeps another file, so
        // that it is a folder in use and not an empty mount point.
        write_frame(&sub.join("keep.tif"), &R6, 21);
        assert_eq!(lib.index_folder(&sub, &mut quiet()).unwrap().added, 1);
        let back = dir.join("a7.tif");
        std::fs::rename(&moved, &back).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.moved, 1, "{report:?}");
        let report = lib.index_folder(&sub, &mut quiet()).unwrap();
        assert_eq!(report.unchanged, 1, "{report:?}");
        assert_eq!(report.missing, 0);
        assert_eq!(lib.len().unwrap(), 4);

        // A copy is not a move: both files are there, so both rows.
        let copy = dir.join("copy.tif");
        std::fs::copy(&back, &copy).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.added, 1, "{report:?}");
        assert_eq!(lib.by_hash(&e.hash).unwrap().len(), 2);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A raw dragged by hand to another folder, or renamed, leaves its
    /// sidecar behind: the pass that sees the move carries the sidecar
    /// to the new place in the placement it had, and the row keeps the
    /// edits and the flag.
    #[test]
    fn a_hand_moved_raw_has_its_sidecar_carried_along() {
        let dir = scratch("carry");
        let (r5, r6, a7) = shoot(&dir);
        let picks = dir.join("picks");
        std::fs::create_dir(&picks).unwrap();
        // A frame in each folder, so neither is ever empty.
        write_frame(&picks.join("keep.tif"), &A7, 21);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&dir, &mut quiet()).unwrap();

        // The raws alone, one with a sidecar beside it and one with
        // its sidecar under the hidden folder.
        let (r5_to, r6_to) = (picks.join("r5.tif"), picks.join("r6.tif"));
        std::fs::rename(&r5, &r5_to).unwrap();
        std::fs::rename(&r6, &r6_to).unwrap();
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert_eq!(
            (report.moved, report.carried, report.added),
            (2, 2, 0),
            "{report:?}"
        );
        assert_eq!(Sidecar::find(&r5_to), Some(Sidecar::path_for(&r5_to)));
        assert_eq!(
            Sidecar::find(&r6_to),
            Some(Sidecar::path_in(&r6_to, Placement::Folder))
        );
        assert!(Sidecar::find(&r5).is_none(), "nothing left behind");
        assert!(Sidecar::find(&r6).is_none(), "nothing left behind");
        let row = lib.by_path(&r5_to).unwrap().unwrap();
        assert_eq!((row.meta.rating, row.meta.flag), (4, Flag::Pick));
        let row = lib.by_path(&r6_to).unwrap().unwrap();
        assert_eq!((row.meta.rating, row.meta.label), (2, Label::Red));

        // Renamed within the folder, the raw alone: the same, and what
        // saves put aside beside the sidecar goes along under the new
        // name, none left for a later frame of the old one.
        for aside in ["r5.tif.gcd.other", "r5.tif.gcd.3.other", "r5.tif.gcd.bad"] {
            std::fs::write(picks.join(aside), aside).unwrap();
        }
        let renamed = picks.join("z.tif");
        std::fs::rename(&r5_to, &renamed).unwrap();
        let report = lib.index_folder(&picks, &mut quiet()).unwrap();
        assert_eq!((report.moved, report.carried), (1, 1), "{report:?}");
        assert_eq!(Sidecar::find(&renamed), Some(Sidecar::path_for(&renamed)));
        assert!(Sidecar::find(&r5_to).is_none());
        for (from, to) in [
            ("r5.tif.gcd.other", "z.tif.gcd.other"),
            ("r5.tif.gcd.3.other", "z.tif.gcd.3.other"),
            ("r5.tif.gcd.bad", "z.tif.gcd.bad"),
        ] {
            assert_eq!(std::fs::read_to_string(picks.join(to)).unwrap(), from);
            assert!(!picks.join(from).exists(), "{from} left behind");
        }
        assert_eq!(lib.by_path(&renamed).unwrap().unwrap().meta.rating, 4);

        // A frame with no sidecar moves as before.
        let a7_to = picks.join("a7.tif");
        std::fs::rename(&a7, &a7_to).unwrap();
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert_eq!((report.moved, report.carried), (1, 0), "{report:?}");
        assert!(Sidecar::find(&a7_to).is_none());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A sidecar at the new place already is the frame's now: the one
    /// left behind stays where it is, and the row reads the new one.
    #[test]
    fn a_sidecar_at_the_new_place_is_not_written_over() {
        let dir = scratch("carry-taken");
        let (r5, _r6, _a7) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let renamed = dir.join("z.tif");
        std::fs::rename(&r5, &renamed).unwrap();
        let mut s = Sidecar::default();
        s.meta.rating = 1;
        s.save(&renamed).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!((report.moved, report.carried), (1, 0), "{report:?}");
        assert!(Sidecar::path_for(&r5).is_file(), "left where it was");
        assert_eq!(lib.by_path(&renamed).unwrap().unwrap().meta.rating, 1);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A folder of raws moved by hand, each with its sidecar and what
    /// saves put aside beside it: the old folder is listed once a
    /// batch for the asides, not once a frame, and each frame's go
    /// with it under its new name.
    #[test]
    fn a_moved_folder_is_listed_once_for_its_asides() {
        let dir = scratch("carry-listed");
        let (from, to) = (dir.join("from"), dir.join("to"));
        std::fs::create_dir(&from).unwrap();
        std::fs::create_dir(&to).unwrap();
        // A frame in each folder, so neither is ever empty.
        write_frame(&from.join("keep.tif"), &A7, 900);
        write_frame(&to.join("keep.tif"), &A7, 901);
        let frames = 8;
        let name = |i: usize| format!("f{i}.tif");
        for i in 0..frames {
            let raw = from.join(name(i));
            write_frame(&raw, &R5, 100 + i as u16);
            let mut s = Sidecar::default();
            s.meta.rating = 3;
            s.save(&raw).unwrap();
            std::fs::write(from.join(format!("{}.gcd.other", name(i))), name(i)).unwrap();
        }
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&from, &mut quiet()).unwrap();
        lib.index_folder(&to, &mut quiet()).unwrap();
        for i in 0..frames {
            std::fs::rename(from.join(name(i)), to.join(name(i))).unwrap();
        }
        LISTED.with(|l| l.set(0));
        let report = lib.index_folder(&to, &mut quiet()).unwrap();
        assert_eq!(
            (report.moved, report.carried),
            (frames, frames),
            "{report:?}"
        );
        let listed = LISTED.with(|l| l.get());
        // One a batch: a slow machine may cut the pass by time.
        assert!(listed < frames, "the old folder listed {listed} times");
        for i in 0..frames {
            let other = format!("{}.gcd.other", name(i));
            assert_eq!(std::fs::read_to_string(to.join(&other)).unwrap(), name(i));
            assert!(!from.join(&other).exists(), "{other} left behind");
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The batch's listing follows its own moves: a sidecar carried
    /// into a folder already listed, then on out of it, takes its
    /// asides along, and each listing held reads as a fresh one.
    #[test]
    fn a_batch_listing_follows_its_own_moves() {
        let dir = scratch("carry-listing");
        let (a, b, c) = (dir.join("a"), dir.join("b"), dir.join("c"));
        for d in [&a, &b, &c] {
            std::fs::create_dir(d).unwrap();
        }
        for (folder, sidecar) in [(&a, "x.tif.gcd"), (&b, "w.tif.gcd")] {
            std::fs::write(folder.join(sidecar), sidecar).unwrap();
            let other = format!("{sidecar}.other");
            std::fs::write(folder.join(&other), &other).unwrap();
        }
        let mut listings = Listings::default();
        // `b` listed, then `x` carried into it, then on to `c`.
        assert!(carry_sidecar(
            &b.join("w.tif"),
            &c.join("w.tif"),
            &mut listings
        ));
        assert!(carry_sidecar(
            &a.join("x.tif"),
            &b.join("y.tif"),
            &mut listings
        ));
        assert!(carry_sidecar(
            &b.join("y.tif"),
            &c.join("z.tif"),
            &mut listings
        ));
        assert_eq!(
            std::fs::read_to_string(c.join("z.tif.gcd.other")).unwrap(),
            "x.tif.gcd.other"
        );
        assert!(!b.join("y.tif.gcd.other").exists(), "left behind");
        let fresh = |folder: &Path| {
            let mut names: Vec<OsString> = std::fs::read_dir(folder)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            names.sort();
            names
        };
        for (folder, names) in &listings.0 {
            let mut held = names.clone();
            held.sort();
            assert_eq!(held, fresh(folder), "{}", folder.display());
        }
        assert_eq!(listings.0.len(), 2, "a and b listed, once each");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A row whose whole folder is gone is not a move's other end:
    /// the folder is a drive not mounted, as likely as not, and a
    /// copy of its file on the laptop must not take its row. When
    /// the drive is back its files are still where the index says.
    #[test]
    fn a_copy_from_an_unmounted_drive_is_not_a_move() {
        let dir = scratch("drive");
        let drive = dir.join("drive");
        let day = drive.join("day1");
        std::fs::create_dir_all(&day).unwrap();
        let (r5, _, _) = shoot(&day);
        let laptop = dir.join("laptop");
        std::fs::create_dir(&laptop).unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&drive, &mut quiet()).unwrap();
        let on_drive = lib.by_path(&r5).unwrap().unwrap();
        // The copy made, then the drive unmounted.
        std::fs::copy(&r5, laptop.join("r5.tif")).unwrap();
        let off = dir.join("drive.off");
        std::fs::rename(&drive, &off).unwrap();
        let report = lib.index_folder(&laptop, &mut quiet()).unwrap();
        assert_eq!(report.added, 1, "{report:?}");
        assert_eq!(report.moved, 0);
        let same = lib.by_hash(&on_drive.hash).unwrap();
        assert_eq!(same.len(), 2);
        assert!(same.iter().any(|e| e.path == on_drive.path && !e.missing));
        // The drive back: its file is unchanged, not new.
        std::fs::rename(&off, &drive).unwrap();
        let report = lib.index_tree(&drive, &mut quiet()).unwrap();
        assert_eq!(report.added, 0, "{report:?}");
        assert_eq!(report.unchanged, 3);
        assert_eq!(lib.len().unwrap(), 4);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_file_gone_is_missing_until_pruned_or_back() {
        let dir = scratch("missing");
        let (r5, _, _) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let bytes = std::fs::read(&r5).unwrap();
        std::fs::remove_file(&r5).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.missing, 1, "{report:?}");
        assert_eq!(names(&lib, ""), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "missing:yes"), ["r5.tif"]);
        assert_eq!(names(&lib, "missing:no"), ["a7.tif", "r6.tif"]);
        assert!(lib.by_path(&r5).unwrap().unwrap().missing);
        // Still missing next time, and not counted twice.
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.missing, 0, "{report:?}");
        // Back where it was: found, and its row is the same row.
        let id = lib.by_path(&r5).unwrap().unwrap().id;
        std::fs::write(&r5, &bytes).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert!(report.returned == 1 || report.changed == 1, "{report:?}");
        assert_eq!(report.added, 0);
        let e = lib.by_path(&r5).unwrap().unwrap();
        assert_eq!(e.id, id);
        assert!(!e.missing);
        // Gone for good, and pruned.
        std::fs::remove_file(&r5).unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(lib.prune_missing().unwrap(), 1);
        assert_eq!(lib.len().unwrap(), 2);
        assert!(lib.by_path(&r5).unwrap().is_none());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A delete the editor made: the rows go at once, keywords and
    /// all, by the path as it was handed over, and the neighbors stay.
    #[test]
    fn a_deleted_file_is_forgotten_at_once() {
        let dir = scratch("forget");
        let (r5, r6, a7) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(lib.len().unwrap(), 3);
        assert_eq!(names(&lib, "keyword:Harbor"), ["r5.tif"]);
        std::fs::remove_file(&r5).unwrap();
        // Gone from disk already, and a path never indexed: one row.
        assert_eq!(lib.forget(&[r5.clone(), dir.join("never.tif")]).unwrap(), 1);
        assert!(lib.by_path(&r5).unwrap().is_none());
        assert!(lib.by_path(&r6).unwrap().is_some());
        assert!(lib.by_path(&a7).unwrap().is_some());
        assert!(names(&lib, "missing:yes").is_empty());
        assert!(names(&lib, "keyword:Harbor").is_empty());
        // A second forget finds nothing, and a pass over the folder
        // marks nothing missing.
        assert_eq!(lib.forget(std::slice::from_ref(&r5)).unwrap(), 0);
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.missing, 0, "{report:?}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A move the editor made, told to the index: the row goes to the
    /// new path with its id, its hash and its keywords, found again if
    /// it was missing, even out of a folder the move emptied, which a
    /// pass takes for a drive that is away. A stale row at the new path
    /// goes; a path with no row moves nothing.
    #[test]
    fn a_move_the_editor_made_takes_the_row_along() {
        let dir = scratch("told-move");
        let rejects = dir.join("rejects");
        std::fs::create_dir_all(&rejects).unwrap();
        let (r5, r6, _a7) = shoot(&dir);
        let from = rejects.join("r5.tif");
        std::fs::rename(&r5, &from).unwrap();
        std::fs::rename(Sidecar::path_for(&r5), Sidecar::path_for(&from)).unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        lib.index_folder(&rejects, &mut quiet()).unwrap();
        let before = lib.by_path(&from).unwrap().unwrap();
        // A row at the new path from a file once there, gone since:
        // r6 renamed away and its row marked missing.
        let stale = dir.join("r6-gone.tif");
        std::fs::rename(&r6, &stale).unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        std::fs::remove_file(&stale).unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        assert!(lib.by_path(&stale).unwrap().unwrap().missing);
        let to = stale;
        std::fs::rename(&from, &to).unwrap();

        assert!(lib.file_moved(&from, &to).unwrap());
        let after = lib.by_path(&to).unwrap().unwrap();
        assert_eq!(after.id, before.id);
        assert_eq!(after.hash, before.hash);
        assert!(!after.missing);
        assert_eq!(after.name(), "r6-gone.tif");
        assert!(lib.by_path(&from).unwrap().is_none());
        assert_eq!(names(&lib, "keyword:Harbor"), ["r6-gone.tif"]);
        // The emptied folder's pass and the new one's agree: nothing
        // added, nothing missing.
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!((report.added, report.missing), (0, 0), "{report:?}");
        lib.index_folder(&rejects, &mut quiet()).unwrap();
        assert!(names(&lib, "missing:yes").is_empty());
        assert!(!lib.file_moved(&from, &to).unwrap(), "nothing left at from");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A folder deleted with its files: a pass over the tree above
    /// it marks them missing, and so does a pass over the folder
    /// itself, which is not an error while its parent is there.
    #[test]
    fn a_deleted_folder_marks_its_files_missing() {
        let dir = scratch("gone");
        let day1 = dir.join("day1");
        let day2 = dir.join("day2");
        std::fs::create_dir_all(&day1).unwrap();
        std::fs::create_dir_all(&day2).unwrap();
        shoot(&day1);
        write_frame(&day2.join("x.tif"), &A7, 7);
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert_eq!(report.added, 4, "{report:?}");

        greycard_core::testing::remove_dir_retry(&day1);
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert_eq!(report.missing, 3, "{report:?}");
        assert_eq!(names(&lib, ""), ["x.tif"]);
        assert_eq!(names(&lib, "missing:yes").len(), 3);
        // Not marked twice.
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert_eq!(report.missing, 0, "{report:?}");
        assert_eq!(lib.prune_missing().unwrap(), 3);
        assert_eq!(lib.len().unwrap(), 1);

        // The folder on its own.
        greycard_core::testing::remove_dir_retry(&day2);
        let report = lib.index_folder(&day2, &mut quiet()).unwrap();
        assert_eq!(report.missing, 1, "{report:?}");
        assert!(lib.by_path(&day2.join("x.tif")).unwrap().unwrap().missing);
        // A folder whose parent is gone too is a drive gone, and
        // an error rather than a marking.
        assert!(
            lib.index_folder(&dir.join("nowhere").join("deep"), &mut quiet())
                .is_err()
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// macOS's `._` resource forks and `.DS_Store` beside a raw on a
    /// share are no rows, and give the raw no companion.
    #[test]
    fn hidden_files_beside_a_raw_are_not_rows_or_companions() {
        let dir = scratch("hidden");
        let raw = dir.join("IMG.CR3");
        std::fs::write(&raw, b"not a raw").unwrap();
        for name in ["._IMG.CR3", "._IMG.xmp", "._IMG.JPG", ".DS_Store"] {
            std::fs::write(dir.join(name), b"mac").unwrap();
        }
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!((report.seen(), report.added), (1, 1), "{report:?}");
        let rows: Vec<String> = lib
            .query(&Filter::default())
            .unwrap()
            .iter()
            .map(Entry::name)
            .collect();
        assert_eq!(rows, ["IMG.CR3"]);
        let paired = crate::pairs::pair(list_folder(&dir).unwrap(), false);
        assert_eq!(paired.frames, std::slice::from_ref(&raw));
        assert!(paired.companions.is_empty());
        assert!(crate::pairs::companions_of(&[raw]).is_empty());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Rows an index made of hidden files before the listing passed
    /// over them go on the next pass, there or not, missing or not,
    /// rather than wait under missing for Forget missing.
    #[test]
    fn old_rows_of_hidden_files_go_on_the_next_pass() {
        let dir = scratch("hidden-rows");
        for name in ["IMG.CR3", "A.CR3", "B.CR3"] {
            std::fs::write(dir.join(name), b"not a raw").unwrap();
        }
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        // As an older build left them: `._A.CR3` there, `._B.CR3` gone
        // and already marked missing.
        let folder = canonical(&dir).unwrap();
        for (from, to, missing) in [("A.CR3", "._A.CR3", None), ("B.CR3", "._B.CR3", Some(1))] {
            std::fs::rename(dir.join(from), dir.join(to)).unwrap();
            lib.conn_mut()
                .execute(
                    "UPDATE files SET path = ?, name = ?, missing_since = ? WHERE path = ?",
                    params![
                        path_bytes(&folder.join(to)),
                        to,
                        missing,
                        path_bytes(&folder.join(from))
                    ],
                )
                .unwrap();
        }
        std::fs::remove_file(dir.join("._B.CR3")).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.missing, 0, "{report:?}");
        let names: Vec<String> = lib
            .conn_mut()
            .prepare("SELECT name FROM files ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(names, ["IMG.CR3"]);
        // Nor does one named on its own come back.
        lib.index_file(&dir.join("._A.CR3")).unwrap();
        assert_eq!(lib.len().unwrap(), 1);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_file_that_will_not_probe_is_still_a_row() {
        let dir = scratch("bad");
        let bad = dir.join("IMG_0001.CR3");
        std::fs::write(&bad, b"not a raw at all").unwrap();
        let mut s = Sidecar::default();
        s.meta.rating = 1;
        s.save(&bad).unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.added, 1);
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert_eq!(report.errors[0].0, canonical(&bad).unwrap());
        let e = lib.by_path(&bad).unwrap().unwrap();
        assert_eq!(e.exif, Exif::default());
        assert_eq!(e.meta.rating, 1);
        assert_eq!(names(&lib, "rating:1"), ["IMG_0001.CR3"]);
        // Not retried while the file stands.
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert!(report.errors.is_empty());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A decoder that panics on a file's bytes costs that file its
    /// EXIF and nothing else: the rest of the folder is indexed and
    /// the panic is a line in the report. The bytes here are a TIFF
    /// header pointing its directory past the end of the file,
    /// which is the kind of thing a decoder trips on.
    #[test]
    fn a_panic_in_the_probe_is_one_files_error() {
        let dir = scratch("panic");
        let (_r5, _, _) = shoot(&dir);
        // The first 8,192 bytes of the sample P1000247.RW2 with
        // fifteen bytes changed by a fuzz pass (offsets 15, 42, 54,
        // 59, 102, 114, 122, 123, 167, 185, 190, 201, 206, 222, 245),
        // the body's and the lens's serial numbers zeroed and every
        // byte past 6,150 zeroed (the embedded preview starts at
        // 6,144, and this repo holds nobody's picture), on which
        // rawler 0.8's RW2 decoder divides by zero in `rw2.rs:76`; a
        // 6,144-byte cut does not reach that line. In the folder
        // with good files: the folder is indexed, the file has a row,
        // the panic is one line.
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rawler-rw2-divide-by-zero.RW2");
        let bad = dir.join("P1000247.RW2");
        std::fs::copy(&fixture, &bad).unwrap();
        // Quiet the panic's own print: the test is that it is caught.
        let before = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&dir, &mut quiet());
        std::panic::set_hook(before);
        let report = report.unwrap();
        assert_eq!(report.added, 4, "{report:?}");
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert_eq!(report.errors[0].0, canonical(&bad).unwrap());
        assert!(
            report.errors[0].1.contains("gave up"),
            "{}",
            report.errors[0].1
        );
        assert_eq!(lib.len().unwrap(), 4);
        assert_eq!(names(&lib, "camera:canon").len(), 2);
        let e = lib.by_path(&bad).unwrap().unwrap();
        assert_eq!(e.exif, Exif::default());
        assert_eq!(e.hash, hash::hash_file(&bad).unwrap());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A walk leaving out a folder below its root (a share mounted there,
    /// passed over as its own) neither goes into it nor marks its rows,
    /// and walks the rest.
    #[test]
    fn a_walk_leaves_out_the_folders_it_is_told() {
        let dir = scratch("left-out");
        let (day, nas) = (dir.join("day"), dir.join("nas"));
        std::fs::create_dir_all(nas.join("2026")).unwrap();
        std::fs::create_dir_all(&day).unwrap();
        write_frame(&day.join("a.tif"), &R5, 1);
        write_frame(&nas.join("b.tif"), &R6, 2);
        write_frame(&nas.join("2026").join("c.tif"), &A7, 3);
        let mut lib = Library::open_in_memory().unwrap();
        // Indexed whole once, as before the share was found below it.
        lib.index_tree(&dir, &mut quiet()).unwrap();
        assert_eq!(lib.len().unwrap(), 3);
        // Then the share's folders are not there to be read (the share
        // gone): a walk told to leave them out marks nothing, and finds
        // a new file outside them.
        // Out of the root altogether, so no walk finds it elsewhere.
        let gone = dir.with_extension("gone-nas");
        std::fs::rename(&nas, &gone).unwrap();
        std::fs::create_dir_all(&nas).unwrap();
        write_frame(&day.join("d.tif"), &R5, 4);
        let mut walk = lib
            .tree_walk(&dir)
            .unwrap()
            .leaving_out(std::slice::from_ref(&nas));
        let report = lib.walk_until(&mut walk, &mut quiet(), &|| false).unwrap();
        assert_eq!(report.added, 1, "{report:?}");
        assert_eq!(
            report.missing, 0,
            "the share's rows left as they were: {report:?}"
        );
        let rows = lib.query(&Filter::parse("").unwrap()).unwrap();
        assert_eq!(rows.iter().filter(|e| !e.missing).count(), 4);
        // Without it, and the mount point gone too, the same walk marks
        // the share's two missing.
        std::fs::remove_dir(&nas).unwrap();
        let mut walk = lib.tree_walk(&dir).unwrap();
        let report = lib.walk_until(&mut walk, &mut quiet(), &|| false).unwrap();
        assert_eq!(report.missing, 2, "{report:?}");
        greycard_core::testing::remove_dir_retry(&dir);
        greycard_core::testing::remove_dir_retry(&gone);
    }

    /// A walk stopped partway and taken up again with a folder now left
    /// out (a share found below the root meanwhile) does not go into it.
    #[test]
    fn a_walk_taken_up_again_leaves_out_what_is_left_out_now() {
        let dir = scratch("left-out-resumed");
        let (day, nas) = (dir.join("a-day"), dir.join("z-nas"));
        std::fs::create_dir_all(&day).unwrap();
        std::fs::create_dir_all(&nas).unwrap();
        write_frame(&day.join("a.tif"), &R5, 1);
        write_frame(&nas.join("b.tif"), &R6, 2);
        let mut lib = Library::open_in_memory().unwrap();
        let mut walk = lib.tree_walk(&dir).unwrap();
        // Stopped at the first chance, after the root's own folder.
        let report = lib.walk_until(&mut walk, &mut quiet(), &|| true).unwrap();
        assert!(report.stopped);
        let mut walk = walk.leaving_out(std::slice::from_ref(&nas));
        let report = lib.walk_until(&mut walk, &mut quiet(), &|| false).unwrap();
        assert!(!report.stopped);
        assert_eq!(report.added, 1, "{report:?}");
        assert!(lib.by_path(&nas.join("b.tif")).unwrap().is_none());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A pass asks the disk everything in its first phase and nothing
    /// under the write lock: over files whose rows are current (the
    /// common case, a re-poll of an indexed archive), with a sidecar
    /// changed, an XMP appearing, a file new and a file renamed. So a
    /// share that stops answering mid-pass stops it holding no lock.
    #[test]
    fn a_batch_asks_the_disk_nothing_under_the_lock() {
        use greycard_edit_io::xmp;
        let dir = scratch("under-lock");
        let (r5, r6, a7) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        // All as indexed: every row current.
        DISK_CALLS.with(|c| c.set((0, 0)));
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.unchanged, 3, "{report:?}");
        let (all, locked) = DISK_CALLS.with(|c| c.get());
        assert!(
            all >= 6,
            "the stats and the sidecars, in the first phase: {all}"
        );
        assert_eq!(locked, 0, "nothing asked of the disk under the lock");
        // A sidecar changed, an XMP beside another, a file new and a
        // file renamed.
        let mut s = Sidecar::load_as_file(&r5).unwrap().unwrap();
        s.meta.rating = 1;
        s.save(&r5).unwrap();
        let three = Meta {
            rating: 3,
            ..Meta::default()
        };
        std::fs::write(xmp::short_path(&a7), xmp::fresh(&three, None)).unwrap();
        write_frame(&dir.join("new.tif"), &R5, 77);
        std::fs::rename(&r6, dir.join("renamed.tif")).unwrap();
        DISK_CALLS.with(|c| c.set((0, 0)));
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.meta_refreshed, 2, "{report:?}");
        assert_eq!(report.added, 1, "{report:?}");
        assert_eq!(report.moved, 1, "{report:?}");
        assert_eq!(DISK_CALLS.with(|c| c.get()).1, 0);
        assert_eq!(lib.by_path(&r5).unwrap().unwrap().meta.rating, 1);
        assert_eq!(lib.by_path(&a7).unwrap().unwrap().meta.rating, 3);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The editor saves a rating and calls `index_file` while a pass
    /// over the folder is between its phases: the pass read the old
    /// sidecar before the lock, and must not write it back over the
    /// newer meta.
    #[test]
    fn a_meta_saved_between_the_phases_is_not_overwritten() {
        let dir = scratch("stale");
        let (r5, _, a7) = shoot(&dir);
        let db = dir.join("lib").join("library.sidecar.sqlite");
        let mut lib = Library::open(&db).unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(lib.by_path(&r5).unwrap().unwrap().meta.rating, 4);
        let mut other = Library::open(&db).unwrap();
        // The progress call for the last file comes after the first
        // files' phase one and before the batch is written.
        let last = canonical(&dir).unwrap().join("r6.tif");
        let mut saved = false;
        let report = lib
            .index_folder(&dir, &mut |p| {
                if p.path == last {
                    let mut s = Sidecar::load_as_file(&r5).unwrap().unwrap();
                    s.meta.rating = 5;
                    s.save(&r5).unwrap();
                    let r = other.index_file(&r5).unwrap();
                    assert_eq!(r.meta_refreshed, 1, "{r:?}");
                    saved = true;
                }
            })
            .unwrap();
        assert!(saved);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        // The five stands, in the index and on disk.
        assert_eq!(lib.by_path(&r5).unwrap().unwrap().meta.rating, 5);
        assert_eq!(Sidecar::load_as_file(&r5).unwrap().unwrap().meta.rating, 5);
        assert_eq!(lib.by_path(&a7).unwrap().unwrap().meta.rating, 0);
        // The same with the file itself changed on disk between the
        // passes' snapshots, which takes the other write path.
        write_frame(&r5, &A7, 31);
        std::fs::File::options()
            .write(true)
            .open(&r5)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        let report = lib
            .index_folder(&dir, &mut |p| {
                if p.path == last {
                    let mut s = Sidecar::load_as_file(&r5).unwrap().unwrap();
                    s.meta.rating = 1;
                    s.save(&r5).unwrap();
                    other.index_file(&r5).unwrap();
                }
            })
            .unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        let e = lib.by_path(&r5).unwrap().unwrap();
        assert_eq!(e.meta.rating, 1);
        assert_eq!(e.exif.camera, "SONY ILCE-7M4");
        // Changed on disk before the pass, then set back to what the
        // index holds in the seam: the row's sidecar hash is the
        // snapshot's again, and the pass must still not write the
        // change it would have read before the lock. The index says
        // what the disk says.
        let mut s = Sidecar::load_as_file(&r5).unwrap().unwrap();
        s.meta.rating = 2;
        s.save(&r5).unwrap();
        let report = lib
            .index_folder(&dir, &mut |p| {
                if p.path == last {
                    let mut s = Sidecar::load_as_file(&r5).unwrap().unwrap();
                    s.meta.rating = 1;
                    s.save(&r5).unwrap();
                    other.index_file(&r5).unwrap();
                }
            })
            .unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(lib.by_path(&r5).unwrap().unwrap().meta.rating, 1);
        assert_eq!(Sidecar::load_as_file(&r5).unwrap().unwrap().meta.rating, 1);
        drop(other);
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// `index_file` on a copy in the same folder must not take the
    /// original's row: a pass over one file knows nothing of the
    /// folder's listing, and the original is on disk.
    #[test]
    fn an_index_file_on_a_copy_leaves_the_original_its_row() {
        let dir = scratch("copyfile");
        let (r5, _, a7) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        assert_eq!(lib.index_folder(&dir, &mut quiet()).unwrap().added, 3);
        let original = lib.by_path(&r5).unwrap().unwrap();
        let other = lib.by_path(&a7).unwrap().unwrap();
        let copy = dir.join("r5_copy.tif");
        std::fs::copy(&r5, &copy).unwrap();
        let report = lib.index_file(&copy).unwrap();
        assert_eq!((report.added, report.moved), (1, 0), "{report:?}");
        assert_eq!(lib.len().unwrap(), 4);
        assert_eq!(lib.by_path(&r5).unwrap().unwrap().id, original.id);
        assert_eq!(lib.by_path(&a7).unwrap().unwrap().id, other.id);
        let added = lib.by_path(&copy).unwrap().expect("the copy's own row");
        assert_ne!(added.id, original.id);
        assert_eq!(added.hash, original.hash);
        // And the folder pass agrees: nothing to do.
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.unchanged, 4, "{report:?}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Two passes on two folders at once, on a library that is not
    /// there yet: both make rows, neither fails. This is the CLI's
    /// `index a & index b` on a fresh database.
    #[test]
    fn two_passes_on_two_folders_at_once_both_finish() {
        let dir = scratch("twopass");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let n = BATCH * 3;
        for i in 0..n {
            write_frame(&a.join(format!("a{i:03}.tif")), &R5, i as u16);
            write_frame(&b.join(format!("b{i:03}.tif")), &R6, (i + 500) as u16);
        }
        for round in 0..3 {
            let db = dir.join(format!("lib{round}")).join("library.sqlite");
            let passes: Vec<_> = [a.clone(), b.clone()]
                .into_iter()
                .map(|folder| {
                    let db = db.clone();
                    std::thread::spawn(move || {
                        let mut lib = Library::open(&db).map_err(|e| format!("open: {e:?}"))?;
                        lib.index_folder(&folder, &mut |_| {})
                            .map_err(|e| format!("pass over {}: {e:?}", folder.display()))
                    })
                })
                .collect();
            for pass in passes {
                let report = pass
                    .join()
                    .unwrap()
                    .unwrap_or_else(|e| panic!("round {round}: {e}"));
                assert_eq!(report.added, n, "round {round}: {report:?}");
                assert!(report.errors.is_empty(), "{:?}", report.errors);
            }
            let lib = Library::open_read_only(&db).unwrap();
            assert_eq!(lib.len().unwrap(), 2 * n);
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Two writers take turns rather than colliding: a folder pass
    /// with the editor's `index_file` on the folder's last file
    /// running beside it, from another connection on another thread,
    /// finishes with every row and no error on either side.
    #[test]
    fn a_pass_and_a_concurrent_index_file_both_finish() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = scratch("concurrent");
        let n = BATCH * 4;
        for i in 0..n {
            write_frame(&dir.join(format!("f{i:03}.tif")), &R5, i as u16);
        }
        let last = dir.join(format!("f{:03}.tif", n - 1));
        let db = dir.join("lib").join("library.sqlite");
        let mut lib = Library::open(&db).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let writer = std::thread::spawn({
            let (db, last, stop) = (db.clone(), last.clone(), stop.clone());
            move || {
                let mut other = Library::open(&db).unwrap();
                let (mut calls, mut errors) = (0usize, Vec::new());
                while !stop.load(Ordering::Relaxed) {
                    match other.index_file(&last) {
                        Ok(_) => calls += 1,
                        Err(e) => errors.push(e.to_string()),
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                (calls, errors)
            }
        });
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        stop.store(true, Ordering::Relaxed);
        let (calls, errors) = writer.join().unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert!(calls > 0);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        // The last file was added by whichever got there first and
        // found unchanged by the other; every file has one row.
        assert_eq!(report.seen(), n, "{report:?}");
        assert_eq!(lib.len().unwrap(), n);
        assert!(lib.by_path(&last).unwrap().is_some());
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// An unmounted drive leaves its mount point behind, an empty
    /// folder: nothing under it is marked missing or claimed as a
    /// move, and a prune forgets nothing of it.
    #[test]
    fn an_empty_mount_point_is_unavailable_not_deleted() {
        let dir = scratch("mount");
        let mnt = dir.join("mnt");
        let drive = mnt.join("drive");
        let day1 = drive.join("day1");
        std::fs::create_dir_all(&day1).unwrap();
        let (r5, _, _) = shoot(&day1);
        let mut lib = Library::open_in_memory().unwrap();
        assert_eq!(lib.index_tree(&drive, &mut quiet()).unwrap().added, 3);
        // Unmounted: the folder stays, empty.
        let off = dir.join("drive.off");
        std::fs::rename(&drive, &off).unwrap();
        std::fs::create_dir(&drive).unwrap();
        let laptop = dir.join("laptop");
        std::fs::create_dir(&laptop).unwrap();
        std::fs::copy(off.join("day1").join("r5.tif"), laptop.join("r5.tif")).unwrap();
        let report = lib.index_folder(&laptop, &mut quiet()).unwrap();
        assert_eq!((report.added, report.moved), (1, 0), "{report:?}");
        let report = lib.index_tree(&drive, &mut quiet()).unwrap();
        assert_eq!(
            report,
            Report {
                unavailable: vec![canonical(&drive).unwrap()],
                ..Report::default()
            }
        );
        let report = lib.index_folder(&drive, &mut quiet()).unwrap();
        assert_eq!(report.unavailable.len(), 1, "{report:?}");
        assert_eq!(lib.prune_missing().unwrap(), 0);
        assert_eq!(names(&lib, "missing:yes"), Vec::<String>::new());
        assert!(!lib.by_path(&r5).unwrap().unwrap().missing);
        // A mount point inside a tree shields what is under it too.
        let report = lib.index_tree(&mnt, &mut quiet()).unwrap();
        assert_eq!(
            (report.missing, report.unavailable.len()),
            (0, 1),
            "{report:?}"
        );
        // Mounted again: everything is where it was.
        std::fs::remove_dir(&drive).unwrap();
        std::fs::rename(&off, &drive).unwrap();
        let report = lib.index_tree(&drive, &mut quiet()).unwrap();
        assert_eq!((report.unchanged, report.added), (3, 0), "{report:?}");
        // An empty folder the index knows nothing of is just empty.
        let fresh = dir.join("fresh");
        std::fs::create_dir(&fresh).unwrap();
        assert_eq!(
            lib.index_folder(&fresh, &mut quiet()).unwrap(),
            Report::default()
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A folder that is not there and that the index knows nothing
    /// of is a name mistyped, and an error; one with rows is the
    /// deleted-folder case.
    #[test]
    fn a_mistyped_folder_is_an_error() {
        let dir = scratch("typo");
        shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        let typo = dir.join("tpyo");
        let said = lib
            .index_folder(&typo, &mut quiet())
            .unwrap_err()
            .to_string();
        assert!(said.contains("tpyo"), "{said}");
        assert!(lib.index_tree(&typo, &mut quiet()).is_err());
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let sub = dir.join("sub");
        std::fs::create_dir(&sub).unwrap();
        write_frame(&sub.join("x.tif"), &A7, 1);
        lib.index_folder(&sub, &mut quiet()).unwrap();
        greycard_core::testing::remove_dir_retry(&sub);
        assert_eq!(lib.index_folder(&sub, &mut quiet()).unwrap().missing, 1);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A link to a file is one entry of its folder, under its own
    /// name, and a save through the link or a lookup by it finds that
    /// row and not a second one for the target.
    #[cfg(unix)]
    #[test]
    fn a_linked_file_has_one_row() {
        let dir = scratch("link");
        let (r5, _, _) = shoot(&dir);
        let link = dir.join("link.tif");
        std::os::unix::fs::symlink(&r5, &link).unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        assert_eq!(lib.index_folder(&dir, &mut quiet()).unwrap().added, 4);
        let e = lib.by_path(&link).unwrap().expect("the link's own row");
        assert_eq!(e.name(), "link.tif");
        let report = lib.index_file(&link).unwrap();
        assert_eq!((report.added, report.unchanged), (0, 1), "{report:?}");
        assert_eq!(lib.len().unwrap(), 4);
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.unchanged, 4, "{report:?}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Two names that a lossy conversion spells the same are two
    /// files, and a copy of a file with such a name is a copy.
    #[cfg(unix)]
    #[test]
    fn a_name_that_is_not_utf8_is_stored_as_its_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let dir = scratch("bytes");
        let a = dir.join(std::ffi::OsStr::from_bytes(b"caf\xe9.tif"));
        let b = dir.join(std::ffi::OsStr::from_bytes(b"caf\xff.tif"));
        // APFS refuses a name that is not UTF-8 ("Illegal byte
        // sequence"), so on a Mac there is nothing to test.
        if let Err(e) = std::fs::write(&b, b"") {
            eprintln!("skipped a_name_that_is_not_utf8_is_stored_as_its_bytes: {e}");
            return;
        }
        write_frame(&a, &R5, 11);
        write_frame(&b, &R6, 12);
        assert_eq!(a.to_string_lossy(), b.to_string_lossy());
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.added, 2, "{report:?}");
        assert!(report.errors.is_empty());
        assert_eq!(lib.len().unwrap(), 2);
        assert_eq!(
            lib.by_path(&a).unwrap().unwrap().exif.camera,
            "Canon EOS R5"
        );
        assert_eq!(
            lib.by_path(&b).unwrap().unwrap().exif.camera,
            "Canon EOS R6m2"
        );
        assert_eq!(
            lib.by_path(&a).unwrap().unwrap().path,
            canonical(&a).unwrap()
        );
        // A second pass and a copy: no flip-flop between moved and
        // added, since the stored path is one that exists.
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.unchanged, 2, "{report:?}");
        std::fs::copy(&a, dir.join("copy.tif")).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.added, 1, "{report:?}");
        assert_eq!(report.moved, 0);
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.unchanged, 3, "{report:?}");
        assert_eq!(report.moved, 0);
        assert_eq!(lib.len().unwrap(), 3);
        // The folder listing shows the name as best it can.
        assert_eq!(names(&lib, "caf").len(), 2);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_tree_is_indexed_folder_by_folder_and_hidden_folders_are_not() {
        let dir = scratch("tree");
        shoot(&dir);
        let sub = dir.join("day2");
        std::fs::create_dir(&sub).unwrap();
        write_frame(&sub.join("x.tif"), &A7, 7);
        // The hidden sidecar folder holds a .gcd, and a hidden
        // folder with a picture in it is somebody else's business.
        let hidden = dir.join(".cache");
        std::fs::create_dir(&hidden).unwrap();
        write_frame(&hidden.join("thumb.tif"), &A7, 8);
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert_eq!(report.added, 4, "{report:?}");
        assert!(report.skipped.is_empty());
        assert_eq!(lib.folders().unwrap().len(), 2);
        assert_eq!(names(&lib, "camera:sony"), ["a7.tif", "x.tif"]);
        // A symbolic link to a folder is not followed, and is named.
        #[cfg(unix)]
        {
            let link = dir.join("link");
            std::os::unix::fs::symlink(&sub, &link).unwrap();
            let report = lib.index_tree(&dir, &mut quiet()).unwrap();
            assert_eq!(report.skipped, vec![link]);
            assert_eq!(report.added, 0);
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A folder past the batch size commits as it goes, so a reader
    /// on another connection sees the first batch before the folder
    /// is done, and a read-only open never waits.
    #[test]
    fn a_pass_commits_in_batches_and_a_reader_gets_in() {
        let dir = scratch("batch");
        for i in 0..(BATCH + 5) {
            write_frame(&dir.join(format!("f{i:03}.tif")), &R5, i as u16);
        }
        let db = dir.join("lib").join("library.sqlite");
        let mut lib = Library::open(&db).unwrap();
        let reader = Library::open_read_only(&db).unwrap();
        assert!(reader.is_read_only());
        let mut seen_mid_pass = None;
        let report = lib
            .index_folder(&dir, &mut |p| {
                if p.done == BATCH + 2 {
                    seen_mid_pass = Some(reader.len().unwrap());
                }
            })
            .unwrap();
        assert_eq!(report.added, BATCH + 5);
        // The first batch was committed before the pass ended.
        assert_eq!(seen_mid_pass, Some(BATCH));
        assert_eq!(reader.len().unwrap(), BATCH + 5);
        drop(reader);
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A pass asked to stop ends after the batch it is on, keeps what
    /// it wrote, marks nothing missing, and the next pass takes up
    /// where it left off.
    #[test]
    fn a_pass_stops_between_batches_and_the_next_takes_up_from_there() {
        let dir = scratch("stop");
        for i in 0..(BATCH + 5) {
            write_frame(&dir.join(format!("f{i:03}.tif")), &R5, i as u16);
        }
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib
            .index_folder_until(&dir, &mut quiet(), &|| true)
            .unwrap();
        assert!(report.stopped);
        assert_eq!(report.added, BATCH);
        assert_eq!(lib.len().unwrap(), BATCH);
        // A file gone before a stopped pass is not marked: the pass
        // did not look for it.
        std::fs::remove_file(dir.join("f000.tif")).unwrap();
        let report = lib
            .index_folder_until(&dir, &mut quiet(), &|| true)
            .unwrap();
        assert!(report.stopped);
        assert_eq!(report.missing, 0);
        // Its batch was f001 to f050, the last of them new.
        assert_eq!((report.unchanged, report.added), (BATCH - 1, 1));
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert!(!report.stopped);
        assert_eq!(report.added, 4);
        assert_eq!(report.missing, 1);
        assert_eq!(report.unchanged, BATCH);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_library_on_disk_keeps_what_it_indexed() {
        let dir = scratch("disk");
        shoot(&dir);
        let db = dir.join("lib").join("library.sqlite");
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&dir, &mut quiet()).unwrap();
            lib.checkpoint().unwrap();
            assert!(lib.size_on_disk().unwrap() > 0);
        }
        let lib = Library::open_read_only(&db).unwrap();
        assert_eq!(lib.len().unwrap(), 3);
        assert_eq!(names(&lib, "flag:pick"), ["r5.tif"]);
        assert_eq!(lib.path(), db);
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The sample raws, when they are to hand: `GREYCARD_SAMPLES`
    /// or the workspace's `target/work/samples`. Real cameras' EXIF
    /// through the raw probe. Skipped, and says so, when neither is
    /// there.
    #[test]
    fn the_sample_raws_index_with_their_exif() {
        let samples = std::env::var_os("GREYCARD_SAMPLES")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/work/samples")
            });
        let Ok(files) = list_folder(&samples) else {
            eprintln!(
                "no sample raws at {}; set GREYCARD_SAMPLES to a folder of them",
                samples.display()
            );
            return;
        };
        let raws: Vec<&PathBuf> = files
            .iter()
            .filter(|p| greycard_core::decode::is_raw_path(p))
            .collect();
        if raws.is_empty() {
            eprintln!("no raws under {}", samples.display());
            return;
        }
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&samples, &mut quiet()).unwrap();
        assert_eq!(report.added, files.len(), "{report:?}");
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        for raw in &raws {
            let e = lib.by_path(raw).unwrap().unwrap();
            assert!(!e.exif.camera.is_empty(), "{}: no camera", raw.display());
            assert!(e.exif.iso.is_some(), "{}: no ISO", raw.display());
            assert!(e.exif.shutter.is_some(), "{}: no shutter", raw.display());
            assert!(e.exif.aperture.is_some(), "{}: no aperture", raw.display());
            assert!(e.exif.focal.is_some(), "{}: no focal length", raw.display());
            let taken = e
                .exif
                .taken
                .as_deref()
                .unwrap_or_else(|| panic!("{}: no date", raw.display()));
            assert!(
                taken.len() >= 10 && &taken[4..5] == "-",
                "{}: {taken}",
                raw.display()
            );
        }
        // A lens is not every body's to say (a Panasonic RW2 in the
        // samples names none), but most do.
        let with_lens = lib
            .query(&Filter::default())
            .unwrap()
            .iter()
            .filter(|e| e.exif.lens.is_some())
            .count();
        assert!(
            with_lens * 2 > raws.len(),
            "{with_lens} of {} lenses",
            raws.len()
        );
        // Every raw answers some camera filter, and the filters
        // partition the folder.
        let all = lib.count(&Filter::default()).unwrap();
        let canon = lib.count(&Filter::parse("make:canon").unwrap()).unwrap();
        let not_canon = lib.count(&Filter::parse("make!=canon").unwrap()).unwrap();
        let no_make = lib
            .query(&Filter::default())
            .unwrap()
            .iter()
            .filter(|e| e.exif.make.is_empty())
            .count();
        assert_eq!(canon + not_canon + no_make, all);
        // A picture without EXIF has no ISO and answers neither side.
        let low = lib.count(&Filter::parse("iso<800").unwrap()).unwrap();
        let high = lib.count(&Filter::parse("iso>=800").unwrap()).unwrap();
        let with_iso = lib.count(&Filter::parse("iso>=0").unwrap()).unwrap();
        assert_eq!(low + high, with_iso);
        assert!(with_iso >= raws.len());
        let report = lib.index_folder(&samples, &mut quiet()).unwrap();
        assert_eq!(report.unchanged, files.len(), "{report:?}");
        // The maker's tags beside the EXIF: every Canon and Fujifilm
        // raw names its maker, and a frame called fixed has a style.
        for raw in &raws {
            let e = lib.by_path(raw).unwrap().unwrap();
            let make = e.exif.make.to_ascii_lowercase();
            if make.starts_with("canon") || make.starts_with("fujifilm") {
                assert!(e.exif.style.maker.is_some(), "{}: no maker", raw.display());
            }
            if e.exif.style.fixed {
                assert!(e.exif.style.style.is_some(), "{}", raw.display());
            }
        }
    }

    /// The numbers for the notes: a thousand frames, half with a
    /// sidecar, into a library on disk. Prints the database's size
    /// and the times; ignored, since it is a measurement and not a
    /// check. `cargo test -p greycard-library -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn a_thousand_frames_for_the_numbers() {
        let dir = scratch("thousand");
        let frames = [R5, R6, A7];
        for i in 0..1000u32 {
            let path = dir.join(format!("IMG_{i:04}.tif"));
            write_frame(&path, &frames[(i % 3) as usize], i as u16);
            if i % 2 == 0 {
                let mut s = Sidecar::default();
                s.meta.rating = (i % 6) as u8;
                s.meta.flag = if i % 5 == 0 { Flag::Pick } else { Flag::None };
                s.meta
                    .set_keywords(vec![format!("shoot{}", i / 100), "Skye".into()]);
                s.save(&path).unwrap();
            }
        }
        let db = dir.join("lib").join("library.sqlite");
        let mut lib = Library::open(&db).unwrap();
        let start = std::time::Instant::now();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        let first = start.elapsed();
        assert_eq!(report.added, 1000, "{report:?}");
        let start = std::time::Instant::now();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        let second = start.elapsed();
        assert_eq!(report.unchanged, 1000, "{report:?}");
        lib.checkpoint().unwrap();
        let size = lib.size_on_disk().unwrap();
        let start = std::time::Instant::now();
        let picks = lib
            .query(&Filter::parse("flag:pick rating>=3 keyword:shoot4 camera:R6").unwrap())
            .unwrap();
        let listed = start.elapsed();
        let start = std::time::Instant::now();
        let n = lib
            .count(&Filter::parse("iso>=3200 date:2026-09").unwrap())
            .unwrap();
        let counted = start.elapsed();
        eprintln!(
            "1000 frames under {}: first pass {:.3} s, second {:.3} s; {size} bytes on disk, {} a row; \
             a four-term query {:.1} ms for {} rows, a count {:.1} ms for {n}",
            dir.display(),
            first.as_secs_f64(),
            second.as_secs_f64(),
            size / 1000,
            listed.as_secs_f64() * 1000.0,
            picks.len(),
            counted.as_secs_f64() * 1000.0,
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The roots' questions: every file under them by folder then
    /// name, a root's count, the missing left out, and a folder
    /// beside a root whose name starts like it not under it.
    #[test]
    fn the_files_under_the_roots_are_listed_by_folder_then_name() {
        let dir = scratch("under");
        let (a, a2, b, ab) = (
            dir.join("a"),
            dir.join("a2"),
            dir.join("b"),
            dir.join("a").join("day"),
        );
        for d in [&a, &a2, &b, &ab] {
            std::fs::create_dir_all(d).unwrap();
        }
        write_frame(&a.join("z.tif"), &R5, 1);
        write_frame(&ab.join("c.tif"), &R6, 2);
        write_frame(&ab.join("b.tif"), &A7, 3);
        write_frame(&a2.join("n.tif"), &R5, 4);
        write_frame(&b.join("m.tif"), &R6, 5);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&dir, &mut quiet()).unwrap();
        let under =
            |lib: &Library, roots: &[PathBuf]| -> Vec<PathBuf> { lib.paths_under(roots).unwrap() };
        assert_eq!(
            under(&lib, std::slice::from_ref(&a)),
            [a.join("z.tif"), ab.join("b.tif"), ab.join("c.tif")]
        );
        assert_eq!(
            under(&lib, &[b.clone(), a.clone()]),
            [
                a.join("z.tif"),
                ab.join("b.tif"),
                ab.join("c.tif"),
                b.join("m.tif")
            ]
        );
        assert_eq!(lib.count_under(&a).unwrap(), 3);
        assert_eq!(lib.count_under(&a2).unwrap(), 1);
        assert!(under(&lib, &[]).is_empty());
        // The whole disk as a root (`/`, or a drive's own `D:\` on
        // Windows): nothing is left out for want of a separator
        // doubled.
        let disk = dir.ancestors().last().unwrap().to_path_buf();
        assert_eq!(under(&lib, &[disk]).len(), 5);
        std::fs::remove_file(ab.join("b.tif")).unwrap();
        lib.index_folder(&ab, &mut quiet()).unwrap();
        assert_eq!(
            under(&lib, std::slice::from_ref(&a)),
            [a.join("z.tif"), ab.join("c.tif")]
        );
        assert_eq!(lib.count_under(&a).unwrap(), 2);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A root's folders as a tree is built from them: each folder with
    /// files directly in it and how many, the root's own among them, a
    /// folder beside the root whose name starts like it left out, and
    /// a folder's own rows without those of the folders under it.
    #[test]
    fn a_roots_folders_are_counted_and_a_folders_own_rows_listed() {
        let dir = scratch("folder-counts");
        let (a, a2, ab, abc) = (
            dir.join("a"),
            dir.join("a2"),
            dir.join("a").join("day"),
            dir.join("a").join("day").join("more"),
        );
        for d in [&a, &a2, &ab, &abc] {
            std::fs::create_dir_all(d).unwrap();
        }
        write_frame(&a.join("z.tif"), &R5, 1);
        write_frame(&ab.join("c.tif"), &R6, 2);
        write_frame(&ab.join("b.tif"), &A7, 3);
        write_frame(&abc.join("d.tif"), &A7, 4);
        write_frame(&a2.join("n.tif"), &R5, 5);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&dir, &mut quiet()).unwrap();
        assert_eq!(
            lib.folder_counts_under_canonical(&a).unwrap(),
            [(a.clone(), 1), (ab.clone(), 2), (abc.clone(), 1)]
        );
        // The parent of them all has no file of its own and is not
        // listed.
        assert_eq!(lib.folder_counts_under_canonical(&dir).unwrap().len(), 4);
        let own: Vec<PathBuf> = lib
            .rows_in_canonical(&ab, crate::Order::NAME)
            .unwrap()
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(own, [ab.join("b.tif"), ab.join("c.tif")]);
        assert_eq!(lib.count_in_canonical(&ab).unwrap(), 2);
        assert_eq!(lib.count_in_canonical(&dir).unwrap(), 0);
        assert!(
            lib.rows_in_canonical(&dir, crate::Order::NAME)
                .unwrap()
                .is_empty()
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A file moved from one folder under a root to another, either
    /// way round the walk's order, keeps its row and is not missing
    /// at the end of the tree pass; and the window can ask where the
    /// row it knew is now.
    #[test]
    fn a_file_moved_across_folders_is_found_by_a_tree_pass() {
        let dir = scratch("tree-move");
        let (early, late) = (dir.join("a-early"), dir.join("z-late"));
        std::fs::create_dir_all(&early).unwrap();
        std::fs::create_dir_all(&late).unwrap();
        // A frame left in each folder, so neither is ever empty (an
        // emptied folder is an unmounted drive's mount point to the
        // index, §160).
        write_frame(&early.join("stay.tif"), &A7, 1);
        write_frame(&late.join("stay2.tif"), &A7, 2);
        write_frame(&early.join("x.tif"), &R5, 3);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&dir, &mut quiet()).unwrap();
        let id = lib.by_path(&early.join("x.tif")).unwrap().unwrap().id;

        // Walked first, then its new folder: missing, then claimed.
        std::fs::rename(early.join("x.tif"), late.join("x.tif")).unwrap();
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert_eq!(report.moved, 1, "{report:?}");
        assert_eq!(report.added, 0, "{report:?}");
        let row = lib.by_path(&late.join("x.tif")).unwrap().unwrap();
        assert_eq!((row.id, row.missing), (id, false));
        assert_eq!(lib.found_at(&[id]).unwrap()[&id], late.join("x.tif"));

        // And back, the new folder walked first: claimed at once.
        std::fs::rename(late.join("x.tif"), early.join("x.tif")).unwrap();
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert_eq!((report.moved, report.missing, report.added), (1, 0, 0));
        let row = lib.by_path(&early.join("x.tif")).unwrap().unwrap();
        assert_eq!((row.id, row.missing), (id, false));
        assert_eq!(lib.len().unwrap(), 3);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A folder renamed is not a move to the index — the old folder
    /// is gone, which is what an unmounted drive looks like — so its
    /// files are new rows. The window still finds its frame, by the
    /// hash of the row it knew.
    #[test]
    fn a_frame_in_a_renamed_folder_is_found_by_its_hash() {
        let dir = scratch("found-at");
        let (old, new) = (dir.join("old"), dir.join("new"));
        std::fs::create_dir_all(&old).unwrap();
        write_frame(&old.join("x.tif"), &R5, 1);
        write_frame(&old.join("y.tif"), &R6, 2);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&dir, &mut quiet()).unwrap();
        let id = lib.by_path(&old.join("x.tif")).unwrap().unwrap().id;
        std::fs::rename(&old, &new).unwrap();
        lib.index_tree(&dir, &mut quiet()).unwrap();
        let old_row = lib.by_hash(&hash::hash_file(&new.join("x.tif")).unwrap());
        assert_eq!(
            old_row.unwrap().len(),
            2,
            "the old row, missing, and a new one"
        );
        assert_eq!(lib.found_at(&[id]).unwrap()[&id], new.join("x.tif"));
        // Gone for good: no answer.
        std::fs::remove_file(new.join("x.tif")).unwrap();
        lib.index_folder(&new, &mut quiet()).unwrap();
        assert!(lib.found_at(&[id]).unwrap().is_empty());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A file away when its folder was renamed waits for it in the new
    /// folder: moved back there under another name, it is a move, its
    /// row and its sidecar kept. A row whose name the new folder has
    /// stays in the old one.
    #[test]
    fn a_file_away_while_its_folder_is_renamed_is_a_move_when_it_comes_back() {
        let dir = scratch("away-renamed");
        let root = dir.join("root");
        let [a, b] = ["a", "b"].map(|n| root.join(n));
        std::fs::create_dir_all(&a).unwrap();
        write_frame(&a.join("x.tif"), &R5, 1);
        write_frame(&a.join("y.tif"), &R6, 2);
        write_frame(&a.join("z.tif"), &A7, 3);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&root, &mut quiet()).unwrap();
        let before = lib.by_path(&a.join("z.tif")).unwrap().unwrap();
        let mut edit = Sidecar::default();
        edit.meta.rating = 5;
        edit.save(&a.join("z.tif")).unwrap();
        lib.index_tree(&root, &mut quiet()).unwrap();
        // z out of the root, then the folder renamed.
        let outside = dir.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::rename(a.join("z.tif"), outside.join("z.tif")).unwrap();
        lib.index_tree(&root, &mut quiet()).unwrap();
        std::fs::rename(&a, &b).unwrap();
        let report = lib.index_tree(&root, &mut quiet()).unwrap();
        assert_eq!(report.moved_folders.len(), 1, "{report:?}");
        // Back into the new folder, renamed.
        std::fs::rename(outside.join("z.tif"), b.join("z2.tif")).unwrap();
        let report = lib.index_tree(&root, &mut quiet()).unwrap();
        let cb = canonical(&b).unwrap();
        assert_eq!(
            report.moved_files,
            [(cb.join("z.tif"), cb.join("z2.tif"))],
            "{report:?}"
        );
        assert_eq!(report.added, 0, "{report:?}");
        let after = lib.by_path(&b.join("z2.tif")).unwrap().unwrap();
        assert_eq!(after.id, before.id);
        assert_eq!(after.meta.rating, 5);
        // x and y, live in the new folder, left their old rows behind.
        assert!(lib.by_path(&a.join("x.tif")).unwrap().unwrap().missing);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A folder renamed is said by its old path and its new: found in
    /// one tree pass (the editor closed), or in two passes on the same
    /// connection, one over each folder, in either order (the watcher's,
    /// the editor open), and renamed back to a name whose rows are
    /// missing. Copied twice and the original deleted is a tie, and no
    /// rename; deleted with one of three files carried out first, none.
    #[test]
    fn a_renamed_folder_is_said_by_both_paths() {
        let dir = scratch("folder-renamed");
        let [a, b, c] = ["a", "b", "c"].map(|n| dir.join(n));
        std::fs::create_dir_all(&a).unwrap();
        write_frame(&a.join("x.tif"), &R5, 1);
        write_frame(&a.join("y.tif"), &R6, 2);
        write_frame(&a.join("z.tif"), &A7, 3);
        let canon = |p: &Path| canonical(p).unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&dir, &mut quiet()).unwrap();
        let ca = canon(&a);

        // One pass over the root.
        std::fs::rename(&a, &b).unwrap();
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert_eq!(
            report.moved_folders,
            [(ca.clone(), canon(&b))],
            "{report:?}"
        );
        assert_eq!((report.added, report.missing), (3, 3), "{report:?}");
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert!(report.moved_folders.is_empty(), "nothing new: {report:?}");

        // Two passes, the new folder's first.
        std::fs::rename(&b, &c).unwrap();
        let cb = ca.with_file_name("b");
        let report = lib.index_tree(&c, &mut quiet()).unwrap();
        assert!(report.moved_folders.is_empty(), "{report:?}");
        let report = lib.index_tree(&b, &mut quiet()).unwrap();
        assert_eq!(
            report.moved_folders,
            [(cb.clone(), canon(&c))],
            "{report:?}"
        );

        // Back to a, whose rows come back from missing: the old
        // folder's pass first.
        let cc = canon(&c);
        std::fs::rename(&c, &a).unwrap();
        let report = lib.index_tree(&c, &mut quiet()).unwrap();
        assert!(report.moved_folders.is_empty(), "{report:?}");
        let report = lib.index_tree(&a, &mut quiet()).unwrap();
        assert_eq!(report.moved_folders, [(cc, ca.clone())], "{report:?}");
        assert_eq!(report.returned, 3, "{report:?}");

        // Copied twice, the original deleted: a tie.
        for to in [&b, &c] {
            std::fs::create_dir_all(to).unwrap();
            for name in ["x.tif", "y.tif", "z.tif"] {
                std::fs::copy(a.join(name), to.join(name)).unwrap();
            }
        }
        lib.index_tree(&dir, &mut quiet()).unwrap();
        greycard_core::testing::remove_dir_retry(&a);
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert!(report.moved_folders.is_empty(), "{report:?}");
        assert_eq!(report.missing, 3, "{report:?}");

        // One file of three carried out, and the folder deleted.
        greycard_core::testing::remove_dir_retry(&c);
        let picks = dir.join("picks");
        std::fs::create_dir_all(&picks).unwrap();
        std::fs::rename(b.join("x.tif"), picks.join("x.tif")).unwrap();
        greycard_core::testing::remove_dir_retry(&b);
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert!(report.moved_folders.is_empty(), "{report:?}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The halves of a rename wait a while and no longer; a new folder
    /// is claimed once; more than half of each side's files is needed;
    /// a tie drops the halves it weighed; and folders gone in one pass
    /// are none of them another's new name, in either order.
    #[test]
    fn a_half_of_a_rename_waits_and_is_matched_once() {
        let files = |names: &[&str]| -> Files {
            names
                .iter()
                .map(|n| (OsString::from(n), format!("h-{n}")))
                .collect()
        };
        let at = |n: &str| Path::new(&std::path::MAIN_SEPARATOR_STR).join("s").join(n);
        let gone =
            |r: &mut Renames, n: &str, f: &[&str], t: Instant| r.gone(vec![(at(n), files(f))], t);
        let t = Instant::now();
        let mut r = Renames::default();
        assert!(gone(&mut r, "a", &["x", "y"], t).is_empty());
        let late = t + RENAME_WAIT + Duration::from_secs(1);
        assert!(
            r.live(at("b"), files(&["x", "y"]), late).is_none(),
            "too late"
        );
        // The live half waits now; a gone folder takes it, and a second
        // gone folder of the same files finds nothing left.
        assert_eq!(gone(&mut r, "c", &["x", "y"], late), [(at("c"), at("b"))]);
        assert!(gone(&mut r, "d", &["x", "y"], late).is_empty());
        // Half of the files is not more than half, on either side.
        let mut r = Renames::default();
        gone(&mut r, "a", &["x", "y"], t);
        assert!(r.live(at("b"), files(&["x"]), t).is_none());
        let mut r = Renames::default();
        gone(&mut r, "a", &["x", "y"], t);
        assert!(r.live(at("b"), files(&["x", "y", "p", "q"]), t).is_none());
        // A tie: two copies weighed against one gone folder are dropped,
        // and are no later folder's new name.
        let mut r = Renames::default();
        r.live(at("b"), files(&["x", "y", "z"]), t);
        r.live(at("c"), files(&["x", "y", "z"]), t);
        assert!(gone(&mut r, "a", &["x", "y", "z"], t).is_empty());
        assert!(r.live.is_empty());
        // Gone in one pass, in either order: neither is the other's.
        for order in [["b", "c"], ["c", "b"]] {
            let mut r = Renames::default();
            r.live(at("c"), files(&["x", "y", "z"]), t);
            let went = order.map(|n| (at(n), files(&["x", "y", "z"]))).to_vec();
            assert!(r.gone(went, t).is_empty(), "{order:?}");
        }
        // Kept to a bound.
        let mut r = Renames::default();
        for i in 0..RENAME_MOST + 5 {
            gone(&mut r, &i.to_string(), &["x"], t);
        }
        assert_eq!(r.gone.len(), RENAME_MOST);
    }

    /// A folder under the root that cannot be read is an error in the
    /// report, its rows left as they were, and the rest of the tree
    /// is indexed.
    #[cfg(unix)]
    #[test]
    fn a_locked_folder_under_a_root_is_said_and_walked_around() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("locked");
        let (locked, open) = (dir.join("a-locked"), dir.join("b-open"));
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::create_dir_all(&open).unwrap();
        write_frame(&locked.join("x.tif"), &R5, 1);
        write_frame(&open.join("y.tif"), &R6, 2);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&dir, &mut quiet()).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        write_frame(&open.join("z.tif"), &A7, 3);
        let passed = lib.index_tree(&dir, &mut quiet());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if std::fs::read_dir(&locked).is_ok() && passed.as_ref().is_ok_and(|r| r.errors.is_empty())
        {
            // Run as root, which reads the folder anyway.
            greycard_core::testing::remove_dir_retry(&dir);
            return;
        }
        let report = passed.unwrap();
        assert_eq!(report.added, 1, "{report:?}");
        assert_eq!(report.missing, 0, "the locked folder's row stands");
        assert_eq!(report.errors.len(), 1, "{report:?}");
        assert_eq!(report.errors[0].0, locked);
        assert_eq!(
            report.unreadable,
            std::slice::from_ref(&locked),
            "said as a folder"
        );
        assert_eq!(lib.len().unwrap(), 3);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A tree pass asked to stop ends between folders, marks nothing
    /// missing, and the next one finishes the walk.
    #[test]
    fn a_tree_pass_stops_between_folders_and_takes_up_again() {
        let dir = scratch("tree-stop");
        for d in ["a", "b", "c"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
            write_frame(&dir.join(d).join("f.tif"), &R5, d.len() as u16);
        }
        std::fs::create_dir_all(dir.join("gone")).unwrap();
        write_frame(&dir.join("gone").join("g.tif"), &R6, 9);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&dir, &mut quiet()).unwrap();
        greycard_core::testing::remove_dir_retry(&dir.join("gone"));
        for d in ["a", "b", "c"] {
            write_frame(&dir.join(d).join("new.tif"), &A7, 7);
        }
        let folders = std::cell::Cell::new(0);
        let report = lib
            .index_tree_until(
                &dir,
                &mut |p| {
                    if p.done == 0 {
                        folders.set(folders.get() + 1);
                    }
                },
                &|| folders.get() >= 2,
            )
            .unwrap();
        assert!(report.stopped, "{report:?}");
        assert_eq!(report.missing, 0, "the gone folder was not looked for");
        assert!(report.added < 3, "{report:?}");
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert!(!report.stopped);
        assert_eq!(report.missing, 1);
        assert_eq!(
            lib.paths_under(std::slice::from_ref(&dir)).unwrap().len(),
            6
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The review's repro for a frame "followed" to its backup: the
    /// frame copied to `backup/` and the original deleted. The copy is
    /// not where the frame went, and `found_at` has no answer for it,
    /// since the frame's own folder is still there.
    #[test]
    fn a_deleted_frame_is_not_found_at_its_copy() {
        let dir = scratch("found-copy");
        let (shoot, backup) = (dir.join("shoot"), dir.join("backup"));
        std::fs::create_dir_all(&shoot).unwrap();
        std::fs::create_dir_all(&backup).unwrap();
        write_frame(&shoot.join("x.tif"), &R5, 1);
        write_frame(&shoot.join("y.tif"), &R6, 2);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&dir, &mut quiet()).unwrap();
        let id = lib.by_path(&shoot.join("x.tif")).unwrap().unwrap().id;
        std::fs::copy(shoot.join("x.tif"), backup.join("x.tif")).unwrap();
        lib.index_tree(&dir, &mut quiet()).unwrap();
        std::fs::remove_file(shoot.join("x.tif")).unwrap();
        lib.index_tree(&dir, &mut quiet()).unwrap();
        assert!(lib.by_path(&shoot.join("x.tif")).unwrap().unwrap().missing);
        assert!(
            lib.found_at(&[id]).unwrap().is_empty(),
            "a copy is not a move"
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A tree pass stopped over and over, as the launch pass is while
    /// someone culls, takes up where it stopped: every file is looked
    /// at once, not once a stop.
    #[test]
    fn a_tree_walk_stopped_again_and_again_still_gets_there() {
        let dir = scratch("tree-walk");
        for d in ["a", "b", "c", "d"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
            for n in 0..3 {
                write_frame(&dir.join(d).join(format!("f{n}.tif")), &R5, n);
            }
        }
        let mut lib = Library::open_in_memory().unwrap();
        let mut walk = lib.tree_walk(&dir).unwrap();
        let mut looked = 0;
        let mut calls = 0;
        loop {
            calls += 1;
            // A stop asked for before every folder, as a save waiting
            // does.
            let report = lib
                .walk_until(&mut walk, &mut |_| looked += 1, &|| true)
                .unwrap();
            if !report.stopped {
                assert_eq!(report.added, 12, "{report:?}");
                break;
            }
            assert!(calls < 20, "no headway");
        }
        assert_eq!(looked, 12, "each file looked at once");
        assert_eq!(lib.len().unwrap(), 12);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A walk stopped in the middle of a folder counts that folder's
    /// files once when it is done, as the first look found them, not
    /// again as unchanged to the second.
    #[test]
    fn a_walk_stopped_inside_a_folder_counts_its_files_once() {
        let dir = scratch("tree-walk-once");
        for i in 0..(BATCH + 10) {
            write_frame(&dir.join(format!("f{i:03}.tif")), &R5, i as u16);
        }
        let mut lib = Library::open_in_memory().unwrap();
        let mut walk = lib.tree_walk(&dir).unwrap();
        let first = lib.walk_until(&mut walk, &mut |_| {}, &|| true).unwrap();
        assert!(first.stopped, "stopped after the first batch");
        let done = lib.walk_until(&mut walk, &mut |_| {}, &|| false).unwrap();
        assert!(!done.stopped);
        assert_eq!((done.added, done.unchanged), (BATCH + 10, 0), "{done:?}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A pass names the files it found changed, so a window makes
    /// their pictures again and no others.
    #[test]
    fn a_pass_names_the_files_it_found_changed() {
        let dir = scratch("changed-files");
        write_frame(&dir.join("a.tif"), &R5, 1);
        write_frame(&dir.join("b.tif"), &R6, 2);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        // A copy that finished: another length, another mtime.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_frame(&dir.join("b.tif"), &A7, 3);
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert_eq!(report.changed_files, [dir.join("b.tif")]);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A file rewritten in place under its own name is said with the
    /// hash its row had and the one it has now; a save of its sidecar
    /// alone, or a pass that finds it as it was, says none.
    #[test]
    fn a_file_rewritten_in_place_is_said_with_both_hashes() {
        let dir = scratch("rehashed");
        let a = dir.join("a.tif");
        write_frame(&a, &R5, 1);
        write_frame(&dir.join("b.tif"), &R6, 2);
        let mut lib = Library::open_in_memory().unwrap();
        let first = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert!(first.rehashed.is_empty());
        let old = lib.by_path(&a).unwrap().unwrap().hash;
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_frame(&a, &A7, 3);
        let new = crate::hash_file(&a).unwrap();
        assert_ne!(old, new);
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(
            report.rehashed,
            [Rehash {
                path: a.clone(),
                old: old.clone(),
                new: new.clone()
            }]
        );
        // The rows of the new hash, and none of the old.
        let rows = lib.rows_by_hash(&new).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, a);
        assert!(lib.rows_by_hash(&old).unwrap().is_empty());
        let ids = lib.ids_by_hashes([new.as_str(), old.as_str()]).unwrap();
        assert_eq!(ids.len(), 1);
        // Again, nothing changed: nothing said.
        let again = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert!(again.rehashed.is_empty());
        // The editor's own row for the file says it too.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_frame(&a, &R6, 4);
        let one = lib.index_file(&a).unwrap();
        assert_eq!(one.rehashed.len(), 1);
        assert_eq!(one.rehashed[0].old, new);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Sets a row's maker tags by hand, as the reader would have for
    /// a raw: the test frames are TIFFs, which carry none.
    fn tag(lib: &mut Library, path: &Path, style: &str, fixed: bool) {
        lib.conn_mut()
            .execute(
                "UPDATE files SET maker = 'Canon', style = ?, style_fixed = ? WHERE path = ?",
                params![style, fixed, path_bytes(path)],
            )
            .unwrap();
    }

    /// Frames of two bodies in two folders, some in each style, with
    /// their dates a day apart in the order they are written.
    fn styled_library(dir: &Path) -> (Library, Vec<PathBuf>) {
        let mut lib = Library::open_in_memory().unwrap();
        let mut paths = Vec::new();
        for (sub, n) in [("a", 5usize), ("b", 3)] {
            let folder = dir.join(sub);
            std::fs::create_dir_all(&folder).unwrap();
            for i in 0..n {
                let taken = format!("2026:0{}:1{i} 10:00:00", if sub == "a" { 3 } else { 4 });
                let frame = crate::fixture::Frame {
                    taken: &taken,
                    ..if i % 2 == 0 { R6 } else { R5 }
                };
                let path = folder.join(format!("{sub}{i}.tif"));
                write_frame(&path, &frame, i as u16);
                paths.push(path);
            }
            lib.index_folder(&folder, &mut quiet()).unwrap();
        }
        (lib, paths)
    }

    #[test]
    fn the_groups_are_body_and_style_with_their_fixed_counts() {
        let dir = scratch("style-groups");
        let (mut lib, paths) = styled_library(&dir);
        // R6 frames are a0, a2, a4, b0, b2; R5 frames a1, a3, b1.
        for (i, p) in paths.iter().enumerate() {
            let style = if i == 3 {
                "Canon Standard"
            } else {
                "Canon Faithful"
            };
            // a2 has the optimizer on; everything else is fixed.
            tag(&mut lib, p, style, i != 2);
        }
        let groups = lib.style_groups(None).unwrap();
        let seen: Vec<(&str, &str, usize, usize)> = groups
            .iter()
            .map(|g| (g.camera.as_str(), g.style.as_str(), g.fixed, g.unfixed))
            .collect();
        assert_eq!(
            seen,
            [
                ("Canon EOS R5", "Canon Faithful", 2, 0),
                ("Canon EOS R5", "Canon Standard", 1, 0),
                ("Canon EOS R6m2", "Canon Faithful", 4, 1),
            ]
        );
        assert_eq!(groups[2].maker, "Canon");
        // Under one folder, only its frames count.
        let under = lib.style_groups(Some(&[dir.join("b")])).unwrap();
        let seen: Vec<(&str, usize)> = under
            .iter()
            .map(|g| (g.camera.as_str(), g.fixed + g.unfixed))
            .collect();
        assert_eq!(seen, [("Canon EOS R5", 1), ("Canon EOS R6m2", 2)]);
        // The frames of one group, by date, with whether each is fixed.
        let frames = lib.style_frames(&groups[2], None).unwrap();
        let names: Vec<(String, bool)> = frames
            .iter()
            .map(|f| {
                (
                    f.path.file_name().unwrap().to_string_lossy().into_owned(),
                    f.fixed,
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                ("a0.tif".to_string(), true),
                ("a2.tif".to_string(), false),
                ("a4.tif".to_string(), true),
                ("b0.tif".to_string(), true),
                ("b2.tif".to_string(), true),
            ]
        );
        assert_eq!(frames[0].taken.as_deref(), Some("2026-03-10 10:00:00"));
        assert_eq!(frames[0].lens.as_deref(), Some(R6.lens));
        let frames = lib
            .style_frames(&groups[2], Some(&[dir.join("a")]))
            .unwrap();
        assert_eq!(frames.len(), 3);
        // The style is a facet too, and a chip lists what it counted.
        let chips = lib
            .facet_counts(crate::Facet::Style, None, &Filter::default())
            .unwrap();
        let seen: Vec<(&str, usize)> = chips.iter().map(|c| (c.value.as_str(), c.count)).collect();
        assert_eq!(seen, [("Canon Faithful", 7), ("Canon Standard", 1)]);
        let faithful = Filter {
            terms: vec![crate::filter::Term::Facet {
                facet: crate::Facet::Style,
                values: vec!["Canon Faithful".into()],
            }],
        };
        assert_eq!(lib.count(&faithful).unwrap(), 7);
        // A frame gone from its folder is in no group.
        std::fs::remove_file(&paths[0]).unwrap();
        lib.index_folder(&dir.join("a"), &mut quiet()).unwrap();
        let groups = lib.style_groups(None).unwrap();
        assert_eq!((groups[2].fixed, groups[2].unfixed), (3, 1));
        // Only raws count as unread or as having no style: a row named
        // like a raw with its tags not read, and one read with none.
        assert_eq!(lib.styles_unread(None).unwrap(), 0);
        assert_eq!(lib.styles_missing(None).unwrap(), 0);
        lib.conn_mut()
            .execute(
                "UPDATE files SET name = 'x.CR3', style_read = 0 WHERE path = ?",
                params![path_bytes(&paths[6])],
            )
            .unwrap();
        lib.conn_mut()
            .execute(
                "UPDATE files SET name = 'y.raf', style = NULL, style_read = 1 WHERE path = ?",
                params![path_bytes(&paths[7])],
            )
            .unwrap();
        assert_eq!(lib.styles_unread(None).unwrap(), 1);
        assert_eq!(lib.styles_missing(None).unwrap(), 1);
        assert_eq!(lib.styles_missing(Some(&[dir.join("a")])).unwrap(), 0);
        // The tags come back on the row's entry too.
        assert_eq!(
            lib.by_path(&paths[1])
                .unwrap()
                .unwrap()
                .exif
                .style
                .maker
                .as_deref(),
            Some("Canon")
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A library from before the maker's tags keeps its rows and gets
    /// the new columns, every row marked unread; the next pass reads
    /// the tags of each and nothing else.
    #[test]
    fn a_schema_2_library_is_brought_up_and_its_rows_refilled() {
        let dir = scratch("migrate-2");
        let db = dir.join("library.sqlite");
        let shoot_dir = dir.join("shoot");
        std::fs::create_dir_all(&shoot_dir).unwrap();
        let (r5, _, _) = shoot(&shoot_dir);
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
            // Back to schema 2 as it was: the columns and the index
            // this schema added gone, the version at 2.
            lib.conn_mut()
                .execute_batch(
                    "DROP INDEX files_style;
                     ALTER TABLE files DROP COLUMN maker;
                     ALTER TABLE files DROP COLUMN style;
                     ALTER TABLE files DROP COLUMN style_fixed;
                     ALTER TABLE files DROP COLUMN peripheral;
                     ALTER TABLE files DROP COLUMN style_read;
                     ALTER TABLE files DROP COLUMN edited;
                     ALTER TABLE files DROP COLUMN turns;
                     ALTER TABLE files DROP COLUMN whole_hash;
                     ALTER TABLE files DROP COLUMN exported; ALTER TABLE files DROP COLUMN develop_key;
                     ALTER TABLE files DROP COLUMN mosaic; DROP INDEX files_taken_at;
                     ALTER TABLE files DROP COLUMN taken_at; ALTER TABLE files DROP COLUMN shift; ALTER TABLE files DROP COLUMN focus;
                     PRAGMA user_version = 2;",
                )
                .unwrap();
        }
        // A reader cannot bring it up, and says so.
        assert!(matches!(
            Library::open_read_only(&db),
            Err(Error::NeedsRebuild(_))
        ));
        let mut lib = Library::open(&db).unwrap();
        let version: i32 = lib
            .conn_mut()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, crate::SCHEMA_VERSION);
        // The rows are the same rows, meta and all.
        assert_eq!(lib.len().unwrap(), 3);
        let e = lib.by_path(&r5).unwrap().unwrap();
        assert_eq!(e.meta.rating, 4);
        assert_eq!(e.exif.style, StyleTags::default());
        // Every row is marked unread, but the test frames are TIFFs,
        // and only raws are counted as waiting for their tags.
        let unread: i64 = lib
            .conn_mut()
            .query_row("SELECT count(*) FROM files WHERE style_read = 0", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(unread, 3);
        assert_eq!(lib.styles_unread(None).unwrap(), 0);
        // The next pass reads the tags, and the two sidecars again for
        // schema 4's columns, and nothing else.
        let report = lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
        assert_eq!(
            (report.styled, report.unchanged, report.meta_refreshed),
            (3, 1, 2),
            "{report:?}"
        );
        assert_eq!(report.added + report.changed, 0);
        assert_eq!(lib.styles_unread(None).unwrap(), 0);
        let report = lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
        assert_eq!(report.styled, 0, "{report:?}");
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A row sums up a turned frame's edit as the editor reads it (§724):
    /// a state placed under the turn before is shown under the file's, by
    /// the shape the file writes, so the develop key is the one the
    /// editor's read makes and not that of the state as placed.
    #[test]
    fn a_row_sums_up_a_turned_frame_as_the_editor_reads_it() {
        use greycard_edit::Edit;
        let mut s = Sidecar::default();
        let mut e = Edit::default();
        e.geometry.crop = Some(greycard_edit::geometry::Crop::of([0.1, 0.2, 0.3, 0.4]));
        s.record(e);
        assert!(s.turn_by(1, Some(1.5)).is_some());
        let json = s.to_json();
        let raw = Path::new("frame.cr3");
        let summary = summary_from_json(json.as_bytes(), Path::new("frame.cr3.gcd"), Some(raw));
        let key = |e: &Edit| {
            greycard_edit::key::decode_key(
                greycard_edit_io::key::develop_key(e, crate::lens_database().as_deref()),
                raw,
            )
        };
        let editor = Sidecar::from_json(&json, &greycard_edit::Edit::default()).unwrap();
        assert_eq!(summary.develop_key, Some(key(editor.current())));
        let mut placed = editor.current_written().into_owned();
        placed.placed_under = 0;
        assert_ne!(summary.develop_key, Some(key(&placed)));
    }

    /// A row says whether its sidecar holds a develop, and which way
    /// up the picture is shown, beside the meta: what a browser needs
    /// to list a frame, badge it and turn its thumbnail without
    /// reading the sidecar. A frame only rated has no develop; a
    /// picture's default is a picture's, not a raw's.
    #[test]
    fn a_row_mirrors_the_develop_and_the_turns_beside_the_meta() {
        use greycard_edit::Edit;
        let dir = scratch("edited");
        let rated = dir.join("rated.tif");
        let developed = dir.join("developed.tif");
        let turned = dir.join("turned.tif");
        let bare = dir.join("bare.tif");
        for (i, p) in [&rated, &developed, &turned, &bare].iter().enumerate() {
            write_frame(p, &R5, i as u16 + 1);
        }
        let fresh = || Sidecar {
            line: greycard_edit::Line::of(Edit::for_picture(), 0).unwrap(),
            ..Sidecar::default()
        };
        // Rated only: the picture's own default edit, three stars.
        let mut s = fresh();
        s.meta.rating = 3;
        s.save(&rated).unwrap();
        // Developed: one step in the history.
        let mut s = fresh();
        let mut e = Edit::for_picture();
        e.light.exposure = 0.7;
        s.record(e);
        s.save(&developed).unwrap();
        // Turned: the edit's quarter turn and mirror, and the frame's
        // own turn on top.
        let mut s = fresh();
        let mut e = Edit::for_picture();
        e.geometry.turns = 1;
        e.geometry.flip = true;
        let shown = e.geometry.shown_turns(1);
        s.turn = 1;
        s.record(e);
        s.save(&turned).unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let rows = lib
            .rows_under_canonical(std::slice::from_ref(&dir), crate::Order::NAME)
            .unwrap();
        let by_name: HashMap<String, &crate::RowMeta> = rows
            .iter()
            .map(|(p, r)| (p.file_name().unwrap().to_string_lossy().into_owned(), r))
            .collect();
        assert_eq!(by_name.len(), 4);
        let r = by_name["rated.tif"];
        assert_eq!(
            (r.meta.rating, r.edited, r.shown_turns()),
            (3, false, (0, false))
        );
        let d = by_name["developed.tif"];
        assert_eq!((d.meta.rating, d.edited), (0, true));
        let t = by_name["turned.tif"];
        assert!(t.edited);
        assert_eq!(t.shown_turns(), shown);
        assert_eq!(shown, (2, true), "the turn and the mirror both count");
        let b = by_name["bare.tif"];
        assert_eq!((b.edited, b.shown_turns()), (false, (0, false)));
        assert_eq!(b.hash, crate::hash_file(&bare).unwrap());
        // By path, a file the index has not seen among them.
        let asked = lib
            .rows_of_with(
                &[rated.clone(), dir.join("none.tif"), turned.clone()],
                &mut |d| d.to_path_buf(),
            )
            .unwrap();
        assert_eq!(asked[0].as_ref().map(|r| r.meta.rating), Some(3));
        assert!(asked[1].is_none());
        assert_eq!(
            asked[2].as_ref().map(|r| r.turns),
            Some(crate::pack_turns(shown))
        );
        let e = lib.by_path(&turned).unwrap().unwrap();
        assert_eq!((e.edited, crate::unpack_turns(e.turns)), (true, shown));

        // The rated frame gets a develop: the next pass reads its
        // sidecar again and the row says so.
        let mut s = Sidecar::load_as_file(&rated).unwrap().unwrap();
        let mut e = s.current().clone();
        e.light.exposure = -0.3;
        s.record(e);
        s.save(&rated).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(
            (report.meta_refreshed, report.unchanged),
            (1, 3),
            "{report:?}"
        );
        let e = lib.by_path(&rated).unwrap().unwrap();
        assert_eq!((e.meta.rating, e.edited), (3, true));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A picture's sidecar an older build wrote when it was only rated
    /// holds the picture default of then, its lens corrections on: not
    /// edited, in the row as in memory. One with a real edit over that
    /// old default is.
    #[test]
    fn a_picture_rated_before_its_lens_started_off_is_not_edited() {
        use greycard_edit::Edit;
        let dir = scratch("picture-lens-on");
        let rated = dir.join("rated.tif");
        let edited = dir.join("edited.tif");
        for (i, p) in [&rated, &edited].iter().enumerate() {
            write_frame(p, &R5, i as u16 + 1);
        }
        let old = Edit::picture_defaults()[1].clone();
        assert!(old.lens.distortion && old.lens.vignetting && old.lens.chromatic_aberration);
        let mut s = Sidecar {
            line: greycard_edit::Line::of(old.clone(), 0).unwrap(),
            ..Sidecar::default()
        };
        s.meta.rating = 2;
        s.save(&rated).unwrap();
        let mut moved = old.clone();
        moved.light.exposure = 0.4;
        let mut s = Sidecar {
            line: greycard_edit::Line::of(moved, 0).unwrap(),
            ..Sidecar::default()
        };
        s.save(&edited).unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let r = lib.by_path(&rated).unwrap().unwrap();
        assert_eq!((r.meta.rating, r.edited), (2, false));
        assert!(lib.by_path(&edited).unwrap().unwrap().edited);
        // The rule in memory, which the strip's mark follows.
        let back = Sidecar::load_as_file(&rated).unwrap().unwrap();
        assert_eq!(crate::develop_marks(&back, &rated, None), (false, false));
        let back = Sidecar::load_as_file(&edited).unwrap().unwrap();
        assert_eq!(crate::develop_marks(&back, &edited, None), (true, false));
        // A raw holding a picture's old default is edited: the rule is
        // a picture's.
        assert!(!crate::is_default_edit(&old, &dir.join("IMG.CR3"), None));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A reset leaves its history behind and is no develop; the blend
    /// the ISO seeds is none either, one set on purpose is. An export
    /// is a fact about the frame: it stays through an undo past the
    /// exported state and past the history's cap, and a sidecar from
    /// before the fact is read from its history.
    #[test]
    fn a_row_mirrors_an_export_and_a_reset_is_not_a_develop() {
        use greycard_edit::{Edit, Exported};
        let _held = lens_name_held();
        let dir = scratch("exported");
        let names = [
            "plain", "reset", "now", "behind", "undone", "capped", "seeded", "blended", "legacy",
        ];
        let path = |n: &str| dir.join(format!("{n}.tif"));
        for (i, n) in names.iter().enumerate() {
            write_frame(&path(n), &R5, i as u16 + 1);
        }
        let fresh = || Sidecar {
            line: greycard_edit::Line::of(Edit::for_picture(), 0).unwrap(),
            ..Sidecar::default()
        };
        let moved = |x: f32| {
            let mut e = Edit::for_picture();
            e.light.exposure = x;
            e
        };
        let sent = || Exported {
            file: "out.jpg".into(),
            at: 5,
            ..Exported::default()
        };
        let mut s = fresh();
        s.record(moved(0.5));
        s.record(Edit::for_picture());
        s.save(&path("reset")).unwrap();
        let mut s = fresh();
        s.record(moved(0.5));
        assert!(s.record_export(&moved(0.5), sent()));
        s.save(&path("now")).unwrap();
        let mut s = fresh();
        s.record(moved(0.5));
        assert!(s.record_export(&moved(0.5), sent()));
        s.record(moved(0.9));
        s.save(&path("behind")).unwrap();
        // Exported, then undone past the exported state: its record
        // goes with the redo stack, which is not written.
        let mut s = fresh();
        s.record(moved(0.5));
        s.record(moved(0.9));
        assert!(s.record_export(&moved(0.9), sent()));
        assert!(s.undo());
        s.save(&path("undone")).unwrap();
        // Exported, then more steps than the history keeps.
        let mut s = fresh();
        s.record(moved(0.5));
        assert!(s.record_export(&moved(0.5), sent()));
        for k in 0..51 {
            s.record(moved(1.0 + k as f32 / 100.0));
        }
        assert!(
            s.history.iter().all(|h| h.exports.is_empty()),
            "the record fell off the history"
        );
        s.save(&path("capped")).unwrap();
        fresh().save(&path("plain")).unwrap();
        // The blend the ISO seeds, as an open or an export leaves it,
        // and the same frame with the blend moved.
        let mut s = fresh();
        {
            let mut e = s.current().clone();
            {
                e.noise.learned_strength = greycard_edit::Noise::blend_for_iso(Some(R5.iso.into()))
            }
            s.record(e);
        }
        s.save(&path("seeded")).unwrap();
        let mut s = fresh();
        {
            let mut e = s.current().clone();
            e.noise.learned_strength = 0.8;
            s.record(e);
        }
        s.save(&path("blended")).unwrap();
        // A sidecar from before `last_export`: its history says.
        let mut s = fresh();
        s.record(moved(0.5));
        assert!(s.record_export(&moved(0.5), sent()));
        s.record(moved(0.9));
        s.save(&path("legacy")).unwrap();
        let gcd = dir.join("legacy.tif.gcd");
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&gcd).unwrap()).unwrap();
        assert!(
            json.as_object_mut()
                .unwrap()
                .remove("last_export")
                .is_some()
        );
        std::fs::write(&gcd, serde_json::to_vec(&json).unwrap()).unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let said = |n: &str| {
            let e = lib.by_path(&path(n)).unwrap().unwrap();
            (e.edited, e.exported)
        };
        assert_eq!(said("plain"), (false, false));
        assert_eq!(said("reset"), (false, false));
        assert_eq!(said("seeded"), (false, false), "the seed is no edit");
        assert_eq!(said("blended"), (true, false), "a blend set is");
        assert_eq!(said("now"), (true, true));
        assert_eq!(said("behind"), (true, true));
        assert_eq!(said("undone"), (true, true), "an undo does not unexport");
        assert_eq!(said("capped"), (true, true), "nor does the cap");
        assert_eq!(said("legacy"), (true, true));
        // The same rule on the sidecar in memory.
        let iso = Some(Some(u32::from(R5.iso)));
        for n in names {
            let s = Sidecar::load_as_file(&path(n)).unwrap().unwrap();
            assert_eq!(crate::develop_marks(&s, &path(n), iso), said(n), "{n}");
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A schema 8 library gains the develop key column empty; the next
    /// pass over a folder reads the sidecar of each edited row without
    /// a key and fills it, and nothing else of the folder is read. The
    /// key is the edit's own, as the editor makes it.
    #[test]
    fn a_schema_8_library_fills_its_develop_keys_on_the_next_pass() {
        use greycard_edit::Edit;
        let held = lens_name_held();
        let dir = scratch("schema8");
        // The frames are pictures: the first is edited; the second's
        // sidecar holds a rating and no state (§724), so it is no more
        // edited than the third, with no sidecar.
        let (edited, _rated, plain) = shoot(&dir);
        let mut brightened = Edit::for_picture();
        brightened.light.exposure = 0.7;
        let mut s = Sidecar {
            line: greycard_edit::Line::of(brightened.clone(), 0).unwrap(),
            ..Sidecar::default()
        };
        s.save(&edited).unwrap();
        let db = dir.join("library.sqlite");
        let key_of = |lib: &mut Library, p: &Path| -> Option<i64> {
            lib.conn_mut()
                .query_row(
                    "SELECT develop_key FROM files WHERE path = ?",
                    params![path_bytes(p)],
                    |r| r.get(0),
                )
                .unwrap()
        };
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&dir, &mut quiet()).unwrap();
            assert_eq!(
                key_of(&mut lib, &edited),
                Some(greycard_edit_io::key::develop_key(&brightened, None) as i64),
                "the key is written with the row"
            );
            assert_eq!(key_of(&mut lib, &plain), None, "no sidecar, no key");
            lib.conn_mut()
                .execute_batch(
                    "ALTER TABLE files DROP COLUMN develop_key; ALTER TABLE files DROP COLUMN mosaic; DROP INDEX files_taken_at; ALTER TABLE files DROP COLUMN taken_at; ALTER TABLE files DROP COLUMN shift; ALTER TABLE files DROP COLUMN focus; PRAGMA user_version = 8;",
                )
                .unwrap();
        }
        let mut lib = Library::open(&db).unwrap();
        assert_eq!(
            key_of(&mut lib, &edited),
            None,
            "the column comes back empty"
        );
        // Both sidecars are read again with the schema's step; the
        // edited row alone gets a key.
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.meta_refreshed, 2, "{report:?}");
        assert_eq!(
            key_of(&mut lib, &edited),
            Some(greycard_edit_io::key::develop_key(&brightened, None) as i64)
        );
        // Filled, the next pass reads nothing again.
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.meta_refreshed, 0, "{report:?}");
        // The lens database changing clears the keys, and the pass
        // after fills them under its name.
        drop(held);
        let named = LensNamed::new("elsewhere 2026");
        assert_eq!(lib.clear_develop_keys().unwrap(), 1);
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.meta_refreshed, 1, "{report:?}");
        assert_eq!(
            key_of(&mut lib, &edited),
            Some(greycard_edit_io::key::develop_key(&brightened, Some("elsewhere 2026")) as i64)
        );
        drop(named);
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The rows of the `versions` table the file's row lists, by id,
    /// name and place.
    fn version_rows(lib: &Library, dir: &Path, file: &Path) -> Vec<crate::VersionRow> {
        lib.rows_in_canonical(dir, crate::Order::NAME)
            .unwrap()
            .into_iter()
            .find(|(p, _)| p == file)
            .map(|(_, row)| row.versions)
            .unwrap_or_default()
    }

    fn versions_in_table(lib: &mut Library) -> i64 {
        lib.conn_mut()
            .query_row("SELECT count(*) FROM versions", [], |r| r.get(0))
            .unwrap()
    }

    /// A frame's sidecar with two versions beside its own: one a copy
    /// of its brightened edit, renamed, and one from the defaults.
    fn with_versions(path: &Path) -> (Sidecar, String, String, greycard_edit::Edit) {
        let mut brightened = greycard_edit::Edit::for_picture();
        brightened.light.exposure = 0.7;
        let mut s = Sidecar {
            line: greycard_edit::Line::of(brightened.clone(), 0).unwrap(),
            ..Sidecar::default()
        };
        s.meta.rating = 3;
        let warm = s.new_version(None, Some("Warm")).unwrap();
        let fresh = s
            .new_version_from_original(greycard_edit::Edit::for_picture(), None)
            .unwrap();
        s.save(path).unwrap();
        (s, warm, fresh, brightened)
    }

    /// A sidecar with two versions makes two rows of the `versions`
    /// table, in the sidecar's order, each with its own name, marks and
    /// key; the file's row keeps version 0's, and `has:versions` finds
    /// the file.
    #[test]
    fn a_sidecar_with_two_versions_gives_two_version_rows() {
        let _held = lens_name_held();
        let dir = scratch("versions");
        let (r5, _r6, _a7) = shoot(&dir);
        let (_s, warm, fresh, brightened) = with_versions(&r5);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let rows = version_rows(&lib, &dir, &r5);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(
            (rows[0].id.as_str(), rows[0].name.as_str()),
            (warm.as_str(), "Warm")
        );
        assert_eq!(
            (rows[1].id.as_str(), rows[1].name.as_str()),
            (fresh.as_str(), "Version 3")
        );
        assert!(rows[0].edited, "a copy of an edit is edited");
        assert!(!rows[1].edited, "the defaults are not");
        assert!(!rows[0].exported && !rows[1].exported);
        assert_eq!(
            rows[0].develop_key,
            Some(greycard_edit_io::key::develop_key(&brightened, None))
        );
        assert_eq!(
            rows[1].develop_key, None,
            "nobody developed it: no state to key (§724)"
        );
        assert!(rows[0].made > 0);
        let entry = lib.by_path(&r5).unwrap().unwrap();
        assert!(
            entry.edited && entry.meta.rating == 3,
            "the row is version 0's"
        );
        assert_eq!(names(&lib, "has:versions"), ["r5.tif"]);
        let mut others = names(&lib, "has!=versions");
        others.sort();
        assert_eq!(others, ["a7.tif", "r6.tif"]);
        // The other files have none.
        assert!(version_rows(&lib, &dir, &dir.join("a7.tif")).is_empty());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A version deleted from the sidecar drops its row at the next
    /// pass, the last one deleted leaves none, and a file forgotten
    /// takes its versions' rows with it.
    #[test]
    fn a_version_deleted_drops_its_row() {
        let dir = scratch("version-deleted");
        let (r5, _r6, _a7) = shoot(&dir);
        let (mut s, warm, fresh, _) = with_versions(&r5);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(versions_in_table(&mut lib), 2);
        s.delete_version(&warm).unwrap();
        s.save(&r5).unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let rows = version_rows(&lib, &dir, &r5);
        assert_eq!(
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            [fresh.as_str()]
        );
        // The save's own word to the index does the same.
        s.delete_version(&fresh).unwrap();
        s.save(&r5).unwrap();
        lib.index_file(&r5).unwrap();
        assert!(version_rows(&lib, &dir, &r5).is_empty());
        assert_eq!(versions_in_table(&mut lib), 0);
        // Forgetting the file takes its versions.
        let (_s, _, _, _) = with_versions(&r5);
        lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(versions_in_table(&mut lib), 2);
        lib.forget(std::slice::from_ref(&r5)).unwrap();
        assert_eq!(versions_in_table(&mut lib), 0, "cascaded with the file");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A schema 9 library gains the `versions` table empty, and the
    /// next pass reads every sidecar again (as schema 8's did) and
    /// fills it, and nothing else of the folder.
    #[test]
    fn a_schema_9_library_gains_the_versions_table_on_the_next_pass() {
        let dir = scratch("schema9");
        let (r5, _r6, _a7) = shoot(&dir);
        let (_s, warm, _fresh, _) = with_versions(&r5);
        let db = dir.join("library.sqlite");
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&dir, &mut quiet()).unwrap();
            lib.conn_mut()
                .execute_batch("DROP TABLE versions; ALTER TABLE files DROP COLUMN mosaic; DROP INDEX files_taken_at; ALTER TABLE files DROP COLUMN taken_at; ALTER TABLE files DROP COLUMN shift; ALTER TABLE files DROP COLUMN focus; PRAGMA user_version = 9;")
                .unwrap();
        }
        let mut lib = Library::open(&db).unwrap();
        let version: i32 = lib
            .conn_mut()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, crate::SCHEMA_VERSION);
        assert_eq!(versions_in_table(&mut lib), 0, "the table comes empty");
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(
            (report.meta_refreshed, report.added),
            (2, 0),
            "the two sidecars alone: {report:?}"
        );
        let rows = version_rows(&lib, &dir, &r5);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, warm);
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.meta_refreshed, 0, "{report:?}");
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// What the rows' develop keys were made under is kept in the
    /// index, and a library settled under anything else (a look
    /// refitted, the lens database's cache wiped, the recipe raised)
    /// has every key cleared, for the next pass to make anew; settled
    /// under the same, before or after a reopen, it keeps them.
    #[test]
    fn the_develop_keys_go_when_what_they_were_made_under_moves() {
        use greycard_edit::Edit;
        let dir = scratch("made-under");
        let (edited, _rated, _plain) = shoot(&dir);
        let mut brightened = Edit::for_picture();
        brightened.light.exposure = 0.7;
        Sidecar {
            line: greycard_edit::Line::of(brightened, 0).unwrap(),
            ..Sidecar::default()
        }
        .save(&edited)
        .unwrap();
        let db = dir.join("library.sqlite");
        let key_of = |lib: &mut Library| -> Option<i64> {
            lib.conn_mut()
                .query_row(
                    "SELECT develop_key FROM files WHERE path = ?",
                    params![path_bytes(&edited)],
                    |r| r.get(0),
                )
                .unwrap()
        };
        let first = "recipe 1; lenses None; files 00";
        let mut lib = Library::open(&db).unwrap();
        assert_eq!(
            lib.settle_develop_keys_under(first).unwrap(),
            Some(0),
            "nothing to clear in a new library, and what it is under kept"
        );
        lib.index_folder(&dir, &mut quiet()).unwrap();
        assert!(key_of(&mut lib).is_some());
        assert_eq!(lib.settle_develop_keys_under(first).unwrap(), None);
        drop(lib);
        let mut lib = Library::open(&db).unwrap();
        assert_eq!(lib.settle_develop_keys_under(first).unwrap(), None);
        assert!(
            key_of(&mut lib).is_some(),
            "a reopen under the same keeps them"
        );
        for (under, why) in [
            ("recipe 1; lenses None; files 01", "a look refitted"),
            (
                "recipe 1; lenses Some(\"/x/lensfun 2026\"); files 01",
                "the lens database fetched",
            ),
            ("recipe 1; lenses None; files 01", "its cache wiped"),
            ("recipe 2; lenses None; files 01", "the recipe raised"),
        ] {
            assert_eq!(
                lib.settle_develop_keys_under(under).unwrap(),
                Some(1),
                "{why}: the edited row's key goes"
            );
            assert_eq!(key_of(&mut lib), None, "{why}");
            let report = lib.index_folder(&dir, &mut quiet()).unwrap();
            assert_eq!(report.meta_refreshed, 1, "{why}: {report:?}");
            assert!(key_of(&mut lib).is_some(), "{why}: made anew");
        }
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A schema 7 library comes up with the exports its sidecars
    /// record and without the develop a history-only reset made.
    #[test]
    fn a_schema_7_library_reads_its_sidecars_for_exports_and_resets() {
        use greycard_edit::{Edit, Exported};
        let dir = scratch("schema7");
        let (exported, reset, _) = shoot(&dir);
        let base = |p: &Path| {
            if greycard_core::picture::is_picture_path(p) {
                Edit::for_picture()
            } else {
                Edit::default()
            }
        };
        let mut moved = base(&exported);
        moved.light.exposure = 0.5;
        let mut s = Sidecar {
            line: greycard_edit::Line::of(base(&exported), 0).unwrap(),
            ..Sidecar::default()
        };
        s.record(moved.clone());
        assert!(s.record_export(&moved, Exported::default()));
        s.save(&exported).unwrap();
        let mut s = Sidecar {
            line: greycard_edit::Line::of(base(&reset), 0).unwrap(),
            ..Sidecar::default()
        };
        s.record(moved);
        s.record(base(&reset));
        s.save(&reset).unwrap();
        let db = dir.join("library.sqlite");
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&dir, &mut quiet()).unwrap();
            // As schema 7 left them: the reset's history made it edited,
            // and there was no column for the export.
            lib.conn_mut()
                .execute(
                    "UPDATE files SET edited = 1 WHERE path = ?",
                    params![path_bytes(&reset)],
                )
                .unwrap();
            lib.conn_mut()
                .execute_batch("ALTER TABLE files DROP COLUMN exported; ALTER TABLE files DROP COLUMN develop_key; ALTER TABLE files DROP COLUMN mosaic; DROP INDEX files_taken_at; ALTER TABLE files DROP COLUMN taken_at; ALTER TABLE files DROP COLUMN shift; ALTER TABLE files DROP COLUMN focus; PRAGMA user_version = 7;")
                .unwrap();
        }
        let mut lib = Library::open(&db).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.meta_refreshed, 2, "{report:?}");
        let a = lib.by_path(&exported).unwrap().unwrap();
        assert_eq!((a.edited, a.exported), (true, true));
        let b = lib.by_path(&reset).unwrap().unwrap();
        assert_eq!((b.edited, b.exported), (false, false));
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A row's key takes the headroom's version in for a CR3, as the
    /// engine's does, and for no other raw; its versions' keys too.
    #[test]
    fn a_cr3_rows_key_takes_the_headroom_in() {
        let mut sidecar = greycard_edit::Sidecar::default();
        {
            let mut e = sidecar.current().clone();
            e.light.exposure = 0.5;
            sidecar.record(e);
        }
        let json = serde_json::to_vec(&sidecar).unwrap();
        let own = greycard_edit_io::key::develop_key(
            sidecar.line.current(),
            crate::lens_database().as_deref(),
        );
        let nef = summary_from_json(&json, Path::new("x.gcd"), Some(Path::new("x.nef")));
        assert_eq!(nef.develop_key, Some(own));
        let cr3 = summary_from_json(&json, Path::new("x.gcd"), Some(Path::new("x.CR3")));
        assert_eq!(
            cr3.develop_key,
            Some(greycard_edit::key::headroom_key(own, true))
        );
        assert_ne!(cr3.develop_key, nef.develop_key);
    }

    /// A row's key takes the DNG decode's version in for a DNG, as
    /// the engine's does, and for a CR3 or a RAF not.
    #[test]
    fn a_dng_rows_key_takes_the_dng_decode_in() {
        let mut sidecar = greycard_edit::Sidecar::default();
        {
            let mut e = sidecar.current().clone();
            e.light.exposure = 0.5;
            sidecar.record(e);
        }
        let json = serde_json::to_vec(&sidecar).unwrap();
        let own = greycard_edit_io::key::develop_key(
            sidecar.line.current(),
            crate::lens_database().as_deref(),
        );
        let row = |raw: &str| summary_from_json(&json, Path::new("x.gcd"), Some(Path::new(raw)));
        assert_eq!(
            row("x.DNG").develop_key,
            Some(greycard_edit::key::dng_decode_key(own, true))
        );
        assert_eq!(row("x.RAF").develop_key, Some(own));
        assert_eq!(
            row("x.cr3").develop_key,
            Some(greycard_edit::key::headroom_key(own, true))
        );
    }

    /// A sidecar whose edit this build cannot read still gives up its
    /// stars, counts as edited, and shows its picture by the frame's
    /// own turn alone.
    #[test]
    fn an_unreadable_edit_is_an_edit_with_its_stars_kept() {
        let json = br#"{"version": 99, "current": {"light": "nonsense"}, "meta": {"rating": 4}, "turn": 3}"#;
        let s = summary_from_json(json, Path::new("x.gcd"), Some(Path::new("x.cr3")));
        assert_eq!(s.meta.rating, 4);
        assert!(s.edited);
        assert_eq!(crate::unpack_turns(s.turns), (1, false));
        // Nothing but meta: no develop, and a raw's default is the
        // raw's.
        let s = summary_from_json(br#"{"meta": {"rating": 1}}"#, Path::new("x.gcd"), None);
        assert_eq!((s.meta.rating, s.edited, s.turns), (1, false, 0));
        // Not JSON at all: there is something in it, and it counts as
        // a develop; nothing else is known of it.
        let s = summary_from_json(b"not json", Path::new("x.gcd"), None);
        assert_eq!(
            s,
            Summary {
                edited: true,
                // The mark for a develop whose edit cannot be read: the
                // row is not read again for a key every pass.
                develop_key: Some(0),
                ..Summary::default()
            }
        );
    }

    /// A library at schema 3 is brought up in place: the two columns
    /// added, and every row with a sidecar marked for the next pass
    /// to read the sidecar again and fill them, nothing else of the
    /// file read.
    #[test]
    fn a_schema_3_library_is_brought_up_and_its_sidecars_read_again() {
        let dir = scratch("migrate-3");
        let db = dir.join("library.sqlite");
        let shoot_dir = dir.join("shoot");
        std::fs::create_dir_all(&shoot_dir).unwrap();
        let (r5, _, _) = shoot(&shoot_dir);
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
            lib.conn_mut()
                .execute_batch(
                    "ALTER TABLE files DROP COLUMN edited;
                     ALTER TABLE files DROP COLUMN turns;
                     ALTER TABLE files DROP COLUMN whole_hash;
                     ALTER TABLE files DROP COLUMN exported; ALTER TABLE files DROP COLUMN develop_key;
                     ALTER TABLE files DROP COLUMN mosaic; DROP INDEX files_taken_at; ALTER TABLE files DROP COLUMN taken_at; ALTER TABLE files DROP COLUMN shift; ALTER TABLE files DROP COLUMN focus; PRAGMA user_version = 3;",
                )
                .unwrap();
        }
        assert!(matches!(
            Library::open_read_only(&db),
            Err(Error::NeedsRebuild(_))
        ));
        let mut lib = Library::open(&db).unwrap();
        let version: i32 = lib
            .conn_mut()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, crate::SCHEMA_VERSION);
        assert_eq!(lib.len().unwrap(), 3);
        let e = lib.by_path(&r5).unwrap().unwrap();
        assert_eq!(
            (e.meta.rating, e.edited),
            (4, false),
            "the meta kept, the develop not known yet"
        );
        let waiting: i64 = lib
            .conn_mut()
            .query_row(
                "SELECT count(*) FROM files WHERE sidecar IS NOT NULL AND sidecar_hash IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            waiting, 2,
            "the two rows with a sidecar wait for it to be read again"
        );
        // The next pass reads those two sidecars and nothing else.
        let report = lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
        assert_eq!(
            (report.meta_refreshed, report.unchanged),
            (2, 1),
            "{report:?}"
        );
        assert_eq!(report.added + report.changed, 0);
        let e = lib.by_path(&r5).unwrap().unwrap();
        // The shoot's sidecars hold a rating and no state (§724): no
        // develop.
        assert_eq!((e.meta.rating, e.edited), (4, false));
        let report = lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
        assert_eq!(report.meta_refreshed, 0, "{report:?}");
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A raw with a Lightroom develop in the XMP beside it and nothing
    /// of ours is taken by the pass as an open takes it: a `.gcd` with
    /// one state, "Imported from Lightroom", which the row reads as an
    /// edit. A pass again, or over a fresh index, takes nothing twice;
    /// an edit of ours is left alone.
    #[test]
    fn a_pass_takes_a_lightroom_develop_beside_a_raw_once() {
        use greycard_edit_io::xmp;
        // Written by hand: a stop brighter, three stars, one keyword path.
        const PACKET: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:lr="http://ns.adobe.com/lightroom/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   xmp:Rating="3" crs:Exposure2012="+1.00" crs:HasSettings="True">
   <lr:hierarchicalSubject><rdf:Bag><rdf:li>Places|Skye</rdf:li></rdf:Bag></lr:hierarchicalSubject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let dir = scratch("lightroom-xmp");
        let (taken, ours, rated, bare) = (
            dir.join("taken.CR3"),
            dir.join("ours.CR3"),
            dir.join("rated.CR3"),
            dir.join("bare.CR3"),
        );
        for p in [&taken, &ours, &rated, &bare] {
            std::fs::write(p, b"not a raw").unwrap();
        }
        for p in [&taken, &ours, &rated] {
            std::fs::write(xmp::short_path(p), PACKET).unwrap();
        }
        // Rated here, nothing developed: takes the develop.
        let mut one = Sidecar::default();
        one.meta.rating = 1;
        one.save(&rated).unwrap();
        let mut s = Sidecar::default();
        let mut edit = greycard_edit::Edit::default();
        edit.light.exposure = -0.5;
        assert!(s.record(edit));
        s.save(&ours).unwrap();

        let steps = |p: &Path| {
            Sidecar::load_as_file(p)
                .unwrap()
                .map(|s| s.line.history.len())
        };
        // A library not told where sidecars go takes nothing, and does
        // not count the XMPs looked at: told, its next pass takes.
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(steps(&taken), None);
        assert_eq!(steps(&rated), Some(0));
        assert!(!lib.by_path(&taken).unwrap().unwrap().edited);
        lib.set_lightroom_takes(Some(greycard_edit::Placement::Beside));
        lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(steps(&taken), Some(1));
        assert_eq!(steps(&rated), Some(1));
        assert!(lib.by_path(&taken).unwrap().unwrap().edited);
        drop(lib);
        for _ in 0..2 {
            let mut lib = Library::open_in_memory().unwrap();
            lib.set_lightroom_takes(Some(greycard_edit::Placement::Beside));
            lib.index_folder(&dir, &mut quiet()).unwrap();
            lib.index_folder(&dir, &mut quiet()).unwrap();
            let e = lib.by_path(&taken).unwrap().unwrap();
            assert_eq!((e.meta.rating, e.edited), (3, true));
            assert_eq!(e.meta.keywords, ["Places/Skye"]);
            assert!(e.sidecar.is_some(), "the .gcd the pass wrote");
            assert_eq!(steps(&taken), Some(1));
            let s = Sidecar::load_as_file(&taken).unwrap().unwrap();
            assert_eq!(s.line.current_label(), Some("Imported from Lightroom"));
            assert!((s.line.current().light.exposure - 1.0).abs() < 1e-6);
            let s = Sidecar::load_as_file(&ours).unwrap().unwrap();
            assert_eq!(
                (s.line.history.len(), s.line.current().light.exposure),
                (1, -0.5)
            );
            assert_eq!(steps(&bare), None, "nothing beside it, nothing written");
            // Its `.gcd` never took this XMP's word, so the XMP's
            // rating wins, as an open's adoption has it.
            let e = lib.by_path(&rated).unwrap().unwrap();
            assert_eq!((e.meta.rating, e.edited), (3, true));
            assert_eq!(steps(&rated), Some(1));
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The row takes an XMP beside the frame the way the editor takes
    /// it when it opens the frame: an XMP alone gives its rating and
    /// keywords; an XMP the `.gcd` has not taken from yet lays its
    /// fields over the `.gcd`'s meta, whatever the two files' times;
    /// one the `.gcd` has taken from already (its mark) says nothing
    /// more, so a rating cleared here is not undone by an older
    /// opinion; and a change to the XMP is a change to the row.
    #[test]
    fn a_row_takes_the_xmp_beside_the_frame_as_the_editor_does() {
        use greycard_edit_io::xmp;
        let dir = scratch("xmp");
        let only = dir.join("only.tif");
        let both = dir.join("both.tif");
        let taken = dir.join("taken.tif");
        let bare = dir.join("bare.tif");
        for (i, p) in [&only, &both, &taken, &bare].iter().enumerate() {
            write_frame(p, &R6, i as u16 + 1);
        }
        let three = Meta {
            rating: 3,
            keywords: vec!["Harbor".into()],
            ..Meta::default()
        };
        // Rated in another tool, never opened here.
        std::fs::write(xmp::short_path(&only), xmp::fresh(&three, None)).unwrap();
        // A `.gcd` with two stars and a label, and an XMP it has not
        // taken from: the XMP's rating and keywords over it, the label
        // kept (the XMP does not name one).
        let mut s = Sidecar::default();
        s.meta.rating = 2;
        s.meta.label = Label::Red;
        s.save(&both).unwrap();
        std::fs::write(xmp::short_path(&both), xmp::fresh(&three, None)).unwrap();
        // Taken from and then cleared here: the same XMP has nothing
        // new to say, however its time compares.
        let mut s = Sidecar::default();
        std::fs::write(xmp::short_path(&taken), xmp::fresh(&three, None)).unwrap();
        assert!(xmp::adopt(&taken, &mut s, || None).moved());
        s.meta.set_rating(0);
        s.save(&taken).unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let row = |lib: &Library, p: &Path| lib.by_path(p).unwrap().unwrap();
        let e = row(&lib, &only);
        assert_eq!((e.meta.rating, e.edited), (3, false));
        assert_eq!(e.meta.keywords, ["Harbor"]);
        assert!(e.sidecar.is_none(), "no .gcd; the XMP is what the row read");
        let e = row(&lib, &both);
        assert_eq!((e.meta.rating, e.meta.label), (3, Label::Red));
        assert_eq!(e.meta.keywords, ["Harbor"]);
        assert_eq!(row(&lib, &taken).meta.rating, 0, "already taken from");
        assert_eq!(row(&lib, &bare).meta.rating, 0);
        // The same, through the filter and the rows.
        assert_eq!(names(&lib, "rating>=3"), ["both.tif", "only.tif"]);
        let rows = lib
            .rows_under_canonical(std::slice::from_ref(&dir), crate::Order::NAME)
            .unwrap();
        assert_eq!(rows.len(), 4);

        // The other tool rates `only` five: the XMP changed, and the
        // next pass reads it again.
        let five = Meta {
            rating: 5,
            ..Meta::default()
        };
        std::fs::write(xmp::short_path(&only), xmp::fresh(&five, None)).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(
            (report.meta_refreshed, report.unchanged),
            (1, 3),
            "{report:?}"
        );
        assert_eq!(row(&lib, &only).meta.rating, 5);
        // A `.gcd` appearing beside the XMP is a change too.
        let mut s = Sidecar::default();
        s.meta.rating = 1;
        s.save(&only).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.meta_refreshed, 1, "{report:?}");
        // The `.gcd` has no mark, so the XMP is taken over it, as the
        // editor would take it on opening.
        assert_eq!(row(&lib, &only).meta.rating, 5);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// An XMP past the most read is passed over by the pass as an
    /// unreadable one is: the frame alone is as bare, the `.gcd`
    /// beside one is what the row reads. Sparse, so no 64 MB is
    /// written.
    #[test]
    fn a_row_passes_over_an_xmp_past_the_most_read() {
        use greycard_edit_io::xmp;
        let dir = scratch("xmp-most");
        let alone = dir.join("alone.tif");
        let rated = dir.join("rated.tif");
        for (i, p) in [&alone, &rated].iter().enumerate() {
            write_frame(p, &R6, i as u16 + 1);
            let file = std::fs::File::create(xmp::short_path(p)).unwrap();
            file.set_len(xmp::BESIDE_MOST as u64 + 1).unwrap();
        }
        let mut s = Sidecar::default();
        s.meta.rating = 2;
        s.save(&rated).unwrap();
        assert!(sidecar_of(&alone, true).is_none());
        let now = sidecar_of(&rated, true).expect("the .gcd");
        assert!(now.json.is_some());

        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let row = |p: &Path| lib.by_path(p).unwrap().unwrap();
        assert_eq!(row(&alone).meta.rating, 0);
        assert_eq!(row(&rated).meta.rating, 2);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A culling tool's `IMG.xmp` beside a raw and its JPEG rates the
    /// raw's row, and the JPEG's row reads nothing of it.
    #[test]
    fn a_culling_tools_shared_xmp_rates_the_raw_and_not_its_jpeg() {
        use greycard_edit_io::xmp;
        let dir = scratch("culled");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"not a raw").unwrap();
        let jpeg = dir.join("IMG_0001.JPG");
        image::RgbImage::from_pixel(4, 3, image::Rgb([90, 120, 150]))
            .save(&jpeg)
            .unwrap();
        let packet = |rating: i32| {
            format!(
                r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF
                xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description
                rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
                xmp:Rating="{rating}" xmp:Label="Red"/></rdf:RDF></x:xmpmeta>"#
            )
        };
        std::fs::write(xmp::short_path(&raw), packet(4)).unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let row = |lib: &Library, p: &Path| lib.by_path(p).unwrap().unwrap();
        let e = row(&lib, &raw);
        assert_eq!((e.meta.rating, e.meta.label), (4, Label::Red));
        let e = row(&lib, &jpeg);
        assert_eq!((e.meta.rating, e.meta.label), (0, Label::None));

        // Rated again there: the next pass sees it.
        std::fs::write(xmp::short_path(&raw), packet(5)).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.meta_refreshed, 1, "{report:?}");
        assert_eq!(row(&lib, &raw).meta.rating, 5);
        assert_eq!(row(&lib, &jpeg).meta.rating, 0);
        // Reading wrote nothing into the folder.
        assert!(Sidecar::find(&raw).is_none());
        assert!(!xmp::long_path(&raw).exists());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A copy of a frame under another root is found by its hash there,
    /// whatever its path, the missing left out; and the whole file's
    /// hash a backup took is kept on the row until the file changes.
    #[test]
    fn a_copy_is_found_by_hash_under_a_root_and_keeps_its_whole_hash() {
        let dir = scratch("by-hash-under");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        std::fs::create_dir_all(local.join("shoot")).unwrap();
        std::fs::create_dir_all(nas.join("elsewhere").join("deeper")).unwrap();
        let (r5, r6, a7) = shoot(&local.join("shoot"));
        let copy = nas.join("elsewhere").join("deeper").join("renamed.tif");
        std::fs::copy(&r5, &copy).unwrap();
        // A neighbor, so the folder is not left empty when the copy goes.
        std::fs::copy(&r6, nas.join("elsewhere").join("deeper").join("r6.tif")).unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&local, &mut quiet()).unwrap();
        lib.index_tree(&nas, &mut quiet()).unwrap();
        let hash = lib.by_path(&r5).unwrap().unwrap().hash;
        let under: Vec<PathBuf> = lib
            .by_hash_under(&hash, std::slice::from_ref(&nas))
            .unwrap()
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert_eq!(under, std::slice::from_ref(&copy));
        assert_eq!(
            lib.by_hash_under(&hash, std::slice::from_ref(&local))
                .unwrap()
                .len(),
            1
        );
        assert!(lib.by_hash_under(&hash, &[]).unwrap().is_empty());
        let a7_hash = lib.by_path(&a7).unwrap().unwrap().hash;
        assert!(
            lib.by_hash_under(&a7_hash, std::slice::from_ref(&nas))
                .unwrap()
                .is_empty()
        );
        // Gone from the archive: missing, and not found there.
        std::fs::remove_file(&copy).unwrap();
        lib.index_tree(&nas, &mut quiet()).unwrap();
        assert!(
            lib.by_hash_under(&hash, std::slice::from_ref(&nas))
                .unwrap()
                .is_empty()
        );

        // The whole hash: kept only at the size it was taken at, and
        // gone when the file changes.
        let size = std::fs::metadata(&r5).unwrap().len();
        assert_eq!(lib.whole_hash(&r5).unwrap(), None);
        assert!(!lib.set_whole_hash(&r5, size + 1, "x").unwrap());
        assert!(lib.set_whole_hash(&r5, size, "abc").unwrap());
        assert_eq!(lib.whole_hash(&r5).unwrap().as_deref(), Some("abc"));
        lib.index_tree(&local, &mut quiet()).unwrap();
        assert_eq!(
            lib.whole_hash(&r5).unwrap().as_deref(),
            Some("abc"),
            "an unchanged file keeps it"
        );
        let mut bytes = std::fs::read(&r5).unwrap();
        bytes.extend_from_slice(b"more");
        std::fs::write(&r5, &bytes).unwrap();
        lib.index_tree(&local, &mut quiet()).unwrap();
        assert_eq!(lib.whole_hash(&r5).unwrap(), None);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A schema 5 library gains the two sync tables empty, in place,
    /// and its rows are kept.
    #[test]
    fn a_schema_5_library_gains_the_sync_tables() {
        let dir = scratch("schema5");
        let (_, _, _) = shoot(&dir);
        let db = dir.join("library.sqlite");
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&dir, &mut quiet()).unwrap();
            lib.conn_mut()
                .execute_batch(
                    "DROP TABLE pending; DROP TABLE archive_writes;
                     ALTER TABLE files DROP COLUMN exported; ALTER TABLE files DROP COLUMN develop_key;
                     ALTER TABLE files DROP COLUMN mosaic; DROP INDEX files_taken_at; ALTER TABLE files DROP COLUMN taken_at; ALTER TABLE files DROP COLUMN shift; ALTER TABLE files DROP COLUMN focus; PRAGMA user_version = 5;",
                )
                .unwrap();
        }
        let mut lib = Library::open(&db).unwrap();
        let version: i32 = lib
            .conn_mut()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, crate::SCHEMA_VERSION);
        assert!(lib.pending_all().unwrap().is_empty());
        assert_eq!(lib.archive_write("abc", &dir, &dir).unwrap(), None);
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        // Schema 8 asks the two sidecars again for the exports in them;
        // the file with none is not read.
        assert_eq!(
            (report.meta_refreshed, report.unchanged),
            (2, 1),
            "{report:?}"
        );
        // Windows holds the file while the library is open.
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A schema 6 library's rows with a maker's tags are marked to be
    /// read again: the reader learned three makers and changed which
    /// frames group. A row whose tags gave no maker is left as it was.
    #[test]
    fn a_schema_6_library_reads_the_makers_tags_again() {
        let dir = scratch("schema6");
        let (r5, r6, _) = shoot(&dir);
        let db = dir.join("library.sqlite");
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&dir, &mut quiet()).unwrap();
            lib.conn_mut()
                .execute(
                    "UPDATE files SET maker = 'Nikon' WHERE path = ?",
                    params![path_bytes(&r5)],
                )
                .unwrap();
            lib.conn_mut()
                .execute(
                    "UPDATE files SET maker = 'Canon' WHERE path = ?",
                    params![path_bytes(&r6)],
                )
                .unwrap();
            lib.conn_mut()
                .execute_batch("ALTER TABLE files DROP COLUMN exported; ALTER TABLE files DROP COLUMN develop_key; ALTER TABLE files DROP COLUMN mosaic; DROP INDEX files_taken_at; ALTER TABLE files DROP COLUMN taken_at; ALTER TABLE files DROP COLUMN shift; ALTER TABLE files DROP COLUMN focus; PRAGMA user_version = 6;")
                .unwrap();
        }
        let mut lib = Library::open(&db).unwrap();
        let version: i32 = lib
            .conn_mut()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, crate::SCHEMA_VERSION);
        let unread: Vec<(String, i64)> = lib
            .conn_mut()
            .prepare("SELECT name, style_read FROM files ORDER BY name")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        let name = |p: &Path| p.file_name().unwrap().to_string_lossy().into_owned();
        for (row, read) in &unread {
            assert_eq!(*read == 0, *row == name(&r5) || *row == name(&r6), "{row}");
        }
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.styled, 2, "{report:?}");
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// What we last wrote to an archive copy, and the writes that
    /// wait: keyed by the frame's hash and the archive, a frame once
    /// each, the archive's path as the index keeps roots.
    #[test]
    fn archive_writes_and_pending_are_kept_per_frame_archive_and_copy() {
        let dir = scratch("sync-tables");
        let archive = dir.join("nas");
        let other = dir.join("disk");
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let frame = dir.join("IMG_0001.CR3");
        std::fs::write(&frame, b"raw").unwrap();
        let copy = archive.join("shoot").join("IMG_0001.CR3");
        let selects = archive.join("selects").join("IMG_0001.CR3");
        let mut lib = Library::open_in_memory().unwrap();
        assert_eq!(lib.archive_write("h1", &archive, &copy).unwrap(), None);
        lib.record_archive_write(
            "h1",
            &archive,
            &copy,
            &ArchiveWrite {
                path: copy.with_extension("CR3.gcd"),
                revision: "r1".into(),
                size: 10,
                mtime: 100,
                written: 0,
            },
        )
        .unwrap();
        let w = lib.archive_write("h1", &archive, &copy).unwrap().unwrap();
        assert_eq!((w.revision.as_str(), w.size, w.mtime), ("r1", 10, 100));
        assert_eq!(w.path, copy.with_extension("CR3.gcd"));
        assert!(w.written > 0);
        // Written again: one row, the latest. Another copy of the same
        // hash on the same archive is its own row.
        lib.record_archive_write(
            "h1",
            &archive,
            &copy,
            &ArchiveWrite {
                path: copy.with_extension("CR3.gcd"),
                revision: "r2".into(),
                size: 12,
                mtime: 200,
                written: 0,
            },
        )
        .unwrap();
        lib.record_archive_write(
            "h1",
            &archive,
            &selects,
            &ArchiveWrite {
                path: selects.with_extension("CR3.gcd"),
                revision: "r9".into(),
                size: 1,
                mtime: 1,
                written: 0,
            },
        )
        .unwrap();
        let w = lib.archive_write("h1", &archive, &copy).unwrap().unwrap();
        assert_eq!((w.revision.as_str(), w.size, w.mtime), ("r2", 12, 200));
        assert_eq!(
            lib.archive_write("h1", &archive, &selects)
                .unwrap()
                .unwrap()
                .revision,
            "r9"
        );
        assert_eq!(lib.archive_write("h1", &other, &copy).unwrap(), None);

        assert!(!lib.is_pending("h1").unwrap());
        lib.add_pending("h1", &archive, &copy, &frame, "offline")
            .unwrap();
        lib.add_pending("h2", &archive, &copy, &frame, "offline")
            .unwrap();
        lib.add_pending("h1", &other, &copy, &frame, "quiet")
            .unwrap();
        lib.add_pending("h1", &archive, Path::new(""), &frame, "two copies")
            .unwrap();
        assert!(lib.is_pending("h1").unwrap());
        let for_archive = lib.pending_for(&archive).unwrap();
        assert_eq!(for_archive.len(), 3);
        let first = for_archive
            .iter()
            .find(|p| p.hash == "h1" && p.copy == copy)
            .unwrap();
        assert_eq!(first.archive, crate::canonical(&archive).unwrap());
        assert_eq!(first.frame, crate::canonical(&frame).unwrap());
        assert_eq!((first.tries, first.reason.as_str()), (1, "offline"));
        let since = first.since;
        assert!(
            for_archive
                .iter()
                .any(|p| p.copy.as_os_str().is_empty() && p.reason == "two copies")
        );
        // Queued again: the same row, one more try, the new reason.
        lib.add_pending("h1", &archive, &copy, &frame, "diverged")
            .unwrap();
        let again = lib.pending_for(&archive).unwrap();
        let row = again
            .iter()
            .find(|p| p.hash == "h1" && p.copy == copy)
            .unwrap();
        assert_eq!((row.tries, row.reason.as_str()), (2, "diverged"));
        assert_eq!(row.since, since);
        lib.note_pending("h1", &archive, &copy, &frame, "unreadable")
            .unwrap();
        let row = lib
            .pending_for(&archive)
            .unwrap()
            .into_iter()
            .find(|p| p.hash == "h1" && p.copy == copy)
            .unwrap();
        assert_eq!((row.tries, row.reason.as_str()), (2, "unreadable"));
        let moved = dir.join("IMG_0002.CR3");
        std::fs::write(&moved, b"raw").unwrap();
        lib.move_pending("h1", &archive, &copy, &frame, &moved)
            .unwrap();
        let row = lib
            .pending_for(&archive)
            .unwrap()
            .into_iter()
            .find(|p| p.hash == "h1" && p.copy == copy)
            .unwrap();
        assert_eq!(row.frame, crate::canonical(&moved).unwrap());
        // A second frame of the same hash waits on a row of its own,
        // its copy unknown too; the first's rows are not touched.
        let twin = dir.join("selects").join("IMG_0001.CR3");
        std::fs::create_dir_all(twin.parent().unwrap()).unwrap();
        std::fs::write(&twin, b"raw").unwrap();
        lib.add_pending("h1", &archive, Path::new(""), &twin, "two copies")
            .unwrap();
        let mut counts = lib.pending_counts().unwrap();
        counts.sort();
        let mut expect = vec![
            PendingCount {
                archive: crate::canonical(&other).unwrap(),
                total: 1,
                unknown: 0,
            },
            PendingCount {
                archive: crate::canonical(&archive).unwrap(),
                total: 4,
                unknown: 2,
            },
        ];
        expect.sort();
        assert_eq!(counts, expect);
        assert_eq!(lib.pending_all().unwrap().len(), 5);
        assert!(lib.clear_pending("h1", &archive, &copy, &moved).unwrap());
        assert!(!lib.clear_pending("h1", &archive, &copy, &moved).unwrap());
        assert!(lib.is_pending("h1").unwrap(), "still pending elsewhere");
        assert!(lib.clear_pending("h1", &other, &copy, &frame).unwrap());
        assert!(
            lib.clear_pending("h1", &archive, Path::new(""), &frame)
                .unwrap()
        );
        assert!(lib.is_pending("h1").unwrap(), "the twin's row is its own");
        assert!(
            lib.clear_pending("h1", &archive, Path::new(""), &twin)
                .unwrap()
        );
        assert!(!lib.is_pending("h1").unwrap());
        assert_eq!(lib.pending_for(&archive).unwrap().len(), 1);
        // Moved onto a frame that waits already: one row, not two.
        lib.add_pending("h2", &archive, &copy, &moved, "offline")
            .unwrap();
        lib.move_pending("h2", &archive, &copy, &frame, &moved)
            .unwrap();
        assert_eq!(lib.pending_for(&archive).unwrap().len(), 1);
        // The frame deleted by the editor: its rows go with its row.
        lib.add_pending("h2", &other, &copy, &moved, "offline")
            .unwrap();
        lib.forget(std::slice::from_ref(&moved)).unwrap();
        assert!(!lib.is_pending("h2").unwrap());
        // A root taken out of the library: the rows of frames under
        // it, and the rows owed to it as an archive, go; the rest stay.
        let kept = dir.join("kept").join("IMG_0003.CR3");
        std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
        std::fs::write(&kept, b"raw").unwrap();
        lib.add_pending("h3", &archive, &copy, &frame, "offline")
            .unwrap();
        lib.add_pending("h4", &other, &copy, &kept, "offline")
            .unwrap();
        lib.add_pending("h5", &archive, &copy, &kept, "offline")
            .unwrap();
        assert_eq!(lib.clear_pending_under(&archive).unwrap(), 2, "h3 and h5");
        assert_eq!(
            lib.clear_pending_under(dir.join("kept").as_path()).unwrap(),
            1,
            "h4"
        );
        assert!(!lib.is_pending("h3").unwrap() && !lib.is_pending("h5").unwrap());
        assert!(!lib.is_pending("h4").unwrap());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// `open_current` opens a library at this build's schema for
    /// writing and refuses anything else without a byte changed: an
    /// older library (the window's to bring up), one that is not
    /// there (nothing made), a file that is not a library.
    #[test]
    fn open_current_refuses_an_older_library_untouched_and_makes_nothing() {
        let dir = scratch("open-current");
        let db = dir.join("library.sqlite");
        let wait = std::time::Duration::from_millis(50);
        // Not there: nothing made.
        assert!(Library::open_current(&db, wait).is_err());
        assert!(!db.exists());
        // At schema 5: refused, and the file is as it was.
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(crate::SCHEMA).unwrap();
            conn.execute_batch("ALTER TABLE files DROP COLUMN exported; ALTER TABLE files DROP COLUMN develop_key;")
                .unwrap();
            conn.pragma_update(None, "application_id", crate::APPLICATION_ID)
                .unwrap();
            conn.execute_batch(
                "ALTER TABLE files DROP COLUMN mosaic; DROP INDEX files_taken_at;
                 ALTER TABLE files DROP COLUMN taken_at; ALTER TABLE files DROP COLUMN shift; ALTER TABLE files DROP COLUMN focus;",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 5).unwrap();
        }
        let before = std::fs::read(&db).unwrap();
        assert!(matches!(
            Library::open_current(&db, wait),
            Err(crate::Error::NeedsRebuild(_))
        ));
        assert_eq!(std::fs::read(&db).unwrap(), before, "not a byte changed");
        let version: i32 = rusqlite::Connection::open(&db)
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, 5);
        // Brought up by the editor's open: then it opens, and writes.
        drop(Library::open(&db).unwrap());
        let mut lib = Library::open_current(&db, wait).unwrap();
        let frame = dir.join("IMG_0001.CR3");
        std::fs::write(&frame, b"raw").unwrap();
        lib.add_pending("h1", &dir, Path::new(""), &frame, "x")
            .unwrap();
        assert!(lib.is_pending("h1").unwrap());
        // Not a library at all.
        let other = dir.join("other.sqlite");
        rusqlite::Connection::open(&other)
            .unwrap()
            .execute_batch("CREATE TABLE t (x)")
            .unwrap();
        assert!(matches!(
            Library::open_current(&other, wait),
            Err(crate::Error::NotALibrary(_))
        ));
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A schema 4 library is brought up in place with the whole hash's
    /// column, every row without one.
    #[test]
    fn a_schema_4_library_gains_the_whole_hash_column() {
        let dir = scratch("migrate-4");
        let db = dir.join("library.sqlite");
        let shoot_dir = dir.join("shoot");
        std::fs::create_dir_all(&shoot_dir).unwrap();
        let (r5, _, _) = shoot(&shoot_dir);
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
            lib.conn_mut()
                .execute_batch(
                    "ALTER TABLE files DROP COLUMN whole_hash;
                     ALTER TABLE files DROP COLUMN exported; ALTER TABLE files DROP COLUMN develop_key;
                     ALTER TABLE files DROP COLUMN mosaic; DROP INDEX files_taken_at; ALTER TABLE files DROP COLUMN taken_at; ALTER TABLE files DROP COLUMN shift; ALTER TABLE files DROP COLUMN focus; PRAGMA user_version = 4;",
                )
                .unwrap();
        }
        let mut lib = Library::open(&db).unwrap();
        let version: i32 = lib
            .conn_mut()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, crate::SCHEMA_VERSION);
        assert_eq!(lib.len().unwrap(), 3);
        assert_eq!(lib.whole_hash(&r5).unwrap(), None);
        let report = lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
        // Only the sidecars are asked again, for schema 8's exports.
        assert_eq!(
            (report.meta_refreshed, report.unchanged),
            (2, 1),
            "{report:?}"
        );
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }
}
