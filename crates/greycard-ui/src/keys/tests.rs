use super::*;
use crate::testing::{press, window};
use slint::Model;
use slint::platform::WindowEvent;
use std::collections::BTreeSet;

const GRID: View = View {
    loupe: false,
    brush: false,
};
const IN_HAND: View = View {
    loupe: true,
    brush: true,
};

/// Every chord the table holds.
fn table_chords() -> BTreeSet<&'static str> {
    TABLE
        .iter()
        .flat_map(|g| g.lines)
        .flat_map(|b| b.chords.iter().copied())
        .collect()
}

/// Slint source with its comments taken out and each string
/// literal's contents put aside: the code with every literal as
/// `"#n"`, and the literals by `n`. A brace, a `chord == "` or an
/// `e.text ==` in a comment or a string is then no part of the code.
fn code_of(source: &str) -> (String, Vec<String>) {
    let mut code = String::new();
    let mut literals = Vec::new();
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        code.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = ' ';
                for c in chars.by_ref() {
                    if last == '*' && c == '/' {
                        break;
                    }
                    last = c;
                }
            }
            '"' => {
                let mut lit = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            if let Some(n) = chars.next() {
                                lit.push(n);
                            }
                        }
                        '"' => break,
                        c => lit.push(c),
                    }
                }
                code.push_str(&format!("\"#{}\"", literals.len()));
                literals.push(lit);
            }
            c => code.push(c),
        }
    }
    (code, literals)
}

/// The key handler in `source`, as code ([`code_of`]): from
/// `keys := FocusScope` to the brace that closes it.
fn handler_in(source: &str) -> (String, Vec<String>) {
    let (code, literals) = code_of(source);
    let start = code
        .find("keys := FocusScope {")
        .expect("the window's key handler");
    let mut depth = 0;
    for (i, c) in code[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return (code[start..start + i + 1].to_string(), literals);
                }
            }
            _ => {}
        }
    }
    panic!("the handler's braces do not close");
}

fn handler() -> String {
    handler_in(greycard_slint::APP_SLINT).0
}

/// The names a handler compares a chord with, less the empty one
/// it turns away first.
fn chords_in(source: &str) -> BTreeSet<String> {
    let (h, literals) = handler_in(source);
    let mut out = BTreeSet::new();
    let mut rest = h.as_str();
    while let Some(at) = rest.find("chord == \"#") {
        rest = &rest[at + "chord == \"#".len()..];
        let end = rest.find('"').expect("a closed string");
        let n: usize = rest[..end].parse().expect("a literal's number");
        if !literals[n].is_empty() {
            out.insert(literals[n].clone());
        }
        rest = &rest[end..];
    }
    out
}

fn handler_chords() -> BTreeSet<String> {
    chords_in(greycard_slint::APP_SLINT)
}

/// A comment or a string that looks like a comparison, or holds a
/// brace, is not read as one, and does not end the handler early.
#[test]
fn the_handlers_reading_skips_comments_and_strings() {
    let source = r#"
            keys := FocusScope {
                // chord == "Q" } and e.text == "q"
                /* chord == "R" { */
                key-pressed(e) => {
                    let s = "chord == \"W\" } {";
                    if (chord == "G") { return accept; }
                    if (chord == "Ctrl+'") { }
                }
            }
            if (chord == "Outside") { }
        "#;
    let (code, _) = handler_in(source);
    assert!(!code.contains("e.text =="), "{code}");
    assert!(
        code.contains("return accept"),
        "the handler ends at its own brace"
    );
    let got: Vec<String> = chords_in(source).into_iter().collect();
    assert_eq!(got, ["Ctrl+'", "G"]);
}

#[test]
fn every_key_the_handler_binds_is_on_the_sheet_and_every_line_is_bound() {
    let handler = handler();
    // The handler reads the press only through the table's name.
    assert!(
        !handler.contains("e.text =="),
        "the key handler compares the event's text: bind the key in keys::TABLE and compare the chord"
    );
    // An action's alternates reach it as its default.
    let table: BTreeSet<String> = table_chords()
        .into_iter()
        .filter(|c| !actions().iter().any(|a| a.alternates.contains(c)))
        .chain(BRUSH.iter().map(|(_, n)| *n))
        .map(String::from)
        .collect();
    let compared = handler_chords();
    let unbound: Vec<_> = compared.difference(&table).collect();
    assert!(
        unbound.is_empty(),
        "compared in the handler, not in the table: {unbound:?}"
    );
    let idle: Vec<_> = table.difference(&compared).collect();
    assert!(
        idle.is_empty(),
        "on the sheet, never compared in the handler: {idle:?}"
    );
    // Every chord on the sheet is one the window answers, and is
    // on the sheet: each line's keys are its chords, said.
    for mac in [false, true] {
        let sheet = rows("", mac);
        for b in TABLE.iter().flat_map(|g| g.lines) {
            if !shown_here(b, mac) {
                for c in b.chords {
                    assert!(!bound(c, mac), "{c} is bound where its line is not shown");
                }
                continue;
            }
            let keys = keys_of(b, &Keymap::default(), mac);
            assert!(
                sheet.iter().any(|r| r.keys == keys.as_str()
                    && r.does == platform(&crate::i18n::text(b.does), mac).as_str()),
                "{keys} is not on the sheet"
            );
            for c in b.chords {
                if !here(c, mac) {
                    continue;
                }
                assert!(bound(c, mac), "{c}");
            }
        }
        // The lines and the headings, and nothing else.
        let lines = TABLE
            .iter()
            .flat_map(|g| g.lines)
            .filter(|b| shown_here(b, mac))
            .count()
            + POINTER.len();
        assert_eq!(sheet.len(), lines + TABLE.len());
    }
}

#[test]
fn a_press_is_named_as_the_table_names_it() {
    let esc = SharedString::from(Key::Escape);
    let left = SharedString::from(Key::LeftArrow);
    assert_eq!(name("g", false, false, false, false), "G");
    assert_eq!(
        name("G", false, false, false, false),
        "G",
        "Caps Lock is no Shift"
    );
    assert_eq!(name("S", true, true, false, false), "Ctrl+Shift+S");
    assert_eq!(name("s", true, true, false, false), "Ctrl+Shift+S");
    assert_eq!(name("?", false, true, false, false), "?");
    assert_eq!(name("+", false, true, false, false), "+");
    assert_eq!(
        name("1", false, true, false, false),
        "1",
        "a digit typed with Shift"
    );
    assert_eq!(name(&left, false, true, false, false), "Shift+Left");
    assert_eq!(name(&esc, false, false, false, false), "Esc");
    assert_eq!(
        name(&esc, true, true, true, false),
        "Esc",
        "Esc whatever is held"
    );
    assert_eq!(name(" ", false, false, false, false), "Space");
    assert_eq!(name("'", true, false, false, false), "Ctrl+'");
    assert_eq!(name("g", false, false, true, false), "Alt+G");
    // Bound or not.
    assert_eq!(chord_on("g", false, false, false, false, false, LOUPE), "G");
    assert_eq!(
        chord_on("g", false, false, true, false, false, LOUPE),
        "",
        "Alt+G is nobody's"
    );
    assert_eq!(chord_on("q", false, false, false, false, false, LOUPE), "");
    // What master took, kept: the flags with Shift, the tiles' size
    // and the ends with Ctrl.
    assert_eq!(
        chord_on("P", false, true, false, false, false, LOUPE),
        "Shift+P"
    );
    assert_eq!(
        chord_on("=", true, false, false, false, false, LOUPE),
        "Ctrl+="
    );
    let home = SharedString::from(Key::Home);
    assert_eq!(
        chord_on(&home, true, false, false, false, false, LOUPE),
        "Ctrl+Home"
    );
    // And what is gone on purpose.
    assert_eq!(chord_on("l", false, true, false, false, false, LOUPE), "");
    assert_eq!(chord_on("z", true, false, true, false, false, LOUPE), "");
    let back = SharedString::from(Key::Backspace);
    assert_eq!(
        chord_on(&back, false, false, false, false, false, LOUPE),
        ""
    );
    assert_eq!(
        chord_on(&back, false, false, false, false, true, LOUPE),
        "Backspace"
    );
}

/// F11 is full screen everywhere but a Mac, where the key is the
/// system's and its line is off the sheet; nothing else there
/// loses a line.
#[test]
fn f11_is_bound_everywhere_but_a_mac() {
    KEYMAP.with(|m| m.borrow_mut().reset());
    let f11 = SharedString::from(Key::F11);
    assert_eq!(
        chord_on(&f11, false, false, false, false, false, LOUPE),
        "F11"
    );
    assert_eq!(chord_on(&f11, false, false, false, false, true, LOUPE), "");
    let full = |mac| {
        let does = crate::i18n::text("Full screen, and back");
        rows("", mac)
            .iter()
            .any(|r| !r.heading && r.does == does && r.keys == "F11")
    };
    assert!(full(false));
    assert!(!full(true));
    assert_eq!(rows("", true).len() + 1, rows("", false).len());
}

#[test]
fn the_sheet_says_the_platforms_modifier() {
    // A line by its words in the catalog's language, as the sheet
    // shows it.
    let find = |mac, does: &str| {
        let does = crate::i18n::text(does);
        rows("", mac)
            .into_iter()
            .find(|r| r.does == does)
            .map(|r| r.keys.to_string())
    };
    assert_eq!(find(false, "Undo").as_deref(), Some("Ctrl+Z"));
    assert_eq!(find(true, "Undo").as_deref(), Some("Cmd+Z"));
    let or = |a: &str, b: &str| crate::tr!("{} or {}", a, b);
    assert_eq!(find(false, "Redo"), Some(or("Ctrl+Shift+Z", "Ctrl+Y")));
    assert_eq!(find(true, "Redo"), Some(or("Cmd+Shift+Z", "Cmd+Y")));
    let delete = "Delete the selected frames, after a confirmation";
    assert_eq!(find(false, delete).as_deref(), Some("Delete"));
    assert_eq!(find(true, delete).as_deref(), Some("Delete, ⌫"));
    let pointer = "Add a frame to the selection, or take it out";
    assert_eq!(find(true, pointer).as_deref(), Some("Cmd+click"));
}

#[test]
fn a_search_keeps_the_lines_that_hold_it_under_their_headings() {
    let sheet = rows("stack", false);
    let words: Vec<(bool, String)> = sheet
        .iter()
        .map(|r| (r.heading, r.keys.to_string()))
        .collect();
    assert_eq!(
        words,
        [
            (true, "Views".to_string()),
            (false, "S".to_string()),
            (true, "Rating and sorting".to_string()),
            (false, "Ctrl+G".to_string()),
            (false, "Ctrl+Shift+G".to_string()),
            (false, "S, Shift+S".to_string()),
            (false, "Shift+[ Shift+]".to_string()),
        ]
    );
    // Keys are searched too, and a heading brings its group.
    assert!(
        rows("ctrl+shift+s", false)
            .iter()
            .any(|r| r.does.contains("Sync"))
    );
    let editing = rows("editing", false);
    assert_eq!(editing.len(), 1 + TABLE[3].lines.len());
    assert!(rows("nothing binds this", false).is_empty());
}

fn row_count(app: &App) -> usize {
    app.get_keys_rows().row_count()
}

#[test]
fn question_mark_opens_the_sheet_and_escape_closes_it() {
    let app = window(3);
    // A bare window, as a test's: the chords still answer.
    assert!(!app.get_keys_open());
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    press(&app, "?");
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
    assert!(app.get_keys_open());
    assert_eq!(row_count(&app), rows("", cfg!(target_os = "macos")).len());
    // The search narrows it, and an emptied search brings it back.
    app.invoke_keys_searched("lightbox".into());
    assert!(row_count(&app) < 6, "{}", row_count(&app));
    app.invoke_keys_searched("".into());
    assert_eq!(row_count(&app), rows("", cfg!(target_os = "macos")).len());
    // Under the sheet the window's keys are nobody's: G does not
    // open the grid behind it.
    press(&app, "g");
    assert!(!app.get_grid_open());
    press(&app, Key::Escape);
    assert!(!app.get_keys_open());
    // And they are the window's again, once the sheet's closing
    // has handed them back.
    slint::platform::update_timers_and_animations();
    press(&app, "g");
    assert!(app.get_grid_open());
}

/// `key` pressed with Shift held.
fn shifted(app: &App, key: &str) {
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    press(app, key);
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
}

/// A window with a frame open in Develop, as the tools' keys want.
fn develop_window() -> (
    App,
    std::rc::Rc<crate::Shared>,
    std::rc::Rc<crate::worker::Worker>,
) {
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    app.invoke_select(0);
    (app, state, worker)
}

/// R is the Crop button: its tab, crop mode in, and out again with
/// the level put down, as the button's Done leaves it.
#[test]
fn r_takes_the_crop_in_and_out_as_its_button_does() {
    let (app, _state, _worker) = develop_window();
    assert_eq!(app.get_panel_tab(), "Develop");
    press(&app, "r");
    assert_eq!(app.get_panel_tab(), "Crop");
    assert!(app.get_crop_mode());
    app.set_level_mode(true);
    press(&app, "r");
    assert!(!app.get_crop_mode());
    assert!(!app.get_level_mode(), "put down as the button does");
    assert_eq!(app.get_panel_tab(), "Crop");
    // The button and the key are one: in by the key, out by the
    // button, which reads Done.
    press(&app, "r");
    assert!(app.get_crop_mode());
    let (at, size) = crate::testing::by_id(&app, "CropSection::crop-done-button");
    crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert!(!app.get_crop_mode());
}

