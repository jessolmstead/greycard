use super::*;

thread_local! {
    /// The trees asked for, waiting for the test to build and land
    /// them.
    pub(super) static SENT: RefCell<Vec<Build>> = const { RefCell::new(Vec::new()) };
}

use crate::roots::{land_sent, open_view};
use crate::testing::{click, state_for, window};

/// A folder's row in the tree, keyed by the folder's path.
const FOLDER_LINE: &str = "FoldersSection::folder-line";
/// A folder's triangle in the tree, keyed by the folder's path.
const FOLD: &str = "FoldersSection::fold";
/// A folder's tile at the top of the grid, keyed by its name.
const FOLDER_TILE: &str = "GridSheet::folder-tile";

/// The key of the folder at `path`: the path as the tree has it.
fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The middle of the element `id` keyed `key`.
fn middle_key(app: &App, id: &str, key: &str) -> (f32, f32) {
    let (at, size) = crate::testing::by_key(app, id, key);
    (at.x + size.width / 2.0, at.y + size.height / 2.0)
}
use greycard_library::fixture::{A7, R5, R6, write_frame};

fn node(path: &str, name: &str, depth: usize, direct: usize, total: usize, children: bool) -> Node {
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
    assert!(nothing_in(day, false).contains("turn on With subfolders"));
    assert_eq!(nothing_in(day, true), "nothing under 2025");
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
fn archive(dir: &Path, window_px: Option<(f32, f32)>) -> (App, Rc<Shared>, Rc<Worker>) {
    let root = dir.join("archive");
    frames(&root, &["r.tif"]);
    frames(&root.join("day"), &["d1.tif", "d2.tif"]);
    frames(&root.join("day").join("more"), &["m.tif"]);
    open_root(dir, &root, window_px)
}

/// `root` indexed into `dir`'s index, and a window on it with the
/// root in its library and the root's view open, its tree landed.
fn open_root(
    dir: &Path,
    root: &Path,
    window_px: Option<(f32, f32)>,
) -> (App, Rc<Shared>, Rc<Worker>) {
    let root = root.to_path_buf();
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
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        // By name over the folders: the frames are told apart by their
        // place in the list this built, in that order.
        st.views.sort = crate::sort::Kept::by_name();
        // Sidecars as the editor keeps them, so a frame listed
        // from its row stands in until its sidecar is wanted.
        st.window.write_sidecars = true;
        st.views.library.roots.add(&root).unwrap();
        crate::roots::recount(&mut st);
    }
    open_view(&state, &app, &worker, View::Roots(Some(root.clone())));
    land_sent(&state, &app, &worker);
    (app, state, worker)
}

