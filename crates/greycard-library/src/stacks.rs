//! Stacks on disk: frames of one folder grouped by hand, one of them on
//! top, the rest behind it.
//!
//! A stack, like a collection, cannot be rebuilt from the disk, so it
//! lives beside the collections and not in the index: one small file a
//! stack, `<id>.json`, in `stacks/` under the collections' folder. A
//! build that knows no stacks reads that folder's files by their ends
//! and passes over a folder in it, so the collections it reads are
//! the ones it always read. The id is made at random when the stack
//! is; unstacking deletes the file.
//!
//! A member is a collection's reference: the picture's content hash,
//! the path it was last seen at, and the version of it, none for the
//! file's own. The references are the stack's order, its top first.
//! The window finds a member by its path, as the index keys it; the
//! hash is what follows the file when another tool rewrites it in
//! place, and what a later carry across machines finds it by.
//!
//! This is the files and their one writer, for the editor and the
//! command line alike; the editor's store is `greycard-ui`'s `stacks`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::collections::{Reference, Stamp, new_id, now, stamp_of, write_atomic};

/// The folder the stacks are kept in, under the collections'.
pub const DIR: &str = "stacks";

/// What the files say their shape is: a later build that changes it
/// bumps this, and this build leaves a file of a later one alone.
pub const VERSION: u32 = 1;

/// A stack: its members in order, the top first.
#[derive(Debug, Clone, PartialEq)]
pub struct Stack {
    pub id: String,
    /// When it was made, seconds since the epoch.
    pub made: u64,
    /// Shown with its members in the list, not as its top alone.
    pub open: bool,
    pub refs: Vec<Reference>,
}

/// A stack's file as it is written.
#[derive(Serialize, Deserialize)]
pub struct StackFile {
    pub version: u32,
    #[serde(default)]
    pub made: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
    #[serde(default)]
    pub refs: Vec<Reference>,
}

impl Stack {
    /// A new stack of these members, the first on top, closed.
    pub fn new(refs: Vec<Reference>) -> Stack {
        Stack {
            id: new_id(),
            made: now(),
            open: false,
            refs,
        }
    }

    fn to_file(&self) -> StackFile {
        StackFile {
            version: VERSION,
            made: self.made,
            open: self.open,
            refs: self.refs.clone(),
        }
    }

    /// Written to its file in the folder `dir`, through the part file;
    /// the hash of what was written.
    pub fn write(&self, dir: &Path) -> std::io::Result<Stamp> {
        let text = serde_json::to_string_pretty(&self.to_file()).map_err(std::io::Error::other)?;
        write_atomic(&file_of(dir, &self.id), &text)?;
        Ok(blake3::hash(text.as_bytes()))
    }
}

/// The stacks' folder for the collections' folder `collections`.
pub fn dir_in(collections: &Path) -> PathBuf {
    collections.join(DIR)
}

/// The file of stack `id` in the folder `dir`.
pub fn file_of(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// A stack's file read, whatever its shape says.
pub fn read_stack(path: &Path) -> Result<StackFile, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice::<StackFile>(&bytes).map_err(|e| e.to_string())
}

/// The stacks of a folder as read, and each file's stamp.
#[derive(Debug, Default)]
pub struct Folder {
    pub list: Vec<Stack>,
    pub stamps: HashMap<String, Stamp>,
}

/// Every stack in `dir`, by a build that reads files up to shape
/// `up_to`. A file that will not read, or one a later build wrote, is
/// said and left where it is. A folder not there yet is none.
pub fn read_folder(dir: &Path, up_to: u32) -> Folder {
    let mut folder = Folder::default();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return folder,
        Err(e) => {
            log::warn!("stacks {}: {e}", dir.display());
            return folder;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(id) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
        else {
            continue;
        };
        match read_stack(&path) {
            Ok(f) if f.version <= up_to => {
                if let Some(stamp) = stamp_of(&path) {
                    folder.stamps.insert(id.to_string(), stamp);
                }
                folder.list.push(Stack {
                    id: id.to_string(),
                    made: f.made,
                    open: f.open,
                    refs: f.refs,
                });
            }
            Ok(f) => log::warn!(
                "stack {}: version {} is a later build's; left alone",
                path.display(),
                f.version
            ),
            Err(e) => log::warn!("stack {}: {e}; left alone", path.display()),
        }
    }
    sort(&mut folder.list);
    folder
}

