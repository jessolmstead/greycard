//! Where model files live and how they arrive: a cache directory, a
//! download with a progress callback, and a hash check before the
//! file is trusted.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::registry::{File, Model};

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
}

fn fetch_to(file: &File, part: &Path, progress: &mut impl FnMut(Progress)) -> Result<()> {
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
        let _ = std::fs::remove_file(part);
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

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
    }
}
