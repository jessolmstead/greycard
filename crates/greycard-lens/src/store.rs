//! Where the database lives and how it arrives: the user's cache, a
//! system copy when lensfun is installed, and a download of the
//! database's own tarball on first use.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::db::Database;

/// The database's version, which is the tarball's name and the
/// directory the files keep.
pub const VERSION: u32 = 2;

/// Where the tarball is published, in the order tried: the database's
/// own site, and its maintainer's mirror.
pub const SOURCES: &[&str] = &[
    "https://lensfun.github.io/db/",
    "https://wilson.bronger.org/lensfun-db/",
];

/// The database's license.
pub const LICENSE: (&str, &str) = (
    "CC BY-SA 3.0",
    "https://creativecommons.org/licenses/by-sa/3.0/",
);

/// About how large the tarball is, for a sheet to say.
pub const APPROX_BYTES: u64 = 450_000;

/// Download progress.
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub done: u64,
    /// Zero when the server did not say.
    pub total: u64,
}

/// The database's places on this machine.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// The user's cache: `$XDG_CACHE_HOME/greycard/lensfun` when that
    /// is set, else `greycard/lensfun` under the platform's cache
    /// directory: `~/.cache` on Linux, `~/Library/Caches` on macOS,
    /// `%LOCALAPPDATA%` on Windows.
    pub fn user() -> Result<Self, Error> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(dirs::cache_dir)
            .ok_or(Error::NoCacheDir)?;
        Ok(Self::at(base.join("greycard").join("lensfun")))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory the fetched files keep.
    pub fn dir(&self) -> PathBuf {
        self.root.join(format!("version_{VERSION}"))
    }

    /// The directories a database may be read from, in the order
    /// tried: the fetched copy, then lensfun's own places on a system
    /// that has it.
    pub fn candidates(&self) -> Vec<PathBuf> {
        let mut out = vec![self.dir()];
        for sys in [
            "/var/lib/lensfun-updates",
            "/usr/share/lensfun",
            "/usr/local/share/lensfun",
        ] {
            out.push(Path::new(sys).join(format!("version_{VERSION}")));
        }
        out
    }

    /// Whether a fetched copy is here.
    pub fn have(&self) -> bool {
        self.dir().join("timestamp.txt").is_file()
    }

    /// The first database that reads, and where it came from.
    pub fn load(&self) -> Option<(Database, PathBuf)> {
        let found = self
            .candidates()
            .into_iter()
            .find_map(|dir| match Database::load_dir(&dir) {
                Ok(db) => Some((db, dir)),
                Err(e) => {
                    log::debug!("no lens database in {}: {e}", dir.display());
                    None
                }
            });
        match &found {
            Some((db, dir)) => log::info!(
                "lens database {}: {} cameras, {} lenses",
                dir.display(),
                db.cameras.len(),
                db.lenses.len()
            ),
            // The callers say so in their own words: the editor's
            // panel offers the fetch, the CLI says to run with it.
            None => log::debug!("no lens database in any of the places tried"),
        }
        found
    }

    /// Fetch the tarball from the first source that answers and
    /// unpack its XML files into [`Store::dir`], with a note of the
    /// license beside them.
    pub fn fetch(&self, mut progress: impl FnMut(Progress)) -> Result<(), Error> {
        let name = format!("version_{VERSION}.tar.bz2");
        let mut last = None;
        for base in SOURCES {
            let url = format!("{base}{name}");
            match fetch_bytes(&url, &mut progress) {
                Ok(bytes) => {
                    log::info!("lens database fetched from {url}");
                    return self.unpack(&bytes, &url);
                }
                Err(e) => {
                    log::warn!("lens database from {url}: {e}");
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or(Error::NoSource))
    }

    fn unpack(&self, bytes: &[u8], url: &str) -> Result<(), Error> {
        let dir = self.dir();
        let part = self.root.join(format!("version_{VERSION}.part"));
        let _ = std::fs::remove_dir_all(&part);
        std::fs::create_dir_all(&part)?;
        let decoder = bzip2::read::MultiBzDecoder::new(bytes);
        let mut archive = tar::Archive::new(decoder);
        let mut files = 0;
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.into_owned();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.ends_with(".xml") || !entry.header().entry_type().is_file() {
                continue;
            }
            let mut text = Vec::new();
            entry.read_to_end(&mut text)?;
            std::fs::write(part.join(name), text)?;
            files += 1;
        }
        if files == 0 {
            let _ = std::fs::remove_dir_all(&part);
            return Err(Error::Empty(part));
        }
        std::fs::write(
            part.join("timestamp.txt"),
            format!(
                "{}\n",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            ),
        )?;
        std::fs::write(part.join("LICENSE.txt"), license_note(url))?;
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::rename(&part, &dir)?;
        Ok(())
    }
}

/// The note written beside the files.
pub fn license_note(url: &str) -> String {
    format!(
        "The lensfun lens database\n\nLicence: {} <{}>\nSource: {url}\nProject: https://lensfun.github.io/\n\nDownloaded by greycard on first use. greycard does not redistribute these files.\n",
        LICENSE.0, LICENSE.1
    )
}

fn fetch_bytes(url: &str, progress: &mut impl FnMut(Progress)) -> Result<Vec<u8>, Error> {
    let http = |e: String| Error::Http {
        url: url.to_string(),
        message: e,
    };
    let mut response = ureq::get(url).call().map_err(|e| http(e.to_string()))?;
    let total = response.body().content_length().unwrap_or(0);
    let mut reader = response.body_mut().as_reader();
    let mut out = Vec::with_capacity(total as usize);
    let mut buf = vec![0u8; 1 << 16];
    progress(Progress { done: 0, total });
    loop {
        let n = reader.read(&mut buf).map_err(|e| http(e.to_string()))?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        progress(Progress {
            done: out.len() as u64,
            total,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_is_under_the_cache_and_a_tarball_unpacks() {
        let s = Store::user().expect("a cache directory in the test environment");
        assert!(s.root().ends_with("greycard/lensfun"));
        assert!(s.dir().ends_with("version_2"));
        assert_eq!(s.candidates().len(), 4);
        // A tarball made here: one XML file, one other, unpacked into
        // the versioned directory with its notes.
        let dir = std::env::temp_dir().join(format!("greycard-lens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::at(&dir);
        assert!(!store.have());
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(crate::db::SAMPLE.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            builder
                .append_data(
                    &mut header,
                    "version_2/sample.xml",
                    crate::db::SAMPLE.as_bytes(),
                )
                .unwrap();
            let mut header = tar::Header::new_gnu();
            header.set_size(3);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            builder
                .append_data(&mut header, "version_2/other.dtd", &b"abc"[..])
                .unwrap();
            builder.finish().unwrap();
        }
        let mut bz = Vec::new();
        {
            use std::io::Write;
            let mut enc = bzip2::write::BzEncoder::new(&mut bz, bzip2::Compression::fast());
            enc.write_all(&tar_bytes).unwrap();
            enc.finish().unwrap();
        }
        store.unpack(&bz, "test://tarball").unwrap();
        assert!(store.have());
        assert!(store.dir().join("sample.xml").is_file());
        assert!(!store.dir().join("other.dtd").exists());
        assert!(
            std::fs::read_to_string(store.dir().join("LICENSE.txt"))
                .unwrap()
                .contains("CC BY-SA 3.0")
        );
        let (db, from) = store.load().unwrap();
        assert_eq!(from, store.dir());
        assert_eq!(db.lenses.len(), 6);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
