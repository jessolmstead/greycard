//! Collections and smart collections: frames gathered by hand from
//! anywhere in the library, and the filter bar's question kept under a
//! name.
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
//! the path it was last seen at as a hint. Every local frame of a hash
//! is a member, since it is the same picture; opened, the collection
//! shows each hash once, from the hinted path when the index has a
//! file there, else from the first local frame of the hash it knows.
//! Adding another copy of a member adds nothing and moves the hint.
//! A file rewritten in place by another tool has a new hash at the
//! same path: the reference takes the new hash, by its hint, when the
//! index says so ([`Store::heal`]) or when the collection is opened and
//! the old hash is nowhere ([`resolve`]). A reference with nothing at
//! its hash or its hint is kept, and counted as missing, never dropped.
//!
//! A collection's list is its order: a frame added goes at the end,
//! and a drag in the grid moves it. Capture order is a view of the same
//! list, sorted by the time taken, and rewrites nothing. A smart
//! collection has no list; it is the filter bar's state and the root
//! it looks under, and it sorts as the grid does.
//!
//! Nothing here touches the window: the window's side is
//! `panel::collections`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use greycard_library::{Library, Rehash, RowMeta};
use serde::{Deserialize, Serialize};

use crate::CollectionChip;
use crate::filter::Saved;

/// The folder the collections are kept in, beside the library.
pub(crate) const DIR: &str = "collections";

/// What the files say their shape is: a later build that changes it
/// bumps this, and this build leaves a file of a later one alone.
const VERSION: u32 = 1;

const SMART_END: &str = ".smart.json";
const PLAIN_END: &str = ".json";

/// One frame of a collection: the picture by its content hash, and the
/// path it was last seen at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Reference {
    pub(crate) hash: String,
    /// The path as the index keys it, as text: a hint, and what a file
    /// rewritten in place is found by.
    pub(crate) hint: String,
}

/// A collection filled by hand.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Collection {
    pub(crate) id: String,
    pub(crate) name: String,
    /// The sets it sits in, as a slash path (`Clients/2026`); empty for
    /// none.
    pub(crate) set: String,
    /// When it was made, seconds since the epoch.
    pub(crate) made: u64,
    /// The frames, in the collection's own order.
    pub(crate) refs: Vec<Reference>,
}

/// A smart collection: the filter bar's question under a name.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Smart {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) set: String,
    pub(crate) made: u64,
    /// The bar as the settings keep it: the chips and the text, the
    /// text in the filter language.
    pub(crate) filter: Saved,
    /// The one root it looks under; none for all of them.
    pub(crate) root: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct CollectionFile {
    version: u32,
    name: String,
    #[serde(default)]
    set: String,
    #[serde(default)]
    made: u64,
    #[serde(default)]
    refs: Vec<Reference>,
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
pub(crate) struct Added {
    pub(crate) added: usize,
    pub(crate) moved: usize,
}

/// A name as typed, split at its last slash into the sets and the
/// name: `Clients/2026/Cover` is the collection `Cover` in the set
/// `Clients/2026`. Empty pieces go.
pub(crate) fn split_name(typed: &str) -> (String, String) {
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
pub(crate) fn full_name(set: &str, name: &str) -> String {
    if set.is_empty() {
        name.to_string()
    } else {
        format!("{set}/{name}")
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A new id: 32 hex digits from the standard library's random hasher
/// keys, the time and the process mixed in.
fn new_id() -> String {
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
pub(crate) fn hint_of(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl Collection {
    /// A new, empty collection named as typed (`Sets/Name`).
    pub(crate) fn new(typed: &str) -> Collection {
        let (set, name) = split_name(typed);
        Collection {
            id: new_id(),
            name,
            set,
            made: now(),
            refs: Vec::new(),
        }
    }

    pub(crate) fn has(&self, hash: &str) -> bool {
        self.refs.iter().any(|r| r.hash == hash)
    }

    /// Frames added, each by its hash and the path it is at: a hash not
    /// in the collection goes at the end, in the order given; one that
    /// is moves its hint to this path, and keeps its place.
    pub(crate) fn add(&mut self, frames: &[(String, PathBuf)]) -> Added {
        let mut out = Added::default();
        for (hash, path) in frames {
            let hint = hint_of(path);
            match self.refs.iter_mut().find(|r| &r.hash == hash) {
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
                    });
                    out.added += 1;
                }
            }
        }
        out
    }

    /// The references of these hashes taken out; how many were.
    pub(crate) fn remove(&mut self, hashes: &HashSet<&str>) -> usize {
        let before = self.refs.len();
        self.refs.retain(|r| !hashes.contains(r.hash.as_str()));
        before - self.refs.len()
    }

    /// The references of `moving` put, in the order they have now, just
    /// before the reference of `before`, or at the end for none. A
    /// `before` that is itself moving is the first reference after it
    /// that is not. True when the order changed.
    pub(crate) fn reorder(&mut self, moving: &HashSet<&str>, before: Option<&str>) -> bool {
        let was = self.refs.clone();
        let anchor = before.and_then(|b| {
            let at = self.refs.iter().position(|r| r.hash == b)?;
            self.refs[at..]
                .iter()
                .find(|r| !moving.contains(r.hash.as_str()))
                .map(|r| r.hash.clone())
        });
        let (taken, mut kept): (Vec<Reference>, Vec<Reference>) = self
            .refs
            .drain(..)
            .partition(|r| moving.contains(r.hash.as_str()));
        let at = anchor
            .and_then(|a| kept.iter().position(|r| r.hash == a))
            .unwrap_or(kept.len());
        kept.splice(at..at, taken);
        self.refs = kept;
        self.refs != was
    }

    /// The reference of `old` takes `new` and the hint `path`. When
    /// another reference holds `new` already, taking it would put that
    /// picture in the collection twice, and this reference is never
    /// dropped for it: while a copy of its own picture survives
    /// (`survivor`, that copy's path) its hint moves to the copy; with
    /// none known it stays as it is, on its old hash, missing until
    /// found. True when anything changed.
    fn take_hash(&mut self, old: &str, new: &str, path: &str, survivor: Option<&str>) -> bool {
        let Some(at) = self.refs.iter().position(|r| r.hash == old) else {
            return false;
        };
        if old == new {
            if self.refs[at].hint == path {
                return false;
            }
            self.refs[at].hint = path.to_string();
            return true;
        }
        match (self.has(new), survivor.filter(|c| !c.is_empty())) {
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
                };
            }
        }
        true
    }

    fn to_file(&self) -> CollectionFile {
        CollectionFile {
            version: VERSION,
            name: self.name.clone(),
            set: self.set.clone(),
            made: self.made,
            refs: self.refs.clone(),
        }
    }
}

impl Smart {
    /// A new smart collection named as typed, of the filter given.
    pub(crate) fn new(typed: &str, filter: Saved, root: Option<PathBuf>) -> Smart {
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
            version: VERSION,
            name: self.name.clone(),
            set: self.set.clone(),
            made: self.made,
            filter: self.filter.clone(),
            root: self.root.as_deref().map(hint_of),
        }
    }
}

