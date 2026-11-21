//! The lightbox's bar and the tile size slider, on the testing backend:
//! the bar's coming, resting, and holding; its close; the slider and
//! the keys agreeing, in the grid and in the lightbox; the size a
//! narrow sheet holds; the pictures asked for as the size moves; the
//! sizes kept across a restart; the surround following MONITOR's
//! canvas; and the grid's header with its slider at the narrowest
//! window, in English and in French.

use std::time::Duration;

use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};

use super::{ACROSS, FEWEST, MOST};
use crate::testing::{folder, state_for, window};
use crate::{App, grid};

/// Timers and animations `ms` on, the pointer left where it is.
fn wait(ms: u64) {
    for _ in 0..ms / 50 {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
        slint::platform::update_timers_and_animations();
    }
}

fn move_to(app: &App, x: f32, y: f32) {
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(x, y),
    });
    slint::platform::update_timers_and_animations();
}

fn down(app: &App, x: f32, y: f32) {
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: LogicalPosition::new(x, y),
        button: PointerEventButton::Left,
    });
    slint::platform::update_timers_and_animations();
}

fn up(app: &App, x: f32, y: f32) {
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: LogicalPosition::new(x, y),
        button: PointerEventButton::Left,
    });
    slint::platform::update_timers_and_animations();
}

/// A key, its handlers run, and the pointer left where it is.
fn key(app: &App, key: impl Into<slint::SharedString>) {
    crate::testing::press(app, key);
    slint::platform::update_timers_and_animations();
}

/// A window of `count` frames at 1500 by 950, wired as a session is,
/// the first frame chosen.
fn session(count: usize) -> (App, std::rc::Rc<crate::Shared>) {
    let app = window(count);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, folder(count));
    app.set_left_hidden(false);
    app.set_right_hidden(false);
    app.set_strip_hidden(false);
    app.invoke_select(0);
    crate::testing::settle(&app);
    (app, state)
}

/// Into the lightbox by L, the pointer then over the tiles.
fn into_lightbox(app: &App) {
    key(app, "l");
    crate::testing::settle(app);
    move_to(app, 700.0, 500.0);
    assert!(app.get_lightbox());
}

/// The lightbox bar's close button, by its id (its words are the
/// catalog's), found without moving the pointer.
fn close(app: &App) -> Vec<ElementHandle> {
    ElementHandle::find_by_element_id(app, "GridSheet::bar-close")
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .collect()
}

/// The one visible tile size slider, the header's or the bar's, by its
/// type: its label is the catalog's.
fn slider(app: &App) -> ElementHandle {
    let mut all: Vec<_> = ElementHandle::find_by_element_type_name(app, "SizeSlider")
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Slider))
        .collect();
    assert_eq!(all.len(), 1, "one tile size slider on screen");
    all.remove(0)
}

fn center(e: &ElementHandle) -> (f32, f32) {
    let (p, s) = (e.absolute_position(), e.size());
    (p.x + s.width / 2.0, p.y + s.height / 2.0)
}

/// Points on the slider: its very ends, inside it, where the knob goes
/// to 0 and to 1, and its height's middle.
fn ends(e: &ElementHandle) -> (f32, f32, f32) {
    let (p, s) = (e.absolute_position(), e.size());
    (p.x + 1.0, p.x + s.width - 1.0, p.y + s.height / 2.0)
}

/// The point `f` of the way along the slider's track (its knob's
/// half width in from each end).
fn along(e: &ElementHandle, f: f32) -> f32 {
    let (p, s) = (e.absolute_position(), e.size());
    p.x + 6.0 + f * (s.width - 12.0)
}

/// The bar comes with the pointer and goes once it rests; keys alone
/// bring nothing up; the pointer over the bar keeps it.
#[test]
fn the_bar_comes_with_the_pointer_and_goes_at_rest() {
    let (app, _state) = session(24);
    assert!(!app.get_lightbox_bar_up(), "not over the grid");
    into_lightbox(&app);
    assert!(app.get_lightbox_bar_up(), "up as the lightbox opens");
    wait(1500);
    assert!(app.get_lightbox_bar_up(), "a moment yet");
    wait(800);
    assert!(!app.get_lightbox_bar_up(), "down at rest");
    wait(500);
    assert!(close(&app).is_empty(), "faded out, gone");
    // Keys move nothing on screen: the bar stays down.
    key(&app, Key::RightArrow);
    key(&app, "-");
    wait(200);
    assert!(!app.get_lightbox_bar_up());
    // The pointer moves: up, and the rest starts again.
    move_to(&app, 720.0, 520.0);
    assert!(app.get_lightbox_bar_up());
    assert_eq!(close(&app).len(), 1);
    wait(1500);
    move_to(&app, 730.0, 520.0);
    wait(1500);
    assert!(
        app.get_lightbox_bar_up(),
        "the rest counts from the last move"
    );
    // Over the bar it stays, however long.
    move_to(&app, 700.0, 22.0);
    wait(6000);
    assert!(app.get_lightbox_bar_up(), "kept under the pointer");
    // Off it, down after the rest.
    move_to(&app, 700.0, 500.0);
    wait(2500);
    assert!(!app.get_lightbox_bar_up());
    // The grid has no bar.
    key(&app, "l");
    move_to(&app, 700.0, 520.0);
    assert!(!app.get_lightbox() && !app.get_lightbox_bar_up());
}

