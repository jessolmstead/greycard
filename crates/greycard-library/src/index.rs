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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use greycard_edit::meta::Meta;
use greycard_edit::{Placement, Sidecar};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use crate::{
    Error, Exif, Library, Result, StyleTags, canonical, canonical_file, filter, hash, mtime_of,
    now_secs, path_bytes, path_from_bytes, path_text,
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
    /// Of those, the ones whose sidecar was left behind at the old
    /// path and carried to the new one by the pass.
    pub carried: usize,
    /// Files whose size or mtime changed, read again whole.
    pub changed: usize,
    /// Those files, by path: whose pictures are to be made again.
    pub changed_files: Vec<PathBuf>,
    /// Files unchanged whose sidecar changed: the meta written again.
    pub meta_refreshed: usize,
    /// Files whose row was right already.
    pub unchanged: usize,
    /// Files unchanged whose maker's tags were read for the first
    /// time: rows from a library made before the tags were indexed.
    /// Counted among `unchanged` as well.
    pub styled: usize,
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
    /// The pass was asked to stop and did, between two batches: what
    /// it wrote is written, and nothing was marked missing, since the
    /// files it did not reach were not looked for.
    pub stopped: bool,
}

impl Report {
    /// Files looked at.
    pub fn seen(&self) -> usize {
        self.added + self.moved + self.changed + self.meta_refreshed + self.unchanged
    }

