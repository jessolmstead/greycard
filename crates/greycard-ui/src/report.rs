//! Report a problem: the bug report form on GitHub, opened in the
//! browser with what a tester cannot be expected to know already
//! filled in, the version, the OS and its version, the GPU, the CPU,
//! and the log's folder opened beside it so the file is one drag away.
//!
//! The form is `.github/ISSUE_TEMPLATE/bug.yml`, and GitHub fills an
//! issue form's fields from query parameters named by the fields'
//! ids, so those ids are the contract between this file and that one.
//! Nothing is sent from here: the browser shows the form, and the
//! tester reads it, adds to it and submits it or does not.

use std::path::Path;
use std::rc::Rc;

use anyhow::{Context, Result};
use slint::ComponentHandle;

use crate::{App, Shared, gpu, log, tr};

// A path of ours as shown, the home folder short, in the library
// beside the files, where the export queue's lines take it too.
#[cfg(test)]
use crate::files::abbreviate;
pub(crate) use crate::files::path_shown;

/// The fields of the form this fills, by id, in the form's order.
pub struct Report {
    pub version: String,
    pub os: String,
    pub gpu: String,
    pub cpu: String,
    pub log: String,
}

/// The form to open: `new` for the repository, the bug form chosen,
/// each field that has something in it filled. A field is held to
/// `FIELD` bytes once encoded, so the URL stays well inside what a
/// browser and GitHub take, whatever a driver string says.
pub fn issue_url(repository: &str, report: &Report) -> String {
    let mut url = format!(
        "{}/issues/new?template=bug.yml",
        repository.trim_end_matches('/')
    );
    for (id, value) in [
        ("version", &report.version),
        ("os", &report.os),
        ("gpu", &report.gpu),
        ("cpu", &report.cpu),
        ("log", &report.log),
    ] {
        if !value.is_empty() {
            url.push_str(&format!("&{id}={}", encode(value, FIELD)));
        }
    }
    url
}

/// The most a field may take in the URL, encoded.
const FIELD: usize = 1024;

/// `s` percent-encoded for a query value, everything but the
/// unreserved characters escaped, cut at a whole character before it
/// would pass `limit` bytes.
fn encode(s: &str, limit: usize) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let mut piece = String::new();
        let mut utf8 = [0; 4];
        for &b in c.encode_utf8(&mut utf8).as_bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                piece.push(b as char);
            } else {
                piece.push_str(&format!("%{b:02X}"));
            }
        }
        if out.len() + piece.len() > limit {
            break;
        }
        out.push_str(&piece);
    }
    out
}

/// The log field's text: where the log is, said without the user's
/// name in it, since the form is public, and which of the two files
/// to send.
pub fn log_note(log: &str) -> String {
    format!(
        "The log is {log}, in the folder greycard opened beside this form. \
         Drag greycard-ui.log here. If greycard closed on its own before this, \
         greycard-ui-1.log is that run: drag it too."
    )
}

/// The adapter as the log's start and the form both say it.
pub fn gpu_line(info: &gpu::AdapterInfo) -> String {
    format!(
        "{} ({:?}, {:?}), driver {} {}",
        info.name, info.backend, info.device_type, info.driver, info.driver_info
    )
    .trim_end()
    .to_string()
}

/// The OS, its version and the architecture, as a tester would be
/// asked for them: the build number on Windows, the version and
/// build on macOS, the distribution, kernel and session on Linux.
pub fn os() -> String {
    format!("{}, {}", os_version(), std::env::consts::ARCH)
}

/// Windows as the kernel says it, not as `GetVersionEx` does: that
/// answers 6.2 to a program with no compatibility manifest.
/// `RtlGetVersion` is ntdll's and has no such shim. Windows 11 still
/// calls itself 10.0 and is told apart by its build, 22000 onwards.
#[cfg(windows)]
fn os_version() -> String {
    #[repr(C)]
    struct OsVersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        service_pack: [u16; 128],
    }
    #[link(name = "ntdll", kind = "raw-dylib")]
    unsafe extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfo) -> i32;
    }
    let mut info = OsVersionInfo {
        size: std::mem::size_of::<OsVersionInfo>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform: 0,
        service_pack: [0; 128],
    };
    // SAFETY: `info` is an OSVERSIONINFOW with its size set, which is
    // all the call asks, and it writes only within that size.
    if unsafe { RtlGetVersion(&mut info) } != 0 {
        return "Windows (version unknown)".into();
    }
    windows_name(info.major, info.minor, info.build)
}

