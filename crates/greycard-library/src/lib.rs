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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use greycard_core::decode::Probe;
use greycard_core::raw::{Shot, camera_name};
use greycard_edit::meta::Meta;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};

pub mod filter;
#[cfg(any(test, feature = "fixtures"))]
pub mod fixture;
pub mod hash;
mod index;
pub mod roots;
pub mod thumbs;

pub use filter::{Facet, Filter, ParseError};
pub use hash::hash_file;
pub use index::{Progress, Report, TreeWalk, is_indexed_path, read_meta};
pub use roots::{Change, Roots, Watcher};
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
/// be more code than the rebuild costs. The exception is a step that
/// only adds columns a pass can fill without rehashing anything:
/// [`MIGRATIONS`] holds those, and a library they reach is brought up
/// in place, its rows marked for the next pass to fill. A newer one
/// is refused.
pub const SCHEMA_VERSION: i32 = 5;

/// The steps a library is brought up by in place rather than rebuilt:
/// from the version on the left to the next, the statements on the
/// right. Schema 3 added the maker's rendering tags, which the camera
/// match groups frames by; a schema 2 row gets them from the next
/// pass over its folder, which reads the tags of every row whose
/// `style_read` is zero and nothing else of the file. Schema 4 added
/// whether the sidecar holds a develop (`edited`) and the quarter
/// turns its picture is shown at (`turns`), both read from the
/// sidecar beside the meta; a schema 3 row's sidecar hash is cleared
/// so the next pass over its folder reads the sidecar again and fills
/// them, and nothing else of the file is read. Schema 5 added the
/// whole file's BLAKE3 (`whole_hash`, notes §216), which only a backup
/// or a bring-back fills, so a schema 4 row simply has none.
const MIGRATIONS: &[(i32, &str)] = &[
    (
        2,
        "ALTER TABLE files ADD COLUMN maker TEXT;
         ALTER TABLE files ADD COLUMN style TEXT;
         ALTER TABLE files ADD COLUMN style_fixed INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE files ADD COLUMN peripheral INTEGER;
         ALTER TABLE files ADD COLUMN style_read INTEGER NOT NULL DEFAULT 0;
         CREATE INDEX IF NOT EXISTS files_style ON files(model, style);",
    ),
    (
        3,
        "ALTER TABLE files ADD COLUMN edited INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE files ADD COLUMN turns INTEGER NOT NULL DEFAULT 0;
         UPDATE files SET sidecar_hash = NULL WHERE sidecar IS NOT NULL;",
    ),
    (4, "ALTER TABLE files ADD COLUMN whole_hash TEXT;"),
];

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
    missing_since INTEGER,
    maker         TEXT,
    style         TEXT,
    style_fixed   INTEGER NOT NULL DEFAULT 0,
    peripheral    INTEGER,
    style_read    INTEGER NOT NULL DEFAULT 0,
    edited        INTEGER NOT NULL DEFAULT 0,
    turns         INTEGER NOT NULL DEFAULT 0,
    whole_hash    TEXT
);
CREATE INDEX IF NOT EXISTS files_folder ON files(folder);
CREATE INDEX IF NOT EXISTS files_hash ON files(hash);
CREATE INDEX IF NOT EXISTS files_taken ON files(taken);
CREATE INDEX IF NOT EXISTS files_style ON files(model, style);
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
    files.rating, files.flag, files.label, files.keywords, files.missing_since, \
    files.maker, files.style, files.style_fixed, files.peripheral, files.edited, \
    files.turns";

/// The columns [`RowMeta`] is read from, in the order `row_meta`
/// reads them, after the path.
const META_COLUMNS: &str = "files.id, files.hash, files.mtime, files.rating, files.flag, files.label, \
    files.keywords, files.edited, files.turns";

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
    /// The maker's rendering tags, read beside the EXIF from the
    /// maker note; empty for a picture and for a raw the reader does
    /// not know.
    pub style: StyleTags,
}

/// What a raw says of the rendering its camera JPEG was made with,
/// as the index keeps it: `greycard_core::decode::CameraStyle` cut
/// down to what a query asks about.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyleTags {
    /// The maker as the reader names it: "Canon", "Fujifilm".
    pub maker: Option<String>,
    /// The group key, the maker and a fixed style: "Canon Faithful",
    /// "Fujifilm Reala Ace". None with no style, or a style that
    /// adapts to the scene itself.
    pub style: Option<String>,
    /// A fixed style with every adaptive setting off: a frame the
    /// camera match can fit on.
    pub fixed: bool,
    /// The camera's own vignetting correction, where the file says.
    pub peripheral: Option<bool>,
}

