//! A start that dies while the window's GPU is set up, and the next
//! start's way around it.
//!
//! A driver can crash the process as Slint makes its wgpu instance:
//! Intel's legacy Vulkan driver for the UHD 630 on Windows does, beside
//! a GPU that would draw fine. Nothing in the process can catch that,
//! so before Slint's select we leave a marker in the cache saying
//! which backends this start set up on, and take it off once the
//! select has come back. A marker found at the start means the last
//! start died there. On Windows the next start then goes through DX12
//! alone, by `WGPU_BACKEND`, which Slint reads, so the Vulkan loader
//! never loads the driver; once that works the marker keeps saying so,
//! and later starts go to DX12 straight. Should DX12 die as well, the
//! start goes on as it always has, with a warning, rather than turn
//! between the two for ever. Elsewhere there is no second backend
//! Slint draws with (GL it avoids), so the marker only says so in the
//! log. A `WGPU_BACKEND` set by hand is left alone, the marker too.
//!
//! The marker covers the select alone, a few hundred milliseconds:
//! Slint's check there makes a throwaway instance and enumerates its
//! adapters, loading every driver, which is where the Intel driver
//! dies. Through the first frame it would also have counted a start
//! killed, logged off, or out of memory while slow as a death. What
//! the narrow window gives up: a driver that crashes only as the real
//! device is made, after the select, and a `SLINT_WGPU_CPU` set by
//! hand, with which the select skips the enumeration, so a crash there
//! too comes after it. A start whose select we sent to DX12 whose
//! event loop ends with no device made, the device refused inside it,
//! counts DX12 as failed, so the next start goes back to the default;
//! one that returns earlier, an error of its own (a preset misnamed),
//! leaves the marker as the select left it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The variable wgpu, and so Slint, reads for the backends to load.
const WGPU_BACKEND: &str = "WGPU_BACKEND";

/// Set beside `WGPU_BACKEND` where we set it: a greycard started from
/// anything we start inherits both, and reads the first as ours and
/// not as set by hand.
const OURS: &str = "GREYCARD_SET_WGPU_BACKEND";

/// Whether we set `WGPU_BACKEND` ourselves, and so take it off the
/// commands we start that let us.
static WE_SET: AtomicBool = AtomicBool::new(false);

/// The backends a start sets the window up on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    /// Slint's own choice, with nothing set.
    Default,
    /// DX12 alone.
    Dx12,
}

impl Backend {
    fn name(self) -> &'static str {
        match self {
            Backend::Default => "default",
            Backend::Dx12 => "dx12",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "default" => Some(Backend::Default),
            "dx12" => Some(Backend::Dx12),
            _ => None,
        }
    }
}

/// What the marker file holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Marker {
    /// The backends a start was setting up on and did not finish: it
    /// died there, unless it is this start.
    pub(crate) setup: Option<Backend>,
    /// Those a start died setting up on before.
    pub(crate) died: Vec<Backend>,
}

impl Marker {
    /// The file's text. A line a word: the reader takes what it knows.
    pub(crate) fn text(&self) -> String {
        let mut text = String::from("# greycard: GPU setup under way, or died in it\n");
        if let Some(setup) = self.setup {
            text.push_str(&format!("setup: {}\n", setup.name()));
        }
        if !self.died.is_empty() {
            let died: Vec<_> = self.died.iter().map(|b| b.name()).collect();
            text.push_str(&format!("died: {}\n", died.join(" ")));
        }
        text
    }

    /// The file's text read back; what it does not know it leaves out.
    pub(crate) fn parse(text: &str) -> Self {
        let mut marker = Marker::default();
        for line in text.lines() {
            if let Some(name) = line.strip_prefix("setup:") {
                marker.setup = Backend::parse(name.trim());
            } else if let Some(names) = line.strip_prefix("died:") {
                for b in names.split_whitespace().filter_map(Backend::parse) {
                    if !marker.died.contains(&b) {
                        marker.died.push(b);
                    }
                }
            }
        }
        marker
    }
}

/// What to do to the environment before the select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Env {
    /// Nothing: the variable is the user's.
    Leave,
    /// Set it, to the name given, with our marker beside it.
    Set(&'static str),
    /// Take off one a greycard set for itself, so Slint chooses.
    Clear,
}

/// What to do to the marker file at a moment of the start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum File {
    Leave,
    Write(Marker),
    Remove,
}

