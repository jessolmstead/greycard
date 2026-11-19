//! A start that dies while the window's GPU is set up, and the next
//! start's way around it.
//!
//! A driver can crash the process while the window's GPU is set up:
//! Intel's legacy Vulkan driver on Windows does, beside a DX12 that
//! draws fine, for one UHD 630 in Slint's select and for another UHD
//! after it, as the window's own instance, surface, or device is made
//! (the window already placed). Nothing in the process can catch
//! that, so before Slint's select we leave a marker in the cache
//! saying which backends this start sets up on, and rewrite it once
//! the window's renderer is set up: Slint's rendering setup, where the
//! `gpu:` line is logged, with the viewport's pipelines built and the
//! worker handed the device. On the default backends it is then taken
//! off; on DX12 it becomes the pin. A setup line found at the start
//! means the last start died there. On Windows the next start then
//! goes through DX12 alone, by `WGPU_BACKEND`, which Slint reads, so
//! the Vulkan loader never loads the driver. Elsewhere there is no
//! second backend Slint draws with (GL it avoids), so the marker only
//! says so in the log. A `WGPU_BACKEND` set by hand is left alone, the
//! marker too: such a start writes nothing to it. Every warning that
//! reports a death or a pin kept names the file and says deleting it
//! starts over, as the info line of a start sent to DX12 does.
//!
//! The pin (`died: default`, `drew: dx12`) is written only once DX12
//! has set a renderer up on this machine, so it is proof DX12 draws
//! here, and every later start goes to DX12 straight. While it stands,
//! a start on DX12 that dies or panics before its first frame (killed,
//! logged off, out of memory, a bug of ours) leaves `setup: dx12`
//! beside `drew: dx12`, which the next start reads as the pin kept,
//! with a warning, never as DX12 dead. Three things undo a pin:
//! deleting the file, which the log names; DX12 failing with evidence
//! (its select an error, or its event loop ending in one before the
//! setup), which sends the next start back to the default backends;
//! and the default drawing on a try after DX12 has died
//! [`KEPT_LIMIT`] times in a row (below). On DX12's first start,
//! before it has drawn here, a death or a panic counts against it;
//! should the default then die too, both are counted dead, and starts
//! go on with the default, with a warning, trying DX12 again only
//! after deaths in a row there (below). A file without the `drew:` line (0.11's) reads as it
//! did: its pin gains the line the first time DX12 sets up from it.
//!
//! The marker covers the select through the renderer's setup; a lock
//! beside it is held as long, so a second start meanwhile finds the
//! marker live and no death. Through the select alone (up to 0.11) it
//! missed the second driver above. The window is not short: it runs
//! through `App::new`, the wiring, and the first frame Slint draws,
//! whose rendering setup comes only once a surface texture is acquired
//! (over a second on a healthy machine), and a window started
//! minimized or under a locked session, whose frames are skipped, holds
//! it until first drawn. An event loop that ends without a frame drawn
//! is therefore no evidence about the backend, except on a try (below),
//! which is one try whatever ends it. A start killed, logged
//! off, shut down, or out of memory in the window counts as a death:
//! winit hears no session end and the process is ended without our
//! drop, and catching it would take a hook on the window's messages of
//! our own. On the default backends on Windows such a false death costs
//! a pin to DX12, which draws, until the file is deleted or DX12 keeps
//! dying; on a pinned machine one costs nothing, and [`KEPT_LIMIT`] in
//! a row cost a try of the default.
//!
//! What a start leaves, by how it ends (`found` is the marker as the
//! start read it):
//!
//! | ends                                  | on the default    | on DX12 we set     |
//! |---------------------------------------|-------------------|--------------------|
//! | renderer set up, then anything        | nothing           | the pin            |
//! | dies before it (no code runs)         | `setup: default`  | `setup: dx12`      |
//! | panics before it                      | `setup: default`  | `setup: dx12`      |
//! | returns early, an error of its own    | `found`           | `found`            |
//! | event loop ends cleanly, no frame     | `found`           | `found`            |
//! | event loop ends in an error, no frame | `found`           | DX12 failed        |
//! | select on DX12 an error               | (n/a)             | DX12 failed, the default selected in-process |
//!
//! `setup: dx12` keeps the pin's `drew: dx12` where there was one, and
//! is then no death. "DX12 failed" is `died: dx12`, with no `drew:`:
//! the next start goes to the default, and a death there counts both
//! dead, which only deaths in a row there undo (below). A try's return
//! or clean end is no `found`: it goes back (below). A panic counts as
//! a death because wgpu's errors on the window's thread panic (`gpu_errors`): a shader the driver will not
//! compile in the setup would otherwise recur on every start without
//! DX12 tried. A panic of ours that is no GPU's counts too: on Windows
//! on the default it costs a pin to DX12, elsewhere a warning. A lost
//! device in the setup that the event loop survives
//! (`gpu_lost::survive`) is a clean end.
//!
//! What it cannot catch: a driver that dies after the setup, drawing a
//! frame (Slint's own pipelines are built on its first) or setting up
//! again after a lost device; and a driver that loses the device in
//! the setup on every start rather than killing the process, whose
//! loss the event loop survives as a clean end, so the machine never
//! reaches DX12 unless the loss surfaces as a panic before
//! `gpu_lost::install`.
//!
//! A backend that keeps dying is given up on, for one start. Where a
//! death sends the next start back to the backend it died on (a pinned
//! DX12, or the default with both counted dead), the marker counts it
//! (`kept: N`); at [`KEPT_LIMIT`] in a row the next start tries the
//! other backend once. Drawing there settles it (the default leaves
//! nothing, DX12 the pin); dying there sends the start after back,
//! with the count at nought, and so does any other end there without a
//! frame (a return, an event loop's end, clean or in an error, a lost
//! device survived among them): a try is one try, never repeated at
//! every start.
//! The default's select failing on a try keeps the pin whatever comes
//! after it in the process: a software adapter drawing proves nothing
//! of the GPU. A renderer set up clears the count.
//!
//! The file is written to a part file beside it and renamed over it,
//! so a kill inside the write never leaves half a marker; a file cut
//! short anyway (an older build's, written in place) reads as none.
//! Part files a killed start left are swept by the next that holds
//! the lock.

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
pub enum Backend {
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
pub struct Marker {
    /// The backends a start was setting up on and did not finish: it
    /// died there, unless it is this start.
    pub setup: Option<Backend>,
    /// Those a start died setting up on before.
    pub died: Vec<Backend>,
    /// The fallback once it has set a renderer up here: proof it draws,
    /// so a start on it that dies before its first frame is no death of
    /// it. A file without the line (0.11's) has none.
    pub drew: Vec<Backend>,
    /// Deaths in a row on the backend a death sends the next start
    /// back to; a file without the line (0.12's and before) has none.
    pub kept: u32,
}

/// Deaths in a row on the backend a start is sent back to, after which
/// the next start tries the other once. A pin is meant to outlast
/// false deaths (a kill, a logoff, a shutdown, out of memory in the
/// second or more before the first frame), and one is common enough;
/// three in a row are not, while a backend that truly died takes the
/// user three starts to leave. A try that dies costs one start and
/// comes back.
pub const KEPT_LIMIT: u32 = 3;

impl Marker {
    /// The file's text. A line a word: the reader takes what it knows.
    pub fn text(&self) -> String {
        let mut text = String::from("# greycard: GPU setup under way, or died in it\n");
        if let Some(setup) = self.setup {
            text.push_str(&format!("setup: {}\n", setup.name()));
        }
        if !self.died.is_empty() {
            let died: Vec<_> = self.died.iter().map(|b| b.name()).collect();
            text.push_str(&format!("died: {}\n", died.join(" ")));
        }
        if !self.drew.is_empty() {
            let drew: Vec<_> = self.drew.iter().map(|b| b.name()).collect();
            text.push_str(&format!("drew: {}\n", drew.join(" ")));
        }
        if self.kept > 0 {
            text.push_str(&format!("kept: {}\n", self.kept));
        }
        text
    }