/// A press with no move before it, as a finger or a pen gives one with
/// no hover, brings the bar up, and the tile under it is chosen as ever.
#[test]
fn a_touch_brings_the_bar_up() {
    let (app, _state) = session(24);
    into_lightbox(&app);
    wait(2500);
    assert!(!app.get_lightbox_bar_up());
    // The pointer is at (700, 500) already: the press is the event.
    down(&app, 700.0, 500.0);
    up(&app, 700.0, 500.0);
    assert!(app.get_lightbox_bar_up(), "a tap wakes it");
    wait(2500);
    assert!(!app.get_lightbox_bar_up(), "and it rests as after a move");
}

/// The bar stays while its slider is dragged, however long the hand
/// rests on it; it goes after the rest once let go.
#[test]
fn the_bar_stays_while_its_slider_is_held() {
    let (app, _state) = session(24);
    into_lightbox(&app);
    let s = slider(&app);
    let (x, y) = center(&s);
    move_to(&app, x, y);
    down(&app, x, y);
    // Off the bar, still held: a drag goes where the hand goes.
    move_to(&app, x, 400.0);
    wait(5000);
    assert!(app.get_lightbox_bar_up(), "held by the drag");
    up(&app, x, 400.0);
    wait(2500);
    assert!(!app.get_lightbox_bar_up(), "let go, it rests");
}

/// A press on the slider that never comes up (the pointer leaves the
/// window, another takes the grab) hands the keys back to the window
/// all the same: an arrow then steps the frame, not the tiles, and the
/// bar rests.
#[test]
fn a_canceled_press_on_the_slider_hands_the_keys_back() {
    let (app, _state) = session(24);
    into_lightbox(&app);
    let (x, y) = center(&slider(&app));
    move_to(&app, x, y);
    down(&app, x, y);
    app.window().dispatch_event(WindowEvent::PointerExited);
    slint::platform::update_timers_and_animations();
    let (across, selected) = (app.get_lightbox_across(), app.get_selected());
    key(&app, Key::RightArrow);
    assert_eq!(app.get_lightbox_across(), across, "not the slider's");
    assert_eq!(app.get_selected(), selected + 1, "the window's arrow");
    wait(2500);
    assert!(!app.get_lightbox_bar_up(), "nothing holds the bar");
    // Escape is the window's too.
    key(&app, Key::Escape);
    assert!(!app.get_lightbox() && app.get_grid_open());
}

/// The move that wakes the bar, landing in the strip it then covers,
/// holds it: the bar does not fade from under a still pointer.
#[test]
fn a_pointer_resting_in_the_bar_s_strip_holds_it() {
    let (app, _state) = session(24);
    into_lightbox(&app);
    wait(2500);
    assert!(!app.get_lightbox_bar_up());
    move_to(&app, 550.0, 20.0);
    assert!(app.get_lightbox_bar_up());
    wait(6000);
    assert!(app.get_lightbox_bar_up(), "held under the still pointer");
    move_to(&app, 550.0, 400.0);
    wait(2500);
    assert!(!app.get_lightbox_bar_up(), "off it, it rests");
}

/// A press on the bar's scrim is the bar's: the tile under it is not
/// chosen.
#[test]
fn a_press_on_the_bar_does_not_choose_the_tile_beneath() {
    let (app, _state) = session(24);
    into_lightbox(&app);
    assert_eq!(app.get_selected(), 0);
    // The second tile's top, under the bar's middle words.
    move_to(&app, 550.0, 30.0);
    down(&app, 550.0, 30.0);
    up(&app, 550.0, 30.0);
    assert!(app.get_lightbox_bar_up());
    assert_eq!(app.get_selected(), 0, "still the first");
    // Below the bar, the same tile is chosen.
    move_to(&app, 550.0, 200.0);
    down(&app, 550.0, 200.0);
    up(&app, 550.0, 200.0);
    assert_eq!(app.get_selected(), 1);
}

