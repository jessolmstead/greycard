use super::*;
use crate::roots::{land_sent, open_view};
use crate::testing::{click, folder, state_for, window};
use greycard_library::fixture::{A7, R5, R6, write_frame};

struct Setup {
    dir: PathBuf,
    /// x, y and z in the shoot.
    files: Vec<PathBuf>,
    /// x's copy in the selects.
    copy: PathBuf,
    writer: greycard_library::Library,
    app: App,
    state: Rc<Shared>,
    worker: Rc<Worker>,
}

/// A root of a shoot and its selects, indexed, on a window with the
/// grid up over every root, its collections kept in the test's own
/// folder.
fn setup(what: &str) -> Setup {
    let dir = crate::testing::scratch_dir(&format!("coll-{what}"));
    std::fs::create_dir_all(dir.join("photos").join("shoot")).unwrap();
    std::fs::create_dir_all(dir.join("photos").join("selects")).unwrap();
    let dir = dunce::canonicalize(&dir).unwrap();
    let root = dir.join("photos");
    let (shoot, selects) = (root.join("shoot"), root.join("selects"));
    let files: Vec<PathBuf> = ["x.tif", "y.tif", "z.tif"]
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let p = shoot.join(n);
            write_frame(&p, [&R5, &R6, &A7][i], i as u16 + 1);
            p
        })
        .collect();
    let copy = selects.join("x.tif");
    std::fs::copy(&files[0], &copy).unwrap();
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&root, &mut |_| {}).unwrap();
    let app = window(0);
    app.window()
        .set_size(slint::LogicalSize::new(1400.0, 900.0));
    app.set_grid_open(true);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        // By name over the folders: the frames are told apart by their
        // place in the list this built, in that order.
        st.views.sort = crate::sort::Kept::by_name();
        st.views.library.roots.add(&root).unwrap();
        st.views.collections.dir = Some(dir.join(crate::collections::DIR));
        // Sidecars on, as a session has them: a frame stands in from
        // its row, and the row is what marks it.
        st.window.write_sidecars = true;
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_sent(&state, &app, &worker);
    Setup {
        dir,
        files,
        copy,
        writer,
        app,
        state,
        worker,
    }
}

/// The test's folder removed, everything that holds a file in it
/// let go first: the writer, and the window's reader, which the
/// window's callbacks keep alive past a drop of the test's state
/// (Windows refuses to remove a file SQLite has open).
fn teardown(s: Setup) {
    let Setup {
        dir,
        writer,
        app,
        state,
        worker,
        ..
    } = s;
    drop(writer);
    state.borrow_mut().views.index_reader = None;
    drop(app);
    drop(worker);
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// The list, each frame as its folder and its name.
fn names(st: &State) -> Vec<String> {
    st.files
        .iter()
        .map(|f| {
            let dir = f.parent().unwrap().file_name().unwrap().to_string_lossy();
            format!("{dir}/{}", f.file_name().unwrap().to_string_lossy())
        })
        .collect()
}

fn row_of(st: &State, path: &Path) -> i32 {
    let f = st.files.iter().position(|p| p == path).unwrap();
    crate::panel::browser::row_of(st, f).unwrap() as i32
}

fn marks(app: &App) -> Vec<bool> {
    let model = app.get_thumbs();
    (0..model.row_count())
        .map(|r| model.row_data(r).unwrap().collected)
        .collect()
}

/// The middle of the pane's row of the collection `id`.
fn middle_of(app: &App, id: &str) -> (f32, f32) {
    let (at, size) = crate::testing::by_key(app, "CollectionsSection::collection-row", id);
    (at.x + size.width / 2.0, at.y + size.height / 2.0)
}

fn file_of(s: &Setup, id: &str) -> serde_json::Value {
    let path = s
        .dir
        .join(crate::collections::DIR)
        .join(format!("{id}.json"));
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn hints(s: &Setup, id: &str) -> Vec<String> {
    file_of(s, id)["refs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let p = Path::new(r["hint"].as_str().unwrap());
            let dir = p.parent().unwrap().file_name().unwrap().to_string_lossy();
            format!("{dir}/{}", p.file_name().unwrap().to_string_lossy())
        })
        .collect()
}

fn open_collection_submenu(app: &App) {
    use crate::testing::{click_with, keyed, settle};
    use slint::platform::PointerEventButton;

    settle(app);
    let cell = keyed(app, "GridSheet::cell", 0)
        .and_then(|c| c.query_descendants().match_id("GridSheet::ct").find_first())
        .unwrap();
    let (at, size) = (cell.absolute_position(), cell.size());
    click_with(
        app,
        PointerEventButton::Right,
        at.x + size.width / 2.0,
        at.y + size.height / 2.0,
    );
    collection_menu_entry(app, "Add to collection")
        .unwrap()
        .mock_single_click(PointerEventButton::Left);
}

/// Collections named Collection 001 on, made a second apart in that
/// order, in the store (not yet on disk), and shown; the last one's id.
fn numbered_collections(s: &Setup, count: usize) -> String {
    let mut st = s.state.borrow_mut();
    st.views.collections.list = (1..=count)
        .map(|i| {
            let mut c = Collection::new(&format!("Collection {i:03}"));
            c.made = 1_700_000_000 + i as u64;
            c
        })
        .collect();
    show(&st, &s.app);
    st.views.collections.list.last().unwrap().id.clone()
}

fn collection_menu_entry(app: &App, msgid: &str) -> Option<i_slint_backend_testing::ElementHandle> {
    let title = crate::i18n::text(msgid);
    crate::testing::menu_entries(app)
        .into_iter()
        .find(|e| e.accessible_label().as_deref() == Some(title.as_str()))
}

#[test]
fn ten_collections_fit_in_the_submenu_and_a_pick_moves_to_the_front() {
    use crate::testing::settle;
    use slint::platform::PointerEventButton;

    let s = setup("short-picker");
    let app = &s.app;
    let last = numbered_collections(&s, 10);
    let folder = s.dir.join(crate::collections::DIR);
    let earlier = s.state.borrow().views.collections.list[3].id.clone();
    // Another window used a collection after this window opened.
    greycard_library::collections::added_to(&folder, &earlier).unwrap();
    open_collection_submenu(app);
    assert!(collection_menu_entry(app, "All collections...").is_none());
    assert!(collection_menu_entry(app, "New collection...").is_some());
    collection_menu_entry(app, "Collection 010")
        .unwrap()
        .mock_single_click(PointerEventButton::Left);
    settle(app);
    assert!(!app.invoke_sheet_open());
    assert_eq!(hints(&s, &last), ["selects/x.tif"]);
    let order: Vec<_> = app
        .get_collection_quick()
        .iter()
        .take(2)
        .map(|choice| choice.id.to_string())
        .collect();
    assert_eq!(order, [last.clone(), earlier.clone()]);
    // Kept beside the library's collections, not in the settings.
    assert_eq!(
        greycard_library::collections::read_recent(&folder),
        [last, earlier]
    );
    teardown(s);
}

#[test]
fn the_frame_menu_can_add_to_the_last_of_392_collections() {
    use crate::testing::{press, settle, type_text};
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::{Key, PointerEventButton, WindowEvent};

    let s = setup("long-picker");
    let app = &s.app;
    let last = numbered_collections(&s, 392);
    app.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.5 });
    open_collection_submenu(app);
    assert_eq!(app.get_collection_quick().row_count(), 10);
    assert!(collection_menu_entry(app, "Collection 011").is_none());
    collection_menu_entry(app, "All collections...")
        .unwrap()
        .mock_single_click(PointerEventButton::Left);
    settle(app);
    assert!(app.invoke_sheet_open());
    let pane = ElementHandle::find_by_element_id(app, "CollectionPickerSheet::scroll")
        .next()
        .unwrap();
    let (at, size) = (pane.absolute_position(), pane.size());
    assert!(size.height <= 360.0);
    assert!(
        ElementHandle::find_by_accessible_label(app, "Collection 392")
            .next()
            .is_none()
    );
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: slint::LogicalPosition::new(at.x + 40.0, at.y + 40.0),
        delta_x: 0.0,
        delta_y: -30_000.0,
    });
    settle(app);
    assert!(
        ElementHandle::find_by_accessible_label(app, "Collection 392")
            .next()
            .is_some()
    );
    type_text(app, "COLLECTION 392");
    settle(app);
    assert!(
        ElementHandle::find_by_accessible_label(app, "Collection 391")
            .next()
            .is_none()
    );
    press(app, Key::Return);
    settle(app);
    assert!(!app.invoke_sheet_open());
    assert_eq!(hints(&s, &last), ["selects/x.tif"]);
    teardown(s);
}

