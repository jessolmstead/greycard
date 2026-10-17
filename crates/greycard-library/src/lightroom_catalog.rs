//! A Lightroom Classic catalog brought across: the metadata exactly,
//! the develop settings as an approximation, the collections and their
//! sets, the smart collections whose rules the filter language has,
//! and the root folders as the library's roots.
//!
//! An `.lrcat` is a SQLite database, opened here read-only and never
//! written; Lightroom locks it while it runs, so the import refuses a
//! catalog with Lightroom's `.lock` beside it and is meant for a copy.
//! Lightroom keeps it in write-ahead mode, the newest changes in the
//! `-wal` beside it until it quits: with a log that holds changes the
//! two are read together, and without one the catalog is opened
//! immutable, so a dry run makes no file beside it ([`Wal`]).
//! The tables have barely moved in years, but a schema this import was
//! not tested against is refused by its number ([`KNOWN`]) rather than
//! read on a guess.
//!
//! **Where each piece lands.** The rating, the pick flag, the color
//! label, the keywords (a keyword under others as the slash path the
//! meta keeps, `Places/Scotland/Skye`), the title and the caption go
//! to the sidecar's meta as the catalog has them. The develop settings
//! are the catalog's Lua table of Camera Raw keys, mapped by
//! [`greycard_edit::lightroom::read_table`] and recorded as one named
//! state over the frame's starting edit, for the user to see and
//! redo; what the mapper has no place for is counted by name. A
//! virtual copy becomes a version of its file under the copy's name;
//! the meta is the file's, so a copy rated or labeled apart from its
//! master keeps the master's, and is counted. Collections and sets go
//! to the collections file under a set named for the catalog, a smart
//! collection when every rule has a term in the filter language, and
//! anything else is named in the report with why; Lightroom's six stock
//! smart collections, as every catalog has them, are counted and left
//! out. Stacks are counted and not carried.
//!
//! **Where the files are now.** A root folder's path is the one it had
//! on the machine the catalog lived on, so each is mapped to where it
//! is now (`--root A:/Photos=/photos`, the catalog's path or a folder
//! above it, or the root's name), and a mapping is taken only when
//! some of the catalog's files are found under it. A root not mapped
//! is looked for where the catalog says it is.
//!
//! **A dry run first.** [`import`] without `write` reads everything,
//! looks at the disk and the sidecars already there, and reports what
//! a write would do; it writes nothing. With `write`, the sidecars are
//! written through the sidecar's own save, the collections through
//! theirs, and the roots through the roots file's; a frame whose
//! sidecar already holds history, versions or meta is left alone
//! unless `force`.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use greycard_edit::lightroom::{self, Value};
use greycard_edit::meta::{Flag, Label, Meta, STARS};
use greycard_edit::{Edit, Placement, Sidecar};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::collections::{self, Collection, Smart};
use crate::filter::{Filter, Saved};
use crate::roots::{self, Roots};

/// The catalog schemas this import was tested against: the number
/// the catalog's `Adobe_DBVersion` holds, and which Lightroom wrote the
/// catalogs it was tested on.
pub const KNOWN: [(i64, &str); 2] = [
    (1_500_000, "Lightroom Classic 15.2 and 15.3"),
    (1_504_001, "Lightroom Classic 15.4 and 15.5"),
];

/// How many of a root's files are looked for to take a mapping.
const CHECKED: usize = 5;

/// The process version from which the sliders are the current ones:
/// Camera Raw's 2012 process. Settings from before it, the 2003 and
/// 2010 processes, map through the same keys and are further off.
const PROCESS_2012: f32 = 6.7;

/// Why a catalog could not be imported.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("{path}: {source}")]
    Open {
        path: PathBuf,
        source: rusqlite::Error,
    },
    #[error("{0}: Lightroom has it open (the .lock beside it); quit Lightroom, or import a copy")]
    Locked(PathBuf),
    #[error("{0} is not a Lightroom catalog: it has no Adobe_DBVersion")]
    NotACatalog(PathBuf),
    #[error(
        "{path} is catalog schema {found}, which this import was not tested against; \
         it knows {known}"
    )]
    Unknown {
        path: PathBuf,
        found: i64,
        known: String,
    },
    #[error("reading the catalog: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("--root {0}: {1}")]
    Root(String, String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// What the import is asked to do.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Where the catalog's roots are now: a root's path as the catalog
    /// has it, a folder above it, or its name, and the folder here.
    pub roots: Vec<(String, PathBuf)>,
    /// Write; without it, a dry run.
    pub write: bool,
    /// Write over a sidecar that has history, versions or meta.
    pub force: bool,
    /// The set the collections go under; none for the catalog's name,
    /// an empty one for none at all.
    pub set: Option<String>,
    /// The library: the roots file and the collections are beside it.
    pub library: PathBuf,
    /// The editor's setting for where a new sidecar goes: under the
    /// folder's `.greycard` rather than beside the file. A folder that
    /// has a `.greycard` already gets its new sidecars there either way.
    pub sidecars_in_folder: bool,
}

/// A root folder of the catalog.
#[derive(Debug, Clone)]
struct CatalogRoot {
    id: i64,
    /// As the catalog has it: absolute, on the machine it lived on.
    path: String,
    name: String,
}

/// One image of the catalog: a file's own, or a virtual copy of it.
#[derive(Debug, Clone)]
struct Image {
    id: i64,
    file: i64,
    root: i64,
    /// The file under its root, slash-separated.
    relative: String,
    /// The image it is a virtual copy of.
    master: Option<i64>,
    copy_name: Option<String>,
    meta: Meta,
    /// A label whose text is no color's (a renamed label set).
    odd_label: Option<String>,
    develop: Option<String>,
    process: Option<String>,
    edited: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Plain,
    Smart(String),
    Set,
    /// A book, print, slideshow or web page: its frames are a
    /// collection's, its layout is not carried.
    Creation(String),
}

#[derive(Debug, Clone)]
struct Node {
    id: i64,
    name: String,
    parent: Option<i64>,
    kind: Kind,
    system: bool,
    members: Vec<i64>,
}

/// The catalog as read.
struct Catalog {
    schema: i64,
    written_by: &'static str,
    roots: Vec<CatalogRoot>,
    images: Vec<Image>,
    nodes: Vec<Node>,
    stacks: usize,
    wal: Wal,
}

/// One root of the catalog: where it is now and what was found there.
#[derive(Debug, Clone, PartialEq)]
pub struct RootReport {
    /// As the catalog has it.
    pub catalog: String,
    pub name: String,
    /// Where it was looked for, and whether a `--root` said so.
    pub local: Option<PathBuf>,
    pub given: bool,
    /// Of the first few files looked for, how many were there.
    pub checked: usize,
    pub checked_found: usize,
    /// Whether the mapping was taken: some of its files are there.
    pub confirmed: bool,
    pub files: usize,
    pub found: usize,
}

/// A collection as it is to land.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionReport {
    /// Its name with the sets it is in, as typed (`Sets/Name`).
    pub name: String,
    pub frames: usize,
    pub found: usize,
    /// A book, print, slideshow or web page's kind, for one of those.
    pub creation: Option<String>,
}

/// What was written, with `write`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Written {
    pub sidecars: usize,
    /// Of them, those under their folder's `.greycard`.
    pub in_folder: usize,
    pub left_alone: usize,
    pub failed: Vec<(PathBuf, String)>,
    pub roots: Vec<(PathBuf, String)>,
    pub collections: usize,
    /// Collections of the same name there already: frames added to
    /// them, or all there and the file left as it was.
    pub joined: usize,
    pub collections_there: usize,
    pub smart: usize,
    pub smart_there: usize,
    /// A virtual copy in a collection whose version is not in the
    /// sidecar (left alone without one): the frame's own taken instead.
    pub copies_as_own: usize,
}

/// What an import found, and what it would do or did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub catalog: PathBuf,
    pub schema: i64,
    pub written_by: String,
    pub roots: Vec<RootReport>,
    pub images: usize,
    pub files: usize,
    pub copies: usize,
    pub found: usize,
    pub missing: usize,
    pub with_edits: usize,
    /// Of them, those whose file was found: what a write carries.
    pub with_edits_found: usize,
    /// What the develop settings held that has no place here, by name,
    /// and on how many images.
    pub not_carried: BTreeMap<String, usize>,
    pub unreadable_develop: usize,
    /// Edits on the 2003 or 2010 process, further from Lightroom's.
    pub old_process: usize,
    pub with_meta: usize,
    pub with_meta_found: usize,
    pub odd_labels: BTreeMap<String, usize>,
    pub copies_found: usize,
    /// Copies whose meta differs from their master's.
    pub copies_apart: usize,
    pub keywords: usize,
    pub keyworded: usize,
    pub stacks: usize,
    pub sets: usize,
    pub collections: Vec<CollectionReport>,
    /// Each smart collection translated, as typed, with its filter.
    pub smart: Vec<(String, String)>,
    /// Each one not, with why.
    pub smart_skipped: Vec<(String, String)>,
    /// Lightroom's own six as every catalog has them, left out.
    pub smart_stock: usize,
    /// Where a new sidecar goes, as the setting says.
    pub sidecars_in_folder: bool,
    /// What the catalog's write-ahead log says of how it was read.
    pub wal: Wal,
    /// Frames whose sidecar has history, versions or meta already.
    pub already: usize,
    pub unreadable_sidecars: Vec<(PathBuf, String)>,
    pub written: Option<Written>,
}

/// How the catalog's write-ahead log stood when it was read. Lightroom
/// keeps its catalogs in SQLite's write-ahead mode: the newest changes
/// sit in `<catalog>-wal` until Lightroom quits, so a catalog copied
/// without it while Lightroom ran is missing them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Wal {
    /// Not in write-ahead mode, or its log there and empty: all of it is
    /// in the catalog.
    #[default]
    Whole,
    /// In write-ahead mode with no log beside it: whole when Lightroom
    /// had quit before it was copied, else missing the newest changes.
    Absent,
    /// Its log beside it with changes in it, read with them.
    Read,
}

