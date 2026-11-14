//! An export with no window: the frames named, each under its
//! sidecar's edit with the overrides laid on top, sent to a worker of
//! its own as a set, and the caller waiting on the set's outcome. The
//! command line's `greycard export` and the editor's `--export` both
//! run it.
//!
//! The export never needed the window: its part was only to read the
//! sidecar, lay the overrides on it, fill the sheet, and wait, and all
//! of that is here. What it lent was its device, and the export runs
//! there every stage with a device version, as the viewport does. The
//! engine makes no device: the caller lends the run one ([`Lent`]), the
//! command line's own made by `Context::for_export` where the machine
//! has a GPU, and the run lends it to its worker as the window lends
//! its own, so the editor's export and this one are one picture on one
//! machine. Without one, every stage runs on the CPU.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use greycard_edit::{DisplayCurve, Edit, WhiteBalance, preset};
use greycard_render::export;

use crate::ai::{parts_left_out, parts_words};
use crate::preset_fit::preset_at_start;
use crate::settings::Shared;
use crate::worker::{Job, Outcome, Worker};
use crate::{export_name, queue, sheet};

/// What an export is asked for, as the command line names it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Request {
    /// The frames: one file; one folder, whose frame opened is the one
    /// the settings remember in it, else its first, as the window opens
    /// it; or several files, each a frame of the set (with `all`, a
    /// folder among them brings its every frame). None for the folder
    /// of the last file the settings remember.
    pub paths: Vec<PathBuf>,
    /// The file the frame goes to, or the folder the set goes into (one
    /// that is there, or a path ending in a separator).
    pub target: PathBuf,
    /// Rows of the folder opened (from 0) in the set beside the frame
    /// opened.
    pub also: Vec<usize>,
    /// Every frame of the folder opened.
    pub all: bool,
    /// A develop preset laid over the frame opened's edit, as a step
    /// in its history.
    pub preset: Option<String>,
    /// The export preset the sheet is filled from.
    pub export_preset: Option<String>,
    /// The long edge over the sheet's size.
    pub long_edge: Option<u32>,
    /// What a file there already gets, over the sheet's choice.
    pub on_exists: Option<export::OnExists>,
    /// The version of each frame exported, by its name.
    pub version_name: Option<String>,
    /// What is laid over the frame opened's edit for this run alone.
    pub overrides: Overrides,
    /// Neither read nor write sidecars.
    pub no_sidecars: bool,
    /// Write sidecars under the frame folder's `.greycard`, over the
    /// settings' choice.
    pub sidecar_folder: bool,
    /// Write the `.xmp` with each sidecar, over the settings' choice.
    pub xmp_sidecars: bool,
    /// The author each file names, over the settings' (and so over the
    /// camera's); blank for none this run, the camera's standing.
    pub author: Option<String>,
    /// The copyright each file carries, as `author`.
    pub copyright: Option<String>,
}

impl Request {
    /// The author and copyright this run writes: each flag's over the
    /// settings' own.
    pub fn rights(&self, remembered: &Shared) -> export::Rights {
        let kept = remembered.rights();
        let given = export::Rights::new(
            self.author.as_deref().unwrap_or_default(),
            self.copyright.as_deref().unwrap_or_default(),
        );
        export::Rights {
            author: match &self.author {
                Some(_) => given.author,
                None => kept.author,
            },
            copyright: match &self.copyright {
                Some(_) => given.copyright,
                None => kept.copyright,
            },
        }
    }
}

/// What an export run writes: each frame under its edit, and the
/// sheet the set is written under.
pub struct Plan {
    pub frames: Vec<queue::Frame>,
    /// `--export DIR`: the folder the frames go into. None for
    /// `--export FILE`, one frame to the path given.
    pub folder: Option<PathBuf>,
    pub settings: export::Settings,
    pub on_exists: export::OnExists,
    /// The export preset the sheet is as saved, for each frame's
    /// history; none when it is not one.
    pub preset: Option<String>,
    /// Where each frame's sidecar is written with the export noted in
    /// its history; none with sidecars off.
    pub placement: Option<greycard_edit::Placement>,
    /// Whether that write writes the frame's `.xmp` too, as the
    /// window's every sidecar write does (`--xmp-sidecars`, or the
    /// Settings sheet's choice).
    pub xmp: bool,
    /// The user's library index, when the run may note in it the
    /// archive copies its sidecar writes leave waiting (§233): the
    /// editor's own, never one named on the command line, and None
    /// in a test. It is opened only if it is there and at this
    /// build's schema; a run never makes, migrates or rebuilds it.
    pub index: Option<PathBuf>,
}

/// What came of a run.
pub struct Finished {
    pub tally: queue::Tally,
    /// The frames written without something their edit names: a look
    /// not in the look directory, a learned model not downloaded or
    /// failing.
    pub left_out: usize,
    /// The line the run ends on, for whoever ran it.
    pub line: String,
}

/// The exit codes: every frame written as its edit asks (or left, as
/// `--on-exists skip` asks); a frame not written, or a run that could
/// not begin; every frame written, but some without something their
/// edit names that this machine has not got (a look, a learned model)
/// or could not run.
pub const WRITTEN: u8 = 0;
pub const FAILED: u8 = 1;
pub const LEFT_OUT: u8 = 2;

/// What a frame counted toward [`LEFT_OUT`] went without, in the run's
/// last line.
const MISSING: &str = "something its edit names that this machine has not got";

impl Finished {
    /// Whether every frame went: one that failed, or a set that never
    /// finished, is a failure a script can see. A file there already
    /// and left alone, as `--on-exists skip` asks, is not.
    pub fn succeeded(&self) -> bool {
        self.tally.failed.is_empty() && self.tally.canceled == 0
    }

    /// The process's exit code for the run.
    pub fn code(&self) -> u8 {
        if !self.succeeded() {
            FAILED
        } else if self.left_out > 0 {
            LEFT_OUT
        } else {
            WRITTEN
        }
    }
}

/// What `req` asks to be written, with `remembered` the settings
/// file's sheet and presets and `presets` the store a `--preset` is
/// looked up in. An error for anything the request names that cannot
/// be done, before a frame is begun.
pub fn plan(
    req: &Request,
    remembered: &Shared,
    presets: Option<&preset::Store>,
    index: Option<PathBuf>,
) -> Result<Plan> {
    check_all(req)?;
    let target = req.target.as_path();
    // The files, and which of them is the one opened: as the window
    // opens them, the file named, or a folder's last file when the
    // settings remember one in it, else its first; several named are
    // each a frame of the set.
    let last_file = (!remembered.last_file.is_empty())
        .then(|| PathBuf::from(&remembered.last_file))
        .filter(|p| p.is_file());
    let several = req.paths.len() > 1;
    let (files, first) = match req.paths.as_slice() {
        [path] => {
            let files = greycard_library::files::list_files(path, remembered.jpegs_apart)?;
            anyhow::ensure!(!files.is_empty(), "no RAW files at {}", path.display());
            let first = if path.is_dir() {
                position_of(&files, last_file.as_deref())
            } else {
                0
            };
            (files, first)
        }
        [] => {
            let dir = last_file.as_deref().and_then(Path::parent).context(
                "an export wants a file or a folder to export from, and no last file is remembered",
            )?;
            let files = greycard_library::files::list_files(dir, remembered.jpegs_apart)?;
            anyhow::ensure!(!files.is_empty(), "no RAW files at {}", dir.display());
            let first = position_of(&files, last_file.as_deref());
            (files, first)
        }
        paths => (named(paths, req.all, remembered.jpegs_apart)?, 0),
    };
    let into_folder = export_folder(target, !req.also.is_empty() || req.all || several);
    check_also(req, into_folder, files.len())?;
    anyhow::ensure!(
        !(req.all || several) || into_folder,
        "{} names a set, and {} is one frame's file: give a folder (one that is there, or a path ending in a separator)",
        if req.all {
            "--all"
        } else {
            "more than one frame"
        },
        target.display()
    );
    if !into_folder {
        check_file_target(target, &files)?;
    }

    let write_sidecars = !req.no_sidecars;
    let placement = if req.sidecar_folder || remembered.sidecars_in_folder {
        greycard_edit::Placement::Folder
    } else {
        greycard_edit::Placement::Beside
    };
    let preset = match &req.preset {
        Some(name) => Some(
            presets
                .and_then(|s| {
                    // The film presets, as the window's first run
                    // puts them there.
                    s.seed();
                    s.find(name)
                })
                .with_context(|| format!("no preset called {name}"))?
                .preset,
        ),
        None => None,
    };

    // The sheet as the window would open with it, and the preset it
    // still is as saved.
    let (sheet, chosen) =
        remembered.opening_sheet(req.export_preset.as_deref(), req.long_edge, req.on_exists)?;
    let sheet_settings = sheet.settings();
    // Metadata None writes nothing, the author and copyright included:
    // the choice wins over the flags, and the run says so.
    let rights = if sheet_settings.metadata == export::Metadata::None {
        let flagged = export::Rights::new(
            req.author.as_deref().unwrap_or_default(),
            req.copyright.as_deref().unwrap_or_default(),
        );
        if !flagged.is_empty() {
            tracing::warn!(
                "--author and --copyright are not written: the export's metadata is None"
            );
        }
        export::Rights::default()
    } else {
        req.rights(remembered)
    };
    let sheet_settings = export::Settings {
        rights,
        ..sheet_settings
    };
    let preset_name = chosen
        .filter(|n| !sheet::reserved(n))
        .and_then(|n| sheet::find(&remembered.export_presets, &n))
        .filter(|p| p.sheet.same(&sheet))
        .map(|p| p.name.clone());

    // The set: the frame opened and the `--also` rows, in file order;
    // with `--all`, every row the folder's listing has.
    let mut rows: Vec<usize> = if req.all || several {
        (0..files.len()).collect()
    } else {
        std::iter::once(first)
            .chain(if into_folder {
                req.also.clone()
            } else {
                Vec::new()
            })
            .collect()
    };
    rows.sort_unstable();
    rows.dedup();

    let mut frames = Vec::with_capacity(rows.len());
    for &i in &rows {
        let source = files[i].clone();
        let held = greycard_library::held::from_disk(
            &source,
            write_sidecars.then_some(placement),
            remembered.fresh(),
        );
        let unreadable = matches!(
            held.trouble,
            Some(greycard_edit_io::on_disk::Trouble::Unreadable(_))
        );
        let mut sidecar = held.sidecar;
        // The version `--version-name` named, first by position when two
        // share the name; the file's own line without it. Found before
        // the preset goes on, so the preset lands on the line exported.
        let frame_version = match &req.version_name {
            None => None,
            Some(name) => {
                let matching: Vec<_> = sidecar
                    .versions
                    .iter()
                    .filter(|v| v.name == *name)
                    .collect();
                if matching.is_empty() {
                    anyhow::bail!("{} has no version named {name}", source.display());
                }
                if matching.len() > 1 {
                    tracing::warn!(
                        "{} has {} versions named {name}; the first is exported",
                        source.display(),
                        matching.len()
                    );
                }
                Some(queue::Version {
                    id: matching[0].id.clone(),
                    name: matching[0].name.clone(),
                })
            }
        };
        let version = frame_version.as_ref().map(|v| v.id.as_str());
        if i == first
            && let Some(preset) = &preset
        {
            // Onto the line exported and written, as the window lays it;
            // not over a sidecar that would not read.
            let placement = (write_sidecars && !unreadable).then_some(placement);
            preset_at_start(&mut sidecar, version, &source, preset, placement);
        }
        greycard_edit_io::on_disk::migrate_from_file(&mut sidecar, &source);
        let line = sidecar
            .line(version)
            .expect("the version was found in this sidecar above");
        let mut edit = line.current.clone();
        // The line's own wait for its ISO.
        let raw = !greycard_core::picture::is_picture_path(&source);
        let seed = raw && greycard_edit_io::on_disk::line_never_developed(line);

        // The command line's temperature, exposure and curve, over the
        // edit of the frame opened (every frame's, with `--all` or
        // several named) and never onto its sidecar.
        if i == first || req.all || several {
            req.overrides.apply(&mut edit);
        }
        frames.push(queue::Frame {
            source,
            edit,
            // The version `--version-name` named, none without it.
            version: frame_version,
            turn: sidecar.turn,
            time_shift: sidecar.meta.time_shift,
            seed_blend: seed,
            out: PathBuf::new(),
            moved_from: None,
        });
    }

    let (folder, settings, preset_name) = if into_folder {
        let sources = queue::sources_of(&frames);
        // Each file named by the sheet's Name, as the window names it;
        // no window here, so a shot is read from each file.
        let shots = export_name::wants_shot(&sheet.name);
        let shifted: Vec<_> = frames
            .iter()
            .map(|f| {
                let shot = shots.then(|| export_name::read_shot(&f.source));
                (f.source.clone(), shot, f.time_shift)
            })
            .collect();
        let (stems, problem) = export_name::stems(&sheet.name, &shifted);
        if let Some(problem) = problem {
            tracing::warn!("{problem}");
        }
        for (frame, (out, moved_from)) in frames.iter_mut().zip(queue::named_as(
            &sources,
            &stems,
            Some(target),
            None,
            sheet_settings.format,
            sheet.on_exists(),
        )) {
            frame.out = out;
            frame.moved_from = moved_from;
        }
        (Some(target.to_path_buf()), sheet_settings, preset_name)
    } else {
        // One frame to the path given, in the format its extension
        // names; a format the path changed is not the preset's file.
        let settings = batch_settings(target, sheet_settings.clone());
        let preset_name = preset_name.filter(|_| settings == sheet_settings);
        frames[0].out = target.to_path_buf();
        (None, settings, preset_name)
    };
    Ok(Plan {
        frames,
        folder,
        settings,
        on_exists: sheet.on_exists(),
        preset: preset_name,
        placement: write_sidecars.then_some(placement),
        xmp: req.xmp_sidecars || remembered.xmp_sidecars,
        index,
    })
}