/// The frame menu makes a collection of the set and the sheet names
/// it; every copy of a member is marked; the sidebar's row opens it
/// as the grid's list, each picture once, in its own order; B adds
/// to it from another view and takes out what is all in already; a
/// copy of a member moves its hint; Remove takes a frame out of the
/// list and the file.
#[test]
fn the_menu_and_the_sidebar_make_fill_and_open_a_collection() {
    let s = setup("menu");
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    assert_eq!(
        names(&state.borrow()),
        ["selects/x.tif", "shoot/x.tif", "shoot/y.tif", "shoot/z.tif"]
    );
    // The shoot's x and z chosen, and the menu's New collection.
    let x = row_of(&state.borrow(), &s.files[0]);
    app.invoke_select(x);
    state.borrow_mut().views.picked = vec![1, 3];
    app.invoke_collection_menu_new();
    assert!(app.get_collection_sheet_open());
    assert_eq!(app.get_collection_sheet_note(), "2 frames go into it");
    app.set_collection_sheet_name("Clients/Cover picks".into());
    app.invoke_collection_sheet_done();
    assert!(!app.get_collection_sheet_open());
    let id = {
        let st = state.borrow();
        assert_eq!(st.views.collections.list.len(), 1);
        let c = &st.views.collections.list[0];
        assert_eq!(
            (c.set.as_str(), c.name.as_str()),
            ("Clients", "Cover picks")
        );
        c.id.clone()
    };
    assert_eq!(hints(&s, &id), ["shoot/x.tif", "shoot/z.tif"]);
    // Every local frame of a member's hash is marked: the selects'
    // copy of x too, and not y.
    assert_eq!(marks(app), [true, true, false, true]);
    let in_menu: Vec<String> = app
        .get_collection_quick()
        .iter()
        .map(|c| c.name.to_string())
        .collect();
    assert_eq!(in_menu, ["Clients/Cover picks"]);
    assert_eq!(app.get_collection_open(), "");

    // Its row in the pane opens it: x and z, each once, x from the
    // shoot where it was added.
    let (cx, cy) = middle_of(app, &id);
    click(app, cx, cy);
    land_sent(state, app, worker);
    assert_eq!(
        state.borrow().views.view,
        View::Collection { id: id.clone() }
    );
    assert_eq!(names(&state.borrow()), ["shoot/x.tif", "shoot/z.tif"]);
    assert_eq!(app.get_collection_open(), "Clients/Cover picks");
    assert!(app.get_collection_reorder());
    assert_eq!(app.get_status(), "Clients/Cover picks: 2 frames");
    assert!(
        state
            .borrow()
            .views
            .collections_held
            .rows
            .row_data(0)
            .unwrap()
            .on
    );

    // Back to every root: y chosen and B, which goes to the last
    // collection made.
    open_view(state, app, worker, View::Roots(None));
    land_sent(state, app, worker);
    let y = row_of(&state.borrow(), &s.files[1]);
    app.invoke_select(y);
    crate::testing::press(app, "b");
    assert_eq!(app.get_status(), "1 frame added to Clients/Cover picks");
    assert_eq!(
        hints(&s, &id),
        ["shoot/x.tif", "shoot/z.tif", "shoot/y.tif"]
    );
    // B again on y, all of it in: out again.
    crate::testing::press(app, "b");
    assert_eq!(app.get_status(), "1 frame taken out of Clients/Cover picks");
    assert_eq!(hints(&s, &id), ["shoot/x.tif", "shoot/z.tif"]);
    // The selects' x through the menu: already in, its hint moved.
    let copy = row_of(&state.borrow(), &s.copy);
    app.invoke_select(copy);
    app.invoke_collection_menu_add(app.get_collection_quick().row_data(0).unwrap());
    assert_eq!(
        app.get_status(),
        "already in Clients/Cover picks; shown from this copy now"
    );
    assert_eq!(hints(&s, &id), ["selects/x.tif", "shoot/z.tif"]);

    // Opened again: x from the selects now. The menu's Remove on
    // z: z leaves the list, and the file.
    app.invoke_collection_picked(id.as_str().into());
    land_sent(state, app, worker);
    assert_eq!(names(&state.borrow()), ["selects/x.tif", "shoot/z.tif"]);
    app.invoke_select(1);
    app.invoke_frame_menu_asked(1);
    app.invoke_collection_menu_remove();
    land_sent(state, app, worker);
    assert_eq!(names(&state.borrow()), ["selects/x.tif"]);
    assert_eq!(hints(&s, &id), ["selects/x.tif"]);
    // Kept on disk: a new session reads the same collection back.
    let back = crate::collections::Store::load(s.dir.join(crate::collections::DIR));
    assert_eq!(back.list, state.borrow().views.collections.list);
    teardown(s);
}

/// A drag in the grid moves a frame in the collection's own order;
/// capture order from the row's menu shows it by time and moves
/// nothing.
#[test]
fn a_drag_in_the_grid_reorders_the_collection() {
    let s = setup("drag");
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    let id = {
        let mut st = state.borrow_mut();
        let shoot: Vec<usize> = s
            .files
            .iter()
            .map(|f| st.files.iter().position(|p| p == f).unwrap())
            .collect();
        let frames = frames_of(&st, &shoot);
        let mut c = Collection::new("Order");
        c.add(&frames);
        let id = c.id.clone();
        st.views.collections.list.push(c);
        st.views.collections.save(&id);
        id
    };
    app.invoke_collection_picked(id.as_str().into());
    land_sent(state, app, worker);
    assert_eq!(
        names(&state.borrow()),
        ["shoot/x.tif", "shoot/y.tif", "shoot/z.tif"]
    );
    app.invoke_select(1);
    // A cell, by its name under the picture: a point on its picture.
    let at = |name: &str| {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
        let e = crate::testing::cell_names(app, name)
            .into_iter()
            .min_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y))
            .unwrap_or_else(|| panic!("no cell {name}"));
        let (p, size) = (e.absolute_position(), e.size());
        (p.x + size.width / 2.0, p.y - 30.0)
    };
    let (zx, zy) = at("z.tif");
    let (xx, xy) = at("x.tif");
    // z dragged to the gap before x: the left half of x's cell.
    let to = (xx - 40.0, xy);
    crate::testing::drag(app, &[(zx, zy), (zx - 12.0, zy), to]);
    land_sent(state, app, worker);
    assert_eq!(
        names(&state.borrow()),
        ["shoot/z.tif", "shoot/x.tif", "shoot/y.tif"]
    );
    assert_eq!(
        hints(&s, &id),
        ["shoot/z.tif", "shoot/x.tif", "shoot/y.tif"]
    );
    // The drag was no click: y is still the frame on screen.
    {
        let st = state.borrow();
        assert_eq!(
            st.current.map(|c| st.files[c].clone()),
            Some(s.files[1].clone())
        );
    }
    // Capture order: by time (the R5 years before the A7, the A7
    // before the R6), the file's order kept.
    app.invoke_collection_capture(id.as_str().into());
    land_sent(state, app, worker);
    assert_eq!(
        names(&state.borrow()),
        ["shoot/x.tif", "shoot/z.tif", "shoot/y.tif"]
    );
    assert!(!app.get_collection_reorder());
    assert!(
        state
            .borrow()
            .views
            .collections_held
            .rows
            .row_data(0)
            .unwrap()
            .capture
    );
    assert_eq!(
        hints(&s, &id),
        ["shoot/z.tif", "shoot/x.tif", "shoot/y.tif"]
    );
    // A move asked for in capture order is nothing.
    app.invoke_collection_moved(2, 0);
    assert_eq!(
        hints(&s, &id),
        ["shoot/z.tif", "shoot/x.tif", "shoot/y.tif"]
    );
    teardown(s);
}

/// The lightbox over a collection lays its tiles out in the
/// collection's own order, not the frames' time or name.
#[test]
fn the_lightbox_over_a_collection_is_in_its_hand_order() {
    let s = setup("lightbox");
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    let id = {
        let mut st = state.borrow_mut();
        // z, x, y: neither the names' order nor the time's.
        let order: Vec<usize> = [2, 0, 1]
            .iter()
            .map(|&i| st.files.iter().position(|p| *p == s.files[i]).unwrap())
            .collect();
        let frames = frames_of(&st, &order);
        let mut c = Collection::new("Sequence");
        c.add(&frames);
        let id = c.id.clone();
        st.views.collections.list.push(c);
        st.views.collections.save(&id);
        id
    };
    app.invoke_collection_picked(id.as_str().into());
    land_sent(state, app, worker);
    app.invoke_select(0);
    crate::testing::press(app, "l");
    assert!(app.get_lightbox());
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
    let thumbs = app.get_thumbs();
    let mut tiles: Vec<(f32, f32, &str)> = ["x.tif", "y.tif", "z.tif"]
        .into_iter()
        .map(|name| {
            // The tile of the row named so, the cells keyed by row.
            let row = (0..thumbs.row_count())
                .find(|&r| thumbs.row_data(r).is_some_and(|t| t.name == name))
                .unwrap_or_else(|| panic!("no row {name}"));
            let e = crate::testing::keyed(app, "GridSheet::cell", row)
                .unwrap_or_else(|| panic!("no tile {name}"));
            let p = e.absolute_position();
            (p.y, p.x, name)
        })
        .collect();
    tiles.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let order: Vec<&str> = tiles.iter().map(|t| t.2).collect();
    assert_eq!(order, ["z.tif", "x.tif", "y.tif"]);
    teardown(s);
}