/// The name a Windows version goes by, and its numbers.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_name(major: u32, minor: u32, build: u32) -> String {
    let name = if major == 10 && build >= 22000 {
        "Windows 11"
    } else if major == 10 {
        "Windows 10"
    } else {
        "Windows"
    };
    format!("{name} {major}.{minor}.{build}")
}

/// macOS from `sw_vers`, which is what About This Mac reads: the
/// version and the build.
#[cfg(target_os = "macos")]
fn os_version() -> String {
    std::process::Command::new("sw_vers")
        .output()
        .ok()
        .and_then(|o| mac_name(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_else(|| "macOS (version unknown)".into())
}

/// `sw_vers`'s three lines as one: "macOS 15.3.1 (24D70)".
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac_name(sw_vers: &str) -> Option<String> {
    let field = |key: &str| {
        sw_vers.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == key).then(|| v.trim().to_string())
        })
    };
    let name = field("ProductName").unwrap_or_else(|| "macOS".into());
    let version = field("ProductVersion")?;
    Some(match field("BuildVersion") {
        Some(build) => format!("{name} {version} ({build})"),
        None => format!("{name} {version}"),
    })
}

/// Linux: the distribution from os-release, the kernel, and the
/// desktop and whether it is Wayland or X11, which the form used to
/// ask a tester for by hand.
#[cfg(not(any(windows, target_os = "macos")))]
fn os_version() -> String {
    let release = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .unwrap_or_default();
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    linux_name(
        &release,
        kernel.trim(),
        &var("XDG_CURRENT_DESKTOP"),
        &var("XDG_SESSION_TYPE"),
    )
}

/// "Fedora Linux 41 (Workstation Edition), kernel 6.11.4, GNOME on
/// Wayland", from os-release's text, the kernel release and the two
/// session variables; whatever is missing is left out.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn linux_name(os_release: &str, kernel: &str, desktop: &str, session: &str) -> String {
    let field = |key: &str| {
        os_release.lines().find_map(|l| {
            let v = l.strip_prefix(key)?.strip_prefix('=')?;
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
            (!v.is_empty()).then(|| v.to_string())
        })
    };
    let mut out = field("PRETTY_NAME")
        .or_else(|| field("NAME"))
        .unwrap_or_else(|| "Linux".into());
    if !kernel.is_empty() {
        out.push_str(&format!(", kernel {kernel}"));
    }
    // XDG_CURRENT_DESKTOP is a list, most specific first.
    let desktop = desktop.split(':').next().unwrap_or("");
    let session = match session {
        "wayland" => "Wayland",
        "x11" => "X11",
        other => other,
    };
    match (desktop, session) {
        ("", "") => {}
        ("", s) => out.push_str(&format!(", {s}")),
        (d, "") => out.push_str(&format!(", {d}")),
        (d, s) => out.push_str(&format!(", {d} on {s}")),
    }
    out
}

/// The processor as the system names it, as the form's placeholder
/// has it: `/proc/cpuinfo`'s model name on Linux, the brand string
/// CPUID answers on any other x86-64 machine, `sysctl` on a Mac.
/// Empty, for the tester to fill in, where none of those answers, as
/// on Windows on ARM.
pub fn cpu() -> String {
    cpu_name().unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn cpu_name() -> Option<String> {
    let out = crate::adapter::scrub(&mut std::process::Command::new("sysctl"))
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !name.is_empty()).then_some(name)
}

#[cfg(windows)]
fn cpu_name() -> Option<String> {
    greycard_ai::runtime::processor_name()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn cpu_name() -> Option<String> {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|text| cpuinfo_name(&text))
        .or_else(greycard_ai::runtime::processor_name)
}

