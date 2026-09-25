//! What the panel remembers between runs.
//!
//! A small JSON file under the user's configuration directory holding
//! the export sheet's choices and its presets, the scope on show, the clipping
//! warnings, the soft proof's choices, the monitor's profile and the
//! last file open, by the same names the panel uses for them. Read at
//! startup, written when the window closes and after an export; the
//! last file is also written as soon as it develops, so it survives a
//! session that never closes cleanly. Trouble either way is ignored: a
//! preference is not worth an error, and the defaults are good.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::sheet::{ExportPreset, Sheet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The export sheet as it was left, under the `export_` names it
    /// has always had in the file.
    #[serde(flatten)]
    pub export: Sheet,
    /// The export presets, in the order they were first saved.
    pub export_presets: Vec<ExportPreset>,
    /// The preset last chosen or saved, empty for none; the sheet
    /// above may have been edited since.
    pub export_preset: String,
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
    /// Whether a frame's `.gcd` is written under a hidden `.greycard`
    /// folder in the frame's folder rather than beside the frame.
    /// Off by default. Reading looks in both places either way, so
    /// flipping this loses nothing; only where the next write goes
    /// changes. The XMPs stay beside the frame whatever this says,
    /// since beside is where Lightroom and darktable look.
    pub sidecars_in_folder: bool,
    /// Whether the lens profiles' download, offered unprompted the
    /// first time a picture opens with no database on the machine,
    /// was answered Not now. It is asked once: from then on the LENS
    /// section's own button is the way to them.
    pub lenses_declined: bool,
    /// The absolute path of the last file open, developed and all, so
    /// the next run with no path can jump back to it. Written as soon
    /// as it develops, not waited for the window to close; empty
    /// until then.
    pub last_file: String,
    /// The most the thumbnail cache under the user's cache directory
    /// may hold, in megabytes; past it the least recently used
    /// pictures go. Zero turns the cache off.
    pub thumb_cache_mb: u64,
    /// The import sheet as it was last started: written when an
    /// import begins, not gathered when the window closes.
    pub import: ImportChoices,
    /// The browser's filter as it was left: its chips, the rows'
    /// reading and the text, put back on the next launch unless the
    /// command line names one. Clear empties it, and an empty one is
    /// what is kept then.
    pub filter: crate::filter::Saved,
}

/// What the import sheet keeps for next time. The source is not
/// among them: a card is looked for afresh each time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportChoices {
    pub destination: String,
    pub subfolder: String,
    pub name: String,
    /// Empty for none.
    pub backup: String,
    /// The develop preset by name, empty for none.
    pub preset: String,
}

impl Default for ImportChoices {
    fn default() -> Self {
        Self {
            destination: String::new(),
            subfolder: "{date}".into(),
            name: "{name}".into(),
            backup: String::new(),
            preset: String::new(),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            export: Sheet::default(),
            export_presets: Vec::new(),
            export_preset: String::new(),
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
            sidecars_in_folder: false,
            lenses_declined: false,
            last_file: String::new(),
            thumb_cache_mb: greycard_library::thumbs::DEFAULT_CAP / (1024 * 1024),
            import: ImportChoices::default(),
            filter: crate::filter::Saved::default(),
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
        let sheet = Sheet {
            format: "PNG".into(),
            quality: 60.0,
            size: "2048".into(),
            custom: "3000".into(),
            space: "Display P3".into(),
            embed: false,
            sharpen: "High".into(),
            on_exists: "Skip".into(),
            metadata: "None".into(),
            mark: "Image".into(),
            mark_text: "© x".into(),
            mark_image: "/home/x/logo.png".into(),
            mark_color: "Black".into(),
            mark_position: "Top left".into(),
            mark_size: 12.5,
            mark_margin: 3.0,
            mark_opacity: 40.0,
        };
        let mine = Settings {
            export: sheet.clone(),
            export_presets: vec![
                ExportPreset {
                    name: "Web".into(),
                    sheet: Sheet {
                        mark: "Text".into(),
                        ..sheet.clone()
                    },
                },
                ExportPreset {
                    name: "Print".into(),
                    sheet: Sheet::default(),
                },
            ],
            export_preset: "Web".into(),
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
            sidecars_in_folder: true,
            lenses_declined: true,
            last_file: "/home/x/Pictures/IMG_0001.CR3".into(),
            thumb_cache_mb: 1024,
            import: ImportChoices {
                destination: "/home/x/Pictures".into(),
                subfolder: "{yyyy}/{date}".into(),
                name: "{date}-{seq}".into(),
                backup: "/mnt/backup".into(),
                preset: "Faded film".into(),
            },
            filter: crate::filter::Saved {
                stars: 3,
                exact: true,
                flags: vec![greycard_edit::meta::Flag::Pick],
                labels: vec![greycard_edit::meta::Label::Red],
                text: "harbor iso>=3200".into(),
                facets: [("camera".to_string(), vec!["Canon EOS R6m2".to_string()])]
                    .into_iter()
                    .collect(),
            },
        };
        let text = serde_json::to_string(&mine).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), mine);
    }

    #[test]
    fn a_file_from_another_version_keeps_what_it_knows() {
        // A field this version has dropped, and ones it has not
        // written yet: neither is a reason to lose the rest.
        let text = r#"{"export_quality": 60, "export_size": "1024", "grain_seed": 7}"#;
        let read: Settings = serde_json::from_str(text).unwrap();
        assert_eq!(read.export.quality, 60.0);
        assert_eq!(read.export.size, "1024");
        assert_eq!(read.export.format, Settings::default().export.format);
        assert_eq!(read.export.mark, Settings::default().export.mark);
        assert!(read.export_presets.is_empty());
        assert_eq!(read.scope, Settings::default().scope);
    }

    #[test]
    fn what_is_written_is_what_comes_back() {
        let dir = std::env::temp_dir().join(format!("greycard-settings-{}", std::process::id()));
        let path = dir.join("greycard").join("settings.json");
        let mine = Settings {
            export: Sheet {
                size: "1024".into(),
                ..Sheet::default()
            },
            export_presets: vec![ExportPreset {
                name: "Web 2048".into(),
                sheet: Sheet {
                    size: "2048".into(),
                    mark: "Text".into(),
                    mark_text: "© greycard".into(),
                    ..Sheet::default()
                },
            }],
            export_preset: "Web 2048".into(),
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
