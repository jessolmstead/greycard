//! The editor's log: what it and the libraries under it say, kept in
//! a file so a tester who never opens a terminal still has something
//! to send, and shown on the terminal from warnings up.
//!
//! Every crate speaks through the `log` or `tracing` facade, and this
//! is the one subscriber that listens: a file sink at info, a terminal
//! sink at warn, `-v` for more and `RUST_LOG` for exact control over
//! both. wgpu, naga, rawler, ureq and rfd arrive through `log`; ONNX
//! Runtime's own logger arrives through `tracing`, forwarded by the
//! ort crate. A panic in any thread is written with its backtrace.
//! Nothing here touches a file descriptor: the odd line a C library
//! prints straight to stderr goes where the desktop keeps it, the
//! unified log on a Mac and the user journal on Linux.

use std::fs::File;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Layer};

/// The log's name; the previous run's is kept under `.1`, because a
/// tester whose editor vanished opens it again before thinking of the
/// log.
pub const FILE: &str = "greycard-ui.log";

/// The crates whose lines are ours: the file keeps them from info,
/// and `-v` raises them. Everything else, the libraries, from warn.
const OURS: [&str; 6] = [
    "greycard_ui",
    "greycard_core",
    "greycard_edit",
    "greycard_ai",
    "greycard_lens",
    "greycard_gpu",
];

/// `$XDG_STATE_HOME/greycard` when that is set, else the platform's
/// place for logs: `~/Library/Logs/greycard` on macOS, where
/// Console.app looks; `~/.local/state/greycard` on Linux, which is
/// the XDG state directory's default; `%LOCALAPPDATA%\greycard\logs`
/// on Windows.
pub fn dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return Some(p.join("greycard"));
    }
    if cfg!(target_os = "macos") {
        Some(
            dirs::home_dir()?
                .join("Library")
                .join("Logs")
                .join("greycard"),
        )
    } else if cfg!(windows) {
        Some(dirs::data_local_dir()?.join("greycard").join("logs"))
    } else {
        Some(dirs::state_dir()?.join("greycard"))
    }
}

/// Start listening, and say where the file is: none when there is no
/// directory to keep it in, and the editor goes on with the terminal
/// alone. `verbose` is the count of `-v`: once, the terminal shows
/// what the file keeps; twice, both go down to debug for our crates.
/// `RUST_LOG`, when set, decides for both instead.
pub fn start(verbose: u8) -> Option<PathBuf> {
    let file = dir().and_then(open);
    install(file.as_ref().map(|(_, f)| f), verbose, true);
    file.map(|(path, _)| path)
}

/// The file for this run, the previous one moved to `.1`, with a
/// first line that says what was run even if nothing else follows.
fn open(dir: PathBuf) -> Option<(PathBuf, File)> {
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(FILE);
    let _ = std::fs::rename(&path, dir.join(format!("{FILE}.1")));
    let mut file = File::create(&path).ok()?;
    writeln!(
        file,
        "greycard-ui {} on {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
    .ok()?;
    Some((path, file))
}

/// Libraries whose warnings are known and benign, kept to errors:
/// arboard says twice at every start that the compositor has no
/// data-control protocol and it is using the X11 clipboard instead,
/// which is every GNOME desktop and nothing to act on. rawler's lens
/// resolver warns on every decode whose lens its own database lacks
/// (the Sigma 28 Art, the Sony 24-70 GM II) and asks for an upstream
/// issue; the corrections here come from lensfun, so its lookup is
/// nothing we act on either. ONNX Runtime's optimizer warns per
/// session build that it cannot constant-fold a node the WebGPU
/// provider has no CPU kernel for, which is how the graph runs.
const QUIET: [&str; 4] = [
    "arboard",
    "rawler::lens",
    "rawler::decoders",
    "ort::logging",
];

/// Our crates at `ours`, the libraries at warn, the known chatter at
/// error.
fn filter(ours: &str) -> EnvFilter {
    let mut spec = String::from("warn");
    for target in OURS {
        spec.push_str(&format!(",{target}={ours}"));
    }
    for target in QUIET {
        spec.push_str(&format!(",{target}=error"));
    }
    EnvFilter::new(spec)
}

/// The filter for a sink whose default for our crates is `ours`:
/// `RUST_LOG` when it is set, else the default.
fn filter_or_env(ours: &str) -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| filter(ours))
}

/// `terminal` is whether stderr listens too. The test binary says
/// no: the subscriber is global to the process, so a terminal sink
/// installed by one test would print every later test's expected
/// failures as ERROR lines through a green run.
fn install(file: Option<&File>, verbose: u8, terminal: bool) {
    let env_set = std::env::var_os(EnvFilter::DEFAULT_ENV).is_some();
    let (to_file, to_terminal) = match verbose {
        0 => ("info", "warn"),
        1 => ("info", "info"),
        _ => ("debug", "debug"),
    };
    let file_layer = file.map(|f| {
        let sink = Sink(Mutex::new(
            f.try_clone().expect("a duplicate of the log's handle"),
        ));
        tracing_subscriber::fmt::layer()
            .with_writer(sink)
            .with_ansi(false)
            .with_target(true)
            .with_thread_names(true)
            .with_timer(Uptime(std::time::Instant::now()))
            .with_filter(filter_or_env(to_file))
    });
    let terminal_layer = terminal.then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(std::io::stderr().is_terminal())
            .with_target(false)
            .without_time()
            .with_filter(filter_or_env(to_terminal))
    });
    let subscriber = tracing_subscriber::registry()
        .with(file_layer)
        .with(terminal_layer);
    // The `log` facade's records come over as events. Its own level
    // gate is set so a library's trace lines cost nothing unless
    // somebody asked for them.
    let max = if env_set || verbose > 1 {
        log::LevelFilter::Trace
    } else {
        log::LevelFilter::Info
    };
    let _ = tracing_log::LogTracer::builder().with_max_level(max).init();
    // Fails only when one is installed already, which is a test binary.
    let _ = tracing::subscriber::set_global_default(subscriber);
    std::panic::set_hook(Box::new(on_panic));
}

