//! Roots: the folders the user has added to the library (notes §72),
//! and the watcher that keeps the index up with them while the
//! editor runs.
//!
//! The list is truth, not cache: the index can be deleted and rebuilt
//! from the files, but not from nothing, and what it is rebuilt from
//! is this list. So it lives in a file of its own beside the index,
//! `roots.json` in the same folder as `library.sqlite`, rather than in
//! a table the index's rebuild would drop. Beside the index and not
//! in the editor's settings because it describes the library, not
//! the window: a library opened with `--library` elsewhere has its
//! own roots beside it, a test's library has its own in its own
//! directory, and the collections §72 puts in "one small file of
//! their own under the data directory" will sit beside it for the
//! same reason.
//!
//! A root is a folder, canonical, with no other root above or below
//! it: adding a folder under a root is nothing to do, and adding one
//! above roots takes their place. So a file is under one root at
//! most, and a pass over every root never walks a folder twice.
//!
//! The watcher is `notify`'s: inotify on Linux, FSEvents on macOS,
//! ReadDirectoryChangesW on Windows. Its events are turned into
//! [`Change`]s — a folder whose files changed, or a folder that
//! appeared or went, to be walked — and handed on in batches once
//! the disk has been quiet for a moment. A root with a folder under
//! it that cannot be read is watched folder by folder, around it. A
//! watcher that cannot start, or a root it cannot watch at all (too
//! many inotify watches, a network mount), is said and left: the
//! launch pass still brings the index up to date, only not while the
//! editor runs.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::canonical;

/// The roots file's name, beside the library's.
pub const FILE_NAME: &str = "roots.json";

/// The version the file is written at.
const VERSION: u64 = 1;

/// The folders that make up the library.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roots {
    list: Vec<PathBuf>,
}

/// What a roots file said.
enum Parsed {
    Roots(Roots),
    /// A version this build does not know.
    Newer(u64),
    /// Not a roots file.
    Not,
}

/// What adding a folder did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Added {
    /// The folder is a root now.
    New(PathBuf),
    /// The folder was a root already, or is under this one.
    Covered(PathBuf),
    /// The folder is a root now, and these roots under it are not
    /// roots of their own any more.
    Absorbed(PathBuf, Vec<PathBuf>),
}

/// Why a folder could not be added.
#[derive(Debug, thiserror::Error)]
pub enum RootError {
    #[error("{0}: not a folder")]
    NotAFolder(PathBuf),
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    /// The roots file is JSON, and JSON holds text: a path that is
    /// not valid Unicode cannot be written into it.
    #[error("{0}: the name is not valid Unicode, and the roots file cannot hold it")]
    NotUnicode(PathBuf),
}

impl Roots {
    /// Where the roots file goes for a library at `library`: beside
    /// it, in the same folder.
    pub fn path_beside(library: &Path) -> PathBuf {
        library
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .join(FILE_NAME)
    }