/// The bar ends short of the scroll bar: the thumb at the sheet's top,
/// under the bar's strip, is still grabbed and dragged.
#[test]
fn the_scroll_bar_s_thumb_is_dragged_beside_the_bar() {
    let (app, _state) = session(600);
    into_lightbox(&app);
    assert!(app.get_lightbox_bar_up());
    assert_eq!(app.get_grid_scroll(), 0.0);
    let band = app.global::<crate::Theme>().get_scrollbar_band();
    let x = 1500.0 - band / 2.0;
    move_to(&app, x, 16.0);
    assert!(app.get_lightbox_bar_up());
    down(&app, x, 16.0);
    move_to(&app, x, 200.0);
    up(&app, x, 200.0);
    assert!(app.get_grid_scroll() > 0.0, "the thumb was the band's");
}

/// Back from the lightbox's loupe the bar comes up, as on the way in,
/// and rests.
#[test]
fn the_bar_comes_up_back_from_the_loupe_and_rests() {
    let (app, _state) = session(24);
    into_lightbox(&app);
    wait(2500);
    key(&app, Key::Return);
    assert!(app.get_lightbox_loupe() && !app.get_grid_open());
    key(&app, Key::Return);
    assert!(app.get_grid_open() && app.get_lightbox());
    assert!(app.get_lightbox_bar_up(), "up on the way back");
    wait(2500);
    assert!(!app.get_lightbox_bar_up(), "and down at rest");
}

/// The close is a button to a screen reader, named; its click and its
/// default action leave for the grid as L does; its words at the
/// pointer say the key.
#[test]
fn the_close_leaves_for_the_grid() {
    let (app, _state) = session(24);
    into_lightbox(&app);
    let button = close(&app);
    assert_eq!(button.len(), 1);
    let (x, y) = center(&button[0]);
    move_to(&app, x, y);
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(800));
    slint::platform::update_timers_and_animations();
    assert_eq!(app.get_tip_text(), "Close lightbox (L)");
    down(&app, x, y);
    up(&app, x, y);
    assert!(!app.get_lightbox() && app.get_grid_open(), "the click");
    assert!(!app.get_left_hidden(), "the panes back as L puts them");
    into_lightbox(&app);
    close(&app)[0].invoke_accessible_default_action();
    slint::platform::update_timers_and_animations();
    assert!(!app.get_lightbox() && app.get_grid_open(), "the action");
}

/// Escape lets go of a set first, then leaves, with the bar up or not.
#[test]
fn escape_lets_go_of_the_set_then_leaves() {
    let (app, _state) = session(12);
    into_lightbox(&app);
    app.invoke_frame_clicked(2, true, false);
    app.invoke_frame_clicked(3, true, false);
    assert_eq!(app.get_set_count(), 3);
    assert!(app.get_lightbox_bar_up());
    key(&app, Key::Escape);
    assert_eq!(app.get_set_count(), 1);
    assert!(app.get_lightbox());
    key(&app, Key::Escape);
    assert!(!app.get_lightbox() && app.get_grid_open());
}

/// The bar's middle says the grid header's words: the frame on screen,
/// it and the rest of the set, and the count.
#[test]
fn the_bar_says_the_selection_and_the_count() {
    let (app, _state) = session(12);
    into_lightbox(&app);
    let words = |app: &App| {
        ElementHandle::find_by_element_type_name(app, "Text")
            .filter_map(|e| e.accessible_label())
            .find(|l| l.ends_with("12 frames"))
            .map(|l| l.to_string())
    };
    assert_eq!(words(&app).as_deref(), Some("f00.CR3 · 12 frames"));
    app.invoke_frame_clicked(2, true, false);
    app.invoke_frame_clicked(3, true, false);
    move_to(&app, 710.0, 500.0);
    assert_eq!(
        words(&app).as_deref(),
        Some("f00.CR3 and 2 others · 12 frames")
    );
}

/// In the lightbox the slider is the tiles across: plus, minus, and
/// Ctrl with the wheel move its knob, and a drag to either end or
/// between sets the count, snapped to whole counts; the grid's cell is
/// left alone.
#[test]
fn the_lightbox_slider_and_the_keys_agree() {
    let (app, _state) = session(60);
    let grid_cell = app.get_grid_cell();
    into_lightbox(&app);
    let at = |app: &App| slider(app).accessible_value().unwrap().to_string();
    let cell = |app: &App| format!("{}", app.get_tile_cell().round());
    assert_eq!(app.get_lightbox_across(), ACROSS);
    assert_eq!(at(&app), cell(&app), "the size said is the tiles'");
    key(&app, "+");
    assert_eq!(app.get_lightbox_across(), ACROSS - 1);
    assert_eq!(at(&app), cell(&app));
    let s = slider(&app);
    let (x0, x1, y) = ends(&s);
    // To the right end: the fewest across, the largest.
    move_to(&app, x0 + 20.0, y);
    down(&app, x0 + 20.0, y);
    move_to(&app, x1 + 40.0, y);
    up(&app, x1 + 40.0, y);
    assert_eq!(app.get_lightbox_across(), FEWEST);
    assert_eq!(super::slider_at(FEWEST), 1.0);
    // To the left end: the most.
    down(&app, x0, y);
    up(&app, x0, y);
    assert_eq!(app.get_lightbox_across(), MOST);
    // Three tenths along: snapped to a quarter, one from the most.
    let q = along(&s, 0.3);
    down(&app, q, y);
    up(&app, q, y);
    assert_eq!(app.get_lightbox_across(), MOST - 1);
    // The keys from there, and the knob follows.
    key(&app, "-");
    assert_eq!(app.get_lightbox_across(), MOST);
    assert_eq!(at(&app), cell(&app));
    // A screen reader's step is plus's.
    slider(&app).invoke_accessible_increment_action();
    slint::platform::update_timers_and_animations();
    assert_eq!(app.get_lightbox_across(), MOST - 1);
    // Ctrl and the wheel over the sheet.
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: LogicalPosition::new(700.0, 500.0),
        delta_x: 0.0,
        delta_y: 60.0,
    });
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    slint::platform::update_timers_and_animations();
    assert_eq!(app.get_lightbox_across(), MOST - 2);
    assert_eq!(at(&app), cell(&app));
    assert_eq!(app.get_grid_cell(), grid_cell, "the grid's own size kept");
}

