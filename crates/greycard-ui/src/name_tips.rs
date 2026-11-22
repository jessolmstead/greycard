//! The whole of a name the window cuts short, at the pointer after a
//! rest: the strip's and the grid's names, the grid's folder tiles, the
//! grid header's line naming the chosen frame, the open frame's name
//! over the Develop panel, and the folder tree. A name that fits says
//! nothing, and a press takes the name down until the pointer leaves.

use i_slint_backend_testing::{ElementHandle, mock_elapsed_time};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::time::Duration;

use std::path::PathBuf;
use std::rc::Rc;

use crate::testing::{settle, state_for, window};
use crate::{App, FolderRow, FolderTile, Shared, Thumb};

const LONG: &str = "a frame named at length off a card reader, 0001.CR3";
const SHORT: &str = "b.CR3";

/// The middles of every Text drawing `words`, top first; its own
/// label is its words, whole.
fn texts(app: &App, words: &str) -> Vec<(f32, f32)> {
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(0.0, 0.0),
    });
    // A frame's name is said by its cell, a folder's by its words.
    let mut found: Vec<_> = ElementHandle::find_by_element_type_name(app, "Text")
        .filter(|e| e.accessible_label().is_some_and(|l| l == words))
        .chain(crate::testing::cell_names(app, words))
        .filter(|e| e.size().width > 0.0)
        .map(|e| {
            let (at, size) = (e.absolute_position(), e.size());
            (at.x + size.width / 2.0, at.y + size.height / 2.0)
        })
        .collect();
    found.sort_by(|a, b| a.1.total_cmp(&b.1));
    found
}

fn move_to(app: &App, (x, y): (f32, f32)) {
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(x, y),
    });
    slint::platform::update_timers_and_animations();
}

fn rest(app: &App, at: (f32, f32)) {
    move_to(app, at);
    mock_elapsed_time(Duration::from_millis(800));
    slint::platform::update_timers_and_animations();
}

fn press_at(app: &App, button: PointerEventButton, (x, y): (f32, f32)) {
    let position = slint::LogicalPosition::new(x, y);
    app.window()
        .dispatch_event(WindowEvent::PointerPressed { position, button });
    app.window()
        .dispatch_event(WindowEvent::PointerReleased { position, button });
    slint::platform::update_timers_and_animations();
}

/// A left press held on `from`, slid onto `to`, and held there past a
/// rest: the hover text then, before the release at `to`.
fn held_onto(app: &App, from: (f32, f32), to: (f32, f32)) -> String {
    move_to(app, from);
    let button = PointerEventButton::Left;
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: slint::LogicalPosition::new(from.0, from.1),
        button,
    });
    move_to(app, ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0));
    move_to(app, to);
    mock_elapsed_time(Duration::from_millis(800));
    slint::platform::update_timers_and_animations();
    let tip = app.get_tip_text().to_string();
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: slint::LogicalPosition::new(to.0, to.1),
        button,
    });
    slint::platform::update_timers_and_animations();
    tip
}

fn away(app: &App) {
    move_to(app, (2.0, 2.0));
}

/// The rest, the press and the leaving at `long`, and nothing at
/// `short` when the place has one beside it, with `button` for the
/// press.
fn says_the_whole(
    app: &App,
    long: (f32, f32),
    short: Option<(f32, f32)>,
    name: &str,
    button: PointerEventButton,
) {
    away(app);
    assert_eq!(app.get_tip_text(), "", "nothing before a rest");
    rest(app, long);
    assert_eq!(app.get_tip_text(), name, "the whole name after a rest");
    away(app);
    assert_eq!(app.get_tip_text(), "", "the name leaves with the pointer");
    rest(app, long);
    assert_eq!(app.get_tip_text(), name);
    press_at(app, button, long);
    assert_eq!(app.get_tip_text(), "", "a press takes it down");
    mock_elapsed_time(Duration::from_secs(2));
    slint::platform::update_timers_and_animations();
    assert_eq!(
        app.get_tip_text(),
        "",
        "and it stays down while the pointer does"
    );
    away(app);
    if let Some(short) = short {
        rest(app, short);
        assert_eq!(app.get_tip_text(), "", "a name that fits says nothing");
        away(app);
    }
}

