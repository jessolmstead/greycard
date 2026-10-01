//! What the panel remembers between runs.
//!
//! A small JSON file under the user's configuration directory holding
//! the export sheet's choices and its presets, the scope on show, the clipping
//! warnings, the soft proof's choices, the monitor's profile, the
//! last file open and the folders opened, by the same names the panel
//! uses for them. Read at startup, written when the window closes and
//! after an export; the last file is also written as soon as it
//! develops, and a folder as it opens, so they survive a session that
//! never closes cleanly. Trouble either way is ignored: a
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
    /// The left pane, the develop panel and the strip, each as put
    /// away (F7, F8, F6, Tab for all three) or not when the window
    /// closed.
    pub hide_left: bool,
    pub hide_right: bool,
    pub hide_strip: bool,
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
    /// Whether a rating, flag or label key in culling moves the
    /// selection on to the next frame. Off by default: the key and
    /// the arrow are two presses until a culler asks for one.
    pub cull_move_on: bool,
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
    /// The folders opened, the most recent first and at most
    /// [`RECENT_FOLDERS`] of them, each by its canonical path and
    /// once: a folder opened again moves to the front. Written as a
    /// folder opens, as `last_file` is. A folder gone from the disk
    /// stays until newer ones push it off the end; choosing it says
    /// it is gone rather than taking it out behind the user's back.
    pub recent_folders: Vec<String>,
    /// Whether a folder chosen in a root's tree in the left pane opens
    /// with the frames of the folders under it too, or only its own.
    /// Off by default: a folder's own frames are the cheap way into a
    /// large archive. Written as the switch is flipped.
    pub folder_tree_subfolders: bool,
    /// The most the thumbnail cache under the user's cache directory
    /// may hold, in megabytes; past it the least recently used
    /// pictures go. Zero turns the cache off.
    pub thumb_cache_mb: u64,
    /// The most the local previews (the culling loupe's pictures of the
    /// frames under the roots, kept beside the thumbnails) may hold, in
    /// megabytes, apart from the thumbnails' cap; past it the least
    /// recently used go. Zero keeps none.
    pub preview_cache_mb: u64,
    /// How often, in minutes, a library root on a network mount (NFS,
    /// SMB, sshfs), which is not watched, is passed over for what
    /// changed on it. Zero for never: the pass at launch and the list's
    /// reads as the index reports are then all it gets. A local root is
    /// watched and never passed over on a timer.
    pub network_poll_minutes: u64,
    /// The import sheet as it was last started: written when an
    /// import begins, not gathered when the window closes.
    pub import: ImportChoices,
    /// The browser's filter as it was left: its chips, the rows'
    /// reading and the text, put back on the next launch unless the
    /// command line names one. Clear empties it, and an empty one is
    /// what is kept then.
    pub filter: crate::filter::Saved,
    /// The check for a newer release: its switch, when it last reached
    /// GitHub and what it found, and a release the pane was told not
    /// to offer. Written as each changes, not gathered at the close.
    pub update: crate::update::Kept,
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
            hide_left: false,
            hide_right: false,
            hide_strip: false,
            xmp_sidecars: false,
            sidecars_in_folder: false,
            cull_move_on: false,
            lenses_declined: false,
            last_file: String::new(),
            recent_folders: Vec::new(),
            folder_tree_subfolders: false,
            thumb_cache_mb: greycard_library::thumbs::DEFAULT_CAP / (1024 * 1024),
            preview_cache_mb: greycard_library::thumbs::DEFAULT_PREVIEW_CAP / (1024 * 1024),
            network_poll_minutes: 10,
            import: ImportChoices::default(),
            filter: crate::filter::Saved::default(),
            update: crate::update::Kept::default(),
        }
    }
}

/// How many folders Recently opened keeps.
pub const RECENT_FOLDERS: usize = 10;

/// `folder` opened: put at the front of `list`, taken from wherever
/// it was further down, and the list cut to [`RECENT_FOLDERS`].
pub fn push_recent(list: &mut Vec<String>, folder: &str) {
    list.retain(|f| f != folder);
    list.insert(0, folder.to_string());
    list.truncate(RECENT_FOLDERS);
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
            hide_left: true,
            hide_right: false,
            hide_strip: true,
            xmp_sidecars: true,
            sidecars_in_folder: true,
            cull_move_on: true,
            lenses_declined: true,
            last_file: "/home/x/Pictures/IMG_0001.CR3".into(),
            recent_folders: vec!["/home/x/Pictures/b".into(), "/mnt/gone/a".into()],
            folder_tree_subfolders: true,
            thumb_cache_mb: 1024,
            preview_cache_mb: 20000,
            network_poll_minutes: 30,
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
            update: crate::update::Kept {
                check: false,
                checked_at: 1_790_000_000,
                tag: "v0.1.4".into(),
                url: "https://github.com/jessolmstead/greycard/releases/tag/v0.1.4".into(),
                dismissed: "v0.1.3".into(),
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
        // A file from before Recently opened has an empty list.
        assert!(read.recent_folders.is_empty());
        // And from before the folder tree, a folder's own frames.
        assert!(!read.folder_tree_subfolders);
        // And from before the previews' own cap, 8 GB of them.
        assert_eq!(read.preview_cache_mb, 8192);
        assert_eq!(read.thumb_cache_mb, 300);
        // And from before the network roots' timer, every ten minutes.
        assert_eq!(read.network_poll_minutes, 10);
        // A file from before the update check has it on.
        assert!(read.update.check);
        assert_eq!(read.update.checked_at, 0);
    }

    #[test]
    fn a_folder_opened_again_moves_to_the_front_and_the_list_keeps_ten() {
        let mut list = Vec::new();
        for i in 0..12 {
            push_recent(&mut list, &format!("/p/{i}"));
        }
        // The newest first, and the two oldest pushed off the end.
        assert_eq!(list.len(), RECENT_FOLDERS);
        assert_eq!(list[0], "/p/11");
        assert_eq!(list[9], "/p/2");
        // Opened again: moved up, not listed twice.
        push_recent(&mut list, "/p/5");
        assert_eq!(list.len(), RECENT_FOLDERS);
        assert_eq!(list[0], "/p/5");
        assert_eq!(list.iter().filter(|f| *f == "/p/5").count(), 1);
        assert_eq!(list[1], "/p/11");
        assert_eq!(list[9], "/p/2");
        // The one already at the front stays where it is.
        push_recent(&mut list, "/p/5");
        assert_eq!(list[0], "/p/5");
        assert_eq!(list[1], "/p/11");
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
            recent_folders: vec!["/home/x/Pictures/shoot".into()],
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
