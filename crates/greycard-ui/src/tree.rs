//! The open root's folders as a tree in the left pane: shown while a
//! root's view is open (its row, or a folder of it), and for a folder
//! opened from the disk that lies under a root, that folder marked;
//! each folder with its frame count, a click opening that folder's
//! frames.
//!
//! The tree is the index's, not the disk's: every folder the index
//! holds frames in under the root, and the folders above them up to
//! the root, so an offline root still shows its shape and nothing is
//! listed on disk to draw it. The counts are one `GROUP BY` over the
//! folder column, read off the window's thread on a connection of the
//! build's own, and the tree handed over whole, as a view's rows are.
//!
//! A folder opened from the tree is a view of the index narrowed to
//! it ([`View::Branch`]): its own rows, or with the switch those under
//! it too, each frame standing in from its row until something needs
//! its sidecar, and the root's row staying lit. Recently opened records
//! it as the folder it is, through the browser's open as every list.
//!
//! The folders under the one open are shown in the grid too, as tiles
//! above its cells, each with the frames under it: a click on one opens
//! it exactly as its row does. With the switch on there are none, since
//! the list already holds everything under the folder.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::roots::View;
use crate::worker::Worker;
use crate::{App, FolderRow, FolderTile, Shared, State, tr};

/// A folder of the tree: where it is, what it is called, how deep
/// under the root (the root itself is 0), the frames directly in it
/// and under it in all, and whether it has folders of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) depth: usize,
    pub(crate) direct: usize,
    pub(crate) total: usize,
    pub(crate) children: bool,
}

/// What the window keeps of the tree: the nodes of the root it was
/// last built for, the build out, which folders are unfolded, the
/// switch, and the pane's model.
#[derive(Default)]
pub(crate) struct Tree {
    /// A folder chosen opens with the frames of those under it too.
    /// `settings.folder_tree_subfolders`.
    pub(crate) subfolders: bool,
    /// The root the nodes are of, and its count in the index when they
    /// were asked for: a count moved since has them built again.
    root: Option<PathBuf>,
    built_at: Option<usize>,
    nodes: Vec<Node>,
    /// The number of the build asked for last: an older one landing is
    /// dropped. And what it was asked for, so it is not asked twice.
    token: u64,
    asked: Option<(PathBuf, Option<usize>)>,
    /// A pass changed what is under the root since the tree was built.
    stale: bool,
    /// The folders shown unfolded. A root is unfolded the first time
    /// its tree is shown, and left as the user leaves it after.
    expanded: HashSet<PathBuf>,
    seen: HashSet<PathBuf>,
    /// The folder last marked open: a folder marked anew has the
    /// folders above it unfolded once, so the mark is in sight however
    /// it was opened, and a fold made after is left alone.
    revealed: Option<PathBuf>,
    /// The open folder whose row the pane last brought into view, once
    /// that row was there: a folder opened anew has its row scrolled
    /// to, and a fold, a resize, or the same folder shown again does
    /// not move the list.
    in_view: RefCell<Option<PathBuf>>,
    /// The folder chosen by a click on its own row, until it is marked:
    /// its row is under the pointer, and the list stays where it is.
    clicked: RefCell<Option<PathBuf>>,
    /// The rows as the pane shows them: one model for the session, its
    /// rows replaced only when they change.
    rows: Rc<VecModel<FolderRow>>,
    /// Each row's folder, in the model's order: a click comes back as
    /// the row's number and is looked up here, since a path through
    /// the window's strings would lose a name that is not UTF-8.
    shown: RefCell<Vec<PathBuf>>,
    /// The grid's folder tiles: one model for the session, as the rows,
    /// and each tile's folder in its order, looked up by number as a
    /// row's is.
    tiles: Rc<VecModel<FolderTile>>,
    tile_folders: RefCell<Vec<PathBuf>>,
    /// A folder opened from the disk under no root has no tree; its
    /// own folders are read from the disk for its tiles: the folder,
    /// and each folder in it as a node.
    loose: Option<(PathBuf, Vec<Node>)>,
    loose_token: u64,
    loose_asked: Option<PathBuf>,
    /// The folder's own folders may have changed: read again at the
    /// next `want`.
    loose_stale: bool,
}