fn listed(state: &Rc<Shared>) -> Vec<String> {
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

/// A title typed for a frame and a folder picked before it was
/// sent: nothing is put on the new folder's frame, and the Title
/// field over it starts clean.
#[test]
fn a_folder_picked_under_a_typed_title_puts_nothing_on_its_frames() {
    let dir = scratch("title");
    let (app, state, worker) = archive(&dir, None);
    pick(&app, "day");
    land_sent(&state, &app, &worker);
    app.invoke_select(0);
    land_sent(&state, &app, &worker);
    // Typed for d1, not sent: the pick's own path, which the field
    // hears of only after the list has moved, and both frames'
    // titles empty, so nothing about the title itself moves.
    app.set_title_text("Dusk".into());
    app.set_title_dirty(true);
    pick(&app, "archive");
    land_sent(&state, &app, &worker);
    slint::platform::update_timers_and_animations();
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["r.tif"]);
    app.invoke_select(0);
    land_sent(&state, &app, &worker);
    assert_eq!(state.borrow().sidecars[0].meta.title, "", "not on r.tif");
    assert_eq!(app.get_title_text(), "");
    assert!(!app.get_title_dirty(), "the field starts clean");
    crate::testing::remove_scratch(state, &dir);
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
    assert!(!state.borrow().views.tree.subfolders, "off by default");

    pick(&app, "day");
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif"]);
    assert_eq!(
        state.borrow().views.view,
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
    assert!(state.borrow().views.tree.subfolders);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif", "m.tif"]);
    assert!(matches!(
        state.borrow().views.view,
        View::Branch { deep: true, .. }
    ));

    // The root, with the switch on: the root's view, as its row.
    pick(&app, "archive");
    land_sent(&state, &app, &worker);
    assert_eq!(state.borrow().views.view, View::Roots(Some(root.clone())));
    // By name across the folders (a test's window lists a view of
    // several folders by name), the root's own frame among them.
    assert_eq!(listed(&state), ["d1.tif", "d2.tif", "m.tif", "r.tif"]);
    // And off again: the root's own frame alone.
    app.set_folder_tree_subfolders(false);
    app.invoke_folder_tree_subfolders_changed();
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["r.tif"]);
    assert_eq!(rows(&app)[0], ("archive".into(), 0, 1, true));

    // A folder with nothing of its own opens empty and says why.
    std::fs::remove_file(root.join("r.tif")).unwrap();
    state.borrow_mut().views.index_reader = None;
    greycard_library::Library::open(&dir.join("index").join("library.sqlite"))
        .unwrap()
        .index_folder(&root, &mut |_| {})
        .unwrap();
    state.borrow_mut().views.index_reader = Some(
        greycard_library::Library::open_read_only(&dir.join("index").join("library.sqlite"))
            .unwrap(),
    );
    pick(&app, "archive");
    land_sent(&state, &app, &worker);
    assert!(listed(&state).is_empty());
    assert!(
        app.get_status().starts_with("nothing directly in archive"),
        "{}",
        app.get_status()
    );
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
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
    assert_eq!(state.borrow().views.view, View::Folder);
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
    assert_eq!(crate::testing::count_keyed(&app, FOLDER_LINE, key(&day)), 1);
    crate::testing::click_key(&app, FOLD, key(&day));
    assert_eq!(rows(&app)[2], ("more".into(), 2, 1, false));
    // Chosen by its row: a click on a row the unfold has just
    // added lands past the box's old foot on the testing backend,
    // which has not laid the box out again.
    pick(&app, "more");
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["m.tif"]);
    assert!(matches!(
        state.borrow().views.view,
        View::Branch { deep: false, .. }
    ));
    assert!(app.get_library_roots().row_data(0).unwrap().on);

    // The day from the disk again, and the switch turned on over
    // it: the day with the folders under it, from the index.
    crate::panel::browser::open_folder(&state, &app, &worker, &day);
    land_sent(&state, &app, &worker);
    assert_eq!(state.borrow().views.view, View::Folder);
    app.set_folder_tree_subfolders(true);
    app.invoke_folder_tree_subfolders_changed();
    land_sent(&state, &app, &worker);
    assert_eq!(
        state.borrow().views.view,
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
    assert_eq!(state.borrow().views.view, View::Folder);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif"]);
    assert!(state.borrow().views.tree.subfolders, "the setting kept");
    assert!(!app.get_folder_tree_subfolders(), "shown off");
    app.set_folder_tree_subfolders(true);
    app.invoke_folder_tree_subfolders_changed();
    land_sent(&state, &app, &worker);
    assert!(matches!(
        state.borrow().views.view,
        View::Branch { deep: true, .. }
    ));
    assert!(app.get_folder_tree_subfolders());
    // The switch off again, and the day from the disk.
    app.set_folder_tree_subfolders(false);
    app.invoke_folder_tree_subfolders_changed();
    land_sent(&state, &app, &worker);
    crate::panel::browser::open_folder(&state, &app, &worker, &day);
    land_sent(&state, &app, &worker);
    assert_eq!(state.borrow().views.view, View::Folder);

    // A folder deeper down, opened from the disk with the day
    // folded over it: the day unfolded to show it, marked.
    let row = rows(&app).iter().position(|r| r.0 == "day").unwrap();
    app.invoke_folder_tree_folded(row as i32);
    assert_eq!(rows(&app).len(), 2, "the day folded");
    crate::panel::browser::open_folder(&state, &app, &worker, &day.join("more"));
    land_sent(&state, &app, &worker);
    assert_eq!(state.borrow().views.view, View::Folder);
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
    assert_eq!(crate::testing::count_keyed(&app, FOLDER_LINE, key(&day)), 0);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// A title typed for a frame, then a click on another folder in the
/// pane, as a hand does it: the click sends the title first, so it
/// is on the frame it was typed for, on disk, and not on the
/// frames of the folder clicked.
#[test]
fn a_click_on_a_folder_sends_the_title_typed_to_its_own_frame() {
    let dir = scratch("click-title");
    let (app, state, worker) = archive(&dir, Some((1500.0, 950.0)));
    let d1 = dir.join("archive").join("day").join("d1.tif");
    pick(&app, "day");
    land_sent(&state, &app, &worker);
    app.invoke_select(0);
    // The frame opened is read, as the window reads the one on
    // screen.
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(state.borrow().files[0], d1);
    assert!(crate::rows::is_loaded(&state.borrow(), 0));
    slint::platform::update_timers_and_animations();
    let (at, size) = crate::testing::by_id(&app, "MetaSection::title-field");
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    crate::testing::type_text(&app, "Dusk");
    assert!(app.get_title_dirty());
    // The root's own row, at the head of its tree.
    let root = dir.join("archive");
    crate::testing::click_key(&app, FOLDER_LINE, key(&root));
    land_sent(&state, &app, &worker);
    slint::platform::update_timers_and_animations();
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["r.tif"], "the click opened it");
    let s = greycard_edit::Sidecar::load_as_file(&d1)
        .unwrap()
        .expect("d1's sidecar");
    assert_eq!(s.meta.title, "Dusk");
    assert_eq!(state.borrow().sidecars[0].meta.title, "", "not on r.tif");
    assert_eq!(app.get_title_text(), "");
    crate::testing::remove_scratch(state, &dir);
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
    crate::testing::click_key(&app, FOLDER_LINE, key(&day));
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif"]);
    assert_eq!(
        state.borrow().views.recent.open.as_deref(),
        Some(day.as_path())
    );
    assert_eq!(
        state
            .borrow()
            .views
            .recent
            .folders
            .first()
            .map(String::as_str),
        day.to_str()
    );
    assert_eq!(app.get_open_folder_name(), "day");
    assert_eq!(app.get_open_folder_root(), "archive");
    assert!(rows(&app)[1].3, "the day marked open");

    // Its triangle unfolds it without opening it.
    crate::testing::click_key(&app, FOLD, key(&day));
    land_sent(&state, &app, &worker);
    assert_eq!(rows(&app).len(), 3);
    assert_eq!(rows(&app)[1], ("day".into(), 1, 2, true));
    assert_eq!(rows(&app)[2], ("more".into(), 2, 1, false));
    assert_eq!(listed(&state), ["d1.tif", "d2.tif"], "still the day's");
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// A screen reader's press on a folder's triangle folds and unfolds
/// it, as a click does; a folder with none under it has no fold to
/// press, says so, and the press does nothing.
#[test]
fn a_screen_reader_presses_a_folders_fold() {
    let dir = scratch("press-fold");
    let (app, state, worker) = archive(&dir, Some((1200.0, 800.0)));
    // The fold of the tree's row `i`, by that row's path, its key.
    let fold = |i: usize| {
        crate::testing::settle(&app);
        let path = app.get_folder_tree().row_data(i).expect("the row").path;
        crate::testing::keyed(&app, FOLD, &path).unwrap_or_else(|| panic!("no fold {path}"))
    };
    let expanded = || -> Vec<bool> { app.get_folder_tree().iter().map(|r| r.expanded).collect() };
    assert_eq!(expanded(), [true, false], "the root open, the day not");
    assert_eq!(fold(1).accessible_enabled(), Some(true));
    fold(1).invoke_accessible_default_action();
    land_sent(&state, &app, &worker);
    assert_eq!(expanded(), [true, true, false], "the day unfolded");
    assert_eq!(rows(&app)[2], ("more".into(), 2, 1, false));
    let leaf = fold(2);
    assert_eq!(leaf.accessible_enabled(), Some(false), "more has none");
    leaf.invoke_accessible_default_action();
    land_sent(&state, &app, &worker);
    assert_eq!(expanded(), [true, true, false], "nothing to unfold");
    assert_eq!(state.borrow().views.tree.expanded.len(), 2);
    fold(1).invoke_accessible_default_action();
    land_sent(&state, &app, &worker);
    assert_eq!(expanded(), [true, false], "the day folded again");
    done(&dir, state);
}

/// A pass that adds a folder under the root has the tree built
/// again with it; one that only refreshed a sidecar does not.
#[test]
fn the_tree_follows_a_pass_that_adds_a_folder() {
    let dir = scratch("pass");
    let (app, state, worker) = archive(&dir, None);
    let root = dir.join("archive");
    let token = state.borrow().views.tree.token;
    let quiet = greycard_library::Report {
        meta_refreshed: 1,
        ..Default::default()
    };
    crate::roots::background_done(&state, &app, &root, &quiet, false, false);
    land_sent(&state, &app, &worker);
    assert_eq!(
        state.borrow().views.tree.token,
        token,
        "no build for a rating"
    );

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
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
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
            st.views.view = View::Folder;
            want(&mut st, &app);
            st.views.view = View::Roots(Some(root.clone()));
            want(&mut st, &app);
            assert!(st.views.tree.root.is_none() && st.views.tree.asked.is_some());
        }
        std::fs::create_dir_all(root.join("later")).unwrap();
        greycard_core::testing::rename_retry(
            root.join("day").join("d2.tif"),
            root.join("later").join("d2.tif"),
        );
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
        state.borrow_mut().views.index_reader = None;
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }
}

