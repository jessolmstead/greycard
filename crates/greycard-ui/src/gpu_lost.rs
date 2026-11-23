//! The window's device lost, and the start again that follows.
//!
//! A driver resets a GPU whose one submission runs past its watchdog
//! (Windows' TDR after about two seconds, amdgpu's ring timeout after
//! ten), and wgpu then reports the device lost. The window's device is
//! Slint's: its renderer made every buffer and pipeline the window
//! draws with on it, and Slint 1.19 can neither hand a window a new
//! device nor make the window again on one, its wgpu setup running
//! once a process. Every frame after the loss fails, and femtovg panics
//! at the first buffer it maps. So we start again: on the loss the
//! worker's ops go to the CPU, the window's event loop is ended, the
//! quit saves what any close saves (the open frame's edit among it),
//! and the last thing it does is start greycard again with the same
//! arguments and `--cpu-ops`, so the frame that lost the device opens
//! with the ops on the CPU, once this process has ended
//! ([`wait_for_the_last_run`]). The user sees the window close and come
//! back with the edit as it was, where a crash used to be followed by
//! another at every start on the same file.
//!
//! A run under `--cpu-ops` that loses its device is not started again:
//! its ops never ran there, so the window's own drawing lost it, and a
//! start again could lose it the same way for ever. It quits, saved.
//!
//! Unless a model lost it: the ops are not all that runs on the card,
//! and a learned denoiser on a small, slow card lost it in the window
//! and again under `--cpu-ops`. A model whose run was under way when
//! the device was lost (read in the callback, in memory only), or whose
//! run failed on a lost device, is put on record as off this card,
//! driver, and build (`greycard_ai::runtime::put_off_card`, by the quit,
//! off the callback), and is not run there again. A run under
//! `--cpu-ops` starts again then, but only when that record was written
//! and read back, and the start again carries `--after-model-loss`: a
//! loss in it starts no other, whatever the record says.
//!
//! Until the quit, nothing on the window's thread may panic on the
//! dead device: the viewport's waits go through its `lost::poll`, the
//! uncaptured handler lets every error through once [`lost`] says so,
//! and a panic of Slint's drawing that still comes is caught around
//! the event loop ([`survive`]).

use std::ffi::OsString;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use greycard_ai::runtime::OnCard;
use greycard_gpu::wgpu;

/// The hidden argument a start again after a model's loss carries: a
/// loss in that run starts no other.
pub(crate) const AFTER_MODEL_LOSS: &str = "--after-model-loss";

/// What a loss leaves for the rest of the run.
struct Loss<'a> {
    /// The window's device is lost: the viewport's flag, which its
    /// waits read and set.
    lost: &'a AtomicBool,
    /// The run was under `--cpu-ops` when it was lost.
    cpu_ops: AtomicBool,
    /// The models' runs under way on the card when it was lost.
    runs: Mutex<Vec<OnCard>>,
    /// The quit is under way: a device-lost callback now is the
    /// device let go, not a loss.
    leaving: AtomicBool,
}

impl<'a> Loss<'a> {
    const fn new(lost: &'a AtomicBool) -> Self {
        Self {
            lost,
            cpu_ops: AtomicBool::new(false),
            runs: Mutex::new(Vec::new()),
            leaving: AtomicBool::new(false),
        }
    }

    /// The device-lost callback's body: once a run, and not while
    /// leaving. Keeps the models' runs under way on the card
    /// (`on_card_now`, in memory: the record is the quit's), tells the
    /// worker (`again`: whether it develops the picture on screen again
    /// itself, which it need not when the editor may start again), then
    /// asks for the quit. True when it acted.
    fn on_lost(
        &self,
        reason: wgpu::DeviceLostReason,
        message: &str,
        cpu_ops: bool,
        on_card_now: &dyn Fn() -> Vec<OnCard>,
        tell_worker: &dyn Fn(bool),
        quit: &dyn Fn(),
    ) -> bool {
        if self.leaving.load(Ordering::SeqCst) || self.lost.swap(true, Ordering::SeqCst) {
            return false;
        }
        let runs = on_card_now();
        self.cpu_ops.store(cpu_ops, Ordering::SeqCst);
        let message = if message.is_empty() {
            String::new()
        } else {
            format!(": {message}")
        };
        if !runs.is_empty() {
            let models: Vec<&str> = runs.iter().map(OnCard::model).collect();
            tracing::warn!(
                "the window's GPU device was lost ({reason:?}){message} while {} ran on the card",
                models.join(", ")
            );
        } else if cpu_ops {
            tracing::warn!(
                "the window's GPU device was lost ({reason:?}){message}, under --cpu-ops"
            );
        } else {
            tracing::warn!(
                "the window's GPU device was lost ({reason:?}){message}; greycard saves \
                 and starts again with the engine's ops on the CPU (--cpu-ops)"
            );
        }
        *self.runs.lock().unwrap_or_else(|e| e.into_inner()) = runs;
        tell_worker(false);
        quit();
        true
    }
}

