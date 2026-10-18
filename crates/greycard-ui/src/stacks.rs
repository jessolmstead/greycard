//! Stacks: frames of one folder grouped by hand, one of them on top.
//!
//! A stack is kept beside the collections, one small file a stack
//! (`greycard_library::stacks`), and is per machine as they are. Its
//! members are references as a collection's are: the picture's hash,
//! the path it was last seen at, and the version, none for the file's
//! own; their order is the stack's, the top first.
//!
//! In the list a stack is found by its members' paths: a member is
//! listed when its path is a listed file's and its version is one the
//! sidecar has. The first member listed is the top; a stack with fewer
//! than two listed is no stack in that list, and its members are plain
//! frames. Closed, a stack is its top's cell alone, where the top
//! stands in the list, and its other members have no cell: the arrows,
//! the filter, the lightbox, culling and an export see one frame. Open,
//! its members follow the top in the stack's order. A collection's
//! list is its own order, and shows no stacks.
//!
//! The paths follow the file: a pass that finds a member moved, or
//! rewritten in place, carries its reference along (`Store::heal`).
//! A frame the window deletes, or moves out of its folder, leaves its
//! stack (`Store::left`); a stack down to one member is no stack, and
//! its file goes.
//!
//! The window's side, the menu, the keys and the badge, is
//! `panel::stacks`.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use greycard_library::Rehash;
use greycard_library::collections::{Reference, Stamp, hint_of, stamp_of};
pub(crate) use greycard_library::stacks::Stack;
use greycard_library::stacks::{VERSION, file_of, read_folder, read_stack};

use crate::State;
use crate::versions::Cell;

/// A member as the store tells it from another: its path and its
/// version. Not the hash, which two copies of one picture share: two
/// identical files in one folder are two frames.
type Frame = (String, Option<String>);

fn frame_of(r: &Reference) -> Frame {
    (r.hint.clone(), r.version.clone())
}

/// The stacks as the editor holds them.
#[derive(Default)]
pub(crate) struct Store {
    /// None for a session that keeps nothing (a test's, unless it gives
    /// one), or with no data directory.
    pub(crate) dir: Option<PathBuf>,
    pub(crate) list: Vec<Stack>,
    /// Each file's content hash as it was read or last written here.
    stamps: HashMap<String, Stamp>,
    /// Each member's path to the stacks it is in, by their place in
    /// `list`: what a listed file is looked up by.
    by_hint: HashMap<String, Vec<usize>>,
    /// A folder listed from the disk, to its path as the index keys it,
    /// asked once a session.
    canon: RefCell<HashMap<PathBuf, PathBuf>>,
}

impl Store {
    /// Every stack in `dir`. A file that will not read, or one a later
    /// build wrote, is said and left where it is.
    pub(crate) fn load(dir: PathBuf) -> Store {
        let folder = read_folder(&dir, VERSION);
        let mut store = Store {
            dir: Some(dir.clone()),
            list: folder.list,
            stamps: folder.stamps,
            ..Store::default()
        };
        store.index();
        tracing::info!("stacks: {} from {}", store.list.len(), dir.display());
        store
    }

    /// `by_hint` made again from the list.
    fn index(&mut self) {
        self.by_hint.clear();
        for (i, s) in self.list.iter().enumerate() {
            for r in &s.refs {
                let at = self.by_hint.entry(r.hint.clone()).or_default();
                if !at.contains(&i) {
                    at.push(i);
                }
            }
        }
    }

    pub(crate) fn stack(&self, id: &str) -> Option<&Stack> {
        self.list.iter().find(|s| s.id == id)
    }

    fn stack_mut(&mut self, id: &str) -> Option<&mut Stack> {
        self.list.iter_mut().find(|s| s.id == id)
    }

