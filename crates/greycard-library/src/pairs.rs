//! RAW+JPEG pairs: a camera set to shoot both writes `IMG_1234.CR3`
//! and `IMG_1234.JPG` for one press, and the JPEG is the raw's
//! companion, not a second frame.
//!
//! The rule, the one place it is written: a JPEG (`.jpg` or `.jpeg`,
//! any case) is a companion when a raw ([`greycard_core::decode::
//! is_raw_path`]) of the same stem is in the same folder, the stems
//! compared without regard to case as the import's scan and the move
//! of the rejects compare them. A JPEG with no raw beside it is a
//! frame of its own, and so is every raw: two raws of one stem (a
//! `.dng` made from the `.cr3` beside it) are two frames. A companion
//! has one raw, so a move of either raw does not take it from the
//! other: the camera's own format before a DNG, since a DNG beside a
//! camera's raw of the same name was made from it later and the JPEG
//! was written with the camera's at the press; then the order of
//! `RAW_EXTENSIONS`, then the name. A DNG alone with a JPEG (a camera
//! that writes DNG) is the raw.
//!
//! The index answers the same rule in SQL ([`COMPANION_SQL`]) through
//! `pairkey`, which is [`pair_key`] registered with SQLite, so the two
//! cannot disagree.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The kind [`crate::filter::kind_of`] gives a JPEG.
pub const JPEG: &str = "jpeg";

/// Whether `path` names a JPEG, by its extension.
pub fn is_jpeg(path: &Path) -> bool {
    crate::filter::kind_of(path) == Some(JPEG)
}

/// The stem a raw or a JPEG is paired by: the name up to its last
/// dot, case folded. `None` for any other file, which is never in a
/// pair.
pub fn stem_key(path: &Path) -> Option<String> {
    let pairs = greycard_core::decode::is_raw_path(path) || is_jpeg(path);
    pairs.then(|| path.file_stem().map(|s| s.to_string_lossy().to_lowercase()))?
}

/// Where a raw stands among the raws of one stem for the JPEG beside
/// them, first lowest: the camera's own format before a DNG, then the
/// order the decoder lists the extensions in.
pub fn raw_rank(path: &Path) -> (bool, usize) {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let at = greycard_core::decode::RAW_EXTENSIONS
        .iter()
        .position(|e| *e == ext)
        .unwrap_or(usize::MAX);
    (ext == "dng", at)
}

/// The pairs among `paths`, as `(raw, companion)` indices into it:
/// each JPEG with a raw of its stem in its folder, given to the raw
/// [`raw_rank`] puts first. A path in no pair is in none of them. A raw
/// may have more than one (an `IMG.JPG` and an `IMG.jpeg`); a JPEG is
/// in one pair at most.
pub fn pairs_in(paths: &[PathBuf]) -> Vec<(usize, usize)> {
    let mut raws: HashMap<(&Path, String), usize> = HashMap::new();
    for (i, p) in paths.iter().enumerate() {
        if !greycard_core::decode::is_raw_path(p) {
            continue;
        }
        let Some(stem) = stem_key(p) else {
            continue;
        };
        let folder = p.parent().unwrap_or(Path::new(""));
        raws.entry((folder, stem))
            .and_modify(|best| {
                let (a, b) = (&paths[i], &paths[*best]);
                if (raw_rank(a), a.file_name()) < (raw_rank(b), b.file_name()) {
                    *best = i;
                }
            })
            .or_insert(i);
    }
    paths
        .iter()
        .enumerate()
        .filter(|(_, p)| is_jpeg(p))
        .filter_map(|(j, p)| {
            let folder = p.parent().unwrap_or(Path::new(""));
            let raw = raws.get(&(folder, stem_key(p)?))?;
            Some((*raw, j))
        })
        .collect()
}

/// A listing with its pairs found: the frames, in the order given,
/// every path but a companion; and each raw that has companions with
/// them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Paired {
    pub frames: Vec<PathBuf>,
    pub companions: Vec<(PathBuf, Vec<PathBuf>)>,
}

/// `paths` with the pairs among them found ([`pairs_in`]); with
/// `apart`, as a camera's JPEG treated as a frame of its own, every
/// path is a frame and none is anyone's companion.
pub fn pair(paths: Vec<PathBuf>, apart: bool) -> Paired {
    if apart {
        return Paired {
            frames: paths,
            companions: Vec::new(),
        };
    }
    let mut taken = vec![false; paths.len()];
    let mut of_raw: BTreeMap<usize, Vec<PathBuf>> = BTreeMap::new();
    for (raw, jpeg) in pairs_in(&paths) {
        taken[jpeg] = true;
        of_raw.entry(raw).or_default().push(paths[jpeg].clone());
    }
    let companions = of_raw
        .into_iter()
        .map(|(r, with)| (paths[r].clone(), with))
        .collect();
    let frames = paths
        .into_iter()
        .zip(taken)
        .filter_map(|(p, t)| (!t).then_some(p))
        .collect();
    Paired { frames, companions }
}

