//! The desktop's file chooser, for a file or files to open, a folder,
//! or where an export goes. On Linux it is the file chooser portal,
//! over D-Bus, and zenity and then kdialog when the portal cannot be
//! reached at all, as under a window manager started bare; everywhere
//! else it is the platform's own dialog, through `rfd`. A dialog takes
//! as long as the user, so it is never waited on where the window
//! draws: the portal and the helpers block a thread of their own, and
//! `rfd`'s dialog, which AppKit and the Windows common dialogs want on
//! the window's thread, is awaited on Slint's event loop, which goes on
//! drawing while it is up.

#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::Result;
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "linux"))]
use std::sync::{Arc, Mutex};

/// What a file chooser is being asked for. The desktop portal and the
/// native dialogs answer the same request in their own ways, so the
/// calls below build one of these and hand it to `ask`.
struct Ask {
    /// The dialog's title.
    title: &'static str,
    /// The folder it opens on.
    folder: PathBuf,
    /// The name a save dialog offers. None asks for a file to open:
    /// there is nothing to name until the user picks one.
    name: Option<String>,
    /// A name and its globs; empty offers no filter at all.
    filter: (String, Vec<String>),
    /// A folder chooser rather than a file one.
    directory: bool,
    /// Files to open, as many as the user picks, rather than one.
    multiple: bool,
}

/// Ask the desktop for a file to open: `start` is the folder to open
/// on, `filter` a name and its globs. `done` gets the path, none when
/// the user canceled, or the error when there was no chooser to ask.
pub fn choose_open(
    title: &'static str,
    start: PathBuf,
    filter: (String, Vec<String>),
    done: impl FnOnce(Result<Option<PathBuf>>) + Send + 'static,
) {
    ask(
        Ask {
            title,
            folder: start,
            name: None,
            filter,
            directory: false,
            multiple: false,
        },
        move |picked| done(picked.map(|p| p.into_iter().next())),
    );
}

/// Ask for files to open, the same way, as many as the user picks:
/// `done` gets them, none when the user canceled.
pub fn choose_open_many(
    title: &'static str,
    start: PathBuf,
    filter: (String, Vec<String>),
    done: impl FnOnce(Result<Vec<PathBuf>>) + Send + 'static,
) {
    ask(
        Ask {
            title,
            folder: start,
            name: None,
            filter,
            directory: false,
            multiple: true,
        },
        done,
    );
}

/// Ask the desktop for a folder to open, the same way. A folder
/// chooser has no filter and replies with one directory.
pub fn choose_folder(
    title: &'static str,
    start: PathBuf,
    done: impl FnOnce(Result<Option<PathBuf>>) + Send + 'static,
) {
    ask(
        Ask {
            title,
            folder: start,
            name: None,
            filter: (String::new(), Vec::new()),
            directory: true,
            multiple: false,
        },
        move |picked| done(picked.map(|p| p.into_iter().next())),
    );
}

/// Linux asks the desktop's file chooser portal, on a thread of its
/// own since the dialog takes as long as the user and the call is a
/// blocking one. `done` gets what was picked, nothing when the user
/// canceled.
#[cfg(target_os = "linux")]
fn ask(request: Ask, done: impl FnOnce(Result<Vec<PathBuf>>) + Send + 'static) {
    // No file chooser from a test: it answers as a cancel.
    if cfg!(test) {
        return done(Ok(Vec::new()));
    }
    // The folder it opens on is looked at here, on the thread that
    // may wait on a mount, not on the window's.
    std::thread::spawn(move || {
        let mut request = request;
        request.folder = nearest_folder(&request.folder);
        done(choose(&request))
    });
}

/// The portal, or zenity and then kdialog when the portal could not
/// be asked at all: a window manager started bare, as i3 often is,
/// has no session bus or no portal on it, and either program draws
/// a chooser of its own. A portal that was asked and failed is not
/// asked again elsewhere, since the user may have seen its dialog.
/// With neither program there, the portal's error stands, and
/// `chooser_failed` says what to install.
#[cfg(target_os = "linux")]
fn choose(request: &Ask) -> Result<Vec<PathBuf>> {
    route(portal(request), || {
        [Helper::Zenity, Helper::Kdialog]
            .into_iter()
            .find_map(|helper| helper.ask(request))
    })
}

