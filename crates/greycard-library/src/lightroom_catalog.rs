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
//! redo, the crop with them where the file says its size
//! ([`greycard_edit::lightroom::crop`]); what the mapper has no place
//! for is counted by name. A
//! virtual copy becomes a version of its file under the copy's name;
//! the meta is the file's, so a copy rated or labeled apart from its
//! master keeps the master's, and is counted. Collections and sets go
//! to the collections file under a set named for the catalog, a smart
//! collection when every rule has a term in the filter language (a
//! rule on the camera or the lens asked of the names the catalog's and
//! the library's cameras and lenses go by), and anything else is named
//! in the report with why; Lightroom's six stock smart collections, as
//! every catalog has them, are counted and left out. Stacks go to the
//! stacks' files beside the collections, a virtual copy in one as its
//! version.
//!
//! **Where the files are now.** A root folder's path is the one it had
//! on the machine the catalog lived on, so each is mapped to where it
//! is now (`--root A:/Photos=/photos`, the catalog's path or a folder
//! above it, or the root's name), and a mapping is taken only when
//! some of the catalog's files are found under it. A root not mapped
//! is looked for where the catalog says it is, then in a folder of its
//! name beside the catalog or under the library's roots.
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
use std::sync::atomic::{AtomicBool, Ordering};

use greycard_edit::lightroom::{self, Value};
use greycard_edit::meta::{Flag, Label, Meta, STARS};
use greycard_edit::{Edit, Placement, Sidecar};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use greycard_edit::lightroom::crop::Crop;

use crate::collections::{self, Collection, Reference, Smart};
use crate::filter::{Filter, Saved};
use crate::roots::{self, Roots};
use crate::stacks::{self, Stack};

mod sheet;
pub use sheet::Sheet;

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
    /// Told to stop before it wrote anything: [`import_until`].
    #[error("stopped before it was done")]
    Stopped,
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
    /// Leave out the books, prints, slideshows and web pages; without
    /// it, each comes as a collection of its frames.
    pub no_creations: bool,
}

/// How far a long import has come, for a caller that shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// The catalog read; its roots' folders being looked for, the
    /// `done`th of `of` under way. Each is looked for where the catalog
    /// says, beside the catalog and three deep under the library's
    /// roots, which over a network share is the slow part.
    Mapping { done: usize, of: usize },
    /// The roots' folders found; their files being looked for, and their
    /// sidecars read.
    Looking { done: usize, of: usize },
    /// The sidecars being written, a file at a time.
    Writing { done: usize, of: usize },
    /// The collections, the stacks and the roots being written.
    Finishing,
}

/// How far the import has come, as the sheet, the status line and the
/// command say it.
impl fmt::Display for Progress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = crate::grouped;
        match *self {
            Progress::Mapping { done, of } => {
                write!(
                    f,
                    "looking for the catalog's folders, {} of {}",
                    n(done),
                    n(of)
                )
            }
            Progress::Looking { done, of } => {
                write!(
                    f,
                    "looking for the catalog's files, {} of {}",
                    n(done),
                    n(of)
                )
            }
            Progress::Writing { done, of } => write!(f, "writing {} of {} frames", n(done), n(of)),
            Progress::Finishing => f.write_str("the collections, the stacks and the roots"),
        }
    }
}

impl Progress {
    /// The status line's and the command's line for how far the import
    /// has come.
    pub fn line(&self) -> String {
        format!("importing the catalog: {self}")
    }
}

/// Which of the import's steps a terminal or a log says: a line a
/// second at most through a step's counts, but each of the catalog's
/// folders (each slow to look for over a share), a step's first count
/// and its last, and the finish always. The sheet's throttle, at a
/// pace a person reads rather than a window redraws.
#[derive(Debug, Default)]
pub struct Pace {
    last: Option<(std::time::Instant, std::mem::Discriminant<Progress>)>,
}

impl Pace {
    /// Whether `p`, heard at `now`, is said.
    pub fn due(&mut self, p: Progress, now: std::time::Instant) -> bool {
        let kind = std::mem::discriminant(&p);
        let ends = match p {
            Progress::Mapping { .. } | Progress::Finishing => true,
            Progress::Looking { done, of } | Progress::Writing { done, of } => done >= of,
        };
        let due = match self.last {
            None => true,
            Some((at, was)) => {
                ends || was != kind || now.duration_since(at) >= std::time::Duration::from_secs(1)
            }
        };
        if due {
            self.last = Some((now, kind));
        }
        due
    }
}

/// What the catalog keeps of a file's frame, which its crop is laid
/// over: its size before any turn and Lightroom's orientation code.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shape {
    pub size: Option<(f64, f64)>,
    pub orientation: Option<String>,
}

/// A stack of the catalog: its images, the top first.
#[derive(Debug, Clone)]
pub(crate) struct CatalogStack {
    pub(crate) images: Vec<i64>,
    pub(crate) collapsed: bool,
}

/// A root folder of the catalog.
#[derive(Debug, Clone)]
pub(crate) struct CatalogRoot {
    pub(crate) id: i64,
    /// As the catalog has it: absolute, on the machine it lived on.
    pub(crate) path: String,
    pub(crate) name: String,
}

/// One image of the catalog: a file's own, or a virtual copy of it.
#[derive(Debug, Clone)]
pub(crate) struct Image {
    pub(crate) id: i64,
    pub(crate) file: i64,
    pub(crate) root: i64,
    /// The file under its root, slash-separated.
    pub(crate) relative: String,
    /// The image it is a virtual copy of.
    pub(crate) master: Option<i64>,
    pub(crate) copy_name: Option<String>,
    pub(crate) meta: Meta,
    /// A label whose text is no color's (a renamed label set).
    pub(crate) odd_label: Option<String>,
    pub(crate) develop: Option<String>,
    pub(crate) process: Option<String>,
    pub(crate) edited: bool,
    pub(crate) shape: Shape,
    /// The file's size in bytes when it was imported, as the catalog's
    /// `importHash` ends with it.
    pub(crate) size: Option<u64>,
    /// `Adobe_images.captureTime`: Lightroom's capture time for the
    /// image, local with no zone (`2022-11-02T09:05:33`), which differs
    /// from the file's own when the time was corrected there.
    pub(crate) captured: Option<String>,
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
pub(crate) struct Catalog {
    pub(crate) schema: i64,
    pub(crate) written_by: &'static str,
    pub(crate) roots: Vec<CatalogRoot>,
    pub(crate) images: Vec<Image>,
    nodes: Vec<Node>,
    pub(crate) stacks: Vec<CatalogStack>,
    pub(crate) wal: Wal,
    /// The cameras and the lenses its images name, as Lightroom spells
    /// them (the files' EXIF), and which image names which.
    pub(crate) cameras: Vec<String>,
    pub(crate) lenses: Vec<String>,
    pub(crate) exif: Vec<ExifNames>,
}

/// The camera and the lens an image of the catalog names.
#[derive(Debug, Clone)]
pub(crate) struct ExifNames {
    pub(crate) image: i64,
    pub(crate) camera: Option<String>,
    pub(crate) lens: Option<String>,
}

/// How a root's folder here was found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Found {
    /// Not found, and not mapped.
    #[default]
    Not,
    /// A mapping said so.
    Given,
    /// Where the catalog says.
    AsCatalogSays,
    /// A folder of the root's name beside the catalog, or the catalog's
    /// own folder by that name.
    BesideCatalog,
    /// A folder of the root's name under this root of the library.
    UnderRoot(PathBuf),
}

/// One root of the catalog: where it is now and what was found there.
#[derive(Debug, Clone, PartialEq)]
pub struct RootReport {
    /// As the catalog has it.
    pub catalog: String,
    pub name: String,
    /// Where it was looked for, and how that was found.
    pub local: Option<PathBuf>,
    pub how: Found,
    /// Of the first few files looked for, how many were there.
    pub checked: usize,
    pub checked_found: usize,
    /// Whether the mapping was taken: some of its files are there.
    pub confirmed: bool,
    pub files: usize,
    pub found: usize,
    /// The nearest a guess came when it took nothing: a folder of the
    /// root's name holding some of the sample, short of all but one or
    /// at other sizes.
    pub near_miss: Option<NearMiss>,
}

/// A folder of a root's name a guess did not take, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct NearMiss {
    pub dir: PathBuf,
    /// Of the files looked for, how many it holds.
    pub there: usize,
    pub looked_for: usize,
    /// Most of those whose size the catalog keeps are at another size.
    pub other_sizes: bool,
}

impl NearMiss {
    /// What a report says of it, a folder chosen being taken on any file.
    pub fn said(&self, voice: Voice) -> String {
        let take = match voice {
            Voice::Command => "--root it to take it",
            Voice::Sheet => "choose it to take it",
        };
        if self.other_sizes {
            format!(
                "{} holds {} of {} looked for, most at other sizes than the catalog's, \
                 another shoot's files of the same names it may be; {take}",
                self.dir.display(),
                self.there,
                self.looked_for
            )
        } else {
            format!(
                "{} holds {} of {} looked for; {take}",
                self.dir.display(),
                self.there,
                self.looked_for
            )
        }
    }
}

/// What a write did with one of the catalog's roots found here.
#[derive(Debug, Clone, PartialEq)]
pub enum Rooted {
    Added,
    /// Added, taking in this many roots under it.
    Absorbed(usize),
    Already,
    /// Under this root of the library already.
    Under(PathBuf),
    NotAdded(String),
}

impl Rooted {
    /// Whether the library has a new root for it.
    pub fn added(&self) -> bool {
        matches!(self, Rooted::Added | Rooted::Absorbed(_))
    }

    /// What a report says of it, its paths said through `path`.
    pub fn said(&self, path: &dyn Fn(&Path) -> String) -> String {
        match self {
            Rooted::Added => "added".to_string(),
            Rooted::Absorbed(n) => format!("added, taking in {n} roots under it"),
            Rooted::Already => "a root already".to_string(),
            Rooted::Under(by) => format!("under {}", path(by)),
            Rooted::NotAdded(e) => format!("not added: {e}"),
        }
    }
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
    /// Of the sidecars, those written over with `force`: their frame's
    /// sidecar held history, versions or meta, or would not read.
    pub written_over: usize,
    pub failed: Vec<(PathBuf, String)>,
    /// Each root found, and what became of it in the library's roots.
    pub roots: Vec<(PathBuf, Rooted)>,
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
    /// Crops carried, those drawn in to stay on the turned picture, and
    /// those not carried by why.
    pub crops: usize,
    pub crops_drawn_in: usize,
    pub crops_not_carried: BTreeMap<String, usize>,
    /// Frames turned in Lightroom whose turn was carried; those Flipped
    /// there, whose mirror is not; and orientation codes not read.
    pub turned: usize,
    pub flips: usize,
    pub orientations_unread: BTreeMap<String, usize>,
    /// The raws whose sidecars were written, as the import found them.
    pub frames: Vec<PathBuf>,
    /// Stacks written, there already, and not written by why.
    pub stacks: usize,
    pub stacks_there: usize,
    pub stacks_not_written: BTreeMap<String, usize>,
    /// Frames given the shift that carries the capture time corrected
    /// in Lightroom, and those with a Lightroom time whose file's own
    /// could not be read to compare it with.
    pub time_shifted: usize,
    pub time_unread: usize,
    /// Bytes of the raws read for their probes and stances, for the log.
    pub read: u64,
}

impl Written {
    /// What one frame's write counted, added to these: the counts the
    /// frame's sidecar and its crops and turns make.
    fn absorb(&mut self, frame: Written) {
        self.crops += frame.crops;
        self.crops_drawn_in += frame.crops_drawn_in;
        for (why, n) in frame.crops_not_carried {
            *self.crops_not_carried.entry(why).or_default() += n;
        }
        self.turned += frame.turned;
        self.flips += frame.flips;
        for (code, n) in frame.orientations_unread {
            *self.orientations_unread.entry(code).or_default() += n;
        }
        self.time_shifted += frame.time_shifted;
        self.time_unread += frame.time_unread;
        self.read += frame.read;
    }
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
    /// The developed images with a crop, and those whose file was found:
    /// each carried where the file says its size.
    pub crops: usize,
    pub crops_found: usize,
    /// The images with Lightroom's orientation; those whose orientation
    /// there is mirrored (a Flip), and those found; codes not read.
    pub oriented: usize,
    pub flipped: usize,
    pub flipped_found: usize,
    pub orientations_unread: BTreeMap<String, usize>,
    pub stacks: usize,
    /// Stacks with at least two of their images' files found: what a
    /// write can carry.
    pub stacks_found: usize,
    pub sets: usize,
    pub collections: Vec<CollectionReport>,
    /// Each smart collection translated, as typed, with its filter.
    pub smart: Vec<(String, String)>,
    /// Each one not, with why.
    pub smart_skipped: Vec<(String, String)>,
    /// Lightroom's own six as every catalog has them, left out.
    pub smart_stock: usize,
    /// The smart collections with a rule on a camera or a lens:
    /// translated, and skipped for whatever reason (a rule matching no
    /// camera or lens known, one matching several, or any other rule of
    /// the collection with no term).
    pub exif_rules: usize,
    pub exif_rules_skipped: usize,
    /// The books, prints, slideshows and web pages left out, by name.
    pub creations_left: Vec<String>,
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
    import_with(path, options, &mut |_| {})
}

