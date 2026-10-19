//! Collections and smart collections on disk: frames gathered by hand
//! from anywhere in the library, and the filter bar's question kept
//! under a name.
//!
//! Neither can be rebuilt from the disk, so neither lives in the index,
//! which can be deleted and rebuilt at any time. They live in a folder
//! of their own beside it, `collections/` under the library's data
//! directory, one small file a collection: `<id>.json` for one filled
//! by hand, `<id>.smart.json` for a saved filter. The id is made at
//! random when the collection is, so a rename never moves the file and
//! two machines never make the same one; deleting a collection deletes
//! its file. Each is written through a part file renamed over it, as
//! the roots file is.
//!
//! A collection holds pictures, not paths. A reference is the frame's
//! content hash, the index's own (the first 64 KB and the size), with
//! the path it was last seen at as a hint. A reference may name a
//! version of the picture by its id in the sidecar; none is the file's
//! own. A file with a version in it is written at shape 2, which an
//! older build leaves alone; one without stays at 1.
//!
//! This is the files and their one writer, for the editor and the
//! command line alike: the editor's store, which joins a file another
//! editor changed and heals references the index found rewritten, is
//! `greycard-ui`'s `collections`, and the Lightroom import writes here
//! too.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::filter::Saved;

/// The folder the collections are kept in, beside the library.
pub const DIR: &str = "collections";

/// What the files say their shape is: a later build that changes it
/// bumps this, and this build leaves a file of a later one alone. 2 is
/// a collection holding a version of a picture; a file without one is
/// written at 1 as before, so a build that knows no versions goes on
/// reading every collection that has none.
pub const VERSION: u32 = 2;

/// The shape a file without a version in it is written at.
pub const PLAIN_VERSION: u32 = 1;

const SMART_END: &str = ".smart.json";
const PLAIN_END: &str = ".json";

/// One frame of a collection: the picture by its content hash, the
/// version of it, and the path it was last seen at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub hash: String,
    /// The path as the index keys it, as text: a hint, and what a file
    /// rewritten in place is found by.
    pub hint: String,
    /// The version's id in the sidecar, none for the file's own: left
    /// out of the file then, so a collection with no version in it is
    /// the file it always was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// A frame as a collection names it: its picture's hash and its
/// version, none for the file's own.
pub type Pair<'a> = (&'a str, Option<&'a str>);

/// A frame to add: its picture's hash, the path it is at, and its
/// version, none for the file's own.
pub type Frame = (String, PathBuf, Option<String>);

impl Reference {
    pub fn pair(&self) -> Pair<'_> {
        (self.hash.as_str(), self.version.as_deref())
    }

    pub fn owned_pair(&self) -> (String, Option<String>) {
        (self.hash.clone(), self.version.clone())
    }
}

/// A collection filled by hand.
#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    pub id: String,
    pub name: String,
    /// The sets it sits in, as a slash path (`Clients/2026`); empty for
    /// none.
    pub set: String,
    /// When it was made, seconds since the epoch.
    pub made: u64,
    /// The frames, in the collection's own order.
    pub refs: Vec<Reference>,
}

/// A smart collection: the filter bar's question under a name.
#[derive(Debug, Clone, PartialEq)]
pub struct Smart {
    pub id: String,
    pub name: String,
    pub set: String,
    pub made: u64,
    /// The bar as the settings keep it: the chips and the text, the
    /// text in the filter language.
    pub filter: Saved,
    /// The one root it looks under; none for all of them.
    pub root: Option<PathBuf>,
}

/// A collection's file as it is written.
#[derive(Serialize, Deserialize)]
pub struct CollectionFile {
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub set: String,
    #[serde(default)]
    pub made: u64,
    #[serde(default)]
    pub refs: Vec<Reference>,
}

#[derive(Serialize, Deserialize)]
struct SmartFile {
    version: u32,
    name: String,
    #[serde(default)]
    set: String,
    #[serde(default)]
    made: u64,
    #[serde(default)]
    filter: Saved,
    #[serde(default)]
    root: Option<String>,
}