/// The portal's answer, or `helper`'s when the portal could not be
/// asked and a helper is installed to ask instead.
#[cfg(target_os = "linux")]
fn route(
    answer: Result<Vec<PathBuf>>,
    helper: impl FnOnce() -> Option<Result<Vec<PathBuf>>>,
) -> Result<Vec<PathBuf>> {
    let e = match answer {
        Err(e) if e.downcast_ref::<Unreached>().is_some() => e,
        answer => return answer,
    };
    tracing::info!("file chooser: {e:#}; trying zenity, then kdialog");
    helper().unwrap_or(Err(e))
}

/// Whether the portal's call failed because nothing was there to
/// take it: no portal on the bus, one that would not start, or no
/// file chooser behind it. Any other error is a portal that took the
/// request and refused it, our mistake (§88's empty filter was one)
/// and not a reason to ask zenity. zbus hands every error reply over
/// as a `MethodError`, by its D-Bus name.
#[cfg(target_os = "linux")]
fn absent(e: &zbus::Error) -> bool {
    match e {
        zbus::Error::MethodError(name, _, _) => absent_name(name.as_str()),
        _ => false,
    }
}

/// `absent` by the error's D-Bus name. The bus's own: no owner, no
/// such method, a start that failed (`Spawn.*`), or a start that
/// timed out. dbus-daemon sends `TimedOut` only for a start, and
/// drops the call; the portal sends it only when its backend would
/// not start, before it asks the backend. Either way no dialog was
/// shown. Any of systemd's, which dbus-daemon passes on when systemd
/// will not queue the portal's unit (none, masked, a bad setting);
/// dbus-broker says `NameHasNoOwner` for all of those. `NoReply` is
/// not one: the call did reach a portal, which may yet show its
/// dialog.
#[cfg(target_os = "linux")]
fn absent_name(name: &str) -> bool {
    if let Some(name) = name.strip_prefix("org.freedesktop.DBus.Error.") {
        matches!(
            name,
            "ServiceUnknown"
                | "NameHasNoOwner"
                | "UnknownMethod"
                | "UnknownObject"
                | "UnknownInterface"
                | "TimedOut"
        ) || name.starts_with("Spawn.")
    } else {
        name.starts_with("org.freedesktop.systemd1.")
    }
}

/// Why the portal could not be asked, as against a portal asked that
/// then failed: only this sends the request to a helper instead.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq)]
enum Unreached {
    /// No session bus: `DBUS_SESSION_BUS_ADDRESS` unset and nothing at
    /// `$XDG_RUNTIME_DIR/bus`.
    Bus,
    /// A bus, but no portal on it with a file chooser behind it, or
    /// one that would not start.
    Portal,
}

#[cfg(target_os = "linux")]
impl std::fmt::Display for Unreached {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(match self {
            Unreached::Bus => "no D-Bus session bus",
            Unreached::Portal => "no file chooser portal on the session bus",
        })
    }
}

/// The status line for a chooser that answered with an error: what
/// was missing and what to install when it was the desktop's
/// plumbing, the error itself otherwise.
pub fn chooser_failed(e: &anyhow::Error) -> String {
    #[cfg(target_os = "linux")]
    match e.downcast_ref::<Unreached>() {
        Some(Unreached::Bus) => {
            return "no file chooser: there is no D-Bus session bus; \
                    install zenity, or see the user guide"
                .into();
        }
        Some(Unreached::Portal) => {
            return "no file chooser: no desktop portal answered; \
                    install xdg-desktop-portal-gtk or zenity"
                .into();
        }
        None => {}
    }
    format!("the file chooser failed: {e:#}")
}

/// A program that draws a file chooser and prints the paths picked,
/// one a line: GNOME's zenity, KDE's kdialog.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq)]
enum Helper {
    Zenity,
    Kdialog,
}

