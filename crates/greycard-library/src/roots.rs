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
//! the disk has been quiet for a moment. A watcher that cannot
//! start, or a root it cannot watch (too many inotify watches, a
//! network mount), is said and left: the launch pass still brings
//! the index up to date, only not while the editor runs.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;
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
    /// empty list over something a person might want back.
    pub fn load(path: &Path) -> Roots {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Roots::default(),
            Err(e) => {
                log::warn!("{}: {e}; no roots", path.display());
                return Roots::default();
            }
        };
        match Self::parse(&bytes) {
            Some(roots) => roots,
            None => {
                let aside = path.with_extension("json.unreadable");
                log::warn!(
                    "{}: not a roots file; set aside as {}",
                    path.display(),
                    aside.display()
                );
                let _ = std::fs::rename(path, &aside);
                Roots::default()
            }
        }
    }

    fn parse(bytes: &[u8]) -> Option<Roots> {
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        let list = value.get("roots")?.as_array()?;
        let mut roots = Roots::default();
        for item in list {
            let path = PathBuf::from(item.as_str()?);
            if !roots.list.contains(&path) {
                roots.list.push(path);
            }
        }
        Some(roots)
    }

    /// Write the list to `path`, through a file beside it renamed
    /// over it, so a crash mid-write never leaves half a list.
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
        let part = path.with_extension("json.part");
        std::fs::write(&part, text)?;
        std::fs::rename(&part, path)
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
    if crate::is_indexed_path(path) {
        let parent = path.parent()?;
        return Some(Change::Folder(parent.to_path_buf()));
    }
    if path.is_dir() {
        return Some(Change::Tree(path.to_path_buf()));
    }
    if path.exists() || path.extension().is_some() {
        return None;
    }
    // Gone, and with no extension: a folder, as likely as not. The
    // highest folder gone under the root is walked, since a tree pass
    // over a folder whose parent is gone too is refused.
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
    _watcher: notify::RecommendedWatcher,
    /// The roots it watches: every root asked for less the ones it
    /// could not.
    pub watched: Vec<PathBuf>,
}

/// How long the disk has to be quiet before a batch is handed on,
/// and the longest a batch waits for that. A copy of a shoot is a
/// stream of events for seconds; the quiet lets it land first, and
/// the cap keeps a slow copy from hiding its first files for as long
/// as it runs.
pub const QUIET: Duration = Duration::from_millis(400);
pub const LONGEST: Duration = Duration::from_secs(5);

impl Watcher {
    /// Watch `roots`, recursively, and hand each batch of changes to
    /// `changed` on a thread of the watcher's own. What could not be
    /// watched comes back beside it, by root, with the reason: all of
    /// them, and no watcher, when the platform's watcher would not
    /// start at all.
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
        let mut failed = Vec::new();
        for root in roots {
            match watcher.watch(root, notify::RecursiveMode::Recursive) {
                Ok(()) => watched.push(root.clone()),
                Err(e) => {
                    // A recursive watch that ran out of inotify
                    // watches halfway leaves the half it managed:
                    // taken back, so the root is watched whole or not
                    // at all.
                    let _ = watcher.unwatch(root);
                    failed.push((root.clone(), e.to_string()));
                }
            }
        }
        let roots_seen = watched.clone();
        let spawned = std::thread::Builder::new()
            .name("greycard watch".into())
            .spawn(move || debounce(&roots_seen, &events, quiet, longest, &changed));
        if let Err(e) = spawned {
            let why = e.to_string();
            return (
                None,
                roots.iter().map(|r| (r.clone(), why.clone())).collect(),
            );
        }
        (
            Some(Watcher {
                _watcher: watcher,
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

/// The watcher's thread: events gathered into a batch until the disk
/// is quiet for `quiet`, or `longest` has passed since the first,
/// then handed on. It ends when the watcher is dropped.
fn debounce(
    roots: &[PathBuf],
    events: &mpsc::Receiver<notify::Result<notify::Event>>,
    quiet: Duration,
    longest: Duration,
    changed: &dyn Fn(Vec<Change>),
) {
    let mut batch: Vec<Change> = Vec::new();
    let mut first: Option<Instant> = None;
    loop {
        let next = match first {
            None => events
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            Some(at) => {
                let left = longest.saturating_sub(at.elapsed()).min(quiet);
                events.recv_timeout(left)
            }
        };
        match next {
            Ok(Ok(event)) => {
                if event.need_rescan() {
                    // The platform dropped events (inotify's queue
                    // overflowed): every root walked again.
                    batch.extend(roots.iter().cloned().map(Change::Tree));
                } else if is_change(&event.kind) {
                    batch.extend(event.paths.iter().filter_map(|p| change_for(roots, p)));
                }
                if !batch.is_empty() && first.is_none() {
                    first = Some(Instant::now());
                }
                if first.is_some_and(|at| at.elapsed() < longest) {
                    continue;
                }
            }
            Ok(Err(e)) => {
                log::warn!("watching the roots: {e}");
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if !batch.is_empty() {
                    changed(settle(std::mem::take(&mut batch)));
                }
                return;
            }
        }
        if !batch.is_empty() {
            changed(settle(std::mem::take(&mut batch)));
        }
        first = None;
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
        assert_eq!(Roots::load(&path), Roots::default(), "no file is no roots");
        let mut roots = Roots::default();
        roots.add(&a).unwrap();
        roots.add(&b).unwrap();
        roots.save(&path).unwrap();
        assert_eq!(Roots::load(&path), roots);
        assert!(!path.with_extension("json.part").exists());
        // The library's own file can go, and the roots stay.
        std::fs::write(dir.join("library.sqlite"), b"").unwrap();
        std::fs::remove_file(dir.join("library.sqlite")).unwrap();
        assert_eq!(Roots::load(&path).list(), [a.clone(), b.clone()]);

        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(Roots::load(&path), Roots::default());
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
        // The sidecars' hidden folder, a hidden temporary, a sidecar
        // beside its frame, an XMP, a part-copied file, and anything
        // outside the roots, are nobody's business.
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
        // The hidden folder first: if it said anything it would come
        // in the same batch as the raw below.
        std::fs::create_dir_all(root.join("shoot").join(".greycard")).unwrap();
        std::fs::write(
            root.join("shoot").join(".greycard").join("a.CR3.gcd"),
            b"{}",
        )
        .unwrap();
        std::fs::write(root.join("shoot").join("a.CR3"), b"not a raw").unwrap();
        let all = wait(&|c| c.contains(&Change::Folder(root.join("shoot"))));
        // Nothing under the hidden folder. A platform may say the
        // folders the writes touched as well, which is a tree pass
        // over them and harmless.
        assert!(
            all.iter().all(|c| c.path().starts_with(&root)
                && !c.path().components().any(|p| p.as_os_str() == ".greycard")),
            "{all:?}"
        );
        std::fs::create_dir_all(root.join("new")).unwrap();
        wait(&|c| c.contains(&Change::Tree(root.join("new"))));
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
}