    /// Stack `id` written to its file. A file changed on disk since it
    /// was read or written here (another editor, a hand) is taken as a
    /// collection's is, less the join, a stack being an order and a
    /// top, whole: one gone was unstacked there, and the stack
    /// goes here too rather than come back; one a later build wrote is
    /// never written over; one that will not read is kept beside itself
    /// as `.json.bad` first; one that reads is written over. Each is
    /// said in the log.
    fn save(&mut self, id: &str) {
        let Some(dir) = self.dir.clone() else {
            return;
        };
        let file = file_of(&dir, id);
        if self.stamps.contains_key(id) && !file.exists() {
            tracing::warn!(
                "stack {}: gone from the disk since it was read (unstacked elsewhere); \
                 dropped here too",
                file.display()
            );
            self.list.retain(|s| s.id != id);
            self.stamps.remove(id);
            self.index();
            return;
        }
        if let Some(now) = stamp_of(&file)
            && self.stamps.get(id) != Some(&now)
        {
            match read_stack(&file) {
                Ok(f) if f.version > VERSION => {
                    tracing::warn!(
                        "stack {}: a later build wrote it since; left alone",
                        file.display()
                    );
                    return;
                }
                Ok(_) => tracing::warn!(
                    "stack {}: changed on disk since it was read; ours written over it",
                    file.display()
                ),
                Err(e) => {
                    let aside = file.with_extension("json.bad");
                    let kept = match std::fs::copy(&file, &aside) {
                        Ok(_) => format!("kept as {}", aside.display()),
                        Err(c) => format!("could not keep a copy ({c})"),
                    };
                    tracing::warn!(
                        "stack {}: changed on disk and unreadable ({e}); {kept}; ours written over it",
                        file.display()
                    );
                }
            }
        }
        let Some(s) = self.stack(id) else {
            return;
        };
        match s.write(&dir) {
            Ok(stamp) => {
                self.stamps.insert(id.to_string(), stamp);
            }
            Err(e) => tracing::warn!("stack not saved to {}: {e}", file.display()),
        }
    }