#[cfg(target_os = "linux")]
impl Helper {
    fn program(self) -> &'static str {
        match self {
            Helper::Zenity => "zenity",
            Helper::Kdialog => "kdialog",
        }
    }

    /// The request as the program's arguments. Both take the folder
    /// to open on as a path, with the offered name on the end for a
    /// save; zenity wants the trailing slash to open inside a folder
    /// rather than offer it as the name.
    fn args(self, request: &Ask) -> Vec<std::ffi::OsString> {
        use std::ffi::OsString;
        let mut start = request.folder.clone().into_os_string();
        match &request.name {
            Some(name) => {
                start.push("/");
                start.push(name);
            }
            None if self == Helper::Zenity => start.push("/"),
            None => {}
        }
        let (name, globs) = &request.filter;
        let globs: Vec<&str> = globs
            .iter()
            .map(String::as_str)
            .filter(|g| !g.is_empty())
            .collect();
        let filter = (!name.is_empty() && !globs.is_empty()).then(|| (name, globs.join(" ")));
        let mut args: Vec<OsString> = Vec::new();
        match self {
            Helper::Zenity => {
                args.push("--file-selection".into());
                args.push(format!("--title={}", request.title).into());
                let mut filename = OsString::from("--filename=");
                filename.push(&start);
                args.push(filename);
                if request.name.is_some() {
                    args.push("--save".into());
                    args.push("--confirm-overwrite".into());
                }
                if request.directory {
                    args.push("--directory".into());
                }
                if request.multiple {
                    args.push("--multiple".into());
                    args.push("--separator=\n".into());
                }
                if let Some((name, globs)) = filter {
                    args.push(format!("--file-filter={name} | {globs}").into());
                }
            }
            Helper::Kdialog => {
                args.push("--title".into());
                args.push(request.title.into());
                args.push(
                    if request.directory {
                        "--getexistingdirectory"
                    } else if request.name.is_some() {
                        "--getsavefilename"
                    } else {
                        "--getopenfilename"
                    }
                    .into(),
                );
                args.push(start);
                if let Some((name, globs)) = filter
                    && !request.directory
                {
                    // Qt's form: kdialog splits on `|` into separate
                    // filters, so `*.jpg|JPEG` would offer one that
                    // matches a file named JPEG.
                    args.push(format!("{name} ({globs})").into());
                }
                if request.multiple {
                    args.push("--multiple".into());
                    args.push("--separate-output".into());
                }
            }
        }
        args
    }

    /// The chooser, waited on: the paths picked, none for a cancel,
    /// and nothing at all when the program is not installed, so the
    /// next is tried.
    fn ask(self, request: &Ask) -> Option<Result<Vec<PathBuf>>> {
        self.run(self.program().as_ref(), request)
    }

    /// `ask` with the program named by path, for the tests' stand-ins.
    /// Both programs exit 1 for a cancel, but GTK exits 1 too when it
    /// cannot open the display, which is told apart by what it says
    /// (`display_failure`).
    /// zenity takes its exit codes from `ZENITY_OK` and the like when
    /// they are set, so they are not passed on. A path picked to open
    /// that is not there was mangled on its way back (kdialog prints
    /// in the locale's encoding, and a name with a newline in it
    /// splits in two), and is said so rather than opened.
    fn run(self, program: &std::ffi::OsStr, request: &Ask) -> Option<Result<Vec<PathBuf>>> {
        let name = self.program();
        let mut command = std::process::Command::new(program);
        for var in [
            "ZENITY_OK",
            "ZENITY_CANCEL",
            "ZENITY_ESC",
            "ZENITY_ERROR",
            "ZENITY_EXTRA",
            "ZENITY_TIMEOUT",
        ] {
            command.env_remove(var);
        }
        let output = match command
            .args(self.args(request))
            .stdin(std::process::Stdio::null())
            .output()
        {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => return Some(Err(e).with_context(|| format!("starting {name}"))),
            Ok(output) => output,
        };
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        Some(match output.status.code() {
            Some(0) => {
                let paths = helper_paths(&output.stdout);
                match paths.iter().find(|p| request.name.is_none() && !p.exists()) {
                    Some(gone) => Err(anyhow::anyhow!(
                        "{name} answered with {}, which is not there",
                        gone.display()
                    )),
                    None => Ok(paths),
                }
            }
            Some(1) if display_failure(stderr).is_none() => Ok(Vec::new()),
            _ => {
                tracing::warn!("file chooser: {name}: {stderr}");
                match display_failure(stderr) {
                    Some(line) => Err(anyhow::anyhow!("{name}: {line}")),
                    None => Err(anyhow::anyhow!("{name} {}", output.status)),
                }
            }
        })
    }
}

/// The line of a helper's stderr that says it could not reach the
/// display: GTK 3's "cannot open display" and GTK 4's "failed to
/// open display" before they exit 1, Qt's "could not connect to
/// display" before it aborts. Only those: GTK
/// mentions the display in warnings it prints on a dialog that works,
/// as Xlib's missing RANDR under Xvfb.
#[cfg(target_os = "linux")]
fn display_failure(stderr: &str) -> Option<&str> {
    stderr.lines().map(str::trim).find(|line| {
        let line = line.to_lowercase();
        [
            "cannot open display",
            "failed to open display",
            "could not connect to display",
        ]
        .iter()
        .any(|phrase| line.contains(phrase))
    })
}