/// Read the catalog at `path` and report what importing it would do;
/// with `options.write`, do it.
pub fn import(path: &Path, options: &Options) -> Result<Report, ImportError> {
    let catalog = read(path)?;
    let (roots, local_roots) = map_roots(&catalog, &options.roots)?;
    let mut report = Report {
        catalog: path.to_path_buf(),
        schema: catalog.schema,
        written_by: catalog.written_by.to_string(),
        images: catalog.images.len(),
        stacks: catalog.stacks,
        sidecars_in_folder: options.sidecars_in_folder,
        wal: catalog.wal,
        ..Report::default()
    };
    // Where each image's file is, when its root is mapped and it is
    // there.
    let mut at: HashMap<i64, PathBuf> = HashMap::new();
    let mut files: HashMap<i64, bool> = HashMap::new();
    for image in &catalog.images {
        let Some(local) = local_roots.get(&image.root) else {
            files.entry(image.file).or_insert(false);
            continue;
        };
        let path = under(local, &image.relative);
        let there = *files.entry(image.file).or_insert_with(|| path.exists());
        if there {
            at.insert(image.id, path);
        }
    }
    report.roots = roots
        .into_iter()
        .map(|mut r| {
            let id = catalog
                .roots
                .iter()
                .find(|c| c.path == r.catalog)
                .map(|c| c.id);
            let of_root = catalog
                .images
                .iter()
                .filter(|i| Some(i.root) == id && i.master.is_none());
            r.files = of_root.clone().count();
            r.found = of_root.filter(|i| at.contains_key(&i.id)).count();
            r
        })
        .collect();
    report.files = files.len();
    report.copies = catalog.images.iter().filter(|i| i.master.is_some()).count();
    report.found = at.len();
    report.missing = report.images - report.found;
    let by_id: HashMap<i64, &Image> = catalog.images.iter().map(|i| (i.id, i)).collect();
    let mut keywords: std::collections::HashSet<String> = std::collections::HashSet::new();
    for image in &catalog.images {
        let found = at.contains_key(&image.id);
        if image.edited {
            report.with_edits += 1;
            report.with_edits_found += usize::from(found);
            if image
                .process
                .as_deref()
                .and_then(|p| p.trim().parse::<f32>().ok())
                .is_some_and(|p| p < PROCESS_2012)
            {
                report.old_process += 1;
            }
            match image
                .develop
                .as_deref()
                .map(|t| lightroom::read_table(t, ""))
            {
                Some(Ok(imported)) => {
                    for name in imported.unmapped {
                        *report.not_carried.entry(name).or_default() += 1;
                    }
                }
                _ => report.unreadable_develop += 1,
            }
        }
        if let Some(label) = &image.odd_label {
            *report.odd_labels.entry(label.clone()).or_default() += 1;
        }
        match image.master.and_then(|m| by_id.get(&m)) {
            Some(master) => {
                report.copies_found += usize::from(found);
                if master.meta != image.meta {
                    report.copies_apart += 1;
                }
            }
            None => {
                if !image.meta.is_empty() {
                    report.with_meta += 1;
                    report.with_meta_found += usize::from(found);
                }
                if !image.meta.keywords.is_empty() {
                    report.keyworded += 1;
                }
                keywords.extend(image.meta.keywords.iter().map(|k| k.to_lowercase()));
            }
        }
    }
    report.keywords = keywords.len();

    // The sidecars there already, a frame's own image standing for
    // its file.
    let mut kept: HashMap<i64, bool> = HashMap::new();
    for image in catalog.images.iter().filter(|i| i.master.is_none()) {
        let Some(raw) = at.get(&image.id) else {
            continue;
        };
        match Sidecar::load(raw) {
            Ok(Some(s)) if has_content(&s) => {
                report.already += 1;
                kept.insert(image.file, true);
            }
            Ok(_) => {}
            Err(e) => {
                report
                    .unreadable_sidecars
                    .push((raw.clone(), e.to_string()));
                kept.insert(image.file, true);
            }
        }
    }

    // The collections, under the set named for the catalog.
    let top = match &options.set {
        Some(set) => set.trim().trim_matches('/').to_string(),
        None => path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    // A smart collection's scope is a root as the roots file keeps it:
    // canonical, or the editor's view of it would not match.
    let one_root = (local_roots.len() == 1 && catalog.roots.len() == 1)
        .then(|| local_roots.values().next())
        .flatten()
        .map(|p| crate::canonical(p).unwrap_or_else(|_| p.clone()));
    let mut plain: Vec<(&Node, String)> = Vec::new();
    let mut smart: Vec<(String, Saved)> = Vec::new();
    for node in &catalog.nodes {
        let quick = node.system && node.kind == Kind::Plain;
        let own = if quick {
            "Quick Collection".to_string()
        } else {
            node.name.replace('/', "-")
        };
        let name = collections::full_name(&set_of(&catalog.nodes, node, &top), &own);
        match &node.kind {
            Kind::Set => report.sets += 1,
            Kind::Plain | Kind::Creation(_) => {
                if quick && node.members.is_empty() {
                    continue;
                }
                report.collections.push(CollectionReport {
                    name: name.clone(),
                    frames: node.members.len(),
                    found: node.members.iter().filter(|m| at.contains_key(m)).count(),
                    creation: match &node.kind {
                        Kind::Creation(k) => Some(k.clone()),
                        _ => None,
                    },
                });
                plain.push((node, name));
            }
            Kind::Smart(rules) if is_stock(&node.name, rules) => report.smart_stock += 1,
            Kind::Smart(rules) => match translate(rules) {
                Ok(text) => {
                    report.smart.push((name.clone(), text.clone()));
                    smart.push((
                        name,
                        Saved {
                            text,
                            ..Saved::default()
                        },
                    ));
                }
                Err(why) => report.smart_skipped.push((name, why)),
            },
        }
    }

    if !options.write {
        return Ok(report);
    }
    let mut written = Written::default();

    // The sidecars: each file once, its own image and its copies.
    let mut versions: HashMap<i64, Option<String>> = HashMap::new();
    for image in catalog.images.iter().filter(|i| i.master.is_none()) {
        let Some(raw) = at.get(&image.id) else {
            continue;
        };
        let copies: Vec<&Image> = catalog
            .images
            .iter()
            .filter(|c| c.master == Some(image.id))
            .collect();
        if kept.get(&image.file).copied().unwrap_or(false) && !options.force {
            written.left_alone += 1;
            // A re-import's copies are the versions it made last time,
            // found by their names.
            if let Ok(Some(s)) = Sidecar::load(raw) {
                for c in &copies {
                    let name = copy_name(c);
                    versions.insert(
                        c.id,
                        s.versions
                            .iter()
                            .find(|v| v.name == name)
                            .map(|v| v.id.clone()),
                    );
                }
            }
            continue;
        }
        match write_frame(raw, image, &copies, options.sidecars_in_folder) {
            Ok((wrote, made)) => {
                if let Some(placement) = wrote {
                    written.sidecars += 1;
                    if placement == Placement::Folder {
                        written.in_folder += 1;
                    }
                }
                for (id, version) in made {
                    versions.insert(id, Some(version));
                }
            }
            Err(e) => written.failed.push((raw.clone(), e)),
        }
    }

    // The collections, joined to one of the same name already there.
    let dir = collections::dir_beside(&options.library);
    let mut there = collections::read_folder(&dir, collections::VERSION);
    let mut hashes: HashMap<PathBuf, Option<String>> = HashMap::new();
    for (node, name) in plain {
        let mut frames: Vec<collections::Frame> = Vec::new();
        for member in &node.members {
            let Some(image) = by_id.get(member) else {
                continue;
            };
            let own = image.master.unwrap_or(image.id);
            let Some(raw) = at.get(&own) else {
                continue;
            };
            let hash = hashes
                .entry(raw.clone())
                .or_insert_with(|| crate::hash_file(raw).ok())
                .clone();
            let Some(hash) = hash else {
                continue;
            };
            let version = if image.master.is_some() {
                let v = versions.get(&image.id).cloned().flatten();
                if v.is_none() {
                    written.copies_as_own += 1;
                }
                v
            } else {
                None
            };
            frames.push((hash, crate::key_path(raw), version));
        }
        // One of the same name there already, or made earlier in this
        // run (two of one name in the catalog), is joined.
        let (set, bare) = collections::split_name(&name);
        match there
            .list
            .iter_mut()
            .find(|c| c.set == set && c.name == bare)
        {
            Some(c) => {
                let added = c.add(&frames);
                // One there already with every frame is left as it is.
                if added.added + added.moved > 0 {
                    c.write(&dir)?;
                    written.joined += 1;
                } else {
                    written.collections_there += 1;
                }
            }
            None => {
                let mut c = Collection::new(&name);
                c.add(&frames);
                c.write(&dir)?;
                there.list.push(c);
                written.collections += 1;
            }
        }
    }
    for (name, filter) in smart {
        let (set, bare) = collections::split_name(&name);
        if there.smart.iter().any(|s| s.set == set && s.name == bare) {
            written.smart_there += 1;
            continue;
        }
        let s = Smart::new(&name, filter, one_root.clone());
        s.write(&dir)?;
        there.smart.push(s);
        written.smart += 1;
    }

    // The roots, through the library's list.
    let confirmed: Vec<PathBuf> = report
        .roots
        .iter()
        .filter(|r| r.confirmed)
        .filter_map(|r| r.local.clone())
        .collect();
    let roots_file = Roots::path_beside(&options.library);
    let canonical: Vec<PathBuf> = confirmed
        .iter()
        .map(|dir| crate::canonical(dir).unwrap_or_else(|_| dir.clone()))
        .collect();
    // Every one a root already, or under one: the file is not written.
    let list = Roots::load(&roots_file)?;
    if canonical.iter().all(|d| list.root_of(d).is_some()) {
        written.roots = canonical
            .iter()
            .filter_map(|d| {
                let by = list.root_of(d)?;
                let said = if by == d {
                    "a root already".to_string()
                } else {
                    format!("under {}", by.display())
                };
                Some((d.clone(), said))
            })
            .collect();
    } else {
        let (_, added) = Roots::edit(&roots_file, |list| {
            canonical
                .iter()
                .map(|dir| {
                    let dir = dir.clone();
                    let said = match list.add(&dir) {
                        Ok(roots::Added::New(_)) => "added".to_string(),
                        Ok(roots::Added::Covered(by)) if by == dir => "a root already".into(),
                        Ok(roots::Added::Covered(by)) => format!("under {}", by.display()),
                        Ok(roots::Added::Absorbed(_, under)) => {
                            format!("added, taking in {} roots under it", under.len())
                        }
                        Err(e) => format!("not added: {e}"),
                    };
                    (dir, said)
                })
                .collect::<Vec<_>>()
        })?;
        written.roots = added;
    }
    report.written = Some(written);
    Ok(report)
}

/// Whether a sidecar holds anything an import would write over.
fn has_content(s: &Sidecar) -> bool {
    !s.history.is_empty() || !s.versions.is_empty() || !s.meta.is_empty()
}

/// A virtual copy's name as its version takes it.
fn copy_name(image: &Image) -> String {
    image
        .copy_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("Copy")
        .to_string()
}

/// The edit a frame nobody has developed starts from.
fn starting_edit(raw: &Path) -> Edit {
    if greycard_core::picture::is_picture_path(raw) {
        return Edit::for_picture();
    }
    let iso = greycard_core::decode::probe_path(raw)
        .ok()
        .and_then(|p| p.iso);
    let mut edit = Edit::default();
    edit.noise.learned_strength = greycard_edit::Noise::blend_for_iso(iso);
    edit
}

/// The catalog's develop settings for `image` laid over `base`; none
/// when the image was never developed or the settings will not read.
fn developed(image: &Image, base: &Edit) -> Option<Edit> {
    if !image.edited {
        return None;
    }
    let imported = lightroom::read_table(image.develop.as_deref()?, "").ok()?;
    Some(imported.preset.applied(base))
}

/// Where a frame's sidecar goes: where it is, when it has one; else
/// under the folder's `.greycard` when the folder has one, so a shoot
/// whose sidecars live there keeps them together; else as the editor's
/// setting says.
fn placement_for(raw: &Path, in_folder: bool) -> Placement {
    if Sidecar::find(raw).is_some() {
        return Placement::of(raw);
    }
    if in_folder || raw.with_file_name(greycard_edit::SIDECAR_FOLDER).is_dir() {
        Placement::Folder
    } else {
        Placement::Beside
    }
}

/// Where a frame's sidecar was written, none when nothing was, and each
/// virtual copy's version id by the copy's image.
type FrameWritten = (Option<Placement>, Vec<(i64, String)>);

/// One file's sidecar written: the meta, the develop as a named state,
/// each copy as a version. Where it was written when it was, and each
/// copy's version id.
fn write_frame(
    raw: &Path,
    image: &Image,
    copies: &[&Image],
    in_folder: bool,
) -> Result<FrameWritten, String> {
    let found = Sidecar::load(raw).map_err(|e| e.to_string())?;
    let starting = starting_edit(raw);
    let mut sidecar = found.unwrap_or_else(|| Sidecar {
        line: greycard_edit::Line::of(starting.clone()),
        ..Sidecar::default()
    });
    // A sidecar from before schema 4 does not know which way up an
    // `Original` crop goes until the frame says, as `presets` asks.
    if sidecar.needs_frame()
        && let Some((w, h)) = greycard_core::decode::stance_path(raw)
            .ok()
            .and_then(|s| s.shown_size(sidecar.turn))
    {
        sidecar.migrate_with_frame(h > w);
    }
    let mut changed = false;
    if sidecar.meta != image.meta {
        sidecar.meta = image.meta.clone();
        changed = true;
    }
    let label = greycard_edit::history::import_label("Lightroom");
    if let Some(edit) = developed(image, &sidecar.current) {
        changed |= sidecar.record_as(edit, Some(label.clone()));
    }
    let mut made = Vec::new();
    // A version of the copy's name there already is the copy, as a
    // forced import made it before: its line takes the settings again
    // rather than a twin being made. Each is taken once, so two copies
    // of one name stay two.
    let mut there: Vec<(String, String)> = sidecar
        .versions
        .iter()
        .map(|v| (v.name.clone(), v.id.clone()))
        .collect();
    for copy in copies {
        let name = copy_name(copy);
        let id = match there.iter().position(|(n, _)| *n == name) {
            Some(at) => there.remove(at).1,
            None => {
                changed = true;
                sidecar.new_version_from_original(starting.clone(), Some(&name))
            }
        };
        if let Some(line) = sidecar.line_mut(Some(&id))
            && let Some(edit) = developed(copy, &line.current)
        {
            changed |= line.record_as(edit, Some(label.clone()));
        }
        made.push((copy.id, id));
    }
    if !changed {
        // Nothing to say about the frame: no sidecar made for it, and
        // one there left as it was.
        return Ok((None, made));
    }
    let placement = placement_for(raw, in_folder);
    sidecar.save_in(raw, placement).map_err(|e| e.to_string())?;
    Ok((Some(placement), made))
}

/// A file under a folder, the relative path slash-separated as the
/// catalog keeps it.
fn under(dir: &Path, relative: &str) -> PathBuf {
    let mut out = dir.to_path_buf();
    for part in relative.split('/').filter(|p| !p.is_empty()) {
        out.push(part);
    }
    out
}

/// The sets a node sits in, under `top`, as a slash path.
fn set_of(nodes: &[Node], node: &Node, top: &str) -> String {
    let mut chain: Vec<&str> = Vec::new();
    let mut parent = node.parent;
    // A cycle in a damaged catalog stops at the count of nodes.
    for _ in 0..nodes.len() {
        let Some(p) = parent.and_then(|id| nodes.iter().find(|n| n.id == id)) else {
            break;
        };
        chain.push(&p.name);
        parent = p.parent;
    }
    chain.reverse();
    let mut parts: Vec<&str> = Vec::new();
    if !top.is_empty() {
        parts.push(top);
    }
    parts.extend(chain);
    parts
        .iter()
        .map(|p| p.replace('/', "-"))
        .collect::<Vec<_>>()
        .join("/")
}

/// A path's components as the catalog's paths are compared: slashes
/// either way, empty pieces gone, and the case folded for a Windows
/// path, whose file system folds it.
fn components(path: &str) -> Vec<String> {
    let windows = path.as_bytes().get(1) == Some(&b':') || path.starts_with("\\\\");
    path.split(['/', '\\'])
        .filter(|p| !p.is_empty())
        .map(|p| {
            if windows {
                p.to_lowercase()
            } else {
                p.to_string()
            }
        })
        .collect()
}

/// Each root of the catalog with where it is looked for and what of it
/// was found there, and the roots taken, by the catalog's id.
fn map_roots(
    catalog: &Catalog,
    given: &[(String, PathBuf)],
) -> Result<(Vec<RootReport>, HashMap<i64, PathBuf>), ImportError> {
    // Every mapping must name a root.
    for (from, _) in given {
        let prefix = components(from);
        let names_one = catalog.roots.iter().any(|r| {
            r.name == *from || (!prefix.is_empty() && components(&r.path).starts_with(&prefix))
        });
        if !names_one {
            let roots: Vec<String> = catalog
                .roots
                .iter()
                .map(|r| format!("{} ({})", r.path, r.name))
                .collect();
            return Err(ImportError::Root(
                from.clone(),
                format!(
                    "names no root of the catalog; its roots are {}",
                    roots.join(", ")
                ),
            ));
        }
    }
    let mut reports = Vec::new();
    let mut taken = HashMap::new();
    for root in &catalog.roots {
        let parts = components(&root.path);
        // The mapping that says most: the root by name, else the
        // longest folder above it.
        let mapped = given
            .iter()
            .filter_map(|(from, to)| {
                if *from == root.name {
                    return Some((usize::MAX, to.clone()));
                }
                let prefix = components(from);
                (!prefix.is_empty() && parts.starts_with(&prefix)).then(|| {
                    let rest: Vec<&str> = root
                        .path
                        .split(['/', '\\'])
                        .filter(|p| !p.is_empty())
                        .skip(prefix.len())
                        .collect();
                    (prefix.len(), under(to, &rest.join("/")))
                })
            })
            .max_by_key(|(n, _)| *n)
            .map(|(_, to)| to);
        let given_one = mapped.is_some();
        let local = mapped.or_else(|| {
            let here = PathBuf::from(&root.path);
            (here.is_absolute() && here.is_dir()).then_some(here)
        });
        let mut report = RootReport {
            catalog: root.path.clone(),
            name: root.name.clone(),
            local: local.clone(),
            given: given_one,
            checked: 0,
            checked_found: 0,
            confirmed: false,
            files: 0,
            found: 0,
        };
        if let Some(local) = &local {
            let sample: Vec<&Image> = catalog
                .images
                .iter()
                .filter(|i| i.root == root.id && i.master.is_none())
                .take(CHECKED)
                .collect();
            report.checked = sample.len();
            report.checked_found = sample
                .iter()
                .filter(|i| under(local, &i.relative).exists())
                .count();
            report.confirmed = if sample.is_empty() {
                local.is_dir()
            } else {
                report.checked_found > 0
            };
            if report.confirmed {
                taken.insert(root.id, local.clone());
            }
        }
        reports.push(report);
    }
    Ok((reports, taken))
}

/// Open the catalog read-only and read what the import carries.
fn read(path: &Path) -> Result<Catalog, ImportError> {
    let mut lock = path.as_os_str().to_owned();
    lock.push(".lock");
    if Path::new(&lock).exists() {
        return Err(ImportError::Locked(path.to_path_buf()));
    }
    // A log with changes in it is read with the catalog, which SQLite
    // does only through the shared memory file it makes beside them;
    // without one, the catalog is opened immutable, which makes and
    // locks nothing, so a dry run leaves the folder as it found it.
    let mut log = path.as_os_str().to_owned();
    log.push("-wal");
    let log_len = std::fs::metadata(&log).map(|m| m.len()).ok();
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let (opened, wal) = if log_len.unwrap_or(0) > 0 {
        (Connection::open_with_flags(path, flags), Wal::Read)
    } else {
        let wal = if log_len.is_none() && write_ahead(path) {
            Wal::Absent
        } else {
            Wal::Whole
        };
        (
            Connection::open_with_flags(immutable_uri(path), flags | OpenFlags::SQLITE_OPEN_URI),
            wal,
        )
    };
    let db = opened.map_err(|source| ImportError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let schema: Option<f64> = db
        .query_row(
            "SELECT value FROM Adobe_variablesTable WHERE name = 'Adobe_DBVersion'",
            [],
            |r| {
                let v: rusqlite::types::Value = r.get(0)?;
                Ok(match v {
                    rusqlite::types::Value::Integer(n) => Some(n as f64),
                    rusqlite::types::Value::Real(f) => Some(f),
                    rusqlite::types::Value::Text(t) => t.trim().parse().ok(),
                    _ => None,
                })
            },
        )
        .optional()
        .map_err(|e| {
            // Another program's database, or no database at all, is no
            // catalog; anything else (a folder that will not let the
            // file be opened, a disk error) is said as it is.
            let not_ours = e.sqlite_error_code() == Some(rusqlite::ErrorCode::NotADatabase)
                || e.to_string().contains("no such table");
            if not_ours {
                ImportError::NotACatalog(path.to_path_buf())
            } else {
                ImportError::Open {
                    path: path.to_path_buf(),
                    source: e,
                }
            }
        })?
        .flatten();
    let Some(schema) = schema.map(|s| s as i64) else {
        return Err(ImportError::NotACatalog(path.to_path_buf()));
    };
    let Some(&(_, written_by)) = KNOWN.iter().find(|(n, _)| *n == schema) else {
        return Err(ImportError::Unknown {
            path: path.to_path_buf(),
            found: schema,
            known: KNOWN
                .iter()
                .map(|(n, by)| format!("{n} ({by})"))
                .collect::<Vec<_>>()
                .join(", "),
        });
    };

    let roots: Vec<CatalogRoot> = db
        .prepare("SELECT id_local, absolutePath, name FROM AgLibraryRootFolder ORDER BY id_local")?
        .query_map([], |r| {
            Ok(CatalogRoot {
                id: r.get(0)?,
                path: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                name: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            })
        })?
        .collect::<Result<_, _>>()?;

    let keywords = keyword_paths(&db)?;
    let mut tags: HashMap<i64, Vec<String>> = HashMap::new();
    for row in db
        .prepare("SELECT image, tag FROM AgLibraryKeywordImage ORDER BY id_local")?
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
    {
        let (image, tag) = row?;
        if let Some(word) = keywords.get(&tag) {
            tags.entry(image).or_default().push(word.clone());
        }
    }
    let captions: HashMap<i64, String> = db
        .prepare("SELECT image, caption FROM AgLibraryIPTC WHERE caption IS NOT NULL")?
        .query_map([], |r| Ok((r.get(0)?, text(r, 1)?)))?
        .collect::<Result<_, _>>()?;
    let titles: HashMap<i64, String> = db
        .prepare("SELECT image, xmp FROM Adobe_AdditionalMetadata")?
        .query_map([], |r| {
            let image: i64 = r.get(0)?;
            let blob: Option<Vec<u8>> = match r.get_ref(1)? {
                rusqlite::types::ValueRef::Blob(b) => Some(b.to_vec()),
                rusqlite::types::ValueRef::Text(t) => Some(t.to_vec()),
                _ => None,
            };
            Ok((image, blob))
        })?
        .filter_map(|row| {
            let (image, blob) = row.ok()?;
            let title = xmp_title(&blob?)?;
            Some((image, title))
        })
        .collect();
    let steps: HashMap<i64, i64> = db
        .prepare("SELECT image, count(*) FROM Adobe_libraryImageDevelopHistoryStep GROUP BY image")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    type Develop = (Option<String>, Option<String>, Option<f64>);
    let develop: HashMap<i64, Develop> = db
        .prepare(
            "SELECT image, text, processVersion, hasDevelopAdjustmentsEx \
             FROM Adobe_imageDevelopSettings",
        )?
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                (
                    Some(text(r, 1)?).filter(|t| !t.is_empty()),
                    number_text(r, 2)?,
                    r.get::<_, Option<f64>>(3).ok().flatten(),
                ),
            ))
        })?
        .collect::<Result<_, _>>()?;

    let mut images = Vec::new();
    let mut stmt = db.prepare(
        "SELECT i.id_local, i.rootFile, i.masterImage, i.copyName, i.rating, i.pick, \
                i.colorLabels, fo.rootFolder, fo.pathFromRoot, fi.baseName, fi.extension \
         FROM Adobe_images i \
         JOIN AgLibraryFile fi ON fi.id_local = i.rootFile \
         JOIN AgLibraryFolder fo ON fo.id_local = fi.folder \
         ORDER BY i.id_local",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let id: i64 = r.get(0)?;
        let rating = r.get::<_, Option<f64>>(4)?.unwrap_or(0.0);
        let pick = r.get::<_, Option<f64>>(5)?.unwrap_or(0.0);
        let label_text = text(r, 6)?;
        let (label, odd_label) = label_of(&label_text);
        let folder = text(r, 8)?;
        let base = text(r, 9)?;
        let extension = text(r, 10)?;
        let name = if extension.is_empty() {
            base
        } else {
            format!("{base}.{extension}")
        };
        let mut meta = Meta {
            rating: rating.clamp(0.0, f64::from(STARS)) as u8,
            flag: if pick > 0.0 {
                Flag::Pick
            } else if pick < 0.0 {
                Flag::Reject
            } else {
                Flag::None
            },
            label,
            ..Meta::default()
        };
        meta.set_keywords(tags.remove(&id).unwrap_or_default());
        meta.set_title(titles.get(&id).map_or("", String::as_str));
        meta.set_caption(captions.get(&id).map_or("", String::as_str));
        let (develop_text, process, adjusted) = develop.get(&id).cloned().unwrap_or_default();
        let edited = match adjusted {
            Some(flag) => flag > 0.0,
            // Lightroom's own mark missing: a step past the one the
            // image arrived with.
            None => steps.get(&id).copied().unwrap_or(0) > 1,
        };
        images.push(Image {
            id,
            file: r.get(1)?,
            root: r.get(7)?,
            relative: format!("{folder}{name}"),
            master: r.get(2)?,
            copy_name: r.get::<_, Option<String>>(3).ok().flatten(),
            meta,
            odd_label,
            develop: develop_text,
            process,
            edited,
        });
    }
    drop(rows);
    drop(stmt);

    let mut members: HashMap<i64, Vec<i64>> = HashMap::new();
    for row in db
        .prepare(
            "SELECT collection, image FROM AgLibraryCollectionImage \
             ORDER BY collection, positionInCollection, id_local",
        )?
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
    {
        let (collection, image) = row?;
        members.entry(collection).or_default().push(image);
    }
    let rules: HashMap<i64, String> = db
        .prepare(
            "SELECT collection, content FROM AgLibraryCollectionContent \
             WHERE owningModule = 'ag.library.smart_collection'",
        )?
        .query_map([], |r| Ok((r.get(0)?, text(r, 1)?)))?
        .collect::<Result<_, _>>()?;
    let nodes: Vec<Node> = db
        .prepare(
            "SELECT id_local, creationId, name, parent, systemOnly FROM AgLibraryCollection \
             ORDER BY id_local",
        )?
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                text(r, 1)?,
                text(r, 2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<f64>>(4).ok().flatten().unwrap_or(0.0) > 0.0,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(id, creation, name, parent, system)| {
            let kind = match creation.as_str() {
                "com.adobe.ag.library.collection" => Kind::Plain,
                "com.adobe.ag.library.group" => Kind::Set,
                "com.adobe.ag.library.smart_collection" => {
                    Kind::Smart(rules.get(&id).cloned().unwrap_or_default())
                }
                other => Kind::Creation(other.rsplit('.').next().unwrap_or(other).to_string()),
            };
            Node {
                id,
                name,
                parent,
                kind,
                system,
                members: members.remove(&id).unwrap_or_default(),
            }
        })
        .collect();
    let stacks: i64 = db.query_row("SELECT count(*) FROM AgLibraryFolderStack", [], |r| {
        r.get(0)
    })?;
    Ok(Catalog {
        schema,
        written_by,
        roots,
        images,
        nodes,
        stacks: stacks.max(0) as usize,
        wal,
    })
}