    /// Stack `id` gone, with its file.
    fn delete(&mut self, id: &str) {
        self.list.retain(|s| s.id != id);
        self.stamps.remove(id);
        if let Some(dir) = &self.dir {
            let file = file_of(dir, id);
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!("stack {}: not deleted: {e}", file.display()),
            }
        }
    }

    /// The stacks `changed` written, each one left with fewer than two
    /// members gone instead, and the lookup made again.
    fn settle(&mut self, changed: &BTreeSet<String>) {
        for id in changed {
            match self.stack(id) {
                Some(s) if s.refs.len() < 2 => self.delete(id),
                Some(_) => self.save(id),
                None => {}
            }
        }
        self.index();
    }

    /// A new stack of `refs`, the first on top. A frame in another stack
    /// leaves it first: a frame is in one stack. Its id.
    pub(crate) fn make(&mut self, refs: Vec<Reference>) -> String {
        let frames: HashSet<Frame> = refs.iter().map(frame_of).collect();
        let mut changed = self.take_out(&frames);
        let stack = Stack::new(refs);
        let id = stack.id.clone();
        self.list.push(stack);
        greycard_library::stacks::sort(&mut self.list);
        changed.insert(id.clone());
        self.settle(&changed);
        id
    }

    /// The frames `frames` out of every stack they are in, nothing
    /// written: the ids of the stacks they left.
    fn take_out(&mut self, frames: &HashSet<Frame>) -> BTreeSet<String> {
        let mut changed = BTreeSet::new();
        for s in &mut self.list {
            let before = s.refs.len();
            s.refs.retain(|r| !frames.contains(&frame_of(r)));
            if s.refs.len() != before {
                changed.insert(s.id.clone());
            }
        }
        changed
    }

    /// The frames `frames` (each a path and a version) out of every
    /// stack: a version deleted. True when a stack changed.
    fn drop_frames(&mut self, frames: &HashSet<Frame>) -> bool {
        let changed = self.take_out(frames);
        self.settle(&changed);
        !changed.is_empty()
    }

    /// Stack `id` no more: its frames are plain frames again.
    pub(crate) fn unstack(&mut self, id: &str) -> bool {
        let had = self.stack(id).is_some();
        self.delete(id);
        self.index();
        had
    }

    /// The member at `at` in stack `id`'s references on top, the others
    /// keeping their order behind it. True when it moved.
    pub(crate) fn set_top_at(&mut self, id: &str, at: usize) -> bool {
        let Some(s) = self.stack_mut(id) else {
            return false;
        };
        if at == 0 || at >= s.refs.len() {
            return false;
        }
        let r = s.refs.remove(at);
        s.refs.insert(0, r);
        self.save(id);
        true
    }

    /// The members at `at` in stack `id`'s references out of it; the
    /// stack goes when fewer than two are left. How many came out.
    pub(crate) fn remove_at(&mut self, id: &str, at: &[usize]) -> usize {
        let Some(s) = self.stack_mut(id) else {
            return 0;
        };
        let before = s.refs.len();
        let mut i = 0;
        s.refs.retain(|_| {
            i += 1;
            !at.contains(&(i - 1))
        });
        let n = before - s.refs.len();
        if n > 0 {
            self.settle(&BTreeSet::from([id.to_string()]));
        }
        n
    }

    /// Stack `id` open, or closed.
    pub(crate) fn set_open(&mut self, id: &str, open: bool) -> bool {
        let Some(s) = self.stack_mut(id) else {
            return false;
        };
        if s.open == open {
            return false;
        }
        s.open = open;
        self.save(id);
        true
    }

    /// The files at `paths` (as the index keys them) gone from their
    /// folder by the window's own hand, deleted or moved out: every
    /// frame of each leaves its stack. True when a stack changed.
    pub(crate) fn left(&mut self, paths: &[PathBuf]) -> bool {
        let hints: HashSet<String> = paths.iter().map(|p| hint_of(p)).collect();
        if !hints.iter().any(|h| self.by_hint.contains_key(h)) {
            return false;
        }
        let mut changed = BTreeSet::new();
        for s in &mut self.list {
            let before = s.refs.len();
            s.refs.retain(|r| !hints.contains(&r.hint));
            if s.refs.len() != before {
                changed.insert(s.id.clone());
            }
        }
        self.settle(&changed);
        !changed.is_empty()
    }

    /// What an index pass found, carried into the members: a file found
    /// renamed in its folder takes its references to the new name, one
    /// found moved to another folder leaves its stack (a stack is one
    /// folder's, and the frame is no longer in it), and one rewritten in
    /// place gives the references at its path its new hash. True when a
    /// stack changed.
    pub(crate) fn heal(&mut self, moved: &[(PathBuf, PathBuf)], rehashed: &[Rehash]) -> bool {
        if self.list.is_empty() || (moved.is_empty() && rehashed.is_empty()) {
            return false;
        }
        let (renamed, away): (Vec<_>, Vec<_>) = moved
            .iter()
            .partition(|(from, to)| from.parent() == to.parent());
        let to: HashMap<String, String> = renamed
            .iter()
            .map(|(from, to)| (hint_of(from), hint_of(to)))
            .collect();
        let away: HashSet<String> = away.iter().map(|(from, _)| hint_of(from)).collect();
        let rewritten: HashMap<(String, &str), &str> = rehashed
            .iter()
            .map(|r| ((hint_of(&r.path), r.old.as_str()), r.new.as_str()))
            .collect();
        let mut changed = BTreeSet::new();
        for s in &mut self.list {
            let before = s.refs.len();
            s.refs.retain(|r| !away.contains(&r.hint));
            if s.refs.len() != before {
                changed.insert(s.id.clone());
            }
            for r in &mut s.refs {
                if let Some(new) = to.get(&r.hint) {
                    r.hint = new.clone();
                    changed.insert(s.id.clone());
                }
                if let Some(new) = rewritten.get(&(r.hint.clone(), r.hash.as_str())) {
                    r.hash = (*new).to_string();
                    changed.insert(s.id.clone());
                }
            }
        }
        for id in &changed {
            tracing::info!("stack {id}: its members followed the index's pass");
        }
        self.settle(&changed);
        !changed.is_empty()
    }
}