/// An offline root still shows its tree and opens its folders, from
/// the index alone: its frames listed from their rows, dimmed.
#[test]
fn an_offline_roots_tree_comes_from_the_index() {
    let dir = scratch("offline");
    let (app, state, worker) = archive(&dir, None);
    let root = dir.join("archive");
    greycard_core::testing::rename_retry(&root, dir.join("away"));
    // The tree built again with the root away.
    {
        let mut st = state.borrow_mut();
        st.views.tree.stale = true;
        want(&mut st, &app);
    }
    land_sent(&state, &app, &worker);
    assert_eq!(rows(&app).len(), 2);
    pick(&app, "day");
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif"]);
    assert!(state.borrow().views.library.offline.contains(&root));
    assert!(app.get_thumbs().row_data(0).unwrap().offline);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// The two ways an archive lays out a client: `cedar` with its raws
/// in it and an export folder beside them, and `birch` with nothing
/// of its own, a same-named folder of raws under it and an export
/// folder beside that. Under a root `studio`, in a 1500 by 950
/// window.
fn studio(dir: &Path) -> (App, Rc<Shared>, Rc<Worker>) {
    let root = dir.join("studio");
    frames(&root.join("cedar"), &["c1.tif", "c2.tif", "c3.tif"]);
    frames(&root.join("cedar").join("export"), &["ce1.tif", "ce2.tif"]);
    frames(&root.join("birch").join("birch"), &["b1.tif", "b2.tif"]);
    frames(&root.join("birch").join("export"), &["be1.tif"]);
    open_root(dir, &root, Some((1500.0, 950.0)))
}

/// The grid's tiles: each one's name and count.
fn tiles(app: &App) -> Vec<(String, i32)> {
    app.get_grid_tiles()
        .iter()
        .map(|t| (t.name.to_string(), t.count))
        .collect()
}

fn done(dir: &Path, state: Rc<Shared>) {
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(dir);
}

/// A tile's menu opened by a right-click while the filter's words
/// have the focus, the presses as the window has them (its event
/// filter, which sets `menu-up` from Slint's popups, then Slint): a
/// back press over another tile only closes the menu, and the next
/// one visits the folder before, as the grid's side buttons do.
#[test]
fn a_tiles_menu_from_a_text_box_holds_the_folder_visit() {
    use crate::panel::side_buttons::SideButtons;
    use slint::platform::{PointerEventButton, WindowEvent, update_timers_and_animations};
    use slint::winit_030::winit::event::{
        DeviceId, ElementState, MouseButton, WindowEvent as Heard,
    };
    let dir = scratch("tiles-menu-box");
    let (app, state, _worker) = studio(&dir);
    app.set_grid_open(true);
    update_timers_and_animations();
    let visits = Rc::new(RefCell::new(Vec::new()));
    let seen = visits.clone();
    app.on_visit_folder(move |back| seen.borrow_mut().push(back));
    let middle = |name: &str| {
        let (x, y) = middle_key(&app, FOLDER_TILE, name);
        slint::LogicalPosition::new(x, y)
    };
    let mut b = SideButtons::default();
    // A press as the window has it: its filter first, then Slint.
    let mut press_over = |position, button: PointerEventButton| {
        let heard = |state| Heard::MouseInput {
            device_id: DeviceId::dummy(),
            state,
            button: match button {
                PointerEventButton::Back => MouseButton::Back,
                _ => MouseButton::Right,
            },
        };
        b.event(&app, &heard(ElementState::Pressed));
        for event in [
            WindowEvent::PointerMoved { position },
            WindowEvent::PointerPressed { position, button },
            WindowEvent::PointerReleased { position, button },
        ] {
            app.window().dispatch_event(event);
        }
        b.event(&app, &heard(ElementState::Released));
    };
    crate::testing::press(&app, "/");
    update_timers_and_animations();
    let typing = app.global::<crate::Typing>();
    assert_eq!(typing.get_boxes(), 1, "the filter's words");
    press_over(middle("cedar"), PointerEventButton::Right);
    assert_eq!(typing.get_boxes(), 0, "the box let go");
    assert!(app.get_menu_up());
    press_over(middle("birch"), PointerEventButton::Back);
    assert!(visits.borrow().is_empty(), "the press only closed the menu");
    assert!(!app.get_menu_up());
    press_over(middle("birch"), PointerEventButton::Back);
    assert_eq!(*visits.borrow(), [true]);
    done(&dir, state);
}

/// A folder with folders under it has a tile for each, counted as
/// the tree counts them, under it in all; one with none has none.
/// The root's own view has its first-level folders.
#[test]
fn a_folder_with_folders_has_their_tiles_and_one_without_none() {
    let dir = scratch("tiles");
    let (app, state, worker) = archive(&dir, None);
    assert_eq!(tiles(&app), [("day".into(), 3)], "the root's own view");
    pick(&app, "day");
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif"]);
    assert_eq!(tiles(&app), [("more".into(), 1)]);
    let row = rows(&app).iter().position(|r| r.0 == "day").unwrap();
    app.invoke_folder_tree_folded(row as i32);
    assert_eq!(rows(&app).len(), 3, "the day unfolded");
    assert_eq!(tiles(&app), [("more".into(), 1)], "a fold is the tree's");
    pick(&app, "more");
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["m.tif"]);
    assert!(tiles(&app).is_empty());

    // The client layouts: the frames and the export folder, never
    // mixed; the root's view has both clients.
    let dir2 = scratch("tiles-studio");
    let (app2, state2, worker2) = studio(&dir2);
    assert_eq!(tiles(&app2), [("birch".into(), 3), ("cedar".into(), 5)]);
    pick(&app2, "cedar");
    land_sent(&state2, &app2, &worker2);
    assert_eq!(listed(&state2), ["c1.tif", "c2.tif", "c3.tif"]);
    assert_eq!(tiles(&app2), [("export".into(), 2)]);
    done(&dir2, state2);

    // A folder from the disk under the root takes the root's tree,
    // and its tiles with it.
    crate::panel::browser::open_folder(&state, &app, &worker, &dir.join("archive").join("day"));
    land_sent(&state, &app, &worker);
    assert_eq!(state.borrow().views.view, View::Folder);
    assert_eq!(tiles(&app), [("more".into(), 1)]);
    // A folder under no root: no tree, and its own folders' tiles
    // from the disk.
    let elsewhere = dir.join("elsewhere");
    frames(&elsewhere, &["e.tif"]);
    frames(&elsewhere.join("sub"), &["s.tif"]);
    crate::panel::browser::open_folder(&state, &app, &worker, &elsewhere);
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["e.tif"]);
    assert!(!app.get_folder_tree_shown());
    assert_eq!(tiles(&app), [("sub".into(), 1)]);
    done(&dir, state);
}

/// A folder with nothing of its own opens to its tiles alone, the
/// status line saying why as it did; a tile clicked in the grid
/// opens that folder exactly as its row in the tree does: the same
/// view, the tree marked at it, Recently opened told.
#[test]
fn a_bare_folder_opens_to_its_tiles_and_a_tile_opens_as_the_row_does() {
    let dir = scratch("bare");
    let (app, state, worker) = studio(&dir);
    let birch = dir.join("studio").join("birch");
    pick(&app, "birch");
    land_sent(&state, &app, &worker);
    assert!(listed(&state).is_empty());
    assert_eq!(tiles(&app), [("birch".into(), 2), ("export".into(), 1)]);
    assert!(
        app.get_status().starts_with("nothing directly in birch"),
        "{}",
        app.get_status()
    );

    app.set_grid_open(true);
    slint::platform::update_timers_and_animations();
    assert_eq!(
        crate::testing::count_keyed(&app, FOLDER_TILE, "birch"),
        1,
        "one tile; the tree's row is named birch"
    );
    let (x, y) = middle_key(&app, FOLDER_TILE, "birch");
    click(&app, x, y);
    land_sent(&state, &app, &worker);
    let inner = birch.join("birch");
    let by_tile = state.borrow().views.view.clone();
    assert_eq!(
        by_tile,
        View::Branch {
            root: dir.join("studio"),
            folder: inner.clone(),
            deep: false
        }
    );
    assert_eq!(listed(&state), ["b1.tif", "b2.tif"]);
    assert!(tiles(&app).is_empty());
    assert_eq!(
        state.borrow().views.recent.open.as_deref(),
        Some(inner.as_path())
    );
    assert_eq!(
        state
            .borrow()
            .views
            .recent
            .folders
            .first()
            .map(String::as_str),
        inner.to_str()
    );
    let marked: Vec<String> = rows(&app)
        .into_iter()
        .filter(|r| r.3)
        .map(|r| format!("{}:{}", r.0, r.1))
        .collect();
    assert_eq!(marked, ["birch:2"], "the inner birch marked, two deep");

    // The same folder by its row in the tree: the same view.
    pick(&app, "studio");
    land_sent(&state, &app, &worker);
    pick(&app, "birch");
    land_sent(&state, &app, &worker);
    let row = rows(&app)
        .iter()
        .position(|r| r.0 == "birch" && r.1 == 2)
        .unwrap();
    app.invoke_folder_tree_picked(row as i32);
    land_sent(&state, &app, &worker);
    assert_eq!(state.borrow().views.view, by_tile);
    done(&dir, state);
}

/// Row `i` chosen and its frame opened, its sidecar read as the
/// window reads the one on screen.
fn open_at(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, i: i32) {
    app.invoke_select(i);
    crate::rows::land_pending(state, app, worker);
}

/// The frame on screen, by name, and the develop it is on.
fn on_screen(state: &Rc<Shared>) -> (Option<String>, u64) {
    let st = state.borrow();
    let name = st
        .current
        .and_then(|c| st.path_at(c))
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned());
    (name, st.develop.generation)
}