/// F puts the mat on the fit and takes it off; the settings keep
/// it, and a zoom asked for is not touched.
#[test]
fn f_mats_the_fitted_picture_and_again_takes_it_off() {
    let (app, state, _worker) = develop_window();
    state.borrow_mut().develop.image_size = (6000, 4000);
    let fit = |st: &crate::State| crate::panel::viewport::effective_zoom(st, 1000, 800);
    let bare = fit(&state.borrow());
    press(&app, "f");
    assert!(state.borrow().develop.mat);
    let matted = fit(&state.borrow());
    assert!(matted < bare, "{matted} inside the mat, {bare} without");
    let mut kept = crate::settings::Settings::default();
    crate::panel::startup::take_state_choices(&mut kept, &state.borrow());
    assert!(kept.mat, "kept for the next run");
    state.borrow_mut().develop.zoom = 1.0;
    assert_eq!(fit(&state.borrow()), 1.0, "1:1 fills the view");
    state.borrow_mut().develop.zoom = 0.0;
    press(&app, "f");
    assert!(!state.borrow().develop.mat);
    assert_eq!(fit(&state.borrow()), bare);
}

/// W is the white balance's pipette, from any tab; again, put
/// down. From the crop, it leaves the crop for the Develop tab.
#[test]
fn w_arms_the_white_balance_pipette() {
    let (app, _state, _worker) = develop_window();
    press(&app, "w");
    assert_eq!(app.get_picking(), "White");
    press(&app, "w");
    assert_eq!(app.get_picking(), "", "again, put down");
    press(&app, "r");
    assert!(app.get_crop_mode());
    press(&app, "w");
    assert_eq!(app.get_panel_tab(), "Develop");
    assert!(!app.get_crop_mode(), "the frame shown whole again");
    assert_eq!(app.get_picking(), "White");
}

/// M starts a mask with a Linear in hand, Shift+M one with a
/// Radial, and the next drag places it; M or Shift+M before the
/// drag changes the shape in hand rather than leave an empty mask
/// behind, and the same key again puts it down.
#[test]
fn m_and_shift_m_start_a_mask_placed_by_the_next_drag() {
    let (app, state, _worker) = develop_window();
    let masks = |state: &std::rc::Rc<crate::Shared>| state.borrow().develop.edit.adjustments.len();
    press(&app, "m");
    assert_eq!(app.get_panel_tab(), "Masks");
    assert_eq!(app.get_placing(), "Linear");
    assert!(app.get_placing_into());
    assert_eq!(masks(&state), 1);
    app.invoke_place_pressed(300.0, 200.0, false);
    app.invoke_place_dragged(600.0, 500.0);
    app.invoke_place_released();
    assert_eq!(app.get_component_kind(), "Linear");
    assert_eq!(
        state.borrow().develop.edit.adjustments[0]
            .mask
            .components
            .len(),
        1
    );

    shifted(&app, "M");
    assert_eq!(app.get_placing(), "Radial");
    assert_eq!(masks(&state), 2);
    press(&app, "m");
    assert_eq!(app.get_placing(), "Linear", "the shape in hand changed");
    assert_eq!(masks(&state), 2, "in the same mask");
    shifted(&app, "M");
    assert_eq!(app.get_placing(), "Radial");
    app.invoke_place_pressed(300.0, 200.0, false);
    app.invoke_place_dragged(600.0, 500.0);
    app.invoke_place_released();
    assert_eq!(app.get_component_kind(), "Radial");
    assert_eq!(masks(&state), 2);

    // Again before a drag: down, as the button again.
    press(&app, "m");
    assert_eq!(app.get_placing(), "Linear");
    assert_eq!(masks(&state), 3);
    press(&app, "m");
    assert_eq!(app.get_placing(), "");
}

/// With the brush in hand, Shift and a bracket take its feather a
/// twentieth down or up and Ctrl and one its flow, held to the
/// sliders' range; with no brush they do neither. O is the chosen
/// mask's Show mask, as a click on it.
#[test]
fn the_brush_keys_feather_and_flow_and_o_shows_the_mask() {
    let (app, _state, _worker) = develop_window();
    let ctrl = |key: &str| {
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        press(&app, key);
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
    };
    // No brush: the keys leave the brush as it is.
    shifted(&app, "}");
    ctrl("[");
    assert_eq!(app.get_brush_feather(), 0.5);
    assert_eq!(app.get_brush_flow(), 1.0);
    press(&app, "m");
    app.invoke_add_shape("Brush".into(), "Add".into());
    assert_eq!(app.get_placing(), "Brush");
    shifted(&app, "}");
    assert!((app.get_brush_feather() - 0.55).abs() < 1e-6);
    shifted(&app, "{");
    shifted(&app, "{");
    assert!((app.get_brush_feather() - 0.45).abs() < 1e-6);
    ctrl("[");
    assert!((app.get_brush_flow() - 0.95).abs() < 1e-6);
    for _ in 0..24 {
        ctrl("]");
    }
    assert_eq!(app.get_brush_flow(), 1.0, "held at the top");
    // Show mask, on and off.
    let shown = app.get_show_mask();
    press(&app, "o");
    assert_eq!(app.get_show_mask(), !shown);
    press(&app, "o");
    assert_eq!(app.get_show_mask(), shown);
}

/// The brackets turn the frame with nothing in hand; with the
/// brush in hand they size it, and with a repair's patch the
/// patch, by the wheel's step, and the frame stays as it was.
#[test]
fn the_brackets_size_the_brush_in_hand_and_else_turn_the_frame() {
    let (app, _state, _worker) = develop_window();
    let turns = std::rc::Rc::new(std::cell::Cell::new(0));
    {
        let turns = turns.clone();
        app.on_frame_turned(move |_| turns.set(turns.get() + 1));
    }
    press(&app, "]");
    assert_eq!(turns.get(), 1, "nothing in hand: a turn");
    press(&app, "m");
    app.invoke_add_shape("Brush".into(), "Add".into());
    assert_eq!(app.get_placing(), "Brush");
    let size = app.get_brush_size();
    press(&app, "]");
    assert!((app.get_brush_size() - size * 1.1).abs() < 1e-6);
    assert_eq!(turns.get(), 1, "the brush sized, the frame not turned");
    press(&app, "[");
    press(&app, "[");
    assert!((app.get_brush_size() - size / 1.1).abs() < 1e-6);
    for _ in 0..80 {
        press(&app, "[");
    }
    assert_eq!(app.get_brush_size(), 0.002, "held at the wheel's floor");
    assert_eq!(turns.get(), 1);
    // Put down: the brackets turn again.
    press(&app, Key::Escape);
    assert_eq!(app.get_placing(), "");
    press(&app, "[");
    assert_eq!(turns.get(), 2);
    // A repair's patch in hand.
    let edited = std::rc::Rc::new(std::cell::Cell::new(0));
    {
        let edited = edited.clone();
        app.on_patch_edited(move || edited.set(edited.get() + 1));
    }
    app.set_retouch_mode("Heal".into());
    let patch = app.get_patch_size();
    press(&app, "]");
    assert!((app.get_patch_size() - patch * 1.1).abs() < 1e-6);
    assert_eq!(edited.get(), 1, "edited as the wheel's sizing is");
    assert_eq!(turns.get(), 2);
    app.set_retouch_mode("".into());

    // The turn on no key: the bracket still sizes the brush in
    // hand, and turns nothing without it.
    KEYMAP.with(|m| m.borrow_mut().clear("turn-right"));
    press(&app, "m");
    app.invoke_add_shape("Brush".into(), "Add".into());
    let size = app.get_brush_size();
    press(&app, "]");
    assert!((app.get_brush_size() - size * 1.1).abs() < 1e-6);
    assert_eq!(turns.get(), 2);
    // The turn on Q: the bracket sizes, Q turns, the brush in hand
    // or not.
    KEYMAP.with(|m| m.borrow_mut().set("turn-right", "Q"));
    let size = app.get_brush_size();
    press(&app, "]");
    assert!((app.get_brush_size() - size * 1.1).abs() < 1e-6);
    assert_eq!(turns.get(), 2);
    press(&app, "q");
    assert_eq!(turns.get(), 3, "Q turns");
    assert!(
        (app.get_brush_size() - size * 1.1).abs() < 1e-6,
        "and sizes nothing"
    );
    press(&app, Key::Escape);
    press(&app, "]");
    assert_eq!(turns.get(), 3, "] is nobody's with nothing in hand");
    press(&app, "q");
    assert_eq!(turns.get(), 4);
    // The table's names for it.
    let mut map = Keymap::default();
    assert_eq!(map.resolve("]", false, IN_HAND), "Brush]");
    assert_eq!(map.resolve("]", false, LOUPE), "]");
    assert!(map.set("turn-right", "Q"));
    assert_eq!(map.resolve("Q", false, IN_HAND), "]");
    assert_eq!(map.resolve("]", false, IN_HAND), "Brush]");
}

/// The step on a chord with Shift in it steps without adding to
/// the selection: the Shift is the chord's, not a Shift form.
#[test]
fn the_step_on_a_shifted_chord_is_a_plain_step() {
    let app = window(11);
    KEYMAP.with(|m| m.borrow_mut().set("previous-frame", "Shift+A"));
    let steps = Rc::new(RefCell::new(Vec::new()));
    let flat = steps.clone();
    app.on_step(move |by, extend| flat.borrow_mut().push((by, extend)));
    shifted(&app, "A");
    shifted(&app, &SharedString::from(Key::RightArrow));
    assert_eq!(*steps.borrow(), vec![(-1, false), (1, true)]);
}

/// The step's Shift form conflicts as a chord does: on M it takes
/// Shift+M from the Radial, and both say so; moved, its old
/// Shift+Left is free, and an action there conflicts with nothing.
#[test]
fn the_steps_shift_form_is_in_the_conflicts() {
    let added = "Previous or next frame, added to the selection";
    let mut map = Keymap::default();
    assert!(map.set("previous-frame", "M"));
    let found = map.conflicts(false);
    fn of(found: &[(&'static Action, Vec<&'static str>)], id: &str) -> Vec<&'static str> {
        found
            .iter()
            .find(|(a, _)| a.id == id)
            .map(|(_, o)| o.clone())
            .unwrap_or_default()
    }
    assert!(
        of(&found, "previous-frame").contains(&"New mask with a Radial, placed by the next drag"),
        "{found:?}"
    );
    assert!(of(&found, "radial-mask").contains(&added), "{found:?}");
    // Moved: Shift+Left is nobody's, so the crop there is alone.
    let mut map = Keymap::default();
    assert!(map.set("previous-frame", "A"));
    assert!(map.set("crop", "Shift+Left"));
    assert!(
        map.conflicts(false).is_empty(),
        "{:?}",
        map.conflicts(false)
    );
    // Unmoved, the crop takes it from the step.
    let mut map = Keymap::default();
    assert!(map.set("crop", "Shift+Left"));
    assert_eq!(of(&map.conflicts(false), "crop"), vec![added]);
}

/// The turn moved among the brackets conflicts with nothing: the
/// brush's line shares them with the turn by design.
#[test]
fn the_turn_on_the_other_bracket_conflicts_with_nothing() {
    let mut map = Keymap::default();
    assert!(map.set("turn-right", "Q"));
    assert!(map.set("turn-left", "]"));
    assert!(
        map.conflicts(false).is_empty(),
        "{:?}",
        map.conflicts(false)
    );
    // Another action on a bracket is the brush's conflict.
    assert!(map.set("crop", "["));
    assert_eq!(
        map.conflicts(false)
            .iter()
            .find(|(a, _)| a.id == "crop")
            .map(|(_, o)| o.clone()),
        Some(vec!["Brush size, smaller and bigger"])
    );
}

/// The step moved and an action on Shift+Left: the Loupe gives it
/// to the action, the grid keeps it for adding to the selection.
#[test]
fn the_grid_keeps_shift_left_from_an_action_there() {
    let mut map = Keymap::default();
    assert!(map.set("previous-frame", "A"));
    assert!(map.set("crop", "Shift+Left"));
    assert_eq!(map.resolve("Shift+Left", false, LOUPE), "R");
    assert_eq!(map.resolve("Shift+Left", false, GRID), "Shift+Left");
    assert_eq!(map.resolve("Shift+A", false, LOUPE), "Shift+Left");
    assert_eq!(map.resolve("Left", false, GRID), "Left");
}