/// The order the stacks are kept and judged in, on every machine: by
/// when each was made, then by its id. A frame two stacks claim (two
/// editors, two machines) is shown in the first, so it is the same one
/// wherever the folder is read.
pub fn sort(list: &mut [Stack]) {
    list.sort_by(|a, b| (a.made, &a.id).cmp(&(b.made, &b.id)));
}

/// A stack as a list shows it: its place among the stacks it was
/// judged with, its members the list has, each once, in its order, the
/// top first, each one's place among its references, and whether it is
/// open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown<K> {
    pub at: usize,
    pub members: Vec<K>,
    pub refs: Vec<usize>,
    pub open: bool,
}

/// The one rule of what a list shows of the stacks, the editor's and
/// the command line's alike. `stacks` come in [`sort`]'s order, each
/// with its place; `place` says where the list has a member, none when
/// it has not (its path not listed, its version gone from the
/// sidecar). A frame is shown in the first stack that has it; the
/// first member a stack keeps is its top; a stack keeping fewer than
/// two is no stack in the list.
pub fn shown<'a, K: Clone + Eq + std::hash::Hash>(
    stacks: impl IntoIterator<Item = (usize, &'a Stack)>,
    place: &dyn Fn(&Reference) -> Option<K>,
) -> Vec<Shown<K>> {
    let mut claimed: HashSet<K> = HashSet::new();
    let mut out = Vec::new();
    for (at, s) in stacks {
        let mut members = Vec::new();
        let mut refs = Vec::new();
        for (i, r) in s.refs.iter().enumerate() {
            if let Some(k) = place(r)
                && claimed.insert(k.clone())
            {
                members.push(k);
                refs.push(i);
            }
        }
        if members.len() >= 2 {
            out.push(Shown {
                at,
                members,
                refs,
                open: s.open,
            });
        } else {
            // A stack that is none gives back what it took.
            for k in &members {
                claimed.remove(k);
            }
        }
    }
    out
}

/// What a list of whole files, the command line's, shows of the stacks
/// closed: the files a closed stack keeps out of sight, and for each
/// top's file how many frames its stack shows.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Collapsed {
    pub hidden: HashSet<PathBuf>,
    pub tops: HashMap<PathBuf, usize>,
    /// Each stack shown open, its files in its order, the top's first,
    /// each once: what [`gather`] puts behind the top.
    pub open: Vec<Vec<PathBuf>>,
}

/// An open stack's members put behind its top, in the stack's order,
/// wherever the list's order had them: the list keeps its order for
/// everything else, and the stack stands where its top does. `path`
/// says each item's file; `open` is [`Collapsed::open`].
pub fn gather<T>(list: Vec<T>, path: impl Fn(&T) -> &Path, open: &[Vec<PathBuf>]) -> Vec<T> {
    // Each member's stack and its place there, the top's left out: the
    // top stays where it is, and the rest go after it.
    let mut behind: HashMap<&Path, (usize, usize)> = HashMap::new();
    for (s, files) in open.iter().enumerate() {
        for (i, f) in files.iter().enumerate().skip(1) {
            behind.entry(f.as_path()).or_insert((s, i));
        }
    }
    let tops: HashMap<&Path, usize> = open
        .iter()
        .enumerate()
        .filter_map(|(s, files)| Some((files.first()?.as_path(), s)))
        .collect();
    let mut held: Vec<Vec<(usize, T)>> = (0..open.len()).map(|_| Vec::new()).collect();
    let mut rest: Vec<(Option<usize>, T)> = Vec::with_capacity(list.len());
    for item in list {
        let p = path(&item);
        match behind.get(p) {
            // A member waits for its top.
            Some(&(s, i)) => held[s].push((i, item)),
            None => {
                let top = tops.get(p).copied();
                rest.push((top, item));
            }
        }
    }
    let present: HashSet<usize> = rest.iter().filter_map(|(t, _)| *t).collect();
    let mut out = Vec::with_capacity(rest.len());
    for (top, item) in rest {
        out.push(item);
        if let Some(s) = top {
            let mut members = std::mem::take(&mut held[s]);
            members.sort_by_key(|(i, _)| *i);
            out.extend(members.into_iter().map(|(_, m)| m));
        }
    }
    // Members of a stack whose top the list does not have keep to the
    // end, in their stack's order, rather than be lost.
    for (s, mut members) in held.into_iter().enumerate() {
        if !present.contains(&s) {
            members.sort_by_key(|(i, _)| *i);
            out.extend(members.into_iter().map(|(_, m)| m));
        }
    }
    out
}