/// The tree of `root`'s folders from the index's counts of the frames
/// directly in each (`Library::folder_counts_under_canonical`): the
/// root first, then each folder after the one it is in, its own folders
/// by name (case folded, then as spelled) before the next. A folder the
/// index has nothing directly in is still there when something under
/// it is. A folder not under the root is left out. The root's name is
/// left empty: the pane names it as the root's row does.
pub(crate) fn build(root: &Path, counts: &[(PathBuf, usize)]) -> Vec<Node> {
    #[derive(Default)]
    struct Dir {
        direct: usize,
        under: BTreeMap<(String, OsString), Dir>,
    }
    let mut top = Dir::default();
    for (folder, n) in counts {
        let Ok(rel) = folder.strip_prefix(root) else {
            continue;
        };
        let mut at = &mut top;
        for c in rel.components() {
            let Component::Normal(name) = c else {
                continue;
            };
            let key = (name.to_string_lossy().to_lowercase(), name.to_os_string());
            at = at.under.entry(key).or_default();
        }
        at.direct += n;
    }
    fn walk(dir: &Dir, path: PathBuf, name: String, depth: usize, out: &mut Vec<Node>) -> usize {
        let at = out.len();
        out.push(Node {
            path: path.clone(),
            name,
            depth,
            direct: dir.direct,
            total: 0,
            children: !dir.under.is_empty(),
        });
        let mut total = dir.direct;
        for ((_, os), sub) in &dir.under {
            let name = os.to_string_lossy().into_owned();
            total += walk(sub, path.join(os), name, depth + 1, out);
        }
        out[at].total = total;
        total
    }
    let mut out = Vec::new();
    walk(&top, root.to_path_buf(), String::new(), 0, &mut out);
    out
}

/// The nodes shown: every one whose folders above it are all unfolded.
pub(crate) fn visible<'a>(nodes: &'a [Node], expanded: &HashSet<PathBuf>) -> Vec<&'a Node> {
    let mut out = Vec::with_capacity(nodes.len());
    let mut folded_at: Option<usize> = None;
    for n in nodes {
        if let Some(d) = folded_at {
            if n.depth > d {
                continue;
            }
            folded_at = None;
        }
        out.push(n);
        if n.children && !expanded.contains(&n.path) {
            folded_at = Some(n.depth);
        }
    }
    out
}

/// The count a row shows: the frames directly in the folder, or folded,
/// all those under it. None for an unfolded folder with nothing of its
/// own (the year above the days), which would only say 0.
pub(crate) fn count_shown(n: &Node, expanded: bool) -> Option<usize> {
    if n.children && expanded {
        (n.direct > 0).then_some(n.direct)
    } else {
        Some(n.total)
    }
}

/// The folders directly under `folder`, as the tree has them: none for
/// a folder that is not in it.
fn children<'a>(nodes: &'a [Node], folder: &Path) -> Vec<&'a Node> {
    let Some(at) = nodes.iter().position(|n| n.path == folder) else {
        return Vec::new();
    };
    let depth = nodes[at].depth;
    nodes[at + 1..]
        .iter()
        .take_while(|n| n.depth > depth)
        .filter(|n| n.depth == depth + 1)
        .collect()
}

/// The folders the grid shows as tiles over the view: those directly
/// under the folder the tree marks, while the list is that folder's own
/// frames. None with the switch on (a folder's view with those under
/// it, or the root's own view, which is everything under the root), for
/// a view with no tree, or before the tree for its root has landed.
pub(crate) fn tiles_for(st: &State) -> Vec<&Node> {
    if let Some(dir) = loose_for(st) {
        return match &st.views.tree.loose {
            Some((have, nodes)) if have == dir => nodes.iter().collect(),
            _ => Vec::new(),
        };
    }
    let Some((root, folder)) = shown_for(st) else {
        return Vec::new();
    };
    if st.views.tree.root.as_deref() != Some(root) {
        return Vec::new();
    }
    let deep = match &st.views.view {
        View::Branch { deep, .. } => *deep,
        View::Roots(_) => st.views.tree.subfolders,
        View::Folder | View::Collection { .. } | View::Queue => false,
    };
    if deep {
        return Vec::new();
    }
    children(&st.views.tree.nodes, folder)
}

/// The grid's tiles as the state has them, changed in place, tile by
/// tile. A tile's name tip goes down when a tile now names another
/// folder, or a tile goes, since a tile destroyed never hears the
/// pointer leave; a count moving or a tile added at the end leaves it.
fn show_tiles(st: &State, app: &App) {
    let nodes = tiles_for(st);
    let offline = shown_for(st).is_some_and(|(r, _)| st.views.library.offline.contains(r));
    *st.views.tree.tile_folders.borrow_mut() = nodes.iter().map(|n| n.path.clone()).collect();
    let tiles: Vec<FolderTile> = nodes
        .into_iter()
        .map(|n| FolderTile {
            name: n.name.clone().into(),
            count: n.total as i32,
            offline,
        })
        .collect();
    // In place, as the tree's rows: a count moving keeps every tile.
    let model = &st.views.tree.tiles;
    let renamed = names_moved(model, &tiles, |t| t.name.clone());
    crate::panel::startup::sync_rows(model, tiles);
    if renamed {
        app.invoke_hide_tip();
    }
}