/// In the grid the header's slider is the cell: a drag sets it in whole
/// pixels between the first step and the most, and plus, minus, and
/// Ctrl with the wheel move the knob with the size.
#[test]
fn the_grid_slider_and_the_keys_agree() {
    let (app, _state) = session(60);
    key(&app, "g");
    crate::testing::settle(&app);
    assert!(app.get_grid_open() && !app.get_lightbox());
    let at = |app: &App| slider(app).accessible_value().unwrap().to_string();
    let cell = |app: &App| format!("{}", app.get_tile_cell().round());
    assert_eq!(at(&app), cell(&app));
    let most = grid::most(app.get_grid_w());
    assert_eq!(
        slider(&app).accessible_value_maximum(),
        Some(most),
        "the range said"
    );
    assert_eq!(
        slider(&app).accessible_value_minimum(),
        Some(grid::STEPS[0])
    );
    key(&app, "+");
    assert_eq!(app.get_grid_cell(), 256.0);
    assert_eq!(at(&app), "256");
    let (x0, x1, y) = ends(&slider(&app));
    move_to(&app, x0, y);
    down(&app, x0, y);
    move_to(&app, x1 + 30.0, y);
    assert_eq!(app.get_tile_cell(), most, "the right end is the most");
    move_to(&app, x0 - 30.0, y);
    assert_eq!(
        app.get_tile_cell(),
        grid::STEPS[0],
        "the left end the least"
    );
    let half = along(&slider(&app), 0.5);
    move_to(&app, half, y);
    up(&app, half, y);
    let mid = app.get_grid_cell();
    assert!(mid > 176.0 && mid < 256.0, "{mid}: halfway, in proportion");
    assert_eq!(mid, mid.round(), "whole pixels");
    assert_eq!(at(&app), cell(&app));
    // The Settings sheet's row follows, as for the keys.
    assert_eq!(app.get_grid_cell_typed(), format!("{mid}"));
    // The keys step on from the size the slider left.
    key(&app, "-");
    assert_eq!(app.get_grid_cell(), 176.0);
    assert_eq!(at(&app), "176");
    slider(&app).invoke_accessible_increment_action();
    slint::platform::update_timers_and_animations();
    assert_eq!(app.get_grid_cell(), 256.0);
    // Back in the grid the keys hand back to the window: an arrow steps
    // the frame, not the size.
    key(&app, Key::RightArrow);
    assert_eq!(app.get_selected(), 1);
    assert_eq!(app.get_grid_cell(), 256.0);
}

/// The sizes the slider sets are the ones the settings keep: a new
/// session opens the grid and the lightbox at them, the knobs where
/// they were left.
#[test]
fn the_slider_s_sizes_are_kept_across_a_restart() {
    let (app, _state) = session(24);
    key(&app, "g");
    crate::testing::settle(&app);
    let (_, _, y) = ends(&slider(&app));
    let x = along(&slider(&app), 0.8);
    down(&app, x, y);
    up(&app, x, y);
    let cell = app.get_grid_cell();
    assert!(cell > 256.0);
    into_lightbox(&app);
    let (x0, _, y) = ends(&slider(&app));
    down(&app, x0, y);
    up(&app, x0, y);
    assert_eq!(app.get_lightbox_across(), MOST);
    let (grid_at, lightbox_at) = (
        grid::slider_at(cell, grid::most(app.get_grid_w())),
        super::slider_at(MOST),
    );
    key(&app, Key::Escape);
    let kept = crate::panel::startup::remember(&app);
    let dir = crate::testing::scratch_dir("lightbox-bar-kept");
    let path = dir.join("settings.json");
    kept.save_to(&path);
    let back = crate::settings::Settings::load_from(&path);
    crate::testing::remove_dir_retry(&dir);
    assert_eq!((back.grid_cell, back.lightbox_across), (cell, MOST));
    // The next session, set up as the start sets it up.
    let (app, _state) = session(24);
    app.set_grid_cell(grid::clamp_cell(back.grid_cell));
    app.set_lightbox_across(super::held(back.lightbox_across));
    key(&app, "g");
    crate::testing::settle(&app);
    assert_eq!(app.get_grid_cell(), cell);
    assert_eq!(
        slider(&app).accessible_value().unwrap(),
        format!("{}", cell.round())
    );
    assert_eq!(
        grid::slider_at(app.get_tile_cell(), grid::most(app.get_grid_w())),
        grid_at
    );
    into_lightbox(&app);
    assert_eq!(app.get_lightbox_across(), MOST);
    assert_eq!(super::slider_at(app.get_lightbox_across()), lightbox_at);
}