    /// The file's text read back, or `None` where the text was cut
    /// short: our lines all end in a line end, so one that does not was
    /// cut inside a write, and is read as no marker at all.
    pub fn read(text: &str) -> Option<Self> {
        (text.is_empty() || text.ends_with('\n')).then(|| Self::parse(text))
    }

    /// The file's text read back; what it does not know it leaves out.
    pub fn parse(text: &str) -> Self {
        let mut marker = Marker::default();
        for line in text.lines() {
            if let Some(name) = line.strip_prefix("setup:") {
                marker.setup = Backend::parse(name.trim());
            } else if let Some(names) = line.strip_prefix("died:") {
                add_all(&mut marker.died, names);
            } else if let Some(names) = line.strip_prefix("drew:") {
                // Only the fallback is ever proved: anything else here
                // is not ours.
                add_all(&mut marker.drew, names);
                marker.drew.retain(|b| *b == Backend::Dx12);
            } else if let Some(n) = line.strip_prefix("kept:") {
                marker.kept = n.trim().parse().unwrap_or(0);
            }
        }
        marker
    }
}

/// The backends `names` names, added to `to` once each.
fn add_all(to: &mut Vec<Backend>, names: &str) {
    for b in names.split_whitespace().filter_map(Backend::parse) {
        if !to.contains(&b) {
            to.push(b);
        }
    }
}

/// What to do to the environment before the select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Env {
    /// Nothing: the variable is the user's.
    Leave,
    /// Set it, to the name given, with our marker beside it.
    Set(&'static str),
    /// Take off one a greycard set for itself, so Slint chooses.
    Clear,
}

/// What to do to the marker file at a moment of the start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum File {
    Leave,
    Write(Marker),
    Remove,
}

/// The start's choice from what the marker said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub env: Env,
    /// Written before the select.
    pub now: File,
    /// Once the window's renderer is set up.
    pub made: File,
    /// Said at warn, and at info.
    pub warn: Option<String>,
    pub info: Option<String>,
    /// The backend this start tries once, after deaths in a row on the
    /// other.
    pub retry: Option<Backend>,
}

/// The choice, from the marker's text (`None` for no file), whether
/// another start holds its lock (`live`: that start is setting up now,
/// and its `setup` line is no death), the `WGPU_BACKEND` set by hand if
/// any, and the backend to fall back on here (DX12 on Windows, none
/// elsewhere). A live start's marker is its own: this one reads what
/// died before and writes nothing.
pub fn decide(
    found: Option<&str>,
    live: bool,
    by_hand: Option<&str>,
    fallback: Option<Backend>,
) -> Plan {
    let mut marker = found.and_then(Marker::read).unwrap_or_default();
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
            retry: None,
        };
    }
    // The last start's setup line is a death, unless on a backend that
    // has drawn here: a pin kept.
    let last_died = marker.setup.filter(|b| !marker.drew.contains(b));
    let mut died = marker.died.clone();
    if let Some(b) = last_died
        && !died.contains(&b)
    {
        died.push(b);
    }
    let pick = std::iter::once(Backend::Default)
        .chain(fallback)
        .find(|b| !died.contains(b));
    let stays = pick.unwrap_or(Backend::Default);
    // The last start died on the backend this one is sent back to: a
    // death in a row, and at the limit the other is tried once. A try
    // that dies is no such death (it died elsewhere), so the count is
    // back at nought.
    let other = match stays {
        Backend::Default => fallback,
        Backend::Dx12 => Some(Backend::Default),
    };
    let kept = match other {
        Some(_) if marker.setup == Some(stays) => marker.kept.saturating_add(1),
        _ => 0,
    };
    let retry = other.filter(|_| kept >= KEPT_LIMIT);
    let backend = retry.unwrap_or(stays);
    let tried: Vec<_> = died.iter().map(|b| b.name()).collect();
    let tried = tried.join(" and ");
    let (warn, info) = match (last_died, pick) {
        _ if retry.is_some() => (
            Some(format!(
                "the last {kept} starts in a row died during GPU setup on {}; this start tries {} once more",
                stays.name(),
                backend.name()
            )),
            None,
        ),
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
        (None, Some(Backend::Dx12)) if marker.setup.is_some() => (
            Some(String::from(
                "the last start died during GPU setup on dx12, which has drawn here before, so this start sets up on it again",
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
            retry: None,
        };
    }
    Plan {
        env: env_for(backend),
        // A try's count is nought: should it die, the next start is
        // back where it was with none; should it end with no evidence,
        // the marker as found, count and all, is put back.
        now: File::Write(Marker {
            setup: Some(backend),
            died: died.clone(),
            drew: marker.drew.clone(),
            kept: if retry.is_some() { 0 } else { kept },
        }),
        // The default backends working leave nothing to remember; the
        // fallback working is kept, with its proof it draws, so the next
        // start goes there too, and a death there is not held against it.
        // Counted dead before (a try from both dead), it is no longer.
        made: match backend {
            Backend::Default => File::Remove,
            Backend::Dx12 => File::Write(Marker {
                setup: None,
                died: died.into_iter().filter(|b| *b != Backend::Dx12).collect(),
                drew: vec![Backend::Dx12],
                kept: 0,
            }),
        },
        warn,
        info,
        retry,
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
pub fn after_fallback_error(died: &[Backend]) -> Marker {
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
        drew: Vec::new(),
        kept: 0,
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

/// The marker's text written to a part file beside `path`, synced, and
/// renamed over it: a kill inside the write leaves the old marker or
/// the new, never part of one. The part file carries the process's id
/// and a count, so two starts never write the same one, and goes if the
/// write or the rename fails.
fn write_whole(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = part_path(path, PART_COUNT.fetch_add(1, Ordering::Relaxed));
    greycard_edit::write_synced(&part, text.as_bytes())?;
    // Once more after a pause: on Windows a rename over a file another
    // process has open for a moment (a second start reading it, a
    // virus scanner) fails until it is closed.
    std::fs::rename(&part, path)
        .or_else(|_| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            std::fs::rename(&part, path)
        })
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&part);
        })
}

/// The count in the part files' names.
static PART_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The part file of write `n` of this process beside the marker.
fn part_path(path: &Path, n: u64) -> PathBuf {
    path.with_extension(format!("{}-{n}.part", std::process::id()))
}

/// Part files beside the marker that a start killed inside a write
/// left, removed. Only under the lock: no other start writes then.
fn sweep_parts(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let head = format!("{}.", name.to_string_lossy());
    for entry in entries.flatten() {
        let file = entry.file_name();
        let file = file.to_string_lossy();
        if file.starts_with(&head) && file.ends_with(".part") {
            match std::fs::remove_file(entry.path()) {
                Ok(()) => tracing::debug!("GPU setup marker: swept {file}"),
                Err(e) => tracing::debug!("GPU setup marker: {file} not swept: {e}"),
            }
        }
    }
}