/// Whether the database at `path` is in write-ahead mode: its header's
/// read and write versions, bytes 18 and 19, are 2.
fn write_ahead(path: &Path) -> bool {
    let mut head = [0u8; 20];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| head[18] == 2 && head[19] == 2)
}

/// The catalog as an SQLite URI opened immutable: read as it stands,
/// with no lock taken and no file made beside it.
fn immutable_uri(path: &Path) -> String {
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let text = full.to_string_lossy().replace('\\', "/");
    // `file:///C:/...` on Windows, `file:///home/...` elsewhere.
    let mut uri = String::from(if text.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for c in text.chars() {
        match c {
            '%' => uri.push_str("%25"),
            '?' => uri.push_str("%3f"),
            '#' => uri.push_str("%23"),
            c => uri.push(c),
        }
    }
    uri.push_str("?immutable=1");
    uri
}

/// A column as text, whatever SQLite kept it as; none is empty.
fn text(r: &rusqlite::Row, i: usize) -> rusqlite::Result<String> {
    Ok(match r.get_ref(i)? {
        rusqlite::types::ValueRef::Text(t) | rusqlite::types::ValueRef::Blob(t) => {
            String::from_utf8_lossy(t).into_owned()
        }
        rusqlite::types::ValueRef::Integer(n) => n.to_string(),
        rusqlite::types::ValueRef::Real(f) => f.to_string(),
        rusqlite::types::ValueRef::Null => String::new(),
    })
}

/// A column that holds a number as text or as a number, as text.
fn number_text(r: &rusqlite::Row, i: usize) -> rusqlite::Result<Option<String>> {
    Ok(Some(text(r, i)?).filter(|t| !t.is_empty()))
}

/// Each keyword's path from the top of the tree, slashed, by its id.
/// The tree's own root has no name and is no keyword.
fn keyword_paths(db: &Connection) -> rusqlite::Result<HashMap<i64, String>> {
    let rows: HashMap<i64, (Option<String>, Option<i64>)> = db
        .prepare("SELECT id_local, name, parent FROM AgLibraryKeyword")?
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
        .collect::<Result<_, _>>()?;
    let mut out = HashMap::new();
    for (&id, (name, _)) in &rows {
        if name.as_deref().is_none_or(|n| n.trim().is_empty()) {
            continue;
        }
        let mut chain: Vec<&str> = Vec::new();
        let mut at = Some(id);
        for _ in 0..rows.len() {
            let Some((name, parent)) = at.and_then(|i| rows.get(&i)) else {
                break;
            };
            match name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                Some(n) => chain.push(n),
                None => break,
            }
            at = *parent;
        }
        chain.reverse();
        out.insert(id, chain.join("/"));
    }
    Ok(out)
}

