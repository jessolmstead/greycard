//! What the editor opens and in what order: which files a path
//! names, which of them the selection lands on, and whether a
//! sidecar holds a develop or only a star.
//!
//! Pure, and here rather than in the browser's callbacks, so the
//! rules can be read and tested without a window. It touches the
//! disk — a listing and a canonical path are questions about the
//! disk — but it knows nothing of the panel.

use crate::*;

/// `files`' index of `last`, when it is among them; the first
/// otherwise. Compared by the canonical path, since a directory scan
/// and a remembered path are not always spelled the same way.
pub fn select_index(files: &[PathBuf], last: Option<&Path>) -> usize {
    last.and_then(|want| {
        let want = std::fs::canonicalize(want).unwrap_or_else(|_| want.to_path_buf());
        files
            .iter()
            .position(|f| std::fs::canonicalize(f).map(|c| c == want).unwrap_or(false))
    })
    .unwrap_or(0)
}

/// Whether a raw's sidecar has nothing in it that a develop put
/// there: the edit is the default, and there is no history and no
/// snapshot behind it. A file rated but never developed has a
/// sidecar, and that is not the same as having been developed —
/// its learned blend is still waiting for its ISO.
pub fn never_developed(sidecar: &Sidecar) -> bool {
    sidecar.current == Edit::default() && sidecar.history.is_empty() && sidecar.snapshots.is_empty()
}

/// Map a .gcd sidecar path to its raw: IMG.CR3.gcd -> IMG.CR3, and
/// .greycard/IMG.CR3.gcd -> IMG.CR3 beside the folder, since a
/// sidecar under the hidden folder belongs to the frame above it.
/// Other paths pass through unchanged.
pub fn sidecar_to_raw(p: PathBuf) -> PathBuf {
    if !p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gcd"))
    {
        return p;
    }
    let raw = p.with_extension("");
    let folder = raw.parent().and_then(Path::file_name)
        == Some(std::ffi::OsStr::new(greycard_edit::SIDECAR_FOLDER));
    match (folder, raw.parent().and_then(Path::parent), raw.file_name()) {
        (true, Some(shoot), Some(name)) => shoot.join(name),
        _ => raw,
    }
}

/// Several paths opened at once, as the Finder sends a selection:
/// each listed as `list_files` lists it, in the order given, a file
/// named twice (or a sidecar beside its raw) kept once, and the
/// paths that list nothing returned with why.
pub fn list_paths(paths: &[PathBuf]) -> (Vec<PathBuf>, Vec<anyhow::Error>) {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut refused = Vec::new();
    for path in paths {
        match list_files(path) {
            Ok(listed) => {
                for f in listed {
                    if !files.contains(&f) {
                        files.push(f);
                    }
                }
            }
            Err(e) => refused.push(e),
        }
    }
    (files, refused)
}