/// The start's choice from what the marker said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) env: Env,
    /// Written before the select.
    pub(crate) now: File,
    /// Once the window's device is made.
    pub(crate) made: File,
    /// Said at warn, and at info.
    pub(crate) warn: Option<String>,
    pub(crate) info: Option<String>,
}

/// The choice, from the marker's text (`None` for no file), whether
/// another start holds its lock (`live`: that start is setting up now,
/// and its `setup` line is no death), the `WGPU_BACKEND` set by hand if
/// any, and the backend to fall back on here (DX12 on Windows, none
/// elsewhere). A live start's marker is its own: this one reads what
/// died before and writes nothing.
pub(crate) fn decide(
    found: Option<&str>,
    live: bool,
    by_hand: Option<&str>,
    fallback: Option<Backend>,
) -> Plan {
    let mut marker = found.map(Marker::parse).unwrap_or_default();
    if live {
        marker.setup = None;
    }
    if let Some(by_hand) = by_hand {
        return Plan {
            env: Env::Leave,
            now: File::Leave,
            made: File::Leave,
            warn: marker.setup.map(|b| {
                format!(
                    "the last start died during GPU setup on the {} backends; {WGPU_BACKEND}={by_hand} is set by hand, so this start takes it",
                    b.name()
                )
            }),
            info: None,
        };
    }
    let mut died = marker.died.clone();
    if let Some(b) = marker.setup
        && !died.contains(&b)
    {
        died.push(b);
    }
    let pick = std::iter::once(Backend::Default)
        .chain(fallback)
        .find(|b| !died.contains(b));
    let backend = pick.unwrap_or(Backend::Default);
    let tried: Vec<_> = died.iter().map(|b| b.name()).collect();
    let tried = tried.join(" and ");
    let (warn, info) = match (marker.setup, pick) {
        (_, None) if fallback.is_none() => (
            Some(format!(
                "the last start died during GPU setup on the {tried} backends; there is no other Slint draws with here, so this start goes on as before"
            )),
            None,
        ),
        (_, None) => (
            Some(format!(
                "starts died during GPU setup on the {tried} backends; this start goes on with the default backends"
            )),
            None,
        ),
        (Some(last), Some(b)) => (
            Some(format!(
                "the last start died during GPU setup on the {} backends; this start sets up on {}",
                last.name(),
                b.name()
            )),
            None,
        ),
        (None, Some(Backend::Dx12)) => (
            None,
            Some(format!(
                "GPU setup on dx12: a start died setting up on the {tried} backends before"
            )),
        ),
        (None, Some(Backend::Default)) => (None, None),
    };
    // A death the live start counted is its to say.
    if live {
        return Plan {
            env: env_for(backend),
            now: File::Leave,
            made: File::Leave,
            warn: None,
            info,
        };
    }
    Plan {
        env: env_for(backend),
        now: File::Write(Marker {
            setup: Some(backend),
            died: died.clone(),
        }),
        // The default backends working leave nothing to remember; the
        // fallback working is kept, so the next start goes there too.
        made: match backend {
            Backend::Default => File::Remove,
            Backend::Dx12 => File::Write(Marker { setup: None, died }),
        },
        warn,
        info,
    }
}

fn env_for(backend: Backend) -> Env {
    match backend {
        Backend::Default => Env::Clear,
        Backend::Dx12 => Env::Set("dx12"),
    }
}

/// The marker once the select on the fallback came back an error, not
/// a death: the default backends tried next, in this process, with the
/// fallback counted as failed so a death on the default after it does
/// not send the next start back to it.
pub(crate) fn after_fallback_error(died: &[Backend]) -> Marker {
    let mut died: Vec<_> = died
        .iter()
        .copied()
        .filter(|b| *b != Backend::Default)
        .collect();
    if !died.contains(&Backend::Dx12) {
        died.push(Backend::Dx12);
    }
    Marker {
        setup: Some(Backend::Default),
        died,
    }
}