/// The switch flipped, and what it sends landed: the reads of the
/// list, and the sidecars of a frame it opens.
fn flip(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, on: bool) {
    app.set_folder_tree_subfolders(on);
    app.invoke_folder_tree_subfolders_changed();
    land_sent(state, app, worker);
    crate::rows::land_pending(state, app, worker);
}

/// The switch turned on over a folder with a frame open: the list
/// grows to those under it around the frame, which stays on screen
/// on the develop it had, never opened again; the tiles go, and the
/// tree's mark and the switch say the new view.
#[test]
fn the_switch_turned_on_keeps_the_frame_on_screen() {
    let dir = scratch("switch-on-kept");
    let (app, state, worker) = archive(&dir, Some((1500.0, 950.0)));
    let (root, day) = (dir.join("archive"), dir.join("archive").join("day"));
    pick(&app, "day");
    land_sent(&state, &app, &worker);
    open_at(&state, &app, &worker, 1);
    let before = on_screen(&state);
    assert_eq!(before.0.as_deref(), Some("d2.tif"));
    assert_eq!(tiles(&app), [("more".into(), 1)]);

    flip(&state, &app, &worker, true);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif", "m.tif"]);
    assert_eq!(
        state.borrow().views.view,
        View::Branch {
            root,
            folder: day,
            deep: true
        }
    );
    assert_eq!(
        on_screen(&state),
        before,
        "the same frame, not opened again"
    );
    assert!(tiles(&app).is_empty());
    assert!(app.get_folder_tree_subfolders());
    assert_eq!(rows(&app)[1], ("day".into(), 1, 3, true));
    crate::testing::remove_scratch(state, &dir);
}

/// The switch turned off with the frame on screen directly in the
/// folder: the list shrinks to the folder's own around it, and the
/// frame stays on screen; the tiles come back.
#[test]
fn the_switch_turned_off_keeps_a_frame_of_the_folders_own() {
    let dir = scratch("switch-off-kept");
    let (app, state, worker) = archive(&dir, Some((1500.0, 950.0)));
    state.borrow_mut().views.tree.subfolders = true;
    app.set_folder_tree_subfolders(true);
    pick(&app, "day");
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif", "m.tif"]);
    open_at(&state, &app, &worker, 1);
    let before = on_screen(&state);
    assert_eq!(before.0.as_deref(), Some("d2.tif"));

    flip(&state, &app, &worker, false);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif"]);
    assert!(matches!(
        state.borrow().views.view,
        View::Branch { deep: false, .. }
    ));
    assert_eq!(
        on_screen(&state),
        before,
        "the same frame, not opened again"
    );
    assert_eq!(tiles(&app), [("more".into(), 1)]);
    assert!(!app.get_folder_tree_subfolders());
    crate::testing::remove_scratch(state, &dir);
}

/// The switch turned off with the frame on screen in a folder
/// below: it is not in the new list, so the view is opened again
/// as any is, and the frame of the folder below is not kept in the
/// folder's own list as a merge would keep it.
#[test]
fn the_switch_turned_off_under_a_frame_below_opens_the_folder_again() {
    let dir = scratch("switch-off-below");
    let (app, state, worker) = archive(&dir, Some((1500.0, 950.0)));
    state.borrow_mut().views.tree.subfolders = true;
    app.set_folder_tree_subfolders(true);
    pick(&app, "day");
    land_sent(&state, &app, &worker);
    open_at(&state, &app, &worker, 2);
    assert_eq!(on_screen(&state).0.as_deref(), Some("m.tif"));

    flip(&state, &app, &worker, false);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif"]);
    assert!(matches!(
        state.borrow().views.view,
        View::Branch { deep: false, .. }
    ));
    // The list replaced and its first frame to be opened: in a
    // test's window, from the rendering setup it waits for.
    assert_eq!(on_screen(&state).0, None);
    assert_eq!(state.borrow().launch.select_at_start, Some(0));
    crate::testing::remove_scratch(state, &dir);
}

/// Over the root, whose view with the switch on is the root's own:
/// on and off again with the root's own frame on screen, the list
/// grows and shrinks around it and it stays on screen throughout.
#[test]
fn the_switch_over_the_root_keeps_the_roots_own_frame() {
    let dir = scratch("switch-root-kept");
    let (app, state, worker) = archive(&dir, Some((1500.0, 950.0)));
    let root = dir.join("archive");
    pick(&app, "archive");
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["r.tif"]);
    open_at(&state, &app, &worker, 0);
    let before = on_screen(&state);
    assert_eq!(before.0.as_deref(), Some("r.tif"));

    flip(&state, &app, &worker, true);
    assert_eq!(state.borrow().views.view, View::Roots(Some(root.clone())));
    assert_eq!(listed(&state), ["d1.tif", "d2.tif", "m.tif", "r.tif"]);
    assert_eq!(
        on_screen(&state),
        before,
        "the same frame, not opened again"
    );
    assert!(tiles(&app).is_empty());
    assert_eq!(state.borrow().views.recent.open, None, "the root's view");

    flip(&state, &app, &worker, false);
    assert_eq!(
        state.borrow().views.view,
        View::Branch {
            root: root.clone(),
            folder: root.clone(),
            deep: false
        }
    );
    assert_eq!(listed(&state), ["r.tif"]);
    assert_eq!(
        on_screen(&state),
        before,
        "the same frame, not opened again"
    );
    assert_eq!(tiles(&app), [("day".into(), 3)]);
    assert_eq!(state.borrow().views.recent.open, Some(root));
    crate::testing::remove_scratch(state, &dir);
}

/// A folder opened from the disk under a root, a frame of it on
/// screen, and the switch turned on: the folder with those under
/// it from the index, the frame kept on screen.
#[test]
fn the_switch_turned_on_over_a_disk_folder_keeps_the_frame_on_screen() {
    let dir = scratch("switch-disk-kept");
    let (app, state, worker) = archive(&dir, Some((1500.0, 950.0)));
    let day = dir.join("archive").join("day");
    crate::panel::browser::open_folder(&state, &app, &worker, &day);
    land_sent(&state, &app, &worker);
    assert_eq!(state.borrow().views.view, View::Folder);
    open_at(&state, &app, &worker, 0);
    let before = on_screen(&state);
    assert_eq!(before.0.as_deref(), Some("d1.tif"));

    flip(&state, &app, &worker, true);
    assert!(matches!(
        state.borrow().views.view,
        View::Branch { deep: true, .. }
    ));
    assert_eq!(listed(&state), ["d1.tif", "d2.tif", "m.tif"]);
    assert_eq!(
        on_screen(&state),
        before,
        "the same frame, not opened again"
    );
    assert!(app.get_folder_tree_subfolders());
    crate::testing::remove_scratch(state, &dir);
}

/// A read of the list out when the switch flips is of the view
/// before: the new view's is sent at once beside it, and the old
/// one, landing after, is let go and does not put its list back.
#[test]
fn the_switch_over_a_read_still_out_reads_the_new_view_at_once() {
    let dir = scratch("switch-merging");
    let (app, state, worker) = archive(&dir, Some((1500.0, 950.0)));
    pick(&app, "day");
    land_sent(&state, &app, &worker);
    open_at(&state, &app, &worker, 1);
    let before = on_screen(&state);
    assert!(crate::roots::refresh_view(&state, &app, &worker));
    assert_eq!(crate::roots::sent_reads(), 1, "the day's own read out");

    app.set_folder_tree_subfolders(true);
    app.invoke_folder_tree_subfolders_changed();
    assert_eq!(crate::roots::sent_reads(), 2, "the new view's sent at once");
    crate::roots::land_sent_last_first(&state, &app, &worker);
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif", "m.tif"]);
    assert_eq!(on_screen(&state), before);
    assert!(!state.borrow().views.library.merging);
    crate::testing::remove_scratch(state, &dir);
}