/// A label as the catalog spells it: one of the five colors by name,
/// or none and the text it had when it is no color.
fn label_of(text: &str) -> (Label, Option<String>) {
    let text = text.trim();
    let label = Label::ALL
        .into_iter()
        .find(|l| *l != Label::None && l.name().eq_ignore_ascii_case(text));
    match label {
        Some(l) => (l, None),
        None if text.is_empty() => (Label::None, None),
        None => (Label::None, Some(text.to_string())),
    }
}

/// The title from an image's XMP as the catalog keeps it: four bytes
/// of length and the packet deflated, or the packet as text.
fn xmp_title(blob: &[u8]) -> Option<String> {
    let packet = if blob.first() == Some(&b'<') {
        String::from_utf8_lossy(blob).into_owned()
    } else {
        let mut out = String::new();
        flate2::read::ZlibDecoder::new(blob.get(4..)?)
            .read_to_string(&mut out)
            .ok()?;
        out
    };
    let title = greycard_edit::xmp::read(&packet).ok()?.meta.title;
    (!title.is_empty()).then_some(title)
}

/// Lightroom's six smart collections as every new catalog has them: the
/// name, and the one rule's criteria, operation, value and units.
const STOCK: [(&str, &str, &str, &str, Option<&str>); 6] = [
    ("Colored Red", "labelColor", "==", "1", None),
    ("Five Stars", "rating", ">=", "5", None),
    ("Past Month", "captureTime", "inLast", "1", Some("months")),
    (
        "Recently Modified",
        "touchTime",
        "inLast",
        "2",
        Some("days"),
    ),
    ("Video Files", "fileFormat", "==", "VIDEO", None),
    ("Without Keywords", "keywords", "empty", "", None),
];

/// Whether a smart collection is one of Lightroom's six, untouched: its
/// name and its one rule as the catalog was made with them. Every
/// catalog has them, so they are left out rather than brought into each
/// catalog's set.
fn is_stock(name: &str, rules: &str) -> bool {
    let Some(&(_, criteria, op, value, units)) = STOCK.iter().find(|s| s.0 == name) else {
        return false;
    };
    let Ok(entries) = lightroom::parse_table(rules) else {
        return false;
    };
    let mut list = Vec::new();
    let mut combine = None;
    for (key, v) in entries {
        match (key.as_deref(), v) {
            (Some("combine"), Value::Word(w)) => combine = Some(w),
            (None, Value::Table(fields)) => list.push(fields),
            _ => return false,
        }
    }
    let [rule] = list.as_slice() else {
        return false;
    };
    if combine.as_deref() != Some("intersect") {
        return false;
    }
    let fields: HashMap<&str, &str> = rule
        .iter()
        .filter_map(|(k, v)| match (k, v) {
            (Some(k), Value::Word(w)) => Some((k.as_str(), w.as_str())),
            _ => None,
        })
        .collect();
    let wanted = 2 + usize::from(!value.is_empty()) + usize::from(units.is_some());
    fields.len() == rule.len()
        && fields.len() == wanted
        && fields.get("criteria") == Some(&criteria)
        && fields.get("operation") == Some(&op)
        && fields.get("value").copied().unwrap_or("") == value
        && fields.get("value_units").copied() == units
}