/// What an add did: frames added at the end, and members whose hint
/// moved to the copy given.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Added {
    pub added: usize,
    pub moved: usize,
}

/// A name as typed, split at its last slash into the sets and the
/// name: `Clients/2026/Cover` is the collection `Cover` in the set
/// `Clients/2026`. Empty pieces go.
pub fn split_name(typed: &str) -> (String, String) {
    let parts: Vec<&str> = typed
        .split('/')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    match parts.split_last() {
        None => (String::new(), String::new()),
        Some((name, sets)) => (sets.join("/"), name.to_string()),
    }
}

/// The name as it is typed back: the sets and the name, slashed.
pub fn full_name(set: &str, name: &str) -> String {
    if set.is_empty() {
        name.to_string()
    } else {
        format!("{set}/{name}")
    }
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A new id: 32 hex digits from the standard library's random hasher
/// keys, the time and the process mixed in.
pub(crate) fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut a = std::collections::hash_map::RandomState::new().build_hasher();
    a.write_u128(nanos);
    a.write_u32(std::process::id());
    let a = a.finish();
    let mut b = std::collections::hash_map::RandomState::new().build_hasher();
    b.write_u64(a);
    b.write_u128(nanos);
    format!("{a:016x}{:016x}", b.finish())
}

/// A path as a reference keeps it.
pub fn hint_of(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl Collection {
    /// A new, empty collection named as typed (`Sets/Name`).
    pub fn new(typed: &str) -> Collection {
        let (set, name) = split_name(typed);
        Collection {
            id: new_id(),
            name,
            set,
            made: now(),
            refs: Vec::new(),
        }
    }

    /// Whether the frame `pair` is in it: that version of that picture.
    pub fn has(&self, pair: Pair<'_>) -> bool {
        self.refs.iter().any(|r| r.pair() == pair)
    }

    /// Frames added, each by its hash, the path it is at and its
    /// version: a frame not in the collection goes at the end, in the
    /// order given; one that is moves its hint to this path, and keeps
    /// its place. Two versions of one picture are two frames.
    pub fn add(&mut self, frames: &[Frame]) -> Added {
        let mut out = Added::default();
        for (hash, path, version) in frames {
            let hint = hint_of(path);
            let pair = (hash.as_str(), version.as_deref());
            match self.refs.iter_mut().find(|r| r.pair() == pair) {
                Some(r) => {
                    if r.hint != hint {
                        r.hint = hint;
                        out.moved += 1;
                    }
                }
                None => {
                    self.refs.push(Reference {
                        hash: hash.clone(),
                        hint,
                        version: version.clone(),
                    });
                    out.added += 1;
                }
            }
        }
        out
    }

    /// Frame `frame` put just after the frame `after`, or at the end when
    /// that is not in it; a frame in it already stays where it is. True
    /// when it went in.
    pub fn insert_after(&mut self, after: Pair<'_>, frame: &Frame) -> bool {
        let (hash, path, version) = frame;
        if self.has((hash.as_str(), version.as_deref())) {
            return false;
        }
        let at = self
            .refs
            .iter()
            .position(|r| r.pair() == after)
            .map_or(self.refs.len(), |i| i + 1);
        self.refs.insert(
            at,
            Reference {
                hash: hash.clone(),
                hint: hint_of(path),
                version: version.clone(),
            },
        );
        true
    }

    /// The references of these frames taken out; how many frames were,
    /// a frame named twice in the file counted once.
    pub fn remove(&mut self, pairs: &HashSet<Pair<'_>>) -> usize {
        let n = pairs.iter().filter(|&&p| self.has(p)).count();
        self.refs.retain(|r| !pairs.contains(&r.pair()));
        n
    }

    /// How many frames it holds, each (hash, version) once however many
    /// times the file names it.
    pub fn count(&self) -> usize {
        self.refs
            .iter()
            .map(Reference::pair)
            .collect::<HashSet<_>>()
            .len()
    }

    /// The references of `moving` put, in the order they have now, just
    /// before the reference of `before`, or at the end for none. A
    /// `before` that is itself moving is the first reference after it
    /// that is not. True when the order changed.
    pub fn reorder(&mut self, moving: &HashSet<Pair<'_>>, before: Option<Pair<'_>>) -> bool {
        let was = self.refs.clone();
        let anchor = before.and_then(|b| {
            let at = self.refs.iter().position(|r| r.pair() == b)?;
            self.refs[at..]
                .iter()
                .find(|r| !moving.contains(&r.pair()))
                .map(Reference::owned_pair)
        });
        let (taken, mut kept): (Vec<Reference>, Vec<Reference>) = self
            .refs
            .drain(..)
            .partition(|r| moving.contains(&r.pair()));
        let at = anchor
            .and_then(|(h, v)| kept.iter().position(|r| r.pair() == (&h, v.as_deref())))
            .unwrap_or(kept.len());
        kept.splice(at..at, taken);
        self.refs = kept;
        self.refs != was
    }

    /// The reference of `old` (that version of the old picture) takes
    /// `new` and the hint `path`. When another reference holds that
    /// version of `new` already, taking it would put that frame in the
    /// collection twice, and this reference is never dropped for it:
    /// while a copy of its own picture survives (`survivor`, that copy's
    /// path) its hint moves to the copy; with none known it stays as it
    /// is, on its old hash, missing until found. True when anything
    /// changed.
    pub fn take_hash(
        &mut self,
        old: Pair<'_>,
        new: &str,
        path: &str,
        survivor: Option<&str>,
    ) -> bool {
        let Some(at) = self.refs.iter().position(|r| r.pair() == old) else {
            return false;
        };
        let (old_hash, version) = (old.0.to_string(), old.1.map(str::to_string));
        if old_hash == new {
            if self.refs[at].hint == path {
                return false;
            }
            self.refs[at].hint = path.to_string();
            return true;
        }
        match (
            self.has((new, version.as_deref())),
            survivor.filter(|c| !c.is_empty()),
        ) {
            (true, None) => return false,
            (true, Some(copy)) => {
                if self.refs[at].hint == copy {
                    return false;
                }
                self.refs[at].hint = copy.to_string();
            }
            (false, _) => {
                self.refs[at] = Reference {
                    hash: new.to_string(),
                    hint: path.to_string(),
                    version,
                };
            }
        }
        true
    }

    /// The shape its file is written at: 2 when a reference names a
    /// version, else 1, so a build that knows no versions reads it.
    fn file_version(&self) -> u32 {
        if self.refs.iter().any(|r| r.version.is_some()) {
            VERSION
        } else {
            PLAIN_VERSION
        }
    }

    fn to_file(&self) -> CollectionFile {
        CollectionFile {
            version: self.file_version(),
            name: self.name.clone(),
            set: self.set.clone(),
            made: self.made,
            refs: self.refs.clone(),
        }
    }

    /// Written to its file in the folder `dir`, through the part file;
    /// the hash of what was written, the stamp a later save compares
    /// the file on disk against.
    pub fn write(&self, dir: &Path) -> std::io::Result<Stamp> {
        let text = serde_json::to_string_pretty(&self.to_file()).map_err(std::io::Error::other)?;
        write_atomic(&file_of(dir, &self.id, false), &text)?;
        Ok(blake3::hash(text.as_bytes()))
    }
}

impl Smart {
    /// A new smart collection named as typed, of the filter given.
    pub fn new(typed: &str, filter: Saved, root: Option<PathBuf>) -> Smart {
        let (set, name) = split_name(typed);
        Smart {
            id: new_id(),
            name,
            set,
            made: now(),
            filter,
            root,
        }
    }

    fn to_file(&self) -> SmartFile {
        SmartFile {
            version: PLAIN_VERSION,
            name: self.name.clone(),
            set: self.set.clone(),
            made: self.made,
            filter: self.filter.clone(),
            root: self.root.as_deref().map(hint_of),
        }
    }

    /// Written to its file in the folder `dir`, as
    /// [`Collection::write`] writes a collection.
    pub fn write(&self, dir: &Path) -> std::io::Result<Stamp> {
        let text = serde_json::to_string_pretty(&self.to_file()).map_err(std::io::Error::other)?;
        write_atomic(&file_of(dir, &self.id, true), &text)?;
        Ok(blake3::hash(text.as_bytes()))
    }
}

/// A hash of a file's contents.
pub type Stamp = blake3::Hash;

/// The hash of the file at `path`, none when it cannot be read.
pub fn stamp_of(path: &Path) -> Option<Stamp> {
    let bytes = std::fs::read(path).ok()?;
    Some(blake3::hash(&bytes))
}

/// A collection's file read, whatever its shape says.
pub fn read_collection(path: &Path) -> Result<CollectionFile, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice::<CollectionFile>(&bytes).map_err(|e| e.to_string())
}

