//! One editor a user and profile. A launch with a path, or with none,
//! first asks whether an editor of the same user and the same
//! configuration folder is running; if one answers, the launch hands
//! it the path and ends, and the running editor opens it and comes
//! forward. Otherwise the launch becomes that editor and listens. On
//! Linux and macOS the two meet at a Unix socket in the runtime folder
//! (`XDG_RUNTIME_DIR`, when it is the user's alone), else in the cache
//! folder (`socket`); on Windows at a named pipe only the user and the
//! system may open (`pipe`). Each side checks the other is the same
//! user before a byte of the paths is trusted or sent.
//!
//! No window here: the address, the wire, the connect or bind, and a
//! listener that hands each launch to a closure. The window side opens
//! the paths (`panel::handed`).

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[cfg(windows)]
mod pipe;
#[cfg(unix)]
mod socket;
#[cfg(windows)]
use pipe as sys;
#[cfg(unix)]
use socket as sys;
#[cfg(unix)]
pub use socket::socket_path;

/// What every message starts with: a stray connection, or another
/// program's, is told apart from a launch.
const MAGIC: &[u8; 8] = b"greycrd1";
/// The running editor's answer once the window has the paths,
/// followed by its process id.
const TAKEN: &[u8; 2] = b"ok";
/// More paths than any desktop sends, and longer ones than any file
/// system keeps: past these the message is not a launch's. Likewise
/// the tokens.
const MOST_PATHS: u32 = 4096;
const LONGEST: u32 = 1 << 16;
const MOST_TOKENS: u32 = 16;
const LONGEST_TOKEN: u32 = 4096;
/// How long a launch waits for the running editor to say the window
/// has its paths. The editor reads them on a thread of its own and
/// answers once the window's queue has them; a long wait is an editor
/// still starting, whose thread is not yet listening, or one hung.
/// Past it the launch opens a window of its own.
const ANSWER_WAIT: Duration = Duration::from_secs(10);
/// How long the running editor waits on a launch that has connected
/// for what it sends, and for it to hang up after the answer.
const READ_WAIT: Duration = Duration::from_secs(5);
/// The variables a desktop sets for the launch it starts, which a
/// window would need to be let forward by the compositor (Wayland's
/// xdg-activation, X11's startup notification). Carried over; the
/// running editor only logs them, winit 0.30 having no way to
/// activate a window that is up with a token from outside.
const TOKENS: [&str; 2] = ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"];

/// Whether this command line takes part: a path alone, or nothing,
/// with `-v` as often as it likes, `--cpu-ops`, which the start
/// again after a lost GPU adds, and `--after-model-loss`, which one
/// after a model lost it adds too. Any other flag is a run the command
/// line arranges (an export, a capture, a measurement), which keeps
/// its own window, or none, and never hands over or listens.
pub fn plain_launch(args: &[OsString]) -> bool {
    let mut positional = 0;
    let mut flags_over = false;
    for arg in args {
        if !flags_over {
            let bytes = arg.as_encoded_bytes();
            if bytes == b"--" {
                flags_over = true;
                continue;
            }
            if bytes == b"--verbose"
                || bytes == b"--cpu-ops"
                || bytes == b"--after-model-loss"
                || (bytes.len() > 1 && bytes[0] == b'-' && bytes[1..].iter().all(|&b| b == b'v'))
            {
                continue;
            }
            if bytes.first() == Some(&b'-') {
                return false;
            }
        }
        positional += 1;
    }
    positional <= 1
}

/// The paths to hand over: absolute, as a running editor with another
/// working folder can open them, and otherwise as given. Not made
/// canonical: a link, a mapped drive or a `subst` stays as the user
/// named it, so the frame lands under the root it would on a cold
/// start. A path that is not there hands over nothing (`None`): the
/// launch goes on alone and says so, as the command line always has.
pub fn absolute(path: Option<&Path>) -> Option<Vec<PathBuf>> {
    match path {
        None => Some(Vec::new()),
        Some(path) => std::path::absolute(path)
            .ok()
            .filter(|p| p.exists())
            .map(|p| vec![p]),
    }
}

/// What one launch hands over: its paths, none for a launch that asks
/// only that the window come forward, and the desktop's tokens it was
/// started with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    pub paths: Vec<PathBuf>,
    pub tokens: Vec<(String, String)>,
}

impl Launch {
    /// These paths, with the tokens this process was started with.
    pub fn here(paths: Vec<PathBuf>) -> Self {
        let tokens = TOKENS
            .iter()
            .filter_map(|name| {
                let value = std::env::var(name).ok().filter(|v| !v.is_empty())?;
                Some(((*name).to_string(), value))
            })
            .collect();
        Self { paths, tokens }
    }
}

