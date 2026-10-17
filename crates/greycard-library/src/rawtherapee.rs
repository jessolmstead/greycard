//! RawTherapee's processing profiles brought across: for each frame,
//! the `.pp3` beside it read by [`greycard_edit::pp3`], its rank, color
//! label and trash mark into the meta exactly, and its white balance,
//! exposure, geometry and lens settings, as near as this engine has
//! them, laid over the frame's edit as one state named for the import.
//! What has no place here is named.
//!
//! The command (`greycard rawtherapee`) and the window's sheet run the
//! one [`import`], and say what it did through the one
//! [`Report::lines`], in the voice of each ([`Voice`]).
//!
//! **A dry run first.** Without `write` nothing is written: each
//! profile is read, the frame's sidecar too, and the report says what
//! a write would carry. A frame with a history here already (a state
//! before its current one, on any version) waits for `force`; forced,
//! it takes every setting the profile's read sections speak to,
//! RawTherapee's neutral ones too ([`pp3::Carry::Every`]). A profile or
//! a sidecar that will not read, or a sidecar that will not write, is
//! that frame's line, and the run goes on.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use greycard_edit::meta::{Flag, Label, Meta};
use greycard_edit::{Placement, Sidecar, pp3};

pub use crate::lightroom_catalog::Voice;

/// What the import is asked to do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Write; without it, a dry run.
    pub write: bool,
    /// Import into a frame with a history here already, every setting
    /// of the profile's read sections, the neutral ones too.
    pub force: bool,
    /// Where a frame with no sidecar yet gets one: under the folder's
    /// `.greycard` (`Some(true)`), as the editor's setting says, or
    /// beside it unless the folder has a `.greycard` (`Some(false)`); or
    /// with none, beside it, for the command, which has no setting.
    /// A sidecar there already is written where it is either way.
    pub sidecars_in_folder: Option<bool>,
}

/// How far a long import has come: a frame at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub of: usize,
}

/// A folder named that would not list.
#[derive(Debug, thiserror::Error)]
#[error("reading {}: {source}", path.display())]
pub struct ImportError {
    pub path: PathBuf,
    pub source: std::io::Error,
}

/// How one frame with a profile was left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fate {
    /// The profile, or the frame's sidecar, would not read: the words
    /// that say which and why.
    Unreadable(String),
    /// Something of its own here already ([`FrameReport::here`]), and
    /// no `force`.
    Held,
    /// The sidecar has all of it already.
    Unchanged,
    /// What a write would write, on a dry run.
    Would,
    /// Written.
    Wrote,
    /// The sidecar would not write: why.
    Failed(String),
}

/// What a frame has of its own here already, that an import waits to
/// be asked to write over.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Here {
    /// Nothing: no sidecar, or one with neither of the two below.
    #[default]
    Nothing,
    /// Stars, a color label or a flag set here (a frame culled here),
    /// and no history. An import holds it back only when the profile
    /// would replace some of them, and reports it as
    /// [`Here::Nothing`] when the profile would set them as they are.
    Meta,
    /// A history: a state before its current one, on any version.
    History,
}

impl Here {
    /// What `sidecar` has of its own.
    pub fn of(sidecar: &Sidecar) -> Self {
        if pp3::has_history(sidecar) {
            Here::History
        } else if culled(&sidecar.meta) {
            Here::Meta
        } else {
            Here::Nothing
        }
    }
}

/// Whether `meta` has stars, a label or a flag: what culling sets, and
/// what a profile's rank, label and trash mark would write over.
fn culled(meta: &Meta) -> bool {
    meta.rating != 0 || meta.label != Label::None || meta.flag != Flag::None
}

/// What a write replaces of a frame's own culling, in words: "5 stars to
/// 0", "label red to none", "the reject cleared". None when `before`
/// had none of its own or nothing of it changes.
fn replaced(before: &Meta, after: &Meta) -> Vec<String> {
    let mut out = Vec::new();
    if !culled(before) {
        return out;
    }
    if before.rating != after.rating {
        out.push(format!("{} stars to {}", before.rating, after.rating));
    }
    if before.label != after.label {
        out.push(format!(
            "label {} to {}",
            before.label.name().to_lowercase(),
            after.label.name().to_lowercase()
        ));
    }
    if before.flag != after.flag {
        out.push(match (before.flag, after.flag) {
            (Flag::Reject, Flag::None) => "the reject cleared".to_string(),
            (Flag::Pick, Flag::None) => "the pick cleared".to_string(),
            (b, a) => format!("{} to {}", b.name().to_lowercase(), a.name().to_lowercase()),
        });
    }
    out
}

/// One frame with a profile beside it, and what the import made of it.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameReport {
    pub file: PathBuf,
    pub profile: PathBuf,
    /// Where the profile came from: "RawTherapee 5.10, profile version
    /// 351". Empty when it would not read.
    pub source: String,
    /// The mapper's report, a kind in a word and its words.
    pub said: Vec<(&'static str, String)>,
    /// What the profile sets that has no place here.
    pub skipped: Vec<String>,
    /// What it had of its own here before the import.
    pub here: Here,
    /// Laid over what it had here, as `force` asked.
    pub forced: bool,
    /// What the write replaces of the frame's own stars, label and flag,
    /// in words.
    pub replaced: Vec<String>,
    /// The meta changes.
    pub meta: bool,
    /// A state is recorded.
    pub edit: bool,
    /// The frame's other versions, which keep their own edits.
    pub versions: usize,
    pub fate: Fate,
}