/// A read of the list out and a view still opening when the switch
/// flips: nothing is read in place, the view is opened as any is,
/// and the read out is still marked out, so a refresh asked for
/// meanwhile waits for it rather than sending a second.
#[test]
fn the_switch_while_a_view_opens_leaves_the_read_out_marked() {
    let dir = scratch("switch-loading");
    let (app, state, worker) = archive(&dir, Some((1500.0, 950.0)));
    pick(&app, "day");
    land_sent(&state, &app, &worker);
    open_at(&state, &app, &worker, 1);
    assert!(crate::roots::refresh_view(&state, &app, &worker));
    {
        let mut st = state.borrow_mut();
        st.views.library.loading = true;
        st.views.library.loading_since = Some(std::time::Instant::now());
    }
    app.set_folder_tree_subfolders(true);
    app.invoke_folder_tree_subfolders_changed();
    {
        let st = state.borrow();
        assert!(st.views.library.merging, "the read out is still out");
        assert!(st.views.library.merge_since.is_some());
        // Opened as any view is: the view changes when its read
        // lands, not before.
        assert!(matches!(st.views.view, View::Branch { deep: false, .. }));
    }
    assert_eq!(crate::roots::sent_reads(), 2, "the open's read sent");
    land_sent(&state, &app, &worker);
    assert!(matches!(
        state.borrow().views.view,
        View::Branch { deep: true, .. }
    ));
    assert_eq!(listed(&state), ["d1.tif", "d2.tif", "m.tif"]);
    crate::testing::remove_scratch(state, &dir);
}

/// A folder from the disk whose pass was saying something on the
/// filter's line (its progress, or that the index is busy) and the
/// switch turned on: the pass is dropped, and its words with it,
/// though the list (a folder with nothing under it) does not move.
#[test]
fn the_switch_over_a_disk_folder_clears_what_its_pass_said() {
    let dir = scratch("switch-pass-said");
    let (app, state, worker) = archive(&dir, Some((1500.0, 950.0)));
    let day = dir.join("archive").join("day");
    let said = |state: &Rc<Shared>, app: &App| {
        crate::panel::cull::show_filter(&state.borrow(), app);
        app.get_filter_facet_note().to_string()
    };
    crate::panel::browser::open_folder(&state, &app, &worker, &day.join("more"));
    land_sent(&state, &app, &worker);
    open_at(&state, &app, &worker, 0);
    state.borrow_mut().views.index_progress = Some((0, 1));
    assert_eq!(said(&state, &app), "Indexing the folder: 0 of 1");
    flip(&state, &app, &worker, true);
    assert_eq!(listed(&state), ["m.tif"]);
    assert!(matches!(
        state.borrow().views.view,
        View::Branch { deep: true, .. }
    ));
    assert!(!app.get_filter_facet_note().starts_with("Indexing"));

    flip(&state, &app, &worker, false);
    crate::panel::browser::open_folder(&state, &app, &worker, &day);
    land_sent(&state, &app, &worker);
    open_at(&state, &app, &worker, 0);
    {
        let mut st = state.borrow_mut();
        st.views.index_tries = 1;
        st.views.index_error = Some("Library index busy; trying again (locked)".into());
    }
    assert!(said(&state, &app).starts_with("Library index busy"));
    flip(&state, &app, &worker, true);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif", "m.tif"]);
    assert!(!app.get_filter_facet_note().starts_with("Library index"));
    assert_eq!(state.borrow().views.index_tries, 0);
    crate::testing::remove_scratch(state, &dir);
}

/// In Cull the switch keeps the frame on screen too, and the mode
/// stays on over the new list.
#[test]
fn the_switch_in_cull_keeps_the_frame_and_the_mode() {
    let dir = scratch("switch-cull");
    let (app, state, worker) = archive(&dir, Some((1500.0, 950.0)));
    pick(&app, "day");
    land_sent(&state, &app, &worker);
    open_at(&state, &app, &worker, 1);
    app.invoke_cull_toggled();
    assert!(state.borrow().culling.cull.is_some());
    let before = on_screen(&state);

    flip(&state, &app, &worker, true);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif", "m.tif"]);
    // The frame by its path is what an open of the view would
    // lose; the mode stays on through an open as well.
    assert_eq!(on_screen(&state), before);
    assert!(state.borrow().culling.cull.is_some(), "still culling");
    crate::testing::remove_scratch(state, &dir);
}

/// With the switch on there are no tiles: a folder's view holds
/// everything under it, and so does the root's.
#[test]
fn the_switch_on_takes_the_tiles_away() {
    let dir = scratch("tiles-switch");
    let (app, state, worker) = studio(&dir);
    pick(&app, "cedar");
    land_sent(&state, &app, &worker);
    assert_eq!(tiles(&app).len(), 1);
    app.set_folder_tree_subfolders(true);
    app.invoke_folder_tree_subfolders_changed();
    land_sent(&state, &app, &worker);
    assert_eq!(
        listed(&state).len(),
        5,
        "the export's frames with the switch"
    );
    assert!(tiles(&app).is_empty());
    pick(&app, "studio");
    land_sent(&state, &app, &worker);
    assert!(matches!(state.borrow().views.view, View::Roots(Some(_))));
    assert!(tiles(&app).is_empty());
    // Off again over the root: its first-level folders.
    app.set_folder_tree_subfolders(false);
    app.invoke_folder_tree_subfolders_changed();
    land_sent(&state, &app, &worker);
    assert_eq!(tiles(&app).len(), 2);
    done(&dir, state);
}

/// The tiles push the cells down by their row: the grid reports the
/// tiles' height with its range, a tile is where the first cell was
/// and a click there opens the folder, the cells sit a tile row
/// lower, and the arrows move through the frames alone.
#[test]
fn the_tiles_shift_the_cells_and_the_keys_skip_them() {
    let dir = scratch("tiles-layout");
    let (app, state, worker) = studio(&dir);
    pick(&app, "cedar");
    land_sent(&state, &app, &worker);
    let seen = Rc::new(RefCell::new(Vec::new()));
    {
        // Heard here, and the cells moved as the browser's own
        // report moves them.
        let (seen, app_weak) = (seen.clone(), app.as_weak());
        app.on_grid_range(move |scroll, height, cols, top| {
            seen.borrow_mut().push((scroll, height, cols, top));
            if let Some(app) = app_weak.upgrade() {
                let (at, n) = crate::cells::layout(
                    &app,
                    app.get_grid_cell(),
                    cols,
                    app.get_thumbs().row_count() as i32,
                    &[],
                    crate::panel::browser::grid_shape(&app),
                )
                .window(scroll, height, top);
                crate::cells::show(&app, crate::cells::View::Grid, at, n);
            }
        });
    }
    app.invoke_select(0);
    app.set_grid_open(true);
    slint::platform::update_timers_and_animations();
    crate::testing::press(&app, slint::platform::Key::Shift);
    // Six columns beside the pane; one tile row, 88 and its gap.
    let top = crate::grid::tiles_top(1, 176.0, 6);
    assert_eq!(top, 92.0);
    assert_eq!(seen.borrow().last().map(|r| (r.2, r.3)), Some((6, top)));
    // The tile at the sheet's first place: under the header (its two
    // rows of controls, two of facets, and the ranges' row, 177) and
    // the padding, at the pane's edge (240), the padding and the slack
    // beside six cells in 1,260.
    let slack = crate::grid::slack(1260.0, 176.0, 6);
    let (x0, y0) = (240.0 + crate::grid::PAD + slack, 177.0 + crate::grid::PAD);
    let (at, size) = crate::testing::by_key(&app, FOLDER_TILE, "export");
    assert_eq!((at.x, at.y), (x0, y0));
    assert_eq!((size.width, size.height), (176.0, 88.0));
    // A frame's cell, a tile row lower: a click on the third opens
    // it.
    let cell = |i: f32| (x0 + i * 180.0 + 88.0, y0 + top + 88.0);
    let (x, y) = cell(2.0);
    click(&app, x, y);
    assert_eq!(app.get_selected(), 2);
    // The arrows: up from the first row stays on the frames, left
    // and right walk them; no tile is ever the selection.
    crate::testing::press(&app, slint::platform::Key::UpArrow);
    assert_eq!(app.get_selected(), 2);
    crate::testing::press(&app, slint::platform::Key::LeftArrow);
    crate::testing::press(&app, slint::platform::Key::LeftArrow);
    crate::testing::press(&app, slint::platform::Key::LeftArrow);
    assert_eq!(app.get_selected(), 0);
    crate::testing::press(&app, slint::platform::Key::UpArrow);
    assert_eq!(app.get_selected(), 0);
    assert_eq!(
        listed(&state),
        ["c1.tif", "c2.tif", "c3.tif"],
        "nothing opened"
    );
    // And the tile, where the first cell was without it, opens the
    // export folder.
    click(&app, x0 + 88.0, y0 + 44.0);
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["ce1.tif", "ce2.tif"]);
    assert!(tiles(&app).is_empty());
    crate::testing::press(&app, slint::platform::Key::Shift);
    assert_eq!(
        seen.borrow().last().map(|r| r.3),
        Some(0.0),
        "no tiles, no shift"
    );
    done(&dir, state);
}

