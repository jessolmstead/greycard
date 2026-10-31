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
    /// The folder last chosen in an export's chooser, where the next
    /// chooser opens. Not part of the sheet or a preset: a
    /// preset that names a subfolder never asks, and one that asks
    /// opens where the last export went. Empty for none.
    pub export_last_folder: String,
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
    /// The monitor's profile: "System" for the color manager's
    /// (colord, ColorSync), a standard's name, or a file's path; and
    /// under "System", which of its monitors, by model, the primary
    /// when empty.
    pub display_profile: String,
    pub display_monitor: String,
    /// The viewport's surround, by the name `render::canvas_name`
    /// gives it: "Black", or one of three greys.
    pub canvas_color: String,
    /// Whether the fitted picture has a mat, a band of the canvas
    /// around it. Off by default.
    pub mat: bool,
    /// The panel's sections folded away, by the names the panel
    /// keeps them under.
    pub collapsed: Vec<String>,
    /// Whether opening one of the develop panel's sections folds the
    /// others of its tab. Off by default. Written as the switch is
    /// flipped.
    pub one_section: bool,
    /// The grid's cell, logical pixels: the zoom it was left at.
    pub grid_cell: f32,
    /// Whether Ctrl and the wheel size the grid's cells. On by
    /// default; off, the wheel only scrolls, Ctrl or not, and the plus
    /// and minus keys still size them. Written as the switch is
    /// flipped.
    #[serde(default = "on", deserialize_with = "bool_on_leniently")]
    pub grid_wheel: bool,
    /// How many tiles go across the lightbox, as it was left. Held to
    /// the lightbox's own range when read.
    pub lightbox_across: i32,
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
    /// Whether a camera's JPEG beside a raw of its name is listed as a
    /// frame of its own, as Lightroom's "treat JPEG files next to raw
    /// files as separate photos" lists it. Off by default: the JPEG is
    /// the raw's companion, marked on its tile, and goes where the raw
    /// goes (`greycard_library::pairs`). The command line's listing and
    /// `--export` read it too. Written as the switch is flipped.
    pub jpegs_apart: bool,
    /// Whether a rating, flag or label key in culling moves the
    /// selection on to the next frame. Off by default: the key and
    /// the arrow are two presses until a culler asks for one.
    pub cull_move_on: bool,
    /// The Loupe's mode the last run left it in: Cull (true) or
    /// Develop. Library has no mode of its own. Written as it changes.
    pub loupe_cull: bool,
    /// Whether Cull shows the developed picture rather than the
    /// camera's JPEG. Written as the switch is flipped.
    pub cull_developed: bool,
    /// Whether the lens profiles' download, offered unprompted the
    /// first time a picture opens with no database on the machine,
    /// was answered Not now. It is asked once: from then on the LENS
    /// section's own button is the way to them.
    pub lenses_declined: bool,
    /// The folders whose RawTherapee profiles were offered as they
    /// opened and answered, by Not now, or by a write that brought a
    /// frame in, each by its canonical path: never offered again. Kept
    /// here rather than in the folder, where a `.greycard` made to hold
    /// it would take the folder's new sidecars from then on. Written as
    /// the offer is answered.
    pub rawtherapee_answered: Vec<String>,
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
    /// The folder a launch with no path opens, as chosen in the
    /// Settings sheet; empty for the last folder, the last file's, which
    /// is what every earlier build opened. Taken as written, with no `~`
    /// made a home. One that is gone or empty opens the last folder
    /// instead; one that holds only folders opens as the folder tree
    /// opens it, when it is a root or under one ([`start_listing`]).
    /// Written as it is chosen.
    #[serde(deserialize_with = "string_leniently")]
    pub start_folder: String,
    /// Whether the filter bar is shown, over the grid and in culling's
    /// section. On by default; off, the bar's room goes to the frames,
    /// and a filter set before still applies. Written as the switch
    /// is flipped.
    #[serde(default = "on", deserialize_with = "bool_on_leniently")]
    pub filter_bar: bool,
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
    /// The grid's order, a choice for each kind of view and each
    /// collection (`sort::Kept`): written as it is chosen, not gathered
    /// at the close. A file without it is every view in its default.
    pub sort: crate::sort::Kept,
    /// The check for a newer release: its switch, when it last reached
    /// GitHub and what it found, and a release the pane was told not
    /// to offer. Written as each changes, not gathered at the close.
    pub update: crate::update::Kept,
    /// The groups the camera match's sheet was left with unchecked,
    /// by the scope it was opened on, the most recent scope first.
    /// Written as a box is ticked. A group not named here, seen for
    /// the first time, is checked.
    pub match_unchecked: Vec<MatchChoice>,
    /// Which of the sync sheet's Temperature, Tint and Exposure shift
    /// each frame by the frame synced from's own move rather than set
    /// it to that frame's value, as the sheet was left.
    pub sync_shift: SyncShift,
    /// The aspect the crop opens under on a frame not cropped yet, by
    /// the Settings sheet's word for it (`panel::crop::Start`):
    /// "Original", "Last used" or "Free". Written as it is chosen.
    pub crop_start: String,
    /// The aspect last picked from the crop's list, as
    /// `panel::crop::aspect_key` spells it ("3:2", "Custom 7:5"):
    /// what "Last used" opens under. Written as it is picked.
    pub crop_last: String,
    /// What a Fill drawn next is made by, "Patch", "Fast" or "Best"
    /// (`FillChoice`), as last chosen in the panel or the Settings
    /// sheet; empty until one is, for the machine's default.
    pub fill_choice: String,
    /// The interface's scale, percent of the system's own: 50 to 300
    /// (`window_fit::scale`), clamped when read. Applied at the next
    /// start; written as it is typed.
    pub ui_scale: u32,
    /// The system's own scale the window last opened under, as winit
    /// reported it, written at the close: the next start's guess of
    /// it, which the scale multiplies. Zero for none seen.
    pub ui_scale_system: f64,
    /// The keys rebound from the Settings sheet, each action's name
    /// (`keys::TABLE`) to its chord ("crop": "Ctrl+R"); an action at
    /// its default is not here. Written as a key is bound.
    #[serde(
        skip_serializing_if = "std::collections::BTreeMap::is_empty",
        deserialize_with = "keys_leniently"
    )]
    pub keys: std::collections::BTreeMap<String, String>,
    /// The Settings sheet's category last shown, one of
    /// [`CATEGORIES`], where the sheet next opens. Written as it is
    /// chosen; anything else in the file reads as the first.
    #[serde(deserialize_with = "category_leniently")]
    pub settings_category: String,
    /// The interface's palette, one of [`THEMES`]. Written as it is
    /// chosen; anything else in the file reads as the first.
    #[serde(deserialize_with = "theme_leniently")]
    pub theme: String,
    /// Whether the scroll bars fade at rest or stay drawn, one of
    /// [`SCROLL_BARS`]. Written as it is chosen; anything else in the
    /// file reads as the first.
    #[serde(deserialize_with = "scroll_bars_leniently")]
    pub scroll_bars: String,
    /// How thick the scroll bars are, one of [`SCROLL_BAR_SIZES`], as
    /// leniently.
    #[serde(deserialize_with = "scroll_bar_size_leniently")]
    pub scroll_bar_size: String,
}

