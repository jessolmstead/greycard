//! A profile folder read with its subfolders, as Adobe keeps its own:
//! camera profiles (`.dcp`) by camera, lens profiles (`.lcp`) by
//! maker. A profile is named by its path under the folder, folders
//! joined by `/` on every system; a file at the top by its bare file
//! name. The bounds and the rules are §662's.

use std::collections::{HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};

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

/// The bounds a walk keeps to: [`BOUNDS`] in use, smaller in tests.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub depth: usize,
    pub examined: usize,
    pub kept: usize,
}

pub const BOUNDS: Bounds = Bounds {
    depth: DEPTH,
    examined: EXAMINED,
    kept: KEPT,
};

/// What kind of profile a walk looks for.
#[derive(Clone, Copy, Debug)]
pub struct Kind<'a> {
    /// The files' extension, matched in any case.
    pub extension: &'a str,
    /// What the log calls them: "camera profiles".
    pub what: &'a str,
    /// Whether a profile's name ends in its file's extension, as a
    /// lens profile's does, or in the stem alone, as a camera
    /// profile's.
    pub named_with_extension: bool,
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

/// The path under `dir` of the profile `name` names, its parts joined
/// as the system joins them; `None` for a name [`is_name`] refuses.
pub fn join(dir: &Path, name: &str) -> Option<PathBuf> {
    is_name(name).then(|| {
        name.split('/')
            .fold(dir.to_path_buf(), |p, part| p.join(part))
    })
}

/// Every profile file under `root`, with the name the edit gives it:
/// the folders, top first and in name order within each, to the
/// bounds' depth, within their counts. A link is followed to what it
/// names, a copy of Adobe's folder linked in being the common way to
/// bring it, but a folder already walked (a link back up, or two links
/// to one place) is walked once. A hidden folder, a folder that will
/// not read, and a name the edit could not hold are passed over, the
/// last two with a word in the log.
///
/// Each folder read is handed to `folder` as the walk comes to it, the
/// root even when it is not there: a caller that holds a listing notes
/// the folders' clocks, which move when a profile is added, removed,
/// or renamed in them.
pub fn walk(
    root: &Path,
    kind: Kind,
    bounds: Bounds,
    folder: &mut dyn FnMut(&Path),
) -> Vec<(String, PathBuf)> {
    let what = kind.what;
    let mut out = Vec::new();
    let Ok(real) = std::fs::canonicalize(root) else {
        folder(root);
        return out;
    };
    let mut visited = HashSet::from([real]);
    let mut queue = VecDeque::from([(root.to_path_buf(), String::new(), 0usize)]);
    let mut examined = 0usize;
    while let Some((dir, under, depth)) = queue.pop_front() {
        folder(&dir);
        let read = match std::fs::read_dir(&dir) {
            Ok(read) => read,
            Err(e) => {
                log::warn!("{what}: {} not read: {e}", dir.display());
                continue;
            }
        };
        let mut children: Vec<std::fs::DirEntry> = Vec::new();
        for entry in read {
            examined += 1;
            if examined > bounds.examined {
                log::warn!(
                    "{what}: stopped after {} files and folders under {}; {} profiles listed",
                    bounds.examined,
                    root.display(),
                    out.len()
                );
                return out;
            }
            match entry {
                Ok(entry) => children.push(entry),
                Err(e) => log::warn!("{what}: in {}: {e}", dir.display()),
            }
        }
        children.sort_by_key(|e| e.file_name());
        for entry in children {
            let path = entry.path();
            let Some(file) = entry.file_name().to_str().map(str::to_string) else {
                log::warn!(
                    "{what}: {} has a name that is not text; passed over",
                    path.display()
                );
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // What a link names, else the entry itself.
            let (is_dir, is_file) = if file_type.is_symlink() {
                std::fs::metadata(&path).map_or((false, false), |m| (m.is_dir(), m.is_file()))
            } else {
                (file_type.is_dir(), file_type.is_file())
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
                        "{what}: {} is more than {} folders down; not read",
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
                                "{what}: {} is a folder already read; not read again",
                                path.display()
                            );
                        }
                    }
                    Err(e) => log::warn!("{what}: {} not read: {e}", path.display()),
                }
                continue;
            }
            let is_profile = path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case(kind.extension));
            // A device, a pipe, or a broken link named like a profile
            // is never opened: reading one may never end or start.
            if is_profile && !is_file {
                log::warn!(
                    "{what}: {} is not a regular file; passed over",
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
            let name = join(if kind.named_with_extension {
                &file
            } else {
                stem
            });
            if !is_name(&name) {
                log::warn!(
                    "{what}: {} has a name an edit cannot hold; passed over",
                    path.display()
                );
                continue;
            }
            if out.len() >= bounds.kept {
                log::warn!(
                    "{what}: {} profiles listed under {}; the rest are not",
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