/// A panic, in whatever thread, as one error event with the message,
/// the place and a backtrace, so the file says why before the process
/// goes. This replaces the default hook rather than following it:
/// the terminal sink passes errors, so the terminal sees it too.
fn on_panic(info: &std::panic::PanicHookInfo<'_>) {
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("?");
    let message = info.payload_as_str().unwrap_or("(no message)");
    let at = info
        .location()
        .map(|l| format!(" at {}:{}", l.file(), l.line()))
        .unwrap_or_default();
    let backtrace = std::backtrace::Backtrace::force_capture();
    tracing::error!("thread '{thread}' panicked{at}: {message}\n{backtrace}");
}

/// Seconds since the log started, to the millisecond: enough to see
/// what took long, without a wall clock the reader has to subtract.
struct Uptime(std::time::Instant);

impl tracing_subscriber::fmt::time::FormatTime for Uptime {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        write!(w, "{:9.3}s", self.0.elapsed().as_secs_f64())
    }
}

/// The file behind a lock, taken even when poisoned: the panic hook
/// writes through here, and a panic while writing must not become a
/// second one.
struct Sink(Mutex<File>);

struct Held<'a>(MutexGuard<'a, File>);

impl Write for Held<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl<'a> MakeWriter<'a> for Sink {
    type Writer = Held<'a>;
    fn make_writer(&'a self) -> Held<'a> {
        Held(self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directory_is_the_platforms_or_the_variables() {
        // The variable wins when it is set and absolute.
        let dir = std::env::temp_dir().join(format!("greycard-log-{}", std::process::id()));
        // SAFETY: the variable is read only by this module's `dir`,
        // and no other test in this binary sets it.
        unsafe { std::env::set_var("XDG_STATE_HOME", &dir) };
        assert_eq!(super::dir(), Some(dir.join("greycard")));
        unsafe { std::env::set_var("XDG_STATE_HOME", "relative/no") };
        let platform = super::dir().expect("a state directory in the test environment");
        assert!(platform.is_absolute());
        assert!(platform.ends_with("greycard") || platform.ends_with("greycard/logs"));
        unsafe { std::env::remove_var("XDG_STATE_HOME") };
        assert_eq!(super::dir(), Some(platform));
    }

    #[test]
    fn the_filter_keeps_ours_and_quiets_the_rest() {
        let spec = filter("info").to_string();
        assert!(spec.split(',').any(|d| d == "warn"), "{spec}");
        assert!(spec.contains("greycard_core=info"), "{spec}");
        assert!(!spec.contains("wgpu"), "{spec}");
        assert!(spec.contains("arboard=error"), "{spec}");
        assert!(spec.contains("rawler::lens=error"), "{spec}");
    }

    /// The one test that installs the subscriber; a process takes
    /// only one, so anything else about what reaches the file goes
    /// in here.
    #[test]
    fn what_is_said_and_a_panic_reach_the_file_and_the_previous_run_is_kept() {
        let dir = std::env::temp_dir().join(format!("greycard-log-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: read only by `filter_or_env`, and only this test
        // installs; the shell's setting would decide the levels.
        unsafe { std::env::remove_var(EnvFilter::DEFAULT_ENV) };
        // A run before this one, so there is something to keep.
        let (_, mut first) = open(dir.clone()).unwrap();
        writeln!(first, "first run").unwrap();
        drop(first);

        let (path, file) = open(dir.clone()).unwrap();
        install(Some(&file), 0, false);
        tracing::info!("an info line");
        tracing::debug!("a debug line");
        log::warn!(target: "some_library", "a library's warning");
        log::info!(target: "some_library", "a library's chatter");
        let worker = std::thread::Builder::new()
            .name("worker".into())
            .spawn(|| panic!("on purpose"))
            .unwrap();
        assert!(worker.join().is_err());

        let now = std::fs::read_to_string(&path).unwrap();
        let before = std::fs::read_to_string(dir.join(format!("{FILE}.1"))).unwrap();
        assert!(now.starts_with("greycard-ui "), "{now:?}");
        assert!(now.contains("an info line"), "{now:?}");
        assert!(!now.contains("a debug line"), "{now:?}");
        assert!(now.contains("a library's warning"), "{now:?}");
        assert!(!now.contains("a library's chatter"), "{now:?}");
        assert!(now.contains("thread 'worker' panicked"), "{now:?}");
        assert!(now.contains("on purpose"), "{now:?}");
        assert!(now.contains("log.rs"), "the place: {now:?}");
        assert!(before.contains("first run"), "{before:?}");
        assert!(!now.contains("first run"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