    /// The roots in the file at `path`: none when it is not there.
    /// A file that will not parse is said and set aside as
    /// `roots.json.unreadable`, so the next save does not write an
    /// empty list over something a person might want back. A file
    /// that cannot be read (a permission, a disk error), or one a
    /// later build wrote, is an error: the caller then keeps no file
    /// to save to, rather than write an empty or older list over it.
    pub fn load(path: &Path) -> std::io::Result<Roots> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Roots::default()),
            Err(e) => return Err(e),
        };
        match Self::parse(&bytes) {
            Parsed::Roots(roots) => Ok(roots),
            Parsed::Newer(v) => Err(std::io::Error::other(format!(
                "{}: written by a later greycard (version {v}); left alone",
                path.display()
            ))),
            Parsed::Not => {
                let aside = path.with_extension("json.unreadable");
                log::warn!(
                    "{}: not a roots file; set aside as {}",
                    path.display(),
                    aside.display()
                );
                let _ = std::fs::rename(path, &aside);
                Ok(Roots::default())
            }
        }
    }

    fn parse(bytes: &[u8]) -> Parsed {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
            return Parsed::Not;
        };
        if let Some(v) = value.get("version").and_then(|v| v.as_u64())
            && v > VERSION
        {
            return Parsed::Newer(v);
        }
        let Some(list) = value.get("roots").and_then(|l| l.as_array()) else {
            return Parsed::Not;
        };
        let mut roots = Roots::default();
        for item in list {
            let Some(text) = item.as_str() else {
                return Parsed::Not;
            };
            let path = PathBuf::from(text);
            if !roots.list.contains(&path) {
                roots.list.push(path);
            }
        }
        Parsed::Roots(roots)
    }

    /// Write the list to `path`, through a file beside it renamed
    /// over it, so a crash mid-write never leaves half a list. The
    /// part file carries the process's id, so two editors saving at
    /// once never write the same one.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let list: Vec<serde_json::Value> = self
            .list
            .iter()
            .map(|p| serde_json::Value::String(p.to_string_lossy().into_owned()))
            .collect();
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "version": VERSION,
            "roots": list,
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

    /// Change the list kept at `path` as it is on disk now, not as
    /// this process last read it, and save it: another editor may have
    /// added or removed a root since. `change` is applied to the list
    /// read afresh; what it returns comes back beside the list saved.
    pub fn edit<T>(
        path: &Path,
        change: impl FnOnce(&mut Roots) -> T,
    ) -> std::io::Result<(Roots, T)> {
        let mut roots = Self::load(path)?;
        let out = change(&mut roots);
        roots.save(path)?;
        Ok((roots, out))
    }

    /// The roots, in the order they were added.
    pub fn list(&self) -> &[PathBuf] {
        &self.list
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// A list of these folders as they are, for a run that names its
    /// roots on the command line. Nothing is checked or made
    /// canonical here: [`Roots::add`] is for that.
    pub fn from_list(list: Vec<PathBuf>) -> Roots {
        let mut roots = Roots::default();
        for p in list {
            if !roots.list.contains(&p) {
                roots.list.push(p);
            }
        }
        roots
    }

    /// Add a folder, canonical. A folder already covered by a root
    /// changes nothing; a folder above roots takes their place.
    pub fn add(&mut self, dir: &Path) -> Result<Added, RootError> {
        let dir = canonical(dir).map_err(|e| RootError::Io(dir.to_path_buf(), e))?;
        if !dir.is_dir() {
            return Err(RootError::NotAFolder(dir));
        }
        if dir.to_str().is_none() {
            return Err(RootError::NotUnicode(dir));
        }
        if let Some(root) = self.root_of(&dir) {
            return Ok(Added::Covered(root.to_path_buf()));
        }
        let under: Vec<PathBuf> = self
            .list
            .iter()
            .filter(|r| r.starts_with(&dir))
            .cloned()
            .collect();
        self.list.retain(|r| !r.starts_with(&dir));
        self.list.push(dir.clone());
        Ok(if under.is_empty() {
            Added::New(dir)
        } else {
            Added::Absorbed(dir, under)
        })
    }

    /// Forget a root. The folder and its files are not touched, nor
    /// are their rows in the index, which is a cache and costs
    /// nothing to keep; the all-roots view simply stops listing them.
    /// True when it was a root.
    pub fn remove(&mut self, dir: &Path) -> bool {
        let canonical = canonical(dir).unwrap_or_else(|_| dir.to_path_buf());
        let before = self.list.len();
        self.list.retain(|r| r != dir && *r != canonical);
        self.list.len() != before
    }

    /// The root a path is under, the root itself included. The path
    /// is compared as it is: canonical, for an answer that means
    /// anything.
    pub fn root_of(&self, path: &Path) -> Option<&Path> {
        self.list
            .iter()
            .find(|r| path.starts_with(r))
            .map(PathBuf::as_path)
    }
}

/// What the index is to do about something that changed on disk.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Change {
    /// A file in this folder was added, changed, renamed or removed:
    /// a pass over the folder.
    Folder(PathBuf),
    /// This folder appeared, or went, with whatever was under it: a
    /// pass over the tree, which for a folder gone marks its rows
    /// missing.
    Tree(PathBuf),
}