/// [`decide`]'s `by_hand` from an environment `var` reads: a
/// `WGPU_BACKEND` a greycard set, with its marker beside it, is not
/// the user's.
fn by_hand_in(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<String> {
    let value = var(WGPU_BACKEND)?;
    var(OURS)
        .is_none()
        .then(|| value.to_string_lossy().into_owned())
}

/// The marker's place under a cache root.
fn path_under(cache: &Path) -> PathBuf {
    cache.join("greycard").join("gpu-setup")
}

/// The lock beside the marker, held by a start through its setup: the
/// marker stays readable to a second start (Windows' locks keep others
/// from reading a locked file), which finds the lock held and so the
/// marker live. The system lets go when the process ends, however.
fn lock_path(marker: &Path) -> PathBuf {
    marker.with_extension("lock")
}

/// The setup's lock taken, or `None` where another start holds it.
/// Where the file system cannot lock, the start goes on as the owner,
/// as it would with no lock at all.
fn take_lock(marker: &Path) -> Option<Option<std::fs::File>> {
    let path = lock_path(marker);
    let opened = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)
        });
    let file = match opened {
        Ok(file) => file,
        Err(e) => {
            tracing::warn!("GPU setup lock {}: {e}", path.display());
            return Some(None);
        }
    };
    match file.try_lock() {
        Ok(()) => Some(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => None,
        Err(std::fs::TryLockError::Error(e)) => {
            tracing::warn!("GPU setup lock {}: {e}", path.display());
            Some(None)
        }
    }
}

/// The marker's place: beside the thumbnail cache, under
/// `$XDG_CACHE_HOME` when that is set, else the platform's cache.
fn path() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::cache_dir)
        .map(|c| path_under(&c))
}

fn apply(path: &Path, file: &File) {
    let done = match file {
        File::Leave => Ok(()),
        File::Write(marker) => path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(path, marker.text())),
        File::Remove => match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            done => done,
        },
    };
    if let Err(e) = done {
        tracing::warn!("GPU setup marker {}: {e}", path.display());
    }
}

/// The start's marker while the window's device is not made yet: what
/// to leave once it is, and the file as it was found, put back if the
/// start ends without one and without dying.
struct Pending {
    path: PathBuf,
    made: File,
    found: Option<String>,
    /// Whether the marker is this start's to write: not where another
    /// start is setting up.
    owned: bool,
    /// The setup's lock, let go once the select has come back.
    lock: Option<std::fs::File>,
    /// Whether the select has come back, and `made` been applied.
    selected: bool,
}

/// How a start with no device made ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// Returned on the way, an error of its own.
    Returned,
    /// Its event loop ended.
    RunEnded,
}

/// What the marker is left as when the start ends with no device
/// made: before the select came back, as it was found, since the start
/// ended and did not die; after it, when the event loop ended on the
/// fallback we set, with that counted as failed, so the next start
/// does not go back to it; otherwise as the select left it.
fn at_end(selected: bool, end: End, we_set: bool, found: Option<&str>, made: &File) -> File {
    match (selected, end, we_set) {
        (false, _, _) => match found {
            Some(text) => File::Write(Marker::parse(text)),
            None => File::Remove,
        },
        (true, End::RunEnded, true) => {
            let died = match made {
                File::Write(marker) => marker.died.clone(),
                _ => Vec::new(),
            };
            File::Write(Marker {
                setup: None,
                ..after_fallback_error(&died)
            })
        }
        (true, _, _) => File::Leave,
    }
}

/// The marker left as [`at_end`] has it, where this start owns it and
/// made no device.
fn end(how: End) {
    if let Some(p) = take_pending().filter(|p| p.owned) {
        let we_set = WE_SET.load(Ordering::Relaxed);
        let file = at_end(p.selected, how, we_set, p.found.as_deref(), &p.made);
        apply(&p.path, &file);
    }
}

/// Right after the window's event loop ends: one that ended with no
/// device made, on the fallback, counts it as failed.
pub(crate) fn run_ended() {
    end(End::RunEnded);
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

fn take_pending() -> Option<Pending> {
    PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// Held by the window's start: dropped with no device made, it leaves
/// the marker as [`at_end`] has it.
pub(crate) struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        end(End::Returned);
    }
}

