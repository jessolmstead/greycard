//! Lightroom's own exports of a catalog's images, found beside them and
//! paired with the settings they were rendered from: what measuring the
//! import's mapping against Lightroom takes (the editor's
//! `--measure-lightroom`).
//!
//! **Which settings an export was rendered from.** Lightroom puts every
//! export in the image's develop history, as a step named `Export -
//! <destination> (<local time>)` that carries the settings of that
//! moment, deflated as the XMP packets are, and its time in seconds from
//! 2001. A file written just before one of those steps was written by
//! it ([`SAME_EXPORT`], [`STEP_BEFORE`]), and its settings are the
//! step's ([`Confidence::History`]). A file found by its name alone is
//! taken with the image's current settings ([`Confidence::Name`], or
//! [`Confidence::Elsewhere`] when the image was exported at other
//! times); in the catalogs we read, an image's last export step held its
//! current settings in all but a few cases, and the time is what says
//! which. An image never developed in the catalog is not paired by name:
//! the import writes nothing for it.
//!
//! **Where the files are.** Wherever the user sent them, so in folders
//! named for exports ([`EXPORT_FOLDERS`], any case) in the raw's folder,
//! in the folder above it, in the catalog's folder and the folder above
//! that, and in any folder given. A file is a raw's when its name is the
//! raw's stem, alone or with `-2` and the like after it (Lightroom's
//! suffix when the name is taken, a virtual copy's as a rule) or with
//! `_`, `-` or a space and more ("_bw").
//!
//! **One file, one image.** A master and its copies share a stem and
//! are often exported in the same second, so the time alone does not
//! tell them apart; the shape does. Each image's export has the crop's
//! proportions ([`Crop`]), and a file is an image's only when its long
//! side over its short side is that image's, within [`ASPECT`]. Where a
//! color master and its black and white copy are one shape (in one
//! catalog every image left in doubt was), the less colorful of their
//! files is the copy's. What is left with more than one way to pair is
//! reported as ambiguous, not guessed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use greycard_edit::lightroom::{self, Value};

use crate::lightroom_catalog::{self, ImportError, RootReport};

/// Folder names an export is looked for under, compared without case.
pub const EXPORT_FOLDERS: [&str; 6] = ["export", "exports", "exported", "jpeg", "jpegs", "jpg"];

/// Seconds from the Unix epoch to Lightroom's, 2001-01-01 UTC.
const EPOCH_2001: f64 = 978_307_200.0;

/// How long after a file is written its export step may be stamped, in
/// seconds. Lightroom stamps the step once the file is written: in one
/// catalog of about 280 images every one of its 74 files had a step of
/// its image stamped 0.1 to 22 seconds after it, most within two.
pub const SAME_EXPORT: f64 = 60.0;

/// How long before a file its export step may be stamped, in seconds:
/// a few were, by up to a second and a half, in the same catalog.
pub const STEP_BEFORE: f64 = 5.0;

/// How far apart two shapes may be, as a share of the long side over
/// the short, and still be one: an export resized to a long edge rounds
/// its short side to a pixel.
pub const ASPECT: f64 = 0.015;

/// What to pair with.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Where the catalog's roots are now, as the import takes them.
    pub roots: Vec<(String, PathBuf)>,
    /// More folders to look in for exports.
    pub folders: Vec<PathBuf>,
}

/// One image of the catalog, as the measurement reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    pub image: i64,
    /// Where its raw is here; none when its root is not mapped or the
    /// file is not there.
    pub raw: Option<PathBuf>,
    /// The file's name as the catalog has it.
    pub name: String,
    /// A virtual copy's name.
    pub copy: Option<String>,
    /// The current develop settings, a Lua table.
    pub settings: Option<String>,
    pub process: Option<String>,
    /// Whether Lightroom counts it as developed
    /// (`hasDevelopAdjustmentsEx`, else a step past its first).
    pub developed: bool,
    /// The settings it arrived with: its first history step's.
    pub imported: Option<String>,
    /// Its export steps, in time order.
    pub exports: Vec<Step>,
    /// The file's size in pixels as the catalog has it, before any turn.
    pub size: Option<(f64, f64)>,
    /// Lightroom's orientation code of the image (`AB`, `DA`).
    pub orientation: Option<String>,
}

impl Shot {
    /// What the catalog keeps of the file's frame, which its crop is
    /// laid over.
    pub fn shape(&self) -> crate::lightroom_catalog::Shape {
        crate::lightroom_catalog::Shape {
            size: self.size,
            orientation: self.orientation.clone(),
        }
    }
}

impl Shot {
    /// The file name without its extension.
    pub fn stem(&self) -> &str {
        self.name.rsplit_once('.').map_or(&self.name, |(s, _)| s)
    }

    /// The long side over the short of what `settings` export: the
    /// crop's shape when cropped, the file's otherwise. None without
    /// the file's size.
    pub fn aspect(&self, settings: &str) -> Option<f64> {
        let (w, h) = self.size?;
        let (cw, ch) = match Crop::of(&words(settings)) {
            Some(c) => c.size(w, h),
            None => (w, h),
        };
        long_over_short(cw, ch)
    }
}