static LOSS: Loss<'static> = Loss::new(&greycard_viewport::lost::LOST);

/// Whether the window's device has been lost this run.
pub(crate) use greycard_viewport::lost::lost;

/// Set the window's device-lost callback, once its device is made.
/// `cpu_ops` is whether this run was started with `--cpu-ops`;
/// `tell_worker` is [`crate::worker::Worker::gpu_lost_teller`]'s.
pub(crate) fn install(
    device: &wgpu::Device,
    cpu_ops: bool,
    tell_worker: impl Fn(bool) + Send + Sync + 'static,
) {
    device.set_device_lost_callback(callback(&LOSS, cpu_ops, tell_worker, || {
        // From whichever thread found the loss: the worker's, in a
        // wait, as often as the window's.
        if let Err(e) = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        }) {
            tracing::debug!("quitting after the device was lost: {e}");
        }
    }));
}

/// The device-lost callback on `loss`: the models' runs under way on
/// the card read from the process's own ([`greycard_ai::runtime::on_card_now`]).
fn callback(
    loss: &'static Loss<'static>,
    cpu_ops: bool,
    tell_worker: impl Fn(bool) + Send + Sync + 'static,
    quit: impl Fn() + Send + Sync + 'static,
) -> impl Fn(wgpu::DeviceLostReason, String) + Send + Sync + 'static {
    move |reason, message| {
        loss.on_lost(
            reason,
            &message,
            cpu_ops,
            &greycard_ai::runtime::on_card_now,
            &tell_worker,
            &quit,
        );
    }
}

/// The quit is under way: the device let go from here on is no loss.
pub(crate) fn leaving() {
    LOSS.leaving.store(true, Ordering::SeqCst);
}

/// `run`, the window's event loop, with a panic after the device was
/// lost caught and taken for the loop's end, so the quit's saves and
/// the start again still come; `ended` is what the loop returns then.
/// Any other panic goes on as it was.
pub(crate) fn survive<T>(run: impl FnOnce() -> T, ended: T) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(out) => out,
        Err(_) if lost() => {
            tracing::warn!("the window's drawing panicked on the lost device; quitting, saved");
            ended
        }
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// The arguments of the start again: this run's, `--cpu-ops` once,
/// [`AFTER_MODEL_LOSS`] once where `after_model` (a model was put off
/// the card), and the file given replaced by the one open now, so the
/// start opens where the run left off. A folder given is kept: its
/// start opens on the frame last open in it, which the quit has just
/// remembered.
fn arguments(
    args: &[OsString],
    given: Option<&Path>,
    current: Option<&Path>,
    after_model: bool,
) -> Vec<OsString> {
    let given_file = given.filter(|g| !g.is_dir());
    let mut out: Vec<OsString> = args
        .iter()
        .filter(|a| a.as_os_str() != "--cpu-ops" && a.as_os_str() != AFTER_MODEL_LOSS)
        .map(|a| match (given_file, current) {
            (Some(g), Some(c)) if Path::new(a) == g => c.as_os_str().to_owned(),
            _ => a.clone(),
        })
        .collect();
    out.push("--cpu-ops".into());
    if after_model {
        out.push(AFTER_MODEL_LOSS.into());
    }
    out
}

/// The program to start again: the AppImage itself rather than the
/// binary in its mount, which goes with this process.
fn program() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(program_of(
        exe,
        std::env::var_os("APPIMAGE").as_deref(),
        std::env::var_os("APPDIR").as_deref(),
    ))
}

