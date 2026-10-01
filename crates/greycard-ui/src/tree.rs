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

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::path::Component;
use std::time::Instant;

use crate::roots::View;
use crate::*;

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
    /// The rows as the pane shows them: one model for the session, its
    /// rows replaced only when they change.
    rows: Rc<VecModel<FolderRow>>,
    /// Each row's folder, in the model's order: a click comes back as
    /// the row's number and is looked up here, since a path through
    /// the window's strings would lose a name that is not UTF-8.
    shown: RefCell<Vec<PathBuf>>,
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
    match &st.view {
        View::Branch { root, folder, .. } => Some((root, folder)),
        View::Roots(Some(r)) => Some((r, r)),
        View::Roots(None) => None,
        View::Folder => {
            let dir = st.recent.open.as_deref()?;
            let root = st.library.roots.root_of(dir)?;
            Some((root, dir))
        }
    }
}

/// What the status line says for a folder of the tree opened empty:
/// nothing under it at all. (A folder with nothing directly in it
/// opens with those under it, so that case never lands empty.)
pub(crate) fn nothing_in(folder: &Path) -> String {
    let name = folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| folder.display().to_string());
    format!("nothing under {name}")
}

/// The root's count as the index last said, for knowing when the tree
/// is behind it.
fn root_count(st: &State, root: &Path) -> Option<usize> {
    let i = st.library.roots.list().iter().position(|r| r == root)?;
    st.library.counts.get(i).copied()
}

