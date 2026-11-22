//! Tests of the roots pane: its rows, its menus and the keys over it.

use slint::Model;

use super::*;

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
    st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
    st.views.library.roots.add(&a).unwrap();
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
    let (count, lit, _) = root_row(&app, &a);
    assert_eq!((count.as_str(), lit), ("3", false));
    assert_eq!(all_roots_row(&app).0, "3");
    assert_eq!(
        crate::testing::count_id(&app, "RootsSection::add-open-link"),
        1
    );
    let mut st = state.borrow_mut();
    st.views.library.roots.add(&b).unwrap();
    st.views.view = View::Roots(None);
    recount(&mut st);
    show(&st, &app);
    assert_eq!(app.get_library_all_count(), 4);
    assert!(app.get_library_all_on());
    assert!(!app.get_library_can_add_open());
    assert_eq!(app.get_library_note(), "");
    drop(st);
    let (count, lit, _) = all_roots_row(&app);
    assert_eq!((count.as_str(), lit), ("4", true));
    assert_eq!(root_row(&app, &b).0, "1");
    assert!(!root_row(&app, &b).1);
    assert_eq!(
        crate::testing::count_id(&app, "RootsSection::add-open-link"),
        0,
        "b is a root now"
    );
    // One root's view: its row lit, All roots not.
    let mut st = state.borrow_mut();
    st.views.view = View::Roots(Some(a.clone()));
    show(&st, &app);
    drop(st);
    assert!(root_row(&app, &a).1);
    assert!(!all_roots_row(&app).1);
    let mut st = state.borrow_mut();
    // A name given is the chip's, and the path is still the root.
    st.views.library.roots.set_name(&a, "Archive");
    assert_eq!(root_name(&st.views.library.roots, &a), "Archive");
    assert_eq!(root_name(&st.views.library.roots, &b), "b");
    show(&st, &app);
    let chip = app.get_library_roots().row_data(0).unwrap();
    assert_eq!(chip.name.as_str(), "Archive");
    assert_eq!(chip.path.as_str(), a.to_str().unwrap());
    st.views.index_reader = None;
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
        st.views.library.roots.add(&a).unwrap();
        st.views.view = View::Roots(Some(a.clone()));
        show(&st, &app);
        assert!(app.get_library_roots().row_data(0).unwrap().on);
    }
    crate::panel::browser::open_folder(&state, &app, &worker, &b);
    // The chip stays lit until the folder lands, since the list on
    // screen is still the view's.
    assert_eq!(state.borrow().views.view, View::Roots(Some(a.clone())));
    assert_eq!(land_all(&state, &app, &worker), 1);
    assert_eq!(state.borrow().views.view, View::Folder);
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

/// A root's row in the pane, keyed by the root's path.
const ROOT_ROW: &str = "RootsSection::root-row";

/// The key of the root at `path`: the path as the pane has it.
fn root_key(path: impl AsRef<std::path::Path>) -> String {
    path.as_ref().to_string_lossy().into_owned()
}

/// The middle of the row of the root at `path`.
fn middle_of(app: &App, path: impl AsRef<std::path::Path>) -> (f32, f32) {
    let (at, size) = crate::testing::by_key(app, ROOT_ROW, root_key(path));
    (at.x + size.width / 2.0, at.y + size.height / 2.0)
}

/// A point near the left end of that row: clear of a menu opened
/// from a row's right half, which drops down and to the right of
/// the pointer over the rows below it.
fn left_of(app: &App, path: impl AsRef<std::path::Path>) -> (f32, f32) {
    let (at, size) = crate::testing::by_key(app, ROOT_ROW, root_key(path));
    (at.x + 4.0, at.y + size.height / 2.0)
}

/// The middle of the pane's control `id`.
fn middle_of_id(app: &App, id: &str) -> (f32, f32) {
    let (at, size) = crate::testing::by_id(app, id);
    (at.x + size.width / 2.0, at.y + size.height / 2.0)
}

/// The pane's All roots row.
const ALL_ROOTS: &str = "RootsSection::all-roots";