/// A stack as the list has it: its place in the store, its members
/// that are listed (their file listed, their version on the sidecar),
/// each once, in its order, the top first; and whether it is open.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Listed {
    pub(crate) id: String,
    pub(crate) members: Vec<Cell>,
    /// Each member's place among the stack's references.
    pub(crate) refs: Vec<usize>,
    pub(crate) open: bool,
}

/// The path file `file` is kept at in a stack: as the index keys it.
/// The list of a view of the index is in those paths already; a folder
/// listed from the disk is asked its own once a session; a frame the
/// window follows on an archive's copy is its local file's.
pub(crate) fn hint_of_file(st: &State, file: usize) -> Option<String> {
    let path = st.files.get(file)?;
    let path = crate::sync::local_of(st, path).unwrap_or(path);
    if st.views.view.lists_rows() {
        return Some(hint_of(path));
    }
    let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
        return Some(hint_of(path));
    };
    let mut canon = st.views.stacks.canon.borrow_mut();
    let key = canon
        .entry(folder.to_path_buf())
        .or_insert_with(|| greycard_library::key_folder(folder));
    Some(hint_of(&key.join(name)))
}

/// The stacks the list shows: each with at least two members listed,
/// in the order of the store. None while a collection is the list.
pub(crate) fn listed(st: &State) -> Vec<Listed> {
    let store = &st.views.stacks;
    if store.list.is_empty() || st.views.collection_frames.is_some() {
        return Vec::new();
    }
    let mut touched: BTreeSet<usize> = BTreeSet::new();
    let mut file_at: HashMap<String, usize> = HashMap::new();
    for f in 0..st.files.len() {
        let Some(hint) = hint_of_file(st, f) else {
            continue;
        };
        if let Some(at) = store.by_hint.get(&hint) {
            touched.extend(at.iter().copied());
            file_at.insert(hint, f);
        }
    }
    // The library's rule, the command line's too: a stack none of whose
    // members is listed claims nothing, so the stacks touched suffice.
    let place = |r: &Reference| -> Option<Cell> {
        let file = *file_at.get(&r.hint)?;
        if let Some(v) = r.version.as_deref()
            && st
                .sidecars
                .get(file)
                .is_none_or(|sc| sc.version(v).is_none())
        {
            return None;
        }
        Some(Cell {
            file,
            version: r.version.as_deref().map(Arc::from),
        })
    };
    greycard_library::stacks::shown(touched.into_iter().map(|i| (i, &store.list[i])), &place)
        .into_iter()
        .map(|s| Listed {
            id: store.list[s.at].id.clone(),
            members: s.members,
            refs: s.refs,
            open: s.open,
        })
        .collect()
}

/// The cells `natural` (each file's own and its versions, in order)
/// with the stacks `listed` laid over them: a stack where its top
/// stands, its other members after it when it is open and nowhere when
/// it is closed.
pub(crate) fn arrange(natural: Vec<Cell>, listed: &[Listed]) -> Vec<Cell> {
    if listed.is_empty() {
        return natural;
    }
    let mut at: HashMap<&Cell, (usize, usize)> = HashMap::new();
    for (s, l) in listed.iter().enumerate() {
        for (i, m) in l.members.iter().enumerate() {
            at.insert(m, (s, i));
        }
    }
    let mut out = Vec::with_capacity(natural.len());
    for c in &natural {
        match at.get(c) {
            None => out.push(c.clone()),
            Some(&(s, 0)) => {
                out.push(c.clone());
                if listed[s].open {
                    out.extend(listed[s].members[1..].iter().cloned());
                }
            }
            Some(_) => {}
        }
    }
    out
}