/// A step of an image's develop history that exported it.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// `Export - Hard Drive (<local time>)`.
    pub name: String,
    /// When, in seconds from the Unix epoch.
    pub at: f64,
    /// The settings then; none when they will not read.
    pub settings: Option<String>,
}

/// The long side over the short; none for an empty shape.
fn long_over_short(w: f64, h: f64) -> Option<f64> {
    let (long, short) = if w >= h { (w, h) } else { (h, w) };
    (short > 0.0).then(|| long / short)
}

pub use greycard_edit::lightroom::crop::Crop;

/// The settings' top-level keys with their values as written; a nested
/// table that holds anything (a curve, the masks) is its key with an
/// empty value, and an empty one (`LensBlur = { }` on every image) is
/// left out. Empty when the text does not read.
pub fn words(settings: &str) -> BTreeMap<String, String> {
    lightroom::parse_table(settings)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, v)| match v {
            Value::Word(w) => Some((k?, w)),
            Value::Table(items) if !items.is_empty() => Some((k?, String::new())),
            Value::Table(_) => None,
        })
        .collect()
}

/// How sure a pair is of the settings its export was rendered from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// The file was written by an export step in the image's history:
    /// the settings are that step's.
    History,
    /// The file is the image's by name and shape, and its history has
    /// no export step: the image's current settings are assumed.
    Name,
    /// As [`Confidence::Name`], but the history has export steps, none
    /// at the file's time: the file was written by another export, from
    /// another catalog or a copy of this one, and the current settings
    /// are a guess at what it held.
    Elsewhere,
}

impl Confidence {
    pub fn word(self) -> &'static str {
        match self {
            Confidence::History => "history",
            Confidence::Name => "name",
            Confidence::Elsewhere => "name, exported elsewhere",
        }
    }
}

/// An image and Lightroom's export of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    /// Into [`Pairing::shots`].
    pub shot: usize,
    pub export: PathBuf,
    pub confidence: Confidence,
    /// The export step that wrote the file, by its name, for a
    /// [`Confidence::History`] pair.
    pub step: Option<String>,
    /// The settings the export was rendered from, as [`Confidence`]
    /// says.
    pub settings: String,
}

/// What pairing a catalog with its exports found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pairing {
    /// The catalog's schema number.
    pub schema: i64,
    pub shots: Vec<Shot>,
    pub roots: Vec<RootReport>,
    /// The folders looked in, and the files in them.
    pub folders: Vec<(PathBuf, usize)>,
    pub pairs: Vec<Pair>,
    /// Images their history says were exported, no file of whose name
    /// was found.
    pub not_found: Vec<usize>,
    /// Images with files of their name, none of them their shape.
    pub misshapen: Vec<(usize, Vec<PathBuf>)>,
    /// Images never developed in the catalog with files of their name
    /// and no export step at the files' time: the import writes nothing
    /// for such an image, so whatever rendered the files is not what it
    /// carries, and they are not paired.
    pub undeveloped: Vec<(usize, Vec<PathBuf>)>,
    /// Images that more than one file could be, or that share the
    /// file they could be with another image, with those files.
    pub ambiguous: Vec<(usize, Vec<PathBuf>)>,
    /// Files in the folders that are no image's.
    pub unclaimed: Vec<PathBuf>,
    /// Images neither exported by their history nor with a file of
    /// their name.
    pub never_exported: usize,
}

/// One file in an export folder.
#[derive(Debug, Clone)]
struct Exported {
    path: PathBuf,
    /// The name without the extension, lowercased.
    stem: String,
    /// Seconds from the Unix epoch.
    written: Option<f64>,
    aspect: Option<f64>,
}

/// Read the catalog at `path`, find its images' raws as the import
/// would, and pair each image with Lightroom's export of it.
pub fn pair(path: &Path, options: &Options) -> Result<Pairing, ImportError> {
    let (shots, roots, schema) = shots(path, &options.roots)?;
    let folders = folders_for(path, &shots, &options.folders);
    let mut files = Vec::new();
    let mut listed = Vec::new();
    for folder in &folders {
        let found = exported_in(folder);
        listed.push((folder.clone(), found.len()));
        files.extend(found);
    }
    let mut pairing = pair_files(shots, &files, &spread);
    pairing.roots = roots;
    pairing.schema = schema;
    pairing.folders = listed;
    Ok(pairing)
}

