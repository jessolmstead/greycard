//! The develop key: a name for the picture an edit makes, for the
//! pictures kept of it (the thumbnails and the larger picture made from
//! the edit, notes on thumbnails from the edit).
//!
//! It is a hash of what makes the picture and of nothing else: the
//! edit itself, less what never reaches a pixel (an adjustment's name,
//! the schema version it was written at); a recipe number for the
//! pipeline, raised when the same edit comes to make another picture;
//! the version of the learned stages each of its Sky, Landscape and
//! People masks is made by ([`mask_pipelines`]), so a mask made again
//! by a newer rule makes the frames that have one again and no other;
//! the contents of every file outside the sidecar that the develop
//! reads by name: the look's table (a `.cube` or a HaldCLUT, the camera
//! match's fitted tables among them, and its variant for the edit's
//! display curve) and a DCP camera profile; and, when the edit corrects
//! the lens by its profile, the caller's name for the lens database it
//! has (this crate does not know the database). A refit of a look
//! changes the key of every frame that uses it and of no other, and a
//! database fetched or updated changes the key of every frame whose
//! lens it may correct.
//!
//! The frame's own quarter turns (the sidecar's `turn`) are not in it:
//! the pictures kept under a key are kept as the camera's tag turns the
//! frame, with the frame's turns taken back out, and a turn moves the
//! crop and the masks inside the edit (`Edit::turn`), which the key
//! does see. So a frame turned in the strip keeps the pictures of its
//! edit, and they are turned at draw time like the camera's.
//!
//! The sidecar's history, its ids, its meta (a rating, a flag, the
//! keywords) and its exports are not in it, so a rating does not change
//! the key, and an undo and a redo back to the same edit give the key
//! they gave before.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use crate::look::LutChoice;
use crate::mask::{LandscapeGroup, Shape};
use crate::{Edit, Sidecar};

/// The pipeline's own part of the key: raise it when the develop or
/// the finish comes to make a different picture from the same edit (a
/// change to an op, a default the edit does not store, the shrink that
/// makes the kept pictures), and every picture kept under the old
/// number is never asked for again.
///
/// 2: a tint blends toward its hue rather than turning it, 2026-10-10.
/// 3: highlights by the frame's own range and whites pivoting at the
/// span's foot, 2026-10-13.
pub const DEVELOP_RECIPE: u32 = 3;

/// The learned masks' stages, by version: raise one when its raster
/// comes to be another from the same shape. greycard-ui's mask cache
/// keys its rasters by these, and the develop key of an edit with such
/// a mask takes them in, so the pictures kept of it are made again.
///
/// The Sky's: the prior, the gate, SAM's outline and the color-line
/// matte. 2: the gate's core over the kept queries, 2026-10-17. 3: the
/// sure sky held up over a diluted map, and its pockets, 2026-10-17.
pub const SKY_PIPELINE: &str = "sky-3";
/// The landscape groups' gates and edges.
pub const LANDSCAPE_PIPELINE: &str = "landscape-2";
/// A People mask's routes and the people found.
pub const PART_PIPELINE: &str = "part-2";
/// A whole person's edge, Subject's once a Subject model is in.
pub const PERSON_EDGE: &str = "person-edge-1";

/// The learned stages `shape`'s raster is made by: Foliage is cut by
/// the found sky, so takes the sky's too; every People mask takes the
/// whole person's edge, which only a whole person reads, so a change
/// to that edge remakes the pictures of the other routes once as well.
pub fn mask_pipelines(shape: &Shape) -> &'static [&'static str] {
    match shape {
        Shape::Sky { .. } => &[SKY_PIPELINE],
        Shape::Landscape {
            group: LandscapeGroup::Foliage,
        } => &[LANDSCAPE_PIPELINE, SKY_PIPELINE],
        Shape::Landscape { .. } => &[LANDSCAPE_PIPELINE],
        Shape::Part { .. } => &[PART_PIPELINE, PERSON_EDGE],
        _ => &[],
    }
}

/// What the develop of `edit` reads outside the sidecar, with the
/// directories it reads them from: the look directory and the profile
/// directory. `None` for a directory there is none of.
fn files_read(edit: &Edit, looks: Option<&Path>, profiles: Option<&Path>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let (LutChoice::Named(name), Some(dir)) = (&edit.look_lut.lut, looks) {
        // The look's own table, whichever extension is there, and its
        // variant for the edit's display curve: `look_under` reads the
        // variant when it declares the curve and the own table
        // otherwise, so a change to either may change the picture.
        files.extend(crate::look::tables_in(dir, name, edit.display_curve));
    }
    if let (crate::camera::ProfileChoice::Named(name), Some(dir)) = (&edit.camera.profile, profiles)
    {
        files.push(dir.join(format!("{name}.{}", crate::camera::EXTENSION)));
    }
    files
}