impl StyleTags {
    pub fn from_style(style: &greycard_core::decode::CameraStyle) -> StyleTags {
        StyleTags {
            maker: Some(style.maker.name().to_string()),
            style: style.group_key(),
            fixed: style.is_fixed(),
            peripheral: style.peripheral_correction,
        }
    }
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
            style: StyleTags::default(),
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
    /// Whether the sidecar holds a develop: see [`RowMeta::edited`].
    pub edited: bool,
    /// The quarter turns the picture is shown at, packed: see
    /// [`RowMeta::turns`].
    pub turns: u8,
}

impl Entry {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// What a row mirrors of a file's sidecar, with the file's identity:
/// enough for a browser to list the file, badge it, filter it and
/// find its thumbnail without reading the sidecar or the file. The
/// sidecar stays the truth; this is as of the last pass over the
/// folder, or the last save the editor indexed.
#[derive(Debug, Clone, PartialEq)]
pub struct RowMeta {
    pub id: i64,
    /// The content hash, hex; see [`hash`].
    pub hash: String,
    /// The file's mtime, nanoseconds since the epoch.
    pub mtime: i64,
    pub meta: Meta,
    /// The sidecar holds a develop: a step in its history, a snapshot,
    /// or a current edit that is not the frame's default (a raw's
    /// `Edit::default()`, a picture's `Edit::for_picture()`). A frame
    /// only rated, or never opened, has none. A sidecar whose edit
    /// this build cannot read counts as edited: there is something in
    /// it, whatever it is.
    pub edited: bool,
    /// The quarter turns and the mirror the frame's picture is shown
    /// at (`Geometry::shown_turns` of its edit with its own turn),
    /// packed as the turns plus four when mirrored: see
    /// [`RowMeta::shown_turns`].
    pub turns: u8,
}

impl RowMeta {
    /// The quarter turns clockwise and whether the picture is mirrored,
    /// as the grid and the strip draw it.
    pub fn shown_turns(&self) -> (u8, bool) {
        unpack_turns(self.turns)
    }
}

/// `(turns, flip)` as the `turns` column holds it.
pub fn pack_turns((turns, flip): (u8, bool)) -> u8 {
    (turns % 4) + if flip { 4 } else { 0 }
}

/// The `turns` column as `(turns, flip)`.
pub fn unpack_turns(packed: u8) -> (u8, bool) {
    (packed % 4, packed & 4 != 0)
}

/// One chip of a facet: a value the files hold and how many hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetCount {
    /// The value as a [`filter::Term::Facet`] takes it back: the
    /// camera or the lens as the row has it, the ISO and the focal
    /// length as numbers (`35`, `23.9`), the day as `2026-09-21`, a
    /// keyword folded to lower case.
    pub value: String,
    /// The value as a chip shows it: the same, except a keyword,
    /// which keeps a case it was written in.
    pub label: String,
    pub count: usize,
}

/// A body and fixed style the index holds frames of: what the camera
/// match fits a look for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleGroup {
    pub make: String,
    pub model: String,
    /// Make and model as the panel shows them.
    pub camera: String,
    /// The maker as the style reader names it.
    pub maker: String,
    /// The group key: the maker and the style, "Canon Faithful".
    pub style: String,
    /// Frames with every adaptive setting off.
    pub fixed: usize,
    /// Frames with one on, or one the reader could not name.
    pub unfixed: usize,
}

/// One frame of a [`StyleGroup`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleFrame {
    pub path: PathBuf,
    /// As [`Exif::taken`] has it.
    pub taken: Option<String>,
    pub lens: Option<String>,
    pub fixed: bool,
    pub peripheral: Option<bool>,
}

/// A file's path as the index keys it — its folder canonical, its
/// own name kept — for a caller that looks rows up by the paths it
/// holds.
pub fn key_path(path: &Path) -> PathBuf {
    canonical_file(path)
}

