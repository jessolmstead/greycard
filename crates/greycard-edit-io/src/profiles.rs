//! The camera profile directory: where it is, what it holds, and the
//! profiles read from it and kept. What a profile is, how one fits a
//! body, and what the panel lists and says are
//! `greycard_edit::camera`'s.
//!
//! The directory is read with its subfolders, as Adobe keeps its own
//! ("Camera/Canon EOS R5/..."), and a profile is named by its path
//! under it ([`greycard_edit::camera::ProfileChoice::Named`]): a file
//! at the top by its bare file name, as before there were subfolders.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use greycard_core::Dcp;
use greycard_core::color::Profile;
use greycard_core::dcp::DcpInfo;
use greycard_edit::camera::{EXTENSION, Entry};

/// Where the profiles live: `$XDG_DATA_HOME/greycard/profiles` when
/// that is set, else `greycard/profiles` under the platform's data
/// directory — `~/.local/share` on Linux, `~/Library/Application
/// Support` on macOS, `%APPDATA%` on Windows (notes §106).
pub fn store_dir() -> Option<PathBuf> {
    if let Some(dir) = STORE.get() {
        return Some(dir.clone());
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::data_dir)?;
    Some(base.join("greycard").join("profiles"))
}

static STORE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The profile directory for the rest of this process, in place of
/// [`store_dir`]'s own: for a test harness, which must never read the
/// user's. The first call holds; later ones change nothing.
pub fn use_store(dir: PathBuf) {
    let _ = STORE.set(dir);
}

/// Where a profile of this name is kept. A name that is not a path
/// under the directory, or tries to climb out of it, is refused.
pub fn path_for(name: &str) -> Option<PathBuf> {
    path_in(store_dir().as_deref(), name)
}

/// [`path_for`] in the profile directory `dir`, `None` for none.
pub fn path_in(dir: Option<&Path>, name: &str) -> Option<PathBuf> {
    if !is_name(name) {
        log::warn!("camera profile {name:?} is not a name in the profile directory");
        return None;
    }
    let mut path = dir?.to_path_buf();
    let mut parts = name.split('/').peekable();
    while let Some(part) = parts.next() {
        match parts.peek() {
            Some(_) => path.push(part),
            None => path.push(format!("{part}.{EXTENSION}")),
        }
    }
    Some(other_case(&path).unwrap_or(path))
}

/// The file `path` names with its extension in another case, as the
/// listing takes `Real.DCP` for the profile `Real`, when there is no
/// `.dcp` file of that name: of the regular files (after any link) of
/// that stem whose extension is `.dcp` in any case, the first by file
/// name, which is the one the listing shows first. `None` when the
/// lowercase file is there, or no other is.
fn other_case(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return None;
    }
    let stem = path.file_stem()?;
    std::fs::read_dir(path.parent()?)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_stem() == Some(stem)
                && p.extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case(EXTENSION))
                && p.is_file()
        })
        .min_by(|a, b| a.file_name().cmp(&b.file_name()))
}

pub use greycard_core::profile_folder::is_name;
use greycard_core::profile_folder::{BOUNDS, Bounds, Kind};

/// Every profile file under `root`, with the name the edit gives it,
/// as `greycard_core::profile_folder::walk` finds them. Each folder
/// read is noted in `folders` with its clock as the walk found it, the
/// root even when it is not there: a profile added, removed, or
/// renamed in a folder moves that folder's clock, which is how
/// [`refresh_in`] tells a listing is out of date without a look at
/// every file.
fn walk(root: &Path, bounds: Bounds, folders: &mut Vec<Folder>) -> Vec<(String, PathBuf)> {
    WALKS.with(|w| w.set(w.get() + 1));
    let kind = Kind {
        extension: EXTENSION,
        what: "camera profiles",
        named_with_extension: false,
    };
    greycard_core::profile_folder::walk(root, kind, bounds, &mut |dir| {
        folders.push(Folder::now(dir));
    })
}

/// A profile file's header as last read, with the length and the
/// clock it was read at: a listing reads again only what changed.
struct Header {
    len: u64,
    stamp: Option<SystemTime>,
    /// `None` for a file that would not read, said once in the log.
    info: Option<DcpInfo>,
}

static HEADERS: LazyLock<Mutex<HashMap<PathBuf, Header>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether the file at `path`, whose header was read, has another
/// length or clock now, or is gone. A file whose header was never read
/// is not a listed profile, and says nothing.
fn header_moved(path: &Path) -> bool {
    let Some((len, stamp)) = HEADERS
        .lock()
        .ok()
        .and_then(|h| h.get(path).map(|h| (h.len, h.stamp)))
    else {
        return false;
    };
    match std::fs::metadata(path) {
        Ok(m) => m.len() != len || m.modified().ok() != stamp,
        Err(_) => true,
    }
}

/// What the file at `path` says about itself: its header, read once
/// and kept until the file's length or its clock changes.
fn header(path: &Path) -> Option<DcpInfo> {
    let meta = std::fs::metadata(path).ok()?;
    let (len, stamp) = (meta.len(), meta.modified().ok());
    if let Ok(headers) = HEADERS.lock()
        && let Some(h) = headers.get(path)
        && h.len == len
        && h.stamp == stamp
    {
        return h.info.clone();
    }
    let info = match Dcp::info(path) {
        Ok(info) => Some(info),
        Err(e) => {
            log::warn!("camera profile {}: {e}", path.display());
            None
        }
    };
    if let Ok(mut headers) = HEADERS.lock() {
        headers.insert(
            path.to_path_buf(),
            Header {
                len,
                stamp,
                info: info.clone(),
            },
        );
    }
    info
}

/// A folder a walk read, and its clock as the walk found it: `None`
/// for one that was not there.
#[derive(Clone)]
struct Folder {
    path: PathBuf,
    stamp: Option<SystemTime>,
}

impl Folder {
    fn now(path: &Path) -> Folder {
        Folder {
            path: path.to_path_buf(),
            stamp: std::fs::metadata(path).and_then(|m| m.modified()).ok(),
        }
    }

