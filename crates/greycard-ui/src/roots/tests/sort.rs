//! The grid's order: a view's read in it, a choice from the Sort menu
//! read again in place, and every walker of the list following it.

use super::*;
use crate::sort::Choice;
use greycard_library::fixture::Frame;
use greycard_library::{Order, SortBy};
use slint::Model;
use slint::platform::Key;

/// Two bodies' frames in two folders of one root, their letter prefixes
/// apart and their capture times interleaved: `b/_DSC0001` at 10:00:00,
/// `a/IMG_0001` at :01, `b/_DSC0002` at :02, `a/IMG_0002` at :03.
fn two_bodies(what: &str) -> (PathBuf, PathBuf, greycard_library::Library) {
    let dir = scratch(what);
    let root = dir.join("root");
    let (a, b) = (root.join("a"), root.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let at = |s: u32| format!("2026:05:01 10:00:{s:02}");
    let shots = [
        (a.join("IMG_0001.tif"), at(1), &R5),
        (a.join("IMG_0002.tif"), at(3), &R5),
        (b.join("_DSC0001.tif"), at(0), &A7),
        (b.join("_DSC0002.tif"), at(2), &A7),
    ];
    for (i, (path, taken, body)) in shots.iter().enumerate() {
        let frame = Frame { taken, ..**body };
        write_frame(path, &frame, i as u16);
    }
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&root, &mut |_| {}).unwrap();
    (dir, root, writer)
}

/// A window on the root, the library open as the editor has it.
fn window_on(dir: &Path, root: &Path) -> (App, Rc<RefCell<State>>, Rc<Worker>) {
    let db = dir.join("index").join("library.sqlite");
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.views.index_path = Some(db);
        st.views.library.roots.add(root).unwrap();
    }
    (app, state, worker)
}