/// A pair's two on one chord: the row is red, with no words of its
/// own; a third action on it is named.
#[test]
fn a_pairs_own_partner_is_red_without_words() {
    let mut map = Keymap::default();
    assert!(map.set("turn-left", "Q"));
    assert!(map.set("turn-right", "Q"));
    let rows = binding_rows(&map, false);
    let turn = rows.iter().find(|r| r.id == "turn-left").unwrap();
    assert!(turn.warn);
    assert_eq!(turn.also, "");
    assert!(map.set("crop", "Q"));
    let rows = binding_rows(&map, false);
    let turn = rows.iter().find(|r| r.id == "turn-left").unwrap();
    assert!(turn.warn);
    assert_eq!(turn.also, "Also: Crop, and Done");
    let fine = rows.iter().find(|r| r.id == "library").unwrap();
    assert!(!fine.warn && fine.also.is_empty());
}

/// The step moved off the arrows: the Loupe steps on its new key,
/// with Shift on it too, and not on the arrow; the grid and the
/// lightbox keep the arrows for their selection.
#[test]
fn the_loupes_step_rebound_leaves_the_grids_arrows() {
    let mut map = Keymap::default();
    assert!(map.set("previous-frame", "A"));
    assert_eq!(map.resolve("A", false, LOUPE), "Left");
    assert_eq!(map.resolve("Left", false, LOUPE), "", "the arrow is free");
    assert_eq!(map.resolve("Shift+A", false, LOUPE), "Shift+Left");
    assert_eq!(map.resolve("Shift+Left", false, LOUPE), "");
    assert_eq!(map.resolve("Right", false, LOUPE), "Right", "not moved");
    assert_eq!(map.resolve("Shift+Right", false, LOUPE), "Shift+Right");
    // Out of the Loupe, the defaults.
    assert_eq!(map.resolve("Left", false, GRID), "Left");
    assert_eq!(map.resolve("Shift+Left", false, GRID), "Shift+Left");
    assert_eq!(map.resolve("A", false, GRID), "");
    // The help says the keys as bound.
    let sheet = rows_in(&map, "", false);
    let keys = |does: &str| {
        sheet
            .iter()
            .find(|r| r.does == crate::i18n::text(does))
            .map(|r| r.keys.to_string())
    };
    assert_eq!(keys("Previous or next frame").as_deref(), Some("A, →"));
    assert_eq!(
        keys("Previous or next frame, added to the selection").as_deref(),
        Some("Shift+A, Shift+→")
    );
    // A key that carries its Shift has no Shift form.
    assert!(map.set("previous-frame", "?"));
    assert_eq!(map.resolve("Shift+Left", false, LOUPE), "");
    // And through the window.
    let app = window(11);
    KEYMAP.with(|m| m.borrow_mut().set("previous-frame", "A"));
    let steps = Rc::new(RefCell::new(Vec::new()));
    let flat = steps.clone();
    app.on_step(move |by, extend| flat.borrow_mut().push((by, extend)));
    let grid = Rc::new(RefCell::new(Vec::new()));
    let two = grid.clone();
    app.on_grid_step(move |dx, dy, _, _| two.borrow_mut().push((dx, dy)));
    press(&app, "a");
    press(&app, Key::LeftArrow);
    shifted(&app, "A");
    press(&app, Key::RightArrow);
    assert_eq!(*steps.borrow(), vec![(-1, false), (-1, true), (1, false)]);
    press(&app, "g");
    press(&app, Key::LeftArrow);
    press(&app, "a");
    assert_eq!(*grid.borrow(), vec![(-1, 0)], "the grid's arrow, as ever");
    assert_eq!(steps.borrow().len(), 3);
    press(&app, "l");
    assert!(app.get_lightbox());
    press(&app, Key::LeftArrow);
    assert_eq!(grid.borrow().len(), 2, "the lightbox's too");
    press(&app, "l");
    press(&app, "g");
    assert!(!app.get_grid_open());
    // Bound to no key: the arrow does nothing in the Loupe.
    KEYMAP.with(|m| m.borrow_mut().clear("previous-frame"));
    steps.borrow_mut().clear();
    press(&app, Key::LeftArrow);
    press(&app, "a");
    shifted(&app, &SharedString::from(Key::LeftArrow));
    assert!(steps.borrow().is_empty(), "{:?}", steps.borrow());
    press(&app, "g");
    press(&app, Key::LeftArrow);
    assert_eq!(grid.borrow().len(), 3);
}

/// No key for an action: its rows say None, nothing conflicts
/// over it, and two of them share nothing.
#[test]
fn an_action_bound_to_no_key_reads_none() {
    let mut map = Keymap::default();
    assert!(map.clear("turn-left"));
    assert!(map.clear("crop"));
    assert!(!map.clear("fly"));
    assert!(map.conflicts(false).is_empty());
    assert_eq!(map.resolve("[", false, LOUPE), "");
    assert_eq!(map.resolve("]", false, LOUPE), "]");
    let rows = binding_rows(&map, false);
    let turn = rows.iter().find(|r| r.id == "turn-left").unwrap();
    assert_eq!(
        (turn.keys.as_str(), turn.keys2.as_str()),
        (no_chord().as_str(), "]")
    );
    let crop = rows.iter().find(|r| r.id == "crop").unwrap();
    assert_eq!(crop.keys, no_chord());
    let sheet = rows_in(&map, "", false);
    let turn = crate::i18n::text("Turn the frame a quarter left or right; not part of the edit");
    assert!(
        sheet
            .iter()
            .any(|r| r.keys == format!("{}, ]", no_chord()) && r.does == turn),
        "the turn's help line"
    );
    assert!(
        sheet
            .iter()
            .any(|r| r.keys == "[ ]"
                && r.does == crate::i18n::text("Brush size, smaller and bigger")),
        "the brush's size stays on the brackets"
    );
    assert_eq!(map.saved().get("turn-left").map(String::as_str), Some(""));
    assert_eq!(Keymap::from_saved(&map.saved()), map);
    map.reset();
    assert_eq!(map, Keymap::default());
}

/// R in the middle of a crop's drag: the crop as dragged so far is
/// the edit, saved as the release would have, and the release
/// after it adds nothing.
#[test]
fn r_in_the_middle_of_a_crop_drag_keeps_the_drag() {
    let (app, _state, _worker) = develop_window();
    let changes = std::rc::Rc::new(std::cell::Cell::new(0));
    {
        let changes = changes.clone();
        app.on_view_changed(move || changes.set(changes.get() + 1));
    }
    press(&app, "r");
    assert!(app.get_crop_mode());
    let entered = changes.get();
    app.invoke_crop_pressed(0);
    press(&app, "r");
    assert!(!app.get_crop_mode());
    assert_eq!(changes.get(), entered + 1, "the drag made the edit");
    app.invoke_crop_released();
    assert_eq!(changes.get(), entered + 1, "and the release nothing more");
    // Without a drag, leaving makes no edit.
    press(&app, "r");
    let entered = changes.get();
    press(&app, "r");
    assert_eq!(changes.get(), entered);
}

/// The pipette in hand, then M or R: the pipette is put down, so
/// the next drag is the shape's or the crop's alone.
#[test]
fn a_tool_key_puts_the_pipette_down() {
    let (app, state, _worker) = develop_window();
    press(&app, "w");
    assert_eq!(app.get_picking(), "White");
    press(&app, "m");
    assert_eq!(app.get_picking(), "");
    assert_eq!(app.get_placing(), "Linear");
    // Placed by the drag, and only placed.
    app.invoke_place_pressed(300.0, 200.0, false);
    app.invoke_place_dragged(600.0, 500.0);
    app.invoke_place_released();
    assert_eq!(app.get_component_kind(), "Linear");
    assert_eq!(app.get_picking(), "");

    press(&app, "w");
    assert_eq!(app.get_picking(), "White");
    press(&app, "r");
    assert_eq!(app.get_picking(), "", "put down for the crop");
    assert!(app.get_crop_mode());
    press(&app, "r");
    press(&app, "m");
    assert_eq!(app.get_picking(), "");
    assert_eq!(app.get_placing(), "Linear");
    assert_eq!(state.borrow().develop.edit.adjustments.len(), 2);
}

/// With no frame open, the tools' keys do nothing.
#[test]
fn the_tool_keys_do_nothing_with_no_frame_open() {
    let app = window(0);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::state_for(&app, Vec::new());
    assert_eq!(app.get_selected(), -1);
    press(&app, "r");
    press(&app, "w");
    press(&app, "m");
    shifted(&app, "M");
    assert!(!app.get_crop_mode());
    assert_eq!(app.get_picking(), "");
    assert_eq!(app.get_placing(), "");
    assert_eq!(app.get_panel_tab(), "Develop");
    assert!(state.borrow().develop.edit.adjustments.is_empty());
}

/// In the grid, the lightbox, and culling the tools' keys are
/// nobody's.
#[test]
fn the_tool_keys_do_nothing_in_the_grid_or_culling() {
    let (app, state, _worker) = develop_window();
    // The tools pressed: nothing in hand, no mask, the tab as it
    // was (culling's own panel is its own tab).
    let tools_do_nothing = |app: &App| {
        let tab = app.get_panel_tab();
        for k in ["r", "w", "m"] {
            press(app, k);
        }
        shifted(app, "M");
        assert!(!app.get_crop_mode());
        assert_eq!(app.get_picking(), "");
        assert_eq!(app.get_placing(), "");
        assert_eq!(app.get_panel_tab(), tab);
        assert!(state.borrow().develop.edit.adjustments.is_empty());
    };
    press(&app, "g");
    assert!(app.get_grid_open());
    tools_do_nothing(&app);
    press(&app, "l");
    assert!(app.get_lightbox());
    tools_do_nothing(&app);
    press(&app, "l");
    press(&app, "g");
    assert!(!app.get_grid_open());
    press(&app, "c");
    assert!(app.get_cull_mode());
    tools_do_nothing(&app);
}

/// Every chord the table holds, and a few of the user's, read back
/// from what they print as and from how a sheet says them.
#[test]
fn a_chord_parses_and_prints_both_ways() {
    for c in table_chords() {
        assert_eq!(parse(c).as_deref(), Some(c), "{c}");
        for mac in [false, true] {
            assert_eq!(
                parse(&said(c, mac)).as_deref(),
                Some(c),
                "{c} on a Mac: {mac}"
            );
        }
    }
    let cases = [
        ("ctrl+r", "Ctrl+R"),
        ("Cmd+Shift+s", "Ctrl+Shift+S"),
        ("shift+ctrl+alt+K", "Ctrl+Alt+Shift+K"),
        ("Meta+F2", "Meta+F2"),
        ("Shift+←", "Shift+Left"),
        ("Enter", "Return"),
        ("Ctrl++", "Ctrl++"),
        ("+", "+"),
        ("Ctrl+'", "Ctrl+'"),
        ("space", "Space"),
        ("Ctrl+Escape", "Esc"),
    ];
    for (text, chord) in cases {
        assert_eq!(parse(text).as_deref(), Some(chord), "{text}");
    }
    for text in [
        "", "Ctrl+", "Hyper+R", "Shift+?", "Shift+1", "RR", "Ctrl+Fly",
    ] {
        assert_eq!(parse(text), None, "{text}");
    }
    assert_eq!(said("Ctrl+Shift+R", true), "Cmd+Shift+R");
    assert_eq!(said("Shift+Left", false), "Shift+←");
    assert_eq!(said("Ctrl++", true), "Cmd++");
    // A press names what parses: an F key, and nothing for a
    // modifier alone.
    let f2 = SharedString::from(Key::F2);
    assert_eq!(name(&f2, true, false, false, false), "Ctrl+F2");
    let shift = SharedString::from(Key::Shift);
    assert_eq!(name(&shift, false, true, false, false), "");
    assert!(!bindable("Esc"), "Escape cancels, and is nobody's to bind");
}

/// The actions are the table's: one chord a line, or a pair's two,
/// one default an action, none shared, and none in conflict as
/// shipped.
#[test]
fn the_actions_are_the_tables_lines() {
    let lines: Vec<&Binding> = TABLE
        .iter()
        .flat_map(|g| g.lines)
        .filter(|b| b.action.is_some() || b.pair.is_some())
        .collect();
    for b in &lines {
        assert!(b.action.is_none() || b.pair.is_none(), "{}", b.does);
        let ids = actions_of(b);
        let mut held = 0;
        for (i, (id, _)) in ids.into_iter().enumerate() {
            let a = actions()
                .iter()
                .find(|a| a.id == id)
                .expect("each line's action");
            assert_eq!(a.default, b.chords[i], "{}: one default", a.id);
            assert!(bindable(a.default) && here(a.default, false) && here(a.default, true));
            held += 1 + a.alternates.len();
            for c in a.alternates {
                assert!(bindable(c) && here(c, false) && here(c, true), "{c}");
            }
        }
        assert_eq!(b.chords.len(), held, "{}", b.does);
    }
    let defaults: BTreeSet<&str> = actions()
        .iter()
        .flat_map(|a| std::iter::once(a.default).chain(a.alternates.iter().copied()))
        .collect();
    let held: usize = actions().iter().map(|a| 1 + a.alternates.len()).sum();
    assert_eq!(defaults.len(), held, "two actions, one default");
    // Redo alone has a second default.
    let alternated: Vec<_> = actions()
        .iter()
        .filter(|a| !a.alternates.is_empty())
        .map(|a| (a.id, a.default, a.alternates))
        .collect();
    assert_eq!(
        alternated,
        [("redo", "Ctrl+Shift+Z", &["Ctrl+Y"][..])],
        "the only action with two defaults"
    );
    // J's two lines are one action.
    let j = actions().iter().find(|a| a.default == "J").unwrap();
    assert_eq!(j.does.len(), 2);
    assert!(does_of(j).contains("; in the Loupe"), "{}", does_of(j));
    assert!(Keymap::default().conflicts(false).is_empty());
    assert!(Keymap::default().conflicts(true).is_empty());
    // The sheet's rows: each action once, a pair's two on one row,
    // under the headings.
    let rows = binding_rows(&Keymap::default(), false);
    let pairs = actions().iter().filter(|a| a.paired).count();
    assert_eq!(pairs, 3, "the step, the way back, and the turn");
    assert_eq!(
        rows.iter().filter(|r| !r.heading).count(),
        actions().len() - pairs
    );
    let turn = rows.iter().find(|r| r.id == "turn-left").unwrap();
    assert_eq!((turn.keys.as_str(), turn.id2.as_str()), ("[", "turn-right"));
    assert_eq!(turn.keys2, "]");
    assert!(rows[0].heading);
}