    /// Whether the folder's clock has moved since, or it has come or
    /// gone: something in it was added, removed, or renamed.
    fn moved(&self) -> bool {
        Folder::now(&self.path).stamp != self.stamp
    }
}

/// The directory's profiles as last walked, kept for the window to
/// show without walking it again.
#[derive(Clone, Debug)]
pub struct Listing {
    /// Changes when, and only when, a walk finds a different list:
    /// the window compares it to know whether to show the list again.
    pub generation: u64,
    pub entries: Arc<Vec<Entry>>,
}

/// A listing and the folders its walk read.
struct Held {
    listing: Listing,
    folders: Arc<Vec<Folder>>,
}

/// The listing of each directory walked: in the editor, the one
/// directory.
static HELD: LazyLock<Mutex<HashMap<PathBuf, Held>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// One walk at a time, so two asked together walk once.
static WALKING: Mutex<()> = Mutex::new(());
static GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The walks made on this thread, for a test to count.
    static WALKS: Cell<usize> = const { Cell::new(0) };
}

/// How many times the directory has been walked on this thread: a
/// test's count of what a step costs, not a clock.
pub fn walks_here() -> usize {
    WALKS.with(Cell::get)
}

/// Every profile in the directory and its subfolders, by name: the
/// listing held when there is one, else a walk made now.
///
/// Only each file's header is read — its name, its camera and its
/// copyright — not its tables, and only once while the file stays as
/// it was. The profile that is chosen is read in full by [`load`],
/// once. A file that will not read is passed over with a word in the
/// log. Two that are one profile under two file names are paired by
/// [`pair_up`].
///
/// The window shows [`held`] and asks [`refresh`] on a thread of its
/// own; this is for a caller that needs the list in hand and may wait
/// for it, as the command line does.
pub fn list() -> Vec<Entry> {
    let Some(dir) = store_dir() else {
        return Vec::new();
    };
    held_in(&dir)
        .unwrap_or_else(|| refresh_in(&dir, false, &[]))
        .entries
        .to_vec()
}

/// The listing held for the directory, never a walk: `None` until the
/// first walk of it is done.
pub fn held() -> Option<Listing> {
    held_in(&store_dir()?)
}

/// [`held`] of the directory `dir`.
pub fn held_in(dir: &Path) -> Option<Listing> {
    let held = HELD.lock().ok()?;
    held.get(dir).map(|h| h.listing.clone())
}

/// The directory's listing, walked again when `force` is set, when
/// none is held, or when any folder the last walk read has had
/// something added, removed, or renamed in it since (its clock moved).
/// Otherwise the held listing, at the cost of a look at each folder,
/// not at each file. A walk that finds the same list keeps the same
/// generation. It can take a while: for a thread, not the window's.
///
/// `files` are listed profiles to look at too, their length and clock
/// against the ones their header was read under: a profile rewritten
/// in place moves no folder's clock, so the ones on show are named
/// here, and a walk reads again the headers of those that changed.
pub fn refresh(force: bool, files: &[PathBuf]) -> Option<Listing> {
    Some(refresh_in(&store_dir()?, force, files))
}

/// [`refresh`] of the directory `dir`.
pub fn refresh_in(dir: &Path, force: bool, files: &[PathBuf]) -> Listing {
    let _turn = WALKING.lock().unwrap_or_else(|e| e.into_inner());
    // Taken out of the lock, so the window's [`held`] never waits on
    // the look at the folders.
    let before = HELD
        .lock()
        .ok()
        .and_then(|h| h.get(dir).map(|h| (h.listing.clone(), h.folders.clone())));
    let previous = match before {
        Some((listing, folders))
            if !force
                && !folders.iter().any(Folder::moved)
                && !files.iter().any(|f| header_moved(f)) =>
        {
            return listing;
        }
        Some((listing, _)) => Some(listing),
        None => None,
    };
    let mut folders = Vec::new();
    let entries = list_within(dir, BOUNDS, &mut folders);
    let listing = match previous {
        Some(p) if *p.entries == entries => p,
        _ => Listing {
            generation: GENERATION.fetch_add(1, Ordering::Relaxed) + 1,
            entries: Arc::new(entries),
        },
    };
    if let Ok(mut held) = HELD.lock() {
        held.insert(
            dir.to_path_buf(),
            Held {
                listing: listing.clone(),
                folders: Arc::new(folders),
            },
        );
    }
    let_go_of_changed();
    listing
}

/// Read every profile's header ahead of the window's first look, on a
/// thread of its own at the start: the listing the window then shows
/// is held, and the first open of a large tree (a copy of Adobe's, its
/// headers not yet in the disk's cache) waits on nothing.
pub fn warm() -> Option<Listing> {
    let dir = store_dir()?;
    let t = std::time::Instant::now();
    let listing = refresh_in(&dir, true, &[]);
    log::info!(
        "camera profiles: {} under {} read in {:.0?}",
        listing.entries.len(),
        dir.display(),
        t.elapsed()
    );
    Some(listing)
}

/// Every profile in the directory `dir`, walked now and not held.
pub fn list_in(dir: &Path) -> Vec<Entry> {
    list_within(dir, BOUNDS, &mut Vec::new())
}

fn list_within(dir: &Path, bounds: Bounds, folders: &mut Vec<Folder>) -> Vec<Entry> {
    let found = walk(dir, bounds, folders);
    let seen: HashSet<&Path> = found.iter().map(|(_, p)| p.as_path()).collect();
    let mut out: Vec<Entry> = found
        .iter()
        .filter_map(|(name, path)| {
            let info = header(path)?;
            Some(Entry {
                name: name.clone(),
                path: path.clone(),
                title: info.name,
                unique_camera_model: info.unique_camera_model,
                copyright: info.copyright,
                same_as: None,
                tone_curve: info.tone_curve,
            })
        })
        .collect();
    // The headers of files no longer there are let go.
    if let Ok(mut headers) = HEADERS.lock() {
        headers.retain(|p, _| !p.starts_with(dir) || seen.contains(p.as_path()));
    }
    // By name first, so "the first by name" below is this order.
    out.sort_by(|a, b| a.name.cmp(&b.name));
    pair_up(&mut out);
    out.sort_by_key(|e| e.label().to_lowercase());
    out
}