/// The list's files by name, in its order.
fn listed(st: &State) -> Vec<String> {
    st.files
        .iter()
        .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

/// The strip's and the grid's rows by the names under their pictures.
fn rows_shown(app: &App) -> Vec<String> {
    app.get_thumbs()
        .iter()
        .map(|t| t.name.to_string())
        .collect()
}

/// The frame on screen, by name.
fn on_screen(st: &State) -> Option<String> {
    let c = st.current?;
    Some(st.path_at(c)?.file_name()?.to_string_lossy().into_owned())
}

fn teardown(dir: &Path, state: Rc<RefCell<State>>, writer: greycard_library::Library) {
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(dir);
}

/// Every root opens by capture time, the two bodies' frames as the day
/// went; Name from the menu reads the list again by name with the frame
/// on screen still chosen and its row brought into view; the way up
/// turned reverses it; the strip's rows and culling's arrows, Home and
/// End all walk the list in its order.
#[test]
fn every_root_reads_by_capture_time_and_a_sort_keeps_the_frame_on_screen() {
    let (dir, root, writer) = two_bodies("every-root");
    let (app, state, worker) = window_on(&dir, &root);
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    let by_time = [
        "_DSC0001.tif",
        "IMG_0001.tif",
        "_DSC0002.tif",
        "IMG_0002.tif",
    ];
    assert_eq!(listed(&state.borrow()), by_time);
    assert_eq!(rows_shown(&app), by_time);
    let menu = app.get_sort_menu();
    assert_eq!(menu.words, "Capture time");
    assert_eq!(menu.tip, "Sort: Capture time, oldest first");
    assert_eq!(menu.label, "capture time");
    assert_eq!(menu.checked, 0);
    assert!(!menu.descending);
    assert_eq!(
        menu.entries.iter().collect::<Vec<_>>(),
        ["Capture time", "Name", "Rating", "Date edited"]
    );

    // IMG_0001 chosen, then Name from the menu: the list read again in
    // place, IMG_0001 still the frame, on its new row.
    app.invoke_select(1);
    assert_eq!(on_screen(&state.borrow()).as_deref(), Some("IMG_0001.tif"));
    app.invoke_sort_picked(1);
    assert!(
        state.borrow().views.sort_reveal,
        "a read is out for the sort"
    );
    land_all(&state, &app, &worker);
    let by_name = [
        "IMG_0001.tif",
        "IMG_0002.tif",
        "_DSC0001.tif",
        "_DSC0002.tif",
    ];
    assert_eq!(listed(&state.borrow()), by_name);
    assert_eq!(rows_shown(&app), by_name);
    assert_eq!(on_screen(&state.borrow()).as_deref(), Some("IMG_0001.tif"));
    assert_eq!(app.get_selected(), 0);
    assert!(
        !state.borrow().views.sort_reveal,
        "brought into view as it landed"
    );
    assert_eq!(app.get_sort_menu().words, "Name");
    assert_eq!(app.get_sort_menu().label, "name");
    assert_eq!(
        state.borrow().views.sort.choice(&View::Roots(None)),
        Choice::By(Order::NAME)
    );
    // Kept for every view of several folders, and for those alone.
    assert_eq!(state.borrow().views.sort.roots.as_deref(), Some("name"));
    assert_eq!(state.borrow().views.sort.folder, None);

    // Culling's arrows walk the list in the order chosen, and Home and
    // End go to its ends.
    app.invoke_cull_toggled();
    assert!(state.borrow().culling.cull.is_some());
    crate::testing::press(&app, Key::RightArrow);
    assert_eq!(on_screen(&state.borrow()).as_deref(), Some("IMG_0002.tif"));
    crate::testing::press(&app, Key::RightArrow);
    assert_eq!(on_screen(&state.borrow()).as_deref(), Some("_DSC0001.tif"));
    app.invoke_step_end(true, false);
    assert_eq!(on_screen(&state.borrow()).as_deref(), Some("_DSC0002.tif"));

    // The other way up, from the menu: Z to A, the frame kept.
    app.invoke_sort_turned(true);
    land_all(&state, &app, &worker);
    let back = [
        "_DSC0002.tif",
        "_DSC0001.tif",
        "IMG_0002.tif",
        "IMG_0001.tif",
    ];
    assert_eq!(listed(&state.borrow()), back);
    assert_eq!(rows_shown(&app), back);
    assert_eq!(on_screen(&state.borrow()).as_deref(), Some("_DSC0002.tif"));
    assert_eq!(app.get_selected(), 0);
    assert!(app.get_sort_menu().descending);
    assert_eq!(app.get_sort_menu().tip, "Sort: Name, Z to A");
    crate::testing::press(&app, Key::RightArrow);
    assert_eq!(on_screen(&state.borrow()).as_deref(), Some("_DSC0001.tif"));
    app.invoke_step_end(false, false);
    assert_eq!(on_screen(&state.borrow()).as_deref(), Some("_DSC0002.tif"));
    app.invoke_cull_leave();

    // The strip's arrows outside culling walk it the same way.
    app.invoke_step(1, false);
    assert_eq!(on_screen(&state.borrow()).as_deref(), Some("_DSC0001.tif"));
    teardown(&dir, state, writer);
}

/// A sort chosen while a view of the same kind is still opening (one
/// root's chip pressed from every root's view, its read out) cannot be
/// read again then; the view lands in the order it was asked in, and
/// is read again at once in the order chosen, the menu and the list
/// agreeing.
#[test]
fn a_sort_chosen_while_a_view_opens_is_read_again_when_it_lands() {
    let (dir, root, writer) = two_bodies("while-opening");
    let (app, state, worker) = window_on(&dir, &root);
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    open_view(&state, &app, &worker, View::Roots(Some(root.clone())));
    assert!(
        state.borrow().views.library.loading,
        "the root's read is out"
    );
    app.invoke_sort_picked(1);
    assert_eq!(app.get_sort_menu().words, "Name");
    assert!(state.borrow().views.sort_reveal, "kept for the landing");
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().views.view, View::Roots(Some(root.clone())));
    let by_name = [
        "IMG_0001.tif",
        "IMG_0002.tif",
        "_DSC0001.tif",
        "_DSC0002.tif",
    ];
    assert_eq!(listed(&state.borrow()), by_name);
    assert_eq!(rows_shown(&app), by_name);
    assert_eq!(app.get_sort_menu().words, "Name");
    assert!(!state.borrow().views.sort_reveal);
    teardown(&dir, state, writer);
}

/// A folder opened from the disk is by name, the camera's numbering,
/// until another order is chosen for folders; by capture time it is
/// sorted by the same rule from the index's rows. The choice is kept
/// for folders alone: every root stays by capture time.
#[test]
fn a_folder_from_the_disk_is_by_name_until_its_order_is_chosen() {
    let (dir, root, writer) = two_bodies("one-folder");
    let (app, state, worker) = window_on(&dir, &root);
    // A third frame whose name comes first and whose time is the
    // latest: a copy of IMG_0002, named IMG_0000.
    let a = root.join("a");
    std::fs::copy(a.join("IMG_0002.tif"), a.join("IMG_0000.tif")).unwrap();
    let mut writer = writer;
    writer.index_folder(&a, &mut |_| {}).unwrap();
    open_listing(
        &state,
        &app,
        &worker,
        Listing::Folder(a.clone()),
        Select::Row(0),
    );
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().views.view, View::Folder);
    assert_eq!(
        listed(&state.borrow()),
        ["IMG_0000.tif", "IMG_0001.tif", "IMG_0002.tif"]
    );
    assert_eq!(app.get_sort_menu().words, "Name");
    // Capture time, newest first: IMG_0000 and IMG_0002 share a time
    // (one is a copy of the other) and go by name.
    let newest = Choice::By(Order::new(SortBy::Taken, true));
    crate::panel::sort::choose(&state, &app, &worker, &View::Folder, newest);
    land_all(&state, &app, &worker);
    assert_eq!(
        listed(&state.borrow()),
        ["IMG_0000.tif", "IMG_0002.tif", "IMG_0001.tif"]
    );
    assert_eq!(app.get_sort_menu().words, "Capture time");
    assert_eq!(
        state.borrow().views.sort.folder.as_deref(),
        Some("capture-desc")
    );
    assert_eq!(
        state.borrow().views.sort.choice(&View::Roots(None)),
        Choice::By(Order::TAKEN)
    );
    teardown(&dir, state, writer);
}