/// A rebinding moves the action's every line, on both sheets, and
/// what the window reads; its old chord is nobody's.
#[test]
fn a_rebinding_moves_the_action_and_frees_its_old_chord() {
    let mut map = Keymap::default();
    assert!(map.set("proof", "Q"));
    assert!(!map.set("fly", "Q"), "no such action");
    assert!(!map.set("crop", "Shift+?"), "no such chord");
    assert_eq!(
        map.resolve("Q", false, LOUPE),
        "S",
        "the handler's name for it"
    );
    assert_eq!(map.resolve("S", false, LOUPE), "", "S is free");
    assert_eq!(map.resolve("Shift+S", false, LOUPE), "Shift+S", "fixed");
    let sheet = rows_in(&map, "", false);
    let keys = |does: &str| {
        sheet
            .iter()
            .find(|r| r.does == crate::i18n::text(does))
            .map(|r| r.keys.to_string())
    };
    assert_eq!(
        keys("Soft proof (in the grid, a stack: below)").as_deref(),
        Some("Q")
    );
    assert_eq!(
        keys("In the grid, open or close a stack; make the frame its top").as_deref(),
        Some("Q, Shift+S"),
        "S's other line goes with it"
    );
    assert_eq!(
        keys("In the grid, move the frame up or down in its stack").as_deref(),
        Some("Shift+[ Shift+]"),
        "the braces' own line, as the sheet writes them"
    );
    assert!(map.set("undo", "Ctrl+U"));
    let row = binding_rows(&map, true)
        .into_iter()
        .find(|r| r.id == "undo")
        .unwrap();
    assert_eq!(row.keys, "Cmd+U", "the Mac's own name");
    // Back to the default is not moved.
    assert!(map.set("proof", "S"));
    assert!(map.set("undo", "Ctrl+Z"));
    assert_eq!(map, Keymap::default());
}

/// Two actions on one chord, or an action on a fixed key: each says
/// what else has it. The press goes to an action moved onto the
/// chord before one holding it by default, and between two moved
/// there the first in the table.
#[test]
fn a_shared_chord_is_reported() {
    let mut map = Keymap::default();
    assert!(map.set("crop", "Ctrl+Z"));
    let found = map.conflicts(false);
    let of = |id: &str| {
        found
            .iter()
            .find(|(a, _)| a.id == id)
            .map(|(_, o)| o.clone())
    };
    assert_eq!(of("crop"), Some(vec!["Undo"]));
    assert_eq!(of("undo"), Some(vec!["Crop, and Done"]));
    assert_eq!(found.len(), 2);
    assert_eq!(
        map.resolve("Ctrl+Z", false, LOUPE),
        "R",
        "the crop was put there, undo holds it by default"
    );
    // Both moved there: the first in the table.
    let mut both = map.clone();
    assert!(both.set("undo", "Ctrl+K"));
    assert!(both.set("crop", "Ctrl+K"));
    assert!(both.set("library", "Ctrl+K"));
    assert_eq!(
        both.resolve("Ctrl+K", false, LOUPE),
        "G",
        "library is first"
    );
    let rows = binding_rows(&map, false);
    let also = |id: &str| rows.iter().find(|r| r.id == id).unwrap().also.to_string();
    assert_eq!(also("crop"), "Also: Undo");
    assert_eq!(also("undo"), "Also: Crop, and Done");
    assert_eq!(also("library"), "");
    // On a fixed key the action has it, and says so.
    assert!(map.set("crop", "1"));
    assert_eq!(map.resolve("1", false, LOUPE), "R");
    let found = map.conflicts(false);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, vec!["Set a rating, or clear it"]);
    // Reset to defaults: no conflict, every press as shipped.
    map.reset();
    assert!(map.conflicts(false).is_empty());
    assert_eq!(map.resolve("R", false, LOUPE), "R");
    assert_eq!(map.resolve("1", false, LOUPE), "1");
}

/// Redo's row and its line on the keys sheet, by its words.
fn redo_keys(map: &Keymap, mac: bool) -> (String, String) {
    let row = binding_rows(map, mac)
        .into_iter()
        .find(|r| r.id == "redo")
        .expect("redo's row");
    let line = rows_in(map, "", mac)
        .into_iter()
        .find(|r| !r.heading && r.does == crate::i18n::text("Redo"))
        .expect("redo's line");
    (row.keys.to_string(), line.keys.to_string())
}

/// Redo has two defaults, Ctrl+Shift+Z and Ctrl+Y: both redo while it
/// is unmoved, and its row says the first. Moved, its new chord alone
/// redoes, the two are free, another action takes either with no
/// conflict, and Reset to defaults brings both back.
#[test]
fn redo_moved_lets_go_of_both_defaults_and_reset_brings_them_back() {
    let mut map = Keymap::default();
    for c in ["Ctrl+Shift+Z", "Ctrl+Y"] {
        assert_eq!(map.resolve(c, false, LOUPE), "Ctrl+Shift+Z", "{c}");
        assert_eq!(map.resolve(c, true, GRID), "Ctrl+Shift+Z", "{c}");
    }
    // The Settings row says both, as the keys sheet does, on a wide
    // chip; once moved, one chord on a chip of one.
    let or = |a: &str, b: &str| crate::tr!("{} or {}", a, b);
    let both = or("Ctrl+Shift+Z", "Ctrl+Y");
    assert_eq!(redo_keys(&map, false), (both.clone(), both));
    let both = or("Cmd+Shift+Z", "Cmd+Y");
    assert_eq!(redo_keys(&map, true), (both.clone(), both));
    let wide = |map: &Keymap| {
        binding_rows(map, false)
            .into_iter()
            .filter(|r| r.wide)
            .map(|r| r.id.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(wide(&map), ["redo"]);

    assert!(map.set("redo", "Ctrl+U"));
    assert_eq!(map.resolve("Ctrl+U", false, LOUPE), "Ctrl+Shift+Z");
    for c in ["Ctrl+Shift+Z", "Ctrl+Y"] {
        assert_eq!(map.resolve(c, false, LOUPE), "", "{c} is free");
    }
    assert_eq!(redo_keys(&map, false), ("Ctrl+U".into(), "Ctrl+U".into()));
    assert_eq!(redo_keys(&map, true), ("Cmd+U".into(), "Cmd+U".into()));
    assert!(wide(&map).is_empty());
    assert!(map.conflicts(false).is_empty());
    // Either default, now free, is another action's without a word.
    assert!(map.set("crop", "Ctrl+Y"));
    assert!(map.set("mat", "Ctrl+Shift+Z"));
    assert_eq!(map.resolve("Ctrl+Y", false, LOUPE), "R");
    assert_eq!(map.resolve("Ctrl+Shift+Z", false, LOUPE), "F");
    assert!(map.conflicts(false).is_empty());
    assert!(map.conflicts(true).is_empty());
    let saved = map.saved();
    assert_eq!(saved.get("redo").map(String::as_str), Some("Ctrl+U"));
    assert_eq!(Keymap::from_saved(&saved), map);

    // Moved onto its second default, that alone redoes.
    let mut second = Keymap::default();
    assert!(second.set("redo", "Ctrl+Y"));
    assert_eq!(second.resolve("Ctrl+Y", false, LOUPE), "Ctrl+Shift+Z");
    assert_eq!(second.resolve("Ctrl+Shift+Z", false, LOUPE), "");
    assert_eq!(
        redo_keys(&second, false),
        ("Ctrl+Y".into(), "Ctrl+Y".into())
    );
    // Bound to no key: none redoes, and the sheet says None.
    assert!(second.clear("redo"));
    for c in ["Ctrl+Shift+Z", "Ctrl+Y"] {
        assert_eq!(second.resolve(c, false, LOUPE), "", "{c}");
    }
    assert_eq!(redo_keys(&second, false), (no_chord(), no_chord()));

    map.reset();
    assert_eq!(map, Keymap::default());
    for c in ["Ctrl+Shift+Z", "Ctrl+Y"] {
        assert_eq!(map.resolve(c, false, LOUPE), "Ctrl+Shift+Z", "{c}");
    }
    assert!(map.saved().is_empty());
}

/// While Redo is unmoved both its defaults are taken: an action on
/// either says Redo has it and Redo says the action does, but the
/// press is the action's, which the user put there; Redo keeps the
/// other default.
#[test]
fn an_action_on_either_of_redos_defaults_conflicts() {
    for c in ["Ctrl+Shift+Z", "Ctrl+Y"] {
        let mut map = Keymap::default();
        assert!(map.set("crop", c));
        let found = map.conflicts(false);
        let of = |id: &str| {
            found
                .iter()
                .find(|(a, _)| a.id == id)
                .map(|(_, o)| o.clone())
        };
        assert_eq!(of("crop"), Some(vec!["Redo"]), "{c}");
        assert_eq!(of("redo"), Some(vec!["Crop, and Done"]), "{c}");
        assert_eq!(found.len(), 2, "{c}");
        assert_eq!(map.resolve(c, false, LOUPE), "R", "{c}: the crop's");
        let other = if c == "Ctrl+Y" {
            "Ctrl+Shift+Z"
        } else {
            "Ctrl+Y"
        };
        assert_eq!(map.resolve(other, false, LOUPE), "Ctrl+Shift+Z", "{other}");
        let rows = binding_rows(&map, false);
        let redo = rows.iter().find(|r| r.id == "redo").unwrap();
        assert!(redo.warn);
        assert_eq!(redo.also, "Also: Crop, and Done");
    }
}

/// A 0.12 file that put Export on Ctrl+Y, which Redo's second default
/// then gave up to it: Ctrl+Y still exports, Ctrl+Shift+Z still
/// redoes.
#[test]
fn a_012_file_with_export_on_ctrl_y_keeps_exporting() {
    let saved: BTreeMap<String, String> = [("export".to_string(), "Ctrl+Y".to_string())].into();
    let map = Keymap::from_saved(&saved);
    for view in [LOUPE, GRID] {
        assert_eq!(map.resolve("Ctrl+Y", false, view), "Ctrl+Shift+E");
        assert_eq!(map.resolve("Ctrl+Shift+Z", false, view), "Ctrl+Shift+Z");
    }
}

/// The step moved onto Ctrl+Z: its Shift form, Ctrl+Shift+Z, steps
/// and adds in the Loupe as before Redo could move, over Redo's
/// default; the grid keeps Ctrl+Shift+Z for Redo.
#[test]
fn a_moved_steps_shift_form_beats_redos_default() {
    let mut map = Keymap::default();
    assert!(map.set("previous-frame", "Ctrl+Z"));
    assert_eq!(map.resolve("Ctrl+Z", false, LOUPE), "Left");
    assert_eq!(map.resolve("Ctrl+Shift+Z", false, LOUPE), "Shift+Left");
    assert_eq!(map.resolve("Ctrl+Y", false, LOUPE), "Ctrl+Shift+Z");
    assert_eq!(map.resolve("Ctrl+Shift+Z", false, GRID), "Ctrl+Shift+Z");
}

/// Settings from before Redo could be moved, with no line for it:
/// Redo is at its defaults, the rest as saved.
#[test]
fn settings_without_redo_leave_it_at_its_defaults() {
    let saved: BTreeMap<String, String> = [("crop".to_string(), "K".to_string())].into();
    let map = Keymap::from_saved(&saved);
    for c in ["Ctrl+Shift+Z", "Ctrl+Y"] {
        assert_eq!(map.resolve(c, false, LOUPE), "Ctrl+Shift+Z", "{c}");
    }
    assert_eq!(map.resolve("K", false, LOUPE), "R");
    assert_eq!(map.saved(), saved);
}

/// `key` pressed with Ctrl held, Shift too when `shift`.
fn with_ctrl(app: &App, key: &str, shift: bool) {
    let mods: &[Key] = if shift {
        &[Key::Control, Key::Shift]
    } else {
        &[Key::Control]
    };
    for m in mods {
        app.window()
            .dispatch_event(WindowEvent::KeyPressed { text: (*m).into() });
    }
    press(app, key);
    for m in mods.iter().rev() {
        app.window()
            .dispatch_event(WindowEvent::KeyReleased { text: (*m).into() });
    }
}

/// Through the window: Ctrl+Shift+Z and Ctrl+Y each redo a rating;
/// Redo bound in the Settings sheet to Ctrl+U, that alone does, and
/// Reset to defaults brings the two back.
#[test]
fn redo_bound_in_the_settings_sheet_is_what_the_window_goes_by() {
    let (app, state, _worker) = settings_window();
    app.set_settings_open(false);
    slint::platform::update_timers_and_animations();
    let rating = || state.borrow().sidecars[0].meta.rating;
    // A rating made and undone, then `redo` pressed: whether it redid.
    let redoes = |redo: &dyn Fn()| {
        app.invoke_meta_key("3".into());
        assert_eq!(rating(), 3);
        app.invoke_undo();
        assert_eq!(rating(), 0);
        redo();
        let did = rating() == 3;
        if !did {
            app.invoke_redo();
        }
        did
    };
    assert!(redoes(&|| with_ctrl(&app, "z", true)), "Ctrl+Shift+Z");
    assert!(redoes(&|| with_ctrl(&app, "y", false)), "Ctrl+Y");

    app.set_settings_open(true);
    row_of(&app, "redo").invoke_accessible_default_action();
    assert_eq!(value(&row_of(&app, "redo")), listening());
    with_ctrl(&app, "u", false);
    // The row says the chord in the platform's words, Cmd on a Mac.
    assert_eq!(
        value(&row_of(&app, "redo")),
        said("Ctrl+U", cfg!(target_os = "macos"))
    );
    press(&app, Key::Escape);
    assert!(!app.get_settings_open());
    slint::platform::update_timers_and_animations();
    assert!(redoes(&|| with_ctrl(&app, "u", false)), "Ctrl+U");
    assert!(
        !redoes(&|| with_ctrl(&app, "z", true)),
        "Ctrl+Shift+Z let go"
    );
    assert!(!redoes(&|| with_ctrl(&app, "y", false)), "Ctrl+Y let go");
    assert!(
        rows("", false)
            .iter()
            .any(|r| r.keys == "Ctrl+U" && r.does == crate::i18n::text("Redo")),
        "the keys sheet shows the binding"
    );

    app.invoke_keys_reset();
    assert!(redoes(&|| with_ctrl(&app, "z", true)), "Ctrl+Shift+Z back");
    assert!(redoes(&|| with_ctrl(&app, "y", false)), "Ctrl+Y back");
    assert!(!redoes(&|| with_ctrl(&app, "u", false)), "Ctrl+U nobody's");
}

/// Through the window: the Settings sheet's row listens for the
/// next press, which the window's keys then go by; Escape keeps
/// the old chord; Reset to defaults puts R back.
/// A frame open in Develop and the Settings sheet over it, the
/// window tall enough for the whole sheet: a row scrolled out of
/// the view is out of the accessible tree too.
fn settings_window() -> (
    App,
    std::rc::Rc<crate::Shared>,
    std::rc::Rc<crate::worker::Worker>,
) {
    let (app, state, worker) = develop_window();
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 4000.0));
    app.set_settings_category("Keys".into());
    app.set_settings_open(true);
    (app, state, worker)
}