/// A window over frames named `names`, wired as the editor's is, so
/// the strip and the grid lay their cells out.
fn frames(names: &[&str]) -> (App, Rc<Shared>) {
    let app = window(0);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let files = names
        .iter()
        .map(|n| PathBuf::from("/nowhere").join(n))
        .collect();
    let (state, _worker) = state_for(&app, files);
    crate::cells::set_rows(&app, rows(names));
    settle(&app);
    (app, state)
}

fn rows(names: &[&str]) -> Vec<Thumb> {
    names
        .iter()
        .map(|n| Thumb {
            name: (*n).into(),
            ..Default::default()
        })
        .collect()
}

#[test]
fn the_strips_names_cut_short_come_up_whole() {
    let (app, _state) = frames(&[LONG, SHORT, "c.CR3"]);
    let long = *texts(&app, LONG)
        .last()
        .expect("the long name in the strip");
    let short = *texts(&app, SHORT)
        .last()
        .expect("the short name in the strip");
    says_the_whole(&app, long, Some(short), LONG, PointerEventButton::Left);
    // The picture above the name says nothing.
    let picture = crate::testing::strip_cell(&app, 0);
    rest(&app, picture);
    assert_eq!(app.get_tip_text(), "", "nothing over the picture");
    away(&app);
    // A press held on the picture and slid onto the name is no rest
    // on the name.
    assert_eq!(held_onto(&app, picture, long), "", "nothing mid-press");
    away(&app);
    // A re-flow (a thumbnail's shape come in, which the strip shares
    // with the grid) leaves another control's word alone.
    app.set_can_undo(true);
    crate::testing::rest_on_id(&app, "PanelHead::undo-button");
    assert_eq!(app.get_tip_text(), crate::tr!("Undo"));
    assert!(crate::cells::note_aspect(&app, 2, 0.5));
    assert!(crate::cells::take_aspects(&app));
    slint::platform::update_timers_and_animations();
    assert_eq!(
        app.get_tip_text(),
        crate::tr!("Undo"),
        "a re-flow left the word up"
    );
    away(&app);
    crate::testing::rest_on_id(&app, "PanelHead::undo-button");
    assert_eq!(app.get_tip_text(), crate::tr!("Undo"), "and it comes again");
    away(&app);
    let long = *texts(&app, LONG)
        .last()
        .expect("the long name in the strip");
    // A new list makes every cell again, and the one under the pointer
    // never hears it leave: the name goes with the list.
    rest(&app, long);
    assert_eq!(app.get_tip_text(), LONG);
    crate::cells::set_rows(&app, rows(&[LONG, SHORT, "c.CR3", "d.CR3"]));
    for _ in 0..5 {
        mock_elapsed_time(Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
    }
    move_to(&app, (long.0 + 3.0, long.1));
    assert_eq!(app.get_tip_text(), "", "a new list took the name down");
    away(&app);
    // A right press asks for the menu: the name goes, and stays gone.
    rest(&app, long);
    assert_eq!(app.get_tip_text(), LONG);
    press_at(&app, PointerEventButton::Right, long);
    assert_eq!(app.get_tip_text(), "", "the menu's press takes it down");
}

#[test]
fn the_grids_names_and_folders_cut_short_come_up_whole() {
    let mut names = vec![LONG, SHORT];
    let filler: Vec<String> = (0..200).map(|i| format!("f{i:03}.CR3")).collect();
    names.extend(filler.iter().map(String::as_str));
    let (app, _state) = frames(&names);
    let tile = "a folder named at length for the shoot it holds, day one";
    app.set_grid_tiles(ModelRc::new(VecModel::from(vec![
        FolderTile {
            name: tile.into(),
            count: 4,
            offline: false,
        },
        FolderTile {
            name: "day".into(),
            count: 2,
            offline: false,
        },
    ])));
    app.set_grid_open(true);
    settle(&app);
    let long = *texts(&app, LONG)
        .first()
        .expect("the long name in the grid");
    let short = *texts(&app, SHORT)
        .first()
        .expect("the short name in the grid");
    says_the_whole(&app, long, Some(short), LONG, PointerEventButton::Left);
    // The press chose the frame, whose name the grid's header now
    // shows too: the cell's is the one below it, over the strip's.
    let long = *texts(&app, LONG)
        .iter()
        .find(|p| p.1 > 100.0)
        .expect("the long name in the grid");
    // The picture above the name says nothing.
    rest(&app, (long.0, long.1 - 40.0));
    assert_eq!(app.get_tip_text(), "", "nothing over the picture");
    // A re-flow, a picture's shape come in, moves the cells under a
    // still pointer: the name goes with it.
    rest(&app, long);
    assert_eq!(app.get_tip_text(), LONG);
    assert!(crate::cells::note_aspect(&app, 1, 0.5));
    assert!(crate::cells::take_aspects(&app));
    slint::platform::update_timers_and_animations();
    assert_eq!(app.get_tip_text(), "", "a re-flow took the name down");
    mock_elapsed_time(Duration::from_secs(2));
    slint::platform::update_timers_and_animations();
    assert_eq!(app.get_tip_text(), "", "and put up none while still");
    let long = *texts(&app, LONG)
        .iter()
        .find(|p| p.1 > 100.0)
        .expect("the long name in the grid");
    rest(&app, (long.0 + 3.0, long.1));
    assert_eq!(app.get_tip_text(), LONG, "back after a move and a rest");
    away(&app);
    // A press held on the picture, as a reorder drag starts, and slid
    // onto the name is no rest on the name.
    assert_eq!(
        held_onto(&app, (long.0, long.1 - 40.0), long),
        "",
        "nothing mid-drag"
    );
    away(&app);
    // The sheet scrolled under a still pointer takes the name down
    // and puts up none for whatever comes under it.
    let long = *texts(&app, LONG)
        .iter()
        .find(|p| p.1 > 100.0)
        .expect("the long name in the grid");
    rest(&app, long);
    assert_eq!(app.get_tip_text(), LONG);
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: slint::LogicalPosition::new(long.0, long.1),
        delta_x: 0.0,
        delta_y: -120.0,
    });
    slint::platform::update_timers_and_animations();
    assert!(app.get_grid_scroll() > 0.0, "the sheet scrolled");
    assert_eq!(app.get_tip_text(), "", "a scroll took the name down");
    mock_elapsed_time(Duration::from_secs(2));
    slint::platform::update_timers_and_animations();
    assert_eq!(app.get_tip_text(), "", "and put up none");
    // A folder tile's name, the sheet scrolled back first.
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: slint::LogicalPosition::new(long.0, long.1),
        delta_x: 0.0,
        delta_y: 1200.0,
    });
    slint::platform::update_timers_and_animations();
    assert_eq!(app.get_grid_scroll(), 0.0);
    let tile_at = *texts(&app, tile).first().expect("the tile's name");
    let day = *texts(&app, "day").first().expect("the short tile's name");
    says_the_whole(&app, tile_at, Some(day), tile, PointerEventButton::Left);
}