/// Whether a row of `model` will name something else once `rows` are
/// in it, by `key` (the words a tip shows and what they stand for), or
/// a row will go. Rows added at the end name nothing a tip is up over.
fn names_moved<T: Clone + 'static, K: PartialEq>(
    model: &VecModel<T>,
    rows: &[T],
    key: impl Fn(&T) -> K,
) -> bool {
    rows.len() < model.row_count()
        || model
            .iter()
            .zip(rows)
            .any(|(was, now)| key(&was) != key(now))
}

/// The view a folder of `root`'s tree opens: its own frames, or with
/// `deep` those under it too; the root with those under it is the
/// root's own view, as its row opens it.
pub(crate) fn view_for(root: &Path, folder: &Path, deep: bool) -> View {
    if deep && folder == root {
        View::Roots(Some(root.to_path_buf()))
    } else {
        View::Branch {
            root: root.to_path_buf(),
            folder: folder.to_path_buf(),
            deep,
        }
    }
}

/// The root whose tree the pane shows, and the folder it marks open:
/// the view's own for a root's view or a folder of its tree, and for a
/// folder opened from the disk, the root it lies under with the folder
/// itself marked. None for all the roots, files from several folders,
/// or a folder under no root.
pub(crate) fn shown_for(st: &State) -> Option<(&Path, &Path)> {
    match &st.views.view {
        View::Branch { root, folder, .. } => Some((root, folder)),
        View::Roots(Some(r)) => Some((r, r)),
        View::Roots(None) | View::Collection { .. } | View::Queue => None,
        View::Folder => {
            let dir = st.views.recent.open.as_deref()?;
            let root = st.views.library.roots.root_of(dir)?;
            Some((root, dir))
        }
    }
}

/// The folder open from the disk when it lies under no root: its
/// tiles are its own folders, read from the disk.
fn loose_for(st: &State) -> Option<&Path> {
    if st.views.view != View::Folder {
        return None;
    }
    let dir = st.views.recent.open.as_deref()?;
    st.views.library.roots.root_of(dir).is_none().then_some(dir)
}

/// The folders in `dir`, as tiles have them: each that is not hidden
/// and not a link and has frames directly in it, by name as the tree
/// sorts them, counted by those frames. What is further down is not
/// walked, since the folder may be on a share; a folder with frames
/// only further down has no tile, since Open folder would find nothing
/// in it to open. Off the window's thread.
fn loose_nodes(dir: &Path, apart: bool) -> std::io::Result<Vec<Node>> {
    let mut nodes: Vec<((String, OsString), Node)> = Vec::new();
    for entry in std::fs::read_dir(dir)?.flatten() {
        if entry.file_name().to_string_lossy().starts_with('.')
            || !entry.file_type().is_ok_and(|t| t.is_dir())
        {
            continue;
        }
        let path = entry.path();
        let direct = crate::files::list_files(&path, apart).map_or(0, |f| f.len());
        if direct == 0 {
            continue;
        }
        let os = entry.file_name();
        let name = os.to_string_lossy().into_owned();
        nodes.push((
            (name.to_lowercase(), os),
            Node {
                path,
                name,
                depth: 1,
                direct,
                total: direct,
                children: false,
            },
        ));
    }
    nodes.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(nodes.into_iter().map(|(_, n)| n).collect())
}

/// The folder open from the disk may have folders it did not have, or
/// lost one: opened again, or a move or a delete in it. Its tiles read
/// again, when the view is such a folder.
pub(crate) fn loose_changed(st: &mut State, app: &App) {
    st.views.tree.loose_stale = true;
    want(st, app);
}

/// What a folder's count is has changed for every folder (the JPEGs
/// beside raws counted as frames, or not): the tree and the tiles made
/// again.
pub(crate) fn recounted(st: &mut State, app: &App) {
    st.views.tree.stale = true;
    st.views.tree.loose_stale = true;
    want(st, app);
}