/// The launch, written as it is sent: the magic, a count, then each
/// path as its length and its bytes; then a count of tokens, each a
/// name and a value as length and UTF-8 bytes. On Unix a path's bytes
/// are its own, so a name that is not UTF-8 arrives as it was; on
/// Windows they are its UTF-16 units, unpaired surrogates and all.
pub fn write_launch(to: &mut impl Write, launch: &Launch) -> io::Result<()> {
    let mut message = MAGIC.to_vec();
    let counted = |n: usize, most: u32, what: &str| {
        u32::try_from(n)
            .ok()
            .filter(|&n| n <= most)
            .ok_or_else(|| invalid(what))
    };
    message.extend_from_slice(
        &counted(launch.paths.len(), MOST_PATHS, "too many paths")?.to_le_bytes(),
    );
    for path in &launch.paths {
        let bytes = path_bytes(path);
        message.extend_from_slice(&counted(bytes.len(), LONGEST, "a path too long")?.to_le_bytes());
        message.extend_from_slice(&bytes);
    }
    message.extend_from_slice(
        &counted(launch.tokens.len(), MOST_TOKENS, "too many tokens")?.to_le_bytes(),
    );
    for (name, value) in &launch.tokens {
        for text in [name, value] {
            message.extend_from_slice(
                &counted(text.len(), LONGEST_TOKEN, "a token too long")?.to_le_bytes(),
            );
            message.extend_from_slice(text.as_bytes());
        }
    }
    to.write_all(&message)?;
    to.flush()
}

/// The launch a launch sent, read as `write_launch` wrote it.
pub fn read_launch(from: &mut impl Read) -> io::Result<Launch> {
    let mut magic = [0; 8];
    from.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(invalid("not a launch of greycard"));
    }
    let count = read_u32(from)?;
    if count > MOST_PATHS {
        return Err(invalid("too many paths"));
    }
    let mut paths = Vec::with_capacity(count as usize);
    for _ in 0..count {
        paths.push(path_of(read_bytes(from, LONGEST)?)?);
    }
    let count = read_u32(from)?;
    if count > MOST_TOKENS {
        return Err(invalid("too many tokens"));
    }
    let mut tokens = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let text = |from: &mut _| {
            String::from_utf8(read_bytes(from, LONGEST_TOKEN)?)
                .map_err(|_| invalid("a token not UTF-8"))
        };
        tokens.push((text(from)?, text(from)?));
    }
    Ok(Launch { paths, tokens })
}

fn read_u32(from: &mut impl Read) -> io::Result<u32> {
    let mut four = [0; 4];
    from.read_exact(&mut four)?;
    Ok(u32::from_le_bytes(four))
}

fn read_bytes(from: &mut impl Read, most: u32) -> io::Result<Vec<u8>> {
    let len = read_u32(from)?;
    if len > most {
        return Err(invalid("a field too long"));
    }
    let mut bytes = vec![0; len as usize];
    from.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
fn path_of(bytes: Vec<u8>) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(windows)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(windows)]
fn path_of(bytes: Vec<u8>) -> io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    if !bytes.len().is_multiple_of(2) {
        return Err(invalid("a path of half a unit"));
    }
    let wide: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .collect();
    Ok(PathBuf::from(OsString::from_wide(&wide)))
}

/// The launch's side once connected: the launch sent, and the running
/// editor's answer waited for; its process id.
fn hand(stream: &mut (impl Read + Write), launch: &Launch) -> io::Result<u32> {
    write_launch(stream, launch)?;
    let mut answer = [0; 2];
    stream.read_exact(&mut answer).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof => io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "the running editor did not take the paths",
        ),
        _ => e,
    })?;
    if &answer != TAKEN {
        return Err(invalid("the running editor answered something else"));
    }
    read_u32(stream)
}

