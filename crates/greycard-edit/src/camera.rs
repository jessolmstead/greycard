//! The camera profile an edit develops with: the file's own, or a DCP
//! from the user's profile directory.
//!
//! The choice lives in the edit because it changes the picture and a
//! preset should be able to carry it (notes §78). The engine takes the
//! resolved profile, not a name: reading the file is the
//! [`crate::Loader`]'s the caller hands in (greycard-edit-io's stores,
//! which list the directory too), done once per file and kept, so every
//! develop of the same edit — the viewport's and the export's — is
//! handed the same profile.
//!
//! A name that resolves to nothing is a warning in the log and the
//! file's own calibrations. A missing profile never fails a develop.
//!
//! A DCP's look table goes on with its map ([`Camera::look_table`]).
//! An edit written before the table was applied says nothing about it
//! and reads as off, so its picture does not move and it writes back
//! byte for byte, with no new schema version (as a mask's local white
//! and its Detail came in). Every door that lays a Camera section
//! naming a DCP on a frame turns the table on ([`Camera::take`]): a
//! preset, a paste, a sync, or an import's preset, whatever the frame
//! had and whatever the source says; so does choosing a different
//! profile ([`Camera::choose`]). The only DCP edit without its table is
//! an old one nothing has laid a Camera section on; an undo or a
//! snapshot restores the edit whole, switch and all.

use std::path::PathBuf;
use std::sync::Arc;

use greycard_core::color::Profile;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Loader;

/// The file extension of a camera profile.
pub const EXTENSION: &str = "dcp";

/// The word for the file's own calibrations, in the sidecar and on the
/// panel.
pub const EMBEDDED: &str = "embedded";

/// Which camera profile develops the picture.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProfileChoice {
    /// The matrices the raw file carries.
    #[default]
    Embedded,
    /// A DCP in the profile directory, by its path under it without
    /// the extension, folders joined by `/` on every system: a file at
    /// the top is its bare file name, as every edit made before the
    /// folder was read with its subfolders names it.
    Named(String),
}

impl ProfileChoice {
    /// The name as the sidecar writes it.
    pub fn name(&self) -> &str {
        match self {
            ProfileChoice::Embedded => EMBEDDED,
            ProfileChoice::Named(name) => name,
        }
    }

    /// A name from the panel or a sidecar. Anything empty, or the word
    /// for the file's own, is [`ProfileChoice::Embedded`].
    pub fn from_name(name: &str) -> Self {
        let name = name.trim();
        if name.is_empty() || name.eq_ignore_ascii_case(EMBEDDED) {
            ProfileChoice::Embedded
        } else {
            ProfileChoice::Named(name.to_string())
        }
    }

    pub fn is_embedded(&self) -> bool {
        matches!(self, ProfileChoice::Embedded)
    }
}

impl Serialize for ProfileChoice {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for ProfileChoice {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(ProfileChoice::from_name(&String::deserialize(d)?))
    }
}

/// The CAMERA section of an edit: what the sensor's color is taken
/// through, before everything else.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct Camera {
    pub profile: ProfileChoice,
    /// Whether a DCP's look table goes on with its map: off when the
    /// sidecar says nothing, which is every edit made before the table
    /// was applied (the module's doc).
    pub look_table: bool,
}

/// The switch is written only when it is on and a DCP is named, so an
/// edit from before the table writes the bytes it always wrote, and its
/// develop key, its thumbnail, and its masks' bases stay as they were.
impl Serialize for Camera {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let written = self.look_table && !self.profile.is_embedded();
        let mut st = s.serialize_struct("Camera", 1 + usize::from(written))?;
        st.serialize_field("profile", &self.profile)?;
        if written {
            st.serialize_field("look_table", &true)?;
        } else {
            st.skip_field("look_table")?;
        }
        st.end()
    }
}

impl Camera {
    /// Choose a profile on the panel. Choosing the one already chosen
    /// changes nothing, so an old edit keeps its picture; any other
    /// choice is [`Camera::take`].
    pub fn choose(&mut self, profile: ProfileChoice) {
        if profile != self.profile {
            self.take(&Camera {
                profile,
                look_table: false,
            });
        }
    }

