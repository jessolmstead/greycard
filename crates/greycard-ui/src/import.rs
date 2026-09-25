//! Import from a card: every frame copied to the destination under
//! its new name, verified by a hash of the whole file, then copied
//! again to a backup folder if one is asked for, and given a preset.
//!
//! Nothing here is written on the card or deleted from it. A file is
//! written under a temporary name in its own destination folder and
//! renamed once its copy reads back the same as the card's did, so a
//! stop, a full disk or a dying card never leaves a half-written file
//! under a real name; the temporary is in the destination folder and
//! not a temp directory because a rename across drives is a copy
//! (and on Windows an error). A name that is there already is never
//! written over.
//!
//! No window here: the sheet (`panel::import`) and `--import` both
//! run it on a thread of their own and hear its progress through a
//! callback.

use crate::naming::{self, Day, Fields};
use anyhow::{Context, Result, bail};
use greycard_edit::Preset;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// What an import is asked to do.
#[derive(Debug, Clone)]
pub struct Options {
    /// The card's folder (its `DCIM`, or anything under it), walked
    /// whole.
    pub source: PathBuf,
    pub destination: PathBuf,
    /// The folders under the destination, a [`naming`] pattern;
    /// empty for the destination itself.
    pub subfolder: String,
    /// The file's name before its extension, a [`naming`] pattern.
    pub name: String,
    /// Laid over each frame as one history step, "Preset: X".
    pub preset: Option<Preset>,
    /// A second copy of every file, under the same folders and names.
    pub backup: Option<PathBuf>,
    /// The library index to ask whether a file is imported already.
    pub library: Option<PathBuf>,
}

/// A frame on the card and the files that go with it: a raw's XMP
/// and the camera's JPEG of the same name.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub file: PathBuf,
    pub companions: Vec<PathBuf>,
}

/// What a card holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
    pub items: Vec<Item>,
    /// Files left on the card: ones the editor does not open (a
    /// video, the camera's own catalog) and XMPs with no frame.
    pub left: Vec<PathBuf>,
}

impl Scan {
    /// Every file that would be copied, companions included.
    pub fn files(&self) -> usize {
        self.items.iter().map(|i| 1 + i.companions.len()).sum()
    }
}

fn is_frame(p: &Path) -> bool {
    greycard_core::decode::is_raw_path(p) || greycard_core::picture::is_picture_path(p)
}