/// The collections as the editor holds them: the folder they are kept
/// in, each one, and what the window marks with them.
#[derive(Default)]
pub(crate) struct Store {
    /// None for a session that keeps nothing (a test's, unless it gives
    /// one), or with no data directory.
    pub(crate) dir: Option<PathBuf>,
    pub(crate) list: Vec<Collection>,
    pub(crate) smart: Vec<Smart>,
    /// The collection B adds to outside a collection's own view: the
    /// last one added to from the menu or made.
    pub(crate) target: Option<String>,
    /// The index's rows whose hash is a member of any collection: the
    /// frames the browser marks.
    pub(crate) member_ids: HashSet<i64>,
    /// The sidebar's rows, one model for the session, its rows replaced
    /// only when they change.
    pub(crate) rows: Rc<slint::VecModel<CollectionChip>>,
    /// The last read of a collection: its id, how many of its
    /// references were missing, and how many of those Forget missing
    /// may drop (the rest hinted under an offline root).
    pub(crate) missing: Option<(String, usize, usize)>,
    /// The sheet's question while it is up.
    pub(crate) asked: Option<crate::panel::collections::Asked>,
    /// The frame menu's names of the collections, one model for the
    /// session, replaced only when they change: a model made afresh
    /// rebuilds the menu's items under a menu that is up.
    pub(crate) names: Rc<slint::VecModel<slint::SharedString>>,
    /// Each file's content hash as it was when read or last written
    /// here: a file whose bytes differ was written since by another
    /// editor or by hand, and is read and joined before ours goes over
    /// it.
    stamps: std::collections::HashMap<String, Stamp>,
    /// Each collection's hashes as its file held them when read or last
    /// written here: what a file changed since is joined against, so
    /// only the other side's own additions and removals are taken.
    bases: std::collections::HashMap<String, HashSet<String>>,
}

/// A hash of a file's contents.
type Stamp = blake3::Hash;

fn stamp_of(path: &Path) -> Option<Stamp> {
    let bytes = std::fs::read(path).ok()?;
    Some(blake3::hash(&bytes))
}

fn read_collection(path: &Path) -> Result<CollectionFile, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice::<CollectionFile>(&bytes).map_err(|e| e.to_string())
}

/// Write `text` to `path` through a part file beside it, renamed over
/// it, so a crash mid-write never leaves half a file. The part file
/// carries the process's id, so two editors never write the same one.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let part = path.with_extension(format!("json.{}.part", std::process::id()));
    std::fs::write(&part, text)?;
    std::fs::rename(&part, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })
}

impl Store {
    /// The folder for a library at `library`: beside it.
    pub(crate) fn dir_beside(library: &Path) -> PathBuf {
        library
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .join(DIR)
    }

    /// Every collection in `dir`. A file that will not read, or one a
    /// later build wrote, is said and left where it is: nothing here
    /// writes to a file it did not read.
    pub(crate) fn load(dir: PathBuf) -> Store {
        let mut store = Store {
            dir: Some(dir.clone()),
            ..Store::default()
        };
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return store,
            Err(e) => {
                tracing::warn!("collections {}: {e}", dir.display());
                return store;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let read = || std::fs::read(&path);
            if let Some(id) = file.strip_suffix(SMART_END) {
                match read().map_err(|e| e.to_string()).and_then(|b| {
                    serde_json::from_slice::<SmartFile>(&b).map_err(|e| e.to_string())
                }) {
                    Ok(f) if f.version <= VERSION => {
                        if let Some(stamp) = stamp_of(&path) {
                            store.stamps.insert(id.to_string(), stamp);
                        }
                        store.smart.push(Smart {
                            id: id.to_string(),
                            name: f.name,
                            set: f.set,
                            made: f.made,
                            filter: f.filter,
                            root: f.root.map(PathBuf::from),
                        })
                    }
                    Ok(f) => tracing::warn!(
                        "collection {}: version {} is a later build's; left alone",
                        path.display(),
                        f.version
                    ),
                    Err(e) => tracing::warn!("collection {}: {e}; left alone", path.display()),
                }
            } else if let Some(id) = file.strip_suffix(PLAIN_END) {
                match read_collection(&path) {
                    Ok(f) if f.version <= VERSION => {
                        if let Some(stamp) = stamp_of(&path) {
                            store.stamps.insert(id.to_string(), stamp);
                        }
                        store.bases.insert(
                            id.to_string(),
                            f.refs.iter().map(|r| r.hash.clone()).collect(),
                        );
                        store.list.push(Collection {
                            id: id.to_string(),
                            name: f.name,
                            set: f.set,
                            made: f.made,
                            refs: f.refs,
                        })
                    }
                    Ok(f) => tracing::warn!(
                        "collection {}: version {} is a later build's; left alone",
                        path.display(),
                        f.version
                    ),
                    Err(e) => tracing::warn!("collection {}: {e}; left alone", path.display()),
                }
            }
        }
        store.sort();
        tracing::info!(
            "collections: {} and {} smart, from {}",
            store.list.len(),
            store.smart.len(),
            dir.display()
        );
        store
    }

    /// By set, then name, case aside: the sidebar's order.
    pub(crate) fn sort(&mut self) {
        let key = |set: &str, name: &str| (set.to_lowercase(), name.to_lowercase());
        self.list.sort_by_key(|c| key(&c.set, &c.name));
        self.smart.sort_by_key(|s| key(&s.set, &s.name));
    }

    pub(crate) fn collection(&self, id: &str) -> Option<&Collection> {
        self.list.iter().find(|c| c.id == id)
    }

    pub(crate) fn collection_mut(&mut self, id: &str) -> Option<&mut Collection> {
        self.list.iter_mut().find(|c| c.id == id)
    }

    pub(crate) fn smart(&self, id: &str) -> Option<&Smart> {
        self.smart.iter().find(|s| s.id == id)
    }

    /// Whether `id` is taken: by a collection here, or by a file in
    /// the folder.
    pub(crate) fn id_taken(&self, id: &str) -> bool {
        self.collection(id).is_some()
            || self.smart(id).is_some()
            || [false, true]
                .iter()
                .any(|&smart| self.file_of(id, smart).is_some_and(|f| f.exists()))
    }

    /// Every member hash, each once.
    pub(crate) fn hashes(&self) -> HashSet<&str> {
        self.list
            .iter()
            .flat_map(|c| c.refs.iter().map(|r| r.hash.as_str()))
            .collect()
    }

    fn file_of(&self, id: &str, smart: bool) -> Option<PathBuf> {
        let end = if smart { SMART_END } else { PLAIN_END };
        Some(self.dir.as_ref()?.join(format!("{id}{end}")))
    }