/// The grid scrolled, and holding its rows in place as pictures bring
/// their shapes in, moves no other control: the word over a header
/// button stays up.
#[test]
fn the_grid_holding_its_place_leaves_a_buttons_word_up() {
    let names: Vec<String> = (0..300).map(|i| format!("f{i:03}.CR3")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let (app, _state) = frames(&names);
    app.set_sort_menu(crate::SortMenu {
        tip: "Sort: Capture time".into(),
        ..Default::default()
    });
    app.set_grid_open(true);
    settle(&app);
    let cell = *texts(&app, "f005.CR3").first().expect("a cell's name");
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: slint::LogicalPosition::new(cell.0, cell.1),
        delta_x: 0.0,
        delta_y: -600.0,
    });
    slint::platform::update_timers_and_animations();
    let scroll = app.get_grid_scroll();
    assert!(scroll > 0.0, "the sheet scrolled");
    crate::testing::rest_on_id(&app, "SortButton::root");
    assert_eq!(app.get_tip_text(), "Sort: Capture time");
    // Every picture above the view comes in wide: the rows above get
    // shorter, and the sheet's scroll moves to hold what is on screen.
    for row in 0..60 {
        crate::cells::note_aspect(&app, row, 3.0);
    }
    crate::panel::browser::reshape(&app);
    slint::platform::update_timers_and_animations();
    assert_ne!(app.get_grid_scroll(), scroll, "the sheet held its place");
    assert_eq!(
        app.get_tip_text(),
        "Sort: Capture time",
        "the word stayed up"
    );
}

