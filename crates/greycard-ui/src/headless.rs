//! `--export` with no window: the editor's alias, for a release, of
//! `greycard export`. The flags are read into the engine's request and
//! the engine's export run (`greycard_engine::headless`) does the rest,
//! as the command line's does.

use std::path::Path;

use anyhow::Result;
use greycard_edit::preset;
use greycard_engine::headless::{Request, plan, run};
use greycard_engine::settings::Shared;

use crate::Cli;
use crate::panel::startup::overrides_of;
use crate::settings;

/// `--export TARGET`, from the command line to the exit code.
pub(crate) fn export(cli: &Cli, target: &Path) -> Result<std::process::ExitCode> {
    tracing::warn!(
        "--export is now `greycard export`, and goes from the editor in a later release"
    );
    refuse_window_flags(cli)?;
    // The window's test hook as much as the user's file.
    let remembered = settings::path()
        .map(|p| Shared::load_from(&p))
        .unwrap_or_default();
    let presets = preset::Store::user();
    let plan = plan(
        &request(cli, target),
        &remembered,
        presets.as_ref(),
        greycard_library::Library::user_path(),
    )?;
    let finished = run(plan, lent(cli.cpu_ops));
    // The log is a file, and the terminal shows warnings up: the one
    // line a batch caller wants is said here whichever way it went.
    eprintln!("{}", finished.line);
    leave(finished.code())
}

/// What `cli` asks the engine to write to `target`.
pub(crate) fn request(cli: &Cli, target: &Path) -> Request {
    Request {
        paths: cli.path.iter().cloned().collect(),
        target: target.to_path_buf(),
        also: cli.also.clone(),
        all: cli.all,
        preset: cli.preset.clone(),
        export_preset: cli.export_preset.clone(),
        long_edge: cli.long_edge,
        on_exists: cli.on_exists,
        version_name: cli.version_name.clone(),
        overrides: overrides_of(cli),
        no_sidecars: cli.no_sidecars,
        sidecar_folder: cli.sidecar_folder,
        xmp_sidecars: cli.xmp_sidecars,
        // The settings' alone: the editor's `--export` takes no flag
        // for them.
        author: None,
        copyright: None,
    }
}

/// The context the run's device stages go on: none under
/// `--cpu-ops`, the editor's own switch for its ops, else one of our
/// own where the machine has a GPU, a software adapter taken only when
/// asked for by hand, as the window takes one.
fn lent(cpu_ops: bool) -> greycard_engine::headless::Lent {
    if cpu_ops {
        return Err("--cpu-ops".into());
    }
    greycard_gpu::Context::for_export(crate::adapter::asked()).map_err(|d| d.to_string())
}