impl Change {
    pub fn path(&self) -> &Path {
        match self {
            Change::Folder(p) | Change::Tree(p) => p,
        }
    }
}

/// What a path the watcher named means for the index, under these
/// roots: nothing when it is outside them, under a hidden folder
/// (the sidecars' own `.greycard`, whose writes the editor indexes
/// itself), a hidden file (a copier's temporary, `.DS_Store`), or a
/// file the index does not hold (a sidecar beside its frame, an XMP,
/// a half-copied `.part`). A raw or a picture is its folder's pass;
/// a folder, one there or one gone, is a pass over its tree.
pub fn change_for(roots: &[PathBuf], path: &Path) -> Option<Change> {
    let root = roots.iter().find(|r| path.starts_with(r))?;
    let rel = path.strip_prefix(root).ok()?;
    let hidden = rel.components().any(|c| match c {
        Component::Normal(name) => name.to_string_lossy().starts_with('.'),
        _ => false,
    });
    if hidden {
        return None;
    }
    // A root that is not there (a drive unplugged, its mount point
    // taken away with it) says nothing: a pass over it would mark the
    // whole shoot missing, which is the exclamation mark §72 set out
    // not to build.
    if !root.is_dir() {
        return None;
    }
    if crate::is_indexed_path(path) {
        let parent = path.parent()?;
        return Some(Change::Folder(parent.to_path_buf()));
    }
    if path.is_dir() {
        return Some(Change::Tree(path.to_path_buf()));
    }
    if path.exists() {
        return None;
    }
    // Gone, and not a file the index holds: a folder, as likely as
    // not, and one named like `2026.09.24` has what looks like an
    // extension. A pass over a path the index has no rows under is
    // refused at once, so a gone sidecar or temporary costs nothing.
    // The highest folder gone under the root is walked, since a tree
    // pass over a folder whose parent is gone too is refused.
    let mut gone = path.to_path_buf();
    while let Some(parent) = gone.parent() {
        if parent == root.as_path() || parent.exists() {
            break;
        }
        gone = parent.to_path_buf();
    }
    Some(Change::Tree(gone))
}

/// A batch of changes with the repeats out and every folder under a
/// tree in the batch folded into it, in the order first met.
pub fn settle(changes: Vec<Change>) -> Vec<Change> {
    let trees: Vec<PathBuf> = changes
        .iter()
        .filter_map(|c| match c {
            Change::Tree(p) => Some(p.clone()),
            Change::Folder(_) => None,
        })
        .collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for change in changes {
        let covered = match &change {
            Change::Folder(p) => trees.iter().any(|t| p.starts_with(t)),
            Change::Tree(p) => trees.iter().any(|t| t != p && p.starts_with(t)),
        };
        if !covered && seen.insert(change.clone()) {
            out.push(change);
        }
    }
    out
}

/// The roots watched, while this is held. Dropping it stops the
/// watching and, once the last events are handed on, its thread.
pub struct Watcher {
    /// Shared with the watcher's thread, which holds it weakly: a
    /// root watched folder by folder has each new folder added as it
    /// appears. Dropping this drops the platform's watcher, which ends
    /// the thread.
    _watcher: Arc<Mutex<notify::RecommendedWatcher>>,
    /// The roots it watches, whole or in part: every root asked for
    /// less the ones it could not watch at all.
    pub watched: Vec<PathBuf>,
}

/// How long the disk has to be quiet before a batch is handed on,
/// and the longest a batch waits for that. A copy of a shoot is a
/// stream of events for seconds; the quiet lets it land first, and
/// the cap keeps a slow copy from hiding its first files for as long
/// as it runs.
pub const QUIET: Duration = Duration::from_millis(400);
pub const LONGEST: Duration = Duration::from_secs(5);

