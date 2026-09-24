//! The library: directories as the truth, this index as a cache
//! (notes §72).
//!
//! One SQLite file, one row a file: where it is, how big it is and
//! when it changed, a content hash that names it when its path
//! cannot, the EXIF worth filtering on, and its sidecar's meta —
//! rating, flag, label, keywords — mirrored in with a hash of the
//! sidecar's bytes so a changed sidecar always wins. Nothing here is
//! authoritative: the raw and its `.gcd` are, and the index can be
//! deleted and rebuilt from them at any time.
//!
//! Indexing a folder is incremental. A file whose size and mtime
//! the index already holds is not read; a sidecar whose bytes
//! changed refreshes the meta and nothing else; a file that arrives
//! under a new path with a hash the index knows, gone from a folder
//! that is still there, is a move and keeps its row. A file gone
//! from a folder, or a whole folder gone from under a tree, is
//! marked missing rather than forgotten, so the move can be found
//! from either side, and [`Library::prune_missing`] forgets them
//! when asked.
//!
//! The filter language over the index is [`filter`]; the CLI's
//! `library list` is its first surface and the filter bar its
//! second. This crate depends on `greycard-edit` for the sidecar and
//! `greycard-core` for the probe, and never on a UI.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use greycard_core::decode::Probe;
use greycard_core::raw::{Shot, camera_name};
use greycard_edit::meta::Meta;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};

pub mod filter;
pub mod hash;
mod index;
pub mod thumbs;

