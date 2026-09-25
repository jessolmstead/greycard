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

use greycard_edit::Sidecar;
use greycard_edit::meta::Meta;
use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use crate::{
    Error, Exif, Library, Result, canonical, canonical_file, filter, hash, mtime_of, now_secs,
    path_bytes, path_from_bytes, path_text,
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
    /// Files whose size or mtime changed, read again whole.
    pub changed: usize,
    /// Those files, by path: whose pictures are to be made again.
    pub changed_files: Vec<PathBuf>,
    /// Files unchanged whose sidecar changed: the meta written again.
    pub meta_refreshed: usize,
    /// Files whose row was right already.
    pub unchanged: usize,
    /// Files back at a path the index had marked missing.
    pub returned: usize,
    /// Rows whose file, or whose whole folder, is gone.
    pub missing: usize,
    /// Folders found empty on disk with rows in the index under
    /// them: a drive not mounted, its mount point left behind, as
    /// likely as a shoot deleted, so left as they were.
    pub unavailable: Vec<PathBuf>,
    /// Files that could not be read, and why; each has a row anyway
    /// when it could be hashed.
    pub errors: Vec<(PathBuf, String)>,
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
        self.changed += other.changed;
        self.changed_files.extend(other.changed_files);
        self.meta_refreshed += other.meta_refreshed;
        self.unchanged += other.unchanged;
        self.returned += other.returned;
        self.missing += other.missing;
        self.unavailable.extend(other.unavailable);
        self.errors.extend(other.errors);
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

    fn stopped(&self) -> Report {
        Report {
            stopped: true,
            ..self.report.clone()
        }
    }
}

/// A folder's row as the pass needs it.
struct Row {
    id: i64,
    size: u64,
    mtime: i64,
    sidecar: Option<Vec<u8>>,
    sidecar_hash: Option<String>,
    missing: bool,
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
                } else if kind.is_dir() {
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
            if walk.visited.contains(&folder)
                || path_from_bytes(&folder).is_dir()
                || walk.shielded.iter().any(|s| folder.starts_with(s))
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
    })
}

fn rows_in_folder(conn: &Connection, folder: &[u8]) -> Result<HashMap<Vec<u8>, Row>> {
    let mut stmt = conn.prepare_cached(
        "SELECT path, id, size, mtime, sidecar, sidecar_hash, missing_since \
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
            "SELECT id, size, mtime, sidecar, sidecar_hash, missing_since \
             FROM files WHERE path = ?",
        )?
        .query_row(params![path], |r| row_from(r, 0))
        .optional()?)
}

/// The sidecar a file has now: where, its bytes' hash, its mtime,
/// and the bytes themselves for the meta to be read from once.
struct SidecarNow {
    path: PathBuf,
    hash: String,
    mtime: i64,
    json: Vec<u8>,
}

/// A sidecar is small and is read whole: its hash is what says
/// whether it changed, since a save that changes one digit of a
/// rating changes neither its length nor, within one timestamp
/// tick, its mtime.
fn sidecar_of(raw: &Path) -> Option<SidecarNow> {
    let path = Sidecar::find(raw)?;
    let json = std::fs::read(&path).ok()?;
    let mtime = std::fs::metadata(&path).map(|m| mtime_of(&m)).unwrap_or(0);
    Some(SidecarNow {
        hash: blake3::hash(&json).to_hex().to_string(),
        path,
        mtime,
        json,
    })
}

impl Row {
    fn same_sidecar(&self, now: Option<&SidecarNow>) -> bool {
        match now {
            None => self.sidecar.is_none(),
            Some(s) => {
                self.sidecar.as_deref() == Some(path_bytes(&s.path).as_slice())
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
    /// The row's size and mtime match the file's.
    Same(Row),
    /// The file changed on disk: hashed and probed again.
    Changed { row: Row, hash: String, exif: Exif },
    /// No row at this path when the folder's rows were read: hashed,
    /// and probed unless a move looked likely, since a move keeps
    /// its EXIF.
    Fresh { hash: String, exif: Option<Exif> },
}

/// One file looked at, with what the disk had to say of the file
/// itself. Its sidecar is not read here: that is read under the lock
/// in the second phase, so that a save made between the phases is
/// what gets written.
struct Looked<'a> {
    path: &'a Path,
    key: Vec<u8>,
    size: u64,
    mtime: i64,
    plan: Plan,
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
                + report.returned,
        });
        let key = path_bytes(path);
        let stat = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                report.errors.push((path.clone(), e.to_string()));
                continue;
            }
        };
        let (size, mtime) = (stat.len(), mtime_of(&stat));
        let hashed = |report: &mut Report| match hash::hash_file(path) {
            Ok(h) => Some(h),
            Err(e) => {
                report.errors.push((path.clone(), e.to_string()));
                None
            }
        };
        let plan = match existing.remove(&key) {
            Some(row) if row.size == size && row.mtime == mtime => Plan::Same(row),
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
                let exif = if gone_by_hash(conn, &hash, &mut disk)?.is_some() {
                    None
                } else {
                    Some(probe(path, &mut report))
                };
                Plan::Fresh { hash, exif }
            }
        };
        looked.push(Looked {
            path,
            key,
            size,
            mtime,
            plan,
        });
        if looked.len() >= BATCH || since.elapsed() >= BATCH_TIME {
            write_batch(
                conn,
                &folder_bytes,
                &folder_text,
                &mut looked,
                &mut existing,
                &mut disk,
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
        &mut disk,
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
    disk: &mut Disk<'_>,
    report: &mut Report,
) -> Result<()> {
    if looked.is_empty() {
        return Ok(());
    }
    let tx = begin(conn)?;
    for Looked {
        path,
        key,
        size,
        mtime,
        plan,
    } in looked.drain(..)
    {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        // The sidecar as it is now, under the lock: a save made since
        // the first phase, or since another writer's pass, is what
        // the row is held to.
        let sidecar = sidecar_of(path);
        match plan {
            Plan::Same(row) => {
                let row = current(&tx, &key, row)?;
                settle_same(&tx, row, sidecar.as_ref(), report)?;
            }
            Plan::Changed { row, hash, exif } => {
                let row = current(&tx, &key, row)?;
                update_file(&tx, row.id, size, mtime, &hash, &exif)?;
                if !row.same_sidecar(sidecar.as_ref()) {
                    write_meta(&tx, row.id, sidecar.as_ref())?;
                }
                report.changed += 1;
                report.changed_files.push(path.to_path_buf());
            }
            Plan::Fresh { hash, exif } => {
                if let Some(row) = row_at(&tx, &key)? {
                    // Added by another writer since the folder's rows
                    // were read; this row is current, and the sidecar
                    // it knows may be newer than the one read.
                    if row.size == size && row.mtime == mtime {
                        settle_same(&tx, row, sidecar.as_ref(), report)?;
                    } else {
                        let exif = exif.unwrap_or_else(|| probe(path, report));
                        update_file(&tx, row.id, size, mtime, &hash, &exif)?;
                        if !row.same_sidecar(sidecar.as_ref()) {
                            write_meta(&tx, row.id, sidecar.as_ref())?;
                        }
                        report.changed += 1;
                        report.changed_files.push(path.to_path_buf());
                    }
                } else if let Some((id, old_path)) = gone_by_hash(&tx, &hash, disk)? {
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
                    write_meta(&tx, id, sidecar.as_ref())?;
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
                         make, model, camera, lens, iso, focal, aperture, shutter, taken) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
                    ])?;
                    let id = tx.last_insert_rowid();
                    write_meta(&tx, id, sidecar.as_ref())?;
                    report.added += 1;
                }
            }
        }
    }
    tx.commit()?;
    Ok(())
}

