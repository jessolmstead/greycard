use super::*;
use crate::testing::{state_for, window};
use greycard_library::fixture::{A7, R5, R6, write_frame};
use std::time::Duration;

thread_local! {
    /// The reads sent off the window's thread, waiting for the
    /// test to run and land them.
    pub(super) static SENT: RefCell<Vec<Look>> = const { RefCell::new(Vec::new()) };
    /// The watchers asked for, waiting for the test to build and
    /// take them.
    pub(super) static BUILDS: RefCell<Vec<Plan>> = const { RefCell::new(Vec::new()) };
    /// What the test says each root is on: a network mount's type, or
    /// none for a local disk. A root it says nothing of is a panic.
    pub(super) static REMOTE: RefCell<HashMap<PathBuf, Option<String>>> = RefCell::new(HashMap::new());
    /// The network mounts the test says are below a root, with their
    /// types; none for a root it says nothing of.
    pub(super) static REMOTE_UNDER: RefCell<HashMap<PathBuf, Below>> = RefCell::new(HashMap::new());
    /// What an automount below a root mounts once looked into: a
    /// network type, none for still only an automount, or no entry
    /// at all (`None` in the map) for a local disk.
    pub(super) static AFTER_LOOK: RefCell<HashMap<PathBuf, Option<Option<String>>>> = RefCell::new(HashMap::new());
    /// The paths looked at by `answer_of`, in order.
    pub(super) static LOOKED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    /// Paths the test says do not answer a look, as a share gone
    /// away would not: each is taken for no answer at once, on this
    /// thread, with no look.
    pub(crate) static NOT_ANSWERING: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    /// The folders' looks for an add or a remove are queued, not
    /// run in place, while this is set.
    pub(super) static HOLD_CHECKS: RefCell<bool> = const { RefCell::new(false) };
    pub(super) static CHECKS_SENT: RefCell<Vec<(PathBuf, Check)>> = const { RefCell::new(Vec::new()) };
}

/// The looks queued while held, run and landed, as the thread and
/// the event loop would; how many.
#[cfg(unix)]
fn run_checks(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) -> usize {
    let queued: Vec<(PathBuf, Check)> = CHECKS_SENT.with(|c| c.borrow_mut().drain(..).collect());
    for (dir, check) in &queued {
        let checked = checked_within(*check, dir);
        check.land(state, app, worker, dir, checked);
    }
    queued.len()
}

/// The mounts below a root, each with its network type, or none for
/// an automount not mounted yet.
pub(super) type Below = Vec<(PathBuf, Option<String>)>;

/// Whether the test said `path`, or a folder above it, does not
/// answer.
pub(super) fn not_answering(path: &Path) -> bool {
    NOT_ANSWERING.with(|n| n.borrow().iter().any(|p| path.starts_with(p)))
}

/// The reads sent, how many.
fn sent() -> usize {
    SENT.with(|s| s.borrow().len())
}

/// Every read sent run and landed, and those their landing sent;
/// how many.
fn land_all(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) -> usize {
    land_sent(state, app, worker)
}

/// The sidecars a read would bring for the files of `next` the
/// window does not list yet.
fn read_new(st: &State, next: &[PathBuf]) -> HashMap<PathBuf, Held> {
    let fresh: Vec<PathBuf> = next
        .iter()
        .filter(|p| !st.files.contains(p))
        .cloned()
        .collect();
    let none = vec![None; fresh.len()];
    let held = rows::bring(&fresh, &none, st.write_sidecars, None);
    fresh.into_iter().zip(held).collect()
}

/// The frame on screen as a read would find it, with the index at
/// `db` to follow it by.
fn seen_now(st: &State, db: Option<&Path>) -> OnScreen {
    st.current
        .and_then(|c| {
            let path = st.path_at(c)?.clone();
            let id = st.index_ids.get(st.file_at(c)?).copied().flatten();
            let lib = db.and_then(|db| greycard_library::Library::open_read_only(db).ok());
            Some(OnScreen::look(path, id, lib.as_ref()))
        })
        .unwrap_or_default()
}

/// `merge` as a landed read would call it: the new files' sidecars
/// and the frame on screen read first.
fn merge_read(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Worker,
    next: Vec<PathBuf>,
    db: Option<&Path>,
) -> Option<usize> {
    let (read, seen) = {
        let st = state.borrow();
        (read_new(&st, &next), seen_now(&st, db))
    };
    let brought = Brought {
        read,
        seen,
        rows: None,
    };
    merge(&mut state.borrow_mut(), app, worker, next, brought).0
}

/// A folder of this test's own, canonical, as the index keys
/// folders (macOS's temporary directory is behind a link).
fn scratch(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "greycard-ui-roots-{what}-{}-{}",
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

/// A list merged keeps what each frame that stays already has, by
/// its path: the sidecar as edited in memory, the picture made,
/// its place in the selection. A frame new to the list has its
/// sidecar read from disk; one gone leaves the selection.
#[test]
fn a_list_merged_keeps_each_frames_own_by_its_path() {
    let dir = scratch("merge");
    let files = frames(&dir, &["a.tif", "b.tif", "c.tif"]);
    let app = window(3);
    let (state, worker) = state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.write_sidecars = true;
        st.sidecars[1].meta.rating = 4;
        st.thumb_base[2] = Some((1, 1, vec![9, 9, 9]));
        st.thumb_made[2] = 128;
        st.current = Some(1);
        st.picked = vec![1, 2];
        rebuild_browser(&mut st, &app);
    }
    let aa = frames(&dir, &["aa.tif"]).remove(0);
    let mut s = Sidecar::default();
    s.meta.rating = 2;
    s.save(&aa).unwrap();
    let next = vec![
        files[0].clone(),
        aa.clone(),
        files[1].clone(),
        files[2].clone(),
    ];
    let row = merge_read(&state, &app, &worker, next.clone(), None);
    {
        let st = state.borrow();
        assert_eq!(st.files, next);
        assert_eq!(st.sidecars[2].meta.rating, 4, "b's own, as edited");
        assert_eq!(st.sidecars[1].meta.rating, 2, "aa's, from its sidecar");
        assert_eq!(st.thumb_made[3], 128);
        assert!(st.thumb_base[3].is_some());
        assert_eq!(st.current, Some(2));
        assert_eq!(st.picked, vec![2, 3]);
        assert_eq!(row, None, "the frame on screen stays");
        assert_eq!(app.get_thumbs().row_count(), 4);
        assert_eq!(app.get_selected(), 2);
    }
    // c goes from the list: out of the selection too.
    let next = vec![files[0].clone(), aa.clone(), files[1].clone()];
    assert_eq!(merge_read(&state, &app, &worker, next, None), None);
    assert_eq!(state.borrow().picked, vec![2]);
    // The same list again is nothing to do.
    let same = state.borrow().files.clone();
    assert_eq!(merge_read(&state, &app, &worker, same, None), None);
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// The frame on screen deleted from under the window: the nearest
/// row is what is opened next, and nothing is kept as current, so
/// no sidecar is written for a file that is gone.
#[test]
fn a_frame_on_screen_that_is_gone_hands_on_to_the_nearest() {
    let dir = scratch("gone");
    let files = frames(&dir, &["a.tif", "b.tif", "c.tif"]);
    let app = window(3);
    let (state, worker) = state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.current = Some(2);
        rebuild_browser(&mut st, &app);
    }
    std::fs::remove_file(&files[2]).unwrap();
    let next = files[..2].to_vec();
    let row = merge_read(&state, &app, &worker, next, None);
    assert_eq!(row, Some(1), "c's row, clamped to the last");
    assert_eq!(state.borrow().current, None);
    assert!(!files[2].with_extension("tif.gcd").exists());
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// A frame on screen renamed, or moved to another folder, is
/// followed there by its row in the index: its path in the window
/// is the new one, its edit in memory is kept, and in a folder's
/// view it stays in the list though the folder no longer has it.
#[test]
fn a_frame_on_screen_that_moved_is_followed_to_its_new_path() {
    let dir = scratch("follow");
    let (shoot, other) = (dir.join("shoot"), dir.join("other"));
    std::fs::create_dir_all(&shoot).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let files = frames(&shoot, &["a.tif", "b.tif", "c.tif"]);
    frames(&other, &["o.tif"]);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(3);
    let (state, worker) = state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        crate::library::refresh_ids(&mut st);
        st.sidecars[1].meta.rating = 5;
        st.current = Some(1);
        rebuild_browser(&mut st, &app);
    }
    // Renamed within the folder.
    let renamed = shoot.join("bb.tif");
    std::fs::rename(&files[1], &renamed).unwrap();
    writer.index_folder(&shoot, &mut |_| {}).unwrap();
    let listed = crate::files::list_files(&shoot).unwrap();
    assert_eq!(merge_read(&state, &app, &worker, listed, Some(&db)), None);
    {
        let st = state.borrow();
        let c = st.current.expect("still on screen");
        assert_eq!(st.files[c], renamed);
        assert_eq!(st.sidecars[c].meta.rating, 5);
    }
    // Moved to the other folder: followed, and kept in the list.
    let moved = other.join("bb.tif");
    std::fs::rename(&renamed, &moved).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let listed = crate::files::list_files(&shoot).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(merge_read(&state, &app, &worker, listed, Some(&db)), None);
    {
        let st = state.borrow();
        let c = st.current.expect("still on screen");
        assert_eq!(st.files[c], moved);
        assert_eq!(st.files.len(), 3);
        assert_eq!(st.sidecars[c].meta.rating, 5);
    }
    // The window's thread keeps the state, and with it the
    // reader: closed by hand, since Windows will not remove a
    // folder with a database open in it.
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// The pane's roots, with the grid up: a row a root with its count
/// from the index, All roots with their sum, the one open lit, and
/// "Add this folder" while the folder open is under none of them.
#[test]
fn the_pane_lists_the_roots_with_their_counts() {
    let dir = scratch("header");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(a.join("day")).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    frames(&a, &["x.tif"]);
    frames(&a.join("day"), &["y.tif", "z.tif"]);
    let open = frames(&b, &["o.tif"]);
    let db = dir.join("index").join("library.sqlite");
    greycard_library::Library::open(&db)
        .unwrap()
        .index_tree(&dir, &mut |_| {})
        .unwrap();
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1200.0, 700.0));
    app.set_grid_open(true);
    let (state, _worker) = state_for(&app, open);
    let mut st = state.borrow_mut();
    st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
    st.library.roots.add(&a).unwrap();
    recount(&mut st);
    show(&st, &app);
    let chips = app.get_library_roots();
    assert_eq!(chips.row_count(), 1);
    let chip = chips.row_data(0).unwrap();
    assert_eq!((chip.name.as_str(), chip.count, chip.on), ("a", 3, false));
    assert_eq!(app.get_library_all_count(), 3);
    assert!(!app.get_library_all_on());
    assert!(app.get_library_can_add_open(), "b is under no root");
    drop(st);
    let (count, lit, _) = root_row(&app, "a");
    assert_eq!((count.as_str(), lit), ("3", false));
    assert_eq!(root_row(&app, "All roots").0, "3");
    assert_eq!(buttons_named(&app, "+ Add this folder"), 1);
    let mut st = state.borrow_mut();
    st.library.roots.add(&b).unwrap();
    st.view = View::Roots(None);
    recount(&mut st);
    show(&st, &app);
    assert_eq!(app.get_library_all_count(), 4);
    assert!(app.get_library_all_on());
    assert!(!app.get_library_can_add_open());
    assert_eq!(app.get_library_note(), "");
    drop(st);
    let (count, lit, _) = root_row(&app, "All roots");
    assert_eq!((count.as_str(), lit), ("4", true));
    assert_eq!(root_row(&app, "b").0, "1");
    assert!(!root_row(&app, "b").1);
    assert_eq!(
        buttons_named(&app, "+ Add this folder"),
        0,
        "b is a root now"
    );
    // One root's view: its row lit, All roots not.
    let mut st = state.borrow_mut();
    st.view = View::Roots(Some(a.clone()));
    show(&st, &app);
    drop(st);
    assert!(root_row(&app, "a").1);
    assert!(!root_row(&app, "All roots").1);
    let mut st = state.borrow_mut();
    // A name given is the chip's, and the path is still the root.
    st.library.roots.set_name(&a, "Archive");
    assert_eq!(root_name(&st.library.roots, &a), "Archive");
    assert_eq!(root_name(&st.library.roots, &b), "b");
    show(&st, &app);
    let chip = app.get_library_roots().row_data(0).unwrap();
    assert_eq!(chip.name.as_str(), "Archive");
    assert_eq!(chip.path.as_str(), a.to_str().unwrap());
    st.index_reader = None;
    drop(st);
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// A folder opened over a view of the roots takes the chip's
/// light with it: the view is the folder's, and no root is on.
/// The first cut set the view and left the chip lit.
#[test]
fn a_folder_opened_over_a_root_view_turns_its_chip_off() {
    let dir = scratch("chip-off");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    frames(&a, &["x.tif"]);
    frames(&b, &["o.tif"]);
    let app = window(1);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.library.roots.add(&a).unwrap();
        st.view = View::Roots(Some(a.clone()));
        show(&st, &app);
        assert!(app.get_library_roots().row_data(0).unwrap().on);
    }
    crate::panel::browser::open_folder(&state, &app, &worker, &b);
    // The chip stays lit until the folder lands, since the list on
    // screen is still the view's.
    assert_eq!(state.borrow().view, View::Roots(Some(a.clone())));
    assert_eq!(land_all(&state, &app, &worker), 1);
    assert_eq!(state.borrow().view, View::Folder);
    assert!(
        !app.get_library_roots().row_data(0).unwrap().on,
        "the root's chip stayed lit after a folder opened"
    );
    assert!(!app.get_library_all_on());
    crate::testing::remove_dir_retry(&dir);
}

fn right_click(app: &App, x: f32, y: f32) {
    use slint::platform::{PointerEventButton, WindowEvent};
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
}

/// The middle of the button a screen reader knows as `label`: the
/// one highest up, so a root's row comes before the tree's row of
/// the same name.
fn middle_of(app: &App, label: &str) -> (f32, f32) {
    let (at, size) = crate::testing::buttons(app, label)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no button {label:?}"));
    (at.x + size.width / 2.0, at.y + size.height / 2.0)
}

/// A point near the left end of that button: clear of a menu opened
/// from a row's right half, which drops down and to the right of
/// the pointer over the rows below it.
fn left_of(app: &App, label: &str) -> (f32, f32) {
    let (at, size) = crate::testing::buttons(app, label)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no button {label:?}"));
    (at.x + 4.0, at.y + size.height / 2.0)
}

/// How many buttons a screen reader knows as `label`.
fn buttons_named(app: &App, label: &str) -> usize {
    crate::testing::buttons(app, label).len()
}

/// A root's row in the pane, by its name: its count as a screen
/// reader has it, whether it is lit, and its width.
fn root_row(app: &App, name: &str) -> (String, bool, f32) {
    use i_slint_backend_testing::ElementHandle;
    crate::testing::count_labeled(app, name);
    let row = ElementHandle::find_by_accessible_label(app, name)
        .find(|e| e.accessible_item_selectable() == Some(true))
        .unwrap_or_else(|| panic!("no root's row {name:?}"));
    (
        row.accessible_value().unwrap_or_default().to_string(),
        row.accessible_item_selected().unwrap_or(false),
        row.size().width,
    )
}

/// A window with the grid up over two roots of the test's own,
/// every press of the pane's roots recorded and reaching nothing
/// else.
fn roots_in_the_pane(names: &[&str]) -> (App, Rc<RefCell<Vec<String>>>) {
    let app = window(0);
    app.window()
        .set_size(slint::LogicalSize::new(1200.0, 700.0));
    app.set_grid_open(true);
    let chips: Vec<RootChip> = names
        .iter()
        .map(|n| RootChip {
            name: (*n).into(),
            path: format!("/x/{n}").into(),
            count: 3,
            on: false,
            offline: false,
            archive: false,
        })
        .collect();
    app.set_library_all_count(3 * names.len() as i32);
    app.set_library_roots(ModelRc::new(VecModel::from(chips)));
    app.set_library_can_add_open(true);
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    let s = seen.clone();
    app.on_library_root_picked(move |p| s.borrow_mut().push(format!("picked {p}")));
    let s = seen.clone();
    app.on_library_root_removed(move |p| s.borrow_mut().push(format!("removed {p}")));
    let s = seen.clone();
    app.on_library_root_add(move || s.borrow_mut().push("add".into()));
    let s = seen.clone();
    app.on_library_root_add_open(move || s.borrow_mut().push("add open".into()));
    slint::platform::update_timers_and_animations();
    (app, seen)
}

/// The pane's roots are wired, with the grid up: All roots, a root's
/// row, and the two ways to add one each hand back their own press;
/// the header carries none of it. A long name is cut short inside
/// the pane rather than widening it, and every root of six is a
/// row that answers.
#[test]
fn the_panes_roots_hand_back_what_was_pressed() {
    let (app, seen) = roots_in_the_pane(&["shoots"]);
    for label in [
        "All roots",
        "shoots",
        "+ Add a root...",
        "+ Add this folder",
    ] {
        let (x, y) = middle_of(&app, label);
        assert!(x < 240.0, "{label} is in the pane, at {x}");
        crate::testing::click(&app, x, y);
    }
    assert_eq!(
        *seen.borrow(),
        ["picked ", "picked /x/shoots", "add", "add open"]
    );
    // The count at the row's right, and nothing lit.
    assert_eq!(root_row(&app, "shoots").0, "3");
    assert_eq!(root_row(&app, "All roots").0, "3");
    assert!(!root_row(&app, "shoots").1);
    // The header keeps none of it: the pane put away takes every
    // root with it, and the loupe's pane has none.
    assert_eq!(buttons_named(&app, "All roots"), 1);
    crate::testing::press(&app, slint::platform::Key::F7);
    assert!(app.get_left_hidden());
    assert_eq!(crate::testing::count_labeled(&app, "All roots"), 0);
    assert_eq!(crate::testing::count_labeled(&app, "shoots"), 0);
    assert_eq!(crate::testing::count_labeled(&app, "+ Add a root..."), 0);
    crate::testing::press(&app, slint::platform::Key::F7);
    assert_eq!(buttons_named(&app, "All roots"), 1);
    app.set_grid_open(false);
    assert_eq!(crate::testing::count_labeled(&app, "All roots"), 0);
    app.set_grid_open(true);

    // Six roots with long names: each a row inside the pane, and
    // the last one reached.
    seen.borrow_mut().clear();
    let six: Vec<RootChip> = (0..6)
        .map(|i| RootChip {
            name: format!("a long shoot folder name that runs on {i}").into(),
            path: format!("/x/r{i}").into(),
            count: 100,
            on: i == 5,
            offline: i == 0,
            archive: false,
        })
        .collect();
    app.set_library_roots(ModelRc::new(VecModel::from(six)));
    // A root whose drive is out says so in place of its count.
    assert_eq!(
        root_row(&app, "a long shoot folder name that runs on 0").0,
        "offline"
    );
    assert_eq!(
        root_row(&app, "a long shoot folder name that runs on 1").0,
        "100"
    );
    let last = "a long shoot folder name that runs on 5";
    let (_, lit, width) = root_row(&app, last);
    assert!(lit, "the one open is lit");
    assert!(width <= 240.0, "{width}");
    let (x, y) = middle_of(&app, last);
    // Cut short, the whole name is its hover text after a rest;
    // All roots, whole already, has none.
    let rest_on = |x: f32, y: f32| {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(x, y),
            });
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(800));
        slint::platform::update_timers_and_animations();
    };
    rest_on(x, y);
    assert_eq!(app.get_tip_text(), last);
    let all = middle_of(&app, "All roots");
    rest_on(all.0, all.1);
    assert_eq!(app.get_tip_text(), "", "the tip went with the pointer");
    crate::testing::click(&app, x, y);
    assert_eq!(*seen.borrow(), ["picked /x/r5"]);
}

