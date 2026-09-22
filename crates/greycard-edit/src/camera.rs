//! The camera profile an edit develops with: the file's own, or a DCP
//! from the user's profile directory.
//!
//! The choice lives in the edit because it changes the picture and a
//! preset should be able to carry it (notes §78). The engine takes the
//! resolved profile, not a name: reading the file is this crate's job,
//! and it is done once per file and kept, so every develop of the same
//! edit — the viewport's and the export's — is handed the same profile.
//!
//! A name that resolves to nothing is a warning in the log and the
//! file's own calibrations. A missing profile never fails a develop.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use greycard_core::Dcp;
use greycard_core::color::Profile;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
    /// A DCP in the profile directory, by file name without the
    /// extension.
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
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Camera {
    pub profile: ProfileChoice,
}

impl Camera {
    /// The engine's profile for this choice: the DCP read and kept, or
    /// `None` for the file's own. A profile that will not read warns
    /// once and comes back `None`, which is the file's own again.
    pub fn profile(&self) -> Option<Arc<Profile>> {
        let ProfileChoice::Named(name) = &self.profile else {
            return None;
        };
        let path = path_for(name)?;
        load(&path)
    }
}

/// Where the profiles live: `$XDG_DATA_HOME/greycard/profiles` when
/// that is set, else `greycard/profiles` under the platform's data
/// directory — `~/.local/share` on Linux, `~/Library/Application
/// Support` on macOS, `%APPDATA%` on Windows (notes §106).
pub fn store_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::data_dir)?;
    Some(base.join("greycard").join("profiles"))
}

/// Where a profile of this name is kept. A name that is a path, or
/// tries to climb out of the directory, is refused.
pub fn path_for(name: &str) -> Option<PathBuf> {
    if name.is_empty()
        || name.contains(['/', '\\'])
        || name.contains("..")
        || Path::new(name).is_absolute()
    {
        log::warn!("camera profile {name:?} is not a name in the profile directory");
        return None;
    }
    Some(store_dir()?.join(format!("{name}.{EXTENSION}")))
}

/// A profile in the directory, as the panel lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The file name without its extension: what the edit stores.
    pub name: String,
    pub path: PathBuf,
    /// The profile's own name, when it carries one.
    pub title: Option<String>,
    /// The camera it was made for, when it says.
    pub unique_camera_model: Option<String>,
    pub copyright: Option<String>,
}

impl Entry {
    /// What the panel shows: the profile's own name when it has one
    /// and it is not simply the file's name again.
    pub fn label(&self) -> &str {
        match &self.title {
            Some(title) if !title.is_empty() && title != &self.name => title,
            _ => &self.name,
        }
    }

    /// Whether this profile was made for a camera named `make` and
    /// `model`.
    ///
    /// DCPs name their camera with DNG's `UniqueCameraModel`, which is
    /// the maker's name and the model together ("Canon EOS 6D Mark
    /// II"), while a decoded frame carries them apart and cleaned up,
    /// and some decoders leave the make on the front of the model.
    /// Both sides are folded to their letters and digits alone, and
    /// the profile fits when the folded unique model is the frame's
    /// make and model together or its model alone. Equality, not
    /// containment: "CANON EOS R6" would otherwise fit an EOS R6 Mark
    /// II. A profile that names no camera fits everything, since it
    /// cannot say otherwise; one whose maker writes the model its own
    /// way does not fit, and is shown with the warning rather than
    /// hidden.
    pub fn fits(&self, make: &str, model: &str) -> bool {
        let Some(unique) = self.unique_camera_model.as_deref() else {
            return true;
        };
        let unique = folded(unique);
        unique.is_empty() || unique == folded(&format!("{make} {model}")) || unique == folded(model)
    }
}

fn folded(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Every profile in the directory, by name.
///
/// Only each file's header is read — its name, its camera and its
/// copyright — not its tables: a directory of a dozen profiles with
/// look tables is megabytes of floats nobody has asked for yet, and
/// this runs on the thread that draws the panel. The profile that is
/// chosen is read in full by [`load`], once. A file that will not read
/// is passed over with a word in the log.
pub fn list() -> Vec<Entry> {
    // A listing is also the moment to let go of what was read before
    // it: a profile that has been replaced or is no longer wanted has
    // no claim on the memory its tables take.
    forget();
    let Some(dir) = store_dir() else {
        return Vec::new();
    };
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = read
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|x| x.eq_ignore_ascii_case(EXTENSION))
        })
        .filter_map(|path| {
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let info = match Dcp::info(&path) {
                Ok(info) => info,
                Err(e) => {
                    log::warn!("camera profile {}: {e}", path.display());
                    return None;
                }
            };
            Some(Entry {
                name,
                path,
                title: info.name,
                unique_camera_model: info.unique_camera_model,
                copyright: info.copyright,
            })
        })
        .collect();
    out.sort_by_key(|e| e.label().to_lowercase());
    out
}