/// A file's contents as the key takes them: BLAKE3 of the whole file,
/// kept by its path, length and modification time so a table read
/// once is not hashed again for every frame that names it. A file
/// written in the last two seconds is read again whatever the cache
/// holds: a refit of the same length landing within one tick of the
/// filesystem's time (coarse on Windows) would otherwise keep the old
/// hash. `None` for a file that is not there or will not read.
fn contents(path: &Path) -> Option<[u8; 32]> {
    type Seen = HashMap<PathBuf, (u64, Option<SystemTime>, [u8; 32])>;
    static SEEN: LazyLock<Mutex<Seen>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let stat = (meta.len(), meta.modified().ok());
    let settled = stat
        .1
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age >= std::time::Duration::from_secs(2));
    if settled
        && let Some(&(len, time, hash)) = SEEN.lock().ok()?.get(path)
        && (len, time) == stat
    {
        return Some(hash);
    }
    let bytes = std::fs::read(path).ok()?;
    let hash = *blake3::hash(&bytes).as_bytes();
    if let Ok(mut seen) = SEEN.lock() {
        seen.insert(path.to_path_buf(), (stat.0, stat.1, hash));
    }
    Some(hash)
}

impl Edit {
    /// The key of the picture this edit makes, with the look and the
    /// profile read from the user's directories. `lenses` names the lens
    /// database the caller develops with (where it was found and when it
    /// was made), `None` for none; it is in the key only while the edit
    /// corrects the lens by its profile. See the module.
    pub fn develop_key(&self, lenses: Option<&str>) -> u64 {
        self.develop_key_in(
            crate::look::store_dir().as_deref(),
            crate::camera::store_dir().as_deref(),
            lenses,
        )
    }

    /// [`Edit::develop_key`] with the look and profile directories
    /// named: for the tests.
    pub fn develop_key_in(
        &self,
        looks: Option<&Path>,
        profiles: Option<&Path>,
        lenses: Option<&str>,
    ) -> u64 {
        self.develop_key_by(looks, profiles, lenses, |shape| {
            mask_pipelines(shape)
                .iter()
                .map(|v| v.to_string())
                .collect()
        })
    }