/// A folder's path as the index keys it (canonical, or as near as the
/// disk still has it): what [`Library::ids_of_with`] is handed.
pub fn key_folder(dir: &Path) -> PathBuf {
    nearest_canonical(dir)
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

    /// [`Library::open`] or [`Library::open_read_only`] with `handler`
    /// as the connection's busy handler from the first statement, the
    /// open's own reads among them: for a caller whose waits on the
    /// library's lock have to be told from a disk that stopped
    /// answering (notes §215, §216).
    pub fn open_with_busy(
        path: &Path,
        read_only: bool,
        handler: fn(i32) -> bool,
    ) -> Result<Library> {
        let conn = if read_only {
            Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?
        } else {
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)?;
            }
            Connection::open(path)?
        };
        conn.busy_handler(Some(handler))?;
        Self::prepare(conn, path.to_path_buf(), read_only)
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
                Judgement::ToMake if now.migrates() => {
                    for (from, sql) in MIGRATIONS.iter().filter(|(v, _)| *v >= now.version) {
                        log::info!(
                            "{}: schema version {from} brought up to {}",
                            path.display(),
                            from + 1
                        );
                        tx.execute_batch(sql)?;
                    }
                    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
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

    /// How long a query waits for a lock before it answers busy;
    /// rusqlite's default is five seconds. A reader on a window's
    /// thread wants to hear busy at once and keep its last answer
    /// rather than hold the window: under write-ahead logging a
    /// reader waits only while the log is being recovered or
    /// checkpointed from under it, which is rare and short.
    pub fn set_busy_timeout(&self, timeout: std::time::Duration) -> Result<()> {
        self.conn.busy_timeout(timeout)?;
        Ok(())
    }

    /// In place of the busy timeout, `handler` asked each time a query
    /// finds the lock held, with how many times it has been asked for
    /// this wait; it sleeps as long as it likes and answers whether to
    /// try again. For a caller that wants to hear, while it waits, that
    /// the wait is a lock's and not the disk's.
    pub fn set_busy_handler(&self, handler: fn(i32) -> bool) -> Result<()> {
        self.conn.busy_handler(Some(handler))?;
        Ok(())
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

    /// The row id of each of `paths`, where the index holds the file
    /// and last found it there; `None` for a file it has no row for
    /// yet. One query a folder, each folder made canonical once, so
    /// the filter bar can map its frames to rows on every refresh.
    pub fn ids_of(&self, paths: &[PathBuf]) -> Result<Vec<Option<i64>>> {
        self.ids_of_with(paths, &mut |dir| nearest_canonical(dir))
    }

    /// [`Library::ids_of`], each folder made canonical by `canonical`
    /// rather than by asking the disk: a caller that already knows a
    /// folder's canonical form (the index's own paths are) keeps its
    /// thread off the disk.
    pub fn ids_of_with(
        &self,
        paths: &[PathBuf],
        canonical: &mut dyn FnMut(&Path) -> PathBuf,
    ) -> Result<Vec<Option<i64>>> {
        let mut folders: HashMap<PathBuf, (PathBuf, HashMap<Vec<u8>, i64>)> = HashMap::new();
        let mut out = Vec::with_capacity(paths.len());
        let mut stmt = self.conn.prepare_cached(
            "SELECT path, id FROM files WHERE folder = ? AND missing_since IS NULL",
        )?;
        for path in paths {
            let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
                out.push(None);
                continue;
            };
            if !folders.contains_key(parent) {
                let dir = if parent.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    parent
                };
                let canonical = canonical(dir);
                let rows = stmt
                    .query_map(params![path_bytes(&canonical)], |r| {
                        Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?))
                    })?
                    .collect::<rusqlite::Result<HashMap<_, _>>>()?;
                folders.insert(parent.to_path_buf(), (canonical, rows));
            }
            let (canonical, rows) = &folders[parent];
            out.push(rows.get(&path_bytes(&canonical.join(name))).copied());
        }
        Ok(out)
    }

    /// Which of the rows `ids` pass `filter`, under the same rule as
    /// [`Library::query`].
    pub fn ids_passing(&self, ids: &[i64], filter: &Filter) -> Result<HashSet<i64>> {
        let (body, params) = filter.to_sql();
        let sql = format!(
            "SELECT files.id FROM files WHERE files.id IN (SELECT value FROM json_each(?)) \
             AND ({body}){}",
            if filter.mentions_missing() {
                ""
            } else {
                " AND files.missing_since IS NULL"
            }
        );
        let mut all = vec![rusqlite::types::Value::Text(ids_json(ids))];
        all.extend(params.iter().map(param));
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(all), |r| r.get::<_, i64>(0))?;
        Ok(rows.collect::<rusqlite::Result<HashSet<_>>>()?)
    }

    /// A facet's chips: each value `facet` takes among the rows that
    /// pass `filter`, with how many files hold it — one `GROUP BY`.
    /// `within` narrows the rows to those ids first, which is how the
    /// filter bar counts only the frames its other tests leave; `None`
    /// is the whole library. A file that does not say (no lens, no
    /// date) is on no chip. Cameras, lenses and keywords come most
    /// held first; ISOs, focal lengths and days in their own order.
    pub fn facet_counts(
        &self,
        facet: Facet,
        within: Option<&[i64]>,
        filter: &Filter,
    ) -> Result<Vec<FacetCount>> {
        let (body, params) = filter.to_sql();
        let key = facet.key();
        let (from, label, count) = match facet {
            Facet::Keyword => (
                "files JOIN keywords k ON k.file = files.id",
                "min(k.word)",
                "count(DISTINCT files.id)",
            ),
            _ => ("files", key, "count(*)"),
        };
        let order = match facet {
            Facet::Camera | Facet::Lens | Facet::Style | Facet::Keyword => "2 DESC, 1",
            Facet::Iso | Facet::Focal | Facet::Date => "1",
        };
        let sql = format!(
            "SELECT {key}, {count}, {label} FROM {from} \
             WHERE {key} IS NOT NULL AND {key} <> '' AND ({body}){}{} \
             GROUP BY 1 ORDER BY {order}",
            if filter.mentions_missing() {
                ""
            } else {
                " AND files.missing_since IS NULL"
            },
            if within.is_some() {
                " AND files.id IN (SELECT value FROM json_each(?))"
            } else {
                ""
            }
        );
        let mut all: Vec<rusqlite::types::Value> = params.iter().map(param).collect();
        if let Some(ids) = within {
            all.push(rusqlite::types::Value::Text(ids_json(ids)));
        }
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(all), |r| {
            use rusqlite::types::ValueRef;
            let value = match r.get_ref(0)? {
                ValueRef::Integer(i) => i.to_string(),
                // `35.0` reads back as 35, and 23.9 as 23.9: the
                // shortest spelling that parses to the same number,
                // which is what a chip hands back to be matched.
                ValueRef::Real(f) => f.to_string(),
                ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                _ => String::new(),
            };
            let label = match r.get_ref(2)? {
                ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                _ => value.clone(),
            };
            Ok(FacetCount {
                value,
                label,
                count: r.get::<_, i64>(1)? as usize,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every file with this content hash, missing ones included: a
    /// copy on two disks is two rows with one hash.
    pub fn by_hash(&self, hash: &str) -> Result<Vec<Entry>> {
        let sql = format!("SELECT {COLUMNS} FROM files WHERE hash = ? ORDER BY files.path");
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params![hash], entry)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The files with this content hash under any of `roots` (canonical,
    /// as [`Roots`] keeps them), the missing left out: where a copy of a
    /// frame is under an archive, or under the local roots, whatever
    /// its path there. Nothing is asked of the disk; a caller confirms
    /// each find there before it trusts it.
    pub fn by_hash_under(&self, hash: &str, roots: &[PathBuf]) -> Result<Vec<Entry>> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }
        let (clause, mut values) = under_roots_as(roots, false);
        let sql = format!(
            "SELECT {COLUMNS} FROM files WHERE hash = ? AND missing_since IS NULL \
             AND ({clause}) ORDER BY files.path"
        );
        values.insert(0, rusqlite::types::Value::Text(hash.to_owned()));
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values), entry)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Keep the whole file's BLAKE3 on its row, as a backup or a
    /// bring-back took it, when the row is there and holds the size
    /// the hash was taken at: a file changed since is not given a hash
    /// that is no longer its own. True when a row took it.
    pub fn set_whole_hash(&mut self, path: &Path, size: u64, whole: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE files SET whole_hash = ? WHERE path = ? AND size = ?",
            params![whole, path_bytes(&canonical_file(path)), size as i64],
        )?;
        Ok(n > 0)
    }

    /// The whole file's BLAKE3 the row keeps, if a backup or a
    /// bring-back took it and the file has not changed since.
    pub fn whole_hash(&self, path: &Path) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT whole_hash FROM files WHERE path = ?")?
            .query_row(params![path_bytes(&canonical_file(path))], |r| {
                r.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten())
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

    /// Every file under these roots the index last found where it
    /// is, by folder then name: the all-roots view's list. A root is
    /// a folder, and a file is under it when its folder is the root
    /// or starts with it and a separator.
    pub fn paths_under(&self, roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
        self.paths_under_as(roots, true)
    }

    /// [`Library::paths_under`] for roots already canonical, as
    /// [`Roots`] keeps them: nothing is asked of the disk.
    pub fn paths_under_canonical(&self, roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
        self.paths_under_as(roots, false)
    }

    fn paths_under_as(&self, roots: &[PathBuf], canonicalize: bool) -> Result<Vec<PathBuf>> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }
        let (clause, params) = under_roots_as(roots, canonicalize);
        let sql = format!(
            "SELECT path FROM files WHERE missing_since IS NULL AND ({clause}) \
             ORDER BY folder, name"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            r.get::<_, Vec<u8>>(0).map(|b| path_from_bytes(&b))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// [`Library::paths_under_canonical`] with what each row mirrors
    /// of the file's sidecar beside the path, by folder then name: a
    /// view of the roots built from this reads no sidecar.
    pub fn rows_under_canonical(&self, roots: &[PathBuf]) -> Result<Vec<(PathBuf, RowMeta)>> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }
        let (clause, params) = under_roots_as(roots, false);
        let sql = format!(
            "SELECT files.path, {META_COLUMNS} FROM files WHERE missing_since IS NULL \
             AND ({clause}) ORDER BY folder, name"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok((path_from_bytes(&r.get::<_, Vec<u8>>(0)?), row_meta(r, 1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// [`Library::ids_of_with`] with what each row mirrors of the
    /// file's sidecar: `None` for a file the index has no row for yet.
    pub fn rows_of_with(
        &self,
        paths: &[PathBuf],
        canonical: &mut dyn FnMut(&Path) -> PathBuf,
    ) -> Result<Vec<Option<RowMeta>>> {
        let mut folders: HashMap<PathBuf, (PathBuf, HashMap<Vec<u8>, RowMeta>)> = HashMap::new();
        let mut out = Vec::with_capacity(paths.len());
        let sql = format!(
            "SELECT files.path, {META_COLUMNS} FROM files WHERE folder = ? \
             AND missing_since IS NULL"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        for path in paths {
            let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
                out.push(None);
                continue;
            };
            if !folders.contains_key(parent) {
                let dir = if parent.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    parent
                };
                let canonical = canonical(dir);
                let rows = stmt
                    .query_map(params![path_bytes(&canonical)], |r| {
                        Ok((r.get::<_, Vec<u8>>(0)?, row_meta(r, 1)?))
                    })?
                    .collect::<rusqlite::Result<HashMap<_, _>>>()?;
                folders.insert(parent.to_path_buf(), (canonical, rows));
            }
            let (canonical, rows) = &folders[parent];
            out.push(rows.get(&path_bytes(&canonical.join(name))).cloned());
        }
        Ok(out)
    }

    /// How many files the index holds under one root, the missing
    /// left out: the count beside the root's name.
    pub fn count_under(&self, root: &Path) -> Result<usize> {
        self.count_under_as(root, true)
    }

    /// [`Library::count_under`] for a root already canonical, as
    /// [`Roots`] keeps it: nothing is asked of the disk.
    pub fn count_under_canonical(&self, root: &Path) -> Result<usize> {
        self.count_under_as(root, false)
    }

    fn count_under_as(&self, root: &Path, canonicalize: bool) -> Result<usize> {
        let (clause, params) =
            under_roots_as(std::slice::from_ref(&root.to_path_buf()), canonicalize);
        let sql = format!("SELECT count(*) FROM files WHERE missing_since IS NULL AND ({clause})");
        let n: i64 = self
            .conn
            .prepare_cached(&sql)?
            .query_row(rusqlite::params_from_iter(params), |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Each folder under a root (canonical, as [`Roots`] keeps it) that
    /// the index holds files in, the root itself among them, with how
    /// many are directly in it, the missing left out; by folder. What
    /// a tree of the root's folders is built from: a folder with files
    /// only in the folders under it is not here, and its place in the
    /// tree is made by theirs.
    pub fn folder_counts_under_canonical(&self, root: &Path) -> Result<Vec<(PathBuf, usize)>> {
        let (clause, params) = under_roots_as(std::slice::from_ref(&root.to_path_buf()), false);
        let sql = format!(
            "SELECT folder, count(*) FROM files WHERE missing_since IS NULL AND ({clause}) \
             GROUP BY folder ORDER BY folder"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok((
                path_from_bytes(&r.get::<_, Vec<u8>>(0)?),
                r.get::<_, i64>(1)? as usize,
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The files directly in one folder (canonical, as the index keys
    /// it), not those in the folders under it, with what each row
    /// mirrors of the file's sidecar; by name, the missing left out.
    /// A folder of a root opened from the index reads no sidecar.
    pub fn rows_in_canonical(&self, folder: &Path) -> Result<Vec<(PathBuf, RowMeta)>> {
        let sql = format!(
            "SELECT files.path, {META_COLUMNS} FROM files WHERE folder = ? \
             AND missing_since IS NULL ORDER BY name"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params![path_bytes(folder)], |r| {
            Ok((path_from_bytes(&r.get::<_, Vec<u8>>(0)?), row_meta(r, 1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// How many files are directly in one folder (canonical), the
    /// missing left out.
    pub fn count_in_canonical(&self, folder: &Path) -> Result<usize> {
        let n: i64 = self
            .conn
            .prepare_cached(
                "SELECT count(*) FROM files WHERE folder = ? AND missing_since IS NULL",
            )?
            .query_row(params![path_bytes(folder)], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// The camera match's groups: each body and fixed style the index
    /// holds frames of, under `roots` or in the whole library, with
    /// how many of its frames had every adaptive setting off and how
    /// many had one on. By camera, then style. A file last found
    /// missing is not counted.
    pub fn style_groups(&self, roots: Option<&[PathBuf]>) -> Result<Vec<StyleGroup>> {
        let Some((clause, params)) = within_roots(roots) else {
            return Ok(Vec::new());
        };
        let sql = format!(
            "SELECT coalesce(make, ''), coalesce(model, ''), coalesce(camera, ''), \
             coalesce(maker, ''), style, sum(style_fixed), count(*) FROM files \
             WHERE style IS NOT NULL AND missing_since IS NULL{clause} \
             GROUP BY make, model, style ORDER BY 3, 5"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            let fixed = r.get::<_, i64>(5)? as usize;
            Ok(StyleGroup {
                make: r.get(0)?,
                model: r.get(1)?,
                camera: r.get(2)?,
                maker: r.get(3)?,
                style: r.get(4)?,
                fixed,
                unfixed: r.get::<_, i64>(6)? as usize - fixed,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The frames of one group of [`Library::style_groups`], by date
    /// taken then path; a frame with no date sorts first.
    pub fn style_frames(
        &self,
        group: &StyleGroup,
        roots: Option<&[PathBuf]>,
    ) -> Result<Vec<StyleFrame>> {
        use rusqlite::types::Value;
        let Some((clause, mut params)) = within_roots(roots) else {
            return Ok(Vec::new());
        };
        let sql = format!(
            "SELECT path, taken, lens, style_fixed, peripheral FROM files \
             WHERE coalesce(make, '') = ? AND coalesce(model, '') = ? AND style = ? \
             AND missing_since IS NULL{clause} ORDER BY taken, path"
        );
        let mut all = vec![
            Value::Text(group.make.clone()),
            Value::Text(group.model.clone()),
            Value::Text(group.style.clone()),
        ];
        all.append(&mut params);
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(all), |r| {
            Ok(StyleFrame {
                path: path_from_bytes(&r.get::<_, Vec<u8>>(0)?),
                taken: r.get(1)?,
                lens: r.get(2)?,
                fixed: r.get::<_, i64>(3)? != 0,
                peripheral: r.get::<_, Option<i64>>(4)?.map(|v| v != 0),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// How many raws, under `roots` or in the whole library, have not
    /// had their maker's tags read yet: rows from before the tags were
    /// indexed, which the next pass over their folders fills.
    pub fn styles_unread(&self, roots: Option<&[PathBuf]>) -> Result<usize> {
        self.count_raws("style_read = 0", roots)
    }

    /// How many raws, under `roots` or in the whole library, had their
    /// maker's tags read and carry no fixed style: an adaptive one such
    /// as Auto, or a maker whose styles are not read.
    pub fn styles_missing(&self, roots: Option<&[PathBuf]>) -> Result<usize> {
        self.count_raws("style_read = 1 AND style IS NULL", roots)
    }

    /// The raws present under `roots` (or anywhere) that pass `test`.
    fn count_raws(&self, test: &str, roots: Option<&[PathBuf]>) -> Result<usize> {
        let Some((clause, params)) = within_roots(roots) else {
            return Ok(0);
        };
        let sql = format!(
            "SELECT count(*) FROM files WHERE {test} AND {} AND missing_since IS NULL{clause}",
            raw_name_clause()
        );
        let n: i64 = self
            .conn
            .prepare_cached(&sql)?
            .query_row(rusqlite::params_from_iter(params), |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Where each of these rows' files is now, for a window holding
    /// frames by path when a pass may have moved them. A row the index
    /// last found at its path is there, which is a move within a root
    /// (§160 keeps the row's id). A row marked missing is answered by
    /// its hash in one case only, a folder renamed: the index takes a
    /// renamed folder's files for new rows, since a row whose folder is
    /// gone is never a move's other end (§160), so the answer is a row
    /// with the same hash and the same name, present, added after the
    /// missing one, with the missing row's own folder gone from the
    /// disk. A copy is not an answer: the first cut took any present
    /// row with the hash, and a frame deleted after it was backed up
    /// was "followed" to the backup, whose sidecar its edit would then
    /// have been saved over. A row with no answer is left out.
    pub fn found_at(&self, ids: &[i64]) -> Result<HashMap<i64, PathBuf>> {
        let mut out = HashMap::new();
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, path, folder, hash, name, missing_since FROM files \
             WHERE id IN (SELECT value FROM json_each(?))",
        )?;
        let rows = stmt
            .query_map(params![ids_json(ids)], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<i64>>(5)?.is_some(),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut twins = self.conn.prepare_cached(
            "SELECT path FROM files WHERE hash = ? AND id > ? AND name IS ? \
             AND missing_since IS NULL ORDER BY id",
        )?;
        for (id, path, folder, hash, name, missing) in rows {
            if !missing {
                out.insert(id, path_from_bytes(&path));
                continue;
            }
            if path_from_bytes(&folder).exists() {
                continue;
            }
            let found = twins
                .query_map(params![hash, id, name], |r| {
                    Ok(path_from_bytes(&r.get::<_, Vec<u8>>(0)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if let Some(p) = found.into_iter().find(|p| p.is_file()) {
                out.insert(id, p);
            }
        }
        Ok(out)
    }

    /// Forget every file in the library last found missing; how many
    /// went.
    pub fn prune_missing(&mut self) -> Result<usize> {
        Ok(self
            .conn
            .execute("DELETE FROM files WHERE missing_since IS NOT NULL", [])?)
    }

    /// Forget the rows of these files, whatever state they are in; how
    /// many went. For a delete the editor made itself: the files are
    /// gone on purpose, so their rows are not kept missing for a move
    /// to find, as a pass over the folder would keep them. The paths
    /// are keyed as the index keys them, the folder canonical, so a
    /// path whose file is already gone still finds its row while its
    /// folder is there. The keywords go with the rows.
    pub fn forget(&mut self, paths: &[PathBuf]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut gone = 0;
        {
            let mut stmt = tx.prepare_cached("DELETE FROM files WHERE path = ?")?;
            for p in paths {
                gone += stmt.execute(params![path_bytes(&canonical_file(p))])?;
            }
        }
        tx.commit()?;
        Ok(gone)
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

    /// An older library of ours that [`MIGRATIONS`] brings up to this
    /// schema in place: there is a step from its version and from
    /// every one after it.
    fn migrates(&self) -> bool {
        self.app_id == APPLICATION_ID
            && self.version < SCHEMA_VERSION
            && (self.version..SCHEMA_VERSION).all(|v| MIGRATIONS.iter().any(|(from, _)| *from == v))
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

/// The `WHERE` clause for "under one of these roots", over the folder
/// column's bytes, and its parameters. A root is compared canonical,
/// as the index stores folders. "Starts with the root and a
/// separator" is asked as a range, from that prefix up to the prefix
/// with its last byte one higher, which the folder's index answers
/// without reading every row; BLOBs compare as `memcmp` does, so the
/// range holds exactly the folders that start with the prefix. The
/// separator's last byte is `/` or `\`'s low byte or the zero after
/// it in UTF-16, never 0xFF, so it always has a next.
fn under_roots(roots: &[PathBuf]) -> (String, Vec<rusqlite::types::Value>) {
    under_roots_as(roots, true)
}

/// [`under_roots`], with the roots made canonical first or taken as
/// they are.
fn under_roots_as(roots: &[PathBuf], canonicalize: bool) -> (String, Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value;
    let mut clauses = Vec::with_capacity(roots.len());
    let mut params = Vec::with_capacity(roots.len() * 3);
    for root in roots {
        let bytes = if canonicalize {
            path_bytes(&nearest_canonical(root))
        } else {
            path_bytes(root)
        };
        let prefix = index::under_prefix(&bytes);
        let mut past = prefix.clone();
        if let Some(last) = past.last_mut() {
            *last = last.saturating_add(1);
        }
        clauses.push("folder = ? OR (folder >= ? AND folder < ?)");
        params.push(Value::Blob(bytes));
        params.push(Value::Blob(prefix));
        params.push(Value::Blob(past));
    }
    (clauses.join(" OR "), params)
}

/// "The file's name has a raw's extension", as
/// `greycard_core::decode::is_raw_path` has it, over the name column.
/// `LIKE` folds ASCII case, which is all an extension has.
fn raw_name_clause() -> String {
    let each: Vec<String> = greycard_core::decode::RAW_EXTENSIONS
        .iter()
        .map(|e| format!("files.name LIKE '%.{e}'"))
        .collect();
    format!("({})", each.join(" OR "))
}

/// " AND under these roots" for a query that may be over the whole
/// library (`None`), and its parameters; `None` back for no roots at
/// all, which nothing is under.
fn within_roots(roots: Option<&[PathBuf]>) -> Option<(String, Vec<rusqlite::types::Value>)> {
    match roots {
        None => Some((String::new(), Vec::new())),
        Some([]) => None,
        Some(roots) => {
            let (clause, params) = under_roots(roots);
            Some((format!(" AND ({clause})"), params))
        }
    }
}

/// Row ids as the JSON array `json_each` takes.
fn ids_json(ids: &[i64]) -> String {
    let mut s = String::with_capacity(ids.len() * 6 + 2);
    s.push('[');
    for (n, id) in ids.iter().enumerate() {
        if n > 0 {
            s.push(',');
        }
        s.push_str(&id.to_string());
    }
    s.push(']');
    s
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
            style: StyleTags {
                maker: row.get(21)?,
                style: row.get(22)?,
                fixed: row.get::<_, i64>(23)? != 0,
                peripheral: row.get::<_, Option<i64>>(24)?.map(|v| v != 0),
            },
        },
        meta,
        sidecar: row
            .get::<_, Option<Vec<u8>>>(14)?
            .map(|b| path_from_bytes(&b)),
        sidecar_mtime: row.get(15)?,
        missing: row.get::<_, Option<i64>>(20)?.is_some(),
        edited: row.get::<_, i64>(25)? != 0,
        turns: (row.get::<_, i64>(26)?.clamp(0, 7)) as u8,
    })
}

/// A [`RowMeta`] from a row of [`META_COLUMNS`] starting at `from`.
fn row_meta(row: &rusqlite::Row<'_>, from: usize) -> rusqlite::Result<RowMeta> {
    let keywords: String = row.get(from + 6)?;
    let mut meta = Meta {
        rating: row.get::<_, i64>(from + 3)?.clamp(0, 255) as u8,
        flag: filter::flag_from_name(&row.get::<_, String>(from + 4)?),
        label: filter::label_from_name(&row.get::<_, String>(from + 5)?),
        ..Meta::default()
    };
    meta.set_keywords(serde_json::from_str(&keywords).unwrap_or_default());
    Ok(RowMeta {
        id: row.get(from)?,
        hash: row.get(from + 1)?,
        mtime: row.get(from + 2)?,
        meta,
        edited: row.get::<_, i64>(from + 7)? != 0,
        turns: (row.get::<_, i64>(from + 8)?.clamp(0, 7)) as u8,
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
