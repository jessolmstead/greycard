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
//!
//! A root can carry a name of the user's ("Archive" for a folder
//! called Photos), shown wherever the folder's own name would be.
//! The name is only a label: the root is its path everywhere, and
//! naming one never moves or changes it. The names sit in the file
//! beside the list, under a key of their own, so the list reads as
//! it did to a build that knows nothing of names.
//!
//! A root can also be marked as an archive (notes §197, §216): a NAS
//! or a mounted cloud folder that shoots are backed up to. The marks
//! are a list of paths under a key of their own, `archives`, and the
//! folder each source folder was backed up to under an archive under
//! another, `backups` (§219: the pairing is the folder's, not the
//! root's), with the order they were chosen in under a third,
//! `backup_order`, so a build that knows nothing of archives still
//! reads the file as a list of ordinary roots.

use std::collections::{HashMap, HashSet};
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
    /// The names the user gave, by root; a root with none goes by its
    /// folder's name.
    names: HashMap<PathBuf, String>,
    /// The roots marked as archives, in the order they were marked.
    archives: Vec<PathBuf>,
    /// The pairings (§219): each folder backed up (the view's folder,
    /// or the chosen frames' common folder) and the folder under an
    /// archive it went to, one an archive at most for each source
    /// folder, oldest chosen first.
    backups: Vec<Pairing>,
}

/// A folder here and the folder under an archive it was backed up to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    pub source: PathBuf,
    pub dest: PathBuf,
}

/// Where Back up's default destination came from (§219), in the order
/// they are tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupDefault {
    /// The folder's own pairing to this archive.
    Own,
    /// A folder above it was backed up to this archive under its own
    /// name: that pairing's destination with the folder's path under it.
    Under,
    /// A pairing's destination was used as a container (a folder backed
    /// up to the year folder itself): the container with the folder's
    /// own name in it. From a folder above this one, or from the most
    /// recent pairing of the same root.
    Inside,
    /// The parent of the most recent destination chosen from the same
    /// root to this archive, that destination named as its source, with
    /// the folder's own name.
    Sibling,
    /// `<archive>/<root's label>/<path under the root>`.
    Mirror,
}