impl FrameReport {
    /// It carried something, or would on a write: not held back, not
    /// unreadable, not failed.
    fn imported(&self) -> bool {
        matches!(self.fate, Fate::Unchanged | Fate::Would | Fate::Wrote)
    }

    /// What it writes, in words: "the meta and a state \"Imported from
    /// RawTherapee\"".
    fn what(&self) -> String {
        let state = format!(
            "a state \"{}\"",
            greycard_edit::history::import_label(pp3::SOURCE)
        );
        match (self.meta, self.edit) {
            (true, true) => format!("the meta and {state}"),
            (true, false) => "the meta".to_string(),
            (false, true) => state,
            (false, false) => String::new(),
        }
    }

    /// Its lines in the report: a word for the line, or none for one
    /// that goes on from the line before, and the text.
    pub fn lines(&self, voice: Voice) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        let mut line = |label: &str, text: String| out.push((label.to_string(), text));
        if let Fate::Unreadable(why) = &self.fate {
            line("unreadable", why.clone());
            return out;
        }
        let shown = match voice {
            Voice::Command => self.file.display().to_string(),
            Voice::Sheet => self
                .file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.file.display().to_string()),
        };
        line("frame", shown);
        let profile = self
            .profile
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        line("  profile", format!("{profile}, {}", self.source));
        for (kind, words) in &self.said {
            line(&format!("  {kind}"), words.clone());
        }
        if self.fate == Fate::Held {
            let what = match self.here {
                Here::Meta => "its stars, label or flag are set here already",
                Here::History | Here::Nothing => "it has a history here already",
            };
            let how = match voice {
                Voice::Command => "--force lays the import over it",
                Voice::Sheet => {
                    "writing over the frames developed or rated here lays the import over it"
                }
            };
            line("  held back", format!("{what}; {how}"));
            return out;
        }
        if self.forced {
            line(
                "  forced",
                match self.here {
                    Here::History => "over its history here, RawTherapee's neutral settings too",
                    Here::Meta | Here::Nothing => "over its stars, label and flag set here",
                }
                .to_string(),
            );
        }
        if !self.replaced.is_empty() {
            line("  replaces", self.replaced.join(", "));
        }
        match &self.fate {
            Fate::Unchanged => line(
                "  unchanged",
                "the sidecar has all of it already".to_string(),
            ),
            Fate::Failed(why) => {
                line("  failed", format!("writing the sidecar: {why}"));
                return out;
            }
            Fate::Wrote => line("  wrote", self.what()),
            Fate::Would => line("  would write", self.what()),
            Fate::Unreadable(_) | Fate::Held => {}
        }
        if self.edit && self.versions > 0 {
            line(
                "",
                format!(
                    "on the original; its {} other versions are as they were",
                    self.versions
                ),
            );
        }
        out
    }
}

/// What the import found and did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// The frames named, or in the folders named.
    pub frames: usize,
    /// Files named that are not frames, passed over.
    pub passed: Vec<PathBuf>,
    /// Each frame with a profile beside it, in the frames' order.
    pub profiled: Vec<FrameReport>,
    /// Whether it wrote.
    pub write: bool,
}

impl Report {
    fn count(&self, f: impl Fn(&FrameReport) -> bool) -> usize {
        self.profiled.iter().filter(|r| f(r)).count()
    }

    /// Frames held back for what they have here: a history, or stars,
    /// a label or a flag.
    pub fn held(&self) -> usize {
        self.count(|r| r.fate == Fate::Held)
    }

    /// Frames held back for what they have here of `kind`.
    fn held_for(&self, kind: Here) -> usize {
        self.count(|r| r.fate == Fate::Held && r.here == kind)
    }

    pub fn unreadable(&self) -> usize {
        self.count(|r| matches!(r.fate, Fate::Unreadable(_)))
    }

    pub fn failed(&self) -> usize {
        self.count(|r| matches!(r.fate, Fate::Failed(_)))
    }

    /// Frames whose meta is carried, or would be.
    pub fn meta(&self) -> usize {
        self.count(|r| r.imported() && r.meta)
    }

    /// Frames given a state, or that would be.
    pub fn edits(&self) -> usize {
        self.count(|r| r.imported() && r.edit)
    }

    /// Frames a write would change: what Write has to do.
    pub fn to_write(&self) -> usize {
        self.count(|r| r.fate == Fate::Would)
    }

    /// Each name passed over, and on how many of the frames imported,
    /// in the order they were first met.
    pub fn skipped(&self) -> Vec<(String, usize)> {
        let mut skipped: Vec<(String, usize)> = Vec::new();
        for r in self.profiled.iter().filter(|r| r.imported()) {
            for name in &r.skipped {
                match skipped.iter_mut().find(|(n, _)| n == name) {
                    Some((_, count)) => *count += 1,
                    None => skipped.push((name.clone(), 1)),
                }
            }
        }
        skipped
    }

    /// The frames whose sidecars were written.
    pub fn written(&self) -> Vec<PathBuf> {
        self.profiled
            .iter()
            .filter(|r| r.fate == Fate::Wrote)
            .map(|r| r.file.clone())
            .collect()
    }

    /// The files passed over and each frame's lines, in order.
    pub fn frame_lines(&self, voice: Voice) -> Vec<(String, String)> {
        self.frame_lines_upto(voice, usize::MAX)
    }

