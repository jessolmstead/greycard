//! What the editor opens and whether a sidecar holds a develop, by the
//! paths the window has always named them by: the listing is the
//! library's (`greycard_library::files`), the sidecar's side
//! edit-io's (`greycard_edit_io::on_disk`).

pub use greycard_edit_io::on_disk::line_never_developed;
pub use greycard_edit_io::on_disk::never_developed;
pub use greycard_library::files::{list, list_files, list_paths};
pub use greycard_library::tilde;

use std::path::{Path, PathBuf};

/// `path` with the first of `prefixes` it lies under swapped for that
/// prefix's name: `%LOCALAPPDATA%` or `~`, so what goes into a public
/// form does not carry the account's name.
pub fn abbreviate(path: &Path, prefixes: &[(Option<PathBuf>, &str)]) -> String {
    for (prefix, name) in prefixes {
        if let Some(rest) = prefix.as_ref().and_then(|p| path.strip_prefix(p).ok()) {
            return format!("{name}{}{}", std::path::MAIN_SEPARATOR, rest.display());
        }
    }
    path.display().to_string()
}

/// A path of ours as the form and the Settings sheet show it, the
/// home folder short, so neither carries the account's name.
pub fn path_shown(path: &Path) -> String {
    if cfg!(windows) {
        abbreviate(
            path,
            &[
                (dirs::data_local_dir(), "%LOCALAPPDATA%"),
                (dirs::config_dir(), "%APPDATA%"),
                (dirs::home_dir(), "%USERPROFILE%"),
            ],
        )
    } else {
        abbreviate(path, &[(dirs::home_dir(), "~")])
    }
}