/// A root is named from its row's menu, and the name is only a
/// label: the root is its path throughout. A plain add is one
/// action, with no sheet. A right-click on the row opens the menu
/// and picks nothing, and the press that closes the menu is not a
/// press on the row; the next is.
#[test]
fn a_root_is_named_from_its_menu() {
    let dir = scratch("rename");
    let (photos, b) = (dir.join("Photos"), dir.join("b"));
    std::fs::create_dir_all(&photos).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    frames(&photos, &["x.tif"]);
    let open = frames(&b, &["o.tif"]);
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1200.0, 700.0));
    app.set_grid_open(true);
    let (state, _worker) = state_for(&app, open);
    state.borrow_mut().library.roots.add(&photos).unwrap();
    show(&state.borrow(), &app);
    slint::platform::update_timers_and_animations();

    let (x, y) = middle_of(&app, "Photos");
    right_click(&app, x, y);
    assert!(
        app.get_menu_up(),
        "a right-click on the root's row opens its menu"
    );
    assert_eq!(state.borrow().view, View::Folder, "nothing picked");
    assert!(state.borrow().library.wanted.is_none());
    // The press that closes it, on the row just left of where the
    // menu opened, is let go by.
    let (x, y) = left_of(&app, "Photos");
    crate::testing::click(&app, x, y);
    assert!(!app.get_menu_up());
    assert!(state.borrow().library.wanted.is_none(), "not a pick");
    assert_eq!(
        state.borrow().library.roots.list(),
        std::slice::from_ref(&photos)
    );
    // The next is a press on the row, which picks its root.
    crate::testing::click(&app, x, y);
    assert_eq!(
        state.borrow_mut().library.wanted.take(),
        Some(View::Roots(Some(photos.clone())))
    );

    // Rename..., from the menu.
    app.invoke_library_root_rename(photos.to_string_lossy().into_owned().into());
    assert!(app.get_root_sheet_open());
    assert_eq!(app.get_root_sheet_folder(), "Photos");
    assert_eq!(app.get_root_sheet_name(), "");
    app.set_root_sheet_name("Archive".into());
    app.invoke_root_sheet_done();
    assert!(!app.get_root_sheet_open());
    let chips = app.get_library_roots();
    let chip = chips.row_data(0).unwrap();
    assert_eq!(
        (chip.name.as_str(), chip.path.as_str()),
        ("Archive", photos.to_str().unwrap())
    );
    assert!(photos.is_dir(), "the folder is where it was");
    assert_eq!(buttons_named(&app, "Archive"), 1);

    // Opened again, the sheet has the name to change; Escape,
    // with the field holding the focus, leaves it as it was.
    app.invoke_library_root_rename(photos.to_string_lossy().into_owned().into());
    assert_eq!(app.get_root_sheet_name(), "Archive");
    slint::platform::update_timers_and_animations();
    crate::testing::press(&app, "Q");
    assert_eq!(
        app.get_root_sheet_name(),
        "Q",
        "the name chosen, typed over"
    );
    crate::testing::press(&app, slint::platform::Key::Escape);
    assert!(!app.get_root_sheet_open(), "Escape closed it");
    assert_eq!(state.borrow().library.roots.name(&photos), Some("Archive"));
    // A click beside the sheet closes it too.
    app.invoke_library_root_rename(photos.to_string_lossy().into_owned().into());
    crate::testing::click(&app, 20.0, 680.0);
    assert!(!app.get_root_sheet_open());

    // The folder open, added at a press with no sheet, by its
    // folder's name.
    assert!(app.get_library_can_add_open());
    app.invoke_library_root_add_open();
    assert!(!app.get_root_sheet_open());
    {
        let st = state.borrow();
        assert_eq!(st.library.roots.list(), [photos.clone(), b.clone()]);
        assert_eq!(st.library.roots.name(&b), None);
    }
    assert!(
        app.get_status().contains("right-click"),
        "{}",
        app.get_status()
    );
    let names: Vec<String> = app
        .get_library_roots()
        .iter()
        .map(|c| c.name.to_string())
        .collect();
    assert_eq!(names, ["Archive", "b"]);

    // Cleared, the name goes back to the folder's.
    app.invoke_library_root_rename(photos.to_string_lossy().into_owned().into());
    app.set_root_sheet_name("  ".into());
    app.invoke_root_sheet_done();
    assert_eq!(
        app.get_library_roots().row_data(0).unwrap().name.as_str(),
        "Photos"
    );
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// With a root's menu up, a right-click on another root opens that
/// root's, and the left press that closes it does nothing on
/// whatever row it lands on: not All roots, not the other root,
/// not the ways to add one. The pane's callbacks are the test's
/// own, so no press reaches anything outside it.
#[test]
fn the_press_that_closes_a_roots_menu_is_nothing_more() {
    let (app, seen) = roots_in_the_pane(&["one", "two"]);
    // The menus asked for at a row's right, the closing presses
    // made at the rows' left, clear of the menu dropped over them.
    let right = |label: &str| {
        let (x, y) = middle_of(&app, label);
        (x + 60.0, y)
    };
    let one = right("one");
    let two = right("two");
    // All roots has no menu.
    let all = right("All roots");
    right_click(&app, all.0, all.1);
    assert!(!app.get_menu_up(), "All roots has no menu");
    // A right-click on the second with the first's menu up opens
    // the second's.
    right_click(&app, one.0, one.1);
    assert!(app.get_menu_up());
    right_click(&app, two.0, two.1);
    assert!(app.get_menu_up(), "the second root's menu is up");
    crate::testing::press(&app, slint::platform::Key::Escape);
    assert!(!app.get_menu_up());

    // Every row, closing the first's menu, does nothing there.
    let rows = [
        "All roots",
        "one",
        "two",
        "+ Add a root...",
        "+ Add this folder",
    ];
    for label in rows {
        right_click(&app, one.0, one.1);
        assert!(app.get_menu_up());
        let (x, y) = left_of(&app, label);
        crate::testing::click(&app, x, y);
        assert!(!app.get_menu_up(), "closed on {label}");
    }
    assert!(seen.borrow().is_empty(), "{:?}", seen.borrow());
    // And with no menu up, the same rows answer.
    for label in rows {
        let (x, y) = left_of(&app, label);
        crate::testing::click(&app, x, y);
    }
    assert_eq!(
        *seen.borrow(),
        [
            "picked ",
            "picked /x/one",
            "picked /x/two",
            "add",
            "add open"
        ]
    );
}

/// With the grid up the pane lies over the strip's left end, which
/// the grid leaves drawn under it: a press or a wheel in the pane's
/// empty foot reaches no strip cell. In the loupe the same point is
/// the strip's, and answers.
#[test]
fn the_pane_over_the_grid_keeps_the_strip_under_it_from_the_pointer() {
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::{Key, WindowEvent};
    let app = window(40);
    app.window()
        .set_size(slint::LogicalSize::new(1200.0, 700.0));
    let (_state, _worker) = state_for(&app, crate::testing::folder(40));
    let clicked = Rc::new(RefCell::new(0));
    let c = clicked.clone();
    app.on_frame_clicked(move |_, _, _| *c.borrow_mut() += 1);
    // Where the strip's first cell on screen starts: it moves when
    // the strip scrolls.
    let strip_at = || {
        crate::testing::press(&app, Key::Shift);
        ElementHandle::find_by_element_id(&app, "Filmstrip::cell")
            .map(|e| e.absolute_position().x)
            .fold(f32::INFINITY, f32::min)
    };
    // In the strip's first cell (12 to 190 across, 564 to 688
    // down), and with the grid up in the pane's left margin, clear
    // of its buttons.
    let (x, y) = (14.0, 620.0);
    let press_and_wheel = || {
        crate::testing::click(&app, x, y);
        app.window().dispatch_event(WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(x, y),
            delta_x: 0.0,
            delta_y: -300.0,
        });
    };
    // The loupe: the point is the strip's first cell, and answers.
    let before = strip_at();
    assert!(before.is_finite(), "the strip has cells");
    press_and_wheel();
    assert_eq!(*clicked.borrow(), 1, "the strip's cell in the loupe");
    assert!(strip_at() < before, "the strip scrolled in the loupe");
    app.invoke_scroll_by(-1e9);
    // The grid up: the point is the pane's, and nothing reaches the
    // strip under it.
    app.set_grid_open(true);
    let before = strip_at();
    *clicked.borrow_mut() = 0;
    press_and_wheel();
    assert_eq!(*clicked.borrow(), 0, "a strip cell under the pane");
    assert_eq!(strip_at(), before, "the strip scrolled under the pane");
}

/// With the grid up, the pane's Import asks for the sheet, and
/// Ctrl+O and Ctrl+Shift+I still reach the chooser and the sheet
/// with the header's buttons gone. An import's running line is
/// the pane's, under Import, and the header's in place of the
/// selection only while the pane is put away.
#[test]
fn the_panes_import_and_the_keys_still_reach_their_sheets() {
    use slint::platform::{Key, WindowEvent};
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1200.0, 700.0));
    app.set_grid_open(true);
    let seen = Rc::new(RefCell::new(Vec::<&str>::new()));
    let s = seen.clone();
    app.on_import_asked(move || s.borrow_mut().push("import"));
    let s = seen.clone();
    app.on_open_folder(move || s.borrow_mut().push("open"));
    slint::platform::update_timers_and_animations();
    let (at, size) = crate::testing::labeled(&app, "Import...");
    assert!(at.x < 240.0, "in the pane");
    crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    let chord = |mods: &[Key], key: &str| {
        for k in mods {
            app.window()
                .dispatch_event(WindowEvent::KeyPressed { text: (*k).into() });
        }
        crate::testing::press(&app, key);
        for k in mods.iter().rev() {
            app.window()
                .dispatch_event(WindowEvent::KeyReleased { text: (*k).into() });
        }
    };
    chord(&[Key::Control], "o");
    chord(&[Key::Control, Key::Shift], "I");
    assert_eq!(*seen.borrow(), ["import", "open", "import"]);

    let line = "Copying 3 of 10";
    app.set_import_running(true);
    app.set_import_status(line.into());
    assert_eq!(crate::testing::count_labeled(&app, line), 1, "the pane's");
    assert_eq!(crate::testing::count_labeled(&app, "Stop import"), 1);
    crate::testing::press(&app, Key::F7);
    assert!(app.get_left_hidden());
    assert_eq!(
        crate::testing::count_labeled(&app, line),
        1,
        "the header's, the pane away"
    );
    assert_eq!(crate::testing::count_labeled(&app, "Stop import"), 0);
}

/// The preset sheet's name field holds the focus as the root
/// sheet's does, and Escape closes it all the same.
#[test]
fn escape_closes_the_preset_sheet_from_its_field() {
    let app = window(0);
    app.set_preset_open(true);
    slint::platform::update_timers_and_animations();
    crate::testing::press(&app, "Q");
    assert_eq!(app.get_preset_name(), "Q", "the field has the keys");
    crate::testing::press(&app, slint::platform::Key::Escape);
    assert!(!app.get_preset_open());
}

/// The indexer's background: the launch pass over a root, and a
/// change the watcher saw, each said when done — on a library of
/// the test's own, beside which its roots would be kept.
#[test]
fn the_indexer_passes_over_the_roots_and_the_changes() {
    use crate::library::Told;
    let dir = scratch("indexer");
    let root = dir.join("root");
    std::fs::create_dir_all(root.join("day")).unwrap();
    frames(&root, &["a.tif"]);
    frames(&root.join("day"), &["b.tif", "c.tif"]);
    let db = dir.join("data").join("library.sqlite");
    assert_eq!(
        Roots::path_beside(&db),
        dir.join("data").join(greycard_library::roots::FILE_NAME)
    );
    let (tx, rx) = std::sync::mpsc::channel();
    let indexer = crate::library::Indexer::start(db.clone(), move |told| {
        let _ = tx.send(told);
    })
    .expect("the indexer starts");
    // Each word printed as it comes: the test has failed on the
    // Windows runner with no panic text in the log, and the capture
    // is shown on a failure.
    let wait = |want: &dyn Fn(&Told) -> bool| loop {
        let told = match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(told) => told,
            Err(e) => panic!("the indexer answers: {e}"),
        };
        eprintln!("told: {told:?}");
        if want(&told) {
            return told;
        }
    };
    wait(&|t| matches!(t, Told::Opened(_)));
    eprintln!("root: {}", root.display());
    indexer.roots(vec![root.clone()]);
    match wait(&|t| matches!(t, Told::Background { .. })) {
        Told::Background {
            path,
            launch,
            report,
            error,
            ..
        } => {
            assert_eq!(path, root);
            assert!(launch);
            assert_eq!(error, None);
            assert_eq!(report.added, 3, "{report:?}");
        }
        other => panic!("{other:?}"),
    }
    frames(&root.join("day"), &["d.tif"]);
    indexer
        .asker()
        .changes(vec![greycard_library::Change::Folder(root.join("day"))]);
    match wait(&|t| matches!(t, Told::Background { .. })) {
        Told::Background {
            path,
            launch,
            report,
            ..
        } => {
            assert_eq!(path, root.join("day"));
            assert!(!launch);
            assert_eq!(report.added, 1, "{report:?}");
        }
        other => panic!("{other:?}"),
    }
    let reader = greycard_library::Library::open_read_only(&db).unwrap();
    assert_eq!(reader.count_under(&root).unwrap(), 4);
    drop(reader);
    eprintln!("counted");
    // An asker still held, as the watcher holds one, does not keep
    // the thread from leaving.
    let _held = indexer.asker();
    indexer.stop(Duration::from_secs(20));
    eprintln!("stopped");
    crate::testing::remove_dir_retry(&dir);
}

/// The first pixel of a row's picture on the window.
fn row_pixel(app: &App, row: usize) -> Option<[u8; 3]> {
    let image = app.get_thumbs().row_data(row)?.image;
    let rgba = image.to_rgba8()?;
    let px = rgba.as_bytes();
    (px.len() >= 3).then(|| [px[0], px[1], px[2]])
}