/// The marker's text at `path`, or `None` for no file, one that will
/// not read, or one cut short inside a write, which is said.
fn read_found(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    if Marker::read(&text).is_none() {
        tracing::warn!(
            "GPU setup marker {}: cut short, read as none",
            path.display()
        );
        return None;
    }
    Some(text)
}

fn apply(path: &Path, file: &File) {
    let done = match file {
        File::Leave => Ok(()),
        File::Write(marker) => write_whole(path, &marker.text()),
        File::Remove => match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            done => done,
        },
    };
    if let Err(e) = done {
        tracing::warn!("GPU setup marker {}: {e}", path.display());
    }
}

/// The start's marker while the window's renderer is not set up yet:
/// what to leave once it is, and the file as it was found, put back if
/// the start ends without one and without dying.
struct Pending {
    path: PathBuf,
    made: File,
    found: Option<String>,
    /// Whether the marker is this start's to write: not where another
    /// start is setting up.
    owned: bool,
    /// The setup's lock, let go with the rest once the renderer is set
    /// up or the start ends.
    _lock: Option<std::fs::File>,
    /// The backend tried once after deaths in a row on the other, where
    /// this start is such a try.
    retry: Option<Backend>,
}

/// How a start with no renderer set up ended, short of dying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// Returned on the way, an error of its own.
    Returned,
    /// Its event loop ended cleanly: a window closed before it was
    /// ever drawn (minimized, a locked session), or a lost device
    /// survived. No evidence about the backend.
    RunEnded,
    /// Its event loop ended in an error.
    RunFailed,
    /// Unwinding from a panic: a death, as a crash is.
    Panicked,
}

/// What the marker is left as when the start ends with no renderer
/// set up (the header's table): a panic leaves the setup line, as a
/// death would (which on a pinned DX12 the next start keeps to);
/// an event loop that failed on the fallback we set counts it as
/// failed, so the next start goes back to the default; a `retry` is
/// one try, so any other end of it goes back ([`back_from_try`]);
/// anything else is no evidence, and leaves the marker as it was found.
fn at_end(end: End, we_set: bool, found: Option<&str>, made: &File, retry: bool) -> File {
    match (end, we_set) {
        (End::Panicked, _) => File::Leave,
        _ if retry => back_from_try(found),
        (End::RunFailed, true) => {
            let died = match made {
                File::Write(marker) => marker.died.clone(),
                _ => Vec::new(),
            };
            File::Write(Marker {
                setup: None,
                ..after_fallback_error(&died)
            })
        }
        _ => match found {
            Some(text) => File::Write(Marker::parse(text)),
            None => File::Remove,
        },
    }
}

/// The marker a try that failed leaves: as found, less the death the
/// try was made on and its count, so the start after goes back to the
/// backend it left (a pin, or the default with both dead) and counts
/// from nought.
fn back_from_try(found: Option<&str>) -> File {
    match found.and_then(Marker::read) {
        Some(marker) => File::Write(Marker {
            setup: None,
            kept: 0,
            ..marker
        }),
        None => File::Remove,
    }
}

impl Pending {
    /// The marker at `path` read, its lock taken, the plan made from it
    /// (with the `WGPU_BACKEND` set `by_hand`, and the `fallback` here),
    /// and its `now` written: on disk from here until the renderer is
    /// set up.
    fn begin(path: PathBuf, by_hand: Option<&str>, fallback: Option<Backend>) -> (Self, Plan) {
        let lock = take_lock(&path);
        let live = lock.is_none();
        // A start set by hand writes nothing, not even the marker put
        // back as found.
        let owned = !live && by_hand.is_none();
        // Held, not merely unlockable: no other start is writing.
        if owned && matches!(lock, Some(Some(_))) {
            sweep_parts(&path);
        }
        let found = read_found(&path);
        let plan = decide(found.as_deref(), live, by_hand, fallback);
        apply(&path, &plan.now);
        let pending = Pending {
            path,
            made: plan.made.clone(),
            found,
            owned,
            _lock: lock.flatten(),
            retry: plan.retry,
        };
        (pending, plan)
    }

    /// Slint's select came back. The marker stays: a driver can still
    /// die making the window's own instance, surface, device, and
    /// pipelines.
    fn selected(&self) {
        tracing::debug!("GPU setup: the select came back; the marker stays until the device");
    }

    /// The window's renderer set up: the marker left as the plan has
    /// it, and the lock let go.
    fn device_made(self) {
        if self.owned {
            apply(&self.path, &self.made);
        }
    }

    /// Ended with no renderer set up, the process going on (a return,
    /// the event loop's end) or unwinding (a panic): the marker left as
    /// [`at_end`] has it, and the lock let go.
    fn ended(self, how: End, we_set: bool) {
        if self.owned {
            apply(
                &self.path,
                &at_end(
                    how,
                    we_set,
                    self.found.as_deref(),
                    &self.made,
                    self.retry.is_some(),
                ),
            );
        }
    }

    /// The select on the fallback an error: the marker rewritten for
    /// the default backends, tried next in this process, with the
    /// fallback counted as failed. A try of DX12 so ended is over.
    fn select_failed(&mut self) {
        if !self.owned {
            return;
        }
        let (now, found) = after_select_failed(self.found.as_deref());
        apply(&self.path, &File::Write(now));
        self.found = Some(found);
        self.made = File::Remove;
        self.retry = None;
    }

    /// The select on the default backends an error. On a try of them
    /// from a pin, the try is over and the pin kept, whatever follows in
    /// this process: a software adapter drawing next, or a return. The
    /// marker on disk keeps the try's setup line, so a death after it
    /// reads as the try's, which goes back to the pin too.
    fn default_select_failed(&mut self) {
        if !self.owned || self.retry != Some(Backend::Default) {
            return;
        }
        let back = back_from_try(self.found.as_deref());
        tracing::warn!(
            "Slint's select on the default backends failed on their try; the dx12 pin stays ({})",
            self.path.display()
        );
        self.found = match &back {
            File::Write(marker) => Some(marker.text()),
            _ => None,
        };
        self.made = back;
        self.retry = None;
    }
}

/// After Slint's select on the default backends failed, before anything
/// looks for a software adapter: where this start was a try of them
/// from a DX12 pin, the pin is kept whatever comes next.
pub fn default_select_failed() {
    if let Some(p) = PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_mut()
    {
        p.default_select_failed();
    }
}

/// The select on the fallback an error, from the marker as `found`:
/// the marker to write now, and what the start counts as found from
/// here, which a start that ends with an error leaves (the same, less
/// the setup it did not die in). A device made after it leaves nothing.
fn after_select_failed(found: Option<&str>) -> (Marker, String) {
    let died = found.map(Marker::parse).unwrap_or_default().died;
    let now = after_fallback_error(&died);
    let found = Marker {
        setup: None,
        ..now.clone()
    }
    .text();
    (now, found)
}

fn end(how: End) {
    if let Some(p) = take_pending() {
        p.ended(how, WE_SET.load(Ordering::Relaxed));
    }
}