/// End the process with `code`, the run's work all done and said.
///
/// By `greycard_ai::runtime::end`: `_exit` on unix, `TerminateProcess`
/// on Windows, which skip the C library's exit handlers and the static
/// destructors behind them, and on Windows every DLL's detach and the
/// TLS callbacks too. Three callers, for two reasons.
///
/// The headless export, always. A run whose edit asked for a learned
/// mask, a fill or the learned denoiser has loaded ONNX Runtime's
/// WebGPU provider, Dawn, and Dawn's Vulkan instance is torn down by a
/// static destructor that died in libvulkan on the way out: SIGSEGV,
/// exit 139, after the file was written; on Windows `ort` tears it
/// down from a TLS callback at process detach, and the GPU's driver
/// and the shader compiler's DLL faulted there after the window
/// closed (`greycard_ai::runtime::leave`). A batch caller reads the exit
/// code, and must not read a written file as a crash. Nothing is lost
/// there: the set is done, the worker asked to stop (and joined, unless
/// it outlasts the wait), every file written is closed.
///
/// The window, only when its worker is still in a job after the
/// quit's wait (`startup::main`). The exit handlers would tear the GPU
/// driver down under a worker that may be waiting on a fence in it.
/// Its saves are all written by then; what the worker had in hand is
/// lost, as a quit always lost it.
///
/// For both, the log's file and stderr are written unbuffered, and
/// stdout and the C library's streams are flushed first.
///
/// And a measurement with no window (`--match-compare` and the rest)
/// whose develops opened a model (`startup::ended`), for the export's
/// reason.
pub(crate) fn leave(code: u8) -> ! {
    // The same exit, and the same reason, `greycard_ai::runtime::leave`
    // gives the command line once a model has been opened.
    greycard_ai::runtime::end(code)
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use greycard_engine::headless::Overrides;
    use std::path::PathBuf;

    fn cli(args: &[&str]) -> Cli {
        let mut all = vec!["greycard-ui"];
        all.extend_from_slice(args);
        Cli::try_parse_from(all).unwrap()
    }

    /// Every flag `--export` takes reaches the engine's request as
    /// `greycard export` hands it over.
    #[test]
    fn the_flags_are_the_engine_s_request() {
        let flags = cli(&[
            "shoot",
            "--export",
            "out/",
            "--also",
            "2,0",
            "--preset",
            "Warm",
            "--export-preset",
            "Web",
            "--long-edge",
            "1600",
            "--on-exists",
            "skip",
            "--version-name",
            "Two",
            "--exposure",
            "0.5",
            "--develop-temperature",
            "4800",
            "--agx",
            "--no-sidecars",
            "--sidecar-folder",
            "--xmp-sidecars",
        ]);
        assert_eq!(
            request(&flags, Path::new("out/")),
            Request {
                paths: vec![PathBuf::from("shoot")],
                target: PathBuf::from("out/"),
                also: vec![2, 0],
                all: false,
                preset: Some("Warm".into()),
                export_preset: Some("Web".into()),
                long_edge: Some(1600),
                on_exists: Some(crate::export::OnExists::Skip),
                version_name: Some("Two".into()),
                overrides: Overrides {
                    temperature: Some(4800.0),
                    exposure: Some(0.5),
                    display_curve: Some(greycard_edit::DisplayCurve::Agx),
                },
                no_sidecars: true,
                sidecar_folder: true,
                xmp_sidecars: true,
                author: None,
                copyright: None,
            }
        );
        assert_eq!(
            lent(true).err().as_deref(),
            Some("--cpu-ops"),
            "--cpu-ops is the export's --cpu"
        );
        let bare = request(&cli(&["--export", "out.jpg"]), Path::new("out.jpg"));
        assert!(bare.paths.is_empty() && bare.overrides.is_empty());
        // A given exposure is the override, 0 included, as `greycard
        // export` reads it; a negative one parses.
        let ev = |value: &str| {
            request(
                &cli(&["x.CR3", "--export", "out.jpg", "--exposure", value]),
                Path::new("out.jpg"),
            )
            .overrides
            .exposure
        };
        assert_eq!(ev("0"), Some(0.0));
        assert_eq!(ev("-1"), Some(-1.0));
        let all = request(
            &cli(&["shoot", "--export", "out/", "--all"]),
            Path::new("out/"),
        );
        assert!(all.all);
    }

    /// `--all` is the folder's whole set: not with `--also`, which
    /// names part of one, and not without `--export`.
    #[test]
    fn all_with_also_or_without_export_is_refused() {
        let refused = |args: &[&str]| {
            Cli::try_parse_from(args)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        };
        let e = refused(&[
            "greycard-ui",
            "shoot",
            "--export",
            "out/",
            "--all",
            "--also",
            "1",
        ]);
        assert!(e.contains("--all") && e.contains("--also"), "{e}");
        let e = refused(&["greycard-ui", "shoot", "--all"]);
        assert!(e.contains("--export"), "{e}");
    }

    /// What wants a window is refused before a frame is begun, with the
    /// reason.
    #[test]
    fn what_wants_a_window_is_refused_with_the_reason() {
        let refused = |args: &[&str]| {
            refuse_window_flags(&cli(args))
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        };
        let e = refused(&["x.CR3", "--export", "one.jpg", "--snapshot", "snap.png"]);
        assert!(e.contains("--snapshot") && e.contains("no window"), "{e}");
        for (flag, value) in [
            ("--turn", "1"),
            ("--preview-temperature", "4000"),
            ("--library", "lib.sqlite"),
        ] {
            let e = refused(&["x.CR3", "--export", "one.jpg", flag, value]);
            assert!(e.contains(flag) && e.contains("no window"), "{e}");
        }
        assert_eq!(refused(&["x.CR3", "--export", "one.jpg", "--agx"]), "");
    }
}