fn extension_is(p: &Path, want: &str) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(want))
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn stem_of(p: &Path) -> String {
    p.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Every file under `dir`, walked depth first, hidden files and
/// folders left out (the Mac's `._` shadows, a `.greycard` folder).
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(&path, out)?,
            Ok(t) if t.is_file() => out.push(path),
            // A link is followed as what it points at.
            Ok(t) if t.is_symlink() => {
                if path.is_dir() {
                    walk(&path, out)?;
                } else if path.is_file() {
                    out.push(path);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The frames under `source`, in the order the camera numbered them
/// (by path: `100CANON` before `101CANON`, `IMG_0001` before
/// `IMG_0002`), each raw with its companions. A picture with a raw of
/// the same name in its folder is the camera's JPEG of that raw and
/// goes along with it; a picture with no raw is a frame of its own,
/// as a JPEG-only camera writes them. An XMP goes with the frame it
/// names, `IMG.xmp` or `IMG.CR3.xmp`, and is left when there is none.
pub fn scan(source: &Path) -> Result<Scan> {
    anyhow::ensure!(source.is_dir(), "{} is not a folder", source.display());
    let mut files = Vec::new();
    walk(source, &mut files).with_context(|| format!("reading {}", source.display()))?;
    files.sort();
    let key = |p: &Path| (p.parent().map(Path::to_path_buf), stem_of(p).to_lowercase());
    // The raws by folder and stem: they take the companions.
    let mut raws: HashMap<(Option<PathBuf>, String), usize> = HashMap::new();
    let mut items: Vec<Item> = Vec::new();
    for f in files
        .iter()
        .filter(|f| greycard_core::decode::is_raw_path(f))
    {
        raws.entry(key(f)).or_insert(items.len());
        items.push(Item {
            file: f.clone(),
            companions: Vec::new(),
        });
    }
    let mut left = Vec::new();
    for f in &files {
        if greycard_core::decode::is_raw_path(f) {
            continue;
        }
        let picture = greycard_core::picture::is_picture_path(f);
        let xmp = extension_is(f, "xmp");
        // `IMG.CR3.xmp` names its frame by the whole name.
        let owner = raws.get(&key(f)).copied().or_else(|| {
            xmp.then(|| {
                let inner = f.with_extension("");
                raws.get(&key(&inner)).copied()
            })
            .flatten()
        });
        match owner {
            Some(i) if picture || xmp => items[i].companions.push(f.clone()),
            None if picture => items.push(Item {
                file: f.clone(),
                companions: Vec::new(),
            }),
            _ => left.push(f.clone()),
        }
    }
    // Lone pictures joined the list after the raws; the camera's order
    // is the path's.
    items.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(Scan { items, left })
}

/// The tokens' values for `file`, the `seq`th frame of the run: the
/// day and the camera from its EXIF, the day from its modification
/// time when the EXIF has none. A file the probe cannot read (or that
/// makes the decoder give up) is named from its modification time and
/// no camera, and still imported.
pub fn fields_of(file: &Path, seq: usize) -> Fields {
    let probed = std::panic::catch_unwind(|| {
        if greycard_core::picture::is_picture_path(file) {
            greycard_core::picture::probe_path(file)
        } else {
            greycard_core::decode::probe_path(file)
        }
    });
    let (taken, camera) = match probed {
        Ok(Ok(p)) => (
            p.taken.as_deref().and_then(Day::from_exif),
            greycard_core::raw::camera_name(&p.make, &p.model),
        ),
        _ => (None, String::new()),
    };
    let day = taken.unwrap_or_else(|| modified_day(file));
    Fields {
        day,
        name: stem_of(file),
        camera,
        seq,
    }
}

/// The local day a file was last written: what a card's clock stamped
/// it with, near enough, when the EXIF says nothing.
fn modified_day(file: &Path) -> Day {
    use chrono::Datelike;
    let when = std::fs::metadata(file)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| std::time::SystemTime::now());
    let local: chrono::DateTime<chrono::Local> = when.into();
    Day {
        year: local.year(),
        month: local.month(),
        day: local.day(),
    }
}

/// Where a frame goes, relative to the destination: its folders and
/// its name. The folder part is the backup's as well.
pub fn relative(opts: &Options, file: &Path, fields: &Fields) -> Result<PathBuf> {
    let folders = naming::folders(&opts.subfolder, fields)?;
    let extension = file
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = naming::file_name(&opts.name, fields, &extension)?;
    let mut rel = PathBuf::new();
    for f in folders {
        rel.push(f);
    }
    rel.push(name);
    Ok(rel)
}

/// What a companion is called beside its frame's new name: the new
/// stem and whatever followed the old stem, so `IMG_0001.JPG` beside
/// `IMG_0001.CR3` renamed `2026-0001.CR3` is `2026-0001.JPG`, and
/// `IMG_0001.CR3.xmp` is `2026-0001.CR3.xmp`.
pub fn companion_name(frame: &Path, new_frame: &Path, companion: &Path) -> String {
    let old_stem = stem_of(frame);
    let new_stem = stem_of(new_frame);
    let name = name_of(companion);
    let tail = name.get(old_stem.len()..).unwrap_or_default();
    naming::safe(&format!("{new_stem}{tail}"))
}

/// Stop after the frame in hand.
#[derive(Debug, Default)]
pub struct Job {
    canceled: AtomicBool,
}

impl Job {
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::SeqCst);
    }
    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }
}

/// What an import came to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// Frames on the card.
    pub total: usize,
    /// Frames copied and verified.
    pub frames: usize,
    /// Files copied and verified, companions included; and their bytes.
    pub files: usize,
    pub bytes: u64,
    /// Files copied to the backup and verified there.
    pub backed_up: usize,
    /// Frames already imported: the same bytes under the destination,
    /// or a file the library holds with the same content hash.
    pub already: usize,
    /// Frames not imported because their name was taken by another
    /// file, which is never written over.
    pub taken: usize,
    /// Frames given the preset.
    pub preset: usize,
    /// The stop was asked for before the last frame.
    pub canceled: bool,
    /// The file that stopped the import, and why.
    pub error: Option<(PathBuf, String)>,
    pub seconds: f64,
    /// The folders written to and how many frames each took, in the
    /// order they were first written.
    pub folders: Vec<(PathBuf, usize)>,
}

impl Report {
    /// The folder that took the most frames: the one to open.
    pub fn main_folder(&self) -> Option<&Path> {
        let most = self.folders.iter().map(|(_, n)| *n).max()?;
        self.folders
            .iter()
            .find(|(_, n)| *n == most)
            .map(|(f, _)| f.as_path())
    }

    fn landed_in(&mut self, folder: &Path) {
        match self.folders.iter_mut().find(|(f, _)| f == folder) {
            Some((_, n)) => *n += 1,
            None => self.folders.push((folder.to_path_buf(), 1)),
        }
    }

    /// Megabytes a second read off the card and written.
    pub fn rate(&self) -> f64 {
        if self.seconds > 0.0 {
            self.bytes as f64 / 1e6 / self.seconds
        } else {
            0.0
        }
    }

