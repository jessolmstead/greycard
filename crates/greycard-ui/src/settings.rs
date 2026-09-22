//! What the panel remembers between runs.
//!
//! A small JSON file under the user's configuration directory holding
//! the export sheet's choices, the scope on show, the clipping
//! warnings, the soft proof's choices, the monitor's profile and the
//! last file open, by the same names the panel uses for them. Read at
//! startup, written when the window closes and after an export; the
//! last file is also written as soon as it develops, so it survives a
//! session that never closes cleanly. Trouble either way is ignored: a
//! preference is not worth an error, and the defaults are good.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub export_format: String,
    pub export_quality: f32,
    pub export_size: String,
    /// The long edge typed for a custom size, as typed.
    pub export_custom: String,
    pub export_space: String,
    pub export_embed: bool,
    pub export_sharpen: String,
    /// What an export does when a file of that name is already there:
    /// Increment, Overwrite or Skip.
    pub export_on_exists: String,
    pub scope: String,
    /// Which curve the CURVES section shows: "Parametric" or "Point".
    pub curve_mode: String,
    pub warn_shadows: bool,
    pub warn_highlights: bool,
    /// The proof's profile, a space's name or a file's path, and its
    /// intent; whether the proof is on is not kept, as a proof is a
    /// look at one export.
    pub proof_profile: String,
    pub proof_intent: String,
    pub gamut_warning: bool,
    /// The monitor's profile: "System" for colord's, a standard's
    /// name, or a file's path; and under "System", which of colord's
    /// monitors, by model, the primary when empty.
    pub display_profile: String,
    pub display_monitor: String,
    /// The viewport's surround, by the name `render::canvas_name`
    /// gives it: "Black", or one of three greys.
    pub canvas_color: String,
    /// The panel's sections folded away, by the names the panel
    /// keeps them under.
    pub collapsed: Vec<String>,
    /// The grid's cell, logical pixels: the zoom it was left at.
    pub grid_cell: f32,
    /// Whether the meta (the rating, the flag, the label, the
    /// keywords and the words) is also written to an `.xmp` beside
    /// the frame, for Lightroom, Bridge and darktable to read. Off
    /// by default: the `.gcd` is the truth and one file beside a
    /// frame is the rule (§117), so a second one is for a folder
    /// shared with another tool and is asked for. Reading an XMP
    /// that is there is not a choice and happens either way.
    pub xmp_sidecars: bool,
    /// The absolute path of the last file open, developed and all, so
    /// the next run with no path can jump back to it. Written as soon
    /// as it develops, not waited for the window to close; empty
    /// until then.
    pub last_file: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            export_format: "JPEG".into(),
            export_quality: 92.0,
            export_size: "Full".into(),
            export_custom: "1600".into(),
            export_space: "sRGB".into(),
            export_embed: true,
            export_sharpen: crate::export::Sharpen::default().name().into(),
            export_on_exists: crate::export::OnExists::default().name().into(),
            scope: crate::scope::Scope::default().name().into(),
            curve_mode: "Point".into(),
            warn_shadows: false,
            warn_highlights: false,
            proof_profile: crate::export::Space::Srgb.name().into(),
            proof_intent: crate::display::ProofIntent::default().name().into(),
            gamut_warning: true,
            display_profile: "System".into(),
            display_monitor: String::new(),
            canvas_color: crate::render::CANVAS_NAMES[0].into(),
            collapsed: Vec::new(),
            grid_cell: crate::grid::CELL,
            xmp_sidecars: false,
            last_file: String::new(),
        }
    }
}

/// `$XDG_CONFIG_HOME/greycard/settings.json` when that is set, else
/// `greycard/settings.json` under the platform's configuration
/// directory: `~/.config` on Linux, `~/Library/Application Support` on
/// macOS, `%APPDATA%` on Windows.
pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::config_dir)?;
    Some(base.join("greycard").join("settings.json"))
}

impl Settings {
    /// What was left last time, or the defaults.
    pub fn load() -> Self {
        path().map(|p| Self::load_from(&p)).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(path) = path() {
            self.save_to(&path);
        }
    }

    /// The file, or the defaults: a file that is not there is the
    /// first run, and says nothing; one that will not parse is worth
    /// a warning, since the defaults then appear from nowhere.
    pub fn load_from(path: &std::path::Path) -> Self {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!("settings {}: {e}; defaults", path.display());
                return Self::default();
            }
        };
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!("settings {}: {e}; defaults", path.display());
            Self::default()
        })
    }

    pub fn save_to(&self, path: &std::path::Path) {
        let text = match serde_json::to_string_pretty(self) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("settings not saved: {e}");
                return;
            }
        };
        if let Some(dir) = path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            tracing::warn!("settings directory {}: {e}", dir.display());
            return;
        }
        if let Err(e) = std::fs::write(path, text) {
            tracing::warn!("settings not saved to {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_round_trip() {
        let mine = Settings {
            export_format: "PNG".into(),
            export_quality: 60.0,
            export_size: "2048".into(),
            export_custom: "3000".into(),
            export_space: "Display P3".into(),
            export_embed: false,
            export_sharpen: "High".into(),
            export_on_exists: "Skip".into(),
            scope: "Vector".into(),
            curve_mode: "Parametric".into(),
            warn_shadows: true,
            warn_highlights: false,
            proof_profile: "/tmp/paper.icc".into(),
            proof_intent: "Relative".into(),
            gamut_warning: false,
            display_profile: "Adobe RGB".into(),
            display_monitor: "PA279CRV".into(),
            canvas_color: "Mid grey".into(),
            collapsed: vec!["grain".into(), "demosaic".into()],
            grid_cell: 256.0,
            xmp_sidecars: true,
            last_file: "/home/x/Pictures/IMG_0001.CR3".into(),
        };
        let text = serde_json::to_string(&mine).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), mine);
    }

    #[test]
    fn a_file_from_another_version_keeps_what_it_knows() {
        // A field this version has dropped, and ones it has not
        // written yet: neither is a reason to lose the rest.
        let text = r#"{"export_quality": 60, "grain_seed": 7}"#;
        let read: Settings = serde_json::from_str(text).unwrap();
        assert_eq!(read.export_quality, 60.0);
        assert_eq!(read.export_format, Settings::default().export_format);
        assert_eq!(read.scope, Settings::default().scope);
    }

    #[test]
    fn what_is_written_is_what_comes_back() {
        let dir = std::env::temp_dir().join(format!("greycard-settings-{}", std::process::id()));
        let path = dir.join("greycard").join("settings.json");
        let mine = Settings {
            export_size: "1024".into(),
            scope: "Parade".into(),
            ..Settings::default()
        };
        mine.save_to(&path);
        assert_eq!(Settings::load_from(&path), mine);
        // A file that is not there is the defaults, not an error.
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Settings::load_from(&path), Settings::default());
    }

    #[test]
    fn the_path_is_under_the_configuration_directory() {
        let path = path().expect("a configuration directory in the test environment");
        assert!(path.ends_with("greycard/settings.json"), "{path:?}");
        assert!(path.is_absolute());
    }
}