/// The context a run's device stages run on, made by the
/// caller on a device of its own; or why there is none, in the
/// caller's words (`--cpu`, or [`greycard_gpu::Declined`]'s), for the
/// run's first line.
pub type Lent = std::result::Result<greycard_gpu::Context, String>;

/// The run's first line: where it develops.
pub fn lent_line(lent: &Lent) -> String {
    match lent {
        Ok(ctx) => format!("export: on {}", ctx.name()),
        Err(why) => format!("export: on the CPU ({why})"),
    }
}

/// Write `plan` on a worker of its own and wait for the set: each
/// frame's export noted in its sidecar's history as it lands, as the
/// window notes it. The worker's ops run on `lent` where it is a
/// context.
pub fn run(plan: Plan, lent: Lent) -> Finished {
    if let Some(dir) = &plan.folder
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        let line = format!("export: {}: {e}", dir.display());
        tracing::info!("{line}");
        return Finished {
            tally: queue::Tally {
                failed: vec![(dir.display().to_string(), e.to_string())],
                ..Default::default()
            },
            left_out: 0,
            line,
        };
    }
    run_on(plan, lent)
}

/// [`run`], its folder there.
fn run_on(plan: Plan, lent: Lent) -> Finished {
    let Plan {
        frames,
        folder,
        settings,
        on_exists,
        preset,
        placement,
        xmp,
        index,
    } = plan;
    tracing::info!("{}", lent_line(&lent));
    let total = frames.len();
    let single = folder.is_none().then(|| frames[0].out.clone());
    let set = Arc::new(queue::Set::new(total, folder, settings, on_exists).with_preset(preset));
    tracing::info!(
        "exporting {} with no window: {}",
        match &single {
            Some(path) => format!("to {}", path.display()),
            None => format!(
                "{} to {}",
                queue::frames(total),
                set.folder.as_deref().unwrap_or(Path::new("")).display()
            ),
        },
        set.settings.describe()
    );
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = Worker::headless(move |outcome| {
        let _ = tx.send(outcome);
    });
    // The context watches its own device: a loss marks it lost, and
    // the worker's ops go to the CPU from the next frame. A frame whose
    // op met the loss as an error goes on on the CPU; one whose wait
    // panicked on it fails, and the run says so with exit code 1.
    if let Ok(ctx) = lent {
        worker.set_context(ctx);
    }
    worker.send(Job::ExportSet {
        set: set.clone(),
        frames,
    });
    let mut written = Vec::new();
    let mut left_out = 0;
    // Of those, the frames without something other than a People mask
    // written empty, which the line words apart.
    let mut missing_other = 0;
    let mut tally = None;
    // The index and the roots kept beside it, when both are there and
    // an archive is among the roots: an export's record on a sidecar
    // is a save, and its archive copy is owed a write. The library is
    // opened only as it is (`open_current`): a run never brings it up.
    let archive_roots = index.filter(|p| p.is_file()).and_then(|index| {
        let roots =
            greycard_library::Roots::load(&greycard_library::Roots::path_beside(&index)).ok()?;
        (!roots.archives().is_empty()).then_some((index, roots))
    });
    loop {
        let outcome = match rx.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(outcome) => outcome,
            // A frame takes as long as it takes; only a worker that is
            // gone ends the wait.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if worker.running() => continue,
            Err(_) => break,
        };
        match outcome {
            Outcome::SetFrameStarted { index, name, .. } => {
                tracing::info!("{}", queue::progress_line(index, total, &name));
            }
            Outcome::SetFrameDone {
                index,
                source,
                edit,
                version,
                done,
                ..
            } => {
                let at = format!("{} of {total}", index + 1);
                match done {
                    queue::Done::Exported {
                        path,
                        seconds,
                        note,
                        left_out: missing,
                    } => {
                        tracing::info!("exported {at}: {} in {seconds:.2} s", path.display());
                        if let Some(note) = note {
                            tracing::warn!("exported {}: {note}", path.display());
                        }
                        // On the terminal: the file is out, and not
                        // as its edit asked.
                        for item in &missing {
                            tracing::warn!("{}: {item}", source.display());
                        }
                        if !missing.is_empty() {
                            left_out += 1;
                        }
                        let (asks, nobody) = parts_left_out(&missing);
                        if missing.len() > asks + nobody {
                            missing_other += 1;
                        }
                        if let Some(placement) = placement {
                            let exported = greycard_edit::Exported {
                                file: path.to_string_lossy().into_owned(),
                                preset: set.preset.clone(),
                                at: now(),
                            };
                            if greycard_library::export_record::record_export_on_disk(
                                &source,
                                &edit,
                                version.as_deref(),
                                exported,
                                placement,
                                xmp,
                            ) && let Some((index, roots)) = archive_roots.as_ref()
                            {
                                // The copy on the archive is not
                                // written here: noted as waiting, for
                                // the next window's catch-up (§233).
                                greycard_library::sync_note::note_disk_save(index, roots, &source);
                            }
                        }
                        written.push(path);
                    }
                    queue::Done::Skipped { path } => {
                        tracing::warn!("skipped {at}: {} is there already", path.display());
                    }
                    queue::Done::Failed { message } => {
                        tracing::error!("export {at} failed: {}: {message}", source.display());
                    }
                    queue::Done::Canceled => {}
                }
            }
            Outcome::SetDone { tally: t, .. } => {
                tally = Some(t);
                break;
            }
            // A develop's own words (a fill at work, a model fetched)
            // are the window's; nothing else is asked of this worker.
            _ => {}
        }
    }
    worker.stop();
    let seconds = set.started.elapsed().as_secs_f64();
    let Some(tally) = tally else {
        let line = "export: the worker stopped before the set was done".to_string();
        tracing::info!("{line}");
        return Finished {
            tally: queue::Tally {
                canceled: total,
                ..Default::default()
            },
            left_out,
            line,
        };
    };
    let mut line = match (&single, written.as_slice()) {
        // One frame: the file it went to, as the window's status said.
        (Some(_), [path]) => format!("exported {} in {seconds:.2} s", path.display()),
        (Some(path), _) if tally.skipped > 0 => {
            format!("{} is there already: nothing exported", path.display())
        }
        (Some(path), _) => match tally.failed.first() {
            Some((_, why)) => format!("export to {} failed: {why}", path.display()),
            None => format!("export to {} did not happen", path.display()),
        },
        (None, _) => queue::finished_line(&tally, total, &set.place(), seconds),
    };
    match (missing_other, &single) {
        (0, _) => {}
        (_, Some(_)) => line.push_str(&format!(", without {MISSING} (see above)")),
        (n, None) => line.push_str(&format!(", {n} without {MISSING} (see above)")),
    }
    // A set's line has its Parts written empty already
    // (`queue::finished_line`); a frame's says them here.
    if single.is_some() {
        line.push_str(&parts_words(false, tally.parts));
    }
    // Into the log at info whichever way it went: `export` says it on
    // stderr itself, and a warning here would say it there twice. The
    // frames that failed or went without something were warned above.
    tracing::info!("{line}");
    Finished {
        tally,
        left_out,
        line,
    }
}

/// One frame's file: refused, before anything is read or written, when
/// its extension names no format an export writes (a second raw named
/// last, which would be written over in another format) or when it is
/// one of the frames.
fn check_file_target(target: &Path, files: &[PathBuf]) -> Result<()> {
    anyhow::ensure!(
        export::Format::from_path(target).is_some(),
        "{} names no format an export writes: end it in .jpg, .png, .tif, or .webp, or give a folder (one that is there, or a path ending in a separator)",
        target.display()
    );
    let same = |f: &PathBuf| match (f.canonicalize(), target.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => f == target,
    };
    anyhow::ensure!(
        !files.iter().any(same),
        "{} is one of the frames: give the file the export writes last",
        target.display()
    );
    Ok(())
}