/// The sheet's categories: it opens at the one last shown, with the
/// column holding the keys; Down and Up step it, held at the ends,
/// and Ctrl+Tab goes round; Escape there closes the sheet and the
/// keys are the window's again.
#[test]
fn the_settings_categories_step_by_the_keys() {
    let (app, _state, _worker) = develop_window();
    app.set_settings_category("Editing".into());
    app.invoke_settings_asked();
    assert_eq!(
        crate::testing::count_id(&app, "SettingsSheet::crop-heading"),
        1
    );
    assert_eq!(
        crate::testing::count_id(&app, "SettingsSheet::sidecars-heading"),
        0
    );
    press(&app, Key::DownArrow);
    assert_eq!(app.get_settings_category(), "Models");
    assert_eq!(
        crate::testing::count_id(&app, "SettingsSheet::crop-heading"),
        0
    );
    assert_eq!(
        crate::testing::count_id(&app, "SettingsSheet::models-heading"),
        1
    );
    press(&app, Key::DownArrow);
    assert_eq!(app.get_settings_category(), "Keys");
    assert_eq!(
        crate::testing::count_id(&app, "SettingsSheet::keys-heading"),
        1
    );
    press(&app, Key::DownArrow);
    press(&app, Key::DownArrow);
    assert_eq!(app.get_settings_category(), "About", "held at the foot");
    let ctrl = |key: Key| {
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        press(&app, key);
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
    };
    ctrl(Key::Tab);
    assert_eq!(app.get_settings_category(), "Interface", "round to the top");
    press(&app, Key::UpArrow);
    assert_eq!(app.get_settings_category(), "Interface", "held at the top");
    assert_eq!(
        crate::testing::count_id(&app, "SettingsSheet::interface-heading"),
        1
    );
    // A click on a name shows its category.
    let tab = category_tab(&app, "Library");
    tab.invoke_accessible_default_action();
    assert_eq!(app.get_settings_category(), "Library");
    assert_eq!(
        crate::testing::count_id(&app, "SettingsSheet::sidecars-heading"),
        1
    );
    press(&app, Key::Escape);
    assert!(!app.get_settings_open());
    slint::platform::update_timers_and_animations();
    press(&app, "g");
    assert!(app.get_grid_open(), "G is the window's");
    // Opened again, at the category left.
    app.set_grid_open(false);
    app.invoke_settings_asked();
    assert_eq!(app.get_settings_category(), "Library");
    assert_eq!(
        crate::testing::count_id(&app, "SettingsSheet::sidecars-heading"),
        1
    );
}

/// In the smallest window the fit allows, the sheet keeps Done in
/// the window and the keys table scrolls inside its category.
#[test]
fn the_keys_category_scrolls_in_a_small_window() {
    use i_slint_backend_testing::ElementHandle;
    let (app, _state, _worker) = develop_window();
    app.window()
        .set_size(slint::LogicalSize::new(1024.0, 640.0));
    app.set_settings_category("Keys".into());
    app.invoke_settings_asked();
    // Every Done there is, the sheet's among them, in the window.
    assert!(crate::testing::count_id(&app, "SettingsSheet::done") > 0);
    for done in ElementHandle::find_by_element_id(&app, "SettingsSheet::done") {
        let foot = done.absolute_position().y + done.size().height;
        assert!(foot <= 640.0, "Done at {foot}, past the window");
    }
    // The table runs past the view: its foot is scrolled out.
    assert_eq!(
        crate::testing::count_id(&app, "SettingsSheet::reset-keys-button"),
        0
    );
    assert!(crate::testing::count_keyed(&app, "SettingsSheet::bind-chip", "select-all") > 0);
}

/// An action's chord button, by the action's settings name, not the
/// words beside it, found afresh: a binding makes the rows again.
fn row_of(app: &App, action: &str) -> i_slint_backend_testing::ElementHandle {
    crate::testing::keyed(app, "SettingsSheet::bind-chip", action).expect("the action's row")
}

/// The Crop action's chord button.
fn row(app: &App) -> i_slint_backend_testing::ElementHandle {
    row_of(app, "crop")
}

/// The Settings sheet's tab of the category `category`.
fn category_tab(app: &App, category: &str) -> i_slint_backend_testing::ElementHandle {
    crate::testing::keyed(app, "SettingsSheet::category-tab", category)
        .unwrap_or_else(|| panic!("the {category} tab"))
}

fn shown(app: &App) -> String {
    row(app).accessible_value().unwrap_or_default().to_string()
}

fn repeated(app: &App, key: &str) {
    app.window()
        .dispatch_event(WindowEvent::KeyPressRepeated { text: key.into() });
}

#[test]
fn a_key_bound_in_the_settings_sheet_is_what_the_window_goes_by() {
    let (app, _state, _worker) = settings_window();
    assert_eq!(shown(&app), "R");
    row(&app).invoke_accessible_default_action();
    assert_eq!(shown(&app), listening());
    // A modifier alone waits; the chord with it is taken.
    shifted(&app, "k");
    assert_eq!(shown(&app), "Shift+K");
    assert!(app.get_settings_open());
    row(&app).invoke_accessible_default_action();
    press(&app, "k");
    assert_eq!(shown(&app), "K");
    // Escape while listening keeps K and the sheet; the next one
    // closes it, and the keys are the window's again.
    row(&app).invoke_accessible_default_action();
    press(&app, Key::Escape);
    assert_eq!(shown(&app), "K");
    assert!(app.get_settings_open());
    press(&app, Key::Escape);
    assert!(!app.get_settings_open());
    slint::platform::update_timers_and_animations();

    press(&app, "r");
    assert!(!app.get_crop_mode(), "R is nobody's now");
    press(&app, "k");
    assert!(app.get_crop_mode(), "K is the crop");
    press(&app, "k");
    assert!(!app.get_crop_mode());
    assert!(
        rows("", false)
            .iter()
            .any(|r| r.keys == "K" && r.does == crate::i18n::text("Crop, and Done")),
        "the keys sheet shows the binding"
    );
    // A modifier alone does not end the listening.
    let shift = SharedString::from(Key::Shift);
    assert!(!app.invoke_key_rebound("crop".into(), shift, false, true, false, false));

    app.invoke_keys_reset();
    press(&app, "k");
    assert!(!app.get_crop_mode());
    press(&app, "r");
    assert!(app.get_crop_mode());
}

/// A pair's chord buttons on its one row, the first's and the
/// second's, by the two actions' settings names: none for an action
/// the row does not have.
fn chips_of(app: &App, pair: [&str; 2]) -> Vec<i_slint_backend_testing::ElementHandle> {
    let first = crate::testing::keyed(app, "SettingsSheet::bind-chip", pair[0]);
    let second = crate::testing::keyed(app, "SettingsSheet::second-bind-chip", pair[1]);
    first.into_iter().chain(second).collect()
}

/// What a chord button says while it listens, and for no chord, in
/// the window's language.
fn listening() -> String {
    crate::tr!("Press a key")
}

fn no_chord() -> String {
    crate::tr!("key binding" => "None")
}

fn value(e: &i_slint_backend_testing::ElementHandle) -> String {
    e.accessible_value().unwrap_or_default().to_string()
}

/// The step's row holds both its chords; Backspace or Delete while
/// one listens binds it to no key and the row says None, Escape
/// keeps the chord, and Reset to defaults brings the arrows back.
#[test]
fn backspace_or_delete_binds_an_action_to_no_key() {
    let (app, _state, _worker) = settings_window();
    let step = ["previous-frame", "next-frame"];
    let chips = chips_of(&app, step);
    assert_eq!(chips.len(), 2, "one row, two chords");
    // On one row, the first's left of the second's.
    let (a, b) = (chips[0].absolute_position(), chips[1].absolute_position());
    assert!((a.y - b.y).abs() < 1.0 && a.x < b.x, "{a:?} {b:?}");
    assert_eq!(
        (value(&chips[0]), value(&chips[1])),
        ("←".into(), "→".into())
    );
    chips[0].invoke_accessible_default_action();
    assert_eq!(value(&chips_of(&app, step)[0]), listening());
    press(&app, Key::Backspace);
    assert_eq!(value(&chips_of(&app, step)[0]), no_chord());
    assert!(app.get_settings_open());
    // The second by Delete, after an Escape that kept it.
    chips_of(&app, step)[1].invoke_accessible_default_action();
    press(&app, Key::Escape);
    assert_eq!(value(&chips_of(&app, step)[1]), "→");
    chips_of(&app, step)[1].invoke_accessible_default_action();
    press(&app, Key::Delete);
    assert_eq!(value(&chips_of(&app, step)[1]), no_chord());
    // The second chip binds its own action to a chord.
    chips_of(&app, step)[1].invoke_accessible_default_action();
    press(&app, "k");
    assert_eq!(value(&chips_of(&app, step)[1]), "K");
    assert_eq!(value(&chips_of(&app, step)[0]), no_chord());
    assert_eq!(
        with_map(|m| m.saved()),
        BTreeMap::from([
            ("next-frame".to_string(), "K".to_string()),
            ("previous-frame".to_string(), String::new()),
        ])
    );
    app.invoke_keys_reset();
    let chips = chips_of(&app, step);
    assert_eq!(
        (value(&chips[0]), value(&chips[1])),
        ("←".into(), "→".into())
    );
}

/// The sheet closed while a row listens (Done, a click beside it,
/// or from Rust): the keys are the window's again.
#[test]
fn closing_the_sheet_while_a_row_listens_gives_the_keys_back() {
    let (app, _state, _worker) = settings_window();
    row(&app).invoke_accessible_default_action();
    assert_eq!(shown(&app), listening());
    app.set_settings_open(false);
    slint::platform::update_timers_and_animations();
    press(&app, "g");
    assert!(app.get_grid_open(), "G is the window's");
}