pub fn list_files(path: &std::path::Path) -> Result<Vec<PathBuf>> {
    // Before the extension: a name with the right ending but no file
    // behind it used to pass and open a blank window that never said
    // anything.
    anyhow::ensure!(path.exists(), "there is no {}", path.display());
    let opens = |p: &std::path::Path| {
        greycard_core::decode::is_raw_path(p) || greycard_core::picture::is_picture_path(p)
    };
    if path.is_dir() {
        // In a directory, just filter for actual raw and picture files.
        // .gcd sidecars are naturally excluded since .gcd is not a raw or
        // picture extension; they stay beside the raws they edit.
        // Only regular files, or links to them: a folder, a named
        // pipe or a socket with a raw's name would hold a thumbnail
        // thread, and the indexer, on a read that never returns.
        let mut files: Vec<PathBuf> = std::fs::read_dir(path)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| opens(p) && p.is_file())
            .collect();
        files.sort();
        Ok(files)
    } else {
        // For a single file, map .gcd sidecars to their raw files, but
        // ensure the raw actually exists.
        let mapped = sidecar_to_raw(path.to_path_buf());
        anyhow::ensure!(
            opens(&mapped),
            "{} is not a RAW, JPEG, PNG or TIFF file (or a .gcd sidecar for one)",
            path.display()
        );
        // If it was a .gcd, verify the raw exists.
        if path != mapped && !mapped.exists() {
            anyhow::bail!(
                "{} names {}, which is not there",
                path.display(),
                mapped.display()
            );
        }
        Ok(vec![mapped])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_index_finds_the_last_file_or_falls_back_to_the_first() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-select-index-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<PathBuf> = ["a.cr3", "b.cr3", "c.cr3"]
            .iter()
            .map(|n| {
                let p = dir.join(n);
                std::fs::write(&p, b"").unwrap();
                p
            })
            .collect();
        // The middle file, found by its canonical path even when
        // spelled with a redundant `.`.
        let spelled = dir.join(".").join("b.cr3");
        assert_eq!(select_index(&files, Some(&spelled)), 1);
        // Not among them: the first file.
        assert_eq!(select_index(&files, Some(Path::new("/no/such/file"))), 0);
        // Nothing remembered: the first file.
        assert_eq!(select_index(&files, None), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Only regular files are listed: a folder named as a raw is not,
    /// and on Unix neither is a named pipe, whose read never returns.
    #[test]
    fn a_folder_lists_only_its_regular_files() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-regular-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("folder.CR3")).unwrap();
        std::fs::write(dir.join("a.CR3"), b"").unwrap();
        #[cfg(unix)]
        {
            let made = std::process::Command::new("mkfifo")
                .arg(dir.join("pipe.CR3"))
                .status();
            // Without mkfifo on the machine the folder case stands alone.
            if made.is_ok_and(|s| s.success()) {
                assert!(dir.join("pipe.CR3").exists());
            }
        }
        assert_eq!(list_files(&dir).unwrap(), vec![dir.join("a.CR3")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_is_not_there_is_refused_by_name() {
        // The extension is one the editor opens, so the old check
        // passed it through and the window came up empty.
        // A name of this run's own: the temp directory is shared, and
        // another worktree may be running these tests too.
        let name = format!(
            "greycard-no-such-frame-{}-{}.CR3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let missing = std::env::temp_dir().join(&name);
        let e = list_files(&missing).expect_err("a missing file is an error");
        let said = e.to_string();
        assert!(said.contains(&name), "{said}");

        // A file that is there is listed as before.
        std::fs::write(&missing, b"not a raw, but a file").unwrap();
        assert_eq!(list_files(&missing).unwrap(), vec![missing.clone()]);
        std::fs::remove_file(&missing).unwrap();
    }

    #[test]
    fn list_paths_keeps_each_file_once_and_says_what_it_refused() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-list-paths-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.CR3"), dir.join("b.RAF"));
        for p in [&a, &b] {
            std::fs::write(p, b"").unwrap();
        }
        let gcd = dir.join("a.CR3.gcd");
        std::fs::write(&gcd, b"{}").unwrap();
        let missing = dir.join("gone.NEF");
        // The Finder's order, not the name's; the sidecar is its raw,
        // already listed.
        let (files, refused) = list_paths(&[b.clone(), a.clone(), gcd, missing]);
        assert_eq!(files, vec![b, a]);
        assert_eq!(refused.len(), 1);
        assert!(refused[0].to_string().contains("gone.NEF"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_sidecar_under_the_folder_names_the_frame_above_it() {
        let under = PathBuf::from("/shoot/.greycard/IMG_0001.CR3.gcd");
        assert_eq!(sidecar_to_raw(under), PathBuf::from("/shoot/IMG_0001.CR3"));
        // Only that folder: a `.gcd` in any other folder is beside
        // its raw.
        let elsewhere = PathBuf::from("/shoot/edits/IMG_0001.CR3.gcd");
        assert_eq!(
            sidecar_to_raw(elsewhere),
            PathBuf::from("/shoot/edits/IMG_0001.CR3")
        );
    }

    #[test]
    fn a_folder_listing_skips_the_hidden_folder() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-list-hidden-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let hidden = dir.join(greycard_edit::SIDECAR_FOLDER);
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(dir.join("a.CR3"), b"raw").unwrap();
        std::fs::write(hidden.join("a.CR3.gcd"), b"{}").unwrap();
        let files = list_files(&dir).unwrap();
        assert_eq!(files, vec![dir.join("a.CR3")]);
        // And the sidecar under it opens as its frame.
        assert_eq!(
            list_files(&hidden.join("a.CR3.gcd")).unwrap(),
            vec![dir.join("a.CR3")]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sidecar_to_raw_maps_gcd_to_raw_and_leaves_others_alone() {
        use std::path::PathBuf;

        let cr3 = PathBuf::from("IMG_0001.CR3");
        assert_eq!(sidecar_to_raw(cr3.clone()), cr3);

        let gcd = PathBuf::from("IMG_0001.CR3.gcd");
        let raw = PathBuf::from("IMG_0001.CR3");
        assert_eq!(sidecar_to_raw(gcd), raw);

        let jpeg = PathBuf::from("photo.JPG");
        assert_eq!(sidecar_to_raw(jpeg.clone()), jpeg);

        let gcd_jpeg = PathBuf::from("photo.JPG.gcd");
        let jpeg_raw = PathBuf::from("photo.JPG");
        assert_eq!(sidecar_to_raw(gcd_jpeg), jpeg_raw);
    }

    #[test]
    fn sidecar_mapping_preserves_directory_paths() {
        use std::path::PathBuf;

        let dir_raw = PathBuf::from("/path/to/IMG_0001.CR3");
        assert_eq!(sidecar_to_raw(dir_raw.clone()), dir_raw);

        let dir_gcd = PathBuf::from("/path/to/IMG_0001.CR3.gcd");
        let dir_expected = PathBuf::from("/path/to/IMG_0001.CR3");
        assert_eq!(sidecar_to_raw(dir_gcd), dir_expected);
    }

    /// A star is not a develop: the sidecar a rating makes still has
    /// nothing of a develop in it, so the frame's learned blend is
    /// still waiting for its ISO.
    #[test]
    fn a_rated_sidecar_is_still_undeveloped() {
        let mut sidecar = Sidecar::default();
        assert!(never_developed(&sidecar));
        sidecar.meta.rating = 3;
        assert!(never_developed(&sidecar));
        let mut edit = Edit::default();
        edit.light.exposure = 0.5;
        assert!(sidecar.record(edit));
        assert!(!never_developed(&sidecar));
    }
}
