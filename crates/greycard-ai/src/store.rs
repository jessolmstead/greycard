//! Where model files live and how they arrive: a cache directory, a
//! download with a progress callback, resumed for a large file whose
//! last download failed partway, and a hash check before the file is
//! trusted.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::registry::{File, Model, RETIRED, Retired, Runtime};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no cache directory: XDG_CACHE_HOME is not set and the platform names none")]
    NoCacheDir,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("fetching {url}: {message}")]
    Http { url: String, message: String },
    #[error("{file} does not match its published hash: expected {expected}, got {got}")]
    Hash {
        file: String,
        expected: String,
        got: String,
    },
    #[error("{archive} {message}")]
    Archive { archive: String, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Download progress for one file of a model.
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub file: &'static str,
    pub done: u64,
    /// Zero when the server did not say.
    pub total: u64,
}

/// The model cache.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// The user's cache: `$XDG_CACHE_HOME/greycard/models` when that
    /// is set, else `greycard/models` under the platform's cache
    /// directory: `~/.cache` on Linux, `~/Library/Caches` on macOS,
    /// `%LOCALAPPDATA%` on Windows.
    pub fn user() -> Result<Self> {
        let base = cache_home().ok_or(Error::NoCacheDir)?;
        Ok(Self::at(base.join("greycard").join("models")))
    }

    /// The [`RETIRED`] files removed from the user's store on a thread
    /// of its own, since hashing one takes a while: for the editor and
    /// the CLI to call once as they start, and never a test.
    pub fn sweep_user_in_background() {
        let spawned = std::thread::Builder::new()
            .name("model store sweep".into())
            .spawn(|| {
                if let Ok(store) = Self::user() {
                    store.sweep(RETIRED);
                }
            });
        if let Err(e) = spawned {
            log::warn!("model store: no thread to sweep it on: {e}");
        }
    }

    /// Remove the `retired` files that are here at their size and
    /// hash, each logged with its size, and a retired model's
    /// directory with them when nothing but its license note is left.
    /// A file of the same name that is not the one listed is kept.
    /// What was removed, and its size.
    pub fn sweep(&self, retired: &[Retired]) -> Vec<(PathBuf, u64)> {
        let mut removed = Vec::new();
        for r in retired {
            let plain = |p: &str| {
                let mut c = Path::new(p).components();
                matches!(c.next(), Some(std::path::Component::Normal(_))) && c.next().is_none()
            };
            debug_assert!(plain(r.dir) && plain(r.file.name), "{r:?}");
            let dir = self.root.join(r.dir);
            let path = dir.join(r.file.name);
            if !std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == r.file.bytes) {
                continue;
            }
            let hashed = std::fs::File::open(&path).and_then(|mut f| {
                let mut hasher = Sha256::new();
                std::io::copy(&mut f, &mut hasher)?;
                Ok(hex(&hasher.finalize()))
            });
            if !hashed.is_ok_and(|h| h == r.file.sha256) {
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    log::info!(
                        "model store: removed {}, a retired model's file ({:.0} MB)",
                        path.display(),
                        r.file.bytes as f64 / 1e6
                    );
                    removed.push((path, r.file.bytes));
                }
                Err(e) => log::warn!(
                    "model store: {} is retired, not removed: {e}",
                    path.display()
                ),
            }
            let others = std::fs::read_dir(&dir).map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.file_name() != "LICENSE.txt")
                    .count()
            });
            if others.is_ok_and(|n| n == 0) {
                let _ = std::fs::remove_file(dir.join("LICENSE.txt"));
                let _ = std::fs::remove_dir(&dir);
            }
        }
        removed
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A model's directory: `<root>/<id>`.
    pub fn dir(&self, model: &Model) -> PathBuf {
        self.root.join(model.id)
    }

    /// One of a model's files.
    pub fn path(&self, model: &Model, file: &File) -> PathBuf {
        self.dir(model).join(file.name)
    }

    /// Whether every file of the model is here at its published size.
    /// (The hash was checked when it arrived.)
    pub fn have(&self, model: &Model) -> bool {
        model.files.iter().all(|f| {
            std::fs::metadata(self.path(model, f)).is_ok_and(|m| m.is_file() && m.len() == f.bytes)
        })
    }

    /// Fetch the model's files that are missing, checking each hash,
    /// and write the license note beside them. `progress` hears about
    /// every chunk. A file over [`RESUMABLE`] whose download failed
    /// partway is resumed from where it stopped.
    pub fn fetch(&self, model: &Model, mut progress: impl FnMut(Progress)) -> Result<()> {
        let dir = self.dir(model);
        std::fs::create_dir_all(&dir)?;
        for file in model.files {
            let path = self.path(model, file);
            if std::fs::metadata(&path).is_ok_and(|m| m.len() == file.bytes) {
                continue;
            }
            let part = path.with_file_name(format!("{}.part", file.name));
            fetch_to(file, &part, &mut progress)?;
            std::fs::rename(&part, &path)?;
        }
        std::fs::write(dir.join("LICENSE.txt"), model.license_note())?;
        Ok(())
    }

    /// A CPU-only runtime's directory: `<root>/<id>`, beside the models.
    pub fn runtime_dir(&self, runtime: &Runtime) -> PathBuf {
        self.root.join(runtime.id)
    }

    /// Where the runtime's library is kept.
    pub fn runtime_library(&self, runtime: &Runtime) -> PathBuf {
        self.runtime_dir(runtime).join(runtime.library.name)
    }

    /// Whether the runtime's library is here as Microsoft published
    /// it: at its size and its hash, the hash checked again each time
    /// this is asked, since it is code to be run and not a model's
    /// weights. Asked once a process, by the decision, and only on a
    /// machine that wants it.
    pub fn have_runtime(&self, runtime: &Runtime) -> bool {
        let path = self.runtime_library(runtime);
        if !std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == runtime.library.bytes)
        {
            return false;
        }
        let Ok(mut file) = std::fs::File::open(&path) else {
            return false;
        };
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher).is_ok()
            && hex(&hasher.finalize()) == runtime.library.sha256
    }

    /// Fetch the runtime's archive, checking its hash, take the
    /// library and the files that go with it out of it, each checked
    /// against its own hash, and write the license note beside them;
    /// the library's path comes back. Nothing is downloaded when every
    /// file is here already, at its size and (the library) its hash.
    /// The archive is removed once it is unpacked, and on any failure
    /// of its download or its unpacking, and the runtime's directory
    /// with it when that leaves it empty, so a failed fetch leaves
    /// nothing that reads as a runtime half had. `progress` hears
    /// about every chunk of the download.
    pub fn fetch_runtime(
        &self,
        runtime: &Runtime,
        mut progress: impl FnMut(Progress),
    ) -> Result<PathBuf> {
        let dir = self.runtime_dir(runtime);
        std::fs::create_dir_all(&dir)?;
        let fetched = (|| {
            let kept = runtime.kept.iter().all(|m| {
                std::fs::metadata(dir.join(m.name)).is_ok_and(|f| f.is_file() && f.len() == m.bytes)
            });
            if !(kept && self.have_runtime(runtime)) {
                let archive = dir.join(format!("{}.part", runtime.archive.name));
                fetch_to(&runtime.archive, &archive, &mut progress)?;
                let unpacked = crate::unpack::unpack(
                    &archive,
                    runtime.archive.name,
                    runtime.packing,
                    runtime.members(),
                    &dir,
                );
                let _ = std::fs::remove_file(&archive);
                unpacked?;
            }
            std::fs::write(dir.join("LICENSE.txt"), runtime.license_note())?;
            Ok(self.runtime_library(runtime))
        })();
        if fetched.is_err() {
            // Only when empty: a good library kept from before stays.
            let _ = std::fs::remove_dir(&dir);
        }
        fetched
    }

    /// Remove the runtime from the store: for a library that arrived
    /// whole and would not load here, so the next launch decides as
    /// before it was fetched rather than trusting it by its hash.
    pub fn remove_runtime(&self, runtime: &Runtime) -> Result<()> {
        match std::fs::remove_dir_all(self.runtime_dir(runtime)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

/// Files over this size keep what a failed download brought, and the
/// next fetch asks for the rest: a 5 GB model over a home connection
/// should not start again at a dropped connection. Smaller ones start
/// over, which costs little.
pub const RESUMABLE: u64 = 256 << 20;

/// The user's cache directory: `$XDG_CACHE_HOME` when that is set and
/// absolute, else the platform's (`~/.cache`, `~/Library/Caches`,
/// `%LOCALAPPDATA%`). greycard's own things go under `greycard/` in it.
pub(crate) fn cache_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::cache_dir)
}