/// A reference to a picture nothing has is kept and said; a file
/// another tool rewrote in place heals the reference by its hint in
/// the pass that found it, and the collection still shows it.
#[test]
fn a_missing_frame_is_kept_and_a_rewritten_one_healed() {
    let mut s = setup("heal");
    let id = {
        let mut st = s.state.borrow_mut();
        let x = st.files.iter().position(|p| *p == s.files[0]).unwrap();
        let mut frames = frames_of(&st, &[x]);
        frames.push((
            "0123456789abcdef".into(),
            PathBuf::from("/gone/away.tif"),
            None,
        ));
        let mut c = Collection::new("Kept");
        c.add(&frames);
        let id = c.id.clone();
        st.views.collections.list.push(c);
        st.views.collections.save(&id);
        id
    };
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    app.invoke_collection_picked(id.as_str().into());
    land_sent(state, app, worker);
    assert_eq!(names(&state.borrow()), ["shoot/x.tif"]);
    assert_eq!(
        app.get_status(),
        "Kept: 1 frame shown, 1 missing (moved or deleted, and kept until found)"
    );
    assert_eq!(file_of(&s, &id)["refs"].as_array().unwrap().len(), 2);
    let old = file_of(&s, &id)["refs"][0]["hash"]
        .as_str()
        .unwrap()
        .to_string();

    // The selects' copy gone (another frame left in its folder, so
    // the pass takes it for gone), and x rewritten in place: the
    // old hash is nowhere, the pass says so, and the reference
    // heals by its hint.
    std::fs::remove_file(&s.copy).unwrap();
    write_frame(&s.copy.with_file_name("keep.tif"), &A7, 9);
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_frame(&s.files[0], &R6, 7);
    let root = s.dir.join("photos");
    let report = s.writer.index_tree(&root, &mut |_| {}).unwrap();
    assert_eq!(report.rehashed.len(), 1, "{report:?}");
    healed(&s.state, &s.app, &report.rehashed);
    land_sent(&s.state, &s.app, &s.worker);
    let refs = file_of(&s, &id)["refs"].clone();
    assert_ne!(refs[0]["hash"].as_str().unwrap(), old);
    assert_eq!(refs[0]["hash"].as_str().unwrap(), report.rehashed[0].new);
    assert_eq!(refs.as_array().unwrap().len(), 2, "the missing one kept");
    assert_eq!(names(&s.state.borrow()), ["shoot/x.tif"]);
    assert_eq!(marks(&s.app), [true]);
    teardown(s);
}

/// The open collection's row offers to forget its missing frames
/// when it has some: the one found nowhere goes, from the list and
/// the file, the one hinted under an offline root stays, and the
/// status line counts what is left.
#[test]
fn forget_missing_drops_the_gone_and_keeps_the_offline() {
    let s = setup("forget");
    let drive = s.dir.join("drive");
    let id = {
        let mut st = s.state.borrow_mut();
        // A root of the library, unplugged: never made on disk.
        st.views.library.roots.add_checked(drive.clone()).unwrap();
        st.views.library.offline.insert(drive.clone());
        let x = st.files.iter().position(|p| *p == s.files[0]).unwrap();
        let mut frames = frames_of(&st, &[x]);
        frames.push((
            "0123456789abcdef".into(),
            s.dir.join("gone").join("a.tif"),
            None,
        ));
        frames.push(("fedcba9876543210".into(), drive.join("b.tif"), None));
        let mut c = Collection::new("Kept");
        c.add(&frames);
        let id = c.id.clone();
        st.views.collections.list.push(c);
        st.views.collections.save(&id);
        id
    };
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    let chip = |app: &App| {
        let rows = app.get_collections();
        (0..rows.row_count())
            .map(|r| rows.row_data(r).unwrap())
            .find(|c| c.id.as_str() == id)
            .unwrap()
    };
    show(&state.borrow(), app);
    let offered = |app: &App| app.invoke_collection_forgettable(id.as_str().into());
    assert_eq!(offered(app), 0, "nothing offered while closed");
    app.invoke_collection_picked(id.as_str().into());
    land_sent(state, app, worker);
    assert_eq!(names(&state.borrow()), ["shoot/x.tif"]);
    assert_eq!(
        app.get_status(),
        "Kept: 1 frame shown, 2 missing (moved or deleted, and kept until found)"
    );
    assert_eq!(offered(app), 1, "the offline one is not offered");
    // A pass under way: nothing offered, and a press refused.
    state.borrow_mut().views.library.launch_left = 1;
    assert_eq!(offered(app), 0);
    app.invoke_collection_forget_missing(id.as_str().into());
    assert_eq!(
        app.get_status(),
        "the library is being indexed; nothing forgotten from Kept until it is done"
    );
    assert_eq!(hints(&s, &id).len(), 3);
    state.borrow_mut().views.library.launch_left = 0;
    assert_eq!(chip(app).count, 3);

    app.invoke_collection_forget_missing(id.as_str().into());
    assert_eq!(
        app.get_status(),
        "Kept: 1 frame shown, 1 missing (moved or deleted, and kept until found); \
         forgot 1 missing"
    );
    assert_eq!(hints(&s, &id), ["shoot/x.tif", "drive/b.tif"]);
    assert_eq!(offered(app), 0);
    // Pressed again: nothing to forget, and the reason is the drive.
    app.invoke_collection_forget_missing(id.as_str().into());
    assert_eq!(
        app.get_status(),
        "Kept: 1 frame shown, 1 missing (moved or deleted, and kept until found); \
         nothing forgotten: 1 frame on a drive that is offline or not answering"
    );
    // The drive pulled since the window last looked: the set it holds
    // does not say offline, but the root answers no, and b is kept.
    state.borrow_mut().views.library.offline.clear();
    app.invoke_collection_forget_missing(id.as_str().into());
    assert_eq!(
        app.get_status(),
        "Kept: 1 frame shown, 1 missing (moved or deleted, and kept until found); \
         nothing forgotten: 1 frame on a drive that is offline or not answering"
    );
    assert_eq!(hints(&s, &id), ["shoot/x.tif", "drive/b.tif"]);
    state
        .borrow_mut()
        .views
        .library
        .offline
        .insert(drive.clone());
    assert_eq!(chip(app).count, 2);
    assert_eq!(names(&state.borrow()), ["shoot/x.tif"]);
    // Opened again: the status line's count is the one left.
    app.invoke_collection_picked(id.as_str().into());
    land_sent(state, app, worker);
    assert_eq!(
        app.get_status(),
        "Kept: 1 frame shown, 1 missing (moved or deleted, and kept until found)"
    );
    teardown(s);
}

/// With no index to read, a pass's rewrite heals nothing it cannot
/// be sure of: a reference whose new hash another holds keeps its
/// hint as it was, never an empty one.
#[test]
fn a_heal_with_no_index_to_ask_keeps_the_hint() {
    let s = setup("no-reader");
    let id = made(&s, "Both", &[&s.files[0], &s.files[1]]);
    let (hx, hy) = {
        let st = s.state.borrow();
        let c = st.views.collections.collection(&id).unwrap();
        (c.refs[0].hash.clone(), c.refs[1].hash.clone())
    };
    s.state.borrow_mut().views.index_reader = None;
    let rehash = Rehash {
        path: s.files[0].clone(),
        old: hx.clone(),
        new: hy.clone(),
    };
    healed(&s.state, &s.app, &[rehash]);
    let refs = s
        .state
        .borrow()
        .views
        .collections
        .collection(&id)
        .unwrap()
        .refs
        .clone();
    assert_eq!(refs[0].hash, hx);
    assert_eq!(refs[0].hint, s.files[0].to_string_lossy());
    assert_eq!(
        file_of(&s, &id)["refs"][0]["hint"],
        s.files[0].to_string_lossy().as_ref()
    );
    teardown(s);
}

/// The filter bar's Save... keeps the filter as a smart collection;
/// its row puts the filter back over every root, and is lit while
/// the bar says it; its menu renames and deletes it.
#[test]
fn a_smart_collection_keeps_the_filter_and_puts_it_back() {
    let s = setup("smart");
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    app.invoke_filter_save_smart();
    assert!(!app.get_collection_sheet_open(), "nothing to save yet");
    state.borrow_mut().views.filter.text = "y".into();
    app.set_filter_text("y".into());
    crate::panel::cull::apply_filter(state, app);
    assert_eq!(state.borrow().views.shown.len(), 1);
    app.invoke_filter_save_smart();
    assert!(app.get_collection_sheet_open());
    assert_eq!(app.get_collection_sheet_note(), "\"y\", under every root");
    app.set_collection_sheet_name("Just y".into());
    app.invoke_collection_sheet_done();
    let id = state.borrow().views.collections.smart[0].id.clone();
    let file = s
        .dir
        .join(crate::collections::DIR)
        .join(format!("{id}.smart.json"));
    assert!(file.exists());
    assert!(
        state
            .borrow()
            .views
            .collections_held
            .rows
            .row_data(0)
            .unwrap()
            .on
    );
    // Cleared, the row goes dark; pressed, the filter is back.
    app.invoke_filter_cleared();
    assert_eq!(state.borrow().views.shown.len(), 4);
    assert!(
        !state
            .borrow()
            .views
            .collections_held
            .rows
            .row_data(0)
            .unwrap()
            .on
    );
    let (cx, cy) = middle_of(app, &id);
    click(app, cx, cy);
    land_sent(state, app, worker);
    assert_eq!(state.borrow().views.filter.text, "y");
    assert_eq!(app.get_filter_text(), "y");
    assert_eq!(state.borrow().views.shown.len(), 1);
    assert!(
        state
            .borrow()
            .views
            .collections_held
            .rows
            .row_data(0)
            .unwrap()
            .on
    );
    // Read back from its file, the same question.
    let back = crate::collections::Store::load(s.dir.join(crate::collections::DIR));
    assert_eq!(
        back.smart[0].filter,
        Saved::of(&state.borrow().views.filter)
    );
    // Renamed, into a set, and deleted, from its menu's sheet.
    app.invoke_collection_rename(id.as_str().into());
    assert_eq!(app.get_collection_sheet_name(), "Just y");
    app.set_collection_sheet_name("Sets/Only y".into());
    app.invoke_collection_sheet_done();
    assert_eq!(state.borrow().views.collections.smart[0].set, "Sets");
    app.invoke_collection_delete(id.as_str().into());
    assert!(!app.get_collection_sheet_asks_name());
    app.invoke_collection_sheet_done();
    assert!(state.borrow().views.collections.smart.is_empty());
    assert!(!file.exists());
    teardown(s);
}