/// The versions the list wants of file `file`, a member of a stack the
/// cells were last made with (`Views::stacks_listed`): the stacks that
/// hold it laid over their files alone, as the sidecars are now (a
/// member whose version a read took out is none). A version a read
/// brought that is a member stands as a plain one until the cells are
/// made again, which its own difference asks for.
pub(crate) fn wanted_of_file(st: &State, file: usize) -> Vec<Option<Arc<str>>> {
    let has = |c: &Cell| match c.version.as_deref() {
        None => true,
        Some(v) => st
            .sidecars
            .get(c.file)
            .is_some_and(|s| s.version(v).is_some()),
    };
    let stacks: Vec<Listed> = st
        .views
        .stacks_listed
        .iter()
        .filter(|l| l.members.iter().any(|m| m.file == file))
        .map(|l| {
            let (members, refs) = l
                .members
                .iter()
                .zip(&l.refs)
                .filter(|(m, _)| has(m))
                .map(|(m, r)| (m.clone(), *r))
                .unzip();
            Listed {
                members,
                refs,
                ..l.clone()
            }
        })
        .filter(|l| l.members.len() >= 2)
        .collect();
    let files: BTreeSet<usize> = std::iter::once(file)
        .chain(stacks.iter().flat_map(|l| l.members.iter().map(|m| m.file)))
        .collect();
    let natural: Vec<Cell> = files
        .iter()
        .flat_map(|&f| {
            std::iter::once(Cell {
                file: f,
                version: None,
            })
            .chain(st.sidecars.get(f).into_iter().flat_map(move |s| {
                s.versions.iter().map(move |v| Cell {
                    file: f,
                    version: Some(Arc::from(v.id.as_str())),
                })
            }))
        })
        .collect();
    arrange(natural, &stacks)
        .into_iter()
        .filter(|c| c.file == file)
        .map(|c| c.version)
        .collect()
}

/// The files a closed stack keeps out of the list, every frame of
/// them behind a top, by file: what the filter's counts leave out, so a
/// chip counts the frames it would show. None when no stack is listed.
pub(crate) fn behind(st: &State) -> Option<Vec<bool>> {
    if st.views.stacks_listed.is_empty() || st.views.collection_frames.is_some() {
        return None;
    }
    let mut out = vec![true; st.files.len()];
    for c in &st.cells {
        if let Some(b) = out.get_mut(c.file) {
            *b = false;
        }
    }
    Some(out)
}

/// Whether `cells` stand as `versions::cells_of` makes them, some left
/// out: the files in order, each file's cells together, its own first.
pub(crate) fn in_order(cells: &[Cell]) -> bool {
    cells
        .windows(2)
        .all(|w| w[0].file < w[1].file || (w[0].file == w[1].file && w[1].version.is_some()))
}

/// Each file's cells in `cells`, by file, for `files` files, when the
/// cells are not [`in_order`]: what the lookups by file read then.
pub(crate) fn by_file(cells: &[Cell], files: usize) -> Option<Vec<Vec<usize>>> {
    if in_order(cells) {
        return None;
    }
    let mut out = vec![Vec::new(); files];
    for (c, cell) in cells.iter().enumerate() {
        if let Some(of) = out.get_mut(cell.file) {
            of.push(c);
        }
    }
    Some(out)
}

/// What a cell of a stack wears: how many frames the stack shows, the
/// cell's place in it from 1, the top's, and whether it is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Mark {
    pub(crate) count: usize,
    pub(crate) place: usize,
    pub(crate) open: bool,
}

/// The marks of the cells of the stacks `listed`: a closed stack's
/// top, and every member of an open one.
pub(crate) fn marks(listed: &[Listed]) -> HashMap<Cell, Mark> {
    let mut out = HashMap::new();
    for l in listed {
        for (i, m) in l.members.iter().enumerate() {
            if i == 0 || l.open {
                out.insert(
                    m.clone(),
                    Mark {
                        count: l.members.len(),
                        place: i + 1,
                        open: l.open,
                    },
                );
            }
        }
    }
    out
}