/// Right after the window's event loop ends, `failed` where it ended
/// in an error: one that failed with no renderer set up, on the
/// fallback, counts it as failed; one that ended cleanly is no
/// evidence.
pub fn run_ended(failed: bool) {
    end(if failed {
        End::RunFailed
    } else {
        End::RunEnded
    });
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

fn take_pending() -> Option<Pending> {
    PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// Held by the window's start: dropped with no renderer set up, it
/// leaves the marker as [`at_end`] has it, a panic unwinding through
/// it as a death.
pub struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        end(if std::thread::panicking() {
            End::Panicked
        } else {
            End::Returned
        });
    }
}

/// Before Slint's select, and before `WGPUSettings` reads the
/// environment: the marker read, said, and written, and
/// `WGPU_BACKEND` set to match. On the window's thread, once.
pub fn start() -> Guard {
    start_at(
        path(),
        cfg!(target_family = "windows").then_some(Backend::Dx12),
    )
}

/// [`start`] with the marker at `path`, falling back on `fallback`.
fn start_at(path: Option<PathBuf>, fallback: Option<Backend>) -> Guard {
    let Some(path) = path else {
        return Guard;
    };
    let by_hand = by_hand_in(|name| std::env::var_os(name));
    let (pending, plan) = Pending::begin(path, by_hand.as_deref(), fallback);
    let path = &pending.path;
    if let Some(warn) = &plan.warn {
        // Every warning reports a death or a pin kept: the starts where
        // the way back matters.
        tracing::warn!("{warn} (delete {} to start over)", path.display());
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
    *PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(pending);
    Guard
}

/// Once Slint's select has come back with a backend: the marker stays.
pub fn selected() {
    if let Some(p) = PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
    {
        p.selected();
    }
}

/// Once the window's renderer is set up, on its first frame drawn: the
/// setup did not die, the marker is left as the plan has it (on DX12,
/// the pin, with its proof that DX12 draws), and the lock let go.
/// Once; again (a renderer set up again after a lost device) is
/// nothing.
pub fn device_made() {
    if let Some(p) = take_pending() {
        p.device_made();
    }
}

/// After Slint's select failed: where it was on a backend we set, our
/// variable taken off and the marker rewritten for the default
/// backends, so the caller selects again on those, in this process (a
/// failed select sets no platform), before anything looks for a
/// software adapter. False, and nothing done, otherwise.
pub fn fallback_failed(why: &dyn std::fmt::Display) -> bool {
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
    if let Some(p) = PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_mut()
    {
        p.select_failed();
    }
    true
}

/// A command we start, without the variable where we set it.
pub fn scrub(command: &mut std::process::Command) -> &mut std::process::Command {
    if WE_SET.load(Ordering::Relaxed) {
        command.env_remove(WGPU_BACKEND).env_remove(OURS);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use Backend::{Default, Dx12};

    /// A scratch folder, removed when the test ends, a failing one too.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            greycard_engine::testing::remove_dir_retry(&self.0);
        }
    }

    impl std::ops::Deref for Scratch {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    fn scratch(what: &str) -> Scratch {
        Scratch(greycard_engine::testing::scratch_dir(what))
    }

    const WINDOWS: Option<Backend> = Some(Dx12);
    const LINUX: Option<Backend> = None;

    fn m(setup: Option<Backend>, died: &[Backend], drew: &[Backend]) -> Marker {
        Marker {
            setup,
            died: died.to_vec(),
            drew: drew.to_vec(),
            kept: 0,
        }
    }

    /// [`m`] with `kept` deaths in a row.
    fn mk(setup: Option<Backend>, died: &[Backend], drew: &[Backend], kept: u32) -> Marker {
        Marker {
            kept,
            ..m(setup, died, drew)
        }
    }

    fn text(setup: Option<Backend>, died: &[Backend]) -> String {
        m(setup, died, &[]).text()
    }

    /// A pin: a death on the default, and DX12 drawn since.
    fn pin() -> Marker {
        m(None, &[Default], &[Dx12])
    }

    /// The marker in `dir`, if any.
    fn on_disk(dir: &Path) -> Option<Marker> {
        std::fs::read_to_string(path_under(dir))
            .ok()
            .map(|t| Marker::parse(&t))
    }

    #[test]
    fn the_marker_reads_back_what_it_wrote() {
        for marker in [
            Marker::default(),
            m(Some(Dx12), &[Default], &[]),
            m(None, &[Default, Dx12], &[]),
            m(Some(Dx12), &[Default], &[Dx12]),
        ] {
            assert_eq!(Marker::parse(&marker.text()), marker);
        }
        // What it does not know, it leaves out.
        assert_eq!(
            Marker::parse("setup: metal\ndied: gl default\nwhatever"),
            m(None, &[Default], &[])
        );
    }

    #[test]
    fn a_clean_start_sets_up_on_the_default_and_leaves_nothing() {
        for fallback in [WINDOWS, LINUX] {
            let plan = decide(None, false, None, fallback);
            assert_eq!(plan.env, Env::Clear);
            assert_eq!(plan.now, File::Write(m(Some(Default), &[], &[])));
            assert_eq!(plan.made, File::Remove);
            assert_eq!((plan.warn, plan.info), (None, None));
        }
    }

    #[test]
    fn a_death_on_the_default_goes_to_dx12_on_windows_and_stays() {
        let found = text(Some(Default), &[]);
        let plan = decide(Some(&found), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Set("dx12"));
        // While DX12 has not drawn here, a death on it is a death.
        assert_eq!(plan.now, File::Write(m(Some(Dx12), &[Default], &[])));
        assert_eq!(plan.made, File::Write(pin()));
        assert!(plan.warn.unwrap().contains("died during GPU setup"));
        // The next start goes to DX12 straight, and says why at info.
        let plan = decide(Some(&pin().text()), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Set("dx12"));
        assert_eq!(plan.warn, None);
        assert!(plan.info.is_some());
        assert_eq!(plan.now, File::Write(m(Some(Dx12), &[Default], &[Dx12])));
        assert_eq!(plan.made, File::Write(pin()));
    }

    #[test]
    fn a_death_on_a_pinned_dx12_keeps_the_pin() {
        // DX12 has drawn here: a start on it that died before its first
        // frame (killed, logged off, a panic) is not held against it.
        let found = m(Some(Dx12), &[Default], &[Dx12]).text();
        let plan = decide(Some(&found), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Set("dx12"));
        assert!(plan.warn.unwrap().contains("has drawn here before"));
        assert_eq!(plan.made, File::Write(pin()));
    }

    #[test]
    fn a_marker_without_the_drew_line_reads_as_before() {
        // 0.11 wrote no `drew:` line: its pin is `died: default` alone,
        // and a death on DX12 from it is DX12's, as it was then.
        let pinned = "# greycard: GPU setup under way, or died in it\ndied: default\n";
        let plan = decide(Some(pinned), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Set("dx12"));
        assert_eq!(plan.now, File::Write(m(Some(Dx12), &[Default], &[])));
        let died = "# greycard: GPU setup under way, or died in it\nsetup: dx12\ndied: default\n";
        let plan = decide(Some(died), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Clear);
        assert!(plan.warn.unwrap().contains("default and dx12"));
    }

    #[test]
    fn a_death_on_both_goes_on_as_before_and_does_not_turn() {
        let found = text(Some(Dx12), &[Default]);
        let plan = decide(Some(&found), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Clear);
        assert_eq!(
            plan.now,
            File::Write(m(Some(Default), &[Default, Dx12], &[]))
        );
        assert!(plan.warn.as_deref().unwrap().contains("default and dx12"));
        // Dying again: still the default, still warned, one counted
        // (the try of DX12 at the limit is the count's test).
        let File::Write(now) = plan.now else {
            unreachable!()
        };
        let again = decide(Some(&now.text()), false, None, WINDOWS);
        assert_eq!(again.env, Env::Clear);
        assert_eq!(again.now, File::Write(Marker { kept: 1, ..now }));
        assert!(again.warn.is_some());
        // The default working at last forgets the lot.
        assert_eq!(again.made, File::Remove);
    }

    #[test]
    fn off_windows_a_death_is_only_said() {
        let found = text(Some(Default), &[]);
        let plan = decide(Some(&found), false, None, LINUX);
        assert_eq!(plan.env, Env::Clear);
        assert_eq!(plan.made, File::Remove);
        assert!(plan.warn.as_deref().unwrap().contains("goes on as before"));
    }

    #[test]
    fn a_backend_set_by_hand_is_left_alone_and_the_marker_too() {
        let found = text(Some(Default), &[]);
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

    /// The tests that use the process's globals (the environment, the
    /// pending start, `WE_SET`), one at a time.
    static GLOBALS: Mutex<()> = Mutex::new(());

    fn globals() -> std::sync::MutexGuard<'static, ()> {
        GLOBALS.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The globals as a process starts with them.
    fn reset() {
        // SAFETY: under `GLOBALS`; no other test of this binary reads
        // these two variables.
        unsafe {
            std::env::remove_var(WGPU_BACKEND);
            std::env::remove_var(OURS);
        }
        WE_SET.store(false, Ordering::Relaxed);
        take_pending();
    }

    #[test]
    fn the_variable_we_set_is_taken_off_the_commands_we_start() {
        let _globals = globals();
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
    fn the_guard_and_the_loop_end_leave_what_the_table_says() {
        let _globals = globals();
        // A run with the variable set (the GPU tests' recipe) gets it
        // back at the end, a failing test's too.
        struct Theirs(Option<std::ffi::OsString>);
        impl Drop for Theirs {
            fn drop(&mut self) {
                reset();
                if let Some(theirs) = self.0.take() {
                    // SAFETY: as in `reset`, still under `GLOBALS`.
                    unsafe { std::env::set_var(WGPU_BACKEND, theirs) };
                }
            }
        }
        let _theirs = Theirs(std::env::var_os(WGPU_BACKEND));
        let dir = scratch("gpu-marker-glue");
        let path = path_under(&dir);
        // DX12's first start, after a death on the default: each ending
        // leaves something of its own.
        let first = m(Some(Default), &[], &[]);
        let case = |end: &dyn Fn(Guard)| {
            reset();
            apply(&path, &File::Write(first.clone()));
            let guard = start_at(Some(path.clone()), WINDOWS);
            assert!(WE_SET.load(Ordering::Relaxed), "on DX12 we set");
            end(guard);
            let left = on_disk(&dir);
            reset();
            left
        };
        // Returned early: as found.
        assert_eq!(case(&|guard| drop(guard)), Some(first.clone()));
        // A panic unwinding through the guard: a death on DX12.
        let panicked = case(&|guard| {
            let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let _guard = guard;
                panic!("a wgpu error in the setup, on purpose");
            }));
            assert!(unwound.is_err());
        });
        assert_eq!(panicked, Some(m(Some(Dx12), &[Default], &[])));
        // The event loop ended cleanly with no frame: as found.
        let clean = case(&|guard| {
            run_ended(false);
            drop(guard);
        });
        assert_eq!(clean, Some(first.clone()));
        // The event loop failed with no frame: DX12 failed.
        let failed = case(&|guard| {
            run_ended(true);
            drop(guard);
        });
        assert_eq!(failed, Some(m(None, &[Dx12], &[])));
        // The renderer set up: the pin.
        let made = case(&|guard| {
            selected();
            device_made();
            run_ended(true);
            drop(guard);
        });
        assert_eq!(made, Some(pin()));
    }

    #[test]
    fn a_live_start_is_no_death_and_its_marker_is_not_touched() {
        // A second start meanwhile (Explorer's Open with on several
        // files starts one each) goes as the first, and writes nothing.
        let found = text(Some(Default), &[]);
        for fallback in [WINDOWS, LINUX] {
            let plan = decide(Some(&found), true, None, fallback);
            assert_eq!(plan.env, Env::Clear);
            assert_eq!((plan.now, plan.made), (File::Leave, File::Leave));
            assert_eq!((plan.warn, plan.info), (None, None));
        }
        // What died before still counts, said by the live start alone.
        let found = text(Some(Default), &[Default]);
        let plan = decide(Some(&found), true, None, LINUX);
        assert_eq!((plan.env, plan.warn), (Env::Clear, None));
        let found = text(Some(Dx12), &[Default]);
        let plan = decide(Some(&found), true, None, WINDOWS);
        assert_eq!(plan.env, Env::Set("dx12"));
        assert_eq!((plan.now, plan.made), (File::Leave, File::Leave));
        assert_eq!(plan.warn, None);
    }

    #[test]
    fn a_failed_select_on_the_fallback_goes_back_to_the_default() {
        let now = after_fallback_error(&[Default]);
        assert_eq!(now, m(Some(Default), &[Dx12], &[]));
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
    fn the_marker_stays_through_the_select_and_goes_with_the_renderer() {
        let dir = scratch("gpu-marker-life");
        let (start, plan) = Pending::begin(path_under(&dir), None, WINDOWS);
        assert_eq!(plan.env, Env::Clear);
        start.selected();
        // Between the select and the renderer, where a driver killed a
        // start with the window placed: the marker says so.
        let between = on_disk(&dir).expect("on disk after the select");
        assert_eq!(between.setup, Some(Default));
        start.device_made();
        assert_eq!(on_disk(&dir), None, "a clean start leaves nothing");
    }

    #[test]
    fn a_second_start_during_the_first_setup_is_no_death() {
        let dir = scratch("gpu-marker-second");
        let (first, _) = Pending::begin(path_under(&dir), None, WINDOWS);
        first.selected();
        // The second, between the first's select and its renderer: the
        // lock is held, so the marker is live, not a death.
        let (second, plan) = Pending::begin(path_under(&dir), None, WINDOWS);
        assert_eq!(plan.env, Env::Clear);
        assert_eq!((plan.warn, plan.info), (None, None));
        second.selected();
        second.device_made();
        assert!(on_disk(&dir).is_some(), "the first's marker, untouched");
        first.device_made();
        assert_eq!(on_disk(&dir), None);
    }

    #[test]
    fn an_event_loop_failing_on_dx12_counts_it_as_failed() {
        let found = text(Some(Default), &[]);
        let failed = at_end(
            End::RunFailed,
            true,
            Some(&found),
            &File::Write(pin()),
            false,
        );
        assert_eq!(failed, File::Write(m(None, &[Dx12], &[])));
        // On the default it is no evidence: as found.
        assert_eq!(
            at_end(End::RunFailed, false, None, &File::Remove, false),
            File::Remove
        );
    }

    /// How a start in the enumeration ends.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Ending {
        /// The renderer set up.
        Drew,
        /// The process gone before it: a crash, a kill, a logoff.
        Died,
        Panicked,
        /// An error of its own on the way.
        Returned,
        /// The event loop ended cleanly with no frame drawn.
        Clean,
        /// The event loop ended in an error with no frame drawn.
        Failed,
        /// The select on DX12 an error; on the default after it, in the
        /// process.
        SelectFailed,
    }

    const ENDINGS: [Ending; 7] = [
        Ending::Drew,
        Ending::Died,
        Ending::Panicked,
        Ending::Returned,
        Ending::Clean,
        Ending::Failed,
        Ending::SelectFailed,
    ];

    /// The marker's text once `file` is applied to `disk`.
    fn after(disk: Option<String>, file: &File) -> Option<String> {
        match file {
            File::Leave => disk,
            File::Write(marker) => Some(marker.text()),
            File::Remove => None,
        }
    }

    /// One start on the marker's text `disk`, in memory, as [`Pending`]
    /// does it on a file it owns, asked to end as `asked`; on a
    /// `killer` machine the default backends kill the process before
    /// the setup, so a start on them dies whatever was asked. Whether
    /// it was on DX12, how it ended, and whether it was a try of the
    /// other backend after deaths in a row.
    fn one_start(
        disk: &mut Option<String>,
        fallback: Option<Backend>,
        asked: Ending,
        killer: bool,
    ) -> (bool, Ending, bool) {
        // As `read_found` hands it to `Pending`: a file cut short is none.
        let found = disk.clone().filter(|t| Marker::read(t).is_some());
        let plan = decide(found.as_deref(), false, None, fallback);
        *disk = after(disk.take(), &plan.now);
        let dx12 = plan.env == Env::Set("dx12");
        let retry = plan.retry.is_some();
        let ending = if killer && !dx12 { Ending::Died } else { asked };
        let end = |how: End| at_end(how, dx12, found.as_deref(), &plan.made, retry);
        let file = match ending {
            Ending::Drew => plan.made.clone(),
            // Nothing more runs.
            Ending::Died => File::Leave,
            Ending::Panicked => end(End::Panicked),
            Ending::Returned => end(End::Returned),
            Ending::Clean => end(End::RunEnded),
            Ending::Failed => end(End::RunFailed),
            // The default selected after it, in the process: it dies
            // there on a killer, else its renderer is set up, which
            // leaves nothing.
            Ending::SelectFailed if dx12 => match killer {
                true => File::Write(after_select_failed(found.as_deref()).0),
                false => File::Remove,
            },
            // On the default, Slint's error ends the start (`startup`);
            // on a try of it, after `default_select_failed`, which leaves
            // what a return on a try does.
            Ending::SelectFailed => end(End::Returned),
        };
        *disk = after(disk.take(), &file);
        (dx12, ending, retry)
    }

    /// Whether DX12 counts as failed in `marker`.
    fn dx12_dead(marker: &Marker) -> bool {
        marker.died.contains(&Dx12) || (marker.setup == Some(Dx12) && !marker.drew.contains(&Dx12))
    }

    /// Every sequence of four endings, from every state a marker can be
    /// in (and some it should never be in), on a machine whose default
    /// backends kill the process and on a healthy one, on Windows and
    /// off it; then a fifth start, to see where it goes. On Windows a
    /// killer machine is on DX12 at the fifth unless DX12 failed with
    /// evidence (its select an error, its event loop failed), died or
    /// panicked before it had ever drawn here, or was dead at the
    /// start, the fifth going the other way where it is a try of the
    /// other backend after deaths in a row; a healthy one leaves the
    /// default only after a death or a panic on it; off Windows nothing
    /// is set. In memory: the file's side is the lifecycle tests'.
    #[test]
    fn every_sequence_of_endings_keeps_a_working_dx12() {
        const HEAD: &str = "# greycard: GPU setup under way, or died in it\n";
        let mut states: Vec<Option<String>> = [
            m(Some(Default), &[], &[]),
            m(None, &[Default], &[]),
            pin(),
            m(Some(Dx12), &[Default], &[]),
            m(Some(Dx12), &[Default], &[Dx12]),
            m(None, &[Dx12], &[]),
            m(Some(Default), &[Default, Dx12], &[]),
            m(Some(Default), &[Dx12], &[]),
        ]
        .iter()
        .map(|s| Some(s.text()))
        .collect();
        // No file, an empty one, garbage, the header alone, lines not
        // ours, a pin with Windows line ends, and what 0.11 wrote.
        states.push(None);
        for odd in [
            String::new(),
            "\u{0}garbage\n".into(),
            HEAD.into(),
            format!("{HEAD}setup: metal\nkept: 3\ndrew: default\n"),
            format!("{HEAD}setup: dx12\ndied: default\ndrew: dx12\nkept: 2\n"),
            format!("{HEAD}setup: default\ndied: default dx12\nkept: 2\n"),
            format!("{HEAD}setup: default\ndied: default dx12\nkept: lots\n"),
            format!("{HEAD}setup: default\ndied: default dx12\nkept: 9\n"),
            format!("{HEAD}setup: dx12\ndied: def"),
            format!("{HEAD}died: default\ndrew: dx12\n").replace('\n', "\r\n"),
            format!("{HEAD}setup: default\n"),
            format!("{HEAD}died: default\n"),
            format!("{HEAD}setup: dx12\ndied: default\n"),
            format!("{HEAD}died: dx12\n"),
            format!("{HEAD}setup: default\ndied: dx12\n"),
            format!("{HEAD}setup: default\ndied: default dx12\n"),
        ] {
            states.push(Some(odd));
        }
        let count = ENDINGS.len().pow(4);
        for fallback in [WINDOWS, LINUX] {
            for state in &states {
                let read = state.as_deref().and_then(Marker::read).unwrap_or_default();
                for killer in [true, false] {
                    for n in 0..count {
                        let asked =
                            (0..4).map(|i| ENDINGS[(n / ENDINGS.len().pow(i)) % ENDINGS.len()]);
                        let mut disk = state.clone();
                        let trace: Vec<(bool, Ending, bool)> = asked
                            .map(|e| one_start(&mut disk, fallback, e, killer))
                            .collect();
                        let (on_dx12, _, retry) =
                            one_start(&mut disk, fallback, Ending::Died, killer);
                        let fifth = (on_dx12, retry);
                        let on: Vec<bool> = trace
                            .iter()
                            .map(|t| t.0)
                            .chain(std::iter::once(on_dx12))
                            .collect();
                        let said = || {
                            format!(
                                "from {state:?}, killer {killer}, fallback {fallback:?}: {trace:?}, fifth on dx12, a try {fifth:?}"
                            )
                        };
                        if fallback.is_none() {
                            assert!(on.iter().all(|on| !on), "{}", said());
                            continue;
                        }
                        if killer {
                            let mut drew = read.drew.contains(&Dx12);
                            let mut failed = dx12_dead(&read);
                            for &(dx12, ending, _) in &trace {
                                if !dx12 {
                                    continue;
                                }
                                match ending {
                                    // Evidence forgets the proof too.
                                    Ending::Failed | Ending::SelectFailed => {
                                        failed = true;
                                        drew = false;
                                    }
                                    Ending::Died | Ending::Panicked if !drew => failed = true,
                                    // A try from both dead that draws
                                    // undoes the count against DX12.
                                    Ending::Drew => (drew, failed) = (true, false),
                                    _ => {}
                                }
                            }
                            // A try of the other backend, after deaths
                            // in a row, goes the other way, once.
                            assert_eq!(fifth.0, failed == fifth.1, "{}", said());
                        } else {
                            for i in 1..on.len() {
                                if on[i] && !on[i - 1] {
                                    // Or a try of the default, from a
                                    // pin, that failed.
                                    assert!(
                                        trace[i - 1].2
                                            || matches!(
                                                trace[i - 1].1,
                                                Ending::Died | Ending::Panicked
                                            ),
                                        "left the default without a death: {}",
                                        said()
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_start_set_by_hand_writes_nothing() {
        let dir = scratch("gpu-marker-by-hand");
        let path = path_under(&dir);
        // Not in our own form: a rewrite would show.
        let theirs = "setup: default\ndied:  default\n";
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, theirs).unwrap();
        for how in [End::Returned, End::RunEnded, End::RunFailed, End::Panicked] {
            let (start, plan) = Pending::begin(path.clone(), Some("vulkan"), WINDOWS);
            assert_eq!(plan.env, Env::Leave);
            start.ended(how, false);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), theirs);
        }
        let (start, _) = Pending::begin(path.clone(), Some("vulkan"), WINDOWS);
        start.device_made();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), theirs);
    }

    #[test]
    fn only_dx12_is_proved() {
        assert_eq!(Marker::parse("drew: default dx12 metal\n").drew, vec![Dx12]);
        assert!(Marker::parse("drew: default\n").drew.is_empty());
    }

    #[test]
    fn a_held_lock_reads_as_a_live_start() {
        let dir = scratch("gpu-marker-lock");
        let path = path_under(&dir);
        let first = take_lock(&path).expect("the first start takes it");
        assert!(first.is_some());
        // A second handle, as a second process's: the lock is held.
        assert!(take_lock(&path).is_none());
        drop(first);
        assert!(take_lock(&path).is_some_and(|l| l.is_some()));
    }

    /// The plan's `now` marker.
    fn now_of(plan: &Plan) -> Marker {
        match &plan.now {
            File::Write(marker) => marker.clone(),
            other => panic!("nothing written: {other:?}"),
        }
    }

    #[test]
    fn the_count_rises_on_a_pin_that_dies_and_a_renderer_clears_it() {
        let mut found = m(Some(Dx12), &[Default], &[Dx12]);
        for n in 1..KEPT_LIMIT {
            let plan = decide(Some(&found.text()), false, None, WINDOWS);
            assert_eq!(plan.env, Env::Set("dx12"), "death {n} keeps the pin");
            assert_eq!(now_of(&plan), mk(Some(Dx12), &[Default], &[Dx12], n));
            // Drawn at last: the pin, the count gone.
            assert_eq!(plan.made, File::Write(pin()));
            found = now_of(&plan);
        }
        // A start that finds no death starts the count again.
        let plan = decide(
            Some(&mk(None, &[Default], &[Dx12], 2).text()),
            false,
            None,
            WINDOWS,
        );
        assert_eq!(now_of(&plan).kept, 0);
    }

    #[test]
    fn a_pin_that_keeps_dying_tries_the_default_once() {
        let found = mk(Some(Dx12), &[Default], &[Dx12], KEPT_LIMIT - 1);
        let plan = decide(Some(&found.text()), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Clear);
        assert!(
            plan.warn
                .as_ref()
                .unwrap()
                .contains("tries default once more")
        );
        let tried = now_of(&plan);
        assert_eq!(tried, m(Some(Default), &[Default], &[Dx12]));
        // The default draws: nothing left, the pin forgotten.
        assert_eq!(plan.made, File::Remove);
        // The default dies too: back on the pin, the count at nought.
        let back = decide(Some(&tried.text()), false, None, WINDOWS);
        assert_eq!(back.env, Env::Set("dx12"));
        assert_eq!(now_of(&back), m(Some(Dx12), &[Default], &[Dx12]));
        // A try is one try: any end without a frame (a return, an event
        // loop's error, or a clean end, a lost device survived among
        // them) goes back to the pin, the count at nought, so no start
        // tries again until deaths in a row say so.
        for how in [End::Returned, End::RunEnded, End::RunFailed] {
            let left = at_end(how, false, Some(&found.text()), &plan.made, true);
            assert_eq!(left, File::Write(pin()), "{how:?}");
            let File::Write(left) = left else {
                unreachable!()
            };
            let next = decide(Some(&left.text()), false, None, WINDOWS);
            assert_eq!((next.env, next.retry), (Env::Set("dx12"), None));
        }
    }

    #[test]
    fn a_try_of_dx12_that_errors_goes_back_to_the_default() {
        let found = mk(Some(Default), &[Default, Dx12], &[], KEPT_LIMIT - 1);
        let plan = decide(Some(&found.text()), false, None, WINDOWS);
        assert_eq!(plan.retry, Some(Dx12));
        for how in [End::Returned, End::RunEnded, End::RunFailed] {
            let left = at_end(how, true, Some(&found.text()), &plan.made, true);
            assert_eq!(left, File::Write(m(None, &[Default, Dx12], &[])), "{how:?}");
        }
    }

    #[test]
    fn a_failed_default_select_on_a_try_keeps_the_pin() {
        let dir = scratch("gpu-marker-try-select");
        let path = path_under(&dir);
        let found = mk(Some(Dx12), &[Default], &[Dx12], KEPT_LIMIT - 1);
        apply(&path, &File::Write(found.clone()));
        // The default's select fails, a software adapter draws next.
        let (mut start, plan) = Pending::begin(path.clone(), None, WINDOWS);
        assert_eq!(plan.retry, Some(Default));
        start.default_select_failed();
        // A death from here reads as the try's: back on the pin.
        assert_eq!(on_disk(&dir), Some(m(Some(Default), &[Default], &[Dx12])));
        start.device_made();
        assert_eq!(on_disk(&dir), Some(pin()), "software drew: the pin kept");
        // Or the start returns the select's error: the pin, too.
        apply(&path, &File::Write(found.clone()));
        let (mut start, _) = Pending::begin(path.clone(), None, WINDOWS);
        start.default_select_failed();
        start.ended(End::Returned, false);
        assert_eq!(on_disk(&dir), Some(pin()));
        // Off a try, a failed default select is as before: software
        // drawing leaves nothing.
        apply(&path, &File::Remove);
        let (mut start, plan) = Pending::begin(path.clone(), None, WINDOWS);
        assert_eq!(plan.retry, None);
        start.default_select_failed();
        start.device_made();
        assert_eq!(on_disk(&dir), None);
    }

    #[test]
    fn a_start_holding_the_lock_sweeps_left_part_files() {
        let dir = scratch("gpu-marker-sweep");
        let path = path_under(&dir);
        let folder = path.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&folder).unwrap();
        let left = folder.join("gpu-setup.4242-7.part");
        let theirs = folder.join("other.part");
        std::fs::write(&left, "setup: def").unwrap();
        std::fs::write(&theirs, "x").unwrap();
        // A start that finds the lock held sweeps nothing: the holder
        // may be writing.
        let holder = take_lock(&path).unwrap();
        let (live, _) = Pending::begin(path.clone(), None, WINDOWS);
        assert!(left.exists());
        live.device_made();
        drop(holder);
        // The lock's holder sweeps ours, and only ours.
        let (start, _) = Pending::begin(path.clone(), None, WINDOWS);
        assert!(!left.exists());
        assert!(theirs.exists());
        start.device_made();
    }

    #[test]
    fn both_dead_tries_dx12_once_after_deaths_in_a_row() {
        let mut found = m(Some(Default), &[Default, Dx12], &[]);
        for n in 1..KEPT_LIMIT {
            let plan = decide(Some(&found.text()), false, None, WINDOWS);
            assert_eq!(plan.env, Env::Clear);
            assert_eq!(now_of(&plan).kept, n);
            found = now_of(&plan);
        }
        let plan = decide(Some(&found.text()), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Set("dx12"));
        let tried = now_of(&plan);
        assert_eq!(tried, m(Some(Dx12), &[Default, Dx12], &[]));
        // DX12 draws: a pin, DX12 no longer counted dead.
        assert_eq!(plan.made, File::Write(pin()));
        // DX12 dies: the default again, counting from nought.
        let back = decide(Some(&tried.text()), false, None, WINDOWS);
        assert_eq!(back.env, Env::Clear);
        assert_eq!(now_of(&back), m(Some(Default), &[Default, Dx12], &[]));
        // Off Windows there is no other to try: no count, no try.
        let found = mk(Some(Default), &[Default], &[], KEPT_LIMIT + 5);
        let plan = decide(Some(&found.text()), false, None, LINUX);
        assert_eq!((plan.env, now_of(&plan).kept), (Env::Clear, 0));
    }

    #[test]
    fn a_dying_dx12_on_a_pinned_machine_is_left_within_the_limit() {
        // A healthy machine pinned by a false death, then DX12 dying at
        // every start: the default is tried after `KEPT_LIMIT` deaths,
        // draws, and the marker is gone.
        let mut disk = Some(pin().text());
        let mut starts = Vec::new();
        for _ in 0..=KEPT_LIMIT {
            let plan = decide(disk.as_deref(), false, None, WINDOWS);
            disk = after(disk, &plan.now);
            let dx12 = plan.env == Env::Set("dx12");
            if !dx12 {
                disk = after(disk, &plan.made);
            }
            starts.push(dx12);
        }
        assert_eq!(starts.iter().filter(|d| **d).count(), KEPT_LIMIT as usize);
        assert_eq!(starts.last(), Some(&false));
        assert_eq!(disk, None);
    }

    #[test]
    fn a_marker_without_the_kept_line_reads_with_none() {
        // 0.12's pin that died: a count from nought.
        let head = "# greycard: GPU setup under way, or died in it\n";
        let old = format!("{head}setup: dx12\ndied: default\ndrew: dx12\n");
        assert_eq!(Marker::read(&old), Some(m(Some(Dx12), &[Default], &[Dx12])));
        let plan = decide(Some(&old), false, None, WINDOWS);
        assert_eq!(plan.env, Env::Set("dx12"));
        assert_eq!(now_of(&plan).kept, 1);
        // A count not a number is none.
        assert_eq!(Marker::parse("kept: x\n").kept, 0);
        assert_eq!(Marker::parse(&mk(None, &[], &[], 7).text()).kept, 7);
    }

    #[test]
    fn a_torn_marker_reads_as_none() {
        let full = mk(Some(Dx12), &[Default], &[Dx12], 2).text();
        // Every prefix cut inside a line is no marker: the start goes
        // on the default as a clean one, and says nothing of a death.
        for cut in (1..full.len()).filter(|i| !full[..*i].ends_with('\n')) {
            let torn = &full[..cut];
            assert_eq!(Marker::read(torn), None, "{torn:?}");
            let plan = decide(Some(torn), false, None, WINDOWS);
            assert_eq!(plan.env, Env::Clear, "{torn:?}");
            assert_eq!(now_of(&plan), m(Some(Default), &[], &[]));
            assert_eq!((plan.warn, plan.info), (None, None), "{torn:?}");
        }
        // On disk: read as none, and written over with the start's own.
        let dir = scratch("gpu-marker-torn");
        let path = path_under(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &full[..full.len() - 3]).unwrap();
        let (start, plan) = Pending::begin(path.clone(), None, WINDOWS);
        assert_eq!(plan.env, Env::Clear);
        assert_eq!(on_disk(&dir), Some(m(Some(Default), &[], &[])));
        // Ending with no evidence puts back what was found: nothing.
        start.ended(End::Returned, false);
        assert_eq!(on_disk(&dir), None);
        // Not text at all: none, too.
        std::fs::write(&path, [0xff, 0xfe, b'\n']).unwrap();
        let (start, plan) = Pending::begin(path.clone(), None, WINDOWS);
        assert_eq!((plan.env, plan.warn), (Env::Clear, None));
        start.device_made();
        assert_eq!(on_disk(&dir), None);
    }

    /// The names in the marker's folder, the lock's aside.
    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !n.ends_with(".lock"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_write_that_fails_leaves_the_old_marker_whole() {
        // The part files' next names taken by folders, so writing the
        // part fails: a marker written through one is untouched, where
        // one written in place would be the new text.
        let dir = scratch("gpu-marker-part-fails");
        let path = path_under(&dir);
        apply(&path, &File::Write(pin()));
        let before = std::fs::read(&path).unwrap();
        // Room for writes of other tests meanwhile.
        let next = PART_COUNT.load(Ordering::Relaxed);
        for n in next..next + 256 {
            std::fs::create_dir_all(part_path(&path, n)).unwrap();
        }
        apply(&path, &File::Write(mk(Some(Dx12), &[Default], &[Dx12], 2)));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn the_marker_is_renamed_into_place_whole() {
        let dir = scratch("gpu-marker-rename");
        let path = path_under(&dir);
        let folder = path.parent().unwrap().to_path_buf();
        let long = mk(Some(Dx12), &[Default], &[Dx12], 2);
        apply(&path, &File::Write(long.clone()));
        apply(&path, &File::Write(m(Some(Default), &[], &[])));
        // The shorter text over the longer: no tail of the old one, and
        // no part file left beside it.
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            m(Some(Default), &[], &[]).text()
        );
        assert_eq!(names_in(&folder), vec!["gpu-setup".to_string()]);
        // A rename that fails (a folder in the marker's place) leaves
        // that folder as it was and no part file.
        apply(&path, &File::Remove);
        std::fs::create_dir_all(path.join("inside")).unwrap();
        apply(&path, &File::Write(long));
        assert!(path.join("inside").is_dir());
        assert_eq!(names_in(&folder), vec!["gpu-setup".to_string()]);
        // And a start meeting it reads no marker and goes on.
        let (start, plan) = Pending::begin(path.clone(), None, WINDOWS);
        assert_eq!((plan.env, plan.warn), (Env::Clear, None));
        start.device_made();
    }

    #[test]
    fn the_file_is_written_and_removed_where_asked() {
        let dir = scratch("gpu-marker");
        let path = path_under(&dir);
        let marker = m(Some(Default), &[], &[]);
        apply(&path, &File::Write(marker.clone()));
        assert_eq!(
            Marker::parse(&std::fs::read_to_string(&path).unwrap()),
            marker
        );
        apply(&path, &File::Remove);
        assert!(!path.exists());
        // Removing what is not there is no error.
        apply(&path, &File::Remove);
    }
}