/// The tiles of a folder under no root asked for when they are not the
/// ones held; dropped when the view is not such a folder.
fn want_loose(st: &mut State, app: &App) {
    let Some(dir) = loose_for(st).map(Path::to_path_buf) else {
        if st.views.tree.loose.is_some() || st.views.tree.loose_asked.is_some() {
            st.views.tree.loose = None;
            st.views.tree.loose_asked = None;
            st.views.tree.loose_token += 1;
            show_tiles(st, app);
        }
        return;
    };
    let held = st.views.tree.loose.as_ref().is_some_and(|(d, _)| *d == dir);
    let asked = st.views.tree.loose_asked.as_ref() == Some(&dir);
    if (held || asked) && !st.views.tree.loose_stale {
        return;
    }
    // A read still out is let finish, and asked again when it lands:
    // a burst of moves on a slow share reads the folder twice, not
    // once a move.
    if asked {
        return;
    }
    if !held {
        st.views.tree.loose = None;
        show_tiles(st, app);
    }
    st.views.tree.loose_stale = false;
    st.views.tree.loose_token += 1;
    st.views.tree.loose_asked = Some(dir.clone());
    let build = Build {
        token: st.views.tree.loose_token,
        root: dir,
        count: None,
        index: None,
        counts: None,
        loose: true,
        apart: st.views.jpegs_apart,
    };
    if !send(app, build) {
        st.views.tree.loose_asked = None;
    }
}

/// What the status line says for a folder of the tree opened empty:
/// nothing directly in it, the switch off, or nothing under it at all.
pub(crate) fn nothing_in(folder: &Path, deep: bool) -> String {
    let name = folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| folder.display().to_string());
    if deep {
        tr!("nothing under {name}", name = name)
    } else {
        tr!(
            "nothing directly in {name}: turn on With subfolders to see the frames under it",
            name = name
        )
    }
}

/// The root's count as the index last said, for knowing when the tree
/// is behind it.
fn root_count(st: &State, root: &Path) -> Option<usize> {
    let i = st
        .views
        .library
        .roots
        .list()
        .iter()
        .position(|r| r == root)?;
    st.views.library.counts.get(i).copied()
}

/// The tree the view wants, asked for when it is not the one held: a
/// root's view opened, another root's, or the root's count moved or a
/// pass gone over it since the tree was built. None held for a view
/// that is not one root's, or a folder under no root. The tree already
/// held is kept on screen until the new one lands, unless it is
/// another root's.
pub(crate) fn want(st: &mut State, app: &App) {
    want_loose(st, app);
    reveal(st);
    let Some(root) = shown_for(st).map(|(r, _)| r.to_path_buf()) else {
        if st.views.tree.root.is_some() || st.views.tree.asked.is_some() {
            st.views.tree.root = None;
            st.views.tree.nodes.clear();
            st.views.tree.built_at = None;
            st.views.tree.asked = None;
            st.views.tree.token += 1;
        }
        return;
    };
    let count = root_count(st, &root);
    let held = st.views.tree.root.as_ref() == Some(&root) && st.views.tree.built_at == count;
    if held && !st.views.tree.stale {
        return;
    }
    let asked = st.views.tree.asked.as_ref() == Some(&(root.clone(), count));
    if asked && !st.views.tree.stale {
        return;
    }
    if st.views.tree.root.as_ref() != Some(&root) {
        st.views.tree.root = None;
        st.views.tree.nodes.clear();
        st.views.tree.built_at = None;
    }
    // The counts off the window's thread when the index's path is
    // known; a window with a reader and no path (the tests') reads
    // them here.
    let counts = if st.views.index_path.is_some() {
        None
    } else {
        let Some(lib) = &st.views.index_reader else {
            return;
        };
        match lib.folder_counts_under_canonical(&root) {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::debug!("library: {e}; the tree kept");
                return;
            }
        }
    };
    st.views.tree.stale = false;
    st.views.tree.token += 1;
    st.views.tree.asked = Some((root.clone(), count));
    let build = Build {
        token: st.views.tree.token,
        root,
        count,
        index: st.views.index_path.clone(),
        counts,
        loose: false,
        apart: st.views.jpegs_apart,
    };
    if !send(app, build) {
        st.views.tree.asked = None;
    }
}

/// The folder the tree marks, when it is marked anew, with the folders
/// between it and the root unfolded: a folder opened from the disk, or
/// from Recently opened, is shown where it is rather than inside a
/// folded year.
fn reveal(st: &mut State) {
    let Some((root, folder)) = shown_for(st) else {
        st.views.tree.revealed = None;
        return;
    };
    if st.views.tree.revealed.as_deref() == Some(folder) {
        return;
    }
    let (root, folder) = (root.to_path_buf(), folder.to_path_buf());
    for above in folder.ancestors().skip(1) {
        if !above.starts_with(&root) {
            break;
        }
        st.views.tree.expanded.insert(above.to_path_buf());
    }
    st.views.tree.revealed = Some(folder);
}