/// The catalog's images with their raws found as the import finds them,
/// their history's first step and export steps, and their sizes.
fn shots(
    path: &Path,
    given: &[(String, PathBuf)],
) -> Result<(Vec<Shot>, Vec<RootReport>, i64), ImportError> {
    let catalog = lightroom_catalog::read(path)?;
    let (roots, local) =
        lightroom_catalog::map_roots(&catalog, given, &lightroom_catalog::Near::beside(path))?;
    let (db, ..) = lightroom_catalog::open(path)?;
    let mut steps: HashMap<i64, Vec<Step>> = HashMap::new();
    {
        let mut stmt = db.prepare(
            "SELECT image, dateCreated, name, text FROM Adobe_libraryImageDevelopHistoryStep \
             ORDER BY image, dateCreated, id_local",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let image: i64 = r.get(0)?;
            let at = r.get::<_, Option<f64>>(1).ok().flatten().unwrap_or(0.0);
            let name = r
                .get::<_, Option<String>>(2)
                .ok()
                .flatten()
                .unwrap_or_default();
            let settings = match r.get_ref(3)? {
                rusqlite::types::ValueRef::Blob(b) if b.starts_with(b"s =") => {
                    Some(String::from_utf8_lossy(b).into_owned())
                }
                rusqlite::types::ValueRef::Blob(b) => lightroom_catalog::inflated(b),
                rusqlite::types::ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
                _ => None,
            };
            steps.entry(image).or_default().push(Step {
                name,
                at: at + EPOCH_2001,
                settings,
            });
        }
    }
    let sizes: HashMap<i64, (f64, f64)> = db
        .prepare("SELECT id_local, fileWidth, fileHeight FROM Adobe_images")?
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<f64>>(1).ok().flatten(),
                r.get::<_, Option<f64>>(2).ok().flatten(),
            ))
        })?
        .filter_map(|row| {
            let (id, w, h) = row.ok()?;
            Some((id, (w?, h?)))
        })
        .collect();
    let shots = catalog
        .images
        .iter()
        .map(|image| {
            let raw = local
                .get(&image.root)
                .map(|dir| lightroom_catalog::under(dir, &image.relative))
                .filter(|p| p.is_file());
            let history = steps.remove(&image.id).unwrap_or_default();
            let imported = history.first().and_then(|s| s.settings.clone());
            let exports = history
                .into_iter()
                .filter(|s| s.name.starts_with("Export - "))
                .collect();
            Shot {
                image: image.id,
                raw,
                name: image
                    .relative
                    .rsplit('/')
                    .next()
                    .unwrap_or(&image.relative)
                    .to_string(),
                copy: image.copy_name.clone().filter(|_| image.master.is_some()),
                settings: image.develop.clone(),
                process: image.process.clone(),
                developed: image.edited,
                imported,
                exports,
                size: sizes.get(&image.id).copied(),
                orientation: image.shape.orientation.clone(),
            }
        })
        .collect();
    Ok((shots, roots, catalog.schema))
}

/// The folders an export may be in: those named for one under each
/// raw's folder, the folder above it, the catalog's folder and the
/// folder above that, and the ones given. Each once, in order.
fn folders_for(catalog: &Path, shots: &[Shot], given: &[PathBuf]) -> Vec<PathBuf> {
    let mut bases: BTreeSet<PathBuf> = BTreeSet::new();
    let mut add = |dir: Option<&Path>| {
        if let Some(d) = dir.filter(|d| !d.as_os_str().is_empty()) {
            bases.insert(d.to_path_buf());
            if let Some(up) = d.parent().filter(|u| !u.as_os_str().is_empty()) {
                bases.insert(up.to_path_buf());
            }
        }
    };
    let catalog_dir = std::path::absolute(catalog).unwrap_or_else(|_| catalog.to_path_buf());
    add(catalog_dir.parent());
    let raw_dirs: BTreeSet<&Path> = shots
        .iter()
        .filter_map(|s| s.raw.as_deref()?.parent())
        .collect();
    for d in raw_dirs {
        add(Some(d));
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for base in &bases {
        let Ok(entries) = std::fs::read_dir(base) else {
            continue;
        };
        let mut named: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_lowercase();
                EXPORT_FOLDERS.contains(&name.as_str())
            })
            .map(|e| e.path())
            .collect();
        named.sort();
        for p in named {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    for p in given {
        if !out.contains(p) {
            out.push(p.clone());
        }
    }
    out
}

/// The JPEGs in `folder`, not below it.
fn exported_in(folder: &Path) -> Vec<Exported> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut out: Vec<Exported> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"))
        })
        .filter(|p| p.is_file())
        .map(|path| {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let written = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64());
            let aspect = image::image_dimensions(&path)
                .ok()
                .and_then(|(w, h)| long_over_short(f64::from(w), f64::from(h)));
            Exported {
                path,
                stem,
                written,
                aspect,
            }
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Whether settings turn the picture black and white.
pub fn is_mono(settings: &str) -> bool {
    words(settings)
        .get("ConvertToGrayscale")
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// How colorful the picture at `path` is: the median, over a grid of
/// samples, of the spread between a pixel's channels, in 8-bit levels.
/// A toned black and white keeps a little color, so it is not zero
/// there; in one catalog its toned black and white copies read 6 to 8
/// and their color masters 27 to 30. None when it will not decode.
fn spread(path: &Path) -> Option<f32> {
    let rgb = image::open(path).ok()?.to_rgb8();
    let (w, h) = rgb.dimensions();
    let step = (w.max(h) / 200).max(1) as usize;
    let mut spreads: Vec<u8> = Vec::new();
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let [r, g, b] = rgb.get_pixel(x, y).0;
            spreads.push(r.max(g).max(b) - r.min(g).min(b));
        }
    }
    if spreads.is_empty() {
        return None;
    }
    let mid = spreads.len() / 2;
    Some(f32::from(*spreads.select_nth_unstable(mid).1))
}