/// The name the row of the root at `path` shows, as a screen reader
/// is told it: the root's name, data, never the window's words.
fn root_name_shown(app: &App, path: impl AsRef<std::path::Path>) -> String {
    let key = root_key(path);
    crate::testing::keyed(app, ROOT_ROW, &key)
        .unwrap_or_else(|| panic!("no root's row {key:?}"))
        .accessible_label()
        .unwrap_or_default()
        .to_string()
}

/// A root's row in the pane, by the root's path: its count as a
/// screen reader has it, whether it is lit, and its width.
fn root_row(app: &App, path: impl AsRef<std::path::Path>) -> (String, bool, f32) {
    let key = root_key(path);
    let row = crate::testing::keyed(app, ROOT_ROW, &key)
        .unwrap_or_else(|| panic!("no root's row {key:?}"));
    row_facts(&row)
}

/// [`root_row`] of the All roots row.
fn all_roots_row(app: &App) -> (String, bool, f32) {
    crate::testing::count_id(app, ALL_ROOTS);
    let row = i_slint_backend_testing::ElementHandle::find_by_element_id(app, ALL_ROOTS)
        .next()
        .expect("the All roots row");
    row_facts(&row)
}

/// A root's row's count as a screen reader has it, whether it is
/// lit, and its width.
fn row_facts(row: &i_slint_backend_testing::ElementHandle) -> (String, bool, f32) {
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
    // The root's row by its name, the rest by their ids.
    for what in [
        ALL_ROOTS,
        "shoots",
        "RootsSection::add-root-link",
        "RootsSection::add-open-link",
    ] {
        let (x, y) = match what {
            "shoots" => middle_of(&app, "/x/shoots"),
            id => middle_of_id(&app, id),
        };
        assert!(x < 240.0, "{what} is in the pane, at {x}");
        crate::testing::click(&app, x, y);
    }
    assert_eq!(
        *seen.borrow(),
        ["picked ", "picked /x/shoots", "add", "add open"]
    );
    // The count at the row's right, and nothing lit.
    assert_eq!(root_row(&app, "/x/shoots").0, "3");
    assert_eq!(all_roots_row(&app).0, "3");
    assert!(!root_row(&app, "/x/shoots").1);
    // The header keeps none of it: the pane put away takes every
    // root with it, and the loupe's pane has none.
    assert_eq!(crate::testing::count_id(&app, ALL_ROOTS), 1);
    crate::testing::press(&app, slint::platform::Key::F7);
    assert!(app.get_left_hidden());
    assert_eq!(crate::testing::count_id(&app, ALL_ROOTS), 0);
    assert_eq!(crate::testing::count_keyed(&app, ROOT_ROW, "/x/shoots"), 0);
    assert_eq!(
        crate::testing::count_id(&app, "RootsSection::add-root-link"),
        0
    );
    crate::testing::press(&app, slint::platform::Key::F7);
    assert_eq!(crate::testing::count_id(&app, ALL_ROOTS), 1);
    app.set_grid_open(false);
    assert_eq!(crate::testing::count_id(&app, ALL_ROOTS), 0);
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
    assert_eq!(root_row(&app, "/x/r0").0, crate::i18n::text("offline"));
    assert_eq!(root_row(&app, "/x/r1").0, "100");
    let last = "a long shoot folder name that runs on 5";
    let (_, lit, width) = root_row(&app, "/x/r5");
    assert!(lit, "the one open is lit");
    assert!(width <= 240.0, "{width}");
    let (x, y) = middle_of(&app, "/x/r5");
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
    let all = middle_of_id(&app, ALL_ROOTS);
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
    state.borrow_mut().views.library.roots.add(&photos).unwrap();
    show(&state.borrow(), &app);
    slint::platform::update_timers_and_animations();

    let (x, y) = middle_of(&app, &photos);
    right_click(&app, x, y);
    assert!(
        app.get_menu_up(),
        "a right-click on the root's row opens its menu"
    );
    assert_eq!(state.borrow().views.view, View::Folder, "nothing picked");
    assert!(state.borrow().views.library.wanted.is_none());
    // The press that closes it, on the row just left of where the
    // menu opened, is let go by.
    let (x, y) = left_of(&app, &photos);
    crate::testing::click(&app, x, y);
    assert!(!app.get_menu_up());
    assert!(state.borrow().views.library.wanted.is_none(), "not a pick");
    assert_eq!(
        state.borrow().views.library.roots.list(),
        std::slice::from_ref(&photos)
    );
    // The next is a press on the row, which picks its root: its own
    // frames, With subfolders being off.
    crate::testing::click(&app, x, y);
    assert_eq!(
        state.borrow_mut().views.library.wanted.take(),
        Some(View::Branch {
            root: photos.clone(),
            folder: photos.clone(),
            deep: false,
        })
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
    assert_eq!(root_name_shown(&app, &photos), "Archive");

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
    assert_eq!(
        state.borrow().views.library.roots.name(&photos),
        Some("Archive")
    );
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
        assert_eq!(st.views.library.roots.list(), [photos.clone(), b.clone()]);
        assert_eq!(st.views.library.roots.name(&b), None);
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
    let right = |name: &str| {
        let (x, y) = middle_of(&app, format!("/x/{name}"));
        (x + 60.0, y)
    };
    let one = right("one");
    let two = right("two");
    // All roots has no menu.
    let all = middle_of_id(&app, ALL_ROOTS);
    let all = (all.0 + 60.0, all.1);
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

    // Every row, closing the first's menu, does nothing there: the
    // roots' rows by their names, the rest by their ids.
    let rows = [
        ALL_ROOTS,
        "one",
        "two",
        "RootsSection::add-root-link",
        "RootsSection::add-open-link",
    ];
    let left = |what: &str| match what {
        "one" | "two" => left_of(&app, format!("/x/{what}")),
        id => {
            let (at, size) = crate::testing::by_id(&app, id);
            (at.x + 4.0, at.y + size.height / 2.0)
        }
    };
    for label in rows {
        right_click(&app, one.0, one.1);
        assert!(app.get_menu_up());
        let (x, y) = left(label);
        crate::testing::click(&app, x, y);
        assert!(!app.get_menu_up(), "closed on {label}");
    }
    assert!(seen.borrow().is_empty(), "{:?}", seen.borrow());
    // And with no menu up, the same rows answer.
    for label in rows {
        let (x, y) = left(label);
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
    let (at, _) = crate::testing::by_id(&app, "App::import-button");
    assert!(at.x < 240.0, "in the pane");
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
    assert_eq!(*seen.borrow(), ["open", "import"]);

    let line = "Copying 3 of 10";
    app.set_import_running(true);
    app.set_import_status(line.into());
    let (pane_line, header_line) = ("App::import-status-line", "GridSheet::import-line-text");
    assert_eq!(crate::testing::count_id(&app, pane_line), 1, "the pane's");
    assert_eq!(crate::testing::label_of(&app, pane_line), line);
    assert_eq!(crate::testing::count_id(&app, header_line), 0);
    // The pane's one button, its Stop while an import runs.
    assert_eq!(crate::testing::count_id(&app, "App::import-button"), 1);
    assert_eq!(
        crate::testing::label_of(&app, "App::import-button"),
        crate::i18n::text("Stop import")
    );
    crate::testing::press(&app, Key::F7);
    assert!(app.get_left_hidden());
    assert_eq!(crate::testing::count_id(&app, pane_line), 0);
    assert_eq!(
        crate::testing::count_id(&app, header_line),
        1,
        "the header's, the pane away"
    );
    assert_eq!(crate::testing::label_of(&app, header_line), line);
    assert_eq!(crate::testing::count_id(&app, "App::import-button"), 0);
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

/// The pane's one Import... button, clicked, opens a menu of the ways
/// in; its second item asks for the Lightroom sheet.
fn open_import_menu(app: &App) {
    slint::platform::update_timers_and_animations();
    let (at, size) = crate::testing::by_id(app, "App::import-button");
    assert!(at.x < 240.0, "in the pane");
    assert!(!app.get_menu_up());
    crate::testing::click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert!(app.get_menu_up(), "the menu opened");
}

/// One Import... button opens a menu with the two ways in, and each
/// item asks for its own sheet.
#[test]
fn the_import_button_opens_a_menu_of_the_two_ways_in() {
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1200.0, 700.0));
    app.set_grid_open(true);
    let seen = Rc::new(RefCell::new(Vec::<&str>::new()));
    let s = seen.clone();
    app.on_import_asked(move || s.borrow_mut().push("card"));
    let s = seen.clone();
    app.on_lr_asked(move || s.borrow_mut().push("catalog"));
    slint::platform::update_timers_and_animations();
    // One button, not one for each way in.
    assert_eq!(crate::testing::count_id(&app, "App::import-button"), 1);
    assert!(crate::testing::menu_entries(&app).is_empty(), "no menu yet");
    for (i, want) in ["card", "catalog"].into_iter().enumerate() {
        open_import_menu(&app);
        assert_eq!(
            crate::testing::count_menu_entries(&app, "From a card or folder..."),
            1
        );
        assert_eq!(
            crate::testing::count_menu_entries(&app, "A Lightroom catalog..."),
            1
        );
        // The menu's keys: Down to the item, Return to pick it.
        for _ in 0..=i {
            crate::testing::press(&app, slint::platform::Key::DownArrow);
        }
        crate::testing::press(&app, slint::platform::Key::Return);
        assert_eq!(seen.borrow().last(), Some(&want));
        slint::platform::update_timers_and_animations();
        assert!(!app.get_menu_up(), "the pick closed the menu");
    }
    assert_eq!(*seen.borrow(), ["card", "catalog"]);
}

/// Escape closes the menu and picks nothing.
#[test]
fn escape_closes_the_import_menu() {
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1200.0, 700.0));
    app.set_grid_open(true);
    let seen = Rc::new(RefCell::new(0));
    let s = seen.clone();
    app.on_import_asked(move || *s.borrow_mut() += 1);
    let s = seen.clone();
    app.on_lr_asked(move || *s.borrow_mut() += 1);
    open_import_menu(&app);
    crate::testing::press(&app, slint::platform::Key::Escape);
    assert!(!app.get_menu_up(), "Escape closed it");
    assert_eq!(*seen.borrow(), 0);
}