    /// Lay `from` over this one: a preset, a paste, a sync. A DCP comes
    /// with its look table on, whatever this one had and whatever
    /// `from` says; the file's own profile has no table, and its switch
    /// is off.
    pub fn take(&mut self, from: &Camera) {
        self.profile = from.profile.clone();
        self.look_table = !self.profile.is_embedded();
    }

    /// Whether a look table goes on: the switch, and a DCP to carry one.
    /// A hand-edited sidecar can set the switch on the file's own
    /// profile; that is no table.
    pub fn applies_look_table(&self) -> bool {
        self.look_table && !self.profile.is_embedded()
    }

    /// Whether laying `from` over this one would leave it as it is: how
    /// a preset reads as applied.
    pub fn has(&self, from: &Camera) -> bool {
        let mut laid = self.clone();
        laid.take(from);
        laid == *self
    }

    /// The engine's profile for this choice: the DCP `loader` reads
    /// and keeps, or `None` for the file's own. A profile that will not
    /// read warns once and comes back `None`, which is the file's own
    /// again.
    pub fn profile(&self, loader: &dyn Loader) -> Option<Arc<Profile>> {
        let ProfileChoice::Named(name) = &self.profile else {
            return None;
        };
        loader.profile(name)
    }
}

/// A profile in the directory, as the panel lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The file's path under the profile directory without its
    /// extension, folders joined by `/`: what the edit stores
    /// ([`ProfileChoice::Named`]).
    pub name: String,
    pub path: PathBuf,
    /// The profile's own name, when it carries one.
    pub title: Option<String>,
    /// The camera it was made for, when it says.
    pub unique_camera_model: Option<String>,
    pub copyright: Option<String>,
    /// The entry this file is the same profile as, byte for byte under
    /// another file name: that one stands for both on the panel. The
    /// file is still an entry, so an edit naming it still finds it.
    pub same_as: Option<String>,
}

impl Entry {
    /// What the panel shows: the profile's own name when it has one
    /// and it is not simply the file's name again. Two that differ but
    /// share it are told apart by the panel, which knows which rows
    /// stand side by side.
    pub fn label(&self) -> &str {
        match &self.title {
            Some(title) if !title.is_empty() && title != self.file_name() => title,
            _ => self.file_name(),
        }
    }

    /// The file's name without its extension or its folders.
    pub fn file_name(&self) -> &str {
        self.name.rsplit('/').next().unwrap_or(&self.name)
    }

    /// Whether this profile was made for `body`: its folded
    /// `UniqueCameraModel` is one of the folded [`Body::names`].
    ///
    /// DCPs name their camera with DNG's `UniqueCameraModel`, which is
    /// the maker's name and the model together ("Canon EOS 6D Mark
    /// II"), while a decoded frame carries them apart and cleaned up,
    /// and some decoders leave the make on the front of the model.
    /// Both sides are folded to their letters and digits alone, and
    /// the profile fits when the folded unique model is any of the
    /// body's names, its own or one it is sold under elsewhere.
    /// Equality, not containment: "CANON EOS R6" would otherwise fit
    /// an EOS R6 Mark II. A profile that names no camera fits
    /// everything, since it cannot say otherwise; one whose maker
    /// writes the model its own way does not fit, and is shown with
    /// the warning rather than hidden.
    pub fn fits(&self, body: Body) -> bool {
        let unique = self.folded_unique();
        unique.is_empty() || body.names().iter().any(|n| folded(n) == unique)
    }

    /// The camera the profile names, folded to its letters and digits
    /// alone: empty for one that names none.
    pub fn folded_unique(&self) -> String {
        self.unique_camera_model
            .as_deref()
            .map(folded)
            .unwrap_or_default()
    }
}

/// A camera as a profile's fit reads it: its make and model, and the
/// other names the same body is sold under (an EOS 850D is also an
/// EOS Rebel T8i).
#[derive(Debug, Clone, Copy, Default)]
pub struct Body<'a> {
    pub make: &'a str,
    pub model: &'a str,
    pub other_names: &'a [String],
}