/// Whether a file's stem, lowercased, names the raw of `stem`: the stem
/// alone, or with a separator and more after it.
fn names(file_stem: &str, stem: &str) -> bool {
    let stem = stem.to_lowercase();
    match file_stem.strip_prefix(&stem) {
        Some("") => true,
        Some(rest) => rest.starts_with(['-', '_', ' ']) && rest.len() > 1,
        None => false,
    }
}

/// The export step of `shot` that wrote a file at `written`: the
/// earliest stamped from [`STEP_BEFORE`] before the file to
/// [`SAME_EXPORT`] after it. Not the nearest: an image exported twice
/// half a minute apart has a step on each side of its second file,
/// and the earlier one held other settings.
fn step_for(shot: &Shot, written: f64) -> Option<&Step> {
    shot.exports
        .iter()
        .filter(|s| (-STEP_BEFORE..=SAME_EXPORT).contains(&(s.at - written)))
        .min_by(|a, b| a.at.total_cmp(&b.at))
}

/// One possible pairing of an image and a file: whether an export step
/// is at the file's time, which, and the settings it would be paired
/// with.
struct Edge {
    timed: bool,
    step: Option<String>,
    settings: Option<String>,
}

/// Pair the shots with the files. An edge is a file of a shot's name
/// whose shape is the shot's under the settings it would be paired
/// with; an edge at the time of one of the shot's export steps is
/// timed, and an image never developed has only those. A shot or a
/// file with a timed edge keeps only its timed ones. Files that color
/// and black and white images could both be are told apart by `spread`:
/// of two or more contested by the same images, the least colorful is
/// the black and white one's. Then a shot with one edge, to a file with
/// one edge, is a pair, again until none is left; what still has edges
/// is ambiguous.
fn pair_files(
    shots: Vec<Shot>,
    files: &[Exported],
    spread: &dyn Fn(&Path) -> Option<f32>,
) -> Pairing {
    let mut edges: BTreeMap<(usize, usize), Edge> = BTreeMap::new();
    let mut named: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut refused: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (s, shot) in shots.iter().enumerate() {
        for (f, file) in files.iter().enumerate() {
            if !names(&file.stem, shot.stem()) {
                continue;
            }
            let step = file.written.and_then(|t| step_for(shot, t));
            if step.is_none() && !shot.developed {
                refused.entry(s).or_default().push(f);
                continue;
            }
            named.entry(s).or_default().push(f);
            let edge = match step {
                Some(step) => Edge {
                    timed: true,
                    step: Some(step.name.clone()),
                    // A step whose settings will not read is still the
                    // export's time; the current settings stand in.
                    settings: step.settings.clone().or_else(|| shot.settings.clone()),
                },
                None => Edge {
                    timed: false,
                    step: None,
                    settings: shot.settings.clone(),
                },
            };
            let fits = match (
                edge.settings.as_deref().and_then(|t| shot.aspect(t)),
                file.aspect,
            ) {
                (Some(a), Some(b)) => (a - b).abs() <= ASPECT * a,
                _ => true,
            };
            if fits {
                edges.insert((s, f), edge);
            }
        }
    }
    // A timed edge rules out the untimed ones at either end.
    let timed_shots: BTreeSet<usize> = edges
        .iter()
        .filter(|(_, e)| e.timed)
        .map(|((s, _), _)| *s)
        .collect();
    let timed_files: BTreeSet<usize> = edges
        .iter()
        .filter(|(_, e)| e.timed)
        .map(|((_, f), _)| *f)
        .collect();
    edges.retain(|(s, f), e| e.timed || !(timed_shots.contains(s) || timed_files.contains(f)));
    // Files that a color image and a black and white one could both be
    // (a master and its mono copy, of one shape, exported in the same
    // second), grouped by the images contesting them.
    let mut by_file: BTreeMap<usize, Vec<(usize, bool)>> = BTreeMap::new();
    for ((s, f), e) in &edges {
        let mono = e.settings.as_deref().is_some_and(is_mono);
        by_file.entry(*f).or_default().push((*s, mono));
    }
    let mut contested: BTreeMap<Vec<usize>, Vec<usize>> = BTreeMap::new();
    for (f, ends) in &by_file {
        if ends.iter().any(|(_, m)| *m) && ends.iter().any(|(_, m)| !*m) {
            contested
                .entry(ends.iter().map(|(s, _)| *s).collect())
                .or_default()
                .push(*f);
        }
    }
    for fs in contested.into_values().filter(|fs| fs.len() > 1) {
        let Some(spreads) = fs
            .iter()
            .map(|&f| spread(&files[f].path))
            .collect::<Option<Vec<f32>>>()
        else {
            continue;
        };
        let least = spreads.iter().copied().fold(f32::INFINITY, f32::min);
        if spreads.iter().filter(|&&v| v == least).count() > 1 {
            continue;
        }
        for (&f, &v) in fs.iter().zip(&spreads) {
            let grey = v == least;
            for &(s, mono) in &by_file[&f] {
                if mono != grey {
                    edges.remove(&(s, f));
                }
            }
        }
    }

    let mut pairs = Vec::new();
    loop {
        let mut by_shot: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        let mut by_file: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for &(s, f) in edges.keys() {
            by_shot.entry(s).or_default().push(f);
            by_file.entry(f).or_default().push(s);
        }
        let sure: Vec<(usize, usize)> = by_shot
            .iter()
            .filter(|(_, fs)| fs.len() == 1)
            .map(|(&s, fs)| (s, fs[0]))
            .filter(|(_, f)| by_file[f].len() == 1)
            .collect();
        if sure.is_empty() {
            break;
        }
        for (s, f) in sure {
            let edge = edges.remove(&(s, f)).expect("an edge");
            edges.retain(|(s2, f2), _| *s2 != s && *f2 != f);
            let Some(settings) = edge.settings else {
                continue;
            };
            let confidence = if edge.timed {
                Confidence::History
            } else if shots[s].exports.is_empty() {
                Confidence::Name
            } else {
                Confidence::Elsewhere
            };
            pairs.push(Pair {
                shot: s,
                export: files[f].path.clone(),
                confidence,
                step: edge.step,
                settings,
            });
        }
    }
    pairs.sort_by_key(|p| p.shot);

    let paired_shots: BTreeSet<usize> = pairs.iter().map(|p| p.shot).collect();
    let paired_files: BTreeSet<&Path> = pairs.iter().map(|p| p.export.as_path()).collect();
    let mut ambiguous: BTreeMap<usize, Vec<PathBuf>> = BTreeMap::new();
    for &(s, f) in edges.keys() {
        ambiguous.entry(s).or_default().push(files[f].path.clone());
    }
    let in_doubt: BTreeSet<&Path> = ambiguous.values().flatten().map(PathBuf::as_path).collect();
    let mut misshapen = Vec::new();
    let mut undeveloped = Vec::new();
    let mut not_found = Vec::new();
    let mut never_exported = 0;
    let left = |fs: Option<&Vec<usize>>| -> Vec<PathBuf> {
        fs.into_iter()
            .flatten()
            .map(|&f| files[f].path.clone())
            .filter(|p| !paired_files.contains(p.as_path()))
            .collect()
    };
    for (s, shot) in shots.iter().enumerate() {
        if paired_shots.contains(&s) || ambiguous.contains_key(&s) {
            continue;
        }
        let of_name = left(named.get(&s));
        let refused_here = left(refused.get(&s));
        if !of_name.is_empty() {
            misshapen.push((s, of_name));
        } else if !refused_here.is_empty() {
            undeveloped.push((s, refused_here));
        } else if !shot.exports.is_empty() {
            not_found.push(s);
        } else {
            never_exported += 1;
        }
    }
    let unclaimed = files
        .iter()
        .map(|f| f.path.clone())
        .filter(|p| !paired_files.contains(p.as_path()) && !in_doubt.contains(p.as_path()))
        .filter(|p| !misshapen.iter().any(|(_, fs)| fs.contains(p)))
        .filter(|p| !undeveloped.iter().any(|(_, fs)| fs.contains(p)))
        .collect();
    Pairing {
        shots,
        pairs,
        not_found,
        misshapen,
        undeveloped,
        ambiguous: ambiguous.into_iter().collect(),
        unclaimed,
        never_exported,
        ..Pairing::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::tests::scratch;
    use crate::lightroom_catalog::tests::empty_catalog;
    use std::io::Write;

    /// Settings as the catalog keeps them, with an exposure and an
    /// optional crop.
    fn settings(exposure: f32, crop: Option<(f64, f64, f64, f64, f64)>) -> String {
        let crop = crop.map_or(String::new(), |(l, t, r, b, a)| {
            format!("CropLeft = {l},\nCropTop = {t},\nCropRight = {r},\nCropBottom = {b},\nCropAngle = {a},\n")
        });
        format!(
            "s = {{ CameraProfile = \"Camera Faithful\",\n{crop}Exposure2012 = {exposure},\n\
             WhiteBalance = \"As Shot\" }}\n"
        )
    }

    /// A history step's settings as the catalog keeps them: four bytes
    /// of length and the text deflated.
    fn deflated(text: &str) -> Vec<u8> {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(text.as_bytes()).unwrap();
        let mut out = (text.len() as u32).to_be_bytes().to_vec();
        out.extend(z.finish().unwrap());
        out
    }

    /// A grey JPEG of `w` by `h` at `path`, written at `when` (Unix
    /// seconds).
    fn jpeg(path: &Path, w: u32, h: u32, when: f64) {
        jpeg_of(path, w, h, when, [128, 128, 128]);
    }

    /// A JPEG of one color.
    fn jpeg_of(path: &Path, w: u32, h: u32, when: f64, rgb: [u8; 3]) {
        image::RgbImage::from_pixel(w, h, image::Rgb(rgb))
            .save(path)
            .unwrap();
        let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(when);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    /// A shoot of four raws, 600 by 400, beside an `Export` folder:
    ///
    /// - `IMG_0001`: exported at T0 uncropped; its copy "Square"
    ///   exported in the same second cropped square. The files are
    ///   `IMG_0001.jpg` (the square, written first) and
    ///   `IMG_0001-2.jpg` (the whole frame): the time cannot tell them
    ///   apart, the shape does.
    /// - `IMG_0002`: exported half a minute before T1 at exposure 0.7,
    ///   at T1 at exposure 1, then changed to 2; its file written just
    ///   before T1, so the step stamped after it is its export.
    /// - `IMG_0003`: no export in its history, and a file of its name
    ///   written much later: by name, its current settings.
    /// - `IMG_0004`: exported, no file. `IMG_0005`: never developed,
    ///   with a file of its name and no step at its time: not paired.
    /// - `IMG_0008`: exported at T0, its file written a day later:
    ///   by name, exported elsewhere.
    /// - `OTHER_9.jpg`: no image's. And `IMG_0006` with two files of
    ///   its shape and no history: ambiguous.
    /// - `IMG_0007` in color and its copy "Mono" in black and white,
    ///   one shape, exported in one second: `IMG_0007.jpg` is grey and
    ///   the copy's, `IMG_0007-2.jpg` in color and the master's.
    fn shoot(dir: &Path) -> (PathBuf, PathBuf, f64) {
        let catalog = dir.join("Shoot.lrcat");
        let raws = dir.join("raws");
        let export = raws.join("Export");
        std::fs::create_dir_all(&export).unwrap();
        for n in 1..=6 {
            std::fs::write(raws.join(format!("IMG_000{n}.CR3")), "not a raw").unwrap();
        }
        let db = empty_catalog(&catalog, "1504001");
        db.execute_batch(
            "ALTER TABLE Adobe_libraryImageDevelopHistoryStep ADD COLUMN dateCreated;
             ALTER TABLE Adobe_libraryImageDevelopHistoryStep ADD COLUMN text;
             ALTER TABLE Adobe_images ADD COLUMN fileWidth;
             ALTER TABLE Adobe_images ADD COLUMN fileHeight;
             INSERT INTO AgLibraryRootFolder VALUES (1, 'r', 'C:/Shoot/', 'Shoot', NULL);
             INSERT INTO AgLibraryFolder VALUES (2, 'f', NULL, '', 1);",
        )
        .unwrap();
        for n in 1..=6 {
            db.execute(
                "INSERT INTO AgLibraryFile VALUES (?1, 'x', ?2, 'CR3', 2)",
                rusqlite::params![10 + n, format!("IMG_000{n}")],
            )
            .unwrap();
            db.execute(
                "INSERT INTO Adobe_images (id_local, id_global, rootFile, rating, pick, \
                 fileWidth, fileHeight) VALUES (?1, 'i', ?2, 0, 0, 600, 400)",
                rusqlite::params![100 + n, 10 + n],
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO Adobe_images (id_local, id_global, rootFile, masterImage, copyName, \
             rating, pick, fileWidth, fileHeight) VALUES (107, 'c', 11, 101, 'Square', 0, 0, \
             600, 400)",
            [],
        )
        .unwrap();
        std::fs::write(raws.join("IMG_0007.CR3"), "not a raw").unwrap();
        std::fs::write(raws.join("IMG_0008.CR3"), "not a raw").unwrap();
        db.execute_batch(
            "INSERT INTO AgLibraryFile VALUES (18, 'x', 'IMG_0008', 'CR3', 2);
             INSERT INTO Adobe_images (id_local, id_global, rootFile, rating, pick, fileWidth,
                 fileHeight) VALUES (110, 'i', 18, 0, 0, 600, 400);
             INSERT INTO AgLibraryFile VALUES (17, 'x', 'IMG_0007', 'CR3', 2);
             INSERT INTO Adobe_images (id_local, id_global, rootFile, rating, pick, fileWidth,
                 fileHeight) VALUES (108, 'i', 17, 0, 0, 600, 400);
             INSERT INTO Adobe_images (id_local, id_global, rootFile, masterImage, copyName,
                 rating, pick, fileWidth, fileHeight) VALUES (109, 'm', 17, 108, 'Mono', 0, 0,
                 600, 400);",
        )
        .unwrap();
        let t0 = 800_000_000.0;
        let t1 = t0 + 3600.0;
        let square = settings(0.5, Some((1.0 / 6.0, 0.0, 5.0 / 6.0, 1.0, 0.0)));
        let develop = |image: i64, text: &str| {
            // Every frame developed but the fifth.
            let developed = if image == 105 { -1.0 } else { 1.0 };
            db.execute(
                "INSERT INTO Adobe_imageDevelopSettings (image, text, processVersion, \
                 hasDevelopAdjustmentsEx) VALUES (?1, ?2, '15.4', ?3)",
                rusqlite::params![image, text, developed],
            )
            .unwrap();
        };
        let step = |image: i64, at: f64, name: &str, text: &str| {
            db.execute(
                "INSERT INTO Adobe_libraryImageDevelopHistoryStep (image, name, dateCreated, \
                 text) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![image, name, at - EPOCH_2001, deflated(text)],
            )
            .unwrap();
        };
        develop(101, &settings(0.5, None));
        develop(107, &square);
        develop(102, &settings(2.0, None));
        develop(103, &settings(-1.0, None));
        develop(104, &settings(0.0, None));
        develop(105, &settings(0.0, None));
        develop(106, &settings(0.0, None));
        let mono =
            settings(0.0, None).replace("WhiteBalance", "ConvertToGrayscale = true,\nWhiteBalance");
        develop(108, &settings(0.0, None));
        develop(109, &mono);
        develop(110, &settings(0.0, None));
        for image in 101..=110 {
            step(
                image,
                t0 - 7200.0,
                "Import (1/1/26 9:00:00 AM)",
                &settings(0.0, None),
            );
        }
        step(
            101,
            t0,
            "Export - Hard Drive (1/1/26 12:00:00 PM)",
            &settings(0.5, None),
        );
        step(107, t0, "Export - Hard Drive (1/1/26 12:00:00 PM)", &square);
        step(
            102,
            t1,
            "Export - Hard Drive (1/1/26 1:00:00 PM)",
            &settings(1.0, None),
        );
        step(
            102,
            t1 - 28.0,
            "Export - Hard Drive (1/1/26 12:59:32 PM)",
            &settings(0.7, None),
        );
        step(102, t1 + 60.0, "Exposure", &settings(2.0, None));
        step(
            110,
            t0,
            "Export - Hard Drive (1/1/26 12:00:00 PM)",
            &settings(0.0, None),
        );
        jpeg(&export.join("IMG_0005.jpg"), 300, 200, t1 + 86400.0);
        jpeg(&export.join("IMG_0008.jpg"), 300, 200, t1 + 86400.0);
        step(
            104,
            t1,
            "Export - Hard Drive (1/1/26 1:00:00 PM)",
            &settings(0.0, None),
        );
        jpeg(&export.join("IMG_0001.jpg"), 300, 300, t0 + 1.0);
        jpeg(&export.join("IMG_0001-2.jpg"), 300, 200, t0 + 2.0);
        jpeg(&export.join("IMG_0002.jpg"), 300, 200, t1 - 0.4);
        jpeg(&export.join("IMG_0003.jpg"), 300, 200, t1 + 86400.0);
        jpeg(&export.join("IMG_0006.jpg"), 300, 200, t1 + 86400.0);
        jpeg(&export.join("IMG_0006_bw.jpg"), 300, 200, t1 + 86400.0);
        jpeg(&export.join("OTHER_9.jpg"), 300, 200, t1);
        step(
            108,
            t1,
            "Export - Hard Drive (1/1/26 1:00:00 PM)",
            &settings(0.0, None),
        );
        step(109, t1, "Export - Hard Drive (1/1/26 1:00:00 PM)", &mono);
        jpeg(&export.join("IMG_0007.jpg"), 300, 200, t1 + 1.0);
        jpeg_of(
            &export.join("IMG_0007-2.jpg"),
            300,
            200,
            t1 + 1.0,
            [170, 120, 80],
        );
        (catalog, raws, t0)
    }

    #[test]
    fn exports_pair_by_history_then_by_name_and_shape() {
        let dir = scratch("lr-exports");
        let (catalog, raws, _) = shoot(&dir);
        let options = Options {
            roots: vec![("C:/Shoot".into(), raws.clone())],
            folders: Vec::new(),
        };
        let p = pair(&catalog, &options).unwrap();
        let export = raws.join("Export");
        assert_eq!(p.folders, vec![(export.clone(), 11)]);
        let of = |image: i64| p.shots.iter().position(|s| s.image == image).unwrap();
        let paired: Vec<(i64, String, Confidence)> = p
            .pairs
            .iter()
            .map(|x| {
                let name = x.export.file_name().unwrap().to_string_lossy().into_owned();
                (p.shots[x.shot].image, name, x.confidence)
            })
            .collect();
        assert_eq!(
            paired,
            vec![
                (101, "IMG_0001-2.jpg".into(), Confidence::History),
                (102, "IMG_0002.jpg".into(), Confidence::History),
                (103, "IMG_0003.jpg".into(), Confidence::Name),
                (107, "IMG_0001.jpg".into(), Confidence::History),
                // The color master and its mono copy, by their pixels.
                (108, "IMG_0007-2.jpg".into(), Confidence::History),
                (109, "IMG_0007.jpg".into(), Confidence::History),
                (110, "IMG_0008.jpg".into(), Confidence::Elsewhere),
            ]
        );
        // The export of the second frame is the settings of the step
        // stamped after its file, not the earlier export's or the ones
        // changed after it.
        let second = p
            .pairs
            .iter()
            .find(|x| p.shots[x.shot].image == 102)
            .unwrap();
        assert!(
            second.settings.contains("Exposure2012 = 1"),
            "{}",
            second.settings
        );
        // The third frame's by name: its current settings.
        let third = p
            .pairs
            .iter()
            .find(|x| p.shots[x.shot].image == 103)
            .unwrap();
        assert!(
            third.settings.contains("Exposure2012 = -1"),
            "{}",
            third.settings
        );
        assert_eq!(p.not_found, vec![of(104)]);
        assert_eq!(
            p.ambiguous,
            vec![(
                of(106),
                vec![export.join("IMG_0006.jpg"), export.join("IMG_0006_bw.jpg")]
            )]
        );
        assert_eq!(p.unclaimed, vec![export.join("OTHER_9.jpg")]);
        assert_eq!(
            p.undeveloped,
            vec![(of(105), vec![export.join("IMG_0005.jpg")])]
        );
        assert_eq!(p.never_exported, 0);
        assert!(p.misshapen.is_empty());
        assert!(second.step.as_deref().unwrap().contains("1:00:00 PM"));
        // The copy keeps its name; the first step is what it arrived with.
        assert_eq!(p.shots[of(107)].copy.as_deref(), Some("Square"));
        assert!(
            p.shots[of(101)]
                .imported
                .as_deref()
                .unwrap()
                .contains("Exposure2012 = 0")
        );
        assert!(p.shots[of(101)].raw.as_ref().unwrap().is_file());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_of_the_name_and_another_shape_is_not_paired() {
        let dir = scratch("lr-exports-shape");
        let (catalog, raws, t0) = shoot(&dir);
        // The whole frame's file swapped for a square one: the copy
        // still has its square, and the master has none of its shape.
        let export = raws.join("Export");
        jpeg(&export.join("IMG_0001-2.jpg"), 300, 300, t0 + 2.0);
        let options = Options {
            roots: vec![("Shoot".into(), raws.clone())],
            folders: Vec::new(),
        };
        let p = pair(&catalog, &options).unwrap();
        let master = p.shots.iter().position(|s| s.image == 101).unwrap();
        let copy = p.shots.iter().position(|s| s.image == 107).unwrap();
        assert!(!p.pairs.iter().any(|x| x.shot == master || x.shot == copy));
        // Both square files fit the copy alone, so it is in doubt; the
        // master has files of its name and none of its shape.
        assert_eq!(
            p.ambiguous
                .iter()
                .find(|(s, _)| *s == copy)
                .unwrap()
                .1
                .len(),
            2
        );
        assert_eq!(
            p.misshapen,
            vec![(
                master,
                vec![export.join("IMG_0001-2.jpg"), export.join("IMG_0001.jpg")]
            )]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_turned_crop_has_the_size_the_catalog_says() {
        // The catalog's own cropped sizes beside its settings, to the
        // pixel: a 8192 by 5464 frame at -0.24 degrees, and a 6000 by
        // 4000 frame at 1.1.
        let crop = Crop {
            left: 0.014077,
            top: 0.006163,
            right: 0.998233,
            bottom: 0.981413,
            angle: -0.24,
        };
        let (w, h) = crop.size(8192.0, 5464.0);
        assert_eq!((w.round(), h.round()), (8040.0, 5362.0));
        let crop = Crop {
            left: 0.066054,
            top: 1e-6,
            right: 0.920273,
            bottom: 1.0,
            angle: 1.1,
        };
        let (w, h) = crop.size(6000.0, 4000.0);
        assert_eq!((w.round(), h.round()), (5201.0, 3901.0));
    }

    #[test]
    fn a_crop_is_read_only_where_the_settings_crop() {
        let w = |text: &str| words(text);
        assert_eq!(Crop::of(&w(&settings(0.0, None))), None);
        let c = Crop::of(&w("s = { CropLeft = 0.1, CropAngle = 2 }")).unwrap();
        assert_eq!(
            (c.left, c.top, c.right, c.bottom, c.angle),
            (0.1, 0.0, 1.0, 1.0, 2.0)
        );
        assert_eq!(
            Crop::of(&w("s = { HasCrop = false, CropLeft = 0.1 }")),
            None
        );
        assert_eq!(Crop::of(&w("s = { CropLeft = 0, CropRight = 1 }")), None);
    }

    #[test]
    fn a_file_names_a_raw_by_its_stem_and_a_separated_suffix() {
        assert!(names("img_0001", "IMG_0001"));
        assert!(names("img_0001-2", "IMG_0001"));
        assert!(names("img_0001_bw", "IMG_0001"));
        assert!(!names("img_00012", "IMG_0001"));
        assert!(!names("img_0001-", "IMG_0001"));
        assert!(!names("img_000", "IMG_0001"));
    }
}