/// [`program`] from what it reads: `APPIMAGE` only when this binary
/// is inside `APPDIR`, the AppImage's mount, and so ours; an AppImage
/// that started us from its own mount leaves both set for us to
/// inherit, and its file is not greycard.
fn program_of(
    exe: std::path::PathBuf,
    appimage: Option<&std::ffi::OsStr>,
    appdir: Option<&std::ffi::OsStr>,
) -> std::path::PathBuf {
    // `current_exe` is resolved (`/proc/self/exe`), so the mount's
    // path is too: under a symlinked TMPDIR the two would not match,
    // and we would start the binary in a mount that goes with us.
    let dir = appdir
        .filter(|d| !d.is_empty())
        .map(|d| std::fs::canonicalize(d).unwrap_or_else(|_| d.into()));
    match (appimage, dir) {
        (Some(image), Some(dir)) if !image.is_empty() && exe.starts_with(&dir) => {
            std::path::PathBuf::from(image)
        }
        _ => exe,
    }
}

/// What the quit does about a loss.
#[derive(Debug, PartialEq, Eq)]
enum AfterQuit {
    /// No loss, or one it does not start again for: it ends as it would.
    Nothing,
    /// A scripted run on a lost device: it fails, and nothing starts.
    Fail,
    /// Start greycard again; `after_model` when a model was put off the
    /// card, so the start carries [`AFTER_MODEL_LOSS`].
    StartAgain { after_model: bool },
}

impl Loss<'_> {
    /// [`start_again`]'s choice, after the models whose runs were under
    /// way are put off the card (`put_off`, which names every model this
    /// process has put off it and read back, those whose run failed on a
    /// lost device before too). A scripted run fails on any loss. A run
    /// started again after a model's loss (`after_model_loss`) starts no
    /// other. Off `--cpu-ops` it starts again, as it always has; under
    /// it, only when a model was put off the card and its record landed.
    fn after_quit(
        &self,
        scripted: bool,
        after_model_loss: bool,
        put_off: &dyn Fn(&[OnCard]) -> Vec<String>,
    ) -> AfterQuit {
        if !self.lost.load(Ordering::SeqCst) {
            return AfterQuit::Nothing;
        }
        let runs = std::mem::take(&mut *self.runs.lock().unwrap_or_else(|e| e.into_inner()));
        let models = put_off(&runs);
        if !models.is_empty() {
            tracing::warn!("put off the card: {}", models.join(", "));
        }
        if scripted {
            return AfterQuit::Fail;
        }
        if after_model_loss {
            tracing::warn!(
                "greycard not started again: this run was started after a model's loss already"
            );
            return AfterQuit::Nothing;
        }
        let after_model = !models.is_empty();
        if !self.cpu_ops.load(Ordering::SeqCst) || after_model {
            AfterQuit::StartAgain { after_model }
        } else {
            tracing::warn!("greycard saves and quits rather than start again");
            AfterQuit::Nothing
        }
    }
}

/// What [`start_again`] does.
#[derive(Debug, PartialEq, Eq)]
enum Start {
    Nothing,
    Fail,
    /// Start greycard again with these arguments.
    With(Vec<OsString>),
}

impl Loss<'_> {
    /// [`start_again`]'s choice from this run's arguments `args`: the
    /// once-only [`AFTER_MODEL_LOSS`] read from them, and the start's
    /// own made from them ([`arguments`]).
    fn start_with(
        &self,
        args: &[OsString],
        given: Option<&Path>,
        current: Option<&Path>,
        scripted: bool,
        put_off: &dyn Fn(&[OnCard]) -> Vec<String>,
    ) -> Start {
        let after_model_loss = args.iter().any(|a| a.as_os_str() == AFTER_MODEL_LOSS);
        match self.after_quit(scripted, after_model_loss, put_off) {
            AfterQuit::Nothing => Start::Nothing,
            AfterQuit::Fail => Start::Fail,
            AfterQuit::StartAgain { after_model } => {
                Start::With(arguments(args, given, current, after_model))
            }
        }
    }
}