/// A collection is in its own hand order until a sort is chosen, which
/// lays it aside without rewriting it; "Hand order", the first entry of
/// a collection's menu, brings it back, and a drag moves frames only in
/// it.
#[test]
fn a_collection_keeps_its_hand_order_until_a_sort_and_comes_back_to_it() {
    let (dir, root, writer) = two_bodies("collection");
    let (app, state, worker) = window_on(&dir, &root);
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    // By hand: IMG_0002, _DSC0001, IMG_0001.
    let id = {
        let mut st = state.borrow_mut();
        let file = |n: &str| st.files.iter().position(|f| f.ends_with(n)).unwrap();
        let picked = [
            file("IMG_0002.tif"),
            file("_DSC0001.tif"),
            file("IMG_0001.tif"),
        ];
        let frames = crate::panel::collections::frames_of(&st, &picked);
        let mut c = crate::collections::Collection::new("Picks");
        c.add(&frames);
        let id = c.id.clone();
        st.views.collections.list.push(c);
        id
    };
    let view = View::Collection { id: id.clone() };
    open_view(&state, &app, &worker, view.clone());
    land_all(&state, &app, &worker);
    let hand = ["IMG_0002.tif", "_DSC0001.tif", "IMG_0001.tif"];
    assert_eq!(rows_shown(&app), hand);
    let menu = app.get_sort_menu();
    assert_eq!(menu.words, "Hand order");
    assert_eq!(menu.tip, "Sort: Hand order");
    assert_eq!(menu.entries.iter().next().unwrap(), "Hand order");
    assert_eq!(menu.checked, 0);
    assert_eq!(menu.ways.row_count(), 0, "a hand order has no way up");
    assert!(app.get_collection_reorder());

    // Capture time, the second entry: by time, the file untouched.
    app.invoke_select(2);
    app.invoke_sort_picked(1);
    land_all(&state, &app, &worker);
    assert_eq!(
        rows_shown(&app),
        ["_DSC0001.tif", "IMG_0001.tif", "IMG_0002.tif"]
    );
    assert_eq!(on_screen(&state.borrow()).as_deref(), Some("IMG_0001.tif"));
    assert_eq!(app.get_selected(), 1);
    assert!(!app.get_collection_reorder(), "no drag in an order chosen");
    let hints: Vec<String> = state
        .borrow()
        .views
        .collections
        .collection(&id)
        .unwrap()
        .refs
        .iter()
        .map(|r| {
            Path::new(&r.hint)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(hints, hand, "the collection's own order is kept");
    let row = state.borrow().views.collections.rows.row_data(0).unwrap();
    assert!(row.capture);
    assert_eq!(row.order, "capture time", "the open row names its order");

    // Hand order again.
    app.invoke_sort_picked(0);
    land_all(&state, &app, &worker);
    assert_eq!(rows_shown(&app), hand);
    assert_eq!(app.get_sort_menu().words, "Hand order");
    assert!(app.get_collection_reorder());
    // Newest first is an order chosen too: the row's Capture order is
    // checked, and takes it back to its own order, not to oldest first.
    app.invoke_sort_picked(1);
    land_all(&state, &app, &worker);
    app.invoke_sort_turned(true);
    land_all(&state, &app, &worker);
    assert_eq!(
        rows_shown(&app),
        ["IMG_0002.tif", "IMG_0001.tif", "_DSC0001.tif"]
    );
    assert!(
        state
            .borrow()
            .views
            .collections
            .rows
            .row_data(0)
            .unwrap()
            .capture
    );
    app.invoke_collection_capture(id.as_str().into());
    land_all(&state, &app, &worker);
    assert_eq!(rows_shown(&app), hand);
    assert_eq!(app.get_sort_menu().words, "Hand order");
    let row = state.borrow().views.collections.rows.row_data(0).unwrap();
    assert!(!row.capture);
    assert_eq!(row.order, "");
    // Another collection is in its own order still.
    assert_eq!(
        state
            .borrow()
            .views
            .sort
            .choice(&View::Collection { id: "other".into() }),
        Choice::Hand
    );
    teardown(&dir, state, writer);
}