/// The stack cell `c` is a member of in the list, and its place in it
/// from 0, when it is one.
pub(crate) fn stack_of(st: &State, c: usize) -> Option<(Listed, usize)> {
    let cell = st.cell(c)?;
    listed(st).into_iter().find_map(|l| {
        let at = l.members.iter().position(|m| m == cell)?;
        Some((l, at))
    })
}

/// The files `gone` (by their number in the list) deleted or moved out
/// of their folder by the window: each leaves its stack, before the
/// list forgets where they were. True when a stack changed.
pub(crate) fn left_files(st: &mut State, gone: &[usize]) -> bool {
    if st.views.stacks.list.is_empty() {
        return false;
    }
    let paths: Vec<PathBuf> = gone
        .iter()
        .filter_map(|&f| hint_of_file(st, f))
        .map(PathBuf::from)
        .collect();
    st.views.stacks.left(&paths)
}

/// Version `id` of file `file` deleted by the window: it leaves its
/// stack, as a frame deleted does. True when a stack changed.
pub(crate) fn version_left(st: &mut State, file: usize, id: &str) -> bool {
    let Some(hint) = hint_of_file(st, file) else {
        return false;
    };
    st.views
        .stacks
        .drop_frames(&HashSet::from([(hint, Some(id.to_string()))]))
}

