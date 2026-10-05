//! `--export` with no window: the frames the command line names, each
//! under its sidecar's edit with the command line's overrides laid on
//! top, sent to the worker as a set, and the process waiting on the
//! set's outcome.
//!
//! The export never needed the window. The worker writes a frame on
//! the CPU reference path whatever device the window lent it (an
//! export develops with no GPU, and a base whose CA the GPU corrected
//! is made again on the CPU), so the window's part was only to read
//! the sidecar, lay the overrides on it, fill the sheet and wait. All
//! of that is here, from the same functions the window used, and no
//! wgpu device is asked for: there is nothing for one to do.

use crate::panel::startup::{Overrides, check_also, export_folder, opening_sheet, preset_at_start};
use crate::*;

/// What an `--export` run writes: each frame under its edit, and the
/// sheet the set is written under.
pub(crate) struct Plan {
    pub(crate) frames: Vec<queue::Frame>,
    /// `--export DIR`: the folder the frames go into. None for
    /// `--export FILE`, one frame to the path given.
    pub(crate) folder: Option<PathBuf>,
    pub(crate) settings: export::Settings,
    pub(crate) on_exists: export::OnExists,
    /// The export preset the sheet is as saved, for each frame's
    /// history; none when it is not one.
    pub(crate) preset: Option<String>,
    /// Where each frame's sidecar is written with the export noted in
    /// its history; none with sidecars off.
    pub(crate) placement: Option<greycard_edit::Placement>,
    /// Whether that write writes the frame's `.xmp` too, as the
    /// window's every sidecar write does (`--xmp-sidecars`, or the
    /// Settings sheet's choice).
    pub(crate) xmp: bool,
    /// The user's library index, when the run may note in it the
    /// archive copies its sidecar writes leave waiting (§233): the
    /// editor's own, never one named on the command line, and None
    /// in a test. It is opened only if it is there and at this
    /// build's schema; a run never makes, migrates or rebuilds it.
    pub(crate) index: Option<PathBuf>,
}

/// What came of a run.
pub(crate) struct Finished {
    pub(crate) tally: queue::Tally,
    /// The frames written without something their edit names: a look
    /// not in the look directory, a learned model not downloaded or
    /// failing.
    pub(crate) left_out: usize,
    /// The line the run ends on, for whoever ran it.
    pub(crate) line: String,
}

/// The exit codes: every frame written as its edit asks (or left, as
/// `--on-exists skip` asks); a frame not written, or a run that could
/// not begin; every frame written, but some without something their
/// edit names that this machine has not got (a look, a learned model)
/// or could not run.
pub(crate) const WRITTEN: u8 = 0;
pub(crate) const FAILED: u8 = 1;
pub(crate) const LEFT_OUT: u8 = 2;

/// What a frame counted toward [`LEFT_OUT`] went without, in the run's
/// last line.
const MISSING: &str = "something its edit names that this machine has not got";

impl Finished {
    /// Whether every frame went: one that failed, or a set that never
    /// finished, is a failure a script can see. A file there already
    /// and left alone, as `--on-exists skip` asks, is not.
    pub(crate) fn succeeded(&self) -> bool {
        self.tally.failed.is_empty() && self.tally.canceled == 0
    }

    /// The process's exit code for the run.
    pub(crate) fn code(&self) -> u8 {
        if !self.succeeded() {
            FAILED
        } else if self.left_out > 0 {
            LEFT_OUT
        } else {
            WRITTEN
        }
    }
}

/// `--export TARGET`, from the command line to the exit code.
pub(crate) fn export(cli: &Cli, target: &Path) -> Result<std::process::ExitCode> {
    let remembered = settings::Settings::load();
    let presets = preset::Store::user();
    let plan = plan(
        cli,
        target,
        &remembered,
        presets.as_ref(),
        greycard_library::Library::user_path(),
    )?;
    let finished = run(plan);
    // The log is a file, and the terminal shows warnings up: the one
    // line a batch caller wants is said here whichever way it went.
    eprintln!("{}", finished.line);
    leave(finished.code())
}