/// The folders under `dir` a watch can take, `dir` among them: not
/// hidden, not a link, and readable. Those that cannot be read come
/// back beside, with the reason.
#[allow(clippy::type_complexity)]
fn folders_under(dir: &Path) -> (Vec<PathBuf>, Vec<(PathBuf, String)>) {
    let mut found = Vec::new();
    let mut unreadable = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        match std::fs::read_dir(&d) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let hidden = entry.file_name().to_string_lossy().starts_with('.');
                    if !hidden && entry.file_type().is_ok_and(|t| t.is_dir()) {
                        stack.push(entry.path());
                    }
                }
                found.push(d);
            }
            Err(e) => unreadable.push((d, e.to_string())),
        }
    }
    (found, unreadable)
}

impl Watcher {
    /// Watch `roots`, recursively, and hand each batch of changes to
    /// `changed` on a thread of the watcher's own. A root the platform
    /// will not watch whole (a folder under it that cannot be read,
    /// a drive's `lost+found`) is watched folder by folder instead,
    /// every folder that can be, and the ones that cannot come back
    /// beside it, with the reason, as does a root not watched at all.
    /// All the roots come back, and no watcher, when the platform's
    /// watcher would not start.
    #[allow(clippy::type_complexity)]
    pub fn start(
        roots: &[PathBuf],
        quiet: Duration,
        longest: Duration,
        changed: impl Fn(Vec<Change>) + Send + 'static,
    ) -> (Option<Watcher>, Vec<(PathBuf, String)>) {
        use notify::Watcher as _;
        let (send, events) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = match notify::recommended_watcher(send) {
            Ok(w) => w,
            Err(e) => {
                let why = e.to_string();
                return (
                    None,
                    roots.iter().map(|r| (r.clone(), why.clone())).collect(),
                );
            }
        };
        let mut watched = Vec::new();
        let mut by_folder = Vec::new();
        let mut failed = Vec::new();
        for root in roots {
            if watcher
                .watch(root, notify::RecursiveMode::Recursive)
                .is_ok()
            {
                watched.push(root.clone());
                continue;
            }
            // Taken back, whatever part of it the recursive watch
            // managed before it failed, and done a folder at a time.
            let _ = watcher.unwatch(root);
            let (folders, unreadable) = folders_under(root);
            let mut any = false;
            for folder in &folders {
                match watcher.watch(folder, notify::RecursiveMode::NonRecursive) {
                    Ok(()) => any = true,
                    Err(e) => failed.push((folder.clone(), e.to_string())),
                }
            }
            failed.extend(unreadable);
            if any {
                watched.push(root.clone());
                by_folder.push(root.clone());
            }
        }
        let shared = Arc::new(Mutex::new(watcher));
        let weak = Arc::downgrade(&shared);
        let roots_seen = watched.clone();
        let spawned = std::thread::Builder::new()
            .name("greycard watch".into())
            .spawn(move || {
                let add = |dir: &Path| {
                    // A folder new under a root watched folder by
                    // folder: it and whatever came with it.
                    if !by_folder.iter().any(|r| dir.starts_with(r)) {
                        return;
                    }
                    let Some(w) = weak.upgrade() else {
                        return;
                    };
                    let Ok(mut w) = w.lock() else {
                        return;
                    };
                    for folder in folders_under(dir).0 {
                        let _ = w.watch(&folder, notify::RecursiveMode::NonRecursive);
                    }
                };
                debounce(&roots_seen, &events, quiet, longest, &add, &changed)
            });
        if let Err(e) = spawned {
            let why = e.to_string();
            return (
                None,
                roots.iter().map(|r| (r.clone(), why.clone())).collect(),
            );
        }
        (
            Some(Watcher {
                _watcher: shared,
                watched,
            }),
            failed,
        )
    }
}

/// Whether an event says something changed. Opening a file and
/// reading it is not a change — the index's own pass and the
/// thumbnails open every file under a root, and taking those for
/// changes would have the watcher chase its own tail — and neither
/// is a closing that wrote nothing.
fn is_change(kind: &notify::EventKind) -> bool {
    use notify::EventKind;
    use notify::event::{AccessKind, AccessMode, MetadataKind, ModifyKind};
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)) => false,
        _ => true,
    }
}