/// While a card import runs the button is its Stop and a click stops
/// it with no menu; while a catalog writes it says so and is disabled.
#[test]
fn the_import_button_stops_a_card_import_and_waits_for_a_catalog() {
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1200.0, 700.0));
    app.set_grid_open(true);
    let stops = Rc::new(RefCell::new(0));
    let s = stops.clone();
    app.on_import_stop(move || *s.borrow_mut() += 1);
    let asked = Rc::new(RefCell::new(0));
    let s = asked.clone();
    app.on_import_asked(move || *s.borrow_mut() += 1);
    app.set_import_running(true);
    slint::platform::update_timers_and_animations();
    let button = "App::import-button";
    assert_eq!(
        crate::testing::label_of(&app, button),
        crate::i18n::text("Stop import")
    );
    crate::testing::click_id(&app, button);
    assert_eq!(*stops.borrow(), 1, "a click stops it");
    assert!(!app.get_menu_up(), "with no menu");
    assert_eq!(*asked.borrow(), 0);
    app.set_import_stopping(true);
    slint::platform::update_timers_and_animations();
    assert_eq!(
        crate::testing::label_of(&app, button),
        crate::i18n::text("Stopping...")
    );
    crate::testing::click_id(&app, button);
    assert_eq!(*stops.borrow(), 1, "once stopping, a click does nothing");
    app.set_import_stopping(false);
    app.set_import_running(false);
    app.set_lr_running(true);
    slint::platform::update_timers_and_animations();
    assert_eq!(
        crate::testing::label_of(&app, button),
        crate::i18n::text("Importing the catalog...")
    );
    crate::testing::click_id(&app, button);
    assert!(!app.get_menu_up(), "a catalog writing opens no menu");
    assert_eq!(*stops.borrow(), 1);
    assert_eq!(*asked.borrow(), 0);
    assert_ne!(
        crate::testing::label_of(&app, button),
        crate::i18n::text("Import...")
    );
}