/// End the process with `code`, the run's work all done and said.
///
/// On unix by `_exit`, which skips the C library's exit handlers and
/// the static destructors behind them. A run whose edit asked for a
/// learned mask, a fill or the learned denoiser has loaded ONNX
/// Runtime's WebGPU provider, Dawn, and Dawn's Vulkan instance is torn
/// down by a static destructor that dies in libvulkan on the way out:
/// SIGSEGV, exit 139, after the file was written. The window leaves
/// the same way and always has; a batch caller reads the exit code,
/// and must not read a written file as a crash. Nothing is lost by
/// skipping them: the worker has been stopped and joined, every file
/// written is closed, the log's file and stderr are written
/// unbuffered, and stdout and the C library's streams are flushed
/// first. On other systems the process returns as it otherwise
/// would; the teardown has not been seen to crash there, and is
/// unchecked.
fn leave(code: u8) -> Result<std::process::ExitCode> {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    #[cfg(unix)]
    // SAFETY: `fflush(NULL)` flushes every C stdio stream, so a C or
    // C++ library's buffered output is not lost with the exit
    // handlers that would have flushed it; `_exit` then ends the
    // process at once and returns nothing to undo. Every thread with
    // work in hand has been joined above.
    unsafe {
        libc::fflush(std::ptr::null_mut());
        libc::_exit(i32::from(code))
    }
    #[cfg(not(unix))]
    Ok(std::process::ExitCode::from(code))
}