/// B1, the review's repro: eight frames, two flagged reject, the
/// rejects moved out. Every frame left keeps its own picture; the
/// first cut carried the picture at old row r to new file r, and
/// every frame after a reject showed its neighbor's.
#[test]
fn after_the_rejects_move_each_frame_keeps_its_own_picture() {
    let dir = scratch("rejects-pictures");
    let names: Vec<String> = (0..8).map(|i| format!("f{i}.tif")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let files = frames(&dir, &names);
    let app = window(8);
    let (state, worker) = state_for(&app, files.clone());
    // The strip with a cell for every frame, as it has once laid
    // out: only a row with a cell carries a picture.
    crate::cells::show(&app, crate::cells::View::Strip, 0, 8);
    {
        let mut st = state.borrow_mut();
        st.write_sidecars = true;
        for (i, f) in files.iter().enumerate() {
            // A picture of one pixel, its red the frame's number.
            st.thumb_base[i] = Some((1, 1, vec![10 * i as u8 + 5, 0, 0]));
            st.thumb_made[i] = 128;
            if i == 2 || i == 5 {
                st.sidecars[i].meta.flag = meta::Flag::Reject;
                st.sidecars[i].save(f).unwrap();
            }
        }
        rebuild_browser(&mut st, &app);
        for row in 0..8 {
            assert_eq!(row_pixel(&app, row), Some([10 * row as u8 + 5, 0, 0]));
        }
        crate::panel::cull::move_rejects(&mut st, &app, &worker);
        assert_eq!(st.files.len(), 6);
    }
    let st = state.borrow();
    for (row, &f) in st.shown.iter().enumerate() {
        let own = files.iter().position(|p| *p == st.files[f]).unwrap();
        assert_eq!(
            row_pixel(&app, row),
            Some([10 * own as u8 + 5, 0, 0]),
            "row {row}, {}",
            st.files[f].display()
        );
    }
    drop(st);
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// B2, the review's repro: a merge while the thumbnails are still
/// coming (a raw copied in 0.74 s after the view came up). Every
/// frame without a picture is asked for again under its new
/// number; the first cut asked only for the new file, the jobs
/// already queued arrived under numbers that now meant other files
/// and were dropped, and the grid stayed blank.
#[test]
fn a_merge_while_the_pictures_are_coming_asks_for_every_one_still_owed() {
    let dir = scratch("merge-owed");
    let files = frames(&dir, &["a.tif", "b.tif", "c.tif", "d.tif"]);
    let app = window(4);
    let (state, _worker) = state_for(&app, files.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = Worker::new(move |outcome| {
        let _ = tx.send(outcome);
    });
    // b's picture is in; the rest were queued under the old
    // numbers, by the open, and have not come.
    {
        let mut st = state.borrow_mut();
        st.thumb_base[1] = Some((1, 1, vec![1, 2, 3]));
        st.thumb_made[1] = 128;
        rebuild_browser(&mut st, &app);
    }
    let aa = frames(&dir, &["aa.tif"]).remove(0);
    let next = vec![
        files[0].clone(),
        aa.clone(),
        files[1].clone(),
        files[2].clone(),
        files[3].clone(),
    ];
    merge_read(&state, &app, &worker, next.clone(), None);
    let owed: Vec<usize> = vec![0, 1, 3, 4];
    let mut came: Vec<usize> = Vec::new();
    let until = std::time::Instant::now() + Duration::from_secs(30);
    while came.len() < owed.len() && std::time::Instant::now() < until {
        let Ok(outcome) = rx.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        let (index, frame) = match outcome {
            crate::worker::Outcome::Thumbnail { index, frame, .. }
            | crate::worker::Outcome::NoThumbnail { index, frame } => (index, frame),
            _ => continue,
        };
        // What the window would take: a picture for the file
        // that is at that number now.
        if next.get(index) == Some(&frame.path) && !came.contains(&index) {
            came.push(index);
        }
    }
    came.sort();
    assert_eq!(came, owed, "each frame still owed a picture gets one");
    worker.stop();
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// B3, the review's repro: the frame on screen copied to `backup/`
/// and the original deleted. The frame is gone, not moved: the
/// window does not take the copy's path for it (which would have
/// saved its edit over the copy's sidecar), the copy is not in the
/// list twice, and the nearest row is what opens next.
#[test]
fn a_frame_deleted_after_a_backup_is_gone_not_followed_to_the_copy() {
    let dir = scratch("backup");
    let (shoot, backup) = (dir.join("shoot"), dir.join("backup"));
    std::fs::create_dir_all(&shoot).unwrap();
    std::fs::create_dir_all(&backup).unwrap();
    let files = frames(&shoot, &["a.tif", "b.tif", "c.tif"]);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(3);
    let (state, worker) = state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        crate::library::refresh_ids(&mut st);
        st.sidecars[1].meta.rating = 5;
        st.current = Some(1);
        rebuild_browser(&mut st, &app);
    }
    std::fs::copy(&files[1], backup.join("b.tif")).unwrap();
    writer.index_folder(&backup, &mut |_| {}).unwrap();
    std::fs::remove_file(&files[1]).unwrap();
    writer.index_folder(&shoot, &mut |_| {}).unwrap();
    // The all-roots view's list: the copy is under the root too.
    let listed = writer.paths_under(std::slice::from_ref(&dir)).unwrap();
    assert_eq!(listed.len(), 3);
    let row = merge_read(&state, &app, &worker, listed.clone(), Some(&db));
    {
        let st = state.borrow();
        assert_eq!(st.current, None, "the frame is gone, not followed");
        assert_eq!(st.files, listed, "each path once");
        assert!(row.is_some(), "the nearest row opens next");
        let copy = st.files.iter().position(|p| *p == backup.join("b.tif"));
        assert_eq!(st.sidecars[copy.unwrap()].meta.rating, 0, "the copy's own");
    }
    assert!(!backup.join("b.tif.gcd").exists());
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// B4, the review's repro: a filter kept from the last session
/// (picks only) over a capture of a folder, or a file named on the
/// command line or double-clicked from the desktop. None puts it
/// back; a session opened on a folder does. (An export from the
/// command line has no browser to filter: `headless`.)
#[test]
fn a_kept_filter_is_not_put_back_over_a_file_or_a_batch_run() {
    use crate::panel::startup::restores_filter;
    let dir = scratch("kept-filter");
    let files = frames(&dir, &["x.tif"]);
    let x = files[0].to_string_lossy().into_owned();
    let d = dir.to_string_lossy().into_owned();
    let cli = |args: &[&str]| {
        let mut all = vec!["greycard-ui"];
        all.extend_from_slice(args);
        Cli::parse_from(all)
    };
    assert!(!restores_filter(&cli(&[&d, "--screenshot", "s.png"])));
    assert!(!restores_filter(&cli(&[&x])), "a file named");
    assert!(!restores_filter(&cli(&[&d, "--snapshot", "s.png"])));
    assert!(restores_filter(&cli(&[&d])), "a session on a folder");
    assert!(restores_filter(&cli(&[])), "a session on the last folder");

    // The desktop's open of a file mid-session: the filter goes
    // rather than hide the file asked for.
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.filter = filter::Filter::from_name("Picks").unwrap();
        st.library.loading = true;
    }
    crate::panel::browser::open_paths(&state, &app, &worker, &files);
    assert!(state.borrow().filter.is_empty(), "cleared at once");
    assert_eq!(land_all(&state, &app, &worker), 1);
    let st = state.borrow();
    assert!(st.filter.is_empty());
    assert_eq!(st.shown, [0]);
    assert!(!st.library.loading, "the folder's own load is done");
    drop(st);
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// A frame under an offline root is culled from its local preview,
/// found by the key its row gives, and nothing of its file is read:
/// the loupe asks for the preview alone, the preview comes from the
/// cache, and it is marked as the local preview. A frame of the
/// same root with no preview kept says so and is not asked for; a
/// frame under a root on a network mount asks for the preview and
/// then its file; one on a local disk, its file.
#[test]
fn an_offline_frame_is_culled_from_its_local_preview_and_its_file_is_not_read() {
    use crate::previews::{self, Previews};
    let dir = scratch("offline-preview");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let x = frames(&a, &["x.tif"]).remove(0);
    let zs = frames(&b, &["z.tif", "zz.tif"]);
    let (z, zz) = (zs[0].clone(), zs[1].clone());
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.write_sidecars = true;
        st.library.roots.add(&a).unwrap();
        st.library.roots.add(&b).unwrap();
        recount(&mut st);
    }
    let away = dir.join("b-away");
    crate::testing::rename_away(&b, &away);
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    let (zi, zzi) = {
        let st = state.borrow();
        let at = |p: &PathBuf| st.files.iter().position(|f| f == p).unwrap();
        (at(&z), at(&zz))
    };
    // z's preview, as the pool made it while the root was there,
    // under the key its row gives.
    let (hash, stamp) = state.borrow().from_row[zi]
        .key
        .clone()
        .expect("the row's key");
    let cache: crate::worker::ThumbCache = std::sync::Arc::new(std::sync::Mutex::new(Some(
        greycard_library::Thumbs::at(dir.join("thumbs"), greycard_library::thumbs::DEFAULT_CAP)
            .with_previews(
                crate::previews::SIZE,
                greycard_library::thumbs::DEFAULT_PREVIEW_CAP,
            ),
    )));
    let kept = greycard_library::thumbs::Thumb {
        width: 300,
        height: 200,
        rgb: vec![120; 300 * 200 * 3],
    };
    let cache_put = |hash: &str, stamp: u64| {
        cache
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .put_at(
                hash,
                previews::SIZE,
                crate::worker::thumb_tag((0, stamp)),
                &kept,
                previews::QUALITY,
            )
            .unwrap()
    };
    cache_put(&hash, stamp);
    let local = std::sync::Arc::new(Previews::new(cache.clone()));
    // Culling on z.
    {
        let mut st = state.borrow_mut();
        st.prefetch.set_previews(local.clone());
        st.current = Some(zi);
        crate::panel::cull::enter_cull(&mut st, &app, 1);
        assert!(st.cull.is_some());
    }
    let asked = state.borrow().prefetch.asked();
    let want = asked.iter().find(|w| w.file == zi).expect("z asked for");
    assert_eq!(
        want.from,
        crate::cull::Source::Preview {
            hash: hash.clone(),
            stamp
        },
        "the preview alone"
    );
    let zz_want = asked.iter().find(|w| w.file == zzi).expect("zz asked for");
    assert!(matches!(zz_want.from, crate::cull::Source::Preview { .. }));
    let x_want = asked.iter().find(|w| w.path == x).expect("x asked for");
    assert_eq!(
        x_want.from,
        crate::cull::Source::File,
        "a local root's file"
    );
    // The decode thread's work, here: the preview from the cache,
    // and for zz, which has none kept, the word that says so.
    let fetched = |want: &crate::cull::Want| {
        let got = std::cell::RefCell::new(Vec::new());
        crate::cull::fetch(want, Some(&local), &|l| got.borrow_mut().push(l));
        got.into_inner()
    };
    let got = fetched(want);
    assert_eq!(got.len(), 1);
    crate::panel::cull::deliver_preview(&app, got.into_iter().next().unwrap());
    let got = fetched(zz_want);
    assert!(matches!(&got[..], [crate::cull::Loaded::NoPreview { .. }]));
    crate::panel::cull::deliver_preview(&app, got.into_iter().next().unwrap());
    assert_eq!(
        state
            .borrow()
            .cull
            .as_ref()
            .unwrap()
            .failed
            .get(&zzi)
            .map(String::as_str),
        Some(crate::cull::NO_LOCAL_PREVIEW)
    );
    {
        let st = state.borrow();
        let shown = st.cull.as_ref().unwrap().cache.get(zi).expect("up");
        assert!(shown.local, "marked as the local preview");
        assert_eq!((shown.width, shown.height), (300, 200));
        assert!(!shown.small());
    }
    let read = |p: &Path| {
        crate::cull::FILE_READS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|r| r == p)
    };
    assert!(!read(&z), "nothing of z's file read");
    assert!(!read(&zz), "nor of zz's");
    assert_eq!(disk_reads_under(&away), 0);
    // Its words on the status line.
    let named = [crate::panel::cull::Named {
        file: zi,
        size: (300, 200),
        small: false,
        own: true,
        local: crate::cull::Origin::Local,
        version: None,
    }];
    let line = crate::panel::cull::cull_status(
        state.borrow().cull.as_ref().unwrap(),
        zi,
        0.0,
        true,
        &named,
    );
    assert!(
        line.starts_with("culling: the local preview, 300 \u{d7} 200, fitted"),
        "{line}"
    );
    // The same view with a's root on a network mount: its frame
    // asks for its local preview and its file, every preview of
    // the window ahead of every file.
    let xi = state.borrow().files.iter().position(|f| *f == x).unwrap();
    {
        let mut st = state.borrow_mut();
        st.library.remote = vec![(a.clone(), "nfs".into())];
        crate::panel::cull::cull_refresh(&mut st);
    }
    let asked = state.borrow().prefetch.asked();
    let local_at = asked
        .iter()
        .position(|w| w.file == xi && matches!(w.from, crate::cull::Source::Preview { .. }))
        .expect("x's local preview asked for");
    let file_at = asked
        .iter()
        .position(|w| w.file == xi && w.from == crate::cull::Source::File)
        .expect("x's file asked for");
    let first_file = asked
        .iter()
        .position(|w| w.from == crate::cull::Source::File)
        .unwrap();
    assert!(local_at < first_file && first_file <= file_at, "{asked:?}");
    // Its local preview up, then its file fails (the share has
    // stopped answering): the preview stays, and the next step asks
    // for neither again.
    let (x_hash, x_stamp) = state.borrow().from_row[xi].key.clone().unwrap();
    cache_put(&x_hash, x_stamp);
    let x_local = asked[local_at].clone();
    let got = fetched(&x_local);
    crate::panel::cull::deliver_preview(&app, got.into_iter().next().unwrap());
    crate::panel::cull::deliver_preview(
        &app,
        crate::cull::Loaded::Failed {
            file: xi,
            frame: crate::versions::Key::file(x.clone()),
            size: asked[file_at].size,
            message: "the share did not answer".into(),
        },
    );
    {
        let mut st = state.borrow_mut();
        let cull = st.cull.as_ref().unwrap();
        assert!(cull.cache.get(xi).is_some_and(|p| p.local), "stays up");
        assert!(cull.file_failed.contains(&xi));
        assert!(!cull.failed.contains_key(&xi), "no failure over it");
        crate::panel::cull::cull_refresh(&mut st);
    }
    let asked = state.borrow().prefetch.asked();
    assert!(
        !asked.iter().any(|w| w.file == xi),
        "not asked again: {asked:?}"
    );
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A frame with a copy on an archive is one frame (§216): All roots
/// lists the local copy and leaves the archive's out while the local
/// one is there. When it is not, its root offline or its folder
/// gone after a backup, the frame is listed from the archive, with a
/// picture, rather than as a dimmed stand-in for a file that cannot
/// be opened.
#[test]
fn all_roots_lists_the_archives_copy_when_the_local_one_is_away() {
    let dir = scratch("two-places");
    let (a, nas) = (dir.join("a"), dir.join("nas"));
    std::fs::create_dir_all(a.join("day")).unwrap();
    std::fs::create_dir_all(nas.join("day")).unwrap();
    let x = a.join("x.tif");
    let y = a.join("day").join("y.tif");
    write_frame(&x, &R5, 1);
    write_frame(&y, &R6, 2);
    write_frame(&nas.join("z.tif"), &A7, 3);
    std::fs::copy(&x, nas.join("x.tif")).unwrap();
    std::fs::copy(&y, nas.join("day").join("y.tif")).unwrap();
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.library.roots.add(&a).unwrap();
        st.library.roots.add(&nas).unwrap();
        st.library.roots.set_archive(&nas, true);
        recount(&mut st);
    }
    let listed = |state: &Rc<RefCell<State>>| {
        let mut files = state.borrow().files.clone();
        files.sort();
        files
    };
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert_eq!(
        listed(&state),
        [y.clone(), x.clone(), nas.join("z.tif")],
        "the copies hidden"
    );

    // The day's folder deleted after its backup: its frame is the
    // archive's now.
    std::fs::remove_dir_all(a.join("day")).unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert_eq!(
        listed(&state),
        [x.clone(), nas.join("day").join("y.tif"), nas.join("z.tif")]
    );

    // The root unplugged: both from the archive, and not dimmed.
    crate::testing::rename_away(&a, &dir.join("a-away"));
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert_eq!(
        listed(&state),
        [
            nas.join("day").join("y.tif"),
            nas.join("x.tif"),
            nas.join("z.tif")
        ]
    );
    let thumbs = app.get_thumbs();
    for i in 0..3 {
        assert!(
            !thumbs.row_data(i).unwrap().offline,
            "listed from the archive"
        );
    }
    drop(writer);
    crate::testing::remove_scratch(state, &dir);
}

/// A root that is not there (unplugged, its mount point gone) is
/// offline: its row says so, the launch pass does not touch it, so
/// its rows stay as they were rather than all go missing, and its
/// frames are in the view all the same, from their rows: dimmed,
/// badged, and filtered like any other. Opening one says why
/// nothing more happens, and a key on one changes nothing. A folder
/// under a root that cannot be read is left out of the view. Both
/// are looked at off the window's thread.
#[test]
fn an_offline_root_is_listed_dimmed_and_its_frames_left_alone() {
    let dir = scratch("offline");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(a.join("day")).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    frames(&a, &["x.tif"]);
    frames(&a.join("day"), &["y.tif"]);
    let z = frames(&b, &["z.tif"]).remove(0);
    let mut s = Sidecar::default();
    s.meta.rating = 3;
    s.meta.flag = meta::Flag::Pick;
    s.save(&z).unwrap();
    let z_sidecar = Sidecar::path_for(&z);
    let z_bytes = std::fs::read(&z_sidecar).unwrap();
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.write_sidecars = true;
        st.library.roots.add(&a).unwrap();
        st.library.roots.add(&b).unwrap();
        recount(&mut st);
    }
    // b unplugged: the folder gone from the disk.
    let away = dir.join("b-away");
    crate::testing::rename_away(&b, &away);
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        let chips = app.get_library_roots();
        assert!(!chips.row_data(0).unwrap().offline);
        assert!(chips.row_data(1).unwrap().offline);
        assert_eq!(
            st.files,
            [a.join("x.tif"), a.join("day").join("y.tif"), z.clone()],
            "the offline root's frame is listed"
        );
        assert_eq!(st.library.counts, vec![2, 1], "b's row kept as it was");
        let thumbs = app.get_thumbs();
        assert!(!thumbs.row_data(0).unwrap().offline);
        let tile = thumbs.row_data(2).unwrap();
        assert!(tile.offline, "dimmed");
        assert_eq!((tile.rating, tile.flag), (3, 1), "badged from its row");
        assert!(!st.from_row[2].read);
        assert!(
            st.from_row[2].key.is_some(),
            "the cache's key, from the row"
        );
        assert_eq!(disk_reads_under(&dir), 0);
    }
    // The launch pass over an offline root is not asked for; had
    // it been, the tree pass would mark its row missing.
    assert_eq!(
        state
            .borrow()
            .library
            .roots
            .list()
            .iter()
            .filter(|r| online(r))
            .count(),
        1
    );
    // The filter works on it.
    {
        let mut st = state.borrow_mut();
        st.filter = filter::Filter::from_name("Picks").unwrap();
        rebuild_browser(&mut st, &app);
        assert_eq!(st.shown, [2], "the pick under the offline root");
    }
    // Opening it: selected, and told why there is no picture.
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::open_row(&mut st, &app, &worker, 0, false);
        assert_eq!(st.current, Some(2));
        assert!(!st.from_row[2].read, "nothing read");
        assert!(
            app.get_status().contains("is offline"),
            "{}",
            app.get_status()
        );
        // A key: nothing changes, and nothing is written.
        app.set_status("".into());
        let (_, next) =
            crate::panel::browser::set_meta(&mut st, &app, &[2], meta::Change::Rating(5));
        assert_eq!(next, None);
        assert_eq!(st.sidecars[2].meta.rating, 3);
        assert!(
            app.get_status().contains("is offline"),
            "{}",
            app.get_status()
        );
        assert_eq!(disk_reads_under(&dir), 0);
    }
    // The sidecar went away with its root, and is as it was there.
    let moved = away.join(z_sidecar.file_name().unwrap());
    assert_eq!(
        std::fs::read(&moved).unwrap(),
        z_bytes,
        "the sidecar as it was"
    );
    assert!(!b.exists(), "nothing written where the root was");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let day = a.join("day");
        std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o000)).unwrap();
        let locked = std::fs::read_dir(&day).is_err();
        // Opened again by hand: every folder looked at again.
        open_view(&state, &app, &worker, View::Roots(None));
        land_all(&state, &app, &worker);
        std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o755)).unwrap();
        if locked {
            assert_eq!(
                state.borrow().files,
                [a.join("x.tif"), z.clone()],
                "the locked folder's frame left out"
            );
        }
    }
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A folder under a root that cannot be read (locked after the
/// index walked it) leaves the list at the next read, which looks
/// at it off the window's thread, and comes back at the read after
/// it can be read again, with no pass over it and no open by hand:
/// on a network share a failure can be a moment's. The pass that
/// could not read it says so as a folder, and has the list read.
#[cfg(unix)]
#[test]
fn a_folder_that_cannot_be_read_leaves_the_list_and_comes_back() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("unreadable");
    let root = dir.join("root");
    let (day, other) = (root.join("day"), root.join("other"));
    std::fs::create_dir_all(&day).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    frames(&root, &["x.tif"]);
    frames(&day, &["y.tif", "z.tif"]);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&root, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.index_path = Some(db.clone());
        st.library.roots.add(&root).unwrap();
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().files.len(), 3);
    assert!(state.borrow().library.readable.contains(&day));

    std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&day).is_ok() {
        // Run as a user no permission stops: nothing to see.
        std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o755)).unwrap();
        state.borrow_mut().index_reader = None;
        return;
    }
    let report = writer.index_tree(&root, &mut |_| {}).unwrap();
    assert_eq!(report.unreadable, std::slice::from_ref(&day), "{report:?}");
    folders_passed(&mut state.borrow_mut(), &root);
    assert!(background_done(&state, &app, &root, &report, false, false));
    assert_eq!(sent(), 1);
    land_all(&state, &app, &worker);
    std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(state.borrow().files, [root.join("x.tif")]);
    assert!(
        !state.borrow().library.readable.contains(&day),
        "not kept as readable, and not kept as unreadable either"
    );

    // A file into another folder, and its pass: the read for it
    // looks at the locked folder again, finds it readable, and
    // brings its frames back.
    frames(&other, &["o.tif"]);
    let report = writer.index_folder(&other, &mut |_| {}).unwrap();
    folders_passed(&mut state.borrow_mut(), &other);
    assert!(background_done(&state, &app, &other, &report, false, false));
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().files.len(), 4);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A pass over a folder lands while a read that looked at the
/// folder is out: what the read saw of it is not kept, since the
/// pass is newer, and the next read looks at it again.
#[test]
fn a_read_out_during_a_pass_keeps_nothing_of_the_pass_s_folders() {
    let dir = scratch("epoch");
    let root = dir.join("root");
    let (d, e) = (root.join("d"), root.join("e"));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::create_dir_all(&e).unwrap();
    frames(&d, &["d1.tif"]);
    frames(&e, &["e1.tif"]);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&root, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.library.roots.add(&root).unwrap();
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    folders_passed(&mut state.borrow_mut(), &d);
    frames(&e, &["e2.tif"]);
    let report = writer.index_folder(&e, &mut |_| {}).unwrap();
    folders_passed(&mut state.borrow_mut(), &e);
    assert!(background_done(&state, &app, &e, &report, false, false));
    let found = SENT.with(|s| s.borrow_mut().pop()).unwrap().run();
    // A pass over d lands before the read does.
    folders_passed(&mut state.borrow_mut(), &d);
    land(&state, &app, &worker, found);
    let st = state.borrow();
    assert!(!st.library.readable.contains(&d), "d's look is older");
    assert!(st.library.readable.contains(&e), "e's is not");
    assert_eq!(st.files.len(), 3);
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A read that never comes back (a share that stopped answering in
/// the middle of it) does not hold the list for good: the next
/// refresh past [`READ_GIVES_UP`] sends another, and the one given
/// up on is dropped if it ever lands.
#[test]
fn a_read_that_never_lands_is_given_up_on() {
    let dir = scratch("given-up");
    let files = frames(&dir, &["a.tif", "b.tif"]);
    let app = window(0);
    let (state, worker) = state_for(&app, files[..1].to_vec());
    assert!(refresh_view(&state, &app, &worker));
    let lost = SENT.with(|s| s.borrow_mut().pop()).unwrap();
    assert!(refresh_view(&state, &app, &worker));
    assert_eq!(sent(), 0, "one out at a time");
    state.borrow_mut().library.merge_since =
        Some(Instant::now() - READ_GIVES_UP - Duration::from_secs(1));
    assert!(refresh_view(&state, &app, &worker));
    assert_eq!(sent(), 1, "another sent");
    land(&state, &app, &worker, lost.run());
    assert!(state.borrow().library.merging, "the lost one dropped");
    assert_eq!(state.borrow().files.len(), 1);
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().files, files);
    assert!(!state.borrow().library.merging);
    // A folder's view looks at no root.
    let look = ask(&state.borrow()).unwrap();
    assert!(look.roots.is_empty());
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// Let `blocks` go: the test that holds it is done with it.
static LET_GO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A root's look that does not come back until `LET_GO`, as a stat
/// under a hard-mounted share gone away does not.
fn blocks(_: &Path) -> bool {
    while !LET_GO.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

/// A root that does not answer is offline for the read after the
/// wait, and costs one thread: the reads after it take it for
/// offline at once, without another thread or another wait, until
/// it answers. Then it is looked at afresh.
#[test]
fn a_root_that_does_not_answer_costs_one_look_and_no_more_waits() {
    let checks = Checks::new(blocks);
    let hung = PathBuf::from("/hung/share");
    let wait = Duration::from_millis(200);
    let started = Instant::now();
    let off = checks.offline(std::slice::from_ref(&hung), wait);
    assert!(started.elapsed() >= wait, "the first read waits for it");
    assert_eq!(off, HashSet::from([hung.clone()]));
    for _ in 0..5 {
        let started = Instant::now();
        let off = checks.offline(std::slice::from_ref(&hung), wait);
        assert!(started.elapsed() < wait / 2, "later reads do not wait");
        assert_eq!(off, HashSet::from([hung.clone()]));
    }
    assert_eq!(checks.started.load(Ordering::Relaxed), 1, "one thread");
    LET_GO.store(true, Ordering::SeqCst);
    let until = Instant::now() + Duration::from_secs(5);
    while checks
        .offline(std::slice::from_ref(&hung), wait)
        .contains(&hung)
    {
        assert!(
            Instant::now() < until,
            "it answered, and is looked at again"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // The real look, on a root that is not there and one that is.
    let dir = scratch("roots-answer");
    let gone = dir.join("gone");
    let real = Checks::new(online);
    assert_eq!(
        real.offline(&[dir.clone(), gone.clone()], ROOT_WAIT),
        HashSet::from([gone])
    );
    crate::testing::remove_dir_retry(&dir);
}

/// The frame on screen under a root the read found offline is not
/// looked at (a stat there would not come back): it stays on screen
/// and in the list, while the rest of the root's frames leave.
#[test]
fn the_frame_on_screen_under_a_root_gone_offline_stays() {
    let dir = scratch("offline-current");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    frames(&a, &["x.tif"]);
    frames(&b, &["y.tif", "z.tif"]);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.index_path = Some(db.clone());
        st.library.roots.add(&a).unwrap();
        st.library.roots.add(&b).unwrap();
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    let y = b.join("y.tif");
    {
        let mut st = state.borrow_mut();
        st.current = st.files.iter().position(|f| *f == y);
    }
    crate::testing::rename_away(&b, &dir.join("b-away"));
    assert!(refresh_view(&state, &app, &worker));
    land_all(&state, &app, &worker);
    let st = state.borrow();
    assert!(st.library.offline.contains(&b));
    assert_eq!(st.current.map(|c| st.files[c].clone()), Some(y.clone()));
    assert_eq!(
        st.files,
        [a.join("x.tif"), y, b.join("z.tif")],
        "the root's frames stay, dimmed, the frame on screen among them"
    );
    let thumbs = app.get_thumbs();
    assert!(!thumbs.row_data(0).unwrap().offline);
    assert!(thumbs.row_data(1).unwrap().offline);
    assert!(thumbs.row_data(2).unwrap().offline);
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A root of three frames in the index, and a window on it with
/// the library open: what the loading bar's tests start from.
fn three_under_a_root(
    what: &str,
) -> (
    PathBuf,
    greycard_library::Library,
    App,
    Rc<RefCell<State>>,
    Rc<Worker>,
) {
    let dir = scratch(what);
    let a = dir.join("a");
    std::fs::create_dir_all(&a).unwrap();
    frames(&a, &["x.tif", "y.tif", "z.tif"]);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.library.roots.add(&a).unwrap();
    }
    (dir, writer, app, state, worker)
}

/// The count a view's read keeps, as the pool would bump it.
fn read_so_far(state: &Rc<RefCell<State>>, n: usize) {
    let st = state.borrow();
    let p = st.library.progress.as_ref().expect("a view's read is out");
    p.read.store(n, Ordering::Relaxed);
}

/// The window's timer run on, as the event loop would.
fn tick_on(by: Duration) {
    i_slint_backend_testing::mock_elapsed_time(by);
    slint::platform::update_timers_and_animations();
}

#[test]
fn the_bar_s_words_group_the_thousands() {
    assert_eq!(
        loading_words(3400, 11711, None),
        "Reading sidecars… 3,400 of 11,711"
    );
    assert_eq!(loading_words(0, 1, None), "Reading sidecars… 0 of 1");
    assert_eq!(loading_words(0, 1, Some("Archive")), "Waiting on Archive…");
    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1000), "1,000");
    assert_eq!(grouped(1234567), "1,234,567");
}

/// A view whose sidecars take a while: no bar for the first
/// moment, then one filled to the count read, by the window's own
/// timer, and gone when the view lands.
#[test]
fn a_slow_view_shows_its_bar_after_the_wait_and_drops_it_on_landing() {
    let (dir, writer, app, state, worker) = three_under_a_root("loading-slow");
    open_three(&state, &app, &worker, &dir);
    let look = SENT.with(|s| s.borrow_mut().pop()).unwrap();
    assert!(state.borrow().library.loading);
    assert!(state.borrow().library.loading_timer.running());
    read_so_far(&state, 1);
    let since = state.borrow().library.loading_since.unwrap();
    loading_tick(
        &mut state.borrow_mut(),
        &app,
        since + LOADING_SHOWN_AFTER / 2,
    );
    assert!(!app.get_loading_shown(), "not within the first moment");
    loading_tick(
        &mut state.borrow_mut(),
        &app,
        since + LOADING_SHOWN_AFTER * 2,
    );
    assert!(app.get_loading_shown(), "past it");
    assert!((app.get_loading_fraction() - 1.0 / 3.0).abs() < 1e-6);
    // The window's own timer from here, the read put far enough
    // back that the machine's clock cannot matter.
    state.borrow_mut().library.loading_since =
        Some(Instant::now() - LOADING_SHOWN_AFTER - Duration::from_millis(10));
    tick_on(LOADING_TICK + Duration::from_millis(1));
    assert!(app.get_loading_shown());
    assert!((app.get_loading_fraction() - 1.0 / 3.0).abs() < 1e-6);
    assert_eq!(app.get_loading_line(), "Reading sidecars… 1 of 3");
    read_so_far(&state, 2);
    tick_on(LOADING_TICK + Duration::from_millis(1));
    assert!((app.get_loading_fraction() - 2.0 / 3.0).abs() < 1e-6);
    assert_eq!(app.get_loading_line(), "Reading sidecars… 2 of 3");
    // The count standing still names the root the read waits on,
    // and the count is back as soon as it moves.
    let moved = state.borrow().library.progress_counted.unwrap();
    let later = moved + LOADING_WAITING_AFTER + Duration::from_millis(500);
    loading_tick(&mut state.borrow_mut(), &app, later);
    assert_eq!(app.get_loading_line(), "Waiting on a…");
    read_so_far(&state, 3);
    loading_tick(&mut state.borrow_mut(), &app, later);
    assert_eq!(app.get_loading_line(), "Reading sidecars… 3 of 3");
    // The read itself counts: what it brings is all three.
    land(&state, &app, &worker, look.run());
    {
        let st = state.borrow();
        assert_eq!(st.files.len(), 3);
        assert!(!st.library.loading);
        assert!(st.library.progress.is_none());
        assert!(!st.library.loading_timer.running(), "the timer stopped");
    }
    assert!(!app.get_loading_shown(), "gone on landing");
    tick_on(LOADING_TICK + Duration::from_millis(1));
    assert!(!app.get_loading_shown());
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A view that lands within the first moment never shows a bar.
#[test]
fn a_fast_view_never_shows_its_bar() {
    let (dir, writer, app, state, worker) = three_under_a_root("loading-fast");
    open_view(&state, &app, &worker, View::Roots(None));
    let since = state.borrow().library.loading_since.unwrap();
    for ms in [0, 100, 199] {
        loading_tick(
            &mut state.borrow_mut(),
            &app,
            since + Duration::from_millis(ms),
        );
        assert!(!app.get_loading_shown(), "{ms} ms");
    }
    assert_eq!(land_all(&state, &app, &worker), 1);
    assert!(!app.get_loading_shown());
    assert!(!state.borrow().library.loading_timer.running());
    loading_tick(
        &mut state.borrow_mut(),
        &app,
        since + LOADING_SHOWN_AFTER * 2,
    );
    assert!(!app.get_loading_shown(), "nothing loading, nothing shown");
    assert_eq!(state.borrow().files.len(), 3);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A read that hangs with no report coming is given up on by the
/// timer once it is past READ_GIVES_UP and its count has stood
/// still; one still moving past it keeps its bar.
#[test]
fn the_timer_gives_up_on_a_stopped_read_and_not_a_moving_one() {
    let (dir, writer, app, state, worker) = three_under_a_root("loading-stalled");
    open_three(&state, &app, &worker, &dir);
    let lost = SENT.with(|s| s.borrow_mut().pop()).unwrap();
    let since = state.borrow().library.loading_since.unwrap();
    let tick = |at: Duration| loading_tick(&mut state.borrow_mut(), &app, since + at);
    // Moving: a sidecar every 20 s, well past the give-up.
    tick(Duration::from_secs(1));
    read_so_far(&state, 1);
    tick(READ_GIVES_UP - Duration::from_secs(5));
    read_so_far(&state, 2);
    tick(READ_GIVES_UP + Duration::from_secs(5));
    tick(READ_GIVES_UP + LOADING_STALLED);
    assert!(state.borrow().library.loading, "still moving: kept");
    assert!(app.get_loading_shown());
    assert_eq!(app.get_loading_line(), "Waiting on a…", "ten seconds still");
    // Stopped: the count at 2 for longer than LOADING_STALLED.
    tick(READ_GIVES_UP + Duration::from_secs(5) + LOADING_STALLED + Duration::from_secs(1));
    {
        let st = state.borrow();
        assert!(!st.library.loading, "given up");
        assert!(!st.library.loading_timer.running());
    }
    assert!(!app.get_loading_shown());
    assert!(
        app.get_status().contains("did not come in"),
        "{}",
        app.get_status()
    );
    land(&state, &app, &worker, lost.run());
    assert_eq!(
        state.borrow().view,
        View::Folder,
        "the late read opens nothing"
    );
    assert!(!app.get_loading_shown());
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A total that arrives after a slow listing is movement: the bar
/// shows the count first, and names the root only once both have
/// stood still.
#[test]
fn a_late_total_shows_the_count_before_waiting() {
    let (dir, writer, app, state, worker) = three_under_a_root("loading-late-total");
    open_three(&state, &app, &worker, &dir);
    let look = SENT.with(|s| s.borrow_mut().pop()).unwrap();
    let since = state.borrow().library.loading_since.unwrap();
    let tick = |at: Duration| loading_tick(&mut state.borrow_mut(), &app, since + at);
    tick(Duration::from_secs(2));
    assert_eq!(app.get_loading_line(), "Waiting on a…", "nothing moved");
    let progress = state.borrow().library.progress.clone().unwrap();
    progress.total.store(2, Ordering::Relaxed);
    tick(Duration::from_millis(2100));
    assert_eq!(app.get_loading_line(), "Reading sidecars… 0 of 2");
    tick(Duration::from_millis(3200));
    assert_eq!(app.get_loading_line(), "Waiting on a…");
    drop(look);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A view's read given up on takes its bar with it, and the status
/// line says why; a folder opened over a view's read does too.
#[test]
fn a_view_given_up_on_or_overtaken_drops_its_bar() {
    let (dir, writer, app, state, worker) = three_under_a_root("loading-given-up");
    open_three(&state, &app, &worker, &dir);
    let lost = SENT.with(|s| s.borrow_mut().pop()).unwrap();
    state.borrow_mut().library.loading_since =
        Some(Instant::now() - READ_GIVES_UP - Duration::from_secs(1));
    tick_on(LOADING_TICK + Duration::from_millis(1));
    assert!(app.get_loading_shown(), "still reading, and said so");
    assert_eq!(app.get_loading_line(), "Reading sidecars… 0 of 3");
    refresh_view(&state, &app, &worker);
    assert!(!app.get_loading_shown());
    assert!(!state.borrow().library.loading_timer.running());
    assert!(
        app.get_status().contains("did not come in"),
        "{}",
        app.get_status()
    );
    land(&state, &app, &worker, lost.run());
    land_all(&state, &app, &worker);
    assert!(!app.get_loading_shown(), "the late read brings no bar back");

    // A folder opened while a view's read is out.
    open_view(&state, &app, &worker, View::Roots(None));
    state.borrow_mut().library.loading_since =
        Some(Instant::now() - LOADING_SHOWN_AFTER - Duration::from_millis(10));
    tick_on(LOADING_TICK + Duration::from_millis(1));
    assert!(app.get_loading_shown());
    let one = dir.join("a").join("x.tif");
    crate::panel::browser::open_paths(&state, &app, &worker, &[one]);
    assert!(
        !app.get_loading_shown(),
        "the view's bar went with its read"
    );
    assert!(
        state.borrow().library.loading_timer.running(),
        "the folder's own load has the timer now"
    );
    land_all(&state, &app, &worker);
    assert!(!app.get_loading_shown());
    assert!(!state.borrow().library.loading_timer.running());
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A view's read given up on: the status says so rather than
/// "reading N frames..." for good, a capture waiting on it lets go,
/// and when the read does land it changes nothing, the roots found
/// offline among it.
#[test]
fn a_view_s_read_given_up_on_lets_go_and_lands_as_nothing() {
    let dir = scratch("open-given-up");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let files = frames(&a, &["x.tif"]);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.library.roots.add(&a).unwrap();
        st.library.roots.add(&b).unwrap();
        st.library.awaiting = true;
    }
    std::fs::remove_dir(&b).unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    assert!(app.get_status().contains("reading"), "{}", app.get_status());
    let lost = SENT.with(|s| s.borrow_mut().pop()).unwrap();
    state.borrow_mut().library.loading_since =
        Some(Instant::now() - READ_GIVES_UP - Duration::from_secs(1));
    refresh_view(&state, &app, &worker);
    {
        let st = state.borrow();
        assert!(!st.library.loading);
        assert!(!st.library.awaiting, "a capture lets go");
    }
    assert!(
        !app.get_status().contains("reading"),
        "{}",
        app.get_status()
    );
    land_all(&state, &app, &worker);
    land(&state, &app, &worker, lost.run());
    let st = state.borrow();
    assert!(st.library.offline.is_empty(), "the late read keeps nothing");
    assert_eq!(st.view, View::Folder, "nor opens its view");
    assert_eq!(st.files, files);
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// The review's round trip: a folder under a root locked, and the
/// watcher's pass over that folder fails outright (an error, and a
/// report that says nothing); then unlocked, and the pass over it
/// finds nothing changed. Each has the list read, with no other
/// report needed: the frames leave, and come back.
#[cfg(unix)]
#[test]
fn a_folder_locked_and_unlocked_leaves_and_comes_back_by_its_own_passes() {
    use std::os::unix::fs::PermissionsExt;
    let mut s = Shoot::new("rv-chmod");
    std::fs::set_permissions(&s.d, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&s.d).is_ok() {
        std::fs::set_permissions(&s.d, std::fs::Permissions::from_mode(0o755)).unwrap();
        s.done();
        return;
    }
    let passed = s.writer.index_folder(&s.d, &mut |_| {});
    assert!(passed.is_err(), "the pass over the locked folder fails");
    folders_passed(&mut s.state.borrow_mut(), &s.d);
    let nothing = greycard_library::Report::default();
    assert!(background_done(
        &s.state, &s.app, &s.d, &nothing, false, true
    ));
    land_all(&s.state, &s.app, &s.worker);
    assert_eq!(s.in_d(), 0, "its frames leave");

    std::fs::set_permissions(&s.d, std::fs::Permissions::from_mode(0o755)).unwrap();
    let report = s.writer.index_folder(&s.d, &mut |_| {}).unwrap();
    assert_eq!(report.added + report.changed + report.missing, 0);
    folders_passed(&mut s.state.borrow_mut(), &s.d);
    assert!(background_done(
        &s.state, &s.app, &s.d, &report, false, false
    ));
    land_all(&s.state, &s.app, &s.worker);
    assert_eq!(s.in_d(), 2, "and come back");
    s.done();
}

/// A folder's view through a link, the link pointed elsewhere
/// meanwhile: the next read makes the folder canonical again, so
/// the rows, the passes and "Add this folder" go by where it points
/// now.
#[cfg(unix)]
#[test]
fn a_folder_through_a_link_retargeted_is_followed_by_the_next_read() {
    let dir = scratch("link");
    let (a, b, link) = (dir.join("a"), dir.join("b"), dir.join("link"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    frames(&a, &["x.tif"]);
    frames(&b, &["x.tif", "y.tif"]);
    std::os::unix::fs::symlink(&a, &link).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, vec![link.join("x.tif")]);
    assert!(refresh_view(&state, &app, &worker));
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().library.canonical.get(&link), Some(&a));
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&b, &link).unwrap();
    assert!(refresh_view(&state, &app, &worker));
    land_all(&state, &app, &worker);
    let st = state.borrow();
    assert_eq!(st.library.canonical.get(&link), Some(&b));
    assert_eq!(st.files.len(), 2);
    drop(st);
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// The merge asks the disk nothing: a list whose folders are all
/// gone merges as given, and a read landed after its folder went
/// merges what the read saw.
#[test]
fn a_merge_over_folders_all_gone_completes() {
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    let nowhere = crate::testing::folder(4);
    let read: HashMap<PathBuf, Held> = nowhere
        .iter()
        .map(|p| {
            (
                p.clone(),
                Held {
                    sidecar: Sidecar::default(),
                    from_row: rows::FromRow::read(),
                    id: None,
                    trouble: None,
                },
            )
        })
        .collect();
    merge(
        &mut state.borrow_mut(),
        &app,
        &worker,
        nowhere.clone(),
        Brought {
            read,
            ..Brought::default()
        },
    );
    assert_eq!(state.borrow().files, nowhere);

    // A read of a folder, then the folder deleted before it lands.
    let dir = scratch("all-gone");
    let files = frames(&dir, &["a.tif", "b.tif"]);
    let (state, worker) = state_for(&app, files[..1].to_vec());
    assert!(refresh_view(&state, &app, &worker));
    let look = SENT.with(|s| s.borrow_mut().pop()).unwrap();
    let found = look.run();
    crate::testing::remove_dir_retry(&dir);
    land(&state, &app, &worker, found);
    assert_eq!(state.borrow().files, files);
    assert!(!state.borrow().library.merging);
}

/// How many of this test's frames had their sidecars read from disk.
fn disk_reads_under(dir: &Path) -> usize {
    rows::DISK_READS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|p| p.starts_with(dir))
        .count()
}

/// The three frames of `three_under_a_root` opened as a folder's
/// files, on a window with no index path: every sidecar read from
/// disk, which is what the bar counts.
fn open_three(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path) {
    let a = dir.join("a");
    let files = vec![a.join("x.tif"), a.join("y.tif"), a.join("z.tif")];
    open_listing(state, app, worker, Listing::Files(files), Select::Row(0));
}

/// A root of three frames with sidecars worth looking at, indexed,
/// on a window with the library open, sidecars on: x rated and
/// picked, y developed with a quarter turn and a keyword, z bare.
#[allow(clippy::type_complexity)]
fn rated_under_a_root(
    what: &str,
) -> (
    PathBuf,
    Vec<PathBuf>,
    greycard_library::Library,
    App,
    Rc<RefCell<State>>,
    Rc<Worker>,
) {
    let dir = scratch(what);
    let a = dir.join("a");
    std::fs::create_dir_all(&a).unwrap();
    let files = frames(&a, &["x.tif", "y.tif", "z.tif"]);
    let fresh = || Sidecar {
        line: greycard_edit::Line::of(greycard_edit::Edit::for_picture()),
        ..Sidecar::default()
    };
    let mut s = fresh();
    s.meta.rating = 3;
    s.meta.flag = meta::Flag::Pick;
    s.save(&files[0]).unwrap();
    let mut s = fresh();
    let mut e = greycard_edit::Edit::for_picture();
    e.geometry.turns = 1;
    s.record(e);
    s.meta.set_keywords(vec!["Harbor".into()]);
    s.save(&files[1]).unwrap();
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.index_path = Some(db);
        st.write_sidecars = true;
        st.library.roots.add(&a).unwrap();
    }
    (dir, files, writer, app, state, worker)
}

/// A slider moved over a pick whose sidecar is still being read is
/// refused and said: the stand-in takes nothing, so the read's
/// landing loses nothing and the sidecar on disk is as it was. So
/// is one moved after the read was given up on.
#[test]
fn an_edit_during_a_picks_read_is_refused_and_said() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-pick-edit");
    let on_disk = std::fs::read(Sidecar::path_for(&files[1])).unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    let moved = {
        let mut e = state.borrow().sidecars[1].current.clone();
        e.light.exposure += 1.0;
        e
    };
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::open_row(&mut st, &app, &worker, 1, false);
        assert_eq!(st.pick_pending, Some(files[1].clone()));
        assert!(crate::panel::edit::save_edit(&mut st, moved.clone()));
        assert_eq!(
            crate::rows::not_kept(&st, 1).as_deref(),
            Some("y.tif's sidecar is still being read: the change is not kept")
        );
        assert!(st.sidecars[1].history.is_empty(), "nothing on the stand-in");
        // Unmoved, nothing to say.
        let stand_in = st.sidecars[1].current.clone();
        assert!(!crate::panel::edit::save_edit(&mut st, stand_in));
    }
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert!(st.from_row[1].read);
        assert_eq!(st.sidecars[1].history.len(), 1, "the sidecar's own");
        assert_eq!(st.sidecars[1].current.geometry.turns, 1);
        assert_eq!(
            st.edit, st.sidecars[1].current,
            "the panel is the sidecar's"
        );
    }
    assert_eq!(
        std::fs::read(Sidecar::path_for(&files[1])).unwrap(),
        on_disk,
        "nothing written over it"
    );
    // A pick whose read was given up on stands in still: refused,
    // in other words.
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::open_row(&mut st, &app, &worker, 2, false);
        st.pick_pending = None;
        assert!(crate::panel::edit::save_edit(&mut st, moved));
        assert_eq!(
            crate::rows::not_kept(&st, 2).as_deref(),
            Some("z.tif's sidecar did not come in: the change is not kept")
        );
        assert!(st.sidecars[2].history.is_empty());
    }
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// `--show-mask` reaches the first file when the index stands it
/// in: the open waits for its sidecar (`pick_pending`), the frame
/// is current by the time the read lands and opens it, and that
/// open is still the first.
#[test]
fn the_command_lines_mask_reaches_a_first_file_read_after_its_pick() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-pick-show-mask");
    let mut s = Sidecar::load(&files[1]).unwrap().unwrap();
    let mut e = s.current.clone();
    e.adjustments.push(greycard_edit::Adjustment::default());
    s.record(e);
    s.save(&files[1]).unwrap();
    state.borrow_mut().show_mask = Some(0);
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().current, None, "nothing opened at the start");
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    assert_eq!(state.borrow().pick_pending, Some(files[1].clone()));
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.current, Some(1));
        assert_eq!(st.edit.adjustments.len(), 1, "the sidecar's own edit");
        assert_eq!(st.target, Some(0), "the mask asked for is chosen");
        assert_eq!(st.show_mask, None, "and taken");
    }
    assert!(app.get_show_mask());
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A pick in culling whose sidecar is slow to come: the mode's own
/// line takes the status over meanwhile, and the bar still stops
/// when the read lands.
#[test]
fn a_pick_in_culling_stops_the_bar_when_its_read_lands() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-pick-cull-bar");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::cull::enter_cull(&mut state.borrow_mut(), &app, 1);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    assert_eq!(state.borrow().pick_pending, Some(files[1].clone()));
    assert!(app.get_busy());
    app.set_status("y.tif: waiting for its preview".into());
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    assert_eq!(state.borrow().current, Some(1));
    assert!(state.borrow().pick_pending.is_none());
    assert!(!app.get_busy(), "the bar stops with the read");
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// An export pressed on a frame whose sidecar is being read, the
/// user moving on to another before it lands, exports the pressed
/// frame, as a set of one under its own edit, and not the frame on
/// screen when the read came in.
#[test]
fn an_export_is_of_the_frame_it_was_pressed_on() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-export-source");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 2, false);
    crate::rows::land_pending(&state, &app, &worker);
    assert!(state.borrow().from_row[2].read);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    assert_eq!(state.borrow().pick_pending, Some(files[1].clone()));
    app.invoke_export();
    assert_eq!(state.borrow().loads.whats(), ["open of y.tif", "export"]);
    // z is loaded: open at once, the export still waiting.
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 2, false);
    assert_eq!(state.borrow().current, Some(2));
    crate::rows::land_pending(&state, &app, &worker);
    let st = state.borrow();
    assert_eq!(st.current, Some(2), "the export opened nothing");
    // A set of one, into the folder the sheet shows for it.
    let set = st.exporting.clone().expect("the set");
    assert_eq!(set.total, 1);
    assert_eq!(set.folder.as_deref(), files[1].parent());
    set.cancel();
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// An export asked for before culling, its request waiting on a
/// frame of the set that stands in, and landing in culling on that
/// frame: the panel is no frame's there, so each frame of the set
/// takes its own sidecar's edit, and the one now current is not
/// given the panel's (the frame the mode was entered from).
/// Through Add to queue, whose entry says what each frame took.
#[test]
fn an_export_landing_in_culling_takes_each_frames_own_edit() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-export-cull");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    // z open, its panel at 2.0.
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 2, false);
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(state.borrow().current, Some(2));
    app.set_exposure(2.0);
    // y and z chosen, y standing in still: the queue's request
    // waits on its read.
    {
        let mut st = state.borrow_mut();
        st.picked = vec![1, 2];
        assert!(!st.from_row[1].read);
    }
    state.borrow_mut().export_last_folder = Some(dir.join("out"));
    app.invoke_export_queue_add();
    assert!(state.borrow().export_queue.is_empty(), "waiting");
    // Into culling, on to y; then the read lands.
    {
        let mut st = state.borrow_mut();
        crate::panel::cull::enter_cull(&mut st, &app, 1);
        crate::panel::cull::cull_select(&mut st, &app, 1);
        assert_eq!(st.current, Some(1));
    }
    crate::rows::land_pending(&state, &app, &worker);
    let st = state.borrow();
    assert_eq!(st.export_queue.len(), 1);
    let got: Vec<(PathBuf, f32, u8)> = st.export_queue[0]
        .frames
        .iter()
        .map(|f| {
            (
                f.source.clone(),
                f.edit.light.exposure,
                f.edit.geometry.turns,
            )
        })
        .collect();
    let y = &st.sidecars[1].current;
    assert_eq!(
        got,
        vec![
            (files[1].clone(), y.light.exposure, 1),
            (files[2].clone(), 2.0, 0),
        ],
        "y under its own edit, z under the one saved on the way in"
    );
    assert_ne!(y.light.exposure, 2.0);
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A sidecar the index says holds a develop, and that cannot be
/// read when its frame is opened, or is not there, is not replaced
/// by the default: the frame stands in still, the status line says
/// why, and neither a slider nor a key writes over the file.
#[test]
fn a_failed_read_keeps_the_stand_in() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-failed-read");
    let gcd = Sidecar::path_for(&files[1]);
    std::fs::write(&gcd, b"{ not a sidecar").unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    {
        let mut st = state.borrow_mut();
        assert_eq!(st.current, Some(1));
        assert!(st.pick_pending.is_none());
        assert!(!st.from_row[1].read, "stands in still");
        assert!(st.from_row[1].unread.is_some());
        assert!(
            app.get_status()
                .starts_with("y.tif's sidecar could not be read"),
            "{}",
            app.get_status()
        );
        assert_eq!(st.sidecars[1].meta.keywords, ["Harbor"], "the row's meta");
        let mut moved = st.sidecars[1].current.clone();
        moved.light.exposure += 1.0;
        assert!(crate::panel::edit::save_edit(&mut st, moved));
        assert_eq!(
            crate::rows::not_kept(&st, 1).as_deref(),
            Some("y.tif's sidecar could not be read: the change is not kept")
        );
    }
    // A key reads it again, fails again, and writes nothing.
    crate::panel::browser::meta_on_selection(&state, &app, &worker, meta::Change::Rating(5));
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(std::fs::read(&gcd).unwrap(), b"{ not a sidecar");
    assert!(!state.borrow().from_row[1].read);
    // Not there at all, though the row says there is a develop:
    // the same.
    std::fs::remove_file(&gcd).unwrap();
    {
        let mut st = state.borrow_mut();
        st.from_row[1].unread = None;
        assert!(!crate::rows::load_frame(&mut st, &app, 1));
        assert!(!st.from_row[1].read);
        assert!(
            app.get_status().contains("was not found"),
            "{}",
            app.get_status()
        );
    }
    assert!(!gcd.exists(), "nothing written in its place");
    // A frame the row says has no develop and no sidecar is read as
    // the default, as it always was.
    {
        let mut st = state.borrow_mut();
        assert!(crate::rows::load_frame(&mut st, &app, 2));
        assert!(st.from_row[2].read);
    }
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A pick whose read was started under a list a view's read has
/// since moved on from (the generation bumped) is not opened on the
/// stand-in as if offline: its sidecar is asked for again, and the
/// frame opens on it.
#[test]
fn a_pick_whose_read_was_discarded_is_read_again() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-pick-again");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::open_row(&mut st, &app, &worker, 1, false);
        assert_eq!(st.pick_pending, Some(files[1].clone()));
        // A view's read asked for meanwhile.
        st.view_generation += 1;
    }
    assert_eq!(
        crate::rows::land_pending(&state, &app, &worker),
        2,
        "discarded, then read again"
    );
    let st = state.borrow();
    assert_eq!(st.current, Some(1));
    assert!(st.pick_pending.is_none());
    assert!(st.from_row[1].read);
    assert_eq!(st.sidecars[1].history.len(), 1);
    assert_eq!(st.edit, st.sidecars[1].current);
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A pick whose read is given up on, then brought in by a key's
/// read, is opened on its own sidecar: the panel takes the frame's
/// edit (its quarter turn), and the next slider moves from it
/// rather than writing the stand-in's default over it.
#[test]
fn a_frame_loaded_after_the_give_up_takes_its_own_edit_onto_the_panel() {
    use crate::panel::edit::{read_edit, save_edit};
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-late-load");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    {
        let mut st = state.borrow_mut();
        let mut e = read_edit(&app, &st.edit, st.target);
        e.light.exposure = 0.8;
        assert!(save_edit(&mut st, e), "refused while the read is out");
    }
    let since = state.borrow().loads.since.expect("a read out");
    crate::rows::bar_tick(&state, &app, &worker, since + Duration::from_secs(120));
    assert!(state.borrow().pick_pending.is_none(), "given up on");
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Rating(2)
    ));
    crate::rows::land_pending(&state, &app, &worker);
    {
        let mut st = state.borrow_mut();
        assert_eq!(st.current, Some(1));
        assert!(st.from_row[1].read);
        assert!(st.panel_stand_in.is_none());
        assert_eq!(st.edit.geometry.turns, 1, "the frame's own edit");
        let mut e = read_edit(&app, &st.edit, st.target);
        assert_eq!(e.geometry.turns, 1);
        e.light.exposure = 0.8;
        assert!(!save_edit(&mut st, e), "kept");
    }
    let on_disk = Sidecar::load(&files[1]).unwrap().unwrap();
    assert_eq!(on_disk.current.geometry.turns, 1);
    assert_eq!(on_disk.current.light.exposure, 0.8);
    assert_eq!(on_disk.history.len(), 2, "its own step, then the slider");
    assert_eq!(on_disk.meta.rating, 2);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A preset pressed on a frame whose read was given up on loads the