/// What a path read to last time.
enum Cached {
    /// There was no file to read, and it has been said once.
    Missing,
    /// A file of this age and size read to this, or to nothing.
    Read {
        stamp: Option<std::time::SystemTime>,
        len: u64,
        profile: Option<Arc<Profile>>,
    },
}

/// How many profiles are kept at once. One edit names one, and a
/// filmstrip of files naming different ones is the only way to reach
/// even a handful; past this the lot is dropped rather than grown.
const KEEP: usize = 8;

static CACHE: LazyLock<Mutex<HashMap<PathBuf, Cached>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The profile in a file, read once and kept until the file changes.
///
/// A file that is missing, or that will not read, is remembered as
/// such, so the warning is said once rather than on every develop.
pub fn load(path: &Path) -> Option<Arc<Profile>> {
    let stamped = match std::fs::metadata(path) {
        Ok(m) => Some((m.modified().ok(), m.len())),
        Err(_) => None,
    };
    let mut cache = CACHE.lock().ok()?;
    match (cache.get(path), &stamped) {
        // Missing, and already said so.
        (Some(Cached::Missing), None) => return None,
        // Read before, from a file that has not been touched since.
        (
            Some(Cached::Read {
                stamp,
                len,
                profile,
            }),
            Some((now, size)),
        ) if stamp == now && len == size => {
            return profile.clone();
        }
        _ => {}
    }
    if cache.len() >= KEEP {
        cache.clear();
    }
    let Some((stamp, len)) = stamped else {
        log::warn!("camera profile {}: it is not there", path.display());
        cache.insert(path.to_path_buf(), Cached::Missing);
        return None;
    };
    let profile = match Dcp::load(path).and_then(|dcp| dcp.profile()) {
        Ok(profile) => Some(Arc::new(profile)),
        Err(e) => {
            log::warn!("camera profile {}: {e}", path.display());
            None
        }
    };
    cache.insert(
        path.to_path_buf(),
        Cached::Read {
            stamp,
            len,
            profile: profile.clone(),
        },
    );
    profile
}

/// Forget what has been read, so the next develop reads it again.
pub fn forget() {
    if let Ok(mut cache) = CACHE.lock() {
        cache.clear();
    }
}