/// An older smart collection holding an ISO chip loads as the
/// range's term, and its row lights while the bar says it, though
/// its file, not rewritten, still holds the chip.
#[test]
fn an_older_smart_collection_with_iso_chips_lights_when_opened() {
    let s = setup("smart-iso");
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    state.borrow_mut().views.filter.text = "y".into();
    app.set_filter_text("y".into());
    crate::panel::cull::apply_filter(state, app);
    app.invoke_filter_save_smart();
    app.set_collection_sheet_name("Low ISO".into());
    app.invoke_collection_sheet_done();
    // As a build with the chips kept it: no text, an ISO chip.
    let older: Saved = serde_json::from_str(
        r#"{"stars": 0, "exact": false, "flags": [], "labels": [], "text": "",
                "facets": {"iso": ["100"]}}"#,
    )
    .unwrap();
    state.borrow_mut().views.collections.smart[0].filter = older.clone();
    let id = state.borrow().views.collections.smart[0].id.clone();
    let file = s
        .dir
        .join(crate::collections::DIR)
        .join(format!("{id}.smart.json"));
    let kept = std::fs::read(&file).unwrap();
    app.invoke_filter_cleared();
    show(&state.borrow(), app);
    let (cx, cy) = middle_of(app, &id);
    click(app, cx, cy);
    land_sent(state, app, worker);
    assert_eq!(app.get_filter_text(), "iso:100");
    let lit = |state: &Rc<Shared>| {
        state
            .borrow()
            .views
            .collections_held
            .rows
            .row_data(0)
            .unwrap()
            .on
    };
    assert!(lit(state), "the row is lit while its filter is on");
    assert_eq!(state.borrow().views.collections.smart[0].filter, older);
    assert_eq!(
        std::fs::read(&file).unwrap(),
        kept,
        "opening rewrites nothing"
    );
    app.invoke_filter_cleared();
    assert!(!lit(state));
    teardown(s);
}

/// A collection made with the frames in it, saved, by the shoot's
/// files given.
fn made(s: &Setup, name: &str, files: &[&PathBuf]) -> String {
    let mut st = s.state.borrow_mut();
    let at: Vec<usize> = files
        .iter()
        .map(|f| st.files.iter().position(|p| p == *f).unwrap())
        .collect();
    let frames = frames_of(&st, &at);
    let mut c = Collection::new(name);
    c.add(&frames);
    let id = c.id.clone();
    st.views.collections.list.push(c);
    st.views.collections.save(&id);
    id
}

/// A merge read of one collection that lands after another was
/// opened puts what it found into its own collection, not into the
/// one open by then.
#[test]
fn a_late_merge_read_heals_its_own_collection() {
    let s = setup("late-merge");
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    let a = made(&s, "A", &[&s.files[0]]);
    let b = made(&s, "B", &[&s.files[0]]);
    app.invoke_collection_picked(a.as_str().into());
    land_sent(state, app, worker);
    // A's hint gone stale, so its read has a change to bring.
    let stale = s.files[0]
        .with_file_name("gone.tif")
        .to_string_lossy()
        .into_owned();
    state
        .borrow_mut()
        .views
        .collections
        .collection_mut(&a)
        .unwrap()
        .refs[0]
        .hint = stale.clone();
    // A merge of A's list sent, then B opened; B's read lands first.
    assert!(crate::roots::refresh_view(state, app, worker));
    app.invoke_collection_picked(b.as_str().into());
    crate::roots::land_sent_last_first(state, app, worker);
    let st = state.borrow();
    let hint = |id: &str| {
        st.views.collections.collection(id).unwrap().refs[0]
            .hint
            .clone()
    };
    assert_eq!(hint(&b), s.files[0].to_string_lossy(), "B untouched");
    assert_ne!(hint(&a), stale, "A's own read healed A");
    assert_eq!(st.views.view, View::Collection { id: b.clone() });
    drop(st);
    teardown(s);
}

/// The keys come back to the window when the sheet closes: B asks
/// for a new collection, its name typed and Return, and the next B
/// adds to it; Escape on the sheet gives the keys back too.
#[test]
fn b_works_again_after_the_sheet_closes() {
    let s = setup("keys");
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    let x = row_of(&state.borrow(), &s.files[0]);
    app.invoke_select(x);
    crate::testing::press(app, "b");
    assert!(app.get_collection_sheet_open(), "no collection yet: asked");
    crate::testing::press(app, slint::platform::Key::Escape);
    assert!(!app.get_collection_sheet_open());
    crate::testing::press(app, "b");
    assert!(app.get_collection_sheet_open(), "B again after Escape");
    crate::testing::type_text(app, "Picks");
    crate::testing::press(app, slint::platform::Key::Return);
    assert!(!app.get_collection_sheet_open());
    assert_eq!(state.borrow().views.collections.list.len(), 1);
    let y = row_of(&state.borrow(), &s.files[1]);
    app.invoke_select(y);
    crate::testing::press(app, "b");
    assert_eq!(app.get_status(), "1 frame added to Picks");
    assert_eq!(state.borrow().views.collections.list[0].refs.len(), 2);
    let _ = worker;
    teardown(s);
}

/// Save... keeps the view's own scope: a folder of a root's tree is
/// kept as that folder and the folders in it, and opens so; over a
/// collection there is nothing to save.
#[test]
fn save_keeps_a_branch_and_is_not_offered_over_a_collection() {
    let s = setup("scope");
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    let root = s.dir.join("photos");
    let shoot = root.join("shoot");
    open_view(
        state,
        app,
        worker,
        View::Branch {
            root: root.clone(),
            folder: shoot.clone(),
            deep: false,
        },
    );
    land_sent(state, app, worker);
    state.borrow_mut().views.filter.text = "z".into();
    app.set_filter_text("z".into());
    crate::panel::cull::apply_filter(state, app);
    app.invoke_filter_save_smart();
    assert_eq!(
        app.get_collection_sheet_note(),
        format!("\"z\", under {} and the folders in it", shoot.display())
    );
    app.set_collection_sheet_name("Zs".into());
    app.invoke_collection_sheet_done();
    let id = state.borrow().views.collections.smart[0].id.clone();
    assert_eq!(
        state.borrow().views.collections.smart[0].root,
        Some(shoot.clone())
    );
    open_view(state, app, worker, View::Roots(None));
    land_sent(state, app, worker);
    app.invoke_collection_picked(id.as_str().into());
    land_sent(state, app, worker);
    assert_eq!(
        state.borrow().views.view,
        View::Branch {
            root: root.clone(),
            folder: shoot.clone(),
            deep: true
        }
    );
    assert!(
        state
            .borrow()
            .views
            .collections_held
            .rows
            .row_data(0)
            .unwrap()
            .on
    );
    // Over a collection: no button, and the callback says why.
    let c = made(&s, "C", &[&s.files[2]]);
    app.invoke_collection_picked(c.as_str().into());
    land_sent(state, app, worker);
    assert_eq!(app.get_collection_open(), "C");
    app.invoke_filter_save_smart();
    assert!(!app.get_collection_sheet_open());
    assert_eq!(
        app.get_status(),
        "a smart collection is saved from a root's view, not a collection's"
    );
    teardown(s);
}

/// A version named `name` made in the sidecar of the shoot's x on
/// disk, the index brought up to date with it and every root's view
/// opened again: its id.
fn version_of_x(s: &mut Setup, name: &str) -> String {
    let x = &s.files[0];
    let mut sidecar = greycard_edit::Sidecar::load_as_file(x)
        .unwrap()
        .unwrap_or_default();
    let id = sidecar.new_version(None, Some(name)).unwrap();
    sidecar.save(x).unwrap();
    s.writer
        .index_tree(&s.dir.join("photos"), &mut |_| {})
        .unwrap();
    open_view(&s.state, &s.app, &s.worker, View::Roots(None));
    land_sent(&s.state, &s.app, &s.worker);
    id
}

/// Each cell as its file's folder and name, and its version's id.
fn frames(st: &State) -> Vec<(String, Option<String>)> {
    let names = names(st);
    st.cells
        .iter()
        .map(|c| {
            (
                names[c.file].clone(),
                c.version.as_deref().map(str::to_string),
            )
        })
        .collect()
}

/// The row the cell of `path`'s version `version` is on.
fn row_of_frame(st: &State, path: &Path, version: Option<&str>) -> i32 {
    let f = st.files.iter().position(|p| p == path).unwrap();
    let c = st.cell_of(f, version).unwrap();
    crate::panel::browser::row_of(st, c).unwrap() as i32
}