    /// Write the collection or the smart collection `id` to its file.
    /// Said in the log when it cannot be; the editor goes on. A file
    /// changed on disk since it was read or last written here (another
    /// editor, a hand edit, or only its time moved by a sync tool) is
    /// read first and joined against what it held then: a reference
    /// the other side added is put at our end, and one it took out is
    /// taken out of ours; what we added or took out ourselves stands.
    /// A file that will not read is kept beside itself as `.json.bad`
    /// before ours goes over it. A smart collection is ours, and the
    /// other is said in the log.
    pub(crate) fn save(&mut self, id: &str) {
        let smart = self.smart(id).is_some();
        if let Some(file) = self.file_of(id, smart)
            && let Some(now) = stamp_of(&file)
            && self.stamps.get(id) != Some(&now)
        {
            if smart {
                tracing::warn!(
                    "smart collection {}: changed on disk since it was read; ours written over it",
                    file.display()
                );
            } else {
                match read_collection(&file) {
                    Ok(theirs) if theirs.version <= VERSION => {
                        let base = self.bases.get(id).cloned().unwrap_or_default();
                        if let Some(c) = self.collection_mut(id) {
                            let on_disk: HashSet<&str> =
                                theirs.refs.iter().map(|r| r.hash.as_str()).collect();
                            let before = c.refs.len();
                            c.refs.retain(|r| {
                                !base.contains(&r.hash) || on_disk.contains(r.hash.as_str())
                            });
                            let dropped = before - c.refs.len();
                            let mut joined = 0;
                            for r in theirs.refs {
                                if !base.contains(&r.hash) && !c.has(&r.hash) {
                                    c.refs.push(r);
                                    joined += 1;
                                }
                            }
                            if joined + dropped > 0 {
                                tracing::warn!(
                                    "collection {}: changed on disk since it was read; \
                                     {joined} of its references joined to ours, {dropped} \
                                     taken out as it took them out",
                                    file.display()
                                );
                            }
                        }
                    }
                    Ok(_) => {
                        tracing::warn!(
                            "collection {}: a later build wrote it since; left alone",
                            file.display()
                        );
                        return;
                    }
                    Err(e) => {
                        // A second broken file goes over the earlier
                        // copy, on purpose: one is enough to recover a
                        // hand edit from, and names need no stamps.
                        let aside = file.with_extension("json.bad");
                        let kept = match std::fs::copy(&file, &aside) {
                            Ok(_) => format!("kept as {}", aside.display()),
                            Err(c) => format!("could not keep a copy ({c})"),
                        };
                        tracing::warn!(
                            "collection {}: changed on disk and unreadable ({e}); {kept}; ours written over it",
                            file.display()
                        );
                    }
                }
            }
        }
        let (text, file) = if let Some(c) = self.collection(id) {
            (
                serde_json::to_string_pretty(&c.to_file()),
                self.file_of(id, false),
            )
        } else if let Some(s) = self.smart(id) {
            (
                serde_json::to_string_pretty(&s.to_file()),
                self.file_of(id, true),
            )
        } else {
            return;
        };
        let Some(file) = file else {
            return;
        };
        let written = text
            .map_err(std::io::Error::other)
            .and_then(|t| write_atomic(&file, &t));
        match written {
            Ok(()) => {
                if let Some(stamp) = stamp_of(&file) {
                    self.stamps.insert(id.to_string(), stamp);
                }
                if let Some(c) = self.collection(id) {
                    let base = c.refs.iter().map(|r| r.hash.clone()).collect();
                    self.bases.insert(id.to_string(), base);
                }
            }
            Err(e) => tracing::warn!("collection not saved to {}: {e}", file.display()),
        }
    }

    /// The collection or smart collection `id` gone, with its file.
    pub(crate) fn delete(&mut self, id: &str) -> bool {
        let smart = self.smart(id).is_some();
        let before = self.list.len() + self.smart.len();
        self.list.retain(|c| c.id != id);
        self.smart.retain(|s| s.id != id);
        if self.target.as_deref() == Some(id) {
            self.target = None;
        }
        if let Some(file) = self.file_of(id, smart) {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!("collection {}: not deleted: {e}", file.display()),
            }
        }
        before != self.list.len() + self.smart.len()
    }

    /// The files the index found rewritten in place, healed into the
    /// references. First by the hint: a reference to the old hash whose
    /// hint is the rewritten file's path follows the file, to its new
    /// hash. Then, for a reference to an old hash that no rewrite's path
    /// matched, only when the index now has that hash nowhere (`copies`
    /// lists its files, the missing left out; `None` when the index
    /// could not say, and then nothing is taken for gone): the picture
    /// was rewritten wherever it was, and the reference follows the
    /// first rewrite of it. A reference whose new hash another holds
    /// already is never dropped: while a copy of its own picture
    /// survives it is pointed at that copy, and with none known it
    /// stays as it was, missing. Only a member's hash is asked of the
    /// index.
    /// Each collection changed is written; their ids.
    pub(crate) fn heal(
        &mut self,
        rehashed: &[Rehash],
        copies: &dyn Fn(&str) -> Option<Vec<PathBuf>>,
    ) -> Vec<String> {
        let members: HashSet<String> = self.hashes().into_iter().map(str::to_string).collect();
        let ours: Vec<&Rehash> = rehashed
            .iter()
            .filter(|r| r.old != r.new && members.contains(&r.old))
            .collect();
        if ours.is_empty() {
            return Vec::new();
        }
        // Each old hash's first copy left, asked once: `None` when the
        // index could not say, `Some(None)` when there is none.
        let mut left: std::collections::HashMap<&str, Option<Option<String>>> =
            std::collections::HashMap::new();
        for r in &ours {
            left.entry(r.old.as_str()).or_insert_with(|| {
                copies(&r.old).map(|c| c.iter().map(|p| hint_of(p)).find(|p| !p.is_empty()))
            });
        }
        let by_path: std::collections::HashMap<(&str, String), &Rehash> = ours
            .iter()
            .map(|r| ((r.old.as_str(), hint_of(&r.path)), *r))
            .collect();
        let mut changed = Vec::new();
        for c in &mut self.list {
            // What each reference heals by, decided before any changes:
            // its own path first, then the fallback.
            let mut plan: Vec<(String, &Rehash)> = Vec::new();
            for x in &c.refs {
                if let Some(r) = by_path.get(&(x.hash.as_str(), x.hint.clone())) {
                    plan.push((x.hash.clone(), r));
                }
            }
            for x in &c.refs {
                if plan.iter().any(|(h, _)| *h == x.hash) {
                    continue;
                }
                if matches!(left.get(x.hash.as_str()), Some(Some(None)))
                    && let Some(r) = ours.iter().find(|r| r.old == x.hash)
                {
                    plan.push((x.hash.clone(), r));
                }
            }
            let mut any = false;
            for (old, r) in plan {
                let survivor = left.get(old.as_str()).cloned().flatten().flatten();
                any |= c.take_hash(&old, &r.new, &hint_of(&r.path), survivor.as_deref());
            }
            if any {
                changed.push(c.id.clone());
            }
        }
        for id in &changed {
            tracing::info!(
                "collection {}: healed by its hints after a file was rewritten",
                self.collection(id).map(|c| c.name.as_str()).unwrap_or("?")
            );
            self.save(id);
        }
        changed
    }

    /// The references of collection `id` the index has nowhere, by the
    /// same test the status line counts them missing by ([`resolve`],
    /// asked again here), dropped and the file written. Kept, as perhaps
    /// only out of reach: one whose hint is under a root in `away`
    /// (offline), under a network mount in `far`, or in a folder that
    /// does not answer `answers` (the editor's looks, each with a time
    /// limit, so a share gone quiet never holds the caller); and one
    /// whose hint is a file on disk, which a pass has yet to read, or
    /// that the disk will not say is gone (a folder that cannot be read,
    /// a mount that died). The disk is asked of a hint only once its
    /// folder has answered, and a reference is dropped only when the
    /// disk says its file, or its folder, is not found. With
    /// no index to ask, or one that will not answer, nothing is known
    /// gone and nothing changes (`None`). The caller makes sure no pass
    /// is under way: the index says a file moved is missing until the
    /// pass reaches where it went.
    pub(crate) fn forget_missing(
        &mut self,
        id: &str,
        lib: Option<&Library>,
        away: &[PathBuf],
        far: &[PathBuf],
        answers: &Answers<'_>,
    ) -> Option<Forgot> {
        let c = self.collection(id)?;
        let shown = match resolve(lib?, &c.refs, away, &[], false) {
            Ok(shown) => shown,
            Err(e) => {
                tracing::warn!("collection {}: {e}; nothing forgotten", c.name);
                return None;
            }
        };
        let mut kept_away = shown.away;
        let (far_off, near): (Vec<&Reference>, Vec<&Reference>) = shown
            .gone
            .iter()
            .partition(|r| far.iter().any(|f| Path::new(&r.hint).starts_with(f)));
        kept_away += far_off.len();
        let mut folders: Vec<PathBuf> = Vec::new();
        for r in &near {
            if let Some(dir) = Path::new(&r.hint).parent()
                && !folders.iter().any(|d| d == dir)
            {
                folders.push(dir.to_path_buf());
            }
        }
        let answered = if folders.is_empty() {
            Default::default()
        } else {
            answers(&folders)
        };
        let mut on_disk = 0;
        let mut gone: HashSet<&str> = HashSet::new();
        for r in near {
            let hint = Path::new(&r.hint);
            let folder = hint
                .parent()
                .and_then(|d| answered.get(d))
                .copied()
                .flatten();
            let absent = |p: &Path| {
                matches!(
                    std::fs::symlink_metadata(p),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound
                )
            };
            match folder {
                None => kept_away += 1,
                // The folder not there at all: its files neither.
                Some(false) if hint.parent().is_some_and(absent) => {
                    gone.insert(r.hash.as_str());
                }
                // Gone only when the disk says so of the file itself: a
                // folder that cannot be read, a mount that has died
                // under us, any other error, keeps it.
                Some(_) => match std::fs::symlink_metadata(hint) {
                    Ok(_) => on_disk += 1,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        gone.insert(r.hash.as_str());
                    }
                    Err(_) => kept_away += 1,
                },
            }
        }
        let c = self.collection_mut(id)?;
        let n = c.remove(&gone);
        if n > 0 {
            tracing::info!("collection {}: {n} missing forgotten", c.name);
            self.save(id);
        }
        let forgot = Forgot {
            forgotten: n,
            missing: shown.missing - n,
            away: kept_away,
            on_disk,
        };
        self.missing = Some((id.to_string(), forgot.missing, 0));
        Some(forgot)
    }

    /// What a read of a collection found to change in it, put in: each
    /// reference of `was` takes its new hash and hint. True when the
    /// collection changed, which is then written.
    pub(crate) fn apply(&mut self, id: &str, changes: &[Change]) -> bool {
        let Some(c) = self.collection_mut(id) else {
            return false;
        };
        let mut any = false;
        // A read changes a reference's hash only when nothing of its own
        // is left in the index: no copy survives it.
        for ch in changes {
            any |= c.take_hash(&ch.was, &ch.hash, &ch.hint, None);
        }
        if any {
            self.save(id);
        }
        any
    }
}

