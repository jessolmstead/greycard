//! The develop key: a name for the picture an edit makes, for the
//! pictures kept of it (the thumbnails and the larger picture made from
//! the edit, notes on thumbnails from the edit).
//!
//! It is a hash of what makes the picture and of nothing else: the
//! edit itself, less what never reaches a pixel (an adjustment's name,
//! the schema version it was written at); a recipe number for the
//! pipeline, raised when the same edit comes to make another picture;
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
use crate::{Edit, Sidecar};

/// The pipeline's own part of the key: raise it when the develop or
/// the finish comes to make a different picture from the same edit (a
/// change to an op, a default the edit does not store, the shrink that
/// makes the kept pictures), and every picture kept under the old
/// number is never asked for again.
pub const DEVELOP_RECIPE: u32 = 1;

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
/// once is not hashed again for every frame that names it. `None` for
/// a file that is not there or will not read.
fn contents(path: &Path) -> Option<[u8; 32]> {
    type Seen = HashMap<PathBuf, (u64, Option<SystemTime>, [u8; 32])>;
    static SEEN: LazyLock<Mutex<Seen>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let stat = (meta.len(), meta.modified().ok());
    if let Some(&(len, time, hash)) = SEEN.lock().ok()?.get(path)
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
        // What reaches no pixel is taken out: the version the edit was
        // written at (a migration that changes nothing changes no
        // picture), and an adjustment's name.
        let mut edit = self.clone();
        edit.version = 0;
        for a in &mut edit.adjustments {
            a.name.clear();
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
}