/// Two files that are one profile under two names, as a body's Adobe
/// Standard saved once by its US name and once by its own, list as
/// one. `entries` is in file-name order.
///
/// Only entries whose camera and name collide are looked at further,
/// and only those whose lengths agree are read: the listing otherwise
/// stays a header read.
fn pair_up(entries: &mut [Entry]) {
    let key = |e: &Entry| (e.folded_unique(), e.label().to_string());
    let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        groups.entry(key(e)).or_default().push(i);
    }
    for group in groups.values().filter(|g| g.len() > 1) {
        // The same contents: equal lengths, then equal bytes, each
        // file read at most once. A file that will not read is its own
        // profile.
        let lens: Vec<Option<u64>> = group
            .iter()
            .map(|&i| std::fs::metadata(&entries[i].path).ok().map(|m| m.len()))
            .collect();
        let mut contents: Vec<Option<Vec<u8>>> = vec![None; group.len()];
        let mut same: Vec<Vec<usize>> = Vec::new();
        for a in 0..group.len() {
            let found = same.iter().position(|class| {
                let b = class[0];
                if lens[a].is_none() || lens[a] != lens[b] {
                    return false;
                }
                for k in [a, b] {
                    if contents[k].is_none() {
                        contents[k] =
                            Some(std::fs::read(&entries[group[k]].path).unwrap_or_default());
                    }
                }
                contents[a].as_ref().is_some_and(|c| !c.is_empty()) && contents[a] == contents[b]
            });
            match found {
                Some(c) => same[c].push(a),
                None => same.push(vec![a]),
            }
        }
        let same: Vec<Vec<usize>> = same
            .into_iter()
            .map(|class| class.into_iter().map(|k| group[k]).collect())
            .collect();
        for class in same.iter().filter(|c| c.len() > 1) {
            // The file named for the profile stands for the pair, else
            // the first by name.
            let title = entries[class[0]].label().to_lowercase();
            let stands = class
                .iter()
                .copied()
                .find(|&i| entries[i].name.to_lowercase().contains(&title))
                .unwrap_or(class[0]);
            let name = entries[stands].name.clone();
            for &i in class.iter().filter(|&&i| i != stands) {
                entries[i].same_as = Some(name.clone());
            }
        }
    }
}

/// What a path read to last time.
enum Cached {
    /// There was no file to read, and it has been said once.
    Missing,
    /// A file of this age and size read to this, or to nothing.
    Read {
        stamp: Option<std::time::SystemTime>,
        len: u64,
        profile: Option<Arc<Profile>>,
    },
}

/// How many profiles are kept at once. One edit names one, and a
/// filmstrip of files naming different ones is the only way to reach
/// even a handful; past this the lot is dropped rather than grown.
const KEEP: usize = 8;

static CACHE: LazyLock<Mutex<HashMap<PathBuf, Cached>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The profile in a file, read once and kept until the file changes.
///
/// A file that is missing, or that will not read, is remembered as
/// such, so the warning is said once rather than on every develop.
pub fn load(path: &Path) -> Option<Arc<Profile>> {
    let stamped = match std::fs::metadata(path) {
        Ok(m) => Some((m.modified().ok(), m.len())),
        Err(_) => None,
    };
    let mut cache = CACHE.lock().ok()?;
    match (cache.get(path), &stamped) {
        // Missing, and already said so.
        (Some(Cached::Missing), None) => return None,
        // Read before, from a file that has not been touched since.
        (
            Some(Cached::Read {
                stamp,
                len,
                profile,
            }),
            Some((now, size)),
        ) if stamp == now && len == size => {
            return profile.clone();
        }
        _ => {}
    }
    if cache.len() >= KEEP {
        cache.clear();
    }
    let Some((stamp, len)) = stamped else {
        log::warn!("camera profile {}: it is not there", path.display());
        cache.insert(path.to_path_buf(), Cached::Missing);
        return None;
    };
    let profile = match Dcp::load(path).and_then(|dcp| dcp.profile()) {
        Ok(profile) => Some(Arc::new(profile)),
        Err(e) => {
            log::warn!("camera profile {}: {e}", path.display());
            None
        }
    };
    cache.insert(
        path.to_path_buf(),
        Cached::Read {
            stamp,
            len,
            profile: profile.clone(),
        },
    );
    profile
}

/// Forget what has been read, so the next develop reads it again.
pub fn forget() {
    if let Ok(mut cache) = CACHE.lock() {
        cache.clear();
    }
}