/// The companions of `raw` as its folder holds them now: the folder
/// read, its names of `raw`'s stem put through the rule. What a move
/// or a delete of the raw takes with it, asked at the moment it acts,
/// so a JPEG that arrived after the folder was listed goes too. None
/// for a file that is not a raw, or a folder that cannot be read.
pub fn companions_on_disk(raw: &Path) -> Vec<PathBuf> {
    companions_of(std::slice::from_ref(&raw.to_path_buf()))
        .into_iter()
        .flat_map(|(_, with)| with)
        .collect()
}

/// [`companions_on_disk`] for many files at once, each folder read
/// once: each raw among `files` that has companions, with them, in the
/// order given. Files that are not raws are passed over.
pub fn companions_of(files: &[PathBuf]) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let mut by_folder: Vec<(&Path, Vec<&PathBuf>)> = Vec::new();
    for f in files
        .iter()
        .filter(|f| greycard_core::decode::is_raw_path(f))
    {
        let dir = f.parent().unwrap_or(Path::new(""));
        match by_folder.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, raws)) => raws.push(f),
            None => by_folder.push((dir, vec![f])),
        }
    }
    let mut found: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for (dir, raws) in by_folder {
        let stems: HashSet<String> = raws.iter().filter_map(|r| stem_key(r)).collect();
        let near = near(dir, &stems);
        for (raw, with) in pair(near, false).companions {
            found.insert(raw, with);
        }
    }
    files
        .iter()
        .filter_map(|f| Some((f.clone(), found.remove(f)?)))
        .collect()
}

/// The raw `jpeg` is the companion of, as its folder holds them now;
/// none for a JPEG with no raw of its stem beside it, or a file that
/// is not a JPEG.
pub fn raw_on_disk(jpeg: &Path) -> Option<PathBuf> {
    if !is_jpeg(jpeg) {
        return None;
    }
    let dir = jpeg.parent().unwrap_or(Path::new(""));
    let stems: HashSet<String> = stem_key(jpeg).into_iter().collect();
    let near = near(dir, &stems);
    let me = near
        .iter()
        .position(|p| p.file_name() == jpeg.file_name())?;
    pairs_in(&near)
        .into_iter()
        .find(|&(_, j)| j == me)
        .map(|(r, _)| near[r].clone())
}

/// The files in `dir` (spelled as given, "" for the working folder)
/// whose pairing stem is one of `stems`, by name: a link to a file
/// counts, a folder does not.
fn near(dir: &Path, stems: &HashSet<String>) -> Vec<PathBuf> {
    let read = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let Ok(entries) = std::fs::read_dir(read) else {
        return Vec::new();
    };
    let mut near: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
        .map(|e| dir.join(e.file_name()))
        .filter(|p| stem_key(p).is_some_and(|s| stems.contains(&s)) && !p.is_dir())
        .collect();
    near.sort();
    near
}
/// The key a raw and its JPEG share in the index: the folder's bytes as
/// the index keeps them, a zero byte, and the stem ([`stem_key`]), for a
/// file whose kind ([`crate::filter::kind_of`]) is `kind`; `None` for a
/// file of another kind, or one with no stem to pair by.
pub fn pair_key(folder: &[u8], name: &Path, kind: &str) -> Option<Vec<u8>> {
    if crate::filter::kind_of(name) != Some(kind) {
        return None;
    }
    let stem = stem_key(name)?;
    let mut key = Vec::with_capacity(folder.len() + 1 + stem.len());
    key.extend_from_slice(folder);
    key.push(0);
    key.extend_from_slice(stem.as_bytes());
    Some(key)
}