/// A version put in a collection from the menu marks its own cell
/// and no other, and opened, the collection shows that version's
/// tile; B adds the file's own as a frame of its own and takes the
/// version out again by its pair; two versions of one picture are two
/// tiles in the collection's order, each once however many times the
/// file names it; Remove takes the pair out and leaves the other.
#[test]
fn a_version_in_a_collection_is_its_own_tile_and_mark() {
    let mut s = setup("version-tile");
    let warm = version_of_x(&mut s, "Warm");
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    assert_eq!(
        frames(&state.borrow()),
        [
            ("selects/x.tif".to_string(), None),
            ("shoot/x.tif".to_string(), None),
            ("shoot/x.tif".to_string(), Some(warm.clone())),
            ("shoot/y.tif".to_string(), None),
            ("shoot/z.tif".to_string(), None),
        ]
    );
    // Warm alone into a new collection, from the menu.
    let row = row_of_frame(&state.borrow(), &s.files[0], Some(&warm));
    app.invoke_select(row);
    app.invoke_collection_menu_new();
    app.set_collection_sheet_name("Warm ones".into());
    app.invoke_collection_sheet_done();
    let id = state.borrow().views.collections.list[0].id.clone();
    let refs = file_of(&s, &id)["refs"].clone();
    assert_eq!(refs.as_array().unwrap().len(), 1);
    assert_eq!(refs[0]["version"], warm.as_str());
    assert_eq!(file_of(&s, &id)["version"], 2);
    assert_eq!(
        marks(app),
        [false, false, true, false, false],
        "the version's cell alone, not its file's own"
    );
    // B on x's own: a frame of its own, beside the version.
    let own = row_of_frame(&state.borrow(), &s.files[0], None);
    app.invoke_select(own);
    crate::testing::press(app, "b");
    assert_eq!(app.get_status(), "1 frame added to Warm ones");
    // Every copy of the picture wears the original's mark; Warm's
    // cell its own.
    assert_eq!(marks(app), [true, true, true, false, false]);
    // B on Warm, all of it in: Warm out, the original kept.
    app.invoke_select(row);
    crate::testing::press(app, "b");
    assert_eq!(app.get_status(), "1 frame taken out of Warm ones");
    assert_eq!(marks(app), [true, true, false, false, false]);
    assert_eq!(file_of(&s, &id)["version"], 1, "no version left in it");
    crate::testing::press(app, "b");
    assert_eq!(app.get_status(), "1 frame added to Warm ones");

    // Opened: the original, then Warm, in the collection's order, a
    // tile each of the one file.
    app.invoke_collection_picked(id.as_str().into());
    land_sent(state, app, worker);
    assert_eq!(
        frames(&state.borrow()),
        [
            ("shoot/x.tif".to_string(), None),
            ("shoot/x.tif".to_string(), Some(warm.clone())),
        ]
    );
    assert_eq!(names(&state.borrow()), ["shoot/x.tif"]);
    assert_eq!(app.get_status(), "Warm ones: 2 frames");
    assert_eq!(marks(app), [true, true]);
    // A second reference to Warm (two editors' joined files): shown
    // once.
    {
        let mut st = state.borrow_mut();
        let c = st.views.collections.collection_mut(&id).unwrap();
        let again = c.refs[1].clone();
        c.refs.push(again);
    }
    app.invoke_collection_picked(id.as_str().into());
    land_sent(state, app, worker);
    assert_eq!(state.borrow().cells.len(), 2);
    // The menu's Remove on Warm's tile: the pair out, the original
    // kept.
    app.invoke_select(1);
    app.invoke_frame_menu_asked(1);
    app.invoke_collection_menu_remove();
    land_sent(state, app, worker);
    assert_eq!(frames(&state.borrow()), [("shoot/x.tif".to_string(), None)]);
    let refs = state
        .borrow()
        .views
        .collections
        .collection(&id)
        .unwrap()
        .refs
        .clone();
    assert!(refs.iter().all(|r| r.version.is_none()), "{refs:?}");
    teardown(s);
}

/// A version deleted from its sidecar is missing from the collection,
/// said in the status line and kept in the file; Forget missing drops
/// it once the sidecar on disk reads without it.
#[test]
fn a_version_gone_from_its_sidecar_is_missing_and_forgettable() {
    let mut s = setup("version-gone");
    let warm = version_of_x(&mut s, "Warm");
    let id = {
        let st = s.state.borrow();
        let f = st.files.iter().position(|p| *p == s.files[0]).unwrap();
        let cells = [
            st.cell_of(f, None).unwrap(),
            st.cell_of(f, Some(&warm)).unwrap(),
        ];
        let frames = frames_of(&st, &cells);
        drop(st);
        let mut c = Collection::new("Both");
        c.add(&frames);
        let id = c.id.clone();
        let mut st = s.state.borrow_mut();
        st.views.collections.list.push(c);
        st.views.collections.save(&id);
        id
    };
    // Deleted on another machine, the sidecar synced over.
    let mut sidecar = greycard_edit::Sidecar::load_as_file(&s.files[0])
        .unwrap()
        .unwrap();
    assert!(sidecar.delete_version(&warm).is_some());
    sidecar.save(&s.files[0]).unwrap();
    s.writer
        .index_tree(&s.dir.join("photos"), &mut |_| {})
        .unwrap();
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    app.invoke_collection_picked(id.as_str().into());
    land_sent(state, app, worker);
    assert_eq!(frames(&state.borrow()), [("shoot/x.tif".to_string(), None)]);
    assert_eq!(
        app.get_status(),
        "Both: 1 frame shown, 1 missing (moved or deleted, and kept until found)"
    );
    assert_eq!(file_of(&s, &id)["refs"].as_array().unwrap().len(), 2);
    assert_eq!(app.invoke_collection_forgettable(id.as_str().into()), 1);
    app.invoke_collection_forget_missing(id.as_str().into());
    assert_eq!(app.get_status(), "Both: 1 frame; forgot 1 missing");
    let refs = file_of(&s, &id)["refs"].clone();
    assert_eq!(refs.as_array().unwrap().len(), 1);
    assert!(refs[0].get("version").is_none());
    teardown(s);
}

/// A version made while a collection is the list goes into it beside
/// the frame it came from, its cell opened and marked.
#[test]
fn a_version_made_in_a_collection_goes_into_it() {
    let s = setup("version-made");
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    let id = made(&s, "Made here", &[&s.files[0], &s.files[1]]);
    app.invoke_collection_picked(id.as_str().into());
    land_sent(state, app, worker);
    app.invoke_select(0);
    crate::rows::land_pending(state, app, worker);
    crate::panel::versions::new_version(state, app, worker, false, false);
    app.invoke_collection_sheet_done();
    let refs = state
        .borrow()
        .views
        .collections
        .collection(&id)
        .unwrap()
        .refs
        .clone();
    assert_eq!(refs.len(), 3);
    let made = refs[1].version.clone().expect("beside x's own");
    assert_eq!(refs[1].hash, refs[0].hash);
    assert_eq!(
        frames(&state.borrow()),
        [
            ("shoot/x.tif".to_string(), None),
            ("shoot/x.tif".to_string(), Some(made.clone())),
            ("shoot/y.tif".to_string(), None),
        ]
    );
    {
        let st = state.borrow();
        assert_eq!(st.current, Some(1), "the new version's cell open");
    }
    assert_eq!(marks(app), [true, true, true]);
    assert_eq!(file_of(&s, &id)["refs"][1]["version"], made.as_str());
    teardown(s);
}

/// A smart collection is a question over rows: a row that passes
/// brings its versions with it.
#[test]
fn a_smart_collection_lists_a_passing_row_s_versions() {
    let mut s = setup("version-smart");
    let warm = version_of_x(&mut s, "Warm");
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    state.borrow_mut().views.filter.text = "x".into();
    app.set_filter_text("x".into());
    crate::panel::cull::apply_filter(state, app);
    app.invoke_filter_save_smart();
    app.set_collection_sheet_name("Just x".into());
    app.invoke_collection_sheet_done();
    let id = state.borrow().views.collections.smart[0].id.clone();
    app.invoke_filter_cleared();
    assert_eq!(state.borrow().views.shown.len(), 5);
    app.invoke_collection_picked(id.as_str().into());
    land_sent(state, app, worker);
    let st = state.borrow();
    let shown: Vec<(String, Option<String>)> = st
        .views
        .shown
        .iter()
        .map(|&c| frames(&st)[c].clone())
        .collect();
    assert_eq!(
        shown,
        [
            ("selects/x.tif".to_string(), None),
            ("shoot/x.tif".to_string(), None),
            ("shoot/x.tif".to_string(), Some(warm)),
        ]
    );
    drop(st);
    teardown(s);
}

/// A version made in a collection, a merge read of the list landing
/// before the index has read the save (B on another frame while
/// viewing it sends one): the new cell stays, still the frame on
/// screen, and Forget missing keeps it, its sidecar on disk having it.
#[test]
fn a_version_made_survives_a_merge_before_the_index_reads_it() {
    let s = setup("made-merge");
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    let id = made(&s, "Made here", &[&s.files[0], &s.files[1]]);
    app.invoke_collection_picked(id.as_str().into());
    land_sent(state, app, worker);
    app.invoke_select(0);
    crate::rows::land_pending(state, app, worker);
    crate::panel::versions::new_version(state, app, worker, false, false);
    app.invoke_collection_sheet_done();
    assert_eq!(state.borrow().cells.len(), 3);
    let made_id = state
        .borrow()
        .views
        .collections
        .collection(&id)
        .unwrap()
        .refs[1]
        .version
        .clone()
        .unwrap();
    // The index behind: the save is not read into it.
    crate::roots::refresh_view(state, app, worker);
    land_sent(state, app, worker);
    {
        let st = state.borrow();
        assert_eq!(st.cells.len(), 3, "the new version's cell stays");
        let current = st.current.and_then(|c| st.version_of(c));
        assert_eq!(current, Some(made_id.as_str()));
    }
    app.invoke_collection_forget_missing(id.as_str().into());
    assert!(
        app.get_status()
            .ends_with("nothing forgotten: the index has yet to read some of them"),
        "{}",
        app.get_status()
    );
    let refs = state
        .borrow()
        .views
        .collections
        .collection(&id)
        .unwrap()
        .refs
        .clone();
    assert!(
        refs.iter()
            .any(|r| r.version.as_deref() == Some(made_id.as_str())),
        "the version made was forgotten: {refs:?}"
    );
    teardown(s);
}