/// The folder tree's rows as the pane shows them: name, count, and
/// whether marked open.
fn tree_rows(app: &App) -> Vec<(String, i32, bool)> {
    app.get_folder_tree()
        .iter()
        .map(|r| (r.name.to_string(), r.count, r.open))
        .collect()
}

fn listed(state: &Rc<Shared>) -> Vec<String> {
    state
        .borrow()
        .files
        .iter()
        .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

/// A root's row under ROOTS opens the root as its row in the folder
/// tree does (discussion 59): with the switch off its own frames, the
/// folder under it a tile; on, everything under it. The switch never
/// moves for a click, flipping it over the root's view lists it again,
/// back comes to the root as it was shown, All roots is every frame
/// whatever the switch says, and a root with nothing of its own opens
/// empty as the tree's folder does. The row's count is the root's
/// total throughout, as a folded row's in the tree.
#[test]
fn a_roots_row_opens_the_root_as_the_switch_says() {
    let dir = scratch("row-switch");
    let root = dir.join("archive");
    let day = root.join("day");
    std::fs::create_dir_all(&day).unwrap();
    frames(&root, &["r.tif"]);
    frames(&day, &["d1.tif", "d2.tif"]);
    let db = dir.join("index").join("library.sqlite");
    greycard_library::Library::open(&db)
        .unwrap()
        .index_tree(&root, &mut |_| {})
        .unwrap();
    let app = window(0);
    app.set_grid_open(true);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.views.sort = crate::sort::Kept::by_name();
        st.views.library.roots.add(&root).unwrap();
        recount(&mut st);
    }
    let path = root.to_string_lossy().into_owned();
    let pick = |p: &str| {
        app.invoke_library_root_picked(p.into());
        land_all(&state, &app, &worker);
    };
    let own = View::Branch {
        root: root.clone(),
        folder: root.clone(),
        deep: false,
    };

    // The switch off (the default): the root's own frame, the row lit,
    // the tree's root marked, the switch shown off, the day a tile.
    assert!(!state.borrow().views.tree.subfolders);
    pick(&path);
    assert_eq!(state.borrow().views.view, own);
    assert_eq!(listed(&state), ["r.tif"]);
    let chip = app.get_library_roots().row_data(0).unwrap();
    assert!(chip.on, "the root's row lit");
    assert_eq!(chip.count, 3, "the root's total, whatever the switch");
    assert!(!app.get_folder_tree_subfolders());
    assert_eq!(
        tree_rows(&app),
        [("archive".into(), 1, true), ("day".into(), 2, false)]
    );
    let tiles: Vec<String> = app
        .get_grid_tiles()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    assert_eq!(tiles, ["day"]);

    // Flipped on over it: everything under the root, as its row would.
    app.set_folder_tree_subfolders(true);
    app.invoke_folder_tree_subfolders_changed();
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().views.view, View::Roots(Some(root.clone())));
    assert_eq!(listed(&state), ["d1.tif", "d2.tif", "r.tif"]);
    assert_eq!(app.get_grid_tiles().row_count(), 0, "no tiles with it on");
    // The row clicked with it on: the same, and the switch stays on.
    pick(&path);
    assert_eq!(state.borrow().views.view, View::Roots(Some(root.clone())));
    assert_eq!(listed(&state), ["d1.tif", "d2.tif", "r.tif"]);
    assert!(state.borrow().views.tree.subfolders);
    assert!(app.get_folder_tree_subfolders());
    assert_eq!(app.get_library_roots().row_data(0).unwrap().count, 3);

    // Off again: the root's own frame; a click leaves the switch off.
    app.set_folder_tree_subfolders(false);
    app.invoke_folder_tree_subfolders_changed();
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().views.view, own);
    assert_eq!(listed(&state), ["r.tif"]);
    pick(&path);
    assert_eq!(state.borrow().views.view, own);
    assert!(
        !state.borrow().views.tree.subfolders,
        "a click moves no switch"
    );

    // The day from the tree, then back: the root as it was shown.
    let row = tree_rows(&app).iter().position(|r| r.0 == "day").unwrap();
    app.invoke_folder_tree_picked(row as i32);
    land_all(&state, &app, &worker);
    assert_eq!(listed(&state), ["d1.tif", "d2.tif"]);
    app.invoke_visit_folder(true);
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().views.view, own);
    assert_eq!(listed(&state), ["r.tif"]);

    // All roots lists every frame with the switch off.
    pick("");
    assert_eq!(state.borrow().views.view, View::Roots(None));
    assert_eq!(listed(&state), ["d1.tif", "d2.tif", "r.tif"]);

    // A root with nothing of its own, the switch off: empty, as the
    // tree's folder with nothing of its own opens, and says why.
    std::fs::remove_file(root.join("r.tif")).unwrap();
    state.borrow_mut().views.index_reader = None;
    greycard_library::Library::open(&db)
        .unwrap()
        .index_folder(&root, &mut |_| {})
        .unwrap();
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        recount(&mut st);
    }
    pick(&path);
    assert_eq!(state.borrow().views.view, own);
    assert!(listed(&state).is_empty());
    assert!(
        app.get_status().starts_with("nothing directly in archive"),
        "{}",
        app.get_status()
    );
    assert!(!app.get_folder_tree_subfolders());
    crate::testing::remove_scratch(state, &dir);
}