/// A pass in the background changed something under `path`: the tree
/// of a root over it or under it is built again at the next `want`.
pub(crate) fn passed(st: &mut State, path: &Path) {
    // The root held, or the one whose first build is out: a pass that
    // lands meanwhile may have moved what that build read.
    let root = st
        .views
        .tree
        .root
        .as_ref()
        .or(st.views.tree.asked.as_ref().map(|(r, _)| r));
    if let Some(r) = root
        && (path.starts_with(r) || r.starts_with(path))
    {
        st.views.tree.stale = true;
    }
}

/// A tree to build off the window's thread: everything it needs, taken
/// from the window's state when it was asked for.
pub(crate) struct Build {
    token: u64,
    root: PathBuf,
    count: Option<usize>,
    /// The index's database, opened read-only for the counts.
    index: Option<PathBuf>,
    /// The counts, already read on the window's thread.
    counts: Option<Vec<(PathBuf, usize)>>,
    /// `root` is a folder under no root, its own folders read from the
    /// disk rather than a tree from the index.
    loose: bool,
    /// A camera's JPEG beside its raw is counted as a frame of its own.
    apart: bool,
}

/// A tree built.
pub(crate) struct Built {
    token: u64,
    root: PathBuf,
    count: Option<usize>,
    /// None when the index could not be read.
    nodes: Option<Vec<Node>>,
    seconds: f64,
    loose: bool,
}

impl Build {
    /// The counts read and the tree made, on a thread that is not the
    /// window's.
    pub(crate) fn run(self) -> Built {
        let started = Instant::now();
        if self.loose {
            let nodes = match loose_nodes(&self.root, self.apart) {
                Ok(n) => Some(n),
                Err(e) => {
                    tracing::warn!("the folders in {}: {e}", self.root.display());
                    None
                }
            };
            return Built {
                token: self.token,
                root: self.root,
                count: None,
                nodes,
                seconds: started.elapsed().as_secs_f64(),
                loose: true,
            };
        }
        let counts = match self.counts {
            Some(c) => Ok(c),
            None => match &self.index {
                Some(path) => {
                    greycard_library::Library::open_read_only(path).and_then(|mut lib| {
                        lib.set_jpegs_apart(self.apart);
                        lib.folder_counts_under_canonical(&self.root)
                    })
                }
                None => Ok(Vec::new()),
            },
        };
        let nodes = match counts {
            Ok(c) => Some(build(&self.root, &c)),
            Err(e) => {
                tracing::warn!("library: the folders of {}: {e}", self.root.display());
                None
            }
        };
        Built {
            token: self.token,
            root: self.root,
            count: self.count,
            nodes,
            seconds: started.elapsed().as_secs_f64(),
            loose: false,
        }
    }
}

/// A build sent off the window's thread, landed on the window's thread
/// when done. False when no thread could be had.
#[cfg(not(test))]
fn send(app: &App, build: Build) -> bool {
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard folder tree".into())
        .spawn(move || {
            let built = build.run();
            let _ = slint::invoke_from_event_loop(move || {
                let (Some(app), Some(state)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                ) else {
                    return;
                };
                land(&mut state.borrow_mut(), &app, built);
            });
        });
    match spawned {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!("library: no thread to build the folder tree on: {e}");
            false
        }
    }
}

/// In a test a build is queued, and the test lands it
/// (`roots::land_sent`): there is no event loop to land it on.
#[cfg(test)]
fn send(_app: &App, build: Build) -> bool {
    tests::SENT.with(|s| s.borrow_mut().push(build));
    true
}

/// A tree built, on the window's thread: kept when it is the one asked
/// for last, and shown.
fn land(st: &mut State, app: &App, built: Built) {
    if built.loose {
        if built.token != st.views.tree.loose_token {
            return;
        }
        st.views.tree.loose_asked = None;
        // A folder that cannot be read (gone, or its share away) shows
        // no tiles rather than the ones it had.
        st.views.tree.loose = Some((built.root, built.nodes.unwrap_or_default()));
        show_tiles(st, app);
        if st.views.tree.loose_stale {
            want_loose(st, app);
        }
        return;
    }
    if built.token != st.views.tree.token {
        tracing::debug!("library: a folder tree asked for before another came in; dropped");
        return;
    }
    st.views.tree.asked = None;
    let Some(nodes) = built.nodes else {
        return;
    };
    tracing::info!(
        "library: {} folder(s) of {} in a tree in {:.1} ms off the window's thread",
        nodes.len(),
        built.root.display(),
        built.seconds * 1e3
    );
    if st.views.tree.seen.insert(built.root.clone()) {
        st.views.tree.expanded.insert(built.root.clone());
    }
    st.views.tree.root = Some(built.root);
    st.views.tree.built_at = built.count;
    st.views.tree.nodes = nodes;
    show(st, app);
}

