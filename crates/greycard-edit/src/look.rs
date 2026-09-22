//! The look an edit finishes through: a 3D LUT from the user's look
//! directory, at a strength.
//!
//! A look is the output end of what notes §78 calls the two ends of the
//! pipeline — the camera profile being the input end ([`crate::camera`],
//! which this module is built in the shape of). It goes on after the
//! tone curve and the point curves, in the table's own encoding, and
//! before the output transform.
//!
//! The choice lives in the edit because it changes the picture and a
//! preset should carry it: a film preset is exactly a look with a
//! strength. The engine takes the resolved table, not a name: reading
//! the file is this crate's job, and it is done once per file and kept,
//! so the viewport's finish and the export's are handed the same table.
//!
//! **On the name.** [`crate::Look`] is already the part of an edit a
//! mask can carry — light, curves, mixer, color, grading, tint. This is
//! the other thing the word means, a LUT, and to keep the two apart the
//! type here is [`LookLut`] and the field on [`crate::Edit`] is
//! `look_lut`. The sidecar's key is `look`, which is what the roadmap
//! and the panel call it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use greycard_core::lut::{self, Encoding, Kind, Lut3d, Primaries};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The word for no look at all, in the sidecar and on the panel.
pub const NONE: &str = "none";

/// Which look table the picture finishes through.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LutChoice {
    /// The picture as the pipeline leaves it.
    #[default]
    None,
    /// A table in the look directory, by file name without the
    /// extension.
    Named(String),
}

impl LutChoice {
    /// The name as the sidecar writes it.
    pub fn name(&self) -> &str {
        match self {
            LutChoice::None => NONE,
            LutChoice::Named(name) => name,
        }
    }

    /// A name from the panel or a sidecar. Anything empty, or the word
    /// for no look, is [`LutChoice::None`].
    pub fn from_name(name: &str) -> Self {
        let name = name.trim();
        if name.is_empty() || name.eq_ignore_ascii_case(NONE) {
            LutChoice::None
        } else {
            LutChoice::Named(name.to_string())
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, LutChoice::None)
    }
}

impl Serialize for LutChoice {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for LutChoice {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(LutChoice::from_name(&String::deserialize(d)?))
    }
}

/// The LOOK section of an edit: a table by name and how much of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LookLut {
    pub lut: LutChoice,
    /// 0 for none of the table, 1 for the whole of it.
    pub strength: f32,
}

impl Default for LookLut {
    fn default() -> Self {
        LookLut {
            lut: LutChoice::None,
            // A look chosen is a look wanted; the slider is there to
            // take it back, not to have to be found first.
            strength: 1.0,
        }
    }
}

impl LookLut {
    /// Whether this would change any pixel.
    pub fn is_off(&self) -> bool {
        self.lut.is_none() || self.strength <= 0.0
    }

    /// The engine's look for this choice: the table read and kept with
    /// its matrices solved, or `None` when no table is named. A table
    /// that will not read warns once and comes back `None`, which is
    /// no look.
    ///
    /// A named table at a strength of zero still comes back, carrying
    /// that strength: `None` means "there is no table here", which is
    /// what tells a consumer it may let go of one. Whether the look
    /// does anything is [`lut::Look::is_off`].
    pub fn look(&self) -> Option<lut::Look> {
        let LutChoice::Named(name) = &self.lut else {
            return None;
        };
        let path = path_for(name)?;
        let table = load(&path)?;
        match lut::Look::new(table, self.strength) {
            Ok(look) => Some(look),
            Err(e) => {
                log::warn!("look {}: {e}", path.display());
                None
            }
        }
    }
}

/// Where the look tables live: `$XDG_DATA_HOME/greycard/looks` when
/// that is set, else `greycard/looks` under the platform's data
/// directory — `~/.local/share` on Linux, `~/Library/Application
/// Support` on macOS, `%APPDATA%` on Windows (notes §106). The same
/// logic the profile directory uses, one name along.
pub fn store_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::data_dir)?;
    Some(base.join("greycard").join("looks"))
}