    fn add(&mut self, other: Report) {
        self.added += other.added;
        self.moved += other.moved;
        self.carried += other.carried;
        self.changed += other.changed;
        self.changed_files.extend(other.changed_files);
        self.meta_refreshed += other.meta_refreshed;
        self.unchanged += other.unchanged;
        self.styled += other.styled;
        self.returned += other.returned;
        self.missing += other.missing;
        self.unavailable.extend(other.unavailable);
        self.errors.extend(other.errors);
        self.unreadable.extend(other.unreadable);
        self.skipped.extend(other.skipped);
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
}

/// Whether a path is one the index holds: a raw or a picture, as
/// the browser lists a folder.
pub fn is_indexed_path(path: &Path) -> bool {
    greycard_core::decode::is_raw_path(path) || greycard_core::picture::is_picture_path(path)
}

/// The files of one folder, sorted, not descending.
fn list_folder(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        // Regular files, or links to them: a named pipe's read would
        // never return.
        .filter(|p| p.is_file())
        .filter(|p| is_indexed_path(p))
        .collect();
    files.sort();
    Ok(files)
}

/// Whether a folder has no entries at all, hidden ones included.
fn is_empty_dir(dir: &Path) -> std::io::Result<bool> {
    Ok(std::fs::read_dir(dir)?.next().is_none())
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
        let (dir, exists) = resolve_folder(dir)?;
        let folder = path_bytes(&dir);
        if !exists && !has_rows_under(self.conn_mut(), &folder)? {
            return Err(no_such_folder(&dir));
        }
        let files = if exists {
            list_folder(&dir)?
        } else {
            Vec::new()
        };
        if exists
            && files.is_empty()
            && is_empty_dir(&dir)?
            && has_rows_under(self.conn_mut(), &folder)?
        {
            log::info!("{}: empty on disk, left as it was", dir.display());
            return Ok(Report {
                unavailable: vec![dir],
                ..Report::default()
            });
        }
        let existing = rows_in_folder(self.conn_mut(), &folder)?;
        index_paths(
            self.conn_mut(),
            &dir,
            &files,
            true,
            existing,
            progress,
            stop,
        )
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
        let (root, exists) = resolve_folder(root)?;
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
            let one = match self.index_folder_until(&dir, progress, stop) {
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
            let listing = match std::fs::read_dir(&dir) {
                Ok(l) => l,
                Err(e) if dir != root => {
                    walk.report.errors.push((dir.clone(), e.to_string()));
                    walk.report.unreadable.push(dir.clone());
                    walk.shielded.push(under_prefix(&path_bytes(&dir)));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            for entry in listing.filter_map(|e| e.ok()) {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                if kind.is_symlink() && entry.path().is_dir() {
                    walk.report.skipped.push(entry.path());
                } else if kind.is_dir() && !walk.leaves_out(&entry.path()) {
                    under.push(entry.path());
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
        for folder in gone {
            // The cheap questions first: a left-out folder (a share below
            // the root) is not asked of the disk at all.
            if walk.visited.contains(&folder)
                || walk.leaves_out(&path_from_bytes(&folder))
                || walk.shielded.iter().any(|s| folder.starts_with(s))
                || path_from_bytes(&folder).is_dir()
            {
                continue;
            }
            let marked = self.conn_mut().execute(
                "UPDATE files SET missing_since = ? WHERE folder = ? AND missing_since IS NULL",
                params![now, folder],
            )?;
            log::info!(
                "{}: gone, {marked} files marked missing",
                path_from_bytes(&folder).display()
            );
            walk.report.missing += marked;
        }
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
        let files: Vec<PathBuf> = if path.is_file() {
            vec![path]
        } else {
            Vec::new()
        };
        index_paths(
            self.conn_mut(),
            &folder,
            &files,
            false,
            existing,
            &mut |_| {},
            &|| false,
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
    })
}

fn rows_in_folder(conn: &Connection, folder: &[u8]) -> Result<HashMap<Vec<u8>, Row>> {
    let mut stmt = conn.prepare_cached(
        "SELECT path, id, size, mtime, sidecar, sidecar_hash, missing_since, style_read \
         FROM files WHERE folder = ?",
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
            "SELECT id, size, mtime, sidecar, sidecar_hash, missing_since, style_read \
             FROM files WHERE path = ?",
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
/// carried sidecar's. True when a sidecar was carried.
fn carry_sidecar(old: &Path, new: &Path) -> bool {
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
    match move_noreplace(&from, &to) {
        Ok(()) => {
            log::info!("{}: sidecar carried from {}", to.display(), from.display());
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

/// A sidecar is small and is read whole: its hash is what says
/// whether it changed, since a save that changes one digit of a
/// rating changes neither its length nor, within one timestamp
/// tick, its mtime. The XMP beside the frame is read the same way
/// and folded into the hash, so a rating given in another tool is
/// seen by the next pass as a save here is.
fn sidecar_of(raw: &Path) -> Option<SidecarNow> {
    disk_call();
    let path = Sidecar::find(raw);
    let xmp = greycard_edit::xmp::path_of(raw);
    if path.is_none() && xmp.is_none() {
        return None;
    }
    let json = path.as_deref().and_then(|p| std::fs::read(p).ok());
    let xmp_bytes = xmp.as_deref().and_then(|p| std::fs::read(p).ok());
    if json.is_none() && xmp_bytes.is_none() {
        return None;
    }
    let mtime = path
        .as_deref()
        .or(xmp.as_deref())
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| mtime_of(&m))
        .unwrap_or(0);
    // A frame with a `.gcd` alone hashes as it always did; the XMP,
    // when there is one, goes in after it with its length, so that a
    // byte moved from one file to the other is a change too.
    let mut hasher = blake3::Hasher::new();
    hasher.update(json.as_deref().unwrap_or_default());
    if let Some(x) = &xmp_bytes {
        hasher.update(&(x.len() as u64).to_le_bytes());
        hasher.update(x);
    }
    Some(SidecarNow {
        path,
        hash: hasher.finalize().to_hex().to_string(),
        mtime,
        json,
        xmp,
    })
}

impl Row {
    fn same_sidecar(&self, now: Option<&SidecarNow>) -> bool {
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
    /// tags when the row had not had them read.
    Same(Row, Option<StyleTags>),
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
fn index_paths(
    conn: &mut Connection,
    folder: &Path,
    files: &[PathBuf],
    whole_folder: bool,
    mut existing: HashMap<Vec<u8>, Row>,
    progress: &mut dyn FnMut(Progress<'_>),
    stop: &dyn Fn() -> bool,
) -> Result<Report> {
    let mut report = Report::default();
    let total = files.len();
    let folder_bytes = path_bytes(folder);
    let folder_text = path_text(folder);
    let mut looked: Vec<Looked<'_>> = Vec::with_capacity(BATCH);
    let mut disk = Disk {
        folder: &folder_bytes,
        listed: whole_folder.then(|| files.iter().map(|p| path_bytes(p)).collect()),
        in_use: HashMap::new(),
    };
    let mut since = Instant::now();
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
        disk_call();
        let stat = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                report.errors.push((path.clone(), e.to_string()));
                continue;
            }
        };
        let (size, mtime) = (stat.len(), mtime_of(&stat));
        let hashed = |report: &mut Report| {
            disk_call();
            match hash::hash_file(path) {
                Ok(h) => Some(h),
                Err(e) => {
                    report.errors.push((path.clone(), e.to_string()));
                    None
                }
            }
        };
        let mut moved = None;
        let plan = match existing.remove(&key) {
            Some(row) if row.size == size && row.mtime == mtime => {
                let style = (!row.style_read).then(|| read_style(path));
                Plan::Same(row, style)
            }
            Some(row) => {
                let Some(hash) = hashed(&mut report) else {
                    continue;
                };
                let exif = probe(path, &mut report);
                Plan::Changed { row, hash, exif }
            }
            None => {
                let Some(hash) = hashed(&mut report) else {
                    continue;
                };
                let found = gone_by_hash(conn, &hash, &mut disk)?;
                let exif = if found.is_some() {
                    None
                } else {
                    Some(probe(path, &mut report))
                };
                if let Some((_, old)) = &found
                    && carry_sidecar(&path_from_bytes(old), path)
                {
                    report.carried += 1;
                }
                moved = found;
                Plan::Fresh { hash, exif }
            }
        };
        let sidecar = sidecar_of(path);
        let summary = summary_of(sidecar.as_ref(), path);
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
    // have moved the row on since the folder's rows were read.
    if existing.values().any(|r| !r.missing) {
        let tx = begin(conn)?;
        let now = now_secs();
        for (key, row) in existing.iter().filter(|(_, r)| !r.missing) {
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
            Plan::Same(row, style) => {
                let snapshot = row.clone();
                let row = current(tx, &key, row)?;
                if let Some(style) = style.filter(|_| !row.style_read) {
                    write_style(tx, row.id, &style)?;
                    report.styled += 1;
                }
                settle_same(tx, &snapshot, row, seen, report)?;
            }
            Plan::Changed { row, hash, exif } => {
                let snapshot = row.clone();
                let row = current(tx, &key, row)?;
                update_file(tx, row.id, size, mtime, &hash, &exif)?;
                if !wrote_since(&snapshot, &row) && !row.same_sidecar(seen.sidecar) {
                    write_meta(tx, row.id, seen)?;
                }
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
                } else {
                    let exif = exif.unwrap_or_else(|| probe(path, report));
                    tx.prepare_cached(
                        "INSERT INTO files (path, folder, folder_text, name, size, mtime, hash, \
                         make, model, camera, lens, iso, focal, aperture, shutter, taken, \
                         maker, style, style_fixed, peripheral, style_read) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)",
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
         missing_since = NULL, whole_hash = NULL WHERE id = ?",
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
        id
    ])?;
    Ok(())
}

/// The maker's tags into a row that had not had them read.
fn write_style(tx: &Transaction<'_>, id: i64, style: &StyleTags) -> Result<()> {
    tx.prepare_cached(
        "UPDATE files SET maker = ?, style = ?, style_fixed = ?, peripheral = ?, \
         style_read = 1 WHERE id = ?",
    )?
    .execute(params![
        style.maker,
        style.style,
        style.fixed,
        style.peripheral,
        id
    ])?;
    Ok(())
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
}

impl Disk<'_> {
    /// Whether a row's file is on disk: for a row in the folder being
    /// indexed whole, by the listing; for any other, by a stat.
    fn has(&self, row_folder: &[u8], row_path: &[u8]) -> bool {
        match &self.listed {
            Some(listed) if row_folder == self.folder => listed.contains(row_path),
            _ => {
                disk_call();
                path_from_bytes(row_path).exists()
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
        let in_use = dir.is_dir() && !is_empty_dir(&dir).unwrap_or(true);
        self.in_use.insert(folder.to_vec(), in_use);
        in_use
    }
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
        } else {
            greycard_core::decode::probe_path(path)
        }
    });
    let failed = match probed {
        Ok(Ok(p)) => {
            return Exif {
                style: read_style(path),
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

/// A raw's maker tags, as the index keeps them; nothing for a picture,
/// which has no camera JPEG to be fitted against, and nothing for a
/// file the reader cannot read, with a line in the log. Not an error
/// in the report: a file whose tags will not read still indexes, and
/// is not read again until it changes.
fn read_style(path: &Path) -> StyleTags {
    disk_call();
    if !greycard_core::decode::is_raw_path(path) {
        return StyleTags::default();
    }
    match greycard_core::decode::CameraStyle::read(path) {
        Ok(Some(style)) => StyleTags::from_style(&style),
        Ok(None) => StyleTags::default(),
        Err(e) => {
            log::warn!("{}: the maker's tags: {e}", path.display());
            StyleTags::default()
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
}

/// The `.gcd`'s JSON read for what the row mirrors of it, before the
/// XMP beside the frame has its say.
#[derive(Default)]
struct Parsed {
    meta: Meta,
    turn: u8,
    /// What the sidecar last took from an XMP, so the same XMP is not
    /// taken again.
    mark: Option<greycard_edit::xmp::Adopted>,
    /// The edit, brought up to this build's shape; none when there is
    /// no `current` in the file.
    current: Option<greycard_edit::Edit>,
    /// A step in the history, or a snapshot.
    stepped: bool,
    /// The file, or its edit, could not be read: there is something
    /// in it, whatever it is.
    unreadable: bool,
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
    let value: serde_json::Value = match serde_json::from_slice(json) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("{}: {e}", sidecar.display());
            return Parsed {
                unreadable: true,
                ..Parsed::default()
            };
        }
    };
    let meta: Meta = value
        .get("meta")
        .cloned()
        .and_then(|m| serde_json::from_value(m).ok())
        .unwrap_or_default();
    let mark: Option<greycard_edit::xmp::Adopted> = value
        .get("xmp")
        .cloned()
        .and_then(|m| serde_json::from_value(m).ok());
    let count = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_array())
            .map_or(0, Vec::len)
    };
    let (current, unreadable) = match value.get("current").cloned().map(|c| {
        greycard_edit::migrate(c)
            .and_then(|c| Ok(serde_json::from_value::<greycard_edit::Edit>(c)?))
    }) {
        None => (None, false),
        Some(Ok(edit)) => (Some(edit), false),
        Some(Err(e)) => {
            log::warn!("{}: the edit was not read: {e}", sidecar.display());
            (None, true)
        }
    };
    Parsed {
        meta,
        turn: value
            .get("turn")
            .and_then(|t| t.as_u64())
            .map_or(0, |t| (t % 4) as u8),
        mark,
        current,
        stepped: count("history") > 0 || count("snapshots") > 0,
        unreadable,
    }
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
    let p = parsed(now.json.as_deref(), &said);
    let default = if greycard_core::picture::is_picture_path(raw) {
        greycard_edit::Edit::for_picture()
    } else {
        greycard_edit::Edit::default()
    };
    let mut sidecar = Sidecar {
        current: p.current.clone().unwrap_or_else(|| default.clone()),
        meta: p.meta,
        turn: p.turn,
        xmp: p.mark,
        ..Sidecar::default()
    };
    if now.xmp.is_some() {
        // The camera's tag is asked for only when the packet says
        // something about the orientation, as the editor asks.
        greycard_edit::xmp::adopt(raw, &mut sidecar, || {
            disk_call();
            greycard_core::decode::orientation_path(raw)
                .inspect_err(|e| log::warn!("{}: orientation: {e}", raw.display()))
                .ok()
        });
    }
    let edited = p.stepped || p.unreadable || sidecar.current != default;
    Summary {
        meta: sidecar.meta,
        edited,
        turns: crate::pack_turns(sidecar.current.geometry.shown_turns(sidecar.turn)),
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
    };
    summary_of(Some(&now), raw.unwrap_or(Path::new("frame.cr3")))
}

/// The row's meta from its sidecar as the first phase summed it up, or
/// the empty meta when it has none, and the sidecar's hash so the next
/// pass can tell.
fn write_meta(tx: &Transaction<'_>, id: i64, seen: Seen<'_>) -> Result<()> {
    let (sidecar, summary) = (seen.sidecar, seen.summary);
    let meta = &summary.meta;
    tx.prepare_cached(
        "UPDATE files SET sidecar = ?, sidecar_hash = ?, sidecar_mtime = ?, rating = ?, \
         flag = ?, label = ?, keywords = ?, edited = ?, turns = ? WHERE id = ?",
    )?
    .execute(params![
        sidecar.and_then(|s| s.path.as_deref().map(path_bytes)),
        sidecar.map(|s| s.hash.as_str()),
        sidecar.map(|s| s.mtime),
        i64::from(meta.rating),
        filter::flag_name(meta.flag),
        filter::label_name(meta.label),
        serde_json::to_string(&meta.keywords).unwrap_or_else(|_| "[]".into()),
        i64::from(summary.edited),
        i64::from(summary.turns),
        id
    ])?;
    tx.prepare_cached("DELETE FROM keywords WHERE file = ?")?
        .execute(params![id])?;
    let mut insert =
        tx.prepare_cached("INSERT OR IGNORE INTO keywords (file, word) VALUES (?, ?)")?;
    for word in &meta.keywords {
        insert.execute(params![id, word])?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::fixture::{A7, R5, R6, write_frame};
    use crate::{Entry, Filter, Library};
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        assert_eq!(lib.by_path(&bare).unwrap().unwrap().exif, Exif::default());
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_changed_sidecar_refreshes_the_meta_and_nothing_else() {
        let dir = scratch("meta");
        let (r5, _r6, a7) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let before = lib.by_path(&r5).unwrap().unwrap();

        // The R5 loses a star; the A7 gains a sidecar.
        let mut s = Sidecar::load(&r5).unwrap().unwrap();
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
        let mut s = Sidecar::load(&r5).unwrap().unwrap();
        s.meta.rating = 5;
        s.save(&r5).unwrap();
        let report = lib.index_file(&r5).unwrap();
        assert_eq!(report.meta_refreshed, 1, "{report:?}");
        assert_eq!(lib.by_path(&r5).unwrap().unwrap().meta.rating, 5);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A save that changes one digit changes neither the sidecar's
    /// length nor, within one filesystem tick, its mtime. The hash
    /// of its bytes still sees it.
    #[test]
    fn a_same_size_save_in_the_same_tick_is_still_seen() {
        let dir = scratch("tick");
        let (r5, _, _) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let gcd = Sidecar::find(&r5).unwrap();
        let before = std::fs::metadata(&gcd).unwrap();
        let mut s = Sidecar::load(&r5).unwrap().unwrap();
        s.meta.rating = 3;
        s.save(&r5).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::rename(&r5, &renamed).unwrap();
        std::fs::rename(Sidecar::path_for(&r5), Sidecar::path_for(&renamed)).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(
            report,
            Report {
                moved: 1,
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
        std::fs::rename(&a7, &moved).unwrap();
        let report = lib.index_folder(&dir, &mut quiet()).unwrap();
        assert_eq!(report.missing, 1, "{report:?}");
        assert!(lib.by_path(&moved).unwrap().is_none());
        assert!(names(&lib, "").iter().all(|n| n != "a7.tif"));
        assert_eq!(names(&lib, "missing:yes"), ["a7.tif"]);
        let report = lib.index_folder(&sub, &mut quiet()).unwrap();
        assert_eq!(report.moved, 1, "{report:?}");
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
        std::fs::remove_dir_all(&dir).unwrap();
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

        // Renamed within the folder, the raw alone: the same.
        let renamed = picks.join("z.tif");
        std::fs::rename(&r5_to, &renamed).unwrap();
        let report = lib.index_folder(&picks, &mut quiet()).unwrap();
        assert_eq!((report.moved, report.carried), (1, 1), "{report:?}");
        assert_eq!(Sidecar::find(&renamed), Some(Sidecar::path_for(&renamed)));
        assert!(Sidecar::find(&r5_to).is_none());
        assert_eq!(lib.by_path(&renamed).unwrap().unwrap().meta.rating, 4);

        // A frame with no sidecar moves as before.
        let a7_to = picks.join("a7.tif");
        std::fs::rename(&a7, &a7_to).unwrap();
        let report = lib.index_tree(&dir, &mut quiet()).unwrap();
        assert_eq!((report.moved, report.carried), (1, 0), "{report:?}");
        assert!(Sidecar::find(&a7_to).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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

        std::fs::remove_dir_all(&day1).unwrap();
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
        std::fs::remove_dir_all(&day2).unwrap();
        let report = lib.index_folder(&day2, &mut quiet()).unwrap();
        assert_eq!(report.missing, 1, "{report:?}");
        assert!(lib.by_path(&day2.join("x.tif")).unwrap().unwrap().missing);
        // A folder whose parent is gone too is a drive gone, and
        // an error rather than a marking.
        assert!(
            lib.index_folder(&dir.join("nowhere").join("deep"), &mut quiet())
                .is_err()
        );
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&gone).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A pass asks the disk everything in its first phase and nothing
    /// under the write lock: over files whose rows are current (the
    /// common case, a re-poll of an indexed archive), with a sidecar
    /// changed, an XMP appearing, a file new and a file renamed. So a
    /// share that stops answering mid-pass stops it holding no lock.
    #[test]
    fn a_batch_asks_the_disk_nothing_under_the_lock() {
        use greycard_edit::xmp;
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
        let mut s = Sidecar::load(&r5).unwrap().unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
                    let mut s = Sidecar::load(&r5).unwrap().unwrap();
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
        assert_eq!(Sidecar::load(&r5).unwrap().unwrap().meta.rating, 5);
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
                    let mut s = Sidecar::load(&r5).unwrap().unwrap();
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
        let mut s = Sidecar::load(&r5).unwrap().unwrap();
        s.meta.rating = 2;
        s.save(&r5).unwrap();
        let report = lib
            .index_folder(&dir, &mut |p| {
                if p.path == last {
                    let mut s = Sidecar::load(&r5).unwrap().unwrap();
                    s.meta.rating = 1;
                    s.save(&r5).unwrap();
                    other.index_file(&r5).unwrap();
                }
            })
            .unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(lib.by_path(&r5).unwrap().unwrap().meta.rating, 1);
        assert_eq!(Sidecar::load(&r5).unwrap().unwrap().meta.rating, 1);
        drop(other);
        drop(lib);
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&sub).unwrap();
        assert_eq!(lib.index_folder(&sub, &mut quiet()).unwrap().missing, 1);
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
            .rows_in_canonical(&ab)
            .unwrap()
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(own, [ab.join("b.tif"), ab.join("c.tif")]);
        assert_eq!(lib.count_in_canonical(&ab).unwrap(), 2);
        assert_eq!(lib.count_in_canonical(&dir).unwrap(), 0);
        assert!(lib.rows_in_canonical(&dir).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
            std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(dir.join("gone")).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
            current: Edit::for_picture(),
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
        s.record(e);
        s.turn = 1;
        s.save(&turned).unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut quiet()).unwrap();
        let rows = lib
            .rows_under_canonical(std::slice::from_ref(&dir))
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
        let mut s = Sidecar::load(&rated).unwrap().unwrap();
        let mut e = s.current.clone();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
                     PRAGMA user_version = 3;",
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
        // The shoot's sidecars carry a raw's default edit on a picture,
        // which is a develop of a picture.
        assert_eq!((e.meta.rating, e.edited), (4, true));
        let report = lib.index_folder(&shoot_dir, &mut quiet()).unwrap();
        assert_eq!(report.meta_refreshed, 0, "{report:?}");
        drop(lib);
        std::fs::remove_dir_all(&dir).unwrap();
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
        use greycard_edit::xmp;
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
            .rows_under_canonical(std::slice::from_ref(&dir))
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
                     PRAGMA user_version = 4;",
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
        assert_eq!(report.unchanged, 3, "nothing read again: {report:?}");
        drop(lib);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
