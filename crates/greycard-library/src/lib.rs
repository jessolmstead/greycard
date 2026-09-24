//! The library: directories as the truth, this index as a cache
//! (notes §72).
//!
//! One SQLite file, one row a file: where it is, how big it is and
//! when it changed, a content hash that names it when its path
//! cannot, the EXIF worth filtering on, and its sidecar's meta —
//! rating, flag, label, keywords — mirrored in with the sidecar's
//! own mtime so a newer sidecar always wins. Nothing here is
//! authoritative: the raw and its `.gcd` are, and the index can be
//! deleted and rebuilt from them at any time.
//!
//! Indexing a folder is incremental. A file whose size and mtime
//! the index already holds is not read; a sidecar that changed
//! refreshes the meta and nothing else; a file that arrives under a
//! new path with a hash the index knows, whose old path is gone, is
//! a move and keeps its row. A file gone from a folder is marked
//! missing rather than forgotten, so the move can be found from
//! either side, and [`Library::prune_missing`] forgets them when
//! asked.
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
use rusqlite::{Connection, OptionalExtension, params};

pub mod filter;
pub mod hash;
mod index;

pub use filter::{Filter, ParseError};
pub use hash::hash_file;
pub use index::{Progress, Report};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no data directory: XDG_DATA_HOME is not set and the platform names none")]
    NoDataDir,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("library database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("filter: {0}")]
    Filter(#[from] ParseError),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The schema this build writes. A database at another version is
/// dropped and rebuilt on open: it is a cache, and a migration
/// would be more code than the rebuild costs.
pub const SCHEMA_VERSION: i32 = 1;

/// The database's name under the data directory.
pub const FILE_NAME: &str = "library.sqlite";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS files (
    id            INTEGER PRIMARY KEY,
    path          TEXT NOT NULL UNIQUE,
    folder        TEXT NOT NULL,
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
    sidecar       TEXT,
    sidecar_size  INTEGER,
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

/// The columns [`Entry`] is read from, in the order `entry_from_row`
/// reads them.
const COLUMNS: &str = "files.id, files.path, files.size, files.mtime, files.hash, \
    files.make, files.model, files.camera, files.lens, files.iso, files.focal, \
    files.aperture, files.shutter, files.taken, files.sidecar, files.sidecar_mtime, \
    files.rating, files.flag, files.label, files.keywords, files.missing_since";

/// The index, open.
pub struct Library {
    conn: Connection,
    path: PathBuf,
}

/// What a file's EXIF says that a filter can ask about.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Exif {
    pub make: String,
    pub model: String,
    /// Make and model joined as the panel shows them, the make not
    /// said twice: `Canon EOS R5`, `NIKON Z6_3`, `SONY ILCE-7M4`.
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
/// prefix of it. A date that does not start with four digits — a
/// camera with no clock writes spaces — is no date.
pub fn normalize_taken(exif: &str) -> Option<String> {
    let s = exif.trim();
    if s.len() < 4 || !s.bytes().take(4).all(|b| b.is_ascii_digit()) || s.starts_with("0000") {
        return None;
    }
    let (date, time) = s.split_at(s.len().min(10));
    let date = date.replace(':', "-");
    let time = time.trim();
    Some(if time.is_empty() {
        date
    } else {
        format!("{date} {time}")
    })
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

/// A path as the `path` column spells it.
pub(crate) fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl Library {
    /// Where the user's library is: `greycard/library.sqlite` under
    /// `$XDG_DATA_HOME` when that is set, else under the platform's
    /// data directory — `~/.local/share` on Linux, `~/Library/
    /// Application Support` on macOS, `%APPDATA%` on Windows. Under
    /// data rather than cache, since a rebuild of a large library is
    /// hours and cache cleaners wipe `~/.cache` (§72).
    pub fn user_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(dirs::data_dir)?;
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
    /// are not there. A database from another schema version is
    /// emptied and rebuilt, with a warning.
    pub fn open(path: &Path) -> Result<Library> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        Self::prepare(conn, path.to_path_buf())
    }

    /// A library that lives only as long as the process: for tests
    /// and for a listing nobody wants kept.
    pub fn open_in_memory() -> Result<Library> {
        Self::prepare(Connection::open_in_memory()?, PathBuf::from(":memory:"))
    }

    fn prepare(conn: Connection, path: PathBuf) -> Result<Library> {
        // Write-ahead logging keeps a listing readable while an
        // index runs, and `synchronous=NORMAL` is safe under WAL: a
        // crash loses the last transaction, never the file. The
        // index is rebuildable anyway.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != 0 && version != SCHEMA_VERSION {
            log::warn!(
                "{}: schema version {version}, this build writes {SCHEMA_VERSION}; rebuilding",
                path.display()
            );
            conn.execute_batch("DROP TABLE IF EXISTS keywords; DROP TABLE IF EXISTS files;")?;
        }
        conn.execute_batch(SCHEMA)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Library { conn, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
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
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let sql = format!("SELECT {COLUMNS} FROM files WHERE path = ?");
        Ok(self
            .conn
            .prepare_cached(&sql)?
            .query_row(params![path_text(&canonical)], entry)
            .optional()?)
    }

    /// The folders the index holds files in, sorted.
    pub fn folders(&self) -> Result<Vec<PathBuf>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT DISTINCT folder FROM files ORDER BY folder")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0).map(PathBuf::from))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Forget the files last found missing; how many went.
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
    /// something.
    pub fn checkpoint(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    pub(crate) fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
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
        path: PathBuf::from(row.get::<_, String>(1)?),
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
        sidecar: row.get::<_, Option<String>>(14)?.map(PathBuf::from),
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
    fn a_database_from_another_schema_is_rebuilt() {
        let dir = crate::index::tests::scratch("schema");
        let path = dir.join("library.sqlite");
        {
            let lib = Library::open(&path).unwrap();
            lib.conn
                .execute(
                    "INSERT INTO files (path, folder, name, size, mtime, hash) \
                          VALUES ('/a', '/', 'a', 1, 1, 'h')",
                    [],
                )
                .unwrap();
            assert_eq!(lib.len().unwrap(), 1);
            lib.conn.pragma_update(None, "user_version", 99).unwrap();
        }
        let lib = Library::open(&path).unwrap();
        assert_eq!(lib.len().unwrap(), 0);
        let version: i32 = lib
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        drop(lib);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