impl<'a> Body<'a> {
    pub fn new(make: &'a str, model: &'a str, other_names: &'a [String]) -> Body<'a> {
        Body {
            make,
            model,
            other_names,
        }
    }

    /// The names a profile is matched against exactly, in order: the
    /// make and model, the model, then the make and each other name,
    /// and each other name.
    pub fn names(&self) -> Vec<String> {
        let mut out = Vec::new();
        for m in std::iter::once(self.model).chain(self.other_names.iter().map(String::as_str)) {
            out.push(format!("{} {m}", self.make));
            out.push(m.to_string());
        }
        out
    }
}

fn folded(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The entries the panel lists, one a profile: an entry that is the
/// same profile as another is left out, unless it is the one `chosen`
/// names, which then stands for the pair in the other's place so the
/// list shows what the edit names.
pub fn listed<'a>(entries: &'a [Entry], chosen: &str) -> impl Iterator<Item = &'a Entry> {
    let instead = entries
        .iter()
        .find(|e| e.name == chosen)
        .and_then(|e| e.same_as.as_deref());
    entries.iter().filter(move |e| match &e.same_as {
        Some(_) => e.name == chosen,
        None => Some(e.name.as_str()) != instead,
    })
}

/// What the panel shows for each of `rows`, which stand side by side:
/// each profile's label, and when two different profiles go by one
/// label, the file name beside each to tell them apart, or the path
/// under the directory when the file names are one too.
pub fn told_apart(rows: &[&Entry]) -> Vec<String> {
    rows.iter()
        .map(|e| {
            let label = e.label();
            if rows.iter().filter(|o| o.label() == label).count() < 2 {
                return label.to_string();
            }
            let file = e.file_name();
            let same_file = rows
                .iter()
                .filter(|o| o.label() == label && o.file_name() == file)
                .count()
                > 1;
            let by = if same_file { e.name.as_str() } else { file };
            match by == label {
                true => label.to_string(),
                false => format!("{label} ({by})"),
            }
        })
        .collect()
}

/// Where the list of every camera's profiles is drawn from.
#[derive(Debug, Clone, Copy)]
pub struct Listing<'a> {
    /// What the edit names.
    pub chosen: &'a str,
    /// The open frame's camera; no make for none.
    pub body: Body<'a>,
    /// The groups the user has opened (true) or folded, by key.
    pub open: &'a std::collections::BTreeMap<String, bool>,
}

/// A row of the list of every camera's profiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A camera's heading, over its profiles unless folded.
    Heading {
        /// The camera folded to its letters and digits: what a click
        /// on the heading toggles.
        key: String,
        /// The camera as its profiles name it.
        label: String,
        count: usize,
        folded: bool,
        /// The open frame's own camera, under any of its names.
        own: bool,
        /// It holds the chosen profile, so it is open whatever it was
        /// set to.
        locked: bool,
    },
    Profile {
        /// What the edit stores.
        name: String,
        label: String,
        /// Under a heading.
        indented: bool,
    },
}