/// A tile's right-click offers Reveal while the root is online, and
/// nothing while it is offline; over an offline root the tiles come
/// from the index all the same.
#[test]
fn an_offline_roots_tiles_come_from_the_index_with_no_reveal() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let dir = scratch("tiles-offline");
    let (app, state, worker) = studio(&dir);
    app.set_grid_open(true);
    slint::platform::update_timers_and_animations();
    let right = |app: &App, name: &str| {
        let (x, y) = middle_key(app, FOLDER_TILE, name);
        let position = slint::LogicalPosition::new(x, y);
        for event in [
            WindowEvent::PointerMoved { position },
            WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Right,
            },
            WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Right,
            },
        ] {
            app.window().dispatch_event(event);
        }
    };
    right(&app, "cedar");
    assert!(app.get_menu_up(), "the tile's menu, online");
    assert_eq!(listed(&state).len(), 8, "the root's view: all under it");
    crate::testing::press(&app, slint::platform::Key::Escape);
    assert!(!app.get_menu_up());
    // The press that closes the menu is not a click on a tile: the
    // menu up again over cedar, and a left press on birch.
    right(&app, "cedar");
    assert!(app.get_menu_up());
    let (x, y) = middle_key(&app, FOLDER_TILE, "birch");
    click(&app, x, y);
    land_sent(&state, &app, &worker);
    assert!(!app.get_menu_up(), "the press closed the menu");
    assert!(
        matches!(state.borrow().views.view, View::Roots(Some(_))),
        "and opened nothing"
    );
    // What the menu's Reveal shows: each tile's own folder, online.
    let root = dir.join("studio");
    {
        let st = state.borrow();
        assert_eq!(tile_folder(&st, 0), Some(root.join("birch")));
        assert_eq!(reveal_target(&st, 0), Some(root.join("birch")));
        assert_eq!(reveal_target(&st, 1), Some(root.join("cedar")));
        assert_eq!(reveal_target(&st, 2), None, "no third tile");
        assert_eq!(reveal_target(&st, -1), None);
    }
    app.invoke_grid_tile_reveal(0);
    assert!(matches!(state.borrow().views.view, View::Roots(Some(_))));

    // The root away: its tree and tiles from the index, dimmed, no
    // menu.
    greycard_core::testing::rename_retry(&root, dir.join("away"));
    {
        let mut st = state.borrow_mut();
        st.views.tree.stale = true;
        want(&mut st, &app);
    }
    land_sent(&state, &app, &worker);
    pick(&app, "birch");
    land_sent(&state, &app, &worker);
    assert!(state.borrow().views.library.offline.contains(&root));
    assert_eq!(tiles(&app), [("birch".into(), 2), ("export".into(), 1)]);
    assert!(app.get_grid_tiles().iter().all(|t| t.offline));
    right(&app, "export");
    assert!(!app.get_menu_up(), "no reveal for an offline root");
    assert_eq!(
        tile_folder(&state.borrow(), 1),
        Some(root.join("birch").join("export"))
    );
    assert_eq!(
        reveal_target(&state.borrow(), 1),
        None,
        "offline: nothing to show"
    );
    // A click still opens it, from the index.
    let (x, y) = middle_key(&app, FOLDER_TILE, "birch");
    click(&app, x, y);
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["b1.tif", "b2.tif"]);
    done(&dir, state);
}

/// Move rejects in a folder of the tree makes a rejects folder the
/// tree and the grid's tiles show at once: the move is told to the
/// index, which says so, and the tree is built again although the
/// root's count has not moved and the folder passes after the move
/// find nothing to report. Delete rejects folder takes it out of
/// both again.
#[test]
fn the_rejects_folder_a_move_makes_shows_in_the_tree_and_the_tiles() {
    use crate::library::{Indexer, Told};
    use std::time::Duration;
    let dir = scratch("moved");
    let root = dir.join("archive");
    let day = root.join("day");
    frames(&root, &["r.tif"]);
    let d = frames(&day, &["d1.tif", "d2.tif"]);
    frames(&day.join("more"), &["m.tif"]);
    let mut reject = greycard_edit::Sidecar::default();
    reject.meta.flag = greycard_edit::meta::Flag::Reject;
    reject.save(&d[0]).unwrap();
    let (app, state, worker) = open_root(&dir, &root, None);
    let db = dir.join("index").join("library.sqlite");
    let (tx, rx) = std::sync::mpsc::channel();
    let indexer = Indexer::start(db, move |told| {
        let _ = tx.send(told);
    })
    .expect("the indexer starts");
    let wait = |want: &dyn Fn(&Told) -> bool| loop {
        let told = rx
            .recv_timeout(Duration::from_secs(20))
            .expect("the indexer answers");
        if want(&told) {
            return told;
        }
    };
    wait(&|t| matches!(t, Told::Opened(_)));
    crate::STATE.with(|s| *s.borrow_mut() = Some(state.clone()));
    state.borrow_mut().views.index = Some(indexer);

    pick(&app, "day");
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif"]);
    assert_eq!(tiles(&app), [("more".into(), 1)]);
    app.invoke_rejects_answered(true);
    assert!(
        day.join("rejects").join("d1.tif").exists(),
        "{}",
        app.get_status()
    );
    let moved = wait(&|t| matches!(t, Told::Moved(_)));
    crate::library::told(&app, moved);
    land_sent(&state, &app, &worker);
    assert_eq!(
        tiles(&app),
        [("more".into(), 1), ("rejects".into(), 1)],
        "{:?}",
        rows(&app)
    );
    let row = rows(&app).iter().position(|r| r.0 == "day").unwrap();
    app.invoke_folder_tree_folded(row as i32);
    let names: Vec<String> = rows(&app).into_iter().map(|r| r.0).collect();
    assert_eq!(names, ["archive", "day", "more", "rejects"]);

    // Delete rejects folder takes it out of the tree and the tiles
    // as soon as the index has forgotten its rows.
    // No trash in a test: the folder refuses it, so the sheet
    // offers the permanent delete.
    {
        let mut st = state.borrow_mut();
        st.window.deletes_allowed = true;
        st.sheets
            .trash_refused
            .insert(dunce::canonicalize(day.join("rejects")).unwrap());
    }
    app.invoke_delete_asked("rejects".into());
    app.invoke_delete_answered(2);
    crate::panel::delete::tests::land_all(&state, &app, &worker);
    assert!(!day.join("rejects").exists(), "{}", app.get_status());
    let forgotten = wait(&|t| matches!(t, Told::Forgotten(..)));
    let said = format!("{forgotten:?}");
    crate::library::told(&app, forgotten);
    land_sent(&state, &app, &worker);
    let held = greycard_library::Library::open_read_only(&dir.join("index").join("library.sqlite"))
        .and_then(|l| l.folder_counts_under_canonical(&root));
    assert_eq!(
        tiles(&app),
        [("more".into(), 1)],
        "{:?}; {said}; the index holds {held:?}",
        rows(&app)
    );
    let names: Vec<String> = rows(&app).into_iter().map(|r| r.0).collect();
    assert_eq!(names, ["archive", "day", "more"]);

    let indexer = state.borrow_mut().views.index.take().unwrap();
    indexer.stop(Duration::from_secs(20));
    crate::STATE.with(|s| *s.borrow_mut() = None);
    drop(worker);
    done(&dir, state);
}