/// Let go of each profile read whose file has changed since, or gone,
/// and of a missing one that has come: a profile replaced on disk has
/// no claim on the memory its old tables take, and one that stands as
/// it was keeps its `Arc`, so a develop that holds it reads nothing.
/// [`load`] reads a changed file again whether or not this ran; this
/// is the memory's side, done at each walk of the directory.
///
/// The files are looked at with the cache let go of, so a share that
/// hangs holds this walk, not a develop's [`load`]; an entry read again
/// meanwhile is not the one looked at, and is kept.
fn let_go_of_changed() {
    // A file's clock and length as read, `None` for one missing.
    type Stamp = Option<(Option<SystemTime>, u64)>;
    // What each entry was read under: `None` for missing.
    let held: Vec<(PathBuf, Stamp)> = match CACHE.lock() {
        Ok(cache) => cache
            .iter()
            .map(|(path, cached)| {
                let was = match cached {
                    Cached::Missing => None,
                    Cached::Read { stamp, len, .. } => Some((*stamp, *len)),
                };
                (path.clone(), was)
            })
            .collect(),
        Err(_) => return,
    };
    let changed: Vec<(PathBuf, Stamp)> = held
        .into_iter()
        .filter(|(path, was)| {
            let now = std::fs::metadata(path)
                .ok()
                .map(|m| (m.modified().ok(), m.len()));
            now != *was
        })
        .collect();
    if changed.is_empty() {
        return;
    }
    let Ok(mut cache) = CACHE.lock() else {
        return;
    };
    for (path, was) in changed {
        let same = match (cache.get(&path), was) {
            (Some(Cached::Missing), None) => true,
            (Some(Cached::Read { stamp, len, .. }), Some((s, l))) => *stamp == s && *len == l,
            _ => false,
        };
        if same {
            cache.remove(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_core::testing::a_moment;
    use greycard_edit::camera::{Body, EMBEDDED, ProfileChoice, listed, warning};

    #[test]
    fn a_name_that_is_a_path_is_refused() {
        for name in [
            "../../etc/passwd",
            "sub/../../x",
            "./x",
            "a//b",
            "a/",
            "/etc/passwd",
            "",
            "a\\b",
            "C:\\x",
        ] {
            assert!(path_in(Some(Path::new("/p")), name).is_none(), "{name}");
        }
        // Folders under the directory are a name; so is a bare file.
        assert_eq!(
            path_in(Some(Path::new("/p")), "Camera/Canon EOS R5/Adobe Standard"),
            Some(Path::new("/p/Camera/Canon EOS R5/Adobe Standard.dcp").to_path_buf())
        );
        assert_eq!(
            path_in(Some(Path::new("/p")), "Canon EOS R5 Neutral"),
            Some(Path::new("/p/Canon EOS R5 Neutral.dcp").to_path_buf())
        );
    }

    /// A small but real DCP, written by the engine's own writer.
    /// `hues` changes both what is in it and how long it is, so a
    /// rewrite is a different file whatever the clock's resolution.
    fn a_profile_file(path: &Path, hues: usize) {
        a_profile(path, "Test Camera Neutral", "Test Cam", hues);
    }

    /// [`a_profile_file`] with its own name and camera.
    fn a_profile(path: &Path, name: &str, camera: &str, hues: usize) {
        use greycard_core::dcp::{DcpCalibration, EmbedPolicy, HueSatMap};
        let matrix = [0.8, -0.1, -0.1, -0.4, 1.3, 0.1, 0.0, -0.2, 0.9];
        let dcp = Dcp {
            name: Some(name.into()),
            unique_camera_model: Some(camera.into()),
            copyright: Some("public domain".into()),
            calibration_signature: None,
            embed_policy: EmbedPolicy::AllowCopying,
            primary: DcpCalibration {
                illuminant: 17,
                color_matrix: matrix,
                forward_matrix: None,
                hue_sat_map: Some(HueSatMap::identity(hues, 2, 1)),
            },
            secondary: None,
            baseline_exposure_offset: None,
            look: None,
            tone_curve: None,
        };
        std::fs::write(path, dcp.to_bytes()).unwrap();
    }

    /// Whether the volume under `dir` takes two spellings of a name
    /// for one file.
    fn folds_case(dir: &Path) -> bool {
        std::fs::write(dir.join("case-probe"), "").unwrap();
        let folds = dir.join("CASE-PROBE").exists();
        std::fs::remove_file(dir.join("case-probe")).unwrap();
        folds
    }

    /// The cache is one static for the whole process, and these tests
    /// say what is in it, so they take their turn rather than run
    /// alongside each other.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    /// A directory of this test's own, gone when it returns.
    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("greycard-dcp-{what}-{}", std::process::id()));
        greycard_core::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_profile_file_reads_to_a_profile_and_is_kept() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("ok");
        let path = dir.join("Test Cam Neutral.dcp");
        a_profile_file(&path, 4);

        let profile = load(&path).expect("the profile reads");
        assert_eq!(profile.name.as_deref(), Some("Test Camera Neutral"));
        assert!(profile.maps.is_some(), "it carries the map");
        // The second read is the same profile, not another copy of it.
        assert!(Arc::ptr_eq(&profile, &load(&path).unwrap()));

        // Rewritten with a map of another size, it is read afresh: the
        // length alone says so, without leaning on the file clock.
        a_profile_file(&path, 8);
        let again = load(&path).expect("the new one reads");
        assert!(!Arc::ptr_eq(&profile, &again));
        assert_eq!(again.maps.as_ref().unwrap().primary.hue_divisions, 8);

        // And forgetting makes the next read a fresh one too.
        forget();
        assert!(!Arc::ptr_eq(&again, &load(&path).unwrap()));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_file_that_is_not_a_profile_resolves_to_nothing() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("junk");
        let path = dir.join("not-a-profile.dcp");
        std::fs::write(&path, b"this is not a camera profile").unwrap();
        assert!(load(&path).is_none());
        // Twice: the second answer comes from the cache, and is the
        // same nothing.
        assert!(load(&path).is_none());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A profile the edit names and the directory has not got is
    /// remembered as missing, so the develop that follows every slider
    /// does not say so again; and it is read the moment it appears.
    #[test]
    fn a_missing_profile_is_remembered_as_missing() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("missing");
        let path = dir.join("Not Here Yet.dcp");
        assert!(load(&path).is_none());
        assert!(matches!(
            CACHE.lock().unwrap().get(&path),
            Some(Cached::Missing)
        ));
        assert!(load(&path).is_none());

        // Put it there and it is read, missing marker and all.
        a_profile_file(&path, 4);
        assert!(load(&path).is_some());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The listing reads headers, not tables: a profile in the
    /// directory is named without its map being built.
    #[test]
    fn listing_does_not_read_the_tables() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("list");
        let path = dir.join("Test Cam Neutral.dcp");
        a_profile_file(&path, 4);
        forget();
        let info = Dcp::info(&path).unwrap();
        assert_eq!(info.name.as_deref(), Some("Test Camera Neutral"));
        assert_eq!(info.unique_camera_model.as_deref(), Some("Test Cam"));
        assert!(
            !CACHE.lock().unwrap().contains_key(&path),
            "a header read keeps nothing"
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    fn labels<'a>(entries: &'a [Entry], chosen: &str) -> Vec<&'a str> {
        listed(entries, chosen).map(Entry::label).collect()
    }

    /// The tester's case: a body's Adobe Standard saved under its own
    /// name and under its US one is one profile, listed once, by the
    /// file named for the profile even when it is not the first by
    /// name; an edit naming the other file still finds it, and it fits
    /// the body by its other name.
    #[test]
    fn one_profile_under_two_file_names_lists_once() {
        let dir = scratch("pair");
        let rebel = "Canon EOS Rebel T8i";
        let (first, named) = ("Canon EOS 850D", "Canon EOS Rebel T8i Adobe Standard");
        for name in [first, named] {
            a_profile(&dir.join(format!("{name}.dcp")), "Adobe Standard", rebel, 4);
        }
        let entries = list_in(&dir);
        assert_eq!(entries.len(), 2, "both files are entries");
        let shown: Vec<&str> = listed(&entries, EMBEDDED)
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(shown, [named]);
        assert_eq!(labels(&entries, EMBEDDED), ["Adobe Standard"]);
        // A sidecar naming the other file shows that file in its place.
        let shown: Vec<&str> = listed(&entries, first).map(|e| e.name.as_str()).collect();
        assert_eq!(shown, [first]);
        assert_eq!(labels(&entries, first), ["Adobe Standard"]);
        // And the warning finds it by its name, not its label.
        let aliases = ["EOS Rebel T8i".to_string()];
        let body = Body::new("Canon", "EOS 850D", &aliases);
        assert!(warning(&entries, body, first).is_empty());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Two profiles of one name for one camera, with different tables,
    /// are two profiles: both listed, for the panel to tell apart.
    #[test]
    fn two_profiles_of_one_name_both_list() {
        let dir = scratch("twins");
        let rebel = "Canon EOS Rebel T8i";
        a_profile(&dir.join("old.dcp"), "Adobe Standard", rebel, 4);
        a_profile(&dir.join("new.dcp"), "Adobe Standard", rebel, 8);
        // The same length, different bytes: read, and kept apart.
        a_profile(
            &dir.join("other.dcp"),
            "Adobe Standard",
            "CANON EOS REBEL T8I",
            4,
        );
        let tables = |n: &str| std::fs::read(dir.join(n)).unwrap();
        let mut other = tables("other.dcp");
        let last = other.len() - 1;
        other[last] ^= 1;
        std::fs::write(dir.join("other.dcp"), &other).unwrap();
        assert_eq!(other.len(), tables("old.dcp").len());
        let entries = list_in(&dir);
        let shown: Vec<&str> = listed(&entries, EMBEDDED)
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(shown, ["new", "old", "other"]);
        assert!(entries.iter().all(|e| e.same_as.is_none()));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// One name for two cameras: two profiles, listed plainly.
    #[test]
    fn one_name_for_two_cameras_lists_plainly() {
        let dir = scratch("cameras");
        a_profile(&dir.join("r5.dcp"), "Adobe Standard", "Canon EOS R5", 4);
        a_profile(&dir.join("z8.dcp"), "Adobe Standard", "NIKON Z 8", 4);
        let entries = list_in(&dir);
        assert_eq!(
            labels(&entries, EMBEDDED),
            ["Adobe Standard", "Adobe Standard"]
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Adobe's shape: a profile for each camera in a folder of its
    /// own, under "Camera", and one "Adobe Standard" file name in every
    /// one of them, beside a profile at the top as before.
    fn adobe_tree(dir: &Path) {
        for (camera, folder) in [
            ("Canon EOS R5", "Canon EOS R5"),
            ("Canon EOS Rebel T8i", "Canon EOS Rebel T8i"),
            ("NIKON Z 8", "Nikon Z 8"),
        ] {
            let at = dir.join("Camera").join(folder);
            std::fs::create_dir_all(&at).unwrap();
            a_profile(&at.join("Adobe Standard.dcp"), "Adobe Standard", camera, 4);
            a_profile(&at.join("Camera Neutral.dcp"), "Camera Neutral", camera, 4);
        }
        a_profile(&dir.join("Top.dcp"), "Top", "Canon EOS R5", 4);
    }

    /// The subfolders are read, each profile named by its path under
    /// the directory and attributed to the camera its header names; a
    /// body's other names still fit it.
    #[test]
    fn the_subfolders_are_read() {
        let dir = scratch("tree");
        adobe_tree(&dir);
        let entries = list_in(&dir);
        let mut names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        names.sort();
        assert_eq!(
            names,
            [
                "Camera/Canon EOS R5/Adobe Standard",
                "Camera/Canon EOS R5/Camera Neutral",
                "Camera/Canon EOS Rebel T8i/Adobe Standard",
                "Camera/Canon EOS Rebel T8i/Camera Neutral",
                "Camera/Nikon Z 8/Adobe Standard",
                "Camera/Nikon Z 8/Camera Neutral",
                "Top",
            ]
        );
        let camera = |name: &str| {
            entries
                .iter()
                .find(|e| e.name == name)
                .and_then(|e| e.unique_camera_model.clone())
                .unwrap()
        };
        assert_eq!(camera("Camera/Nikon Z 8/Adobe Standard"), "NIKON Z 8");
        assert_eq!(camera("Camera/Canon EOS R5/Camera Neutral"), "Canon EOS R5");
        // An 850D, also sold as the Rebel T8i, is fitted by the
        // Rebel's folder and by nothing else's.
        let aliases = ["EOS Rebel T8i".to_string()];
        let body = Body::new("Canon", "EOS 850D", &aliases);
        let fit: Vec<&str> = entries
            .iter()
            .filter(|e| e.fits(body))
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(fit.len(), 2, "{fit:?}");
        assert!(
            fit.iter()
                .all(|n| n.starts_with("Camera/Canon EOS Rebel T8i/"))
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// One file name in two folders is two profiles, each found by its
    /// own name and read to its own tables; a profile at the top is
    /// still found by its bare name, as an edit made before the
    /// subfolders names it.
    #[test]
    fn one_file_name_in_two_folders_is_two_profiles() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("same-name");
        adobe_tree(&dir);
        // The same camera in two folders, with different tables, so
        // nothing pairs them.
        let again = dir.join("Elsewhere");
        std::fs::create_dir_all(&again).unwrap();
        a_profile(
            &again.join("Adobe Standard.dcp"),
            "Adobe Standard",
            "NIKON Z 8",
            8,
        );
        let entries = list_in(&dir);
        let shown: Vec<&str> = listed(&entries, EMBEDDED)
            .filter(|e| e.label() == "Adobe Standard")
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(shown.len(), 4, "{shown:?}");
        let stores = crate::stores::Stores {
            profiles: Some(dir.clone()),
            looks: None,
        };
        use greycard_edit::Loader;
        let nikon = stores.profile("Camera/Nikon Z 8/Adobe Standard").unwrap();
        let other = stores.profile("Elsewhere/Adobe Standard").unwrap();
        assert_eq!(nikon.maps.as_ref().unwrap().primary.hue_divisions, 4);
        assert_eq!(other.maps.as_ref().unwrap().primary.hue_divisions, 8);
        for e in &entries {
            assert_eq!(path_in(Some(&dir), &e.name).as_ref(), Some(&e.path));
        }
        // The flat name an old sidecar holds.
        assert_eq!(stores.profile("Top").unwrap().name.as_deref(), Some("Top"));
        // And the edit holds the path as it is, through a sidecar.
        let choice = ProfileChoice::Named("Camera/Nikon Z 8/Adobe Standard".into());
        assert_eq!(ProfileChoice::from_name(choice.name()), choice);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A link back up the tree is a loop the walk does not go round; a
    /// link to a folder outside is read as the folder it names.
    #[cfg(unix)]
    #[test]
    fn a_link_loop_is_walked_once() {
        let dir = scratch("loop");
        let outside = scratch("loop-outside");
        adobe_tree(&dir);
        a_profile(&outside.join("Linked.dcp"), "Linked", "Canon EOS R5", 4);
        std::os::unix::fs::symlink(&dir, dir.join("Camera").join("up")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("adobe")).unwrap();
        let entries = list_in(&dir);
        assert_eq!(entries.len(), 8, "the tree, once, and the linked folder");
        assert!(entries.iter().any(|e| e.name == "adobe/Linked"));
        assert!(entries.iter().all(|e| !e.name.contains("up/")));
        greycard_core::testing::remove_dir_retry(&dir);
        greycard_core::testing::remove_dir_retry(&outside);
    }

    /// A folder that will not read is passed over, and the rest listed.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_folder_is_passed_over() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("unreadable");
        adobe_tree(&dir);
        let shut = dir.join("Camera").join("Nikon Z 8");
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads it anyway; there is nothing to show then.
        let blocked = std::fs::read_dir(&shut).is_err();
        let entries = list_in(&dir);
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();
        if blocked {
            assert_eq!(entries.len(), 5);
            assert!(entries.iter().all(|e| !e.name.contains("Nikon")));
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The walk keeps to its depth, its count of what it looks at, and
    /// its count of profiles kept.
    #[test]
    fn the_walk_keeps_to_its_bounds() {
        let dir = scratch("bounds");
        let mut at = dir.clone();
        for depth in 0..5 {
            a_profile(&at.join(format!("p{depth}.dcp")), "P", "Cam", 4);
            at = at.join(format!("d{depth}"));
            std::fs::create_dir_all(&at).unwrap();
        }
        let names = |b: Bounds| -> Vec<String> {
            let mut n: Vec<String> = list_within(&dir, b, &mut Vec::new())
                .into_iter()
                .map(|e| e.name)
                .collect();
            n.sort();
            n
        };
        let wide = Bounds {
            depth: 100,
            examined: 1000,
            kept: 1000,
        };
        assert_eq!(names(wide).len(), 5);
        assert_eq!(
            names(Bounds { depth: 2, ..wide }),
            ["d0/d1/p2", "d0/p1", "p0"]
        );
        assert_eq!(names(Bounds { kept: 2, ..wide }), ["d0/p1", "p0"]);
        // The top's two entries, then the next folder's two.
        assert_eq!(
            names(Bounds {
                examined: 4,
                ..wide
            }),
            ["d0/p1", "p0"]
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// An empty directory and a missing one list nothing; files that
    /// are not profiles, and a folder named like one, are not listed.
    #[test]
    fn what_is_not_a_profile_is_not_listed() {
        let dir = scratch("empty");
        assert!(list_in(&dir).is_empty());
        assert!(list_in(&dir.join("not there")).is_empty());
        std::fs::write(dir.join("notes.txt"), "hello").unwrap();
        std::fs::write(dir.join("junk.dcp"), "not a profile").unwrap();
        std::fs::create_dir_all(dir.join("folder.dcp")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        a_profile(&dir.join(".hidden").join("Hidden.dcp"), "H", "Cam", 4);
        a_profile(&dir.join("folder.dcp").join("Real.DCP"), "Real", "Cam", 4);
        // Links to a device and to a pipe, named like profiles, are not
        // listed, and the listing returns rather than read them.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/dev/zero", dir.join("zero.dcp")).unwrap();
            let _ = std::process::Command::new("mkfifo")
                .arg(dir.join("pipe.dcp"))
                .status();
        }
        let entries = list_in(&dir);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["folder.dcp/Real"]);
        // What is listed resolves, its extension's case and all, by
        // the way a develop reads a profile.
        let found = path_in(Some(&dir), names[0]).unwrap();
        if folds_case(&dir) {
            // Either spelling is the one file there.
            assert!(found.is_file());
        } else {
            assert_eq!(found, entries[0].path);
        }
        use greycard_edit::Loader;
        let stores = crate::stores::Stores {
            profiles: Some(dir.clone()),
            looks: None,
        };
        let real = stores.profile("folder.dcp/Real").expect("Real.DCP loads");
        assert_eq!(real.name.as_deref(), Some("Real"));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A name whose file's extension is in another case resolves to
    /// what the listing shows first under it: of two such files, the
    /// first by file name; past a folder named like the lowercase
    /// file; and to nothing loadable when only a folder has the name.
    #[test]
    fn another_case_resolves_as_the_listing_shows() {
        let dir = scratch("cases");
        if folds_case(&dir) {
            // Windows and macOS hold one spelling of a name, so there
            // is no second case to tell apart.
            greycard_core::testing::remove_dir_retry(&dir);
            return;
        }
        // One title, so the listing's order among them is the walk's.
        a_profile(&dir.join("A.DCP"), "A", "Cam", 4);
        a_profile(&dir.join("A.Dcp"), "A", "Cam", 8);
        std::fs::create_dir_all(dir.join("L.dcp")).unwrap();
        a_profile(&dir.join("L.DCP"), "Shadowed", "Cam", 4);
        std::fs::create_dir_all(dir.join("D.DCP")).unwrap();
        let entries = list_in(&dir);
        let first = |name: &str| {
            listed(&entries, EMBEDDED)
                .find(|e| e.name == name)
                .map(|e| e.path.clone())
        };
        assert_eq!(first("A"), Some(dir.join("A.DCP")));
        assert_eq!(path_in(Some(&dir), "A"), first("A"));
        assert_eq!(first("L"), Some(dir.join("L.DCP")));
        assert_eq!(path_in(Some(&dir), "L"), first("L"));
        // A folder alone is no profile: listed as nothing, and its
        // name resolves to no file.
        assert!(first("D").is_none());
        assert!(!path_in(Some(&dir), "D").unwrap().is_file());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A header read once is not read again while its file stands; a
    /// file rewritten is read again, and one removed is let go.
    #[test]
    fn a_header_is_read_once_while_its_file_stands() {
        let dir = scratch("headers");
        let path = dir.join("a.dcp");
        a_profile(&path, "First", "Cam", 4);
        assert_eq!(list_in(&dir)[0].label(), "First");
        assert!(HEADERS.lock().unwrap().contains_key(&path));
        a_profile(&path, "Second", "Cam", 8);
        assert_eq!(list_in(&dir)[0].label(), "Second");
        std::fs::remove_file(&path).unwrap();
        assert!(list_in(&dir).is_empty());
        assert!(!HEADERS.lock().unwrap().contains_key(&path));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The timing of a listing of 3,000 profiles in 300 folders: the
    /// first, which reads every header, and the next, which only
    /// looks. `cargo test -p greycard-edit-io -- --ignored --nocapture
    /// three_thousand`. With `GREYCARD_TIMED_TREE` naming a folder the
    /// tree is made there once and kept, for a run with its pages out
    /// of the cache.
    #[test]
    #[ignore]
    fn three_thousand_profiles_timed() {
        use greycard_core::dcp::HueSatMap;
        // On the disk under target/, not in the temporary directory,
        // which may be memory: the tree is most of a gigabyte.
        let kept = std::env::var_os("GREYCARD_TIMED_TREE").map(PathBuf::from);
        let dir = kept.clone().unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/greycard-dcp-timed")
                .join(std::process::id().to_string())
        });
        if kept.is_none() {
            greycard_core::testing::remove_dir_retry(&dir);
        }
        let build = kept.is_none() || !dir.join("Camera").exists();
        for c in (0..300).filter(|_| build) {
            let at = dir.join("Camera").join(format!("Camera {c:03}"));
            std::fs::create_dir_all(&at).unwrap();
            for p in 0..10 {
                let path = at.join(format!("Profile {p}.dcp"));
                a_profile(&path, &format!("Profile {p}"), &format!("Cam {c}"), 4);
                // A look table the size of Adobe's (90x16x16), so a
                // file is as long as theirs are.
                let mut dcp = Dcp::load(&path).unwrap();
                dcp.look = Some(HueSatMap::identity(90, 16, 16));
                std::fs::write(&path, dcp.to_bytes()).unwrap();
            }
        }
        HEADERS.lock().unwrap().clear();
        let t = std::time::Instant::now();
        let first = list_in(&dir);
        let cold = t.elapsed();
        let t = std::time::Instant::now();
        let again = list_in(&dir);
        let warm = t.elapsed();
        assert_eq!(first.len(), 3000);
        assert_eq!(again.len(), 3000);
        let bytes: u64 = first
            .iter()
            .map(|e| std::fs::metadata(&e.path).unwrap().len())
            .sum();
        // What reading each file whole for its header took instead.
        let t = std::time::Instant::now();
        for e in &first {
            Dcp::info_from_bytes(&std::fs::read(&e.path).unwrap()).unwrap();
        }
        let whole = t.elapsed();
        println!(
            "3000 profiles in 300 folders ({} MB): first listing {cold:?}, next {warm:?}; \
             the headers read from whole files {whole:?}",
            bytes / 1_000_000
        );
        if kept.is_none() {
            greycard_core::testing::remove_dir_retry(&dir);
        }
    }

    /// The listing held is shown without a walk; a refresh looks at
    /// the folders and walks only when one of them changed, in a
    /// subfolder as at the top, and a walk that finds the same list
    /// keeps its generation.
    #[test]
    fn a_refresh_walks_only_when_a_folder_changed() {
        let dir = scratch("refresh");
        adobe_tree(&dir);
        let walks = walks_here();
        assert!(held_in(&dir).is_none(), "nothing held before a walk");
        let first = refresh_in(&dir, false, &[]);
        assert_eq!(walks_here(), walks + 1);
        assert_eq!(first.entries.len(), 7);
        // Held, and looked at again: no walk, the same list.
        assert_eq!(held_in(&dir).unwrap().generation, first.generation);
        let again = refresh_in(&dir, false, &[]);
        assert_eq!(walks_here(), walks + 1, "nothing changed, nothing walked");
        assert!(Arc::ptr_eq(&again.entries, &first.entries));

        // A profile added in a camera's folder: walked, and listed.
        let nikon = dir.join("Camera").join("Nikon Z 8");
        a_moment(&nikon);
        a_profile(&nikon.join("Added.dcp"), "Added", "NIKON Z 8", 4);
        assert_eq!(held_in(&dir).unwrap().generation, first.generation);
        let added = refresh_in(&dir, false, &[]);
        assert_eq!(walks_here(), walks + 2);
        assert_ne!(added.generation, first.generation);
        assert!(
            added
                .entries
                .iter()
                .any(|e| e.name == "Camera/Nikon Z 8/Added")
        );

        // Walked again by force over the same files: the same list,
        // the same generation.
        let forced = refresh_in(&dir, true, &[]);
        assert_eq!(walks_here(), walks + 3);
        assert_eq!(forced.generation, added.generation);

        // Removed: walked, and gone.
        a_moment(&nikon);
        std::fs::remove_file(nikon.join("Added.dcp")).unwrap();
        let removed = refresh_in(&dir, false, &[]);
        assert_eq!(walks_here(), walks + 4);
        assert_ne!(removed.generation, added.generation);
        assert_eq!(removed.entries.len(), 7);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A profile rewritten in place, its length the same, moves no
    /// folder's clock: a refresh that names it reads its header again,
    /// one that does not leaves it.
    #[test]
    fn a_profile_rewritten_in_place_is_read_again_when_named() {
        let dir = scratch("in-place");
        let path = dir.join("p.dcp");
        a_profile(&path, "Alpha", "Cam", 4);
        let first = refresh_in(&dir, false, &[]);
        assert_eq!(first.entries[0].label(), "Alpha");
        let len = std::fs::metadata(&path).unwrap().len();
        a_moment(&path);
        a_profile(&path, "Bravo", "Cam", 4);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), len);
        let walks = walks_here();
        let unnamed = refresh_in(&dir, false, &[]);
        assert_eq!(unnamed.entries[0].label(), "Alpha", "no folder moved");
        assert_eq!(walks_here(), walks);
        let named = refresh_in(&dir, false, std::slice::from_ref(&path));
        assert_eq!(walks_here(), walks + 1);
        assert_eq!(named.entries[0].label(), "Bravo");
        assert_ne!(named.generation, first.generation);
        // Named again, unchanged: no walk.
        refresh_in(&dir, false, std::slice::from_ref(&path));
        assert_eq!(walks_here(), walks + 1);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A directory not there yet is noticed when it comes.
    #[test]
    fn a_directory_made_later_is_noticed() {
        let dir = scratch("later").join("profiles");
        assert!(refresh_in(&dir, false, &[]).entries.is_empty());
        std::fs::create_dir_all(&dir).unwrap();
        a_profile(&dir.join("New.dcp"), "New", "Cam", 4);
        let listed = refresh_in(&dir, false, &[]);
        assert_eq!(listed.entries.len(), 1);
        greycard_core::testing::remove_dir_retry(dir.parent().unwrap());
    }

    /// A walk lets go of a profile read whose file was replaced, which
    /// the next develop reads afresh, and keeps the one that stands as
    /// it was, pointer and all.
    #[test]
    fn a_walk_keeps_the_profiles_that_did_not_change() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("kept");
        let (kept, replaced) = (dir.join("Kept.dcp"), dir.join("Replaced.dcp"));
        a_profile(&kept, "Kept", "Cam", 4);
        a_profile(&replaced, "Replaced", "Cam", 4);
        let (k, r) = (load(&kept).unwrap(), load(&replaced).unwrap());
        refresh_in(&dir, false, &[]);
        assert!(Arc::ptr_eq(&k, &load(&kept).unwrap()));
        assert!(Arc::ptr_eq(&r, &load(&replaced).unwrap()));

        a_moment(&replaced);
        a_profile(&replaced, "Replaced", "Cam", 8);
        refresh_in(&dir, true, &[]);
        assert!(
            !CACHE.lock().unwrap().contains_key(&replaced),
            "the replaced profile's tables are let go"
        );
        assert!(Arc::ptr_eq(&k, &load(&kept).unwrap()));
        let read = load(&replaced).unwrap();
        assert!(!Arc::ptr_eq(&r, &read));
        assert_eq!(read.maps.as_ref().unwrap().primary.hue_divisions, 8);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// What an open costs: before, a walk of the directory and the
    /// panel's camera filter two or three times; now, the held listing
    /// and the filter once. A tree of 4,400 small profiles in 440
    /// folders, on the disk under target/. `cargo test -p
    /// greycard-edit-io -- --ignored --nocapture an_open_timed`.
    #[test]
    #[ignore]
    fn an_open_timed() {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/greycard-dcp-open-timed");
        if !dir.join("Camera").exists() {
            for c in 0..440 {
                let at = dir.join("Camera").join(format!("Camera {c:03}"));
                std::fs::create_dir_all(&at).unwrap();
                for p in 0..10 {
                    let path = at.join(format!("Profile {p}.dcp"));
                    a_profile(&path, &format!("Profile {p}"), &format!("Maker Cam {c}"), 4);
                }
            }
        }
        let others = Vec::new();
        let body = Body::new("Maker", "Cam 7", &others);
        let filter = |entries: &[Entry]| {
            let (shown, hidden) =
                listed(entries, EMBEDDED).fold((0, 0), |(s, h), e| match e.fits(body) {
                    true => (s + 1, h),
                    false => (s, h + 1),
                });
            assert_eq!((shown, hidden), (10, 4390));
        };
        let timed = |f: &mut dyn FnMut()| {
            let t = std::time::Instant::now();
            f();
            t.elapsed()
        };
        HEADERS.lock().unwrap().clear();
        let cold = timed(&mut || assert_eq!(refresh_in(&dir, true, &[]).entries.len(), 4400));
        let walk = timed(&mut || assert_eq!(list_in(&dir).len(), 4400));
        let check = timed(&mut || {
            refresh_in(&dir, false, &[]);
        });
        // An open's look off the window's thread: the folders, and the
        // files of the ten profiles on show.
        let shown: Vec<PathBuf> = (0..10)
            .map(|p| {
                dir.join("Camera/Camera 007")
                    .join(format!("Profile {p}.dcp"))
            })
            .collect();
        let look = timed(&mut || {
            refresh_in(&dir, false, &shown);
        });
        // The section opened: a walk by force, the headers unchanged.
        let forced = timed(&mut || {
            refresh_in(&dir, true, &[]);
        });
        let one = timed(&mut || filter(&held_in(&dir).unwrap().entries));
        let before = timed(&mut || {
            let entries = list_in(&dir);
            for _ in 0..3 {
                filter(&entries);
            }
        });
        let now = timed(&mut || {
            let held = held_in(&dir).unwrap();
            filter(&held.entries);
        });
        println!(
            "4400 profiles in 440 folders: first walk {cold:?}, a walk after {walk:?}, a look at \
             the folders {check:?}, an open's look {look:?}, the section's walk {forced:?}, one \
             filter {one:?}; an open before {before:?}, now {now:?}"
        );
    }
}