    /// [`Self::frame_lines`] for the first `most` frames, then a line
    /// that says how many more there are, when there are: for a sheet,
    /// which lays out every line it holds at once. The totals count
    /// them all either way.
    pub fn frame_lines_upto(&self, voice: Voice, most: usize) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .passed
            .iter()
            .map(|p| ("not a frame".to_string(), p.display().to_string()))
            .collect();
        for r in self.profiled.iter().take(most) {
            out.extend(r.lines(voice));
        }
        let more = self.profiled.len().saturating_sub(most);
        if more > 0 {
            out.push((
                "and more".to_string(),
                format!(
                    "{more} more {} with a profile, not listed here; the totals count them",
                    if more == 1 { "frame" } else { "frames" }
                ),
            ));
        }
        out
    }

    /// The totals: the frames found and with a profile, what is carried,
    /// what is held back, unreadable or failed, and each name passed
    /// over with the number of frames it was set on.
    pub fn totals(&self, voice: Voice) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        let mut line = |label: &str, text: String| out.push((label.to_string(), text));
        let verb = if self.write { "" } else { "would be " };
        line(
            "frames",
            format!(
                "{} found, {} with a profile",
                self.frames,
                self.profiled.len()
            ),
        );
        if !self.passed.is_empty() {
            line("not frames", format!("{} passed over", self.passed.len()));
        }
        line("meta", format!("{} {verb}carried exactly", self.meta()));
        line(
            "edits",
            format!("{} {verb}carried, as a state of their own", self.edits()),
        );
        for (kind, what) in [
            (Here::History, "with a history here already"),
            (Here::Meta, "with stars, a label or a flag set here already"),
        ] {
            let held = self.held_for(kind);
            if held > 0 {
                line(
                    "held back",
                    match voice {
                        Voice::Command => format!("{held} {what} (--force to import over it)"),
                        Voice::Sheet => format!("{held} {what}, left alone unless written over"),
                    },
                );
            }
        }
        let unreadable = self.unreadable();
        if unreadable > 0 {
            line("unreadable", unreadable.to_string());
        }
        let failed = self.failed();
        if failed > 0 {
            line("failed", failed.to_string());
        }
        let skipped = self.skipped();
        if !skipped.is_empty() {
            let names: Vec<String> = skipped
                .iter()
                .map(|(name, n)| format!("{name} ({n})"))
                .collect();
            line("no place", names.join(", "));
        }
        if !self.write {
            line(
                "dry run",
                match voice {
                    Voice::Command => "nothing written; --write writes it",
                    Voice::Sheet => "nothing written yet",
                }
                .to_string(),
            );
        }
        out
    }

    /// What the report says, a line at a time, as the command prints
    /// it: the frames, then the totals.
    pub fn lines(&self, voice: Voice) -> Vec<(String, String)> {
        let mut out = self.frame_lines(voice);
        out.extend(self.totals(voice));
        out
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (label, text) in self.lines(Voice::Command) {
            writeln!(f, "{label:<13} {text}")?;
        }
        Ok(())
    }
}

/// The profile RawTherapee keeps beside `file`: `IMG_0001.CR3.pp3`,
/// its default name, or `IMG_0001.pp3`, the one it writes when told
/// to replace the extension. The second is a raw's: a picture takes it
/// only when no raw of its name is beside it, so the camera's JPEG of a
/// raw and JPEG pair does not take the raw's lens profile, white
/// balance and exposure.
pub fn profile_beside(file: &Path) -> Option<PathBuf> {
    profile_for(file, &mut RawStems::default())
}

/// The names of the raws in each folder asked about, each folder listed
/// once: what [`profile_for`] asks of a picture with a replaced-extension
/// profile beside it.
#[derive(Default)]
struct RawStems(HashMap<PathBuf, HashSet<OsString>>);

impl RawStems {
    /// Whether a raw of `file`'s name (its stem) is in its folder.
    fn raw_beside(&mut self, file: &Path) -> bool {
        let (Some(dir), Some(stem)) = (file.parent(), file.file_stem()) else {
            return false;
        };
        self.0
            .entry(dir.to_path_buf())
            .or_insert_with(|| {
                std::fs::read_dir(dir)
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok().map(|e| e.path()))
                            .filter(|p| greycard_core::decode::is_raw_path(p))
                            .filter_map(|p| p.file_stem().map(|s| s.to_os_string()))
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .contains(stem)
    }
}

/// [`profile_beside`], each folder's raws listed once in `stems`.
fn profile_for(file: &Path, stems: &mut RawStems) -> Option<PathBuf> {
    let mut name = file.file_name()?.to_os_string();
    name.push(".pp3");
    let appended = file.with_file_name(name);
    if appended.is_file() {
        return Some(appended);
    }
    let replaced = file.with_extension("pp3");
    if replaced == file || !replaced.is_file() {
        return None;
    }
    (greycard_core::decode::is_raw_path(file) || !stems.raw_beside(file)).then_some(replaced)
}

/// Whether a path is a frame: a raw or a picture.
fn is_frame(p: &Path) -> bool {
    greycard_core::decode::is_raw_path(p) || greycard_core::picture::is_picture_path(p)
}

/// The frames `paths` name, each once: each folder's raws and pictures
/// in name order (its own, not its subfolders'), and each file named
/// that is one; then the files named that are not, passed over, so
/// that `dir/*` does not take a profile or a sidecar for a frame.
pub fn frames_named(paths: &[PathBuf]) -> Result<(Vec<PathBuf>, Vec<PathBuf>), ImportError> {
    let mut frames = Vec::new();
    let mut passed = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(path)
                .map_err(|source| ImportError {
                    path: path.clone(),
                    source,
                })?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_file() && is_frame(p))
                .collect();
            found.sort();
            frames.extend(found);
        } else if is_frame(path) {
            frames.push(path.clone());
        } else {
            passed.push(path.clone());
        }
    }
    let mut seen = std::collections::HashSet::new();
    frames.retain(|f| seen.insert(f.clone()));
    Ok((frames, passed))
}