/// `--all` names a folder's every frame: an error, not a quiet export
/// of one, when no folder is given or the one path named is one file;
/// and an error with `--preset`, which would write a step into the
/// history of the frame opened alone, as it would with several frames
/// named. An export to one file is refused once the target is known.
fn check_all(req: &Request) -> Result<()> {
    let several = req.paths.len() > 1;
    if req.all {
        let [path] = req.paths.as_slice() else {
            anyhow::ensure!(several, "--all exports a folder: give its path");
            return check_preset(req, "--all");
        };
        // A path that is not there is the listing's to refuse.
        if path.exists() && !path.is_dir() {
            anyhow::bail!(
                "--all exports every frame of a folder, and {} is one file: open the folder, not a file, to export a set",
                path.display()
            );
        }
        return check_preset(req, "--all");
    }
    if several {
        anyhow::ensure!(
            req.also.is_empty(),
            "--also names rows of one folder's frames, and {} paths are named",
            req.paths.len()
        );
        return check_preset(req, "more than one frame");
    }
    Ok(())
}

/// `--preset` refused for a set of `what`, with the batch apply named.
fn check_preset(req: &Request, what: &str) -> Result<()> {
    anyhow::ensure!(
        req.preset.is_none(),
        "--preset with {what} would write a step into one frame's history; `greycard presets --apply NAME <files>` lays it over every file, then export with --all"
    );
    Ok(())
}

/// The frames of several `paths`, in the order named and each once: a
/// file is itself, a folder its every frame with `all` and refused
/// without.
fn named(paths: &[PathBuf], all: bool, apart: bool) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = Vec::new();
    for path in paths {
        anyhow::ensure!(
            all || !path.is_dir(),
            "{} is a folder: name its frames, or add --all for every frame in it",
            path.display()
        );
        for f in greycard_library::files::list_files(path, apart)? {
            if !files.contains(&f) {
                files.push(f);
            }
        }
    }
    anyhow::ensure!(!files.is_empty(), "no RAW files at the paths named");
    Ok(files)
}

/// Whether the target names a folder for the set rather than a file
/// for one frame: a folder that is there, or a path that ends in a
/// separator, which asks for one. For a set (`--also`, `--all`, or
/// several frames named), a path that is not there and names no
/// picture format is a folder too: a set has no one file to go to.
pub fn export_folder(path: &Path, set: bool) -> bool {
    path.is_dir()
        || path
            .as_os_str()
            .to_string_lossy()
            .ends_with(std::path::is_separator)
        || (set && !path.exists() && export::Format::from_path(path).is_none())
}