/// A folder opened from the disk under no root has its own folders
/// as tiles, read from the disk: each with frames directly in it;
/// not one with frames only further down (Open folder would find
/// nothing to open), nor one empty, hidden or a link. Move rejects
/// there adds the rejects folder's tile; with every frame moved out
/// the rejects folder is still the folder's to delete; a tile
/// clicked opens its folder as Open folder does; a folder gone
/// shows no tiles.
#[test]
fn a_folder_under_no_root_has_its_own_folders_as_tiles() {
    let dir = scratch("loose");
    let (app, state, worker) = archive(&dir, None);
    let loose = dir.join("elsewhere");
    let shot = frames(&loose, &["a.tif", "b.tif"]);
    frames(&loose.join("Birch"), &["x.tif"]);
    frames(&loose.join("nested").join("deeper"), &["q.tif"]);
    std::fs::create_dir_all(loose.join("empty")).unwrap();
    frames(&loose.join(".hidden"), &["h.tif"]);
    #[cfg(unix)]
    std::os::unix::fs::symlink(loose.join("Birch"), loose.join("link")).unwrap();
    let mut reject = greycard_edit::Sidecar::default();
    reject.meta.flag = greycard_edit::meta::Flag::Reject;
    reject.save(&shot[0]).unwrap();

    crate::panel::browser::open_folder(&state, &app, &worker, &loose);
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["a.tif", "b.tif"]);
    assert!(!app.get_folder_tree_shown(), "still no tree");
    assert_eq!(tiles(&app), [("Birch".into(), 1)]);

    app.invoke_rejects_answered(true);
    assert!(
        loose.join("rejects").join("a.tif").exists(),
        "{}",
        app.get_status()
    );
    land_sent(&state, &app, &worker);
    assert_eq!(tiles(&app), [("Birch".into(), 1), ("rejects".into(), 1)]);

    // The last frame out too: the list empty, the rejects folder
    // still the open folder's.
    reject.save(&shot[1]).unwrap();
    {
        let mut st = state.borrow_mut();
        st.sidecars[0].meta.flag = greycard_edit::meta::Flag::Reject;
    }
    app.invoke_rejects_answered(true);
    land_sent(&state, &app, &worker);
    assert!(listed(&state).is_empty(), "{}", app.get_status());
    assert_eq!(tiles(&app), [("Birch".into(), 1), ("rejects".into(), 2)]);
    {
        let mut st = state.borrow_mut();
        st.window.deletes_allowed = true;
        st.sheets
            .trash_refused
            .insert(dunce::canonicalize(loose.join("rejects")).unwrap());
    }
    app.invoke_delete_asked("rejects".into());
    assert_eq!(
        app.get_delete_title(),
        "Delete the rejects folder's 2 frames?",
        "{}",
        app.get_status()
    );
    app.invoke_delete_answered(0);

    app.invoke_grid_tile_picked(1);
    land_sent(&state, &app, &worker);
    assert_eq!(state.borrow().views.view, View::Folder);
    assert_eq!(listed(&state), ["a.tif", "b.tif"]);
    assert!(tiles(&app).is_empty(), "the rejects folder has none");

    // The folder gone from under its view: read again, no tiles.
    crate::panel::browser::open_folder(&state, &app, &worker, &loose.join("Birch"));
    land_sent(&state, &app, &worker);
    assert_eq!(listed(&state), ["x.tif"]);
    std::fs::create_dir_all(loose.join("Birch").join("sub")).unwrap();
    write_frame(&loose.join("Birch").join("sub").join("s.tif"), &R5, 7);
    crate::tree::loose_changed(&mut state.borrow_mut(), &app);
    land_sent(&state, &app, &worker);
    assert_eq!(tiles(&app), [("sub".into(), 1)]);
    greycard_core::testing::rename_retry(loose.join("Birch"), dir.join("birch-away"));
    crate::tree::loose_changed(&mut state.borrow_mut(), &app);
    land_sent(&state, &app, &worker);
    assert!(tiles(&app).is_empty());

    // A root's view again: no tiles of the loose folder's.
    crate::roots::open_view(
        &state,
        &app,
        &worker,
        View::Roots(Some(dir.join("archive"))),
    );
    land_sent(&state, &app, &worker);
    assert_eq!(tiles(&app), [("day".into(), 3)]);
    drop(worker);
    done(&dir, state);
}

/// The tree's list in the window: its top and its foot.
fn list_view(app: &App) -> (f32, f32) {
    use i_slint_backend_testing::ElementHandle;
    crate::testing::settle(app);
    let pane = ElementHandle::find_by_element_id(app, "FoldersSection::folder-pane")
        .next()
        .expect("the tree's list");
    let (at, size) = crate::testing::bounds_of(&pane);
    (at.y, at.y + size.height)
}

/// The row for `folder`, its top left and size in the window; none
/// when the list's scroll has it wholly out of sight, where the
/// search does not look.
fn row_bounds(app: &App, folder: &Path) -> Option<crate::testing::Bounds> {
    crate::testing::keyed(app, FOLDER_LINE, key(folder)).map(|row| crate::testing::bounds_of(&row))
}

/// The top and foot in the window of the row for `folder`.
fn row_at(app: &App, folder: &Path) -> Option<(f32, f32)> {
    row_bounds(app, folder).map(|(at, size)| (at.y, at.y + size.height))
}

fn wholly_in_view(app: &App, folder: &Path) -> bool {
    let (top, foot) = list_view(app);
    row_at(app, folder).is_some_and(|(y, end)| y >= top - 0.5 && end <= foot + 0.5)
}