/// What is wrong with the chosen profile, if anything: that it is not
/// there, or that it was made for another camera. The develop falls
/// back to the file's own calibrations either way.
pub fn warning(profiles: &[Entry], camera: &(String, String), chosen: &str) -> String {
    let name = chosen;
    if chosen == EMBEDDED {
        return String::new();
    }
    let Some(entry) = profiles.iter().find(|e| e.name == name) else {
        return format!("{name} is not in the profile directory; the file's own colors are used.");
    };
    let (make, model) = (camera.0.as_str(), camera.1.as_str());
    if make.is_empty() || entry.fits(make, model) {
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

    /// What the panel says under the profile list: nothing for the
    /// file's own colors or a profile that fits, a line for one the
    /// directory has not got, and a line for one made elsewhere.
    #[test]
    fn a_profile_that_does_not_fit_says_so_and_a_missing_one_says_so() {
        let camera = ("Canon".to_string(), "EOS R5".to_string());
        let mine = Entry {
            name: "mine".into(),
            path: std::path::PathBuf::new(),
            title: None,
            copyright: None,
            unique_camera_model: Some("Canon EOS R5".into()),
        };
        let theirs = Entry {
            name: "theirs".into(),
            unique_camera_model: Some("NIKON Z 8".into()),
            ..mine.clone()
        };
        let profiles = vec![mine.clone(), theirs.clone()];
        // The file's own colors: nothing to say.
        assert!(warning(&profiles, &camera, EMBEDDED).is_empty());
        // A profile for this camera: nothing to say.
        assert!(warning(&profiles, &camera, "mine").is_empty());
        // One made for another: the panel still shows it chosen, and
        // says what it was made for.
        let said = warning(&profiles, &camera, "theirs");
        assert!(
            said.contains("NIKON Z 8") && said.contains("EOS R5"),
            "{said}"
        );
        // One the directory has not got at all: a sidecar or a preset
        // from another machine.
        let said = warning(&profiles, &camera, "elsewhere");
        assert!(said.contains("not in the profile directory"), "{said}");
        // No camera named yet: nothing can be said about the fit.
        let unknown = (String::new(), String::new());
        assert!(warning(&profiles, &unknown, "theirs").is_empty());
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

    #[test]
    fn a_name_that_is_a_path_is_refused() {
        assert!(path_for("../../etc/passwd").is_none());
        assert!(path_for("sub/dir").is_none());
        assert!(path_for("/etc/passwd").is_none());
        assert!(path_for("").is_none());
    }

    #[test]
    fn a_profile_fits_the_camera_it_names() {
        let entry = |unique: Option<&str>| Entry {
            name: "Some Profile".into(),
            path: PathBuf::new(),
            title: None,
            unique_camera_model: unique.map(str::to_string),
            copyright: None,
        };
        assert!(entry(Some("CANON EOS R6")).fits("Canon", "EOS R6"));
        assert!(entry(Some("Canon EOS 6D Mark II")).fits("Canon", "EOS 6D Mark II"));
        // The frame's own make and model concatenated are the same
        // name whether or not the decoder repeats the make.
        assert!(entry(Some("NIKON Z 6")).fits("Nikon", "NIKON Z 6"));
        assert!(!entry(Some("Canon EOS R6")).fits("Canon", "EOS R6 Mark II"));
        assert!(!entry(Some("Canon EOS R6")).fits("Nikon", "Z 6"));
        // A profile that names no camera cannot be said to be wrong.
        assert!(entry(None).fits("Canon", "EOS R6"));
    }

    #[test]
    fn a_label_prefers_the_profiles_own_name() {
        let mut entry = Entry {
            name: "canon-r6-neutral".into(),
            path: PathBuf::new(),
            title: Some("Canon EOS R6 Neutral".into()),
            unique_camera_model: None,
            copyright: None,
        };
        assert_eq!(entry.label(), "Canon EOS R6 Neutral");
        entry.title = None;
        assert_eq!(entry.label(), "canon-r6-neutral");
    }

    /// A small but real DCP, written by the engine's own writer.
    /// `hues` changes both what is in it and how long it is, so a
    /// rewrite is a different file whatever the clock's resolution.
    fn a_profile_file(path: &Path, hues: usize) {
        use greycard_core::dcp::{DcpCalibration, EmbedPolicy, HueSatMap};
        let matrix = [0.8, -0.1, -0.1, -0.4, 1.3, 0.1, 0.0, -0.2, 0.9];
        let dcp = Dcp {
            name: Some("Test Camera Neutral".into()),
            unique_camera_model: Some("Test Cam".into()),
            copyright: Some("public domain".into()),
            calibration_signature: None,
            embed_policy: EmbedPolicy::AllowCopying,
            primary: DcpCalibration {
                illuminant: 17,
                color_matrix: matrix,
                forward_matrix: None,
                hue_sat_map: Some(HueSatMap::identity(hues, 2, 1)),
            },
            secondary: None,
            baseline_exposure_offset: None,
            look: None,
            tone_curve: None,
        };
        std::fs::write(path, dcp.to_bytes()).unwrap();
    }

    /// The cache is one static for the whole process, and these tests
    /// say what is in it, so they take their turn rather than run
    /// alongside each other.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    /// A directory of this test's own, gone when it returns.
    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("greycard-dcp-{what}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_profile_file_reads_to_a_profile_and_is_kept() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("ok");
        let path = dir.join("Test Cam Neutral.dcp");
        a_profile_file(&path, 4);

        let profile = load(&path).expect("the profile reads");
        assert_eq!(profile.name.as_deref(), Some("Test Camera Neutral"));
        assert!(profile.maps.is_some(), "it carries the map");
        // The second read is the same profile, not another copy of it.
        assert!(Arc::ptr_eq(&profile, &load(&path).unwrap()));

        // Rewritten with a map of another size, it is read afresh: the
        // length alone says so, without leaning on the file clock.
        a_profile_file(&path, 8);
        let again = load(&path).expect("the new one reads");
        assert!(!Arc::ptr_eq(&profile, &again));
        assert_eq!(again.maps.as_ref().unwrap().primary.hue_divisions, 8);

        // And forgetting makes the next read a fresh one too.
        forget();
        assert!(!Arc::ptr_eq(&again, &load(&path).unwrap()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_embedded_choice_asks_the_engine_for_nothing() {
        assert!(Camera::default().profile().is_none());
        assert!(crate::Edit::default().settings().profile.is_none());
    }

    #[test]
    fn a_file_that_is_not_a_profile_resolves_to_nothing() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("junk");
        let path = dir.join("not-a-profile.dcp");
        std::fs::write(&path, b"this is not a camera profile").unwrap();
        assert!(load(&path).is_none());
        // Twice: the second answer comes from the cache, and is the
        // same nothing.
        assert!(load(&path).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A profile the edit names and the directory has not got is
    /// remembered as missing, so the develop that follows every slider
    /// does not say so again; and it is read the moment it appears.
    #[test]
    fn a_missing_profile_is_remembered_as_missing() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("missing");
        let path = dir.join("Not Here Yet.dcp");
        assert!(load(&path).is_none());
        assert!(matches!(
            CACHE.lock().unwrap().get(&path),
            Some(Cached::Missing)
        ));
        assert!(load(&path).is_none());

        // Put it there and it is read, missing marker and all.
        a_profile_file(&path, 4);
        assert!(load(&path).is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The listing reads headers, not tables: a profile in the
    /// directory is named without its map being built.
    #[test]
    fn listing_does_not_read_the_tables() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("list");
        let path = dir.join("Test Cam Neutral.dcp");
        a_profile_file(&path, 4);
        forget();
        let info = Dcp::info(&path).unwrap();
        assert_eq!(info.name.as_deref(), Some("Test Camera Neutral"));
        assert_eq!(info.unique_camera_model.as_deref(), Some("Test Cam"));
        assert!(
            !CACHE.lock().unwrap().contains_key(&path),
            "a header read keeps nothing"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
