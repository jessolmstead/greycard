//! Tests of the roots pane: its rows, its menus and the keys over it.

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