/// The tree the view wants, asked for when it is not the one held: a
/// root's view opened, another root's, or the root's count moved or a
/// pass gone over it since the tree was built. None held for a view
/// that is not one root's, or a folder under no root. The tree already
/// held is kept on screen until the new one lands, unless it is
/// another root's.
pub(crate) fn want(st: &mut State, app: &App) {
    reveal(st);
    let Some(root) = shown_for(st).map(|(r, _)| r.to_path_buf()) else {
        if st.tree.root.is_some() || st.tree.asked.is_some() {
            st.tree.root = None;
            st.tree.nodes.clear();
            st.tree.built_at = None;
            st.tree.asked = None;
            st.tree.token += 1;
        }
        return;
    };
    let count = root_count(st, &root);
    let held = st.tree.root.as_ref() == Some(&root) && st.tree.built_at == count;
    if held && !st.tree.stale {
        return;
    }
    let asked = st.tree.asked.as_ref() == Some(&(root.clone(), count));
    if asked && !st.tree.stale {
        return;
    }
    if st.tree.root.as_ref() != Some(&root) {
        st.tree.root = None;
        st.tree.nodes.clear();
        st.tree.built_at = None;
    }
    // The counts off the window's thread when the index's path is
    // known; a window with a reader and no path (the tests') reads
    // them here.
    let counts = if st.index_path.is_some() {
        None
    } else {
        let Some(lib) = &st.index_reader else {
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
    st.tree.stale = false;
    st.tree.token += 1;
    st.tree.asked = Some((root.clone(), count));
    let build = Build {
        token: st.tree.token,
        root,
        count,
        index: st.index_path.clone(),
        counts,
    };
    if !send(app, build) {
        st.tree.asked = None;
    }
}

/// The folder the tree marks, when it is marked anew, with the folders
/// between it and the root unfolded: a folder opened from the disk, or
/// from Recently opened, is shown where it is rather than inside a
/// folded year.
fn reveal(st: &mut State) {
    let Some((root, folder)) = shown_for(st) else {
        st.tree.revealed = None;
        return;
    };
    if st.tree.revealed.as_deref() == Some(folder) {
        return;
    }
    let (root, folder) = (root.to_path_buf(), folder.to_path_buf());
    for above in folder.ancestors().skip(1) {
        if !above.starts_with(&root) {
            break;
        }
        st.tree.expanded.insert(above.to_path_buf());
    }
    st.tree.revealed = Some(folder);
}

/// A pass in the background changed something under `path`: the tree
/// of a root over it or under it is built again at the next `want`.
pub(crate) fn passed(st: &mut State, path: &Path) {
    // The root held, or the one whose first build is out: a pass that
    // lands meanwhile may have moved what that build read.
    let root = st
        .tree
        .root
        .as_ref()
        .or(st.tree.asked.as_ref().map(|(r, _)| r));
    if let Some(r) = root
        && (path.starts_with(r) || r.starts_with(path))
    {
        st.tree.stale = true;
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
}

/// A tree built.
pub(crate) struct Built {
    token: u64,
    root: PathBuf,
    count: Option<usize>,
    /// None when the index could not be read.
    nodes: Option<Vec<Node>>,
    seconds: f64,
}

impl Build {
    /// The counts read and the tree made, on a thread that is not the
    /// window's.
    pub(crate) fn run(self) -> Built {
        let started = Instant::now();
        let counts = match self.counts {
            Some(c) => Ok(c),
            None => match &self.index {
                Some(path) => greycard_library::Library::open_read_only(path)
                    .and_then(|lib| lib.folder_counts_under_canonical(&self.root)),
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
    if built.token != st.tree.token {
        tracing::debug!("library: a folder tree asked for before another came in; dropped");
        return;
    }
    st.tree.asked = None;
    let Some(nodes) = built.nodes else {
        return;
    };
    tracing::info!(
        "library: {} folder(s) of {} in a tree in {:.1} ms off the window's thread",
        nodes.len(),
        built.root.display(),
        built.seconds * 1e3
    );
    if st.tree.seen.insert(built.root.clone()) {
        st.tree.expanded.insert(built.root.clone());
    }
    st.tree.root = Some(built.root);
    st.tree.built_at = built.count;
    st.tree.nodes = nodes;
    show(st, app);
}

/// A test's turn at the trees sent off the window's thread: each built
/// and landed; how many.
#[cfg(test)]
pub(crate) fn land_sent(state: &Rc<RefCell<State>>, app: &App) -> usize {
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
    let switch = st.tree.subfolders && st.view != View::Folder;
    if app.get_folder_tree_subfolders() != switch {
        app.set_folder_tree_subfolders(switch);
    }
    let open = shown.map(|(_, f)| f);
    let nodes: Vec<&Node> = match (root, &st.tree.root) {
        (Some(want), Some(have)) if want == have.as_path() => {
            visible(&st.tree.nodes, &st.tree.expanded)
        }
        _ => Vec::new(),
    };
    *st.tree.shown.borrow_mut() = nodes.iter().map(|n| n.path.clone()).collect();
    let rows: Vec<FolderRow> = nodes
        .into_iter()
        .map(|n| {
            let expanded = st.tree.expanded.contains(&n.path);
            FolderRow {
                name: if n.depth == 0 {
                    st.library.roots.label(&n.path)
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
    let model = &st.tree.rows;
    if !model.iter().eq(rows.iter().cloned()) {
        model.set_vec(rows);
    }
}

/// The folder of the row shown at `row`.
fn row_folder(st: &State, row: i32) -> Option<PathBuf> {
    let row = usize::try_from(row).ok()?;
    st.tree.shown.borrow().get(row).cloned()
}

/// A folder of the tree chosen, by its row: its frames opened from the
/// index, its own or with those under it as the switch says.
fn picked(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, row: i32) {
    let view = {
        let st = state.borrow();
        let Some(folder) = row_folder(&st, row) else {
            return;
        };
        let folder = folder.as_path();
        let Some(root) = st.tree.root.clone() else {
            return;
        };
        if !folder.starts_with(&root) {
            return;
        }
        tracing::info!("library: {} opened from the tree", folder.display());
        view_for(&root, folder, st.tree.subfolders)
    };
    crate::roots::open_view(state, app, worker, view);
}

/// A folder's triangle pressed: its folders shown, or put away.
fn folded(st: &mut State, app: &App, row: i32) {
    let Some(folder) = row_folder(st, row) else {
        return;
    };
    if !st.tree.expanded.remove(&folder) {
        st.tree.expanded.insert(folder);
    }
    show(st, app);
}

/// The switch flipped: kept for the next run, and the folder open in
/// the tree opened again with it.
fn subfolders_changed(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) {
    let on = app.get_folder_tree_subfolders();
    let again = {
        let mut st = state.borrow_mut();
        st.tree.subfolders = on;
        crate::panel::prefs::keep(&st, |s| s.folder_tree_subfolders = on);
        // None to open when it is the view open already: the root's
        // own view, the switch turned on over it; or a folder opened
        // from the disk, the switch turned off, which lists its own
        // frames already.
        match shown_for(&st) {
            Some(_) if st.view == View::Folder && !on => None,
            Some((root, folder)) => Some(view_for(root, folder, on)).filter(|v| *v != st.view),
            None => None,
        }
    };
    if let Some(view) = again {
        crate::roots::open_view(state, app, worker, view);
    }
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

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    app.set_folder_tree(ModelRc::from(state.borrow().tree.rows.clone()));
    app.set_folder_tree_subfolders(state.borrow().tree.subfolders);
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_folder_tree_picked(move |row| {
            if let Some(app) = app_weak.upgrade() {
                picked(&state, &app, &worker, row);
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
mod tests {
    use super::*;

    thread_local! {
        /// The trees asked for, waiting for the test to build and land
        /// them.
        pub(super) static SENT: RefCell<Vec<Build>> = const { RefCell::new(Vec::new()) };
    }

    use crate::roots::{land_sent, open_view};
    use crate::testing::{click, labeled, state_for, window};
    use greycard_library::fixture::{A7, R5, R6, write_frame};

    fn node(
        path: &str,
        name: &str,
        depth: usize,
        direct: usize,
        total: usize,
        children: bool,
    ) -> Node {
        Node {
            path: PathBuf::from(path),
            name: name.into(),
            depth,
            direct,
            total,
            children,
        }
    }

    /// Every folder under the root, the ones above a folder with frames
    /// among them, each after the one it is in and its own by name, the
    /// case folded; the counts directly in each and under each; a folder
    /// beside the root whose name starts like it left out.
    #[test]
    fn a_tree_is_built_from_the_folders_counts() {
        let counts: Vec<(PathBuf, usize)> = [
            ("/r", 2),
            ("/r/2024/01-02", 3),
            ("/r/2024/01-10", 1),
            ("/r/B", 4),
            ("/r/a", 1),
            ("/r/a/x/y", 2),
            ("/r2/x", 9),
            ("/elsewhere", 9),
        ]
        .into_iter()
        .map(|(p, n)| (PathBuf::from(p), n))
        .collect();
        assert_eq!(
            build(Path::new("/r"), &counts),
            [
                node("/r", "", 0, 2, 13, true),
                node("/r/2024", "2024", 1, 0, 4, true),
                node("/r/2024/01-02", "01-02", 2, 3, 3, false),
                node("/r/2024/01-10", "01-10", 2, 1, 1, false),
                node("/r/a", "a", 1, 1, 3, true),
                node("/r/a/x", "x", 2, 0, 2, true),
                node("/r/a/x/y", "y", 3, 2, 2, false),
                node("/r/B", "B", 1, 4, 4, false),
            ]
        );
    }

    #[test]
    fn a_root_with_one_folder_or_none_is_a_tree_too() {
        let one = [(PathBuf::from("/r/day"), 5)];
        assert_eq!(
            build(Path::new("/r"), &one),
            [
                node("/r", "", 0, 0, 5, true),
                node("/r/day", "day", 1, 5, 5, false)
            ]
        );
        // Nothing indexed under it yet: the root alone, with nothing.
        assert_eq!(
            build(Path::new("/r"), &[]),
            [node("/r", "", 0, 0, 0, false)]
        );
        // Its frames all directly in it: the root alone, with them.
        let flat = [(PathBuf::from("/r"), 7)];
        assert_eq!(
            build(Path::new("/r"), &flat),
            [node("/r", "", 0, 7, 7, false)]
        );
    }

    /// A folded folder hides everything under it and shows the count
    /// under it; unfolded, the count directly in it, or none for a
    /// folder with nothing of its own.
    #[test]
    fn a_folded_folder_hides_those_under_it_and_counts_them() {
        let counts = [
            (PathBuf::from("/r/2024/01-02"), 3),
            (PathBuf::from("/r/2024/01-10"), 1),
            (PathBuf::from("/r/b"), 4),
        ];
        let nodes = build(Path::new("/r"), &counts);
        let names = |e: &HashSet<PathBuf>| -> Vec<String> {
            visible(&nodes, e)
                .into_iter()
                .map(|n| format!("{}{}", "-".repeat(n.depth), n.name))
                .collect()
        };
        let mut expanded = HashSet::new();
        assert_eq!(names(&expanded), [""], "the root folded");
        expanded.insert(PathBuf::from("/r"));
        assert_eq!(names(&expanded), ["", "-2024", "-b"]);
        expanded.insert(PathBuf::from("/r/2024"));
        assert_eq!(names(&expanded), ["", "-2024", "--01-02", "--01-10", "-b"]);
        // Unfolded under a folded root: still hidden.
        expanded.remove(Path::new("/r"));
        assert_eq!(names(&expanded), [""]);
        assert_eq!(count_shown(&nodes[1], false), Some(4));
        assert_eq!(count_shown(&nodes[1], true), None);
        assert_eq!(count_shown(&nodes[2], true), Some(3), "a leaf");
    }

    #[test]
    fn the_root_with_its_folders_is_the_roots_own_view() {
        let (r, day) = (Path::new("/r"), Path::new("/r/day"));
        assert_eq!(view_for(r, r, true), View::Roots(Some(r.to_path_buf())));
        assert_eq!(
            view_for(r, r, false),
            View::Branch {
                root: r.to_path_buf(),
                folder: r.to_path_buf(),
                deep: false
            }
        );
        assert_eq!(
            view_for(r, day, true),
            View::Branch {
                root: r.to_path_buf(),
                folder: day.to_path_buf(),
                deep: true
            }
        );
    }

    /// An empty folder opened says the switch only when it is off.
    #[test]
    fn an_empty_folder_says_the_switch_only_when_it_is_off() {
        let day = Path::new("/r/2025");
        assert_eq!(nothing_in(day), "nothing under 2025");
    }

    /// A folder of this test's own, canonical, as the index keys
    /// folders.
    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-ui-tree-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dunce::canonicalize(&dir).unwrap()
    }

    fn frames(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
        std::fs::create_dir_all(dir).unwrap();
        names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let p = dir.join(n);
                write_frame(&p, [&R5, &R6, &A7][i % 3], i as u16);
                p
            })
            .collect()
    }

    /// A root `archive` of a frame of its own, a day of two frames, and
    /// a folder under the day of one, indexed; a window on it with the
    /// root in its library and the root's view open, its tree landed.
    fn archive(dir: &Path, window_px: Option<(f32, f32)>) -> (App, Rc<RefCell<State>>, Rc<Worker>) {
        let root = dir.join("archive");
        frames(&root, &["r.tif"]);
        frames(&root.join("day"), &["d1.tif", "d2.tif"]);
        frames(&root.join("day").join("more"), &["m.tif"]);
        let db = dir.join("index").join("library.sqlite");
        greycard_library::Library::open(&db)
            .unwrap()
            .index_tree(&root, &mut |_| {})
            .unwrap();
        let app = window(0);
        if let Some((w, h)) = window_px {
            app.window().set_size(slint::LogicalSize::new(w, h));
        }
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            // Sidecars as the editor keeps them, so a frame listed
            // from its row stands in until its sidecar is wanted.
            st.write_sidecars = true;
            st.library.roots.add(&root).unwrap();
            crate::roots::recount(&mut st);
        }
        open_view(&state, &app, &worker, View::Roots(Some(root.clone())));
        land_sent(&state, &app, &worker);
        (app, state, worker)
    }

    fn listed(state: &Rc<RefCell<State>>) -> Vec<String> {
        state
            .borrow()
            .files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    /// The row shown under `name` chosen, as a click on it sends it.
    fn pick(app: &App, name: &str) {
        let row = app
            .get_folder_tree()
            .iter()
            .position(|r| r.name == name)
            .unwrap_or_else(|| panic!("no row {name} in {:?}", rows(app)));
        app.invoke_folder_tree_picked(row as i32);
    }

    fn rows(app: &App) -> Vec<(String, i32, i32, bool)> {
        app.get_folder_tree()
            .iter()
            .map(|r| (r.name.to_string(), r.depth, r.count, r.open))
            .collect()
    }

    /// The root's view shows its tree from the index, the root marked
    /// open; a folder chosen opens its own frames alone, and with the
    /// switch those under it too, the root's row lit throughout; the
    /// root chosen with the switch is the root's own view.
    #[test]
    fn a_folder_opens_its_own_frames_or_with_the_switch_those_under_it() {
        let dir = scratch("switch");
        let (app, state, worker) = archive(&dir, None);
        let root = dir.join("archive");
        let day = root.join("day");
        assert!(app.get_folder_tree_shown());
        assert_eq!(
            rows(&app),
            [("archive".into(), 0, 1, true), ("day".into(), 1, 3, false)],
            "the root unfolded, the day folded with the three under it"
        );
        assert!(!state.borrow().tree.subfolders, "off by default");

        pick(&app, "day");
        land_sent(&state, &app, &worker);
        assert_eq!(listed(&state), ["d1.tif", "d2.tif"]);
        assert_eq!(
            state.borrow().view,
            View::Branch {
                root: root.clone(),
                folder: day.clone(),
                deep: false
            }
        );
        let chip = app.get_library_roots().row_data(0).unwrap();
        assert!(chip.on, "the root's row stays lit");
        assert_eq!(rows(&app)[1], ("day".into(), 1, 3, true));
        // No sidecar read: every frame stands in from its row.
        {
            let st = state.borrow();
            assert!((0..st.files.len()).all(|i| !crate::rows::is_loaded(&st, i)));
        }

        app.set_folder_tree_subfolders(true);
        app.invoke_folder_tree_subfolders_changed();
        land_sent(&state, &app, &worker);
        assert!(state.borrow().tree.subfolders);
        assert_eq!(listed(&state), ["d1.tif", "d2.tif", "m.tif"]);
        assert!(matches!(
            state.borrow().view,
            View::Branch { deep: true, .. }
        ));

        // The root, with the switch on: the root's view, as its row.
        pick(&app, "archive");
        land_sent(&state, &app, &worker);
        assert_eq!(state.borrow().view, View::Roots(Some(root.clone())));
        assert_eq!(listed(&state), ["r.tif", "d1.tif", "d2.tif", "m.tif"]);
        // And off again: the root's own frame alone.
        app.set_folder_tree_subfolders(false);
        app.invoke_folder_tree_subfolders_changed();
        land_sent(&state, &app, &worker);
        assert_eq!(listed(&state), ["r.tif"]);
        assert_eq!(rows(&app)[0], ("archive".into(), 0, 1, true));

        // A folder with nothing of its own opens with those under it,
        // the switch off or not: nothing else can be meant.
        std::fs::remove_file(root.join("r.tif")).unwrap();
        state.borrow_mut().index_reader = None;
        greycard_library::Library::open(&dir.join("index").join("library.sqlite"))
            .unwrap()
            .index_folder(&root, &mut |_| {})
            .unwrap();
        state.borrow_mut().index_reader = Some(
            greycard_library::Library::open_read_only(&dir.join("index").join("library.sqlite"))
                .unwrap(),
        );
        pick(&app, "archive");
        land_sent(&state, &app, &worker);
        assert_eq!(listed(&state), ["d1.tif", "d2.tif", "m.tif"]);
        assert!(
            !state.borrow().tree.subfolders,
            "the switch itself is left off"
        );
        // The switch on over it is the root's view; off again, with
        // nothing of its own, it keeps the frames under it.
        app.set_folder_tree_subfolders(true);
        app.invoke_folder_tree_subfolders_changed();
        land_sent(&state, &app, &worker);
        assert_eq!(state.borrow().view, View::Roots(Some(root.clone())));
        app.set_folder_tree_subfolders(false);
        app.invoke_folder_tree_subfolders_changed();
        land_sent(&state, &app, &worker);
        assert_eq!(listed(&state), ["d1.tif", "d2.tif", "m.tif"]);
        state.borrow_mut().index_reader = None;
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A folder opened from the disk that lies under a root has the
    /// root's tree with that folder marked, in the loupe's pane and the
    /// grid's alike; a click there opens a folder of the tree as the
    /// root's view does, and the switch turned on over the disk's
    /// folder opens it with those under it. A folder under no root has
    /// no tree.
    #[test]
    fn a_disk_folder_under_a_root_shows_the_roots_tree_marked_there() {
        let dir = scratch("disk");
        let (app, state, worker) = archive(&dir, Some((1200.0, 800.0)));
        let root = dir.join("archive");
        let day = root.join("day");
        crate::panel::browser::open_folder(&state, &app, &worker, &day);
        land_sent(&state, &app, &worker);
        assert_eq!(state.borrow().view, View::Folder);
        assert_eq!(listed(&state), ["d1.tif", "d2.tif"]);
        assert!(app.get_folder_tree_shown());
        assert_eq!(
            rows(&app),
            [("archive".into(), 0, 1, false), ("day".into(), 1, 3, true)],
            "the root's tree, the day marked"
        );
        assert!(
            !app.get_library_roots().row_data(0).unwrap().on,
            "a folder from the disk is not the root's view"
        );

        // In the grid's pane too, where a click on the root opens it
        // from the index, the root's row lit.
        app.set_grid_open(true);
        slint::platform::update_timers_and_animations();
        assert_eq!(crate::testing::buttons(&app, "day").len(), 1);
        let (at, size) = labeled(&app, "Unfold day");
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert_eq!(rows(&app)[2], ("more".into(), 2, 1, false));
        // Chosen by its row: a click on a row the unfold has just
        // added lands past the box's old foot on the testing backend,
        // which has not laid the box out again.
        pick(&app, "more");
        land_sent(&state, &app, &worker);
        assert_eq!(listed(&state), ["m.tif"]);
        assert!(matches!(
            state.borrow().view,
            View::Branch { deep: false, .. }
        ));
        assert!(app.get_library_roots().row_data(0).unwrap().on);

        // The day from the disk again, and the switch turned on over
        // it: the day with the folders under it, from the index.
        crate::panel::browser::open_folder(&state, &app, &worker, &day);
        land_sent(&state, &app, &worker);
        assert_eq!(state.borrow().view, View::Folder);
        app.set_folder_tree_subfolders(true);
        app.invoke_folder_tree_subfolders_changed();
        land_sent(&state, &app, &worker);
        assert_eq!(
            state.borrow().view,
            View::Branch {
                root: root.clone(),
                folder: day.clone(),
                deep: true
            }
        );
        assert_eq!(listed(&state), ["d1.tif", "d2.tif", "m.tif"]);
        assert!(app.get_folder_tree_subfolders());
        // The day from the disk once more, the switch left on: the list
        // is the day's own frames, so the switch shows off, and turned
        // on it opens the day with those under it again.
        crate::panel::browser::open_folder(&state, &app, &worker, &day);
        land_sent(&state, &app, &worker);
        assert_eq!(state.borrow().view, View::Folder);
        assert_eq!(listed(&state), ["d1.tif", "d2.tif"]);
        assert!(state.borrow().tree.subfolders, "the setting kept");
        assert!(!app.get_folder_tree_subfolders(), "shown off");
        app.set_folder_tree_subfolders(true);
        app.invoke_folder_tree_subfolders_changed();
        land_sent(&state, &app, &worker);
        assert!(matches!(
            state.borrow().view,
            View::Branch { deep: true, .. }
        ));
        assert!(app.get_folder_tree_subfolders());
        // The switch off again, and the day from the disk.
        app.set_folder_tree_subfolders(false);
        app.invoke_folder_tree_subfolders_changed();
        land_sent(&state, &app, &worker);
        crate::panel::browser::open_folder(&state, &app, &worker, &day);
        land_sent(&state, &app, &worker);
        assert_eq!(state.borrow().view, View::Folder);

        // A folder deeper down, opened from the disk with the day
        // folded over it: the day unfolded to show it, marked.
        let row = rows(&app).iter().position(|r| r.0 == "day").unwrap();
        app.invoke_folder_tree_folded(row as i32);
        assert_eq!(rows(&app).len(), 2, "the day folded");
        crate::panel::browser::open_folder(&state, &app, &worker, &day.join("more"));
        land_sent(&state, &app, &worker);
        assert_eq!(state.borrow().view, View::Folder);
        assert_eq!(
            rows(&app),
            [
                ("archive".into(), 0, 1, false),
                ("day".into(), 1, 2, false),
                ("more".into(), 2, 1, true)
            ]
        );
        // Folded again by hand, it stays folded while the folder is
        // open.
        app.invoke_folder_tree_folded(1);
        crate::roots::show(&state.borrow(), &app);
        crate::tree::want(&mut state.borrow_mut(), &app);
        assert_eq!(rows(&app).len(), 2);

        // A folder under no root: no tree.
        let elsewhere = dir.join("elsewhere");
        frames(&elsewhere, &["e.tif"]);
        crate::panel::browser::open_folder(&state, &app, &worker, &elsewhere);
        land_sent(&state, &app, &worker);
        assert_eq!(listed(&state), ["e.tif"]);
        assert!(!app.get_folder_tree_shown());
        assert!(crate::testing::buttons(&app, "day").is_empty());
        state.borrow_mut().index_reader = None;
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A click on a folder in the pane, found by its name, opens its
    /// frames and records it in Recently opened as the folder it is,
    /// named with the root it is under.
    #[test]
    fn a_click_on_a_folder_opens_its_frames_and_records_it() {
        let dir = scratch("click");
        let (app, state, worker) = archive(&dir, Some((1200.0, 800.0)));
        let day = dir.join("archive").join("day");
        slint::platform::update_timers_and_animations();
        let (at, size) = labeled(&app, "day");
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        land_sent(&state, &app, &worker);
        assert_eq!(listed(&state), ["d1.tif", "d2.tif"]);
        assert_eq!(state.borrow().recent.open.as_deref(), Some(day.as_path()));
        assert_eq!(
            state.borrow().recent.folders.first().map(String::as_str),
            day.to_str()
        );
        assert_eq!(app.get_open_folder_name(), "day");
        assert_eq!(app.get_open_folder_root(), "archive");
        assert!(rows(&app)[1].3, "the day marked open");

        // Its triangle, by its own label, unfolds it without opening it.
        let (at, size) = labeled(&app, "Unfold day");
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        land_sent(&state, &app, &worker);
        assert_eq!(rows(&app).len(), 3);
        assert_eq!(rows(&app)[1], ("day".into(), 1, 2, true));
        assert_eq!(rows(&app)[2], ("more".into(), 2, 1, false));
        assert_eq!(listed(&state), ["d1.tif", "d2.tif"], "still the day's");
        state.borrow_mut().index_reader = None;
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A pass that adds a folder under the root has the tree built
    /// again with it; one that only refreshed a sidecar does not.
    #[test]
    fn the_tree_follows_a_pass_that_adds_a_folder() {
        let dir = scratch("pass");
        let (app, state, worker) = archive(&dir, None);
        let root = dir.join("archive");
        let token = state.borrow().tree.token;
        let quiet = greycard_library::Report {
            meta_refreshed: 1,
            ..Default::default()
        };
        crate::roots::background_done(&state, &app, &root, &quiet, false, false);
        land_sent(&state, &app, &worker);
        assert_eq!(state.borrow().tree.token, token, "no build for a rating");

        frames(&root.join("later"), &["l1.tif", "l2.tif"]);
        let report = greycard_library::Library::open(&dir.join("index").join("library.sqlite"))
            .unwrap()
            .index_tree(&root, &mut |_| {})
            .unwrap();
        assert_eq!(report.added, 2);
        crate::roots::background_done(&state, &app, &root, &report, false, false);
        land_sent(&state, &app, &worker);
        assert_eq!(
            rows(&app),
            [
                ("archive".into(), 0, 1, true),
                ("day".into(), 1, 3, false),
                ("later".into(), 1, 2, false)
            ]
        );
        state.borrow_mut().index_reader = None;
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A frame moved from the day to a new folder under the root, the
    /// root's count unchanged: the pass's move alone has the tree built
    /// again, whether the tree was built already or its first build was
    /// still out when the pass landed.
    #[test]
    fn the_tree_follows_a_move_that_leaves_the_count_as_it_was() {
        let moved_tree = [
            ("archive".to_string(), 0, 1, true),
            ("day".to_string(), 1, 2, false),
            ("later".to_string(), 1, 1, false),
        ];
        for first_build_out in [false, true] {
            let dir = scratch(if first_build_out { "move-out" } else { "move" });
            let (app, state, worker) = archive(&dir, None);
            let root = dir.join("archive");
            let db = dir.join("index").join("library.sqlite");
            if first_build_out {
                // The view left and come back to: a build asked for, the
                // tree not held yet.
                let mut st = state.borrow_mut();
                st.view = View::Folder;
                want(&mut st, &app);
                st.view = View::Roots(Some(root.clone()));
                want(&mut st, &app);
                assert!(st.tree.root.is_none() && st.tree.asked.is_some());
            }
            std::fs::create_dir_all(root.join("later")).unwrap();
            std::fs::rename(
                root.join("day").join("d2.tif"),
                root.join("later").join("d2.tif"),
            )
            .unwrap();
            let report = greycard_library::Library::open(&db)
                .unwrap()
                .index_tree(&root, &mut |_| {})
                .unwrap();
            assert!(report.moved > 0, "{report:?}");
            let count = greycard_library::Library::open_read_only(&db)
                .unwrap()
                .count_under(&root)
                .unwrap();
            assert_eq!(count, 4, "the count unchanged: r, d1, m and d2 moved");
            crate::roots::background_done(&state, &app, &root, &report, false, false);
            land_sent(&state, &app, &worker);
            assert_eq!(rows(&app), moved_tree, "first build out: {first_build_out}");
            state.borrow_mut().index_reader = None;
            drop(state);
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    /// An offline root still shows its tree and opens its folders, from
    /// the index alone: its frames listed from their rows, dimmed.
    #[test]
    fn an_offline_roots_tree_comes_from_the_index() {
        let dir = scratch("offline");
        let (app, state, worker) = archive(&dir, None);
        let root = dir.join("archive");
        std::fs::rename(&root, dir.join("away")).unwrap();
        // The tree built again with the root away.
        {
            let mut st = state.borrow_mut();
            st.tree.stale = true;
            want(&mut st, &app);
        }
        land_sent(&state, &app, &worker);
        assert_eq!(rows(&app).len(), 2);
        pick(&app, "day");
        land_sent(&state, &app, &worker);
        assert_eq!(listed(&state), ["d1.tif", "d2.tif"]);
        assert!(state.borrow().library.offline.contains(&root));
        assert!(app.get_thumbs().row_data(0).unwrap().offline);
        state.borrow_mut().index_reader = None;
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