/// The Settings sheet's categories, top to bottom, by the names the
/// sheet shows.
pub const CATEGORIES: [&str; 5] = ["Interface", "Library", "Editing", "Keys", "About"];

/// The category `name` names, or the first for a name that is none.
pub fn category_named(name: &str) -> &'static str {
    CATEGORIES
        .iter()
        .find(|c| **c == name)
        .copied()
        .unwrap_or(CATEGORIES[0])
}

/// The interface's palettes, by the names the Settings sheet shows;
/// the first is the default.
pub const THEMES: [&str; 3] = ["Charcoal", "Grey", "Light"];

/// The theme `name` names, or the first for a name that is none.
pub fn theme_named(name: &str) -> &'static str {
    THEMES
        .iter()
        .find(|t| **t == name)
        .copied()
        .unwrap_or(THEMES[0])
}

/// The scroll bars' two ways, by the names the Settings sheet shows;
/// the first is the default.
pub const SCROLL_BARS: [&str; 2] = ["Fade", "Stay"];

/// The way `name` names, or the first for a name that is none.
pub fn scroll_bars_named(name: &str) -> &'static str {
    SCROLL_BARS
        .iter()
        .find(|t| **t == name)
        .copied()
        .unwrap_or(SCROLL_BARS[0])
}

/// The scroll bars' sizes, as the sheet shows them; the first is the
/// default.
pub const SCROLL_BAR_SIZES: [&str; 3] = ["Thin", "Medium", "Wide"];

/// The size `name` names, or the first for a name that is none.
pub fn scroll_bar_size_named(name: &str) -> &'static str {
    SCROLL_BAR_SIZES
        .iter()
        .find(|t| **t == name)
        .copied()
        .unwrap_or(SCROLL_BAR_SIZES[0])
}

/// The `settings_category` of a file edited by hand: one of the
/// categories, the first for any other value, so a bad one never costs
/// the rest of the file its settings.
fn category_leniently<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    one_of(d, &CATEGORIES, "settings_category")
}