/// Reset to defaults clicked while a row listens: the listening
/// ends, and Escape closes the sheet.
#[test]
fn reset_while_a_row_listens_gives_the_keys_back() {
    let (app, _state, _worker) = settings_window();
    row(&app).invoke_accessible_default_action();
    press(&app, "k");
    assert_eq!(shown(&app), "K");
    row(&app).invoke_accessible_default_action();
    // The sheet's last button above Done, below the last row.
    crate::testing::click_id(&app, "SettingsSheet::reset-keys-button");
    assert_eq!(shown(&app), "R");
    press(&app, Key::DownArrow);
    assert_eq!(app.get_settings_category(), "About", "the column's again");
    press(&app, Key::Escape);
    assert!(!app.get_settings_open());
    slint::platform::update_timers_and_animations();
    press(&app, "g");
    assert!(app.get_grid_open());
}

/// The key being bound, held: its repeats are neither a second
/// chord nor the window's, until a fresh press.
#[test]
fn a_held_key_binds_once_and_does_nothing_more() {
    let (app, _state, _worker) = settings_window();
    // Open a folder, a key the window answers whatever is open.
    let asked = std::rc::Rc::new(std::cell::Cell::new(0));
    {
        let asked = asked.clone();
        app.on_open_folder(move || asked.set(asked.get() + 1));
    }
    let open_folder = |app: &App| {
        row_of(app, "open")
            .accessible_value()
            .unwrap_or_default()
            .to_string()
    };
    row_of(&app, "open").invoke_accessible_default_action();
    repeated(&app, "j");
    assert_eq!(open_folder(&app), listening(), "a repeat is no chord");
    app.window()
        .dispatch_event(WindowEvent::KeyPressed { text: "k".into() });
    assert_eq!(open_folder(&app), "K");
    // K still held as Escape closes the sheet: its repeats reach
    // the window, which knows them for the chord just bound.
    press(&app, Key::Escape);
    assert!(!app.get_settings_open());
    slint::platform::update_timers_and_animations();
    for _ in 0..3 {
        repeated(&app, "k");
    }
    assert_eq!(asked.get(), 0, "the repeats are nobody's");
    app.window()
        .dispatch_event(WindowEvent::KeyReleased { text: "k".into() });
    press(&app, "k");
    assert_eq!(asked.get(), 1, "a fresh press opens one");
}

/// A chord bound, or Escape while listening: the focus is the
/// column's again, so Up and Down step the categories and Ctrl+Tab
/// goes round; a repeat of the chord just bound steps nothing.
#[test]
fn the_column_has_the_keys_once_a_chord_is_bound() {
    let (app, _state, _worker) = settings_window();
    let ctrl = |key: Key| {
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        press(&app, key);
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
    };
    row(&app).invoke_accessible_default_action();
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::DownArrow.into(),
    });
    assert_eq!(shown(&app), "↓");
    for _ in 0..3 {
        repeated(&app, &SharedString::from(Key::DownArrow));
    }
    assert_eq!(
        app.get_settings_category(),
        "Keys",
        "the repeats step nothing"
    );
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::DownArrow.into(),
    });
    press(&app, Key::DownArrow);
    assert_eq!(app.get_settings_category(), "About");
    press(&app, Key::UpArrow);
    assert_eq!(app.get_settings_category(), "Keys");
    row(&app).invoke_accessible_default_action();
    press(&app, Key::Escape);
    assert_eq!(shown(&app), "↓");
    assert!(app.get_settings_open());
    ctrl(Key::Tab);
    assert_eq!(app.get_settings_category(), "About");
}

/// A click on the card's blank leaves a listening row listening,
/// and Ctrl+Tab from a category's rows steps the categories.
#[test]
fn a_click_on_the_card_keeps_the_listening() {
    let (app, _state, _worker) = settings_window();
    row(&app).invoke_accessible_default_action();
    // Below the column's last name, on the card.
    let about = category_tab(&app, "About");
    let (at, size) = (about.absolute_position(), about.size());
    crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height + 40.0);
    assert_eq!(shown(&app), listening());
    press(&app, "k");
    assert_eq!(shown(&app), "K");
    let ctrl = |key: Key, shift: bool| {
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        if shift {
            shifted(&app, &SharedString::from(key));
        } else {
            press(&app, key);
        }
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
    };
    // Tab from the column into the rows, then Ctrl+Shift+Tab.
    press(&app, Key::Tab);
    ctrl(Key::Tab, true);
    assert_eq!(app.get_settings_category(), "Models");
    // The focus left the hidden row for the column: the keys go on.
    ctrl(Key::Tab, false);
    assert_eq!(app.get_settings_category(), "Keys");
    press(&app, Key::Escape);
    assert!(!app.get_settings_open());
}

/// Escape in the crop puts down what is in hand first, one thing a
/// press: a typed value, the Draw, the level; then it leaves the
/// crop, as Done does. In the grid it is the grid's.
#[test]
fn escape_leaves_the_crop_once_nothing_is_in_hand() {
    let (app, _state, _worker) = develop_window();
    press(&app, "r");
    assert!(app.get_crop_mode());
    let (at, size) = crate::testing::slider_field(&app, "RotateSection::angle");
    crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    press(&app, Key::Escape);
    assert!(app.get_crop_mode(), "the typed value went first");
    app.set_crop_draw_armed(true);
    press(&app, Key::Escape);
    assert!(!app.get_crop_draw_armed());
    assert!(app.get_crop_mode(), "the Draw went next");
    app.set_level_mode(true);
    press(&app, Key::Escape);
    assert!(!app.get_level_mode());
    assert!(app.get_crop_mode(), "then the level");
    press(&app, Key::Escape);
    assert!(!app.get_crop_mode(), "then the crop");
    assert_eq!(app.get_panel_tab(), "Crop");
    // Out of the crop, Escape is nothing more here.
    press(&app, Key::Escape);
    assert!(!app.get_crop_mode());
    // The grid over a crop left on: its Escape, back to the loupe.
    press(&app, "r");
    app.set_grid_open(true);
    press(&app, Key::Escape);
    assert!(!app.get_grid_open());
    assert!(app.get_crop_mode(), "the grid's Escape");
}

/// A held Escape is one press: leaving the crop, its repeats do not
/// go on to stop an export or let go of a set.
#[test]
fn a_held_escape_leaves_the_crop_and_does_nothing_more() {
    use std::cell::Cell;
    use std::rc::Rc;
    let (app, _state, _worker) = develop_window();
    let (stops, collapses) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    {
        let stops = stops.clone();
        app.on_export_stop(move || stops.set(stops.get() + 1));
        let collapses = collapses.clone();
        app.on_set_collapsed(move || collapses.set(collapses.get() + 1));
    }
    press(&app, "r");
    app.set_export_running(true);
    app.set_set_count(2);
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
    for _ in 0..2 {
        app.window().dispatch_event(WindowEvent::KeyPressRepeated {
            text: Key::Escape.into(),
        });
    }
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Escape.into(),
    });
    assert!(!app.get_crop_mode());
    assert_eq!(stops.get(), 0, "the export runs on");
    assert_eq!(collapses.get(), 0, "the set is held");
    // A press of its own is the export's.
    press(&app, Key::Escape);
    assert_eq!(stops.get(), 1);
}

/// The custom aspect's box taken away under its focus, by a frame
/// with another aspect: the picture takes the keys at a crossing
/// again.
#[test]
fn a_custom_aspect_box_taken_away_hands_the_keys_back() {
    use crate::testing::{click, type_text};
    let (app, state, _worker) = develop_window();
    press(&app, "r");
    app.set_aspect_name("Custom".into());
    let (at, size) = crate::testing::by_id(&app, "CropSection::custom-aspect-field");
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert_eq!(app.global::<crate::Typing>().get_boxes(), 1);
    app.invoke_select(1);
    assert_ne!(app.get_aspect_name(), "Custom");
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(700.0, 400.0),
    });
    assert_eq!(app.global::<crate::Typing>().get_boxes(), 0);
    type_text(&app, "2");
    assert_eq!(state.borrow().sidecars[1].meta.rating, 2, "rated");
}

/// An aspect changed from the Rust side under another box (the
/// filter's words moving the frame, say) leaves that box the keys.
#[test]
fn an_aspect_change_leaves_another_box_its_keys() {
    use crate::testing::type_text;
    let (app, _state, _worker) = develop_window();
    app.set_aspect_name("1:1".into());
    app.set_panel_tab("Crop".into());
    press(&app, "/");
    slint::platform::update_timers_and_animations();
    type_text(&app, "a");
    app.set_aspect_name("Free".into());
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(0.0, 0.0),
    });
    assert_eq!(app.global::<crate::Typing>().get_boxes(), 1);
    type_text(&app, "b");
    assert_eq!(app.get_filter_text(), "ab");
}

/// Each of the panel's text boxes keeps the keys while the pointer
/// crosses the picture: a digit is typed, not a rating. Escape
/// gives them back to the window.
#[test]
fn a_text_box_keeps_the_keys_over_the_picture() {
    use crate::testing::{click, type_text};
    let (app, state, _worker) = develop_window();
    let rating = |state: &std::rc::Rc<crate::Shared>| state.borrow().sidecars[0].meta.rating;
    let over_the_picture = |app: &App| {
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(700.0, 400.0),
        });
    };
    let click_on = |app: &App, id: &str| {
        let (at, size) = crate::testing::by_id(app, id);
        click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    };
    let mut expected = 0;
    let mut keys_back = |app: &App, what: &str| {
        assert_eq!(rating(&state), expected, "{what}: typed, not rated");
        press(app, Key::Escape);
        expected = if expected == 3 { 4 } else { 3 };
        press(app, expected.to_string());
        assert_eq!(rating(&state), expected, "{what}: the keys are back");
    };

    // The title and the keywords, on the Develop tab.
    click_on(&app, "MetaSection::title-field");
    over_the_picture(&app);
    type_text(&app, "5");
    assert_eq!(app.get_title_text(), "5");
    keys_back(&app, "the title");
    click_on(&app, "MetaSection::keyword-field");
    over_the_picture(&app);
    type_text(&app, "5");
    assert_eq!(app.get_keyword_text(), "5");
    keys_back(&app, "the keywords");

    // A custom aspect, in the crop: its Escape gives the keys
    // back and leaves the crop on.
    press(&app, "r");
    app.set_aspect_name("Custom".into());
    click_on(&app, "CropSection::custom-aspect-field");
    over_the_picture(&app);
    type_text(&app, "5");
    assert!(app.get_aspect_custom().ends_with('5'));
    keys_back(&app, "the custom aspect");
    assert!(app.get_crop_mode(), "the box's Escape, not the crop's");
    press(&app, "r");

    // A mask's name.
    press(&app, "m");
    press(&app, Key::Escape);
    assert_eq!(app.get_placing(), "");
    click_on(&app, "AdjustmentsSection::name-field");
    over_the_picture(&app);
    type_text(&app, "5");
    assert!(app.get_adjustment_name().ends_with('5'));
    keys_back(&app, "the mask's name");

    // The filter's words, in the CULLING section beside the
    // picture.
    app.set_panel_tab(crate::panel::cull::CULL_TAB.into());
    press(&app, "/");
    slint::platform::update_timers_and_animations();
    over_the_picture(&app);
    type_text(&app, "5");
    assert_eq!(app.get_filter_text(), "5");
    keys_back(&app, "the filter");
}

/// The filter's words keep the keys while the pointer crosses the
/// picture; a press on the picture ends the typing, and the next
/// key is the window's.
#[test]
fn a_press_on_the_picture_ends_a_text_boxs_typing() {
    use crate::testing::{click, type_text};
    let (app, state, _worker) = develop_window();
    app.set_panel_tab(crate::panel::cull::CULL_TAB.into());
    press(&app, "/");
    slint::platform::update_timers_and_animations();
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(700.0, 400.0),
    });
    type_text(&app, "ab");
    assert_eq!(app.get_filter_text(), "ab", "the typing goes on");
    // Cleared, so the frame is in view again to be rated.
    press(&app, Key::Backspace);
    press(&app, Key::Backspace);
    assert_eq!(app.get_filter_text(), "");
    click(&app, 700.0, 400.0);
    assert_eq!(app.global::<crate::Typing>().get_boxes(), 0);
    press(&app, "2");
    assert_eq!(app.get_filter_text(), "");
    assert_eq!(state.borrow().sidecars[0].meta.rating, 2, "rated");
}

/// A click on Level or a guide's button hands the keys to the
/// window: Escape puts the tool down with the pointer still over
/// the panel.
#[test]
fn escape_after_a_click_on_level_or_a_guide_puts_it_down() {
    use crate::testing::click;
    let (app, _state, _worker) = develop_window();
    press(&app, "r");
    let click_on = |app: &App, id: &str| {
        let (at, size) = crate::testing::by_id(app, id);
        click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    };
    click_on(&app, "RotateSection::level-button");
    assert!(app.get_level_mode());
    press(&app, Key::Escape);
    assert!(!app.get_level_mode(), "the level put down");
    assert!(app.get_crop_mode());
    click_on(&app, "PerspectiveSection::verticals-button");
    assert_eq!(app.get_guide_mode(), "Vertical");
    press(&app, Key::Escape);
    assert_eq!(app.get_guide_mode(), "", "the guide put down");
    click_on(&app, "PerspectiveSection::horizontals-button");
    assert_eq!(app.get_guide_mode(), "Horizontal");
    press(&app, Key::Escape);
    assert_eq!(app.get_guide_mode(), "");
    click_on(&app, "CropSection::draw-button");
    assert!(app.get_crop_draw_armed());
    press(&app, Key::Escape);
    assert!(!app.get_crop_draw_armed(), "the Draw put down");
    assert!(app.get_crop_mode());
}