/// The lightbox's surround is MONITOR's canvas shade, the default the
/// near black it always was; the grid keeps its sheet.
#[test]
fn the_surround_is_the_canvas_shade() {
    let (app, _state) = session(6);
    assert_eq!(app.get_canvas_choice(), 0, "Black by default");
    let colors: Vec<slint::Color> = {
        use slint::Model;
        app.get_canvas_colors().iter().collect()
    };
    assert_eq!(
        colors[0],
        slint::Color::from_rgb_u8(0x0f, 0x0f, 0x0f),
        "the default renders as before"
    );
    let crate::settings::Settings { canvas_color, .. } = crate::settings::Settings::default();
    assert_eq!(crate::render::canvas_choice(&canvas_color), 0);
    // The sheet draws `grid-ground` and makes no choice of its own: the
    // grid's grey whatever the shade.
    key(&app, "g");
    crate::testing::settle(&app);
    let sheet = app.global::<crate::Theme>().get_sheet();
    app.set_canvas_choice(4);
    slint::platform::update_timers_and_animations();
    assert_eq!(app.get_grid_ground(), sheet, "the grid keeps its sheet");
    app.set_canvas_choice(0);
    into_lightbox(&app);
    for (i, c) in colors.iter().enumerate() {
        app.set_canvas_choice(i as i32);
        slint::platform::update_timers_and_animations();
        assert_eq!(app.get_lightbox_surround(), *c, "shade {i}");
        assert_eq!(app.get_grid_ground(), *c, "the sheet's ground, shade {i}");
    }
}