/// The `theme` of a file edited by hand, as leniently.
fn theme_leniently<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    one_of(d, &THEMES, "theme")
}

/// The `scroll_bars` of a file edited by hand, as leniently.
fn scroll_bars_leniently<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    one_of(d, &SCROLL_BARS, "scroll_bars")
}

/// The `scroll_bar_size` of a file edited by hand, as leniently.
fn scroll_bar_size_leniently<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    one_of(d, &SCROLL_BAR_SIZES, "scroll_bar_size")
}

/// One of `names` read from a file edited by hand, the first for any
/// other value, logged under `field`.
fn one_of<'de, D>(d: D, names: &[&str], field: &str) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(d)?;
    if let serde_json::Value::String(name) = &value
        && names.contains(&name.as_str())
    {
        return Ok(name.clone());
    }
    tracing::warn!("settings: {field} is {value}, not one of {names:?}; the first");
    Ok(names[0].into())
}

fn on() -> bool {
    true
}

/// A string field of a file edited by hand: anything but a string
/// reads as empty, its default, and costs the rest nothing.
fn string_leniently<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => Ok(s),
        other => {
            tracing::warn!("settings: {other} is not a string; empty");
            Ok(String::new())
        }
    }
}

/// A switch that is on by default, from a file edited by hand:
/// anything but `true` or `false` reads as on.
fn bool_on_leniently<'de, D>(d: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Bool(b) => Ok(b),
        other => {
            tracing::warn!("settings: {other} is not a switch; on");
            Ok(true)
        }
    }
}

/// What a launch with no path opens first ([`start_listing`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start<T> {
    /// These frames, as a folder: the chosen one's, or the last one's.
    Frames(Vec<T>),
    /// The chosen folder, there with no frames of its own and folders
    /// in it: a folder of shoots, say. The caller knows the roots, and
    /// opens it as the library's view of it or passes over it.
    Bare(std::path::PathBuf),
    /// Neither folder has frames.
    Nothing,
}

/// The frames a launch with no path opens: the `chosen` folder's, as
/// `list` finds them, while it is a folder on the disk with frames in
/// it, else the `last` folder's, the last file's. Empty is no choice. A
/// chosen folder gone (a drive unplugged, a shoot moved) is said in the
/// log and the last folder opened, so the launch is neither an empty
/// window nor the chooser over it, and so is one there and empty (a
/// card's folder emptied after each import). A chosen folder with no
/// frames of its own but folders in it is handed back bare for the
/// caller to settle.
pub fn start_listing<T>(
    chosen: &str,
    last: Option<&std::path::Path>,
    list: impl Fn(&std::path::Path) -> Option<Vec<T>>,
) -> Start<T> {
    let listed = |dir: &std::path::Path| list(dir).filter(|frames| !frames.is_empty());
    if !chosen.is_empty() {
        let dir = std::path::Path::new(chosen);
        if !dir.is_dir() {
            tracing::info!("start folder {chosen} is not there; the last folder");
        } else if let Some(frames) = listed(dir) {
            return Start::Frames(frames);
        } else if holds_folders(dir) {
            return Start::Bare(dir.to_path_buf());
        } else {
            tracing::info!("start folder {chosen} has no frames; the last folder");
        }
    }
    match last.and_then(listed) {
        Some(frames) => Start::Frames(frames),
        None => Start::Nothing,
    }
}

/// Whether `dir` holds a folder that is not hidden (no dot to its
/// name). The listing stops at the first; one that cannot be read
/// holds none.
fn holds_folders(dir: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        !e.file_name().to_string_lossy().starts_with('.')
            && e.file_type()
                .is_ok_and(|t| t.is_dir() || (t.is_symlink() && e.path().is_dir()))
    })
}

/// The `keys` of a file edited by hand: each string kept, anything
/// else logged and left out, so one bad value never costs the rest of
/// the file its settings.
fn keys_leniently<'de, D>(d: D) -> Result<std::collections::BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(d)?;
    let mut out = std::collections::BTreeMap::new();
    match value {
        serde_json::Value::Object(map) => {
            for (id, v) in map {
                match v {
                    serde_json::Value::String(chord) => {
                        out.insert(id, chord);
                    }
                    other => {
                        tracing::warn!("settings: keys: {id:?} is {other}, not a chord; left out")
                    }
                }
            }
        }
        serde_json::Value::Null => {}
        other => tracing::warn!("settings: keys is {other}, not a map of chords; left out"),
    }
    Ok(out)
}

/// The sync sheet's three relative choices: on, a frame is shifted
/// from its own value; off, set to the source's. All off by default,
/// which is the sync every earlier build ran.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncShift {
    pub temperature: bool,
    pub tint: bool,
    pub exposure: bool,
}