/// A row whose file is as it was: back if it was missing, and its
/// meta refreshed if its sidecar is not the one it knew.
fn settle_same(
    tx: &Transaction<'_>,
    row: Row,
    sidecar: Option<&SidecarNow>,
    report: &mut Report,
) -> Result<()> {
    if row.missing {
        tx.prepare_cached("UPDATE files SET missing_since = NULL WHERE id = ?")?
            .execute(params![row.id])?;
        report.returned += 1;
    }
    if row.same_sidecar(sidecar) {
        report.unchanged += 1;
    } else {
        write_meta(tx, row.id, sidecar)?;
        report.meta_refreshed += 1;
    }
    Ok(())
}

/// A changed file's row: its new size, mtime, hash and EXIF.
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
         taken = ?, missing_since = NULL WHERE id = ?",
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
            _ => path_from_bytes(row_path).exists(),
        }
    }

    /// Whether a folder is there with something in it, asked of the
    /// disk once a folder a pass.
    fn folder_in_use(&mut self, folder: &[u8]) -> bool {
        if let Some(&known) = self.in_use.get(folder) {
            return known;
        }
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
/// have written it since, and the row is settled against what it
/// wrote and the sidecar as it is now. A row gone meanwhile (a prune)
/// is settled as the snapshot, and its updates by id touch nothing.
fn current(tx: &Transaction<'_>, key: &[u8], snapshot: Row) -> Result<Row> {
    Ok(row_at(tx, key)?.unwrap_or(snapshot))
}

/// The file's EXIF, a raw's through its decoder and a picture's
/// from its chunk; a file that will not say is an empty EXIF and a
/// line in the report. A decoder that panics on the file's data is
/// the same case, caught here so one file never ends the folder.
fn probe(path: &Path, report: &mut Report) -> Exif {
    let probed = std::panic::catch_unwind(|| {
        if greycard_core::picture::is_picture_path(path) {
            greycard_core::picture::probe_path(path)
        } else {
            greycard_core::decode::probe_path(path)
        }
    });
    let failed = match probed {
        Ok(Ok(p)) => return Exif::from_probe(&p),
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
    let value: serde_json::Value = match serde_json::from_slice(json) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("{}: {e}", sidecar.display());
            return Meta::default();
        }
    };
    value
        .get("meta")
        .cloned()
        .and_then(|m| serde_json::from_value(m).ok())
        .unwrap_or_default()
}

/// The row's meta from its sidecar, or the empty meta when it has
/// none, and the sidecar's hash so the next pass can tell.
fn write_meta(tx: &Transaction<'_>, id: i64, sidecar: Option<&SidecarNow>) -> Result<()> {
    let meta = sidecar
        .map(|s| meta_from_json(&s.json, &s.path))
        .unwrap_or_default();
    tx.prepare_cached(
        "UPDATE files SET sidecar = ?, sidecar_hash = ?, sidecar_mtime = ?, rating = ?, \
         flag = ?, label = ?, keywords = ? WHERE id = ?",
    )?
    .execute(params![
        sidecar.map(|s| path_bytes(&s.path)),
        sidecar.map(|s| s.hash.as_str()),
        sidecar.map(|s| s.mtime),
        i64::from(meta.rating),
        filter::flag_name(meta.flag),
        filter::label_name(meta.label),
        serde_json::to_string(&meta.keywords).unwrap_or_else(|_| "[]".into()),
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
    use greycard_edit::Placement;
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
            report.errors[0].1.contains("panicked"),
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
}