/// The file of collection `id` in the folder `dir`, the smart one's or
/// the plain one's.
pub fn file_of(dir: &Path, id: &str, smart: bool) -> PathBuf {
    let end = if smart { SMART_END } else { PLAIN_END };
    dir.join(format!("{id}{end}"))
}

/// Write `text` to `path` through a part file beside it, renamed over
/// it, so a crash mid-write never leaves half a file. The part file
/// carries the process's id, so two editors never write the same one.
pub(crate) fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let part = path.with_extension(format!("json.{}.part", std::process::id()));
    // On the disk before it takes the old one's name: a crash before
    // the filesystem flushes could otherwise keep the rename and lose
    // the data, an empty file in place of the one there was. The part
    // file is removed if the write fails.
    greycard_edit::write_synced(&part, text.as_bytes())?;
    std::fs::rename(&part, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })
}

/// The collections of a folder as read: each one, and each file's
/// stamp as it was read.
#[derive(Debug, Default)]
pub struct Folder {
    pub list: Vec<Collection>,
    pub smart: Vec<Smart>,
    pub stamps: HashMap<String, Stamp>,
}

/// Every collection in `dir`, by a build that reads files up to shape
/// `up_to` ([`VERSION`] for this one). A file that will not read, or
/// one a later build wrote, is said and left where it is: nothing here
/// writes to a file it did not read. A folder not there yet is none.
pub fn read_folder(dir: &Path, up_to: u32) -> Folder {
    let mut folder = Folder::default();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return folder,
        Err(e) => {
            log::warn!("collections {}: {e}", dir.display());
            return folder;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let read = || std::fs::read(&path);
        if let Some(id) = file.strip_suffix(SMART_END) {
            match read()
                .map_err(|e| e.to_string())
                .and_then(|b| serde_json::from_slice::<SmartFile>(&b).map_err(|e| e.to_string()))
            {
                Ok(f) if f.version <= up_to => {
                    if let Some(stamp) = stamp_of(&path) {
                        folder.stamps.insert(id.to_string(), stamp);
                    }
                    folder.smart.push(Smart {
                        id: id.to_string(),
                        name: f.name,
                        set: f.set,
                        made: f.made,
                        filter: f.filter,
                        root: f.root.map(PathBuf::from),
                    })
                }
                Ok(f) => log::warn!(
                    "collection {}: version {} is a later build's; left alone",
                    path.display(),
                    f.version
                ),
                Err(e) => log::warn!("collection {}: {e}; left alone", path.display()),
            }
        } else if let Some(id) = file.strip_suffix(PLAIN_END) {
            match read_collection(&path) {
                Ok(f) if f.version <= up_to => {
                    if let Some(stamp) = stamp_of(&path) {
                        folder.stamps.insert(id.to_string(), stamp);
                    }
                    folder.list.push(Collection {
                        id: id.to_string(),
                        name: f.name,
                        set: f.set,
                        made: f.made,
                        refs: f.refs,
                    })
                }
                Ok(f) => log::warn!(
                    "collection {}: version {} is a later build's; left alone",
                    path.display(),
                    f.version
                ),
                Err(e) => log::warn!("collection {}: {e}; left alone", path.display()),
            }
        }
    }
    folder
}

/// The folder for a library at `library`: beside it.
pub fn dir_beside(library: &Path) -> PathBuf {
    library
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .join(DIR)
}