/// A test's turn at the trees sent off the window's thread: each built
/// and landed; how many.
#[cfg(test)]
pub(crate) fn land_sent(state: &Rc<Shared>, app: &App) -> usize {
    let builds: Vec<Build> = tests::SENT.with(|s| s.borrow_mut().drain(..).collect());
    let n = builds.len();
    for b in builds {
        land(&mut state.borrow_mut(), app, b.run());
    }
    n
}

/// The pane's tree as the state has it: shown while a root's view is
/// open or a folder under a root, its rows once built, the open folder
/// marked. Called whenever the roots' rows are, since a root's name is
/// its first row's.
pub(crate) fn show(st: &State, app: &App) {
    let shown = shown_for(st);
    let root = shown.map(|(r, _)| r);
    app.set_folder_tree_shown(root.is_some());
    // Over a folder from the disk the list is its own frames whatever
    // the switch was left at, so it shows off; turned on, it opens the
    // folder with those under it.
    let switch = st.views.tree.subfolders && st.views.view != View::Folder;
    if app.get_folder_tree_subfolders() != switch {
        app.set_folder_tree_subfolders(switch);
    }
    let open = shown.map(|(_, f)| f);
    let nodes: Vec<&Node> = match (root, &st.views.tree.root) {
        (Some(want), Some(have)) if want == have.as_path() => {
            visible(&st.views.tree.nodes, &st.views.tree.expanded)
        }
        _ => Vec::new(),
    };
    let paths: Vec<PathBuf> = nodes.iter().map(|n| n.path.clone()).collect();
    let was = std::mem::replace(&mut *st.views.tree.shown.borrow_mut(), paths);
    let rows: Vec<FolderRow> = nodes
        .into_iter()
        .map(|n| {
            let expanded = st.views.tree.expanded.contains(&n.path);
            FolderRow {
                name: if n.depth == 0 {
                    st.views.library.roots.label(&n.path)
                } else {
                    n.name.clone()
                }
                .into(),
                path: n.path.to_string_lossy().into_owned().into(),
                depth: n.depth as i32,
                count: count_shown(n, expanded).map_or(-1, |c| c as i32),
                children: n.children,
                expanded: n.children && expanded,
                open: open == Some(n.path.as_path()),
            }
        })
        .collect();
    // Row by row, in place, as the roots' rows are (`roots::show`): made
    // again, the list would be empty until the next walk of the window,
    // the tree's box would shrink and its scroll go back to the top in
    // a frame drawn between, and all under FOLDERS would jump. A row
    // that now names another folder (one added or gone above it) takes
    // its tip down: the pointer is over another name.
    let model = &st.views.tree.rows;
    let renamed = names_moved(model, &rows, |r| (r.name.clone(), r.path.clone()));
    crate::panel::startup::sync_rows(model, rows);
    if renamed {
        app.invoke_hide_tip();
    }
    keep_in_place(st, app, &was);
    bring_into_view(st, app, open);
    show_tiles(st, app);
}

/// Where each row of `was`, the list's folders before, is now: told to
/// the list when a folder came or went above one, so the row in sight
/// at its top stays there rather than slide by a row, and the open
/// mark lands on its row's new place at once rather than fade across.
/// Nothing is worked out when every row is where it was, as after a
/// pass that only moved counts or added folders at the end.
fn keep_in_place(st: &State, app: &App, was: &[PathBuf]) {
    let now = st.views.tree.shown.borrow();
    if now.starts_with(was) {
        return;
    }
    let at: HashMap<&Path, usize> = now
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_path(), i))
        .collect();
    let moved: Vec<i32> = was
        .iter()
        .map(|p| at.get(p.as_path()).map_or(-1, |&i| i as i32))
        .collect();
    if moved
        .iter()
        .enumerate()
        .any(|(i, &j)| j >= 0 && j != i as i32)
    {
        app.set_folder_tree_moved(ModelRc::new(VecModel::from(moved)));
        app.set_folder_tree_moves(app.get_folder_tree_moves().wrapping_add(1));
        app.set_folder_tree_still(true);
    }
}