#[test]
fn the_grid_headers_chosen_name_cut_short_comes_up_whole() {
    let long = "a frame named at such length that the grid's header line cannot hold it, \
                not even across the wide sheet of a large window, nor at half the size \
                of the type, which is to say a name a card reader would never make, 0001.CR3";
    let (app, _state) = frames(&[long, SHORT]);
    app.set_grid_open(true);
    settle(&app);
    // The header's line, above the sheet's cells.
    let header = |name: &str| {
        *texts(&app, name)
            .iter()
            .find(|p| p.1 < 100.0)
            .expect("the header's line")
    };
    app.invoke_select(1);
    settle(&app);
    let short = header(SHORT);
    app.invoke_select(0);
    settle(&app);
    let at = header(long);
    says_the_whole(&app, at, None, long, PointerEventButton::Left);
    // A name that fits, in the same place, says nothing.
    app.invoke_select(1);
    settle(&app);
    rest(&app, short);
    assert_eq!(app.get_tip_text(), "", "a name that fits says nothing");
}

#[test]
fn the_open_frames_name_cut_short_comes_up_whole() {
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let long = "a frame named at such length that the panel's head cuts it short, 0001.CR3";
    app.set_file_name(long.into());
    let at = *texts(&app, long).first().expect("the head's name");
    // A name that fits, in the same place.
    app.set_file_name(SHORT.into());
    let short = *texts(&app, SHORT).first().expect("the short name");
    app.set_file_name(long.into());
    away(&app);
    rest(&app, short);
    assert_eq!(app.get_tip_text(), long);
    away(&app);
    app.set_file_name(SHORT.into());
    rest(&app, short);
    assert_eq!(app.get_tip_text(), "", "a name that fits says nothing");
    app.set_file_name(long.into());
    says_the_whole(&app, at, None, long, PointerEventButton::Left);
}

#[test]
fn the_folder_trees_names_cut_short_come_up_whole() {
    let app = window(0);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let long = "a folder named at length for the shoot it holds, day two";
    let row = |name: &str, depth: i32| FolderRow {
        name: name.into(),
        path: format!("/nowhere/{name}").into(),
        depth,
        count: 3,
        children: false,
        expanded: false,
        open: false,
    };
    app.set_folder_tree(ModelRc::new(VecModel::from(vec![
        row("shoots", 0),
        row(long, 1),
    ])));
    app.set_folder_tree_shown(true);
    settle(&app);
    let at = *texts(&app, long).first().expect("the long folder's row");
    let short = *texts(&app, "shoots")
        .first()
        .expect("the short folder's row");
    says_the_whole(&app, at, Some(short), long, PointerEventButton::Left);
}