/// [`import`], saying how far it has come to `progress` as it goes.
pub fn import_with(
    path: &Path,
    options: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Report, ImportError> {
    import_until(path, options, progress, &AtomicBool::new(false))
}

/// [`import_with`], ending in [`ImportError::Stopped`] soon after `stop`
/// is set: between one folder or file looked at and the next. A write
/// heeds it until its first sidecar is written and goes to its end
/// after, so a stop never leaves it half done.
pub fn import_until(
    path: &Path,
    options: &Options,
    progress: &mut dyn FnMut(Progress),
    stop: &AtomicBool,
) -> Result<Report, ImportError> {
    import_on(path, options, progress, stop, write_threads())
}

/// [`import_until`] writing `threads` frames at once.
fn import_on(
    path: &Path,
    options: &Options,
    told: &mut dyn FnMut(Progress),
    stop: &AtomicBool,
    threads: usize,
) -> Result<Report, ImportError> {
    let stopped = || {
        if stop.load(Ordering::Relaxed) {
            Err(ImportError::Stopped)
        } else {
            Ok(())
        }
    };
    // The log hears what the sheet and the command say, at a line a
    // second: a night's write over a share is otherwise silent in it.
    let mut pace = Pace::default();
    let progress: &mut dyn FnMut(Progress) = &mut |p| {
        if pace.due(p, std::time::Instant::now()) {
            log::info!("{}", p.line());
        }
        told(p);
    };
    let catalog = read(path)?;
    log::info!(
        "importing the catalog {} ({}, schema {}), {}: {} roots, {} images, {} files",
        crate::tilde(path),
        catalog.written_by,
        catalog.schema,
        match (options.write, options.force) {
            (false, _) => "a dry run",
            (true, false) => "a write",
            (true, true) => "a forced write",
        },
        catalog.roots.len(),
        catalog.images.len(),
        catalog.images.iter().filter(|i| i.master.is_none()).count(),
    );
    stopped()?;
    let near = Near::of(path, &options.library);
    let (roots, local_roots) = map_roots(&catalog, &options.roots, &near, progress, stop)?;
    // A stop during the last root's search left its candidates short.
    stopped()?;
    let mut report = Report {
        catalog: path.to_path_buf(),
        schema: catalog.schema,
        written_by: catalog.written_by.to_string(),
        images: catalog.images.len(),
        stacks: catalog.stacks.len(),
        sidecars_in_folder: options.sidecars_in_folder,
        wal: catalog.wal,
        ..Report::default()
    };
    // Looking for each file, then reading each one's sidecar.
    let looking =
        catalog.images.len() + catalog.images.iter().filter(|i| i.master.is_none()).count();
    let mut looked = 0;
    // Where each image's file is, when its root is mapped and it is
    // there.
    let mut at: HashMap<i64, PathBuf> = HashMap::new();
    let mut files: HashMap<i64, bool> = HashMap::new();
    for image in &catalog.images {
        stopped()?;
        looked += 1;
        if looked % 25 == 0 {
            progress(Progress::Looking {
                done: looked,
                of: looking,
            });
        }
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
                    // The crop is the frame's, carried beside the
                    // mapper's preset where the file says its size; one
                    // that does not read is not, and stays named.
                    let unread = image
                        .develop
                        .as_deref()
                        .is_some_and(|t| Crop::unreadable(&crate::lightroom_exports::words(t)));
                    for name in imported
                        .unmapped
                        .into_iter()
                        .filter(|n| n != "crop" || unread)
                    {
                        *report.not_carried.entry(name).or_default() += 1;
                    }
                }
                _ => report.unreadable_develop += 1,
            }
            if crop_of(image).is_some() {
                report.crops += 1;
                report.crops_found += usize::from(found);
            }
        }
        if let Some(code) = image.shape.orientation.as_deref().map(str::trim) {
            report.oriented += 1;
            match lightroom::crop::orientation_of(code) {
                Some(o) if lightroom::crop::mirrors(o) => {
                    report.flipped += 1;
                    report.flipped_found += usize::from(found);
                }
                Some(_) => {}
                None => {
                    *report
                        .orientations_unread
                        .entry(code.to_string())
                        .or_default() += 1
                }
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
        stopped()?;
        looked += 1;
        if looked % 25 == 0 {
            progress(Progress::Looking {
                done: looked,
                of: looking,
            });
        }
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
    progress(Progress::Looking {
        done: looked,
        of: looking,
    });

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
    let known = Known::of(&catalog, &options.library, &at);
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
            Kind::Creation(_) if options.no_creations => report.creations_left.push(name),
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
            Kind::Smart(rules) => match translate(rules, &known) {
                Ok(text) => {
                    report.exif_rules += usize::from(asks_exif(rules));
                    report.smart.push((name.clone(), text.clone()));
                    smart.push((
                        name,
                        Saved {
                            text,
                            ..Saved::default()
                        },
                    ));
                }
                Err(why) => {
                    report.exif_rules_skipped += usize::from(asks_exif(rules));
                    report.smart_skipped.push((name, why));
                }
            },
        }
    }
    report.stacks_found = catalog
        .stacks
        .iter()
        .filter(|st| st.images.iter().filter(|i| at.contains_key(i)).count() >= 2)
        .count();

    log::info!(
        "the catalog read: {} of {} images found, {} roots found, {} with edits, \
         {} collections, {} smart, {} stacks, {} sidecars there already",
        report.found,
        report.images,
        report.roots.iter().filter(|r| r.confirmed).count(),
        report.with_edits_found,
        report.collections.len(),
        report.smart.len(),
        report.stacks_found,
        report.already,
    );
    if !options.write {
        return Ok(report);
    }
    // The last word a stop has: from here the write goes to its end.
    stopped()?;
    let mut written = Written::default();

    // The sidecars: each file once, its own image and its copies.
    let mut copies_of: HashMap<i64, Vec<&Image>> = HashMap::new();
    for c in &catalog.images {
        if let Some(master) = c.master {
            copies_of.entry(master).or_default().push(c);
        }
    }
    let frames: Vec<Frame<'_>> = catalog
        .images
        .iter()
        .filter(|i| i.master.is_none())
        .filter_map(|image| {
            Some(Frame {
                raw: at.get(&image.id)?,
                image,
                copies: copies_of.get(&image.id).map_or(&[], Vec::as_slice),
                held: kept.get(&image.file).copied().unwrap_or(false),
            })
        })
        .collect();
    let started = std::time::Instant::now();
    let done = write_frames(&frames, options, threads, progress);
    // In the catalog's order, as one frame after another would have.
    let mut versions: HashMap<i64, Option<String>> = HashMap::new();
    for (frame, done) in frames.iter().zip(done) {
        match done {
            Wrote::LeftAlone(found) => {
                written.left_alone += 1;
                versions.extend(found);
            }
            Wrote::Frame(Ok((wrote, made)), counted) => {
                written.absorb(*counted);
                if let Some(placement) = wrote {
                    written.sidecars += 1;
                    if frame.held {
                        written.written_over += 1;
                    }
                    written.frames.push(frame.raw.clone());
                    if placement == Placement::Folder {
                        written.in_folder += 1;
                    }
                }
                for (id, version) in made {
                    versions.insert(id, Some(version));
                }
            }
            Wrote::Frame(Err(e), counted) => {
                written.absorb(*counted);
                // The error alone: a tester pastes the log, and the
                // report names the frames.
                log::warn!(
                    "a frame not written ({} so far): {e}",
                    written.failed.len() + 1
                );
                written.failed.push((frame.raw.clone(), e));
            }
        }
    }
    log::info!(
        "the catalog's sidecars: {} frames in {:.1} s, {:.1} MB of raws read",
        frames.len(),
        started.elapsed().as_secs_f64(),
        written.read as f64 / 1e6
    );

    progress(Progress::Finishing);
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

    write_stacks(&catalog, &at, &versions, &mut hashes, &dir, &mut written)?;

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
                    Rooted::Already
                } else {
                    Rooted::Under(by.to_path_buf())
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
                        Ok(roots::Added::New(_)) => Rooted::Added,
                        Ok(roots::Added::Covered(by)) if by == dir => Rooted::Already,
                        Ok(roots::Added::Covered(by)) => Rooted::Under(by),
                        Ok(roots::Added::Absorbed(_, under)) => Rooted::Absorbed(under.len()),
                        Err(e) => Rooted::NotAdded(e.to_string()),
                    };
                    (dir, said)
                })
                .collect::<Vec<_>>()
        })?;
        written.roots = added;
    }
    log::info!(
        "the catalog written: {} sidecars ({} written over), {} left alone, {} failed; \
         {} collections, {} joined, {} smart, {} stacks",
        written.sidecars,
        written.written_over,
        written.left_alone,
        written.failed.len(),
        written.collections,
        written.joined,
        written.smart,
        written.stacks,
    );
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

/// The edit a frame nobody has developed starts from, and the body
/// that shot it as the library names one (none for a picture, or a
/// file that will not probe), which the Lightroom mapper's offset is
/// for; another importer takes the edit alone. From the file's ends
/// where its format allows.
pub(crate) fn starting_edit(raw: &Path) -> (Edit, Option<String>) {
    if greycard_core::picture::is_picture_path(raw) {
        return (Edit::for_picture(), None);
    }
    starting_of(greycard_core::decode::probe_ends_path(raw).ok().as_ref())
}

/// A raw's starting edit and its body from its probe.
fn starting_of(probe: Option<&greycard_core::decode::Probe>) -> (Edit, Option<String>) {
    let mut edit = Edit::default();
    edit.noise.learned_strength = greycard_edit::Noise::blend_for_iso(probe.and_then(|p| p.iso));
    let camera = probe.map(|p| greycard_core::raw::camera_name(&p.make, &p.model));
    (edit, camera)
}

/// What an import wants of a frame's file, from one read of it: the
/// edit it starts from and its body ([`starting_edit`]), its own
/// capture time (EXIF's form), and how it stands, which its turn and
/// its crop are laid over. A raw's probe and stance come from one
/// decoder on its two ends where its format allows
/// ([`greycard_core::decode::probe_and_stance_counted`]), not the whole
/// file once for the probe and again for every stance asked.
struct FrameRead {
    starting: Edit,
    camera: Option<String>,
    taken: Option<String>,
    stance: Option<greycard_core::decode::Stance>,
    /// Bytes of a raw read for it; none counted for a picture.
    bytes: u64,
}

impl FrameRead {
    fn of(raw: &Path) -> FrameRead {
        if greycard_core::picture::is_picture_path(raw) {
            // A picture's probe is its head; its stance, the whole of it.
            return FrameRead {
                starting: Edit::for_picture(),
                camera: None,
                taken: greycard_core::picture::probe_path(raw)
                    .ok()
                    .and_then(|p| p.taken),
                stance: greycard_core::decode::stance_path(raw).ok(),
                bytes: 0,
            };
        }
        // A clip or a layered file in the catalog is no raw: nothing of
        // it read, as the probes' errors left it before.
        let read = greycard_core::decode::is_raw_path(raw)
            .then(|| greycard_core::decode::probe_and_stance_counted(raw));
        let (probe, stance, bytes) = match read {
            Some(Ok((probe, stance, bytes))) => (Some(probe), Some(stance), bytes),
            Some(Err(e)) => {
                log::debug!("{}: {e}", raw.display());
                (None, None, 0)
            }
            None => (None, None, 0),
        };
        let (starting, camera) = starting_of(probe.as_ref());
        FrameRead {
            starting,
            camera,
            taken: probe.and_then(|p| p.taken),
            stance,
            bytes,
        }
    }
}

/// The shift in seconds that takes a file's own capture time to the one
/// Lightroom holds for it: none when either does not read as a date,
/// when they agree to within a second (Lightroom rounds), or when they
/// are further apart than a shift can be (two dates that were never
/// the same moment).
fn catalog_shift(catalog: &str, file: &str) -> Option<i64> {
    let by = greycard_core::exif::seconds_of(catalog)?
        .checked_sub(greycard_core::exif::seconds_of(file)?)?;
    (by.abs() >= 2 && by.abs() <= greycard_edit::meta::SHIFT_LIMIT).then_some(by)
}

/// The crop an image's develop settings hold, and whether Lightroom
/// held it to its shape; none when they hold none.
fn crop_of(image: &Image) -> Option<(Crop, bool)> {
    if !image.edited {
        return None;
    }
    crop_in(image.develop.as_deref()?)
}

/// The crop develop settings hold, and whether Lightroom held it to its
/// shape (`CropConstrainAspectRatio`).
fn crop_in(settings: &str) -> Option<(Crop, bool)> {
    let words = crate::lightroom_exports::words(settings);
    let constrained = words
        .get("CropConstrainAspectRatio")
        .is_some_and(|v| v.eq_ignore_ascii_case("true"));
    Crop::of(&words).map(|c| (c, constrained))
}

/// What became of an image's crop.
#[derive(Debug, Clone, PartialEq)]
pub enum Cropped {
    /// The settings hold none.
    None,
    /// Carried, and whether drawn in to stay on the turned picture.
    Carried { drawn_in: bool },
    /// Not carried, and why.
    Not(String),
}

/// The catalog's crop laid onto `edit`'s geometry, for the file that
/// stands as `stance` says (none when it would not say), its frame
/// turned `turn` quarters over its tag: the settings' crop placed in
/// the frame as the decoder reads its size and its tag, against the
/// catalog's own size and orientation in `shape`.
pub fn crop_onto(
    edit: &mut Edit,
    stance: Option<&greycard_core::decode::Stance>,
    settings: &str,
    shape: &Shape,
    turn: u8,
) -> Cropped {
    let unread = || Cropped::Not("its crop does not read".into());
    if Crop::unreadable(&crate::lightroom_exports::words(settings)) {
        return unread();
    }
    let Some((crop, constrained)) = crop_in(settings) else {
        return Cropped::None;
    };
    let no_size = || Cropped::Not("its file says no size".into());
    let Some(stance) = stance else {
        return no_size();
    };
    let Some((w, h)) = stance.size else {
        return no_size();
    };
    let frame = lightroom::crop::Frame {
        size: (f64::from(w), f64::from(h)),
        catalog: shape.size,
        tag: stance.orientation,
        turn,
        lightroom: shape
            .orientation
            .as_deref()
            .and_then(lightroom::crop::orientation_of),
    };
    match lightroom::crop::place(&crop, constrained, &frame) {
        Ok(placed) => {
            edit.geometry = placed.applied(&edit.geometry);
            Cropped::Carried {
                drawn_in: placed.drawn_in,
            }
        }
        Err(lightroom::crop::NotPlaced::Mirrored) => {
            Cropped::Not("Lightroom's orientation mirrors the frame".into())
        }
        Err(lightroom::crop::NotPlaced::NoSize) => no_size(),
        Err(lightroom::crop::NotPlaced::Unreadable) => unread(),
    }
}

/// What Lightroom's orientation of an image did to its edit.
#[derive(Debug, Clone, PartialEq)]
pub enum Turned {
    /// It stands as the frame does here, or the catalog does not say.
    Same,
    /// Turned in Lightroom: the edit's quarter turns take the turn.
    Turned,
    /// Mirrored in Lightroom where the frame is not, or the other way:
    /// a Flip, which no quarter turn carries.
    Mirrored,
    /// A code this import does not read, as the catalog spells it.
    Unread(String),
}

/// Lightroom's orientation of the image laid onto `edit`'s quarter
/// turns, for the file that stands as `stance` says (none when it would
/// not say) turned `turn` quarters over its tag: every frame written
/// takes its turn in Lightroom, cropped or not.
pub fn turn_onto(
    edit: &mut Edit,
    stance: Option<&greycard_core::decode::Stance>,
    shape: &Shape,
    turn: u8,
) -> Turned {
    let Some(code) = shape.orientation.as_deref().map(str::trim) else {
        return Turned::Same;
    };
    let Some(lightroom) = lightroom::crop::orientation_of(code) else {
        return Turned::Unread(code.to_string());
    };
    let Some(stance) = stance else {
        return Turned::Same;
    };
    let shown = stance.orientation.turned(i32::from(turn % 4));
    match lightroom::crop::turns_for(shown, lightroom) {
        Ok(turns) => {
            edit.geometry.turns = turns;
            if turns == 0 {
                Turned::Same
            } else {
                Turned::Turned
            }
        }
        Err(_) => Turned::Mirrored,
    }
}

/// The catalog's develop settings for `image` laid over `base`, its crop
/// and its turn in Lightroom with them, for the file standing as
/// `stance` says turned `turn` over its tag; and what became of the
/// crop and the turn. An image never developed takes only its turn,
/// when it has one; there is no edit when it has neither or the
/// settings will not read.
fn developed(
    image: &Image,
    base: &Edit,
    stance: Option<&greycard_core::decode::Stance>,
    camera: Option<&str>,
    turn: u8,
) -> (Option<Edit>, Cropped, Turned) {
    let mut edit = base.clone();
    if !image.edited {
        let turned = turn_onto(&mut edit, stance, &image.shape, turn);
        let carried = (turned == Turned::Turned).then_some(edit);
        return (carried, Cropped::None, turned);
    }
    let Some(imported) = image
        .develop
        .as_deref()
        .and_then(|s| lightroom::read_table_for(s, "", camera).ok())
    else {
        return (None, Cropped::None, Turned::Same);
    };
    let settings = image.develop.as_deref().unwrap_or_default();
    let mut edit = imported.preset.applied(base);
    let turned = turn_onto(&mut edit, stance, &image.shape, turn);
    let cropped = crop_onto(&mut edit, stance, settings, &image.shape, turn);
    (Some(edit), cropped, turned)
}

/// What [`imported_edit`] gives: the edit, what the mapper had no
/// place for, the body the offset was looked up for and the offset
/// applied.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameEdit {
    pub edit: Edit,
    pub unmapped: Vec<String>,
    /// The body as the library names it, none when the file would not
    /// probe.
    pub camera: Option<String>,
    /// The stops added to the Exposure slider for the body
    /// ([`lightroom::body_offset`]).
    pub offset: f32,
}

/// The edit an import writes for the raw at `raw` from the develop
/// settings `settings` (a catalog's Lua table) when the frame has no
/// sidecar yet: the settings mapped for the frame's body and laid over
/// the frame's starting edit, and with `shape` (the catalog's size and
/// orientation of the file) its crop. Beside it, what the mapper had
/// no place for, the crop among them when it was not carried. Without
/// `shape` the crop is left off and named, for a caller that cuts
/// Lightroom's crop itself.
pub fn imported_edit(
    raw: &Path,
    settings: &str,
    shape: Option<&Shape>,
) -> greycard_edit::Result<FrameEdit> {
    imported_edit_with(raw, settings, shape, &lightroom::Scales::DEFAULT)
}

/// [`imported_edit`] under `scales` rather than the shipped ones: the
/// measurement's way to read an edit as an earlier or a proposed
/// mapper would.
pub fn imported_edit_with(
    raw: &Path,
    settings: &str,
    shape: Option<&Shape>,
    scales: &lightroom::Scales,
) -> greycard_edit::Result<FrameEdit> {
    let FrameRead {
        starting,
        camera,
        stance,
        ..
    } = FrameRead::of(raw);
    let offset = lightroom::body_offset(camera.as_deref());
    let imported = lightroom::read_table_with(settings, "", scales, offset)?;
    let mut edit = imported.preset.applied(&starting);
    let mut unmapped = imported.unmapped;
    if let Some(shape) = shape {
        turn_onto(&mut edit, stance.as_ref(), shape, 0);
        if let Cropped::Carried { .. } = crop_onto(&mut edit, stance.as_ref(), settings, shape, 0) {
            unmapped.retain(|n| n != "crop");
        }
    }
    Ok(FrameEdit {
        edit,
        unmapped,
        camera,
        offset: imported.offset,
    })
}

/// Where a frame's sidecar goes: where it is, when it has one; else
/// under the folder's `.greycard` when the folder has one, so a shoot
/// whose sidecars live there keeps them together; else as the editor's
/// setting says.
pub(crate) fn placement_for(raw: &Path, in_folder: bool) -> Placement {
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

/// How many frames a write works on at once: [`WRITE_THREADS`], or
/// `GREYCARD_IMPORT_THREADS` when that is set, for measuring.
fn write_threads() -> usize {
    std::env::var("GREYCARD_IMPORT_THREADS")
        .ok()
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(WRITE_THREADS)
}

/// Frames written at once. Each frame's reads and its sidecar's write
/// are a run of round trips, and on a share it is their latency that
/// counts, so a few frames are in flight at a time; a share on one
/// spinning disk gains nothing from more, since the reads then wait on
/// its seeks.
const WRITE_THREADS: usize = 4;

/// The one permit to read a raw during a write: see [`write_frame`].
static READING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A file the write takes: its own image, its copies, and whether its
/// sidecar holds something already.
struct Frame<'a> {
    raw: &'a PathBuf,
    image: &'a Image,
    copies: &'a [&'a Image],
    held: bool,
}