/// x rewritten in place, its sidecar with Warm beside it, a copy of
/// the old content left in the selects with a sidecar of its own, and
/// no pass report healing the reference: the open finds Warm in the
/// rewritten file's sidecar, by the hint, and Forget missing never
/// takes the copy's sidecar for Warm's.
#[test]
fn a_rewritten_file_s_version_is_not_forgotten_by_a_copy() {
    let mut s = setup("rewrite-version");
    let warm = version_of_x(&mut s, "Warm");
    greycard_edit::Sidecar::default().save(&s.copy).unwrap();
    let id = {
        let st = s.state.borrow();
        let f = st.files.iter().position(|p| *p == s.files[0]).unwrap();
        let frames = frames_of(&st, &[st.cell_of(f, Some(&warm)).unwrap()]);
        drop(st);
        let mut c = Collection::new("Warm");
        c.add(&frames);
        let id = c.id.clone();
        let mut st = s.state.borrow_mut();
        st.views.collections.list.push(c);
        st.views.collections.save(&id);
        id
    };
    let old = file_of(&s, &id)["refs"][0]["hash"]
        .as_str()
        .unwrap()
        .to_string();
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_frame(&s.files[0], &R6, 7);
    let report = s
        .writer
        .index_tree(&s.dir.join("photos"), &mut |_| {})
        .unwrap();
    assert_eq!(report.rehashed.len(), 1);
    let (app, state, worker) = (&s.app, &s.state, &s.worker);
    app.invoke_collection_picked(id.as_str().into());
    land_sent(state, app, worker);
    assert_eq!(
        frames(&state.borrow()),
        [("shoot/x.tif".to_string(), Some(warm.clone()))]
    );
    assert_eq!(app.get_status(), "Warm: 1 frame");
    let refs = file_of(&s, &id)["refs"].clone();
    assert_ne!(refs[0]["hash"].as_str().unwrap(), old, "the hash healed");
    app.invoke_collection_forget_missing(id.as_str().into());
    assert_eq!(app.get_status(), "Warm: 1 frame; nothing to forget");
    assert_eq!(file_of(&s, &id)["refs"].as_array().unwrap().len(), 1);
    teardown(s);
}

/// Collections named Shoot 0000 on, made a second apart in that order,
/// in the store (not yet on disk), and shown; their ids.
fn shoots(state: &Rc<Shared>, app: &App, n: usize) -> Vec<String> {
    let mut st = state.borrow_mut();
    let ids = (0..n)
        .map(|i| {
            let mut c = Collection::new(&format!("Shoot {i:04}"));
            c.made = 1_700_000_000 + i as u64;
            let id = c.id.clone();
            st.views.collections.list.push(c);
            id
        })
        .collect();
    st.views.collections.sort();
    show(&st, app);
    ids
}

/// The names Add to collection lists, in its order.
fn menu_names(app: &App) -> Vec<String> {
    app.get_collection_quick()
        .iter()
        .map(|c| c.name.to_string())
        .collect()
}

/// Add to collection names those added to last first, then the newest
/// made, ten at most, the sidebar's order between those made in the
/// same second; an id deleted since, or named twice, passed over.
#[test]
fn the_menu_names_those_added_to_last_then_the_newest() {
    let list = |made: &[u64]| -> Vec<Collection> {
        made.iter()
            .enumerate()
            .map(|(i, &m)| {
                let mut c = Collection::new(&format!("C{i:04}"));
                c.made = m;
                c
            })
            .collect()
    };
    let rising = |n: u64| list(&(0..n).collect::<Vec<_>>());
    assert_eq!(menu_places(&[], &[]), (vec![], false));
    let one = rising(1);
    assert_eq!(
        menu_places(&one, &["elsewhere".to_string()]),
        (vec![0], false)
    );
    // Ten: every one, the one added to last first, then the newest.
    let ten = rising(10);
    assert_eq!(
        menu_places(&ten, std::slice::from_ref(&ten[2].id)),
        (vec![2, 9, 8, 7, 6, 5, 4, 3, 1, 0], false)
    );
    // Eleven: the newest ten, the newest first.
    let eleven = rising(11);
    assert_eq!(menu_places(&eleven, &[]), ((1..11).rev().collect(), true));
    let recent = [
        eleven[3].id.clone(),
        "deleted".to_string(),
        eleven[3].id.clone(),
        eleven[7].id.clone(),
    ];
    assert_eq!(
        menu_places(&eleven, &recent),
        (vec![3, 7, 10, 9, 8, 6, 5, 4, 2, 1], true)
    );
    // Made in the same second, as an import makes them: the sidebar's
    // first ten.
    assert_eq!(
        menu_places(&list(&[5; 400]), &[]),
        ((0..10).collect(), true)
    );
    // Five thousand, twelve of them added to last: ten named.
    let many = rising(5000);
    let recent: Vec<String> = (0..12).map(|i| many[i * 100].id.clone()).collect();
    assert_eq!(
        menu_places(&many, &recent),
        ((0..10).map(|i| i * 100).collect(), true)
    );
}

/// All collections...'s sheet put up, as its menu item puts it up.
fn open_picker(app: &App) {
    app.set_collection_picker_open(true);
    crate::testing::settle(app);
}

/// The sheet's rows as it has made them, top first: each one's name,
/// whether it is lit, and whether it is wholly in the list's view.
fn picker_rows(app: &App) -> Vec<(String, bool, bool)> {
    use i_slint_backend_testing::ElementHandle;
    crate::testing::settle(app);
    let pane = ElementHandle::find_by_element_id(app, "CollectionPickerSheet::scroll")
        .next()
        .unwrap();
    let (top, foot) = (
        pane.absolute_position().y,
        pane.absolute_position().y + pane.size().height,
    );
    let mut rows: Vec<_> = crate::testing::all_by_id(app, "CollectionPickerSheet::row");
    rows.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    rows.iter()
        .map(|r| {
            let (y, h) = (r.absolute_position().y, r.size().height);
            (
                r.accessible_label().unwrap().to_string(),
                r.accessible_item_selected().unwrap(),
                y >= top - 0.5 && y + h <= foot + 0.5,
            )
        })
        .collect()
}

/// The lit row's name, and whether it is in view.
fn lit(app: &App) -> Option<(String, bool)> {
    picker_rows(app)
        .into_iter()
        .find(|(_, lit, _)| *lit)
        .map(|(name, _, seen)| (name, seen))
}

/// The sheet over five thousand collections lists them from one model
/// that Rust narrows at each key, where a fresh model at each key and a
/// button a collection were ten seconds to open in a debug build; the
/// rows in view are the list's, in order.
#[test]
fn the_picker_narrows_one_held_list() {
    let s = setup("picker-rows");
    let (app, state) = (&s.app, &s.state);
    shoots(state, app, 5000);
    open_picker(app);
    let found = app.get_collection_found();
    assert_eq!(found.row_count(), 5000);
    // The testing backend's search passes over what the pane clips, so
    // the rows it finds are those in view, each at its own place.
    let rows = picker_rows(app);
    assert!(rows.len() > 5 && rows.len() < 20, "{} rows", rows.len());
    assert_eq!(rows[0].0, "Shoot 0000");
    crate::testing::type_text(app, "shoot 49");
    assert_eq!(found.row_count(), 100, "the same model, narrowed");
    let rows = picker_rows(app);
    assert!(rows.len() > 5 && rows.len() < 20, "{} rows", rows.len());
    let names: Vec<String> = (0..rows.len())
        .map(|i| format!("Shoot {:04}", 4900 + i))
        .collect();
    assert_eq!(rows.into_iter().map(|r| r.0).collect::<Vec<_>>(), names);
    teardown(s);
}

/// The keys in the sheet: the first match lit as it opens and at each
/// key typed; Down and Up move the light, which the list keeps in
/// view; Return takes the row lit; Tab reaches the buttons and no row,
/// and a word typed from a button goes to the field.
#[test]
fn the_picker_s_keys_move_the_light_and_return_takes_it() {
    use crate::testing::{press, type_text};
    use slint::platform::Key;
    let s = setup("picker-keys");
    let (app, state) = (&s.app, &s.state);
    let ids = shoots(state, app, 40);
    let x = row_of(&state.borrow(), &s.files[0]);
    app.invoke_select(x);
    open_picker(app);
    assert_eq!(lit(app), Some(("Shoot 0000".into(), true)));
    press(app, Key::DownArrow);
    press(app, Key::DownArrow);
    press(app, Key::UpArrow);
    assert_eq!(lit(app), Some(("Shoot 0001".into(), true)));
    // Up past the top stays there.
    press(app, Key::UpArrow);
    press(app, Key::UpArrow);
    assert_eq!(lit(app), Some(("Shoot 0000".into(), true)));
    // Down past the view: the list follows.
    for _ in 0..30 {
        press(app, Key::DownArrow);
    }
    assert_eq!(lit(app), Some(("Shoot 0030".into(), true)));
    for _ in 0..30 {
        press(app, Key::DownArrow);
    }
    assert_eq!(lit(app), Some(("Shoot 0039".into(), true)));
    press(app, Key::PageUp);
    let (up, seen) = lit(app).unwrap();
    assert!(seen && up.as_str() < "Shoot 0039", "{up}");
    // A key typed lights the first match again.
    type_text(app, "003");
    assert_eq!(lit(app), Some(("Shoot 0003".into(), true)));
    press(app, Key::DownArrow);
    press(app, Key::Return);
    assert!(!app.get_collection_picker_open());
    assert_eq!(app.get_status(), "1 frame added to Shoot 0030");
    assert_eq!(hints(&s, &ids[30]), ["shoot/x.tif"]);

    // Tab from the field: past the rows to the buttons, and a word
    // typed there is the field's.
    open_picker(app);
    press(app, Key::Tab);
    type_text(app, "0007");
    assert_eq!(app.get_collection_found().row_count(), 1);
    assert_eq!(lit(app), Some(("Shoot 0007".into(), true)));
    press(app, Key::Tab);
    press(app, Key::Tab);
    type_text(app, "x");
    assert_eq!(app.get_collection_found().row_count(), 0);
    assert!(app.get_collection_picker_open());
    teardown(s);
}