/// Where a table of this name is kept. A name that is a path, or tries
/// to climb out of the directory, is refused.
///
/// Two extensions are read, so the one that is there wins; with
/// neither there the `.cube` path comes back, to be named in the
/// warning that follows.
pub fn path_for(name: &str) -> Option<PathBuf> {
    if name.is_empty()
        || name.contains(['/', '\\'])
        || name.contains("..")
        || Path::new(name).is_absolute()
    {
        log::warn!("look {name:?} is not a name in the look directory");
        return None;
    }
    let dir = store_dir()?;
    let mut first = None;
    for extension in lut::EXTENSIONS {
        let path = dir.join(format!("{name}.{extension}"));
        if path.is_file() {
            return Some(path);
        }
        first.get_or_insert(path);
    }
    first
}

/// A table in the directory, as the panel lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The file name without its extension: what the edit stores.
    pub name: String,
    pub path: PathBuf,
    /// The table's own name, when it carries one.
    pub title: Option<String>,
    /// Nodes an axis.
    pub size: usize,
    pub kind: Kind,
    pub encoding: Encoding,
    pub primaries: Primaries,
}

impl Entry {
    /// What the panel shows: the table's own name when it has one and
    /// it is not simply the file's name again.
    pub fn label(&self) -> &str {
        match &self.title {
            Some(title) if !title.is_empty() && title != &self.name => title,
            _ => &self.name,
        }
    }

    /// The line under the list when this one is chosen: what it is and
    /// what it was made for, since neither is on the row.
    pub fn described(&self) -> String {
        let mut out = format!("{}, {} nodes", self.kind.name(), self.size);
        if self.encoding != Encoding::default() || self.primaries != Primaries::default() {
            out.push_str(&format!(
                ", {} in {}",
                self.encoding.name(),
                self.primaries.name()
            ));
        }
        out
    }
}

/// Every look table in the directory, by name.
///
/// Only each file's header is read — its size and what it says it is —
/// not its table: a directory of film stocks at 64 nodes an axis is
/// megabytes of floats nobody has asked for yet, and this runs on the
/// thread that draws the panel. The table that is chosen is read in
/// full by [`load`], once. A file that will not read is passed over
/// with a word in the log.
pub fn list() -> Vec<Entry> {
    // Nothing is forgotten here. [`load`] reads a file again when its
    // size or its clock moved, so a table replaced in place is picked
    // up anyway, and dropping the cache on every listing would re-read
    // the open picture's table and re-upload it to the GPU each time
    // the section is opened.
    let Some(dir) = store_dir() else {
        return Vec::new();
    };
    lut::list_dir(&dir)
        .into_iter()
        .filter_map(|(name, path, info)| {
            let info = match info {
                Ok(info) => info,
                Err(e) => {
                    log::warn!("look {}: {e}", path.display());
                    return None;
                }
            };
            Some(Entry {
                name,
                path,
                title: info.title,
                size: info.size,
                kind: info.kind,
                encoding: info.encoding,
                primaries: info.primaries,
            })
        })
        .collect()
}

/// What a path read to last time.
enum Cached {
    /// There was no file to read, and it has been said once.
    Missing,
    /// A file of this age and size read to this, or to nothing.
    Read {
        stamp: Option<std::time::SystemTime>,
        len: u64,
        table: Option<Arc<Lut3d>>,
    },
}

/// How many tables are kept at once. One edit names one, and a
/// filmstrip of files naming different ones is the only way to reach
/// even a handful; past this the lot is dropped rather than grown.
const KEEP: usize = 4;