/// What became of a frame.
enum Wrote {
    /// Left alone, its sidecar holding something already: the version
    /// each copy found by its name there, as a re-import made it.
    LeftAlone(Vec<(i64, Option<String>)>),
    /// Written, or failed, and what the frame counted.
    Frame(Result<FrameWritten, String>, Box<Written>),
}

/// Each frame written on `threads` scoped threads, and what became of
/// each in `frames`' order. `progress` hears a count of the frames done
/// as they finish, on the calling thread, one more each time.
///
/// Frames whose files are one file here (two roots on one folder, or
/// names apart only in case on a volume that does not tell case) are
/// one job, written one after another in the catalog's order, as the
/// write one frame at a time had them: side by side, both would load no
/// sidecar and the last rename would win, its copies' versions lost to
/// the collections that name them. Names are told apart without case,
/// which costs a volume that does tell case nothing but the overlap.
fn write_frames(
    frames: &[Frame<'_>],
    options: &Options,
    threads: usize,
    progress: &mut dyn FnMut(Progress),
) -> Vec<Wrote> {
    use std::sync::atomic::AtomicUsize;
    let of = frames.len();
    if of == 0 {
        return Vec::new();
    }
    let mut jobs: Vec<Vec<usize>> = Vec::new();
    let mut job_of: HashMap<String, usize> = HashMap::new();
    for (i, frame) in frames.iter().enumerate() {
        let key = frame.raw.to_string_lossy().to_lowercase();
        let at = *job_of.entry(key).or_insert_with(|| {
            jobs.push(Vec::new());
            jobs.len() - 1
        });
        jobs[at].push(i);
    }
    progress(Progress::Writing { done: 0, of });
    let next = AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel::<(usize, Wrote)>();
    let mut out: Vec<Option<Wrote>> = std::iter::repeat_with(|| None).take(of).collect();
    std::thread::scope(|s| {
        for _ in 0..threads.clamp(1, jobs.len()) {
            let tx = tx.clone();
            let (next, jobs) = (&next, &jobs);
            s.spawn(move || {
                loop {
                    let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        return;
                    };
                    for &i in job {
                        if tx.send((i, write_one(&frames[i], options))).is_err() {
                            return;
                        }
                    }
                }
            });
        }
        drop(tx);
        // Ends when every worker has: done, or gone in a panic, which
        // the scope's end carries on.
        for (done, (i, wrote)) in rx.iter().enumerate() {
            out[i] = Some(wrote);
            progress(Progress::Writing { done: done + 1, of });
        }
    });
    out.into_iter()
        .map(|w| w.expect("every frame written"))
        .collect()
}

/// One frame: left alone when its sidecar holds something and the
/// write is not forced, else written.
fn write_one(frame: &Frame<'_>, options: &Options) -> Wrote {
    if frame.held && !options.force {
        // A re-import's copies are the versions it made last time,
        // found by their names.
        let mut found = Vec::new();
        if let Ok(Some(s)) = Sidecar::load(frame.raw) {
            for c in frame.copies {
                let name = copy_name(c);
                found.push((
                    c.id,
                    s.versions
                        .iter()
                        .find(|v| v.name == name)
                        .map(|v| v.id.clone()),
                ));
            }
        }
        return Wrote::LeftAlone(found);
    }
    let mut counted = Written::default();
    let wrote = write_frame(
        frame.raw,
        frame.image,
        frame.copies,
        options.sidecars_in_folder,
        &mut counted,
    );
    Wrote::Frame(wrote, Box::new(counted))
}

/// One file's sidecar written: the meta, the develop as a named state,
/// each copy as a version. Where it was written when it was, and each
/// copy's version id.
fn write_frame(
    raw: &Path,
    image: &Image,
    copies: &[&Image],
    in_folder: bool,
    written: &mut Written,
) -> Result<FrameWritten, String> {
    let found = Sidecar::load(raw).map_err(|e| e.to_string())?;
    let FrameRead {
        starting,
        camera,
        taken,
        stance,
        bytes,
    } = {
        // One frame's raw read at a time: on a share that waits on its
        // disk's seeks, readers side by side are slower than one; the
        // threads win on the sidecars' writes, a run of round trips.
        let _one = READING.lock().unwrap_or_else(|e| e.into_inner());
        FrameRead::of(raw)
    };
    written.read += bytes;
    let stance = stance.as_ref();
    let mut sidecar = found.unwrap_or_else(|| Sidecar {
        line: greycard_edit::Line::of(starting.clone()),
        ..Sidecar::default()
    });
    // A sidecar from before schema 4 does not know which way up an
    // `Original` crop goes until the frame says, as `presets` asks.
    if sidecar.needs_frame()
        && let Some((w, h)) = stance.and_then(|s| s.shown_size(sidecar.turn))
    {
        sidecar.migrate_with_frame(h > w);
    }
    let mut changed = false;
    // The catalog's meta over the sidecar's, but for the capture time's
    // shift. A catalog time that differs from the file's own is the
    // shift, and wins over the sidecar's, since a forced import asks for
    // Lightroom's state. A catalog with no corrected time (none held,
    // the same as the file's, or the file's not readable) has no shift
    // to say, and one set here stays.
    let from_catalog = match (&image.captured, &taken) {
        (Some(catalog), Some(file)) => catalog_shift(catalog, file),
        (Some(_), None) => {
            written.time_unread += 1;
            None
        }
        _ => None,
    };
    written.time_shifted += usize::from(from_catalog.is_some());
    let mut meta = Meta {
        time_shift: sidecar.meta.time_shift,
        ..image.meta.clone()
    };
    if let Some(by) = from_catalog {
        meta.set_time_shift(by);
    }
    if sidecar.meta != meta {
        sidecar.meta = meta;
        changed = true;
    }
    let label = greycard_edit::history::import_label("Lightroom");
    let turn = sidecar.turn;
    let mut tally = |cropped: Cropped, turned: Turned| {
        match cropped {
            Cropped::None => {}
            Cropped::Carried { drawn_in } => {
                written.crops += 1;
                written.crops_drawn_in += usize::from(drawn_in);
            }
            Cropped::Not(why) => *written.crops_not_carried.entry(why).or_default() += 1,
        }
        match turned {
            Turned::Same => {}
            Turned::Turned => written.turned += 1,
            Turned::Mirrored => written.flips += 1,
            Turned::Unread(code) => *written.orientations_unread.entry(code).or_default() += 1,
        }
    };
    let (edit, cropped, turned) =
        developed(image, &sidecar.current, stance, camera.as_deref(), turn);
    tally(cropped, turned);
    if let Some(edit) = edit {
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
        if let Some(line) = sidecar.line_mut(Some(&id)) {
            let (edit, cropped, turned) =
                developed(copy, &line.current, stance, camera.as_deref(), turn);
            tally(cropped, turned);
            if let Some(edit) = edit {
                changed |= line.record_as(edit, Some(label.clone()));
            }
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

/// The catalog's stacks written as ours, each a file in the stacks'
/// folder beside the collections: the members whose files were found,
/// the top first, a virtual copy as the version made for it. A stack
/// there already with every one of these members is left as it is; one
/// with a member in another stack already is not written, since a frame
/// is in one stack at most; one of fewer than two members found, or
/// across folders (Lightroom's never are), is not a stack.
fn write_stacks(
    catalog: &Catalog,
    at: &HashMap<i64, PathBuf>,
    versions: &HashMap<i64, Option<String>>,
    hashes: &mut HashMap<PathBuf, Option<String>>,
    collections_dir: &Path,
    written: &mut Written,
) -> Result<(), ImportError> {
    if catalog.stacks.is_empty() {
        return Ok(());
    }
    let by_id: HashMap<i64, &Image> = catalog.images.iter().map(|i| (i.id, i)).collect();
    let dir = stacks::dir_in(collections_dir);
    let mut there = stacks::read_folder(&dir, stacks::VERSION);
    let mut not = |why: &str| *written.stacks_not_written.entry(why.into()).or_default() += 1;
    let key = |r: &Reference| (r.hint.clone(), r.version.clone());
    let mut made = 0;
    for st in &catalog.stacks {
        let mut refs: Vec<Reference> = Vec::new();
        let mut folders: Vec<PathBuf> = Vec::new();
        for id in &st.images {
            let Some(image) = by_id.get(id) else {
                continue;
            };
            let own = image.master.unwrap_or(image.id);
            let Some(raw) = at.get(&own) else {
                continue;
            };
            let version = match image.master {
                Some(_) => match versions.get(&image.id).cloned().flatten() {
                    Some(v) => Some(v),
                    // A copy whose version was not made (its file's
                    // sidecar left alone without one) is not a frame.
                    None => continue,
                },
                None => None,
            };
            let hash = hashes
                .entry(raw.clone())
                .or_insert_with(|| crate::hash_file(raw).ok())
                .clone();
            let Some(hash) = hash else {
                continue;
            };
            let key_path = crate::key_path(raw);
            if let Some(folder) = key_path.parent() {
                folders.push(folder.to_path_buf());
            }
            refs.push(Reference {
                hash,
                hint: collections::hint_of(&key_path),
                version,
            });
        }
        folders.dedup();
        if refs.len() < 2 {
            not("fewer than two of its frames found");
            continue;
        }
        if folders.len() > 1 {
            not("its frames in more than one folder");
            continue;
        }
        if there
            .list
            .iter()
            .any(|t| refs.iter().all(|r| t.refs.iter().any(|q| key(q) == key(r))))
        {
            written.stacks_there += 1;
            continue;
        }
        if there
            .list
            .iter()
            .any(|t| t.refs.iter().any(|q| refs.iter().any(|r| key(q) == key(r))))
        {
            not("a frame of it in another stack already");
            continue;
        }
        let mut stack = Stack::new(refs);
        stack.open = !st.collapsed;
        stack.write(&dir)?;
        there.list.push(stack);
        made += 1;
    }
    written.stacks += made;
    Ok(())
}

/// Where a root not mapped may be now, beside where the catalog says:
/// a folder of the root's name beside the catalog (a shoot's catalog
/// usually sits beside its pictures' folder), or under one of the
/// library's roots.
#[derive(Debug, Clone, Default)]
pub(crate) struct Near {
    pub(crate) catalog_dir: Option<PathBuf>,
    pub(crate) roots: Vec<PathBuf>,
}

/// How deep under a library's root a root's folder is looked for.
const NEAR_DEPTH: usize = 3;

impl Near {
    /// Beside the catalog at `catalog`, and under the roots of the
    /// library at `library`.
    pub(crate) fn of(catalog: &Path, library: &Path) -> Near {
        let roots = Roots::load(&Roots::path_beside(library))
            .map(|r| r.locals())
            .unwrap_or_default();
        Near {
            roots,
            ..Near::beside(catalog)
        }
    }

    /// Beside the catalog at `catalog` alone.
    pub(crate) fn beside(catalog: &Path) -> Near {
        Near {
            catalog_dir: std::path::absolute(catalog)
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf)),
            roots: Vec::new(),
        }
    }

    /// Every folder that may be the root named `name`, and how each was
    /// found, nearest first: beside the catalog, then under each of the
    /// library's roots a level at a time, three deep. Those found so far
    /// once `stop` is set.
    fn candidates(&self, name: &str, stop: &AtomicBool) -> Vec<(PathBuf, Found)> {
        let mut out: Vec<(PathBuf, Found)> = Vec::new();
        if name.trim().is_empty() {
            return out;
        }
        let named = |p: &Path| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
        };
        if let Some(dir) = &self.catalog_dir {
            let mut beside = vec![dir.join(name)];
            if named(dir) {
                beside.push(dir.clone());
            }
            if let Some(up) = dir.parent() {
                beside.push(up.join(name));
            }
            out.extend(
                beside
                    .into_iter()
                    .filter(|d| d.is_dir())
                    .map(|d| (d, Found::BesideCatalog)),
            );
        }
        for root in &self.roots {
            if named(root) && root.is_dir() {
                out.push((root.clone(), Found::UnderRoot(root.clone())));
            }
            let mut level = vec![root.clone()];
            for _ in 0..NEAR_DEPTH {
                let mut next = Vec::new();
                for dir in &level {
                    if stop.load(Ordering::Relaxed) {
                        return out;
                    }
                    let Ok(entries) = std::fs::read_dir(dir) else {
                        continue;
                    };
                    let mut subs: Vec<PathBuf> = entries
                        .flatten()
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                        .map(|e| e.path())
                        .filter(|p| {
                            !p.file_name()
                                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                        })
                        .collect();
                    subs.sort();
                    next.extend(subs);
                }
                out.extend(
                    next.iter()
                        .filter(|d| named(d))
                        .map(|d| (d.clone(), Found::UnderRoot(root.clone()))),
                );
                level = next;
            }
        }
        out.dedup_by(|a, b| a.0 == b.0);
        out
    }
}

/// How many of a root's files a guess at its folder looks for, spread
/// through the catalog's order.
const GUESS_CHECKED: usize = 25;

/// What a folder holds of a sample of a root's files: how many are
/// there, and of those whose size the catalog keeps, how many have it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Held {
    there: usize,
    sized: usize,
    same_size: usize,
}

impl Held {
    /// What `dir` holds of `sample`.
    fn of(dir: &Path, sample: &[&Image]) -> Held {
        let mut held = Held::default();
        for image in sample {
            let Ok(meta) = std::fs::metadata(under(dir, &image.relative)) else {
                continue;
            };
            held.there += 1;
            if let Some(size) = image.size {
                held.sized += 1;
                held.same_size += usize::from(meta.len() == size);
            }
        }
        held
    }

    /// Whether a guessed folder is the root's: every file of the sample
    /// there, or all but one, and not most of those the catalog has the
    /// size of at another size, which is another shoot's files of the
    /// same names.
    fn is_the_root(self, sampled: usize) -> bool {
        let wanted = sampled.saturating_sub(1).max(1);
        self.there >= wanted && !self.other_files()
    }

    /// Most of the files whose size the catalog keeps at another size.
    fn other_files(self) -> bool {
        self.sized >= 3 && 2 * self.same_size < self.sized
    }
}

/// A camera or a lens as the index names it, and every spelling of it.
pub(crate) type Named = (String, Vec<String>);

/// The cameras and the lenses a smart collection's rule on either can
/// name: the catalog's and the library's, each camera as the index
/// names it with every spelling it goes by.
#[derive(Debug, Clone, Default)]
pub(crate) struct Known {
    /// Each camera's name in the index, and its spellings: that name,
    /// the EXIF models the decoder knows for it and the catalog's.
    pub(crate) cameras: Vec<Named>,
    pub(crate) lenses: Vec<Named>,
}

impl Known {
    /// The catalog's cameras and lenses, and the library's beside
    /// `library` when it has an index. Each name the catalog gives is
    /// put to one of its files found (`at`), whose own reading is the
    /// index's name for it: the camera's and the lens's both, since a
    /// raw's lens is named by its decoder too ("RF50mm F1.2 L USM" in
    /// the EXIF is "RF 50mm F1.2L USM" in the index). A name none of
    /// whose files is here takes the decoder's table for a camera, and
    /// stands as it is for a lens.
    pub(crate) fn of(catalog: &Catalog, library: &Path, at: &HashMap<i64, PathBuf>) -> Known {
        let (mut cameras, mut lenses) = (Vec::new(), Vec::new());
        if library.is_file()
            && let Ok(lib) = crate::Library::open_read_only(library)
        {
            let all = Filter::default();
            for (facet, into) in [
                (crate::Facet::Camera, &mut cameras),
                (crate::Facet::Lens, &mut lenses),
            ] {
                if let Ok(counts) = lib.facet_counts(facet, None, &all) {
                    into.extend(counts.into_iter().map(|c| c.value));
                }
            }
        }
        let mut known = Known::default();
        // Lightroom's test is asked of the EXIF's spellings, which the
        // catalog's own names are; a camera of the library's is known
        // by the decoder's spellings of its name, or by that name when
        // the decoder has none.
        // One file a name, read as the index reads it.
        let mut read: HashMap<PathBuf, Option<(String, Option<String>)>> = HashMap::new();
        let mut ours = |spelled: &dyn Fn(&ExifNames) -> Option<&str>, value: &str| {
            let raw = catalog
                .exif
                .iter()
                .filter(|e| spelled(e) == Some(value))
                .find_map(|e| at.get(&e.image))?;
            read.entry(raw.clone())
                .or_insert_with(|| named_as_indexed(raw))
                .clone()
        };
        for model in &catalog.cameras {
            let name = ours(&|e| e.camera.as_deref(), model)
                .map(|(camera, _)| camera)
                .filter(|c| !c.is_empty())
                .or_else(|| greycard_core::decode::cleaned_camera(model))
                .unwrap_or_else(|| model.trim().to_string());
            known.add_camera(&name, Some(model));
        }
        for name in &cameras {
            known.add_camera(name, None);
        }
        for lens in &catalog.lenses {
            let name = ours(&|e| e.lens.as_deref(), lens)
                .and_then(|(_, lens)| lens)
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| lens.trim().to_string());
            known.add_lens(&name, Some(lens));
        }
        for name in &lenses {
            known.add_lens(name, None);
        }
        known
    }

    /// A lens named `name` in the index, spelled `spelling` by a file
    /// when one says, else known by its name.
    pub(crate) fn add_lens(&mut self, name: &str, spelling: Option<&str>) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let at = match self
            .lenses
            .iter()
            .position(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            Some(at) => at,
            None => {
                self.lenses.push((name.to_string(), Vec::new()));
                self.lenses.len() - 1
            }
        };
        let spelling = spelling.map(str::trim).unwrap_or(name);
        let spellings = &mut self.lenses[at].1;
        if !spellings.iter().any(|s| s.eq_ignore_ascii_case(spelling)) {
            spellings.push(spelling.to_string());
        }
    }

    /// A camera named `name` in the index, spelled `spelling` by a file
    /// when one says. Its spellings are the EXIF's: the decoder's for
    /// the name and the files', or the name itself when there are none.
    pub(crate) fn add_camera(&mut self, name: &str, spelling: Option<&str>) {
        if name.trim().is_empty() {
            return;
        }
        let at = match self
            .cameras
            .iter()
            .position(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            Some(at) => at,
            None => {
                let mut spellings = greycard_core::decode::exif_models(name);
                if spellings.is_empty() && spelling.is_none() {
                    spellings.push(name.to_string());
                }
                self.cameras.push((name.to_string(), spellings));
                self.cameras.len() - 1
            }
        };
        let spellings = &mut self.cameras[at].1;
        if let Some(spelling) = spelling
            && !spellings.iter().any(|s| s.eq_ignore_ascii_case(spelling))
        {
            spellings.push(spelling.trim().to_string());
        }
    }
}