    /// The status line's words once it is done.
    pub fn line(&self, destination: &Path) -> String {
        let mut s = if self.canceled {
            format!(
                "import stopped: {} of {} imported",
                self.frames,
                frames(self.total)
            )
        } else if self.error.is_some() {
            format!(
                "import failed after {} of {}",
                self.frames,
                frames(self.total)
            )
        } else {
            format!("imported {}", frames(self.frames))
        };
        s.push_str(&format!(" to {}", destination.display()));
        if self.bytes > 0 {
            s.push_str(&format!(
                " ({:.0} MB at {:.0} MB/s)",
                self.bytes as f64 / 1e6,
                self.rate()
            ));
        }
        if self.backed_up > 0 {
            s.push_str(&format!(", {} backed up", self.backed_up));
        }
        if self.already > 0 {
            s.push_str(&format!(", {} already there", self.already));
        }
        if self.taken > 0 {
            s.push_str(&format!(
                ", {} skipped (the name is another file's)",
                self.taken
            ));
        }
        if let Some((file, why)) = &self.error {
            s.push_str(&format!(": {}: {why}", name_of(file)));
        }
        s
    }
}

/// "1 frame", "3 frames".
pub fn frames(n: usize) -> String {
    format!("{n} frame{}", if n == 1 { "" } else { "s" })
}

/// The status line while frame `index` (from zero) is copied.
pub fn progress_line(index: usize, total: usize, name: &str) -> String {
    format!("importing {} of {total}: {name}", index + 1)
}

const CHUNK: usize = 1 << 20;

/// BLAKE3 over every byte of a file. The index's hash is the head and
/// the size, which is enough to know a file again but cannot see a
/// copy whose tail never landed; the verify has to read it all.
pub fn hash_whole(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    hash_reader(&mut file).map(|(h, _)| h)
}

fn hash_reader(r: &mut dyn Read) -> std::io::Result<(String, u64)> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    let mut n = 0u64;
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => {
                hasher.update(&buf[..k]);
                n += k as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok((hasher.finalize().to_hex().to_string(), n))
}

/// The temporary name a file is written under beside where it goes:
/// hidden on Linux and macOS, and never a name a camera writes.
pub fn temporary(to: &Path) -> PathBuf {
    let name = name_of(to);
    to.with_file_name(format!(".{name}.greycard-import"))
}

/// Copy `from` to `to` through [`temporary`], and rename it only once
/// it reads back with the hash its bytes had going in; that hash is
/// returned with the byte count. `want` is the hash the bytes must
/// have (a backup is made from the destination's copy and checked
/// against the card's hash). The file keeps `from`'s modification
/// time. On any failure the temporary is gone and `to` untouched.
pub fn land(from: &Path, to: &Path, want: Option<&str>) -> Result<(String, u64)> {
    let mut src = std::fs::File::open(from).with_context(|| "reading it".to_string())?;
    let modified = src.metadata().and_then(|m| m.modified()).ok();
    land_from(&mut src, modified, to, want)
}