static CACHE: LazyLock<Mutex<HashMap<PathBuf, Cached>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The table in a file, read once and kept until the file changes.
///
/// A file that is missing, or that will not read, is remembered as
/// such, so the warning is said once rather than on every develop.
pub fn load(path: &Path) -> Option<Arc<Lut3d>> {
    let stamped = match std::fs::metadata(path) {
        Ok(m) => Some((m.modified().ok(), m.len())),
        Err(_) => None,
    };
    let mut cache = CACHE.lock().ok()?;
    match (cache.get(path), &stamped) {
        (Some(Cached::Missing), None) => return None,
        (Some(Cached::Read { stamp, len, table }), Some((now, size)))
            if stamp == now && len == size =>
        {
            return table.clone();
        }
        _ => {}
    }
    if cache.len() >= KEEP {
        cache.clear();
    }
    let Some((stamp, len)) = stamped else {
        log::warn!("look {}: it is not there", path.display());
        cache.insert(path.to_path_buf(), Cached::Missing);
        return None;
    };
    let table = match Lut3d::load(path) {
        Ok(table) => Some(Arc::new(table)),
        Err(e) => {
            log::warn!("look {}: {e}", path.display());
            None
        }
    };
    cache.insert(
        path.to_path_buf(),
        Cached::Read {
            stamp,
            len,
            table: table.clone(),
        },
    );
    table
}

/// Forget what has been read, so the next develop reads it again.
pub fn forget() {
    if let Ok(mut cache) = CACHE.lock() {
        cache.clear();
    }
}

/// The rows of the look picker: None first, then the directory's, in
/// its order, then the chosen one when the directory has not got it.
/// Each row is the name the edit stores and the label the panel
/// shows.
pub fn rows(looks: &[Entry], chosen: &str) -> Vec<(String, String)> {
    let mut rows = vec![(NONE.to_string(), "None".to_string())];
    rows.extend(
        looks
            .iter()
            .map(|e| (e.name.clone(), e.label().to_string())),
    );
    if chosen != NONE && !rows.iter().any(|(n, _)| n == chosen) {
        rows.push((chosen.to_string(), format!("{chosen} (missing)")));
    }
    rows
}