/// The editor's look at folders, each with a time limit: whether each
/// is there, or `None` when it did not answer in time.
pub(crate) type Answers<'a> =
    dyn Fn(&[PathBuf]) -> std::collections::HashMap<PathBuf, Option<bool>> + 'a;

/// What Forget missing did, by its own fresh read: how many it dropped,
/// how many are missing still, and of those, how many are perhaps only
/// out of reach (an offline root, a network mount, a folder that did
/// not answer) and how many have a file at the hint the index has not
/// read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Forgot {
    pub(crate) forgotten: usize,
    pub(crate) missing: usize,
    pub(crate) away: usize,
    pub(crate) on_disk: usize,
}

/// A reference that a read of its collection found somewhere else: the
/// hash it had, and the hash and the hint it has now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Change {
    pub(crate) was: String,
    pub(crate) hash: String,
    pub(crate) hint: String,
}

/// A collection read from the index for the browser.
#[derive(Debug, Clone, Default)]
pub(crate) struct Shown {
    /// The frames to list, each hash once, in the collection's order
    /// or by capture time.
    pub(crate) listed: Vec<(PathBuf, RowMeta)>,
    /// References with nothing at their hash or their hint.
    pub(crate) missing: usize,
    /// The missing whose hint is under no root that is away: what
    /// Forget missing may drop, once it has looked at the disk.
    pub(crate) gone: Vec<Reference>,
    /// Of the missing, those hinted under a root that is away: perhaps
    /// only out of reach, and kept.
    pub(crate) away: usize,
    /// References found moved or rewritten, for the file.
    pub(crate) changes: Vec<Change>,
}