/// Ctrl+`key`, pressed and let go.
fn ctrl(app: &App, key: &str) {
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    press(app, key);
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
}

/// A click on a Reset leaves the pointer on the panel and the keys
/// with the button, which passes the ones it does not use to the
/// window: Ctrl+Z undoes the reset.
#[test]
fn ctrl_z_after_a_reset_button_undoes_the_reset() {
    use crate::testing::click;
    let (app, state, _worker) = develop_window();
    press(&app, "r");
    app.set_geo_angle(3.0);
    crate::panel::edit::record_panel(&mut state.borrow_mut(), &app);
    let (at, size) = crate::testing::by_id(&app, "PerspectiveSection::reset-all-button");
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert_eq!(app.get_geo_angle(), 0.0, "reset");
    ctrl(&app, "z");
    assert_eq!(app.get_geo_angle(), 3.0, "the reset undone");
}

/// A clicked button keeps Space and Return, which press it, and
/// Tab, Slint's move on; the rest are the window's, Escape too.
#[test]
fn a_clicked_button_keeps_its_own_keys_and_passes_the_rest() {
    use crate::testing::click;
    let (app, _state, _worker) = develop_window();
    let passed = || app.global::<crate::ButtonKeys>().get_passed();
    press(&app, "r");
    let (at, size) = crate::testing::by_id(&app, "RotateSection::flip-button");
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert!(app.get_geo_flip());
    press(&app, Key::Return);
    assert!(!app.get_geo_flip(), "Return presses it");
    press(&app, " ");
    assert!(app.get_geo_flip(), "Space presses it");
    let before = passed();
    press(&app, Key::Tab);
    assert_eq!(passed(), before, "Tab is Slint's");
    press(&app, Key::Escape);
    assert_eq!(passed(), before + 1);
    assert!(!app.get_crop_mode(), "Escape leaves the crop");
}

/// The import sheet has no keys of its own, the window's serve it:
/// Escape after a click on one of its buttons closes it.
#[test]
fn escape_after_a_click_on_the_import_sheet_closes_it() {
    use crate::testing::{by_id, click, settle};
    // No state, so Choose asks the desktop for nothing.
    let app = window(3);
    app.set_import_open(true);
    settle(&app);
    assert!(!app.global::<crate::ButtonKeys>().get_sheet_up());
    let (at, size) = by_id(&app, "FolderRow::choose-button");
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    press(&app, Key::Escape);
    assert_eq!(app.global::<crate::ButtonKeys>().get_passed(), 1);
    assert!(!app.get_import_open(), "closed by the window's Escape");
}

/// The keys a click on the match sheet now passes on are still
/// the window's under a sheet: no export sheet over it, by either
/// key.
#[test]
fn the_export_keys_are_nobodys_under_the_match_sheet() {
    use crate::testing::{click, settle};
    let (app, _state, _worker) = develop_window();
    // The no-op worker never lands the frame: done, as far as an
    // export asks.
    app.set_busy(false);
    let export_keys = |shift: bool| {
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        if shift {
            shifted(&app, "E");
        } else {
            press(&app, "e");
        }
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
    };
    // With no sheet up, each key brings the export's sheet.
    for shift in [true, false] {
        export_keys(shift);
        assert!(app.get_export_open(), "shift {shift}");
        app.set_export_open(false);
        settle(&app);
    }
    app.set_match_open(true);
    settle(&app);
    // On its label: a Toggle's name is the whole row's, whose
    // empty run past the label takes no click.
    let (at, size) = crate::testing::toggle_by_id(&app, "MatchSheet::replace-toggle").label;
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    let passed = app.global::<crate::ButtonKeys>().get_passed();
    for shift in [true, false] {
        export_keys(shift);
        assert!(!app.get_export_open(), "no export sheet over the match");
    }
    assert!(
        app.global::<crate::ButtonKeys>().get_passed() > passed,
        "passed on"
    );
    assert!(app.get_match_open());
}

/// Under a sheet with keys of its own, a clicked button's keys go
/// up to the sheet's, never to the window under it.
#[test]
fn a_button_on_a_sheet_keeps_the_keys_from_the_window() {
    use crate::testing::{by_id, click, settle};
    // No state, so the check asks nothing of the network.
    let app = window(3);
    app.set_settings_category("About".into());
    app.set_settings_open(true);
    settle(&app);
    assert!(app.global::<crate::ButtonKeys>().get_sheet_up());
    let (at, size) = by_id(&app, "SettingsSheet::check-now-button");
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    press(&app, "g");
    assert!(!app.get_grid_open(), "not the window's G");
    assert_eq!(app.global::<crate::ButtonKeys>().get_passed(), 0);
    // The button has the keys: with the sheet's flag down, the
    // same key would have gone on.
    app.global::<crate::ButtonKeys>().set_sheet_up(false);
    press(&app, "g");
    assert_eq!(app.global::<crate::ButtonKeys>().get_passed(), 1);
    app.global::<crate::ButtonKeys>().set_sheet_up(true);
    press(&app, Key::Escape);
    assert!(!app.get_settings_open(), "the sheet's own Escape");
    assert_eq!(app.global::<crate::ButtonKeys>().get_passed(), 1);
    settle(&app);
    assert!(!app.global::<crate::ButtonKeys>().get_sheet_up());
}

/// `key` pressed with Alt held.
fn alt(app: &App, key: impl Into<SharedString>) {
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Alt.into(),
    });
    press(app, key);
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Alt.into(),
    });
}

/// A window whose way back and forward is counted, not taken.
fn visits_of(app: &App) -> Rc<RefCell<Vec<bool>>> {
    let visits = Rc::new(RefCell::new(Vec::new()));
    let asked = visits.clone();
    app.on_visit_folder(move |back| asked.borrow_mut().push(back));
    visits
}

/// Alt+← and Alt+→ are the mouse's side buttons: in the grid and
/// the lightbox, once for a held key, and nobody's in the Loupe,
/// under a sheet, or in the filter's words. The plain arrows still
/// step the frames.
#[test]
fn alt_and_the_arrows_go_back_and_forward_in_the_grid() {
    let (app, _state, _worker) = develop_window();
    let visits = visits_of(&app);
    let left = SharedString::from(Key::LeftArrow);
    let right = SharedString::from(Key::RightArrow);
    alt(&app, left.clone());
    assert!(visits.borrow().is_empty(), "not in the Loupe");

    press(&app, "g");
    assert!(app.get_grid_open());
    let selected = app.get_selected();
    alt(&app, left.clone());
    alt(&app, right.clone());
    assert_eq!(*visits.borrow(), [true, false]);
    assert_eq!(app.get_selected(), selected, "no frame stepped");
    press(&app, right.clone());
    assert_eq!(app.get_selected(), selected + 1, "the arrow steps");

    // Held: once.
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Alt.into(),
    });
    app.window()
        .dispatch_event(WindowEvent::KeyPressed { text: left.clone() });
    app.window()
        .dispatch_event(WindowEvent::KeyPressRepeated { text: left.clone() });
    app.window()
        .dispatch_event(WindowEvent::KeyReleased { text: left.clone() });
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Alt.into(),
    });
    assert_eq!(visits.borrow().len(), 3);

    press(&app, "l");
    assert!(app.get_lightbox());
    alt(&app, right.clone());
    assert_eq!(visits.borrow().last(), Some(&false), "the lightbox's too");
    press(&app, "l");

    app.set_keys_open(true);
    alt(&app, left.clone());
    assert_eq!(visits.borrow().len(), 4, "nobody's under a sheet");
    press(&app, Key::Escape);
    slint::platform::update_timers_and_animations();

    // The filter's words have the keys first.
    press(&app, "/");
    slint::platform::update_timers_and_animations();
    alt(&app, left.clone());
    assert_eq!(visits.borrow().len(), 4, "the field's");
}

/// After a click on a button the keys stay with it, which passes
/// them on: G and then Alt+← still go back.
#[test]
fn alt_left_after_a_clicked_button_goes_back() {
    use crate::testing::click;
    let (app, _state, _worker) = develop_window();
    let visits = visits_of(&app);
    press(&app, "r");
    let (at, size) = crate::testing::by_id(&app, "RotateSection::flip-button");
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    let passed = app.global::<crate::ButtonKeys>().get_passed();
    press(&app, "g");
    assert!(app.get_grid_open());
    alt(&app, Key::LeftArrow);
    assert_eq!(*visits.borrow(), [true]);
    assert!(app.global::<crate::ButtonKeys>().get_passed() >= passed + 2);
}

/// The way back moved to another key goes by it, and Alt+← is then
/// nobody's.
#[test]
fn the_way_back_rebound_goes_by_its_new_key() {
    let mut map = Keymap::default();
    assert!(map.set("visit-back", "Q"));
    assert_eq!(map.resolve("Q", false, GRID), "Alt+Left");
    assert_eq!(map.resolve("Alt+Left", false, GRID), "");
    assert_eq!(map.resolve("Alt+Right", false, GRID), "Alt+Right");
    let (app, _state, _worker) = develop_window();
    let visits = visits_of(&app);
    KEYMAP.with(|m| *m.borrow_mut() = map);
    press(&app, "g");
    alt(&app, Key::LeftArrow);
    assert!(visits.borrow().is_empty());
    press(&app, "q");
    assert_eq!(*visits.borrow(), [true]);
}

#[test]
fn the_guide_is_this_releases() {
    let url = guide_url();
    assert!(
        url.starts_with("https://github.com/jessolmstead/greycard/blob/v"),
        "{url}"
    );
    assert!(url.ends_with("/docs/user-guide.md"), "{url}");
    assert!(url.contains(env!("CARGO_PKG_VERSION")));
}

/// The rows' words wrap, and the list is measured for its pane: the
/// last row on screen in a tall window, and in a short one the list
/// scrolls to it.
#[test]
fn the_keys_sheet_keeps_its_last_row_under_wrapped_rows() {
    let app = window(0);
    let mut rows = vec![KeyRow {
        heading: true,
        keys: "Viewing".into(),
        does: "".into(),
    }];
    let does = "shows the frame at its own size, centered on the point under \
                the pointer, and back to the fit on a second press";
    rows.extend((0..12).map(|i| KeyRow {
        heading: false,
        keys: format!("Ctrl+Shift+{i}").into(),
        does: format!("Row {i}: {does}").into(),
    }));
    // Opened, the sheet fills with the table's rows; these in their
    // place.
    app.set_keys_open(true);
    crate::testing::settle(&app);
    app.set_keys_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    crate::testing::keeps_its_last_line(&app, "Row 11: shows", "KeysSheet::scroll", 1400.0);
}

/// Shift+J paints focus peaking over the Loupe's picture: on a frame
/// with a sharp half and a blurred one, the sharp half and not the
/// blurred; again, nothing. The choice outlives a restart.
#[test]
fn shift_j_paints_the_sharp_half_and_is_kept_across_a_restart() {
    use crate::panel::startup::{peaking_of, remember, show_overlays};
    use crate::render::{Renderer, View};
    let app = window(3);
    assert!(!app.get_grid_open(), "in the Loupe");
    assert_eq!(peaking_of(&app), None, "off by default");
    shifted(&app, "J");
    assert!(app.get_peaking());
    // The clipping paint's key is not it.
    assert!(!app.get_warn_shadows() && !app.get_warn_highlights());

    // The frame, drawn as the Loupe draws it, peaking as the
    // window has it now.
    let shot = |app: &App| -> Option<(usize, usize)> {
        let (device, queue) = greycard_viewport::testing::device("shift_j_paints_the_sharp_half")?;
        let (w, h) = (192usize, 128usize);
        let data = (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                let v = if x < w / 2 {
                    if (x / 2 + y / 2) % 2 == 0 { 0.06 } else { 0.45 }
                } else {
                    0.2
                };
                [v, v, v]
            })
            .collect();
        let image = greycard_core::image::WorkingImage {
            width: w,
            height: h,
            data,
        };
        let mut renderer = Renderer::new(&device, &queue);
        renderer.upload(&greycard_render::halves::Halves::from_image(&image, None));
        let mut view = View::blank();
        view.center = (w as f32 / 2.0, h as f32 / 2.0);
        view.plane = (w as f32, h as f32);
        view.frame_size = (w as f32, h as f32);
        view.peaking = peaking_of(app);
        let target = renderer.render(w as u32, h as u32, &view).0;
        let shown = renderer.read_back(&target).expect("read back");
        let paint = [0x3c, 0xff, 0x28, 0xff];
        let painted = |left: bool| {
            shown
                .enumerate_pixels()
                .filter(|(x, _, p)| {
                    let x = *x as usize;
                    (if left { x + 4 < w / 2 } else { x > w / 2 + 4 }) && p.0 == paint
                })
                .count()
        };
        Some((painted(true), painted(false)))
    };
    if let Some((sharp, blurred)) = shot(&app) {
        assert!(sharp > 1000, "the sharp half: {sharp}");
        assert_eq!(blurred, 0, "the blurred half");
    }

    // Kept: the settings a run leaves, written and read back, put
    // on a new window as the start puts them.
    let kept = serde_json::to_string(&remember(&app)).unwrap();
    let restarted = window(3);
    show_overlays(&restarted, false, &serde_json::from_str(&kept).unwrap());
    assert!(restarted.get_peaking(), "kept across a restart");

    shifted(&app, "J");
    assert!(!app.get_peaking());
    if let Some((sharp, blurred)) = shot(&app) {
        assert_eq!((sharp, blurred), (0, 0), "a second press clears it");
    }
}