pub use filter::{Filter, ParseError};
pub use hash::hash_file;
pub use index::{Progress, Report, is_indexed_path, read_meta};
pub use thumbs::{Thumb, Thumbs};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no data directory: XDG_DATA_HOME is not set and the platform names none")]
    NoDataDir,
    #[error("no cache directory: XDG_CACHE_HOME is not set and the platform names none")]
    NoCacheDir,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("library database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("filter: {0}")]
    Filter(#[from] ParseError),
    /// A SQLite file that is not a greycard library: another
    /// program's, or one made by hand. Left alone rather than
    /// rebuilt over.
    #[error("{0} is not a greycard library")]
    NotALibrary(PathBuf),
    /// A library written by a later build. Refused rather than
    /// rebuilt, since the later build may still want it.
    #[error("{path} is schema {found}, written by a later greycard than this one ({ours})")]
    NewerSchema {
        path: PathBuf,
        found: i32,
        ours: i32,
    },
    /// An older library opened read-only, which cannot be brought
    /// up to date without writing.
    #[error("{0} is an older library; open it for writing once to rebuild it")]
    NeedsRebuild(PathBuf),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The schema this build writes. A library at an older version is
/// dropped and rebuilt on open: it is a cache, and a migration would
/// be more code than the rebuild costs. A newer one is refused.
pub const SCHEMA_VERSION: i32 = 2;

/// `PRAGMA application_id`: "GRCY", so a SQLite file that is not a
/// library is never rebuilt over.
pub const APPLICATION_ID: i32 = 0x4752_4359;

/// The database's name under the data directory.
pub const FILE_NAME: &str = "library.sqlite";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS files (
    id            INTEGER PRIMARY KEY,
    path          BLOB NOT NULL UNIQUE,
    folder        BLOB NOT NULL,
    folder_text   TEXT NOT NULL,
    name          TEXT NOT NULL,
    size          INTEGER NOT NULL,
    mtime         INTEGER NOT NULL,
    hash          TEXT NOT NULL,
    make          TEXT,
    model         TEXT,
    camera        TEXT,
    lens          TEXT,
    iso           INTEGER,
    focal         REAL,
    aperture      REAL,
    shutter       REAL,
    taken         TEXT,
    sidecar       BLOB,
    sidecar_hash  TEXT,
    sidecar_mtime INTEGER,
    rating        INTEGER NOT NULL DEFAULT 0,
    flag          TEXT NOT NULL DEFAULT 'none',
    label         TEXT NOT NULL DEFAULT 'none',
    keywords      TEXT NOT NULL DEFAULT '[]',
    missing_since INTEGER
);
CREATE INDEX IF NOT EXISTS files_folder ON files(folder);
CREATE INDEX IF NOT EXISTS files_hash ON files(hash);
CREATE INDEX IF NOT EXISTS files_taken ON files(taken);
CREATE TABLE IF NOT EXISTS keywords (
    file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    word TEXT NOT NULL,
    PRIMARY KEY (file, word)
);
CREATE INDEX IF NOT EXISTS keywords_word ON keywords(word);
";

/// The columns [`Entry`] is read from, in the order `entry` reads
/// them.
const COLUMNS: &str = "files.id, files.path, files.size, files.mtime, files.hash, \
    files.make, files.model, files.camera, files.lens, files.iso, files.focal, \
    files.aperture, files.shutter, files.taken, files.sidecar, files.sidecar_mtime, \
    files.rating, files.flag, files.label, files.keywords, files.missing_since";

/// The index, open.
pub struct Library {
    conn: Connection,
    path: PathBuf,
    read_only: bool,
}

/// What a file's EXIF says that a filter can ask about.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Exif {
    pub make: String,
    pub model: String,
    /// Make and model joined as the panel shows them, the make not
    /// said twice (`greycard_core::raw::camera_name`): `Canon EOS
    /// R5`, `Nikon Z 6 3`, `Sony ILCE-7M4`, as rawler spells the
    /// bodies.
    pub camera: String,
    pub lens: Option<String>,
    pub iso: Option<u32>,
    /// Millimeters.
    pub focal: Option<f64>,
    /// The f-number.
    pub aperture: Option<f64>,
    /// Seconds.
    pub shutter: Option<f64>,
    /// `YYYY-MM-DD HH:MM:SS`, as far as the file said; lexically
    /// ordered, which is what the date filter compares.
    pub taken: Option<String>,
}

impl Exif {
    /// From a probe, the date normalized.
    pub fn from_probe(probe: &Probe) -> Exif {
        Exif {
            make: probe.make.clone(),
            model: probe.model.clone(),
            camera: camera_name(&probe.make, &probe.model),
            lens: probe.lens_model.clone(),
            iso: probe.iso,
            focal: probe.focal_length,
            aperture: probe.fnumber,
            shutter: probe.exposure_time,
            taken: probe.taken.as_deref().and_then(normalize_taken),
        }
    }

    /// The exposure as the engine's [`Shot`] carries it, for
    /// [`Shot::summary`] and the lens lookup.
    pub fn shot(&self) -> Shot {
        Shot {
            lens_make: None,
            lens_model: self.lens.clone(),
            focal_length: self.focal.map(|v| v as f32),
            f_number: self.aperture.map(|v| v as f32),
            focus_distance: None,
            iso: self.iso,
            exposure_time: self.shutter.map(|v| v as f32),
        }
    }
}

/// EXIF's `YYYY:MM:DD HH:MM:SS` to `YYYY-MM-DD HH:MM:SS`, so the
/// column sorts and compares lexically and a filter's `2026-09` is a
/// prefix of it. What does not fit that shape after the colons are
/// turned — a camera with no clock writes spaces, a tag edited by
/// hand has a letter where a digit should be, rawler's replacement
/// character for a byte it could not decode — is no date rather
/// than a date that `date:2024-01` would find and `date:2024-01-05`
/// would not. Counted in characters, not bytes: the first cut split
/// inside a multibyte character and took the process down.
pub fn normalize_taken(exif: &str) -> Option<String> {
    let s = exif.trim();
    let date: String = s
        .chars()
        .take(10)
        .map(|c| if c == ':' { '-' } else { c })
        .collect();
    let time: String = s.chars().skip(10).collect();
    let time = time.trim();
    let shaped = |text: &str, len: usize, seps: [usize; 2], sep: char| {
        text.chars().count() == len
            && text.chars().enumerate().all(|(i, c)| {
                if seps.contains(&i) {
                    c == sep
                } else {
                    c.is_ascii_digit()
                }
            })
    };
    if !shaped(&date, 10, [4, 7], '-') || date.starts_with("0000") {
        return None;
    }
    if time.is_empty() {
        return Some(date);
    }
    shaped(time, 8, [2, 5], ':').then(|| format!("{date} {time}"))
}

/// One file as the index holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: i64,
    pub path: PathBuf,
    pub size: u64,
    /// Nanoseconds since the epoch.
    pub mtime: i64,
    /// The content hash, hex; see [`hash`].
    pub hash: String,
    pub exif: Exif,
    pub meta: Meta,
    /// The sidecar the meta came from, wherever it was found, and
    /// its mtime; `None` when the file has none.
    pub sidecar: Option<PathBuf>,
    pub sidecar_mtime: Option<i64>,
    /// Whether the file was gone from its path the last time its
    /// folder was indexed.
    pub missing: bool,
}

