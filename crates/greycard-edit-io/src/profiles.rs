//! The camera profile directory: where it is, what it holds, and the
//! profiles read from it and kept. What a profile is, how one fits a
//! body, and what the panel lists and says are
//! `greycard_edit::camera`'s.
//!
//! The directory is read with its subfolders, as Adobe keeps its own
//! ("Camera/Canon EOS R5/..."), and a profile is named by its path
//! under it ([`greycard_edit::camera::ProfileChoice::Named`]): a file
//! at the top by its bare file name, as before there were subfolders.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
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
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::data_dir)?;
    Some(base.join("greycard").join("profiles"))
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

/// Whether `name` is a profile's name: folders and a file under the
/// directory, joined by `/`, each one plain part of a path. Nothing
/// empty, no `.` or `..`, no `\`, no root and no drive.
pub fn is_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\\')
        && name.split('/').all(|part| {
            let mut parts = Path::new(part).components();
            matches!(
                (parts.next(), parts.next()),
                (Some(Component::Normal(p)), None) if p == part
            )
        })
}

/// How many folders below the directory a profile is looked for.
/// Adobe's tree is two deep; the rest is room for a copy of it put a
/// folder or two down, and a stop for a tree that never ends.
const DEPTH: usize = 8;
/// How many files and folders one listing looks at. Adobe's tree is
/// some thousands; past this the listing stops where it is, says so,
/// and lists what it found.
const EXAMINED: usize = 50_000;
/// How many profiles one listing keeps.
const KEPT: usize = 20_000;

/// The bounds a walk keeps to: [`DEPTH`], [`EXAMINED`], and [`KEPT`],
/// smaller in the tests.
#[derive(Clone, Copy)]
struct Bounds {
    depth: usize,
    examined: usize,
    kept: usize,
}

const BOUNDS: Bounds = Bounds {
    depth: DEPTH,
    examined: EXAMINED,
    kept: KEPT,
};

/// Every profile file under `root`, with the name the edit gives it:
/// the folders, top first and in name order within each, to the
/// bounds' depth, within their counts. A link is followed to what it
/// names, a copy of Adobe's folder linked in being the common way to
/// bring it, but a folder already walked (a link back up, or two links
/// to one place) is walked once. A hidden folder, a folder that will
/// not read, and a name the edit could not hold are passed over, the
/// last two with a word in the log.
fn walk(root: &Path, bounds: Bounds) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(real) = std::fs::canonicalize(root) else {
        return out;
    };
    let mut visited = HashSet::from([real]);
    let mut queue = VecDeque::from([(root.to_path_buf(), String::new(), 0usize)]);
    let mut examined = 0usize;
    while let Some((dir, under, depth)) = queue.pop_front() {
        let read = match std::fs::read_dir(&dir) {
            Ok(read) => read,
            Err(e) => {
                log::warn!("camera profiles: {} not read: {e}", dir.display());
                continue;
            }
        };
        let mut children: Vec<std::fs::DirEntry> = Vec::new();
        for entry in read {
            examined += 1;
            if examined > bounds.examined {
                log::warn!(
                    "camera profiles: stopped after {} files and folders under {}; {} profiles \
                     listed",
                    bounds.examined,
                    root.display(),
                    out.len()
                );
                return out;
            }
            match entry {
                Ok(entry) => children.push(entry),
                Err(e) => log::warn!("camera profiles: in {}: {e}", dir.display()),
            }
        }
        children.sort_by_key(|e| e.file_name());
        for entry in children {
            let path = entry.path();
            let Some(file) = entry.file_name().to_str().map(str::to_string) else {
                log::warn!(
                    "camera profiles: {} has a name that is not text; passed over",
                    path.display()
                );
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            // What a link names, else the entry itself.
            let (is_dir, is_file) = if kind.is_symlink() {
                std::fs::metadata(&path).map_or((false, false), |m| (m.is_dir(), m.is_file()))
            } else {
                (kind.is_dir(), kind.is_file())
            };
            let join = |part: &str| match under.as_str() {
                "" => part.to_string(),
                under => format!("{under}/{part}"),
            };
            if is_dir {
                if file.starts_with('.') {
                    continue;
                }
                if depth >= bounds.depth {
                    log::warn!(
                        "camera profiles: {} is more than {} folders down; not read",
                        path.display(),
                        bounds.depth
                    );
                    continue;
                }
                match std::fs::canonicalize(&path) {
                    Ok(real) => {
                        if visited.insert(real) {
                            queue.push_back((path, join(&file), depth + 1));
                        } else {
                            log::info!(
                                "camera profiles: {} is a folder already read; not read again",
                                path.display()
                            );
                        }
                    }
                    Err(e) => log::warn!("camera profiles: {} not read: {e}", path.display()),
                }
                continue;
            }
            let is_profile = path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case(EXTENSION));
            // A device, a pipe, or a broken link named like a profile
            // is never opened: reading one may never end or start.
            if is_profile && !is_file {
                log::warn!(
                    "camera profiles: {} is not a regular file; passed over",
                    path.display()
                );
                continue;
            }
            let Some(stem) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|_| is_profile)
            else {
                continue;
            };
            let name = join(stem);
            if !is_name(&name) {
                log::warn!(
                    "camera profiles: {} has a name an edit cannot hold; passed over",
                    path.display()
                );
                continue;
            }
            if out.len() >= bounds.kept {
                log::warn!(
                    "camera profiles: {} profiles listed under {}; the rest are not",
                    bounds.kept,
                    root.display()
                );
                return out;
            }
            out.push((name, path));
        }
    }
    out
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

/// Every profile in the directory and its subfolders, by name.
///
/// Only each file's header is read — its name, its camera and its
/// copyright — not its tables, and only once while the file stays as
/// it was: the directory is walked again on every listing, which is
/// each file opened, so a listing of a tree already read is the walk
/// and a look at each file's length and clock. The profile that is
/// chosen is read in full by [`load`], once. A file that will not read
/// is passed over with a word in the log. Two that are one profile
/// under two file names are paired by [`pair_up`].
pub fn list() -> Vec<Entry> {
    // A listing is also the moment to let go of what was read before
    // it: a profile that has been replaced or is no longer wanted has
    // no claim on the memory its tables take.
    forget();
    match store_dir() {
        Some(dir) => list_in(&dir),
        None => Vec::new(),
    }
}

/// Read every profile's header ahead of the first [`list`], so the
/// window's first listing of a large tree (a copy of Adobe's, its
/// headers not yet in the disk's cache) is a look at each file rather
/// than a read of it. For a thread of its own at the start; a listing
/// that comes first reads what it needs itself.
pub fn warm() {
    if let Some(dir) = store_dir() {
        let t = std::time::Instant::now();
        let n = list_in(&dir).len();
        log::info!(
            "camera profiles: {n} under {} read in {:.0?}",
            dir.display(),
            t.elapsed()
        );
    }
}

/// [`list`] of the directory `dir`.
pub fn list_in(dir: &Path) -> Vec<Entry> {
    list_within(dir, BOUNDS)
}

fn list_within(dir: &Path, bounds: Bounds) -> Vec<Entry> {
    let found = walk(dir, bounds);
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

#[cfg(test)]
mod tests {
    use super::*;
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
            let mut n: Vec<String> = list_within(&dir, b).into_iter().map(|e| e.name).collect();
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
}