/// [`land`] from any reader, so a read that fails halfway is tested.
pub fn land_from(
    src: &mut dyn Read,
    modified: Option<std::time::SystemTime>,
    to: &Path,
    want: Option<&str>,
) -> Result<(String, u64)> {
    let folder = to.parent().context("no folder to write in")?;
    std::fs::create_dir_all(folder).with_context(|| format!("making {}", folder.display()))?;
    let tmp = temporary(to);
    let result = (|| -> Result<(String, u64)> {
        let mut out = std::fs::File::create(&tmp).context("writing the copy")?;
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; CHUNK];
        let mut bytes = 0u64;
        loop {
            let k = match src.read(&mut buf) {
                Ok(0) => break,
                Ok(k) => k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(anyhow::Error::new(e).context("reading it")),
            };
            hasher.update(&buf[..k]);
            out.write_all(&buf[..k]).context("writing the copy")?;
            bytes += k as u64;
        }
        if let Some(when) = modified {
            // A name's date from the file's time wants the card's.
            let _ = out.set_modified(when);
        }
        out.sync_all().context("writing the copy")?;
        drop(out);
        let hash = hasher.finalize().to_hex().to_string();
        if let Some(want) = want
            && want != hash
        {
            bail!("the copy it was made from no longer matches the card");
        }
        let back = hash_whole(&tmp).context("reading the copy back")?;
        if back != hash {
            bail!("the copy does not read back the same");
        }
        // Checked as late as it can be: a file of this name that
        // appeared meanwhile is not written over.
        if to.exists() {
            bail!("{} appeared while it was copied", to.display());
        }
        std::fs::rename(&tmp, to).context("naming the copy")?;
        Ok((hash, bytes))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// What is under the destination already, by size, so a frame is only
/// hashed against the files that could be it; each hashed at most once.
struct Existing {
    by_size: HashMap<u64, Vec<PathBuf>>,
    hashes: HashMap<PathBuf, String>,
}

impl Existing {
    fn under(root: &Path) -> Self {
        let mut files = Vec::new();
        if root.is_dir()
            && let Err(e) = walk(root, &mut files)
        {
            tracing::warn!("import: {}: {e}", root.display());
        }
        let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
        for f in files {
            if let Ok(m) = std::fs::metadata(&f) {
                by_size.entry(m.len()).or_default().push(f);
            }
        }
        Self {
            by_size,
            hashes: HashMap::new(),
        }
    }

    /// A file here with these bytes, if any.
    fn holds(&mut self, size: u64, hash: &str) -> Option<PathBuf> {
        for f in self.by_size.get(&size).cloned().unwrap_or_default() {
            let h = match self.hashes.get(&f) {
                Some(h) => h.clone(),
                None => match hash_whole(&f) {
                    Ok(h) => {
                        self.hashes.insert(f.clone(), h.clone());
                        h
                    }
                    Err(_) => continue,
                },
            };
            if h == hash {
                return Some(f);
            }
        }
        None
    }

    fn add(&mut self, path: PathBuf, size: u64, hash: String) {
        self.by_size.entry(size).or_default().push(path.clone());
        self.hashes.insert(path, hash);
    }
}

/// Whether `path` is `root` or under it, compared canonically.
fn is_under(path: &Path, root: &Path) -> bool {
    let canon = |p: &Path| dunce::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(path).starts_with(canon(root))
}

/// Refuse what would write on the card or into itself.
pub fn check(opts: &Options) -> Result<()> {
    anyhow::ensure!(
        opts.source.is_dir(),
        "the source {} is not a folder",
        opts.source.display()
    );
    anyhow::ensure!(
        !opts.destination.as_os_str().is_empty(),
        "no destination is chosen"
    );
    anyhow::ensure!(
        !is_under(&opts.destination, &opts.source),
        "the destination is on the card, under {}",
        opts.source.display()
    );
    if let Some(b) = &opts.backup {
        anyhow::ensure!(
            !is_under(b, &opts.source),
            "the backup is on the card, under {}",
            opts.source.display()
        );
        anyhow::ensure!(
            !is_under(b, &opts.destination) && !is_under(&opts.destination, b),
            "the backup is the destination, or inside it"
        );
    }
    naming::folders(&opts.subfolder, &sample())?;
    naming::file_name(&opts.name, &sample(), "CR3")?;
    Ok(())
}

fn sample() -> Fields {
    Fields {
        day: Day {
            year: 2026,
            month: 1,
            day: 1,
        },
        name: "IMG_0001".into(),
        camera: String::new(),
        seq: 1,
    }
}

/// Run the import over `scan`'s frames. `progress` hears each frame as
/// it begins, by its index and name. Stops at the first error, which
/// the report names; a cancel takes effect before the next frame.
pub fn run(
    opts: &Options,
    scan: &Scan,
    job: &Job,
    mut progress: impl FnMut(usize, usize, &str),
) -> Report {
    let started = std::time::Instant::now();
    let total = scan.items.len();
    let mut report = Report {
        total,
        ..Report::default()
    };
    let looked = std::time::Instant::now();
    let mut existing = Existing::under(&opts.destination);
    tracing::info!(
        "import: {} files under {} looked over in {:.2} s",
        existing.by_size.values().map(Vec::len).sum::<usize>(),
        opts.destination.display(),
        looked.elapsed().as_secs_f64()
    );
    let library = opts
        .library
        .as_deref()
        .filter(|p| p.exists())
        .and_then(|p| {
            greycard_library::Library::open_read_only(p)
                .map_err(|e| tracing::warn!("import: the library at {}: {e}", p.display()))
                .ok()
        });
    // The names this run has given, case-blind as two of the three
    // platforms' file systems are, so two frames never meet.
    let mut given: HashSet<String> = HashSet::new();
    for (i, item) in scan.items.iter().enumerate() {
        if job.is_canceled() {
            report.canceled = true;
            break;
        }
        progress(i, total, &name_of(&item.file));
        if let Err(e) = import_one(
            opts,
            item,
            i + 1,
            &mut existing,
            library.as_ref(),
            &mut given,
            &mut report,
        ) {
            report.error = Some((item.file.clone(), format!("{e:#}")));
            break;
        }
    }
    report.seconds = started.elapsed().as_secs_f64();
    report
}

/// A path's name made unique within the run: ` (2)` before the
/// extension, as an export's set does.
fn unique(path: PathBuf, given: &mut HashSet<String>) -> PathBuf {
    let key = |p: &Path| p.to_string_lossy().to_lowercase();
    let mut out = path.clone();
    let mut n = 2;
    while given.contains(&key(&out)) {
        let stem = stem_of(&path);
        let name = match path.extension() {
            Some(e) => format!("{stem} ({n}).{}", e.to_string_lossy()),
            None => format!("{stem} ({n})"),
        };
        out = path.with_file_name(name);
        n += 1;
    }
    given.insert(key(&out));
    out
}

fn import_one(
    opts: &Options,
    item: &Item,
    seq: usize,
    existing: &mut Existing,
    library: Option<&greycard_library::Library>,
    given: &mut HashSet<String>,
    report: &mut Report,
) -> Result<()> {
    let fields = fields_of(&item.file, seq);
    let rel = relative(opts, &item.file, &fields)?;
    let to = unique(opts.destination.join(&rel), given);
    let size = std::fs::metadata(&item.file).context("reading it")?.len();

    // Imported before: the library knows the file by its head and
    // size, somewhere that is not the card itself.
    if let Some(lib) = library
        && let Ok(head) = greycard_library::hash_file(&item.file)
        && let Ok(rows) = lib.by_hash(&head)
        && let Some(row) = rows.iter().find(|r| {
            !r.missing && r.size == size && r.path.is_file() && !is_under(&r.path, &opts.source)
        })
    {
        tracing::info!(
            "import: {} is there already as {} (the library)",
            item.file.display(),
            row.path.display()
        );
        report.already += 1;
        return Ok(());
    }
    // Or under the destination, byte for byte, under any name.
    let whole = if existing.by_size.contains_key(&size) {
        Some(hash_whole(&item.file).context("reading it")?)
    } else {
        None
    };
    if let Some(hash) = &whole
        && let Some(there) = existing.holds(size, hash)
    {
        tracing::info!(
            "import: {} is there already as {}",
            item.file.display(),
            there.display()
        );
        report.already += 1;
        return Ok(());
    }
    if to.exists() {
        tracing::warn!(
            "import: {} not imported: {} is another file",
            item.file.display(),
            to.display()
        );
        report.taken += 1;
        return Ok(());
    }

    let (hash, bytes) = land(&item.file, &to, None)?;
    if let Some(before) = &whole
        && *before != hash
    {
        // Read twice and different: the card is not giving the same
        // bytes back.
        let _ = std::fs::remove_file(&to);
        bail!("the card read differently twice");
    }
    existing.add(to.clone(), bytes, hash.clone());
    report.files += 1;
    report.bytes += bytes;
    let mut landed = vec![(to.clone(), hash)];

    for companion in &item.companions {
        let name = companion_name(&item.file, &to, companion);
        let c_to = unique(to.with_file_name(name), given);
        if c_to.exists() {
            tracing::warn!(
                "import: {} not copied: {} is there already",
                companion.display(),
                c_to.display()
            );
            continue;
        }
        let (h, b) = land(companion, &c_to, None)
            .with_context(|| format!("{} (with {})", name_of(companion), name_of(&item.file)))?;
        existing.add(c_to.clone(), b, h.clone());
        report.files += 1;
        report.bytes += b;
        landed.push((c_to, h));
    }

    if let Some(backup) = &opts.backup {
        for (file, hash) in &landed {
            let rel = file.strip_prefix(&opts.destination).unwrap_or(file);
            let b_to = backup.join(rel);
            if b_to.exists() {
                if hash_whole(&b_to).ok().as_deref() == Some(hash.as_str()) {
                    continue;
                }
                bail!(
                    "the backup's {} is another file, and is not written over",
                    b_to.display()
                );
            }
            land(file, &b_to, Some(hash)).context("the backup copy")?;
            report.backed_up += 1;
        }
    }

    if let Some(preset) = &opts.preset {
        let mut sidecar = greycard_edit::Sidecar::load(&to)
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut seed = crate::files::never_developed(&sidecar);
        crate::panel::startup::preset_at_start(
            &mut sidecar,
            &mut seed,
            &to,
            preset,
            Some(greycard_edit::Placement::Folder),
        );
        if sidecar.current_label.is_some() {
            report.preset += 1;
        }
    }

    report.frames += 1;
    if let Some(folder) = to.parent() {
        report.landed_in(folder);
    }
    Ok(())
}

/// The folders a mounted volume may be under, as each desktop mounts
/// them. On Windows each drive is a volume of its own.
pub fn volumes() -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        // A and B are floppies, and asking one that is not there can
        // wait on the drive.
        for letter in b'C'..=b'Z' {
            out.push(PathBuf::from(format!("{}:\\", letter as char)));
        }
    }
    #[cfg(not(windows))]
    {
        let mut parents = Vec::new();
        if cfg!(target_os = "macos") {
            parents.push(PathBuf::from("/Volumes"));
        } else {
            if let Some(user) = std::env::var_os("USER") {
                parents.push(Path::new("/run/media").join(&user));
                parents.push(Path::new("/media").join(&user));
            }
            parents.push(PathBuf::from("/media"));
        }
        for parent in parents {
            out.extend(volumes_under(&parent));
        }
    }
    out
}