/// How many of a folder's own frames an import would bring something
/// in to unasked: those a dry run without `force` would write
/// ([`Fate::Would`]). Not a frame held back for what it has here, nor
/// one whose sidecar has all of the profile already (its stars from an
/// earlier import of it), nor one whose sidecar or profile will not
/// read. For the window's offer when a folder opens: it is the import's
/// own dry run, frame by frame, and writes nothing.
pub fn waiting_in(dir: &Path) -> usize {
    let Ok((frames, _)) = frames_named(&[dir.to_path_buf()]) else {
        return 0;
    };
    let dry = Options::default();
    let mut stems = RawStems::default();
    frames
        .iter()
        .filter_map(|f| import_frame(f, &dry, &mut stems))
        .filter(|r| r.fate == Fate::Would)
        .count()
}

/// Read the profiles beside the frames `paths` name and report what an
/// import does; with `options.write`, do it.
pub fn import(paths: &[PathBuf], options: &Options) -> Result<Report, ImportError> {
    import_with(paths, options, &mut |_| {})
}

/// [`import`], saying how far it has come to `progress`, a frame at a
/// time.
pub fn import_with(
    paths: &[PathBuf],
    options: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Report, ImportError> {
    import_each(paths, options, &mut |step| {
        if let Step::Frame { progress: p, .. } = step {
            progress(p);
        }
    })
}

/// A step of a run, as [`import_each`] tells it.
#[derive(Debug, Clone, Copy)]
pub enum Step<'a> {
    /// The frames listed, and the files named that are not frames.
    Listed { passed: &'a [PathBuf] },
    /// A frame done: how far the run has come, and its report when it
    /// has a profile.
    Frame {
        progress: Progress,
        report: Option<&'a FrameReport>,
    },
}

/// [`import`], telling `each` every step as it is taken: the command
/// prints a frame's lines as it is done, as it always has.
pub fn import_each(
    paths: &[PathBuf],
    options: &Options,
    each: &mut dyn FnMut(Step<'_>),
) -> Result<Report, ImportError> {
    let (frames, passed) = frames_named(paths)?;
    each(Step::Listed { passed: &passed });
    let mut report = Report {
        frames: frames.len(),
        passed,
        profiled: Vec::new(),
        write: options.write,
    };
    let of = frames.len();
    let mut stems = RawStems::default();
    for (i, file) in frames.iter().enumerate() {
        let done = Progress { done: i + 1, of };
        match import_frame(file, options, &mut stems) {
            Some(r) => {
                report.profiled.push(r);
                each(Step::Frame {
                    progress: done,
                    report: report.profiled.last(),
                });
            }
            None => each(Step::Frame {
                progress: done,
                report: None,
            }),
        }
    }
    Ok(report)
}

/// The sidecar of `file` as an import starts from: the one it has, or a
/// new one at the edit the editor would open the frame at.
fn sidecar_for(file: &Path) -> Result<Sidecar, String> {
    let mut sidecar = match Sidecar::load(file) {
        Ok(Some(s)) => s,
        Ok(None) => Sidecar {
            line: greycard_edit::Line::of(crate::lightroom_catalog::starting_edit(file)),
            ..Sidecar::default()
        },
        Err(e) => return Err(format!("reading the sidecar of {}: {e}", file.display())),
    };
    // A sidecar from before schema 4 does not know which way up an
    // `Original` crop goes until the frame says; the file is here, so
    // it is asked before anything is written back.
    if sidecar.needs_frame() {
        match greycard_core::decode::stance_path(file)
            .ok()
            .and_then(|s| s.shown_size(sidecar.turn))
        {
            Some((w, h)) => sidecar.migrate_with_frame(h > w),
            None => log::warn!(
                "{}: will not say which way up it is, so its Original crop is left for \
                 the editor to bring up to date",
                file.display()
            ),
        }
    }
    Ok(sidecar)
}

/// One frame: its profile read and laid into its sidecar, saved only
/// when `options.write`. None when it has no profile beside it. Only the
/// original takes it: the frame's other versions are as they were, as a
/// preset laid over the frame leaves them.
fn import_frame(file: &Path, options: &Options, stems: &mut RawStems) -> Option<FrameReport> {
    let profile = profile_for(file, stems)?;
    let mut report = FrameReport {
        file: file.to_path_buf(),
        profile: profile.clone(),
        source: String::new(),
        said: Vec::new(),
        skipped: Vec::new(),
        here: Here::Nothing,
        forced: false,
        replaced: Vec::new(),
        meta: false,
        edit: false,
        versions: 0,
        fate: Fate::Unchanged,
    };
    let unreadable = |mut report: FrameReport, why: String| {
        report.fate = Fate::Unreadable(why);
        Some(report)
    };
    let text = match std::fs::read(&profile) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => return unreadable(report, format!("{}: {e}", profile.display())),
    };
    // One frame's sidecar that will not read is that frame's to
    // report, not the end of a folder's run.
    let mut sidecar = match sidecar_for(file) {
        Ok(s) => s,
        Err(why) => return unreadable(report, why),
    };
    let stance = greycard_core::decode::stance_path(file).ok();
    let frame = pp3::Frame {
        size: stance.and_then(|s| s.shown_size(0)),
        turned: sidecar.turn % 4 != 0,
    };
    // Stars, a label or a flag set here hold the frame as a history
    // does when the profile would replace any of them: a profile's rank
    // 0 and no trash mark would wipe a cull. Stars the profile would set
    // as they are (an earlier import of it, or a cull that agrees) hold
    // nothing.
    let mut here = Here::of(&sidecar);
    let carry = if here == Here::History && options.force {
        pp3::Carry::Every
    } else {
        pp3::Carry::Set
    };
    let imported = match pp3::read_carrying(&text, frame, carry) {
        Ok(i) => i,
        Err(e) => return unreadable(report, format!("{}: {e}", profile.display())),
    };
    report.source = imported.source();
    report.said = imported.report();
    report.skipped = imported.skipped.clone();
    if here == Here::Meta {
        let mut after = sidecar.meta.clone();
        imported.rated.apply(&mut after);
        if replaced(&sidecar.meta, &after).is_empty() {
            here = Here::Nothing;
        }
    }
    report.here = here;
    if here != Here::Nothing && !options.force {
        report.fate = Fate::Held;
        return Some(report);
    }
    report.forced = here != Here::Nothing;
    let before = sidecar.meta.clone();
    let landed = pp3::import_into(&mut sidecar, &imported);
    report.replaced = replaced(&before, &sidecar.meta);
    report.meta = landed.meta;
    report.edit = landed.edit;
    report.versions = sidecar.versions.len();
    report.fate = if !landed.meta && !landed.edit {
        Fate::Unchanged
    } else if options.write {
        let placement = match options.sidecars_in_folder {
            None => Placement::of(file),
            Some(in_folder) => crate::lightroom_catalog::placement_for(file, in_folder),
        };
        match sidecar.save_in(file, placement) {
            Ok(()) => Fate::Wrote,
            Err(e) => Fate::Failed(e.to_string()),
        }
    } else {
        Fate::Would
    };
    Some(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::tests::scratch;
    use greycard_edit::meta::{Flag, Label};

    /// A profile of RawTherapee's shape: the meta, an exposure, and a
    /// tool with no place here.
    const PROFILE: &str = "\
[Version]
AppVersion=5.10
Version=351

[General]
Rank=4
ColorLabel=3
InTrash=true

[Exposure]
Compensation=-0.7

[Local Contrast]
Enabled=true
";

    fn write(write: bool, force: bool) -> Options {
        Options {
            write,
            force,
            sidecars_in_folder: None,
        }
    }

    /// The one frame's report of a run over `file`.
    fn one(file: &Path, options: Options) -> Option<FrameReport> {
        let report = import(&[file.to_path_buf()], &options).unwrap();
        report.profiled.into_iter().next()
    }

    fn fate(file: &Path, options: Options) -> Option<Fate> {
        one(file, options).map(|r| r.fate)
    }

    #[test]
    fn the_profile_is_found_by_either_name() {
        let dir = scratch("pp3-names");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"").unwrap();
        assert_eq!(profile_beside(&raw), None);
        std::fs::write(dir.join("IMG_0001.pp3"), PROFILE).unwrap();
        assert_eq!(profile_beside(&raw), Some(dir.join("IMG_0001.pp3")));
        // RawTherapee's own naming wins when both are there.
        std::fs::write(dir.join("IMG_0001.CR3.pp3"), PROFILE).unwrap();
        assert_eq!(profile_beside(&raw), Some(dir.join("IMG_0001.CR3.pp3")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_is_its_frames_each_once() {
        let dir = scratch("pp3-folder");
        for name in ["b.NEF", "a.cr3", "a.cr3.pp3", "a.cr3.gcd", "notes.txt"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("c.cr3"), b"").unwrap();
        let (frames, passed) = frames_named(&[dir.clone(), dir.join("a.cr3")]).unwrap();
        assert_eq!(frames, [dir.join("a.cr3"), dir.join("b.NEF")]);
        assert!(passed.is_empty());
        // `dir/*`: the profile and the sidecar named are not frames.
        let named: Vec<PathBuf> = ["a.cr3", "a.cr3.pp3", "a.cr3.gcd", "b.NEF"]
            .iter()
            .map(|n| dir.join(n))
            .collect();
        let (frames, passed) = frames_named(&named).unwrap();
        assert_eq!(frames, [dir.join("a.cr3"), dir.join("b.NEF")]);
        assert_eq!(passed, [dir.join("a.cr3.pp3"), dir.join("a.cr3.gcd")]);
        // A folder that will not list is the run's error, said with it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let shut = dir.join("shut");
            std::fs::create_dir_all(&shut).unwrap();
            std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
            let listed = frames_named(std::slice::from_ref(&shut));
            std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();
            // Run as root, the folder lists all the same.
            if let Err(e) = listed {
                assert!(e.to_string().starts_with("reading "), "{e}");
                assert_eq!(e.path, shut);
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn forced_the_frame_follows_rawtherapee_back_to_neutral() {
        let dir = scratch("pp3-forced");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"").unwrap();
        let profile = dir.join("IMG_0001.CR3.pp3");
        std::fs::write(&profile, PROFILE).unwrap();
        assert_eq!(fate(&raw, write(true, false)), Some(Fate::Wrote));
        // The exposure set back to nothing there.
        std::fs::write(
            &profile,
            PROFILE.replace("Compensation=-0.7", "Compensation=0"),
        )
        .unwrap();
        assert_eq!(fate(&raw, write(true, false)), Some(Fate::Held));
        let forced = one(&raw, write(true, true)).unwrap();
        assert_eq!(forced.fate, Fate::Wrote);
        assert!(forced.edit && forced.forced);
        let sidecar = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(sidecar.current.light.exposure, 0.0);
        assert_eq!(sidecar.history.len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_sidecar_that_will_not_read_is_that_frame_s_alone() {
        let dir = scratch("pp3-bad-sidecar");
        for n in ["IMG_0001.CR3", "IMG_0002.CR3"] {
            std::fs::write(dir.join(n), b"").unwrap();
            std::fs::write(dir.join(format!("{n}.pp3")), PROFILE).unwrap();
        }
        std::fs::write(dir.join("IMG_0001.CR3.gcd"), "{ not json").unwrap();
        // The folder's run goes on to the next frame.
        let report = import(std::slice::from_ref(&dir), &write(true, false)).unwrap();
        assert!(matches!(
            &report.profiled[0].fate,
            Fate::Unreadable(why) if why.starts_with("reading the sidecar of ")
        ));
        assert_eq!(report.profiled[1].fate, Fate::Wrote);
        assert_eq!(report.unreadable(), 1);
        let next = Sidecar::load(&dir.join("IMG_0002.CR3")).unwrap().unwrap();
        assert_eq!(next.meta.rating, 4);
        assert_eq!(
            std::fs::read_to_string(dir.join("IMG_0001.CR3.gcd")).unwrap(),
            "{ not json"
        );
        // Not offered for either: one will not read, and one has come in
        // (its meta and a state).
        assert_eq!(waiting_in(&dir), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_dry_run_writes_nothing_and_write_writes_the_meta_and_one_state() {
        let dir = scratch("pp3-import");
        // Not a raw: the import reads nothing of the file but its
        // name, and the probes that fail on it say so and go on.
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"").unwrap();
        std::fs::write(dir.join("IMG_0001.CR3.pp3"), PROFILE).unwrap();
        let other = dir.join("IMG_0002.CR3");
        std::fs::write(&other, b"").unwrap();
        assert_eq!(one(&other, write(true, false)), None, "no profile");
        assert_eq!(waiting_in(&dir), 1);

        let dry = one(&raw, write(false, false)).unwrap();
        assert_eq!(dry.fate, Fate::Would);
        assert!(dry.meta && dry.edit);
        assert_eq!(dry.skipped, ["Local Contrast"]);
        assert!(Sidecar::find(&raw).is_none(), "a dry run writes nothing");
        let folder = import(std::slice::from_ref(&dir), &write(false, false)).unwrap();
        assert_eq!(folder.frames, 2);
        assert_eq!(folder.profiled.len(), 1);
        assert_eq!(folder.to_write(), 1);
        assert!(folder.written().is_empty());
        assert!(Sidecar::find(&raw).is_none());

        let wrote = one(&raw, write(true, false)).unwrap();
        assert_eq!(wrote.fate, Fate::Wrote);
        assert_eq!((wrote.meta, wrote.edit), (dry.meta, dry.edit));
        let sidecar = Sidecar::load(&raw).unwrap().expect("written");
        assert_eq!(sidecar.meta.rating, 4);
        assert_eq!(sidecar.meta.label, Label::Green);
        assert_eq!(sidecar.meta.flag, Flag::Reject);
        assert_eq!(sidecar.history.len(), 1);
        assert_eq!(
            sidecar.current_label.as_deref(),
            Some("Imported from RawTherapee")
        );
        assert_eq!(sidecar.current.light.exposure, -0.7);
        assert_eq!(sidecar.history[0].edit.light.exposure, 0.0);
        assert_eq!(waiting_in(&dir), 0, "come in, so not offered again");

        // Now it has a history: held back, unless forced, and forced
        // there is nothing new to lay over it.
        assert_eq!(fate(&raw, write(true, false)), Some(Fate::Held));
        let forced = one(&raw, write(true, true)).unwrap();
        assert_eq!(forced.fate, Fate::Unchanged);
        assert_eq!(forced.skipped, ["Local Contrast"]);
        assert_eq!(Sidecar::load(&raw).unwrap().unwrap().history.len(), 1);

        // A profile that is not one is said, and passed over.
        std::fs::write(dir.join("IMG_0002.CR3.pp3"), "not a profile").unwrap();
        assert!(matches!(
            fate(&other, write(true, false)),
            Some(Fate::Unreadable(_))
        ));
        assert!(Sidecar::find(&other).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_command_s_report_reads_as_it_always_has() {
        let dir = scratch("pp3-lines");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"").unwrap();
        std::fs::write(dir.join("IMG_0001.CR3.pp3"), PROFILE).unwrap();
        std::fs::write(dir.join("IMG_0002.CR3"), b"").unwrap();
        let notes = dir.join("notes.txt");
        std::fs::write(&notes, b"").unwrap();
        let paths = [dir.clone(), notes.clone()];
        let dry = import(&paths, &write(false, false)).unwrap();
        let text = dry.to_string();
        let expected = format!(
            "not a frame   {notes}\n\
             frame         {raw}\n\
             \x20 profile     IMG_0001.CR3.pp3, RawTherapee 5.10, profile version 351\n\
             \x20 exact       4 stars, label green, reject\n\
             \x20 approximate exposure -0.70 EV\n\
             \x20 no place    Local Contrast\n\
             \x20 would write the meta and a state \"Imported from RawTherapee\"\n\
             frames        2 found, 1 with a profile\n\
             not frames    1 passed over\n\
             meta          1 would be carried exactly\n\
             edits         1 would be carried, as a state of their own\n\
             no place      Local Contrast (1)\n\
             dry run       nothing written; --write writes it\n",
            notes = notes.display(),
            raw = raw.display(),
        );
        assert_eq!(text, expected);
        // Written, then run again: held back, and the totals say so.
        import(&paths, &write(true, false)).unwrap();
        let again = import(&paths, &write(true, false)).unwrap();
        let lines = again.lines(Voice::Command);
        assert!(lines.contains(&(
            "  held back".into(),
            "it has a history here already; --force lays the import over it".into()
        )));
        assert!(lines.contains(&(
            "held back".into(),
            "1 with a history here already (--force to import over it)".into()
        )));
        // The sheet's voice names no flag, and the frame by its name.
        let sheet = again.lines(Voice::Sheet);
        assert!(!sheet.iter().any(|(_, t)| t.contains("--")), "{sheet:?}");
        assert!(sheet.contains(&("frame".into(), "IMG_0001.CR3".into())));
        assert_eq!(
            dry.lines(Voice::Sheet).last(),
            Some(&("dry run".to_string(), "nothing written yet".to_string()))
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_run_told_a_step_at_a_time_says_what_the_report_says() {
        let dir = scratch("pp3-steps");
        for n in ["IMG_0001.CR3", "IMG_0002.CR3", "IMG_0003.CR3"] {
            std::fs::write(dir.join(n), b"").unwrap();
        }
        std::fs::write(dir.join("IMG_0001.CR3.pp3"), PROFILE).unwrap();
        std::fs::write(dir.join("IMG_0003.pp3"), PROFILE).unwrap();
        let notes = dir.join("notes.txt");
        std::fs::write(&notes, b"").unwrap();
        // As the command prints it: the files passed over, each frame's
        // lines as it is done, then the totals.
        let mut told = Vec::new();
        let mut done = Vec::new();
        let report = import_each(
            &[dir.clone(), notes],
            &write(false, false),
            &mut |step| match step {
                Step::Listed { passed } => told.extend(
                    passed
                        .iter()
                        .map(|p| ("not a frame".to_string(), p.display().to_string())),
                ),
                Step::Frame { progress, report } => {
                    done.push(progress.done);
                    if let Some(r) = report {
                        told.extend(r.lines(Voice::Command));
                    }
                }
            },
        )
        .unwrap();
        assert_eq!(done, [1, 2, 3]);
        told.extend(report.totals(Voice::Command));
        assert_eq!(told, report.lines(Voice::Command));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_frame_culled_here_keeps_its_stars_and_reject_unless_forced() {
        let dir = scratch("pp3-culled");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"").unwrap();
        // RawTherapee's untouched rank, label and trash mark.
        std::fs::write(
            dir.join("IMG_0001.CR3.pp3"),
            "[Version]\nVersion=351\n[General]\nRank=0\nColorLabel=0\nInTrash=false\n\
             [Exposure]\nCompensation=0.5\n",
        )
        .unwrap();
        // Culled here: 5 stars and a reject, never developed.
        let mut culled = Sidecar::default();
        culled.meta.set_rating(5);
        culled.meta.flag = Flag::Reject;
        culled.save(&raw).unwrap();
        assert_eq!(Here::of(&Sidecar::load(&raw).unwrap().unwrap()), Here::Meta);
        assert_eq!(waiting_in(&dir), 0, "not offered");

        // Unforced: held back, both kept, and said why.
        let report = import(std::slice::from_ref(&dir), &write(true, false)).unwrap();
        let r = &report.profiled[0];
        assert_eq!(r.fate, Fate::Held);
        assert_eq!(r.here, Here::Meta);
        let lines = report.lines(Voice::Command);
        assert!(lines.contains(&(
            "  held back".into(),
            "its stars, label or flag are set here already; --force lays the import over it".into()
        )));
        assert!(lines.contains(&(
            "held back".into(),
            "1 with stars, a label or a flag set here already (--force to import over it)".into()
        )));
        let kept = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!((kept.meta.rating, kept.meta.flag), (5, Flag::Reject));
        assert!(kept.history.is_empty());

        // Forced: the report says what it clears, the dry run first.
        let dry = import(std::slice::from_ref(&dir), &write(false, true)).unwrap();
        let r = &dry.profiled[0];
        assert_eq!(r.fate, Fate::Would);
        assert_eq!(r.replaced, ["5 stars to 0", "the reject cleared"]);
        let lines = dry.lines(Voice::Command);
        assert!(lines.contains(&(
            "  forced".into(),
            "over its stars, label and flag set here".into()
        )));
        assert!(lines.contains(&(
            "  replaces".into(),
            "5 stars to 0, the reject cleared".into()
        )));
        // Forced over meta and no history, the edit is what the profile
        // sets, not its neutral too.
        assert!(!r.said.iter().any(|(k, _)| *k == "neutral"));
        import(std::slice::from_ref(&dir), &write(true, true)).unwrap();
        let after = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!((after.meta.rating, after.meta.flag), (0, Flag::None));
        assert_eq!(after.current.light.exposure, 0.5);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stars_an_earlier_import_set_hold_nothing_and_a_differing_cull_does() {
        let dir = scratch("pp3-meta-again");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"").unwrap();
        let profile = dir.join("IMG_0001.CR3.pp3");
        // A profile of the meta alone: its import leaves no history.
        let meta_only = "[Version]\nVersion=351\n[General]\nRank=3\nColorLabel=1\nInTrash=false\n";
        std::fs::write(&profile, meta_only).unwrap();
        assert_eq!(fate(&raw, write(true, false)), Some(Fate::Wrote));
        let first = Sidecar::load(&raw).unwrap().unwrap();
        assert!(first.history.is_empty());
        assert_eq!(Here::of(&first), Here::Meta);
        // Run again unforced: nothing of its own would be replaced, so
        // nothing holds it; there is nothing new to write, and it is not
        // offered.
        let again = import(std::slice::from_ref(&dir), &write(true, false)).unwrap();
        let r = &again.profiled[0];
        assert_eq!(r.fate, Fate::Unchanged);
        assert_eq!((r.here, r.forced), (Here::Nothing, false));
        assert_eq!(again.held(), 0);
        assert_eq!(waiting_in(&dir), 0);
        // The profile edited again in RawTherapee: its exposure comes
        // across unforced, the stars kept, and it is offered.
        std::fs::write(
            &profile,
            format!("{meta_only}[Exposure]\nCompensation=0.4\n"),
        )
        .unwrap();
        assert_eq!(waiting_in(&dir), 1);
        let r = one(&raw, write(true, false)).unwrap();
        assert_eq!(r.fate, Fate::Wrote);
        assert!(r.edit && !r.forced && r.replaced.is_empty());
        let s = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(s.current.light.exposure, 0.4);
        assert_eq!(s.meta.rating, 3);
        // A cull that differs, on a frame with no history yet, still
        // holds it.
        let other = dir.join("IMG_0002.CR3");
        std::fs::write(&other, b"").unwrap();
        std::fs::write(dir.join("IMG_0002.CR3.pp3"), meta_only).unwrap();
        let mut culled = Sidecar::default();
        culled.meta.set_rating(1);
        culled.save(&other).unwrap();
        let r = one(&other, write(true, false)).unwrap();
        assert_eq!((r.fate, r.here), (Fate::Held, Here::Meta));
        assert_eq!(Sidecar::load(&other).unwrap().unwrap().meta.rating, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_camera_s_jpeg_beside_its_raw_does_not_take_the_raw_s_profile() {
        let dir = scratch("pp3-pair");
        // A raw and the camera's JPEG of it, and RawTherapee's profile
        // by the replaced-extension name, which is the raw's.
        let raw = dir.join("IMG_0001.CR3");
        let jpeg = dir.join("IMG_0001.JPG");
        std::fs::write(&raw, b"").unwrap();
        std::fs::write(&jpeg, b"").unwrap();
        std::fs::write(dir.join("IMG_0001.pp3"), PROFILE).unwrap();
        assert_eq!(profile_beside(&raw), Some(dir.join("IMG_0001.pp3")));
        assert_eq!(profile_beside(&jpeg), None);
        // A JPEG with nothing of its name beside it takes it, and one with
        // a profile of its own full name takes that.
        let lone = dir.join("IMG_0002.jpg");
        std::fs::write(&lone, b"").unwrap();
        std::fs::write(dir.join("IMG_0002.pp3"), PROFILE).unwrap();
        assert_eq!(profile_beside(&lone), Some(dir.join("IMG_0002.pp3")));
        std::fs::write(dir.join("IMG_0001.JPG.pp3"), PROFILE).unwrap();
        assert_eq!(profile_beside(&jpeg), Some(dir.join("IMG_0001.JPG.pp3")));
        std::fs::remove_file(dir.join("IMG_0001.JPG.pp3")).unwrap();
        // The folder's run and the offer agree: the raw and the lone JPEG.
        let report = import(std::slice::from_ref(&dir), &write(false, false)).unwrap();
        let files: Vec<&PathBuf> = report.profiled.iter().map(|r| &r.file).collect();
        assert_eq!(files, [&raw, &lone]);
        assert_eq!(waiting_in(&dir), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_new_sidecar_goes_where_the_options_say() {
        let dir = scratch("pp3-placement");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"").unwrap();
        std::fs::write(dir.join("IMG_0001.CR3.pp3"), PROFILE).unwrap();
        let options = Options {
            write: true,
            force: false,
            sidecars_in_folder: Some(true),
        };
        let mut seen = Vec::new();
        let report =
            import_with(std::slice::from_ref(&dir), &options, &mut |p| seen.push(p)).unwrap();
        assert_eq!(seen, [Progress { done: 1, of: 1 }]);
        assert_eq!(report.written(), std::slice::from_ref(&raw));
        let at = Sidecar::find(&raw).unwrap();
        assert_eq!(Placement::at(&at), Placement::Folder, "{}", at.display());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