/// Download `file` to `part`, checking its hash. On a failure of the
/// connection or a write, `part` is kept when the file is over
/// [`RESUMABLE`], for the next fetch to resume, and removed otherwise;
/// on a hash that does not match it is removed either way, as what it
/// holds is not the file.
fn fetch_to(file: &File, part: &Path, progress: &mut impl FnMut(Progress)) -> Result<()> {
    fetch_resuming(file, part, RESUMABLE, progress)
}

/// [`fetch_to`] with the size past which a part is kept as a
/// parameter, for the tests.
fn fetch_resuming(
    file: &File,
    part: &Path,
    resumable_over: u64,
    progress: &mut impl FnMut(Progress),
) -> Result<()> {
    let resumable = file.bytes > resumable_over;
    let result = download(file, part, resumable, progress);
    let keep = resumable && matches!(result, Err(Error::Http { .. } | Error::Io(_)));
    if result.is_err() && !keep {
        let _ = std::fs::remove_file(part);
    }
    result
}

fn http(url: &str, e: impl std::fmt::Display) -> Error {
    Error::Http {
        url: url.to_string(),
        message: e.to_string(),
    }
}

/// Whether a 206 answer's `Content-Range` starts where the part ends.
fn starts_at(response: &ureq::http::Response<ureq::Body>, offset: u64) -> bool {
    response
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes "))
        .and_then(|v| v.split('-').next())
        .and_then(|v| v.trim().parse::<u64>().ok())
        == Some(offset)
}