/// How many scopes' unchecked groups the match sheet remembers.
pub const MATCH_SCOPES: usize = 40;

/// The groups left unchecked on the camera match's sheet for one
/// scope: `"library"`, or a folder's canonical path.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MatchChoice {
    pub scope: String,
    /// The groups' look names.
    pub unchecked: Vec<String>,
}

/// The groups left unchecked for `scope`; none when it was never
/// seen, which checks every group.
pub fn unchecked_for(list: &[MatchChoice], scope: &str) -> Vec<String> {
    list.iter()
        .find(|c| c.scope == scope)
        .map(|c| c.unchecked.clone())
        .unwrap_or_default()
}

/// Keep `unchecked` as the choice for `scope`: moved to the front, the
/// list cut to [`MATCH_SCOPES`]; a scope with everything checked keeps
/// nothing, since that is what an unseen one is.
pub fn remember_unchecked(list: &mut Vec<MatchChoice>, scope: &str, unchecked: Vec<String>) {
    list.retain(|c| c.scope != scope);
    if !unchecked.is_empty() {
        list.insert(
            0,
            MatchChoice {
                scope: scope.to_string(),
                unchecked,
            },
        );
    }
    list.truncate(MATCH_SCOPES);
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
            export_last_folder: String::new(),
            scope: crate::scope::Scope::default().name().into(),
            curve_mode: "Point".into(),
            warn_shadows: false,
            warn_highlights: false,
            proof_profile: crate::export::Space::Srgb.name().into(),
            proof_intent: crate::display::ProofIntent::default().name().into(),
            gamut_warning: false,
            display_profile: "System".into(),
            display_monitor: String::new(),
            canvas_color: crate::render::CANVAS_NAMES[0].into(),
            mat: false,
            collapsed: Vec::new(),
            one_section: false,
            grid_cell: crate::grid::CELL,
            grid_wheel: true,
            lightbox_across: crate::lightbox::ACROSS,
            hide_left: false,
            hide_right: false,
            hide_strip: false,
            xmp_sidecars: false,
            sidecars_in_folder: false,
            jpegs_apart: false,
            cull_move_on: false,
            loupe_cull: false,
            cull_developed: false,
            lenses_declined: false,
            rawtherapee_answered: Vec::new(),
            last_file: String::new(),
            recent_folders: Vec::new(),
            start_folder: String::new(),
            filter_bar: true,
            folder_tree_subfolders: false,
            thumb_cache_mb: greycard_library::thumbs::DEFAULT_CAP / (1024 * 1024),
            preview_cache_mb: greycard_library::thumbs::DEFAULT_PREVIEW_CAP / (1024 * 1024),
            network_poll_minutes: 10,
            import: ImportChoices::default(),
            filter: crate::filter::Saved::default(),
            sort: crate::sort::Kept::default(),
            update: crate::update::Kept::default(),
            match_unchecked: Vec::new(),
            sync_shift: SyncShift::default(),
            crop_start: "Original".into(),
            crop_last: "Free".into(),
            fill_choice: String::new(),
            ui_scale: 100,
            ui_scale_system: 0.0,
            keys: std::collections::BTreeMap::new(),
            settings_category: CATEGORIES[0].into(),
            theme: THEMES[0].into(),
            scroll_bars: SCROLL_BARS[0].into(),
            scroll_bar_size: SCROLL_BAR_SIZES[0].into(),
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

#[cfg(test)]
thread_local! {
    /// The settings file this thread's test uses, none by default.
    static TEST_PATH: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Point this thread's settings at `path` (`None` for no file).
#[cfg(test)]
pub(crate) fn use_file(path: Option<PathBuf>) {
    TEST_PATH.with(|p| *p.borrow_mut() = path);
}

/// `$XDG_CONFIG_HOME/greycard/settings.json` when that is set, else
/// `greycard/settings.json` under the platform's configuration
/// directory: `~/.config` on Linux, `~/Library/Application Support` on
/// macOS, `%APPDATA%` on Windows.
pub fn path() -> Option<PathBuf> {
    // A test never reads or writes the user's settings: no file, so
    // the defaults, unless the test sets a scratch path of its own.
    #[cfg(test)]
    return TEST_PATH.with(|p| p.borrow().clone());
    #[cfg(not(test))]
    real_path()
}

fn real_path() -> Option<PathBuf> {
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
        Self::read(path).unwrap_or_else(|e| {
            tracing::warn!("settings {}: {e}; defaults", path.display());
            Self::default()
        })
    }

    /// The file as it stands, or none when it is there and will not
    /// read: a write merged over the defaults would put a file
    /// hand-edited with a typo out of reach.
    pub fn load_parsed() -> Option<Self> {
        path().map_or(Some(Self::default()), |p| Self::read(&p).ok())
    }

    /// The file, the defaults when there is none, or why it will not
    /// read.
    fn read(path: &std::path::Path) -> Result<Self, String> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.to_string()),
        };
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }

    pub fn save_to(&self, path: &std::path::Path) {
        let text = match serde_json::to_string_pretty(self) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("settings not saved: {e}");
                return;
            }
        };
        // A symlinked file is written where it points, and stays a link.
        let path = &std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(dir) = path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            tracing::warn!("settings directory {}: {e}", dir.display());
            return;
        }
        // Through a temporary and a rename: the file is written as
        // choices change, and a process ended mid-write must leave the
        // old file whole rather than half of the new one.
        // Named for the process, so two windows never rename each
        // other's half-written file into place; given the old file's
        // permissions.
        let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
        let written = std::fs::write(&temporary, text).and_then(|()| {
            if let Ok(old) = std::fs::metadata(path) {
                std::fs::set_permissions(&temporary, old.permissions())?;
            }
            std::fs::rename(&temporary, path)
        });
        if let Err(e) = written {
            tracing::warn!("settings not saved to {}: {e}", path.display());
            let _ = std::fs::remove_file(&temporary);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A settings file that is a link is written where it points, and
    /// stays a link.
    #[cfg(unix)]
    #[test]
    fn a_linked_file_is_written_through_the_link() {
        let dir = crate::testing::scratch_dir("settings-link");
        let (real, link) = (dir.join("real.json"), dir.join("settings.json"));
        Settings::default().save_to(&real);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mine = Settings {
            canvas_color: crate::render::CANVAS_NAMES[1].into(),
            ..Settings::default()
        };
        mine.save_to(&link);
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(Settings::load_from(&real), mine);
        crate::testing::remove_dir_retry(&dir);
    }

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
            subfolder: "export".into(),
            metadata: "None".into(),
            mark: "Image".into(),
            mark_text: "© x".into(),
            mark_image: "/home/x/logo.png".into(),
            mark_color: "Black".into(),
            mark_position: "Top left".into(),
            mark_size: 12.5,
            mark_margin: 3.0,
            mark_opacity: 40.0,
            border: true,
            border_width: 2.5,
            border_color: "Match".into(),
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
            gamut_warning: true,
            display_profile: "Adobe RGB".into(),
            display_monitor: "PA279CRV".into(),
            canvas_color: "Mid grey".into(),
            mat: true,
            collapsed: vec!["grain".into(), "demosaic".into()],
            one_section: true,
            grid_cell: 256.0,
            grid_wheel: false,
            lightbox_across: 5,
            hide_left: true,
            hide_right: false,
            hide_strip: true,
            xmp_sidecars: true,
            sidecars_in_folder: true,
            jpegs_apart: true,
            cull_move_on: true,
            loupe_cull: true,
            cull_developed: true,
            lenses_declined: true,
            rawtherapee_answered: vec!["/home/x/Pictures/rt".into()],
            export_last_folder: "/home/x/Pictures/out".into(),
            last_file: "/home/x/Pictures/IMG_0001.CR3".into(),
            recent_folders: vec!["/home/x/Pictures/b".into(), "/mnt/gone/a".into()],
            start_folder: "~/Pictures/start".into(),
            filter_bar: false,
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
            sort: crate::sort::Kept {
                folder: Some("rating-desc".into()),
                roots: Some("capture".into()),
                collections: [("0123".to_string(), "hand".to_string())]
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
            match_unchecked: vec![MatchChoice {
                scope: "/home/x/Pictures/shoot".into(),
                unchecked: vec!["Canon EOS R6m2 Standard".into()],
            }],
            sync_shift: SyncShift {
                temperature: true,
                tint: false,
                exposure: true,
            },
            crop_start: "Last used".into(),
            crop_last: "Custom 7:5".into(),
            fill_choice: "Fast".into(),
            ui_scale: 150,
            ui_scale_system: 1.25,
            keys: [("crop".to_string(), "Ctrl+R".to_string())]
                .into_iter()
                .collect(),
            settings_category: "Keys".into(),
            theme: "Light".into(),
            scroll_bars: "Stay".into(),
            scroll_bar_size: "Wide".into(),
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
        // And from before the RawTherapee offer, nothing answered.
        assert!(read.rawtherapee_answered.is_empty());
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
        // And from before the interface scale, the system's, none seen.
        assert_eq!(read.ui_scale, 100);
        assert_eq!(read.ui_scale_system, 0.0);
    }

    /// The interface scale is written like every other field, and read
    /// back from a file that has it, or the defaults from one without.
    #[test]
    fn the_interface_scale_is_written_and_read_back() {
        let text = serde_json::to_string(&Settings::default()).unwrap();
        assert!(text.contains(r#""ui_scale":100"#), "{text}");
        assert!(text.contains(r#""ui_scale_system":0.0"#), "{text}");
        let read: Settings =
            serde_json::from_str(r#"{"ui_scale": 175, "ui_scale_system": 2.0}"#).unwrap();
        assert_eq!((read.ui_scale, read.ui_scale_system), (175, 2.0));
        let dir =
            std::env::temp_dir().join(format!("greycard-settings-scale-{}", std::process::id()));
        let path = dir.join("settings.json");
        let mine = Settings {
            ui_scale: 125,
            ui_scale_system: 1.5,
            ..Settings::default()
        };
        mine.save_to(&path);
        assert_eq!(Settings::load_from(&path), mine);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The keys rebound are under `keys`, only the ones moved: a file
    /// with none moved has no `keys` at all.
    #[test]
    fn the_rebound_keys_are_written_alone_under_one_key() {
        let text = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!text.contains(r#""keys""#), "{text}");
        let mut map = crate::keys::Keymap::default();
        assert!(map.set("crop", "Ctrl+R"));
        assert!(map.set("undo", "Ctrl+Z"), "its default: not moved");
        let mine = Settings {
            keys: map.saved(),
            ..Settings::default()
        };
        let text = serde_json::to_string(&mine).unwrap();
        assert!(text.contains(r#""keys":{"crop":"Ctrl+R"}"#), "{text}");
        let read: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(crate::keys::Keymap::from_saved(&read.keys), map);
        // A name that is no action and a chord that is none are left
        // out as the file is read, and the rest kept.
        let read: Settings =
            serde_json::from_str(r#"{"keys": {"crop": "Cmd+R", "fly": "F", "undo": "Shift+?"}}"#)
                .unwrap();
        assert_eq!(crate::keys::Keymap::from_saved(&read.keys), map);
        // A value that is no string, or a `keys` that is no map, loses
        // itself and nothing else.
        for text in [
            r#"{"ui_scale": 150, "keys": {"crop": 5, "undo": "Ctrl+U"}}"#,
            r#"{"ui_scale": 150, "keys": {"crop": null}}"#,
            r#"{"ui_scale": 150, "keys": null}"#,
            r#"{"ui_scale": 150, "keys": ["crop"]}"#,
        ] {
            let read: Settings = serde_json::from_str(text).expect(text);
            assert_eq!(read.ui_scale, 150, "{text}");
            assert!(read.keys.values().all(|c| c == "Ctrl+U"), "{text}");
        }
    }

    /// The sheet's category: a file without one opens at the first; a
    /// name that is no category, or a value that is no string, reads
    /// as the first and costs the rest of the file nothing.
    #[test]
    fn the_settings_category_is_read_leniently() {
        let read: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(read.settings_category, "Interface");
        let read: Settings = serde_json::from_str(r#"{"settings_category": "About"}"#).unwrap();
        assert_eq!(read.settings_category, "About");
        for text in [
            r#"{"ui_scale": 150, "settings_category": "Colours"}"#,
            r#"{"ui_scale": 150, "settings_category": 3}"#,
            r#"{"ui_scale": 150, "settings_category": null}"#,
        ] {
            let read: Settings = serde_json::from_str(text).expect(text);
            assert_eq!(read.ui_scale, 150, "{text}");
            assert_eq!(read.settings_category, "Interface", "{text}");
        }
        assert_eq!(category_named("Editing"), "Editing");
        assert_eq!(category_named("keys"), "Interface");
    }

    /// The start folder and the filter bar's switch: a file without
    /// them is the last folder and the bar shown; a value of the wrong
    /// kind reads as the same and costs the rest of the file nothing.
    #[test]
    fn the_start_folder_and_the_filter_bar_are_read_leniently() {
        let read: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(read.start_folder, "");
        assert!(read.filter_bar);
        let read: Settings =
            serde_json::from_str(r#"{"start_folder": "/p/start", "filter_bar": false}"#).unwrap();
        assert_eq!(read.start_folder, "/p/start");
        assert!(!read.filter_bar);
        for text in [
            r#"{"ui_scale": 150, "start_folder": 3, "filter_bar": "no"}"#,
            r#"{"ui_scale": 150, "start_folder": null, "filter_bar": null}"#,
            r#"{"ui_scale": 150, "start_folder": ["/p"], "filter_bar": 0}"#,
        ] {
            let read: Settings = serde_json::from_str(text).expect(text);
            assert_eq!(read.ui_scale, 150, "{text}");
            assert_eq!(read.start_folder, "", "{text}");
            assert!(read.filter_bar, "{text}");
        }
    }

    /// The wheel's switch: a file without it, or with a value of the
    /// wrong kind, reads as on, and costs the rest of the file nothing.
    #[test]
    fn the_wheel_switch_is_read_leniently() {
        let read: Settings = serde_json::from_str("{}").unwrap();
        assert!(read.grid_wheel);
        let read: Settings = serde_json::from_str(r#"{"grid_wheel": false}"#).unwrap();
        assert!(!read.grid_wheel);
        for text in [
            r#"{"ui_scale": 150, "grid_wheel": "no"}"#,
            r#"{"ui_scale": 150, "grid_wheel": null}"#,
            r#"{"ui_scale": 150, "grid_wheel": 0}"#,
        ] {
            let read: Settings = serde_json::from_str(text).expect(text);
            assert_eq!(read.ui_scale, 150, "{text}");
            assert!(read.grid_wheel, "{text}");
        }
    }

    /// The frames a launch opens: the chosen folder's while it is
    /// there with frames in it, else the last folder's; no choice is
    /// the last, and neither is nothing. The chosen folder there with
    /// no frames of its own but folders is handed back bare, not
    /// passed over; one empty, or with only hidden folders, is.
    #[test]
    fn the_start_folder_falls_back_to_the_last_one() {
        let dir = std::env::temp_dir().join(format!("greycard-start-{}", std::process::id()));
        let chosen = dir.join("chosen");
        std::fs::create_dir_all(&chosen).unwrap();
        let last = std::path::Path::new("/p/last");
        // A folder's frames by its name, the chosen one's set per case.
        let frames = |chosen_has: bool| {
            let chosen = chosen.clone();
            move |d: &std::path::Path| -> Option<Vec<PathBuf>> {
                if d == chosen {
                    Some(if chosen_has {
                        vec![d.join("a.CR3")]
                    } else {
                        Vec::new()
                    })
                } else if d == std::path::Path::new("/p/last") {
                    Some(vec![d.join("b.CR3")])
                } else {
                    None
                }
            }
        };
        let in_chosen = Start::Frames(vec![chosen.join("a.CR3")]);
        let in_last = Start::Frames(vec![last.join("b.CR3")]);
        let chosen_text = chosen.to_string_lossy();
        assert_eq!(
            start_listing(&chosen_text, Some(last), frames(true)),
            in_chosen
        );
        assert_eq!(start_listing(&chosen_text, None, frames(true)), in_chosen);
        assert_eq!(start_listing("", Some(last), frames(true)), in_last);
        assert_eq!(start_listing("", None, frames(true)), Start::Nothing);
        // There and empty, or with only a hidden folder in it: the
        // last folder, or nothing.
        std::fs::create_dir_all(chosen.join(".thumbs")).unwrap();
        assert_eq!(
            start_listing(&chosen_text, Some(last), frames(false)),
            in_last
        );
        assert_eq!(
            start_listing(&chosen_text, None, frames(false)),
            Start::Nothing
        );
        // No frames of its own, but folders: bare, whatever the last.
        std::fs::create_dir_all(chosen.join("2026")).unwrap();
        assert_eq!(
            start_listing(&chosen_text, Some(last), frames(false)),
            Start::Bare(chosen.clone())
        );
        assert_eq!(
            start_listing(&chosen_text, None, frames(false)),
            Start::Bare(chosen.clone())
        );
        // A file is no folder to open.
        let file = dir.join("a.CR3");
        std::fs::write(&file, b"raw").unwrap();
        assert_eq!(
            start_listing(&file.to_string_lossy(), Some(last), frames(true)),
            in_last
        );
        assert_eq!(
            start_listing(&file.to_string_lossy(), None, frames(true)),
            Start::Nothing
        );
        // Gone from the disk: the last folder, or nothing.
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            start_listing(&chosen_text, Some(last), frames(true)),
            in_last
        );
        assert_eq!(
            start_listing(&chosen_text, None, frames(true)),
            Start::Nothing
        );
        // A `~` is not made a home: taken as written, and not there.
        assert_eq!(start_listing("~", Some(last), frames(true)), in_last);
    }

    /// The theme: a file without one, or with a name that is no
    /// theme, reads as Charcoal and costs the rest of the file nothing.
    #[test]
    fn the_theme_is_read_leniently() {
        let read: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(read.theme, "Charcoal");
        for name in THEMES {
            let text = format!(r#"{{"theme": "{name}"}}"#);
            let read: Settings = serde_json::from_str(&text).unwrap();
            assert_eq!(read.theme, name);
            let back: Settings =
                serde_json::from_str(&serde_json::to_string(&read).unwrap()).unwrap();
            assert_eq!(back.theme, name);
        }
        for text in [
            r#"{"ui_scale": 150, "theme": "light"}"#,
            r#"{"ui_scale": 150, "theme": 2}"#,
            r#"{"ui_scale": 150, "theme": null}"#,
        ] {
            let read: Settings = serde_json::from_str(text).expect(text);
            assert_eq!(read.ui_scale, 150, "{text}");
            assert_eq!(read.theme, "Charcoal", "{text}");
        }
        assert_eq!(theme_named("Grey"), "Grey");
        assert_eq!(theme_named("Gray"), "Charcoal");
    }

    /// The scroll bars' way and size: each name reads and writes back
    /// as itself, and a file without them, or with a name that is
    /// neither, reads as Fade and Thin and costs the rest nothing.
    #[test]
    fn the_scroll_bars_are_read_leniently() {
        let read: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            (read.scroll_bars.as_str(), read.scroll_bar_size.as_str()),
            ("Fade", "Thin")
        );
        for (field, names) in [
            ("scroll_bars", &SCROLL_BARS[..]),
            ("scroll_bar_size", &SCROLL_BAR_SIZES[..]),
        ] {
            for name in names {
                let text = format!(r#"{{"{field}": "{name}"}}"#);
                let read: Settings = serde_json::from_str(&text).unwrap();
                let back: Settings =
                    serde_json::from_str(&serde_json::to_string(&read).unwrap()).unwrap();
                for s in [&read, &back] {
                    let got = if field == "scroll_bars" {
                        &s.scroll_bars
                    } else {
                        &s.scroll_bar_size
                    };
                    assert_eq!(got, name, "{text}");
                }
            }
        }
        for bad in [r#""stay""#, "2", "null", r#""Huge""#] {
            let text =
                format!(r#"{{"ui_scale": 150, "scroll_bars": {bad}, "scroll_bar_size": {bad}}}"#);
            let read: Settings = serde_json::from_str(&text).expect(&text);
            assert_eq!(read.ui_scale, 150, "{text}");
            assert_eq!(read.scroll_bars, "Fade", "{text}");
            assert_eq!(read.scroll_bar_size, "Thin", "{text}");
        }
        assert_eq!(scroll_bars_named("Stay"), "Stay");
        assert_eq!(scroll_bars_named("Always"), "Fade");
        assert_eq!(scroll_bar_size_named("Medium"), "Medium");
        assert_eq!(scroll_bar_size_named("medium"), "Thin");
    }

    #[test]
    fn the_match_sheet_remembers_what_was_unchecked_a_scope_at_a_time() {
        let mut list = Vec::new();
        // Never seen: everything is checked.
        assert!(unchecked_for(&list, "library").is_empty());
        remember_unchecked(&mut list, "library", vec!["A Faithful".into()]);
        remember_unchecked(&mut list, "/p/a", vec!["B Standard".into()]);
        assert_eq!(unchecked_for(&list, "library"), ["A Faithful"]);
        assert_eq!(unchecked_for(&list, "/p/a"), ["B Standard"]);
        // Checking everything again forgets the scope.
        remember_unchecked(&mut list, "library", Vec::new());
        assert!(unchecked_for(&list, "library").is_empty());
        assert_eq!(list.len(), 1);
        // The list is bounded, the newest kept.
        for i in 0..(MATCH_SCOPES + 5) {
            remember_unchecked(&mut list, &format!("/p/{i}"), vec!["x".into()]);
        }
        assert_eq!(list.len(), MATCH_SCOPES);
        assert!(unchecked_for(&list, "/p/a").is_empty());
        // A file from before the choice has none.
        let read: Settings = serde_json::from_str("{}").unwrap();
        assert!(read.match_unchecked.is_empty());
        // And none of the sync's shifts: every one sets, as before.
        assert_eq!(read.sync_shift, SyncShift::default());
    }

    #[test]
    fn a_file_without_the_lightbox_count_opens_it_at_four() {
        let read: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(read.lightbox_across, 4);
        let read: Settings = serde_json::from_str(r#"{"lightbox_across": 6}"#).unwrap();
        assert_eq!(read.lightbox_across, 6);
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
    fn the_gamut_warning_is_off_by_default() {
        assert!(!Settings::default().gamut_warning);
    }

    #[test]
    fn the_path_is_under_the_configuration_directory() {
        let path = real_path().expect("a configuration directory in the test environment");
        assert!(path.ends_with("greycard/settings.json"), "{path:?}");
        assert!(path.is_absolute());
    }
}