/// The watcher's thread: changes gathered into a batch until the
/// disk has been quiet for `quiet`, or `longest` has passed since the
/// first, then handed on. Only an event that is a change counts
/// towards the quiet: the reads of a pass or of the thumbnails under
/// a root being worked on do not hold a batch back. `add` is told of
/// each folder that appears. It ends when the watcher is dropped.
fn debounce(
    roots: &[PathBuf],
    events: &mpsc::Receiver<notify::Result<notify::Event>>,
    quiet: Duration,
    longest: Duration,
    add: &dyn Fn(&Path),
    changed: &dyn Fn(Vec<Change>),
) {
    let mut batch: Vec<Change> = Vec::new();
    // When the batch began, and when its last change came.
    let mut first = Instant::now();
    let mut last = first;
    loop {
        let next = if batch.is_empty() {
            events
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        } else {
            let due = (last + quiet).min(first + longest);
            match due.checked_duration_since(Instant::now()) {
                Some(left) if !left.is_zero() => events.recv_timeout(left),
                _ => Err(mpsc::RecvTimeoutError::Timeout),
            }
        };
        match next {
            Ok(Ok(event)) => {
                let before = batch.len();
                if event.need_rescan() {
                    // The platform dropped events (inotify's queue
                    // overflowed): every root walked again.
                    batch.extend(roots.iter().cloned().map(Change::Tree));
                } else if is_change(&event.kind) {
                    for change in event.paths.iter().filter_map(|p| change_for(roots, p)) {
                        if let Change::Tree(dir) = &change
                            && dir.is_dir()
                        {
                            add(dir);
                        }
                        batch.push(change);
                    }
                }
                if batch.len() > before {
                    let now = Instant::now();
                    if before == 0 {
                        first = now;
                    }
                    last = now;
                }
            }
            Ok(Err(e)) => log::warn!("watching the roots: {e}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                changed(settle(std::mem::take(&mut batch)));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if !batch.is_empty() {
                    changed(settle(std::mem::take(&mut batch)));
                }
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::tests::scratch;

    #[test]
    fn the_roots_file_goes_beside_the_library() {
        assert_eq!(
            Roots::path_beside(Path::new("/data/greycard/library.sqlite")),
            PathBuf::from("/data/greycard/roots.json")
        );
        assert_eq!(
            Roots::path_beside(Path::new("library.sqlite")),
            PathBuf::from("./roots.json")
        );
    }

    #[test]
    fn roots_are_added_canonical_and_never_nested() {
        let dir = scratch("roots-add");
        let (a, b, ab) = (dir.join("a"), dir.join("b"), dir.join("a").join("b"));
        std::fs::create_dir_all(&ab).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let mut roots = Roots::default();
        // Through a `..`, which canonical takes out.
        let through = dir.join("b").join("..").join("a").join("b");
        assert_eq!(roots.add(&through).unwrap(), Added::New(ab.clone()));
        assert_eq!(roots.add(&b).unwrap(), Added::New(b.clone()));
        // Above a root: it takes the root's place.
        assert_eq!(
            roots.add(&a).unwrap(),
            Added::Absorbed(a.clone(), vec![ab.clone()])
        );
        assert_eq!(roots.list(), [b.clone(), a.clone()]);
        // Under a root, or the root again: nothing to do.
        assert_eq!(roots.add(&ab).unwrap(), Added::Covered(a.clone()));
        assert_eq!(roots.add(&a).unwrap(), Added::Covered(a.clone()));
        assert_eq!(roots.root_of(&ab.join("x.CR3")), Some(a.as_path()));
        assert_eq!(roots.root_of(&dir.join("c")), None);
        // A file, and a folder that is not there, are not roots.
        std::fs::write(dir.join("f.txt"), b"x").unwrap();
        assert!(matches!(
            roots.add(&dir.join("f.txt")),
            Err(RootError::NotAFolder(_))
        ));
        assert!(matches!(
            roots.add(&dir.join("nowhere")),
            Err(RootError::Io(..))
        ));
        // Removed by the path given or its canonical spelling; the
        // folder stays where it is.
        assert!(roots.remove(&dir.join("b").join("..").join("a")));
        assert!(!roots.remove(&a));
        assert_eq!(roots.list(), std::slice::from_ref(&b));
        assert!(a.is_dir());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_list_survives_a_round_trip_and_a_bad_file_is_set_aside() {
        let dir = scratch("roots-file");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let path = Roots::path_beside(&dir.join("library.sqlite"));
        assert_eq!(
            Roots::load(&path).unwrap(),
            Roots::default(),
            "no file is no roots"
        );
        let mut roots = Roots::default();
        roots.add(&a).unwrap();
        roots.add(&b).unwrap();
        roots.save(&path).unwrap();
        assert_eq!(Roots::load(&path).unwrap(), roots);
        let parts: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
            .collect();
        assert!(parts.is_empty(), "{parts:?}");
        // The library's own file can go, and the roots stay.
        std::fs::write(dir.join("library.sqlite"), b"").unwrap();
        std::fs::remove_file(dir.join("library.sqlite")).unwrap();
        assert_eq!(Roots::load(&path).unwrap().list(), [a.clone(), b.clone()]);

        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(Roots::load(&path).unwrap(), Roots::default());
        assert!(!path.exists());
        assert_eq!(
            std::fs::read(path.with_extension("json.unreadable")).unwrap(),
            b"{ not json"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_path_under_a_root_is_its_folder_or_its_tree() {
        let dir = scratch("roots-change");
        let root = dir.join("root");
        let shoot = root.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        let roots = vec![root.clone()];
        // A raw, there or gone, is its folder's.
        assert_eq!(
            change_for(&roots, &shoot.join("a.CR3")),
            Some(Change::Folder(shoot.clone()))
        );
        assert_eq!(
            change_for(&roots, &root.join("b.jpg")),
            Some(Change::Folder(root.clone()))
        );
        // A folder that is there is walked.
        assert_eq!(
            change_for(&roots, &shoot),
            Some(Change::Tree(shoot.clone()))
        );
        // A folder gone is walked from the highest gone under the
        // root.
        assert_eq!(
            change_for(&roots, &root.join("gone").join("deeper")),
            Some(Change::Tree(root.join("gone")))
        );
        // A folder gone whose name has a dot in it is a folder gone,
        // not a file (the review's `2026.09.24`, renamed away).
        assert_eq!(
            change_for(&roots, &root.join("2026.09.24")),
            Some(Change::Tree(root.join("2026.09.24")))
        );
        // The sidecars' hidden folder, a hidden temporary, a sidecar
        // beside its frame, an XMP, a part-copied file, and anything
        // outside the roots, are nobody's business.
        std::fs::create_dir_all(shoot.join(".greycard")).unwrap();
        for file in ["a.CR3.gcd", "a.xmp", "a.CR3.part"] {
            std::fs::write(shoot.join(file), b"x").unwrap();
        }
        for nothing in [
            shoot.join(".greycard").join("a.CR3.gcd"),
            shoot.join(".greycard"),
            shoot.join(".a.CR3.Xyz12"),
            shoot.join("a.CR3.gcd"),
            shoot.join("a.xmp"),
            shoot.join("a.CR3.part"),
            dir.join("elsewhere").join("a.CR3"),
        ] {
            assert_eq!(change_for(&roots, &nothing), None, "{}", nothing.display());
        }
        // A root that is not there (a drive unplugged, its mount point
        // gone with it) says nothing about itself or anything under it.
        let offline = vec![dir.join("unplugged")];
        for nothing in [
            dir.join("unplugged"),
            dir.join("unplugged").join("a.CR3"),
            dir.join("unplugged").join("shoot"),
        ] {
            assert_eq!(
                change_for(&offline, &nothing),
                None,
                "{}",
                nothing.display()
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_batch_folds_folders_into_the_trees_over_them() {
        let (r, s, t) = (
            PathBuf::from("/r"),
            PathBuf::from("/r/s"),
            PathBuf::from("/r/t"),
        );
        let settled = settle(vec![
            Change::Folder(s.clone()),
            Change::Folder(r.clone()),
            Change::Tree(t.clone()),
            Change::Folder(t.join("u")),
            Change::Folder(s.clone()),
            Change::Tree(t.join("u")),
            Change::Folder(t.clone()),
        ]);
        assert_eq!(
            settled,
            vec![Change::Folder(s), Change::Folder(r), Change::Tree(t)]
        );
    }

    /// The watcher itself, on this platform's backend: a file copied
    /// into a root is its folder's change, a new folder is a tree,
    /// and the sidecars' hidden folder says nothing.
    #[test]
    fn the_watcher_hands_on_what_changed_under_a_root() {
        let dir = scratch("roots-watch");
        let root = dir.join("root");
        std::fs::create_dir_all(root.join("shoot")).unwrap();
        let (send, got) = mpsc::channel();
        let (watcher, failed) = Watcher::start(
            std::slice::from_ref(&root),
            Duration::from_millis(100),
            Duration::from_secs(2),
            move |changes| {
                let _ = send.send(changes);
            },
        );
        assert!(failed.is_empty(), "{failed:?}");
        let watcher = watcher.expect("a watcher");
        assert_eq!(watcher.watched, std::slice::from_ref(&root));
        let wait = |want: &dyn Fn(&[Change]) -> bool| {
            let until = Instant::now() + Duration::from_secs(10);
            let mut all = Vec::new();
            while Instant::now() < until {
                if let Ok(batch) = got.recv_timeout(Duration::from_millis(100)) {
                    all.extend(batch);
                    if want(&all) {
                        return all;
                    }
                }
            }
            panic!("never saw it: {all:?}");
        };
        // A change that reaches a folder: its own pass, or a tree pass
        // over it or a folder above it. Windows says a folder changed
        // when an entry is made in it, and FSEvents can name the root
        // itself; each is a tree to the classifier, and the batch folds
        // the file's folder into it.
        let reaches = |all: &[Change], dir: &Path| {
            all.iter().any(|c| match c {
                Change::Folder(p) => p == dir,
                Change::Tree(t) => dir.starts_with(t),
            })
        };
        let tree_over = |all: &[Change], dir: &Path| {
            all.iter()
                .any(|c| matches!(c, Change::Tree(t) if dir.starts_with(t)))
        };
        // The hidden folder first: if it said anything it would come
        // in the same batch as the raw below.
        std::fs::create_dir_all(root.join("shoot").join(".greycard")).unwrap();
        std::fs::write(
            root.join("shoot").join(".greycard").join("a.CR3.gcd"),
            b"{}",
        )
        .unwrap();
        std::fs::write(root.join("shoot").join("a.CR3"), b"not a raw").unwrap();
        let all = wait(&|c| reaches(c, &root.join("shoot")));
        // Nothing under the hidden folder. A platform may say the
        // folders the writes touched as well, which is a tree pass
        // over them and harmless.
        assert!(
            all.iter().all(|c| c.path().starts_with(&root)
                && !c.path().components().any(|p| p.as_os_str() == ".greycard")),
            "{all:?}"
        );
        std::fs::create_dir_all(root.join("new")).unwrap();
        wait(&|c| tree_over(c, &root.join("new")));
        drop(watcher);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_root_that_is_not_there_is_said_and_the_others_watched() {
        let dir = scratch("roots-watch-gone");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let (watcher, failed) =
            Watcher::start(&[dir.join("gone"), root.clone()], QUIET, LONGEST, |_| {});
        assert_eq!(watcher.expect("a watcher").watched, [root]);
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].0, dir.join("gone"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Two editors on one roots file: each edit reads the file as it
    /// is now, so neither loses the other's root; a file a later build
    /// wrote, or one that cannot be read, is an error and is left
    /// alone rather than written over.
    #[test]
    fn two_editors_keep_each_others_roots() {
        let dir = scratch("roots-two");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let path = dir.join("roots.json");
        // Both started with the file empty; each adds its own.
        let (first, _) = Roots::edit(&path, |r| r.add(&a).unwrap()).unwrap();
        let (second, _) = Roots::edit(&path, |r| r.add(&b).unwrap()).unwrap();
        assert_eq!(first.list(), std::slice::from_ref(&a));
        assert_eq!(second.list(), [a.clone(), b.clone()]);
        assert_eq!(Roots::load(&path).unwrap(), second);
        // And a removal by one keeps what the other added meanwhile.
        let (_, removed) = Roots::edit(&path, |r| r.remove(&a)).unwrap();
        assert!(removed);
        assert_eq!(Roots::load(&path).unwrap().list(), std::slice::from_ref(&b));

        let newer = br#"{"version": 9, "roots": ["/x"]}"#;
        std::fs::write(&path, newer).unwrap();
        assert!(Roots::load(&path).is_err());
        assert!(Roots::edit(&path, |r| r.add(&a).map(|_| ())).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), newer, "left alone");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(&path, br#"{"version": 1, "roots": []}"#).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
            let read = Roots::load(&path);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            // Root reads it anyway; everyone else hears the error.
            if read.is_ok() {
                std::fs::remove_dir_all(&dir).unwrap();
                return;
            }
            assert!(read.is_err());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A root with a folder under it that cannot be read (a drive's
    /// `lost+found`) is still watched, every folder that can be, and
    /// the one that cannot is said; a folder made later under it is
    /// watched too.
    #[cfg(unix)]
    #[test]
    fn a_root_with_a_locked_folder_is_watched_around_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("roots-watch-locked");
        let root = dir.join("root");
        let (locked, shoot) = (root.join("lost+found"), root.join("shoot"));
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::create_dir_all(&shoot).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&locked).is_ok() {
            // Run as root, which reads it anyway.
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::remove_dir_all(&dir).unwrap();
            return;
        }
        let (send, got) = mpsc::channel();
        let (watcher, failed) = Watcher::start(
            std::slice::from_ref(&root),
            Duration::from_millis(100),
            Duration::from_secs(2),
            move |changes| {
                let _ = send.send(changes);
            },
        );
        let watcher = watcher.expect("a watcher");
        assert_eq!(watcher.watched, std::slice::from_ref(&root));
        assert!(
            failed.iter().all(|(p, _)| *p == locked),
            "only the locked folder is said: {failed:?}"
        );
        let wait = |want: &Change| {
            let until = Instant::now() + Duration::from_secs(10);
            while Instant::now() < until {
                if let Ok(batch) = got.recv_timeout(Duration::from_millis(100))
                    && batch.contains(want)
                {
                    return;
                }
            }
            panic!("never saw {want:?}");
        };
        std::fs::write(shoot.join("a.CR3"), b"not a raw").unwrap();
        wait(&Change::Folder(shoot.clone()));
        let later = root.join("later");
        std::fs::create_dir_all(&later).unwrap();
        wait(&Change::Tree(later.clone()));
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(later.join("b.CR3"), b"not a raw").unwrap();
        wait(&Change::Folder(later));
        drop(watcher);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Reads under a root are not changes, and do not hold a batch
    /// back: a change made while the root is being read all the time
    /// comes after the quiet, not after the cap.
    #[test]
    fn reads_under_a_root_do_not_hold_a_batch_back() {
        let dir = scratch("roots-watch-reads");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("read.CR3"), b"x").unwrap();
        let (send, got) = mpsc::channel();
        let (watcher, _) = Watcher::start(
            std::slice::from_ref(&root),
            Duration::from_millis(150),
            Duration::from_secs(8),
            move |changes| {
                let _ = send.send((Instant::now(), changes));
            },
        );
        let _watcher = watcher.expect("a watcher");
        let reading = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let r = reading.clone();
        let file = root.join("read.CR3");
        let reader = std::thread::spawn(move || {
            while r.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = std::fs::read(&file);
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        std::thread::sleep(Duration::from_millis(200));
        let wrote = Instant::now();
        std::fs::write(root.join("new.CR3"), b"x").unwrap();
        let (at, _) = got
            .recv_timeout(Duration::from_secs(15))
            .expect("the batch");
        reading.store(false, std::sync::atomic::Ordering::SeqCst);
        reader.join().unwrap();
        assert!(
            at.duration_since(wrote) < Duration::from_secs(4),
            "handed on {:?} after the write",
            at.duration_since(wrote)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
