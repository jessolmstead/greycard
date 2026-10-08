//! What happens when a file is written where one is already: the
//! policy, and the name it picks.
//!
//! Every path the editor and the command line write to comes through
//! [`OnExists::resolve`], so nothing is ever replaced without either
//! the user's word for it or a line saying so.

use std::path::{Path, PathBuf};

/// What to do when the file asked for is already there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnExists {
    /// Write beside it under the first free `-1`, `-2`... name.
    #[default]
    Increment,
    /// Write over it.
    Overwrite,
    /// Leave it and write nothing.
    Skip,
}

impl OnExists {
    pub const ALL: [OnExists; 3] = [OnExists::Increment, OnExists::Overwrite, OnExists::Skip];

    /// The name the panel shows and the settings file keeps.
    pub fn name(self) -> &'static str {
        match self {
            OnExists::Increment => "Increment",
            OnExists::Overwrite => "Overwrite",
            OnExists::Skip => "Skip",
        }
    }

    /// A name from the panel, the settings file or the command line;
    /// how it is capitalized is no matter.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|p| p.name().eq_ignore_ascii_case(name.trim()))
    }

    /// Where a write to `path` should go under this policy, and what
    /// that says about what was there.
    pub fn resolve(self, path: &Path) -> Resolved {
        if !path.exists() {
            return Resolved::Free(path.to_path_buf());
        }
        match self {
            OnExists::Overwrite => Resolved::Replaced(path.to_path_buf()),
            OnExists::Skip => Resolved::Skipped(path.to_path_buf()),
            // No free name in ten thousand is not a reason to write
            // over the picture that is there; it is a reason to say so.
            OnExists::Increment => match free_name(path) {
                Some(free) => Resolved::Renamed {
                    path: free,
                    asked: path.to_path_buf(),
                },
                None => Resolved::Skipped(path.to_path_buf()),
            },
        }
    }
}

/// What the policy decided for one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Nothing was there: the path as asked for.
    Free(PathBuf),
    /// Something was there: this name beside it instead.
    Renamed { path: PathBuf, asked: PathBuf },
    /// Something was there: written over.
    Replaced(PathBuf),
    /// Something was there: nothing written.
    Skipped(PathBuf),
}

impl Resolved {
    /// Where to write, or none when nothing is to be written.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Resolved::Free(p) | Resolved::Renamed { path: p, .. } | Resolved::Replaced(p) => {
                Some(p)
            }
            Resolved::Skipped(_) => None,
        }
    }

    /// The path the write was asked for, whatever came of it.
    pub fn asked(&self) -> &Path {
        match self {
            Resolved::Free(p)
            | Resolved::Renamed { asked: p, .. }
            | Resolved::Replaced(p)
            | Resolved::Skipped(p) => p,
        }
    }

    /// What became of the file that was there, in words, for a status
    /// line or a terminal; none when nothing was there to say.
    pub fn note(&self) -> Option<String> {
        let name = |p: &Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string())
        };
        match self {
            Resolved::Free(_) => None,
            Resolved::Renamed { path, asked } => Some(format!(
                "{} was there already: wrote {}",
                name(asked),
                name(path)
            )),
            Resolved::Replaced(p) => Some(format!("wrote over {}", name(p))),
            Resolved::Skipped(p) => Some(format!("skipped {}: it is there already", name(p))),
        }
    }
}

/// The first free `-1`, `-2`... beside `path`, before its extension:
/// `photo.jpg` is `photo-1.jpg`, then `photo-2.jpg`. The name is taken
/// as it is: a stem that ends in a dash and a number (`DSC-0042`,
/// `photo-1`) is never read as a counter, since the next number could
/// be another frame's real name; `DSC-0042.jpg` is `DSC-0042-1.jpg`.
pub fn free_name(path: &Path) -> Option<PathBuf> {
    path.file_stem()?;
    (1..=9999u32)
        .map(|n| numbered(path, n))
        .find(|candidate| !candidate.exists())
}

/// The longest a file name goes on the file systems we write to, in
/// bytes.
pub const NAME_MAX: usize = 255;

/// `path` with `-n` before its extension. A stem too long to take the
/// suffix within [`NAME_MAX`] bytes loses the end of itself, on a
/// character's edge, so the name still writes.
pub fn numbered(path: &Path, n: u32) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default();
    let suffix = format!("-{n}");
    let tail = path.extension().map_or(0, |e| e.len() + 1);
    let mut name = match stem.to_str() {
        Some(text) => {
            let room = NAME_MAX.saturating_sub(suffix.len() + tail).max(1);
            let mut cut = room.min(text.len());
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            std::ffi::OsString::from(&text[..cut])
        }
        None => stem.to_os_string(),
    };
    name.push(suffix);
    if let Some(e) = path.extension() {
        name.push(".");
        name.push(e);
    }
    path.with_file_name(name)
}