/// The grid's header keeps every word it had beside its new slider at
/// the narrowest window the editor is made for, with the left pane at
/// each width it allows, in English, Spanish, and French: each label is not
/// cut, as wide as the same words on a wide window, and inside the
/// header. The selection's name and the status line are the ones meant
/// to give.
#[test]
fn the_header_keeps_its_words_beside_the_slider_at_1366() {
    for lang in ["", "es", "fr"] {
        let (app, _state) = session(24);
        slint::select_bundled_translation(lang).expect("bundled");
        key(&app, "g");
        // Everything the header row can carry.
        app.invoke_frame_clicked(2, true, false);
        // The size in the count's line, which the cap changes: its
        // number as N, to know the line whatever it says.
        let sized = |l: &str| match l.rfind("\u{b7} ") {
            Some(p) if l.ends_with(" px") => format!("{}\u{b7} N px", &l[..p]),
            _ => l.to_string(),
        };
        // The header's top row's words, right of the left pane: each
        // as `sized` names it, its words, its left, and its width.
        let header_texts = |app: &App| -> Vec<(String, String, f32, f32)> {
            crate::testing::settle(app);
            let sheet = app.window().size().to_logical(1.0).width - app.get_grid_w();
            ElementHandle::find_by_element_type_name(app, "Text")
                .filter(|e| {
                    let p = e.absolute_position();
                    p.y < 40.0 && p.x >= sheet - 0.5
                })
                .filter_map(|e| {
                    e.accessible_label().map(|l| {
                        (
                            sized(l.as_str()),
                            l.to_string(),
                            e.absolute_position().x,
                            e.size().width,
                        )
                    })
                })
                // The selection's name and the status, which take the
                // row's slack and are cut short by design.
                .filter(|(l, ..)| {
                    !l.is_empty() && !l.contains(".CR3") && *l != app.get_status_shown().as_str()
                })
                .collect()
        };
        // The width the same words take on a wide window, where nothing
        // is short of room: the size, which the narrow sheet caps, set
        // to the one they say. The count's line is as wide as its
        // number, and digits are not all one width.
        let natural = |app: &App, words: &str| -> f32 {
            let cell = words
                .strip_suffix(" px")
                .and_then(|l| l.rsplit("\u{b7} ").next())
                .and_then(|n| n.parse::<f32>().ok())
                .unwrap_or(512.0);
            app.set_grid_cell(cell);
            app.window()
                .set_size(slint::LogicalSize::new(2560.0, 1000.0));
            app.set_left_size(200.0);
            let wide = header_texts(app);
            let found = wide.iter().find(|(_, w, ..)| w == words).map(|t| t.3);
            found.unwrap_or_else(|| panic!("{lang}: {words:?} not on the wide window: {wide:?}"))
        };
        // The largest size, which the narrow sheets hold.
        app.set_grid_cell(512.0);
        app.window()
            .set_size(slint::LogicalSize::new(2560.0, 1000.0));
        app.set_left_size(200.0);
        let wide = header_texts(&app);
        assert_eq!(app.get_tile_cell(), 512.0);
        // The words checked: the count and the size, and the buttons.
        let (count, rejects, lightbox) = match lang {
            "es" => ("24 fotos", "Rechazadas", "Mesa de luz"),
            "fr" => ("24 photos", "Photos rejetées", "Table lumineuse"),
            _ => ("24 frames", "Rejects", "Lightbox"),
        };
        for word in [count, rejects, lightbox] {
            assert!(
                wide.iter().any(|(l, ..)| l.starts_with(word)),
                "{lang}: {word:?} in {wide:?}"
            );
        }
        for left in [200.0, 240.0, 455.0] {
            let at_1366 = |app: &App| {
                app.set_grid_cell(512.0);
                app.window()
                    .set_size(slint::LogicalSize::new(1366.0, 768.0));
                app.set_left_size(left);
            };
            at_1366(&app);
            let narrow = header_texts(&app);
            let sheet = 1366.0 - app.get_grid_w();
            for (label, ..) in &wide {
                let Some((_, words, x, w)) = narrow.iter().find(|(l, ..)| l == label).cloned()
                else {
                    panic!("{lang} at {left}: {label:?} gone from {narrow:?}");
                };
                // Inside the header, right of the left pane.
                assert!(
                    x >= sheet - 0.5 && x + w <= 1366.0 + 0.5,
                    "{lang} at {left}: {words:?} at {x}+{w}, the header {sheet}..1366"
                );
                // Not cut: as wide as the same words with room to spare.
                let whole = natural(&app, &words);
                at_1366(&app);
                assert!(
                    (w - whole).abs() < 0.5,
                    "{lang} at {left}: {words:?} cut to {w} from {whole}"
                );
            }
            header_texts(&app);
            let s = slider(&app);
            assert!(s.size().width >= 96.0, "{lang} at {left}: the slider whole");
            // Two across at the largest size, as laid out, the pane at
            // its widest included, where the cell is held under 512.
            let (w, cell) = (app.get_grid_w(), app.get_tile_cell());
            assert_eq!(cell, grid::most(w).min(512.0), "{lang} at {left}");
            assert!(grid::columns(w, cell) >= 2, "{lang} at {left}: {w} {cell}");
            if left == 455.0 {
                assert!(cell < 512.0, "{lang}: held at the widest pane");
            }
        }
        slint::select_bundled_translation("").expect("English is the source");
    }
}

/// A grid at 1366 with the left pane at its widest, its sheet too
/// narrow for two of the largest cells.
fn capped_grid() -> (App, std::rc::Rc<crate::Shared>) {
    let (app, state) = session(60);
    app.window()
        .set_size(slint::LogicalSize::new(1366.0, 768.0));
    app.set_left_size(455.0);
    key(&app, "g");
    crate::testing::settle(&app);
    assert!(grid::most(app.get_grid_w()) < 512.0, "{}", app.get_grid_w());
    (app, state)
}

/// The size the header says, in its count's line.
fn header_px(app: &App) -> String {
    ElementHandle::find_by_element_type_name(app, "Text")
        .filter_map(|e| e.accessible_label())
        .find(|l| l.ends_with(" px"))
        .map(|l| l.rsplit("\u{b7} ").next().unwrap_or_default().to_string())
        .expect("the count's line")
}

/// The slider's right end on a sheet that holds the size keeps the
/// largest step, as plus does: the header says what is laid out, the
/// Settings row what is set, and a wider window, or the pane put away,
/// gives the largest back.
#[test]
fn the_slider_s_end_keeps_the_largest_size_on_a_narrow_sheet() {
    let (app, _state) = capped_grid();
    let most = grid::most(app.get_grid_w());
    let (_, x1, y) = ends(&slider(&app));
    move_to(&app, x1, y);
    down(&app, x1, y);
    up(&app, x1, y);
    assert_eq!(app.get_grid_cell(), 512.0, "the largest step kept");
    assert_eq!(app.get_tile_cell(), most, "laid out at the most");
    assert_eq!(header_px(&app), format!("{} px", most.round()));
    assert_eq!(app.get_grid_cell_typed(), "512", "the setting");
    // The pane put away: the sheet is wide enough for 512.
    key(&app, Key::Tab);
    crate::testing::settle(&app);
    assert_eq!(app.get_tile_cell(), 512.0);
    key(&app, Key::Tab);
    crate::testing::settle(&app);
    assert_eq!(app.get_tile_cell(), most);
    // A wider window.
    app.window()
        .set_size(slint::LogicalSize::new(2560.0, 1000.0));
    crate::testing::settle(&app);
    assert_eq!(app.get_tile_cell(), 512.0);
}