/// A helper's output as paths: one a line, the blank ones dropped.
#[cfg(target_os = "linux")]
fn helper_paths(stdout: &[u8]) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    stdout
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| PathBuf::from(std::ffi::OsString::from_vec(line.to_vec())))
        .collect()
}

/// `start` if it is a folder, else the nearest of its parents that
/// is: a chooser opened on a folder since removed would otherwise
/// open nowhere in particular. A mount point left behind with nothing
/// mounted on it is still a folder, and passes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn nearest_folder(start: &Path) -> PathBuf {
    start
        .ancestors()
        .find(|p| p.is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| start.to_path_buf())
}

/// Everywhere else it is the platform's own dialog, through `rfd`.
/// AppKit and the Windows common dialogs want the thread their window
/// is on, so the dialog is built on the event loop's thread rather
/// than a thread of its own, and awaited there too: `rfd` hands back
/// a future the loop can poll, so the window behind the dialog goes
/// on drawing. `done` then runs on that thread, where the callers'
/// `invoke_from_event_loop` is a queued call like any other.
#[cfg(not(target_os = "linux"))]
fn ask(request: Ask, done: impl FnOnce(Result<Vec<PathBuf>>) + Send + 'static) {
    if cfg!(test) {
        return done(Ok(Vec::new()));
    }
    // `done` is answered exactly once, by whichever of the two arms
    // gets to it: the dialog when there is an event loop to put it
    // on, this thread when there is not. It waits in a slot both can
    // reach because posting to the loop takes ownership of it.
    let slot = Arc::new(Mutex::new(Some(done)));
    let posted = {
        let slot = slot.clone();
        slint::invoke_from_event_loop(move || {
            let dialog = native(&request);
            let spawned = slint::spawn_local(async move {
                let picked: Vec<rfd::FileHandle> = if request.directory {
                    dialog.pick_folder().await.into_iter().collect()
                } else if request.name.is_some() {
                    dialog.save_file().await.into_iter().collect()
                } else if request.multiple {
                    dialog.pick_files().await.unwrap_or_default()
                } else {
                    dialog.pick_file().await.into_iter().collect()
                };
                if let Some(done) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    done(Ok(picked.iter().map(|f| f.path().to_path_buf()).collect()));
                }
            });
            if spawned.is_err() {
                tracing::error!("file chooser: the event loop would not take it");
            }
        })
    };
    if posted.is_err()
        && let Some(done) = slot.lock().unwrap_or_else(|e| e.into_inner()).take()
    {
        // No window yet, so nowhere to put a dialog; the caller hears
        // it the way it hears a missing portal.
        done(Err(anyhow::anyhow!("no event loop to ask the desktop on")));
    }
}

/// The platform's dialog as `rfd` builds it. A filter's globs are
/// patterns to the portal but bare extensions here, so `*.jpg`
/// becomes `jpg`; a filter with nothing left is not offered.
#[cfg(not(target_os = "linux"))]
fn native(request: &Ask) -> rfd::AsyncFileDialog {
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title(request.title)
        .set_directory(&request.folder);
    if let Some(name) = &request.name {
        dialog = dialog.set_file_name(name);
    }
    let (name, globs) = &request.filter;
    let extensions = native_extensions(globs);
    if !name.is_empty() && !extensions.is_empty() {
        dialog = dialog.add_filter(name, &extensions);
    }
    dialog
}

/// `*.jpg` and `*.JPG` to the one extension a native dialog matches
/// with, case and all, in the order they were given.
#[cfg(not(target_os = "linux"))]
fn native_extensions(globs: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(globs.len());
    for glob in globs {
        let extension = glob.rsplit('.').next().unwrap_or_default().to_lowercase();
        if extension.is_empty() || extension.contains('*') || out.contains(&extension) {
            continue;
        }
        out.push(extension);
    }
    out
}

/// The portal's `filters` option, `a(sa(us))`: each filter a name and
/// a list of patterns, each pattern a kind (0 a glob, 1 a MIME type)
/// and the pattern itself.
#[cfg(target_os = "linux")]
type PortalFilters = Vec<(String, Vec<(u32, String)>)>;