/// A root's count changing at every word of a pass, as an archive's
/// does for minutes while its files are found, moves nothing in the
/// left pane: not its scroll, not ROOTS' rows, not FOLDERS or its
/// list's scroll, and not COLLECTIONS under them. The pane and the
/// tree's list are scrolled partway, and each place is read the way a
/// frame drawn between two words reads it: from the handle found
/// before, with no walk of the window's tree first, then at each frame
/// of the next 160 ms. New folders found under the open root then
/// lengthen its tree under the FOLDERS heading, which stays where it
/// is, as does the row at the top of the list, folders coming in above
/// it or under it.
#[test]
fn a_roots_count_changing_moves_nothing_in_the_left_pane() {
    use i_slint_backend_testing::ElementHandle;
    let dir = scratch("still");
    let (a, archive) = (dir.join("a"), dir.join("Archive"));
    for i in 0..4 {
        let d = a.join(format!("d{i}"));
        std::fs::create_dir_all(&d).unwrap();
        frames(&d, &["x.tif"]);
    }
    for i in 0..12 {
        let d = archive.join(format!("s{i:02}"));
        std::fs::create_dir_all(&d).unwrap();
        frames(&d, &["x.tif"]);
    }
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&a, &mut |_| {}).unwrap();
    writer.index_tree(&archive, &mut |_| {}).unwrap();
    let app = window(0);
    app.window()
        .set_size(slint::LogicalSize::new(1200.0, 640.0));
    app.set_grid_open(true);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.views.sort = crate::sort::Kept::by_name();
        st.views.library.roots.add(&a).unwrap();
        st.views.library.roots.add(&archive).unwrap();
        st.views.library.roots.set_archive(&archive, true);
        for k in 0..3 {
            st.views
                .collections
                .list
                .push(crate::collections::Collection::new(&format!("c{k}")));
        }
        recount(&mut st);
    }
    open_view(&state, &app, &worker, View::Roots(Some(archive.clone())));
    land_all(&state, &app, &worker);
    crate::testing::settle(&app);

    // The places watched, each by its id, found once.
    let ids = [
        "RootsSection::all-roots",
        "RootsSection::add-root-link",
        "App::folders-section",
        "FoldersSection::folder-pane",
        "App::collections-section",
    ];
    let held: Vec<ElementHandle> = ids
        .iter()
        .map(|id| {
            ElementHandle::find_by_element_id(&app, id)
                .next()
                .unwrap_or_else(|| panic!("no {id}"))
        })
        .collect();
    let places = || -> Vec<f32> {
        held.iter()
            .flat_map(|h| {
                let (at, size) = crate::testing::bounds_of(h);
                [at.y, size.height]
            })
            .collect()
    };
    let wheel = |x: f32, y: f32, by: f32| {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(x, y),
                delta_x: 0.0,
                delta_y: -by,
            });
        crate::testing::settle(&app);
    };
    // Where each of the tree's rows is, as found afresh: its scroll.
    let tree_rows = || -> Vec<f32> {
        crate::testing::settle(&app);
        ElementHandle::find_by_element_id(&app, "FoldersSection::folder-row")
            .map(|r| r.absolute_position().y)
            .collect()
    };
    // Scrolled partway, the pane and the tree's list in it: each holds
    // more than it shows.
    let top = places();
    wheel(100.0, 300.0, 40.0);
    let before = places();
    assert!(
        before[0] < top[0] - 10.0,
        "the pane did not scroll: {top:?} then {before:?}"
    );
    let tree_top = tree_rows();
    let pane = held[3].absolute_position();
    wheel(pane.x + 40.0, pane.y + 10.0, 69.0);
    let tree_before = tree_rows();
    assert_eq!(places(), before, "the tree's wheel scrolled the pane");
    assert_ne!(tree_before, tree_top, "the tree did not scroll");

    // Every frame of a word's next 160 ms, and settled.
    let frames_after = |what: &str| {
        let mut seen = vec![places()];
        for _ in 0..10 {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(16));
            slint::platform::update_timers_and_animations();
            seen.push(places());
        }
        for (k, now) in seen.iter().enumerate() {
            assert_eq!(
                *now,
                before,
                "{what}: {ids:?} moved {} ms after the word",
                k * 16
            );
        }
    };
    let reindex = |writer: &mut greycard_library::Library| {
        state.borrow_mut().views.index_reader = None;
        writer.index_tree(&archive, &mut |_| {}).unwrap();
        state.borrow_mut().views.index_reader =
            Some(greycard_library::Library::open_read_only(&db).unwrap());
    };
    let told = |path: &Path, added: usize| {
        let report = greycard_library::Report {
            added,
            ..Default::default()
        };
        background_done(&state, &app, path, &report, false, false);
    };

    // Frames found in a folder the tree already has: only counts move,
    // the root's and the folder's.
    let day = archive.join("s05");
    for i in 1..=6 {
        let names: Vec<String> = (0..i * 4).map(|k| format!("w{i}-{k}.tif")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        frames(&day, &names);
        reindex(&mut writer);
        told(&day, names.len());
        frames_after(&format!("word {i}, before the reads land"));
        land_all(&state, &app, &worker);
        frames_after(&format!("word {i}, the reads landed"));
        assert_eq!(tree_rows(), tree_before, "word {i}: the tree's list moved");
    }
    let archive_count = app
        .get_library_roots()
        .iter()
        .find(|c| c.archive)
        .map(|c| c.count);
    assert_eq!(
        archive_count,
        Some(12 + (1..=6).map(|i| i * 4).sum::<i32>())
    );
    let day_count = app
        .get_folder_tree()
        .iter()
        .find(|r| r.name == "s05")
        .map(|r| r.count);
    assert_eq!(day_count, Some(1 + (1..=6).map(|i| i * 4).sum::<i32>()));

    // New folders under the open root, after those there and before
    // them: the tree grows under its heading, which stays put, as do
    // ROOTS' rows above it and the tree's own box; and the row at the
    // top of the tree's list stays there, its folder and its place,
    // whether the new folder comes under it or above it, out of sight.
    let heading = |p: &[f32]| p[..8].to_vec();
    let top_row = || -> (String, f32) {
        crate::testing::settle(&app);
        let pane_top = held[3].absolute_position().y;
        ElementHandle::find_by_element_id(&app, "FoldersSection::folder-line")
            .map(|r| {
                let (at, size) = crate::testing::bounds_of(&r);
                let path = r.accessible_description().unwrap_or_default().to_string();
                (path, at.y, at.y + size.height)
            })
            .filter(|(_, _, foot)| *foot > pane_top + 0.5)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(path, y, _)| (path, y))
            .expect("a row in the tree's list")
    };
    let first = top_row();
    assert!(
        first.0.ends_with("s02"),
        "the list scrolled by three rows has s02 at its top: {first:?}"
    );
    for (i, name) in ["t01", "a01", "t02", "a02"].iter().enumerate() {
        let d = archive.join(name);
        std::fs::create_dir_all(&d).unwrap();
        frames(&d, &["x.tif", "y.tif"]);
        reindex(&mut writer);
        told(&d, 2);
        for _ in 0..10 {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(16));
            slint::platform::update_timers_and_animations();
            assert_eq!(heading(&places()), heading(&before), "folder {i}: {name}");
        }
        land_all(&state, &app, &worker);
        crate::testing::settle(&app);
        assert_eq!(heading(&places()), heading(&before), "folder {i}: {name}");
        assert_eq!(top_row(), first, "folder {i}: {name}");
    }
    assert_eq!(
        app.get_folder_tree()
            .iter()
            .filter(|r| r.depth == 1)
            .count(),
        16,
        "every folder in the tree"
    );
    drop(held);
    drop(writer);
    drop(worker);
    crate::testing::remove_scratch(state, &dir);
}