/// What is wrong with the chosen look, if anything: that the
/// directory has not got it. A look that will not read says so in the
/// log and the picture goes out without it.
pub fn warning(looks: &[Entry], chosen: &str) -> String {
    if chosen == NONE {
        return String::new();
    }
    if looks.iter().any(|e| e.name == chosen) {
        return String::new();
    }
    format!("{chosen} is not in the look directory; the picture goes out without it.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_core::lut::{Encoding, Kind, Primaries};

    fn entry(name: &str, title: Option<&str>) -> Entry {
        Entry {
            name: name.into(),
            path: std::path::PathBuf::new(),
            title: title.map(str::to_string),
            size: 33,
            kind: Kind::Cube3d,
            encoding: Encoding::default(),
            primaries: Primaries::default(),
        }
    }

    /// The picker lists None first, then the directory, with the
    /// table's own name where it has one.
    #[test]
    fn the_picker_lists_none_first_then_the_directory() {
        let looks = vec![
            entry("slide-warm", Some("Slide Warm")),
            entry("faded", None),
        ];
        let listed = rows(&looks, "none");
        assert_eq!(
            listed,
            vec![
                ("none".to_string(), "None".to_string()),
                ("slide-warm".to_string(), "Slide Warm".to_string()),
                ("faded".to_string(), "faded".to_string()),
            ]
        );
        assert!(warning(&looks, "none").is_empty());
        assert!(warning(&looks, "faded").is_empty());
    }

    /// A look the edit names and the directory has not got is still a
    /// row, still the chosen one, and says so: the panel says what
    /// the edit says, and a sidecar or a preset from another machine
    /// is how that happens.
    #[test]
    fn a_look_that_is_not_there_is_listed_and_warned_about() {
        let looks = vec![entry("faded", None)];
        let listed = rows(&looks, "Kodachrome-ish");
        assert_eq!(listed.len(), 3);
        assert_eq!(
            listed[2],
            (
                "Kodachrome-ish".to_string(),
                "Kodachrome-ish (missing)".to_string()
            )
        );
        let warning = warning(&looks, "Kodachrome-ish");
        assert!(warning.contains("not in the look directory"), "{warning}");
        // An empty directory is not an error, only an empty list.
        assert_eq!(rows(&[], "none").len(), 1);
    }

    /// A `.cube`'s text: a table that takes everything to one color,
    /// so a strength is easy to read off.
    fn flat_cube(to: [f32; 3]) -> String {
        let mut text = String::from("TITLE \"Flat\"\nLUT_3D_SIZE 2\n");
        for _ in 0..8 {
            text.push_str(&format!("{} {} {}\n", to[0], to[1], to[2]));
        }
        text
    }

    #[test]
    fn a_choice_reads_and_writes_as_a_name() {
        assert_eq!(LutChoice::default(), LutChoice::None);
        assert_eq!(LutChoice::None.name(), "none");
        assert_eq!(LutChoice::from_name(""), LutChoice::None);
        assert_eq!(LutChoice::from_name("None"), LutChoice::None);
        assert_eq!(
            LutChoice::from_name(" Slide Warm "),
            LutChoice::Named("Slide Warm".into())
        );
        let json = serde_json::to_string(&LutChoice::Named("Slide Warm".into())).unwrap();
        assert_eq!(json, "\"Slide Warm\"");
        let back: LutChoice = serde_json::from_str(&json).unwrap();
        assert_eq!(back, LutChoice::Named("Slide Warm".into()));
    }

    /// The default is no look at a full strength, so choosing one from
    /// the panel shows it whole, and a sidecar written before the
    /// section existed reads as the default.
    #[test]
    fn the_section_defaults_to_no_look_at_full_strength() {
        let look = LookLut::default();
        assert!(look.lut.is_none());
        assert_eq!(look.strength, 1.0);
        assert!(look.is_off());
        assert!(look.look().is_none());
        let from_nothing: LookLut = serde_json::from_str("{}").unwrap();
        assert_eq!(from_nothing, LookLut::default());
        // And half of the section written out reads with the rest at
        // its default.
        let half: LookLut = serde_json::from_str("{\"strength\": 0.5}").unwrap();
        assert_eq!(half.strength, 0.5);
        assert!(half.lut.is_none());
    }

    /// The section arrived with a default, so no schema bump: a
    /// sidecar written before it reads unchanged and says no look.
    #[test]
    fn a_sidecar_without_the_section_reads_as_no_look() {
        let old = r#"{"version": 3, "light": {"exposure": 0.5}}"#;
        let edit = crate::Edit::from_json(old).unwrap();
        assert_eq!(edit.look_lut, LookLut::default());
        assert_eq!(edit.light.exposure, 0.5);
        // A version 1 sidecar, migrated, likewise.
        let ancient = r#"{"light": {"exposure": 0.5}}"#;
        assert_eq!(
            crate::Edit::from_json(ancient).unwrap().look_lut,
            LookLut::default()
        );
    }

    #[test]
    fn the_section_round_trips_in_a_sidecar_under_the_key_look() {
        let edit = crate::Edit {
            look_lut: LookLut {
                lut: LutChoice::Named("Slide Warm".into()),
                strength: 0.6,
            },
            ..crate::Edit::default()
        };
        let json = edit.to_json();
        assert!(json.contains("\"look\""), "{json}");
        assert!(json.contains("\"Slide Warm\""), "{json}");
        let back = crate::Edit::from_json(&json).unwrap();
        assert_eq!(back.look_lut, edit.look_lut);
        // A look is a finish, not a develop: choosing one does not
        // send the picture back through the engine.
        assert!(back.same_develop(&crate::Edit::default()));
    }

    /// A film preset is a look and a strength, so a preset carries the
    /// section, and carries it without being asked.
    #[test]
    fn a_preset_carries_the_look() {
        use crate::preset::Section;
        assert!(Section::Look.by_default());
        let from = crate::Edit {
            look_lut: LookLut {
                lut: LutChoice::Named("Slide Warm".into()),
                strength: 0.4,
            },
            ..crate::Edit::default()
        };
        let mut onto = crate::Edit::default();
        onto.light.exposure = 1.0;
        Section::Look.copy(&from, &mut onto);
        assert_eq!(onto.look_lut, from.look_lut);
        // And nothing else came with it.
        assert_eq!(onto.light.exposure, 1.0);
        assert!(!Section::Look.same(&from, &crate::Edit::default()));
    }

    #[test]
    fn a_name_that_is_a_path_is_refused() {
        assert!(path_for("../../etc/passwd").is_none());
        assert!(path_for("sub/dir").is_none());
        assert!(path_for("/etc/passwd").is_none());
        assert!(path_for("").is_none());
    }

    #[test]
    fn a_label_prefers_the_tables_own_name() {
        let mut entry = Entry {
            name: "slide-warm".into(),
            path: PathBuf::new(),
            title: Some("Slide Warm".into()),
            size: 33,
            kind: Kind::Cube3d,
            encoding: Encoding::default(),
            primaries: Primaries::default(),
        };
        assert_eq!(entry.label(), "Slide Warm");
        assert_eq!(entry.described(), "3D .cube, 33 nodes");
        entry.title = None;
        assert_eq!(entry.label(), "slide-warm");
        // A table that is not the usual sRGB says so.
        entry.encoding = Encoding::Linear;
        assert_eq!(entry.described(), "3D .cube, 33 nodes, linear in sRGB");
    }

    /// The cache is one static for the whole process, and these tests
    /// say what is in it, so they take their turn rather than run
    /// alongside each other.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("greycard-look-{what}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_table_file_reads_once_and_is_kept() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("ok");
        let path = dir.join("Flat.cube");
        std::fs::write(&path, flat_cube([1.0, 0.0, 0.0])).unwrap();
        let table = load(&path).expect("it reads");
        assert_eq!(table.title.as_deref(), Some("Flat"));
        assert!(Arc::ptr_eq(&table, &load(&path).unwrap()));
        // Rewritten, it is read afresh: the length alone says so.
        std::fs::write(&path, flat_cube([0.125, 0.0, 0.0])).unwrap();
        let again = load(&path).expect("the new one reads");
        assert!(!Arc::ptr_eq(&table, &again));
        forget();
        assert!(!Arc::ptr_eq(&again, &load(&path).unwrap()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_that_is_not_a_table_resolves_to_nothing() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("junk");
        let path = dir.join("not-a-lut.cube");
        std::fs::write(&path, b"this is not a lookup table").unwrap();
        assert!(load(&path).is_none());
        // Twice: the second answer comes from the cache.
        assert!(load(&path).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_table_is_remembered_as_missing() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("missing");
        let path = dir.join("Not Here Yet.cube");
        assert!(load(&path).is_none());
        assert!(matches!(
            CACHE.lock().unwrap().get(&path),
            Some(Cached::Missing)
        ));
        assert!(load(&path).is_none());
        std::fs::write(&path, flat_cube([0.5, 0.5, 0.5])).unwrap();
        assert!(load(&path).is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The directory the panel lists, and what a header read gives it.
    #[test]
    fn the_directory_lists_what_it_holds() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("list");
        std::fs::write(dir.join("Flat.cube"), flat_cube([1.0, 0.0, 0.0])).unwrap();
        std::fs::write(dir.join("notes.txt"), "not a look").unwrap();
        let listed = lut::list_dir(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, "Flat");
        assert_eq!(listed[0].2.as_ref().unwrap().size, 2);
        // A header read keeps nothing: the table is only built for the
        // one that is chosen.
        assert!(!CACHE.lock().unwrap().contains_key(&listed[0].1));
        std::fs::remove_dir_all(&dir).ok();
    }
}