/// A name and its globs as the portal wants them. A filter must have
/// a name and at least one pattern, so an empty one is left out of
/// the options altogether rather than sent as a nameless entry, which
/// the portal refuses with `InvalidArgument`: a folder chooser asks
/// with no filter at all.
#[cfg(target_os = "linux")]
fn portal_filters(filter: (String, Vec<String>)) -> Option<PortalFilters> {
    let (name, globs) = filter;
    let globs: Vec<(u32, String)> = globs
        .into_iter()
        .filter(|g| !g.is_empty())
        .map(|g| (0u32, g))
        .collect();
    if name.is_empty() || globs.is_empty() {
        return None;
    }
    Some(vec![(name, globs)])
}

/// The portal's `SaveFile` or `OpenFile`: the request's folder is
/// where the chooser opens, its name is offered when there is one,
/// `directory` asks for a folder chooser rather than a file one, and
/// `multiple` for as many files as the user picks. Nothing picked is
/// a cancel.
#[cfg(target_os = "linux")]
fn portal(request: &Ask) -> Result<Vec<PathBuf>> {
    use std::collections::HashMap;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::{AtomicU32, Ordering};
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, SerializeDict, Type};

    #[derive(SerializeDict, Type)]
    #[zvariant(signature = "a{sv}")]
    struct Options {
        handle_token: String,
        modal: bool,
        /// Only a save dialog takes a name; left out otherwise.
        current_name: Option<String>,
        current_folder: Vec<u8>,
        /// Left out when there is no filter to offer: the portal
        /// refuses a filter whose name is empty.
        filters: Option<PortalFilters>,
        /// Only a folder chooser asks for a directory; left out
        /// otherwise.
        directory: Option<bool>,
        /// Likewise a chooser of several files.
        multiple: Option<bool>,
    }

    static COUNTER: AtomicU32 = AtomicU32::new(0);
    const PORTAL: &str = "org.freedesktop.portal.Desktop";
    let method = if request.name.is_some() {
        "SaveFile"
    } else {
        "OpenFile"
    };
    let conn = Connection::session().context(Unreached::Bus)?;
    let sender = conn
        .unique_name()
        .context(Unreached::Bus)?
        .trim_start_matches(':')
        .replace('.', "_");
    let token = format!(
        "greycard{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    // The reply comes as a signal on a request object whose path is
    // known in advance; listen before asking, so it cannot be missed.
    let request_path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let signals = Proxy::new(
        &conn,
        PORTAL,
        request_path.as_str(),
        "org.freedesktop.portal.Request",
    )?;
    let mut responses = signals.receive_signal("Response")?;
    let chooser = Proxy::new(
        &conn,
        PORTAL,
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.FileChooser",
    )?;
    let mut folder = request.folder.as_os_str().as_bytes().to_vec();
    folder.push(0);
    let options = Options {
        handle_token: token,
        modal: true,
        current_name: request.name.clone(),
        current_folder: folder,
        filters: portal_filters(request.filter.clone()),
        directory: request.directory.then_some(true),
        multiple: request.multiple.then_some(true),
    };
    // Nothing is on the screen until the call returns, so a call that
    // found nothing to take it can go to a helper without a second
    // dialog.
    let handle: OwnedObjectPath = match chooser.call(method, &("", request.title, options)) {
        Ok(handle) => handle,
        Err(e) if absent(&e) => return Err(e).context(Unreached::Portal),
        Err(e) => return Err(e).context("the file chooser portal"),
    };
    if handle.as_str() != request_path {
        // An old portal names the request itself.
        let signals = Proxy::new(&conn, PORTAL, handle, "org.freedesktop.portal.Request")?;
        responses = signals.receive_signal("Response")?;
    }
    let message = responses.next().context("the file chooser went away")?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize()?;
    // 1 is the Cancel button. 2 is the chooser ended some other way:
    // its window closed or Escape pressed, which the released GTK and
    // GNOME backends answer with 2 (xdg-desktop-portal-gtk's main
    // answers 1 since 2025), or the backend failing after the call
    // returned. The two cannot be told apart from here, and the first
    // is the common one, so 2 is a cancel too, with a line in the log
    // for the second; the portal's own log says which it was.
    match code {
        0 => {}
        1 => return Ok(Vec::new()),
        _ => {
            tracing::warn!(
                "file chooser: the portal ended it without an answer (response {code}): \
                 its window closed, or its backend failed"
            );
            return Ok(Vec::new());
        }
    }
    let uris = results.get("uris").context("a reply without a file")?;
    let uris = Vec::<String>::try_from(uris.clone()).context("the reply's file")?;
    if uris.is_empty() {
        anyhow::bail!("a reply without a file");
    }
    uris.iter()
        .map(|uri| {
            let path = uri
                .strip_prefix("file://")
                .with_context(|| format!("not a local file: {uri}"))?;
            Ok(PathBuf::from(percent_decode(path)))
        })
        .collect()
}

