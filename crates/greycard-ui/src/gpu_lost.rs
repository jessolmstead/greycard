//! The window's device lost, and the start again that follows.
//!
//! A driver resets a GPU whose one submission runs past its watchdog
//! (Windows' TDR after about two seconds, amdgpu's ring timeout after
//! ten), and wgpu then reports the device lost. The window's device is
//! Slint's: its renderer made every buffer and pipeline the window
//! draws with on it, and Slint 1.18 can neither hand a window a new
//! device nor make the window again on one, its wgpu setup running
//! once a process. Every frame after the loss fails, and femtovg panics
//! at the first buffer it maps. So we start again: on the loss the
//! worker's ops go to the CPU, the window's event loop is ended, the
//! quit saves what any close saves (the open frame's edit among it),
//! and the last thing it does is start greycard again with the same
//! arguments and `--cpu-ops`, so the frame that lost the device opens
//! with the ops on the CPU. The user sees the window close and come
//! back with the edit as it was, where a crash used to be followed by
//! another at every start on the same file.
//!
//! A run under `--cpu-ops` that loses its device is not started again:
//! its ops never ran there, so the window's own drawing lost it, and a
//! start again could lose it the same way for ever. It quits, saved.
//!
//! Until the quit, nothing on the window's thread may panic on the
//! dead device: the viewport's waits go through [`poll`], the
//! uncaptured handler lets every error through once [`lost`] says so,
//! and a panic of Slint's drawing that still comes is caught around
//! the event loop ([`survive`]).

use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use greycard_gpu::wgpu;

/// What a loss leaves for the rest of the run.
struct Loss {
    /// The window's device is lost.
    lost: AtomicBool,
    /// greycard is to start again as the quit ends.
    again: AtomicBool,
    /// The quit is under way: a device-lost callback now is the
    /// device let go, not a loss.
    leaving: AtomicBool,
}

impl Loss {
    const fn new() -> Self {
        Self {
            lost: AtomicBool::new(false),
            again: AtomicBool::new(false),
            leaving: AtomicBool::new(false),
        }
    }

    /// The device-lost callback's body: once a run, and not while
    /// leaving. Tells the worker (`again`: whether it develops the
    /// picture on screen again itself, which it need not when the
    /// editor starts again), then asks for the quit. True when it
    /// acted.
    fn on_lost(
        &self,
        reason: wgpu::DeviceLostReason,
        message: &str,
        cpu_ops: bool,
        tell_worker: &dyn Fn(bool),
        quit: &dyn Fn(),
    ) -> bool {
        if self.leaving.load(Ordering::SeqCst) || self.lost.swap(true, Ordering::SeqCst) {
            return false;
        }
        let again = !cpu_ops;
        self.again.store(again, Ordering::SeqCst);
        let message = if message.is_empty() {
            String::new()
        } else {
            format!(": {message}")
        };
        if again {
            tracing::warn!(
                "the window's GPU device was lost ({reason:?}){message}; greycard saves \
                 and starts again with the engine's ops on the CPU (--cpu-ops)"
            );
        } else {
            tracing::warn!(
                "the window's GPU device was lost ({reason:?}){message}, under --cpu-ops; \
                 greycard saves and quits rather than start again"
            );
        }
        tell_worker(false);
        quit();
        true
    }
}

static LOSS: Loss = Loss::new();

/// Whether the window's device has been lost this run.
pub(crate) fn lost() -> bool {
    LOSS.lost.load(Ordering::SeqCst)
}

/// Set the window's device-lost callback, once its device is made.
/// `cpu_ops` is whether this run was started with `--cpu-ops`;
/// `tell_worker` is [`crate::Worker::gpu_lost_teller`]'s.
pub(crate) fn install(
    device: &wgpu::Device,
    cpu_ops: bool,
    tell_worker: impl Fn(bool) + Send + Sync + 'static,
) {
    device.set_device_lost_callback(move |reason, message| {
        LOSS.on_lost(reason, &message, cpu_ops, &tell_worker, &|| {
            // From whichever thread found the loss: the worker's, in
            // a wait, as often as the window's.
            if let Err(e) = slint::invoke_from_event_loop(|| {
                let _ = slint::quit_event_loop();
            }) {
                tracing::debug!("quitting after the device was lost: {e}");
            }
        });
    });
}

/// The quit is under way: the device let go from here on is no loss.
pub(crate) fn leaving() {
    LOSS.leaving.store(true, Ordering::SeqCst);
}

/// Wait on the window's device as `poll_type` says, for the
/// viewport's read backs and its scopes: an error rather than wgpu's
/// panic once the device is lost.
pub(crate) fn poll(device: &wgpu::Device, poll_type: wgpu::PollType) -> Result<(), String> {
    if lost() {
        return Err("the GPU device was lost".into());
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| device.poll(poll_type))) {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(payload) => {
            LOSS.lost.store(true, Ordering::SeqCst);
            Err(format!(
                "the GPU device was lost: {}",
                crate::worker::panic_message(payload.as_ref())
            ))
        }
    }
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
/// and the file given replaced by the one open now, so the start opens
/// where the run left off. A folder given is kept: its start opens on
/// the frame last open in it, which the quit has just remembered.
fn arguments(args: &[OsString], given: Option<&Path>, current: Option<&Path>) -> Vec<OsString> {
    let given_file = given.filter(|g| !g.is_dir());
    let mut out: Vec<OsString> = args
        .iter()
        .filter(|a| a.as_os_str() != "--cpu-ops")
        .map(|a| match (given_file, current) {
            (Some(g), Some(c)) if Path::new(a) == g => c.as_os_str().to_owned(),
            _ => a.clone(),
        })
        .collect();
    out.push("--cpu-ops".into());
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
    /// No loss, or one under `--cpu-ops`: it ends as it would.
    Nothing,
    /// A scripted run on a lost device: it fails, and nothing starts.
    Fail,
    /// Start greycard again.
    StartAgain,
}