/// The first processor's model name in `/proc/cpuinfo`'s text; ARM
/// kernels often give none.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn cpuinfo_name(cpuinfo: &str) -> Option<String> {
    cpuinfo.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        let v = v.trim();
        (k.trim() == "model name" && !v.is_empty()).then(|| v.to_string())
    })
}

/// Show `file` selected in the platform's file manager: Explorer's
/// `/select`, Finder's `open -R`, and on Linux the file manager's
/// `ShowItems` over D-Bus, which Nautilus, Dolphin, Nemo and Thunar
/// all answer, or the folder through `xdg-open` when none does.
/// Blocks until the file manager has been asked.
#[cfg(windows)]
fn reveal(file: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    // Explorer reads its own command line and wants the path quoted
    // after the comma, which the standard library's quoting of the
    // whole argument would not do. Its exit code means nothing.
    crate::adapter::scrub(&mut std::process::Command::new("explorer"))
        .raw_arg(format!("/select,\"{}\"", file.display()))
        .status()
        .context("starting Explorer")?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn reveal(file: &Path) -> Result<()> {
    let status = crate::adapter::scrub(&mut std::process::Command::new("open"))
        .arg("-R")
        .arg(file)
        .status()
        .context("starting open")?;
    anyhow::ensure!(status.success(), "open -R: {status}");
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn reveal(file: &Path) -> Result<()> {
    use zbus::blocking::{Connection, Proxy};
    const SERVICE: &str = "org.freedesktop.FileManager1";
    let uri = format!(
        "file://{}",
        file.to_string_lossy()
            .split('/')
            .map(|c| encode(c, usize::MAX))
            .collect::<Vec<_>>()
            .join("/")
    );
    let shown = Connection::session().and_then(|conn| {
        let manager = Proxy::new(&conn, SERVICE, "/org/freedesktop/FileManager1", SERVICE)?;
        manager.call_method("ShowItems", &(vec![uri], ""))?;
        Ok(())
    });
    if let Err(e) = shown {
        tracing::debug!("report: no file manager on the bus ({e}); xdg-open instead");
        let dir = file.parent().unwrap_or(file);
        let status = crate::adapter::scrub(&mut std::process::Command::new("xdg-open"))
            .arg(dir)
            .status()
            .context("starting xdg-open")?;
        anyhow::ensure!(status.success(), "xdg-open: {status}");
    }
    Ok(())
}

/// The button: the form in the browser, the log's folder beside it,
/// and the status line saying what to do with the two. The GPU is
/// what the rendering setup found, or nothing for the tester to fill
/// in when it has not run.
pub(crate) fn install(app: &App, state: &Rc<Shared>) {
    let (state, app_weak) = (state.clone(), app.as_weak());
    app.on_report_asked(move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let log = log::dir()
            .map(|d| d.join(log::FILE))
            .filter(|p| p.is_file());
        let report = Report {
            version: greycard_version::LABEL.into(),
            os: os(),
            gpu: state.borrow().develop.gpu.clone().unwrap_or_default(),
            cpu: cpu(),
            log: log
                .as_deref()
                .map(|p| log_note(&path_shown(p)))
                .unwrap_or_default(),
        };
        let url = issue_url(env!("CARGO_PKG_REPOSITORY"), &report);
        tracing::info!("report: opening {url}");
        app.set_status(match log {
            Some(_) => tr!("the form is open in the browser, and the log's folder: \
                 drag greycard-ui.log into the form")
            .into(),
            None => tr!("the form is open in the browser; there is no log to attach").into(),
        });
        // Off the event loop: a browser or a file manager can take
        // its time to answer, and xdg-open waits for it.
        // A test that clicks its way across the pane must not open
        // the browser, once per click.
        if cfg!(test) {
            return;
        }
        let app_weak = app.as_weak();
        std::thread::spawn(move || {
            if let Err(e) = crate::web::open_url(&url) {
                tracing::warn!("report: no browser would open the form: {e}");
                let _ = app_weak.upgrade_in_event_loop(|app| {
                    app.set_status(
                        tr!(
                            "no browser would open: the form is at {}/issues/new",
                            env!("CARGO_PKG_REPOSITORY")
                        )
                        .into(),
                    );
                });
            }
            if let Some(log) = log
                && let Err(e) = reveal(&log)
            {
                tracing::warn!("report: the log's folder would not open: {e:#}");
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Report {
        Report {
            version: "0.1.0".into(),
            os: "Windows 11 10.0.26200, x86_64".into(),
            gpu: "NVIDIA GeForce RTX 5070 Ti (Vulkan, DiscreteGpu), driver NVIDIA 581.57".into(),
            cpu: "AMD Ryzen 7 7800X3D 8-Core Processor".into(),
            log: log_note(r"%LOCALAPPDATA%\greycard\logs\greycard-ui.log"),
        }
    }

    #[test]
    fn the_url_chooses_the_form_and_fills_its_fields_by_id() {
        let url = issue_url("https://github.com/jessolmstead/greycard/", &report());
        assert!(
            url.starts_with(
                "https://github.com/jessolmstead/greycard/issues/new?template=bug.yml&"
            ),
            "{url}"
        );
        assert!(url.contains("&version=0.1.0&"), "{url}");
        assert!(
            url.contains("&os=Windows%2011%2010.0.26200%2C%20x86_64&"),
            "{url}"
        );
        assert!(url.contains("&gpu=NVIDIA%20GeForce%20RTX%205070%20Ti%20%28Vulkan%2C%20DiscreteGpu%29%2C%20driver%20NVIDIA%20581.57&"), "{url}");
        assert!(
            url.contains("&cpu=AMD%20Ryzen%207%207800X3D%208-Core%20Processor&"),
            "{url}"
        );
        assert!(
            url.contains(
                "&log=The%20log%20is%20%25LOCALAPPDATA%25%5Cgreycard%5Clogs%5Cgreycard-ui.log%2C%20"
            ),
            "{url}"
        );
        // Nothing unescaped past the query's own separators.
        let query = url.split_once('?').unwrap().1;
        for pair in query.split('&') {
            let (_, v) = pair.split_once('=').unwrap();
            assert!(
                v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.~%".contains(&b)),
                "{v}"
            );
        }
    }

    #[test]
    fn an_empty_field_is_left_to_the_tester() {
        let r = Report {
            gpu: String::new(),
            cpu: String::new(),
            log: String::new(),
            ..report()
        };
        let url = issue_url("https://github.com/jessolmstead/greycard", &r);
        assert!(!url.contains("gpu="), "{url}");
        assert!(!url.contains("cpu="), "{url}");
        assert!(!url.contains("log="), "{url}");
        assert!(url.contains("&os="), "{url}");
    }

    #[test]
    fn the_encoding_is_utf8_and_stops_at_a_whole_character() {
        assert_eq!(encode("a b&c=d/é", usize::MAX), "a%20b%26c%3Dd%2F%C3%A9");
        assert_eq!(encode("é", 5), "");
        assert_eq!(encode("aé", 6), "a");
        assert_eq!(encode("aé", 7), "a%C3%A9");
        // A field of any length keeps the URL within reach.
        let long = Report {
            gpu: "ü".repeat(10_000),
            os: "x".repeat(10_000),
            ..report()
        };
        let url = issue_url("https://github.com/jessolmstead/greycard", &long);
        assert!(url.len() < 8 * 1024, "{}", url.len());
    }

    #[test]
    fn the_log_path_leaves_the_account_out() {
        let home = std::env::temp_dir().join("home").join("jess");
        let local = home.join("AppData").join("Local");
        let log = local.join("greycard").join("logs").join("greycard-ui.log");
        let sep = std::path::MAIN_SEPARATOR;
        let prefixes = [
            (Some(local.clone()), "%LOCALAPPDATA%"),
            (Some(home.clone()), "~"),
        ];
        let shown = abbreviate(&log, &prefixes);
        assert_eq!(
            shown,
            format!(
                "%LOCALAPPDATA%{sep}{}",
                Path::new("greycard")
                    .join("logs")
                    .join("greycard-ui.log")
                    .display()
            )
        );
        let elsewhere = home.join(".local").join("state").join("greycard-ui.log");
        assert!(abbreviate(&elsewhere, &prefixes).starts_with(&format!("~{sep}.local")));
        let outside = std::env::temp_dir().join("greycard-ui.log");
        assert_eq!(
            abbreviate(&outside, &prefixes),
            outside.display().to_string()
        );
        assert_eq!(abbreviate(&log, &[(None, "~")]), log.display().to_string());
    }

    #[test]
    fn windows_is_told_apart_by_its_build() {
        assert_eq!(windows_name(10, 0, 26200), "Windows 11 10.0.26200");
        assert_eq!(windows_name(10, 0, 19045), "Windows 10 10.0.19045");
        assert_eq!(windows_name(6, 3, 9600), "Windows 6.3.9600");
    }

    #[test]
    fn macos_from_sw_vers() {
        let out = "ProductName:\t\tmacOS\nProductVersion:\t\t15.3.1\nBuildVersion:\t\t24D70\n";
        assert_eq!(mac_name(out).as_deref(), Some("macOS 15.3.1 (24D70)"));
        assert_eq!(
            mac_name("ProductVersion: 14.7\n").as_deref(),
            Some("macOS 14.7")
        );
        assert_eq!(mac_name(""), None);
    }

    #[test]
    fn linux_from_os_release_the_kernel_and_the_session() {
        let release = "NAME=\"Fedora Linux\"\nVERSION_ID=41\n\
                       PRETTY_NAME=\"Fedora Linux 41 (Workstation Edition)\"\n";
        assert_eq!(
            linux_name(release, "6.11.4-301.fc41.x86_64", "GNOME", "wayland"),
            "Fedora Linux 41 (Workstation Edition), kernel 6.11.4-301.fc41.x86_64, GNOME on Wayland"
        );
        assert_eq!(
            linux_name("NAME=Arch Linux\n", "6.12.1", "KDE:plasma", "x11"),
            "Arch Linux, kernel 6.12.1, KDE on X11"
        );
        // PRETTY_NAME_EXTRA is not PRETTY_NAME.
        assert_eq!(
            linux_name("PRETTY_NAME_EXTRA=no\nNAME='Debian'\n", "", "", "tty"),
            "Debian, tty"
        );
        assert_eq!(linux_name("", "", "", ""), "Linux");
    }

    #[test]
    fn the_cpu_from_cpuinfo() {
        let x86 = "processor\t: 0\nvendor_id\t: AuthenticAMD\n\
                   model name\t: AMD Ryzen 7 7800X3D 8-Core Processor\n\n\
                   processor\t: 1\nmodel name\t: AMD Ryzen 7 7800X3D 8-Core Processor\n";
        assert_eq!(
            cpuinfo_name(x86).as_deref(),
            Some("AMD Ryzen 7 7800X3D 8-Core Processor")
        );
        // An ARM kernel's: no model name, and no guess at one.
        let arm = "processor\t: 0\nBogoMIPS\t: 108.00\nCPU implementer\t: 0x41\n\
                   CPU part\t: 0xd0b\n";
        assert_eq!(cpuinfo_name(arm), None);
        assert_eq!(cpuinfo_name("model name\t:\n"), None);
    }

    #[test]
    fn this_machine_is_named_with_its_version() {
        let os = os();
        assert!(os.ends_with(std::env::consts::ARCH), "{os}");
        assert!(!os.contains("unknown"), "{os}");
        if cfg!(windows) {
            assert!(os.starts_with("Windows 1"), "{os}");
        } else if cfg!(target_os = "macos") {
            assert!(
                os.starts_with("macOS 1") || os.starts_with("macOS 2"),
                "{os}"
            );
        }
        // Every machine CI builds on names its processor.
        if cfg!(any(target_arch = "x86_64", target_os = "macos")) {
            assert!(!cpu().is_empty());
        }
    }
}