/// The directory's profiles grouped by the camera each names, one row
/// a profile however many files hold it ([`listed`]): first the open
/// frame's camera's groups, under each name it is sold by, open and
/// marked; then every other camera by name, folded with its count; then
/// the profiles that name no camera, under no heading. A group the
/// user opened or folded stays so ([`Listing::open`]), and the group
/// holding the chosen profile is open whatever it was, so the list
/// shows what the edit says.
pub fn grouped(entries: &[Entry], at: Listing) -> Vec<Row> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<String, (String, Vec<&Entry>)> = BTreeMap::new();
    let mut loose: Vec<&Entry> = Vec::new();
    for e in listed(entries, at.chosen) {
        let key = e.folded_unique();
        if key.is_empty() {
            loose.push(e);
            continue;
        }
        let label = e.unique_camera_model.as_deref().unwrap_or_default().trim();
        groups
            .entry(key)
            .or_insert_with(|| (label.to_string(), Vec::new()))
            .1
            .push(e);
    }
    let has_body = !at.body.make.is_empty() || !at.body.model.is_empty();
    let mut order: Vec<(String, String, Vec<&Entry>, bool)> = groups
        .into_iter()
        .map(|(key, (label, members))| {
            let own = has_body && members.iter().any(|e| e.fits(at.body));
            (key, label, members, own)
        })
        .collect();
    order.sort_by_cached_key(|(key, label, _, own)| (!own, label.to_lowercase(), key.clone()));
    let mut rows = Vec::new();
    for (key, label, members, own) in order {
        let locked = members.iter().any(|e| e.name == at.chosen);
        let folded = at.open.get(&key).map_or(!own, |open| !open) && !locked;
        rows.push(Row::Heading {
            key,
            label,
            count: members.len(),
            folded,
            own,
            locked,
        });
        if !folded {
            let labels = told_apart(&members);
            rows.extend(members.iter().zip(labels).map(|(e, label)| Row::Profile {
                name: e.name.clone(),
                label,
                indented: true,
            }));
        }
    }
    let labels = told_apart(&loose);
    rows.extend(loose.iter().zip(labels).map(|(e, label)| Row::Profile {
        name: e.name.clone(),
        label,
        indented: false,
    }));
    rows
}