/// Where each of `refs` is shown from, by the index alone: the hinted
/// file when the index has it there and its root is not `away`, else
/// the first of the hash's files under a root that is not an archive,
/// else any that is not away, else the hinted one (shown dimmed) or
/// the first. A reference whose hash the index has nowhere takes the
/// hash of the file at its hint, when there is one (a file rewritten
/// while the editor was not looking). A hint with no file of the hash
/// at it moves to the file shown. With `capture`, sorted by the time
/// taken, those with none last, the collection's order kept among
/// equals.
pub(crate) fn resolve(
    lib: &Library,
    refs: &[Reference],
    away: &[PathBuf],
    archives: &[PathBuf],
    capture: bool,
) -> greycard_library::Result<Shown> {
    let mut shown = Shown::default();
    let mut taken: Vec<Option<String>> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let is_away = |p: &Path| away.iter().any(|r| p.starts_with(r));
    let on_archive = |p: &Path| archives.iter().any(|r| p.starts_with(r));
    let held: HashSet<&str> = refs.iter().map(|r| r.hash.as_str()).collect();
    for r in refs {
        let mut hash = r.hash.clone();
        let mut rows = lib.rows_by_hash(&hash)?;
        // The file at the hint is taken for this picture rewritten only
        // when it is no picture the collection holds already: a file
        // overwritten with another member's content is that member, and
        // this reference stays missing rather than be dropped for it.
        if rows.is_empty()
            && let Some(at_hint) = lib.by_path(Path::new(&r.hint))?.filter(|e| !e.missing)
            && !held.contains(at_hint.hash.as_str())
        {
            hash = at_hint.hash;
            rows = lib.rows_by_hash(&hash)?;
        }
        if rows.is_empty() || !seen.insert(hash.clone()) {
            if rows.is_empty() {
                shown.missing += 1;
                // The index alone: the disk is never asked here.
                if is_away(Path::new(&r.hint)) {
                    shown.away += 1;
                } else {
                    shown.gone.push(r.clone());
                }
            }
            continue;
        }
        let hinted = rows.iter().position(|h| hint_of(&h.path) == r.hint);
        let pick = hinted
            .filter(|&i| !is_away(&rows[i].path))
            .or_else(|| {
                rows.iter()
                    .position(|h| !is_away(&h.path) && !on_archive(&h.path))
            })
            .or_else(|| rows.iter().position(|h| !is_away(&h.path)))
            .or(hinted)
            .unwrap_or(0);
        let row = rows.swap_remove(pick);
        // The hint moves when nothing of the hash is at it any more.
        let hint = if hinted.is_none() {
            hint_of(&row.path)
        } else {
            r.hint.clone()
        };
        if hash != r.hash || hint != r.hint {
            shown.changes.push(Change {
                was: r.hash.clone(),
                hash,
                hint,
            });
        }
        taken.push(row.taken);
        shown.listed.push((row.path, row.row));
    }
    if capture {
        let mut order: Vec<usize> = (0..shown.listed.len()).collect();
        order.sort_by(|&a, &b| match (&taken[a], &taken[b]) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        let mut listed: Vec<Option<(PathBuf, RowMeta)>> =
            shown.listed.into_iter().map(Some).collect();
        shown.listed = order.iter().filter_map(|&i| listed[i].take()).collect();
    }
    Ok(shown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Filter;
    use greycard_library::fixture::{A7, R5, R6, write_frame};

    fn scratch(what: &str) -> PathBuf {
        let dir = crate::testing::scratch_dir(&format!("collections-{what}"));
        std::fs::create_dir_all(&dir).unwrap();
        dunce::canonicalize(&dir).unwrap()
    }

    fn r(hash: &str, hint: &str) -> Reference {
        Reference {
            hash: hash.into(),
            hint: hint.into(),
        }
    }

    /// Every folder answers, as the disk says.
    fn answering(dirs: &[PathBuf]) -> std::collections::HashMap<PathBuf, Option<bool>> {
        dirs.iter().map(|d| (d.clone(), Some(d.is_dir()))).collect()
    }

    fn hashes(c: &Collection) -> Vec<&str> {
        c.refs.iter().map(|r| r.hash.as_str()).collect()
    }

    #[test]
    fn a_name_typed_with_slashes_is_its_sets_and_its_name() {
        assert_eq!(
            split_name("Clients/2026/Cover picks"),
            ("Clients/2026".into(), "Cover picks".into())
        );
        assert_eq!(
            split_name(" Portfolio "),
            (String::new(), "Portfolio".into())
        );
        assert_eq!(split_name("/a//b/"), ("a".into(), "b".into()));
        assert_eq!(split_name(""), (String::new(), String::new()));
        assert_eq!(full_name("Clients/2026", "Cover"), "Clients/2026/Cover");
        assert_eq!(full_name("", "Cover"), "Cover");
        assert_ne!(Collection::new("a").id, Collection::new("a").id);
    }

    /// The files round trip, one a collection, in a folder of their own;
    /// a file of a later build and one that will not read are left as
    /// they were, and a delete takes the file with it.
    #[test]
    fn the_files_round_trip_and_leave_what_they_cannot_read_alone() {
        let dir = scratch("round").join(DIR);
        let mut store = Store::load(dir.clone());
        assert!(store.list.is_empty() && store.smart.is_empty());
        let mut c = Collection::new("Clients/2026/Cover picks");
        c.add(&[
            ("aa".into(), PathBuf::from("/shoot/a.CR3")),
            ("bb".into(), PathBuf::from("/shoot/b.CR3")),
        ]);
        let filter = Filter {
            stars: crate::filter::Stars::AtLeast(4),
            text: "camera:R6 iso>=3200".into(),
            ..Filter::default()
        };
        let s = Smart::new(
            "Best of",
            Saved::of(&filter),
            Some(PathBuf::from("/photos")),
        );
        let (cid, sid) = (c.id.clone(), s.id.clone());
        store.list.push(c.clone());
        store.smart.push(s.clone());
        store.save(&cid);
        store.save(&sid);
        let files: HashSet<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            files,
            HashSet::from([format!("{cid}.json"), format!("{sid}.smart.json")])
        );
        let text = std::fs::read_to_string(dir.join(format!("{cid}.json"))).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["name"], "Cover picks");
        assert_eq!(v["set"], "Clients/2026");
        assert_eq!(v["refs"][1]["hash"], "bb");
        assert_eq!(v["refs"][1]["hint"], "/shoot/b.CR3");
        // A later build's, and one that is not JSON.
        std::fs::write(
            dir.join("later.json"),
            r#"{"version": 9, "name": "Later", "refs": []}"#,
        )
        .unwrap();
        std::fs::write(dir.join("broken.json"), "{ not json").unwrap();
        let back = Store::load(dir.clone());
        assert_eq!(back.list, vec![c]);
        assert_eq!(back.smart, vec![s.clone()]);
        assert_eq!(back.smart[0].filter.filter(), filter);
        assert_eq!(
            std::fs::read_to_string(dir.join("broken.json")).unwrap(),
            "{ not json"
        );
        let mut back = back;
        assert!(back.delete(&sid));
        assert!(!dir.join(format!("{sid}.smart.json")).exists());
        assert!(dir.join("later.json").exists());
        crate::testing::remove_dir_retry(dir.parent().unwrap());
    }

    #[test]
    fn an_add_appends_new_pictures_and_moves_the_hint_of_a_member() {
        let mut c = Collection::new("Picks");
        let added = c.add(&[
            ("aa".into(), PathBuf::from("/shoot/a.CR3")),
            ("bb".into(), PathBuf::from("/shoot/b.CR3")),
        ]);
        assert_eq!(added, Added { added: 2, moved: 0 });
        // The selects folder's copy of a: the same picture, no new
        // entry, and the hint is now the copy.
        let again = c.add(&[
            ("cc".into(), PathBuf::from("/shoot/c.CR3")),
            ("aa".into(), PathBuf::from("/selects/a.CR3")),
        ]);
        assert_eq!(again, Added { added: 1, moved: 1 });
        assert_eq!(hashes(&c), ["aa", "bb", "cc"]);
        assert_eq!(c.refs[0].hint, "/selects/a.CR3");
        assert_eq!(c.remove(&HashSet::from(["bb", "zz"])), 1);
        assert_eq!(hashes(&c), ["aa", "cc"]);
    }

    #[test]
    fn a_reorder_puts_the_moving_frames_before_the_one_dropped_on() {
        let mut c = Collection::new("Order");
        c.refs = ["a", "b", "c", "d", "e"].map(|h| r(h, h)).to_vec();
        assert!(c.reorder(&HashSet::from(["d"]), Some("b")));
        assert_eq!(hashes(&c), ["a", "d", "b", "c", "e"]);
        // Two, keeping their own order, to the end.
        assert!(c.reorder(&HashSet::from(["a", "b"]), None));
        assert_eq!(hashes(&c), ["d", "c", "e", "a", "b"]);
        // Dropped on one of themselves: before the next that is not.
        assert!(!c.reorder(&HashSet::from(["d", "c"]), Some("c")));
        assert_eq!(hashes(&c), ["d", "c", "e", "a", "b"]);
        assert!(!c.reorder(&HashSet::from(["e"]), Some("a")));
    }

    /// A pass that finds a file rewritten heals the reference whose hint
    /// is that file, and one whose old hash is gone everywhere; one
    /// whose old hash is still on another copy stays with it.
    #[test]
    fn a_file_rewritten_heals_the_references_by_their_hints() {
        let mut store = Store::default();
        let mut one = Collection::new("One");
        one.refs = vec![r("old", "/shoot/a.CR3"), r("x", "/shoot/x.CR3")];
        let mut two = Collection::new("Two");
        two.refs = vec![r("old", "/selects/a.CR3")];
        let mut three = Collection::new("Three");
        three.refs = vec![r("old", "/selects/a.CR3"), r("new", "/shoot/n.CR3")];
        store.list = vec![one.clone(), two.clone(), three.clone()];
        let rehash = Rehash {
            path: PathBuf::from("/shoot/a.CR3"),
            old: "old".into(),
            new: "new".into(),
        };
        // The selects copy still has the old hash: only the hinted heal.
        let changed = store.heal(std::slice::from_ref(&rehash), &|_| {
            Some(vec![PathBuf::from("/selects/a.CR3")])
        });
        assert_eq!(changed, [one.id.clone()]);
        assert_eq!(store.list[0].refs[0], r("new", "/shoot/a.CR3"));
        assert_eq!(store.list[1].refs, two.refs);
        // The old hash gone from the index: every reference heals; one
        // whose new hash another holds already stays as it was, missing,
        // rather than be dropped or show the other picture twice.
        let changed = store.heal(std::slice::from_ref(&rehash), &|h| {
            Some(if h == "old" {
                Vec::new()
            } else {
                vec![PathBuf::from("/x")]
            })
        });
        assert_eq!(changed, [two.id.clone()]);
        assert_eq!(store.list[1].refs, [r("new", "/shoot/a.CR3")]);
        assert_eq!(store.list[2].refs, three.refs);
        // With no index to ask, the copies unknown: nothing taken for
        // gone, and no hint written empty.
        let mut four = Collection::new("Four");
        four.refs = vec![r("old", "/selects/a.CR3"), r("new", "/shoot/n.CR3")];
        let mut five = Collection::new("Five");
        five.refs = vec![r("old", "/shoot/a.CR3"), r("new", "/shoot/n.CR3")];
        store.list = vec![four.clone(), five.clone()];
        let changed = store.heal(&[rehash], &|_| None);
        assert!(changed.is_empty(), "{changed:?}");
        assert_eq!(store.list[0].refs, four.refs);
        assert_eq!(store.list[1].refs, five.refs, "the hint kept, never empty");
    }

    /// The shoot's copy of a picture rewritten with another member's
    /// content, while the selects' copy keeps the picture: the reference
    /// is not dropped as a duplicate but follows the copy that survives;
    /// and once that copy is gone too, an open does not take the file at
    /// its hint for it (that file is the other member now), and keeps it
    /// as missing.
    #[test]
    fn a_rewrite_into_another_members_picture_drops_no_reference() {
        let dir = scratch("into-member");
        let (shoot, selects) = (dir.join("shoot"), dir.join("selects"));
        std::fs::create_dir_all(&shoot).unwrap();
        std::fs::create_dir_all(&selects).unwrap();
        let (a, b, copy) = (
            shoot.join("a.tif"),
            shoot.join("b.tif"),
            selects.join("a.tif"),
        );
        write_frame(&a, &R5, 1);
        write_frame(&b, &R6, 2);
        std::fs::copy(&a, &copy).unwrap();
        write_frame(&selects.join("other.tif"), &A7, 3);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&dir, &mut |_| {}).unwrap();
        let hash = |lib: &Library, p: &Path| lib.by_path(p).unwrap().unwrap().hash;
        let (h, hb) = (hash(&lib, &a), hash(&lib, &b));
        let mut store = Store::default();
        let mut c = Collection::new("Two");
        c.add(&[(h.clone(), a.clone()), (hb.clone(), b.clone())]);
        store.list.push(c);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::copy(&b, &a).unwrap();
        let report = lib.index_tree(&dir, &mut |_| {}).unwrap();
        assert_eq!(report.rehashed.len(), 1, "{report:?}");
        assert_eq!(
            (&report.rehashed[0].old, &report.rehashed[0].new),
            (&h, &hb)
        );
        let copies = |x: &str| -> Option<Vec<PathBuf>> {
            Some(
                lib.rows_by_hash(x)
                    .unwrap()
                    .into_iter()
                    .map(|r| r.path)
                    .collect(),
            )
        };
        store.heal(&report.rehashed, &copies);
        assert_eq!(
            store.list[0].refs,
            [r(&h, &hint_of(&copy)), r(&hb, &hint_of(&b))],
            "kept, and pointed at the copy that still is the picture"
        );
        let shown = resolve(&lib, &store.list[0].refs, &[], &[], false).unwrap();
        let paths: Vec<&PathBuf> = shown.listed.iter().map(|(p, _)| p).collect();
        assert_eq!(paths, [&copy, &b]);
        // The selects' copy gone too, the reference hinted at the shoot's
        // file, which holds b's picture now: missing, not adopted.
        std::fs::remove_file(&copy).unwrap();
        lib.index_tree(&dir, &mut |_| {}).unwrap();
        let refs = [r(&h, &hint_of(&a)), r(&hb, &hint_of(&b))];
        let shown = resolve(&lib, &refs, &[], &[], false).unwrap();
        assert_eq!(shown.missing, 1);
        assert_eq!(shown.listed.len(), 1);
        assert!(shown.changes.is_empty(), "{:?}", shown.changes);
        crate::testing::remove_dir_retry(&dir);
    }

    /// Both copies of a picture rewritten in one pass, each to a hash of
    /// its own: a reference heals by the rewrite at its own hint, not by
    /// whichever the pass reported first.
    #[test]
    fn both_copies_rewritten_in_one_pass_heal_each_by_its_own_path() {
        let mut store = Store::default();
        let mut c = Collection::new("Both");
        c.refs = vec![r("h", "/selects/a.CR3")];
        store.list.push(c);
        let rehashed = [
            Rehash {
                path: PathBuf::from("/shoot/a.CR3"),
                old: "h".into(),
                new: "s1".into(),
            },
            Rehash {
                path: PathBuf::from("/selects/a.CR3"),
                old: "h".into(),
                new: "s2".into(),
            },
        ];
        store.heal(&rehashed, &|_| Some(Vec::new()));
        assert_eq!(store.list[0].refs, [r("s2", "/selects/a.CR3")]);
        // Hinted somewhere neither rewrite was, with the old hash gone:
        // the first rewrite of it, and the hint goes with it.
        let mut c = Collection::new("Elsewhere");
        c.refs = vec![r("h", "/moved/a.CR3")];
        store.list = vec![c];
        store.heal(&rehashed, &|_| Some(Vec::new()));
        assert_eq!(store.list[0].refs, [r("s1", "/shoot/a.CR3")]);
        // No member's hash among the rewrites: nothing asked of the index.
        let asked = std::cell::Cell::new(0);
        let other = [Rehash {
            path: PathBuf::from("/x.CR3"),
            old: "zz".into(),
            new: "yy".into(),
        }];
        let changed = store.heal(&other, &|_| {
            asked.set(asked.get() + 1);
            Some(Vec::new())
        });
        assert!(changed.is_empty());
        assert_eq!(asked.get(), 0);
    }

    /// A collection's file written by someone else since this store read
    /// it (a second editor, a hand edit) is read and joined before ours
    /// goes over it: neither side's additions are lost.
    #[test]
    fn a_file_changed_on_disk_is_joined_before_it_is_written() {
        let dir = scratch("joined").join(DIR);
        let mut first = Store::load(dir.clone());
        let mut c = Collection::new("Shared");
        c.add(&[("aa".into(), PathBuf::from("/s/a.CR3"))]);
        let id = c.id.clone();
        first.list.push(c);
        first.save(&id);
        // A second editor reads it, adds b and writes.
        let mut second = Store::load(dir.clone());
        second
            .collection_mut(&id)
            .unwrap()
            .add(&[("bb".into(), PathBuf::from("/s/b.CR3"))]);
        second.save(&id);
        // The first adds c and writes: b is joined, not lost.
        first
            .collection_mut(&id)
            .unwrap()
            .add(&[("cc".into(), PathBuf::from("/s/c.CR3"))]);
        first.save(&id);
        let back = Store::load(dir.clone());
        assert_eq!(hashes(back.collection(&id).unwrap()), ["aa", "cc", "bb"]);
        // Its own writes since are not taken for someone else's.
        first
            .collection_mut(&id)
            .unwrap()
            .remove(&HashSet::from(["cc"]));
        first.save(&id);
        let back = Store::load(dir.clone());
        assert_eq!(hashes(back.collection(&id).unwrap()), ["aa", "bb"]);
        assert!(back.id_taken(&id));
        // Our own removal, then only the file's time moved (a sync tool
        // touching it, the content the same): it stays removed.
        first
            .collection_mut(&id)
            .unwrap()
            .remove(&HashSet::from(["bb"]));
        let file = dir.join(format!("{id}.json"));
        let same = std::fs::read(&file).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&file, &same).unwrap();
        first.save(&id);
        let back = Store::load(dir.clone());
        assert_eq!(hashes(back.collection(&id).unwrap()), ["aa"]);
        // A removal the other side made is taken from ours too.
        let mut second = Store::load(dir.clone());
        first
            .collection_mut(&id)
            .unwrap()
            .add(&[("dd".into(), PathBuf::from("/s/d.CR3"))]);
        second
            .collection_mut(&id)
            .unwrap()
            .remove(&HashSet::from(["aa"]));
        second.save(&id);
        first.save(&id);
        let back = Store::load(dir.clone());
        assert_eq!(hashes(back.collection(&id).unwrap()), ["dd"]);
        crate::testing::remove_dir_retry(dir.parent().unwrap());
    }
    /// A collection's file that will not read any more (a hand edit
    /// gone wrong since it was read) is kept beside itself as
    /// `.json.bad`, with what it held, before ours is written over it.
    /// An outside edit that keeps the file's length inside one mtime
    /// tick (a hint moved to a path of the same length) is still a
    /// change, since the stamp is the content's hash.
    #[test]
    fn a_same_length_edit_in_the_same_tick_is_still_joined() {
        let dir = scratch("same-length").join(DIR);
        let mut store = Store::load(dir.clone());
        let mut c = Collection::new("Shared");
        c.add(&[("aa".into(), PathBuf::from("/s/a.CR3"))]);
        let id = c.id.clone();
        store.list.push(c);
        store.save(&id);
        // The other side swaps a for b, same length, no time passing.
        let file = dir.join(format!("{id}.json"));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"aa\""));
        std::fs::write(&file, text.replace("\"aa\"", "\"bb\"")).unwrap();
        store
            .collection_mut(&id)
            .unwrap()
            .add(&[("cc".into(), PathBuf::from("/s/c.CR3"))]);
        store.save(&id);
        // Their removal of a and addition of b are both taken.
        let back = Store::load(dir.clone());
        assert_eq!(hashes(back.collection(&id).unwrap()), ["cc", "bb"]);
        crate::testing::remove_dir_retry(dir.parent().unwrap());
    }
    #[test]
    fn a_file_broken_on_disk_is_kept_aside_before_it_is_written() {
        let dir = scratch("broken").join(DIR);
        let mut store = Store::load(dir.clone());
        let mut c = Collection::new("Shared");
        c.add(&[("aa".into(), PathBuf::from("/s/a.CR3"))]);
        let id = c.id.clone();
        store.list.push(c);
        store.save(&id);
        let file = dir.join(format!("{id}.json"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&file, "{ not json").unwrap();
        store
            .collection_mut(&id)
            .unwrap()
            .add(&[("bb".into(), PathBuf::from("/s/b.CR3"))]);
        store.save(&id);
        let aside = dir.join(format!("{id}.json.bad"));
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "{ not json");
        let back = Store::load(dir.clone());
        assert_eq!(hashes(back.collection(&id).unwrap()), ["aa", "bb"]);
        crate::testing::remove_dir_retry(dir.parent().unwrap());
    }
    /// Two copies of a frame and three references, one of them to a
    /// hash nothing has: opened, each picture once, from the hint while
    /// it holds the hash, the missing one counted and kept; a file
    /// rewritten while nobody looked is found by its hint and healed;
    /// capture order sorts by the time taken.
    #[test]
    fn a_collection_opened_shows_each_picture_once_and_keeps_the_missing() {
        let dir = scratch("resolve");
        let (shoot, selects) = (dir.join("shoot"), dir.join("selects"));
        std::fs::create_dir_all(&shoot).unwrap();
        std::fs::create_dir_all(&selects).unwrap();
        let a = shoot.join("a.tif");
        let b = shoot.join("b.tif");
        write_frame(&a, &R5, 1);
        write_frame(&b, &R6, 2);
        std::fs::copy(&a, selects.join("a.tif")).unwrap();
        // Not alone in its folder: an emptied folder is taken for a
        // drive not there, and its rows are left as they were.
        write_frame(&selects.join("other.tif"), &A7, 9);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&dir, &mut |_| {}).unwrap();
        let hash = |lib: &Library, p: &Path| lib.by_path(p).unwrap().unwrap().hash;
        let (ha, hb) = (hash(&lib, &a), hash(&lib, &b));
        assert_eq!(ha, hash(&lib, &selects.join("a.tif")));
        let mut c = Collection::new("Mixed");
        c.add(&[
            (hb.clone(), b.clone()),
            (ha.clone(), selects.join("a.tif")),
            ("feedface".into(), shoot.join("gone.tif")),
        ]);
        let shown = resolve(&lib, &c.refs, &[], &[], false).unwrap();
        let paths: Vec<&PathBuf> = shown.listed.iter().map(|(p, _)| p).collect();
        assert_eq!(paths, [&b, &selects.join("a.tif")]);
        assert_eq!(shown.missing, 1);
        assert!(shown.changes.is_empty());
        // The hinted copy's root away: the other copy, hint kept.
        let shown = resolve(&lib, &c.refs, std::slice::from_ref(&selects), &[], false).unwrap();
        assert_eq!(shown.listed[1].0, a);
        assert!(shown.changes.is_empty());
        // The selects copy deleted and its folder passed: the shoot's,
        // and the hint follows it.
        std::fs::remove_file(selects.join("a.tif")).unwrap();
        lib.index_tree(&dir, &mut |_| {}).unwrap();
        let shown = resolve(&lib, &c.refs, &[], &[], false).unwrap();
        assert_eq!(shown.listed[1].0, a);
        assert_eq!(
            shown.changes,
            [Change {
                was: ha.clone(),
                hash: ha.clone(),
                hint: hint_of(&a)
            }]
        );
        let mut store = Store::default();
        store.list.push(c.clone());
        assert!(store.apply(&c.id, &shown.changes));
        // b rewritten by another tool, and the index told before the
        // collection heard: found by its hint at the open.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_frame(&b, &A7, 3);
        lib.index_tree(&dir, &mut |_| {}).unwrap();
        let hb2 = hash(&lib, &b);
        let refs = store.list[0].refs.clone();
        let shown = resolve(&lib, &refs, &[], &[], false).unwrap();
        assert_eq!(shown.listed[0].0, b);
        assert_eq!(
            shown.changes,
            [Change {
                was: hb.clone(),
                hash: hb2.clone(),
                hint: hint_of(&b)
            }]
        );
        assert_eq!(shown.missing, 1, "the missing one is still kept");
        assert!(store.apply(&c.id, &shown.changes));
        assert_eq!(store.list[0].refs.len(), 3);
        assert_eq!(store.list[0].refs[0].hash, hb2);
        // Capture order: the test frames' dates follow their seeds.
        let shown = resolve(&lib, &store.list[0].refs, &[], &[], true).unwrap();
        let taken: Vec<Option<String>> = shown
            .listed
            .iter()
            .map(|(p, _)| lib.by_path(p).unwrap().unwrap().exif.taken)
            .collect();
        let mut sorted = taken.clone();
        sorted.sort_by(|x, y| match (x, y) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        assert_eq!(taken, sorted);
        crate::testing::remove_dir_retry(&dir);
    }

    /// Forget missing drops the references the index has nowhere, by the
    /// status line's own count, and writes the file; a frame under an
    /// offline root is kept, whether the index still holds its row or
    /// last saw it missing, and with no index to ask nothing changes.
    #[test]
    fn forget_missing_drops_the_gone_and_keeps_the_offline() {
        let dir = scratch("forget");
        let (shoot, drive) = (dir.join("shoot"), dir.join("drive"));
        std::fs::create_dir_all(&shoot).unwrap();
        std::fs::create_dir_all(&drive).unwrap();
        let (a, b) = (shoot.join("a.tif"), shoot.join("b.tif"));
        let (on_drive, lost) = (drive.join("c.tif"), drive.join("d.tif"));
        write_frame(&a, &R5, 1);
        write_frame(&b, &R6, 2);
        write_frame(&on_drive, &A7, 3);
        write_frame(&lost, &A7, 4);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_tree(&dir, &mut |_| {}).unwrap();
        let hash = |lib: &Library, p: &Path| lib.by_path(p).unwrap().unwrap().hash;
        let (ha, hb, hc, hd) = (
            hash(&lib, &a),
            hash(&lib, &b),
            hash(&lib, &on_drive),
            hash(&lib, &lost),
        );
        // b deleted and d deleted, each seen missing by a pass; then the
        // drive unplugged, its rows left as the index has them.
        std::fs::remove_file(&b).unwrap();
        std::fs::remove_file(&lost).unwrap();
        lib.index_tree(&dir, &mut |_| {}).unwrap();
        assert!(lib.by_path(&lost).unwrap().unwrap().missing);
        let mut c = Collection::new("Picks");
        c.add(&[
            (ha.clone(), a.clone()),
            (hb.clone(), b.clone()),
            (hc.clone(), on_drive.clone()),
            (hd.clone(), lost.clone()),
            ("feedface".into(), shoot.join("never.tif")),
        ]);
        let id = c.id.clone();
        let mut store = Store {
            dir: Some(dir.join(DIR)),
            ..Store::default()
        };
        store.list.push(c);
        store.save(&id);
        let away = std::slice::from_ref(&drive);
        let look = answering;
        let before = resolve(&lib, &store.list[0].refs, away, &[], false).unwrap();
        assert_eq!(before.missing, 3, "b, d and the never-seen one");
        let gone: Vec<&str> = before.gone.iter().map(|r| r.hash.as_str()).collect();
        assert_eq!(gone, [hb.as_str(), "feedface"]);
        // No index to ask: nothing known gone, nothing changed.
        assert_eq!(store.forget_missing(&id, None, away, &[], &look), None);
        assert_eq!(store.list[0].refs.len(), 5);
        let forgot = store
            .forget_missing(&id, Some(&lib), away, &[], &look)
            .unwrap();
        assert_eq!(
            forgot,
            Forgot {
                forgotten: 2,
                missing: 1,
                away: 1,
                on_disk: 0
            }
        );
        assert_eq!(store.missing, Some((id.clone(), 1, 0)));
        assert_eq!(hashes(&store.list[0]), [&ha, &hc, &hd]);
        // The file on disk says so.
        let file = read_collection(&dir.join(DIR).join(format!("{id}.json"))).unwrap();
        let on_disk: Vec<&str> = file.refs.iter().map(|r| r.hash.as_str()).collect();
        assert_eq!(on_disk, [&ha, &hc, &hd]);
        // The status line's count drops by them; the offline one stays.
        let after = resolve(&lib, &store.list[0].refs, away, &[], false).unwrap();
        assert_eq!(after.missing, 1);
        assert!(after.gone.is_empty());
        let forgot = store
            .forget_missing(&id, Some(&lib), away, &[], &look)
            .unwrap();
        assert_eq!((forgot.forgotten, forgot.away), (0, 1));
        // The drive back: d is gone by the same test, and goes.
        let forgot = store
            .forget_missing(&id, Some(&lib), &[], &[], &look)
            .unwrap();
        assert_eq!((forgot.forgotten, forgot.missing), (1, 0));
        assert_eq!(hashes(&store.list[0]), [&ha, &hc]);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A reference whose hint is a file on disk the index has not read
    /// (a pass still to reach it, an index rebuilt) counts missing and
    /// is never forgotten; one under a network mount is kept without
    /// its folder being looked at, and one in a folder that does not
    /// answer is kept.
    #[test]
    fn forget_missing_keeps_a_file_on_disk_and_never_looks_at_a_far_one() {
        let dir = scratch("forget-on-disk");
        let a = dir.join("a.tif");
        write_frame(&a, &R5, 1);
        let (nas, quiet) = (dir.join("nas"), dir.join("quiet"));
        // An index that has read nothing: as after a rebuild.
        let lib = Library::open_in_memory().unwrap();
        let mut c = Collection::new("Unread");
        c.add(&[
            ("0123456789abcdef".into(), a.clone()),
            ("feedface".into(), dir.join("gone.tif")),
            ("cafebabe".into(), nas.join("b.tif")),
            ("deadbeef".into(), quiet.join("c.tif")),
        ]);
        let id = c.id.clone();
        let mut store = Store::default();
        store.list.push(c);
        let shown = resolve(&lib, &store.list[0].refs, &[], &[], false).unwrap();
        assert_eq!((shown.missing, shown.gone.len()), (4, 4));
        let looked = std::cell::RefCell::new(Vec::new());
        let look = |dirs: &[PathBuf]| {
            looked.borrow_mut().extend(dirs.iter().cloned());
            dirs.iter()
                .map(|d| (d.clone(), (!d.starts_with(&quiet)).then(|| d.is_dir())))
                .collect()
        };
        let far = std::slice::from_ref(&nas);
        let forgot = store
            .forget_missing(&id, Some(&lib), &[], far, &look)
            .unwrap();
        assert_eq!(
            forgot,
            Forgot {
                forgotten: 1,
                missing: 3,
                away: 2,
                on_disk: 1
            }
        );
        assert_eq!(
            hashes(&store.list[0]),
            ["0123456789abcdef", "cafebabe", "deadbeef"]
        );
        assert!(
            !looked.borrow().iter().any(|d| d.starts_with(&nas)),
            "the network mount is never looked at: {:?}",
            looked.borrow()
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// A folder that cannot be read answers the look with no, and its
    /// files' stat fails: not "not found", so they are kept. As after a
    /// rebuilt index, whose pass left the folder with no rows.
    #[cfg(unix)]
    #[test]
    fn forget_missing_keeps_the_frames_of_a_folder_that_cannot_be_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("forget-locked");
        let locked = dir.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        let a = locked.join("a.tif");
        write_frame(&a, &R5, 1);
        // The mode put back however the test ends: a folder of mode 000
        // left under target/ stops a `cargo clean`.
        struct Restore<'a>(&'a Path);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
            }
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let restore = Restore(&locked);
        // Run as root, the mode stops nothing, and there is nothing to test.
        let readable = std::fs::read_dir(&locked).is_ok();
        let lib = Library::open_in_memory().unwrap();
        let mut c = Collection::new("Locked");
        c.add(&[
            ("0123456789abcdef".into(), a.clone()),
            ("feedface".into(), dir.join("gone").join("b.tif")),
        ]);
        let id = c.id.clone();
        let mut store = Store::default();
        store.list.push(c);
        let forgot = store
            .forget_missing(&id, Some(&lib), &[], &[], &answering)
            .unwrap();
        drop(restore);
        if !readable {
            assert_eq!(
                forgot,
                Forgot {
                    forgotten: 1,
                    missing: 1,
                    away: 1,
                    on_disk: 0
                }
            );
            assert_eq!(hashes(&store.list[0]), ["0123456789abcdef"]);
        }
        crate::testing::remove_dir_retry(&dir);
    }
}