/// frame there and then: the panel takes its own edit first, and the
/// preset goes over that, with no stand-in's default recorded.
#[test]
fn a_preset_on_a_given_up_pick_goes_over_its_own_edit() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-late-preset");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    let since = state.borrow().loads.since.expect("a read out");
    crate::rows::bar_tick(&state, &app, &worker, since + Duration::from_secs(120));
    let bright = {
        let mut e = greycard_edit::Edit::for_picture();
        e.light.exposure = 1.0;
        greycard_edit::preset::Preset::from_edit(
            "Bright",
            &e,
            &[greycard_edit::preset::Section::Light],
        )
    };
    crate::panel::sync::apply_preset_to(
        &mut state.borrow_mut(),
        &app,
        &worker,
        &bright,
        &[1],
        &[],
        |_, _| None,
    );
    {
        let st = state.borrow();
        assert!(st.from_row[1].read);
        assert!(st.panel_stand_in.is_none());
        assert_eq!(
            st.sidecars[1].current.geometry.turns, 1,
            "its own turn kept"
        );
        assert_eq!(st.sidecars[1].current.light.exposure, 1.0);
        assert_eq!(
            st.sidecars[1].history.len(),
            2,
            "its own step, then the preset"
        );
    }
    let on_disk = Sidecar::load(&files[1]).unwrap().unwrap();
    assert_eq!(on_disk.current.geometry.turns, 1);
    assert_eq!(on_disk.history.len(), 2);
    crate::rows::land_pending(&state, &app, &worker);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Export on a frame on screen whose panel is the stand-in's (its