/// Before Slint's select, and before `WGPUSettings` reads the
/// environment: the marker read, said, and written, and
/// `WGPU_BACKEND` set to match. On the window's thread, once.
pub(crate) fn start() -> Guard {
    let Some(path) = path() else {
        return Guard;
    };
    let lock = take_lock(&path);
    let live = lock.is_none();
    let found = std::fs::read_to_string(&path).ok();
    let fallback = cfg!(target_family = "windows").then_some(Backend::Dx12);
    let plan = decide(
        found.as_deref(),
        live,
        by_hand_in(|name| std::env::var_os(name)).as_deref(),
        fallback,
    );
    if let Some(warn) = &plan.warn {
        tracing::warn!("{warn} ({})", path.display());
    }
    if let Some(info) = &plan.info {
        tracing::info!("{info} (delete {} to try them again)", path.display());
    }
    match plan.env {
        Env::Leave => {}
        // SAFETY: on the window's thread before Slint's backend is
        // selected, beside `window_fit::scale::start`, which sets its
        // variable just before: before the event loop, the worker, any
        // pool of ours, or any wgpu instance starts, so no thread of
        // ours reads the environment meanwhile. `WGPUSettings` and
        // Slint's select read it next, on this thread.
        Env::Set(name) => unsafe {
            std::env::set_var(WGPU_BACKEND, name);
            std::env::set_var(OURS, "1");
            WE_SET.store(true, Ordering::Relaxed);
        },
        // SAFETY: as above.
        Env::Clear => unsafe {
            if std::env::var_os(OURS).is_some() {
                std::env::remove_var(WGPU_BACKEND);
                std::env::remove_var(OURS);
            }
        },
    }
    apply(&path, &plan.now);
    *PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Pending {
        path,
        made: plan.made,
        found,
        owned: !live,
        lock: lock.flatten(),
        selected: false,
    });
    Guard
}

/// Once the window's device is made: nothing is left for the end to
/// leave. What the select left is not written again ([`selected`] did,
/// and does nothing a second time).
pub(crate) fn device_made() {
    selected();
    take_pending();
}

/// Once Slint's select has come back with a backend: the setup did not
/// die, and the lock is let go. Once; again is nothing.
pub(crate) fn selected() {
    let mut pending = PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(p) = pending.as_mut().filter(|p| !p.selected) {
        if p.owned {
            apply(&p.path, &p.made);
        }
        p.selected = true;
        p.lock = None;
    }
}

/// After Slint's select failed: where it was on a backend we set, our
/// variable taken off and the marker rewritten for the default
/// backends, so the caller selects again on those, in this process (a
/// failed select sets no platform), before anything looks for a
/// software adapter. False, and nothing done, otherwise.
pub(crate) fn fallback_failed(why: &dyn std::fmt::Display) -> bool {
    if !WE_SET.swap(false, Ordering::Relaxed) {
        return false;
    }
    tracing::warn!(
        "Slint's select on {} failed ({why}); selecting again on the default backends",
        std::env::var(WGPU_BACKEND).unwrap_or_default()
    );
    // SAFETY: as in `start`: on the window's thread, before the event
    // loop, the worker, or any pool of ours starts; Slint's failed
    // select's instance is dropped, and the next select reads it here.
    unsafe {
        std::env::remove_var(WGPU_BACKEND);
        std::env::remove_var(OURS);
    }
    let mut pending = PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(p) = pending.as_mut().filter(|p| p.owned) {
        let died = p
            .found
            .as_deref()
            .map(Marker::parse)
            .unwrap_or_default()
            .died;
        let now = after_fallback_error(&died);
        apply(&p.path, &File::Write(now.clone()));
        // A start that ends with an error from here leaves the same,
        // less the setup it did not die in.
        p.found = Some(Marker { setup: None, ..now }.text());
        p.made = File::Remove;
    }
    true
}