/// A folder opened other than by its own row (Recently opened, back
/// and forward) has its row brought into the list's view by the
/// least scroll; one in view already moves nothing, and nor does a
/// click on a row the list's foot cuts.
#[test]
fn the_open_folders_row_is_brought_into_the_trees_view() {
    let dir = scratch("in-view");
    let root = dir.join("archive");
    let names: Vec<String> = (0..60).map(|i| format!("f{i:02}")).collect();
    for n in &names {
        frames(&root.join(n), &["x.tif"]);
    }
    let (app, state, worker) = open_root(&dir, &root, Some((1200.0, 700.0)));
    let folder = |i: usize| root.join(&names[i]);
    let marked = |app: &App, name: &str| rows(app).iter().any(|r| r.0 == name && r.3);
    assert!(
        !wholly_in_view(&app, &folder(55)),
        "the list has nothing to prove: {:?} holds f55 at {:?}",
        list_view(&app),
        row_at(&app, &folder(55))
    );

    // Recently opened.
    app.invoke_recent_chosen(folder(55).to_string_lossy().as_ref().into());
    land_sent(&state, &app, &worker);
    assert!(marked(&app, "f55"), "{:?}", rows(&app));
    assert!(
        wholly_in_view(&app, &folder(55)),
        "f55 at {:?} in {:?}",
        row_at(&app, &folder(55)),
        list_view(&app)
    );
    // By the least scroll, a row to spare: f56 in view under it,
    // and no further.
    assert!(wholly_in_view(&app, &folder(56)), "a row to spare");
    assert!(!wholly_in_view(&app, &folder(57)), "the least scroll");

    // One in view already: nothing moves.
    let was = row_at(&app, &folder(55));
    app.invoke_recent_chosen(folder(53).to_string_lossy().as_ref().into());
    land_sent(&state, &app, &worker);
    assert!(marked(&app, "f53"), "{:?}", rows(&app));
    assert_eq!(row_at(&app, &folder(55)), was, "f53 was in view");

    // Back: the folder before, f55, in view still; again: the
    // root's view, its row at the top brought in.
    app.invoke_visit_folder(true);
    land_sent(&state, &app, &worker);
    assert!(marked(&app, "f55"), "{:?}", rows(&app));
    assert_eq!(row_at(&app, &folder(55)), was, "f55 was in view");
    app.invoke_visit_folder(true);
    land_sent(&state, &app, &worker);
    assert!(rows(&app)[0].3, "the root marked: {:?}", rows(&app));
    assert!(wholly_in_view(&app, &root));
    assert!(!wholly_in_view(&app, &folder(55)));
    // Forward: f55 again, brought in.
    app.invoke_visit_folder(false);
    land_sent(&state, &app, &worker);
    assert!(marked(&app, "f55"), "{:?}", rows(&app));
    assert!(wholly_in_view(&app, &folder(55)));

    // A click on a row cut by the list's foot opens it and leaves
    // the list where it is.
    let (_, foot) = list_view(&app);
    let cut = (0..names.len())
        .find(|&i| {
            row_at(&app, &folder(i)).is_some_and(|(y, end)| y < foot - 4.0 && end > foot + 0.5)
        })
        .or_else(|| {
            (0..names.len())
                .rev()
                .find(|&i| wholly_in_view(&app, &folder(i)))
        })
        .unwrap();
    let (at, size) = row_bounds(&app, &folder(cut)).unwrap();
    let was = row_at(&app, &folder(cut));
    click(
        &app,
        at.x + size.width / 2.0,
        (at.y + foot.min(at.y + size.height)) / 2.0,
    );
    land_sent(&state, &app, &worker);
    assert!(marked(&app, &names[cut]), "{:?}", rows(&app));
    assert_eq!(row_at(&app, &folder(cut)), was, "the click scrolled");
    drop(worker);
    done(&dir, state);
}

/// The line of the tree's list for `folder`, by its id; none while it
/// is out of the list's sight.
fn line(app: &App, folder: &Path) -> Option<i_slint_backend_testing::ElementHandle> {
    let path = folder.to_string_lossy().into_owned();
    i_slint_backend_testing::ElementHandle::find_by_element_id(app, "FoldersSection::folder-line")
        .find(|e| e.accessible_description().as_deref() == Some(path.as_str()))
}

/// `name` added under `root` with a frame, indexed, and the pass over
/// it said and landed, as the watcher's pass and the indexer's word of
/// it would.
fn found(dir: &Path, root: &Path, name: &str, state: &Rc<Shared>, app: &App, worker: &Rc<Worker>) {
    frames(&root.join(name), &["n.tif"]);
    let report = greycard_library::Library::open(&dir.join("index").join("library.sqlite"))
        .unwrap()
        .index_tree(root, &mut |_| {})
        .unwrap();
    crate::roots::background_done(state, app, &root.join(name), &report, false, false);
    land_sent(state, app, worker);
}

/// A folder found above the open one while the list is scrolled moves
/// the open folder's row down one in the list, and the list with it,
/// so it stays where it was; its mark moves with it at once, with no
/// fade, the move being no change of the user's: the marks are held
/// still across it, and fade again once it is drawn. A pass that only
/// moves counts, or a folder opened by the user, holds nothing. (The
/// testing backend has no reading of a line's fill, so the hold is
/// what is checked, not the fill.)
#[test]
fn a_folder_found_above_moves_no_mark_across_the_rows() {
    let dir = scratch("marks");
    let root = dir.join("archive");
    for i in 0..12 {
        frames(&root.join(format!("s{i:02}")), &["x.tif"]);
    }
    let (app, state, worker) = open_root(&dir, &root, Some((1200.0, 700.0)));
    pick(&app, "s05");
    land_sent(&state, &app, &worker);
    let (open, above) = (root.join("s05"), root.join("s04"));
    crate::testing::settle(&app);
    let pane = i_slint_backend_testing::ElementHandle::find_by_element_id(
        &app,
        "FoldersSection::folder-pane",
    )
    .next()
    .expect("the tree's list");
    let at = pane.absolute_position();
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(at.x + 40.0, at.y + 10.0),
            delta_x: 0.0,
            delta_y: -21.0,
        });
    crate::testing::settle(&app);
    assert!(line(&app, &above).is_some(), "s04 in sight");
    // Where a row is in the list's view: the pane above it may move as
    // a frame's picture comes up in the Navigator.
    let in_list = |folder: &Path| {
        line(&app, folder).expect("in sight").absolute_position().y - pane.absolute_position().y
    };
    let was = in_list(&open);
    assert!(!app.get_folder_tree_still());

    // Counts only: nothing held.
    frames(&open, &["more.tif"]);
    let report = greycard_library::Library::open(&dir.join("index").join("library.sqlite"))
        .unwrap()
        .index_tree(&root, &mut |_| {})
        .unwrap();
    crate::roots::background_done(&state, &app, &root, &report, false, false);
    land_sent(&state, &app, &worker);
    assert!(!app.get_folder_tree_still(), "a count moved");

    found(&dir, &root, "a01", &state, &app, &worker);
    assert!(
        app.get_folder_tree_still(),
        "the marks held across the move"
    );
    assert!(rows(&app)[7].3, "s05 marked, a row down: {:?}", rows(&app));
    crate::testing::settle(&app);
    assert_eq!(in_list(&open), was, "s05 stays put in the list");
    // The marks fade again once the move is drawn.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(300));
    slint::platform::update_timers_and_animations();
    assert!(!app.get_folder_tree_still());

    // A folder the user opens: its mark fades in as ever.
    pick(&app, "s06");
    land_sent(&state, &app, &worker);
    assert!(rows(&app)[8].3, "s06 marked: {:?}", rows(&app));
    assert!(!app.get_folder_tree_still(), "a folder opened");
    drop(worker);
    done(&dir, state);
}

/// A folder's whole name up at the pointer stays up while a pass adds
/// a folder at the list's end, and goes when one comes in above it,
/// which puts another folder under the pointer.
#[test]
fn a_folder_added_at_the_end_leaves_a_names_tip_up() {
    let dir = scratch("tip");
    let root = dir.join("archive");
    let long = "b folder named at length for the shoot it holds, day two";
    frames(&root.join(long), &["x.tif"]);
    frames(&root.join("c"), &["x.tif"]);
    let (app, state, worker) = open_root(&dir, &root, Some((1200.0, 700.0)));
    // A folder's own frames open, which a pass over a new folder beside
    // it leaves as they are: a new list takes every tip down.
    pick(&app, "c");
    land_sent(&state, &app, &worker);
    crate::testing::settle(&app);
    let row = line(&app, &root.join(long)).expect("the long name's line");
    let (at, size) = crate::testing::bounds_of(&row);
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(
                at.x + size.width / 2.0,
                at.y + size.height / 2.0,
            ),
        });
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(800));
    slint::platform::update_timers_and_animations();
    assert_eq!(app.get_tip_text(), long, "the whole name after a rest");

    found(&dir, &root, "d", &state, &app, &worker);
    slint::platform::update_timers_and_animations();
    assert_eq!(rows(&app).last().map(|r| r.0.clone()).as_deref(), Some("d"));
    assert_eq!(app.get_tip_text(), long, "a folder at the end");

    found(&dir, &root, "a", &state, &app, &worker);
    slint::platform::update_timers_and_animations();
    assert_eq!(app.get_tip_text(), "", "a folder above it");
    drop(worker);
    done(&dir, state);
}