/// Typed after Tab, on a button: a hyphen, a digit, a character of a
/// script with no case, and Backspace all reach the field.
#[test]
fn every_text_key_typed_on_a_button_goes_to_the_field() {
    use crate::testing::{press, type_text};
    use slint::platform::Key;
    let s = setup("picker-text-keys");
    let (app, state) = (&s.app, &s.state);
    {
        let mut st = state.borrow_mut();
        for name in ["Wed-2", "Wed-3", "Wedge", "日本-1"] {
            st.views.collections.list.push(Collection::new(name));
        }
        st.views.collections.sort();
        show(&st, app);
    }
    let found = || -> Vec<String> {
        app.get_collection_found()
            .iter()
            .map(|c| c.name.to_string())
            .collect()
    };
    open_picker(app);
    press(app, Key::Tab);
    type_text(app, "wed-2");
    assert_eq!(found(), ["Wed-2"]);
    press(app, Key::Backspace);
    assert_eq!(found(), ["Wed-2", "Wed-3"]);
    // Backspace from the button, and a script with no case.
    press(app, Key::Tab);
    for _ in 0..4 {
        press(app, Key::Backspace);
    }
    assert_eq!(found().len(), 4);
    press(app, Key::Tab);
    type_text(app, "日");
    assert_eq!(found(), ["日本-1"]);
    assert!(app.get_collection_picker_open());
    teardown(s);
}

/// Escape closes the sheet and adds nothing; words that match nothing
/// say so, and Return then takes nothing and leaves the sheet up.
#[test]
fn the_picker_closes_on_escape_and_takes_nothing_with_no_match() {
    use crate::testing::{press, type_text};
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::Key;
    let s = setup("picker-none");
    let (app, state) = (&s.app, &s.state);
    shoots(state, app, 12);
    let x = row_of(&state.borrow(), &s.files[0]);
    app.invoke_select(x);
    open_picker(app);
    press(app, Key::Escape);
    assert!(!app.get_collection_picker_open());
    open_picker(app);
    type_text(app, "zz");
    assert_eq!(app.get_collection_found().row_count(), 0);
    assert!(picker_rows(app).is_empty());
    assert!(
        ElementHandle::find_by_element_id(app, "CollectionPickerSheet::none")
            .next()
            .is_some()
    );
    let status = app.get_status();
    press(app, Key::Return);
    assert!(app.get_collection_picker_open());
    assert_eq!(app.get_status(), status);
    press(app, Key::Escape);
    assert!(!app.get_collection_picker_open());
    let st = state.borrow();
    assert!(st.views.collections.list.iter().all(|c| c.refs.is_empty()));
    drop(st);
    teardown(s);
}

/// New collection... on the sheet: the sheet goes, the name is asked
/// for, and the selection goes into the collection made.
#[test]
fn new_collection_on_the_picker_asks_a_name_and_fills_it() {
    use i_slint_backend_testing::ElementHandle;
    let s = setup("picker-new");
    let (app, state) = (&s.app, &s.state);
    shoots(state, app, 12);
    let x = row_of(&state.borrow(), &s.files[0]);
    app.invoke_select(x);
    open_picker(app);
    ElementHandle::find_by_element_id(app, "CollectionPickerSheet::new-collection")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    crate::testing::settle(app);
    assert!(!app.get_collection_picker_open());
    assert!(app.get_collection_sheet_open());
    assert_eq!(app.get_collection_sheet_note(), "1 frame go into it");
    app.set_collection_sheet_name("Fresh".into());
    app.invoke_collection_sheet_done();
    let id = {
        let st = state.borrow();
        let c = st.views.collections.list.iter().find(|c| c.name == "Fresh");
        c.unwrap().id.clone()
    };
    assert_eq!(hints(&s, &id), ["shoot/x.tif"]);
    assert_eq!(menu_names(app)[0], "Fresh");
    teardown(s);
}

/// The sheet adds to the collection by its id: one deleted while the
/// sheet was up is said, by its name, and nothing is added; with one
/// above it deleted, the row lit still goes to its own.
#[test]
fn the_picker_adds_by_id_and_says_a_collection_gone() {
    use crate::testing::{press, type_text};
    use slint::platform::Key;
    let s = setup("picker-gone");
    let (app, state) = (&s.app, &s.state);
    let ids = shoots(state, app, 25);
    let x = row_of(&state.borrow(), &s.files[0]);
    app.invoke_select(x);
    open_picker(app);
    type_text(app, "0012");
    assert!(state.borrow_mut().views.collections.delete(&ids[12]));
    press(app, Key::Return);
    assert!(!app.get_collection_picker_open());
    assert_eq!(app.get_status(), "Shoot 0012 is no longer in the list");
    {
        let st = state.borrow();
        assert!(st.views.collections.list.iter().all(|c| c.refs.is_empty()));
    }
    open_picker(app);
    press(app, Key::DownArrow);
    press(app, Key::DownArrow);
    assert_eq!(lit(app).unwrap().0, "Shoot 0002");
    {
        let mut st = state.borrow_mut();
        assert!(st.views.collections.delete(&ids[0]));
        show(&st, app);
    }
    press(app, Key::Return);
    assert_eq!(app.get_status(), "1 frame added to Shoot 0002");
    assert_eq!(hints(&s, &ids[2]), ["shoot/x.tif"]);
    // From the submenu, the collection deleted (as another window
    // deletes it) and the window's lists made again while it was up:
    // still said by the name the menu offered.
    let offered = app.get_collection_quick().row_data(1).unwrap();
    {
        let mut st = state.borrow_mut();
        assert!(st.views.collections.delete(&offered.id));
        show(&st, app);
    }
    assert!(
        app.get_collection_quick()
            .iter()
            .all(|c| c.id != offered.id)
    );
    // Nor in the sheet's list, last narrowed to nothing.
    app.invoke_collection_search("zz".into());
    app.invoke_collection_menu_add(offered.clone());
    assert_eq!(
        app.get_status(),
        format!("{} is no longer in the list", offered.name)
    );
    teardown(s);
}

/// Past ten, Add to collection names the newest made, the newest
/// first; one added to from the sheet then heads them, and is kept in
/// the library's own `recent.txt`, which a second library does not
/// share and a reload of the folder (a Lightroom import's) keeps.
#[test]
fn the_ones_added_to_last_head_the_menu_and_stay_with_the_library() {
    use crate::testing::{press, type_text};
    use slint::platform::Key;
    let s = setup("picker-recent");
    let (app, state) = (&s.app, &s.state);
    let ids = shoots(state, app, 25);
    let newest: Vec<String> = (15..25).rev().map(|i| format!("Shoot {i:04}")).collect();
    assert_eq!(menu_names(app), newest);
    assert!(app.get_collection_has_more());
    let x = row_of(&state.borrow(), &s.files[0]);
    app.invoke_select(x);
    open_picker(app);
    type_text(app, "0007");
    press(app, Key::Return);
    assert_eq!(app.get_status(), "1 frame added to Shoot 0007");
    let named = menu_names(app);
    assert_eq!(named.len(), 10);
    assert_eq!(named[..3], ["Shoot 0007", "Shoot 0024", "Shoot 0023"]);
    let folder = s.dir.join(crate::collections::DIR);
    assert_eq!(
        greycard_library::collections::read_recent(&folder),
        [ids[7].clone()]
    );
    let reloaded = crate::collections::Store::load(folder.clone());
    assert_eq!(reloaded.recent, [ids[7].clone()]);
    let other = s.dir.join("other").join(crate::collections::DIR);
    assert!(crate::collections::Store::load(other).recent.is_empty());
    teardown(s);
}

/// A settings file an earlier build kept the list in: carried into the
/// library's folder once, when the folder has no list yet, and taken
/// out of the settings, so a library opened next does not take it; a
/// folder that has its own keeps it.
#[test]
fn the_settings_list_is_carried_to_the_library_once() {
    use greycard_library::collections::{added_to, read_recent};
    let app = window(0);
    let (state, _worker) = state_for(&app, Vec::new());
    let dir = crate::testing::scratch_dir("coll-carry");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("settings.json");
    let old = r#"{"recent_collections": ["b", "a"], "range_sample": 41}"#;
    std::fs::write(&file, old).unwrap();
    let (one, two, three) = (dir.join("one"), dir.join("two"), dir.join("three"));
    added_to(&three, "own").unwrap();
    let start = |library: &Path| {
        let kept = crate::settings::Settings::load_from(&file).recent_collections;
        let mut st = state.borrow_mut();
        st.window.settings_file = Some(file.clone());
        st.views.collections = crate::collections::Store::load(library.to_path_buf());
        carry_recent(&mut st, &kept);
        st.views.collections.recent.clone()
    };
    // A folder that will not take the file (here a file where the
    // folder would be, as a share not mounted or a read-only folder
    // refuses it): the settings keep the list for the next start.
    let blocked = dir.join("blocked");
    std::fs::write(&blocked, "").unwrap();
    assert!(start(&blocked).is_empty());
    // And through a write of another setting before then.
    crate::settings::Settings::change_at(&file, |s| s.range_sample = 7);
    assert_eq!(
        crate::settings::Settings::load_from(&file).recent_collections,
        ["b", "a"]
    );
    // A folder with a list of its own keeps it, and the settings' list
    // is dropped.
    assert_eq!(start(&three), ["own"]);
    assert!(
        crate::settings::Settings::load_from(&file)
            .recent_collections
            .is_empty()
    );
    std::fs::write(&file, old).unwrap();
    assert_eq!(start(&one), ["b", "a"]);
    assert_eq!(read_recent(&one), ["b", "a"]);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(!text.contains("recent_collections"), "{text}");
    assert_eq!(crate::settings::Settings::load_from(&file).range_sample, 41);
    assert!(start(&two).is_empty());
    assert!(read_recent(&two).is_empty());
    drop(app);
    crate::testing::remove_dir_retry(&dir);
}