impl Entry {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// A file's mtime as the index stores it.
pub(crate) fn mtime_of(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// Now, in seconds, for `missing_since`.
pub(crate) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A path's exact bytes, as the `path` and `folder` columns hold
/// them: the OS string's own bytes on Unix, the UTF-16 units on
/// Windows. Two names that a lossy conversion would spell the same
/// stay two paths.
#[cfg(unix)]
pub(crate) fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
pub(crate) fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

#[cfg(windows)]
pub(crate) fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(windows)]
pub(crate) fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let (pairs, _) = bytes.as_chunks::<2>();
    let wide: Vec<u16> = pairs.iter().map(|c| u16::from_le_bytes(*c)).collect();
    PathBuf::from(std::ffi::OsString::from_wide(&wide))
}

/// A path as the text columns show it, for the `folder:` filter and
/// the log; lossy, and never what a row is matched by.
pub(crate) fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The canonical form of a path, without Windows' `\\?\` prefix.
pub(crate) fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// A file's path as the index keys it: its folder canonical, its own
/// name kept. Canonicalizing the file too would follow a link to its
/// target, and a folder pass lists the link under its own name, so
/// the link's row would never be the one a save or a lookup found.
pub(crate) fn canonical_file(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => {
            let parent = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            nearest_canonical(parent).join(name)
        }
        _ => canonical(path).unwrap_or_else(|_| path.to_path_buf()),
    }
}

/// A folder's canonical path even when the folder is gone: the
/// nearest ancestor that is there, canonical, with the rest of the
/// path put back under it. A lookup by a path under a deleted folder
/// must still key the way the index stored it, which matters where
/// the temporary directory is a link (macOS's `/var` is `/private/var`).
pub(crate) fn nearest_canonical(path: &Path) -> PathBuf {
    match canonical(path) {
        Ok(p) => p,
        Err(_) => match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
                nearest_canonical(parent).join(name)
            }
            _ => path.to_path_buf(),
        },
    }
}