/// Whether a smart collection's rules ask anything of the camera or the
/// lens.
fn asks_exif(rules: &str) -> bool {
    lightroom::parse_table(rules).is_ok_and(|entries| {
        entries.iter().any(|(_, v)| match v {
            Value::Table(fields) => fields.iter().any(|(k, v)| {
                k.as_deref() == Some("criteria")
                    && matches!(v, Value::Word(w) if w == "camera" || w == "lens")
            }),
            _ => false,
        })
    })
}

/// Lightroom's test on a text field, as it asks it of one spelling:
/// case ignored, a word list split on spaces.
fn lightroom_text(op: &str, value: &str, spelling: &str) -> Option<bool> {
    let (v, s) = (value.trim().to_lowercase(), spelling.trim().to_lowercase());
    let words: Vec<&str> = v.split_whitespace().collect();
    Some(match op {
        "==" => s == v,
        "!=" => s != v,
        "any" | "contains" => words.iter().any(|w| s.contains(w)),
        "all" => !words.is_empty() && words.iter().all(|w| s.contains(w)),
        // Every word as the start of one of the spelling's words.
        "words" => {
            !words.is_empty()
                && words
                    .iter()
                    .all(|w| s.split_whitespace().any(|t| t.starts_with(w)))
        }
        "noneOf" => !words.iter().any(|w| s.contains(w)),
        "beginsWith" => s.starts_with(&v),
        "endsWith" => s.ends_with(&v),
        _ => return None,
    })
}

/// A rule on the camera or the lens as filter terms, through the names
/// they go by: Lightroom's test asked of each known one's spellings,
/// and the ones it passes said as the index names them.
///
/// - A test of words the index's names hold the same way (`lens any
///   RF50mm`, passing just the lenses whose name has it) is those words
///   (`lens:RF50mm`).
/// - Else a test passing one is that one (`camera="Canon EOS R6 Mark
///   II"`).
/// - A test leaving some out (`!=`, `noneOf`) is each of those left out
///   (`camera!=...`), since the terms are asked together.
/// - A test passing several and failing others has no term: a filter's
///   terms are asked together, and it has no "either".
/// - One passing none matches nothing and is skipped.
fn exif_terms(
    criteria: &str,
    op: &str,
    value: &str,
    known: &[Named],
) -> Result<Vec<String>, String> {
    let what = if criteria == "camera" {
        "camera"
    } else {
        "lens"
    };
    let quoted = |v: &str| -> Result<String, String> {
        if v.contains('"') {
            return Err(format!("{criteria} names {v}, with a quote in it"));
        }
        Ok(format!("\"{v}\""))
    };
    if lightroom_text(op, value, "").is_none() {
        return Err(format!("{criteria} {op} has no counterpart in the filter"));
    }
    let negative = matches!(op, "!=" | "noneOf");
    // A positive test passes a known one when any of its spellings
    // passes; a negative one, when none of its spellings is left out.
    let passes = |spellings: &[String]| {
        let mut each = spellings
            .iter()
            .map(|s| lightroom_text(op, value, s).unwrap_or(false));
        if negative {
            each.all(|p| p)
        } else {
            each.any(|p| p)
        }
    };
    let (passed, failed): (Vec<&Named>, Vec<&Named>) = known.iter().partition(|(_, sp)| passes(sp));
    if passed.is_empty() {
        return Err(format!(
            "its {what} rule matches no {what} the catalog or the library holds"
        ));
    }
    if negative {
        return failed
            .iter()
            .map(|(name, _)| Ok(format!("{criteria}!={}", quoted(name)?)))
            .collect();
    }
    // Words the index's names answer as Lightroom's spellings do.
    let words: Vec<String> = value.split_whitespace().map(str::to_lowercase).collect();
    let as_words = match op {
        "any" | "contains" if words.len() == 1 => Some(words.clone()),
        "all" | "words" if !words.is_empty() => Some(words.clone()),
        _ => None,
    };
    if let Some(words) = as_words {
        let ours = |name: &str| {
            let name = name.to_lowercase();
            words.iter().all(|w| name.contains(w.as_str()))
        };
        let same = known.iter().all(|(name, sp)| ours(name) == passes(sp));
        if same {
            return words
                .iter()
                .map(|w| {
                    Ok(if w.contains(char::is_whitespace) {
                        format!("{criteria}:{}", quoted(w)?)
                    } else {
                        format!("{criteria}:{w}")
                    })
                })
                .collect();
        }
    }
    let whats = if criteria == "camera" {
        "cameras"
    } else {
        "lenses"
    };
    match passed.as_slice() {
        [(name, _)] => Ok(vec![format!("{criteria}={}", quoted(name)?)]),
        _ if failed.is_empty() => Err(format!(
            "its {what} rule matches every one of the {} {whats} known and names none of them, \
             which no term of the filter says",
            passed.len()
        )),
        _ => Err(format!(
            "its {what} rule matches {} of the {whats} known and not the rest, and a \
             filter's terms are asked together",
            passed.len()
        )),
    }
}

/// A file under a folder, the relative path slash-separated as the
/// catalog keeps it.
pub(crate) fn under(dir: &Path, relative: &str) -> PathBuf {
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
/// was found there, and the roots taken, by the catalog's id; each root
/// said to `progress` as it is looked for, and [`ImportError::Stopped`]
/// once `stop` is set.
pub(crate) fn map_roots(
    catalog: &Catalog,
    given: &[(String, PathBuf)],
    near: &Near,
    progress: &mut dyn FnMut(Progress),
    stop: &AtomicBool,
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
    for (n, root) in catalog.roots.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return Err(ImportError::Stopped);
        }
        progress(Progress::Mapping {
            done: n + 1,
            of: catalog.roots.len(),
        });
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
        let own: Vec<&Image> = catalog
            .images
            .iter()
            .filter(|i| i.root == root.id && i.master.is_none())
            .collect();
        let sample: Vec<&Image> = own.iter().take(CHECKED).copied().collect();
        let guess_sample: Vec<&Image> = evenly(own.len(), GUESS_CHECKED)
            .into_iter()
            .map(|i| own[i])
            .collect();
        let found_under = |dir: &Path| {
            sample
                .iter()
                .filter(|i| under(dir, &i.relative).exists())
                .count()
        };
        // Given, it is taken or not on its own word; where the catalog
        // says, on any of its first files there. Else a guess: of the
        // folders of its name near the catalog or under the library's
        // roots, the one holding most of a sample spread through its
        // files, every one of them or all but one at their sizes, the
        // nearest of those holding as many. A folder of the name with a
        // file or two of the same names is another shoot's, and is
        // passed over.
        let mut guessed: Option<Held> = None;
        let mut missed: Option<NearMiss> = None;
        let (local, how) = match mapped {
            Some(to) => (Some(to), Found::Given),
            None => {
                let here = PathBuf::from(&root.path);
                let as_said = (here.is_absolute() && here.is_dir()).then_some(here);
                let holds = |dir: &Path| {
                    if sample.is_empty() {
                        dir.is_dir()
                    } else {
                        found_under(dir) > 0
                    }
                };
                match as_said {
                    Some(here) if holds(&here) => (Some(here), Found::AsCatalogSays),
                    as_said => {
                        let mut best: Option<(PathBuf, Found, Held)> = None;
                        for (dir, how) in near.candidates(&root.name, stop) {
                            let held = Held::of(&dir, &guess_sample);
                            if !guess_sample.is_empty() && !held.is_the_root(guess_sample.len()) {
                                // The nearest miss, said if nothing is
                                // taken.
                                let nearer = missed
                                    .as_ref()
                                    .is_none_or(|m: &NearMiss| held.there > m.there);
                                if held.there > 0 && nearer {
                                    missed = Some(NearMiss {
                                        dir: dir.clone(),
                                        there: held.there,
                                        looked_for: guess_sample.len(),
                                        other_sizes: held.other_files(),
                                    });
                                }
                                continue;
                            }
                            let better = best.as_ref().is_none_or(|(_, _, b)| {
                                (held.there, held.same_size) > (b.there, b.same_size)
                            });
                            if better {
                                best = Some((dir, how, held));
                            }
                        }
                        match best {
                            Some((dir, how, held)) => {
                                guessed = Some(held);
                                (Some(dir), how)
                            }
                            None => {
                                let how = if as_said.is_some() {
                                    Found::AsCatalogSays
                                } else {
                                    Found::Not
                                };
                                (as_said, how)
                            }
                        }
                    }
                }
            }
        };
        let mut report = RootReport {
            catalog: root.path.clone(),
            name: root.name.clone(),
            local: local.clone(),
            how,
            checked: 0,
            checked_found: 0,
            confirmed: false,
            files: 0,
            found: 0,
            near_miss: None,
        };
        match (&local, guessed) {
            (Some(local), Some(held)) => {
                report.checked = guess_sample.len();
                report.checked_found = held.there;
                report.confirmed = true;
                taken.insert(root.id, local.clone());
            }
            (Some(local), None) => {
                report.checked = sample.len();
                report.checked_found = found_under(local);
                report.confirmed = if sample.is_empty() {
                    local.is_dir()
                } else {
                    report.checked_found > 0
                };
                if report.confirmed {
                    taken.insert(root.id, local.clone());
                }
            }
            (None, _) => {}
        }
        if !report.confirmed {
            report.near_miss = missed;
        }
        reports.push(report);
    }
    Ok((reports, taken))
}

/// The catalog opened read-only, its schema one of [`KNOWN`]: the
/// connection, the schema, which Lightroom wrote it, and how its log
/// stood.
pub(crate) fn open(path: &Path) -> Result<(Connection, i64, &'static str, Wal), ImportError> {
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
    Ok((db, schema, written_by, wal))
}