/// "This row is a companion", over the `files` table: a JPEG whose key
/// is among the keys of the raws the index last found where they are.
/// One key a row, so the raws' are gathered once into a list SQLite
/// looks each JPEG up in: a test of each JPEG against its folder's rows
/// would be the square of a folder's size. A `WHERE` term, `NOT`ed by a
/// listing that leaves companions out; it names the outer table `files`.
pub(crate) const COMPANION_SQL: &str = "coalesce(pairkey(files.folder, files.name, 'jpeg') \
     IN (SELECT pairkey(pr.folder, pr.name, 'raw') FROM files AS pr \
     WHERE pr.missing_since IS NULL), 0)";

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn a_jpeg_beside_a_raw_of_its_stem_is_the_raws_and_any_case_pairs() {
        let all = paths(&[
            "/s/IMG_1.CR3",
            "/s/IMG_1.JPG",
            "/s/img_2.nef",
            "/s/IMG_2.jpeg",
            "/s/IMG_3.JPG",
            "/s/IMG_4.CR3",
            "/s/IMG_4.png",
            "/t/IMG_1.jpg",
        ]);
        assert_eq!(pairs_in(&all), vec![(0, 1), (2, 3)]);
        let paired = pair(all.clone(), false);
        assert_eq!(
            paired.frames,
            paths(&[
                "/s/IMG_1.CR3",
                "/s/img_2.nef",
                "/s/IMG_3.JPG",
                "/s/IMG_4.CR3",
                "/s/IMG_4.png",
                "/t/IMG_1.jpg",
            ])
        );
        assert_eq!(
            paired.companions,
            vec![
                (all[0].clone(), vec![all[1].clone()]),
                (all[2].clone(), vec![all[3].clone()]),
            ]
        );
        // Apart, every file is a frame and nothing is paired.
        let apart = pair(all.clone(), true);
        assert_eq!(apart.frames, all);
        assert!(apart.companions.is_empty());
    }

    #[test]
    fn two_raws_of_one_stem_are_two_frames_and_the_jpeg_is_the_cameras() {
        // The DNG sorts first by name, and is passed over all the same.
        let all = paths(&["/s/DSCF1.DNG", "/s/DSCF1.JPG", "/s/DSCF1.RAF"]);
        let paired = pair(all, false);
        assert_eq!(paired.frames, paths(&["/s/DSCF1.DNG", "/s/DSCF1.RAF"]));
        assert_eq!(
            paired.companions,
            vec![(PathBuf::from("/s/DSCF1.RAF"), paths(&["/s/DSCF1.JPG"]))]
        );
        // A DNG with no camera's raw beside it is the raw.
        let alone = pair(paths(&["/s/L1.dng", "/s/L1.jpg"]), false);
        assert_eq!(alone.frames, paths(&["/s/L1.dng"]));
        // Two of the camera's own: the decoder's order.
        let two = pairs_in(&paths(&["/s/A.NEF", "/s/A.CR3", "/s/A.JPG"]));
        assert_eq!(two, vec![(1, 2)]);
        // Both spellings of the JPEG go with the one raw.
        let both = pair(paths(&["/s/B.CR3", "/s/B.JPG", "/s/B.jpeg"]), false);
        assert_eq!(both.frames, paths(&["/s/B.CR3"]));
        assert_eq!(both.companions[0].1.len(), 2);
    }

    #[test]
    fn only_raws_and_jpegs_have_a_stem_to_pair_by() {
        assert_eq!(
            stem_key(Path::new("/s/IMG_1.CR3")).as_deref(),
            Some("img_1")
        );
        assert_eq!(
            stem_key(Path::new("/s/IMG_1.JpEg")).as_deref(),
            Some("img_1")
        );
        assert_eq!(stem_key(Path::new("/s/IMG_1.tif")), None);
        assert_eq!(stem_key(Path::new("/s/IMG_1.CR3.gcd")), None);
        assert_eq!(stem_key(Path::new("/s/IMG_1.xmp")), None);
        // The index's key: the folder, then the stem, for the kind
        // asked only; a raw and its JPEG of one folder share it.
        let raw = pair_key(b"/s", Path::new("IMG_1.CR3"), "raw");
        assert_eq!(raw.as_deref(), Some(&b"/s\0img_1"[..]));
        assert_eq!(pair_key(b"/s", Path::new("img_1.JPG"), "jpeg"), raw);
        assert_eq!(pair_key(b"/s", Path::new("IMG_1.CR3"), "jpeg"), None);
        assert_eq!(pair_key(b"/s", Path::new("IMG_1.png"), "jpeg"), None);
        assert_ne!(pair_key(b"/t", Path::new("IMG_1.JPG"), "jpeg"), raw);
    }

    #[test]
    fn the_companions_on_disk_are_the_folders_as_it_is_now() {
        let dir = crate::index::tests::scratch("pairs");
        for name in ["A.CR3", "A.DNG", "B.ARW", "C.JPG", "A.CR3.gcd", "A.xmp"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        assert!(companions_on_disk(&dir.join("A.CR3")).is_empty());
        std::fs::write(dir.join("a.jpg"), b"x").unwrap();
        assert_eq!(
            companions_on_disk(&dir.join("A.CR3")),
            vec![dir.join("a.jpg")]
        );
        // The DNG made from it is no claim on the JPEG.
        assert!(companions_on_disk(&dir.join("A.DNG")).is_empty());
        assert!(companions_on_disk(&dir.join("B.ARW")).is_empty());
        // A JPEG has no companions of its own, and knows its raw.
        assert!(companions_on_disk(&dir.join("C.JPG")).is_empty());
        assert_eq!(raw_on_disk(&dir.join("a.jpg")), Some(dir.join("A.CR3")));
        assert_eq!(raw_on_disk(&dir.join("C.JPG")), None);
        assert_eq!(raw_on_disk(&dir.join("A.CR3")), None);
        // Many at once, in the order given, the folder read once.
        std::fs::write(dir.join("B.jpeg"), b"x").unwrap();
        assert_eq!(
            companions_of(&[dir.join("B.ARW"), dir.join("C.JPG"), dir.join("A.CR3")]),
            [
                (dir.join("B.ARW"), vec![dir.join("B.jpeg")]),
                (dir.join("A.CR3"), vec![dir.join("a.jpg")]),
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