/// sidecar could not be read) is refused and said: nothing is sent
/// under the default edit while the worker holds another frame's
/// picture.
#[test]
fn an_export_of_a_stand_in_on_screen_is_refused() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-export-stand-in");
    std::fs::write(Sidecar::path_for(&files[1]), b"{ not a sidecar").unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(
        state.borrow().panel_stand_in.as_deref(),
        Some(files[1].as_path())
    );
    app.invoke_export();
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(
        app.get_status(),
        "y.tif's sidecar could not be read: nothing is exported"
    );
    assert!(!state.borrow().export_choosing);
    assert!(!app.get_busy());
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// The frame on screen followed to its new name keeps its panel's
/// stand-in flag.
#[test]
fn following_the_frame_on_screen_moves_the_stand_in_flag() {
    let app = window(0);
    let files = crate::testing::folder(2);
    let (state, _worker) = state_for(&app, files.clone());
    let to = files[0].with_file_name("renamed.CR3");
    let mut st = state.borrow_mut();
    st.current = Some(0);
    st.panel_stand_in = Some(files[0].clone());
    let seen = OnScreen {
        path: Some(files[0].clone()),
        there: false,
        moved_to: Some(to.clone()),
    };
    assert!(follow_current(&mut st, &seen));
    assert_eq!(st.files[0], to);
    assert_eq!(st.panel_stand_in, Some(to));
}

/// A view of the roots is built from the index's rows: each frame's
/// meta, whether it has a develop and how its picture turns come
/// from its row, and no sidecar is read. Opening a frame reads its
/// own, and only its own.
#[test]
fn a_view_of_the_roots_reads_no_sidecar() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-view");
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.files, files);
        assert_eq!(disk_reads_under(&dir), 0, "no sidecar read for the view");
        assert!(st.from_row.iter().all(|r| !r.read), "every frame stands in");
        assert_eq!(st.sidecars[0].meta.rating, 3);
        assert_eq!(st.sidecars[0].meta.flag, meta::Flag::Pick);
        assert_eq!(st.sidecars[1].meta.keywords, ["Harbor"]);
        assert_eq!(st.sidecars[2].meta, Meta::default());
        assert_eq!(
            crate::panel::browser::thumb_turns(&st, &app, 1),
            (1, false),
            "the edit's quarter turn, from the row"
        );
        assert_eq!(crate::panel::browser::thumb_turns(&st, &app, 0), (0, false));
        let thumbs = app.get_thumbs();
        let x = thumbs.row_data(0).unwrap();
        assert_eq!((x.rating, x.flag, x.offline), (3, 1, false));
        assert!(!st.library.loading);
    }
    // Opening y reads its sidecar, off the window's thread, and
    // nothing else's: y is current from the pick, its panel the
    // stand-in's, and opened when the read lands.
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::open_row(&mut st, &app, &worker, 1, false);
        assert_eq!(st.current, Some(1));
        assert!(!st.from_row[1].read, "on its way");
        assert_eq!(st.pick_pending, Some(files[1].clone()));
        assert!(app.get_status().contains("reading"), "{}", app.get_status());
    }
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.current, Some(1));
        assert!(st.pick_pending.is_none());
        assert!(st.from_row[1].read);
        assert!(!st.from_row[0].read && !st.from_row[2].read);
        assert_eq!(disk_reads_under(&dir), 1);
        assert_eq!(st.sidecars[1].current.geometry.turns, 1);
        assert_eq!(st.sidecars[1].history.len(), 1);
        assert_eq!(st.sidecars[1].meta.keywords, ["Harbor"]);
    }
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// The sidecar stays the truth: a sidecar changed on disk is read by
/// the next pass over its folder, and the pass's word has the list
/// read again, so every frame still standing in takes its row's new
/// meta, even when the list itself did not change. A frame already
/// loaded keeps what memory holds.
#[test]
fn a_frame_s_meta_follows_the_pass_while_its_sidecar_stands_in() {
    let (dir, files, mut writer, app, state, worker) = rated_under_a_root("rows-refresh");
    let a = dir.join("a");
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    // z loaded and edited in memory, unsaved.
    {
        let mut st = state.borrow_mut();
        assert!(rows::load_frame(&mut st, &app, 2));
        st.sidecars[2].meta.rating = 1;
    }
    // Another tool rates x five, gives y a label, and z four.
    let mut s = Sidecar::load(&files[0]).unwrap().unwrap();
    s.meta.rating = 5;
    s.save(&files[0]).unwrap();
    let mut s = Sidecar::load(&files[1]).unwrap().unwrap();
    s.meta.label = meta::Label::Red;
    s.save(&files[1]).unwrap();
    let mut s = Sidecar::default();
    s.meta.rating = 4;
    s.save(&files[2]).unwrap();
    let report = writer.index_folder(&a, &mut |_| {}).unwrap();
    assert_eq!(report.meta_refreshed, 3, "{report:?}");
    assert!(background_done(&state, &app, &a, &report, false, false));
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.files, files, "the same list");
        assert_eq!(st.sidecars[0].meta.rating, 5, "x's row, refreshed");
        assert!(!st.from_row[0].read);
        assert_eq!(st.sidecars[1].meta.label, meta::Label::Red);
        assert_eq!(st.sidecars[2].meta.rating, 1, "z as edited in memory");
        let thumbs = app.get_thumbs();
        assert_eq!(thumbs.row_data(0).unwrap().rating, 5, "the badge followed");
        assert_eq!(thumbs.row_data(1).unwrap().label, meta::Label::Red.code());
        assert_eq!(disk_reads_under(&dir), 1, "only z's sidecar was read");
    }
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A rating key on a frame standing in reads its whole sidecar first,
/// changes it, and writes it; the save's pass over the file then
/// brings the row up to what was written.
#[test]
fn a_key_on_a_view_frame_reads_its_sidecar_then_writes_it() {
    let (dir, files, mut writer, app, state, worker) = rated_under_a_root("rows-key");
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let mut st = state.borrow_mut();
        let (settled, next) =
            crate::panel::browser::set_meta(&mut st, &app, &[1], meta::Change::Rating(4));
        assert_eq!(settled, meta::Change::Rating(4));
        assert_eq!(next, None);
        assert!(st.from_row[1].read, "read for the key");
        assert!(!st.from_row[0].read);
        assert_eq!(disk_reads_under(&dir), 1);
        assert_eq!(st.sidecars[1].meta.rating, 4);
        assert_eq!(st.sidecars[1].history.len(), 1, "the develop kept");
    }
    let on_disk = Sidecar::load(&files[1]).unwrap().unwrap();
    assert_eq!(on_disk.meta.rating, 4);
    assert_eq!(on_disk.meta.keywords, ["Harbor"]);
    assert_eq!(on_disk.history.len(), 1, "the develop written back whole");
    // As the save's index_file would: the row follows.
    writer.index_file(&files[1]).unwrap();
    let row = writer
        .rows_of_with(&files[1..2], &mut |d| d.to_path_buf())
        .unwrap()
        .remove(0)
        .unwrap();
    assert_eq!((row.meta.rating, row.edited), (4, true));
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A folder's own open reads nothing on the window's thread: the
/// read is queued (here) and the list lands with it. A folder the
/// index does not know has every sidecar read off the window's
/// thread; one it knows stands in from its rows. An empty folder is
/// said, and the list left alone.
#[test]
fn a_folder_open_reads_off_the_window_s_thread_or_from_the_rows() {
    let dir = scratch("folder-open");
    let files = frames(&dir, &["a.tif", "b.tif", "c.tif"]);
    let mut s = Sidecar::default();
    s.meta.rating = 2;
    s.save(&files[0]).unwrap();
    let mut s = Sidecar::default();
    s.meta.rating = 4;
    s.save(&files[1]).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    state.borrow_mut().write_sidecars = true;
    open_listing(
        &state,
        &app,
        &worker,
        Listing::Folder(dir.clone()),
        Select::Last(Some(files[1].clone())),
    );
    assert_eq!(sent(), 1);
    {
        let st = state.borrow();
        assert!(st.files.is_empty(), "nothing until the read lands");
        assert!(st.library.loading, "and the bar's timer runs meanwhile");
        assert_eq!(disk_reads_under(&dir), 0);
    }
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.files, files);
        assert_eq!(st.view, View::Folder);
        assert!(!st.library.loading);
        assert!(
            st.from_row.iter().all(|r| r.read),
            "no index: every sidecar read"
        );
        assert_eq!(disk_reads_under(&dir), 3);
        let ratings: Vec<u8> = st.sidecars.iter().map(|s| s.meta.rating).collect();
        assert_eq!(ratings, [2, 4, 0]);
        // Opened from the rendering setup, which no test runs.
        assert_eq!(
            st.select_at_start,
            Some(1),
            "the last file open, matched off the thread"
        );
    }
    // Indexed: the rows stand in, and nothing is read.
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_folder(&dir, &mut |_| {}).unwrap();
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.write_sidecars = true;
        st.index_path = Some(db.clone());
    }
    open_listing(
        &state,
        &app,
        &worker,
        Listing::Files(files.clone()),
        Select::Row(2),
    );
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.files, files);
        assert!(st.from_row.iter().all(|r| !r.read), "the rows stand in");
        assert_eq!(disk_reads_under(&dir), 3, "nothing more read");
        let ratings: Vec<u8> = st.sidecars.iter().map(|s| s.meta.rating).collect();
        assert_eq!(ratings, [2, 4, 0]);
        assert_eq!(st.select_at_start, Some(2));
    }
    // An empty folder: said, the list as it was.
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    crate::panel::browser::open_folder(&state, &app, &worker, &empty);
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.files, files, "left alone");
        assert!(!st.library.loading);
        assert!(
            app.get_status().contains("no pictures in"),
            "{}",
            app.get_status()
        );
    }
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A folder on a share that does not answer: its open is looked at
/// with the roots' wait, off the window's thread, and said; the list,
/// the view and the bar are as they were, and no sidecar or listing
/// under it is read. An open sent after it overtakes it, and the
/// hung one's word does not land over the new list. Recently opened
/// says the same of it.
#[test]
fn a_folder_that_does_not_answer_is_said_and_overtaken() {
    let dir = scratch("hung-folder");
    let here = dir.join("here");
    let gone = dir.join("share").join("day");
    let next = dir.join("next");
    for d in [&here, &gone, &next] {
        std::fs::create_dir_all(d).unwrap();
    }
    let files = frames(&here, &["a.tif", "b.tif"]);
    let next_files = frames(&next, &["c.tif"]);
    frames(&gone, &["d.tif"]);
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    state.borrow_mut().write_sidecars = true;
    crate::panel::browser::open_folder(&state, &app, &worker, &here);
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().files, files);
    tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(dir.join("share")));
    crate::panel::browser::open_folder(&state, &app, &worker, &gone);
    assert!(state.borrow().library.loading, "the card is up meanwhile");
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.files, files, "the list is as it was");
        assert_eq!(st.view, View::Folder);
        assert!(!st.library.loading);
        assert_eq!(disk_reads_under(&gone), 0);
        assert!(!st.library.canonical.contains_key(&gone));
    }
    assert!(
        app.get_status().contains("isn't reachable"),
        "{}",
        app.get_status()
    );
    // Overtaken: the hung folder's open, then another before it
    // lands. The second's list is what stays, and nothing of the
    // first is said over it.
    crate::panel::browser::open_folder(&state, &app, &worker, &gone);
    crate::panel::browser::open_folder(&state, &app, &worker, &next);
    assert_eq!(land_all(&state, &app, &worker), 2);
    assert_eq!(state.borrow().files, next_files);
    assert!(
        !app.get_status().contains("isn't reachable"),
        "{}",
        app.get_status()
    );
    // Recently opened: the same look, the same words.
    crate::panel::recent::choose_for_test(&state, &app, &worker, &gone);
    assert!(
        app.get_status().contains("isn't reachable"),
        "{}",
        app.get_status()
    );
    assert_eq!(state.borrow().files, next_files);
    tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// A folder opened through a link is made canonical by the read, off
/// the window's thread, and Recently opened records the folder it
/// points to from that, not from a look of its own.
#[cfg(unix)]
#[test]
fn a_folder_opened_through_a_link_is_recorded_as_the_read_made_it() {
    let dir = scratch("link-open");
    let real = dir.join("real");
    std::fs::create_dir_all(&real).unwrap();
    frames(&real, &["a.tif"]);
    let link = dir.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    crate::panel::browser::open_folder(&state, &app, &worker, &link);
    land_all(&state, &app, &worker);
    let st = state.borrow();
    assert_eq!(st.files, vec![link.join("a.tif")]);
    assert_eq!(st.library.canonical.get(&link), Some(&real));
    assert_eq!(st.recent.open.as_deref(), Some(real.as_path()));
    drop(st);
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// Two roots on a window with the library open, sidecars on: `a`
/// with x (rated, flagged reject) and `b` with z (flagged reject,
/// three stars), both indexed, then `b` renamed away so it is
/// offline, and the view of the roots opened and landed: x is
/// file 0, z file 1, both standing in from their rows.
#[allow(clippy::type_complexity)]
fn offline_view(
    what: &str,
) -> (
    PathBuf,
    PathBuf,
    Vec<PathBuf>,
    greycard_library::Library,
    App,
    Rc<RefCell<State>>,
    Rc<Worker>,
) {
    let dir = scratch(what);
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let x = frames(&a, &["x.tif"]).remove(0);
    let z = frames(&b, &["z.tif"]).remove(0);
    let mut s = Sidecar::default();
    s.meta.rating = 2;
    s.meta.flag = meta::Flag::Reject;
    s.save(&x).unwrap();
    let mut s = Sidecar::default();
    s.meta.rating = 3;
    s.meta.flag = meta::Flag::Reject;
    s.save(&z).unwrap();
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.index_path = Some(db);
        st.write_sidecars = true;
        st.library.roots.add(&a).unwrap();
        st.library.roots.add(&b).unwrap();
    }
    let away = dir.join("b-away");
    crate::testing::rename_away(&b, &away);
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.files, [x.clone(), z.clone()]);
        assert!(st.library.offline.contains(&b));
        assert!(st.from_row.iter().all(|r| !r.read));
    }
    (dir, away, vec![x, z], writer, app, state, worker)
}