/// Plus and minus from a cell the sheet holds step from the size laid
/// out: minus is the next step under it, at once, and plus from there
/// is the most again.
#[test]
fn plus_and_minus_step_from_the_held_cell() {
    let (app, _state) = capped_grid();
    let most = grid::most(app.get_grid_w());
    app.set_grid_cell(512.0);
    crate::testing::settle(&app);
    assert_eq!(app.get_tile_cell(), most);
    key(&app, "-");
    assert_eq!(app.get_grid_cell(), 360.0);
    assert_eq!(app.get_tile_cell(), 360.0);
    key(&app, "+");
    assert_eq!(app.get_grid_cell(), 512.0);
    assert_eq!(app.get_tile_cell(), most);
    // The slider's knob at its end either way.
    assert_eq!(
        slider(&app).accessible_value().unwrap(),
        format!("{}", most.round())
    );
}

/// A screen reader is told the slider's step: a whole pixel for the
/// grid's, a count's worth of the range for the lightbox's.
#[test]
fn the_slider_says_its_step() {
    let (app, _state) = session(24);
    key(&app, "g");
    crate::testing::settle(&app);
    assert_eq!(slider(&app).accessible_value_step(), Some(1.0));
    into_lightbox(&app);
    let s = slider(&app);
    let (least, most) = (
        s.accessible_value_minimum().unwrap(),
        s.accessible_value_maximum().unwrap(),
    );
    assert_eq!(
        s.accessible_value_step(),
        Some((most - least) / (MOST - FEWEST) as f32)
    );
}

/// A slider dragged through every pixel asks each frame on screen for
/// a larger picture once each size the pool makes, not once a pixel.
#[test]
fn a_drag_asks_for_larger_pictures_once_a_made_size() {
    let (app, state) = session(60);
    key(&app, "g");
    crate::testing::settle(&app);
    let (first, last) = state.borrow().views.grid_shown.expect("on screen");
    let rows: Vec<usize> = (first as usize..=last as usize)
        .map(|r| state.borrow().views.shown[r])
        .collect();
    for &i in &rows {
        state.borrow_mut().views.thumb_made[i] = 176;
    }
    let row = rows[0];
    let mut asks = Vec::new();
    let most = grid::most(app.get_grid_w());
    for cell in 177..=340 {
        app.invoke_tile_size_moved(grid::slider_at(cell as f32, most));
        slint::platform::update_timers_and_animations();
        let asked = state.borrow().views.thumb_asked[row];
        if asks.last() != Some(&asked) {
            asks.push(asked);
        }
    }
    // The first, then once at 256's size and once at 360's.
    assert!(asks.len() <= 3, "{asks:?}");
    assert!(*asks.last().unwrap() > 256, "{asks:?}");
}

/// Keys that scroll the sheet under a still pointer do not hold the
/// bar: the pointer's place is where it was over the sheet, not where
/// the scroll has carried the sheet under it.
#[test]
fn keys_that_scroll_the_sheet_do_not_hold_the_bar() {
    let (app, _state) = session(60);
    into_lightbox(&app);
    move_to(&app, 700.0, 500.0);
    for _ in 0..4 {
        key(&app, Key::DownArrow);
        wait(300);
    }
    assert!(app.get_grid_scroll() > 500.0, "{}", app.get_grid_scroll());
    wait(2500);
    assert!(!app.get_lightbox_bar_up(), "the bar rests");
}

/// The grid with frames on screen whose pictures were made at 176, the
/// first of them and its picture's frame key.
fn grid_with_pictures(count: usize) -> (App, std::rc::Rc<crate::Shared>, usize) {
    let (app, state) = session(count);
    key(&app, "g");
    crate::testing::settle(&app);
    let (first, last) = state.borrow().views.grid_shown.expect("on screen");
    let cells: Vec<usize> = (first as usize..=last as usize)
        .map(|r| state.borrow().views.shown[r])
        .collect();
    for &i in &cells {
        state.borrow_mut().views.thumb_made[i] = 176;
    }
    (app, state, cells[0])
}

/// The pool's picture of cell `i` landing at `size`.
fn landed(app: &App, state: &std::rc::Rc<crate::Shared>, i: usize, size: u32) {
    let frame = state.borrow().key_of(i).expect("a cell");
    crate::panel::deliver::deliver(
        app,
        crate::worker::Outcome::Thumbnail {
            index: i,
            frame,
            size,
            width: 2,
            height: 2,
            rgb: vec![0; 12],
            cached: false,
            seconds: 0.0,
        },
    );
}