impl Loss {
    /// [`start_again`]'s choice. A scripted run fails on any loss,
    /// `--cpu-ops` or not, since its capture never comes.
    fn after_quit(&self, scripted: bool) -> AfterQuit {
        if scripted && self.lost.load(Ordering::SeqCst) {
            AfterQuit::Fail
        } else if self.again.load(Ordering::SeqCst) {
            AfterQuit::StartAgain
        } else {
            AfterQuit::Nothing
        }
    }
}

/// The quit's last step: start greycard again if the device was lost,
/// unless the run is `scripted` (a capture or keys sent: a script waits
/// on this process, and a capture from a child it does not see would
/// land after it moved on), which then fails rather than start again.
/// True when the run is to fail for it.
///
/// `given` is the path this run was started on and `current` the file open
/// now. Our own environment is scrubbed as for every command we start,
/// so the start decides its adapter anew.
pub(crate) fn start_again(given: Option<&Path>, current: Option<&Path>, scripted: bool) -> bool {
    match LOSS.after_quit(scripted) {
        AfterQuit::Nothing => return false,
        AfterQuit::Fail => {
            tracing::warn!("greycard not started again: a scripted run fails on the lost device");
            return true;
        }
        AfterQuit::StartAgain => {}
    }
    let Some(program) = program() else {
        tracing::warn!("greycard not started again: its own path is not known");
        return false;
    };
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let args = arguments(&args, given, current);
    let mut command = std::process::Command::new(&program);
    command.args(&args);
    match crate::adapter::scrub(&mut command).spawn() {
        Ok(child) => tracing::info!(
            "greycard started again as {} (pid {}) {}",
            program.display(),
            child.id(),
            args.iter()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Err(e) => tracing::warn!("greycard not started again: {e}"),
    }
    false
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
                Some(now)
            ),
            os(&["/no/such/dir/IMG_2.CR3", "--grid", "--cpu-ops"])
        );
        // Nothing open now: the file given.
        assert_eq!(
            arguments(&os(&["/no/such/dir/IMG_1.CR3"]), Some(given), None),
            os(&["/no/such/dir/IMG_1.CR3", "--cpu-ops"])
        );
        // No path given: the start finds the folder it remembers.
        assert_eq!(
            arguments(&os(&["--library", "x.sqlite"]), None, Some(now)),
            os(&["--library", "x.sqlite", "--cpu-ops"])
        );
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
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A scripted run fails on a loss under `--cpu-ops` too; an
    /// unscripted one starts again only off it.
    #[test]
    fn a_scripted_run_fails_on_any_loss() {
        let none = |_: bool| {};
        for cpu_ops in [false, true] {
            let loss = Loss::new();
            assert_eq!(loss.after_quit(true), AfterQuit::Nothing);
            loss.on_lost(wgpu::DeviceLostReason::Unknown, "", cpu_ops, &none, &|| {});
            assert_eq!(loss.after_quit(true), AfterQuit::Fail);
            let unscripted = if cpu_ops {
                AfterQuit::Nothing
            } else {
                AfterQuit::StartAgain
            };
            assert_eq!(loss.after_quit(false), unscripted);
        }
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
            arguments(&args, Some(&dir), Some(&now)),
            vec![dir.into_os_string(), "--grid".into(), "--cpu-ops".into()]
        );
    }

    /// The callback tells the worker and asks for the quit once, and
    /// the start again only without `--cpu-ops`: a run already on the
    /// CPU's ops quits saved, so a loss never turns into a loop of
    /// starts. A callback while leaving is the device let go.
    #[test]
    fn a_loss_quits_once_and_starts_again_only_off_cpu_ops() {
        for cpu_ops in [false, true] {
            let loss = Loss::new();
            let told = Mutex::new(Vec::new());
            let quits = Mutex::new(0);
            let tell = |again: bool| told.lock().unwrap().push(again);
            let quit = || *quits.lock().unwrap() += 1;
            let reason = wgpu::DeviceLostReason::Unknown;
            assert!(loss.on_lost(reason, "reset", cpu_ops, &tell, &quit));
            assert!(!loss.on_lost(reason, "again", cpu_ops, &tell, &quit));
            assert_eq!(*told.lock().unwrap(), [false]);
            assert_eq!(*quits.lock().unwrap(), 1);
            assert!(loss.lost.load(Ordering::SeqCst));
            assert_eq!(loss.again.load(Ordering::SeqCst), !cpu_ops);
        }
        let loss = Loss::new();
        loss.leaving.store(true, Ordering::SeqCst);
        let quit = || panic!("no quit while leaving");
        assert!(!loss.on_lost(
            wgpu::DeviceLostReason::Destroyed,
            "",
            false,
            &|_| panic!("nothing told while leaving"),
            &quit
        ));
        assert!(!loss.lost.load(Ordering::SeqCst));
    }
}