/// Moving the rejects leaves a root that is offline alone: nothing
/// is made where it was (a folder made on a mount point would
/// have the next pass mark the drive's frames missing), its
/// frames stay in the list, and its rows stay as they were. The
/// online root's reject goes out as ever. A delete of the
/// selection leaves an offline frame out the same way.
#[test]
fn the_rejects_move_and_a_delete_leave_an_offline_root_alone() {
    let (dir, away, files, mut writer, app, state, worker) = offline_view("offline-rejects");
    let (a, b) = (dir.join("a"), dir.join("b"));
    let (x, z) = (files[0].clone(), files[1].clone());
    {
        let mut st = state.borrow_mut();
        assert!(crate::panel::cull::to_move_out(&st, 0));
        assert!(
            !crate::panel::cull::to_move_out(&st, 1),
            "a reject under an offline root is not one to move"
        );
        crate::panel::cull::move_rejects(&mut st, &app, &worker);
        assert_eq!(
            st.files,
            std::slice::from_ref(&z),
            "x went out; z stays, untouched"
        );
    }
    assert!(!b.exists(), "nothing made where the root was");
    assert!(
        !away.join("rejects").exists(),
        "nothing made on the drive either"
    );
    assert!(away.join("z.tif").exists());
    assert!(!x.exists() && a.join("rejects").join("x.tif").exists());
    // The pass over the online root marks nothing of the other's.
    writer.index_folder(&a, &mut |_| {}).unwrap();
    let row = writer.by_path(&z).unwrap().unwrap();
    assert!(!row.missing, "z's row as it was");
    assert_eq!(writer.count_under(&b).unwrap(), 1);
    // A delete of a set with the offline frame in it leaves it out.
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.picked = vec![0];
        app.set_status("".into());
        crate::panel::delete::ask_delete(&mut st, &app, crate::panel::delete::Which::Selection);
        assert!(st.delete_asked.is_none(), "nothing to ask about");
        assert!(
            app.get_status().contains("is offline"),
            "{}",
            app.get_status()
        );
    }
    assert!(away.join("z.tif").exists());
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A turn on a frame standing in reads its sidecar first and
/// writes the turn into it, and the thumbnail follows; a turn on
/// a frame under an offline root is refused with a word, and
/// nothing changes anywhere.
#[test]
fn a_turn_on_a_view_frame_reads_its_sidecar_then_writes_it() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-turn");
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let mut st = state.borrow_mut();
        assert!(!st.from_row[0].read);
        crate::panel::browser::turn_frames(&mut st, &app, &worker, &[0], 1);
        assert!(st.from_row[0].read, "read for the turn");
        assert_eq!(st.sidecars[0].turn, 1);
        assert_eq!(st.sidecars[0].meta.rating, 3, "its own meta kept");
        assert_eq!(crate::panel::browser::thumb_turns(&st, &app, 0), (3, false));
        assert_eq!(disk_reads_under(&dir), 1);
    }
    let on_disk = Sidecar::load(&files[0]).unwrap().unwrap();
    assert_eq!(on_disk.turn, 1, "written");
    assert_eq!(on_disk.meta.rating, 3);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);

    let (dir, away, _files, writer, app, state, worker) = offline_view("offline-turn");
    let before = std::fs::read(away.join("z.tif.gcd")).unwrap();
    {
        let mut st = state.borrow_mut();
        app.set_status("".into());
        crate::panel::browser::turn_frames(&mut st, &app, &worker, &[1], 1);
        assert!(!st.from_row[1].read, "nothing read");
        assert_eq!(st.sidecars[1].turn, 0, "nothing turned in memory either");
        assert!(
            app.get_status().contains("is offline"),
            "{}",
            app.get_status()
        );
    }
    assert_eq!(std::fs::read(away.join("z.tif.gcd")).unwrap(), before);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A frame under an offline root opened in the loupe shows the
/// default edit that stands in for it, not the last frame's, and
/// is no source: a copy, a sync and a preset from it are refused
/// with a word, in the loupe and in culling alike, and the other
/// frames' sidecars are left as they were.
#[test]
fn an_offline_frame_is_no_source_for_a_copy_a_sync_or_a_preset() {
    use greycard_edit::preset::{Preset, Section};
    let (dir, _away, files, writer, app, state, worker) = offline_view("offline-source");
    let x = files[0].clone();
    let x_before;
    {
        // x on screen first, with an exposure of its own on the
        // panel, so the panel holds something that is not z's.
        crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
        crate::rows::land_pending(&state, &app, &worker);
        let mut st = state.borrow_mut();
        assert_eq!(st.current, Some(0));
        assert!(st.from_row[0].read);
        st.edit.light.exposure = 0.7;
        app.set_exposure(0.7);
        // Then z, under the offline root. Leaving x saves the panel's
        // exposure into x, as leaving any frame does: one step.
        crate::panel::browser::open_row(&mut st, &app, &worker, 1, false);
        assert_eq!(st.current, Some(1));
        assert!(!st.from_row[1].read);
        assert_eq!(st.sidecars[0].history.len(), 1);
        assert_eq!(st.sidecars[0].current.light.exposure, 0.7);
        x_before = std::fs::read(Sidecar::path_for(&x)).unwrap();
        // The frames are pictures: a picture's default stands in.
        assert_eq!(
            st.edit,
            greycard_edit::Edit::for_picture(),
            "the stand-in's default, not x's"
        );
        assert!(
            app.get_status().contains("is offline"),
            "{}",
            app.get_status()
        );
        // Copy: refused.
        app.set_status("".into());
        crate::panel::sync::copy_settings(&mut st, &app);
        assert!(st.clipboard.is_none());
        assert!(
            app.get_status().contains("nothing to copy"),
            "{}",
            app.get_status()
        );
        // Sync onto x: refused, x untouched.
        st.picked = vec![0, 1];
        let synced = crate::panel::sync::sync_selection(
            &mut st,
            &app,
            Section::ALL,
            &[],
            crate::panel::sync::probe,
        );
        assert!(synced.moved.is_empty());
        assert_eq!(st.sidecars[0].history.len(), 1, "x got no step");
        // A preset: refused.
        let mut e = greycard_edit::Edit::default();
        e.light.exposure = 1.5;
        let preset = Preset::from_edit("bright", &e, Section::ALL);
        crate::panel::sync::apply_preset(
            &mut st,
            &app,
            &worker,
            &preset,
            &[],
            crate::panel::sync::probe,
        );
        assert_eq!(st.sidecars[0].history.len(), 1);
        assert_eq!(st.sidecars[1].current, greycard_edit::Edit::for_picture());
        assert!(
            app.get_status().contains("is offline"),
            "{}",
            app.get_status()
        );
        // In culling the sidecar is the truth, and a stand-in is
        // not one to sync from.
        crate::panel::cull::enter_cull(&mut st, &app, 1);
        crate::panel::cull::cull_select(&mut st, &app, 1);
        assert_eq!(st.current, Some(1));
        let synced = crate::panel::sync::sync_selection(
            &mut st,
            &app,
            Section::ALL,
            &[],
            crate::panel::sync::probe,
        );
        assert!(synced.moved.is_empty());
    }
    assert_eq!(
        std::fs::read(Sidecar::path_for(&x)).unwrap(),
        x_before,
        "x's sidecar as it was"
    );
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Culling's compare tiles and the loupe's stand-in read a frame's
/// turns from its row while its sidecar stands in.
#[test]
fn the_culling_tiles_turn_a_frame_by_its_row() {
    let (dir, _files, writer, app, state, worker) = rated_under_a_root("rows-tiles");
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    let st = state.borrow();
    assert!(!st.from_row[1].read);
    assert_eq!(
        crate::panel::cull::thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, 1, &app, false),
        (1, false),
        "y's edit turns it once, says the row"
    );
    assert_eq!(
        crate::panel::cull::thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, 1, &app, true),
        (1, false),
        "and not the panel, which is not y's"
    );
    assert_eq!(
        crate::panel::cull::thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, 0, &app, false),
        (0, false)
    );
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A rating another tool wrote to an XMP beside the frame, with no
/// `.gcd` at all, is in the row and so in a list built from the
/// rows: the badge and the filter see it before the frame is ever
/// opened, as they did when every sidecar was read.
#[test]
fn an_xmp_s_rating_shows_in_a_list_built_from_the_rows() {
    use greycard_edit::xmp;
    let dir = scratch("rows-xmp");
    let files = frames(&dir, &["a.tif", "b.tif"]);
    let three = Meta {
        rating: 3,
        ..Meta::default()
    };
    std::fs::write(xmp::short_path(&files[0]), xmp::fresh(&three, None)).unwrap();
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_folder(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.write_sidecars = true;
        st.index_path = Some(db.clone());
    }
    open_listing(
        &state,
        &app,
        &worker,
        Listing::Files(files.clone()),
        Select::Row(0),
    );
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let mut st = state.borrow_mut();
        assert!(st.from_row.iter().all(|r| !r.read), "from the rows");
        assert_eq!(
            st.sidecars[0].meta.rating, 3,
            "the XMP's rating, through the row"
        );
        assert_eq!(st.sidecars[1].meta.rating, 0);
        assert_eq!(app.get_thumbs().row_data(0).unwrap().rating, 3);
        st.filter = filter::Filter {
            text: "rating>=3".into(),
            ..filter::Filter::default()
        };
        rebuild_browser(&mut st, &app);
        assert_eq!(st.shown, [0], "the filter sees it");
        // As the loader gives it, on opening.
        let (loaded, _) = crate::panel::browser::load_sidecar(&files[0], true);
        assert_eq!(loaded.meta, st.sidecars[0].meta);
    }
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// `n` bare frames under one root, indexed, the view of the roots
/// opened and landed on a window with the library open and sidecars
/// on: every frame standing in from its row.
#[allow(clippy::type_complexity)]
fn many_under_a_root(
    what: &str,
    n: usize,
) -> (
    PathBuf,
    Vec<PathBuf>,
    greycard_library::Library,
    App,
    Rc<RefCell<State>>,
    Rc<Worker>,
) {
    let dir = scratch(what);
    let a = dir.join("a");
    std::fs::create_dir_all(&a).unwrap();
    let names: Vec<String> = (0..n).map(|i| format!("f{i:02}.tif")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let files = frames(&a, &names);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.index_path = Some(db);
        st.write_sidecars = true;
        st.library.roots.add(&a).unwrap();
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().files, files);
    (dir, files, writer, app, state, worker)
}

/// The frame's whole sidecar in memory after its pick landed.
fn open_and_land(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, row: i32) {
    crate::panel::browser::open_row(&mut state.borrow_mut(), app, worker, row, false);
    crate::rows::land_pending(state, app, worker);
}

/// An arrow onto a frame whose sidecar is on its way, then a key:
/// the frame is current from the arrow, so the key is a request on
/// it alone, queued behind its open, and applied once it is open.
/// The frame that was current before is left byte for byte as it
/// was; nothing of the key reaches it.
#[test]
fn a_key_during_a_pick_s_read_reaches_the_pick_alone() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-arrow-key");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    open_and_land(&state, &app, &worker, 0);
    let x_before = std::fs::read(Sidecar::path_for(&files[0])).unwrap();
    {
        let mut st = state.borrow_mut();
        assert_eq!(st.current, Some(0));
        crate::panel::browser::open_row(&mut st, &app, &worker, 1, false);
        assert_eq!(st.current, Some(1), "current from the pick");
        assert_eq!(st.picked, [1]);
        assert!(!st.from_row[1].read);
    }
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Rating(5)
    ));
    {
        let st = state.borrow();
        assert_eq!(
            st.sidecars[1].meta.rating, 0,
            "not yet: queued behind the open"
        );
        assert_eq!(st.sidecars[0].meta.rating, 3, "x untouched");
        assert_eq!(st.loads.queue.len(), 2, "the open, then the key");
    }
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.current, Some(1));
        assert!(st.from_row[1].read);
        assert!(st.pick_pending.is_none());
        assert_eq!(st.sidecars[1].meta.rating, 5, "the key, after the open");
        assert_eq!(st.sidecars[0].meta.rating, 3);
        assert!(st.loads.queue.is_empty());
    }
    assert_eq!(Sidecar::load(&files[1]).unwrap().unwrap().meta.rating, 5);
    assert_eq!(
        std::fs::read(Sidecar::path_for(&files[0])).unwrap(),
        x_before,
        "the frame before the pick, as it was"
    );
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A preset pressed on fifty frames whose sidecars are still to be
/// read, then a click on a loaded frame: the preset lands on the
/// fifty it was pressed on, not on the frame on screen by then.
#[test]
fn a_preset_lands_on_the_frames_it_was_pressed_on() {
    use greycard_edit::preset::{Preset, Section};
    let (dir, _files, writer, app, state, worker) = many_under_a_root("rows-preset-set", 52);
    // Frame 51 loaded, then frame 0 on screen with 0..50 chosen.
    open_and_land(&state, &app, &worker, 51);
    open_and_land(&state, &app, &worker, 0);
    state.borrow_mut().picked = (0..50).collect();
    let mut e = greycard_edit::Edit::for_picture();
    e.light.exposure = 1.5;
    let preset = Preset::from_edit("bright", &e, Section::ALL);
    crate::panel::sync::preset_pressed(&state, &app, &worker, preset);
    assert!(state.borrow().loads.reading(), "49 sidecars to read");
    // Meanwhile: the loaded frame clicked, which opens at once.
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 51, false);
    assert_eq!(state.borrow().current, Some(51));
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    let st = state.borrow();
    for f in 0..50 {
        assert_eq!(st.sidecars[f].history.len(), 1, "frame {f} took the preset");
        assert_eq!(st.sidecars[f].current.light.exposure, 1.5, "frame {f}");
    }
    assert_eq!(st.sidecars[50].history.len(), 0, "not chosen");
    assert_eq!(
        st.sidecars[51].history.len(),
        0,
        "on screen by then, not pressed on"
    );
    assert_eq!(st.current, Some(51));
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Two keys during one read both apply, in order.
#[test]
fn two_keys_during_one_read_both_apply_in_order() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-two-keys");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    for change in [
        meta::Change::Rating(4),
        meta::Change::Flag(meta::Flag::Pick),
        meta::Change::Rating(2),
    ] {
        assert!(crate::panel::browser::meta_on_selection(
            &state, &app, &worker, change
        ));
    }
    assert_eq!(
        state.borrow().loads.queue.len(),
        4,
        "the open and three keys, in order"
    );
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert!(st.loads.queue.is_empty());
        assert_eq!(st.sidecars[1].meta.rating, 2, "the last rating stands");
        assert_eq!(st.sidecars[1].meta.flag, meta::Flag::Pick);
        assert_eq!(
            st.sidecars[1].meta.keywords,
            ["Harbor"],
            "its own meta under them"
        );
        assert_eq!(st.sidecars[0].meta.rating, 3);
    }
    let on_disk = Sidecar::load(&files[1]).unwrap().unwrap();
    assert_eq!(
        (on_disk.meta.rating, on_disk.meta.flag),
        (2, meta::Flag::Pick)
    );
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A click on a frame whose sidecar is to be read, then on a loaded
/// one before it lands: the loaded one opens and stays open; the
/// first's sidecar comes in silently, and nothing jumps back.
#[test]
fn a_pick_moved_on_from_lands_silently() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-moved-on");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    open_and_land(&state, &app, &worker, 0);
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::open_row(&mut st, &app, &worker, 1, false);
        assert_eq!(st.current, Some(1));
        assert_eq!(st.pick_pending, Some(files[1].clone()));
        // Back to x, loaded: opens at once, and the pick is over.
        crate::panel::browser::open_row(&mut st, &app, &worker, 0, false);
        assert_eq!(st.current, Some(0));
        assert!(st.pick_pending.is_none());
        assert!(
            !app.get_status().contains("reading"),
            "{}",
            app.get_status()
        );
    }
    let status = app.get_status();
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    let st = state.borrow();
    assert_eq!(st.current, Some(0), "no jump back");
    assert!(st.from_row[1].read, "y's sidecar in memory all the same");
    assert_eq!(st.sidecars[1].history.len(), 1, "y's own");
    assert_eq!(app.get_status(), status, "nothing said");
    assert!(st.loads.queue.is_empty());
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A read dropped with its list takes the bar down with it, and a
/// frames' read never touches the roots view's read: the two keep
/// their own state.
#[test]
fn a_dropped_read_clears_the_bar() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-dropped");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    let since = state.borrow().loads.since.expect("a read out");
    assert!(state.borrow().loads.timer.running());
    crate::rows::bar_tick(&state, &app, &worker, since + Duration::from_millis(300));
    assert!(app.get_loading_shown(), "the frames' bar, after the moment");
    assert_eq!(app.get_loading_line(), "Reading sidecars… 0 of 1");
    let waited = since + LOADING_WAITING_AFTER + Duration::from_millis(500);
    crate::rows::bar_tick(&state, &app, &worker, waited);
    assert_eq!(app.get_loading_line(), "Waiting on a…");
    assert!(!state.borrow().library.loading, "not the view's read");
    // The list replaced: the request goes, the bar with it.
    open_listing(
        &state,
        &app,
        &worker,
        Listing::Files(vec![files[0].clone()]),
        Select::Row(0),
    );
    // The new list lands, and the requests for the old go with it.
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().files, std::slice::from_ref(&files[0]));
    {
        let st = state.borrow();
        assert!(st.loads.queue.is_empty());
        assert!(st.loads.since.is_none());
        assert!(!st.loads.timer.running());
        assert!(st.pick_pending.is_none());
    }
    assert!(!app.get_loading_shown());
    // The dropped request's read lands on nothing, quietly.
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    assert!(!app.get_loading_shown());
    assert!(state.borrow().loads.queue.is_empty());
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A purple label, and no label, from the menu on a frame whose
/// sidecar is still to be read: the change itself rides with the
/// request, so every label reaches the frame.
#[test]
fn a_purple_label_and_no_label_reach_an_unloaded_frame() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-purple");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 2, false);
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Label(meta::Label::Purple)
    ));
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(state.borrow().sidecars[2].meta.label, meta::Label::Purple);
    assert_eq!(
        Sidecar::load(&files[2]).unwrap().unwrap().meta.label,
        meta::Label::Purple
    );
    // Another frame, still to be read, and no label asked of it: a
    // change all the same, applied when it is in.
    {
        let mut st = state.borrow_mut();
        st.sidecars[1].meta.label = meta::Label::Red;
        st.sidecars[1].save(&files[1]).unwrap();
        // Standing in again, as a list from the rows would have it.
        st.from_row[1].read = false;
    }
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Label(meta::Label::None)
    ));
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(state.borrow().sidecars[1].meta.label, meta::Label::None);
    assert_eq!(
        Sidecar::load(&files[1]).unwrap().unwrap().meta.label,
        meta::Label::None
    );
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// An export record for a frame the list holds as a stand-in (the
/// list replaced while the export ran) reaches the disk: the
/// sidecar is read and written as for a frame the window let go
/// of, never the stand-in.
#[test]
fn an_export_record_reaches_a_frame_that_became_a_stand_in() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-export");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    let edit = Sidecar::load(&files[1]).unwrap().unwrap().line.current;
    {
        let mut st = state.borrow_mut();
        assert!(!st.from_row[1].read, "a stand-in");
        crate::panel::history::record_export(
            &mut st,
            &app,
            &files[1],
            &edit,
            None,
            &dir.join("y.jpg"),
            None,
        );
        assert!(
            !st.from_row[1].read,
            "still a stand-in; nothing recorded on it"
        );
        assert!(st.sidecars[1].current_exports.is_empty());
    }
    let on_disk = Sidecar::load(&files[1]).unwrap().unwrap();
    assert_eq!(on_disk.current_exports.len(), 1);
    assert!(on_disk.current_exports[0].file.ends_with("y.jpg"));
    assert_eq!(on_disk.history.len(), 1, "its develop kept");
    drop(worker);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Culling with move-on, the frames' sidecars still to be read: each