fn asked(state: &std::rc::Rc<crate::Shared>, i: usize) -> u32 {
    state.borrow().views.thumb_asked[i]
}

/// Slid large, the picture asked for; slid small before the pool began
/// it, so it lands small; slid large again: asked for again, not left
/// small for good.
#[test]
fn a_picture_landed_smaller_than_asked_is_asked_for_again() {
    let (app, state, i) = grid_with_pictures(60);
    let most = grid::most(app.get_grid_w());
    let slide = |cell: f32| {
        app.invoke_tile_size_moved(grid::slider_at(cell, most));
        slint::platform::update_timers_and_animations();
    };
    slide(300.0);
    assert!(asked(&state, i) > 256);
    slide(128.0);
    landed(&app, &state, i, 128);
    slide(330.0);
    assert_eq!(
        asked(&state, i),
        state.borrow().views.thumb_want,
        "asked for again at 330"
    );
}

/// The same with the keys alone: up to 360, down to 128 where the
/// picture is made again at 128 (an edit, say), and up again, a step at
/// a time, each step asking for its size.
#[test]
fn the_keys_ask_again_after_a_picture_landed_small() {
    let (app, state, i) = grid_with_pictures(60);
    key(&app, "+");
    key(&app, "+");
    assert_eq!(asked(&state, i), 360);
    for _ in 0..3 {
        key(&app, "-");
    }
    assert_eq!(app.get_grid_cell(), 128.0);
    landed(&app, &state, i, 128);
    let mut steps = Vec::new();
    for _ in 0..3 {
        key(&app, "+");
        steps.push(asked(&state, i));
    }
    assert_eq!(steps, [176, 256, 360]);
}

/// Word that no picture came lowers what was asked to what is in hand:
/// the next look at the larger cell asks again.
#[test]
fn no_picture_lowers_what_was_asked() {
    let (app, state, i) = grid_with_pictures(60);
    key(&app, "+");
    key(&app, "+");
    assert_eq!(asked(&state, i), 360);
    let frame = state.borrow().key_of(i).expect("a cell");
    crate::panel::deliver::deliver(
        &app,
        crate::worker::Outcome::NoThumbnail { index: i, frame },
    );
    assert_eq!(asked(&state, i), 176, "what is in hand");
    key(&app, "-");
    key(&app, "+");
    assert_eq!(asked(&state, i), 360, "asked for again");
}

/// A list changed under the cells carries what each asked for no
/// larger than what it holds: the pool's queue went with the old list.
#[test]
fn a_new_list_carries_no_more_asked_than_in_hand() {
    let app = window(12);
    let (state, worker) = state_for(&app, folder(12));
    {
        let mut st = state.borrow_mut();
        st.views.thumb_made[3] = 128;
        st.views.thumb_asked[3] = 360;
        st.views.thumb_made[4] = 0;
        st.views.thumb_asked[4] = 360;
        st.views.thumb_made[5] = 256;
        st.views.thumb_asked[5] = 256;
    }
    let cells = state.borrow().cells.clone();
    let from: Vec<Option<usize>> = (0..cells.len()).map(Some).collect();
    crate::versions::carry(&mut state.borrow_mut(), &app, &worker, cells, &from);
    let st = state.borrow();
    assert_eq!(st.views.thumb_asked[3], crate::worker::THUMB_WIDTH);
    assert_eq!(st.views.thumb_asked[4], crate::worker::THUMB_WIDTH);
    assert_eq!(st.views.thumb_asked[5], 256);
}

/// A root back online asks again for its frames whose pictures are
/// smaller than the cells want: a miss in the cache at the larger size,
/// while it was away, left the smaller up.
#[test]
fn a_root_back_asks_again_for_small_pictures() {
    let (app, state, i) = grid_with_pictures(60);
    landed(&app, &state, i, 176);
    key(&app, "+");
    let want = state.borrow().views.thumb_want;
    assert!(want > 176);
    // The miss at the larger size, as the cache gave it offline.
    let frame = state.borrow().key_of(i).expect("a cell");
    crate::panel::deliver::deliver(
        &app,
        crate::worker::Outcome::NoThumbnail { index: i, frame },
    );
    assert_eq!(asked(&state, i), 176);
    let root = folder(1)[0].parent().unwrap().to_path_buf();
    let worker = std::rc::Rc::new(crate::worker::Worker::new(|_| {}));
    crate::rows::roots_changed(
        &mut state.borrow_mut(),
        &worker,
        &std::collections::HashSet::from([root]),
        &std::collections::HashSet::new(),
    );
    assert_eq!(asked(&state, i), want, "asked for again at the size wanted");
}