/// The running editor's side of one launch: read, and answered only
/// once `take` says the window has it. Not answered (an editor that is
/// closing, a window gone), the launch opens a window of its own.
/// Whether it was answered.
fn serve_one(
    stream: &mut (impl Read + Write),
    left: &AtomicBool,
    take: &impl Fn(Launch) -> bool,
) -> bool {
    let launch = match read_launch(stream) {
        Ok(launch) => launch,
        Err(e) => {
            tracing::warn!("another launch's paths could not be read: {e}");
            return false;
        }
    };
    if left.load(Ordering::SeqCst) {
        tracing::info!("another launch came as this editor closes; it opens its own window");
        return false;
    }
    tracing::info!("another launch handed over {:?}", launch.paths);
    for (name, _) in &launch.tokens {
        tracing::info!("the launch carried {name}; winit cannot activate the window with it");
    }
    if !take(launch) {
        tracing::warn!("the window could not take another launch's paths; it opens its own");
        return false;
    }
    let mut answer = TAKEN.to_vec();
    answer.extend_from_slice(&std::process::id().to_le_bytes());
    if let Err(e) = stream.write_all(&answer).and_then(|()| stream.flush()) {
        tracing::warn!("another launch was not answered: {e}");
        return false;
    }
    true
}

/// What a launch came to.
pub enum Claim {
    /// No editor answered: this launch is the editor, and listens.
    Instance(Instance),
    /// A running editor, this process id, took the paths; this launch
    /// ends.
    Handed(u32),
    /// Neither: the address could not be had, it was another user's,
    /// or the editor at it did not answer. The launch runs a window of
    /// its own and listens for nobody.
    Alone(io::Error),
}

/// A claim, and whether a socket left by an editor that did not quit
/// cleanly was taken away first.
pub struct Claimed {
    pub claim: Claim,
    pub stale_removed: bool,
}

/// Where the user's editor listens.
#[derive(Clone, Debug)]
pub struct Address {
    #[cfg(unix)]
    socket: PathBuf,
    #[cfg(windows)]
    pipe: Vec<u16>,
    #[cfg(windows)]
    sid: String,
    shown: String,
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.shown)
    }
}

impl Address {
    /// The user's, for the editor whose configuration folder is
    /// `config`: a run with a folder of its own (a second profile, a
    /// test's) meets only editors of that folder. On Unix a socket
    /// named by the user id; on Windows a pipe named by the user's SID
    /// and the session. None where there is nowhere to put it.
    pub fn user(config: Option<&Path>) -> Option<Self> {
        sys::user(&config_key(config))
    }

    /// Hand the launch to the editor listening here, or become it.
    pub fn claim(&self, launch: &Launch) -> Claimed {
        sys::claim(self, launch)
    }
}

/// A short name for the configuration folder: a hash that stays the
/// same from one build to the next, as std's does not promise to.
pub fn config_key(config: Option<&Path>) -> String {
    let bytes = config.map(path_bytes).unwrap_or_default();
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{:08x}", (hash ^ (hash >> 32)) as u32)
}

/// The editor listening, not yet serving: the listener waits until
/// the window can take what arrives, launches meanwhile waiting in its
/// backlog. Dropped unserved (a start that failed before its window),
/// the socket goes as a served one's does.
pub struct Instance {
    #[cfg(unix)]
    listener: std::os::unix::net::UnixListener,
    #[cfg(windows)]
    pipe: std::os::windows::io::OwnedHandle,
    address: Address,
    removal: Served,
}

impl Instance {
    pub fn address(&self) -> &Address {
        &self.address
    }

    /// Listen on a thread of its own, each launch handed to `take`
    /// there as it comes, and answered when `take` says the window has
    /// it. The listening stops, and the socket goes, when what this
    /// returns is dropped or left.
    pub fn serve(self, take: impl Fn(Launch) -> bool + Send + 'static) -> Served {
        let Instance {
            address,
            #[cfg(unix)]
            listener,
            #[cfg(windows)]
            pipe,
            removal,
        } = self;
        let left = removal.left.clone();
        #[cfg(unix)]
        let serve = move || {
            let _ = address;
            sys::serve(listener, &left, take);
        };
        #[cfg(windows)]
        let serve = move || sys::serve(pipe, &address, &left, take);
        if let Err(e) = std::thread::Builder::new()
            .name("instance".into())
            .spawn(serve)
        {
            tracing::warn!("no thread to listen for other launches on: {e}");
        }
        removal
    }
}

/// The editor serving. Dropped, or left, it answers no launch more and
/// the socket goes, so the next launch finds no editor rather than a
/// closing one.
pub struct Served {
    left: Arc<AtomicBool>,
    #[cfg(unix)]
    socket: Option<(PathBuf, (u64, u64))>,
}

impl Served {
    /// From the window's last moment, before the saves that follow it.
    pub fn leave(&mut self) {
        self.left.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        if let Some((path, made)) = self.socket.take() {
            socket::remove(&path, made);
        }
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        self.leave();
    }
}

#[cfg(test)]
mod tests;