/// The frame menu's submenu `title` (as the window shows it) opened
/// over the strip's first frame, `then` run once the menu is up (for
/// what Rust sets as it opens): how many of its entries start with
/// `prefix` (the list's), how many are each of `others` (catalog
/// msgids), and the foot of the lowest of those in the popup; the
/// menus closed again.
fn submenu(
    app: &App,
    title: &str,
    prefix: &str,
    others: &[&str],
    then: &dyn Fn(&App),
) -> (usize, Vec<usize>, f32) {
    use slint::platform::Key;
    // As `--menu` opens it; a click at the strip's cell misses a
    // strip pushed past the foot of a window under the layout's least.
    app.set_menu_at(0);
    crate::testing::settle(app);
    assert!(app.get_menu_up());
    then(app);
    crate::testing::settle(app);
    crate::testing::open_submenu(app, title);
    let others: Vec<String> = others.iter().map(|o| crate::i18n::text(o)).collect();
    let mine: Vec<_> = crate::testing::menu_entries(app)
        .into_iter()
        .filter(|e| {
            e.accessible_label()
                .is_some_and(|l| l.starts_with(prefix) || others.contains(&l.to_string()))
        })
        .collect();
    let named = crate::testing::menu_entries_starting(app, prefix);
    let counts = others
        .iter()
        .map(|o| {
            mine.iter()
                .filter(|e| e.accessible_label().as_deref() == Some(o.as_str()))
                .count()
        })
        .collect();
    let foot = mine
        .iter()
        .map(|e| e.absolute_position().y + e.size().height)
        .fold(0.0, f32::max);
    crate::testing::press(app, Key::Escape);
    crate::testing::press(app, Key::Escape);
    assert!(!app.get_menu_up());
    // Seen let go, so the next ask is a change.
    app.set_menu_at(-1);
    crate::testing::settle(app);
    (named, counts, foot)
}

/// Below its lowest entry a fluent menu has its frame's margin (5 px)
/// and its shadow's blur (16 px), all drawn into the shadow's texture:
/// what must fit in the window for the texture to stay under the
/// device's limit, which the window's own surface is held to.
const BELOW_THE_LOWEST: f32 = 5.0 + 16.0;

/// The frame menu's lists, opened as a hand opens them, over 400
/// names each, in a tall window and in one shorter than the layout's
/// least: Add to collection names ten, or what the window holds, with
/// All collections... and New collection...; Edit in, which Rust does
/// not cut, as many apps as the window holds (`MenuFit`). Each
/// submenu, its frame and its shadow inside the window: 400 rows were
/// 12,883 px.
#[test]
fn the_frame_menus_lists_fit_the_window() {
    let app = window(3);
    let (state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    app.set_busy(false);
    shoots(&state, &app, 400);
    // Apps on the disk, as the menu offers only those: this test's own
    // binary under 400 names.
    let exe = std::env::current_exe().unwrap();
    state.borrow_mut().work.edit_in.apps = (0..400)
        .map(|i| (exe.clone(), format!("App {i:03}")))
        .collect();
    let title = |msgid: &str| crate::i18n::text(msgid);
    for height in [950.0, 420.0] {
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, height));
        crate::testing::settle(&app);
        let fit = app.global::<crate::MenuFit>();
        assert_eq!(fit.get_window_height(), height);
        let (named, others, foot) = submenu(
            &app,
            &title("Add to collection"),
            "Shoot ",
            &["All collections...", "New collection..."],
            &|_| {},
        );
        assert_eq!(
            (named, others),
            (10.min(fit.invoke_rows(3) as usize), vec![1, 1]),
            "{height}"
        );
        assert!(
            foot + BELOW_THE_LOWEST <= height,
            "Add to collection: {foot} in {height}"
        );
        let (named, _, foot) = submenu(&app, &title("Edit in"), "App ", &[], &|_| {});
        assert_eq!(named, fit.invoke_rows(0) as usize, "{height}");
        assert!(named < 400);
        assert!(
            foot + BELOW_THE_LOWEST <= height,
            "Edit in: {foot} in {height}"
        );
    }
}

/// Ten collections, all of them named while the window holds them; in
/// a window too short for ten, those it holds, and All collections...
/// for the rest though the library has no more than ten.
#[test]
fn all_collections_shows_when_the_window_cuts_ten_or_fewer() {
    let app = window(3);
    let (state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    app.set_busy(false);
    shoots(&state, &app, 10);
    assert!(!app.get_collection_has_more());
    let title = crate::i18n::text("Add to collection");
    let others = &["All collections...", "New collection..."];
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    crate::testing::settle(&app);
    let (named, counts, _) = submenu(&app, &title, "Shoot ", others, &|_| {});
    assert_eq!((named, counts), (10, vec![0, 1]));
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 420.0));
    crate::testing::settle(&app);
    // 13 rows of 32 px, two kept, three the submenu's own.
    assert_eq!(app.global::<crate::MenuFit>().invoke_rows(3), 8);
    let (named, counts, foot) = submenu(&app, &title, "Shoot ", others, &|_| {});
    assert_eq!((named, counts), (8, vec![1, 1]));
    assert!(foot + BELOW_THE_LOWEST <= 420.0, "{foot}");
}

/// Every menu fed from a list, over 400 and 5,000 names at three times
/// scale: the frame menu's Add to collection, Edit in, and Back up to,
/// the grid header's archive button and its rejects menu, and the
/// recent folders. Each names no more than the window holds, so its
/// surface stays under the device's texture limit; 400 names at this
/// scale were a surface of 38,000 px. (The presets' Move to group is
/// `preset_list`'s.)
#[test]
fn no_list_fed_menu_passes_the_texture_limit() {
    use crate::testing::{TEXTURE_LIMIT, menu_entries_starting, menu_surface, press, settle};
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::{Key, WindowEvent};
    let names = |prefix: &str, n: usize| -> ModelRc<slint::SharedString> {
        let list: Vec<slint::SharedString> =
            (0..n).map(|i| format!("{prefix}{i:04}").into()).collect();
        ModelRc::new(VecModel::from(list))
    };
    for n in [400, 5000] {
        let s = setup(&format!("surface-{n}"));
        let (app, state) = (&s.app, &s.state);
        app.window()
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 3.0 });
        settle(app);
        shoots(state, app, n);
        let exe = std::env::current_exe().unwrap();
        state.borrow_mut().work.edit_in.apps = (0..n)
            .map(|i| (exe.clone(), format!("App {i:04}")))
            .collect();
        app.invoke_select(0);
        app.set_busy(false);
        let fits = |what: &str, named: usize, own: usize| {
            let surface = menu_surface(app, named, own);
            assert!(named > 0, "{what}: none named");
            assert!(surface <= TEXTURE_LIMIT, "{what} over {n}: {surface} px");
        };
        let title = |msgid: &str| crate::i18n::text(msgid);
        let (named, ..) = submenu(app, &title("Add to collection"), "Shoot ", &[], &|_| {});
        fits("Add to collection", named, 3);
        let (named, ..) = submenu(app, &title("Edit in"), "App ", &[], &|_| {});
        fits("Edit in", named, 0);
        // As though the selection could go to n archives: what Rust
        // sets as the menu opens, set by the test in its place.
        {
            let app_weak = app.as_weak();
            let choices = names("Archive ", n);
            app.on_frame_menu_asked(move |_| {
                let app = app_weak.upgrade().unwrap();
                app.set_archive_verb("Back up".into());
                app.set_archive_choices(choices.clone());
            });
        }
        let (named, ..) = submenu(app, "Back up to", "Archive ", &[], &|_| {});
        fits("Back up to", named, 0);

        // The grid header's buttons.
        app.set_archive_header("Back up".into());
        app.set_archive_header_choices(names("Header ", n));
        app.set_archive_rejects_choices(names("Rejects ", n));
        settle(app);
        for (button, prefix, own) in [
            ("GridSheet::archive-button", "Header ", 0),
            ("GridSheet::rejects-button", "Rejects ", 5),
        ] {
            ElementHandle::find_by_element_id(app, button)
                .next()
                .unwrap_or_else(|| panic!("{button}"))
                .invoke_accessible_default_action();
            settle(app);
            fits(button, menu_entries_starting(app, prefix), own);
            press(app, Key::Escape);
            settle(app);
        }

        // The recent folders, under the folder's name.
        let folders: Vec<crate::RecentFolder> = (0..n)
            .map(|i| crate::RecentFolder {
                title: format!("Folder {i:04}").into(),
                path: format!("/nowhere/{i}").into(),
                open: false,
            })
            .collect();
        app.set_grid_open(false);
        app.set_open_folder_name("Here".into());
        app.set_recent_folders(ModelRc::new(VecModel::from(folders)));
        settle(app);
        ElementHandle::find_by_element_type_name(app, "FolderName")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        settle(app);
        fits("Recent folders", menu_entries_starting(app, "Folder "), 2);
        press(app, Key::Escape);
        teardown(s);
    }
}