/// The stacks over a list of files by [`shown`]'s rule. `versions`
/// says, of a member's path, the ids of the versions its file's sidecar
/// keeps, none when the list has no file there (the index's row
/// missing, or no row). A file is hidden when every frame of it, its
/// own and each version, is a member behind a closed stack's top. With
/// `open` true, every stack is taken as open and nothing is hidden.
pub fn collapsed(
    stacks: &[Stack],
    open: bool,
    versions: &dyn Fn(&str) -> Option<Vec<String>>,
) -> Collapsed {
    type Frame = (String, Option<String>);
    let place = |r: &Reference| -> Option<Frame> {
        let kept = versions(&r.hint)?;
        match &r.version {
            Some(v) if !kept.contains(v) => None,
            v => Some((r.hint.clone(), v.clone())),
        }
    };
    let mut out = Collapsed::default();
    let mut behind: HashSet<Frame> = HashSet::new();
    for s in shown(stacks.iter().enumerate(), &place) {
        out.tops
            .insert(PathBuf::from(&s.members[0].0), s.members.len());
        if !open && !s.open {
            behind.extend(s.members[1..].iter().cloned());
        } else {
            let mut files: Vec<PathBuf> = Vec::new();
            for (hint, _) in &s.members {
                let f = PathBuf::from(hint);
                if !files.contains(&f) {
                    files.push(f);
                }
            }
            out.open.push(files);
        }
    }
    let files: HashSet<&String> = behind.iter().map(|(hint, _)| hint).collect();
    for hint in files {
        let all = std::iter::once(None)
            .chain(versions(hint).unwrap_or_default().into_iter().map(Some))
            .all(|v| behind.contains(&(hint.clone(), v)));
        if all {
            out.hidden.insert(PathBuf::from(hint));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-stacks-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn r(hash: &str, hint: &str, version: Option<&str>) -> Reference {
        Reference {
            hash: hash.into(),
            hint: hint.into(),
            version: version.map(str::to_string),
        }
    }

    /// A stack written reads back as it was, its order, its versions
    /// and whether it is open; a closed one writes no `open` key and a
    /// file's own frame no `version`. A file of a later build, or one
    /// that will not parse, is left alone.
    #[test]
    fn a_stack_round_trips_and_a_later_one_is_left_alone() {
        let dir = scratch("trip");
        let mut s = Stack::new(vec![
            r("b", "/shoot/b.CR3", None),
            r("a", "/shoot/a.CR3", None),
            r("a", "/shoot/a.CR3", Some("v1")),
        ]);
        s.write(&dir).unwrap();
        let text = std::fs::read_to_string(file_of(&dir, &s.id)).unwrap();
        assert!(!text.contains("\"open\""), "{text}");
        assert_eq!(
            text.matches("\"version\"").count(),
            2,
            "the file's and v1's"
        );
        let back = read_folder(&dir, VERSION);
        assert_eq!(back.list, [s.clone()]);
        assert!(back.stamps.contains_key(&s.id));
        s.open = true;
        s.write(&dir).unwrap();
        assert!(read_folder(&dir, VERSION).list[0].open);

        std::fs::write(
            dir.join("later.json"),
            r#"{"version": 9, "refs": [{"hash": "x", "hint": "/x"}]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("broken.json"), "{").unwrap();
        let back = read_folder(&dir, VERSION);
        assert_eq!(back.list.len(), 1);
        assert_eq!(
            std::fs::read_to_string(dir.join("broken.json")).unwrap(),
            "{"
        );
        // Not there yet: none.
        assert!(read_folder(&dir.join("none"), VERSION).list.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The collections as a build before stacks reads them: the stacks'
    /// folder among the collections is passed over, and every
    /// collection reads as it did.
    #[test]
    fn a_collections_folder_with_stacks_reads_as_before() {
        let dir = scratch("old");
        let mut c = crate::collections::Collection::new("Picks");
        c.add(&[("h".into(), PathBuf::from("/shoot/a.CR3"), None)]);
        c.write(&dir).unwrap();
        let before = crate::collections::read_folder(&dir, crate::collections::PLAIN_VERSION);
        Stack::new(vec![
            r("a", "/shoot/a.CR3", None),
            r("b", "/shoot/b.CR3", None),
        ])
        .write(&dir_in(&dir))
        .unwrap();
        let after = crate::collections::read_folder(&dir, crate::collections::PLAIN_VERSION);
        assert_eq!(after.list, before.list);
        assert_eq!(after.list, [c]);
        assert!(after.smart.is_empty());
        // And a folder of 0.7.0's, with no stacks in it, has none.
        std::fs::remove_dir_all(dir_in(&dir)).unwrap();
        assert!(read_folder(&dir_in(&dir), VERSION).list.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A list of files shows a closed stack's top alone, counted; the
    /// first member there is the top, a version the sidecar no longer
    /// has is not there; a stack with one member there is no stack; a
    /// file with a version shown is listed though its own is behind;
    /// open, nothing is hidden.
    #[test]
    fn a_closed_stack_hides_its_members_behind_the_first_there() {
        let mut open = Stack::new(vec![r("c", "/c", None), r("d", "/d", None)]);
        open.open = true;
        let stacks = [
            Stack::new(vec![
                r("gone", "/gone", None),
                r("a", "/a", Some("lost")),
                r("a", "/a", None),
                r("a", "/a", Some("v")),
                r("b", "/b", None),
                r("f", "/f", None),
            ]),
            Stack::new(vec![r("e", "/e", None), r("lost", "/lost", None)]),
            open,
        ];
        let versions = |hint: &str| match hint {
            "/gone" | "/lost" => None,
            "/a" => Some(vec!["v".to_string()]),
            "/f" => Some(vec!["w".to_string()]),
            _ => Some(Vec::new()),
        };
        let shown = collapsed(&stacks, false, &versions);
        assert_eq!(shown.hidden, [PathBuf::from("/b")].into(), "f's w shows");
        assert_eq!(
            shown.tops,
            [(PathBuf::from("/a"), 4), (PathBuf::from("/c"), 2)].into()
        );
        let all = collapsed(&stacks, true, &versions);
        assert!(all.hidden.is_empty());
        assert_eq!(all.tops, shown.tops);
    }

    /// Two stacks claiming one frame (two editors' files): the first in
    /// the kept order has it, and the second, left with one, is none, so
    /// its other frame is plain. The order is by when each was made,
    /// whatever order the folder reads in.
    #[test]
    fn a_frame_two_stacks_claim_is_the_first_s() {
        let mut a = Stack::new(vec![r("1", "/1", None), r("2", "/2", None)]);
        let mut b = Stack::new(vec![r("2", "/2", None), r("3", "/3", None)]);
        a.made = 10;
        b.made = 20;
        let mut stacks = vec![b, a];
        sort(&mut stacks);
        assert_eq!(stacks[0].made, 10);
        let all = |_: &str| Some(Vec::new());
        let shown = collapsed(&stacks, false, &all);
        assert_eq!(shown.hidden, [PathBuf::from("/2")].into());
        assert_eq!(shown.tops, [(PathBuf::from("/1"), 2)].into());
        // Without the first, the second is a stack again.
        let shown = collapsed(&stacks[1..], false, &all);
        assert_eq!(shown.tops, [(PathBuf::from("/2"), 2)].into());
    }

    /// An open stack's members follow its top in the stack's order,
    /// wherever the list's order put them; the rest keep the list's
    /// order, and a member whose top the list lacks is kept at the end.
    #[test]
    fn gather_puts_an_open_stacks_members_behind_its_top() {
        let p = |s: &str| PathBuf::from(s);
        let list = vec![p("/a"), p("/m2"), p("/b"), p("/top"), p("/m1"), p("/c")];
        let open = vec![vec![p("/top"), p("/m1"), p("/m2")]];
        assert_eq!(
            gather(list.clone(), |f| f.as_path(), &open),
            [p("/a"), p("/b"), p("/top"), p("/m1"), p("/m2"), p("/c")]
        );
        let without_top: Vec<PathBuf> = list.into_iter().filter(|f| f != &p("/top")).collect();
        assert_eq!(
            gather(without_top, |f| f.as_path(), &open),
            [p("/a"), p("/b"), p("/c"), p("/m1"), p("/m2")]
        );
        assert_eq!(gather(vec![p("/x")], |f| f.as_path(), &[]), [p("/x")]);
    }
}