/// Fetch `file` into `part`. When `resumable` and a part is there
/// from before, shorter than the file, the request asks for the rest
/// (`Range: bytes=N-`) and a 206 from where the part ends is appended
/// to it, the part read into the hash first so the whole file is
/// checked; any other answer (a 200 from a server that ignores the
/// range, a 206 from elsewhere, a 416) starts over from nothing.
/// Progress starts at what is kept and counts to the whole file.
fn download(
    file: &File,
    part: &Path,
    resumable: bool,
    progress: &mut impl FnMut(Progress),
) -> Result<()> {
    let kept = if resumable {
        std::fs::metadata(part).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    let kept = if kept > file.bytes { 0 } else { kept };
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    if kept == file.bytes && kept > 0 {
        // All there: a fetch that stopped before its rename.
        let mut f = std::fs::File::open(part)?;
        std::io::copy(&mut f, &mut hasher)?;
        done = kept;
    } else {
        let mut request = ureq::get(file.url);
        if kept > 0 {
            request = request.header("Range", format!("bytes={kept}-"));
        }
        let mut response = match request.call() {
            Err(ureq::Error::StatusCode(416)) if kept > 0 => {
                log::info!(
                    "{}: the server would not resume at {kept} bytes; starting over",
                    file.name
                );
                std::fs::remove_file(part)?;
                return download(file, part, resumable, progress);
            }
            other => other.map_err(|e| http(file.url, e))?,
        };
        let resumed = kept > 0 && response.status() == 206 && starts_at(&response, kept);
        if kept > 0 && response.status() == 206 && !resumed {
            log::info!(
                "{}: the server resumed somewhere other than {kept} bytes; starting over",
                file.name
            );
            drop(response);
            std::fs::remove_file(part)?;
            return download(file, part, resumable, progress);
        }
        let mut out = if resumed {
            log::info!("{}: resuming at {kept} of {} bytes", file.name, file.bytes);
            let mut f = std::fs::File::open(part)?;
            std::io::copy(&mut f, &mut hasher)?;
            done = kept;
            std::fs::OpenOptions::new().append(true).open(part)?
        } else {
            if kept > 0 {
                log::info!(
                    "{}: the server answered {} to a range; starting over",
                    file.name,
                    response.status()
                );
            }
            std::fs::File::create(part)?
        };
        let total = if resumed {
            file.bytes
        } else {
            response.body().content_length().unwrap_or(file.bytes)
        };
        let mut reader = response.body_mut().as_reader();
        let mut buf = vec![0u8; 1 << 20];
        progress(Progress {
            file: file.name,
            done,
            total,
        });
        loop {
            let n = reader.read(&mut buf).map_err(|e| http(file.url, e))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            hasher.update(&buf[..n]);
            done += n as u64;
            progress(Progress {
                file: file.name,
                done,
                total,
            });
        }
        out.flush()?;
    }
    if done != file.bytes {
        return Err(http(
            file.url,
            format!("the download ended at {done} of {} bytes", file.bytes),
        ));
    }
    let got = hex(&hasher.finalize());
    if got != file.sha256 {
        return Err(Error::Hash {
            file: file.name.to_string(),
            expected: file.sha256.to_string(),
            got,
        });
    }
    Ok(())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::MODELS;

    #[test]
    fn paths_sit_under_the_model_id() {
        let store = Store::at("/tmp/x");
        let m = &MODELS[0];
        assert_eq!(store.dir(m), Path::new("/tmp/x").join(m.id));
        assert_eq!(
            store.path(m, &m.files[0]),
            Path::new("/tmp/x").join(m.id).join(m.files[0].name)
        );
        assert!(!store.have(m));
    }

    #[test]
    fn the_user_store_follows_xdg() {
        // Only the shape: the variable may or may not be set here.
        let s = Store::user().expect("a cache directory in the test environment");
        assert!(s.root().ends_with("greycard/models"));
    }

    /// A runtime laid out in the store as a fetch leaves it is had,
    /// and a second fetch finds it there and downloads nothing (its
    /// URL goes nowhere); a library changed on disk is not had, as
    /// its hash is checked again, not only its size.
    #[test]
    fn a_runtime_in_the_store_is_had_by_its_hash_and_not_fetched_again() {
        use crate::registry::{Member, Packing};
        let root = std::env::temp_dir().join(format!(
            "greycard-store-runtime-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let lib: &[u8] = b"a library's bytes";
        let license: &[u8] = b"MIT License\n\nCopyright (c) Microsoft Corporation\n";
        let sha =
            |b: &[u8]| -> &'static str { Box::leak(hex(&Sha256::digest(b)).into_boxed_str()) };
        let runtime: &'static Runtime = Box::leak(Box::new(Runtime {
            id: "test-runtime",
            notice: "MIT License",
            archive: File {
                name: "rt.tgz",
                // .invalid never resolves: refused at once, with no
                // socket left waiting on a timeout.
                url: "http://greycard-store-test.invalid/rt.tgz",
                bytes: 1,
                sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            },
            packing: Packing::Tgz,
            library: Member {
                path: "rt/lib/libx.so",
                name: crate::runtime::LIBRARY,
                bytes: lib.len() as u64,
                sha256: sha(lib),
            },
            kept: Box::leak(Box::new([Member {
                path: "rt/LICENSE",
                name: "LICENSE",
                bytes: license.len() as u64,
                sha256: sha(license),
            }])),
            ..crate::registry::ONNXRUNTIME_CPU_LINUX_X64
        }));
        let store = Store::at(&root);
        assert_eq!(store.runtime_dir(runtime), root.join("test-runtime"));
        assert_eq!(
            store.runtime_library(runtime),
            root.join("test-runtime").join(crate::runtime::LIBRARY)
        );
        assert!(!store.have_runtime(runtime));

        std::fs::create_dir_all(store.runtime_dir(runtime)).unwrap();
        std::fs::write(store.runtime_library(runtime), lib).unwrap();
        std::fs::write(store.runtime_dir(runtime).join("LICENSE"), license).unwrap();
        assert!(store.have_runtime(runtime));
        let path = store
            .fetch_runtime(runtime, |_| panic!("nothing to download"))
            .unwrap();
        assert_eq!(path, store.runtime_library(runtime));
        let note = std::fs::read_to_string(store.runtime_dir(runtime).join("LICENSE.txt")).unwrap();
        assert!(note.contains(runtime.library.sha256), "{note}");

        // The same size, other bytes: not had, and a fetch tries the
        // download (which goes nowhere here) rather than keep it.
        let mut changed = lib.to_vec();
        changed[0] ^= 1;
        std::fs::write(store.runtime_library(runtime), &changed).unwrap();
        assert!(!store.have_runtime(runtime));
        assert!(matches!(
            store.fetch_runtime(runtime, |_| {}),
            Err(Error::Http { .. })
        ));
        // The library whole again but the license gone: fetched again
        // too, so what goes with the library is put back.
        std::fs::write(store.runtime_library(runtime), lib).unwrap();
        std::fs::remove_file(store.runtime_dir(runtime).join("LICENSE")).unwrap();
        assert!(store.have_runtime(runtime));
        assert!(matches!(
            store.fetch_runtime(runtime, |_| {}),
            Err(Error::Http { .. })
        ));
        // A failed fetch leaves no archive behind, and what was there
        // stays.
        let names: Vec<String> = std::fs::read_dir(store.runtime_dir(runtime))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(!names.iter().any(|n| n.ends_with(".part")), "{names:?}");
        assert!(store.have_runtime(runtime));

        // Removed, it is gone, and removing it again is no error.
        store.remove_runtime(runtime).unwrap();
        assert!(!store.runtime_dir(runtime).exists());
        store.remove_runtime(runtime).unwrap();
        // A fetch that fails with nothing there leaves no directory
        // that would read as a runtime half had.
        assert!(store.fetch_runtime(runtime, |_| {}).is_err());
        assert!(!store.runtime_dir(runtime).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A small download that fails leaves no part file: under
    /// [`RESUMABLE`] a file is not resumed, and a part left would only
    /// mislead.
    #[test]
    fn a_failed_download_leaves_no_part_file() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-store-part-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("x.part");
        // There from an earlier run: removed with the failure too.
        std::fs::write(&part, b"half").unwrap();
        let file = File {
            name: "x",
            url: "http://greycard-store-test.invalid/x",
            bytes: 4,
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        };
        assert!(fetch_to(&file, &part, &mut |_| {}).is_err());
        assert!(!part.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A server on the loopback for one file's `body`: the first
    /// connection is answered with a 200 and dropped halfway through
    /// the body; after that a `Range: bytes=N-` is answered with a 206
    /// from N when `ranges`, and everything with the whole body in a
    /// 200 otherwise. Answers the file and the Range headers it saw.
    fn serve(
        body: &'static [u8],
        ranges: bool,
    ) -> (File, std::sync::Arc<std::sync::Mutex<Vec<Option<String>>>>) {
        use std::io::BufRead;
        use std::sync::{Arc, Mutex};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url: &'static str = Box::leak(
            format!("http://{}/model.bin", listener.local_addr().unwrap()).into_boxed_str(),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for (n, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { return };
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut range = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("range:") {
                        range = Some(v.trim().to_string());
                    }
                }
                log.lock().unwrap().push(range.clone());
                let from = range
                    .as_deref()
                    .filter(|_| ranges && n > 0)
                    .and_then(|r| r.strip_prefix("bytes="))
                    .and_then(|r| r.strip_suffix('-'))
                    .and_then(|r| r.parse::<usize>().ok());
                let len = body.len();
                let head = match from {
                    Some(f) => format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {f}-{}/{len}\r\nConnection: close\r\n\r\n",
                        len - f,
                        len - 1
                    ),
                    None => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
                    ),
                };
                let _ = stream.write_all(head.as_bytes());
                let rest = &body[from.unwrap_or(0)..];
                let rest = if n == 0 {
                    &rest[..rest.len() / 2]
                } else {
                    rest
                };
                let _ = stream.write_all(rest);
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        });
        let sha: &'static str = Box::leak(hex(&Sha256::digest(body)).into_boxed_str());
        let file = File {
            name: "model.bin",
            url,
            bytes: body.len() as u64,
            sha256: sha,
        };
        (file, seen)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-store-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn body() -> &'static [u8] {
        Box::leak(
            (0..200_000u32)
                .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
                .collect::<Vec<u8>>()
                .into_boxed_slice(),
        )
    }

    /// A large file whose download is dropped halfway keeps its part;
    /// the next fetch asks for the rest, appends it, and the whole is
    /// checked against the hash, the progress starting at what was
    /// kept.
    #[test]
    fn a_large_download_dropped_halfway_resumes_from_its_part() {
        let body = body();
        let (file, seen) = serve(body, true);
        let dir = scratch("resume");
        let part = dir.join("model.bin.part");
        let first = fetch_resuming(&file, &part, 1000, &mut |_| {});
        assert!(matches!(first, Err(Error::Http { .. })), "{first:?}");
        let kept = std::fs::metadata(&part).unwrap().len();
        assert_eq!(kept, body.len() as u64 / 2);
        let mut reports = Vec::new();
        fetch_resuming(&file, &part, 1000, &mut |p| reports.push((p.done, p.total))).unwrap();
        assert_eq!(std::fs::read(&part).unwrap(), body);
        assert_eq!(reports.first(), Some(&(kept, body.len() as u64)));
        assert_eq!(
            reports.last(),
            Some(&(body.len() as u64, body.len() as u64))
        );
        assert_eq!(
            *seen.lock().unwrap(),
            vec![None, Some(format!("bytes={kept}-"))]
        );
        // Under the size, the same failure keeps nothing.
        let (file, _) = serve(body, true);
        let part = dir.join("small.part");
        assert!(fetch_resuming(&file, &part, u64::MAX, &mut |_| {}).is_err());
        assert!(!part.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A server that answers a range with the whole file in a 200:
    /// the part is started over, not appended to, and the file still
    /// arrives whole.
    #[test]
    fn a_server_that_ignores_the_range_starts_the_file_over() {
        let body = body();
        let (file, seen) = serve(body, false);
        let dir = scratch("norange");
        let part = dir.join("model.bin.part");
        assert!(fetch_resuming(&file, &part, 1000, &mut |_| {}).is_err());
        let kept = std::fs::metadata(&part).unwrap().len();
        assert!(kept > 0);
        let mut reports = Vec::new();
        fetch_resuming(&file, &part, 1000, &mut |p| reports.push(p.done)).unwrap();
        assert_eq!(std::fs::read(&part).unwrap(), body);
        assert_eq!(reports.first(), Some(&0));
        assert_eq!(seen.lock().unwrap()[1], Some(format!("bytes={kept}-")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A kept part whose whole does not hash to the file's is removed,
    /// so the next fetch starts clean rather than append to it again.
    #[test]
    fn a_resumed_file_that_does_not_hash_is_removed() {
        let body = body();
        let (file, _) = serve(body, true);
        let dir = scratch("badpart");
        let part = dir.join("model.bin.part");
        assert!(fetch_resuming(&file, &part, 1000, &mut |_| {}).is_err());
        // The kept half, changed on disk.
        let mut half = std::fs::read(&part).unwrap();
        half[10] ^= 1;
        std::fs::write(&part, &half).unwrap();
        let resumed = fetch_resuming(&file, &part, 1000, &mut |_| {});
        assert!(matches!(resumed, Err(Error::Hash { .. })), "{resumed:?}");
        assert!(!part.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A retired file at its size and hash goes, with its directory
    /// and the license note beside it; one of the same name and size
    /// but other bytes stays, as does the directory of a model in use.
    #[test]
    fn the_sweep_removes_a_retired_file_and_only_that() {
        let root = std::env::temp_dir().join(format!(
            "greycard-store-sweep-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let bytes: &[u8] = b"an old model's weights";
        let sha =
            |b: &[u8]| -> &'static str { Box::leak(hex(&Sha256::digest(b)).into_boxed_str()) };
        let file = |name: &'static str| File {
            name,
            url: "http://greycard-store-test.invalid/old.onnx",
            bytes: bytes.len() as u64,
            sha256: sha(bytes),
        };
        let retired = [
            Retired {
                dir: "old-model",
                file: file("old.onnx"),
            },
            Retired {
                dir: "other-model",
                file: file("other.onnx"),
            },
        ];
        let old = root.join("old-model");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("old.onnx"), bytes).unwrap();
        std::fs::write(old.join("LICENSE.txt"), "a note").unwrap();
        // The same name and size, other bytes: not the file listed.
        let other = root.join("other-model");
        std::fs::create_dir_all(&other).unwrap();
        let changed: Vec<u8> = bytes.iter().map(|b| b ^ 1).collect();
        std::fs::write(other.join("other.onnx"), &changed).unwrap();
        let store = Store::at(&root);
        let removed = store.sweep(&retired);
        assert_eq!(removed, vec![(old.join("old.onnx"), bytes.len() as u64)]);
        assert!(!old.exists());
        assert_eq!(std::fs::read(other.join("other.onnx")).unwrap(), changed);
        // A second sweep finds nothing to do.
        assert!(store.sweep(&retired).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The Carve file is retired and is no model's in use.
    #[test]
    fn the_carve_fill_is_retired() {
        assert!(RETIRED.iter().any(|r| r.file.name == "lama_fp32.onnx"));
        for r in RETIRED {
            assert!(MODELS.iter().all(|m| m.id != r.dir), "{} is in use", r.dir);
        }
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
    }
}
