//! What happens when a file is written where one is already: the
//! policy, and the name it picks.
//!
//! Every path the editor and the command line write to comes through
//! [`OnExists::resolve`], so nothing is ever replaced without either
//! the user's word for it or a line saying so.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// What to do when the file asked for is already there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnExists {
    /// Write beside it under the first free ` (2)`, ` (3)`... name.
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

/// The first free ` (2)`, ` (3)`... beside `path`. A name that ends in
/// a number of its own counts from there, so a second export of
/// `a (2).jpg` is `a (3).jpg` and not `a (2) (2).jpg`.
pub fn free_name(path: &Path) -> Option<PathBuf> {
    let stem = base_stem(path.file_stem()?);
    let extension = path.extension();
    let parent = path.parent();
    for n in 2..=9999u32 {
        let mut name = stem.clone();
        name.push(format!(" ({n})"));
        if let Some(e) = extension {
            name.push(".");
            name.push(e);
        }
        let candidate = match parent {
            Some(dir) => dir.join(&name),
            None => PathBuf::from(&name),
        };
        if !candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// A stem without the ` (2)` an earlier increment gave it.
fn base_stem(stem: &OsStr) -> OsString {
    let Some(text) = stem.to_str() else {
        return stem.to_os_string();
    };
    OsString::from(trim_number(text).unwrap_or(text))
}

/// `name (2)` without its number; none when it has none.
fn trim_number(text: &str) -> Option<&str> {
    let (head, number) = text.strip_suffix(')')?.rsplit_once(" (")?;
    if head.is_empty() || number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(head)
}