/// A command we start, without the variable where we set it.
pub(crate) fn scrub(command: &mut std::process::Command) -> &mut std::process::Command {
    if WE_SET.load(Ordering::Relaxed) {
        command.env_remove(WGPU_BACKEND).env_remove(OURS);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS: Option<Backend> = Some(Backend::Dx12);
    const LINUX: Option<Backend> = None;

    fn text(setup: Option<Backend>, died: &[Backend]) -> String {
        Marker {
            setup,
            died: died.to_vec(),
        }
        .text()
    }

    #[test]
    fn the_marker_reads_back_what_it_wrote() {
        use Backend::{Default, Dx12};
        for marker in [
            Marker::default(),
            Marker {
                setup: Some(Dx12),
                died: vec![Default],
            },
            Marker {
                setup: None,
                died: vec![Default, Dx12],
            },
        ] {
            assert_eq!(Marker::parse(&marker.text()), marker);
        }
        // What it does not know, it leaves out.
        assert_eq!(
            Marker::parse("setup: metal\ndied: gl default\nwhatever"),
            Marker {
                setup: None,
                died: vec![Default],
            }
        );
    }

    #[test]
    fn a_clean_start_sets_up_on_the_default_and_leaves_nothing() {
        for fallback in [WINDOWS, LINUX] {
            let plan = decide(None, false, None, fallback);
            assert_eq!(plan.env, Env::Clear);
            assert_eq!(
                plan.now,
                File::Write(Marker::parse(&text(Some(Backend::Default), &[])))
            );
            assert_eq!(plan.made, File::Remove);
            assert_eq!((plan.warn, plan.info), (None, None));
        }
    }

    #[test]
    fn a_death_on_the_default_goes_to_dx12_on_windows_and_stays() {
        let found = text(Some(Backend::Default), &[]);
        let plan = decide(Some(&found), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Set("dx12"));
        let after = Marker {
            setup: None,
            died: vec![Backend::Default],
        };
        assert_eq!(
            plan.now,
            File::Write(Marker {
                setup: Some(Backend::Dx12),
                ..after.clone()
            })
        );
        assert_eq!(plan.made, File::Write(after.clone()));
        assert!(plan.warn.unwrap().contains("died during GPU setup"));
        // The next start goes to DX12 straight, and says why at info.
        let plan = decide(Some(&after.text()), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Set("dx12"));
        assert_eq!(plan.warn, None);
        assert!(plan.info.is_some());
        assert_eq!(plan.made, File::Write(after));
    }

    #[test]
    fn a_death_on_both_goes_on_as_before_and_does_not_turn() {
        let found = text(Some(Backend::Dx12), &[Backend::Default]);
        let plan = decide(Some(&found), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Clear);
        let both = vec![Backend::Default, Backend::Dx12];
        assert_eq!(
            plan.now,
            File::Write(Marker {
                setup: Some(Backend::Default),
                died: both.clone(),
            })
        );
        assert!(plan.warn.as_deref().unwrap().contains("default and dx12"));
        // Dying again changes nothing: still the default, still warned.
        let File::Write(now) = plan.now else {
            unreachable!()
        };
        let again = decide(Some(&now.text()), false, None, WINDOWS);
        assert_eq!(again.env, Env::Clear);
        assert_eq!(again.now, File::Write(now));
        assert!(again.warn.is_some());
        // The default working at last forgets the lot.
        assert_eq!(again.made, File::Remove);
    }

    #[test]
    fn off_windows_a_death_is_only_said() {
        let found = text(Some(Backend::Default), &[]);
        let plan = decide(Some(&found), false, None, LINUX);
        assert_eq!(plan.env, Env::Clear);
        assert_eq!(plan.made, File::Remove);
        assert!(plan.warn.as_deref().unwrap().contains("goes on as before"));
    }

    #[test]
    fn a_backend_set_by_hand_is_left_alone_and_the_marker_too() {
        let found = text(Some(Backend::Default), &[]);
        for fallback in [WINDOWS, LINUX] {
            let plan = decide(Some(&found), false, Some("vulkan"), fallback);
            assert_eq!(
                (plan.env, plan.now, plan.made),
                (Env::Leave, File::Leave, File::Leave)
            );
            assert!(plan.warn.unwrap().contains("set by hand"));
            let plan = decide(None, false, Some("vulkan"), fallback);
            assert_eq!((plan.warn, plan.now), (None, File::Leave));
        }
    }

    #[test]
    fn the_variable_a_greycard_set_is_not_set_by_hand() {
        let env = |set: &'static [&'static str]| {
            move |name: &str| set.contains(&name).then(|| "dx12".into())
        };
        assert_eq!(by_hand_in(env(&[])), None);
        assert_eq!(by_hand_in(env(&[WGPU_BACKEND])).as_deref(), Some("dx12"));
        assert_eq!(by_hand_in(env(&[WGPU_BACKEND, OURS])), None);
    }

    #[test]
    fn the_variable_we_set_is_taken_off_the_commands_we_start() {
        let removed = |c: &std::process::Command, name: &str| {
            c.get_envs().any(|(k, v)| k == name && v.is_none())
        };
        let mut before = std::process::Command::new("true");
        assert!(!removed(scrub(&mut before), WGPU_BACKEND));
        WE_SET.store(true, Ordering::Relaxed);
        let mut after = std::process::Command::new("true");
        scrub(&mut after);
        assert!(removed(&after, WGPU_BACKEND) && removed(&after, OURS));
        WE_SET.store(false, Ordering::Relaxed);
    }

    #[test]
    fn a_live_start_is_no_death_and_its_marker_is_not_touched() {
        // A second start meanwhile (Explorer's Open with on several
        // files starts one each) goes as the first, and writes nothing.
        let found = text(Some(Backend::Default), &[]);
        for fallback in [WINDOWS, LINUX] {
            let plan = decide(Some(&found), true, None, fallback);
            assert_eq!(plan.env, Env::Clear);
            assert_eq!((plan.now, plan.made), (File::Leave, File::Leave));
            assert_eq!((plan.warn, plan.info), (None, None));
        }
        // What died before still counts, said by the live start alone.
        let found = text(Some(Backend::Default), &[Backend::Default]);
        let plan = decide(Some(&found), true, None, LINUX);
        assert_eq!((plan.env, plan.warn), (Env::Clear, None));
        let found = text(Some(Backend::Dx12), &[Backend::Default]);
        let plan = decide(Some(&found), true, None, WINDOWS);
        assert_eq!(plan.env, Env::Set("dx12"));
        assert_eq!((plan.now, plan.made), (File::Leave, File::Leave));
        assert_eq!(plan.warn, None);
    }

    #[test]
    fn a_failed_select_on_the_fallback_goes_back_to_the_default() {
        use Backend::{Default, Dx12};
        let now = after_fallback_error(&[Default]);
        assert_eq!(
            now,
            Marker {
                setup: Some(Default),
                died: vec![Dx12],
            }
        );
        // Dying on the default after that is the end of trying: the
        // next start goes on as before rather than back to DX12.
        let plan = decide(Some(&now.text()), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Clear);
        assert!(plan.warn.is_some());
        // Ending with an error (no death) leaves the default first.
        let ended = Marker { setup: None, ..now };
        let plan = decide(Some(&ended.text()), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Clear);
        assert_eq!((plan.warn, plan.info), (None, None));
    }

    #[test]
    fn a_start_ending_with_no_device_leaves_the_marker_as_it_should() {
        use Backend::{Default, Dx12};
        let found = text(Some(Default), &[]);
        // Before the select came back: as found.
        assert_eq!(
            at_end(false, End::Returned, true, Some(&found), &File::Remove),
            File::Write(Marker::parse(&found))
        );
        assert_eq!(
            at_end(false, End::Returned, false, None, &File::Remove),
            File::Remove
        );
        // After a select on DX12, the device refused in the loop: DX12
        // failed, and the next start goes to the default.
        let pinned = File::Write(Marker {
            setup: None,
            died: vec![Default],
        });
        let File::Write(left) = at_end(true, End::RunEnded, true, Some(&found), &pinned) else {
            panic!("written")
        };
        assert_eq!(
            left,
            Marker {
                setup: None,
                died: vec![Dx12],
            }
        );
        let plan = decide(Some(&left.text()), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Clear);
        // After a select on the default: what the select left stands.
        assert_eq!(
            at_end(true, End::RunEnded, false, None, &File::Remove),
            File::Leave
        );
        // An error of the start's own after a select on DX12 (a preset
        // misnamed), before the event loop: the pin is kept.
        assert_eq!(
            at_end(true, End::Returned, true, Some(&found), &pinned),
            File::Leave
        );
    }

    #[test]
    fn a_held_lock_reads_as_a_live_start() {
        let dir = crate::testing::scratch_dir("gpu-marker-lock");
        let path = path_under(&dir);
        let first = take_lock(&path).expect("the first start takes it");
        assert!(first.is_some());
        // A second handle, as a second process's: the lock is held.
        assert!(take_lock(&path).is_none());
        drop(first);
        assert!(take_lock(&path).is_some_and(|l| l.is_some()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_file_is_written_and_removed_where_asked() {
        let dir = crate::testing::scratch_dir("gpu-marker");
        let path = path_under(&dir);
        let marker = Marker {
            setup: Some(Backend::Default),
            died: vec![],
        };
        apply(&path, &File::Write(marker.clone()));
        assert_eq!(
            Marker::parse(&std::fs::read_to_string(&path).unwrap()),
            marker
        );
        apply(&path, &File::Remove);
        assert!(!path.exists());
        // Removing what is not there is no error.
        apply(&path, &File::Remove);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