/// Open the catalog read-only and read what the import carries.
pub(crate) fn read(path: &Path) -> Result<Catalog, ImportError> {
    let (db, schema, written_by, wal) = open(path)?;
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

    // The file's size and Lightroom's orientation, which its crop is
    // laid over.
    let shapes: HashMap<i64, Shape> = optional(
        db.prepare("SELECT id_local, fileWidth, fileHeight, orientation FROM Adobe_images")
            .and_then(|mut st| {
                st.query_map([], |r| {
                    let w = r.get::<_, Option<f64>>(1).ok().flatten();
                    let h = r.get::<_, Option<f64>>(2).ok().flatten();
                    Ok((
                        r.get::<_, i64>(0)?,
                        Shape {
                            size: w.zip(h).filter(|(w, h)| {
                                w.is_finite() && h.is_finite() && *w >= 1.0 && *h >= 1.0
                            }),
                            orientation: r
                                .get::<_, Option<String>>(3)
                                .ok()
                                .flatten()
                                .filter(|o| !o.trim().is_empty()),
                        },
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
            }),
    )?
    .into_iter()
    .collect();
    // Each file's size when it was imported: Lightroom's `importHash` is
    // its capture time, its name and its size, colons between.
    let sizes: HashMap<i64, u64> = optional(
        db.prepare("SELECT id_local, importHash FROM AgLibraryFile")
            .and_then(|mut st| {
                st.query_map([], |r| Ok((r.get::<_, i64>(0)?, text(r, 1)?)))?
                    .collect::<Result<Vec<_>, _>>()
            }),
    )?
    .into_iter()
    .filter_map(|(id, hash)| {
        let size = hash.rsplit(':').next()?.trim().parse::<u64>().ok()?;
        Some((id, size))
    })
    .collect();
    // Lightroom's capture time for each image, where it holds one.
    let captured: HashMap<i64, String> = optional(
        db.prepare("SELECT id_local, captureTime FROM Adobe_images WHERE captureTime IS NOT NULL")
            .and_then(|mut st| {
                st.query_map([], |r| Ok((r.get::<_, i64>(0)?, text(r, 1)?)))?
                    .collect::<Result<Vec<_>, _>>()
            }),
    )?
    .into_iter()
    .filter(|(_, t)| !t.trim().is_empty())
    .collect();
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
        let shape = shapes.get(&id).cloned().unwrap_or_default();
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
            shape,
            size: sizes.get(&r.get::<_, i64>(1)?).copied(),
            captured: captured.get(&id).cloned(),
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
    // The stacks, their images in their order, the top first. A
    // catalog's stack whose images will not read (a table missing from
    // one we have not seen) is counted and carries none.
    let ids: Vec<i64> = db
        .prepare("SELECT id_local FROM AgLibraryFolderStack ORDER BY id_local")?
        .query_map([], |r| r.get::<_, i64>(0))?
        .collect::<Result<_, _>>()?;
    let mut stacks: Vec<CatalogStack> = ids
        .iter()
        .map(|_| CatalogStack {
            images: Vec::new(),
            collapsed: false,
        })
        .collect();
    let members = optional(
        db.prepare(
            "SELECT s.stack, s.image, f.collapsed FROM AgLibraryFolderStackImage s \
             JOIN AgLibraryFolderStack f ON f.id_local = s.stack \
             ORDER BY s.stack, CAST(s.position AS REAL), s.id_local",
        )
        .and_then(|mut st| {
            st.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<f64>>(2).ok().flatten().unwrap_or(0.0) > 0.0,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
        }),
    )?;
    for (stack, image, collapsed) in members {
        if let Some(at) = ids.iter().position(|&i| i == stack) {
            stacks[at].images.push(image);
            stacks[at].collapsed = collapsed;
        }
    }
    // The camera and the lens each image names, as Lightroom spells
    // them.
    let named = |r: &rusqlite::Row, i: usize| -> rusqlite::Result<Option<String>> {
        Ok(Some(text(r, i)?.trim().to_string()).filter(|n| !n.is_empty()))
    };
    let exif: Vec<ExifNames> = optional(
        db.prepare(
            "SELECT h.image, m.value, l.value FROM AgHarvestedExifMetadata h \
             LEFT JOIN AgInternedExifCameraModel m ON m.id_local = h.cameraModelRef \
             LEFT JOIN AgInternedExifLens l ON l.id_local = h.lensRef \
             ORDER BY h.image",
        )
        .and_then(|mut st| {
            st.query_map([], |r| {
                Ok(ExifNames {
                    image: r.get(0)?,
                    camera: named(r, 1)?,
                    lens: named(r, 2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
        }),
    )?;
    let distinct = |of: &dyn Fn(&ExifNames) -> Option<&String>| {
        let set: std::collections::BTreeSet<String> = exif.iter().filter_map(of).cloned().collect();
        set.into_iter().collect::<Vec<String>>()
    };
    let cameras = distinct(&|e| e.camera.as_ref());
    let lenses = distinct(&|e| e.lens.as_ref());
    Ok(Catalog {
        schema,
        written_by,
        roots,
        images,
        nodes,
        stacks,
        wal,
        cameras,
        lenses,
        exif,
    })
}

/// A file's camera and lens as the index names them: read as an index
/// pass reads it, through the decoder for a raw: from its ends, never
/// mapped.
fn named_as_indexed(path: &Path) -> Option<(String, Option<String>)> {
    let probe = if greycard_core::picture::is_picture_path(path) {
        greycard_core::picture::probe_path(path).ok()?
    } else {
        greycard_core::decode::probe_ends_path(path).ok()?
    };
    let exif = crate::Exif::from_probe(&probe);
    Some((exif.camera, exif.lens))
}

/// `m` places spread evenly through `n`, all of them when `m` is as
/// many.
fn evenly(n: usize, m: usize) -> Vec<usize> {
    if m >= n {
        return (0..n).collect();
    }
    (0..m).map(|k| (2 * k + 1) * n / (2 * m)).collect()
}

/// A read of tables or columns a catalog may not have: none of them
/// when the table or the column is not there, any other error as it is.
fn optional<T>(read: rusqlite::Result<Vec<T>>) -> Result<Vec<T>, ImportError> {
    match read {
        Ok(v) => Ok(v),
        Err(e) if e.to_string().contains("no such") => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
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
        inflated(blob)?
    };
    let title = greycard_edit::xmp::read(&packet).ok()?.meta.title;
    (!title.is_empty()).then_some(title)
}

/// Text the catalog keeps deflated: four bytes of its length, then the
/// text through zlib. The XMP packets and the history steps' settings.
pub(crate) fn inflated(blob: &[u8]) -> Option<String> {
    let mut out = String::new();
    flate2::read::ZlibDecoder::new(blob.get(4..)?)
        .read_to_string(&mut out)
        .ok()?;
    Some(out)
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
/// must all be asked together, since a filter's terms are. A rule on
/// the camera or the lens is asked of the cameras and lenses `known`.
fn translate(rules: &str, known: &Known) -> Result<String, String> {
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
        terms.extend(match criteria {
            "camera" => exif_terms(criteria, op, value, &known.cameras)?,
            "lens" => exif_terms(criteria, op, value, &known.lenses)?,
            _ => term(criteria, op, value, word("value2"))?,
        });
    }
    let text = terms.join(" ");
    Filter::parse(&text).map_err(|e| format!("its filter would not read: {e}"))?;
    Ok(text)
}

/// A rule's number, finite: `NaN` parses as one and casts to zero, and
/// `inf` casts to the end of the integers.
fn finite(value: &str) -> Option<f64> {
    value.trim().parse::<f64>().ok().filter(|n| n.is_finite())
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
            let n = finite(value).ok_or_else(|| format!("a rating of {value}"))?;
            Ok(vec![format!("rating{c}{}", n.round() as i64)])
        }
        "pick" => {
            let is = match op {
                "==" => ":",
                "!=" => "!=",
                _ => return none(),
            };
            let n = finite(value).ok_or_else(|| format!("a flag of {value}"))?;
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
            let color = match finite(value).map(|n| n as i64) {
                Some(0) => "none",
                Some(1) => "red",
                Some(2) => "yellow",
                Some(3) => "green",
                Some(4) => "blue",
                Some(5) => "purple",
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

/// A report's lines as the command prints them: a word in a column,
/// then what it says, a continuation with no word. The window's sheet
/// says the same in its own shape ([`Report::sheet`]).
#[derive(Debug, Default)]
struct Lines(Vec<(String, String)>);

impl Lines {
    fn line(&mut self, label: &str, text: &str) {
        self.0.push((label.to_string(), text.to_string()));
    }

    fn more(&mut self, text: &str) {
        self.0.push((String::new(), text.to_string()));
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (label, text) in self.lines() {
            writeln!(f, "{label:<13} {text}")?;
        }
        Ok(())
    }
}

/// Who a line speaks to where the two answer it differently: the
/// command line, whose user answers with its flags, or the window's
/// sheet, whose user answers with its buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Voice {
    Command,
    Sheet,
}

impl Report {
    /// What the report says as the command prints it, a line at a
    /// time: a word for the line, or none for one that goes on from the
    /// line before, and the text.
    pub fn lines(&self) -> Vec<(String, String)> {
        let mut out = Lines::default();
        let counted = |map: &BTreeMap<String, usize>| {
            let mut v: Vec<(&String, &usize)> = map.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            v.iter()
                .map(|(k, n)| format!("{k} ×{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.line("catalog", &self.catalog.display().to_string());
        out.line(
            "written by",
            &format!("{} (schema {})", self.written_by, self.schema),
        );
        match self.wal {
            Wal::Whole => {}
            Wal::Read => out.line("log", "read with the -wal beside it"),
            Wal::Absent => {
                out.line(
                    "log",
                    "no -wal beside it: whole if Lightroom had quit before it was copied;",
                );
                out.more(
                    "otherwise copy the catalog with its -wal, or quit Lightroom first, \
                     or the newest changes are not in it",
                );
            }
        }
        for r in &self.roots {
            out.line("root", &format!("{} ({})", r.catalog, r.name));
            match &r.local {
                None if r.near_miss.is_some() => {
                    out.more("not here, and not mapped; nearest of its name:")
                }
                None => out.more(
                    "not here, and not mapped: say where with --root <catalog path>=<folder>",
                ),
                Some(local) => {
                    let how = match &r.how {
                        Found::Given => "as given".to_string(),
                        Found::BesideCatalog => "found beside the catalog".to_string(),
                        Found::UnderRoot(root) => {
                            format!("found under the library's root {}", root.display())
                        }
                        Found::AsCatalogSays | Found::Not => "where the catalog says".to_string(),
                    };
                    let took = if r.confirmed { "taken" } else { "not taken" };
                    out.more(&format!(
                        "-> {} {how}: {} of {} files looked for found, {took}",
                        local.display(),
                        r.checked_found,
                        r.checked
                    ));
                }
            }
            if let Some(m) = &r.near_miss {
                out.more(&m.said(Voice::Command));
            }
            out.more(&format!(
                "{} files: {} found, {} missing",
                r.files,
                r.found,
                r.files - r.found
            ));
        }
        out.line(
            "images",
            &format!(
                "{}: {} files and {} virtual copies",
                self.images, self.files, self.copies
            ),
        );
        out.line(
            "found",
            &format!("{} images, {} missing", self.found, self.missing),
        );
        // A dry run says what a write would do, of the files found.
        let would = if self.written.is_some() {
            "is"
        } else {
            "would be"
        };
        out.line(
            "with edits",
            &format!(
                "{}, {} of them found: each found {would} written as one named state, \
                 its settings mapped as an approximation",
                self.with_edits, self.with_edits_found
            ),
        );
        if self.crops > 0 {
            out.line(
                "crops",
                &format!(
                    "{}, {} of them found: each {would} carried with its angle where its file \
                     says its size",
                    self.crops, self.crops_found
                ),
            );
        }
        if self.oriented > 0 {
            let mut turns = format!(
                "each frame {would} stood as Lightroom shows it, a turn there taken as the \
                 edit's quarter turns where its file's tag says otherwise"
            );
            if self.flipped > 0 {
                turns.push_str(&format!(
                    "; {} flipped there ({} found), the mirror and any crop not carried",
                    self.flipped, self.flipped_found
                ));
            }
            if !self.orientations_unread.is_empty() {
                turns.push_str(&format!(
                    "; orientation not read: {}",
                    counted(&self.orientations_unread)
                ));
            }
            out.line("turns", &turns);
        }
        if !self.not_carried.is_empty() {
            out.line("not carried", &counted(&self.not_carried));
        }
        if self.unreadable_develop > 0 {
            out.line(
                "unreadable",
                &format!(
                    "{} images' develop settings will not read",
                    self.unreadable_develop
                ),
            );
        }
        if self.old_process > 0 {
            out.line(
                "old process",
                &format!(
                    "{} edits on the 2003 or 2010 process, further off still",
                    self.old_process
                ),
            );
        }
        out.line(
            "meta",
            &format!(
                "{} files with a rating, flag, label, keyword, title or caption, {} of them found",
                self.with_meta, self.with_meta_found
            ),
        );
        if !self.odd_labels.is_empty() {
            out.line(
                "labels",
                &format!("no color for {}: left unlabeled", counted(&self.odd_labels)),
            );
        }
        out.line(
            "keywords",
            &format!("{} keywords on {} files", self.keywords, self.keyworded),
        );
        let apart = if self.copies_apart > 0 {
            format!(
                "; {} whose rating, flag, label, keywords, title or caption differs from \
                 its file's, which keeps its own",
                self.copies_apart
            )
        } else {
            String::new()
        };
        out.line(
            "copies",
            &format!(
                "{} virtual {}, {} of them found, each a version named for it{apart}",
                self.copies,
                if self.copies == 1 { "copy" } else { "copies" },
                self.copies_found
            ),
        );
        out.line(
            "stacks",
            &format!(
                "{}, {} with two frames or more found: each {would} a stack, open or closed \
                 as Lightroom had it",
                self.stacks, self.stacks_found
            ),
        );
        out.line(
            "collections",
            &format!(
                "{} and {} {}",
                self.collections.len(),
                self.sets,
                if self.sets == 1 { "set" } else { "sets" }
            ),
        );
        for c in &self.collections {
            let creation = c
                .creation
                .as_deref()
                .map(|k| format!(" (a {k}: its frames, not its layout)"))
                .unwrap_or_default();
            out.more(&format!(
                "{}{creation}: {} frames, {} found",
                c.name, c.frames, c.found
            ));
        }
        if !self.creations_left.is_empty() {
            out.more(&format!(
                "left out: {} (books, prints, slideshows and web pages, as asked)",
                self.creations_left.join(", ")
            ));
        }
        out.line(
            "smart",
            &format!(
                "{} translated, {} skipped, {} of Lightroom's own six left out",
                self.smart.len(),
                self.smart_skipped.len(),
                self.smart_stock
            ),
        );
        if self.exif_rules + self.exif_rules_skipped > 0 {
            out.more(&format!(
                "on a camera or a lens: {} translated, {} skipped",
                self.exif_rules, self.exif_rules_skipped
            ));
        }
        for (name, text) in &self.smart {
            out.more(&format!("{name}: {text}"));
        }
        for (name, why) in &self.smart_skipped {
            out.more(&format!("{name}: skipped, {why}"));
        }
        out.line(
            "sidecars",
            if self.sidecars_in_folder {
                "a new one under its folder's .greycard/, as the editor's setting says"
            } else {
                "a new one beside its file, or under the folder's .greycard/ where it has one"
            },
        );
        if self.already > 0 {
            out.line(
                "",
                &format!(
                    "{} {} history, versions or meta already: left alone {}",
                    self.already,
                    if self.already == 1 {
                        "file has"
                    } else {
                        "files have"
                    },
                    "without --force"
                ),
            );
        }
        for (path, why) in &self.unreadable_sidecars {
            out.line(
                "unreadable",
                &format!("{}: {why}; left alone", path.display()),
            );
        }
        match &self.written {
            None => out.line("dry run", "nothing written; --write to import"),
            Some(w) => {
                out.line(
                    "written",
                    &format!(
                        "{} sidecars ({} under .greycard/), {} left alone",
                        w.sidecars, w.in_folder, w.left_alone
                    ),
                );
                for (path, why) in &w.failed {
                    out.line("not written", &format!("{}: {why}", path.display()));
                }
                out.line(
                    "collections",
                    &format!(
                        "{} written, {} joined to one of the same name, {} there already \
                         with every frame; {} smart written, {} there already",
                        w.collections, w.joined, w.collections_there, w.smart, w.smart_there
                    ),
                );
                let mut crops = format!("{} carried", w.crops);
                if w.crops_drawn_in > 0 {
                    crops.push_str(&format!(
                        ", {} of them drawn in to stay on the turned picture",
                        w.crops_drawn_in
                    ));
                }
                if !w.crops_not_carried.is_empty() {
                    crops.push_str(&format!("; not: {}", counted(&w.crops_not_carried)));
                }
                if w.crops + w.crops_not_carried.values().sum::<usize>() > 0 {
                    out.line("crops", &crops);
                }
                let unread: usize = w.orientations_unread.values().sum();
                if w.turned + w.flips + unread > 0 {
                    let mut turns = format!("{} turned in Lightroom, the turn carried", w.turned);
                    if w.flips > 0 {
                        turns.push_str(&format!(
                            "; {} flipped there, the mirror not carried",
                            w.flips
                        ));
                    }
                    if unread > 0 {
                        turns.push_str(&format!(
                            "; orientation not read: {}",
                            counted(&w.orientations_unread)
                        ));
                    }
                    out.line("turns", &turns);
                }
                if w.time_shifted + w.time_unread > 0 {
                    let mut times = format!(
                        "{} frames keep the capture time corrected in Lightroom",
                        w.time_shifted
                    );
                    if w.time_unread > 0 {
                        times.push_str(&format!(
                            "; {} could not be compared, the file's own time not read",
                            w.time_unread
                        ));
                    }
                    out.line("capture times", &times);
                }
                let mut stacks = format!("{} written, {} there already", w.stacks, w.stacks_there);
                if !w.stacks_not_written.is_empty() {
                    stacks.push_str(&format!("; not: {}", counted(&w.stacks_not_written)));
                }
                if self.stacks > 0 {
                    out.line("stacks", &stacks);
                }
                if w.copies_as_own > 0 {
                    out.more(&format!(
                        "{} virtual copies in them as their file's own frame: their \
                             sidecar was left alone",
                        w.copies_as_own
                    ));
                }
                for (dir, said) in &w.roots {
                    out.line(
                        "root",
                        &format!(
                            "{}: {}",
                            dir.display(),
                            said.said(&|p| p.display().to_string())
                        ),
                    );
                }
            }
        }
        out.0
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
    pub(crate) fn empty_catalog(path: &Path, schema: &str) -> Connection {
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
        assert_eq!(report.not_carried.get("crop"), None, "the crop is carried");
        assert_eq!((report.crops, report.crops_found), (2, 2));
        assert_eq!(report.old_process, 0);
        assert_eq!(report.with_meta, 3);
        assert_eq!(report.odd_labels.get("To Print"), Some(&1));
        assert_eq!(report.copies_apart, 1, "the copy's two stars");
        assert_eq!((report.keywords, report.keyworded), (2, 1));
        assert_eq!(report.stacks, 1);
        assert_eq!(report.sets, 1);
        let r = &report.roots[0];
        assert_eq!(r.local.as_deref(), Some(s.here.as_path()));
        assert!(r.how == Found::Given && r.confirmed);
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
        assert_eq!(w.roots[0].1, Rooted::Already);
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
        assert_eq!((w.sidecars, w.left_alone, w.written_over), (1, 1, 0));
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
        assert_eq!(report.written.as_ref().unwrap().written_over, 1);
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

    /// A forced import lays the catalog's rating and the rest over the
    /// sidecar's, and leaves the capture time's shift set here alone:
    /// the catalog's meta has none to say.
    #[test]
    fn a_forced_import_keeps_the_capture_time_shift() {
        let s = setup("force-shift");
        let first = s.here.join("2026/harbor/IMG_0001.CR3");
        let mut mine = Sidecar::default();
        mine.meta.rating = 1;
        mine.meta.set_time_shift(300);
        mine.save(&first).unwrap();
        let forced = Options {
            force: true,
            ..writing(&s)
        };
        import(&s.catalog, &forced).unwrap();
        let a = Sidecar::load(&first).unwrap().unwrap();
        assert_eq!((a.meta.rating, a.meta.time_shift), (4, 300));
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    /// Five pictures whose own capture time is `2026:03:14 10:00:00`
    /// and whose catalog times are, in order: five minutes later, the
    /// same, none, one second later, and an hour earlier.
    fn timed_shoot(what: &str) -> Setup {
        let dir = scratch(&format!("lrcat-{what}"));
        let catalog = dir.join("Times.lrcat");
        let here = dir.join("photos");
        std::fs::create_dir_all(&here).unwrap();
        let db = empty_catalog(&catalog, "1504001");
        db.execute_batch(
            "ALTER TABLE Adobe_images ADD COLUMN captureTime;
             INSERT INTO AgLibraryRootFolder VALUES (1, 'r', 'D:/Times/', 'Times', NULL);
             INSERT INTO AgLibraryFolder VALUES (2, 'f', NULL, '', 1);",
        )
        .unwrap();
        let times = [
            Some("2026-03-14T10:05:00"),
            Some("2026-03-14T10:00:00"),
            None,
            Some("2026-03-14T10:00:01"),
            Some("2026-03-14T09:00:00"),
        ];
        let mut frame = crate::fixture::R5;
        frame.taken = "2026:03:14 10:00:00";
        for (n, time) in times.iter().enumerate() {
            let id = n as i64 + 1;
            crate::fixture::write_frame(&here.join(format!("f{id}.tif")), &frame, id as u16);
            db.execute(
                "INSERT INTO AgLibraryFile VALUES (?1, 'a', ?2, 'tif', 2)",
                rusqlite::params![id * 10, format!("f{id}")],
            )
            .unwrap();
            db.execute(
                "INSERT INTO Adobe_images (id_local, id_global, rootFile, rating, pick, \
                 captureTime) VALUES (?1, 'i', ?2, 1.0, 0.0, ?3)",
                rusqlite::params![id * 100, id * 10, time],
            )
            .unwrap();
        }
        drop(db);
        let options = Options {
            roots: vec![("D:/Times".into(), here.clone())],
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

    fn shift_of(s: &Setup, n: u32) -> i64 {
        Sidecar::load(&s.here.join(format!("f{n}.tif")))
            .unwrap()
            .unwrap()
            .meta
            .time_shift
    }

    /// The catalog's capture time against the file's own: the
    /// difference is the shift, a time the same, missing or a second
    /// off is none, and the report counts the frames shifted.
    #[test]
    fn a_time_corrected_in_lightroom_comes_across_as_a_shift() {
        let s = timed_shoot("times");
        let report = import(&s.catalog, &writing(&s)).unwrap();
        assert_eq!(shift_of(&s, 1), 300);
        assert_eq!(shift_of(&s, 2), 0);
        assert_eq!(shift_of(&s, 3), 0);
        assert_eq!(shift_of(&s, 4), 0);
        assert_eq!(shift_of(&s, 5), -3600);
        // A zero shift writes no key at all.
        let raw = std::fs::read_to_string(
            Sidecar::find(&s.here.join("f2.tif")).expect("a sidecar for the rating"),
        )
        .unwrap();
        assert!(!raw.contains("time_shift"), "{raw}");
        let w = report.written.as_ref().unwrap();
        assert_eq!((w.time_shifted, w.time_unread), (2, 0));
        let said = report.lines();
        assert!(
            said.iter()
                .any(|(_, t)| t.starts_with("2 frames keep the capture time corrected")),
            "{said:?}"
        );
        let shown = report.sheet(&|p| p.display().to_string()).details;
        assert!(
            shown.iter().any(|(_, t)| t
                .starts_with("capture times: 2 frames keep the time corrected in Lightroom")),
            "{shown:?}"
        );
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    /// A forced import over a sidecar with a shift: the catalog's
    /// corrected time wins where it has one, and a frame the catalog
    /// has no correction for keeps the sidecar's.
    #[test]
    fn a_forced_import_takes_the_catalog_s_shift_where_it_has_one() {
        let s = timed_shoot("times-force");
        for n in [1, 2] {
            let mut mine = Sidecar::default();
            mine.meta.rating = 1;
            mine.meta.set_time_shift(60);
            mine.save(&s.here.join(format!("f{n}.tif"))).unwrap();
        }
        let forced = Options {
            force: true,
            ..writing(&s)
        };
        import(&s.catalog, &forced).unwrap();
        assert_eq!(shift_of(&s, 1), 300, "the catalog's");
        assert_eq!(shift_of(&s, 2), 60, "no correction there: the sidecar's");
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    /// A catalog time against a file whose own cannot be read is no
    /// shift, and is counted.
    #[test]
    fn a_catalog_time_with_no_time_in_the_file_is_counted_not_carried() {
        let s = timed_shoot("times-unread");
        std::fs::write(s.here.join("f1.tif"), b"not a picture").unwrap();
        let report = import(&s.catalog, &writing(&s)).unwrap();
        let w = report.written.as_ref().unwrap();
        assert_eq!(shift_of(&s, 1), 0);
        // Frames 2, 4 and 5 hold a catalog time too; frame 5 shifts.
        assert_eq!((w.time_shifted, w.time_unread), (1, 1));
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
        assert!(r.roots[0].confirmed && r.roots[0].how == Found::Given);
        // None given: looked for where the catalog says, which is not
        // here.
        let r = with(Vec::new()).unwrap();
        assert!(r.roots[0].local.is_none() && r.roots[0].how == Found::Not);
        assert_eq!(r.found, 0);
        assert!(r.to_string().contains("--root"), "{r}");
        // A mapping that names no root is refused, naming them.
        let err = with(vec![("E:/Elsewhere".into(), parent.clone())]).unwrap_err();
        assert!(err.to_string().contains("D:/Pictures/Shoots/"), "{err}");
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn smart_rules_translate_where_the_filter_has_them() {
        let none = Known::default();
        let one = |c: &str, op: &str, v: &str| {
            translate(
                &format!(
                    "s = {{ {{ criteria = \"{c}\", operation = \"{op}\", value = {v}, }}, \
                     combine = \"intersect\", }}"
                ),
                &none,
            )
        };
        assert_eq!(one("rating", ">=", "3").unwrap(), "rating>=3");
        assert_eq!(one("rating", "==", "0").unwrap(), "rating=0");
        assert_eq!(one("pick", "==", "1").unwrap(), "flag:pick");
        assert_eq!(one("pick", "!=", "-1").unwrap(), "flag!=reject");
        assert_eq!(one("pick", "==", "0").unwrap(), "flag:none");
        assert_eq!(one("labelColor", "==", "4").unwrap(), "label:blue");
        assert_eq!(
            one("keywords", "all", "\"skye harbor\"").unwrap(),
            "keyword:skye keyword:harbor"
        );
        assert_eq!(one("keywords", "any", "\"skye\"").unwrap(), "keyword:skye");
        // A camera or a lens nothing known is: skipped, and said why.
        assert!(
            one("camera", "==", "\"Canon EOS R5m2\"")
                .unwrap_err()
                .contains("matches no camera")
        );
        assert!(one("lens", "any", "\"RF50mm\"").is_err());
        assert_eq!(
            one("captureTime", ">=", "\"2026-05-01\"").unwrap(),
            "date>=2026-05-01"
        );
        assert_eq!(
            translate(
                "s = { { criteria = \"captureTime\", operation = \"in\", \
                 value = \"2026-05-01\", value2 = \"2026-05-31\", }, }",
                &none
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
        // A number that is not finite, or no number, is refused: no
        // rating, label, or flag.
        for v in ["nan", "NaN", "inf", "-inf", "infinity", "\"up\""] {
            assert!(one("rating", ">=", v).is_err(), "{v}");
            assert!(one("labelColor", "==", v).is_err(), "{v}");
            assert!(one("pick", "==", v).is_err(), "{v}");
        }
        let union = "s = { { criteria = \"rating\", operation = \"==\", value = 5, },\n\
                     { criteria = \"pick\", operation = \"==\", value = 1, },\n\
                     combine = \"union\", }";
        assert!(translate(union, &none).unwrap_err().contains("any of"));
        assert!(translate("s = { combine = \"intersect\", }", &none).is_err());
        assert!(translate("not a table", &none).is_err());
    }

    /// The cameras and lenses of the catalogs we read, as Lightroom
    /// spells them, put through the decoder's names.
    fn known() -> Known {
        let mut k = Known::default();
        for model in [
            "Canon EOS R5m2",
            "Canon EOS R6m2",
            "Canon EOS R6 Mark III",
            "GFX100S II",
        ] {
            let name = greycard_core::decode::cleaned_camera(model).unwrap_or(model.into());
            k.add_camera(&name, Some(model));
        }
        for lens in ["RF50mm F1.2 L USM", "RF50mm F1.8 STM", "GF110mmF2 R LM WR"] {
            k.lenses.push((lens.into(), vec![lens.into()]));
        }
        k
    }

    #[test]
    fn camera_and_lens_rules_are_asked_of_the_names_they_go_by() {
        let k = known();
        let one = |c: &str, op: &str, v: &str| {
            translate(
                &format!(
                    "s = {{ {{ criteria = \"{c}\", operation = \"{op}\", value = \"{v}\", }}, \
                     combine = \"intersect\", }}"
                ),
                &k,
            )
        };
        // Lightroom's EXIF spelling is the index's cleaned name.
        assert_eq!(
            one("camera", "==", "Canon EOS R5m2").unwrap(),
            "camera=\"Canon EOS R5 Mark II\""
        );
        assert_eq!(
            one("camera", "==", "GFX100S II").unwrap(),
            "camera=\"Fujifilm GFX 100S II\"",
            "the decoder spaces the model"
        );
        // A word of one spelling, standing for one camera.
        assert_eq!(
            one("camera", "any", "R6m2").unwrap(),
            "camera=\"Canon EOS R6 Mark II\""
        );
        // Words the index's names hold as Lightroom's do are those
        // words: every Canon here is a "Canon" either way.
        assert_eq!(one("camera", "any", "canon").unwrap(), "camera:canon");
        // Lightroom asks its own spelling, not ours: "II" is in the
        // R6 Mark III's and the GFX's EXIF names, two of the four,
        // where all four of ours have it.
        let err = one("camera", "any", "II").unwrap_err();
        assert!(err.contains("matches 2 of the cameras"), "{err}");
        assert_eq!(one("lens", "any", "RF50mm").unwrap(), "lens:rf50mm");
        assert_eq!(
            one("lens", "==", "RF50mm F1.2 L USM").unwrap(),
            "lens=\"RF50mm F1.2 L USM\""
        );
        // "Is not" leaves each one out by its name.
        assert_eq!(
            one("camera", "!=", "Canon EOS R5m2").unwrap(),
            "camera!=\"Canon EOS R5 Mark II\""
        );
        // A word that one spelling has and its cleaned name has not,
        // passing two cameras of four: no filter says that.
        let err = one("camera", "any", "m2").unwrap_err();
        assert!(err.contains("matches 2 of the cameras"), "{err}");
        // One passing every lens known names none of them.
        let err = one("lens", "beginsWith", "").unwrap_err();
        assert!(
            err.contains("matches every one of the 3 lenses known"),
            "{err}"
        );
        // A camera nothing here is.
        assert!(
            one("camera", "==", "NIKON Z 8")
                .unwrap_err()
                .contains("matches no camera")
        );
        // Each translation reads as a filter, and asks the index's
        // column.
        for text in [
            one("camera", "==", "Canon EOS R5m2").unwrap(),
            one("camera", "!=", "Canon EOS R5m2").unwrap(),
            one("lens", "any", "RF50mm").unwrap(),
        ] {
            Filter::parse(&text).unwrap();
        }
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

    /// A catalog of one picture and a virtual copy of it, both cropped
    /// (the copy turned), the two stacked as Lightroom stacks a copy on
    /// its master. The picture is a small TIFF whose size the decoder
    /// reads, 4 by 3, as the catalog has it.
    fn cropped_shoot(what: &str) -> Setup {
        let dir = scratch(&format!("lrcat-{what}"));
        let catalog = dir.join("Crops.lrcat");
        let here = dir.join("photos");
        std::fs::create_dir_all(&here).unwrap();
        crate::fixture::write_frame(&here.join("frame.tif"), &crate::fixture::R5, 1);
        let db = empty_catalog(&catalog, "1504001");
        db.execute_batch(
            "ALTER TABLE Adobe_images ADD COLUMN fileWidth;
             ALTER TABLE Adobe_images ADD COLUMN fileHeight;
             ALTER TABLE Adobe_images ADD COLUMN orientation;
             ALTER TABLE AgLibraryFolderStack ADD COLUMN collapsed;
             CREATE TABLE AgLibraryFolderStackImage (id_local INTEGER PRIMARY KEY,
                 collapsed, image, position, stack);
             INSERT INTO AgLibraryRootFolder VALUES (1, 'r', 'D:/Crops/', 'Crops', NULL);
             INSERT INTO AgLibraryFolder VALUES (2, 'f', NULL, '', 1);
             INSERT INTO AgLibraryFile VALUES (10, 'a', 'frame', 'tif', 2);
             INSERT INTO Adobe_images (id_local, id_global, rootFile, masterImage, copyName,
                 rating, pick, fileWidth, fileHeight, orientation)
                 VALUES (100, 'i', 10, NULL, NULL, 0.0, 0.0, 4.0, 3.0, 'AB');
             INSERT INTO Adobe_images (id_local, id_global, rootFile, masterImage, copyName,
                 rating, pick, fileWidth, fileHeight, orientation)
                 VALUES (101, 'j', 10, 100, 'Turned', 0.0, 0.0, 4.0, 3.0, 'AB');
             INSERT INTO AgLibraryFolderStack VALUES (1, 's', 0);
             INSERT INTO AgLibraryFolderStackImage VALUES (1, 0, 100, '1.0', 1);
             INSERT INTO AgLibraryFolderStackImage VALUES (2, 0, 101, '2.0', 1);",
        )
        .unwrap();
        let insert = "INSERT INTO Adobe_imageDevelopSettings (image, text, processVersion, \
                      hasDevelopAdjustmentsEx) VALUES (?1, ?2, '15.4', 1.0)";
        let master = "s = { Exposure2012 = 0.5, CropLeft = 0.25, CropRight = 0.75, \
                      CropConstrainAspectRatio = false }";
        let copy = "s = { Exposure2012 = -0.5, CropLeft = 0.2, CropTop = 0.2, \
                    CropRight = 0.8, CropBottom = 0.8, CropAngle = 2, \
                    CropConstrainAspectRatio = true }";
        db.execute(insert, rusqlite::params![100, master]).unwrap();
        db.execute(insert, rusqlite::params![101, copy]).unwrap();
        drop(db);
        let options = Options {
            roots: vec![("D:/Crops".into(), here.clone())],
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

    /// A file size past the floats is no size: SQLite keeps `1e999` as
    /// infinity (and `NaN` as nothing), and a crop laid over it would
    /// not be a number.
    #[test]
    fn a_catalog_size_that_is_not_finite_is_no_size() {
        let s = cropped_shoot("inf-size");
        let db = Connection::open(&s.catalog).unwrap();
        db.execute(
            "UPDATE Adobe_images SET fileWidth = 1e999 WHERE id_local = 100",
            [],
        )
        .unwrap();
        drop(db);
        let catalog = read(&s.catalog).unwrap();
        let size = |id: i64| {
            catalog
                .images
                .iter()
                .find(|i| i.id == id)
                .unwrap()
                .shape
                .size
        };
        assert_eq!(size(100), None);
        assert_eq!(size(101), Some((4.0, 3.0)));
        drop(catalog);
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    /// A crop that does not read, by a key that is no number or by
    /// edges so far out the placing runs past the floats, is not
    /// carried and says so; the geometry is left as it was.
    #[test]
    fn a_crop_that_does_not_read_is_not_carried_and_says_so() {
        let s = cropped_shoot("crop-unread");
        let frame = greycard_core::decode::stance_path(s.here.join("frame.tif")).ok();
        let shape = Shape {
            size: Some((4.0, 3.0)),
            orientation: Some("AB".into()),
        };
        for settings in [
            "s = { CropLeft = 0.25, CropAngle = nan }",
            "s = { CropLeft = 0.25, CropRight = inf }",
            "s = { CropTop = 0.1, CropBottom = \"wide\" }",
            "s = { CropLeft = 0, CropTop = 0, CropRight = 1e300, CropBottom = 0.9, \
             CropAngle = 5, CropConstrainAspectRatio = true }",
        ] {
            let mut edit = Edit::default();
            assert_eq!(
                crop_onto(&mut edit, frame.as_ref(), settings, &shape, 0),
                Cropped::Not("its crop does not read".into()),
                "{settings}"
            );
            assert_eq!(edit.geometry, Edit::default().geometry, "{settings}");
        }
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn a_write_carries_the_crop_and_the_stack() {
        let s = cropped_shoot("crop");
        let dry = import(&s.catalog, &s.options).unwrap();
        assert_eq!((dry.crops, dry.crops_found), (2, 2), "{dry}");
        assert_eq!((dry.stacks, dry.stacks_found), (1, 1));
        assert!(!dry.not_carried.contains_key("crop"));
        assert!(dry.to_string().contains("would be carried with its angle"));
        let report = import(&s.catalog, &writing(&s)).unwrap();
        let w = report.written.as_ref().unwrap();
        assert_eq!((w.crops, w.crops_drawn_in), (2, 0), "{report}");
        assert!(w.crops_not_carried.is_empty(), "{report}");
        assert_eq!((w.stacks, w.stacks_there), (1, 0), "{report}");

        // The master's: the middle half across, the whole height, no
        // turn, its shape free as Lightroom had it.
        let frame = s.here.join("frame.tif");
        let side = Sidecar::load(&frame).unwrap().unwrap();
        let g = &side.current.geometry;
        let c = g.crop.unwrap();
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(near(c.x, 0.25) && near(c.y, 0.0), "{c:?}");
        assert!(near(c.w, 0.5) && near(c.h, 1.0), "{c:?}");
        assert_eq!((g.turns, g.angle), (0, 0.0));
        assert_eq!(g.aspect, greycard_edit::geometry::Aspect::Free);
        assert_eq!(side.current.light.exposure, 0.5);
        // The copy's: turned 2 degrees, held to its shape, on the
        // picture.
        let v = &side.versions[0];
        let g = &v.line.current.geometry;
        assert_eq!(g.angle, 2.0);
        assert!(g.fits(&g.crop.unwrap(), 4.0, 3.0));
        assert!(matches!(
            g.aspect,
            greycard_edit::geometry::Aspect::Ratio { .. }
        ));

        // The stack: the master on top, the copy as its version, open
        // as Lightroom had it.
        let dir = stacks::dir_in(&collections::dir_beside(&s.options.library));
        let folder = stacks::read_folder(&dir, stacks::VERSION);
        assert_eq!(folder.list.len(), 1);
        let st = &folder.list[0];
        assert!(st.open);
        assert_eq!(st.refs.len(), 2);
        assert_eq!(st.refs[0].version, None);
        assert_eq!(st.refs[1].version.as_deref(), Some(v.id.as_str()));
        assert_eq!(
            st.refs[0].hint,
            collections::hint_of(&crate::key_path(&frame))
        );

        // Again: the stack is there, and no second one is made.
        let again = import(&s.catalog, &writing(&s)).unwrap();
        let w = again.written.unwrap();
        assert_eq!((w.stacks, w.stacks_there), (0, 1));
        assert_eq!(stacks::read_folder(&dir, stacks::VERSION).list.len(), 1);
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn a_lens_rule_takes_the_name_the_index_reads_off_the_catalog_s_file() {
        // Lightroom spells the lens one way, the file's reading another:
        // the rule goes by the reading, which is the index's.
        let s = cropped_shoot("lens-rule");
        let db = Connection::open(&s.catalog).unwrap();
        db.execute_batch(
            "CREATE TABLE AgHarvestedExifMetadata (id_local INTEGER PRIMARY KEY, image,
                 cameraModelRef, lensRef);
             CREATE TABLE AgInternedExifCameraModel (id_local INTEGER PRIMARY KEY, value);
             CREATE TABLE AgInternedExifLens (id_local INTEGER PRIMARY KEY, value);
             INSERT INTO AgInternedExifCameraModel VALUES (1, 'Canon EOS R5');
             INSERT INTO AgInternedExifLens VALUES (1, 'RF35mm F1.4L VCM');
             INSERT INTO AgHarvestedExifMetadata VALUES (1, 100, 1, 1);
             INSERT INTO AgLibraryCollection VALUES (20, 'com.adobe.ag.library.smart_collection',
                 'The 35', NULL, 0.0);
             INSERT INTO AgLibraryCollectionContent (collection, content, owningModule)
                 VALUES (20, 's = { { criteria = \"lens\", operation = \"==\",
                 value = \"RF35mm F1.4L VCM\", }, combine = \"intersect\", }',
                 'ag.library.smart_collection');",
        )
        .unwrap();
        drop(db);
        let report = import(&s.catalog, &s.options).unwrap();
        assert_eq!(
            report.smart,
            vec![(
                "Crops/The 35".to_string(),
                "lens=\"RF35mm F1.4 L VCM\"".to_string()
            )],
            "{report}"
        );
        assert_eq!((report.exif_rules, report.exif_rules_skipped), (1, 0));
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    /// The import applies the body's offset where the user takes it:
    /// a GFX 100S II frame at Lightroom's Exposure 0 and its virtual
    /// copy both land at the body's offset in the sidecar, a second
    /// import leaves them there rather than adding it again, and a
    /// frame of another body and a picture land at zero.
    #[test]
    fn a_body_s_offset_is_written_once_and_only_for_that_body() {
        use crate::fixture::write_mosaic_dng_for;
        let gfx = "Fujifilm GFX 100S II";
        let want = lightroom::body_offset(Some(gfx));
        assert!(want < -0.1, "the GFX takes an offset: {want}");
        let dir = scratch("lrcat-body-offset");
        let catalog = dir.join("Offsets.lrcat");
        let here = dir.join("photos");
        let shoot = here.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        write_mosaic_dng_for(
            &shoot.join("DSCF0001.DNG"),
            "RGGB",
            "Fujifilm",
            "GFX 100S II",
        );
        write_mosaic_dng_for(&shoot.join("IMG_0002.DNG"), "RGGB", "Other", "Body");
        std::fs::write(shoot.join("IMG_0003.jpg"), b"not a picture either").unwrap();
        let db = empty_catalog(&catalog, "1504001");
        db.execute_batch(
            "INSERT INTO AgLibraryRootFolder VALUES (1, 'r', 'D:/Shoots/', 'Shoots', NULL);
             INSERT INTO AgLibraryFolder VALUES (2, 'f', NULL, 'shoot/', 1);
             INSERT INTO AgLibraryFile VALUES (10, 'a', 'DSCF0001', 'DNG', 2);
             INSERT INTO AgLibraryFile VALUES (11, 'b', 'IMG_0002', 'DNG', 2);
             INSERT INTO AgLibraryFile VALUES (12, 'c', 'IMG_0003', 'jpg', 2);
             INSERT INTO Adobe_images VALUES (100, 'i', 10, NULL, NULL, 0.0, 0.0, '');
             INSERT INTO Adobe_images VALUES (101, 'j', 11, NULL, NULL, 0.0, 0.0, '');
             INSERT INTO Adobe_images VALUES (102, 'k', 12, NULL, NULL, 0.0, 0.0, '');
             INSERT INTO Adobe_images VALUES (103, 'l', 10, 100, 'Mono', 0.0, 0.0, '');
             INSERT INTO Adobe_libraryImageDevelopHistoryStep VALUES (1, 100, 'Import');
             INSERT INTO Adobe_libraryImageDevelopHistoryStep VALUES (2, 101, 'Import');
             INSERT INTO Adobe_libraryImageDevelopHistoryStep VALUES (3, 102, 'Import');
             INSERT INTO Adobe_libraryImageDevelopHistoryStep VALUES (4, 103, 'Import');",
        )
        .unwrap();
        let insert = "INSERT INTO Adobe_imageDevelopSettings (image, text, processVersion, \
                      hasDevelopAdjustmentsEx) VALUES (?1, ?2, '15.4', 1.0)";
        for image in [100, 101, 102, 103] {
            db.execute(insert, rusqlite::params![image, develop_at(0.0)])
                .unwrap();
        }
        drop(db);
        let options = Options {
            roots: vec![("D:/Shoots".into(), here.clone())],
            library: dir.join("library").join("library.sqlite"),
            write: true,
            ..Options::default()
        };
        let exposures = || {
            let read = |name: &str| {
                Sidecar::load(&shoot.join(name))
                    .unwrap()
                    .unwrap_or_else(|| panic!("a sidecar for {name}"))
            };
            let a = read("DSCF0001.DNG");
            (
                a.current.light.exposure,
                a.versions[0].line.current.light.exposure,
                read("IMG_0002.DNG").current.light.exposure,
                read("IMG_0003.jpg").current.light.exposure,
            )
        };
        let report = import(&catalog, &options).unwrap();
        assert_eq!(report.written.as_ref().unwrap().sidecars, 3, "{report}");
        let (frame, copy, other, picture) = exposures();
        assert!((frame - want).abs() < 1e-6, "the GFX frame: {frame}");
        assert!((copy - want).abs() < 1e-6, "its copy: {copy}");
        assert_eq!(other, 0.0, "another body takes none");
        assert_eq!(picture, 0.0, "a picture takes none");
        // Again, forced so the frames go through the mapping again
        // rather than being skipped as imported already: the offset is
        // of the settings, not added to what the sidecar holds, so the
        // same edit comes out and nothing needs writing.
        let forced = Options {
            force: true,
            ..options.clone()
        };
        let again = import(&catalog, &forced).unwrap();
        let w = again.written.as_ref().unwrap();
        assert_eq!((w.left_alone, w.sidecars), (0, 0), "{again}");
        assert!(w.failed.is_empty(), "{again}");
        assert_eq!(exposures(), (frame, copy, other, picture));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn imported_edit_carries_the_crop_given_the_catalog_s_shape() {
        let s = cropped_shoot("imported-edit");
        let frame = s.here.join("frame.tif");
        let settings = "s = { CropLeft = 0.25, CropRight = 0.75 }";
        let shape = Shape {
            size: Some((4.0, 3.0)),
            orientation: Some("AB".into()),
        };
        let FrameEdit { edit, unmapped, .. } =
            imported_edit(&frame, settings, Some(&shape)).unwrap();
        assert!(edit.geometry.crop.is_some());
        assert!(!unmapped.iter().any(|n| n == "crop"));
        // Without the shape, left off and named, for a caller that cuts
        // it itself.
        let FrameEdit { edit, unmapped, .. } = imported_edit(&frame, settings, None).unwrap();
        assert!(edit.geometry.crop.is_none());
        assert!(unmapped.iter().any(|n| n == "crop"));
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn a_crop_on_a_file_that_says_no_size_is_counted_not_carried() {
        // The shoot's files are not raws: no size, no crop.
        let s = setup("crop-no-size");
        let report = import(&s.catalog, &writing(&s)).unwrap();
        let w = report.written.unwrap();
        assert_eq!(w.crops, 0);
        assert_eq!(w.crops_not_carried.get("its file says no size"), Some(&2));
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn a_root_not_mapped_is_found_beside_the_catalog_or_under_a_library_root() {
        // The shoot's root is named Shoots: a folder of that name beside
        // the catalog.
        let s = setup("near");
        let beside = s.dir.join("Shoots");
        std::fs::rename(&s.here, &beside).unwrap();
        let none = Options {
            roots: Vec::new(),
            ..s.options.clone()
        };
        let r = import(&s.catalog, &none).unwrap();
        assert_eq!(r.roots[0].how, Found::BesideCatalog, "{r}");
        assert_eq!(r.roots[0].local.as_deref(), Some(beside.as_path()));
        assert!(r.roots[0].confirmed);
        assert!(r.to_string().contains("found beside the catalog"), "{r}");
        // Under a root of the library, two folders down.
        let library_root = s.dir.join("archive");
        let deep = library_root.join("2026").join("Shoots");
        std::fs::create_dir_all(deep.parent().unwrap()).unwrap();
        std::fs::rename(&beside, &deep).unwrap();
        Roots::edit(&Roots::path_beside(&s.options.library), |l| {
            l.add(&library_root).unwrap();
        })
        .unwrap();
        let r = import(&s.catalog, &none).unwrap();
        assert!(matches!(r.roots[0].how, Found::UnderRoot(_)), "{r}");
        assert!(r.roots[0].confirmed);
        assert_eq!(r.found, 3);
        // A folder of the name holding none of its files is passed over.
        let decoy = s.dir.join("Shoots");
        std::fs::create_dir_all(&decoy).unwrap();
        let r = import(&s.catalog, &none).unwrap();
        assert!(matches!(r.roots[0].how, Found::UnderRoot(_)), "{r}");
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn a_guess_takes_the_folder_holding_the_root_s_files_not_the_first_of_its_name() {
        // Two folders named for the root under a library root: another
        // shoot's, the first in order, with one file of the same name;
        // and the shoot's own, with both its files there.
        let s = setup("decoy");
        let library_root = s.dir.join("archive");
        let decoy = library_root
            .join("2025-a")
            .join("Shoots")
            .join("2026/harbor");
        std::fs::create_dir_all(&decoy).unwrap();
        std::fs::write(decoy.join("IMG_0002.CR3"), "another shoot's").unwrap();
        let real = library_root.join("2026-b").join("Shoots");
        std::fs::create_dir_all(library_root.join("2026-b")).unwrap();
        std::fs::rename(&s.here, &real).unwrap();
        Roots::edit(&Roots::path_beside(&s.options.library), |l| {
            l.add(&library_root).unwrap();
        })
        .unwrap();
        let none = Options {
            roots: Vec::new(),
            ..s.options.clone()
        };
        let r = import(&s.catalog, &none).unwrap();
        let root = &r.roots[0];
        assert!(
            root.local
                .as_deref()
                .is_some_and(|l| l.ends_with("2026-b/Shoots")),
            "{r}"
        );
        assert!(matches!(root.how, Found::UnderRoot(_)));
        assert_eq!((root.checked, root.checked_found), (3, 2), "{r}");
        // The shoot's own folder gone: the other shoot's is no guess.
        std::fs::remove_dir_all(&real).unwrap();
        let r = import(&s.catalog, &none).unwrap();
        assert!(r.roots[0].local.is_none(), "{r}");
        assert!(!r.roots[0].confirmed);
        assert_eq!(r.found, 0);
        // It is said as the nearest, to be chosen by hand.
        let miss = r.roots[0].near_miss.as_ref().unwrap();
        assert!(miss.dir.ends_with("2025-a/Shoots"), "{miss:?}");
        assert_eq!((miss.there, miss.looked_for), (1, 3));
        let said = r.to_string();
        assert!(
            said.contains("holds 1 of 3 looked for; --root it to take it"),
            "{said}"
        );
        let shown = r.sheet(&|p| p.display().to_string()).details;
        assert!(
            shown
                .iter()
                .any(|(_, t)| t.ends_with("holds 1 of 3 looked for; choose it to take it")),
            "{shown:?}"
        );
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn a_guess_passes_over_files_of_the_root_s_names_at_other_sizes() {
        // The catalog keeps each file's size in its import hash: a
        // folder holding every name at another size is another shoot.
        let s = setup("sizes");
        let db = Connection::open(&s.catalog).unwrap();
        db.execute_batch("ALTER TABLE AgLibraryFile ADD COLUMN importHash;")
            .unwrap();
        for (id, name) in [
            (10, "IMG_0001.CR3"),
            (11, "IMG_0002.CR3"),
            (12, "IMG_0003.CR3"),
        ] {
            let size = format!("not a raw, {name}").len();
            db.execute(
                "UPDATE AgLibraryFile SET importHash = ?1 WHERE id_local = ?2",
                rusqlite::params![format!("800960509.71:{name}:{size}"), id],
            )
            .unwrap();
        }
        drop(db);
        let other = s.dir.join("Shoots").join("2026/harbor");
        std::fs::create_dir_all(&other).unwrap();
        for n in ["IMG_0001.CR3", "IMG_0002.CR3", "IMG_0003.CR3"] {
            std::fs::write(other.join(n), "another shoot's frame, longer").unwrap();
        }
        let none = Options {
            roots: Vec::new(),
            ..s.options.clone()
        };
        let r = import(&s.catalog, &none).unwrap();
        assert!(r.roots[0].local.is_none(), "{r}");
        // At their sizes, beside the catalog: taken.
        std::fs::remove_dir_all(s.dir.join("Shoots")).unwrap();
        std::fs::rename(&s.here, s.dir.join("Shoots")).unwrap();
        let r = import(&s.catalog, &none).unwrap();
        assert_eq!(r.roots[0].how, Found::BesideCatalog, "{r}");
        assert_eq!(r.found, 3);
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn a_turn_in_lightroom_is_carried_on_every_frame_and_a_flip_is_counted() {
        let s = cropped_shoot("turns");
        let db = Connection::open(&s.catalog).unwrap();
        // The master turned right in Lightroom, its crop taken off; the
        // copy flipped there; a third file never developed, turned.
        crate::fixture::write_frame(&s.here.join("other.tif"), &crate::fixture::R5, 2);
        db.execute_batch(
            "UPDATE Adobe_images SET orientation = 'BC' WHERE id_local = 100;
             UPDATE Adobe_images SET orientation = 'BA' WHERE id_local = 101;
             UPDATE Adobe_imageDevelopSettings SET text = 's = { Exposure2012 = 0.5 }'
                 WHERE image = 100;
             INSERT INTO AgLibraryFile VALUES (11, 'b', 'other', 'tif', 2);
             INSERT INTO Adobe_images (id_local, id_global, rootFile, masterImage, copyName,
                 rating, pick, fileWidth, fileHeight, orientation)
                 VALUES (102, 'k', 11, NULL, NULL, 1.0, 0.0, 4.0, 3.0, 'DA');
             INSERT INTO Adobe_images (id_local, id_global, rootFile, masterImage, copyName,
                 rating, pick, fileWidth, fileHeight, orientation)
                 VALUES (103, 'l', 11, NULL, NULL, 0.0, 0.0, 4.0, 3.0, 'ZZ');",
        )
        .unwrap();
        drop(db);
        // The dry run says what it can without the files' tags: the
        // flip and the code it does not read.
        let dry = import(&s.catalog, &s.options).unwrap();
        assert_eq!((dry.oriented, dry.flipped, dry.flipped_found), (4, 1, 1));
        assert_eq!(dry.orientations_unread.get("ZZ"), Some(&1));
        let said = dry.to_string();
        assert!(said.contains("1 flipped there (1 found)"), "{said}");
        let report = import(&s.catalog, &writing(&s)).unwrap();
        let w = report.written.as_ref().unwrap();
        assert_eq!((w.turned, w.flips), (2, 1), "{report}");
        assert_eq!(w.orientations_unread.get("ZZ"), Some(&1), "{report}");
        assert_eq!(
            w.crops_not_carried
                .get("Lightroom's orientation mirrors the frame"),
            Some(&1),
            "{report}"
        );
        let side = Sidecar::load(&s.here.join("frame.tif")).unwrap().unwrap();
        assert_eq!(
            side.current.geometry.turns, 3,
            "a quarter right is three of ours"
        );
        assert!(side.current.geometry.crop.is_none());
        assert_eq!(side.versions[0].line.current.geometry.turns, 0);
        // Never developed, turned left: one of ours, a state of its own.
        let other = Sidecar::load(&s.here.join("other.tif")).unwrap().unwrap();
        assert_eq!(other.current.geometry.turns, 1);
        assert_eq!(other.history.len(), 1);
        assert!(
            report.to_string().contains("2 turned in Lightroom"),
            "{report}"
        );
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn creations_are_left_out_when_asked() {
        let s = setup("creations");
        let options = Options {
            no_creations: true,
            ..writing(&s)
        };
        let report = import(&s.catalog, &options).unwrap();
        let names: Vec<&str> = report.collections.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Harbor/Clients/Cover"]);
        assert_eq!(report.creations_left, ["Harbor/Album"]);
        assert!(report.to_string().contains("left out: Harbor/Album"));
        let folder = collections::read_folder(
            &collections::dir_beside(&s.options.library),
            collections::VERSION,
        );
        assert!(folder.list.iter().all(|c| c.name != "Album"));
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn progress_is_said_as_the_import_goes() {
        let s = setup("progress");
        let mut seen = Vec::new();
        import_with(&s.catalog, &writing(&s), &mut |p| seen.push(p)).unwrap();
        assert_eq!(seen.first(), Some(&Progress::Mapping { done: 1, of: 1 }));
        assert!(
            seen.contains(&Progress::Writing { done: 2, of: 2 }),
            "{seen:?}"
        );
        assert_eq!(seen.last(), Some(&Progress::Finishing));
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    /// A stop set while the roots are looked for ends a write before it
    /// writes anything, and nothing more is said.
    #[test]
    fn a_stop_ends_the_import_before_it_writes() {
        let s = setup("stopped");
        let before = tree(&s.dir);
        let stop = AtomicBool::new(false);
        let mut seen = Vec::new();
        let read = import_until(
            &s.catalog,
            &writing(&s),
            &mut |p| {
                seen.push(p);
                if matches!(p, Progress::Mapping { .. }) {
                    stop.store(true, Ordering::Relaxed);
                }
            },
            &stop,
        );
        assert!(matches!(read, Err(ImportError::Stopped)), "{read:?}");
        assert_eq!(seen, [Progress::Mapping { done: 1, of: 1 }]);
        assert_eq!(tree(&s.dir), before, "nothing written");
        // Set before it starts, it reads the catalog and goes no further.
        let mut heard = 0;
        let read = import_until(&s.catalog, &s.options, &mut |_| heard += 1, &stop);
        assert!(matches!(read, Err(ImportError::Stopped)), "{read:?}");
        assert_eq!(heard, 0);
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    /// A catalog of `n` synthetic raws in one folder: every other frame
    /// developed with a crop, every third turned in Lightroom, every
    /// fourth with a virtual copy developed apart, every seventh not on
    /// the disk, and the fifth with a sidecar of its own already. One
    /// collection holds every image, the copies with them, and one stack
    /// the first two frames and the first copy.
    fn many_shoot(what: &str, n: i64) -> Setup {
        let dir = scratch(&format!("lrcat-{what}"));
        let catalog = dir.join("Many.lrcat");
        let here = dir.join("photos");
        std::fs::create_dir_all(&here).unwrap();
        let db = empty_catalog(&catalog, "1504001");
        db.execute_batch(
            "ALTER TABLE Adobe_images ADD COLUMN fileWidth;
             ALTER TABLE Adobe_images ADD COLUMN fileHeight;
             ALTER TABLE Adobe_images ADD COLUMN orientation;
             ALTER TABLE AgLibraryFolderStack ADD COLUMN collapsed;
             CREATE TABLE AgLibraryFolderStackImage (id_local INTEGER PRIMARY KEY,
                 collapsed, image, position, stack);
             INSERT INTO AgLibraryRootFolder VALUES (1, 'r', 'D:/Many/', 'Many', NULL);
             INSERT INTO AgLibraryFolder VALUES (2, 'f', NULL, '', 1);
             INSERT INTO AgLibraryCollection VALUES (1, 'com.adobe.ag.library.collection',
                 'All', NULL, 0.0);
             INSERT INTO AgLibraryFolderStack VALUES (1, 's', 0);",
        )
        .unwrap();
        let insert_settings = "INSERT INTO Adobe_imageDevelopSettings (image, text, \
                               processVersion, hasDevelopAdjustmentsEx) VALUES (?1, ?2, '15.4', 1.0)";
        let image = "INSERT INTO Adobe_images (id_local, id_global, rootFile, masterImage, \
                     copyName, rating, pick, fileWidth, fileHeight, orientation) \
                     VALUES (?1, 'i', ?2, ?3, ?4, ?5, 0.0, 24.0, 18.0, ?6)";
        let mut member = 0;
        let mut collect = |db: &Connection, id: i64| {
            member += 1;
            db.execute(
                "INSERT INTO AgLibraryCollectionImage VALUES (?1, 1, ?2, ?3)",
                rusqlite::params![member, id, format!("{member:06}")],
            )
            .unwrap();
        };
        for i in 1..=n {
            let name = format!("f{i:04}");
            if i % 7 != 0 {
                let raw = here.join(format!("{name}.dng"));
                crate::fixture::write_mosaic_dng_for(&raw, "RGGB", "Canon", "Canon EOS R5");
                // A length of its own, so each frame hashes apart.
                let mut file = std::fs::OpenOptions::new().append(true).open(&raw).unwrap();
                file.write_all(&vec![0; i as usize]).unwrap();
            }
            db.execute(
                "INSERT INTO AgLibraryFile VALUES (?1, 'a', ?2, 'dng', 2)",
                rusqlite::params![i, name],
            )
            .unwrap();
            let turned = if i % 3 == 0 { "BC" } else { "AB" };
            let id = 1000 + i;
            db.execute(
                image,
                rusqlite::params![id, i, None::<i64>, None::<String>, (i % 6) as f64, turned],
            )
            .unwrap();
            collect(&db, id);
            if i % 2 == 0 {
                db.execute(insert_settings, rusqlite::params![id, DEVELOP])
                    .unwrap();
            }
            if i % 4 == 0 {
                let copy = 100_000 + i;
                db.execute(image, rusqlite::params![copy, i, id, "Warm", 1.0, turned])
                    .unwrap();
                db.execute(insert_settings, rusqlite::params![copy, develop_at(-0.5)])
                    .unwrap();
                collect(&db, copy);
            }
        }
        db.execute_batch(
            "INSERT INTO AgLibraryFolderStackImage VALUES (1, 0, 1001, '1.0', 1);
             INSERT INTO AgLibraryFolderStackImage VALUES (2, 0, 1002, '2.0', 1);
             INSERT INTO AgLibraryFolderStackImage VALUES (3, 0, 100004, '3.0', 1);",
        )
        .unwrap();
        drop(db);
        let mut mine = Sidecar::default();
        mine.meta.rating = 2;
        mine.save(&here.join("f0005.dng")).unwrap();
        let options = Options {
            roots: vec![("D:/Many".into(), here.clone())],
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

    /// With `GREYCARD_LRCAT_FRAMES` a count: a write of that many
    /// synthetic frames timed, on the temporary folder. Ignored, since
    /// it is a measurement.
    #[test]
    #[ignore]
    fn a_write_of_many_frames_timed() {
        let n = std::env::var("GREYCARD_LRCAT_FRAMES")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(400);
        let s = many_shoot("timed", n);
        let start = std::time::Instant::now();
        let report = import(&s.catalog, &writing(&s)).unwrap();
        let took = start.elapsed();
        let w = report.written.as_ref().unwrap();
        println!(
            "{n} frames: {} sidecars in {:.3} s, {} bytes of raws read",
            w.sidecars,
            took.as_secs_f64(),
            w.read
        );
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    /// The folder put back as `before` had it: every file it did not
    /// hold removed, every one it did written back.
    fn put_back(dir: &Path, before: &BTreeMap<PathBuf, Vec<u8>>) {
        for (path, bytes) in tree(dir) {
            match before.get(&path) {
                None => std::fs::remove_file(&path).unwrap(),
                Some(was) if *was != bytes => std::fs::write(&path, was).unwrap(),
                Some(_) => {}
            }
        }
    }

    /// Each sidecar under `dir` as a write leaves it, but for what
    /// differs from one write to the next (ids and times): the meta,
    /// the turn, the edit, and each version's name and edit.
    #[allow(clippy::type_complexity)]
    fn sidecars_under(s: &Setup) -> BTreeMap<String, (Meta, u8, Edit, Vec<(String, Edit)>)> {
        let mut out = BTreeMap::new();
        for e in std::fs::read_dir(&s.here).unwrap().flatten() {
            let raw = e.path();
            if raw.extension().is_none_or(|x| x != "dng") {
                continue;
            }
            if let Some(c) = Sidecar::load(&raw).unwrap() {
                let versions = c
                    .versions
                    .iter()
                    .map(|v| (v.name.clone(), v.line.current.clone()))
                    .collect();
                let name = raw.file_name().unwrap().to_string_lossy().into_owned();
                out.insert(name, (c.meta.clone(), c.turn, c.current.clone(), versions));
            }
        }
        out
    }

    /// A write on a few threads writes what one frame after another
    /// did: the same report, frame for frame in the catalog's order,
    /// the same sidecars, collections, and stacks, a forced write too;
    /// and the write is said as frames finish, from none of them to all.
    #[test]
    fn a_write_on_a_few_threads_is_the_write_one_at_a_time() {
        let s = many_shoot("threads", 60);
        let before = tree(&s.dir);
        let never = AtomicBool::new(false);
        for force in [false, true] {
            let options = Options {
                force,
                ..writing(&s)
            };
            let mut runs = Vec::new();
            for threads in [1, WRITE_THREADS] {
                let mut said = Vec::new();
                let report = import_on(
                    &s.catalog,
                    &options,
                    &mut |p| {
                        if let Progress::Writing { done, of } = p {
                            said.push((done, of));
                        }
                    },
                    &never,
                    threads,
                )
                .unwrap();
                let of = said[0].1;
                assert_eq!(
                    said,
                    (0..=of).map(|d| (d, of)).collect::<Vec<_>>(),
                    "{threads} threads"
                );
                let stacks = stacks::read_folder(
                    &stacks::dir_in(&collections::dir_beside(&s.options.library)),
                    stacks::VERSION,
                );
                let collections = collections::read_folder(
                    &collections::dir_beside(&s.options.library),
                    collections::VERSION,
                );
                let frames = |c: &Collection| c.refs.len();
                runs.push((
                    report,
                    sidecars_under(&s),
                    collections.list.iter().map(frames).collect::<Vec<_>>(),
                    stacks.list.len(),
                ));
                put_back(&s.dir, &before);
            }
            let (one, few) = (&runs[0], &runs[1]);
            let w = one.0.written.as_ref().unwrap();
            assert_eq!(w.sidecars, if force { 52 } else { 51 }, "{}", one.0);
            assert_eq!(w.left_alone, usize::from(!force));
            assert!(w.crops > 0 && w.turned > 0, "{}", one.0);
            assert_eq!(one.2, [60 + 15 - 8 - 2], "the collection's frames found");
            assert_eq!(one.3, 1, "the stack");
            assert_eq!(one, few, "force {force}");
        }
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    /// Two of the catalog's files that are one file here, by two roots
    /// on one folder, are written one after the other on the pool, as
    /// one frame after another wrote them: the sidecar holds both
    /// files' copies, and every version the collection names is in it.
    #[test]
    fn two_catalog_files_on_one_file_here_are_written_in_turn() {
        let dir = scratch("lrcat-one-file");
        let catalog = dir.join("Twice.lrcat");
        let here = dir.join("photos");
        std::fs::create_dir_all(&here).unwrap();
        crate::fixture::write_mosaic_dng_for(&here.join("frame.dng"), "RGGB", "Canon", "R5");
        let db = empty_catalog(&catalog, "1504001");
        db.execute_batch(
            "INSERT INTO AgLibraryRootFolder VALUES (1, 'r', 'D:/A/', 'A', NULL);
             INSERT INTO AgLibraryRootFolder VALUES (2, 's', 'E:/B/', 'B', NULL);
             INSERT INTO AgLibraryFolder VALUES (3, 'f', NULL, '', 1);
             INSERT INTO AgLibraryFolder VALUES (4, 'g', NULL, '', 2);
             INSERT INTO AgLibraryFile VALUES (10, 'a', 'frame', 'dng', 3);
             INSERT INTO AgLibraryFile VALUES (20, 'b', 'frame', 'dng', 4);
             INSERT INTO Adobe_images VALUES (100, 'i', 10, NULL, NULL, 4.0, 0.0, '');
             INSERT INTO Adobe_images VALUES (101, 'j', 10, 100, 'Warm', 0.0, 0.0, '');
             INSERT INTO Adobe_images VALUES (200, 'k', 20, NULL, NULL, 2.0, 0.0, '');
             INSERT INTO Adobe_images VALUES (201, 'l', 20, 200, 'Cool', 0.0, 0.0, '');
             INSERT INTO AgLibraryCollection VALUES (1, 'com.adobe.ag.library.collection',
                 'Both', NULL, 0.0);
             INSERT INTO AgLibraryCollectionImage VALUES (1, 1, 101, 'a');
             INSERT INTO AgLibraryCollectionImage VALUES (2, 1, 201, 'b');",
        )
        .unwrap();
        drop(db);
        let options = Options {
            roots: vec![("D:/A".into(), here.clone()), ("E:/B".into(), here.clone())],
            library: dir.join("library").join("library.sqlite"),
            write: true,
            ..Options::default()
        };
        let never = AtomicBool::new(false);
        let report = import_on(&catalog, &options, &mut |_| {}, &never, WRITE_THREADS).unwrap();
        let w = report.written.as_ref().unwrap();
        assert_eq!((w.sidecars, w.copies_as_own), (2, 0), "{report}");
        let sidecar = Sidecar::load(&here.join("frame.dng")).unwrap().unwrap();
        let names: Vec<&str> = sidecar.versions.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["Warm", "Cool"]);
        let folder = collections::read_folder(
            &collections::dir_beside(&options.library),
            collections::VERSION,
        );
        let both = folder.list.iter().find(|c| c.name == "Both").unwrap();
        assert_eq!(both.refs.len(), 2);
        for r in &both.refs {
            let v = r.version.as_deref().unwrap();
            assert!(sidecar.versions.iter().any(|s| s.id == v), "{v} dangles");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The write reads each raw once for its probe and its stance,
    /// however many of its images turn or crop it: a synthetic DNG is
    /// read whole, so the bytes are each written frame's length. A clip
    /// in the catalog is not read at all. And nothing the import does,
    /// on any thread, the cameras' names among it, memory-maps a raw:
    /// a volume that goes away under a mapped raw is a SIGBUS nothing
    /// catches.
    #[test]
    fn a_write_reads_each_raw_once_and_maps_none() {
        let s = many_shoot("read-once", 24);
        let db = Connection::open(&s.catalog).unwrap();
        db.execute_batch(
            "CREATE TABLE AgHarvestedExifMetadata (id_local INTEGER PRIMARY KEY, image,
                 cameraModelRef, lensRef);
             CREATE TABLE AgInternedExifCameraModel (id_local INTEGER PRIMARY KEY, value);
             CREATE TABLE AgInternedExifLens (id_local INTEGER PRIMARY KEY, value);
             INSERT INTO AgInternedExifCameraModel VALUES (1, 'Canon EOS R5');
             INSERT INTO AgInternedExifLens VALUES (1, 'RF35mm F1.4L VCM');
             INSERT INTO AgHarvestedExifMetadata VALUES (1, 1001, 1, 1);
             INSERT INTO AgLibraryFile VALUES (900, 'm', 'clip', 'mov', 2);
             INSERT INTO Adobe_images (id_local, id_global, rootFile, rating, pick)
                 VALUES (9000, 'm', 900, 3.0, 0.0);",
        )
        .unwrap();
        drop(db);
        std::fs::write(s.here.join("clip.mov"), vec![7u8; 1 << 16]).unwrap();
        greycard_core::decode::watch_maps_under(&s.dir);
        let report = import(&s.catalog, &writing(&s)).unwrap();
        let w = report.written.as_ref().unwrap();
        let mut lengths = 0;
        for e in std::fs::read_dir(&s.here).unwrap().flatten() {
            let raw = e.path();
            // The fifth frame's sidecar holds a rating: left alone.
            if raw.extension().is_some_and(|x| x == "dng") && !raw.ends_with("f0005.dng") {
                lengths += std::fs::metadata(&raw).unwrap().len();
            }
        }
        assert!(w.crops > 0 && w.turned > 0, "{report}");
        assert!(w.frames.contains(&s.here.join("clip.mov")), "its rating");
        assert_eq!(w.read, lengths, "{report}");
        let _ = starting_edit(&s.here.join("f0001.dng"));
        assert_eq!(greycard_core::decode::maps_under(&s.dir), 0, "raws mapped");
        let clip = FrameRead::of(&s.here.join("clip.mov"));
        assert!(clip.stance.is_none() && clip.camera.is_none() && clip.bytes == 0);
        std::fs::remove_dir_all(&s.dir).unwrap();
    }
}