/// key reaches the frame the user saw when pressing it, and the pick
/// moves on at the press, so a run of keys rates a run of frames and
/// ends where the user expects. Stepped when the key landed, every
/// key pressed during the first frame's read went to that frame.
#[test]
fn culling_s_move_on_steps_at_the_press_and_each_key_finds_its_frame() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-move-on");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    {
        let mut st = state.borrow_mut();
        crate::panel::cull::enter_cull(&mut st, &app, 1);
        st.cull_move_on = true;
    }
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
    assert_eq!(state.borrow().current, Some(0));
    for change in [meta::Change::Rating(4), meta::Change::Rating(2)] {
        assert!(crate::panel::browser::meta_on_selection(
            &state, &app, &worker, change
        ));
    }
    assert_eq!(state.borrow().current, Some(2), "moved on at each press");
    crate::rows::land_pending(&state, &app, &worker);
    {
        let st = state.borrow();
        assert_eq!(st.sidecars[0].meta.rating, 4, "x got the first key");
        assert_eq!(st.sidecars[1].meta.rating, 2, "y the second");
        assert_eq!(st.sidecars[2].meta.rating, 0);
        assert_eq!(st.current, Some(2));
        assert!(st.loads.queue.is_empty());
    }
    assert_eq!(Sidecar::load(&files[0]).unwrap().unwrap().meta.rating, 4);
    assert_eq!(Sidecar::load(&files[1]).unwrap().unwrap().meta.rating, 2);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);

    // Ten keys on ten unread frames.
    let (dir, files, writer, app, state, worker) = many_under_a_root("rows-move-on-ten", 12);
    {
        let mut st = state.borrow_mut();
        crate::panel::cull::enter_cull(&mut st, &app, 1);
        st.cull_move_on = true;
    }
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
    for k in 0..10u8 {
        let stars = k % 5 + 1;
        assert!(crate::panel::browser::meta_on_selection(
            &state,
            &app,
            &worker,
            meta::Change::Rating(stars)
        ));
    }
    assert_eq!(state.borrow().current, Some(10));
    crate::rows::land_pending(&state, &app, &worker);
    let st = state.borrow();
    for k in 0..10u8 {
        let f = usize::from(k);
        assert_eq!(st.sidecars[f].meta.rating, k % 5 + 1, "frame {f}");
        assert_eq!(
            Sidecar::load(&files[f]).unwrap().unwrap().meta.rating,
            k % 5 + 1
        );
    }
    assert_eq!(st.sidecars[10].meta.rating, 0);
    assert_eq!(st.current, Some(10));
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A read started under one list lands nothing in the next: the
/// path may have been written meanwhile, and the older bytes would
/// be saved over the newer by the next key. The write survives, and
/// a read of the new list brings it.
#[test]
fn a_read_from_an_older_list_fills_no_stand_in_of_the_new() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-late-read");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    // The list replaced while y's read is out: the same files, from
    // their rows again.
    open_listing(
        &state,
        &app,
        &worker,
        Listing::Files(files.clone()),
        Select::Row(0),
    );
    land_all(&state, &app, &worker);
    assert!(!state.borrow().from_row[1].read);
    // y written meanwhile.
    let mut s = Sidecar::load(&files[1]).unwrap().unwrap();
    s.meta.rating = 5;
    s.save(&files[1]).unwrap();
    // The old read lands: nothing taken.
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    assert!(
        !state.borrow().from_row[1].read,
        "the older read filled nothing"
    );
    assert_eq!(Sidecar::load(&files[1]).unwrap().unwrap().meta.rating, 5);
    // A key on y now: its own read, of the newer bytes, then the key.
    state.borrow_mut().current = Some(1);
    state.borrow_mut().picked = vec![1];
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Flag(meta::Flag::Pick)
    ));
    crate::rows::land_pending(&state, &app, &worker);
    let on_disk = Sidecar::load(&files[1]).unwrap().unwrap();
    assert_eq!(
        (on_disk.meta.rating, on_disk.meta.flag),
        (5, meta::Flag::Pick)
    );
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Giving up on a read names what it dropped, and a dropped pick is
/// let go: not busy, no pick pending, the stand-in shown as an
/// offline frame is.
#[test]
fn giving_up_names_what_was_dropped_and_lets_a_pick_go() {
    let (dir, _files, writer, app, state, worker) = rated_under_a_root("rows-give-up");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    for change in [meta::Change::Rating(4), meta::Change::Rating(3)] {
        assert!(crate::panel::browser::meta_on_selection(
            &state, &app, &worker, change
        ));
    }
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Flag(meta::Flag::Pick)
    ));
    assert!(app.get_busy());
    let since = state.borrow().loads.since.unwrap();
    crate::rows::bar_tick(
        &state,
        &app,
        &worker,
        since + Duration::from_secs(61) + Duration::from_secs(11),
    );
    {
        let st = state.borrow();
        assert_eq!(
            app.get_status(),
            "the open of y.tif, 2 rating keys and the flag key dropped: the sidecars did not come in"
        );
        assert!(st.pick_pending.is_none());
        assert!(!app.get_busy());
        assert!(st.loads.queue.is_empty());
        assert!(st.loads.since.is_none());
        assert_eq!(
            st.current,
            Some(1),
            "the frame stays shown, its stand-in on the panel"
        );
        assert_eq!(st.sidecars[1].meta.rating, 0, "nothing applied");
    }
    assert!(!app.get_loading_shown());
    // The read that was given up on lands on the same list: the
    // sidecar comes in silently, and nothing runs.
    crate::rows::land_pending(&state, &app, &worker);
    let st = state.borrow();
    assert!(st.from_row[1].read);
    assert_eq!(st.sidecars[1].meta.rating, 0);
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A frame a request named that is gone from the list by the time
/// the request runs takes nothing of it, and the status line says
/// so; the others take it.
#[test]
fn a_frame_gone_before_its_request_ran_is_said() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-gone");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    open_and_land(&state, &app, &worker, 0);
    state.borrow_mut().picked = vec![0, 1];
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Rating(5)
    ));
    // y moves before its read lands.
    state.borrow_mut().files[1] = dir.join("a").join("elsewhere.tif");
    crate::rows::land_pending(&state, &app, &worker);
    let st = state.borrow();
    assert_eq!(st.sidecars[0].meta.rating, 5, "x took it");
    assert_eq!(st.sidecars[1].meta.rating, 0);
    assert_eq!(
        app.get_status(),
        "y.tif moved or is gone; the rating key was not applied to it"
    );
    assert_eq!(Sidecar::load(&files[1]).unwrap().unwrap().meta.rating, 0);
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Three picked frames under a root, indexed, the view opened and
/// landed, culling on with move-on and the Picks filter up: what a
/// key that unflags one of them starts from.
#[allow(clippy::type_complexity)]
fn three_picks_culling(
    what: &str,
) -> (
    PathBuf,
    Vec<PathBuf>,
    greycard_library::Library,
    App,
    Rc<RefCell<State>>,
    Rc<Worker>,
) {
    let dir = scratch(what);
    let a = dir.join("a");
    std::fs::create_dir_all(&a).unwrap();
    let files = frames(&a, &["f00.tif", "f01.tif", "f02.tif"]);
    for f in &files {
        let mut s = Sidecar::default();
        s.meta.flag = meta::Flag::Pick;
        s.save(f).unwrap();
    }
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.index_path = Some(db);
        st.write_sidecars = true;
        st.library.roots.add(&a).unwrap();
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    {
        let mut st = state.borrow_mut();
        assert_eq!(st.files, files);
        st.filter = filter::Filter::from_name("Picks").unwrap();
        rebuild_browser(&mut st, &app);
        assert_eq!(st.shown, [0, 1, 2]);
        crate::panel::cull::enter_cull(&mut st, &app, 1);
        st.cull_move_on = true;
    }
    (dir, files, writer, app, state, worker)
}

/// With move-on and the Picks filter, an unflag key on a frame ends
/// on the same frame whether the key applied at once or waited for
/// its read: in the middle of the list the next frame (the step),
/// and on the last frame the nearest one shown (§123's rule), which
/// the landing may choose there since the step had nowhere to go.
#[test]
fn a_queued_key_on_the_last_frame_ends_where_the_key_applied_now_would() {
    // At once, the frames loaded: last row.
    let (dir, _files, writer, app, state, worker) = three_picks_culling("end-now");
    for row in 0..3 {
        open_and_land(&state, &app, &worker, row);
    }
    assert_eq!(state.borrow().current, Some(2));
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Flag(meta::Flag::None)
    ));
    {
        let st = state.borrow();
        assert_eq!(st.shown, [0, 1], "f02 left the list");
        assert_eq!(st.current, Some(1), "the nearest shown, once");
    }
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);

    // Queued behind its read: last row.
    let (dir, _files, writer, app, state, worker) = three_picks_culling("end-queued");
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 2, false);
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Flag(meta::Flag::None)
    ));
    assert_eq!(state.borrow().current, Some(2), "nowhere to step");
    crate::rows::land_pending(&state, &app, &worker);
    {
        let st = state.borrow();
        assert_eq!(st.shown, [0, 1]);
        assert_eq!(
            st.current,
            Some(1),
            "the same frame as the key applied at once"
        );
        assert!(st.from_row[1].read, "opened");
        assert!(st.pick_pending.is_none());
    }
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);

    // At once: the middle row.
    let (dir, _files, writer, app, state, worker) = three_picks_culling("middle-now");
    for row in 0..3 {
        open_and_land(&state, &app, &worker, row);
    }
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Flag(meta::Flag::None)
    ));
    {
        let st = state.borrow();
        assert_eq!(st.shown, [0, 2]);
        assert_eq!(st.current, Some(2), "the next frame, once");
    }
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);

    // Queued: the middle row.
    let (dir, _files, writer, app, state, worker) = three_picks_culling("middle-queued");
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Flag(meta::Flag::None)
    ));
    assert_eq!(state.borrow().current, Some(2), "stepped at the press");
    crate::rows::land_pending(&state, &app, &worker);
    {
        let st = state.borrow();
        assert_eq!(st.shown, [0, 2]);
        assert_eq!(st.current, Some(2), "and stays");
    }
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Culling's undo is a request too, queued behind the keys still
/// waiting on their reads: it takes back the last key the user
/// pressed, not the last that landed.
#[test]
fn culling_s_undo_takes_back_the_last_key_pressed_not_the_last_landed() {
    let (dir, files, writer, app, state, worker) = many_under_a_root("rows-undo", 3);
    open_and_land(&state, &app, &worker, 0);
    {
        let mut st = state.borrow_mut();
        crate::panel::cull::enter_cull(&mut st, &app, 1);
        st.cull_move_on = true;
    }
    // 5 on f00, loaded: at once, and on to f01, whose read goes out.
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Rating(5)
    ));
    {
        let st = state.borrow();
        assert_eq!(st.sidecars[0].meta.rating, 5);
        assert_eq!(st.current, Some(1));
        assert!(!st.from_row[1].read);
    }
    // 1 on f01: queued behind its read. Then undo: queued behind
    // that.
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Rating(1)
    ));
    app.invoke_undo();
    {
        let st = state.borrow();
        assert_eq!(
            st.sidecars[0].meta.rating, 5,
            "not taken back ahead of its turn"
        );
        // The 1 moved the pick on to f02 at its press, as a key does.
        assert_eq!(
            st.loads.whats(),
            ["open of f01.tif", "rating key", "open of f02.tif", "undo"]
        );
    }
    crate::rows::land_pending(&state, &app, &worker);
    {
        let st = state.borrow();
        assert_eq!(
            st.sidecars[1].meta.rating, 0,
            "the 1 applied, then taken back"
        );
        assert_eq!(st.sidecars[0].meta.rating, 5, "the 5 stands");
        assert_eq!(st.current, Some(1), "over to the frame put back, not f00");
        assert!(st.loads.queue.is_empty());
        assert!(
            !app.get_status().starts_with("reading"),
            "the abandoned pick's line is gone: {}",
            app.get_status()
        );
    }
    assert_eq!(Sidecar::load(&files[0]).unwrap().unwrap().meta.rating, 5);
    assert_eq!(Sidecar::load(&files[1]).unwrap().unwrap().meta.rating, 0);
    // Redo brings the 1 back, at once now.
    app.invoke_redo();
    assert_eq!(state.borrow().sidecars[1].meta.rating, 1);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A burst of passes reported while a read is out: one read is out