/// A smart collection's rules in the filter language, or why they have
/// no counterpart there. Every rule must have a term, and the rules
/// must all be asked together, since a filter's terms are.
fn translate(rules: &str) -> Result<String, String> {
    let entries =
        lightroom::parse_table(rules).map_err(|e| format!("its rules will not read: {e}"))?;
    let mut combine = "intersect".to_string();
    let mut list: Vec<HashMap<String, Value>> = Vec::new();
    for (key, value) in entries {
        match (key.as_deref(), value) {
            (Some("combine"), Value::Word(w)) => combine = w,
            (None, Value::Table(fields)) => list.push(
                fields
                    .into_iter()
                    .filter_map(|(k, v)| Some((k?, v)))
                    .collect(),
            ),
            _ => {}
        }
    }
    if list.is_empty() {
        return Err("it has no rules".into());
    }
    match combine.as_str() {
        "intersect" => {}
        "union" if list.len() == 1 => {}
        "union" => return Err("it matches any of its rules, and a filter asks all".into()),
        "exclude" => return Err("it matches none of its rules".into()),
        other => return Err(format!("it combines its rules by {other}")),
    }
    let mut terms: Vec<String> = Vec::new();
    for rule in &list {
        let word = |k: &str| match rule.get(k) {
            Some(Value::Word(w)) => Some(w.as_str()),
            _ => None,
        };
        let criteria = word("criteria").unwrap_or("");
        let op = word("operation").unwrap_or("");
        let value = word("value").unwrap_or("");
        terms.extend(term(criteria, op, value, word("value2"))?);
    }
    let text = terms.join(" ");
    Filter::parse(&text).map_err(|e| format!("its filter would not read: {e}"))?;
    Ok(text)
}

