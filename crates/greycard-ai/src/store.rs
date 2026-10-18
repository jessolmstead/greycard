//! Where model files live and how they arrive: a cache directory, a
//! download with a progress callback, and a hash check before the
//! file is trusted.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::registry::{File, Model, Runtime};

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
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(dirs::cache_dir)
            .ok_or(Error::NoCacheDir)?;
        Ok(Self::at(base.join("greycard").join("models")))
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
    /// every chunk.
    pub fn fetch(&self, model: &Model, mut progress: impl FnMut(Progress)) -> Result<()> {
        let dir = self.dir(model);
        std::fs::create_dir_all(&dir)?;
        for file in model.files {
            let path = self.path(model, file);
            if std::fs::metadata(&path).is_ok_and(|m| m.len() == file.bytes) {
                continue;
            }
            let part = path.with_extension("part");
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

/// Download `file` to `part`, checking its hash. On any failure (the
/// connection, a write, the hash) `part` is removed: a download is not
/// resumed, and a file left half there would only mislead.
fn fetch_to(file: &File, part: &Path, progress: &mut impl FnMut(Progress)) -> Result<()> {
    let result = download(file, part, progress);
    if result.is_err() {
        let _ = std::fs::remove_file(part);
    }
    result
}

fn download(file: &File, part: &Path, progress: &mut impl FnMut(Progress)) -> Result<()> {
    let mut response = ureq::get(file.url).call().map_err(|e| Error::Http {
        url: file.url.to_string(),
        message: e.to_string(),
    })?;
    let total = response.body().content_length().unwrap_or(file.bytes);
    let mut reader = response.body_mut().as_reader();
    let mut out = std::fs::File::create(part)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    progress(Progress {
        file: file.name,
        done,
        total,
    });
    loop {
        let n = reader.read(&mut buf).map_err(|e| Error::Http {
            url: file.url.to_string(),
            message: e.to_string(),
        })?;
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
    drop(out);
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

    /// A download that fails partway leaves no part file: the archive
    /// or model file is not resumed, and a part left would only
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

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
    }
}