/// The folders in `parent`, sorted: the volumes mounted there.
pub fn volumes_under(parent: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(parent)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// The first of `volumes` with a camera's `DCIM` folder on it, that
/// folder. Each check is a stat or two, but a stat of a sleeping or
/// network drive can wait: never call this on the window's thread.
pub fn find_card(volumes: &[PathBuf]) -> Option<PathBuf> {
    volumes.iter().find_map(|v| {
        let dcim = v.join("DCIM");
        dcim.is_dir().then_some(dcim)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of this test's own under the temporary directory,
    /// emptied first.
    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("greycard-import-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn options(card: &Path, dest: &Path) -> Options {
        Options {
            source: card.to_path_buf(),
            destination: dest.to_path_buf(),
            subfolder: String::new(),
            name: "{name}".into(),
            preset: None,
            backup: None,
            library: None,
        }
    }

    /// No file in `dir` or under it has the temporary's name.
    fn no_temporaries(dir: &Path) {
        let mut files = Vec::new();
        // The walk leaves hidden files out, so look for them by hand.
        fn all(d: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    all(&p, out);
                } else {
                    out.push(p);
                }
            }
        }
        all(dir, &mut files);
        let tmp: Vec<_> = files
            .iter()
            .filter(|f| name_of(f).ends_with(".greycard-import"))
            .collect();
        assert!(tmp.is_empty(), "{tmp:?}");
    }

    /// A raw takes its JPEG and its XMP, a picture alone is a frame, a
    /// video and an XMP with no frame are left, and hidden files are
    /// not seen. Nothing here needs to be a real raw: the scan goes by
    /// the names.
    #[test]
    fn a_card_is_read_into_frames_and_companions() {
        let card = dir("scan").join("DCIM");
        let a = card.join("100CANON");
        let b = card.join("101CANON");
        for (f, body) in [
            (a.join("IMG_0001.CR3"), b"raw1".as_slice()),
            (a.join("IMG_0001.JPG"), b"jpg1"),
            (a.join("IMG_0001.CR3.xmp"), b"<x/>"),
            (a.join("IMG_0002.JPG"), b"jpg2"),
            (a.join("MVI_0003.MP4"), b"mov"),
            (a.join("IMG_0004.xmp"), b"<x/>"),
            (a.join("._IMG_0001.CR3"), b"mac"),
            (b.join("IMG_0001.CR3"), b"raw1b"),
            (b.join("IMG_0001.xmp"), b"<x/>"),
        ] {
            write(&f, body);
        }
        let s = scan(&card).unwrap();
        assert_eq!(
            s.items,
            vec![
                Item {
                    file: a.join("IMG_0001.CR3"),
                    companions: vec![a.join("IMG_0001.CR3.xmp"), a.join("IMG_0001.JPG")],
                },
                Item {
                    file: a.join("IMG_0002.JPG"),
                    companions: vec![],
                },
                Item {
                    file: b.join("IMG_0001.CR3"),
                    companions: vec![b.join("IMG_0001.xmp")],
                },
            ]
        );
        assert_eq!(s.left, vec![a.join("IMG_0004.xmp"), a.join("MVI_0003.MP4")]);
        assert_eq!(s.files(), 6);
        assert_eq!(
            companion_name(
                &a.join("IMG_0001.CR3"),
                Path::new("x/2026-0001.CR3"),
                &a.join("IMG_0001.CR3.xmp")
            ),
            "2026-0001.CR3.xmp"
        );
    }

    /// Every frame lands under its name, the companions beside it
    /// under the same new name, each verified, the card untouched, and
    /// a backup made of each; the same card again is all "already
    /// there", even under another name.
    #[test]
    fn an_import_copies_verifies_and_backs_up_and_a_second_is_already_there() {
        let root = dir("copy");
        let card = root.join("card/DCIM/100CANON");
        write(&card.join("IMG_0001.CR3"), &vec![7u8; 3 * CHUNK + 5]);
        write(&card.join("IMG_0001.JPG"), b"camera jpeg");
        write(&card.join("IMG_0002.CR3"), &vec![9u8; 1000]);
        let card_before: Vec<_> = std::fs::read_dir(&card)
            .unwrap()
            .map(|e| {
                let p = e.unwrap().path();
                (p.clone(), std::fs::read(&p).unwrap())
            })
            .collect();
        let dest = root.join("photos");
        let backup = root.join("backup");
        let mut o = options(&root.join("card"), &dest);
        o.name = "shoot-{seq}".into();
        o.subfolder = "set".into();
        o.backup = Some(backup.clone());
        check(&o).unwrap();
        let s = scan(&o.source).unwrap();
        let mut seen = Vec::new();
        let r = run(&o, &s, &Job::default(), |i, n, name| {
            seen.push((i, n, name.to_string()))
        });
        assert_eq!(r.error, None);
        assert_eq!((r.frames, r.files, r.backed_up), (2, 3, 3));
        assert_eq!(r.bytes, (3 * CHUNK + 5 + 11 + 1000) as u64);
        assert_eq!(
            seen,
            vec![(0, 2, "IMG_0001.CR3".into()), (1, 2, "IMG_0002.CR3".into())]
        );
        for (name, from) in [
            ("shoot-0001.CR3", "IMG_0001.CR3"),
            ("shoot-0001.JPG", "IMG_0001.JPG"),
            ("shoot-0002.CR3", "IMG_0002.CR3"),
        ] {
            let want = std::fs::read(card.join(from)).unwrap();
            assert_eq!(std::fs::read(dest.join("set").join(name)).unwrap(), want);
            assert_eq!(std::fs::read(backup.join("set").join(name)).unwrap(), want);
        }
        assert_eq!(r.main_folder(), Some(dest.join("set").as_path()));
        // The card as it was, and no sidecar with no preset.
        for (p, bytes) in &card_before {
            assert_eq!(&std::fs::read(p).unwrap(), bytes);
        }
        assert_eq!(std::fs::read_dir(&card).unwrap().count(), card_before.len());
        assert!(
            !dest
                .join("set")
                .join(greycard_edit::SIDECAR_FOLDER)
                .exists()
        );
        no_temporaries(&root);

        // Again, under another pattern: nothing copied.
        o.name = "{name}".into();
        let again = run(&o, &s, &Job::default(), |_, _, _| {});
        assert_eq!((again.frames, again.already, again.files), (0, 2, 0));
        assert!(!dest.join("set/IMG_0001.CR3").exists());
        assert!(
            again.line(&dest).contains("2 already there"),
            "{}",
            again.line(&dest)
        );
    }

    /// A name that is there with other bytes is never written over;
    /// the frame is counted and left.
    #[test]
    fn a_name_taken_by_another_file_is_skipped_not_overwritten() {
        let root = dir("taken");
        let card = root.join("card");
        write(&card.join("IMG_0001.CR3"), b"from the card");
        write(&card.join("IMG_0002.CR3"), b"second");
        let dest = root.join("dest");
        write(&dest.join("IMG_0001.CR3"), b"last year's frame");
        let o = options(&card, &dest);
        let r = run(&o, &scan(&card).unwrap(), &Job::default(), |_, _, _| {});
        assert_eq!((r.frames, r.taken, r.already), (1, 1, 0));
        assert_eq!(
            std::fs::read(dest.join("IMG_0001.CR3")).unwrap(),
            b"last year's frame"
        );
        assert!(r.line(&dest).contains("1 skipped"));
    }

    /// Two frames of one name in one run (two folders of the card)
    /// are told apart rather than one written over the other.
    #[test]
    fn two_frames_of_one_name_in_a_run_both_land() {
        let root = dir("twice");
        let card = root.join("card");
        write(&card.join("100CANON/IMG_0001.CR3"), b"first body");
        write(&card.join("101CANON/IMG_0001.CR3"), b"second body");
        let dest = root.join("dest");
        let r = run(
            &options(&card, &dest),
            &scan(&card).unwrap(),
            &Job::default(),
            |_, _, _| {},
        );
        assert_eq!(r.frames, 2);
        assert_eq!(
            std::fs::read(dest.join("IMG_0001.CR3")).unwrap(),
            b"first body"
        );
        assert_eq!(
            std::fs::read(dest.join("IMG_0001 (2).CR3")).unwrap(),
            b"second body"
        );
    }

    /// The stop is heard between frames: the frame in hand lands whole,
    /// the rest are not begun, and nothing half-written is left.
    #[test]
    fn a_cancel_lands_the_frame_in_hand_and_leaves_no_partial_file() {
        let root = dir("cancel");
        let card = root.join("card");
        for i in 1..=4 {
            write(
                &card.join(format!("IMG_000{i}.CR3")),
                &vec![i as u8; CHUNK + 17],
            );
        }
        let dest = root.join("dest");
        let job = Job::default();
        let r = run(
            &options(&card, &dest),
            &scan(&card).unwrap(),
            &job,
            |i, _, _| {
                if i == 1 {
                    job.cancel();
                }
            },
        );
        assert!(r.canceled);
        assert_eq!(r.frames, 2);
        let mut landed: Vec<_> = std::fs::read_dir(&dest)
            .unwrap()
            .map(|e| name_of(&e.unwrap().path()))
            .collect();
        landed.sort();
        assert_eq!(landed, vec!["IMG_0001.CR3", "IMG_0002.CR3"]);
        assert_eq!(
            std::fs::read(dest.join("IMG_0002.CR3")).unwrap().len(),
            CHUNK + 17
        );
        assert!(r.line(&dest).starts_with("import stopped: 2 of 4 frames"));
        no_temporaries(&root);
    }

    /// A card that fails partway through a file: the error names the
    /// file, no file of that name is made, and the temporary is gone.
    #[test]
    fn a_read_error_leaves_nothing_behind() {
        struct Dying(usize);
        impl Read for Dying {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0 == 0 {
                    return Err(std::io::Error::other("I/O error on the card"));
                }
                let k = buf.len().min(self.0);
                buf[..k].fill(3);
                self.0 -= k;
                Ok(k)
            }
        }
        let root = dir("dying");
        let to = root.join("dest/IMG_0001.CR3");
        let e = land_from(&mut Dying(CHUNK + 100), None, &to, None).unwrap_err();
        assert!(format!("{e:#}").contains("I/O error on the card"), "{e:#}");
        assert!(!to.exists());
        no_temporaries(&root);

        // A bad file in a run stops the run there and says which.
        let card = root.join("card");
        write(&card.join("IMG_0001.CR3"), b"fine");
        write(&card.join("IMG_0002.CR3"), b"fine too");
        let dest = root.join("run");
        let mut s = scan(&card).unwrap();
        // Gone between the scan and its copy, as a card pulled out.
        std::fs::remove_file(card.join("IMG_0001.CR3")).unwrap();
        s.items.swap(0, 1);
        let r = run(&options(&card, &dest), &s, &Job::default(), |_, _, _| {});
        assert_eq!(r.frames, 1);
        let (file, _) = r.error.clone().expect("the missing file stops it");
        assert_eq!(file, card.join("IMG_0001.CR3"));
        assert!(r.line(&dest).contains("IMG_0001.CR3"), "{}", r.line(&dest));
        no_temporaries(&root);
    }

    /// The index's hash reads the head and the size, so a copy whose
    /// tail is wrong has the same one; the verify's whole-file hash
    /// does not.
    #[test]
    fn a_copy_wrong_in_its_tail_passes_the_head_hash_and_fails_the_whole_one() {
        let root = dir("tail");
        let mut body = vec![5u8; greycard_library::hash::HEAD + 4096];
        let good = root.join("good.CR3");
        std::fs::write(&good, &body).unwrap();
        *body.last_mut().unwrap() = 6;
        let bad = root.join("bad.CR3");
        std::fs::write(&bad, &body).unwrap();
        assert_eq!(
            greycard_library::hash_file(&good).unwrap(),
            greycard_library::hash_file(&bad).unwrap()
        );
        assert_ne!(hash_whole(&good).unwrap(), hash_whole(&bad).unwrap());
        // And a backup checked against a hash it does not have is
        // refused, nothing written.
        let to = root.join("backup/good.CR3");
        let e = land(&bad, &to, Some(&hash_whole(&good).unwrap())).unwrap_err();
        assert!(format!("{e:#}").contains("no longer matches"), "{e:#}");
        assert!(!to.exists());
        no_temporaries(&root);
    }

    /// The preset is one named step in the sidecar under the
    /// destination's hidden folder, as a preset laid by hand is.
    #[test]
    fn a_preset_is_one_named_step_in_the_hidden_folder() {
        let root = dir("preset");
        let card = root.join("card");
        std::fs::create_dir_all(&card).unwrap();
        greycard_library::fixture::write_frame(
            &card.join("IMG_0001.tif"),
            &greycard_library::fixture::R5,
            1,
        );
        let dest = root.join("dest");
        let mut edit = greycard_edit::Edit::default();
        edit.light.exposure = 0.7;
        let preset = Preset::from_edit("Faded film", &edit, &[greycard_edit::Section::Light]);
        let mut o = options(&card, &dest);
        o.preset = Some(preset);
        o.subfolder = "{yyyy}/{date} {camera}".into();
        let r = run(&o, &scan(&card).unwrap(), &Job::default(), |_, _, _| {});
        assert_eq!((r.frames, r.preset), (1, 1), "{r:?}");
        // The EXIF's day and camera, not the file's time.
        let file = dest.join("2024/2024-08-24 Canon EOS R5/IMG_0001.tif");
        assert!(file.is_file());
        let sidecar = greycard_edit::Sidecar::load(&file).unwrap().unwrap();
        assert_eq!(
            greycard_edit::Sidecar::find(&file),
            Some(greycard_edit::Sidecar::path_in(
                &file,
                greycard_edit::Placement::Folder
            ))
        );
        assert_eq!(sidecar.history.len(), 1);
        assert_eq!(sidecar.current_label.as_deref(), Some("Preset: Faded film"));
        assert_eq!(sidecar.current.light.exposure, 0.7);
    }

    #[test]
    fn the_card_and_the_backup_are_never_written_to_from_inside() {
        let root = dir("check");
        let card = root.join("card");
        std::fs::create_dir_all(card.join("DCIM")).unwrap();
        let mut o = options(&card, &card.join("DCIM/out"));
        assert!(check(&o).is_err());
        o.destination = root.join("dest");
        o.backup = Some(card.clone());
        assert!(check(&o).is_err());
        o.backup = Some(root.join("dest/backup"));
        assert!(check(&o).is_err());
        o.backup = Some(root.join("backup"));
        check(&o).unwrap();
        o.name = "{nope}".into();
        assert!(check(&o).is_err());
    }

    #[test]
    fn a_card_is_a_volume_with_a_dcim_folder() {
        let root = dir("volumes");
        std::fs::create_dir_all(root.join("BACKUP")).unwrap();
        std::fs::create_dir_all(root.join("EOS_DIGITAL/DCIM/100CANON")).unwrap();
        std::fs::create_dir_all(root.join("ZZZ/DCIM")).unwrap();
        let v = volumes_under(&root);
        assert_eq!(v.len(), 3);
        assert_eq!(find_card(&v), Some(root.join("EOS_DIGITAL/DCIM")));
        assert_eq!(find_card(&v[..1]), None);
        assert!(volumes_under(&root.join("not there")).is_empty());
    }
}