/// One rule's terms.
fn term(
    criteria: &str,
    op: &str,
    value: &str,
    value2: Option<&str>,
) -> Result<Vec<String>, String> {
    let none = || Err(format!("{criteria} {op} has no counterpart in the filter"));
    let cmp = |op: &str| -> Option<&'static str> {
        Some(match op {
            "==" => "=",
            "!=" => "!=",
            "<" => "<",
            "<=" => "<=",
            ">" => ">",
            ">=" => ">=",
            _ => return None,
        })
    };
    let quoted = |v: &str| -> Result<String, String> {
        if v.contains('"') {
            return Err(format!("{criteria} names {v}, with a quote in it"));
        }
        Ok(if v.contains(char::is_whitespace) {
            format!("\"{v}\"")
        } else {
            v.to_string()
        })
    };
    match criteria {
        "rating" => {
            let Some(c) = cmp(op) else { return none() };
            let n = value
                .parse::<f64>()
                .map_err(|_| format!("a rating of {value}"))?;
            Ok(vec![format!("rating{c}{}", n.round() as i64)])
        }
        "pick" => {
            let is = match op {
                "==" => ":",
                "!=" => "!=",
                _ => return none(),
            };
            let n = value.parse::<f64>().unwrap_or(0.0);
            let flag = if n > 0.0 {
                "pick"
            } else if n < 0.0 {
                "reject"
            } else {
                "none"
            };
            Ok(vec![format!("flag{is}{flag}")])
        }
        "labelColor" => {
            let is = match op {
                "==" => ":",
                "!=" => "!=",
                _ => return none(),
            };
            let color = match value.parse::<f64>().map(|n| n as i64) {
                Ok(0) => "none",
                Ok(1) => "red",
                Ok(2) => "yellow",
                Ok(3) => "green",
                Ok(4) => "blue",
                Ok(5) => "purple",
                _ => return Err(format!("a label of {value}")),
            };
            Ok(vec![format!("label{is}{color}")])
        }
        "keywords" => {
            let words: Vec<&str> = value.split_whitespace().collect();
            match op {
                "all" | "words" if !words.is_empty() => words
                    .iter()
                    .map(|w| Ok(format!("keyword:{}", quoted(w)?)))
                    .collect(),
                "any" if words.len() == 1 => Ok(vec![format!("keyword:{}", quoted(words[0])?)]),
                "any" => {
                    Err("keywords any of several words has no counterpart in the filter".into())
                }
                _ => none(),
            }
        }
        // Lightroom names the camera and the lens as the file's EXIF
        // spells them ("Canon EOS R5m2"); the index keeps the decoder's
        // cleaned names ("EOS R5 Mark II"). A term on the one would
        // match nothing of the other, and a smart collection that
        // silently shows nothing is worse than one skipped and named.
        "camera" | "lens" => Err(format!(
            "Lightroom's {criteria} is the EXIF's name for it, which the index spells \
             otherwise, so the filter could match nothing"
        )),
        "captureTime" => {
            let day = |v: &str| v.get(..10).unwrap_or(v).to_string();
            match (op, value2) {
                ("in", Some(to)) => Ok(vec![
                    format!("date>={}", day(value)),
                    format!("date<={}", day(to)),
                ]),
                (op, _) if cmp(op).is_some() => Ok(vec![format!(
                    "date{}{}",
                    cmp(op).unwrap_or("="),
                    day(value)
                )]),
                _ => Err(format!(
                    "captureTime {op} is a time counted from today, which the filter has not"
                )),
            }
        }
        _ => none(),
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let line =
            |f: &mut fmt::Formatter<'_>, label: &str, text: &str| writeln!(f, "{label:<13} {text}");
        let more = |f: &mut fmt::Formatter<'_>, text: &str| writeln!(f, "{:<13} {text}", "");
        let counted = |map: &BTreeMap<String, usize>| {
            let mut v: Vec<(&String, &usize)> = map.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            v.iter()
                .map(|(k, n)| format!("{k} ×{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        line(f, "catalog", &self.catalog.display().to_string())?;
        line(
            f,
            "written by",
            &format!("{} (schema {})", self.written_by, self.schema),
        )?;
        match self.wal {
            Wal::Whole => {}
            Wal::Read => line(f, "log", "read with the -wal beside it")?,
            Wal::Absent => {
                line(
                    f,
                    "log",
                    "no -wal beside it: whole if Lightroom had quit before it was copied;",
                )?;
                more(
                    f,
                    "otherwise copy the catalog with its -wal, or quit Lightroom first, \
                     or the newest changes are not in it",
                )?;
            }
        }
        for r in &self.roots {
            line(f, "root", &format!("{} ({})", r.catalog, r.name))?;
            match &r.local {
                None => more(
                    f,
                    "not here, and not mapped: say where with --root <catalog path>=<folder>",
                )?,
                Some(local) => {
                    let how = if r.given {
                        "as given"
                    } else {
                        "where the catalog says"
                    };
                    let took = if r.confirmed { "taken" } else { "not taken" };
                    more(
                        f,
                        &format!(
                            "-> {} {how}: {} of {} files looked for found, {took}",
                            local.display(),
                            r.checked_found,
                            r.checked
                        ),
                    )?;
                }
            }
            more(
                f,
                &format!(
                    "{} files: {} found, {} missing",
                    r.files,
                    r.found,
                    r.files - r.found
                ),
            )?;
        }
        line(
            f,
            "images",
            &format!(
                "{}: {} files and {} virtual copies",
                self.images, self.files, self.copies
            ),
        )?;
        line(
            f,
            "found",
            &format!("{} images, {} missing", self.found, self.missing),
        )?;
        // A dry run says what a write would do, of the files found.
        let would = if self.written.is_some() {
            "is"
        } else {
            "would be"
        };
        line(
            f,
            "with edits",
            &format!(
                "{}, {} of them found: each found {would} written as one named state, \
                 its settings mapped as an approximation",
                self.with_edits, self.with_edits_found
            ),
        )?;
        if !self.not_carried.is_empty() {
            line(f, "not carried", &counted(&self.not_carried))?;
        }
        if self.unreadable_develop > 0 {
            line(
                f,
                "unreadable",
                &format!(
                    "{} images' develop settings will not read",
                    self.unreadable_develop
                ),
            )?;
        }
        if self.old_process > 0 {
            line(
                f,
                "old process",
                &format!(
                    "{} edits on the 2003 or 2010 process, further off still",
                    self.old_process
                ),
            )?;
        }
        line(
            f,
            "meta",
            &format!(
                "{} files with a rating, flag, label, keyword, title or caption, {} of them found",
                self.with_meta, self.with_meta_found
            ),
        )?;
        if !self.odd_labels.is_empty() {
            line(
                f,
                "labels",
                &format!("no color for {}: left unlabeled", counted(&self.odd_labels)),
            )?;
        }
        line(
            f,
            "keywords",
            &format!("{} keywords on {} files", self.keywords, self.keyworded),
        )?;
        let apart = if self.copies_apart > 0 {
            format!(
                "; {} whose rating, flag, label, keywords, title or caption differs from \
                 its file's, which keeps its own",
                self.copies_apart
            )
        } else {
            String::new()
        };
        line(
            f,
            "copies",
            &format!(
                "{} virtual {}, {} of them found, each a version named for it{apart}",
                self.copies,
                if self.copies == 1 { "copy" } else { "copies" },
                self.copies_found
            ),
        )?;
        line(f, "stacks", &format!("{}, not carried", self.stacks))?;
        line(
            f,
            "collections",
            &format!(
                "{} and {} {}",
                self.collections.len(),
                self.sets,
                if self.sets == 1 { "set" } else { "sets" }
            ),
        )?;
        for c in &self.collections {
            let creation = c
                .creation
                .as_deref()
                .map(|k| format!(" (a {k}: its frames, not its layout)"))
                .unwrap_or_default();
            more(
                f,
                &format!(
                    "{}{creation}: {} frames, {} found",
                    c.name, c.frames, c.found
                ),
            )?;
        }
        line(
            f,
            "smart",
            &format!(
                "{} translated, {} skipped, {} of Lightroom's own six left out",
                self.smart.len(),
                self.smart_skipped.len(),
                self.smart_stock
            ),
        )?;
        for (name, text) in &self.smart {
            more(f, &format!("{name}: {text}"))?;
        }
        for (name, why) in &self.smart_skipped {
            more(f, &format!("{name}: skipped, {why}"))?;
        }
        line(
            f,
            "sidecars",
            if self.sidecars_in_folder {
                "a new one under its folder's .greycard/, as the editor's setting says"
            } else {
                "a new one beside its file, or under the folder's .greycard/ where it has one"
            },
        )?;
        if self.already > 0 {
            line(
                f,
                "",
                &format!(
                    "{} {} history, versions or meta already: left alone without --force",
                    self.already,
                    if self.already == 1 {
                        "file has"
                    } else {
                        "files have"
                    }
                ),
            )?;
        }
        for (path, why) in &self.unreadable_sidecars {
            line(
                f,
                "unreadable",
                &format!("{}: {why}; left alone", path.display()),
            )?;
        }
        match &self.written {
            None => line(f, "dry run", "nothing written; --write to import")?,
            Some(w) => {
                line(
                    f,
                    "written",
                    &format!(
                        "{} sidecars ({} under .greycard/), {} left alone",
                        w.sidecars, w.in_folder, w.left_alone
                    ),
                )?;
                for (path, why) in &w.failed {
                    line(f, "not written", &format!("{}: {why}", path.display()))?;
                }
                line(
                    f,
                    "collections",
                    &format!(
                        "{} written, {} joined to one of the same name, {} there already \
                         with every frame; {} smart written, {} there already",
                        w.collections, w.joined, w.collections_there, w.smart, w.smart_there
                    ),
                )?;
                if w.copies_as_own > 0 {
                    more(
                        f,
                        &format!(
                            "{} virtual copies in them as their file's own frame: their \
                             sidecar was left alone",
                            w.copies_as_own
                        ),
                    )?;
                }
                for (dir, said) in &w.roots {
                    line(f, "root", &format!("{}: {said}", dir.display()))?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::tests::scratch;
    use std::io::Write;

    /// A catalog's develop table as Lightroom writes one, cut down.
    const DEVELOP: &str = "s = { AutoLateralCA = 1,\n\
        CameraProfile = \"Adobe Standard\",\n\
        Exposure2012 = 1.5,\n\
        Contrast2012 = -20,\n\
        CropLeft = 0.1,\n\
        MaskGroupBasedCorrections = { { CorrectionActive = true,\n\
        CorrectionName = \"Sky\",\n\
        What = \"Correction\" } },\n\
        ProcessVersion = \"15.4\",\n\
        ToneCurvePV2012 = { 0,\n0,\n128,\n140,\n255,\n255 },\n\
        Version = \"18.3\",\n\
        WhiteBalance = \"As Shot\" }\n";

    /// A copy's: the same with its own exposure.
    fn develop_at(exposure: f32) -> String {
        DEVELOP.replace("Exposure2012 = 1.5", &format!("Exposure2012 = {exposure}"))
    }

    /// An image's XMP as the catalog keeps it: four bytes of length
    /// and the packet deflated.
    fn xmp_blob(title: &str) -> Vec<u8> {
        let packet = format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="0">
            <dc:title><rdf:Alt><rdf:li xml:lang="x-default">{title}</rdf:li></rdf:Alt></dc:title>
            </rdf:Description></rdf:RDF></x:xmpmeta>"#
        );
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(packet.as_bytes()).unwrap();
        let body = z.finish().unwrap();
        let mut out = (packet.len() as u32).to_be_bytes().to_vec();
        out.extend(body);
        out
    }

    /// A catalog of the tables the import reads, in their columns, at
    /// `schema`, empty.
    fn empty_catalog(path: &Path, schema: &str) -> Connection {
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE Adobe_variablesTable (id_local INTEGER PRIMARY KEY, name, value);
             CREATE TABLE AgLibraryRootFolder (id_local INTEGER PRIMARY KEY, id_global,
                 absolutePath, name, relativePathFromCatalog);
             CREATE TABLE AgLibraryFolder (id_local INTEGER PRIMARY KEY, id_global, parentId,
                 pathFromRoot, rootFolder);
             CREATE TABLE AgLibraryFile (id_local INTEGER PRIMARY KEY, id_global, baseName,
                 extension, folder);
             CREATE TABLE Adobe_images (id_local INTEGER PRIMARY KEY, id_global, rootFile,
                 masterImage, copyName, rating, pick, colorLabels NOT NULL DEFAULT '');
             CREATE TABLE AgLibraryKeyword (id_local INTEGER PRIMARY KEY, id_global, name,
                 parent);
             CREATE TABLE AgLibraryKeywordImage (id_local INTEGER PRIMARY KEY, image, tag);
             CREATE TABLE AgLibraryIPTC (id_local INTEGER PRIMARY KEY, image, caption);
             CREATE TABLE Adobe_AdditionalMetadata (id_local INTEGER PRIMARY KEY, image, xmp);
             CREATE TABLE Adobe_libraryImageDevelopHistoryStep (id_local INTEGER PRIMARY KEY,
                 image, name);
             CREATE TABLE Adobe_imageDevelopSettings (id_local INTEGER PRIMARY KEY, image, text,
                 processVersion, hasDevelopAdjustmentsEx);
             CREATE TABLE AgLibraryCollection (id_local INTEGER PRIMARY KEY, creationId, name,
                 parent, systemOnly);
             CREATE TABLE AgLibraryCollectionImage (id_local INTEGER PRIMARY KEY, collection,
                 image, positionInCollection);
             CREATE TABLE AgLibraryCollectionContent (id_local INTEGER PRIMARY KEY, collection,
                 content, owningModule);
             CREATE TABLE AgLibraryFolderStack (id_local INTEGER PRIMARY KEY, id_global);",
        )
        .unwrap();
        db.execute(
            "INSERT INTO Adobe_variablesTable (name, value) VALUES ('Adobe_DBVersion', ?1)",
            [schema],
        )
        .unwrap();
        db
    }

    /// A shoot and its catalog, the catalog's root at `catalog_root` on
    /// the machine it came from and the files under `here`:
    ///
    /// - `2026/harbor/IMG_0001.CR3`: four stars, a pick, red, two
    ///   keywords (one under two others), a title and a caption,
    ///   developed; and a virtual copy of it, "Warm", two stars and
    ///   developed apart.
    /// - `2026/harbor/IMG_0002.CR3`: a reject, a label of a renamed
    ///   label set, never developed.
    /// - `2026/harbor/IMG_0003.CR3`: in the catalog, not on the disk.
    ///
    /// A set "Clients" holding the collection "Cover" (the copy, the
    /// first frame, the missing one, in that order) and a smart
    /// collection that translates; a book; the quick collection empty;
    /// a smart collection relative to today and one of any of two
    /// rules. One stack.
    fn shoot(catalog: &Path, catalog_root: &str, here: &Path) {
        let db = empty_catalog(catalog, "1504001");
        let dir = here.join("2026").join("harbor");
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["IMG_0001.CR3", "IMG_0002.CR3"] {
            // Not a raw: the probe fails and the frame takes the
            // default blend, which is all a test needs.
            std::fs::write(dir.join(n), format!("not a raw, {n}")).unwrap();
        }
        db.execute_batch(&format!(
            "INSERT INTO AgLibraryRootFolder VALUES (1, 'r', '{catalog_root}', 'Shoots', NULL);
             INSERT INTO AgLibraryFolder VALUES (2, 'f', NULL, '2026/harbor/', 1);
             INSERT INTO AgLibraryFile VALUES (10, 'a', 'IMG_0001', 'CR3', 2);
             INSERT INTO AgLibraryFile VALUES (11, 'b', 'IMG_0002', 'CR3', 2);
             INSERT INTO AgLibraryFile VALUES (12, 'c', 'IMG_0003', 'CR3', 2);
             INSERT INTO Adobe_images VALUES (100, 'i', 10, NULL, NULL, 4.0, 1.0, 'Red');
             INSERT INTO Adobe_images VALUES (101, 'j', 11, NULL, NULL, 0.0, -1.0, 'To Print');
             INSERT INTO Adobe_images VALUES (102, 'k', 12, NULL, NULL, 3.0, 0.0, '');
             INSERT INTO Adobe_images VALUES (103, 'l', 10, 100, 'Warm', 2.0, 0.0, 'Red');
             INSERT INTO AgLibraryKeyword VALUES (1, 'root', NULL, NULL);
             INSERT INTO AgLibraryKeyword VALUES (2, 'p', 'Places', 1);
             INSERT INTO AgLibraryKeyword VALUES (3, 's', 'Scotland', 2);
             INSERT INTO AgLibraryKeyword VALUES (4, 'k', 'Skye', 3);
             INSERT INTO AgLibraryKeyword VALUES (5, 'h', 'Harbor', 1);
             INSERT INTO AgLibraryKeywordImage VALUES (1, 100, 4);
             INSERT INTO AgLibraryKeywordImage VALUES (2, 100, 5);
             INSERT INTO AgLibraryIPTC VALUES (1, 100, 'Boats at low tide');
             INSERT INTO Adobe_libraryImageDevelopHistoryStep VALUES (1, 100, 'Import');
             INSERT INTO Adobe_libraryImageDevelopHistoryStep VALUES (2, 100, 'Exposure');
             INSERT INTO Adobe_libraryImageDevelopHistoryStep VALUES (3, 101, 'Import');
             INSERT INTO AgLibraryCollection VALUES (1, 'com.adobe.ag.library.group',
                 'Clients', NULL, 0.0);
             INSERT INTO AgLibraryCollection VALUES (2, 'com.adobe.ag.library.collection',
                 'Cover', 1, 0.0);
             INSERT INTO AgLibraryCollection VALUES (3, 'com.adobe.ag.library.collection',
                 'quick collection', NULL, 1.0);
             INSERT INTO AgLibraryCollection VALUES (4, 'com.adobe.ag.library.smart_collection',
                 'Red fours', 1, 0.0);
             INSERT INTO AgLibraryCollection VALUES (5, 'com.adobe.ag.library.smart_collection',
                 'Past Month', NULL, 0.0);
             INSERT INTO AgLibraryCollection VALUES (6, 'com.adobe.ag.library.smart_collection',
                 'Either', NULL, 0.0);
             INSERT INTO AgLibraryCollection VALUES (7, 'com.adobe.ag.bookExport',
                 'Album', NULL, 0.0);
             INSERT INTO AgLibraryCollectionImage VALUES (1, 2, 100, 'b');
             INSERT INTO AgLibraryCollectionImage VALUES (2, 2, 103, 'a');
             INSERT INTO AgLibraryCollectionImage VALUES (3, 2, 102, 'c');
             INSERT INTO AgLibraryCollectionImage VALUES (4, 7, 101, 'a');
             INSERT INTO AgLibraryFolderStack VALUES (1, 's');"
        ))
        .unwrap();
        let insert = "INSERT INTO Adobe_imageDevelopSettings (image, text, processVersion, \
                      hasDevelopAdjustmentsEx) VALUES (?1, ?2, '15.4', ?3)";
        db.execute(insert, rusqlite::params![100, DEVELOP, 1.0])
            .unwrap();
        db.execute(insert, rusqlite::params![101, DEVELOP, -1.0])
            .unwrap();
        db.execute(insert, rusqlite::params![103, develop_at(-0.5), 1.0])
            .unwrap();
        db.execute(
            "INSERT INTO Adobe_AdditionalMetadata (image, xmp) VALUES (100, ?1)",
            [xmp_blob("Low tide")],
        )
        .unwrap();
        let rules = |text: &str| format!("s = {{\n{text}\n\tcombine = \"intersect\",\n}}\n");
        for (id, content) in [
            (
                4,
                rules(
                    "{ criteria = \"rating\", operation = \">=\", value = 4, },\n\
                     { criteria = \"labelColor\", operation = \"==\", value = 1, },",
                ),
            ),
            (
                5,
                rules(
                    "{ criteria = \"captureTime\", operation = \"inLast\", value = 1, \
                     value_units = \"months\", },",
                ),
            ),
            (
                6,
                "s = { { criteria = \"rating\", operation = \"==\", value = 5, },\n\
                 { criteria = \"pick\", operation = \"==\", value = 1, },\n\
                 combine = \"union\", }"
                    .to_string(),
            ),
        ] {
            db.execute(
                "INSERT INTO AgLibraryCollectionContent (collection, content, owningModule) \
                 VALUES (?1, ?2, 'ag.library.smart_collection')",
                rusqlite::params![id, content],
            )
            .unwrap();
        }
    }

    /// Every file under `dir`, by path, with its bytes.
    fn tree(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(d) = todo.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    todo.push(p);
                } else {
                    out.insert(p.clone(), std::fs::read(&p).unwrap());
                }
            }
        }
        out
    }

    struct Setup {
        dir: PathBuf,
        catalog: PathBuf,
        here: PathBuf,
        options: Options,
    }

    fn setup(what: &str) -> Setup {
        let dir = scratch(&format!("lrcat-{what}"));
        let catalog = dir.join("Harbor.lrcat");
        let here = dir.join("photos");
        shoot(&catalog, "D:/Pictures/Shoots/", &here);
        let options = Options {
            roots: vec![("D:/Pictures/Shoots".into(), here.clone())],
            library: dir.join("library").join("library.sqlite"),
            ..Options::default()
        };
        Setup {
            dir,
            catalog,
            here,
            options,
        }
    }

    fn writing(s: &Setup) -> Options {
        Options {
            write: true,
            ..s.options.clone()
        }
    }

    #[test]
    fn a_dry_run_reports_and_writes_nothing() {
        let s = setup("dry");
        let before = tree(&s.dir);
        let report = import(&s.catalog, &s.options).unwrap();
        assert_eq!(tree(&s.dir), before, "a dry run writes nothing");
        assert_eq!(report.schema, 1_504_001);
        assert_eq!(report.written_by, "Lightroom Classic 15.4 and 15.5");
        assert_eq!(
            (report.images, report.files, report.copies),
            (4, 3, 1),
            "{report}"
        );
        assert_eq!((report.found, report.missing), (3, 1));
        assert_eq!(report.with_edits, 2, "the frame and its copy");
        assert_eq!(report.with_edits_found, 2);
        assert_eq!((report.with_meta, report.with_meta_found), (3, 2));
        assert_eq!(report.copies_found, 1);
        assert_eq!(report.wal, Wal::Whole);
        assert_eq!(report.not_carried.get("masks"), Some(&2));
        assert_eq!(report.not_carried.get("crop"), Some(&2));
        assert_eq!(report.old_process, 0);
        assert_eq!(report.with_meta, 3);
        assert_eq!(report.odd_labels.get("To Print"), Some(&1));
        assert_eq!(report.copies_apart, 1, "the copy's two stars");
        assert_eq!((report.keywords, report.keyworded), (2, 1));
        assert_eq!(report.stacks, 1);
        assert_eq!(report.sets, 1);
        let r = &report.roots[0];
        assert_eq!(r.local.as_deref(), Some(s.here.as_path()));
        assert!(r.given && r.confirmed);
        assert_eq!((r.checked, r.checked_found, r.files, r.found), (3, 2, 3, 2));
        // The empty quick collection is left out; the book comes as its
        // frames.
        let names: Vec<&str> = report.collections.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Harbor/Clients/Cover", "Harbor/Album"]);
        assert_eq!(
            (report.collections[0].frames, report.collections[0].found),
            (3, 2)
        );
        assert_eq!(
            report.collections[1].creation.as_deref(),
            Some("bookExport")
        );
        assert_eq!(
            report.smart,
            vec![(
                "Harbor/Clients/Red fours".to_string(),
                "rating>=4 label:red".to_string()
            )]
        );
        let skipped: Vec<&str> = report
            .smart_skipped
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        // Past Month is Lightroom's own, as it made it: left out.
        assert_eq!(skipped, ["Harbor/Either"]);
        assert_eq!(report.smart_stock, 1);
        assert!(report.written.is_none());
        let said = report.to_string();
        assert!(said.contains("dry run"), "{said}");
        assert!(said.contains("would be written"), "{said}");
        assert!(said.contains("1 virtual copy,"), "{said}");
        assert!(said.contains("Either: skipped"), "{said}");
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn a_write_carries_the_meta_the_develop_the_copies_and_the_collections() {
        let s = setup("write");
        let report = import(&s.catalog, &writing(&s)).unwrap();
        let w = report.written.as_ref().unwrap();
        assert_eq!((w.sidecars, w.left_alone), (2, 0), "{report}");
        assert!(w.failed.is_empty());
        let first = s.here.join("2026/harbor/IMG_0001.CR3");
        let second = s.here.join("2026/harbor/IMG_0002.CR3");

        // The meta exactly; the develop as one named state over the
        // starting edit; the copy as a version of its name, its own
        // settings its line's one named state.
        let a = Sidecar::load(&first).unwrap().unwrap();
        assert_eq!(a.meta.rating, 4);
        assert_eq!(a.meta.flag, Flag::Pick);
        assert_eq!(a.meta.label, Label::Red);
        assert_eq!(a.meta.keywords, ["Places/Scotland/Skye", "Harbor"]);
        assert_eq!(a.meta.title, "Low tide");
        assert_eq!(a.meta.caption, "Boats at low tide");
        assert_eq!(a.history.len(), 1);
        assert_eq!(a.current_label.as_deref(), Some("Imported from Lightroom"));
        assert_eq!(a.current.light.exposure, 1.5);
        assert!((a.current.curves.rgb[1][1] - 140.0 / 255.0).abs() < 1e-6);
        assert_eq!(a.versions.len(), 1);
        let v = &a.versions[0];
        assert_eq!(v.name, "Warm");
        assert_eq!(v.line.current.light.exposure, -0.5);
        assert_eq!(
            v.line.current_label.as_deref(),
            Some("Imported from Lightroom")
        );
        assert_eq!(v.line.history.len(), 1);

        // Meta alone: no history.
        let b = Sidecar::load(&second).unwrap().unwrap();
        assert_eq!(b.meta.flag, Flag::Reject);
        assert_eq!(b.meta.label, Label::None, "a label that is no color");
        assert!(b.history.is_empty() && b.versions.is_empty());

        // The collections: the copy as its version, then the frame's
        // own, the missing one left out; the book's frame; the smart
        // one over the root.
        let folder = collections::read_folder(
            &collections::dir_beside(&s.options.library),
            collections::VERSION,
        );
        let cover = folder.list.iter().find(|c| c.name == "Cover").unwrap();
        assert_eq!(cover.set, "Harbor/Clients");
        let hash = crate::hash_file(&first).unwrap();
        let pairs: Vec<(&str, Option<&str>)> = cover.refs.iter().map(|r| r.pair()).collect();
        assert_eq!(
            pairs,
            [(hash.as_str(), Some(v.id.as_str())), (hash.as_str(), None)]
        );
        assert_eq!(
            cover.refs[0].hint,
            collections::hint_of(&crate::key_path(&first))
        );
        assert!(
            folder
                .list
                .iter()
                .any(|c| c.name == "Album" && c.refs.len() == 1)
        );
        assert_eq!(folder.smart.len(), 1);
        assert_eq!(folder.smart[0].name, "Red fours");
        assert_eq!(folder.smart[0].filter.text, "rating>=4 label:red");
        let root = crate::canonical(&s.here).unwrap();
        assert_eq!(folder.smart[0].root.as_deref(), Some(root.as_path()));
        assert_eq!((w.collections, w.joined, w.smart), (2, 0, 1));

        // The root, through the roots file.
        let roots_file = Roots::path_beside(&s.options.library);
        let roots = Roots::load(&roots_file).unwrap();
        assert_eq!(roots.list(), [root]);

        // Again: every frame left alone, the collections there with
        // every frame, the smart one there, the roots file not written.
        let files = tree(&s.dir);
        let stamp = std::fs::metadata(&roots_file).unwrap().modified().unwrap();
        let again = import(&s.catalog, &writing(&s)).unwrap();
        let w = again.written.as_ref().unwrap();
        assert_eq!((w.sidecars, w.left_alone), (0, 2));
        assert_eq!(
            (
                w.collections,
                w.joined,
                w.collections_there,
                w.smart,
                w.smart_there
            ),
            (0, 0, 2, 0, 1)
        );
        assert_eq!(w.roots[0].1, "a root already");
        assert_eq!(tree(&s.dir), files, "a second import changes nothing");
        assert_eq!(
            std::fs::metadata(&roots_file).unwrap().modified().unwrap(),
            stamp
        );
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn a_frame_with_history_or_meta_is_left_alone_unless_forced() {
        let s = setup("force");
        let first = s.here.join("2026/harbor/IMG_0001.CR3");
        let mut mine = Sidecar::default();
        mine.meta.rating = 1;
        mine.save(&first).unwrap();
        let mine_bytes = std::fs::read(Sidecar::path_for(&first)).unwrap();
        let report = import(&s.catalog, &writing(&s)).unwrap();
        assert_eq!(report.already, 1);
        let w = report.written.as_ref().unwrap();
        assert_eq!((w.sidecars, w.left_alone), (1, 1));
        assert_eq!(
            std::fs::read(Sidecar::path_for(&first)).unwrap(),
            mine_bytes
        );
        // The copy in the collection has no version to name: the
        // frame's own stands for it, and that is said.
        assert_eq!(w.copies_as_own, 1);

        let forced = Options {
            force: true,
            ..writing(&s)
        };
        let report = import(&s.catalog, &forced).unwrap();
        assert_eq!(
            report.written.as_ref().unwrap().sidecars,
            1,
            "the second file says what it said already"
        );
        let a = Sidecar::load(&first).unwrap().unwrap();
        assert_eq!(a.meta.rating, 4);
        assert_eq!(a.current_label.as_deref(), Some("Imported from Lightroom"));
        assert_eq!(a.versions.len(), 1);
        // Forced again: the copy's version is found by its name, and
        // nothing is made twice.
        import(&s.catalog, &forced).unwrap();
        let again = Sidecar::load(&first).unwrap().unwrap();
        assert_eq!(again.versions.len(), 1);
        assert_eq!(again.history.len(), a.history.len());
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn an_unknown_schema_is_refused_naming_the_known_ones() {
        let dir = scratch("lrcat-schema");
        let path = dir.join("Old.lrcat");
        drop(empty_catalog(&path, "1100000"));
        let err = import(&path, &Options::default()).unwrap_err();
        let said = err.to_string();
        assert!(
            matches!(
                err,
                ImportError::Unknown {
                    found: 1_100_000,
                    ..
                }
            ),
            "{said}"
        );
        assert!(
            said.contains("1500000") && said.contains("1504001"),
            "{said}"
        );
        // Not a catalog at all.
        let other = dir.join("other.sqlite");
        Connection::open(&other)
            .unwrap()
            .execute_batch("CREATE TABLE t (a)")
            .unwrap();
        assert!(matches!(
            import(&other, &Options::default()),
            Err(ImportError::NotACatalog(_))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_catalog_lightroom_has_open_is_refused() {
        let s = setup("locked");
        std::fs::write(s.dir.join("Harbor.lrcat.lock"), "").unwrap();
        assert!(matches!(
            import(&s.catalog, &s.options),
            Err(ImportError::Locked(_))
        ));
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn roots_map_by_a_folder_above_or_by_name_and_are_taken_only_when_files_are_there() {
        let s = setup("roots");
        let with = |roots: Vec<(String, PathBuf)>| {
            import(
                &s.catalog,
                &Options {
                    roots,
                    ..s.options.clone()
                },
            )
        };
        // A folder above the root, its case folded as Windows folds it,
        // to a folder with nothing in it.
        let parent = s.here.parent().unwrap().to_path_buf();
        let elsewhere = parent.join("elsewhere");
        let r = with(vec![("d:\\pictures".into(), elsewhere.clone())]).unwrap();
        assert_eq!(
            r.roots[0].local.as_deref(),
            Some(elsewhere.join("Shoots").as_path())
        );
        assert!(!r.roots[0].confirmed, "nothing there");
        assert_eq!((r.roots[0].checked_found, r.found, r.missing), (0, 0, 4));
        // The shoot moved there: taken, and counted.
        let up = parent.join("up");
        std::fs::create_dir_all(&up).unwrap();
        std::fs::rename(&s.here, up.join("Shoots")).unwrap();
        let r = with(vec![("D:/Pictures".into(), up.clone())]).unwrap();
        assert!(r.roots[0].confirmed);
        assert_eq!((r.found, r.missing), (3, 1));
        // By its name.
        let r = with(vec![("Shoots".into(), up.join("Shoots"))]).unwrap();
        assert!(r.roots[0].confirmed && r.roots[0].given);
        // None given: looked for where the catalog says, which is not
        // here.
        let r = with(Vec::new()).unwrap();
        assert!(r.roots[0].local.is_none() && !r.roots[0].given);
        assert_eq!(r.found, 0);
        assert!(r.to_string().contains("--root"), "{r}");
        // A mapping that names no root is refused, naming them.
        let err = with(vec![("E:/Elsewhere".into(), parent.clone())]).unwrap_err();
        assert!(err.to_string().contains("D:/Pictures/Shoots/"), "{err}");
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn smart_rules_translate_where_the_filter_has_them() {
        let one = |c: &str, op: &str, v: &str| {
            translate(&format!(
                "s = {{ {{ criteria = \"{c}\", operation = \"{op}\", value = {v}, }}, \
                 combine = \"intersect\", }}"
            ))
        };
        assert_eq!(one("rating", ">=", "3").unwrap(), "rating>=3");
        assert_eq!(one("rating", "==", "0").unwrap(), "rating=0");
        assert_eq!(one("pick", "==", "1").unwrap(), "flag:pick");
        assert_eq!(one("pick", "!=", "-1").unwrap(), "flag!=reject");
        assert_eq!(one("labelColor", "==", "4").unwrap(), "label:blue");
        assert_eq!(
            one("keywords", "all", "\"skye harbor\"").unwrap(),
            "keyword:skye keyword:harbor"
        );
        assert_eq!(one("keywords", "any", "\"skye\"").unwrap(), "keyword:skye");
        // The camera and the lens as EXIF names them are not the
        // index's names: skipped, and said why.
        assert!(
            one("camera", "==", "\"Canon EOS R5m2\"")
                .unwrap_err()
                .contains("EXIF")
        );
        assert!(one("lens", "any", "\"RF50mm\"").is_err());
        assert_eq!(
            one("captureTime", ">=", "\"2026-05-01\"").unwrap(),
            "date>=2026-05-01"
        );
        assert_eq!(
            translate(
                "s = { { criteria = \"captureTime\", operation = \"in\", \
                 value = \"2026-05-01\", value2 = \"2026-05-31\", }, }"
            )
            .unwrap(),
            "date>=2026-05-01 date<=2026-05-31"
        );
        // What has no counterpart says so.
        assert!(one("keywords", "any", "\"skye harbor\"").is_err());
        assert!(one("keywords", "empty", "nil").is_err());
        assert!(one("fileFormat", "==", "\"VIDEO\"").is_err());
        assert!(one("touchTime", "inLast", "2").is_err());
        assert!(one("captureTime", "inLast", "1").is_err());
        assert!(one("keywords", "all", "\"say \\\"cheese\\\"\"").is_err());
        let union = "s = { { criteria = \"rating\", operation = \"==\", value = 5, },\n\
                     { criteria = \"pick\", operation = \"==\", value = 1, },\n\
                     combine = \"union\", }";
        assert!(translate(union).unwrap_err().contains("any of"));
        assert!(translate("s = { combine = \"intersect\", }").is_err());
        assert!(translate("not a table").is_err());
    }

    #[test]
    fn keywords_labels_and_titles_read_as_the_catalog_keeps_them() {
        assert_eq!(label_of("Purple"), (Label::Purple, None));
        assert_eq!(label_of("red"), (Label::Red, None));
        assert_eq!(label_of(""), (Label::None, None));
        assert_eq!(label_of("Approved"), (Label::None, Some("Approved".into())));
        assert_eq!(xmp_title(&xmp_blob("Boats")).as_deref(), Some("Boats"));
        assert_eq!(xmp_title(b"junk"), None);
        let dir = scratch("lrcat-keywords");
        let db = empty_catalog(&dir.join("k.lrcat"), "1504001");
        db.execute_batch(
            "INSERT INTO AgLibraryKeyword VALUES (1, 'r', NULL, NULL);
             INSERT INTO AgLibraryKeyword VALUES (2, 'a', 'People', 1);
             INSERT INTO AgLibraryKeyword VALUES (3, 'b', 'Family', 2);
             INSERT INTO AgLibraryKeyword VALUES (4, 'c', 'Ana', 3);
             INSERT INTO AgLibraryKeyword VALUES (5, 'd', 'Dusk', 1);",
        )
        .unwrap();
        let paths = keyword_paths(&db).unwrap();
        assert_eq!(paths.get(&4).map(String::as_str), Some("People/Family/Ana"));
        assert_eq!(paths.get(&5).map(String::as_str), Some("Dusk"));
        assert!(!paths.contains_key(&1), "the tree's root is no keyword");
        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_write_ahead_catalog_is_read_with_its_log_and_without_one_makes_nothing_beside_it() {
        let s = setup("wal");
        // Lightroom's mode, the log checkpointed and gone at the close:
        // opened immutable, the folder as it was.
        Connection::open(&s.catalog)
            .unwrap()
            .execute_batch("PRAGMA journal_mode = WAL;")
            .unwrap();
        assert!(write_ahead(&s.catalog));
        let before = tree(&s.dir);
        let report = import(&s.catalog, &s.options).unwrap();
        assert_eq!(tree(&s.dir), before, "no -shm or -wal made");
        assert_eq!(report.wal, Wal::Absent);
        assert!(
            report
                .to_string()
                .contains("copy the catalog with its -wal")
        );
        assert_eq!(report.keyworded, 1);
        // A change still in the log, as while Lightroom runs: read.
        let lightroom = Connection::open(&s.catalog).unwrap();
        lightroom
            .execute_batch("INSERT INTO AgLibraryKeywordImage VALUES (3, 101, 5);")
            .unwrap();
        let mut log = s.catalog.as_os_str().to_owned();
        log.push("-wal");
        assert!(std::fs::metadata(&log).unwrap().len() > 0);
        let report = import(&s.catalog, &s.options).unwrap();
        assert_eq!(report.wal, Wal::Read);
        assert_eq!(report.keyworded, 2, "the row in the log");
        drop(lightroom);
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn a_new_sidecar_goes_where_the_folder_keeps_them_or_the_setting_says() {
        let s = setup("placement");
        let folder = s.here.join("2026/harbor");
        std::fs::create_dir_all(folder.join(greycard_edit::SIDECAR_FOLDER)).unwrap();
        let w = import(&s.catalog, &writing(&s)).unwrap().written.unwrap();
        assert_eq!((w.sidecars, w.in_folder), (2, 2));
        let raw = folder.join("IMG_0001.CR3");
        assert_eq!(
            Sidecar::find(&raw),
            Some(Sidecar::path_in(&raw, Placement::Folder))
        );
        assert!(!Sidecar::path_for(&raw).exists());
        std::fs::remove_dir_all(&s.dir).unwrap();

        // No .greycard in the folder: beside, unless the setting says.
        let s = setup("placement-setting");
        let report = import(
            &s.catalog,
            &Options {
                sidecars_in_folder: true,
                ..writing(&s)
            },
        )
        .unwrap();
        assert!(report.sidecars_in_folder);
        assert_eq!(report.written.unwrap().in_folder, 2);
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn stock_smart_collections_are_known_by_name_and_rule() {
        let stock = "s = {\n\t{\n\t\tcriteria = \"rating\",\n\t\toperation = \">=\",\n\
                     \t\tvalue = 5,\n\t},\n\tcombine = \"intersect\",\n}\n";
        assert!(is_stock("Five Stars", stock));
        assert!(!is_stock("My fives", stock), "another name is the user's");
        assert!(!is_stock(
            "Five Stars",
            &stock.replace("value = 5", "value = 4")
        ));
        assert!(!is_stock(
            "Five Stars",
            &stock.replace("\"intersect\"", "\"union\"")
        ));
        let empty = "s = { { criteria = \"keywords\", operation = \"empty\", }, \
                     combine = \"intersect\", }";
        assert!(is_stock("Without Keywords", empty));
        let months = "s = { { criteria = \"captureTime\", operation = \"inLast\", value = 1, \
                      value_units = \"months\", }, combine = \"intersect\", }";
        assert!(is_stock("Past Month", months));
        assert!(!is_stock("Past Month", &months.replace("months", "weeks")));
    }

    #[test]
    fn same_named_collections_join_and_only_a_plain_system_one_is_the_quick_collection() {
        let s = setup("names");
        let db = Connection::open(&s.catalog).unwrap();
        db.execute_batch(
            "INSERT INTO AgLibraryCollection VALUES (8, 'com.adobe.ag.library.collection',
                 'Cover', 1, 0.0);
             INSERT INTO AgLibraryCollectionImage VALUES (5, 8, 101, 'a');
             INSERT INTO AgLibraryCollection VALUES (9, 'com.adobe.ag.library.collection',
                 'Day/Night', NULL, 0.0);
             INSERT INTO AgLibraryCollection VALUES (10, 'com.adobe.ag.print',
                 'Prints', NULL, 1.0);",
        )
        .unwrap();
        drop(db);
        let report = import(&s.catalog, &writing(&s)).unwrap();
        let names: Vec<&str> = report.collections.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Harbor/Clients/Cover",
                "Harbor/Album",
                "Harbor/Clients/Cover",
                "Harbor/Day-Night",
                "Harbor/Prints"
            ],
            "a slash in a name is no set; a system print is no quick collection"
        );
        let w = report.written.unwrap();
        assert_eq!((w.collections, w.joined), (4, 1));
        let folder = collections::read_folder(
            &collections::dir_beside(&s.options.library),
            collections::VERSION,
        );
        let covers: Vec<&Collection> = folder.list.iter().filter(|c| c.name == "Cover").collect();
        assert_eq!(covers.len(), 1, "the second Cover joined the first");
        assert_eq!(covers[0].refs.len(), 3);
        std::fs::remove_dir_all(&s.dir).unwrap();
    }
}