/// The open folder's row brought into the list's view when the folder
/// is opened anew, however it was opened, except by a click on that
/// row. A folder not built into the tree yet is brought in when its row
/// lands. The pane scrolls only when the row is not wholly in view
/// already (`FoldersSection`).
fn bring_into_view(st: &State, app: &App, open: Option<&Path>) {
    let tree = &st.views.tree;
    let row = tree.rows.iter().position(|r| r.open);
    app.set_folder_tree_open_row(row.map_or(-1, |i| i as i32));
    let Some(open) = open else {
        *tree.in_view.borrow_mut() = None;
        return;
    };
    if row.is_none() || tree.in_view.borrow().as_deref() == Some(open) {
        return;
    }
    *tree.in_view.borrow_mut() = Some(open.to_path_buf());
    if tree.clicked.take().as_deref() != Some(open) {
        app.set_folder_tree_reveal(app.get_folder_tree_reveal().wrapping_add(1));
    }
}

/// The folder of the row shown at `row`.
fn row_folder(st: &State, row: i32) -> Option<PathBuf> {
    let row = usize::try_from(row).ok()?;
    st.views.tree.shown.borrow().get(row).cloned()
}

/// The folder of the grid's tile at `tile`.
fn tile_folder(st: &State, tile: i32) -> Option<PathBuf> {
    let tile = usize::try_from(tile).ok()?;
    st.views.tree.tile_folders.borrow().get(tile).cloned()
}

/// A folder of the tree chosen, by its row: opened as [`open`] has it.
fn picked(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, row: i32) {
    let Some(folder) = row_folder(&state.borrow(), row) else {
        return;
    };
    *state.borrow().views.tree.clicked.borrow_mut() = Some(folder.clone());
    open(state, app, worker, &folder, "the tree");
}

/// A folder's tile in the grid clicked: opened as its row in the tree
/// opens it.
fn tile_picked(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, tile: i32) {
    let (folder, loose) = {
        let st = state.borrow();
        let Some(folder) = tile_folder(&st, tile) else {
            return;
        };
        (folder, loose_for(&st).is_some())
    };
    if loose {
        // A folder under no root opens as Open folder opens it.
        tracing::info!("{} opened from its tile", folder.display());
        crate::panel::browser::open_folder(state, app, worker, &folder);
        return;
    }
    open(state, app, worker, &folder, "its tile");
}

/// A folder of the tree opened, from its row or its tile: its frames
/// from the index, its own or with those under it as the switch says.
/// The tree marks it and Recently opened records it once the view
/// lands, as for any view.
fn open(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, folder: &Path, from: &str) {
    let view = {
        let st = state.borrow();
        let Some(root) = st.views.tree.root.clone() else {
            return;
        };
        if !folder.starts_with(&root) {
            return;
        }
        tracing::info!("library: {} opened from {from}", folder.display());
        view_for(&root, folder, st.views.tree.subfolders)
    };
    crate::roots::open_view(state, app, worker, view);
}

/// The folder a tile's Reveal shows: the tile's, while its root is
/// online; none while it is offline, or for no such tile.
fn reveal_target(st: &State, tile: i32) -> Option<PathBuf> {
    if shown_for(st).is_some_and(|(r, _)| st.views.library.offline.contains(r)) {
        return None;
    }
    tile_folder(st, tile)
}

/// A tile's Reveal: its folder in the file manager, while its root is
/// online.
fn tile_reveal(st: &State, app: &App, tile: i32) {
    let Some(folder) = reveal_target(st, tile) else {
        return;
    };
    if let Err(e) = crate::panel::menu::reveal_folder(&folder) {
        tracing::warn!(
            "reveal {}: could not start the file manager: {e}",
            folder.display()
        );
        app.set_status(tr!("could not show {}: {e}", folder.display(), e = e).into());
    }
}

/// A folder's triangle pressed: its folders shown, or put away.
fn folded(st: &mut State, app: &App, row: i32) {
    let Some(folder) = row_folder(st, row) else {
        return;
    };
    if !st.views.tree.expanded.remove(&folder) {
        st.views.tree.expanded.insert(folder);
    }
    show(st, app);
}