impl Library {
    /// Where the user's library is: `greycard/library.sqlite` under
    /// `$XDG_DATA_HOME` when that is set, else under the platform's
    /// local data directory — `~/.local/share` on Linux, `~/Library/
    /// Application Support` on macOS, `%LOCALAPPDATA%` on Windows
    /// (local, not roaming: an index of this machine's disks has no
    /// business following a profile to another). Under data rather
    /// than cache, since a rebuild of a large library is hours and
    /// cache cleaners wipe `~/.cache` (§72).
    pub fn user_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(dirs::data_local_dir)?;
        Some(Self::path_under(&base))
    }

    /// The library's path under a data directory.
    pub fn path_under(data_dir: &Path) -> PathBuf {
        data_dir.join("greycard").join(FILE_NAME)
    }

    /// Open the user's library, making it if it is not there.
    pub fn open_user() -> Result<Library> {
        Self::open(&Self::user_path().ok_or(Error::NoDataDir)?)
    }

    /// Open a library at `path`, making it and its directory if they
    /// are not there. A library from an older schema is emptied and
    /// rebuilt, with a warning; a newer one, or a SQLite file that
    /// is not a library, is refused.
    pub fn open(path: &Path) -> Result<Library> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        Self::prepare(conn, path.to_path_buf(), false)
    }

    /// Open a library for reading only: a listing beside a running
    /// index, which under write-ahead logging never waits for it.
    /// The file has to be there.
    pub fn open_read_only(path: &Path) -> Result<Library> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Self::prepare(conn, path.to_path_buf(), true)
    }

    /// A library that lives only as long as the process: for tests
    /// and for a listing nobody wants kept.
    pub fn open_in_memory() -> Result<Library> {
        Self::prepare(
            Connection::open_in_memory()?,
            PathBuf::from(":memory:"),
            false,
        )
    }

    fn prepare(conn: Connection, path: PathBuf, read_only: bool) -> Result<Library> {
        // Per-connection settings, none of them a write to the file.
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.create_scalar_function(
            "ulower",
            1,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                // Text folded with Unicode's rules; anything else
                // is NULL, which no text test matches.
                Ok(match ctx.get_raw(0) {
                    rusqlite::types::ValueRef::Text(t) => {
                        Some(String::from_utf8_lossy(t).to_lowercase())
                    }
                    _ => None,
                })
            },
        )?;
        // What the file is, read in one transaction so that the
        // three answers are from one moment: read one at a time, they
        // straddled another process's making of the schema and read
        // "no id, no version, tables" — somebody else's file.
        let mut conn = conn;
        let state = {
            let tx = conn.transaction()?;
            let state = State::read(&tx)?;
            tx.commit()?;
            state
        };
        match state.judge() {
            Judgement::Ours => {
                if !read_only {
                    conn.pragma_update(None, "synchronous", "NORMAL")?;
                }
                return Ok(Library {
                    conn,
                    path,
                    read_only,
                });
            }
            Judgement::NotOurs => return Err(Error::NotALibrary(path)),
            Judgement::Newer => {
                return Err(Error::NewerSchema {
                    path,
                    found: state.version,
                    ours: SCHEMA_VERSION,
                });
            }
            Judgement::ToMake => {}
        }
        // Fresh, or older: the file is written, which a read-only
        // connection cannot do.
        if read_only {
            return Err(Error::NeedsRebuild(path));
        }
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        // The making, as one write transaction taken from the start:
        // two processes opening a library that is not there yet both
        // find it fresh, and the second waits here for the first and
        // then finds the schema made. The first cut had both make
        // it, and the second got "database is locked".
        {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Judged again under the write lock: the other opener may
            // have made it while this one waited.
            let now = State::read(&tx)?;
            match now.judge() {
                Judgement::Ours => {}
                Judgement::NotOurs => return Err(Error::NotALibrary(path)),
                Judgement::Newer => {
                    return Err(Error::NewerSchema {
                        path,
                        found: now.version,
                        ours: SCHEMA_VERSION,
                    });
                }
                Judgement::ToMake => {
                    if !now.fresh() {
                        log::warn!(
                            "{}: schema version {}, this build writes {SCHEMA_VERSION}; \
                             rebuilding",
                            path.display(),
                            now.version
                        );
                        tx.execute_batch(
                            "DROP TABLE IF EXISTS keywords; DROP TABLE IF EXISTS files;",
                        )?;
                    }
                    tx.execute_batch(SCHEMA)?;
                    tx.pragma_update(None, "application_id", APPLICATION_ID)?;
                    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
                }
            }
            tx.commit()?;
        }
        // Write-ahead logging is a property of the file and is set
        // once, here: a listing then reads while an index writes.
        // `synchronous=NORMAL` is safe under WAL: a crash loses the
        // last transaction, never the file, and the index is
        // rebuildable anyway. The switch wants the file to itself for
        // a moment and does not wait for it, so it is tried again
        // while another connection is busy; an in-memory database has
        // no WAL and says so, which is not an error either.
        for attempt in 0..50 {
            match conn.pragma_update(None, "journal_mode", "WAL") {
                Ok(()) => break,
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == rusqlite::ErrorCode::DatabaseBusy && attempt < 49 =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(_) => break,
            }
        }
        Ok(Library {
            conn,
            path,
            read_only,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// How many files the index holds, the missing among them.
    pub fn len(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM files", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// The files that pass `filter`, by folder then name. A file the
    /// index last found missing is left out unless the filter asks
    /// about missing files.
    pub fn query(&self, filter: &Filter) -> Result<Vec<Entry>> {
        let (body, params) = filter.to_sql();
        let sql = format!(
            "SELECT {COLUMNS} FROM files WHERE ({body}){} ORDER BY files.folder, files.name",
            if filter.mentions_missing() {
                ""
            } else {
                " AND files.missing_since IS NULL"
            }
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter().map(param)), entry)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// How many files pass `filter`, under the same rule as
    /// [`Library::query`].
    pub fn count(&self, filter: &Filter) -> Result<usize> {
        let (body, params) = filter.to_sql();
        let sql = format!(
            "SELECT count(*) FROM files WHERE ({body}){}",
            if filter.mentions_missing() {
                ""
            } else {
                " AND files.missing_since IS NULL"
            }
        );
        let n: i64 = self
            .conn
            .prepare_cached(&sql)?
            .query_row(rusqlite::params_from_iter(params.iter().map(param)), |r| {
                r.get(0)
            })?;
        Ok(n as usize)
    }

    /// Every file with this content hash, missing ones included: a
    /// copy on two disks is two rows with one hash.
    pub fn by_hash(&self, hash: &str) -> Result<Vec<Entry>> {
        let sql = format!("SELECT {COLUMNS} FROM files WHERE hash = ? ORDER BY files.path");
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params![hash], entry)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The file at this path, if the index holds it. The path is
    /// canonicalized when it can be, as the index stores it.
    pub fn by_path(&self, path: &Path) -> Result<Option<Entry>> {
        let stored = canonical_file(path);
        let sql = format!("SELECT {COLUMNS} FROM files WHERE path = ?");
        Ok(self
            .conn
            .prepare_cached(&sql)?
            .query_row(params![path_bytes(&stored)], entry)
            .optional()?)
    }

    /// The folders the index holds files in, sorted.
    pub fn folders(&self) -> Result<Vec<PathBuf>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT DISTINCT folder FROM files ORDER BY folder")?;
        let rows = stmt.query_map([], |r| r.get::<_, Vec<u8>>(0).map(|b| path_from_bytes(&b)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Forget every file in the library last found missing; how many
    /// went.
    pub fn prune_missing(&mut self) -> Result<usize> {
        Ok(self
            .conn
            .execute("DELETE FROM files WHERE missing_since IS NOT NULL", [])?)
    }

    /// The database's size on disk, for the numbers.
    pub fn size_on_disk(&self) -> Result<u64> {
        if self.path.as_os_str() == ":memory:" {
            return Ok(0);
        }
        Ok(std::fs::metadata(&self.path)?.len())
    }

    /// Fold the write-ahead log into the file, for a size that means
    /// something. Passive: it does what it can without blocking
    /// anyone, and never invokes the busy handler. A truncating
    /// checkpoint, the first cut's, waits for every other connection
    /// and then resets the log under them, and a pass on another
    /// connection starting a read at that moment got "database is
    /// locked" with no busy handler to wait it out.
    pub fn checkpoint(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(())
    }

    pub(crate) fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

/// What a SQLite file says it is, read at one moment.
struct State {
    app_id: i32,
    version: i32,
    has_tables: bool,
    /// A `files` table with this crate's `hash` and `folder` columns:
    /// a table by that name alone is anybody's.
    has_our_files: bool,
}

enum Judgement {
    /// A library at this schema.
    Ours,
    /// Fresh, or an older library: to be made.
    ToMake,
    /// Somebody else's SQLite file.
    NotOurs,
    /// A later build's library.
    Newer,
}

impl State {
    fn read(conn: &Connection) -> rusqlite::Result<State> {
        Ok(State {
            app_id: conn.query_row("PRAGMA application_id", [], |r| r.get(0))?,
            version: conn.query_row("PRAGMA user_version", [], |r| r.get(0))?,
            has_tables: conn.query_row(
                "SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table'",
                [],
                |r| r.get(0),
            )?,
            has_our_files: conn.query_row(
                "SELECT count(*) = 2 FROM pragma_table_info('files') \
                 WHERE name IN ('hash', 'folder')",
                [],
                |r| r.get(0),
            )?,
        })
    }

    fn fresh(&self) -> bool {
        self.app_id == 0 && self.version == 0 && !self.has_tables
    }

    fn judge(&self) -> Judgement {
        // Schema 1 was written before the application id was: a
        // `files` table with this crate's columns at that version
        // with no id is an early library of ours, and is rebuilt like
        // any older one.
        let early = self.app_id == 0 && self.version == 1 && self.has_our_files;
        if !self.fresh() && !early && self.app_id != APPLICATION_ID {
            Judgement::NotOurs
        } else if self.version > SCHEMA_VERSION {
            Judgement::Newer
        } else if self.version == SCHEMA_VERSION {
            Judgement::Ours
        } else {
            Judgement::ToMake
        }
    }
}

fn param(p: &filter::Param) -> rusqlite::types::Value {
    match p {
        filter::Param::Text(s) => rusqlite::types::Value::Text(s.clone()),
        filter::Param::Int(i) => rusqlite::types::Value::Integer(*i),
        filter::Param::Real(r) => rusqlite::types::Value::Real(*r),
    }
}

/// An [`Entry`] from a row of [`COLUMNS`].
fn entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    let keywords: String = row.get(19)?;
    let mut meta = Meta {
        rating: row.get::<_, i64>(16)?.clamp(0, 255) as u8,
        flag: filter::flag_from_name(&row.get::<_, String>(17)?),
        label: filter::label_from_name(&row.get::<_, String>(18)?),
        ..Meta::default()
    };
    meta.set_keywords(serde_json::from_str(&keywords).unwrap_or_default());
    Ok(Entry {
        id: row.get(0)?,
        path: path_from_bytes(&row.get::<_, Vec<u8>>(1)?),
        size: row.get::<_, i64>(2)? as u64,
        mtime: row.get(3)?,
        hash: row.get(4)?,
        exif: Exif {
            make: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            model: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            camera: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
            lens: row.get(8)?,
            iso: row.get::<_, Option<i64>>(9)?.map(|i| i as u32),
            focal: row.get(10)?,
            aperture: row.get(11)?,
            shutter: row.get(12)?,
            taken: row.get(13)?,
        },
        meta,
        sidecar: row
            .get::<_, Option<Vec<u8>>>(14)?
            .map(|b| path_from_bytes(&b)),
        sidecar_mtime: row.get(15)?,
        missing: row.get::<_, Option<i64>>(20)?.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_date_is_normalized_for_lexical_order() {
        assert_eq!(
            normalize_taken("2024:08:24 15:32:39").as_deref(),
            Some("2024-08-24 15:32:39")
        );
        assert_eq!(normalize_taken("2024-08-24").as_deref(), Some("2024-08-24"));
        assert_eq!(
            normalize_taken("  2024:08:24 ").as_deref(),
            Some("2024-08-24")
        );
        assert_eq!(normalize_taken("    :  :     :  :  "), None);
        assert_eq!(normalize_taken("0000:00:00 00:00:00"), None);
        assert_eq!(normalize_taken(""), None);
    }

    /// A multibyte character where a digit should be — a tag edited
    /// by hand, or rawler's replacement character for a byte it
    /// could not decode — used to split the string inside the
    /// character and take the process down.
    #[test]
    fn a_bad_date_is_no_date_and_not_a_panic() {
        assert_eq!(normalize_taken("2024:01:0é 10:00:00"), None);
        assert_eq!(normalize_taken("2024:01:0\u{fffd} 10:00:00"), None);
        assert_eq!(normalize_taken("2024:0é"), None);
        assert_eq!(normalize_taken("é024:01:01"), None);
        // The shape, exactly: a date, or a date and a time.
        assert_eq!(normalize_taken("2024:01:05 10:00"), None);
        assert_eq!(normalize_taken("2024:01:05 10:00:0x"), None);
        assert_eq!(normalize_taken("2024:1:5"), None);
        assert_eq!(normalize_taken("2024-01-05T10:00:00"), None);
        assert_eq!(
            normalize_taken("2024:01:05 10:00:00").as_deref(),
            Some("2024-01-05 10:00:00")
        );
    }

    #[test]
    fn the_user_path_is_under_the_data_directory() {
        assert_eq!(
            Library::path_under(Path::new("/home/x/.local/share")),
            PathBuf::from("/home/x/.local/share/greycard/library.sqlite")
        );
    }

    #[test]
    fn an_exif_becomes_a_shot_the_panel_can_say() {
        let exif = Exif::from_probe(&Probe {
            make: "Canon".into(),
            model: "Canon EOS R5".into(),
            lens_model: Some("RF35mm F1.4 L VCM".into()),
            focal_length: Some(35.0),
            iso: Some(100),
            exposure_time: Some(1.0 / 2500.0),
            fnumber: Some(2.0),
            taken: Some("2024:08:24 15:32:39".into()),
            ..Probe::default()
        });
        assert_eq!(exif.camera, "Canon EOS R5");
        assert_eq!(exif.taken.as_deref(), Some("2024-08-24 15:32:39"));
        let summary = exif.shot().summary(&exif.make, &exif.model);
        assert_eq!(summary.camera, "Canon EOS R5 · RF35mm F1.4 L VCM");
        assert_eq!(summary.exposure, "35 mm · f/2 · 1/2500 s · ISO 100");
    }

    #[test]
    fn an_older_library_is_rebuilt_and_a_newer_one_refused() {
        let dir = crate::index::tests::scratch("schema");
        let path = dir.join("library.sqlite");
        {
            let lib = Library::open(&path).unwrap();
            lib.conn
                .execute(
                    "INSERT INTO files (path, folder, folder_text, name, size, mtime, hash) \
                     VALUES (x'2f61', x'2f', '/', 'a', 1, 1, 'h')",
                    [],
                )
                .unwrap();
            assert_eq!(lib.len().unwrap(), 1);
            lib.conn.pragma_update(None, "user_version", 1).unwrap();
        }
        // Older: rebuilt, empty, at this version and still marked ours.
        let lib = Library::open(&path).unwrap();
        assert_eq!(lib.len().unwrap(), 0);
        let version: i32 = lib
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let app: i32 = lib
            .conn
            .query_row("PRAGMA application_id", [], |r| r.get(0))
            .unwrap();
        assert_eq!(app, APPLICATION_ID);
        // Older and read-only: cannot be rebuilt, says so.
        lib.conn.pragma_update(None, "user_version", 1).unwrap();
        drop(lib);
        assert!(matches!(
            Library::open_read_only(&path),
            Err(Error::NeedsRebuild(_))
        ));
        // Newer: refused, and left as it was.
        {
            let lib = Library::open(&path).unwrap();
            lib.conn
                .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .unwrap();
        }
        match Library::open(&path) {
            Err(Error::NewerSchema { found, ours, .. }) => {
                assert_eq!((found, ours), (SCHEMA_VERSION + 1, SCHEMA_VERSION));
            }
            other => panic!("{:?}", other.map(|_| ())),
        }
        assert!(matches!(
            Library::open_read_only(&path),
            Err(Error::NewerSchema { .. })
        ));
        let conn = Connection::open(&path).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION + 1);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A SQLite file that is somebody else's — no application id,
    /// but tables — is not rebuilt over, whatever its user_version
    /// says.
    #[test]
    fn a_sqlite_file_that_is_not_a_library_is_left_alone() {
        let dir = crate::index::tests::scratch("notours");
        let path = dir.join("other.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE files (id INTEGER PRIMARY KEY, note TEXT); \
                 INSERT INTO files (note) VALUES ('mine');",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)
                .unwrap();
        }
        assert!(matches!(Library::open(&path), Err(Error::NotALibrary(_))));
        assert!(matches!(
            Library::open_read_only(&path),
            Err(Error::NotALibrary(_))
        ));
        let conn = Connection::open(&path).unwrap();
        let note: String = conn
            .query_row("SELECT note FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(note, "mine");
        drop(conn);
        // A library of another application id, likewise.
        let path = dir.join("theirs.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "application_id", 0x1234).unwrap();
            conn.execute_batch("CREATE TABLE t (x);").unwrap();
        }
        assert!(matches!(Library::open(&path), Err(Error::NotALibrary(_))));
        // But schema 1 with a `files` table and no id is an early
        // library of this crate's, and is rebuilt.
        // Not on the name alone: somebody's `files` at version 1
        // without this crate's columns is theirs, and keeps its rows.
        let path = dir.join("theirs-v1.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE files (id INTEGER PRIMARY KEY, note TEXT); \
                 INSERT INTO files (note) VALUES ('precious');",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
        }
        assert!(matches!(Library::open(&path), Err(Error::NotALibrary(_))));
        let conn = Connection::open(&path).unwrap();
        let note: String = conn
            .query_row("SELECT note FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(note, "precious");
        drop(conn);
        let path = dir.join("early.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE files (id INTEGER PRIMARY KEY, path TEXT, folder TEXT, hash TEXT);",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
        }
        let lib = Library::open(&path).unwrap();
        assert_eq!(lib.len().unwrap(), 0);
        let app: i32 = lib
            .conn
            .query_row("PRAGMA application_id", [], |r| r.get(0))
            .unwrap();
        assert_eq!(app, APPLICATION_ID);
        drop(lib);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Two processes opening a library that is not there yet both
    /// make it, and both must come out with the one library. Four
    /// threads at once, on five fresh paths.
    #[test]
    fn a_library_can_be_made_by_several_at_once() {
        let dir = crate::index::tests::scratch("race");
        for round in 0..5 {
            let path = dir.join(format!("lib{round}")).join("library.sqlite");
            let openers: Vec<_> = (0..4)
                .map(|_| {
                    let path = path.clone();
                    std::thread::spawn(move || Library::open(&path).map(|l| l.len().unwrap()))
                })
                .collect();
            for opener in openers {
                let opened = opener.join().unwrap();
                assert!(opened.is_ok(), "round {round}: {:?}", opened.err());
                assert_eq!(opened.unwrap(), 0);
            }
            let lib = Library::open_read_only(&path).unwrap();
            let version: i32 = lib
                .conn
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap();
            assert_eq!(version, SCHEMA_VERSION);
            let mode: String = lib
                .conn
                .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                .unwrap();
            assert_eq!(mode, "wal");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A link to a file is listed by a folder pass under its own
    /// name, so a lookup by the link's path must key the same way:
    /// the folder canonical, the name kept.
    #[cfg(unix)]
    #[test]
    fn a_file_path_keeps_its_own_name_when_keyed() {
        let dir = crate::index::tests::scratch("key");
        std::fs::write(dir.join("target.tif"), b"x").unwrap();
        std::os::unix::fs::symlink(dir.join("target.tif"), dir.join("link.tif")).unwrap();
        let keyed = canonical_file(&dir.join("link.tif"));
        assert_eq!(keyed.file_name().unwrap(), "link.tif");
        assert_eq!(keyed.parent().unwrap(), canonical(&dir).unwrap());
        // A file that is not there keys the same way, so a row for a
        // file gone can still be found.
        assert_eq!(
            canonical_file(&dir.join("gone.tif")),
            canonical(&dir).unwrap().join("gone.tif")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_path_round_trips_through_its_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let odd = PathBuf::from(std::ffi::OsStr::from_bytes(b"/a/caf\xe9.tif"));
        assert_eq!(path_from_bytes(&path_bytes(&odd)), odd);
        assert_eq!(path_bytes(Path::new("/a/b")), b"/a/b");
    }
}