/// `--also` names a set: an error, not a quiet export of one frame,
/// when the export is to one file or a row is not in the folder (rows
/// from 0, as the browser has them).
fn check_also(req: &Request, into_folder: bool, files: usize) -> Result<()> {
    if req.also.is_empty() {
        return Ok(());
    }
    if !into_folder {
        anyhow::bail!(
            "--also names a set, and {} is one frame's file: give a folder (one that is there, or a path ending in a separator)",
            req.target.display()
        );
    }
    if let Some(row) = req.also.iter().find(|r| **r >= files) {
        anyhow::bail!(
            "--also {row}: there {} {files} frame{} to export (rows from 0); open the folder, not a file, to export a set",
            if files == 1 { "is" } else { "are" },
            if files == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

/// `--develop-temperature`, `--exposure`, and `--agx`: what the command
/// line lays over the frame opened's edit for this run, never onto
/// its sidecar, so a run that exports or takes a picture of the frame
/// at another exposure leaves the frame's edit and its history as it
/// found them, and an export record names only a look the frame really
/// had. The window never records the overridden edit as a state of its
/// own: while the panel shows it unmoved, a save, an undo, a click in
/// the history, a snapshot or a preset records nothing of the panel's
/// (`panel::edit::panel_state`). Once the user moves anything, what the
/// panel then holds is recorded like any edit, with whatever of the
/// override's values it still carries.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Overrides {
    pub temperature: Option<f32>,
    pub exposure: Option<f32>,
    pub display_curve: Option<DisplayCurve>,
}

impl Overrides {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// `edit` with these laid over it.
    pub fn apply(&self, edit: &mut Edit) {
        if let Some(t) = self.temperature {
            edit.white_balance = WhiteBalance::Custom {
                temperature: t as f64,
                tint: 0.0,
            };
        }
        if let Some(ev) = self.exposure {
            edit.light.exposure = ev;
        }
        if let Some(curve) = self.display_curve {
            edit.display_curve = curve;
        }
    }
}

/// Where `last` is among `files`, by the file system's own account of
/// the path; the first file when it is none of them.
fn position_of(files: &[PathBuf], last: Option<&Path>) -> usize {
    let Some(want) = last.and_then(|l| l.canonicalize().ok()) else {
        return 0;
    };
    files
        .iter()
        .position(|f| f.canonicalize().is_ok_and(|f| f == want))
        .unwrap_or(0)
}

/// A batch export's settings: the sheet's, as remembered or as the
/// preset named on the command line fills it, in the format the
/// path's extension names, or the sheet's when it names none.
pub fn batch_settings(path: &Path, sheet: export::Settings) -> export::Settings {
    export::Settings {
        format: export::Format::from_path(path).unwrap_or(sheet.format),
        ..sheet
    }
}

/// Now, in seconds since the epoch, for an export's record.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::Sheet;
    use crate::testing::remove_dir_retry;
    use crate::worker::Developed;
    use greycard_core::raw::{Orientation, RawFrame};
    use greycard_edit::{Preset, Sidecar};
    use std::sync::mpsc;
    use std::time::Duration;

    /// The context the command line lends a run on this machine, a
    /// software adapter taken under `GREYCARD_REQUIRE_GPU`, whose
    /// machines run the GPU tests on lavapipe.
    fn lent() -> Lent {
        let required = std::env::var_os("GREYCARD_REQUIRE_GPU").is_some_and(|v| !v.is_empty());
        let asked = greycard_gpu::asked_by_hand(|n| std::env::var_os(n));
        greycard_gpu::Context::for_export(asked || required).map_err(|d| d.to_string())
    }

    /// A folder of the test's own under the workspace's target, gone
    /// when the test is, however it ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/test-scratch")
                .join(format!("headless-{name}-{}", std::process::id()));
            remove_dir_retry(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        /// The scratch's `name` as a folder to export into, its path
        /// ending in a separator.
        fn folder(&self, name: &str) -> PathBuf {
            let mut folder = self.0.join(name).into_os_string();
            folder.push(std::path::MAIN_SEPARATOR_STR);
            PathBuf::from(folder)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            remove_dir_retry(&self.0);
        }
    }

    /// An export of `paths` to `target`, nothing else asked.
    fn to(paths: &[&Path], target: &Path) -> Request {
        Request {
            paths: paths.iter().map(|p| p.to_path_buf()).collect(),
            target: target.to_path_buf(),
            ..Request::default()
        }
    }

    /// `plan`'s refusal of `req`, as said.
    fn refused(req: &Request, remembered: &Shared) -> String {
        plan(req, remembered, None, None)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default()
    }

    /// Files with a raw's name and nothing in them: enough for a plan,
    /// which lists them and reads their sidecars but decodes nothing.
    fn names(dir: &Path, n: usize) -> Vec<PathBuf> {
        (0..n)
            .map(|i| {
                let p = dir.join(format!("IMG_{i:04}.CR3"));
                std::fs::write(&p, b"").unwrap();
                p
            })
            .collect()
    }

    /// A linear DNG of our own writing, a textured scene of `width` by
    /// `height`: a raw the engine opens, which no sample raw in the
    /// repository may stand in for.
    fn dng(path: &Path, width: usize, height: usize) {
        use greycard_core::raw::{
            Calibration, CfaPattern, LevelPattern, Levels, Samples, SensorLayout,
        };
        let m = greycard_core::color::WORKING_SPACE
            .from_xyz_matrix()
            .expect("the working space has a matrix");
        let frame = RawFrame {
            make: "Test".into(),
            model: "Cam".into(),
            other_names: Vec::new(),
            width,
            height,
            channels: 1,
            layout: SensorLayout::Cfa(CfaPattern::rggb()),
            samples: Samples::U16(Vec::new()),
            levels: Levels {
                black: LevelPattern::uniform(0.0, 1),
                white: LevelPattern::uniform(65535.0, 1),
                black_deltas: None,
            },
            as_shot_coefficients: Some([2.0, 1.0, 1.6]),
            calibrations: vec![Calibration {
                illuminant: 21,
                color_matrix: m.rows.iter().flatten().map(|v| *v as f32).collect(),
                forward_matrix: None,
            }],
            crop: None,
            orientation: Orientation::Normal,
            shot: Default::default(),
            dng: Default::default(),
        };
        let mut data = Vec::with_capacity(width * height * 3);
        for y in 0..height {
            for x in 0..width {
                let (fx, fy) = (x as f32, y as f32);
                let v = 0.5
                    + 0.2 * (fx * 0.13).sin() * (fy * 0.09).cos()
                    + 0.1 * ((fx + fy) * 0.031).sin();
                // A ramp across for the curve's whole range, the
                // channels apart for some color.
                let ramp = 0.02 + 0.9 * fx / width as f32;
                data.extend_from_slice(&[ramp * v * 0.5, ramp * v, ramp * v * 0.6]);
            }
        }
        let image = greycard_core::CameraImage::from_data(width, height, data).unwrap();
        let mut file = std::fs::File::create(path).unwrap();
        greycard_core::dng::write_linear_dng(
            &mut file,
            &frame,
            &image,
            [2.0, 1.0, 1.6],
            &Default::default(),
        )
        .unwrap();
    }

    /// What the window does for an export of the frame it has open,
    /// job for job: the device lent to the worker, the
    /// frame opened under `edit` and developed on it, the blend the
    /// open seeds taken into the edit, then the open frame written
    /// under that edit.
    fn window_export(frame: &queue::Frame, settings: export::Settings) -> Option<PathBuf> {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::new(move |o| {
            let _ = tx.send(o);
        });
        // The device the command line takes on this machine, lent as
        // the window lends its own.
        let lent = lent();
        println!("the window's: {}", lent_line(&lent));
        if let Ok(ctx) = &lent {
            worker.set_gpu(ctx.device(), ctx.queue());
        }
        worker.send(Job::Open {
            path: frame.source.clone(),
            version: frame.version.as_ref().map(|v| v.id.as_str().into()),
            edit: frame.edit.clone(),
            generation: 1,
            seed_blend: frame.seed_blend,
            turn: frame.turn,
            stance_known: false,
            learned: Vec::new(),
        });
        let mut edit = frame.edit.clone();
        let wait = Duration::from_secs(300);
        loop {
            match rx.recv_timeout(wait).expect("the develop lands") {
                Outcome::Opened { blend: Some(b), .. } => edit.noise.learned_strength = b,
                Outcome::Developed { image, .. } => {
                    println!(
                        "the window's develop: {}",
                        match image {
                            Developed::Texture(_) => "on the GPU",
                            Developed::Halves(_) => "on the CPU",
                        }
                    );
                    break;
                }
                Outcome::Failed { message, .. } => panic!("the develop failed: {message}"),
                _ => {}
            }
        }
        // The edit carries the blend the open seeded: it is not seeded twice.
        let set = Arc::new(queue::Set::new(
            1,
            None,
            settings,
            export::OnExists::Overwrite,
        ));
        worker.send(Job::ExportSet {
            set,
            frames: vec![queue::Frame {
                edit,
                seed_blend: false,
                ..frame.clone()
            }],
        });
        let written = loop {
            if let Outcome::SetFrameDone { done, .. } =
                rx.recv_timeout(wait).expect("the export lands")
            {
                match done {
                    queue::Done::Exported { path, .. } => break Some(path),
                    queue::Done::Failed { message } => panic!("the export failed: {message}"),
                    _ => break None,
                }
            }
        };
        worker.stop();
        written
    }

    /// Whether every byte where `a` and `b` differ lies in a date and
    /// time `a` writes, EXIF's `2026:10:03 18:00:00`, XMP's
    /// `2026-10-03T18:00:00` or the embedded profile's header's six
    /// numbers after its `RGB XYZ `: the moment each file was written.
    fn differ_only_in_dates(a: &[u8], b: &[u8]) -> bool {
        const ICC: &[u8] = b"RGB XYZ ";
        let in_profile_date = |i: usize| {
            (i.saturating_sub(ICC.len() + 11)..i.saturating_sub(ICC.len() - 1))
                .any(|s| a.get(s..s + ICC.len()) == Some(ICC) && i < s + ICC.len() + 12)
        };
        let date = |s: &[u8]| {
            s.len() == 19
                && s.iter().enumerate().all(|(i, &c)| match i {
                    4 | 7 => c == b':' || c == b'-',
                    10 => c == b' ' || c == b'T',
                    13 | 16 => c == b':',
                    _ => c.is_ascii_digit(),
                })
        };
        a.len() == b.len()
            && (0..a.len()).filter(|&i| a[i] != b[i]).all(|i| {
                (i.saturating_sub(18)..=i).any(|s| s + 19 <= a.len() && date(&a[s..s + 19]))
                    || in_profile_date(i)
            })
    }

    /// The comparison lets the moment of writing differ and nothing
    /// else.
    #[test]
    fn only_the_moment_of_writing_may_differ() {
        let a =
            b"x 2026:10:04 01:20:26 y RGB XYZ \x07\xea\x00\x0a\x00\x04\x00\x01\x00\x14\x00\x1a z";
        let mut b = a.to_vec();
        b[20] = b'7';
        b[24 + 8 + 11] = 0x1b;
        assert!(differ_only_in_dates(a, &b));
        b[0] = b'w';
        assert!(!differ_only_in_dates(a, &b));
        let mut c = a.to_vec();
        c[a.len() - 1] = b'q';
        assert!(!differ_only_in_dates(a, &c));
    }

    /// The point of the run with no window: the file it writes is the
    /// one the window wrote, pixel for pixel and byte for byte but for
    /// the moment it was written, under a sidecar's edit with the
    /// command line's display curve, exposure and temperature laid on
    /// it and a size from the command line. The window's half is the
    /// jobs it sent its worker, on a device of its own as the window's
    /// was, so its develop runs the GPU's sharpen where there is one.
    #[test]
    fn an_export_with_no_window_writes_the_window_s_pixels() {
        let scratch = Scratch::new("parity");
        let source = scratch.0.join("frame.dng");
        dng(&source, 480, 320);
        let mut sidecar = Sidecar::default();
        sidecar.current.light.exposure = 0.4;
        sidecar.current.light.tone.contrast = 1.2;
        sidecar.current.light.tone.highlights = -0.5;
        sidecar
            .save_in(&source, greycard_edit::Placement::Beside)
            .unwrap();
        let out = scratch.0.join("headless.jpg");
        let req = Request {
            overrides: Overrides {
                temperature: Some(4800.0),
                exposure: Some(0.7),
                display_curve: Some(DisplayCurve::Agx),
            },
            long_edge: Some(300),
            ..to(&[&source], &out)
        };
        let remembered = Shared::default();
        let plan = plan(&req, &remembered, None, None).unwrap();

        // The window's edit: the sidecar's, the overrides over it; its
        // sheet: the remembered one with the flags over it, in the
        // path's format.
        let mut edit = Sidecar::load(&source).unwrap().unwrap().line.current;
        req.overrides.apply(&mut edit);
        assert_eq!(plan.frames.len(), 1);
        assert_eq!(plan.frames[0].edit, edit);
        assert_eq!(edit.display_curve, DisplayCurve::Agx);
        assert_eq!(edit.light.exposure, 0.7);
        let (sheet, _) = remembered.opening_sheet(None, Some(300), None).unwrap();
        let settings = batch_settings(&out, sheet.settings());
        assert_eq!(plan.settings, settings);

        let window = queue::Frame {
            out: scratch.0.join("window.jpg"),
            ..plan.frames[0].clone()
        };
        let windowed = window_export(&window, settings).expect("the window wrote it");
        let finished = run(plan, lent());
        assert!(finished.succeeded(), "{}", finished.line);

        let (a, b) = (
            std::fs::read(&windowed).unwrap(),
            std::fs::read(&out).unwrap(),
        );
        let (pa, pb) = (
            image::load_from_memory(&a).unwrap().to_rgb8(),
            image::load_from_memory(&b).unwrap().to_rgb8(),
        );
        assert_eq!(pa.dimensions(), (300, 200));
        assert_eq!(pa.dimensions(), pb.dimensions());
        let differing = pa.pixels().zip(pb.pixels()).filter(|(x, y)| x != y).count();
        assert_eq!(differing, 0, "pixels differ");
        assert!(
            differ_only_in_dates(&a, &b),
            "bytes differ outside the dates"
        );
        // The overrides' edit is no state of the frame's history, so
        // the export is not noted in it, as the window did not note it.
        let kept = Sidecar::load(&source).unwrap().unwrap();
        assert!(kept.current_exports.is_empty());
    }

    /// `.webp` names the WebP export, as `.png` names the PNG; into a
    /// folder, a sheet set to WebP names its files `.webp`.
    #[test]
    fn a_webp_path_writes_a_webp() {
        let scratch = Scratch::new("webp");
        let source = scratch.0.join("frame.dng");
        dng(&source, 96, 64);
        let remembered = Shared::default();
        let out = scratch.0.join("out.WEBP");
        let req = Request {
            no_sidecars: true,
            ..to(&[&source], &out)
        };
        let planned = plan(&req, &remembered, None, None).unwrap();
        assert_eq!(planned.settings.format, export::Format::WebP);
        assert!(run(planned, lent()).succeeded());
        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP");
        let sheet = export::Settings {
            format: export::Format::WebP,
            ..export::Settings::default()
        };
        let into = scratch.0.join("set");
        let settings = batch_settings(&into, sheet.clone());
        assert_eq!(settings.format, export::Format::WebP);
        assert_eq!(
            batch_settings(&scratch.0.join("a.png"), sheet).format,
            export::Format::Png
        );
    }

    /// Under the sidecar's own edit the export is noted in the
    /// frame's history, as the window notes it; with sidecars off
    /// nothing is read or written beside the frame.
    #[test]
    fn an_export_is_noted_in_the_frame_s_history_unless_sidecars_are_off() {
        let scratch = Scratch::new("history");
        let source = scratch.0.join("frame.dng");
        dng(&source, 96, 64);
        let mut sidecar = Sidecar::default();
        sidecar.current.light.exposure = -0.3;
        sidecar
            .save_in(&source, greycard_edit::Placement::Beside)
            .unwrap();
        let out = scratch.0.join("out.png");
        let remembered = Shared::default();
        let planned = plan(&to(&[&source], &out), &remembered, None, None).unwrap();
        assert_eq!(planned.settings.format, export::Format::Png);
        assert_eq!(planned.frames[0].edit.light.exposure, -0.3);
        assert!(run(planned, lent()).succeeded());
        let kept = Sidecar::load(&source).unwrap().unwrap();
        assert_eq!(kept.current_exports.len(), 1);
        assert_eq!(
            kept.current_exports[0].file,
            out.to_string_lossy().into_owned()
        );

        let off = scratch.0.join("off.png");
        let req = Request {
            no_sidecars: true,
            ..to(&[&source], &off)
        };
        let planned = plan(&req, &remembered, None, None).unwrap();
        // The default edit, not the sidecar's.
        assert_eq!(planned.frames[0].edit, Edit::default());
        assert_eq!(planned.placement, None);
        assert!(run(planned, lent()).succeeded());
        assert!(off.is_file());
        assert_eq!(
            Sidecar::load(&source)
                .unwrap()
                .unwrap()
                .current_exports
                .len(),
            1
        );
    }

    /// The export of a frame nobody has developed is at its body's
    /// baseline: a GFX 100S II frame comes out, under the shoulder, as
    /// the same pixels from a body with the default baseline at the
    /// difference on the slider (over it the display white is the
    /// body's own). A sidecar written before the bodies had baselines of
    /// their own comes out as it did: on the GFX frame at +0.3, the
    /// default body's at +0.3.
    #[test]
    fn an_unedited_frame_exports_at_its_body_s_baseline_and_an_older_sidecar_as_it_was() {
        let scratch = Scratch::new("baseline");
        let gfx = scratch.0.join("gfx.dng");
        let other = scratch.0.join("other.dng");
        let (make, model) = crate::testing::GFX;
        crate::testing::linear_dng(&gfx, 96, 64, make, model);
        crate::testing::linear_dng(&other, 96, 64, "Test", "Cam");
        let b = greycard_edit::baseline::for_body(Some("Fujifilm GFX 100S II"));
        let remembered = Shared::default();
        let export = |source: &Path, name: &str| {
            let out = scratch.0.join(name);
            let planned = plan(&to(&[source], &out), &remembered, None, None).unwrap();
            assert!(run(planned, lent()).succeeded());
            image::open(&out).unwrap().to_rgb16()
        };
        let save = |source: &Path, exposure: f32| {
            let mut sidecar = Sidecar::default();
            sidecar.current.light.exposure = exposure;
            sidecar
                .save_in(source, greycard_edit::Placement::Beside)
                .unwrap();
        };
        let unedited = export(&gfx, "gfx.tif");
        save(&other, b - greycard_render::finish::BASELINE_EXPOSURE);
        let moved = export(&other, "other.tif");
        assert_eq!(unedited.dimensions(), moved.dimensions());
        // Under the shoulder, where the display white (the body's) has
        // no say: every channel under half of white on both sides,
        // which is under mid grey in the scene.
        let under: Vec<(u16, u16)> = unedited
            .pixels()
            .zip(moved.pixels())
            .filter(|(p, q)| p.0.iter().chain(q.0.iter()).all(|v| *v < 32768))
            .flat_map(|(p, q)| (0..3).map(move |k| (p[k], q[k])))
            .collect();
        assert!(under.len() > 3 * 1000, "{} channels under", under.len());
        let worst = under.iter().map(|(p, q)| p.abs_diff(*q)).max().unwrap();
        assert!(worst <= 1, "{worst} levels apart");
        // And not the default body's picture as it is.
        std::fs::remove_file(Sidecar::path_for(&other)).unwrap();
        assert_ne!(export(&other, "plain.tif"), unedited);

        // A sidecar as a build before wrote it, at +0.3, beside each.
        for source in [&gfx, &other] {
            save(source, 0.3);
            let path = Sidecar::path_for(source);
            let text = std::fs::read_to_string(&path).unwrap();
            let now = format!("\"version\": {}", greycard_edit::VERSION);
            assert!(text.contains(&now));
            std::fs::write(&path, text.replace(&now, "\"version\": 5")).unwrap();
        }
        assert_eq!(
            export(&gfx, "gfx-before.tif"),
            export(&other, "other-before.tif")
        );
    }

    /// `--version-name NAME`: the frame goes out under that version's
    /// edit, with the name in the file's, and the export is recorded on
    /// the version's line and not the file's own; a name the sidecar
    /// has not got stops the run with the file in the reason.
    #[test]
    fn a_version_named_is_exported_under_its_edit_and_its_name() {
        let scratch = Scratch::new("version");
        let source = scratch.0.join("frame.dng");
        dng(&source, 96, 64);
        let mut sidecar = Sidecar::default();
        sidecar.current.light.exposure = -0.3;
        let id = sidecar.new_version(None, Some("Warm")).unwrap();
        sidecar.line_mut(Some(&id)).unwrap().record(Edit {
            light: greycard_edit::Light {
                exposure: 0.7,
                ..Default::default()
            },
            ..Edit::default()
        });
        sidecar
            .save_in(&source, greycard_edit::Placement::Beside)
            .unwrap();
        let out = scratch.0.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let remembered = Shared::default();
        let named = |name: &str| Request {
            version_name: Some(name.into()),
            ..to(&[&source], &out)
        };
        let planned = plan(&named("Warm"), &remembered, None, None).unwrap();
        assert_eq!(planned.frames[0].edit.light.exposure, 0.7);
        assert_eq!(
            planned.frames[0].version.as_ref().map(|v| v.name.as_str()),
            Some("Warm")
        );
        assert_eq!(planned.frames[0].out, out.join("frame-Warm.jpg"));
        assert!(run(planned, lent()).succeeded());
        assert!(out.join("frame-Warm.jpg").is_file());
        let kept = Sidecar::load(&source).unwrap().unwrap();
        assert_eq!(kept.current_exports.len(), 0, "not on the file's own line");
        let line = kept.line(Some(&id)).unwrap();
        assert_eq!(line.current_exports.len(), 1);
        assert!(line.current_exports[0].file.ends_with("frame-Warm.jpg"));

        let message = format!(
            "{:#}",
            plan(&named("Cool"), &remembered, None, None)
                .err()
                .expect("no such version")
        );
        assert!(
            message.contains("frame.dng") && message.contains("Cool"),
            "{message}"
        );
    }

    /// The author and copyright a run writes: the settings', each
    /// flag's over its own, a blank flag none for the run, and with
    /// `--all` as without.
    #[test]
    fn a_run_writes_the_settings_rights_with_the_flags_over_them() {
        let scratch = Scratch::new("rights");
        names(&scratch.0, 2);
        let target = scratch.folder("out");
        let remembered = Shared {
            author: "Jo Doe".into(),
            copyright: "© Jo Doe".into(),
            ..Default::default()
        };
        let all = Request {
            all: true,
            no_sidecars: true,
            ..to(&[&scratch.0], &target)
        };
        let planned = plan(&all, &remembered, None, None).unwrap();
        assert_eq!(planned.frames.len(), 2);
        assert_eq!(planned.settings.rights, remembered.rights());
        assert_eq!(planned.settings.rights.author.as_deref(), Some("Jo Doe"));

        let flagged = Request {
            author: Some(" Zoë Ångström ".into()),
            copyright: Some(String::new()),
            ..all.clone()
        };
        let rights = plan(&flagged, &remembered, None, None)
            .unwrap()
            .settings
            .rights;
        assert_eq!(rights.author.as_deref(), Some("Zoë Ångström"));
        assert_eq!(rights.copyright, None, "a blank flag is none this run");

        let rights = plan(&flagged, &Shared::default(), None, None)
            .unwrap()
            .settings
            .rights;
        assert_eq!(rights.author.as_deref(), Some("Zoë Ångström"));
        assert!(
            plan(&all, &Shared::default(), None, None)
                .unwrap()
                .settings
                .rights
                .is_empty()
        );

        // Metadata None: neither the settings' nor the flags'.
        let mut none = remembered.clone();
        none.export.metadata = "None".into();
        for req in [&all, &flagged] {
            let planned = plan(req, &none, None, None).unwrap();
            assert_eq!(planned.settings.metadata, export::Metadata::None);
            assert!(planned.settings.rights.is_empty());
        }
        // The other choices keep them.
        for choice in ["All", "No edit"] {
            let mut kept = remembered.clone();
            kept.export.metadata = choice.into();
            let rights = plan(&flagged, &kept, None, None).unwrap().settings.rights;
            assert_eq!(rights.author.as_deref(), Some("Zoë Ångström"), "{choice}");
        }
    }

    /// `--also ROWS` into a folder: the frame opened and the rows, in
    /// file order, each under its own name in the folder; the
    /// overrides on the frame opened alone.
    #[test]
    fn a_set_is_the_frame_opened_and_the_rows_each_under_its_own_name() {
        let scratch = Scratch::new("set");
        let files = names(&scratch.0, 4);
        let target = scratch.folder("out");
        // The folder opened, its last file remembered: that one is
        // the frame opened, as the window opens it.
        let remembered = Shared {
            last_file: files[2].to_string_lossy().into_owned(),
            ..Default::default()
        };
        let req = Request {
            also: vec![3, 0],
            overrides: Overrides {
                exposure: Some(1.0),
                ..Overrides::default()
            },
            no_sidecars: true,
            ..to(&[&scratch.0], &target)
        };
        let planned = plan(&req, &remembered, None, None).unwrap();
        let sources: Vec<&Path> = planned.frames.iter().map(|f| f.source.as_path()).collect();
        assert_eq!(sources, [&files[0], &files[2], &files[3]]);
        let outs: Vec<String> = planned
            .frames
            .iter()
            .map(|f| f.out.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(outs, ["IMG_0000.jpg", "IMG_0002.jpg", "IMG_0003.jpg"]);
        let exposures: Vec<f32> = planned
            .frames
            .iter()
            .map(|f| f.edit.light.exposure)
            .collect();
        assert_eq!(exposures, [0.0, 1.0, 0.0]);
        assert_eq!(planned.folder.as_deref(), Some(target.as_path()));

        // The sheet's Name, as the window would open with it, names
        // each file.
        let named = Shared {
            export: Sheet {
                name: "set-{seq}".into(),
                ..Default::default()
            },
            ..remembered
        };
        let req = Request {
            overrides: Overrides::default(),
            ..req
        };
        let planned = plan(&req, &named, None, None).unwrap();
        let outs: Vec<String> = planned
            .frames
            .iter()
            .map(|f| f.out.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(outs, ["set-1.jpg", "set-2.jpg", "set-3.jpg"]);
    }

    /// `--all` into a folder: every frame of the folder, in its order,
    /// the same files `--also` naming the rest of the rows writes, and
    /// the same files the frames named one by one write.
    #[test]
    fn all_exports_every_frame_as_also_naming_each_row_does() {
        let scratch = Scratch::new("all");
        let shoot = scratch.0.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        let frames: Vec<PathBuf> = (0..3)
            .map(|i| shoot.join(format!("IMG_{i:04}.dng")))
            .collect();
        for frame in &frames {
            dng(frame, 48, 32);
        }
        let remembered = Shared::default();
        let written = |name: &str, how: Request| {
            let target = scratch.folder(name);
            let req = Request {
                target: target.clone(),
                no_sidecars: true,
                ..how
            };
            let planned = plan(&req, &remembered, None, None).unwrap();
            let finished = run(planned, lent());
            assert!(finished.succeeded(), "{}", finished.line);
            let mut names: Vec<String> = std::fs::read_dir(&target)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        let folder = to(&[&shoot], Path::new(""));
        let all = written(
            "all",
            Request {
                all: true,
                ..folder.clone()
            },
        );
        assert_eq!(all, ["IMG_0000.jpg", "IMG_0001.jpg", "IMG_0002.jpg"]);
        let also = Request {
            also: vec![1, 2],
            ..folder
        };
        assert_eq!(all, written("also", also));
        let each: Vec<&Path> = frames.iter().map(PathBuf::as_path).collect();
        assert_eq!(all, written("each", to(&each, Path::new(""))));
    }

    /// Several frames named are a set: into a folder, each under its own
    /// edit with the overrides on every one; a folder among them only
    /// with `--all`, and never `--also` or `--preset`.
    #[test]
    fn several_frames_named_are_a_set_with_the_overrides_on_each() {
        let scratch = Scratch::new("several");
        let files = names(&scratch.0, 3);
        let into = scratch.folder("out");
        let remembered = Shared::default();
        let req = Request {
            overrides: Overrides {
                exposure: Some(0.5),
                ..Overrides::default()
            },
            no_sidecars: true,
            ..to(&[&files[2], &files[0], &files[2]], &into)
        };
        let planned = plan(&req, &remembered, None, None).unwrap();
        let sources: Vec<&Path> = planned.frames.iter().map(|f| f.source.as_path()).collect();
        assert_eq!(sources, [&files[2], &files[0]], "in the order named, once");
        assert!(planned.frames.iter().all(|f| f.edit.light.exposure == 0.5));

        // A set has no one file to go to.
        let one = scratch.0.join("one.jpg");
        let e = refused(&to(&[&files[0], &files[1]], &one), &remembered);
        assert!(e.contains("more than one frame names a set"), "{e}");
        // A folder among them, without --all.
        let sub = scratch.0.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let e = refused(&to(&[&files[0], &sub], &into), &remembered);
        assert!(e.contains("is a folder") && e.contains("--all"), "{e}");
        names(&sub, 1);
        let all = Request {
            all: true,
            no_sidecars: true,
            ..to(&[&files[0], &sub], &into)
        };
        assert_eq!(plan(&all, &remembered, None, None).unwrap().frames.len(), 2);
        let e = refused(
            &Request {
                also: vec![1],
                ..to(&[&files[0], &files[1]], &into)
            },
            &remembered,
        );
        assert!(e.contains("--also"), "{e}");
        let e = refused(
            &Request {
                preset: Some("Warm".into()),
                ..to(&[&files[0], &files[1]], &into)
            },
            &remembered,
        );
        assert!(e.starts_with("--preset with more than one frame"), "{e}");
    }

    /// `--all` with one file to open, one file to write, or no path at
    /// all is refused with the reason rather than exporting a set of
    /// one or a folder the command line did not name.
    #[test]
    fn all_with_one_file_is_refused_with_the_reason() {
        let scratch = Scratch::new("all-one");
        let files = names(&scratch.0, 2);
        let remembered = Shared::default();
        let into = scratch.folder("out");
        let all = |paths: &[&Path], target: &Path| Request {
            all: true,
            ..to(paths, target)
        };
        let e = refused(&all(&[&files[0]], &into), &remembered);
        assert!(
            e.starts_with("--all exports every frame of a folder") && e.contains("IMG_0000.CR3"),
            "{e}"
        );
        let one = scratch.0.join("one.jpg");
        let e = refused(&all(&[&scratch.0], &one), &remembered);
        assert!(e.contains("--all names a set"), "{e}");
        // No path: not the remembered folder, though one is remembered.
        let last = Shared {
            last_file: files[1].to_string_lossy().into_owned(),
            ..Default::default()
        };
        assert_eq!(
            refused(&all(&[], &into), &last),
            "--all exports a folder: give its path"
        );
    }

    /// `--preset` writes a step into the history of the frame opened
    /// alone, so with `--all` it is refused and the batch apply named;
    /// the overrides that touch no sidecar go on every frame of the
    /// set, not only the frame opened (the folder's last file here).
    #[test]
    fn all_takes_the_overrides_on_every_frame_and_refuses_a_preset() {
        let scratch = Scratch::new("all-overrides");
        let files = names(&scratch.0, 3);
        let remembered = Shared {
            last_file: files[1].to_string_lossy().into_owned(),
            ..Default::default()
        };
        let into = scratch.folder("out");
        let all = Request {
            all: true,
            ..to(&[&scratch.0], &into)
        };
        let e = refused(
            &Request {
                preset: Some("Warm".into()),
                ..all.clone()
            },
            &remembered,
        );
        assert!(
            e.starts_with("--preset with --all would write a step into one frame's history")
                && e.contains("greycard presets --apply NAME <files>"),
            "{e}"
        );
        let req = Request {
            overrides: Overrides {
                exposure: Some(1.0),
                display_curve: Some(DisplayCurve::Agx),
                ..Overrides::default()
            },
            no_sidecars: true,
            ..all
        };
        let planned = plan(&req, &remembered, None, None).unwrap();
        let sources: Vec<&Path> = planned.frames.iter().map(|f| f.source.as_path()).collect();
        assert_eq!(sources, [&files[0], &files[1], &files[2]]);
        let exposures: Vec<f32> = planned
            .frames
            .iter()
            .map(|f| f.edit.light.exposure)
            .collect();
        assert_eq!(exposures, [1.0, 1.0, 1.0]);
        assert!(
            planned
                .frames
                .iter()
                .all(|f| f.edit.display_curve == DisplayCurve::Agx)
        );
    }

    /// What the request names and the run cannot do is refused before
    /// a frame is begun, with the reason.
    #[test]
    fn what_names_nothing_is_refused_with_the_reason() {
        let scratch = Scratch::new("refused");
        let files = names(&scratch.0, 2);
        let remembered = Shared::default();
        let out = scratch.0.join("one.jpg");
        let e = refused(
            &Request {
                preset: Some("No such preset".into()),
                ..to(&[&files[0]], &out)
            },
            &remembered,
        );
        assert!(e.contains("no preset called No such preset"), "{e}");
        // `--also` with one file's target, as the window refused it.
        let e = refused(
            &Request {
                also: vec![1],
                ..to(&[&scratch.0], &out)
            },
            &remembered,
        );
        assert!(e.contains("--also names a set"), "{e}");
        let e = refused(&to(&[&scratch.0.join("missing.CR3")], &out), &remembered);
        assert!(e.contains("there is no"), "{e}");
    }

    /// One frame's file must name a format an export writes and must
    /// not be a frame: `greycard export a.CR3 b.CR3` would otherwise
    /// write a JPEG over the second raw.
    #[test]
    fn a_file_target_that_is_no_export_format_or_a_frame_is_refused() {
        let scratch = Scratch::new("file-target");
        let files = names(&scratch.0, 2);
        let remembered = Shared::default();
        let e = refused(&to(&[&files[0]], &files[1]), &remembered);
        assert!(
            e.contains("IMG_0001.CR3") && e.contains("names no format an export writes"),
            "{e}"
        );
        // No extension at all, and not a folder: the same.
        let e = refused(&to(&[&files[0]], &scratch.0.join("out")), &remembered);
        assert!(e.contains("names no format"), "{e}");
        // A picture exported over itself, whatever its extension.
        let tif = scratch.0.join("frame.tif");
        greycard_library::fixture::write_frame(&tif, &greycard_library::fixture::R5, 1);
        let e = refused(&to(&[&tif], &tif), &remembered);
        assert!(e.contains("is one of the frames"), "{e}");
        let spelled = scratch.0.join(".").join("frame.tif");
        let e = refused(&to(&[&tif], &spelled), &remembered);
        assert!(e.contains("is one of the frames"), "{e}");
        // Nothing was written beside the raws.
        assert_eq!(std::fs::read(&files[1]).unwrap(), b"");
        assert!(!scratch.0.join("IMG_0001-1.CR3").exists());
    }

    /// `--also` is a set: into a folder, or an error, never a quiet
    /// export of one frame.
    #[test]
    fn also_names_a_set_or_says_why_not() {
        let also = |rows: &[usize], target: &str| Request {
            also: rows.to_vec(),
            ..to(&[], Path::new(target))
        };
        // A file to write: one frame, so --also is an error.
        assert!(!export_folder(Path::new("out.jpg"), true));
        let e = check_also(&also(&[1], "out.jpg"), false, 3)
            .unwrap_err()
            .to_string();
        assert!(e.contains("one frame's file"), "{e}");
        // A path that is not there and names no format: a folder with
        // --also, and the file it always was without.
        let new = std::env::temp_dir().join("greycard-no-such-folder-for-also");
        assert!(export_folder(&new, true));
        assert!(!export_folder(&new, false));
        // A row the folder has not got: an error naming it.
        let rows = also(&[0, 1, 2], "out/");
        let e = check_also(&rows, true, 1).unwrap_err().to_string();
        assert!(e.starts_with("--also 1: there is 1 frame"), "{e}");
        assert!(check_also(&rows, true, 3).is_ok());
        assert!(check_also(&also(&[], "out.jpg"), false, 1).is_ok());
    }

    /// A frame that cannot be decoded is a failure the exit code says,
    /// with the reason on the line the run ends on.
    #[test]
    fn a_frame_that_will_not_decode_fails_the_run_and_says_why() {
        let scratch = Scratch::new("fails");
        let files = names(&scratch.0, 1);
        let out = scratch.0.join("out.jpg");
        let req = Request {
            no_sidecars: true,
            ..to(&[&files[0]], &out)
        };
        let planned = plan(&req, &Shared::default(), None, None).unwrap();
        let finished = run(planned, lent());
        assert!(!finished.succeeded());
        assert_eq!(finished.tally.failed.len(), 1);
        assert!(
            finished.line.starts_with("export to ") && finished.line.contains("failed"),
            "{}",
            finished.line
        );
        assert!(!out.exists());
    }

    /// A version 3 sidecar whose crop keeps the frame's Original shape
    /// is brought up to date from the file before the export, and the
    /// export is noted on the state it was made from: the record reads
    /// the sidecar again, and brings it up to date the same way before
    /// it compares the two.
    #[test]
    fn an_older_build_s_original_crop_has_its_export_noted() {
        let scratch = Scratch::new("v3");
        let source = scratch.0.join("frame.dng");
        dng(&source, 96, 64);
        let gcd = source.with_file_name("frame.dng.gcd");
        let v3 = r#"{"version":3,"light":{"exposure":0.25},"geometry":{"portrait":true,"aspect":{"kind":"original"}}}"#;
        std::fs::write(&gcd, format!(r#"{{"current":{v3},"history":[{v3}]}}"#)).unwrap();
        assert!(Sidecar::load(&source).unwrap().unwrap().needs_frame());
        let out = scratch.0.join("out.png");
        let planned = plan(&to(&[&source], &out), &Shared::default(), None, None).unwrap();
        assert!(!planned.frames[0].edit.needs_frame());
        let finished = run(planned, lent());
        assert_eq!(finished.code(), WRITTEN, "{}", finished.line);
        let kept = Sidecar::load(&source).unwrap().unwrap();
        assert!(!kept.needs_frame(), "written back up to date");
        assert_eq!(kept.current.light.exposure, 0.25);
        assert_eq!(kept.current_exports.len(), 1, "the export is noted");
    }

    /// A frame whose sidecar carries a capture time shift exports with
    /// the moved time in its EXIF, read back by the engine's own
    /// reader, and the `.xmp` beside it says the moved time too; the
    /// frame's own file is never rewritten.
    #[test]
    fn a_shifted_frame_exports_its_corrected_capture_time() {
        use greycard_library::fixture::{R5, write_frame};
        let scratch = Scratch::new("shifted");
        let source = scratch.0.join("frame.tif");
        // Taken 2024:08:24 15:32:39 by its clock, five minutes slow.
        write_frame(&source, &R5, 1);
        let before = std::fs::read(&source).unwrap();
        let mut sidecar = Sidecar {
            line: greycard_edit::Line::of(Edit::for_picture()),
            ..Sidecar::default()
        };
        sidecar.meta.set_time_shift(300);
        sidecar
            .save_in(&source, greycard_edit::Placement::Beside)
            .unwrap();
        let out = scratch.0.join("out.jpg");
        let req = Request {
            xmp_sidecars: true,
            ..to(&[&source], &out)
        };
        let planned = plan(&req, &Shared::default(), None, None).unwrap();
        assert_eq!(planned.frames[0].time_shift, 300);
        assert!(run(planned, lent()).succeeded());
        let taken = |path: &Path| greycard_core::picture::probe_path(path).unwrap().taken;
        assert_eq!(taken(&out).as_deref(), Some("2024:08:24 15:37:39"));
        assert_eq!(taken(&source).as_deref(), Some("2024:08:24 15:32:39"));
        assert_eq!(std::fs::read(&source).unwrap(), before, "never rewritten");
        let xmp = std::fs::read_to_string(scratch.0.join("frame.xmp")).expect("the .xmp");
        assert!(
            xmp.contains("<exif:DateTimeOriginal>2024-08-24T15:37:39</exif:DateTimeOriginal>"),
            "{xmp}"
        );
        assert!(
            xmp.contains("<greycard:TimeShift>300</greycard:TimeShift>"),
            "{xmp}"
        );
    }

    /// `--xmp-sidecars`: the frame's `.xmp` is written with the sidecar
    /// that notes the export, as every sidecar write of the window's
    /// writes it; without it, none.
    #[test]
    fn the_xmp_follows_the_sidecar_as_the_window_writes_it() {
        let scratch = Scratch::new("xmp");
        let source = scratch.0.join("frame.dng");
        dng(&source, 96, 64);
        let mut sidecar = Sidecar::default();
        sidecar.meta.rating = 4;
        sidecar.current.light.exposure = 0.1;
        sidecar
            .save_in(&source, greycard_edit::Placement::Beside)
            .unwrap();
        let xmp = scratch.0.join("frame.xmp");
        let export = |xmp_sidecars: bool, out: &Path| {
            let req = Request {
                xmp_sidecars,
                ..to(&[&source], out)
            };
            run(plan(&req, &Shared::default(), None, None).unwrap(), lent()).code()
        };
        assert_eq!(export(false, &scratch.0.join("a.png")), WRITTEN);
        assert!(!xmp.exists(), "no .xmp unless asked");
        assert_eq!(export(true, &scratch.0.join("b.png")), WRITTEN);
        let written = std::fs::read_to_string(&xmp).expect("the .xmp is written");
        assert!(written.contains("Rating"), "{written}");
        assert!(Sidecar::load(&source).unwrap().unwrap().xmp.is_some());
    }

    /// The exit code: 0 when every frame went as its edit asked, 2 when
    /// every frame went but some without something their edit asked
    /// for, 1 when one did not go.
    #[test]
    fn the_exit_code_tells_written_from_left_out_from_failed() {
        let finished = |failed: usize, left_out: usize| Finished {
            tally: queue::Tally {
                exported: 2,
                failed: vec![("a".into(), "why".into()); failed],
                ..Default::default()
            },
            left_out,
            line: String::new(),
        };
        assert_eq!(finished(0, 0).code(), 0);
        assert_eq!(finished(0, 1).code(), 2);
        assert_eq!(finished(1, 1).code(), 1);
        assert_eq!(finished(1, 0).code(), 1);
    }

    /// An `.xmp` another tool changed since the sidecar last took its
    /// word (a rating, a turn) is read as an open reads it before the
    /// export is noted, so the `.xmp` written with the note says what
    /// the other tool said and not the `.gcd`'s stale word, and the
    /// `.gcd` takes it.
    #[test]
    fn an_xmp_another_tool_changed_is_taken_and_not_written_over() {
        let scratch = Scratch::new("xmp-changed");
        let source = scratch.0.join("frame.dng");
        dng(&source, 96, 64);
        let mut sidecar = Sidecar::default();
        sidecar.meta.rating = 2;
        sidecar.current.light.exposure = 0.1;
        sidecar
            .save_in(&source, greycard_edit::Placement::Beside)
            .unwrap();
        let xmp = scratch.0.join("frame.xmp");
        let export = |out: &Path| {
            let req = Request {
                xmp_sidecars: true,
                ..to(&[&source], out)
            };
            run(plan(&req, &Shared::default(), None, None).unwrap(), lent()).code()
        };
        assert_eq!(export(&scratch.0.join("a.png")), WRITTEN);
        let written = std::fs::read_to_string(&xmp).unwrap();
        // Another tool's edit: five stars, and the frame turned a
        // quarter clockwise (EXIF 6 over the file's own 1).
        let edited = written
            .replace(r#"xmp:Rating="2""#, r#"xmp:Rating="5""#)
            .replace("<xmp:Rating>2</xmp:Rating>", "<xmp:Rating>5</xmp:Rating>")
            .replace(r#"tiff:Orientation="1""#, r#"tiff:Orientation="6""#)
            .replace(
                "<tiff:Orientation>1</tiff:Orientation>",
                "<tiff:Orientation>6</tiff:Orientation>",
            );
        assert!(edited.contains("5") && edited.contains("6"), "{written}");
        assert_ne!(edited, written, "{written}");
        std::fs::write(&xmp, &edited).unwrap();

        assert_eq!(export(&scratch.0.join("b.png")), WRITTEN);
        let after = std::fs::read_to_string(&xmp).unwrap();
        let rating = |text: &str| {
            text.contains(r#"xmp:Rating="5""#) || text.contains("<xmp:Rating>5</xmp:Rating>")
        };
        let turned = |text: &str| {
            text.contains(r#"tiff:Orientation="6""#)
                || text.contains("<tiff:Orientation>6</tiff:Orientation>")
        };
        assert!(rating(&after), "the other tool's rating stands:\n{after}");
        assert!(turned(&after), "the other tool's turn stands:\n{after}");
        let kept = Sidecar::load(&source).unwrap().unwrap();
        assert_eq!(kept.meta.rating, 5, "the .gcd takes it");
        assert_eq!(kept.turn, 1, "and the turn");
        assert_eq!(kept.current_exports.len(), 2, "both exports noted");
    }

    /// A look the edit names that the look directory has not got is
    /// left out of the file as a missing model is, and counts the
    /// same: exit 2, so a parity run can trust 0. At no strength it
    /// changes nothing, and is not counted.
    #[test]
    fn a_look_this_machine_has_not_got_counts_as_left_out() {
        let scratch = Scratch::new("look");
        let source = scratch.0.join("frame.dng");
        dng(&source, 96, 64);
        let export = |strength: f32, out: &str| {
            let mut sidecar = Sidecar::default();
            sidecar.current.look_lut.lut =
                greycard_edit::look::LutChoice::Named("No such look anywhere".into());
            sidecar.current.look_lut.strength = strength;
            sidecar
                .save_in(&source, greycard_edit::Placement::Beside)
                .unwrap();
            let out = scratch.0.join(out);
            let finished = run(
                plan(&to(&[&source], &out), &Shared::default(), None, None).unwrap(),
                lent(),
            );
            assert!(out.is_file());
            finished
        };
        let finished = export(1.0, "a.png");
        assert_eq!(finished.code(), LEFT_OUT, "{}", finished.line);
        assert!(finished.line.contains(MISSING), "{}", finished.line);
        assert_eq!(export(0.0, "b.png").code(), WRITTEN);
    }

    /// `--preset` with `--version-name`: the preset goes on the version
    /// named, is exported under it, and version 0 gains no step.
    #[test]
    fn a_preset_at_start_lays_on_the_version_named() {
        let scratch = Scratch::new("version-preset");
        let source = scratch.0.join("frame.dng");
        dng(&source, 96, 64);
        let mut sidecar = Sidecar::default();
        sidecar.current.light.exposure = -0.3;
        let id = sidecar.new_version(None, Some("Warm")).unwrap();
        sidecar
            .save_in(&source, greycard_edit::Placement::Beside)
            .unwrap();
        let mut bright = Edit::default();
        bright.light.exposure = 1.1;
        let store = greycard_edit::preset::Store::at(scratch.0.join("presets"));
        store
            .save(&Preset::from_edit(
                "Bright",
                &bright,
                &[greycard_edit::Section::Light],
            ))
            .unwrap();
        let out = scratch.0.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let req = Request {
            version_name: Some("Warm".into()),
            preset: Some("Bright".into()),
            ..to(&[&source], &out)
        };
        let planned = plan(&req, &Shared::default(), Some(&store), None).unwrap();
        assert_eq!(
            planned.frames[0].edit.light.exposure, 1.1,
            "the preset, on the version"
        );
        let back = Sidecar::load(&source).unwrap().unwrap();
        assert_eq!(back.current.light.exposure, -0.3, "version 0 as it was");
        assert!(back.history.is_empty(), "no step on version 0");
        let line = back.line(Some(&id)).unwrap();
        assert_eq!(line.current.light.exposure, 1.1);
        assert_eq!(line.current_label.as_deref(), Some("Preset: Bright"));
    }

    /// A Bayer frame on disk, `width` by `height`, with a lateral
    /// chromatic aberration for the CA to correct.
    fn mosaic(path: &Path, width: usize, height: usize) {
        use greycard_core::merge::{Noise, testing};
        let samples = testing::mosaic_of(width, height, 0.6, Noise::ASSUMED, 7, |x, y, c| {
            let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
            let scale = [1.004, 1.0, 0.996][c];
            let (sx, sy) = (cx + (x as f32 - cx) / scale, cy + (y as f32 - cy) / scale);
            testing::scene(sx as usize, sy as usize, c, width, height) * 0.3
                + 0.1 * ((sx * 0.21).sin() * (sy * 0.17).cos() + 1.0)
        });
        testing::write_dng(path, samples, width, height, (1, 100), "Mosaic");
    }

    /// The point of a device for the command line: on one machine, a
    /// mosaic exported with no window is the file the window writes
    /// of the frame it has open (its viewport's base reused, its CA
    /// and demosaic on the window's device), pixel for pixel; with no
    /// device lent (`--cpu`) the run says so, and its file is the CPU's,
    /// within the bound the GPU's demosaic is held to against it.
    #[test]
    fn a_mosaic_exports_alike_from_the_window_and_the_command_line() {
        exports_alike("mosaic", |_| {});
    }

    /// The same with the Detail section, the dehaze, and the sharpen
    /// on, which an export lent a device runs there too and reads back
    /// in full floats: the window's file is still the command line's,
    /// and both are the CPU's within the same bound.
    #[test]
    fn a_mosaic_with_detail_dehaze_and_sharpen_exports_alike() {
        exports_alike("mosaic-detail", |edit| {
            edit.detail.texture = 0.3;
            edit.detail.clarity = 0.4;
            edit.detail.structure = 0.2;
            edit.detail.dehaze = 0.25;
            edit.sharpen.iterations = 30;
        });
    }

    /// A mask over the whole frame whose Detail has a Sharpen, which
    /// reads the sharpen's delta, with the global sharpen on (the
    /// delta beside the sharpened picture) or off (the delta alone, the
    /// picture read back unsharpened): the window's file is still the
    /// command line's, and another file than the frame's without the
    /// mask, so the mask's Sharpen took.
    #[test]
    fn a_mask_s_sharpen_exports_alike_with_the_global_sharpen_on_and_off() {
        for global in [true, false] {
            let plain = exports_alike(
                if global {
                    "mask-sharpen-plain-on"
                } else {
                    "mask-sharpen-plain-off"
                },
                |edit| edit.sharpen.enabled = global,
            );
            let masked = exports_alike(
                if global {
                    "mask-sharpen-on"
                } else {
                    "mask-sharpen-off"
                },
                |edit| {
                    edit.sharpen.enabled = global;
                    let mut a = greycard_edit::Adjustment {
                        id: 1,
                        ..Default::default()
                    };
                    a.mask.components.push(greycard_edit::mask::Component {
                        shape: greycard_edit::mask::Shape::Linear {
                            from: [0.5, 10.0],
                            to: [0.5, 11.0],
                        },
                        ..Default::default()
                    });
                    a.look.detail.sharpen = 0.8;
                    edit.adjustments.push(a);
                },
            );
            assert!(
                masked != plain,
                "the mask's Sharpen changed nothing (global sharpen {global})"
            );
        }
    }

    /// [`a_mosaic_exports_alike_from_the_window_and_the_command_line`]'s
    /// check, on the edit `with` makes of the frame's: the file's
    /// pixels, which both exports wrote.
    fn exports_alike(name: &str, with: impl Fn(&mut Edit)) -> Rgb16 {
        let on_device = match lent() {
            Ok(ctx) => {
                println!("{}", lent_line(&Ok(ctx)));
                true
            }
            Err(why) => {
                assert!(
                    std::env::var_os("GREYCARD_REQUIRE_GPU").is_none_or(|v| v.is_empty()),
                    "no device for the export ({why}) and GREYCARD_REQUIRE_GPU is set"
                );
                false
            }
        };
        let scratch = Scratch::new(name);
        let source = scratch.0.join("frame.dng");
        mosaic(&source, 480, 320);
        let out = scratch.0.join("headless.tif");
        let req = Request {
            no_sidecars: true,
            ..to(&[&source], &out)
        };
        let remembered = Shared::default();
        let mut plan = plan(&req, &remembered, None, None).unwrap();
        plan.frames.iter_mut().for_each(|f| with(&mut f.edit));
        let window = queue::Frame {
            out: scratch.0.join("window.tif"),
            ..plan.frames[0].clone()
        };
        let settings = plan.settings.clone();
        let windowed = window_export(&window, settings).expect("the window wrote it");
        let finished = run(plan, lent());
        assert!(finished.succeeded(), "{}", finished.line);
        let pixels = |p: &Path| image::open(p).unwrap().to_rgb16();
        let (a, b) = (pixels(&windowed), pixels(&out));
        assert_eq!(a.dimensions(), b.dimensions());
        assert!(a == b, "the window's export and the command line's differ");

        let cpu_out = scratch.0.join("cpu.tif");
        let mut cpu_plan = super::plan(&to(&[&source], &cpu_out), &remembered, None, None).unwrap();
        cpu_plan.frames.iter_mut().for_each(|f| with(&mut f.edit));
        let none: Lent = Err("--cpu".into());
        assert_eq!(lent_line(&none), "export: on the CPU (--cpu)");
        let finished = run(cpu_plan, none);
        assert!(finished.succeeded(), "{}", finished.line);
        let c = pixels(&cpu_out);
        assert_eq!(a.dimensions(), c.dimensions());
        if !on_device {
            assert!(a == c);
            return a;
        }
        // The device ran: its demosaic is not the CPU's to the bit.
        assert!(a != c, "the device's export is the CPU's: did the GPU run?");
        // The GPU's demosaic against the CPU's, as the demosaic's own
        // real-frame test bounds it: no sample past a tenth of the
        // frame's largest value, and at most one in ten thousand past a
        // thousandth of it.
        let apart = Apart::of(&a, &c);
        println!("the device's export against the CPU's: {apart}");
        assert!(apart.most <= apart.largest / 10, "{apart}");
        assert!(apart.past * 10_000 <= apart.samples, "{apart}");
        a
    }

    type Rgb16 = image::ImageBuffer<image::Rgb<u16>, Vec<u16>>;

    /// How far a device's 16-bit file is from the CPU's: its largest
    /// difference, the CPU's largest value, and how many of its samples
    /// are past a thousandth of that.
    struct Apart {
        most: u16,
        largest: u16,
        past: usize,
        samples: usize,
    }

    impl Apart {
        fn of(device: &Rgb16, cpu: &Rgb16) -> Self {
            let largest = *cpu.as_raw().iter().max().unwrap();
            let mut most = 0;
            let mut past = 0;
            for (x, y) in device.as_raw().iter().zip(cpu.as_raw()) {
                let d = x.abs_diff(*y);
                most = most.max(d);
                past += usize::from(f64::from(d) > f64::from(largest) / 1000.0);
            }
            Self {
                most,
                largest,
                past,
                samples: cpu.as_raw().len(),
            }
        }
    }

    impl std::fmt::Display for Apart {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "at most {} of {}, {} of {} past a thousandth",
                self.most, self.largest, self.past, self.samples
            )
        }
    }

    /// The device's export against the CPU's on every raw in the folder
    /// `GREYCARD_SAMPLES` names, with the sharpen off, with the frame's
    /// default edit (its sharpen on), and with the Detail section, the
    /// dehaze, and the sharpen on, in the
    /// measure and bound [`exports_alike`] holds a mosaic to. Nothing is
    /// read or written beside the raws.
    #[test]
    #[ignore = "reads the folder named by GREYCARD_SAMPLES"]
    fn the_device_s_export_against_the_cpu_s_on_the_samples() {
        let Some(dir) = std::env::var_os("GREYCARD_SAMPLES").map(PathBuf::from) else {
            eprintln!("set GREYCARD_SAMPLES to a folder of raws");
            return;
        };
        let raw = |p: &Path| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                [
                    "cr2", "cr3", "nef", "arw", "dng", "raf", "orf", "rw2", "pef",
                ]
                .contains(&e.to_ascii_lowercase().as_str())
            })
        };
        let mut frames: Vec<PathBuf> = std::fs::read_dir(&dir)
            .expect("read GREYCARD_SAMPLES")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && raw(p))
            .collect();
        frames.sort();
        assert!(!frames.is_empty(), "no raw in GREYCARD_SAMPLES");
        let scratch = Scratch::new("samples");
        let detail = |edit: &mut Edit| {
            edit.detail.texture = 0.3;
            edit.detail.clarity = 0.4;
            edit.detail.structure = 0.2;
            edit.detail.dehaze = 0.25;
            edit.sharpen.iterations = 30;
        };
        // The sharpen off: the CA, the demosaic, and the lens alone on
        // the device, as an export had them before the Detail section
        // and the sharpen joined them.
        let bare = |edit: &mut Edit| edit.sharpen.enabled = false;
        let plain = |_: &mut Edit| {};
        type With<'a> = &'a dyn Fn(&mut Edit);
        let edits: [(&str, With); 3] = [
            ("sharpen off", &bare),
            ("default", &plain),
            ("detail", &detail),
        ];
        for frame in &frames {
            let name = frame.file_name().unwrap().to_string_lossy().into_owned();
            for (what, with) in edits {
                let export = |lent: Lent, out: &Path| {
                    let req = Request {
                        no_sidecars: true,
                        ..to(&[frame.as_path()], out)
                    };
                    let mut plan = plan(&req, &Shared::default(), None, None).unwrap();
                    plan.frames.iter_mut().for_each(|f| with(&mut f.edit));
                    let started = std::time::Instant::now();
                    let finished = run(plan, lent);
                    assert!(finished.succeeded(), "{}", finished.line);
                    // A 45 MP file is past the reader's default limits.
                    let mut reader = image::ImageReader::open(out).unwrap();
                    reader.no_limits();
                    let pixels = reader.decode().unwrap().to_rgb16();
                    std::fs::remove_file(out).unwrap();
                    (pixels, started.elapsed().as_secs_f64())
                };
                let (device, on) = export(lent(), &scratch.0.join("device.tif"));
                let (cpu, off) = export(Err("--cpu".into()), &scratch.0.join("cpu.tif"));
                let apart = Apart::of(&device, &cpu);
                println!("{name} {what}: {apart} (device {on:.1} s, CPU {off:.1} s)");
                // The mosaic's bound, which the demosaic's own
                // real-frame test sets.
                assert!(apart.most <= apart.largest / 10, "{name} {what}: {apart}");
                assert!(
                    apart.past * 10_000 <= apart.samples,
                    "{name} {what}: {apart}"
                );
            }
        }
    }

    /// A sixteen-bit PNG, sRGB with no profile, of a soft gradient and a
    /// grey patch: a picture that is not a raw.
    fn png(path: &Path, width: u32, height: u32) {
        let px = image::ImageBuffer::from_fn(width, height, |x, y| {
            if x < width / 4 && y < height / 4 {
                image::Rgb([30000u16, 30000, 30000])
            } else {
                image::Rgb([
                    (x * 65535 / width) as u16,
                    (y * 65535 / height) as u16,
                    ((x + y) * 65535 / (width + height)) as u16,
                ])
            }
        });
        image::DynamicImage::ImageRgb16(px).save(path).unwrap();
    }

    /// The identity on a picture with a white balance offset: the
    /// window's export and the command line's are one file but for the
    /// moment of writing, and the offset reached both, the grey patch
    /// warmer than the picture at As shot makes it.
    #[test]
    fn a_pictures_offset_exports_alike_from_the_window_and_the_command_line() {
        let scratch = Scratch::new("picture-white");
        let source = scratch.0.join("frame.png");
        png(&source, 240, 160);
        let mut sidecar = Sidecar {
            line: greycard_edit::Line::of(Edit::for_picture()),
            ..Sidecar::default()
        };
        sidecar.current.white_balance = WhiteBalance::Offset {
            mired: 40.0,
            tint: 0.002,
        };
        sidecar
            .save_in(&source, greycard_edit::Placement::Beside)
            .unwrap();
        let out = scratch.0.join("headless.tif");
        let req = to(&[&source], &out);
        let remembered = Shared::default();
        let plan = plan(&req, &remembered, None, None).unwrap();
        assert_eq!(
            plan.frames[0].edit.white_balance,
            sidecar.current.white_balance
        );
        let window = queue::Frame {
            out: scratch.0.join("window.tif"),
            ..plan.frames[0].clone()
        };
        let settings = plan.settings.clone();
        let windowed = window_export(&window, settings.clone()).expect("the window wrote it");
        let finished = run(plan, lent());
        assert!(finished.succeeded(), "{}", finished.line);
        let (a, b) = (
            std::fs::read(&windowed).unwrap(),
            std::fs::read(&out).unwrap(),
        );
        let pixels = |p: &Path| image::open(p).unwrap().to_rgb16();
        assert!(pixels(&windowed) == pixels(&out), "the pixels differ");
        assert!(
            differ_only_in_dates(&a, &b),
            "bytes differ outside the dates"
        );

        // The same picture at As shot: the patch is grey there and warm
        // under the offset.
        let shot = queue::Frame {
            out: scratch.0.join("shot.tif"),
            edit: Edit::for_picture(),
            ..window.clone()
        };
        let shot = window_export(&shot, settings).expect("written");
        let at = |p: &Path| *pixels(p).get_pixel(10, 10);
        let (g, w) = (at(&shot), at(&out));
        assert!(g[0].abs_diff(g[2]) < 300, "{g:?}");
        assert!(w[0] > g[0] && w[2] < g[2], "{w:?} against {g:?}");
    }
}