/// What is wrong with the chosen profile, if anything: that it is not
/// there, or that it was made for another camera. The develop falls
/// back to the file's own calibrations either way.
pub fn warning(profiles: &[Entry], camera: Body, chosen: &str) -> String {
    let name = chosen;
    if chosen == EMBEDDED {
        return String::new();
    }
    let Some(entry) = profiles.iter().find(|e| e.name == name) else {
        return format!("{name} is not in the profile directory; the file's own colors are used.");
    };
    let (make, model) = (camera.make, camera.model);
    if make.is_empty() || entry.fits(camera) {
        return String::new();
    }
    let made_for = entry
        .unique_camera_model
        .as_deref()
        .unwrap_or("another camera");
    format!("Made for {made_for}, not {make} {model}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An edit from before the table, one that names a DCP or not,
    /// reads with the table off and writes back byte for byte; a new
    /// DCP edit writes the switch.
    #[test]
    fn an_edit_from_before_the_table_writes_back_byte_for_byte() {
        for old in [r#"{"profile":"Body Natural"}"#, r#"{"profile":"embedded"}"#] {
            let c: Camera = serde_json::from_str(old).unwrap();
            assert!(!c.look_table);
            assert_eq!(serde_json::to_string(&c).unwrap(), old);
        }
        let old =
            r#"{"version": 8, "camera": {"profile": "Body Natural"}, "light": {"exposure": 0.5}}"#;
        let e = crate::Edit::from_json(old).unwrap();
        assert!(!e.camera.look_table);
        assert!(!e.settings(&crate::NoStores).profile_look);
        let json = e.to_json();
        assert!(!json.contains("look_table"), "{json}");
        let mut again = crate::Edit::from_json(&json).unwrap();
        assert_eq!(again.to_json(), json);

        again
            .camera
            .choose(ProfileChoice::Named("Body Vivid".into()));
        let json = serde_json::to_string(&again.camera).unwrap();
        assert_eq!(json, r#"{"profile":"Body Vivid","look_table":true}"#);
        assert_eq!(serde_json::from_str::<Camera>(&json).unwrap(), again.camera);
        assert!(
            crate::Edit::from_json(&again.to_json())
                .unwrap()
                .camera
                .look_table
        );
    }

    /// A hand-edited switch on the file's own profile is no table:
    /// the develop does not apply one, and the engine and a mask's base
    /// ask the same question.
    #[test]
    fn a_switch_on_the_files_own_profile_is_no_table() {
        let e = crate::Edit::from_json(
            r#"{"version": 8, "camera": {"profile": "embedded", "look_table": true}}"#,
        )
        .unwrap();
        assert!(e.camera.look_table);
        assert!(!e.camera.applies_look_table());
        assert!(!e.settings(&crate::NoStores).profile_look);
        let mut named = e.camera.clone();
        named.profile = ProfileChoice::Named("Natural".into());
        assert!(named.applies_look_table());
    }

    /// A brand-new edit is on the file's own profile, which has no
    /// table.
    #[test]
    fn a_new_edit_has_no_table_to_apply() {
        let e = crate::Edit::default();
        assert!(e.camera.profile.is_embedded());
        assert!(!e.camera.look_table);
    }

    /// Choosing another profile turns the table on; choosing the same
    /// one again leaves an old edit's picture as it was; the file's own
    /// profile turns it off.
    #[test]
    fn choosing_another_profile_turns_the_look_table_on() {
        let natural = ProfileChoice::Named("Natural".into());
        let mut c = Camera {
            profile: natural.clone(),
            look_table: false,
        };
        c.choose(natural.clone());
        assert!(!c.look_table);
        c.choose(ProfileChoice::Named("Vivid".into()));
        assert!(c.look_table);
        c.choose(ProfileChoice::Embedded);
        assert!(!c.look_table);
        c.choose(natural);
        assert!(c.look_table);
    }

    /// What the panel says under the profile list: nothing for the
    /// file's own colors or a profile that fits, a line for one the
    /// directory has not got, and a line for one made elsewhere.
    #[test]
    fn a_profile_that_does_not_fit_says_so_and_a_missing_one_says_so() {
        let camera = Body::new("Canon", "EOS R5", &[]);
        let mine = Entry {
            name: "mine".into(),
            path: std::path::PathBuf::new(),
            title: None,
            copyright: None,
            unique_camera_model: Some("Canon EOS R5".into()),
            same_as: None,
        };
        let theirs = Entry {
            name: "theirs".into(),
            unique_camera_model: Some("NIKON Z 8".into()),
            ..mine.clone()
        };
        let profiles = vec![mine.clone(), theirs.clone()];
        // The file's own colors: nothing to say.
        assert!(warning(&profiles, camera, EMBEDDED).is_empty());
        // A profile for this camera: nothing to say.
        assert!(warning(&profiles, camera, "mine").is_empty());
        // One made for another: the panel still shows it chosen, and
        // says what it was made for.
        let said = warning(&profiles, camera, "theirs");
        assert!(
            said.contains("NIKON Z 8") && said.contains("EOS R5"),
            "{said}"
        );
        // One the directory has not got at all: a sidecar or a preset
        // from another machine.
        let said = warning(&profiles, camera, "elsewhere");
        assert!(said.contains("not in the profile directory"), "{said}");
        // No camera named yet: nothing can be said about the fit.
        assert!(warning(&profiles, Body::default(), "theirs").is_empty());
    }

    #[test]
    fn a_choice_reads_and_writes_as_a_name() {
        assert_eq!(ProfileChoice::default(), ProfileChoice::Embedded);
        assert_eq!(ProfileChoice::Embedded.name(), "embedded");
        assert_eq!(ProfileChoice::from_name(""), ProfileChoice::Embedded);
        assert_eq!(
            ProfileChoice::from_name("Embedded"),
            ProfileChoice::Embedded
        );
        assert_eq!(
            ProfileChoice::from_name(" Canon EOS R6 "),
            ProfileChoice::Named("Canon EOS R6".into())
        );
        let json = serde_json::to_string(&ProfileChoice::Named("Adobe Standard".into())).unwrap();
        assert_eq!(json, "\"Adobe Standard\"");
        let back: ProfileChoice = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ProfileChoice::Named("Adobe Standard".into()));
        let camera: Camera = serde_json::from_str("{}").unwrap();
        assert_eq!(camera, Camera::default());
    }

    fn entry(unique: Option<&str>) -> Entry {
        Entry {
            name: "Some Profile".into(),
            path: PathBuf::new(),
            title: None,
            unique_camera_model: unique.map(str::to_string),
            copyright: None,
            same_as: None,
        }
    }

    #[test]
    fn a_profile_fits_the_camera_it_names() {
        let fits = |unique: Option<&str>, make: &str, model: &str| {
            entry(unique).fits(Body::new(make, model, &[]))
        };
        assert!(fits(Some("CANON EOS R6"), "Canon", "EOS R6"));
        assert!(fits(
            Some("Canon EOS 6D Mark II"),
            "Canon",
            "EOS 6D Mark II"
        ));
        // The frame's own make and model concatenated are the same
        // name whether or not the decoder repeats the make.
        assert!(fits(Some("NIKON Z 6"), "Nikon", "NIKON Z 6"));
        assert!(!fits(Some("Canon EOS R6"), "Canon", "EOS R6 Mark II"));
        assert!(!fits(Some("Canon EOS R6"), "Nikon", "Z 6"));
        // A profile that names no camera cannot be said to be wrong.
        assert!(fits(None, "Canon", "EOS R6"));
    }

    /// A body sold under other names fits a profile made under any of
    /// them: Adobe names the EOS 850D's profiles for the Rebel T8i.
    #[test]
    fn a_profile_fits_the_body_by_any_of_its_names() {
        let aliases = ["EOS Rebel T8i".to_string(), "EOS Kiss X10i".to_string()];
        let body = Body::new("Canon", "EOS 850D", &aliases);
        for unique in [
            "Canon EOS 850D",
            "Canon EOS Rebel T8i",
            "EOS Rebel T8i",
            "Canon EOS Kiss X10i",
        ] {
            let e = entry(Some(unique));
            assert!(e.fits(body), "{unique}");
        }
        assert!(!entry(Some("Canon EOS Rebel T7i")).fits(body));
        // Without the other names, the Rebel's profile is another body's.
        let bare = Body::new("Canon", "EOS 850D", &[]);
        assert!(!entry(Some("Canon EOS Rebel T8i")).fits(bare));
        assert_eq!(
            body.names(),
            [
                "Canon EOS 850D",
                "EOS 850D",
                "Canon EOS Rebel T8i",
                "EOS Rebel T8i",
                "Canon EOS Kiss X10i",
                "EOS Kiss X10i",
            ]
        );
    }

    /// A short model never fits a longer one's profile.
    #[test]
    fn a_profile_for_a_longer_model_does_not_fit() {
        let r5 = entry(Some("Canon EOS R5"));
        assert!(!r5.fits(Body::new("Canon", "EOS R", &[])));
        assert!(!r5.fits(Body::new("Canon", "Canon EOS R", &[])));
        assert!(!entry(Some("Google Pixel 9 Pro")).fits(Body::new("Google", "Pixel 9", &[])));
    }

    #[test]
    fn a_label_prefers_the_profiles_own_name() {
        let mut entry = Entry {
            name: "canon-r6-neutral".into(),
            path: PathBuf::new(),
            title: Some("Canon EOS R6 Neutral".into()),
            unique_camera_model: None,
            copyright: None,
            same_as: None,
        };
        assert_eq!(entry.label(), "Canon EOS R6 Neutral");
        entry.title = None;
        assert_eq!(entry.label(), "canon-r6-neutral");
        // A file in a subfolder is labeled by its own name, not its
        // folders; a title that is that name again is no title.
        entry.name = "Camera/Canon EOS R6/canon-r6-neutral".into();
        assert_eq!(entry.label(), "canon-r6-neutral");
        entry.title = Some("canon-r6-neutral".into());
        assert_eq!(entry.file_name(), "canon-r6-neutral");
        assert_eq!(entry.label(), "canon-r6-neutral");
    }

    fn named(name: &str, title: &str, unique: Option<&str>) -> Entry {
        Entry {
            name: name.into(),
            title: Some(title.into()),
            ..entry(unique)
        }
    }

    /// Every camera's profiles: the open camera's groups first under
    /// each of its names, open and marked; the rest by name, folded;
    /// the profiles naming no camera last, under no heading. The group
    /// holding the chosen profile is open, and a fold set by hand
    /// holds.
    #[test]
    fn every_cameras_profiles_group_by_camera() {
        let entries = vec![
            named(
                "Camera/Nikon Z 8/Adobe Standard",
                "Adobe Standard",
                Some("NIKON Z 8"),
            ),
            named(
                "Camera/Canon EOS R5/Adobe Standard",
                "Adobe Standard",
                Some("Canon EOS R5"),
            ),
            named(
                "Camera/Canon EOS R5/Neutral",
                "Neutral",
                Some("CANON EOS R5"),
            ),
            named("t8i", "Adobe Standard", Some("Canon EOS Rebel T8i")),
            named("850d", "Adobe Standard", Some("Canon EOS 850D")),
            named("Anything", "Anything", None),
        ];
        let aliases = ["EOS Rebel T8i".to_string()];
        let body = Body::new("Canon", "EOS 850D", &aliases);
        let mut open = std::collections::BTreeMap::new();
        let heads = |rows: &[Row]| -> Vec<(String, bool, bool)> {
            rows.iter()
                .filter_map(|r| match r {
                    Row::Heading {
                        label, folded, own, ..
                    } => Some((label.clone(), *folded, *own)),
                    _ => None,
                })
                .collect()
        };
        let rows = grouped(
            &entries,
            Listing {
                chosen: EMBEDDED,
                body,
                open: &open,
            },
        );
        assert_eq!(
            heads(&rows),
            [
                ("Canon EOS 850D".to_string(), false, true),
                ("Canon EOS Rebel T8i".to_string(), false, true),
                ("Canon EOS R5".to_string(), true, false),
                ("NIKON Z 8".to_string(), true, false),
            ]
        );
        // The loose one last, not set in.
        assert_eq!(
            rows.last(),
            Some(&Row::Profile {
                name: "Anything".into(),
                label: "Anything".into(),
                indented: false,
            })
        );
        // Two spellings of one camera are one group of two.
        assert!(rows.contains(&Row::Heading {
            key: "canoneosr5".into(),
            label: "Canon EOS R5".into(),
            count: 2,
            folded: true,
            own: false,
            locked: false,
        }));
        // Another camera's profile chosen: its group is open.
        let rows = grouped(
            &entries,
            Listing {
                chosen: "Camera/Nikon Z 8/Adobe Standard",
                body,
                open: &open,
            },
        );
        assert!(heads(&rows).contains(&("NIKON Z 8".to_string(), false, false)));
        assert!(rows.iter().any(|r| matches!(r,
            Row::Profile { name, indented: true, .. } if name == "Camera/Nikon Z 8/Adobe Standard")));
        // Folded by hand, the own group folds; opened, another opens.
        open.insert("canoneos850d".to_string(), false);
        open.insert("canoneosr5".to_string(), true);
        let rows = grouped(
            &entries,
            Listing {
                chosen: EMBEDDED,
                body,
                open: &open,
            },
        );
        assert_eq!(heads(&rows)[0], ("Canon EOS 850D".to_string(), true, true));
        assert!(heads(&rows).contains(&("Canon EOS R5".to_string(), false, false)));
        // No frame open: nothing is own, every group by name.
        let none = Listing {
            chosen: EMBEDDED,
            body: Body::default(),
            open: &std::collections::BTreeMap::new(),
        };
        let rows = grouped(&entries, none);
        assert_eq!(heads(&rows)[0].0, "Canon EOS 850D");
        assert!(heads(&rows).iter().all(|(_, folded, own)| *folded && !own));
    }

    /// Two profiles of one label side by side are told apart by their
    /// file names, or by their paths when the file names agree too.
    #[test]
    fn twins_are_told_apart_by_file_name_then_path() {
        let a = named("Camera/A/Adobe Standard", "Standard", Some("X"));
        let b = named("Camera/B/Adobe Standard", "Standard", Some("X"));
        let c = named("old", "Standard", Some("X"));
        let d = named("Other", "Other", Some("X"));
        assert_eq!(
            told_apart(&[&a, &b, &c, &d]),
            [
                "Standard (Camera/A/Adobe Standard)",
                "Standard (Camera/B/Adobe Standard)",
                "Standard (old)",
                "Other",
            ]
        );
        assert_eq!(told_apart(&[&a, &d]), ["Standard", "Other"]);
    }

    #[test]
    fn an_embedded_choice_asks_the_engine_for_nothing() {
        use crate::testing::Unasked;
        assert!(Camera::default().profile(&Unasked).is_none());
        assert!(crate::Edit::default().settings(&Unasked).profile.is_none());
    }
}
