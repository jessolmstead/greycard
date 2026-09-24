//! The tool's log is its stderr, as a command-line tool's should be:
//! one subscriber to the terminal, warnings from everything by
//! default, `-v` for what greycard's own crates note along the way and
//! `-vv` for their debug lines, `RUST_LOG` for exact control. The
//! libraries arrive through `log`, ONNX Runtime's logger through
//! `tracing` by way of the ort crate. Progress and results are the
//! tool's output and stay as plain prints; this is for what goes wrong
//! and what was found on the way.

use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;

/// The crates whose lines `-v` raises; the libraries stay at warn.
/// The tool's own target is the binary's name, `greycard`, not the
/// package's; a directive matches a target only up to a `::`, so it
/// does not take the other crates with it.
const OURS: [&str; 6] = [
    "greycard",
    "greycard_core",
    "greycard_edit",
    "greycard_ai",
    "greycard_lens",
    "greycard_library",
];

pub fn start(verbose: u8) {
    let env_set = std::env::var_os(EnvFilter::DEFAULT_ENV).is_some();
    let ours = match verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        let mut spec = String::from("warn");
        for target in OURS {
            spec.push_str(&format!(",{target}={ours}"));
        }
        EnvFilter::new(spec)
    });
    let max = if env_set || verbose > 1 {
        log::LevelFilter::Trace
    } else {
        log::LevelFilter::Info
    };
    let _ = tracing_log::LogTracer::builder().with_max_level(max).init();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_target(false)
        .without_time()
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}