/// The quit's last step: start greycard again if the device was lost,
/// unless the run is `scripted` (a capture or keys sent: a script waits
/// on this process, and a capture from a child it does not see would
/// land after it moved on), which then fails rather than start again.
/// True when the run is to fail for it.
///
/// `given` is the path this run was started on and `current` the file
/// open now. Our own environment is scrubbed as for every command we
/// start, so the start decides its adapter anew.
pub(crate) fn start_again(given: Option<&Path>, current: Option<&Path>, scripted: bool) -> bool {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let args = match LOSS.start_with(
        &args,
        given,
        current,
        scripted,
        &greycard_ai::runtime::put_off_card,
    ) {
        Start::Nothing => return false,
        Start::Fail => {
            tracing::warn!("greycard not started again: a scripted run fails on the lost device");
            return true;
        }
        Start::With(args) => args,
    };
    let Some(program) = program() else {
        tracing::warn!("greycard not started again: its own path is not known");
        return false;
    };
    let mut command = std::process::Command::new(&program);
    command
        .args(&args)
        .env(STARTED_AGAIN, "1")
        .stdin(std::process::Stdio::piped());
    match crate::adapter::scrub(&mut command).spawn() {
        Ok(mut child) => {
            // Our end of its input, open until this process ends and
            // the system closes it: the start's sign that we are gone.
            std::mem::forget(child.stdin.take());
            tracing::info!(
                "greycard started again as {} (pid {}) {}",
                program.display(),
                child.id(),
                args.iter()
                    .map(|a| a.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        Err(e) => tracing::warn!("greycard not started again: {e}"),
    }
    false
}

/// The variable a start again carries, so it waits for the run that
/// started it to end ([`wait_for_the_last_run`]).
const STARTED_AGAIN: &str = "GREYCARD_STARTED_AGAIN";

/// The longest a start again waits for the run that started it.
const LAST_RUN_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// The first thing a run does: when it is a start again, wait for the
/// run that started it to end, its input closing as that process does,
/// for no longer than [`LAST_RUN_WAIT`]. That run's quit still holds
/// its GPU device, its models, and its memory as it starts us, and a
/// device lost for want of memory (issue 195) could otherwise be lost
/// again before the old one let go. Waiting here, before the claim of
/// the single instance and before the log starts, the claim finds the
/// old editor gone rather than closing, and the log is ours to turn
/// over. How long it waited, and whether the run ended in that time;
/// None for any other run. The variable goes from our environment, so
/// nothing we start waits in turn.
pub(crate) fn wait_for_the_last_run() -> Option<(std::time::Duration, bool)> {
    std::env::var_os(STARTED_AGAIN)?;
    // SAFETY: the process's first step, before any thread is started.
    unsafe { std::env::remove_var(STARTED_AGAIN) };
    Some(wait_for_end(std::io::stdin(), LAST_RUN_WAIT))
}

/// Read `input` to its end on a thread of its own, for no longer than
/// `most`: how long that took, and whether the end came.
fn wait_for_end(
    mut input: impl std::io::Read + Send + 'static,
    most: std::time::Duration,
) -> (std::time::Duration, bool) {
    let started = std::time::Instant::now();
    let (ended, end) = std::sync::mpsc::channel();
    let reader = std::thread::Builder::new()
        .name("last run".into())
        .spawn(move || {
            let mut buf = [0; 64];
            loop {
                match input.read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    // A pipe whose writer went: the end too.
                    Err(_) => break,
                }
            }
            let _ = ended.send(());
        });
    let came = reader.is_ok() && end.recv_timeout(most).is_ok();
    (started.elapsed(), came)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_start_again_opens_the_file_open_now_on_the_cpu() {
        let given = Path::new("/no/such/dir/IMG_1.CR3");
        let now = Path::new("/no/such/dir/IMG_2.CR3");
        assert_eq!(
            arguments(
                &os(&["/no/such/dir/IMG_1.CR3", "--grid"]),
                Some(given),
                Some(now),
                false
            ),
            os(&["/no/such/dir/IMG_2.CR3", "--grid", "--cpu-ops"])
        );
        // Nothing open now: the file given.
        assert_eq!(
            arguments(&os(&["/no/such/dir/IMG_1.CR3"]), Some(given), None, false),
            os(&["/no/such/dir/IMG_1.CR3", "--cpu-ops"])
        );
        // No path given: the start finds the folder it remembers.
        assert_eq!(
            arguments(&os(&["--library", "x.sqlite"]), None, Some(now), false),
            os(&["--library", "x.sqlite", "--cpu-ops"])
        );
    }

    /// The start again goes on once the old run's end of its input
    /// closes, and no later than its wait should it never close.
    #[test]
    fn the_start_again_waits_for_the_last_run_to_end() {
        use std::time::Duration;
        let (input, old) = std::io::pipe().expect("a pipe");
        let closer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(old);
        });
        let (took, came) = wait_for_end(input, Duration::from_secs(60));
        closer.join().unwrap();
        assert!(came);
        assert!(took >= Duration::from_millis(50), "{took:?}");
        assert!(took < Duration::from_secs(30), "{took:?}");
        // A run that never ends: the wait gives up.
        let (input, old) = std::io::pipe().expect("a pipe");
        let (took, came) = wait_for_end(input, Duration::from_millis(50));
        assert!(!came);
        assert!(took >= Duration::from_millis(50), "{took:?}");
        drop(old);
        // Anything written meanwhile is not the end.
        let (input, mut old) = std::io::pipe().expect("a pipe");
        std::io::Write::write_all(&mut old, b"not yet").unwrap();
        let (_, came) = wait_for_end(input, Duration::from_millis(50));
        assert!(!came);
    }

    #[test]
    fn the_appimage_is_started_only_from_its_own_mount() {
        let exe = std::path::PathBuf::from("/tmp/.mount_greyXY/usr/bin/greycard-ui");
        let image = std::ffi::OsStr::new("/home/u/greycard.AppImage");
        let ours = std::ffi::OsStr::new("/tmp/.mount_greyXY");
        let theirs = std::ffi::OsStr::new("/tmp/.mount_otherZ");
        assert_eq!(
            program_of(exe.clone(), Some(image), Some(ours)),
            Path::new(image)
        );
        // Inherited from an AppImage that started us: our own binary.
        assert_eq!(program_of(exe.clone(), Some(image), Some(theirs)), exe);
        let plain = std::path::PathBuf::from("/usr/bin/greycard-ui");
        assert_eq!(program_of(plain.clone(), Some(image), Some(ours)), plain);
        assert_eq!(program_of(plain.clone(), Some(image), None), plain);
        assert_eq!(program_of(plain.clone(), None, None), plain);
    }

    /// A mount reached through a symlink still counts as ours:
    /// `current_exe` is the resolved path.
    #[cfg(unix)]
    #[test]
    fn the_appimage_mount_is_compared_resolved() {
        let root = std::env::temp_dir().join(format!("greycard-appdir-{}", std::process::id()));
        let real = root.join("real");
        let link = root.join("link");
        std::fs::create_dir_all(real.join("usr/bin")).unwrap();
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let exe = std::fs::canonicalize(&real)
            .unwrap()
            .join("usr/bin/greycard-ui");
        let image = std::ffi::OsStr::new("/home/u/greycard.AppImage");
        assert_eq!(
            program_of(exe.clone(), Some(image), Some(link.as_os_str())),
            Path::new(image)
        );
        // A mount that cannot be resolved is compared as it is.
        let gone = root.join("gone");
        assert_eq!(
            program_of(exe.clone(), Some(image), Some(gone.as_os_str())),
            exe
        );
        crate::testing::remove_dir_retry(&root);
    }

    const ADAPTER: &str = "Test GPU|Vulkan|1.2.3";
    const BUILD: &str = "WebGPU,CPU greycard 0.1.0";

    fn run(model: &str) -> OnCard {
        OnCard::new(Path::new("/no/such/store"), model, "hash", ADAPTER, BUILD)
    }

    /// A loss with `runs` under way, then the quit's choice with a
    /// record that lands (`landed`) or not.
    fn lost(cpu_ops: bool, runs: Vec<OnCard>) -> &'static Loss<'static> {
        let flag: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        let loss: &'static Loss<'static> = Box::leak(Box::new(Loss::new(flag)));
        let reason = wgpu::DeviceLostReason::Unknown;
        assert!(loss.on_lost(reason, "", cpu_ops, &move || runs.clone(), &|_| {}, &|| {}));
        loss
    }

    #[test]
    fn a_folder_given_is_kept_and_cpu_ops_said_once() {
        let dir = std::env::temp_dir();
        let now = dir.join("IMG_2.CR3");
        let args = vec![
            dir.clone().into_os_string(),
            "--cpu-ops".into(),
            "--grid".into(),
        ];
        assert_eq!(
            arguments(&args, Some(&dir), Some(&now), false),
            vec![dir.into_os_string(), "--grid".into(), "--cpu-ops".into()]
        );
    }

    /// The start again carries `--cpu-ops` once, and the once-only
    /// argument once when a model was put off the card.
    #[test]
    fn the_start_again_after_a_model_carries_the_once_only_argument() {
        let args = os(&["--grid", "--cpu-ops", AFTER_MODEL_LOSS]);
        assert_eq!(
            arguments(&args, None, None, true),
            os(&["--grid", "--cpu-ops", AFTER_MODEL_LOSS])
        );
        assert_eq!(
            arguments(&os(&["--grid"]), None, None, false),
            os(&["--grid", "--cpu-ops"])
        );
    }

    /// A loss with no model's run under way and none put off the card
    /// is master's: off `--cpu-ops` it starts again, under it it quits,
    /// a scripted run fails.
    #[test]
    fn a_loss_with_no_model_is_as_it_was() {
        let none = |runs: &[OnCard]| -> Vec<String> {
            assert!(runs.is_empty());
            Vec::new()
        };
        assert_eq!(
            lost(false, vec![]).after_quit(false, false, &none),
            AfterQuit::StartAgain { after_model: false }
        );
        assert_eq!(
            lost(true, vec![]).after_quit(false, false, &none),
            AfterQuit::Nothing
        );
        assert_eq!(
            lost(true, vec![]).after_quit(true, false, &none),
            AfterQuit::Fail
        );
        let flag = AtomicBool::new(false);
        assert_eq!(
            Loss::new(&flag).after_quit(false, false, &none),
            AfterQuit::Nothing
        );
    }

    /// A model's run under way at the loss is handed to the quit, which
    /// puts it off the card: under `--cpu-ops` it starts again only when
    /// the record landed, and then once only.
    #[test]
    fn a_loss_under_a_model_starts_again_only_when_its_record_landed() {
        let best = run("greycard-denoise-best-v20");
        let landed = |runs: &[OnCard]| -> Vec<String> {
            runs.iter().map(|r| r.model().to_string()).collect()
        };
        let refused = |_: &[OnCard]| -> Vec<String> { Vec::new() };
        for cpu_ops in [false, true] {
            assert_eq!(
                lost(cpu_ops, vec![best.clone()]).after_quit(false, false, &landed),
                AfterQuit::StartAgain { after_model: true }
            );
        }
        // The record did not land: as master.
        assert_eq!(
            lost(true, vec![best.clone()]).after_quit(false, false, &refused),
            AfterQuit::Nothing
        );
        assert_eq!(
            lost(false, vec![best.clone()]).after_quit(false, false, &refused),
            AfterQuit::StartAgain { after_model: false }
        );
        // A run failing on the lost device put it off before: nothing
        // under way, and still it starts again, once.
        let before = |_: &[OnCard]| vec!["greycard-denoise-best-v20".to_string()];
        assert_eq!(
            lost(true, vec![]).after_quit(false, false, &before),
            AfterQuit::StartAgain { after_model: true }
        );
    }

    /// A run started again after a model's loss starts no other,
    /// whatever the record says, and still puts its runs off the card.
    #[test]
    fn a_run_after_a_models_loss_never_starts_again() {
        let best = run("greycard-denoise-best-v20");
        let asked = Mutex::new(Vec::new());
        let landed = |runs: &[OnCard]| -> Vec<String> {
            let models: Vec<String> = runs.iter().map(|r| r.model().to_string()).collect();
            asked.lock().unwrap().extend(models.clone());
            models
        };
        for cpu_ops in [false, true] {
            assert_eq!(
                lost(cpu_ops, vec![best.clone()]).after_quit(false, true, &landed),
                AfterQuit::Nothing
            );
        }
        assert_eq!(asked.lock().unwrap().len(), 2);
    }

    /// With a store that cannot be written, the real record puts nothing
    /// off, so a loss under `--cpu-ops` quits rather than loop.
    #[test]
    fn a_store_that_cannot_be_written_quits_under_cpu_ops() {
        let base = std::env::temp_dir().join(format!("greycard-lost-ro-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let file = base.join("not-a-folder");
        std::fs::write(&file, b"x").unwrap();
        let on = OnCard::new(&file.join("store"), "lost-ro-model", "h", ADAPTER, BUILD);
        let held = greycard_ai::runtime::Held::new();
        assert_eq!(
            lost(true, vec![on]).after_quit(false, false, &|runs| held.put_off(runs)),
            AfterQuit::Nothing
        );
        crate::testing::remove_dir_retry(&base);
    }

    /// The callback the window installs reads the process's runs under
    /// way on the card, once, and tells the worker and quits once.
    #[test]
    fn the_installed_callback_reads_the_runs_under_way() {
        let flag: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        let loss: &'static Loss<'static> = Box::leak(Box::new(Loss::new(flag)));
        let told = std::sync::Arc::new(Mutex::new(0));
        let quits = std::sync::Arc::new(Mutex::new(0));
        let (t, q) = (told.clone(), quits.clone());
        let cb = callback(
            loss,
            true,
            move |_| *t.lock().unwrap() += 1,
            move || *q.lock().unwrap() += 1,
        );
        let on = run("installed-callback-test");
        greycard_ai::runtime::HELD
            .run(Some(&on), || {
                cb(wgpu::DeviceLostReason::Unknown, "reset".into());
                cb(wgpu::DeviceLostReason::Unknown, "again".into());
                Ok::<_, String>(())
            })
            .unwrap();
        let runs = loss.runs.lock().unwrap().clone();
        assert!(runs.iter().any(|r| r.model() == "installed-callback-test"));
        assert_eq!((*told.lock().unwrap(), *quits.lock().unwrap()), (1, 1));
        // A callback while leaving is the device let go.
        loss.leaving.store(true, Ordering::SeqCst);
        flag.store(false, Ordering::SeqCst);
        cb(wgpu::DeviceLostReason::Destroyed, String::new());
        assert!(!flag.load(Ordering::SeqCst));
    }

    /// The once-only argument is read from the run's own arguments: a
    /// run that carries it starts no other; one that does not starts
    /// again carrying it, after a model was put off the card.
    #[test]
    fn the_once_only_argument_is_read_from_the_runs_own() {
        let best = run("greycard-denoise-best-v20");
        let landed = |runs: &[OnCard]| -> Vec<String> {
            runs.iter().map(|r| r.model().to_string()).collect()
        };
        let carried = os(&["a.CR3", "--cpu-ops", AFTER_MODEL_LOSS]);
        assert_eq!(
            lost(true, vec![best.clone()]).start_with(&carried, None, None, false, &landed),
            Start::Nothing
        );
        assert_eq!(
            lost(false, vec![best]).start_with(&os(&["a.CR3"]), None, None, false, &landed),
            Start::With(os(&["a.CR3", "--cpu-ops", AFTER_MODEL_LOSS]))
        );
    }

    /// The record is written by the quit, never in the callback: the
    /// store holds nothing after the loss is heard, and the record after
    /// the quit's choice.
    #[test]
    fn the_record_is_written_by_the_quit_not_the_callback() {
        let root =
            std::env::temp_dir().join(format!("greycard-lost-quit-writes-{}", std::process::id()));
        crate::testing::remove_dir_retry(&root);
        let flag: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        let loss: &'static Loss<'static> = Box::leak(Box::new(Loss::new(flag)));
        let cb = callback(loss, true, |_| {}, || {});
        let on = OnCard::new(&root, "quit-writes-test", "h", ADAPTER, BUILD);
        greycard_ai::runtime::HELD
            .run(Some(&on), || {
                cb(wgpu::DeviceLostReason::Unknown, String::new());
                Ok::<_, String>(())
            })
            .unwrap();
        let record = greycard_ai::runtime::off_card_path(&root);
        assert!(!record.exists(), "nothing written in the callback");
        let held = greycard_ai::runtime::Held::new();
        assert_eq!(
            loss.after_quit(false, false, &|runs| held.put_off(runs)),
            AfterQuit::StartAgain { after_model: true }
        );
        assert!(record.exists(), "written by the quit");
        crate::testing::remove_dir_retry(&root);
    }
}