/// What `cli` asks to be written to `target`, with `remembered` the
/// settings file's sheet and presets and `presets` the store a
/// `--preset` is looked up in. An error for anything the command line
/// names that cannot be done, before a frame is begun.
pub(crate) fn plan(
    cli: &Cli,
    target: &Path,
    remembered: &settings::Settings,
    presets: Option<&preset::Store>,
    index: Option<PathBuf>,
) -> Result<Plan> {
    refuse_window_flags(cli)?;
    // The files, and which of them is the one opened: as the window
    // opens them, the command line's file, or a folder's last file
    // when the settings remember one in it, else its first.
    let last_file = (!remembered.last_file.is_empty())
        .then(|| PathBuf::from(&remembered.last_file))
        .filter(|p| p.is_file());
    let (files, first) = match &cli.path {
        Some(path) => {
            let files = files::list_files(path)?;
            anyhow::ensure!(!files.is_empty(), "no RAW files at {}", path.display());
            let first = if path.is_dir() {
                position_of(&files, last_file.as_deref())
            } else {
                0
            };
            (files, first)
        }
        None => {
            let dir = last_file.as_deref().and_then(Path::parent).context(
                "--export wants a file or a folder to export from, and no last file is remembered",
            )?;
            let files = files::list_files(dir)?;
            anyhow::ensure!(!files.is_empty(), "no RAW files at {}", dir.display());
            let first = position_of(&files, last_file.as_deref());
            (files, first)
        }
    };
    let into_folder = export_folder(target, !cli.also.is_empty());
    check_also(cli, into_folder, files.len())?;

    let write_sidecars = !cli.no_sidecars;
    let placement = if cli.sidecar_folder || remembered.sidecars_in_folder {
        greycard_edit::Placement::Folder
    } else {
        greycard_edit::Placement::Beside
    };
    let preset = match &cli.preset {
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
    let overrides = Overrides::of(cli);

    // The sheet as the window would open with it, and the preset it
    // still is as saved.
    let (sheet, chosen) = opening_sheet(cli, remembered)?;
    let sheet_settings = sheet.settings();
    let preset_name = chosen
        .filter(|n| !sheet::reserved(n))
        .and_then(|n| sheet::find(&remembered.export_presets, &n))
        .filter(|p| p.sheet.same(&sheet))
        .map(|p| p.name.clone());

    // The set: the frame opened and the `--also` rows, in file order.
    let mut rows: Vec<usize> = std::iter::once(first)
        .chain(if into_folder {
            cli.also.clone()
        } else {
            Vec::new()
        })
        .collect();
    rows.sort_unstable();
    rows.dedup();

    let mut frames = Vec::with_capacity(rows.len());
    for &i in &rows {
        let source = files[i].clone();
        let held = crate::rows::from_disk(&source, write_sidecars);
        let unreadable = matches!(held.trouble, Some(crate::rows::Trouble::Unreadable(_)));
        let mut sidecar = held.sidecar;
        let mut seed = held.seed;
        if i == first
            && let Some(preset) = &preset
        {
            // Onto the sidecar and written, as the window lays it; not
            // over a sidecar that would not read.
            let placement = (write_sidecars && !unreadable).then_some(placement);
            preset_at_start(&mut sidecar, &mut seed, &source, preset, placement);
        }
        crate::files::migrate_from_file(&mut sidecar, &source);
        let mut edit = sidecar.current.clone();
        // The command line's temperature, exposure and curve, over the
        // first frame's edit and never onto its sidecar.
        if i == first {
            overrides.apply(&mut edit);
        }
        frames.push(queue::Frame {
            source,
            edit,
            turn: sidecar.turn,
            seed_blend: seed,
            out: PathBuf::new(),
        });
    }

    let (folder, settings, preset_name) = if into_folder {
        let sources: Vec<PathBuf> = frames.iter().map(|f| f.source.clone()).collect();
        for (frame, out) in frames.iter_mut().zip(queue::names(
            &sources,
            Some(target),
            None,
            sheet_settings.format,
        )) {
            frame.out = out;
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
        xmp: cli.xmp_sidecars || remembered.xmp_sidecars,
        index,
    })
}

/// Write `plan` on a worker of its own and wait for the set: each
/// frame's export noted in its sidecar's history as it lands, as the
/// window notes it.
pub(crate) fn run(plan: Plan) -> Finished {
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
    if let Some(dir) = &folder
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
    let total = frames.len();
    let single = folder.is_none().then(|| frames[0].out.clone());
    let set = Arc::new(queue::Set::new(total, folder, settings, on_exists).with_preset(preset));
    tracing::info!(
        "exporting {} with no window, on the CPU reference path: {}",
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
    let worker = Worker::batched(move |outcome| {
        let _ = tx.send(outcome);
    });
    worker.send(Job::ExportSet {
        set: set.clone(),
        frames,
    });
    let mut written = Vec::new();
    let mut left_out = 0;
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
                        if let Some(placement) = placement {
                            let exported = greycard_edit::Exported {
                                file: path.to_string_lossy().into_owned(),
                                preset: set.preset.clone(),
                                at: crate::panel::history::now(),
                            };
                            if crate::panel::history::record_export_on_disk(
                                &source, &edit, exported, placement, xmp,
                            ) && let Some((index, roots)) = archive_roots.as_ref()
                            {
                                // The copy on the archive is not
                                // written here: noted as waiting, for
                                // the next window's catch-up (§233).
                                crate::sync::note_disk_save(index, roots, &source);
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
    match (left_out, &single) {
        (0, _) => {}
        (_, Some(_)) => line.push_str(&format!(", without {MISSING} (see above)")),
        (n, None) => line.push_str(&format!(", {n} without {MISSING} (see above)")),
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

/// The flags that ask the window for something an export run has no
/// window to give, refused rather than passed over: a capture of it;
/// the browser's filter and roots, which would decide which frames
/// the rows name; the index (`--library`), which the run reads only
/// to note the archive copies its records leave waiting (§233), never
/// one named on the command line; a turn pressed on the open frame and a
/// temperature put on the panel alone, both for a capture, which
/// change nothing an export writes (`--develop-temperature` is the
/// one that does).
fn refuse_window_flags(cli: &Cli) -> Result<()> {
    let asked: Vec<&str> = [
        (cli.snapshot.is_some(), "--snapshot"),
        (cli.screenshot.is_some(), "--screenshot"),
        (cli.filter.is_some(), "--filter"),
        (!cli.roots.is_empty(), "--roots"),
        (cli.all_roots, "--all-roots"),
        (cli.import.is_some(), "--import"),
        (cli.turn.is_some(), "--turn"),
        (cli.preview_temperature.is_some(), "--preview-temperature"),
        (cli.library.is_some(), "--library"),
    ]
    .into_iter()
    .filter_map(|(on, flag)| on.then_some(flag))
    .collect();
    anyhow::ensure!(
        asked.is_empty(),
        "--export runs with no window, and {} {} the window's: run {} on its own",
        asked.join(", "),
        if asked.len() == 1 { "is" } else { "are" },
        if asked.len() == 1 { "it" } else { "them" }
    );
    Ok(())
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
pub(crate) fn batch_settings(path: &Path, sheet: export::Settings) -> export::Settings {
    export::Settings {
        format: export::Format::from_path(path).unwrap_or(sheet.format),
        ..sheet
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A folder of the test's own under the workspace's target, gone
    /// when the test is, however it ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/test-scratch")
                .join(format!("headless-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn cli(args: &[&std::ffi::OsStr]) -> Cli {
        let mut all: Vec<&std::ffi::OsStr> = vec!["greycard-ui".as_ref()];
        all.extend_from_slice(args);
        Cli::try_parse_from(all).unwrap()
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
            width,
            height,
            channels: 1,
            layout: SensorLayout::Cfa(CfaPattern::rggb()),
            samples: Samples::U16(Vec::new()),
            levels: Levels {
                black: LevelPattern::uniform(0.0, 1),
                white: LevelPattern::uniform(65535.0, 1),
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

    /// A device as the window's is, on the high-performance adapter;
    /// none without one.
    fn device() -> Option<(greycard_gpu::wgpu::Device, greycard_gpu::wgpu::Queue)> {
        use greycard_gpu::wgpu;
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .ok()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("the window's, in a test"),
            required_limits: wgpu::Limits {
                max_texture_dimension_2d: 16384.min(adapter.limits().max_texture_dimension_2d),
                ..wgpu::Limits::default()
            },
            ..Default::default()
        }))
        .ok()
    }

    /// What the window did for `--export FILE` before the export had
    /// no window, job for job: the device lent to the worker, the
    /// frame opened under `edit` and developed on it, the blend the
    /// open seeds taken into the edit, then the open frame written
    /// under that edit.
    fn window_export(frame: &queue::Frame, settings: export::Settings) -> Option<PathBuf> {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::new(move |o| {
            let _ = tx.send(o);
        });
        let lent = device();
        match &lent {
            Some((device, queue)) => worker.set_gpu(device, queue),
            None => println!("no adapter: the window's develop runs on the CPU"),
        }
        worker.send(Job::Open {
            path: frame.source.clone(),
            edit: frame.edit.clone(),
            generation: 1,
            seed_blend: frame.seed_blend,
            turn: frame.turn,
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
        worker.send(Job::Export {
            edit,
            path: frame.out.clone(),
            settings,
            on_exists: export::OnExists::Overwrite,
            source: frame.source.clone(),
            preset: None,
        });
        let written = loop {
            match rx.recv_timeout(wait).expect("the export lands") {
                Outcome::Exported { path, .. } => break Some(path),
                Outcome::ExportFailed { message } => panic!("the export failed: {message}"),
                Outcome::ExportSkipped { .. } => break None,
                _ => {}
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
        let cli = cli(&[
            source.as_os_str(),
            "--agx".as_ref(),
            "--exposure".as_ref(),
            "0.7".as_ref(),
            "--develop-temperature".as_ref(),
            "4800".as_ref(),
            "--long-edge".as_ref(),
            "300".as_ref(),
            "--export".as_ref(),
            out.as_os_str(),
        ]);
        let remembered = settings::Settings::default();
        let plan = plan(&cli, &out, &remembered, None, None).unwrap();

        // The window's edit: the sidecar's, the overrides over it; its
        // sheet: the remembered one with the flags over it, in the
        // path's format.
        let mut edit = Sidecar::load(&source).unwrap().unwrap().current;
        Overrides::of(&cli).apply(&mut edit);
        assert_eq!(plan.frames.len(), 1);
        assert_eq!(plan.frames[0].edit, edit);
        assert_eq!(edit.display_curve, greycard_edit::DisplayCurve::Agx);
        assert_eq!(edit.light.exposure, 0.7);
        let (sheet, _) = opening_sheet(&cli, &remembered).unwrap();
        let settings = batch_settings(&out, sheet.settings());
        assert_eq!(plan.settings, settings);

        let window = queue::Frame {
            out: scratch.0.join("window.jpg"),
            ..plan.frames[0].clone()
        };
        let windowed = window_export(&window, settings).expect("the window wrote it");
        let finished = run(plan);
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
        let remembered = settings::Settings::default();
        let planned = plan(
            &cli(&[source.as_os_str(), "--export".as_ref(), out.as_os_str()]),
            &out,
            &remembered,
            None,
            None,
        )
        .unwrap();
        assert_eq!(planned.settings.format, export::Format::Png);
        assert_eq!(planned.frames[0].edit.light.exposure, -0.3);
        assert!(run(planned).succeeded());
        let kept = Sidecar::load(&source).unwrap().unwrap();
        assert_eq!(kept.current_exports.len(), 1);
        assert_eq!(
            kept.current_exports[0].file,
            out.to_string_lossy().into_owned()
        );

        let off = scratch.0.join("off.png");
        let planned = plan(
            &cli(&[
                source.as_os_str(),
                "--no-sidecars".as_ref(),
                "--export".as_ref(),
                off.as_os_str(),
            ]),
            &off,
            &remembered,
            None,
            None,
        )
        .unwrap();
        // The default edit, not the sidecar's.
        assert_eq!(planned.frames[0].edit, Edit::default());
        assert_eq!(planned.placement, None);
        assert!(run(planned).succeeded());
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

    /// `--export DIR --also ROWS`: the frame opened and the rows, in
    /// file order, each under its own name in the folder; the
    /// overrides on the frame opened alone.
    #[test]
    fn a_set_is_the_frame_opened_and_the_rows_each_under_its_own_name() {
        let scratch = Scratch::new("set");
        let files = names(&scratch.0, 4);
        let mut folder = scratch.0.join("out").into_os_string();
        folder.push(std::path::MAIN_SEPARATOR_STR);
        let target = PathBuf::from(&folder);
        // The folder opened, its last file remembered: that one is
        // the frame opened, as the window opens it.
        let remembered = settings::Settings {
            last_file: files[2].to_string_lossy().into_owned(),
            ..Default::default()
        };
        let planned = plan(
            &cli(&[
                scratch.0.as_os_str(),
                "--export".as_ref(),
                target.as_os_str(),
                "--also".as_ref(),
                "3,0".as_ref(),
                "--exposure".as_ref(),
                "1".as_ref(),
                "--no-sidecars".as_ref(),
            ]),
            &target,
            &remembered,
            None,
            None,
        )
        .unwrap();
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
    }

    /// What the command line names and the run cannot do is refused
    /// before a frame is begun, with the reason.
    #[test]
    fn what_wants_a_window_or_names_nothing_is_refused_with_the_reason() {
        let scratch = Scratch::new("refused");
        let files = names(&scratch.0, 2);
        let remembered = settings::Settings::default();
        let out = scratch.0.join("one.jpg");
        let refused = |args: &[&std::ffi::OsStr]| {
            plan(&cli(args), &out, &remembered, None, None)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        };
        let snap = scratch.0.join("snap.png");
        let e = refused(&[
            files[0].as_os_str(),
            "--export".as_ref(),
            out.as_os_str(),
            "--snapshot".as_ref(),
            snap.as_os_str(),
        ]);
        assert!(e.contains("--snapshot") && e.contains("no window"), "{e}");
        for (flag, value) in [
            ("--turn", "1"),
            ("--preview-temperature", "4000"),
            ("--library", "lib.sqlite"),
        ] {
            let e = refused(&[
                files[0].as_os_str(),
                "--export".as_ref(),
                out.as_os_str(),
                flag.as_ref(),
                value.as_ref(),
            ]);
            assert!(e.contains(flag) && e.contains("no window"), "{e}");
        }
        let e = refused(&[
            files[0].as_os_str(),
            "--export".as_ref(),
            out.as_os_str(),
            "--preset".as_ref(),
            "No such preset".as_ref(),
        ]);
        assert!(e.contains("no preset called No such preset"), "{e}");
        // `--also` with one file's target, as the window refused it.
        let e = refused(&[
            scratch.0.as_os_str(),
            "--export".as_ref(),
            out.as_os_str(),
            "--also".as_ref(),
            "1".as_ref(),
        ]);
        assert!(e.contains("--also names a set"), "{e}");
        let e = refused(&[
            scratch.0.join("missing.CR3").as_os_str(),
            "--export".as_ref(),
            out.as_os_str(),
        ]);
        assert!(e.contains("there is no"), "{e}");
    }

    /// A frame that cannot be decoded is a failure the exit code says,
    /// with the reason on the line the run ends on.
    #[test]
    fn a_frame_that_will_not_decode_fails_the_run_and_says_why() {
        let scratch = Scratch::new("fails");
        let files = names(&scratch.0, 1);
        let out = scratch.0.join("out.jpg");
        let planned = plan(
            &cli(&[
                files[0].as_os_str(),
                "--no-sidecars".as_ref(),
                "--export".as_ref(),
                out.as_os_str(),
            ]),
            &out,
            &settings::Settings::default(),
            None,
            None,
        )
        .unwrap();
        let finished = run(planned);
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
        let planned = plan(
            &cli(&[source.as_os_str(), "--export".as_ref(), out.as_os_str()]),
            &out,
            &settings::Settings::default(),
            None,
            None,
        )
        .unwrap();
        assert!(!planned.frames[0].edit.needs_frame());
        let finished = run(planned);
        assert_eq!(finished.code(), WRITTEN, "{}", finished.line);
        let kept = Sidecar::load(&source).unwrap().unwrap();
        assert!(!kept.needs_frame(), "written back up to date");
        assert_eq!(kept.current.light.exposure, 0.25);
        assert_eq!(kept.current_exports.len(), 1, "the export is noted");
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
        let remembered = settings::Settings::default();
        let export = |args: &[&std::ffi::OsStr], out: &Path| {
            let mut all = vec![source.as_os_str(), "--export".as_ref(), out.as_os_str()];
            all.extend_from_slice(args);
            let planned = plan(&cli(&all), out, &remembered, None, None).unwrap();
            run(planned).code()
        };
        assert_eq!(export(&[], &scratch.0.join("a.png")), WRITTEN);
        assert!(!xmp.exists(), "no .xmp unless asked");
        assert_eq!(
            export(&["--xmp-sidecars".as_ref()], &scratch.0.join("b.png")),
            WRITTEN
        );
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
        let remembered = settings::Settings::default();
        let export = |out: &Path| {
            let all = [
                source.as_os_str(),
                "--xmp-sidecars".as_ref(),
                "--export".as_ref(),
                out.as_os_str(),
            ];
            run(plan(&cli(&all), out, &remembered, None, None).unwrap()).code()
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
            let all = [source.as_os_str(), "--export".as_ref(), out.as_os_str()];
            let finished =
                run(plan(&cli(&all), &out, &settings::Settings::default(), None, None).unwrap());
            assert!(out.is_file());
            finished
        };
        let finished = export(1.0, "a.png");
        assert_eq!(finished.code(), LEFT_OUT, "{}", finished.line);
        assert!(finished.line.contains(MISSING), "{}", finished.line);
        assert_eq!(export(0.0, "b.png").code(), WRITTEN);
    }
}