/// at a time, the burst asks for one more after it, and that one is
/// merged once.
#[test]
fn a_burst_of_reports_is_one_read_and_one_merge() {
    let dir = scratch("burst");
    let root = dir.join("root");
    let days: Vec<PathBuf> = (0..6).map(|i| root.join(format!("day{i}"))).collect();
    for d in &days {
        std::fs::create_dir_all(d).unwrap();
        frames(d, &["a.tif"]);
    }
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&root, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.library.roots.add(&root).unwrap();
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().files.len(), 6);

    // A file copied into the first folder and its pass said: a read
    // goes out. Then one into each of the others, each pass said
    // while that read is out.
    frames(&days[0], &["b.tif"]);
    let report = writer.index_folder(&days[0], &mut |_| {}).unwrap();
    assert!(background_done(
        &state, &app, &days[0], &report, false, false
    ));
    assert_eq!(sent(), 1, "the first report sends a read");
    for d in &days[1..] {
        frames(d, &["b.tif"]);
        let report = writer.index_folder(d, &mut |_| {}).unwrap();
        assert!(background_done(&state, &app, d, &report, false, false));
    }
    assert_eq!(sent(), 1, "the rest wait for it");
    assert!(state.borrow().library.stale);

    // The read out lands and what it saw is merged; the burst's one
    // read after it goes out, and is merged once.
    state.borrow_mut().library.painted = None;
    let look = SENT.with(|s| s.borrow_mut().pop()).unwrap();
    land(&state, &app, &worker, look.run());
    assert!(state.borrow().library.painted.is_some(), "merged");
    assert_eq!(state.borrow().files.len(), 7);
    assert_eq!(sent(), 1, "one more read for the burst");
    state.borrow_mut().library.painted = None;
    assert_eq!(land_all(&state, &app, &worker), 1, "and none after it");
    let st = state.borrow();
    assert!(st.library.painted.is_some(), "merged");
    assert_eq!(st.files.len(), 12);
    assert!(!st.library.merging && !st.library.stale);
    drop(st);
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// M1: a capture of an empty all-roots view ends, failed, with a
/// status saying so, rather than wait for a picture that will not
/// come: the only root offline, or a root with nothing in it once
/// the launch pass is done.
#[test]
fn a_capture_of_an_empty_view_ends() {
    let dir = scratch("empty-view");
    let (gone, empty) = (dir.join("unplugged"), dir.join("empty"));
    std::fs::create_dir_all(&gone).unwrap();
    std::fs::create_dir_all(&empty).unwrap();
    let db = dir.join("index").join("library.sqlite");
    drop(greycard_library::Library::open(&db).unwrap());
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.batch = true;
        st.library.awaiting = true;
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.library.roots.add(&gone).unwrap();
    }
    std::fs::remove_dir(&gone).unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    {
        let st = state.borrow();
        assert!(st.failed, "the capture ends");
        assert!(!st.library.awaiting);
    }
    assert!(app.get_status().contains("offline"), "{}", app.get_status());

    // A fresh library over an empty root: the view waits for the
    // launch pass, and ends when the pass brings nothing.
    {
        let mut st = state.borrow_mut();
        st.failed = false;
        st.library.awaiting = true;
        st.library.roots = Roots::default();
        st.library.roots.add(&empty).unwrap();
        st.library.launch_left = 1;
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert!(!state.borrow().failed, "the pass is still to come");
    let report = greycard_library::Report::default();
    background_done(&state, &app, &empty, &report, true, false);
    assert!(state.borrow().failed, "nothing came of the pass");
    assert!(app.get_status().contains("nothing"), "{}", app.get_status());
    state.borrow_mut().index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// M2: a pass that found one file changed asks for that file's
/// picture again, and no other under the root.
#[test]
fn a_changed_file_asks_for_its_own_picture_only() {
    let dir = scratch("changed-rows");
    let files = frames(&dir, &["a.tif", "b.tif", "c.tif"]);
    let app = window(3);
    let (state, _worker) = state_for(&app, files.clone());
    let report = greycard_library::Report {
        changed: 1,
        changed_files: vec![greycard_library::key_path(&files[1])],
        ..greycard_library::Report::default()
    };
    assert_eq!(changed_rows(&state.borrow(), &report), [1]);
    assert!(changed_rows(&state.borrow(), &greycard_library::Report::default()).is_empty());
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// M3: the frame on screen, the last file in its folder, deleted.
/// The folder is left empty, which the index takes for a drive not
/// mounted and marks nothing; the window still lets the frame go,
/// and opens the nearest, rather than keep a frame whose next save
/// would write a sidecar beside nothing.
#[test]
fn the_last_frame_of_a_folder_deleted_goes_from_the_window() {
    let dir = scratch("last-gone");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    frames(&a, &["x.tif", "y.tif"]);
    let lone = frames(&b, &["z.tif"]).remove(0);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let listed = writer.paths_under(std::slice::from_ref(&dir)).unwrap();
    let app = window(3);
    let (state, worker) = state_for(&app, listed.clone());
    {
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.library.roots.add(&dir).unwrap();
        st.view = View::Roots(None);
        crate::library::refresh_ids(&mut st);
        st.current = listed.iter().position(|p| *p == lone);
        rebuild_browser(&mut st, &app);
    }
    std::fs::remove_file(&lone).unwrap();
    let report = writer.index_folder(&b, &mut |_| {}).unwrap();
    assert_eq!(report.missing, 0, "the index keeps the row: {report:?}");
    assert!(!report.unavailable.is_empty());
    background_done(&state, &app, &b, &report, false, false);
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_ne!(st.current.and_then(|c| st.files.get(c)), Some(&lone));
        assert!(!st.files.contains(&lone), "{:?}", st.files);
    }
    assert!(!lone.with_extension("tif.gcd").exists());
    state.borrow_mut().index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A root with two folders, `d` (two frames) and `e` (one), indexed,
/// and the all-roots view of it landed: what the review's repros
/// start from.
struct Shoot {
    dir: PathBuf,
    d: PathBuf,
    e: PathBuf,
    writer: greycard_library::Library,
    app: App,
    state: Rc<RefCell<State>>,
    worker: Rc<Worker>,
}

impl Shoot {
    fn new(what: &str) -> Shoot {
        let dir = scratch(what);
        let root = dir.join("root");
        let (d, e) = (root.join("d"), root.join("e"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::create_dir_all(&e).unwrap();
        frames(&d, &["d1.tif", "d2.tif"]);
        frames(&e, &["e1.tif"]);
        let db = dir.join("index").join("library.sqlite");
        let mut writer = greycard_library::Library::open(&db).unwrap();
        writer.index_tree(&root, &mut |_| {}).unwrap();
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.index_path = Some(db.clone());
            st.library.roots.add(&root).unwrap();
        }
        open_view(&state, &app, &worker, View::Roots(None));
        land_all(&state, &app, &worker);
        assert_eq!(state.borrow().files.len(), 3);
        Shoot {
            dir,
            d,
            e,
            writer,
            app,
            state,
            worker,
        }
    }

    /// A frame copied into `e` and its pass said: a read goes out,
    /// and is handed back unrun.
    fn copy_into_e(&mut self, name: &str) -> Look {
        frames(&self.e, &[name]);
        let report = self.writer.index_folder(&self.e, &mut |_| {}).unwrap();
        folders_passed(&mut self.state.borrow_mut(), &self.e);
        assert!(background_done(
            &self.state,
            &self.app,
            &self.e,
            &report,
            false,
            false
        ));
        SENT.with(|s| s.borrow_mut().pop()).unwrap()
    }

    // Only the tests that lock a folder count d's frames, and
    // those run on unix alone.
    #[cfg(unix)]
    fn in_d(&self) -> usize {
        let st = self.state.borrow();
        st.files.iter().filter(|f| f.starts_with(&self.d)).count()
    }

    fn done(self) {
        self.state.borrow_mut().index_reader = None;
        drop(self.state);
        drop(self.writer);
        crate::testing::remove_dir_retry(&self.dir);
    }
}

/// The review's repro: a read out while `d` is briefly unreadable,
/// then a pass over `d` (readable again, nothing changed) landing
/// before the read. `d`'s frames come back with the read the pass
/// has sent after it, not only at some later pass.
#[cfg(unix)]
#[test]
fn a_pass_during_a_read_that_saw_a_blip_brings_the_folder_back() {
    use std::os::unix::fs::PermissionsExt;
    let mut s = Shoot::new("rv-epoch");
    folders_passed(&mut s.state.borrow_mut(), &s.d);
    let look = s.copy_into_e("e2.tif");
    std::fs::set_permissions(&s.d, std::fs::Permissions::from_mode(0o000)).unwrap();
    let found = look.run();
    std::fs::set_permissions(&s.d, std::fs::Permissions::from_mode(0o755)).unwrap();
    let report = s.writer.index_folder(&s.d, &mut |_| {}).unwrap();
    assert!(report.errors.is_empty());
    folders_passed(&mut s.state.borrow_mut(), &s.d);
    background_done(&s.state, &s.app, &s.d, &report, false, false);
    land(&s.state, &s.app, &s.worker, found);
    land_all(&s.state, &s.app, &s.worker);
    assert_eq!(s.in_d(), 2, "d is readable");
    let look = s.copy_into_e("e3.tif");
    land(&s.state, &s.app, &s.worker, look.run());
    land_all(&s.state, &s.app, &s.worker);
    assert_eq!(s.in_d(), 2);
    s.done();
}

/// The review's repro: a read that finds `d` unreadable for a moment,
/// with no pass over `d` at all. The next read looks at it again,
/// and its frames are back.
#[cfg(unix)]
#[test]
fn a_folder_unreadable_for_a_moment_is_back_at_the_next_read() {
    use std::os::unix::fs::PermissionsExt;
    let mut s = Shoot::new("rv-blip");
    folders_passed(&mut s.state.borrow_mut(), &s.d);
    let look = s.copy_into_e("e2.tif");
    std::fs::set_permissions(&s.d, std::fs::Permissions::from_mode(0o000)).unwrap();
    let found = look.run();
    std::fs::set_permissions(&s.d, std::fs::Permissions::from_mode(0o755)).unwrap();
    land(&s.state, &s.app, &s.worker, found);
    land_all(&s.state, &s.app, &s.worker);
    for k in 3..5 {
        let look = s.copy_into_e(&format!("e{k}.tif"));
        land(&s.state, &s.app, &s.worker, look.run());
        land_all(&s.state, &s.app, &s.worker);
    }
    assert_eq!(s.in_d(), 2, "d is readable again, and its frames are back");
    s.done();
}

/// The review's check: a rating made in memory while a read is out
/// survives the read's landing, and the frame on screen stays.
#[test]
fn an_edit_made_while_a_read_is_out_survives_its_landing() {
    let mut s = Shoot::new("rv-edit");
    s.state.borrow_mut().write_sidecars = true;
    let d1 = s.d.join("d1.tif");
    let mut on_disk = Sidecar::default();
    on_disk.meta.rating = 1;
    on_disk.save(&d1).unwrap();
    let found = s.copy_into_e("e2.tif").run();
    {
        let mut st = s.state.borrow_mut();
        let i = st.files.iter().position(|f| *f == d1).unwrap();
        st.current = Some(i);
        st.sidecars[i].meta.rating = 5;
    }
    land(&s.state, &s.app, &s.worker, found);
    land_all(&s.state, &s.app, &s.worker);
    {
        let st = s.state.borrow();
        let i = st.files.iter().position(|f| *f == d1).unwrap();
        assert_eq!(st.files.len(), 4);
        assert_eq!(st.sidecars[i].meta.rating, 5, "the edit in memory");
        assert_eq!(st.current.map(|c| st.files[c].clone()), Some(d1));
    }
    s.done();
}

/// The review's check: the frame on screen deleted while a read that
/// saw it there is out. The landing keeps it; the pass over its
/// folder that the deletion brings lets it go.
#[test]
fn the_frame_on_screen_deleted_while_a_read_is_out_goes_at_the_next() {
    let mut s = Shoot::new("rv-del");
    let d1 = s.d.join("d1.tif");
    {
        let mut st = s.state.borrow_mut();
        let i = st.files.iter().position(|f| *f == d1).unwrap();
        st.current = Some(i);
    }
    let found = s.copy_into_e("e2.tif").run();
    std::fs::remove_file(&d1).unwrap();
    land(&s.state, &s.app, &s.worker, found);
    assert!(s.state.borrow().files.contains(&d1), "as the read saw it");
    let report = s.writer.index_folder(&s.d, &mut |_| {}).unwrap();
    folders_passed(&mut s.state.borrow_mut(), &s.d);
    background_done(&s.state, &s.app, &s.d, &report, false, false);
    land_all(&s.state, &s.app, &s.worker);
    {
        let st = s.state.borrow();
        assert!(!st.files.contains(&d1));
        assert_ne!(st.current.map(|c| st.files[c].clone()), Some(d1));
    }
    s.done();
}

/// The watchers asked for, taken out of the queue.
fn builds() -> Vec<Plan> {
    BUILDS.with(|b| b.borrow_mut().drain(..).collect())
}

/// Say what a root is on: a network mount's type, or none for a
/// local disk, whatever the temporary directory is really on.
fn on(root: &Path, fs: Option<&str>) {
    REMOTE.with(|r| {
        r.borrow_mut()
            .insert(root.to_path_buf(), fs.map(str::to_owned))
    });
}

/// A window with an indexer of its own over a library in `dir`, and
/// `roots` as its roots, kept in memory alone, each on a local disk.
fn watched_window(dir: &Path, roots: &[PathBuf]) -> (App, Rc<RefCell<State>>, Rc<Worker>) {
    for root in roots {
        on(root, None);
    }
    let app = window(3);
    let (state, worker) = state_for(&app, Vec::new());
    let db = dir.join("data").join("library.sqlite");
    let indexer = crate::library::Indexer::start(db, |_| {}).expect("the indexer starts");
    {
        let mut st = state.borrow_mut();
        st.index = Some(indexer);
        st.library.roots = Roots::from_list(roots.to_vec());
    }
    (app, state, worker)
}

fn leave(state: &Rc<RefCell<State>>, dir: &Path) {
    let mut st = state.borrow_mut();
    st.library.watcher = None;
    st.library.poll = None;
    if let Some(indexer) = st.index.take() {
        indexer.stop(Duration::from_secs(20));
    }
    drop(st);
    let _ = std::fs::remove_dir_all(dir);
}

fn watched(st: &State) -> Vec<PathBuf> {
    st.library
        .watcher
        .as_ref()
        .map(|w| w.watched.clone())
        .unwrap_or_default()
}

/// The watcher is built off the window's thread, and a build for a
/// set of roots that has changed since it was asked for is dropped
/// when it lands, not put over the newer one.
#[test]
fn a_watcher_built_for_roots_since_changed_is_dropped() {
    let dir = scratch("stale-watch");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let (app, state, _worker) = watched_window(&dir, std::slice::from_ref(&a));
    on(&b, None);
    watch(&mut state.borrow_mut(), &app);
    // Nothing is watched until the build lands.
    assert!(state.borrow().library.watcher.is_none());
    state.borrow_mut().library.roots = Roots::from_list(vec![a.clone(), b.clone()]);
    watch(&mut state.borrow_mut(), &app);
    let plans = builds();
    assert_eq!(plans.len(), 2);
    let mut plans = plans.into_iter();
    let (first, second) = (plans.next().unwrap(), plans.next().unwrap());
    // The newer lands first, then the older, late.
    assert!(build_in_place(&mut state.borrow_mut(), second));
    assert_eq!(watched(&state.borrow()), vec![a.clone(), b.clone()]);
    assert!(!build_in_place(&mut state.borrow_mut(), first));
    assert_eq!(
        watched(&state.borrow()),
        vec![a, b],
        "the older build is not put over the newer"
    );
    leave(&state, &dir);
}

/// A root taken out while the watcher is being built: the build
/// asked for before is not put in place when it lands. The watcher
/// there is kept until the new one lands, so the roots that stay
/// are watched all along, and the new one watches only them.
#[test]
fn a_root_taken_out_during_a_build_is_not_watched_after() {
    let dir = scratch("remove-watch");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let (app, state, worker) = watched_window(&dir, &[a.clone(), b.clone()]);
    watch(&mut state.borrow_mut(), &app);
    let before = builds();
    assert!(build_in_place(
        &mut state.borrow_mut(),
        before.into_iter().next().unwrap()
    ));
    assert_eq!(watched(&state.borrow()), vec![a.clone(), b.clone()]);
    // A new build out (the roots changed), then `b` taken out
    // before it lands.
    watch(&mut state.borrow_mut(), &app);
    remove(&state, &app, &worker, &b);
    assert_eq!(
        watched(&state.borrow()),
        vec![a.clone(), b.clone()],
        "the old watcher stays until the new one is ready"
    );
    let plans = builds();
    assert_eq!(plans.len(), 2);
    let mut plans = plans.into_iter();
    let (stale, now) = (plans.next().unwrap(), plans.next().unwrap());
    assert!(!build_in_place(&mut state.borrow_mut(), stale));
    assert_eq!(watched(&state.borrow()), vec![a.clone(), b.clone()]);
    assert!(build_in_place(&mut state.borrow_mut(), now));
    assert_eq!(watched(&state.borrow()), vec![a]);
    leave(&state, &dir);
}

/// The launch's look at the roots, landing after a newer build's,
/// still asks for the launch pass over the roots still there, but
/// does not put its offline roots over the newer build's.
#[test]
fn a_late_launch_look_does_not_undo_a_newer_one() {
    let dir = scratch("late-launch");
    let (a, gone) = (dir.join("a"), dir.join("gone"));
    std::fs::create_dir_all(&a).unwrap();
    let (app, state, _worker) = watched_window(&dir, &[a.clone(), gone.clone()]);
    start(&mut state.borrow_mut(), &app);
    assert!(state.borrow().library.starting);
    state.borrow_mut().library.roots = Roots::from_list(vec![a.clone()]);
    watch(&mut state.borrow_mut(), &app);
    let mut plans = builds().into_iter();
    let (launch, newer) = (plans.next().unwrap(), plans.next().unwrap());
    assert!(build_in_place(&mut state.borrow_mut(), newer));
    assert!(state.borrow().library.offline.is_empty());
    assert!(!build_in_place(&mut state.borrow_mut(), launch));
    let st = state.borrow();
    assert!(!st.library.starting);
    assert!(st.library.offline.is_empty(), "{:?}", st.library.offline);
    assert_eq!(st.library.launch_left, 1, "the pass over a, not gone");
    drop(st);
    leave(&state, &dir);
}

/// A root added: made canonical and looked at before it is kept
/// (in place in a test), then its pass asked for and its watcher
/// built off the window's thread.
#[test]
fn a_root_added_is_watched_when_its_build_lands() {
    let dir = scratch("add-watch");
    let a = dir.join("a");
    std::fs::create_dir_all(a.join("day")).unwrap();
    let (app, state, worker) = watched_window(&dir, &[]);
    on(&a, None);
    // Through a path that is not canonical.
    add(&state, &app, &worker, &a.join("day").join(".."));
    assert_eq!(
        state.borrow().library.roots.list(),
        std::slice::from_ref(&a)
    );
    assert!(state.borrow().library.watcher.is_none());
    let plans = builds();
    assert_eq!(plans.len(), 1);
    assert!(build_in_place(
        &mut state.borrow_mut(),
        plans.into_iter().next().unwrap()
    ));
    assert_eq!(watched(&state.borrow()), vec![a]);
    // A folder that is not there is refused, and nothing is built.
    add(&state, &app, &worker, &dir.join("nowhere"));
    assert!(app.get_status().starts_with("not added"));
    assert!(builds().is_empty());
    leave(&state, &dir);
}

/// A root taken out by a path the list does not have as it is (a
/// link to it): the window's thread only sends the path off, and
/// the root goes when the look made off it lands. A path that does
/// not answer is said, and nothing is taken out.
#[cfg(unix)]
#[test]
fn a_remove_by_an_unseen_path_is_made_canonical_off_the_window_s_thread() {
    let dir = scratch("remove-unseen");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let link = dir.join("to-a");
    std::os::unix::fs::symlink(&a, &link).unwrap();
    let (app, state, worker) = watched_window(&dir, &[a.clone(), b.clone()]);
    HOLD_CHECKS.with(|h| *h.borrow_mut() = true);
    remove(&state, &app, &worker, &link);
    assert_eq!(
        state.borrow().library.roots.list(),
        [a.clone(), b.clone()],
        "nothing changed on the window's thread"
    );
    assert_eq!(
        CHECKS_SENT.with(|c| c.borrow().clone()),
        vec![(link.clone(), Check::Remove)]
    );
    assert!(builds().is_empty());
    assert_eq!(run_checks(&state, &app, &worker), 1);
    assert_eq!(
        state.borrow().library.roots.list(),
        std::slice::from_ref(&b)
    );
    assert!(app.get_status().contains("is out of the library"));
    // A path that does not answer: said, b kept.
    builds();
    NOT_ANSWERING.with(|n| n.borrow_mut().push(dir.join("share")));
    remove(&state, &app, &worker, &dir.join("share").join("b"));
    assert_eq!(run_checks(&state, &app, &worker), 1);
    assert_eq!(
        state.borrow().library.roots.list(),
        std::slice::from_ref(&b)
    );
    assert!(
        app.get_status().contains("timed out after 3 s"),
        "{}",
        app.get_status()
    );
    // The list's own spelling goes at once, nothing sent.
    remove(&state, &app, &worker, &b);
    assert!(state.borrow().library.roots.is_empty());
    assert_eq!(run_checks(&state, &app, &worker), 0);
    HOLD_CHECKS.with(|h| *h.borrow_mut() = false);
    NOT_ANSWERING.with(|n| n.borrow_mut().clear());
    builds();
    leave(&state, &dir);
}

/// "Add this folder" over a folder no read has made canonical (here,
/// reached through a link): offered from the path as listed, with no
/// look on the window's thread, and added canonical once the look
/// made off it lands. Through a link into a root it is said to be
/// covered. A folder that does not answer is said, and not added.
#[cfg(unix)]
#[test]
fn add_this_folder_on_an_unseen_folder_looks_off_the_window_s_thread() {
    let dir = scratch("add-unseen");
    let real = dir.join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = dir.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let (app, state, worker) = watched_window(&dir, &[]);
    on(&real, None);
    {
        let mut st = state.borrow_mut();
        st.files = vec![link.join("x.tif")];
        assert!(st.library.canonical.is_empty(), "no read has seen it");
    }
    show(&state.borrow(), &app);
    assert!(app.get_library_can_add_open());
    HOLD_CHECKS.with(|h| *h.borrow_mut() = true);
    app.invoke_library_root_add_open();
    assert!(state.borrow().library.roots.is_empty());
    assert_eq!(
        CHECKS_SENT.with(|c| c.borrow().clone()),
        vec![(link.clone(), Check::Add)]
    );
    assert_eq!(run_checks(&state, &app, &worker), 1);
    assert_eq!(
        state.borrow().library.roots.list(),
        std::slice::from_ref(&real)
    );
    builds();
    // Now under a root, through the same unseen link: still offered
    // (the window does not know where the link goes), and the add
    // says it is covered.
    show(&state.borrow(), &app);
    assert!(app.get_library_can_add_open());
    app.invoke_library_root_add_open();
    assert_eq!(run_checks(&state, &app, &worker), 1);
    assert!(
        app.get_status().starts_with("already in the library"),
        "{}",
        app.get_status()
    );
    // A read that made it canonical: not offered.
    state
        .borrow_mut()
        .library
        .canonical
        .insert(link.clone(), real.clone());
    show(&state.borrow(), &app);
    assert!(!app.get_library_can_add_open());
    // A folder that does not answer.
    NOT_ANSWERING.with(|n| n.borrow_mut().push(dir.join("share")));
    add(&state, &app, &worker, &dir.join("share"));
    assert_eq!(run_checks(&state, &app, &worker), 1);
    assert!(
        app.get_status().starts_with("not added") && app.get_status().contains("3 s"),
        "{}",
        app.get_status()
    );
    assert_eq!(
        state.borrow().library.roots.list(),
        std::slice::from_ref(&real)
    );
    HOLD_CHECKS.with(|h| *h.borrow_mut() = false);
    NOT_ANSWERING.with(|n| n.borrow_mut().clear());
    builds();
    leave(&state, &dir);
}

/// A root on a network mount is not watched: it is said once, and
/// passed over on the timer when there is one. The others are
/// watched as ever. On Windows it is watched, and not polled.
#[test]
fn a_root_on_a_network_mount_is_not_watched_but_polled() {
    let dir = scratch("remote-watch");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let (app, state, worker) = watched_window(&dir, &[a.clone(), b.clone()]);
    on(&b, Some("nfs4"));
    state.borrow_mut().library.poll_every = Some(Duration::from_secs(600));
    watch(&mut state.borrow_mut(), &app);
    let plan = builds().into_iter().next().unwrap();
    assert!(build_in_place(&mut state.borrow_mut(), plan));
    {
        let st = state.borrow();
        if WATCH_REMOTE {
            assert_eq!(watched(&st), vec![a.clone(), b.clone()]);
            assert!(st.library.poll.is_none());
        } else {
            assert_eq!(watched(&st), vec![a.clone()]);
            assert!(st.library.poll.is_some());
        }
        assert_eq!(st.library.remote, vec![(b.clone(), "nfs4".to_string())]);
        assert!(st.library.said_remote.contains(&b));
    }
    // The network root taken out: its timer goes at once, before
    // the new watcher lands. Put back for the rest.
    remove(&state, &app, &worker, &b);
    assert!(state.borrow().library.poll.is_none());
    assert!(state.borrow().library.remote.is_empty());
    builds();
    state.borrow_mut().library.roots = Roots::from_list(vec![a.clone(), b.clone()]);
    // With the timer off, none; and with no root on a network
    // mount, none either.
    state.borrow_mut().library.poll_every = None;
    watch(&mut state.borrow_mut(), &app);
    let plan = builds().into_iter().next().unwrap();
    assert!(build_in_place(&mut state.borrow_mut(), plan));
    assert!(state.borrow().library.poll.is_none());
    on(&b, None);
    state.borrow_mut().library.poll_every = Some(Duration::from_secs(600));
    watch(&mut state.borrow_mut(), &app);
    let plan = builds().into_iter().next().unwrap();
    assert!(build_in_place(&mut state.borrow_mut(), plan));
    {
        let st = state.borrow();
        assert_eq!(watched(&st), vec![a, b]);
        assert!(st.library.remote.is_empty());
        assert!(st.library.poll.is_none(), "a local root is never polled");
    }
    leave(&state, &dir);
}

/// A share mounted below a local root is left out of the root's
/// watch and passed over on the timer, as a network root is; the
/// rest of the root is watched. Taking the root out takes the mount
/// off the timer at once.
#[test]
fn a_network_mount_below_a_local_root_is_not_watched_but_polled() {
    let dir = scratch("remote-below");
    let (a, b) = (dir.join("a"), dir.join("b"));
    let nas = a.join("NAS");
    std::fs::create_dir_all(nas.join("2026")).unwrap();
    std::fs::create_dir_all(a.join("day")).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let (app, state, worker) = watched_window(&dir, &[a.clone(), b.clone()]);
    let auto = a.join("Later");
    std::fs::create_dir_all(&auto).unwrap();
    REMOTE_UNDER.with(|r| {
        r.borrow_mut().insert(
            a.clone(),
            vec![
                (nas.clone(), Some("cifs".to_string())),
                (auto.clone(), None),
            ],
        )
    });
    state.borrow_mut().library.poll_every = Some(Duration::from_secs(600));
    watch(&mut state.borrow_mut(), &app);
    let plan = builds().into_iter().next().unwrap();
    assert!(build_in_place(&mut state.borrow_mut(), plan));
    {
        let st = state.borrow();
        let watcher = st.library.watcher.as_ref().expect("a watcher");
        if WATCH_REMOTE {
            assert!(watcher.except.is_empty());
            assert!(st.library.poll.is_none());
        } else {
            assert_eq!(watcher.watched, vec![a.clone(), b.clone()]);
            // The automount not mounted yet left out of the watch
            // too, and not polled: what it mounts is not known.
            assert_eq!(watcher.except, vec![nas.clone(), auto.clone()]);
            assert_eq!(st.library.remote, vec![(nas.clone(), "cifs".to_string())]);
            let poll = st.library.poll.as_ref().expect("the mount is polled");
            assert_eq!(poll.roots, std::slice::from_ref(&nas));
            assert!(st.library.said_remote.contains(&nas));
        }
    }
    // The root taken out: the mount below it goes off the timer at
    // once, before the new watcher lands.
    remove(&state, &app, &worker, &a);
    assert!(state.borrow().library.poll.is_none());
    assert!(state.borrow().library.remote.is_empty());
    builds();
    REMOTE_UNDER.with(|r| r.borrow_mut().clear());
    leave(&state, &dir);
}

/// An automount below a local root not mounted yet is looked into by
/// the build, which mounts it, and classified by what it mounted: a
/// share is left out and polled, a local disk is watched with its
/// root, and one that does not answer is left out and not polled.
#[test]
fn an_automount_below_a_root_is_mounted_on_purpose_and_classified() {
    if WATCH_REMOTE {
        return;
    }
    let dir = scratch("automount");
    let a = dir.join("a");
    let (share, disk, dead) = (a.join("Archive"), a.join("Card"), a.join("Gone"));
    for d in [&share, &disk, &dead] {
        std::fs::create_dir_all(d).unwrap();
    }
    let (app, state, _worker) = watched_window(&dir, std::slice::from_ref(&a));
    REMOTE_UNDER.with(|r| {
        r.borrow_mut().insert(
            a.clone(),
            vec![
                (share.clone(), None),
                (disk.clone(), None),
                (dead.clone(), None),
            ],
        )
    });
    AFTER_LOOK.with(|l| {
        let mut l = l.borrow_mut();
        l.insert(share.clone(), Some(Some("nfs4".to_string())));
        l.insert(disk.clone(), None);
    });
    NOT_ANSWERING.with(|n| n.borrow_mut().push(dead.clone()));
    LOOKED.with(|l| l.borrow_mut().clear());
    state.borrow_mut().library.poll_every = Some(Duration::from_secs(600));
    watch(&mut state.borrow_mut(), &app);
    let plan = builds().into_iter().next().unwrap();
    assert!(build_in_place(&mut state.borrow_mut(), plan));
    {
        let st = state.borrow();
        let looked = LOOKED.with(|l| l.borrow().clone());
        for d in [&share, &disk, &dead] {
            assert!(looked.contains(d), "{} looked into", d.display());
        }
        let watcher = st.library.watcher.as_ref().expect("a watcher");
        assert_eq!(watcher.except, vec![share.clone(), dead.clone()]);
        assert_eq!(st.library.remote, vec![(share.clone(), "nfs4".to_string())]);
        let poll = st.library.poll.as_ref().expect("the share is polled");
        assert_eq!(poll.roots, std::slice::from_ref(&share));
    }
    REMOTE_UNDER.with(|r| r.borrow_mut().clear());
    AFTER_LOOK.with(|l| l.borrow_mut().clear());
    NOT_ANSWERING.with(|n| n.borrow_mut().clear());
    leave(&state, &dir);
}

/// The timer's body, its clock a counter: a tree pass over each
/// network root that answers at each tick, and none over one that
/// does not; it stops when told.
#[test]
fn the_timer_passes_over_the_network_roots_that_answer() {
    let (a, b) = (PathBuf::from("/mnt/nas/a"), PathBuf::from("/mnt/nas/b"));
    let mut ticks = 0;
    let wait = || {
        ticks += 1;
        ticks <= 3
    };
    let away: HashSet<PathBuf> = [b.clone()].into();
    let asked = RefCell::new(Vec::new());
    let n = poll(
        &[a.clone(), b.clone()],
        wait,
        |_| away.clone(),
        &|changes| asked.borrow_mut().push(changes),
    );
    assert_eq!(n, 3);
    let asked = asked.into_inner();
    assert_eq!(asked.len(), 3);
    for batch in asked {
        assert_eq!(batch, vec![a.clone()]);
    }
    // All of them away: nothing asked.
    let mut once = true;
    let n = poll(
        std::slice::from_ref(&b),
        || std::mem::replace(&mut once, false),
        |_| away.clone(),
        &|_| panic!("nothing to pass over"),
    );
    assert_eq!(n, 0);
}

/// The timer on its own thread: it asks, and it stops when dropped.
#[test]
fn the_timer_asks_and_stops_when_dropped() {
    let dir = scratch("poll");
    let (tx, rx) = std::sync::mpsc::channel();
    let poll = Poll::start(vec![dir.clone()], Duration::from_millis(20), move |c| {
        let _ = tx.send(c);
    })
    .expect("a thread");
    let first = rx.recv_timeout(Duration::from_secs(10)).expect("a pass");
    assert_eq!(first, vec![dir.clone()]);
    drop(poll);
    // The thread goes, and its end of the channel with it.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            _ => assert!(Instant::now() < deadline, "the timer still runs"),
        }
    }
    crate::testing::remove_dir_retry(&dir);
}