    /// [`Edit::develop_key_in`] with the masks' stages as `pipelines`
    /// names them: for the tests, which raise one.
    fn develop_key_by(
        &self,
        looks: Option<&Path>,
        profiles: Option<&Path>,
        lenses: Option<&str>,
        pipelines: impl Fn(&Shape) -> Vec<String>,
    ) -> u64 {
        // What reaches no pixel is taken out: the version the edit was
        // written at (a migration that changes nothing changes no
        // picture), an adjustment's name, and the light a mask's
        // switched-off white balance keeps for when it goes on again.
        let mut edit = self.clone();
        edit.version = 0;
        for a in &mut edit.adjustments {
            a.name.clear();
            a.look.white_balance = a.look.white_balance.acting();
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"greycard develop key\0");
        hasher.update(&DEVELOP_RECIPE.to_le_bytes());
        // Serde writes a struct's fields in their order and a list in
        // its, and nothing in an edit is a map, so equal edits write
        // equal bytes.
        let json = serde_json::to_vec(&edit).expect("an edit serializes");
        hasher.update(&(json.len() as u64).to_le_bytes());
        hasher.update(&json);
        // The learned masks' stages, each once, in order: only an edit
        // with such a mask takes them in.
        let stages: std::collections::BTreeSet<String> = self
            .adjustments
            .iter()
            .flat_map(|a| &a.mask.components)
            .flat_map(|c| pipelines(&c.shape))
            .collect();
        for stage in stages {
            hasher.update(b"mask stages\0");
            hasher.update(&(stage.len() as u64).to_le_bytes());
            hasher.update(stage.as_bytes());
        }
        for path in files_read(self, looks, profiles) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            let name = name.unwrap_or_default();
            hasher.update(&(name.len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
            match contents(&path) {
                Some(hash) => {
                    hasher.update(&[1]);
                    hasher.update(&hash);
                }
                None => {
                    hasher.update(&[0]);
                }
            }
        }
        // The database the profile corrections come from, while there are
        // any: one fetched, or updated, is another picture.
        if self.lens.enabled && self.lens.profile {
            let lenses = lenses.unwrap_or_default();
            hasher.update(b"lenses\0");
            hasher.update(&(lenses.len() as u64).to_le_bytes());
            hasher.update(lenses.as_bytes());
        }
        let hash = hasher.finalize();
        u64::from_le_bytes(hash.as_bytes()[..8].try_into().expect("eight bytes"))
    }
}

impl Sidecar {
    /// The key of the picture the current edit makes of this frame: see
    /// [`Edit::develop_key`].
    pub fn develop_key(&self, lenses: Option<&str>) -> u64 {
        self.current.develop_key(lenses)
    }
}

/// What every develop key made now is made under, beside each edit's
/// own: [`DEVELOP_RECIPE`], the learned masks' stages, the lens
/// database's name as the caller gives
/// it, and the look and profile directories as they stand (each file's
/// name, length and modification time; not their contents, which only
/// the files an edit names are read for). A store of keys made under
/// another (the index's) holds keys that may no longer be the edits'
/// own: a recipe or a mask's stages raised, a lens database fetched or wiped with its
/// cache, a look refitted or a profile replaced.
pub fn keys_made_under(lenses: Option<&str>) -> String {
    keys_made_under_in(
        crate::look::store_dir().as_deref(),
        crate::camera::store_dir().as_deref(),
        lenses,
    )
}

/// [`keys_made_under`] with the directories named: for the tests.
pub fn keys_made_under_in(
    looks: Option<&Path>,
    profiles: Option<&Path>,
    lenses: Option<&str>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    for dir in [looks, profiles] {
        hasher.update(b"\0dir\0");
        let Some(dir) = dir else {
            continue;
        };
        let mut files: Vec<(String, u64, u128)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let meta = e.metadata().ok().filter(|m| m.is_file())?;
                let when = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                Some((
                    e.file_name().to_string_lossy().into_owned(),
                    meta.len(),
                    when,
                ))
            })
            .collect();
        files.sort();
        for (name, len, when) in files {
            hasher.update(&(name.len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
            hasher.update(&len.to_le_bytes());
            hasher.update(&when.to_le_bytes());
        }
    }
    let files = hasher.finalize();
    format!(
        "recipe {DEVELOP_RECIPE}; masks {SKY_PIPELINE} {LANDSCAPE_PIPELINE} {PART_PIPELINE} \
         {PERSON_EDGE}; lenses {lenses:?}; files {}",
        &files.to_hex()[..16]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::LookLut;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-key-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn key(edit: &Edit, lenses: Option<&str>) -> u64 {
        edit.develop_key_in(None, None, lenses)
    }

    #[test]
    fn an_undo_and_a_redo_back_to_the_same_picture_give_the_same_key() {
        let mut sidecar = Sidecar::default();
        let before = sidecar.develop_key(None);
        let first = sidecar.current.clone();
        sidecar.current.light.exposure += 1.0;
        let brightened = sidecar.develop_key(None);
        assert_ne!(before, brightened, "a brightening is another picture");
        // Undo: the edit as it was, from the history's copy.
        sidecar.current = first.clone();
        assert_eq!(sidecar.develop_key(None), before);
        // Redo, by setting the same value again rather than by a copy.
        sidecar.current.light.exposure += 1.0;
        assert_eq!(sidecar.develop_key(None), brightened);
        // Through the sidecar's own file and back.
        let json = serde_json::to_string(&sidecar).unwrap();
        let read: Sidecar = serde_json::from_str(&json).unwrap();
        assert_eq!(read.develop_key(None), brightened);
    }

    #[test]
    fn a_rating_a_name_and_the_turn_change_no_key_and_the_crop_does() {
        let mut sidecar = Sidecar::default();
        sidecar.current.light.exposure = 0.7;
        let k = sidecar.develop_key(None);
        sidecar.meta.rating = 4;
        sidecar.meta.keywords.push("kept".into());
        assert_eq!(sidecar.develop_key(None), k, "a rating is not the picture");
        sidecar.current.version = 1;
        assert_eq!(sidecar.develop_key(None), k, "nor is the schema version");
        sidecar.current.adjustments.push(crate::Adjustment {
            id: 3,
            name: "Sky".into(),
            ..Default::default()
        });
        let with = sidecar.develop_key(None);
        sidecar.current.adjustments[0].name = "The sky".into();
        assert_eq!(
            sidecar.develop_key(None),
            with,
            "a mask's name is not the picture"
        );
        sidecar.current.adjustments[0].enabled = false;
        assert_ne!(sidecar.develop_key(None), with);
        sidecar.current.adjustments.clear();
        assert_eq!(sidecar.develop_key(None), k);

        // The frame's turn is not the picture kept: that is the camera's
        // way up, and turned at draw time.
        sidecar.turn = 1;
        assert_eq!(sidecar.develop_key(None), k, "the frame's turn is not");
        sidecar.turn = 0;
        sidecar.current.geometry.crop = Some(crate::geometry::Crop {
            x: 0.1,
            y: 0.0,
            w: 0.9,
            h: 1.0,
        });
        assert_ne!(sidecar.develop_key(None), k, "the crop is");
    }

    /// The lens database is in the key while the edit corrects the lens
    /// by its profile, and only then.
    #[test]
    fn the_lens_database_is_in_the_key_while_the_profile_corrects() {
        let mut edit = Edit::default();
        assert!(edit.lens.enabled && edit.lens.profile);
        let none = key(&edit, None);
        let fetched = key(&edit, Some("/cache/lensfun/version_2 1760000000"));
        assert_ne!(none, fetched, "a database fetched is another picture");
        assert_ne!(
            fetched,
            key(&edit, Some("/cache/lensfun/version_2 1770000000"))
        );
        edit.lens.profile = false;
        assert_eq!(key(&edit, None), key(&edit, Some("anything")));
        edit.lens.profile = true;
        edit.lens.enabled = false;
        assert_eq!(key(&edit, None), key(&edit, Some("anything")));
    }

    /// A mask's white balance switched off is another picture; the
    /// light it keeps for the switch reaches no pixel and is not in the
    /// key, so two frames off at the global share one whatever they
    /// keep.
    #[test]
    fn a_masks_kept_white_is_not_in_the_key() {
        use crate::LocalWhite;
        let mut edit = Edit::default();
        edit.adjustments.push(crate::Adjustment {
            id: 1,
            ..Default::default()
        });
        let none = key(&edit, None);
        edit.adjustments[0].look.white_balance = LocalWhite::Absolute {
            temperature: 3200.0,
            tint: 0.002,
        };
        let on = key(&edit, None);
        assert_ne!(on, none, "a mask's own white is another picture");
        edit.adjustments[0].look.white_balance = LocalWhite::Kept {
            temperature: 3200.0,
            tint: 0.002,
        };
        assert_eq!(key(&edit, None), none, "switched off, the global's");
        edit.adjustments[0].look.white_balance = LocalWhite::Kept {
            temperature: 8000.0,
            tint: -0.01,
        };
        assert_eq!(key(&edit, None), none, "whatever it keeps");
        edit.adjustments[0].look.white_balance = LocalWhite::Absolute {
            temperature: 3200.0,
            tint: 0.002,
        };
        assert_eq!(key(&edit, None), on, "and back on, the same picture");
    }

    #[test]
    fn a_look_s_table_and_a_profile_are_in_the_key_by_their_contents() {
        let dir = scratch("files");
        let (looks, profiles) = (dir.join("looks"), dir.join("profiles"));
        std::fs::create_dir_all(&looks).unwrap();
        std::fs::create_dir_all(&profiles).unwrap();
        let mut edit = Edit::default();
        let plain = edit.develop_key_in(Some(&looks), Some(&profiles), None);
        edit.look_lut = LookLut {
            lut: LutChoice::Named("Fitted".into()),
            strength: 1.0,
        };
        let missing = edit.develop_key_in(Some(&looks), Some(&profiles), None);
        assert_ne!(plain, missing, "the name is in the edit");
        let table = looks.join("Fitted.cube");
        std::fs::write(&table, "LUT_3D_SIZE 2\n0 0 0\n").unwrap();
        let fitted = edit.develop_key_in(Some(&looks), Some(&profiles), None);
        assert_ne!(fitted, missing, "a table there is not a table missing");
        assert_eq!(
            edit.develop_key_in(Some(&looks), Some(&profiles), None),
            fitted
        );
        // A refit: other contents, and the length moved too, so the
        // kept hash is not taken.
        std::fs::write(&table, "LUT_3D_SIZE 2\n0 0 0.5\n").unwrap();
        let refit = edit.develop_key_in(Some(&looks), Some(&profiles), None);
        assert_ne!(refit, fitted, "a refit is another picture");
        // The variant for another curve is read when the edit is under
        // that curve, and only then.
        assert_eq!(edit.display_curve, crate::DisplayCurve::Channels);
        std::fs::write(looks.join("Fitted.agx.cube"), "LUT_3D_SIZE 2\n1 1 1\n").unwrap();
        assert_eq!(
            edit.develop_key_in(Some(&looks), Some(&profiles), None),
            refit
        );

        edit.camera.profile = crate::camera::ProfileChoice::Named("Body".into());
        let without = edit.develop_key_in(Some(&looks), Some(&profiles), None);
        std::fs::write(profiles.join("Body.dcp"), b"one").unwrap();
        let with = edit.develop_key_in(Some(&looks), Some(&profiles), None);
        assert_ne!(with, without);
        std::fs::write(profiles.join("Body.dcp"), b"two").unwrap();
        assert_ne!(
            edit.develop_key_in(Some(&looks), Some(&profiles), None),
            with
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A learned mask's stages raised changes the key of an edit with
    /// such a mask and of no other: a Sky's and a Foliage's when the
    /// sky's stages move, a Water's not.
    #[test]
    fn a_learned_mask_s_stages_raised_change_the_keys_of_its_edits_alone() {
        use crate::mask::{Component, LandscapeGroup, Shape};
        let with = |shape: Shape| {
            let mut edit = Edit::default();
            let mut adjustment = crate::Adjustment::default();
            adjustment.mask.components.push(Component {
                shape,
                ..Component::default()
            });
            edit.adjustments.push(adjustment);
            edit
        };
        let raised = |shape: &Shape| -> Vec<String> {
            mask_pipelines(shape)
                .iter()
                .map(|&v| if v == SKY_PIPELINE { "sky-raised" } else { v }.to_string())
                .collect()
        };
        let both = |edit: &Edit| {
            (
                edit.develop_key_in(None, None, None),
                edit.develop_key_by(None, None, None, raised),
            )
        };
        let (a, b) = both(&with(Shape::Sky { picks: Vec::new() }));
        assert_ne!(a, b, "a Sky");
        let (a, b) = both(&with(Shape::Landscape {
            group: LandscapeGroup::Foliage,
        }));
        assert_ne!(a, b, "a Foliage, cut by the sky");
        let (a, b) = both(&with(Shape::Landscape {
            group: LandscapeGroup::Water,
        }));
        assert_eq!(a, b, "a Water");
        let (a, b) = both(&Edit::default());
        assert_eq!(a, b, "no mask");
        // And the stages are in the key at all: a Sky's key is not the
        // one it would have with no stages.
        assert_ne!(
            with(Shape::Sky { picks: Vec::new() }).develop_key_in(None, None, None),
            with(Shape::Sky { picks: Vec::new() }).develop_key_by(None, None, None, |_| Vec::new()),
        );
    }

    /// What a store of keys was made under moves with a refit of any
    /// look (its length, or its time alone), with the lens database's
    /// name (a fetch, or the cache it lives in wiped), and names the
    /// recipe, so a raise of it is another; and it stands while nothing
    /// moves.
    #[test]
    fn what_the_keys_are_made_under_moves_with_a_refit_the_lenses_and_the_recipe() {
        let dir = scratch("made-under");
        let (looks, profiles) = (dir.join("looks"), dir.join("profiles"));
        std::fs::create_dir_all(&looks).unwrap();
        std::fs::create_dir_all(&profiles).unwrap();
        let table = looks.join("Fitted.cube");
        std::fs::write(&table, "LUT_3D_SIZE 2\n").unwrap();
        let under = |lenses| keys_made_under_in(Some(&looks), Some(&profiles), lenses);
        let first = under(Some("lensfun 1"));
        assert!(
            first.starts_with(&format!("recipe {DEVELOP_RECIPE};")),
            "{first}"
        );
        assert_eq!(under(Some("lensfun 1")), first, "nothing moved");
        assert_ne!(under(None), first, "the lens database's cache wiped");
        assert_ne!(under(Some("lensfun 2")), first, "another database");
        // A refit of the same length: its time alone moves.
        let file = std::fs::File::options().write(true).open(&table).unwrap();
        file.set_modified(SystemTime::now() + std::time::Duration::from_secs(5))
            .unwrap();
        drop(file);
        let refit = under(Some("lensfun 1"));
        assert_ne!(refit, first, "a refit");
        std::fs::write(profiles.join("Body.dcp"), b"one").unwrap();
        assert_ne!(under(Some("lensfun 1")), refit, "a profile added");
        assert_ne!(
            keys_made_under_in(None, None, None),
            keys_made_under_in(Some(&looks), None, None),
            "a directory that is there with a file in it is not none"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