/// A wheel event at the middle of the view, Ctrl held or not.
fn scroll(app: &crate::App, delta_y: f32, ctrl: bool) {
    let key = |down: bool| {
        let text = Key::Control.into();
        app.window().dispatch_event(if down {
            WindowEvent::KeyPressed { text }
        } else {
            WindowEvent::KeyReleased { text }
        });
    };
    if ctrl {
        key(true);
    }
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: slint::LogicalPosition::new(600.0, 400.0),
        delta_x: 0.0,
        delta_y,
    });
    if ctrl {
        key(false);
    }
}

/// A window with a 6000 by 4000 picture at 1:1, nothing open.
fn at_one_to_one() -> (crate::App, Rc<crate::Shared>, Rc<crate::worker::Worker>) {
    let app = window(1);
    let (state, worker) = crate::testing::retouch_state(&app);
    {
        let mut st = state.borrow_mut();
        st.develop.image_size = (6000, 4000);
        st.develop.center = (3000.0, 2000.0);
        st.develop.zoom = 1.0;
    }
    (app, state, worker)
}

/// Ctrl and the wheel zoom with the brush or a repair in hand, as
/// they do with nothing; the plain wheel still sizes the tool.
#[test]
fn ctrl_and_the_wheel_zoom_with_a_tool_in_hand() {
    let (app, state, _worker) = at_one_to_one();
    let zoom = || state.borrow().develop.zoom;
    app.set_brush_size(0.05);
    app.set_placing("Brush".into());
    scroll(&app, 60.0, true);
    assert!((zoom() - 1.25).abs() < 1e-4, "zoomed to {}", zoom());
    assert_eq!(app.get_brush_size(), 0.05, "the brush kept its size");
    scroll(&app, 60.0, false);
    assert!((zoom() - 1.25).abs() < 1e-4, "the plain wheel zoomed");
    assert_ne!(app.get_brush_size(), 0.05, "the plain wheel sized nothing");
    assert_eq!(app.get_placing(), "Brush");
    app.set_placing("".into());

    app.invoke_retouch_toggled("Heal".into());
    let size = app.get_patch_size();
    scroll(&app, -60.0, true);
    assert!((zoom() - 1.0).abs() < 1e-4, "zoomed to {}", zoom());
    assert_eq!(app.get_patch_size(), size);
    assert_eq!(app.get_retouch_mode(), "Heal");
}

/// Plus and minus zoom the Loupe a step, the tool left in hand;
/// over the grid they size its tiles as before.
#[test]
fn plus_and_minus_zoom_the_loupe_and_size_the_grid() {
    let (app, state, _worker) = at_one_to_one();
    let zoom = || state.borrow().develop.zoom;
    app.set_placing("Brush".into());
    press(&app, "+");
    assert!((zoom() - 1.25).abs() < 1e-4, "zoomed to {}", zoom());
    press(&app, "-");
    assert!((zoom() - 1.0).abs() < 1e-4, "zoomed to {}", zoom());
    assert_eq!(app.get_placing(), "Brush", "the tool stays in hand");
    app.set_placing("".into());

    let steps = Rc::new(std::cell::Cell::new(0));
    let counted = steps.clone();
    app.on_grid_zoom(move |by| counted.set(counted.get() + by));
    app.set_grid_open(true);
    press(&app, "+");
    press(&app, "+");
    press(&app, "-");
    assert_eq!(steps.get(), 1, "the grid's tiles");
    assert!((zoom() - 1.0).abs() < 1e-4, "the grid zoomed the Loupe");
}

/// Tab, Shift+Tab, and the arrows `rounds` times over under an
/// open sheet, as a hand walking the sheet would: the edit, the
/// frame, its history, and the panes behind stay as they were, and
/// the sheet stays open. With `reopen`, Escape closes the sheet
/// wherever the walk has the focus, at every step: the sheet is
/// opened again by it and walked back to the same step.
fn walk_holds(
    app: &App,
    state: &crate::Shared,
    open: impl Fn(&App) -> bool,
    reopen: Option<fn(&App, bool)>,
    back: bool,
    rounds: usize,
) {
    let tab = |app: &App| {
        if back {
            let tab: slint::SharedString = Key::Tab.into();
            shifted(app, &tab);
        } else {
            press(app, Key::Tab);
        }
    };
    let look = crate::panel::edit::read_look(app);
    let current = state.borrow().current;
    let states = state.borrow().sidecars[0].states();
    let panes = (
        app.get_left_hidden(),
        app.get_right_hidden(),
        app.get_strip_hidden(),
    );
    for i in 0..rounds {
        tab(app);
        for key in [
            Key::UpArrow,
            Key::RightArrow,
            Key::DownArrow,
            Key::LeftArrow,
        ] {
            press(app, key);
        }
        assert!(open(app), "the sheet closed at Tab {i}");
        assert_eq!(
            crate::panel::edit::read_look(app),
            look,
            "the edit changed at Tab {i}"
        );
        assert_eq!(
            state.borrow().current,
            current,
            "the frame moved at Tab {i}"
        );
        assert_eq!(
            (
                app.get_left_hidden(),
                app.get_right_hidden(),
                app.get_strip_hidden()
            ),
            panes,
            "the panes moved at Tab {i}"
        );
        slint::platform::update_timers_and_animations();
        if let Some(reopen) = reopen {
            // Afresh, with Tab alone to the same step, so no arrow
            // has opened a list or a question that Escape answers
            // first; then again, to go on.
            let walk_to = |app: &App| {
                reopen(app, false);
                slint::platform::update_timers_and_animations();
                reopen(app, true);
                slint::platform::update_timers_and_animations();
                for _ in 0..=i {
                    tab(app);
                }
            };
            walk_to(app);
            press(app, Key::Escape);
            slint::platform::update_timers_and_animations();
            // A slider lets go of the keys at the first, as a field
            // does, and the sheet closes at the second.
            if open(app) {
                press(app, Key::Escape);
                slint::platform::update_timers_and_animations();
            }
            assert!(!open(app), "two Escapes left the sheet open at Tab {i}");
            walk_to(app);
        }
    }
    // Past the panel's rest, so a change would have been saved.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(2));
    slint::platform::update_timers_and_animations();
    assert_eq!(state.borrow().sidecars[0].states(), states, "history");
}

/// Every sheet holds the focus as Settings does, both ways round:
/// those that take the focus as they open and those the window's
/// keys serve (import, Lightroom, RawTherapee, match, model, and
/// backup), whose Tab the keys send into the sheet. Each opens
/// over a panel slider holding the focus, as a click leaves it.
#[test]
fn every_sheet_holds_the_focus() {
    macro_rules! sheets {
        ($($name:literal: $set:ident, $get:ident;)*) => {
            [$(($name, App::$set as fn(&App, bool), App::$get as fn(&App) -> bool)),*]
        };
    }
    let sheets = sheets! {
        "model": set_fetch_open, get_fetch_open;
        "rejects": set_rejects_open, get_rejects_open;
        "delete": set_delete_open, get_delete_open;
        "look remove": set_look_remove_open, get_look_remove_open;
        "look rename": set_look_rename_open, get_look_rename_open;
        "time shift": set_time_shift_open, get_time_shift_open;
        "overwrite": set_overwrite_open, get_overwrite_open;
        "backup": set_archive_open, get_archive_open;
        "archive rejects": set_archive_rejects_open, get_archive_rejects_open;
        "match": set_match_open, get_match_open;
        "preset": set_preset_open, get_preset_open;
        "preset report": set_preset_report_open, get_preset_report_open;
        "Lightroom": set_lr_open, get_lr_open;
        "RawTherapee": set_rt_open, get_rt_open;
        "collection": set_collection_sheet_open, get_collection_sheet_open;
        "preset group": set_preset_group_sheet_open, get_preset_group_sheet_open;
        "root": set_root_sheet_open, get_root_sheet_open;
        "sync": set_sync_open, get_sync_open;
        "export": set_export_open, get_export_open;
        "import": set_import_open, get_import_open;
        "settings": set_settings_open, get_settings_open;
        "keys": set_keys_open, get_keys_open;
    };
    for (name, set, get) in sheets {
        for back in [false, true] {
            let (app, state, _worker) = develop_window();
            let (x, y) = crate::testing::track_middle(&app, "LightSection::exposure");
            crate::testing::click(&app, x, y);
            set(&app, true);
            crate::testing::settle(&app);
            assert!(get(&app), "{name} opened");
            let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                walk_holds(&app, &state, get, Some(set), back, 8)
            }));
            if let Err(e) = held {
                let said = e.downcast_ref::<String>().cloned().unwrap_or_default();
                panic!("the {name} sheet, back {back}: {said}");
            }
            set(&app, false);
            crate::testing::settle(&app);
            press(&app, "g");
            assert!(app.get_grid_open(), "the keys the window's after {name}");
        }
    }
}

/// The Settings sheet holds the focus: Tab and Shift+Tab go round
/// its controls and never out onto the panel behind, where the
/// arrows would move a slider (§658's finding). Escape still
/// closes it, and the window has its keys again.
#[test]
fn the_settings_sheet_holds_the_focus() {
    for back in [false, true] {
        let (app, state, _worker) = develop_window();
        app.set_settings_category("Editing".into());
        app.invoke_settings_asked();
        crate::testing::settle(&app);
        walk_holds(&app, &state, |a| a.get_settings_open(), None, back, 80);
        press(&app, Key::Escape);
        assert!(!app.get_settings_open());
        slint::platform::update_timers_and_animations();
        press(&app, "g");
        assert!(app.get_grid_open(), "G is the window's");
    }
}

/// A sheet closed from Rust over another, with the focus on its
/// control: the keys go to the window, so Escape closes the sheet
/// beneath, after one Tab into the top sheet or two.
#[test]
fn a_sheet_closed_over_another_leaves_the_keys_to_the_window() {
    type Pair = (
        &'static str,
        fn(&App, bool),
        fn(&App) -> bool,
        fn(&App, bool),
    );
    let pairs: [Pair; 4] = [
        (
            "settings, model",
            App::set_settings_open,
            App::get_settings_open,
            App::set_fetch_open,
        ),
        (
            "settings, keys",
            App::set_settings_open,
            App::get_settings_open,
            App::set_keys_open,
        ),
        (
            "export, overwrite",
            App::set_export_open,
            App::get_export_open,
            App::set_overwrite_open,
        ),
        (
            "import, model",
            App::set_import_open,
            App::get_import_open,
            App::set_fetch_open,
        ),
    ];
    for (name, below, below_open, top) in pairs {
        for tabs in 1..=2 {
            let (app, _state, _worker) = develop_window();
            below(&app, true);
            crate::testing::settle(&app);
            top(&app, true);
            crate::testing::settle(&app);
            for _ in 0..tabs {
                press(&app, Key::Tab);
            }
            top(&app, false);
            crate::testing::settle(&app);
            assert!(below_open(&app), "{name}: the sheet beneath stays");
            press(&app, Key::Escape);
            crate::testing::settle(&app);
            assert!(!below_open(&app), "{name}, {tabs} Tabs: Escape closed it");
        }
    }
}

/// A sheet with no control the focus can take (the Lightroom
/// sheet's every button off while its import runs): Tab goes round
/// to nothing and leaves the keys with the window, whose Escape
/// serves the sheet once the import lets it.
#[test]
fn a_sheet_with_no_control_leaves_the_keys_to_the_window() {
    for back in [false, true] {
        for tabs in 1..=4 {
            let (app, _state, _worker) = develop_window();
            let cancels = Rc::new(std::cell::Cell::new(0));
            let counted = cancels.clone();
            app.on_lr_cancel(move || counted.set(counted.get() + 1));
            app.set_lr_step(1);
            app.set_lr_busy(true);
            app.set_lr_running(true);
            app.set_lr_open(true);
            crate::testing::settle(&app);
            for _ in 0..tabs {
                if back {
                    let tab: slint::SharedString = Key::Tab.into();
                    shifted(&app, &tab);
                } else {
                    press(&app, Key::Tab);
                }
            }
            // The import over, its buttons on again, untouched.
            app.set_lr_running(false);
            app.set_lr_busy(false);
            crate::testing::settle(&app);
            press(&app, Key::Escape);
            crate::testing::settle(&app);
            assert_eq!(
                cancels.get(),
                1,
                "back {back}, {tabs} Tabs: Escape reached the sheet"
            );
        }
    }
}