/// `%20` and friends back to bytes.
#[cfg(target_os = "linux")]
fn percent_decode(s: &str) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"),
                16,
            )
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    std::ffi::OsString::from_vec(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::Format;

    #[test]
    fn a_chooser_opens_on_the_nearest_folder_that_is_there() {
        let dir = crate::testing::scratch_dir("nearest-folder");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(nearest_folder(&dir), dir);
        assert_eq!(nearest_folder(&dir.join("gone").join("deeper")), dir);
        crate::testing::remove_dir_retry(&dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_folder_chooser_sends_no_filter() {
        // The portal refuses a filter with an empty name, so the
        // folder chooser's empty filter has to be left out.
        assert_eq!(portal_filters((String::new(), Vec::new())), None);
        assert_eq!(portal_filters((String::new(), vec!["*.jpg".into()])), None);
        assert_eq!(portal_filters(("JPEG".into(), Vec::new())), None);
        assert_eq!(portal_filters(("JPEG".into(), vec![String::new()])), None);
        // A real filter still goes as it did: globs are glob-type 0.
        assert_eq!(
            portal_filters(("JPEG".into(), vec!["*.jpg".into(), "*.jpeg".into()])),
            Some(vec![(
                "JPEG".to_string(),
                vec![(0u32, "*.jpg".to_string()), (0u32, "*.jpeg".to_string())]
            )])
        );
        // Every export format offers one.
        for f in Format::ALL {
            let globs = f.globs().iter().map(|g| g.to_string()).collect();
            assert!(portal_filters((f.name().to_string(), globs)).is_some());
        }
    }

    #[cfg(target_os = "linux")]
    fn asking(name: Option<&str>, directory: bool, multiple: bool) -> Ask {
        Ask {
            title: "Pick",
            folder: PathBuf::from("/home/me/My Pictures"),
            name: name.map(str::to_string),
            filter: if directory {
                (String::new(), Vec::new())
            } else {
                ("JPEG".into(), vec!["*.jpg".into(), "*.jpeg".into()])
            },
            directory,
            multiple,
        }
    }

    #[cfg(target_os = "linux")]
    fn strings(args: Vec<std::ffi::OsString>) -> Vec<String> {
        args.into_iter().map(|a| a.into_string().unwrap()).collect()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn zenity_is_asked_what_the_portal_would_be() {
        let zenity = |ask: Ask| strings(Helper::Zenity.args(&ask));
        // A folder opens inside the folder, with no filter.
        assert_eq!(
            zenity(asking(None, true, false)),
            [
                "--file-selection",
                "--title=Pick",
                "--filename=/home/me/My Pictures/",
                "--directory"
            ]
        );
        // A save offers its name and confirms a replace.
        assert_eq!(
            zenity(asking(Some("a.jpg"), false, false)),
            [
                "--file-selection",
                "--title=Pick",
                "--filename=/home/me/My Pictures/a.jpg",
                "--save",
                "--confirm-overwrite",
                "--file-filter=JPEG | *.jpg *.jpeg"
            ]
        );
        // Several files come back one a line.
        assert_eq!(
            zenity(asking(None, false, true)),
            [
                "--file-selection",
                "--title=Pick",
                "--filename=/home/me/My Pictures/",
                "--multiple",
                "--separator=\n",
                "--file-filter=JPEG | *.jpg *.jpeg"
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kdialog_is_asked_what_the_portal_would_be() {
        let kdialog = |ask: Ask| strings(Helper::Kdialog.args(&ask));
        assert_eq!(
            kdialog(asking(None, true, false)),
            [
                "--title",
                "Pick",
                "--getexistingdirectory",
                "/home/me/My Pictures"
            ]
        );
        assert_eq!(
            kdialog(asking(Some("a.jpg"), false, false)),
            [
                "--title",
                "Pick",
                "--getsavefilename",
                "/home/me/My Pictures/a.jpg",
                "JPEG (*.jpg *.jpeg)"
            ]
        );
        assert_eq!(
            kdialog(asking(None, false, true)),
            [
                "--title",
                "Pick",
                "--getopenfilename",
                "/home/me/My Pictures",
                "JPEG (*.jpg *.jpeg)",
                "--multiple",
                "--separate-output"
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_helpers_answer_is_one_path_a_line() {
        assert_eq!(
            helper_paths(b"/a/b c.jpg\n/d/e.jpg\n"),
            [PathBuf::from("/a/b c.jpg"), PathBuf::from("/d/e.jpg")]
        );
        assert_eq!(helper_paths(b""), Vec::<PathBuf>::new());
        assert_eq!(helper_paths(b"\n"), Vec::<PathBuf>::new());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn only_a_portal_that_is_not_there_goes_to_a_helper() {
        for name in [
            "ServiceUnknown",
            "NameHasNoOwner",
            "UnknownMethod",
            "UnknownObject",
            "UnknownInterface",
            "Spawn.ChildExited",
            "Spawn.ExecFailed",
        ] {
            assert!(absent_name(&format!("org.freedesktop.DBus.Error.{name}")));
        }
        // The bus timing out a start answers before the portal.
        assert!(absent_name("org.freedesktop.DBus.Error.TimedOut"));
        // A portal started as a systemd unit that systemd would not
        // queue.
        for name in ["NoSuchUnit", "UnitMasked", "BadUnitSetting", "LoadFailed"] {
            assert!(absent_name(&format!("org.freedesktop.systemd1.{name}")));
        }
        // A portal that took the request and refused it is ours to
        // hear, and one that did not reply may yet show its dialog.
        for name in [
            "org.freedesktop.DBus.Error.InvalidArgs",
            "org.freedesktop.DBus.Error.NoReply",
            "org.freedesktop.portal.Error.InvalidArgument",
            "ServiceUnknown",
        ] {
            assert!(!absent_name(name), "{name}");
        }
        assert!(!absent(&zbus::Error::InvalidReply));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_helper_is_asked_only_when_the_portal_was_not_reached() {
        let unreached =
            || Err::<Vec<PathBuf>, _>(anyhow::anyhow!("ServiceUnknown")).context(Unreached::Portal);
        let picked = || Some(Ok(vec![PathBuf::from("/picked")]));
        // The portal's own answer, picked or canceled, stands.
        let asked = std::cell::Cell::new(false);
        let answer = route(Ok(vec![PathBuf::from("/portal")]), || {
            asked.set(true);
            picked()
        });
        assert_eq!(answer.unwrap(), [PathBuf::from("/portal")]);
        assert!(!asked.get());
        // A portal that failed after it was asked is not asked again.
        let failed = route(Err(anyhow::anyhow!("the file chooser went away")), || {
            asked.set(true);
            picked()
        });
        assert!(failed.is_err());
        assert!(!asked.get());
        // Not reached: the helper's answer.
        assert_eq!(
            route(unreached(), picked).unwrap(),
            [PathBuf::from("/picked")]
        );
        // Not reached and no helper: the portal's error, which says
        // what to install.
        let none = route(unreached(), || None).unwrap_err();
        assert!(chooser_failed(&none).contains("xdg-desktop-portal-gtk"));
    }

    /// A stand-in for zenity: a shell script with `body`, in `dir`.
    #[cfg(target_os = "linux")]
    fn stub(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_helpers_exit_says_picked_canceled_or_failed() {
        let dir = crate::testing::scratch_dir("helper-exits");
        std::fs::create_dir_all(dir.join("a b")).unwrap();
        std::fs::create_dir_all(dir.join("c")).unwrap();
        let run_asking = |body: &str, name: &str, ask: Ask| {
            let program = stub(&dir, name, body);
            // A script just written can be busy for a moment, while a
            // child another test forked still holds it open to write.
            for _ in 0..50 {
                let answer = Helper::Zenity.run(program.as_os_str(), &ask);
                let busy = matches!(&answer, Some(Err(e)) if e
                    .root_cause()
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::ExecutableFileBusy));
                if !busy {
                    return answer;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            panic!("{name} stayed busy");
        };
        let run = |body: &str, name: &str| run_asking(body, name, asking(None, false, true));
        // Two picked, one a line.
        let two = format!(
            "printf '%s\\n%s\\n' '{}' '{}'",
            dir.join("a b").display(),
            dir.join("c").display()
        );
        assert_eq!(
            run(&two, "two").unwrap().unwrap(),
            [dir.join("a b"), dir.join("c")]
        );
        // A cancel is nothing picked, not an error.
        assert_eq!(
            run("exit 1", "cancel").unwrap().unwrap(),
            Vec::<PathBuf>::new()
        );
        // GTK's exit 1 without a display is an error that says so.
        let no_display = run(
            "echo '(zenity:1): Gtk-WARNING **: cannot open display: :9' >&2; exit 1",
            "no-display",
        )
        .unwrap()
        .unwrap_err();
        assert!(no_display.to_string().contains("cannot open display"));
        // GTK 4's, as zenity 4 says it.
        let no_display = run("echo 'Failed to open display' >&2; exit 1", "no-display-4")
            .unwrap()
            .unwrap_err();
        assert!(no_display.to_string().contains("Failed to open display"));
        // A warning that mentions the display is still a cancel.
        assert_eq!(
            run(
                "echo 'Xlib:  extension \"RANDR\" missing on display \":1\".' >&2; exit 1",
                "randr",
            )
            .unwrap()
            .unwrap(),
            Vec::<PathBuf>::new()
        );
        // Anything else is an error.
        assert!(run("exit 255", "crash").unwrap().is_err());
        // A path that is not there came back mangled.
        assert!(
            run(&format!("echo '{}'", dir.join("gone").display()), "mangled")
                .unwrap()
                .is_err()
        );
        // A save names a file not there yet, and is not checked.
        let new = dir.join("new.jpg");
        assert_eq!(
            run_asking(
                &format!("echo '{}'", new.display()),
                "save",
                asking(Some("new.jpg"), false, false)
            )
            .unwrap()
            .unwrap(),
            [new]
        );
        // Not installed: the next helper is tried.
        assert!(
            Helper::Zenity
                .run(dir.join("missing").as_os_str(), &asking(None, true, false))
                .is_none()
        );
        crate::testing::remove_dir_retry(&dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_missing_bus_or_portal_says_what_to_install() {
        let no_bus = Err::<(), _>(anyhow::anyhow!("connection refused"))
            .context(Unreached::Bus)
            .unwrap_err();
        assert!(chooser_failed(&no_bus).contains("no D-Bus session bus"));
        assert!(chooser_failed(&no_bus).contains("zenity"));
        let no_portal = Err::<(), _>(anyhow::anyhow!("ServiceUnknown"))
            .context(Unreached::Portal)
            .unwrap_err();
        assert!(chooser_failed(&no_portal).contains("xdg-desktop-portal-gtk"));
        // A portal that was asked and failed says so as it is.
        let failed = anyhow::anyhow!("the file chooser went away");
        assert_eq!(
            chooser_failed(&failed),
            "the file chooser failed: the file chooser went away"
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn a_native_filter_takes_extensions_not_globs() {
        // A native dialog matches on the extension itself, so the
        // portal's patterns have to come apart; case folds, and a
        // pattern that leaves nothing behind is dropped.
        assert_eq!(
            native_extensions(&["*.jpg".to_string(), "*.jpeg".to_string()]),
            vec!["jpg".to_string(), "jpeg".to_string()]
        );
        // The same extension in two cases is one entry, in the order
        // it was first given.
        assert_eq!(
            native_extensions(&[
                "*.xmp".to_string(),
                "*.XMP".to_string(),
                "*.gcp".to_string()
            ]),
            vec!["xmp".to_string(), "gcp".to_string()]
        );
        assert_eq!(native_extensions(&[]), Vec::<String>::new());
        assert_eq!(native_extensions(&[String::new()]), Vec::<String>::new());
        assert_eq!(native_extensions(&["*".to_string()]), Vec::<String>::new());
        // Every export format still offers one.
        for f in Format::ALL {
            let globs: Vec<String> = f.globs().iter().map(|g| g.to_string()).collect();
            assert!(!native_extensions(&globs).is_empty());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn percent_decoding() {
        assert_eq!(
            percent_decode("/a%20b/c%2Fd"),
            std::ffi::OsString::from("/a b/c/d")
        );
        assert_eq!(percent_decode("plain"), std::ffi::OsString::from("plain"));
        assert_eq!(percent_decode("bad%"), std::ffi::OsString::from("bad%"));
    }
}