/// A path with no trailing separator and no `.` or doubled separators
/// in it, as its components spell it: `/mnt/x/2026/` is `/mnt/x/2026`.
pub fn tidy(path: &Path) -> PathBuf {
    path.components().collect()
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
        // The names, when there are any: a name for a path that is not
        // a root, or one that is not text, is dropped rather than the
        // whole file taken for something else.
        if let Some(names) = value.get("names").and_then(|n| n.as_object()) {
            for (path, name) in names {
                if let Some(name) = name.as_str() {
                    roots.set_name(Path::new(path), name);
                }
            }
        }
        // The archives, the same way: a path that is not a root, or an
        // item that is not text, is dropped and the rest kept.
        if let Some(archives) = value.get("archives").and_then(|a| a.as_array()) {
            for path in archives.iter().filter_map(|p| p.as_str()) {
                roots.set_archive(Path::new(path), true);
            }
        }
        // The pairings, kept whatever the archives are now: turning the
        // mark off and on again finds the folder chosen before.
        if let Some(backups) = value.get("backups").and_then(|b| b.as_object()) {
            for (source, folders) in backups {
                for dest in folders.as_array().into_iter().flatten() {
                    if let Some(dest) = dest.as_str() {
                        let pairing = Pairing {
                            source: tidy(Path::new(source)),
                            dest: tidy(Path::new(dest)),
                        };
                        if !roots.backups.contains(&pairing) {
                            roots.backups.push(pairing);
                        }
                    }
                }
            }
            // The order they were chosen in, when the file has it (an
            // older build that saved has not kept it): those it names
            // last, in its order; any it does not name first, as read.
            if let Some(order) = value.get("backup_order").and_then(|o| o.as_array()) {
                for item in order {
                    let pair: Vec<&str> = item
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| p.as_str())
                        .collect();
                    let [source, dest] = pair[..] else {
                        continue;
                    };
                    let pairing = Pairing {
                        source: tidy(Path::new(source)),
                        dest: tidy(Path::new(dest)),
                    };
                    if let Some(at) = roots.backups.iter().position(|p| *p == pairing) {
                        let p = roots.backups.remove(at);
                        roots.backups.push(p);
                    }
                }
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
        let mut file = serde_json::json!({
            "version": VERSION,
            "roots": list,
        });
        // Written only when a root has a name, so a library with none
        // keeps the file it had. serde_json keeps an object's keys
        // sorted: the names come out by path, and after the list.
        if !self.names.is_empty() {
            let names: serde_json::Map<String, serde_json::Value> = self
                .names
                .iter()
                .map(|(p, name)| {
                    (
                        p.to_string_lossy().into_owned(),
                        serde_json::Value::String(name.clone()),
                    )
                })
                .collect();
            file["names"] = serde_json::Value::Object(names);
        }
        // The archives and the pairings likewise only when there are
        // any, so a library with none keeps its file byte for byte.
        if !self.archives.is_empty() {
            file["archives"] = serde_json::Value::Array(
                self.archives
                    .iter()
                    .map(|p| serde_json::Value::String(p.to_string_lossy().into_owned()))
                    .collect(),
            );
        }
        if !self.backups.is_empty() {
            let text = |p: &Path| serde_json::Value::String(p.to_string_lossy().into_owned());
            let mut backups = serde_json::Map::new();
            for p in &self.backups {
                let folders = backups
                    .entry(p.source.to_string_lossy().into_owned())
                    .or_insert_with(|| serde_json::Value::Array(Vec::new()));
                if let serde_json::Value::Array(folders) = folders {
                    folders.push(text(&p.dest));
                }
            }
            file["backups"] = serde_json::Value::Object(backups);
            // The object's keys come out sorted, so the order the
            // pairings were chosen in, which Back up's default reads
            // (§219), goes beside it: pairs of source and destination,
            // oldest first. An older build reads past it.
            file["backup_order"] = serde_json::Value::Array(
                self.backups
                    .iter()
                    .map(|p| serde_json::Value::Array(vec![text(&p.source), text(&p.dest)]))
                    .collect(),
            );
        }
        let text = serde_json::to_string_pretty(&file).map_err(std::io::Error::other)?;
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

    /// The name the user gave this root, if any.
    pub fn name(&self, root: &Path) -> Option<&str> {
        self.names.get(root).map(String::as_str)
    }

    /// What a root is called: the name the user gave it, else its
    /// folder's name, else (a disk's root, which has none) the whole
    /// path.
    pub fn label(&self, root: &Path) -> String {
        match self.name(root) {
            Some(name) => name.to_owned(),
            None => root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.to_string_lossy().into_owned()),
        }
    }

    /// Give a root a name, or with an empty one (spaces around it are
    /// not kept) take its name away, so it goes by its folder's again.
    /// The path is untouched. False when `root` is not a root, as the
    /// list spells it.
    pub fn set_name(&mut self, root: &Path, name: &str) -> bool {
        if !self.list.iter().any(|r| r == root) {
            return false;
        }
        let name = name.trim();
        if name.is_empty() {
            self.names.remove(root);
        } else {
            self.names.insert(root.to_path_buf(), name.to_owned());
        }
        true
    }

    /// Whether this root is marked as an archive.
    pub fn is_archive(&self, root: &Path) -> bool {
        self.archives.iter().any(|a| a == root)
    }

    /// The roots marked as archives, in the order they were marked.
    pub fn archives(&self) -> &[PathBuf] {
        &self.archives
    }

    /// The roots that are not archives: the local side of a backup.
    pub fn locals(&self) -> Vec<PathBuf> {
        self.list
            .iter()
            .filter(|r| !self.is_archive(r))
            .cloned()
            .collect()
    }

    /// The archive `path` is under, the archive itself included.
    pub fn archive_of(&self, path: &Path) -> Option<&Path> {
        self.archives
            .iter()
            .find(|a| path.starts_with(a))
            .map(PathBuf::as_path)
    }

    /// Mark a root as an archive, or take the mark away. Nothing on
    /// disk changes either way, and the pairings stay. False when
    /// `root` is not a root, as the list spells it.
    pub fn set_archive(&mut self, root: &Path, archive: bool) -> bool {
        if !self.list.iter().any(|r| r == root) {
            return false;
        }
        if archive {
            if !self.is_archive(root) {
                self.archives.push(root.to_path_buf());
            }
        } else {
            self.archives.retain(|a| a != root);
        }
        true
    }

    /// The folder under `archive` that the folder `source` was last
    /// backed up to, if it has been.
    pub fn backup_folder(&self, source: &Path, archive: &Path) -> Option<&Path> {
        self.backups
            .iter()
            .find(|p| p.source == source && p.dest.starts_with(archive))
            .map(|p| p.dest.as_path())
    }

    /// Every pairing, oldest chosen first.
    pub fn backups(&self) -> &[Pairing] {
        &self.backups
    }

    /// The pairings from a folder under `base` (a root, or a folder of
    /// no root's) to a folder under `archive`, oldest chosen first.
    pub fn backups_between<'a>(
        &'a self,
        base: &'a Path,
        archive: &'a Path,
    ) -> impl Iterator<Item = &'a Pairing> + 'a {
        self.backups
            .iter()
            .filter(move |p| p.source.starts_with(base) && p.dest.starts_with(archive))
    }

    /// Whether any folder under `base` has been backed up to `archive`:
    /// the pairing is the mark, and the header's count is shown for
    /// every folder under a root that has one (§217, §219).
    pub fn backed_up_under(&self, base: &Path, archive: &Path) -> bool {
        self.backups_between(base, archive).next().is_some()
    }

    /// Remember that the folder `source` backs up to `folder`, under
    /// `archive`, in place of the folder it had there, as the most
    /// recent choice. A trailing separator is not kept. False when
    /// `folder` is not under `archive`, or `archive` is not an archive.
    pub fn set_backup_folder(&mut self, source: &Path, archive: &Path, folder: &Path) -> bool {
        let (source, folder) = (tidy(source), tidy(folder));
        if !self.is_archive(archive) || !folder.starts_with(archive) {
            return false;
        }
        self.backups
            .retain(|p| !(p.source == source && p.dest.starts_with(archive)));
        self.backups.push(Pairing {
            source,
            dest: folder,
        });
        true
    }

    /// Forget one pairing. True when it was kept.
    pub fn drop_backup(&mut self, pairing: &Pairing) -> bool {
        let before = self.backups.len();
        self.backups.retain(|p| p != pairing);
        self.backups.len() != before
    }

    /// Whether a pairing's destination is its source folder under its
    /// own name (a shoot to `<year>/<shoot>`, a root to `<archive>/<its
    /// label>`), not a folder it was put into as a container (a shoot,
    /// or a root, to `<year>` itself).
    fn named_as_itself(&self, p: &Pairing) -> bool {
        let Some(last) = p.dest.file_name() else {
            return false;
        };
        p.source.file_name() == Some(last) || *self.label(&p.source) == *last
    }

    /// Back up's default destination for `folder`, under `base` (its
    /// root, or the folder of no root's it is in), to `archive` (§219),
    /// and where it came from, in order:
    /// - the folder's own pairing;
    /// - a folder above it that was backed up there: when that
    ///   destination is its source under its own name, the destination
    ///   with the folder's path under it; when it was a container (a
    ///   root backed up to the year folder itself), the container with
    ///   the folder's own name in it;
    /// - the most recent destination chosen from a folder under `base`
    ///   to `archive`, unless that source is under `folder` (a root
    ///   after one of its shoots): named as its source, its parent with
    ///   the folder's own name (a sibling); a container, the container
    ///   with the folder's own name in it;
    /// - the mirror, `<archive>/<base's label>/<path under base>`.
    pub fn backup_default(
        &self,
        folder: &Path,
        base: &Path,
        archive: &Path,
    ) -> (PathBuf, BackupDefault) {
        if let Some(own) = self.backup_folder(folder, archive) {
            return (own.to_path_buf(), BackupDefault::Own);
        }
        let name = folder.file_name();
        let above = self
            .backups_between(base, archive)
            .filter(|p| folder.starts_with(&p.source))
            .max_by_key(|p| p.source.components().count());
        if let Some(p) = above
            && let Ok(rel) = folder.strip_prefix(&p.source)
        {
            return match name {
                Some(name) if !self.named_as_itself(p) => {
                    (p.dest.join(name), BackupDefault::Inside)
                }
                _ => (p.dest.join(rel), BackupDefault::Under),
            };
        }
        if let Some(recent) = self.backups_between(base, archive).last()
            && !recent.source.starts_with(folder)
            && let Some(name) = name
        {
            if !self.named_as_itself(recent) {
                return (recent.dest.join(name), BackupDefault::Inside);
            }
            if let Some(parent) = recent.dest.parent().filter(|d| d.starts_with(archive)) {
                return (parent.join(name), BackupDefault::Sibling);
            }
        }
        let mirror = archive.join(self.label(base));
        let mirror = match folder.strip_prefix(base) {
            Ok(rel) if !rel.as_os_str().is_empty() => mirror.join(rel),
            _ => mirror,
        };
        (mirror, BackupDefault::Mirror)
    }

    /// Where Bring back may unwind the archive folder `from` to, under
    /// the local root `home` (§219): each pairing from a folder under
    /// `home` whose destination is `from` or a folder above it, the
    /// deepest destination first. One whose destination is `from`
    /// itself is exact; one above `from` holds it only when its source
    /// has the same folder under it, which is the disk's to say (a
    /// folder under a paired destination that was never itself backed
    /// up from there is not that pairing's).
    pub fn unwind(&self, from: &Path, home: &Path, archive: &Path) -> Vec<Pairing> {
        let mut found: Vec<Pairing> = self
            .backups_between(home, archive)
            .filter(|p| from.starts_with(&p.dest))
            .cloned()
            .collect();
        found.sort_by_key(|p| std::cmp::Reverse(p.dest.components().count()));
        found
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
        let dir = Roots::checked(dir)?;
        self.add_checked(dir)
    }

    /// A folder as [`Roots::add`] takes it: canonical, and a folder.
    /// This asks the disk, and a folder on a share that has stopped
    /// answering holds it for as long as the share is gone, so a
    /// window asks it on a thread of its own and hands the answer to
    /// [`Roots::add_checked`].
    pub fn checked(dir: &Path) -> Result<PathBuf, RootError> {
        let dir = canonical(dir).map_err(|e| RootError::Io(dir.to_path_buf(), e))?;
        if !dir.is_dir() {
            return Err(RootError::NotAFolder(dir));
        }
        Ok(dir)
    }

    /// Add a folder [`Roots::checked`] has made canonical, asking the
    /// disk nothing.
    pub fn add_checked(&mut self, dir: PathBuf) -> Result<Added, RootError> {
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
        self.names.retain(|r, _| !r.starts_with(&dir));
        self.archives.retain(|r| !r.starts_with(&dir));
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
    /// True when it was a root. A root named as the list has it is
    /// taken out without asking the disk, so a root on a share that
    /// has stopped answering can still be taken out; any other path is
    /// made canonical first.
    pub fn remove(&mut self, dir: &Path) -> bool {
        let canonical = if self.list.iter().any(|r| r == dir) {
            dir.to_path_buf()
        } else {
            canonical(dir).unwrap_or_else(|_| dir.to_path_buf())
        };
        let before = self.list.len();
        self.list.retain(|r| r != dir && *r != canonical);
        self.names.retain(|r, _| r != dir && *r != canonical);
        self.archives.retain(|r| r != dir && *r != canonical);
        // The pairings from folders under it go with it; those to
        // folders under it go too, since it is no archive now.
        let gone = |p: &Path| p.starts_with(dir) || p.starts_with(&canonical);
        self.backups.retain(|p| !gone(&p.source) && !gone(&p.dest));
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
    /// The folders under them left unwatched, with everything under
    /// each, as asked: a network mount below a local root.
    pub except: Vec<PathBuf>,
}

/// How long the disk has to be quiet before a batch is handed on,
/// and the longest a batch waits for that. A copy of a shoot is a
/// stream of events for seconds; the quiet lets it land first, and
/// the cap keeps a slow copy from hiding its first files for as long
/// as it runs.
pub const QUIET: Duration = Duration::from_millis(400);
pub const LONGEST: Duration = Duration::from_secs(5);

/// The folders under `dir` a watch can take, `dir` among them: not
/// hidden, not a link, readable, and not at or under one of `except`,
/// which is not gone into at all. Those that cannot be read come back
/// beside, with the reason.
#[allow(clippy::type_complexity)]
fn folders_under(dir: &Path, except: &[PathBuf]) -> (Vec<PathBuf>, Vec<(PathBuf, String)>) {
    let mut found = Vec::new();
    let mut unreadable = Vec::new();
    let left_out = |d: &Path| except.iter().any(|e| d.starts_with(e));
    if left_out(dir) {
        return (found, unreadable);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        match std::fs::read_dir(&d) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let hidden = entry.file_name().to_string_lossy().starts_with('.');
                    if !hidden
                        && entry.file_type().is_ok_and(|t| t.is_dir())
                        && !left_out(&entry.path())
                    {
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
        Self::start_except(roots, &[], quiet, longest, changed)
    }

    /// [`Watcher::start`], leaving each folder of `except` under a root
    /// unwatched, everything under it with it: a root with one in it is
    /// watched folder by folder, and the left-out folders are never
    /// gone into, not even to be listed. A network mount below a local
    /// root is one: a recursive watch would walk the share a round trip
    /// a folder, and see only this machine's changes there.
    #[allow(clippy::type_complexity)]
    pub fn start_except(
        roots: &[PathBuf],
        except: &[PathBuf],
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
        let except: Vec<PathBuf> = except
            .iter()
            .filter(|e| roots.iter().any(|r| e.starts_with(r) && *e != r))
            .cloned()
            .collect();
        for root in roots {
            let leaves_out = except.iter().any(|e| e.starts_with(root));
            if !leaves_out
                && watcher
                    .watch(root, notify::RecursiveMode::Recursive)
                    .is_ok()
            {
                watched.push(root.clone());
                continue;
            }
            // Taken back, whatever part of it the recursive watch
            // managed before it failed, and done a folder at a time.
            if !leaves_out {
                let _ = watcher.unwatch(root);
            }
            let (folders, unreadable) = folders_under(root, &except);
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
        let left_out = except.clone();
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
                    for folder in folders_under(dir, &left_out).0 {
                        let _ = w.watch(&folder, notify::RecursiveMode::NonRecursive);
                    }
                };
                debounce(
                    &roots_seen,
                    &left_out,
                    &events,
                    quiet,
                    longest,
                    &add,
                    &changed,
                )
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
                except,
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
/// a root being worked on do not hold a batch back. A change at or
/// under a folder of `except` is dropped: a root watched folder by
/// folder still hears the left-out folder's own entry change (Windows
/// reports a write inside it as a modify of it). `add` is told of
/// each folder that appears. It ends when the watcher is dropped.
fn debounce(
    roots: &[PathBuf],
    except: &[PathBuf],
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
                    let changes = event
                        .paths
                        .iter()
                        .filter_map(|p| change_for(roots, p))
                        .filter(|c| !except.iter().any(|e| c.path().starts_with(e)));
                    for change in changes {
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

    /// A root's name is kept beside its path and survives the file;
    /// the path is what it was. A file with no names is the file the
    /// previous build wrote, and one with names still reads to that
    /// build as the same list.
    #[test]
    fn a_root_keeps_its_name_beside_its_path() {
        let dir = scratch("roots-name");
        let (photos, b) = (dir.join("Photos"), dir.join("b"));
        std::fs::create_dir_all(&photos).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let path = dir.join(FILE_NAME);
        let mut roots = Roots::default();
        roots.add(&photos).unwrap();
        roots.add(&b).unwrap();
        assert_eq!(roots.label(&photos), "Photos");
        assert!(roots.set_name(&photos, "  Archive "));
        assert_eq!(roots.name(&photos), Some("Archive"));
        assert_eq!(roots.label(&photos), "Archive");
        assert_eq!(roots.label(&b), "b");
        assert!(!roots.set_name(&dir.join("nowhere"), "x"), "not a root");
        assert_eq!(roots.list(), [photos.clone(), b.clone()]);
        roots.save(&path).unwrap();
        let back = Roots::load(&path).unwrap();
        assert_eq!(back, roots);
        assert_eq!(back.label(&photos), "Archive");
        // The list itself is the one the previous build reads.
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(
            value["roots"],
            serde_json::json!([photos.to_str().unwrap(), b.to_str().unwrap()])
        );
        assert_eq!(value["names"][photos.to_str().unwrap()], "Archive");

        // Cleared, it goes by the folder again, and the file has no
        // names left in it.
        assert!(roots.set_name(&photos, ""));
        assert_eq!(roots.label(&photos), "Photos");
        roots.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("names"), "{text}");

        // The previous build's file, with no names, reads as roots
        // with none; a name for a path that is not a root, or one that
        // is not text, is dropped and the rest kept.
        let old = serde_json::json!({
            "version": 1,
            "roots": [photos.to_str().unwrap()],
            "names": { b.to_str().unwrap(): "Gone", photos.to_str().unwrap(): 7 },
        });
        std::fs::write(&path, old.to_string()).unwrap();
        let read = Roots::load(&path).unwrap();
        assert_eq!(read.list(), std::slice::from_ref(&photos));
        assert_eq!(read.name(&photos), None);
        assert_eq!(read.name(&b), None);

        // Removing a root, or adding one above it, takes its name with
        // it: a root added again later starts with none.
        roots.set_name(&photos, "Archive");
        roots.remove(&photos);
        roots.add(&photos).unwrap();
        assert_eq!(roots.name(&photos), None);
        roots.set_name(&b, "Other");
        roots.add(&dir).unwrap();
        assert_eq!(roots.name(&b), None);
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
        // As in the test above: FSEvents can name the root itself, a
        // tree over the folder the write was in, so a folder's change
        // is its own pass or a tree pass over it.
        let wait = |want: &dyn Fn(&Change) -> bool, what: &Path| {
            let until = Instant::now() + Duration::from_secs(10);
            let mut all = Vec::new();
            while Instant::now() < until {
                if let Ok(batch) = got.recv_timeout(Duration::from_millis(100)) {
                    all.extend(batch);
                    if all.iter().any(want) {
                        return;
                    }
                }
            }
            panic!("never saw {what:?}: {all:?}");
        };
        let reaches = |dir: &Path| {
            let dir = dir.to_path_buf();
            move |c: &Change| match c {
                Change::Folder(p) => *p == dir,
                Change::Tree(t) => dir.starts_with(t),
            }
        };
        std::fs::write(shoot.join("a.CR3"), b"not a raw").unwrap();
        wait(&reaches(&shoot), &shoot);
        let later = root.join("later");
        std::fs::create_dir_all(&later).unwrap();
        wait(
            &|c| matches!(c, Change::Tree(t) if later.starts_with(t)),
            &later,
        );
        std::thread::sleep(Duration::from_millis(300));
        // What the new folder said is settled; the write below must be
        // seen on its own.
        while got.try_recv().is_ok() {}
        std::fs::write(later.join("b.CR3"), b"not a raw").unwrap();
        wait(&reaches(&later), &later);
        drop(watcher);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A folder left out under a root (a network mount below a local
    /// root) is not watched, nor anything under it, nor a folder made
    /// in it later; the rest of the root is, its new folders too.
    #[test]
    fn a_folder_left_out_under_a_root_is_not_watched() {
        let dir = scratch("roots-watch-except");
        let root = dir.join("root");
        let (day, nas) = (root.join("day"), root.join("nas"));
        std::fs::create_dir_all(&day).unwrap();
        std::fs::create_dir_all(nas.join("2026")).unwrap();
        let (send, got) = mpsc::channel();
        let (watcher, failed) = Watcher::start_except(
            std::slice::from_ref(&root),
            std::slice::from_ref(&nas),
            Duration::from_millis(100),
            Duration::from_secs(2),
            move |changes| {
                let _ = send.send(changes);
            },
        );
        let watcher = watcher.expect("a watcher");
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(watcher.watched, std::slice::from_ref(&root));
        assert_eq!(watcher.except, std::slice::from_ref(&nas));
        let gather = |for_: Duration| {
            let until = Instant::now() + for_;
            let mut all = Vec::new();
            while Instant::now() < until {
                if let Ok(batch) = got.recv_timeout(Duration::from_millis(50)) {
                    all.extend(batch);
                }
            }
            all
        };
        // Under the left-out folder: nothing, however deep, and not in a
        // folder made there after the watch began.
        std::fs::write(nas.join("a.CR3"), b"x").unwrap();
        std::fs::write(nas.join("2026").join("b.CR3"), b"x").unwrap();
        std::fs::create_dir_all(nas.join("later")).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(nas.join("later").join("c.CR3"), b"x").unwrap();
        let seen = gather(Duration::from_millis(800));
        assert!(
            seen.iter().all(|c| !c.path().starts_with(&nas)),
            "nothing under the left-out folder: {seen:?}"
        );
        // The rest of the root: seen, a new folder's files too.
        std::fs::write(day.join("d.CR3"), b"x").unwrap();
        let seen = gather(Duration::from_millis(800));
        assert!(
            seen.iter().any(|c| day.starts_with(c.path())),
            "the rest is watched: {seen:?}"
        );
        let later = root.join("later");
        std::fs::create_dir_all(&later).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        gather(Duration::from_millis(300));
        std::fs::write(later.join("e.CR3"), b"x").unwrap();
        let seen = gather(Duration::from_millis(800));
        assert!(
            seen.iter().any(|c| later.starts_with(c.path())),
            "a new folder outside it is watched: {seen:?}"
        );
        drop(watcher);
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

    /// The archive mark and the pairings round-trip under keys of their
    /// own, the version stays 1, and the list reads as a plain list of
    /// roots: what a build that knows nothing of archives sees.
    #[test]
    fn the_archives_and_their_pairings_round_trip_beside_the_list() {
        let dir = scratch("roots-marks");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(nas.join("clients")).unwrap();
        let path = Roots::path_beside(&dir.join("library.sqlite"));
        let mut roots = Roots::default();
        roots.add(&local).unwrap();
        roots.add(&nas).unwrap();
        // A library with no archive keeps the file it had.
        roots.save(&path).unwrap();
        let plain = std::fs::read_to_string(&path).unwrap();
        assert!(!plain.contains("archives") && !plain.contains("backups"));

        assert!(!roots.set_archive(&dir.join("elsewhere"), true));
        assert!(
            !roots.set_backup_folder(&local, &nas, &nas.join("clients")),
            "no archive yet"
        );
        assert!(roots.set_archive(&nas, true));
        assert!(roots.is_archive(&nas) && !roots.is_archive(&local));
        assert_eq!(roots.locals(), std::slice::from_ref(&local));
        assert_eq!(
            roots.archive_of(&nas.join("x").join("a.CR3")),
            Some(nas.as_path())
        );
        assert!(
            !roots.set_backup_folder(&local, &nas, &dir.join("other")),
            "a folder not under the archive"
        );
        assert!(roots.set_backup_folder(&local, &nas, &nas.join("photos")));
        // Chosen again: the new folder takes the old one's place.
        assert!(roots.set_backup_folder(&local, &nas, &nas.join("clients")));
        assert_eq!(
            roots.backup_folder(&local, &nas),
            Some(nas.join("clients").as_path())
        );
        roots.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["version"], 1);
        // The list itself as an older build reads it: plain strings.
        let listed: Vec<&str> = value["roots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(listed, [local.to_str().unwrap(), nas.to_str().unwrap()]);
        assert_eq!(Roots::load(&path).unwrap(), roots);

        // An older shape, with neither key, reads as no archives.
        std::fs::write(
            &path,
            format!(
                r#"{{"version": 1, "roots": [{:?}, {:?}]}}"#,
                local.to_str().unwrap(),
                nas.to_str().unwrap()
            ),
        )
        .unwrap();
        let old = Roots::load(&path).unwrap();
        assert!(old.archives().is_empty());
        assert_eq!(old.backup_folder(&local, &nas), None);
        assert_eq!(old.list(), roots.list());

        // An archive that is not a root, and an item that is not text,
        // are dropped and the rest kept.
        std::fs::write(
            &path,
            format!(
                r#"{{"version": 1, "roots": [{:?}, {:?}], "archives": [{:?}, 7, "/not/a/root"]}}"#,
                local.to_str().unwrap(),
                nas.to_str().unwrap(),
                nas.to_str().unwrap()
            ),
        )
        .unwrap();
        assert_eq!(
            Roots::load(&path).unwrap().archives(),
            std::slice::from_ref(&nas)
        );

        // The mark off keeps the pairing; the archive taken out of the
        // library takes the mark and the folders under it.
        assert!(roots.set_archive(&nas, false));
        assert!(roots.archives().is_empty());
        roots.set_archive(&nas, true);
        assert!(roots.backup_folder(&local, &nas).is_some());
        assert!(roots.remove(&nas));
        assert!(roots.archives().is_empty());
        assert!(roots.backups().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Two roots, `local` and `nas`, the second an archive, in a scratch
    /// directory of the test's own.
    fn local_and_nas(what: &str) -> (PathBuf, PathBuf, PathBuf, Roots) {
        let dir = scratch(what);
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(&nas).unwrap();
        let mut roots = Roots::default();
        roots.add(&local).unwrap();
        roots.add(&nas).unwrap();
        roots.set_archive(&nas, true);
        (dir, local, nas, roots)
    }

    /// §219: a pairing is the folder's. Back up's default is the
    /// folder's own pairing, else a folder above it that was paired,
    /// else the sibling of the most recent destination from the same
    /// root, else the mirror; in that order.
    #[test]
    fn back_ups_default_is_the_folders_then_a_siblings_then_the_mirror() {
        let (dir, local, nas, mut roots) = local_and_nas("roots-defaults");
        let (a, b, c) = (
            local.join("2026").join("shoot-a"),
            local.join("2026").join("shoot-b"),
            local.join("2026").join("shoot-c"),
        );
        // Nothing paired: the mirror, the path under the root kept.
        assert_eq!(
            roots.backup_default(&a, &local, &nas),
            (
                nas.join("local").join("2026").join("shoot-a"),
                BackupDefault::Mirror
            )
        );
        assert_eq!(
            roots.backup_default(&local, &local, &nas),
            (nas.join("local"), BackupDefault::Mirror)
        );
        assert!(!roots.backed_up_under(&local, &nas));
        // One Back up to the archive's year folder: the next shoot opens
        // beside it, by its own name.
        assert!(roots.set_backup_folder(&a, &nas, &nas.join("2026").join("shoot-a")));
        assert!(roots.backed_up_under(&local, &nas));
        assert_eq!(
            roots.backup_default(&b, &local, &nas),
            (nas.join("2026").join("shoot-b"), BackupDefault::Sibling)
        );
        // The folder's own pairing comes first.
        assert_eq!(
            roots.backup_default(&a, &local, &nas),
            (nas.join("2026").join("shoot-a"), BackupDefault::Own)
        );
        // The most recent of the root's pairings names the sibling.
        assert!(roots.set_backup_folder(&b, &nas, &nas.join("clients").join("shoot-b")));
        assert_eq!(
            roots.backup_default(&c, &local, &nas),
            (nas.join("clients").join("shoot-c"), BackupDefault::Sibling)
        );
        // Chosen again, a pairing is the most recent again.
        assert!(roots.set_backup_folder(&a, &nas, &nas.join("2026").join("shoot-a")));
        assert_eq!(
            roots.backup_default(&c, &local, &nas).0,
            nas.join("2026").join("shoot-c")
        );
        // A folder inside one that was backed up goes inside its
        // destination.
        assert_eq!(
            roots.backup_default(&a.join("exports"), &local, &nas),
            (
                nas.join("2026").join("shoot-a").join("exports"),
                BackupDefault::Under
            )
        );
        // Another root's pairings are not this root's.
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        roots.add(&other).unwrap();
        assert_eq!(
            roots.backup_default(&other.join("x"), &other, &nas),
            (nas.join("other").join("x"), BackupDefault::Mirror)
        );

        // A root view after one of its shoots: the most recent source is
        // under the folder, so no sibling; the mirror.
        assert_eq!(
            roots.backup_default(&local, &local, &nas),
            (nas.join("local"), BackupDefault::Mirror)
        );
        // A selection whose common folder is above the last source: the
        // same.
        assert_eq!(
            roots.backup_default(&local.join("2026"), &local, &nas),
            (nas.join("local").join("2026"), BackupDefault::Mirror)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A destination that is not its source under its own name was a
    /// container: a shoot backed up to the year folder itself, the next
    /// shoot defaults into the year folder by its own name.
    #[test]
    fn a_destination_used_as_a_container_takes_the_next_shoot_inside() {
        let (dir, local, nas, mut roots) = local_and_nas("roots-container");
        let year = nas.join("2026");
        let (a, b) = (local.join("shoot-a"), local.join("shoot-b"));
        roots.set_backup_folder(&a, &nas, &year);
        assert_eq!(
            roots.backup_default(&b, &local, &nas),
            (year.join("shoot-b"), BackupDefault::Inside)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// §219's story as the user's file has it: a root backed up once with
    /// the field changed to the archive's year folder. A shoot of the
    /// root, at any depth, defaults into the year folder by its own name,
    /// not with its path under the root; and Bring back of another shoot
    /// under the year folder has the root's pairing only as a candidate
    /// for the disk to settle (the window's test settles it).
    #[test]
    fn a_root_backed_up_to_the_year_folder_is_a_container() {
        let (dir, local, nas, mut roots) = local_and_nas("roots-story");
        let year = nas.join("2026");
        roots.set_backup_folder(&local, &nas, &year);
        assert_eq!(
            roots.backup_default(&local.join("shoot-b"), &local, &nas),
            (year.join("shoot-b"), BackupDefault::Inside)
        );
        assert_eq!(
            roots.backup_default(&local.join("2026").join("shoot-c"), &local, &nas),
            (year.join("shoot-c"), BackupDefault::Inside)
        );
        // A root backed up under its own name keeps the paths under it.
        roots.set_backup_folder(&local, &nas, &nas.join("local"));
        assert_eq!(
            roots.backup_default(&local.join("2026").join("shoot-c"), &local, &nas),
            (
                nas.join("local").join("2026").join("shoot-c"),
                BackupDefault::Under
            )
        );
        roots.set_backup_folder(&local, &nas, &year);
        let found = roots.unwind(&year.join("never-here"), &local, &nas);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source, local);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A destination typed with a trailing separator is kept without
    /// it, and found by the path as it is spelled without.
    #[test]
    fn a_trailing_separator_is_not_kept() {
        let (dir, local, nas, mut roots) = local_and_nas("roots-slash");
        let shoot = local.join("shoot");
        let typed = format!("{}/", nas.join("2026").display());
        assert!(roots.set_backup_folder(&shoot, &nas, Path::new(&typed)));
        assert_eq!(
            roots.backups()[0].dest.to_string_lossy(),
            nas.join("2026").to_string_lossy()
        );
        let path = Roots::path_beside(&dir.join("library.sqlite"));
        roots.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(&typed), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Bring back unwinds a pairing whose destination is the archive
    /// folder or one above it, from the local root chosen, the deepest
    /// first; a shoot under the paired year folder is a candidate only,
    /// for the disk to settle.
    #[test]
    fn bring_back_unwinds_the_pairings_over_the_folder_deepest_first() {
        let (dir, local, nas, mut roots) = local_and_nas("roots-unwind");
        let year = nas.join("2026");
        roots.set_backup_folder(&local.join("shoot-a"), &nas, &year);
        roots.set_backup_folder(&local.join("shoot-b"), &nas, &year.join("b"));
        let found = roots.unwind(&year.join("b").join("exports"), &local, &nas);
        assert_eq!(
            found.iter().map(|p| p.dest.clone()).collect::<Vec<_>>(),
            [year.join("b"), year.clone()]
        );
        assert_eq!(
            roots.unwind(&year, &local, &nas)[0].source,
            local.join("shoot-a")
        );
        assert!(roots.unwind(&nas.join("2025"), &local, &nas).is_empty());
        // Not from another root.
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        roots.add(&other).unwrap();
        assert!(roots.unwind(&year, &other, &nas).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The pairings round-trip with their order; an older file of the
    /// same shape without the order reads, its pairings in the file's
    /// order; one pairing is dropped alone; and taking out a root drops
    /// the pairings from folders under it, an archive those to it.
    #[test]
    fn the_pairings_keep_their_order_and_go_with_their_roots() {
        let (dir, local, nas, mut roots) = local_and_nas("roots-order");
        let cloud = dir.join("cloud");
        std::fs::create_dir_all(&cloud).unwrap();
        roots.add(&cloud).unwrap();
        roots.set_archive(&cloud, true);
        let (z, a) = (local.join("z-shoot"), local.join("a-shoot"));
        // Chosen z first, then a: the order is not the paths'.
        roots.set_backup_folder(&z, &nas, &nas.join("z"));
        roots.set_backup_folder(&a, &nas, &nas.join("a"));
        roots.set_backup_folder(&a, &cloud, &cloud.join("a"));
        let path = Roots::path_beside(&dir.join("library.sqlite"));
        roots.save(&path).unwrap();
        let back = Roots::load(&path).unwrap();
        assert_eq!(back, roots);
        assert_eq!(
            back.backup_default(&local.join("next"), &local, &nas).0,
            nas.join("a").join("next"),
            "into the most recent, a, not z"
        );
        // The shape §217 wrote, keyed by the folder, with no order.
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            value["backups"][a.to_str().unwrap()],
            serde_json::json!([
                nas.join("a").to_str().unwrap(),
                cloud.join("a").to_str().unwrap()
            ])
        );
        let mut older = value.clone();
        older.as_object_mut().unwrap().remove("backup_order");
        std::fs::write(&path, older.to_string()).unwrap();
        let old = Roots::load(&path).unwrap();
        assert_eq!(old.backups().len(), 3);
        assert_eq!(old.backup_folder(&z, &nas), Some(nas.join("z").as_path()));
        // A bad order item is passed over.
        let mut bad = value.clone();
        bad["backup_order"] =
            serde_json::json!([7, ["only one"], [z.to_str().unwrap(), "/not/kept"]]);
        std::fs::write(&path, bad.to_string()).unwrap();
        assert_eq!(Roots::load(&path).unwrap().backups().len(), 3);

        // One dropped, the rest kept.
        let gone = roots.backups()[0].clone();
        assert!(roots.drop_backup(&gone));
        assert!(!roots.drop_backup(&gone));
        assert_eq!(roots.backups().len(), 2);
        // The archive out: the pairings to it go, the cloud's stays.
        assert!(roots.remove(&nas));
        assert_eq!(roots.backups().len(), 1);
        assert_eq!(roots.backups()[0].dest, cloud.join("a"));
        // The root out: the pairings from folders under it go.
        assert!(roots.remove(&local));
        assert!(roots.backups().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