/// The folder of file `file` as a stack keeps it, for the rule that a
/// stack is of one: a frame followed on an archive's copy is its local
/// file's.
pub(crate) fn folder_of(st: &State, file: usize) -> Option<PathBuf> {
    PathBuf::from(hint_of_file(st, file)?)
        .parent()
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn cell(file: usize, version: Option<&str>) -> Cell {
        Cell {
            file,
            version: version.map(Arc::from),
        }
    }

    fn r(path: &str, version: Option<&str>) -> Reference {
        Reference {
            hash: format!("h-{path}"),
            hint: path.into(),
            version: version.map(str::to_string),
        }
    }

    /// Closed, a stack is its top where the top stands; open, its
    /// members follow the top in the stack's order; a cell of no stack
    /// stays where it is.
    #[test]
    fn a_stack_is_laid_over_the_list_closed_and_open() {
        let natural = vec![
            cell(0, None),
            cell(1, None),
            cell(1, Some("v")),
            cell(2, None),
            cell(3, None),
        ];
        let mut stack = Listed {
            id: "s".into(),
            members: vec![cell(2, None), cell(0, None), cell(1, Some("v"))],
            refs: vec![0, 1, 2],
            open: false,
        };
        let closed = arrange(natural.clone(), std::slice::from_ref(&stack));
        assert_eq!(closed, [cell(1, None), cell(2, None), cell(3, None)]);
        assert!(in_order(&closed));
        stack.open = true;
        let open = arrange(natural.clone(), std::slice::from_ref(&stack));
        assert_eq!(
            open,
            [
                cell(1, None),
                cell(2, None),
                cell(0, None),
                cell(1, Some("v")),
                cell(3, None)
            ]
        );
        assert!(!in_order(&open));
        assert!(in_order(&natural));
        // The marks: the top alone closed, each member open.
        let m = marks(std::slice::from_ref(&stack));
        assert_eq!(m.len(), 3);
        assert_eq!(
            m[&cell(0, None)],
            Mark {
                count: 3,
                place: 2,
                open: true
            }
        );
        stack.open = false;
        let m = marks(std::slice::from_ref(&stack));
        assert_eq!(m.len(), 1);
        assert_eq!(m[&cell(2, None)].place, 1);
    }

    /// The store: a frame is in one stack, so a new stack takes it from
    /// the one it was in; a stack down to one member goes with its
    /// file; the top moves; a frame deleted leaves; a pass's move and
    /// rewrite carry the references.
    #[test]
    fn the_store_keeps_a_frame_in_one_stack_and_drops_a_stack_of_one() {
        let dir = crate::testing::scratch_dir("stacks-store");
        let mut store = Store::load(dir.clone());
        let first = store.make(vec![r("/s/a", None), r("/s/b", None), r("/s/c", None)]);
        assert!(file_of(&dir, &first).exists());
        // b and d into a second: the first keeps a and c.
        let second = store.make(vec![r("/s/b", None), r("/s/d", None)]);
        assert_eq!(store.stack(&first).unwrap().refs.len(), 2);
        assert_eq!(store.by_hint["/s/b"].len(), 1);
        // c out of the first: one left, no stack.
        assert_eq!(store.remove_at(&first, &[1]), 1, "a and c left; c at 1");
        assert!(store.stack(&first).is_none());
        assert!(!file_of(&dir, &first).exists());
        // The top.
        assert!(store.set_top_at(&second, 1));
        assert!(!store.set_top_at(&second, 0), "on top already");
        assert_eq!(store.stack(&second).unwrap().refs[0].hint, "/s/d");
        assert!(store.set_open(&second, true));
        assert!(!store.set_open(&second, true));
        // Read back as written.
        let back = Store::load(dir.clone());
        assert_eq!(back.list, store.list);
        // A pass found d renamed in its folder and b rewritten.
        let third = store.make(vec![r("/s/e", None), r("/s/f", None), r("/s/g", None)]);
        assert!(store.heal(
            &[(PathBuf::from("/s/d"), PathBuf::from("/s/d2"))],
            &[Rehash {
                path: PathBuf::from("/s/b"),
                old: "h-/s/b".into(),
                new: "new".into()
            }]
        ));
        let s = store.stack(&second).unwrap();
        assert_eq!(s.refs[0].hint, "/s/d2");
        assert_eq!(s.refs[1].hash, "new");
        assert!(store.by_hint.contains_key("/s/d2"));
        // g moved to another folder: it leaves its stack.
        assert!(store.heal(&[(PathBuf::from("/s/g"), PathBuf::from("/t/g"))], &[]));
        let hints: Vec<&str> = store
            .stack(&third)
            .unwrap()
            .refs
            .iter()
            .map(|r| r.hint.as_str())
            .collect();
        assert_eq!(hints, ["/s/e", "/s/f"]);
        // e deleted by the window: one left, no stack.
        assert!(!store.left(&[PathBuf::from("/nowhere")]));
        assert!(store.left(&[PathBuf::from("/s/e")]));
        assert!(store.stack(&third).is_none());
        assert!(store.unstack(&second));
        assert!(store.list.is_empty());
        assert!(Store::load(dir.clone()).list.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A stack's file changed on disk since it was read: gone (another
    /// editor unstacked it), the stack goes here too and is not written
    /// back; a later build's is never written over; one broken by hand
    /// is kept as `.json.bad` before ours goes over it.
    #[test]
    fn a_file_changed_on_disk_is_taken_as_it_stands() {
        let dir = crate::testing::scratch_dir("stacks-disk");
        let mut store = Store::load(dir.clone());
        let gone = store.make(vec![r("/s/a", None), r("/s/b", None)]);
        std::fs::remove_file(file_of(&dir, &gone)).unwrap();
        store.set_open(&gone, true);
        assert!(store.stack(&gone).is_none());
        assert!(!store.by_hint.contains_key("/s/a"));
        assert!(!file_of(&dir, &gone).exists(), "not written back");

        let later = store.make(vec![r("/s/c", None), r("/s/d", None)]);
        let text = r#"{"version": 9, "refs": []}"#;
        std::fs::write(file_of(&dir, &later), text).unwrap();
        store.set_open(&later, true);
        assert_eq!(
            std::fs::read_to_string(file_of(&dir, &later)).unwrap(),
            text
        );

        let broken = store.make(vec![r("/s/e", None), r("/s/f", None)]);
        std::fs::write(file_of(&dir, &broken), "{").unwrap();
        store.set_open(&broken, true);
        let aside = file_of(&dir, &broken).with_extension("json.bad");
        assert_eq!(std::fs::read_to_string(aside).unwrap(), "{");
        assert!(read_stack(&file_of(&dir, &broken)).unwrap().open);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