/// The switch flipped: kept for the next run, and the folder open in
/// the tree opened again with it. When the frame on screen is in the
/// new list the view changes in place and the list is merged, as a
/// sort's is: the frame stays on screen, its picture and develop with
/// it, and nothing else is opened again. When it is not (the
/// switch turned off under a frame of a folder below), or there is no
/// frame on screen, the view is opened as any is.
fn subfolders_changed(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>) {
    let on = app.get_folder_tree_subfolders();
    let again = {
        let mut st = state.borrow_mut();
        st.views.tree.subfolders = on;
        crate::panel::prefs::keep(&st, |s| s.folder_tree_subfolders = on);
        // None to open when it is the view open already: the root's
        // own view, the switch turned on over it; or a folder opened
        // from the disk, the switch turned off, which lists its own
        // frames already.
        match shown_for(&st) {
            Some(_) if st.views.view == View::Folder && !on => None,
            Some((root, folder)) => Some(view_for(root, folder, on))
                .filter(|v| *v != st.views.view)
                .map(|v| (v, keeps_frame(&st, folder, on))),
            None => None,
        }
    };
    let Some((view, kept)) = again else {
        return;
    };
    if kept && change_in_place(state, app, worker, view.clone()) {
        return;
    }
    crate::roots::open_view(state, app, worker, view);
}

/// Whether the frame on screen is in the list of `folder`, with `deep`
/// those under it too, as the index spells its paths: the view's folder
/// is the index's spelling, and so is every path of a list read from
/// it. A folder opened from the disk under another spelling (a link to
/// it) is not taken for the same, and is opened again instead.
fn keeps_frame(st: &State, folder: &Path, deep: bool) -> bool {
    let Some(path) = st.current.and_then(|c| st.path_at(c)) else {
        return false;
    };
    if deep {
        path.starts_with(folder)
    } else {
        path.parent() == Some(folder)
    }
}

/// `view` put in place of the one open, over the same folder, and the
/// list read again and merged into the window's, the frame on screen
/// kept (`roots::refresh_view`). What is drawn from the view and not
/// from the list (the tree's mark, the tiles, the switch, the folder
/// named, the Sort menu) follows now, as an open's landing has it.
/// The merge keeps a frame on screen that is not in the list it
/// brings, which is why the caller asks only when it is. A read of
/// the list already out is of the view before, and is let go for this
/// one. Should the read fail (the index unreadable mid-read), the view
/// is the new one over the old list until the next read of it. False,
/// the view left as it was, when there is nothing to read the list
/// from or a view is still opening.
fn change_in_place(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, view: View) -> bool {
    let was = std::mem::replace(&mut state.borrow_mut().views.view, view);
    if !crate::roots::read_view_now(state, app, worker) {
        state.borrow_mut().views.view = was;
        return false;
    }
    let mut st = state.borrow_mut();
    // A folder from the disk had its folder passed over by the
    // indexer; the list is the index's rows now, as an open of the
    // view would have it. What the pass said of itself (its progress,
    // or that the index was busy, a retry the dropped pass will not
    // make) goes from the filter's line with it, as an open's list
    // clears it: the merge may change no row, and redraw nothing.
    if was == View::Folder {
        crate::library::drop_folder_pass(&mut st);
        st.views.index_progress = None;
        st.views.index_tries = 0;
        st.views.index_error = None;
        crate::panel::cull::show_filter(&st, app);
    }
    // The frame on screen brought into view when the merge lands: the
    // rows round it may have come or gone.
    st.views.sort_reveal = true;
    let left = crate::panel::visited::frame_of(&st);
    crate::panel::recent::opened(&mut st, app, left);
    want(&mut st, app);
    crate::roots::show(&st, app);
    crate::panel::sort::show(&st, app);
    true
}

impl Tree {
    /// A tree with nothing built yet, and the switch as the settings
    /// file left it.
    pub(crate) fn with_subfolders(subfolders: bool) -> Tree {
        Tree {
            subfolders,
            ..Tree::default()
        }
    }
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    app.set_folder_tree(ModelRc::from(state.borrow().views.tree.rows.clone()));
    app.set_grid_tiles(ModelRc::from(state.borrow().views.tree.tiles.clone()));
    app.set_folder_tree_subfolders(state.borrow().views.tree.subfolders);
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_folder_tree_picked(move |row| {
            if let Some(app) = app_weak.upgrade() {
                picked(&state, &app, &worker, row);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_grid_tile_picked(move |tile| {
            if let Some(app) = app_weak.upgrade() {
                tile_picked(&state, &app, &worker, tile);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_grid_tile_reveal(move |tile| {
            if let Some(app) = app_weak.upgrade() {
                tile_reveal(&state.borrow(), &app, tile);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_folder_tree_folded(move |row| {
            if let Some(app) = app_weak.upgrade() {
                folded(&mut state.borrow_mut(), &app, row);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_folder_tree_subfolders_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                subfolders_changed(&state, &app, &worker);
            }
        });
    }
}

#[cfg(test)]
mod tests;
