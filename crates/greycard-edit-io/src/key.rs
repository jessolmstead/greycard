//! The develop key's reading of the files an edit names (see
//! `greycard_edit::key`, which makes the key from them) and of the look
//! and profile directories the keys are made under.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use greycard_edit::camera::ProfileChoice;
use greycard_edit::key::{
    ASPECT_FRAME, DEVELOP_RECIPE, LANDSCAPE_PIPELINE, MIDTONES_SHAPE, PART_PIPELINE, PERSON_EDGE,
    Read, SHARPEN_GUIDE, SKY_PIPELINE,
};
use greycard_edit::look::LutChoice;
use greycard_edit::{Edit, LensChoice};

/// The key of the picture `edit` makes, with the look and the profile
/// read from the user's directories: see `Edit::develop_key_with`.
pub fn develop_key(edit: &Edit, lenses: Option<&str>) -> u64 {
    develop_key_in(
        edit,
        crate::looks::store_dir().as_deref(),
        crate::profiles::store_dir().as_deref(),
        lenses,
    )
}

/// [`develop_key`] with the look and profile directories named.
pub fn develop_key_in(
    edit: &Edit,
    looks: Option<&Path>,
    profiles: Option<&Path>,
    lenses: Option<&str>,
) -> u64 {
    let read: Vec<Read> = files_read(edit, looks, profiles)
        .into_iter()
        .map(|path| Read {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            contents: contents(&path),
        })
        .collect();
    edit.develop_key_with(&read, lenses)
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
        files.extend(crate::looks::tables_in(dir, name, edit.display_curve));
    }
    if let (ProfileChoice::Named(name), Some(dir)) = (&edit.camera.profile, profiles)
        && let Some(path) = crate::profiles::path_in(Some(dir), name)
    {
        files.push(path);
    }
    // An Adobe lens profile picked by hand, beside the camera
    // profiles: read by its file name, so only a plain name counts.
    if let (LensChoice::Lcp(file), Some(dir)) = (&edit.lens.chosen, profiles)
        && edit.lens.enabled
        && edit.lens.profile
        && !file.contains(['/', '\\'])
        && !file.contains("..")
    {
        files.push(dir.join(file));
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

/// What every develop key made now is made under, beside each edit's
/// own: [`DEVELOP_RECIPE`], the learned masks' stages, the sharpen's
/// guide ([`SHARPEN_GUIDE`]), the Midtones bell's shape
/// ([`MIDTONES_SHAPE`]), an aspect's framing with no crop stored
/// ([`ASPECT_FRAME`]), the lens database's name as the caller gives
/// it, and the look and profile directories as they stand (each file's
/// name, length and modification time; not their contents, which only
/// the files an edit names are read for). A store of keys made under
/// another (the index's) holds keys that may no longer be the edits'
/// own: a recipe or a mask's stages raised, a lens database fetched or
/// wiped with its cache, a look refitted or a profile replaced.
pub fn keys_made_under(lenses: Option<&str>) -> String {
    keys_made_under_in(
        crate::looks::store_dir().as_deref(),
        crate::profiles::store_dir().as_deref(),
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
         {PERSON_EDGE}; {SHARPEN_GUIDE}; {MIDTONES_SHAPE}; {ASPECT_FRAME}; lenses {lenses:?}; files {}",
        &files.to_hex()[..16]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_edit::look::LookLut;

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

    #[test]
    fn a_look_s_table_and_a_profile_are_in_the_key_by_their_contents() {
        let dir = scratch("files");
        let (looks, profiles) = (dir.join("looks"), dir.join("profiles"));
        std::fs::create_dir_all(&looks).unwrap();
        std::fs::create_dir_all(&profiles).unwrap();
        let mut edit = Edit::default();
        let plain = develop_key_in(&edit, Some(&looks), Some(&profiles), None);
        edit.look_lut = LookLut {
            lut: LutChoice::Named("Fitted".into()),
            strength: 1.0,
        };
        let missing = develop_key_in(&edit, Some(&looks), Some(&profiles), None);
        assert_ne!(plain, missing, "the name is in the edit");
        let table = looks.join("Fitted.cube");
        std::fs::write(&table, "LUT_3D_SIZE 2\n0 0 0\n").unwrap();
        let fitted = develop_key_in(&edit, Some(&looks), Some(&profiles), None);
        assert_ne!(fitted, missing, "a table there is not a table missing");
        assert_eq!(
            develop_key_in(&edit, Some(&looks), Some(&profiles), None),
            fitted
        );
        // A refit: other contents, and the length moved too, so the
        // kept hash is not taken.
        std::fs::write(&table, "LUT_3D_SIZE 2\n0 0 0.5\n").unwrap();
        let refit = develop_key_in(&edit, Some(&looks), Some(&profiles), None);
        assert_ne!(refit, fitted, "a refit is another picture");
        // The variant for another curve is read when the edit is under
        // that curve, and only then.
        assert_eq!(edit.display_curve, greycard_edit::DisplayCurve::Channels);
        std::fs::write(looks.join("Fitted.agx.cube"), "LUT_3D_SIZE 2\n1 1 1\n").unwrap();
        assert_eq!(
            develop_key_in(&edit, Some(&looks), Some(&profiles), None),
            refit
        );

        edit.camera.profile = ProfileChoice::Named("Body".into());
        let without = develop_key_in(&edit, Some(&looks), Some(&profiles), None);
        std::fs::write(profiles.join("Body.dcp"), b"one").unwrap();
        let with = develop_key_in(&edit, Some(&looks), Some(&profiles), None);
        assert_ne!(with, without);
        std::fs::write(profiles.join("Body.dcp"), b"two").unwrap();
        let body = develop_key_in(&edit, Some(&looks), Some(&profiles), None);
        assert_ne!(body, with);

        // A lens profile picked by hand, likewise.
        edit.lens.chosen = LensChoice::Lcp("Lens.lcp".into());
        let without = develop_key_in(&edit, Some(&looks), Some(&profiles), None);
        assert_ne!(without, body, "the choice is in the edit");
        std::fs::write(profiles.join("Lens.lcp"), b"one").unwrap();
        let with = develop_key_in(&edit, Some(&looks), Some(&profiles), None);
        assert_ne!(with, without);
        std::fs::write(profiles.join("Lens.lcp"), b"three").unwrap();
        assert_ne!(
            develop_key_in(&edit, Some(&looks), Some(&profiles), None),
            with
        );
        greycard_core::testing::remove_dir_retry(&dir);
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
        // Each part some edits alone take into their key: raised, the
        // index's keys are settled again (`settle_develop_keys`).
        for part in [SHARPEN_GUIDE, MIDTONES_SHAPE, ASPECT_FRAME] {
            assert!(first.contains(&format!("; {part};")), "{part}: {first}");
        }
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
        greycard_core::testing::remove_dir_retry(&dir);
    }
}
