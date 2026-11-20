//! The keys through a sheet's checks and choices: on each sheet with a
//! carry list, Tab and Shift+Tab go round the sync sheet's Set/Shift
//! choices, Check all, Uncheck all, every check, the masks' choice,
//! and the sheet's buttons in the order they are drawn, each showing
//! the focus ring as it has the keys, each brought into the list's
//! view; Space turns the focused check over, Left and Right move a
//! focused choice; Enter and Escape are the sheet's. The match sheet's
//! scope and group checks likewise. What is focused is read off the
//! drawn window, as a hand sees it: the control whose ring is drawn.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use i_slint_backend_testing::ElementHandle;
use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, Model, ModelRc, SharedPixelBuffer, VecModel};

use crate::testing::{click, drawing_backend, folder, press, settle, state_for, window};
use crate::{App, MatchGroup};

/// Where a control draws its ring: just outside it (a check, a
/// primary button), as its own edge (a plain button), or anywhere in
/// it (a choice, round its current option).
#[derive(Clone, Copy)]
enum Ring {
    Outside,
    Edge,
    Within,
}

/// One stop of the walk: its name, its box in the window, and where
/// its ring goes. A check's box is its box, not its row.
struct Stop {
    name: String,
    at: (f32, f32, f32, f32),
    ring: Ring,
    /// In a scrolling list, drawn only inside its view.
    in_list: bool,
}

/// The sheet under test.
#[derive(Clone, Copy, Debug)]
enum Sheet {
    Sync,
    Preset,
    Match,
}

/// The sync sheet's Set/Shift choices, one name each, in order.
const SHIFTS: [&str; 3] = [
    "ShiftChoice::choice 0",
    "ShiftChoice::choice 1",
    "ShiftChoice::choice 2",
];

impl Sheet {
    /// Its own buttons, by id, in the order they are drawn; the match
    /// sheet's are not read.
    fn buttons(self) -> Vec<(&'static str, Ring)> {
        match self {
            Sheet::Sync => vec![
                ("SyncSheet::cancel", Ring::Edge),
                ("SyncSheet::go", Ring::Outside),
            ],
            Sheet::Preset => vec![
                ("PresetSheet::cancel", Ring::Edge),
                ("PresetSheet::save", Ring::Outside),
            ],
            Sheet::Match => Vec::new(),
        }
    }

    fn rows(self, app: &App) -> ModelRc<crate::CarryRow> {
        match self {
            Sheet::Sync => app.get_sync_rows(),
            Sheet::Preset => app.get_preset_rows(),
            Sheet::Match => unreachable!("the match sheet has no carry list"),
        }
    }

    fn is_open(self, app: &App) -> bool {
        match self {
            Sheet::Sync => app.get_sync_open(),
            Sheet::Preset => app.get_preset_open(),
            Sheet::Match => app.get_match_open(),
        }
    }

    /// Tabs past the sheet's last stop before its first again: the
    /// preset sheet's name field, whose ring we do not read.
    fn between(self) -> usize {
        match self {
            Sheet::Sync | Sheet::Match => 0,
            Sheet::Preset => 1,
        }
    }

    /// The scrolling list's id.
    fn list(self) -> &'static str {
        match self {
            Sheet::Sync | Sheet::Preset => "CarryList::pane",
            Sheet::Match => "MatchSheet::groups-scroll",
        }
    }
}

/// A sheet open, with what its window's callbacks hold, and the count
/// of Enter's (or a match group's toggle).
struct Opened {
    app: App,
    entered: Rc<Cell<u32>>,
    _state: Rc<crate::Shared>,
    _worker: Rc<crate::worker::Worker>,
}

thread_local! {
    static DRAWING: Cell<bool> = const { Cell::new(false) };
}

/// The match sheet's groups.
const GROUPS: [&str; 3] = ["Body A Standard", "Body B Neutral", "Body C Faithful"];

/// A window `height` tall with the sheet open: a carry list over two
/// masks besides the sections, every check on, the sync sheet with its
/// Set/Shift choices; the match sheet with two scopes and three
/// groups, all checked.
fn open(sheet: Sheet, height: f32) -> Opened {
    if !DRAWING.with(|d| d.replace(true)) {
        drawing_backend();
    }
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(900.0, height));
    let (state, worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    let masks = vec![(1, "Mask 1".to_string()), (2, "Mask 2".to_string())];
    {
        let mut st = state.borrow_mut();
        let checks = super::Checks::new(masks, |_| true);
        match sheet {
            Sheet::Sync => {
                st.sheets.sync_checks = checks;
                st.sheets.sync_source = Some((greycard_edit::Edit::default(), None));
                crate::panel::sync::show_sync_checks(&mut st, &app);
            }
            Sheet::Preset => {
                st.sheets.preset_checks = checks;
                crate::panel::assets::show_preset_checks(&mut st, &app);
            }
            Sheet::Match => {}
        }
    }
    let entered = Rc::new(Cell::new(0));
    let counted = entered.clone();
    match sheet {
        Sheet::Sync => {
            app.on_sync_applied(move || counted.set(counted.get() + 1));
            app.set_sync_open(true);
        }
        Sheet::Preset => {
            app.on_preset_saved(move || counted.set(counted.get() + 1));
            app.set_preset_name("Walk".into());
            app.set_preset_open(true);
        }
        Sheet::Match => {
            let groups: Rc<VecModel<MatchGroup>> = Rc::new(VecModel::from(
                GROUPS
                    .iter()
                    .map(|g| MatchGroup {
                        name: (*g).into(),
                        text: format!("{g}: 40 frames").into(),
                        checked: true,
                    })
                    .collect::<Vec<_>>(),
            ));
            app.set_match_groups(ModelRc::from(groups.clone()));
            app.set_match_scopes(ModelRc::new(VecModel::from(vec![
                slint::SharedString::from("This folder"),
                "Library".into(),
            ])));
            app.set_match_scope("This folder".into());
            app.on_match_scope_picked(|_| {});
            app.on_match_group_toggled(move |i| {
                counted.set(counted.get() + 1);
                let mut g = groups.row_data(i as usize).unwrap();
                g.checked = !g.checked;
                groups.set_row_data(i as usize, g);
            });
            app.set_match_open(true);
        }
    }
    settle(&app);
    Opened {
        app,
        entered,
        _state: state,
        _worker: worker,
    }
}

fn bounds(e: &ElementHandle) -> (f32, f32, f32, f32) {
    let (p, s) = (e.absolute_position(), e.size());
    (p.x, p.y, s.width, s.height)
}

fn one(app: &App, id: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(app, id)
        .next()
        .unwrap_or_else(|| panic!("no {id}"))
}

/// The walk's stops by name, in the order drawn: the sync sheet's
/// Set/Shift choices, Check all, Uncheck all, each check as the rows
/// list them (a column top to bottom, a heading then its parts, the
/// left one of a line first) and the masks' choice after the masks,
/// then the sheet's buttons. On the match sheet, its scope and each
/// group's check.
fn walk(app: &App, sheet: Sheet) -> Vec<String> {
    if let Sheet::Match = sheet {
        let mut names = vec!["MatchSheet::scope".to_string()];
        names.extend(GROUPS.iter().map(|g| format!("Fit {g}")));
        return names;
    }
    let mut names = Vec::new();
    if let Sheet::Sync = sheet {
        assert!(app.get_sync_shift_shown());
        names.extend(SHIFTS.iter().map(|s| s.to_string()));
    }
    names.push("CarryList::check-all".to_string());
    names.push("CarryList::uncheck-all".to_string());
    names.extend(sheet.rows(app).iter().map(|r| {
        if r.kind == 2 {
            "CarryList::join".to_string()
        } else {
            r.a11y.to_string()
        }
    }));
    names.extend(sheet.buttons().iter().map(|(id, _)| id.to_string()));
    names
}

/// The checks with `id` drawn now, each by its name and its box.
fn checks(app: &App, id: &str) -> Vec<(String, (f32, f32, f32, f32))> {
    let boxes: Vec<ElementHandle> = ElementHandle::find_by_element_id(app, "Check::box").collect();
    ElementHandle::find_by_element_id(app, id)
        .filter_map(|check| {
            let name = check.accessible_label().unwrap_or_default().to_string();
            // Its box: the one inside it.
            let (cx, cy, cw, ch) = bounds(&check);
            boxes
                .iter()
                .map(bounds)
                .find(|&(x, y, w, h)| x >= cx && y >= cy && x + w <= cx + cw && y + h <= cy + ch)
                .map(|at| (name, at))
        })
        .collect()
}

/// The stops drawn now, one in a list only while some of it is in the
/// list's view (Slint draws no more of the list).
fn stops(app: &App, sheet: Sheet) -> Vec<Stop> {
    let stop = |name: String, at, ring, in_list| Stop {
        name,
        at,
        ring,
        in_list,
    };
    let mut stops = Vec::new();
    if let Sheet::Match = sheet {
        stops.push(stop(
            "MatchSheet::scope".into(),
            bounds(&one(app, "MatchSheet::scope")),
            Ring::Within,
            false,
        ));
        for (name, at) in checks(app, "MatchSheet::group-check") {
            stops.push(stop(name, at, Ring::Outside, true));
        }
        return stops;
    }
    if let Sheet::Sync = sheet {
        for (name, e) in SHIFTS.iter().zip(ElementHandle::find_by_element_id(
            app,
            "ShiftChoice::choice",
        )) {
            stops.push(stop(name.to_string(), bounds(&e), Ring::Within, false));
        }
    }
    for id in ["CarryList::check-all", "CarryList::uncheck-all"] {
        stops.push(stop(id.into(), bounds(&one(app, id)), Ring::Edge, false));
    }
    for (name, at) in checks(app, "CarryList::check") {
        stops.push(stop(name, at, Ring::Outside, true));
    }
    if let Some(join) = ElementHandle::find_by_element_id(app, "CarryList::join").next() {
        stops.push(stop(
            "CarryList::join".into(),
            bounds(&join),
            Ring::Within,
            true,
        ));
    }
    for (id, ring) in sheet.buttons() {
        stops.push(stop(id.into(), bounds(&one(app, id)), ring, false));
    }
    stops
}

/// The list's view in the window: top and foot.
fn view(app: &App, sheet: Sheet) -> (f32, f32) {
    let (_, y, _, h) = bounds(&one(app, sheet.list()));
    (y, y + h)
}

/// A pixel of the focus ring, drawn over the sheet: blue well over
/// red, as `Theme.focus-ring` is over the greys and no grey or text is.
fn ring_pixel(p: slint::Rgba8Pixel) -> bool {
    i32::from(p.b) - i32::from(p.r) > 40 && p.b > 90
}

/// The stops that show a focus ring now. One in a list out of the
/// list's view shows none, whatever it holds.
fn ringed(app: &App, sheet: Sheet) -> Vec<String> {
    // The list's scroll to a focused check animates: we let it end, on
    // the mocked clock.
    for _ in 0..4 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        slint::platform::update_timers_and_animations();
    }
    let shot: SharedPixelBuffer<slint::Rgba8Pixel> =
        app.window().take_snapshot().expect("a drawing backend");
    let scale = app.window().scale_factor();
    let (w, h) = (shot.width() as i32, shot.height() as i32);
    let pixels = shot.as_slice();
    let at = |x: i32, y: i32| pixels[(y * w + x) as usize];
    let (top, foot) = view(app, sheet);
    let mut out = Vec::new();
    for s in stops(app, sheet) {
        let (x, y, sw, sh) = s.at;
        if s.in_list && (y < top || y + sh > foot) {
            continue;
        }
        // The pixels the control covers, any part of each: a box on a
        // half pixel smooths its fill into the next one out.
        let (x0, y0) = ((x * scale).floor() as i32, (y * scale).floor() as i32);
        let (x1, y1) = (
            ((x + sw) * scale).ceil() as i32,
            ((y + sh) * scale).ceil() as i32,
        );
        // As far out as a ring goes (3 px), and no further: the match
        // sheet's boxes are 4 px apart.
        let reach = (3.0 * scale).ceil() as i32;
        let mut count = 0;
        for py in (y0 - reach).max(0)..(y1 + reach).min(h) {
            for px in (x0 - reach).max(0)..(x1 + reach).min(w) {
                let inside = px >= x0 && px < x1 && py >= y0 && py < y1;
                let near = px >= x0 - 1 && px <= x1 && py >= y0 - 1 && py <= y1;
                let edge =
                    inside && (px - x0 < 2 || x1 - 1 - px < 2 || py - y0 < 2 || y1 - 1 - py < 2);
                let looked = match s.ring {
                    // Its sides only: the match sheet's checks are
                    // stacked close, one's ring just over the next's.
                    Ring::Outside => !near && py >= y0 && py < y1,
                    Ring::Edge => edge,
                    Ring::Within => inside,
                };
                if looked && ring_pixel(at(px, py)) {
                    count += 1;
                }
            }
        }
        if count >= 8 {
            out.push(s.name);
        }
    }
    out
}

fn shift_tab(app: &App) {
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    press(app, Key::Tab);
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
}

/// Tab (or Shift+Tab) until `name` alone is ringed, at most `most`
/// presses past the first, those between ringing nothing.
fn tab_to(app: &App, sheet: Sheet, name: &str, back: bool, most: usize) {
    for n in 0..=most {
        if back {
            shift_tab(app);
        } else {
            press(app, Key::Tab);
        }
        let now = ringed(app, sheet);
        if now == [name] {
            return;
        }
        assert!(
            now.is_empty() && n < most,
            "{sheet:?}, back {back}: after {} presses toward {name}, ringed {now:?}",
            n + 1
        );
    }
}

/// The whole walk, forward round and back round, every stop ringed
/// alone and in view as it has the keys. With `short`, the list must
/// have stops past its view to begin with.
fn walks_round(sheet: Sheet, height: f32, short: bool) {
    let opened = open(sheet, height);
    let app = &opened.app;
    let names = walk(app, sheet);
    let (top, foot) = view(app, sheet);
    let in_view = stops(app, sheet)
        .iter()
        .filter(|s| s.in_list && s.at.1 >= top && s.at.1 + s.at.3 <= foot)
        .count();
    let rows = sheet.rows(app).row_count();
    if short {
        assert!(in_view < rows, "{sheet:?}: the whole list is in view");
    } else {
        assert_eq!(in_view, rows, "{sheet:?}: the whole list in view");
    }
    // From where the sheet opens the keys, to the first stop.
    tab_to(app, sheet, &names[0], false, 3);
    for (i, name) in names.iter().enumerate().skip(1) {
        press(app, Key::Tab);
        assert_eq!(ringed(app, sheet), [name.as_str()], "{sheet:?}, Tab {i}");
    }
    // Round past the last to the first.
    tab_to(app, sheet, &names[0], false, sheet.between());
    // And back: past the first to the last, then every stop back.
    tab_to(app, sheet, names.last().unwrap(), true, sheet.between());
    for (i, name) in names.iter().enumerate().rev().skip(1) {
        shift_tab(app);
        assert_eq!(
            ringed(app, sheet),
            [name.as_str()],
            "{sheet:?}, Shift+Tab to {i}"
        );
    }
    assert!(sheet.is_open(app));
    assert_eq!(opened.entered.get(), 0, "no Tab entered the sheet");
}

#[test]
fn tab_walks_the_sync_sheet_round_with_a_ring_on_each_stop() {
    walks_round(Sheet::Sync, 1100.0, false);
}

#[test]
fn tab_walks_the_preset_sheet_round_with_a_ring_on_each_stop() {
    walks_round(Sheet::Preset, 1100.0, false);
}

/// A window too short for the whole list: each stop Tab reaches is
/// scrolled into the list's view, down the list and back up.
#[test]
fn a_check_reached_by_tab_is_scrolled_into_the_list_s_view() {
    for sheet in [Sheet::Sync, Sheet::Preset] {
        walks_round(sheet, 640.0, true);
    }
}

/// Space turns the focused check over and back, a heading's and a
/// part's; Enter on a check is the sheet's Enter; Escape closes it.
#[test]
fn space_turns_the_focused_check_and_enter_and_escape_are_the_sheet_s() {
    for sheet in [Sheet::Sync, Sheet::Preset] {
        let opened = open(sheet, 1100.0);
        let app = &opened.app;
        let names = walk(app, sheet);
        tab_to(app, sheet, &names[0], false, 3);
        // On to the first heading, two past Check all, then to the
        // part after it.
        let first = names
            .iter()
            .position(|n| n == "CarryList::check-all")
            .unwrap()
            + 2;
        for _ in 0..first - 1 {
            press(app, Key::Tab);
        }
        for i in 0..2 {
            press(app, Key::Tab);
            let step = first + i;
            let state = |app: &App| sheet.rows(app).row_data(i).unwrap().state;
            assert_eq!(ringed(app, sheet), [names[step].as_str()]);
            assert_eq!(state(app), 2, "{} starts on", names[step]);
            press(app, " ");
            assert_eq!(state(app), 0, "Space turned {} off", names[step]);
            press(app, " ");
            assert_eq!(state(app), 2, "Space turned {} on", names[step]);
            assert_eq!(
                ringed(app, sheet),
                [names[step].as_str()],
                "{} kept the keys",
                names[step]
            );
        }
        press(app, Key::Return);
        assert_eq!(opened.entered.get(), 1, "{sheet:?}: Enter on a check");
        press(app, Key::Escape);
        assert!(!sheet.is_open(app), "{sheet:?}: Escape on a check");
    }
}

/// Left and Right move a focused choice at once, the ring going with
/// the current option, and stop at the row's ends: the masks' Replace
/// or Add on both carry-list sheets, and the sync sheet's Set or
/// Shift. A click on a choice leaves the keys where they were.
#[test]
fn the_arrows_move_a_focused_choice_and_a_click_takes_no_keys() {
    for sheet in [Sheet::Sync, Sheet::Preset] {
        let opened = open(sheet, 1100.0);
        let app = &opened.app;
        let names = walk(app, sheet);
        let join_row = || {
            let rows = sheet.rows(app);
            rows.iter().find(|r| r.kind == 2).unwrap().state
        };
        tab_to(app, sheet, &names[0], false, 3);
        let join = names.iter().position(|n| n == "CarryList::join").unwrap();
        for _ in 0..join {
            press(app, Key::Tab);
        }
        assert_eq!(ringed(app, sheet), ["CarryList::join"]);
        assert_eq!(join_row(), 0, "{sheet:?}: Replace at first");
        press(app, Key::LeftArrow);
        assert_eq!(join_row(), 0, "{sheet:?}: Left at the first end");
        press(app, Key::RightArrow);
        assert_eq!(join_row(), 1, "{sheet:?}: Right chose Add");
        press(app, Key::RightArrow);
        assert_eq!(join_row(), 1, "{sheet:?}: Right at the last end");
        assert_eq!(ringed(app, sheet), ["CarryList::join"], "kept the keys");
        press(app, Key::LeftArrow);
        assert_eq!(join_row(), 0, "{sheet:?}: Left chose Replace");
        // A click on Add from Check all: Add is chosen, and the keys
        // stay on Check all, so an arrow moves nothing.
        let check_all = names
            .iter()
            .position(|n| n == "CarryList::check-all")
            .unwrap();
        for _ in check_all..join {
            shift_tab(app);
        }
        assert_eq!(ringed(app, sheet), ["CarryList::check-all"]);
        let (x, y, w, h) = bounds(&one(app, "CarryList::join"));
        click(app, x + w * 0.75, y + h / 2.0);
        assert_eq!(join_row(), 1, "{sheet:?}: the click chose Add");
        press(app, Key::LeftArrow);
        assert_eq!(join_row(), 1, "{sheet:?}: the arrow was not the choice's");
        if let Sheet::Sync = sheet {
            // Temperature's Set or Shift, the sheet's first stop.
            for _ in 0..check_all {
                shift_tab(app);
            }
            assert_eq!(ringed(app, sheet), [SHIFTS[0]]);
            assert!(!app.get_sync_shift_temperature());
            press(app, Key::RightArrow);
            assert!(app.get_sync_shift_temperature(), "Right chose Shift");
            assert_eq!(ringed(app, sheet), [SHIFTS[0]], "kept the keys");
            press(app, Key::LeftArrow);
            assert!(!app.get_sync_shift_temperature(), "Left chose Set");
        }
        assert!(sheet.is_open(app));
        assert_eq!(opened.entered.get(), 0);
    }
}

/// The match sheet: Tab reaches the scope, then each group's check in
/// order, each ringed, and goes on past the last; Shift+Tab comes back.
/// Right moves the scope, Space turns the focused group over.
#[test]
fn tab_walks_the_match_sheet_s_scope_and_group_checks() {
    let sheet = Sheet::Match;
    let opened = open(sheet, 1100.0);
    let app = &opened.app;
    let names = walk(app, sheet);
    assert_eq!(checks(app, "MatchSheet::group-check").len(), GROUPS.len());
    tab_to(app, sheet, &names[0], false, 3);
    for (i, name) in names.iter().enumerate().skip(1) {
        press(app, Key::Tab);
        assert_eq!(ringed(app, sheet), [name.as_str()], "Tab {i}");
    }
    // On to the switch and the buttons, which ring nothing we read.
    press(app, Key::Tab);
    assert!(ringed(app, sheet).is_empty());
    shift_tab(app);
    for (i, name) in names.iter().enumerate().rev() {
        if i + 1 < names.len() {
            shift_tab(app);
        }
        assert_eq!(ringed(app, sheet), [name.as_str()], "Shift+Tab to {i}");
    }
    let picked = Rc::new(RefCell::new(Vec::new()));
    let seen = picked.clone();
    app.on_match_scope_picked(move |s| seen.borrow_mut().push(s.to_string()));
    press(app, Key::RightArrow);
    assert_eq!(app.get_match_scope(), "Library");
    assert_eq!(*picked.borrow(), ["Library"]);
    // The second group: Space turns it off and on.
    press(app, Key::Tab);
    press(app, Key::Tab);
    assert_eq!(ringed(app, sheet), [names[2].as_str()]);
    press(app, " ");
    assert_eq!(opened.entered.get(), 1);
    assert!(!app.get_match_groups().row_data(1).unwrap().checked);
    press(app, " ");
    assert!(app.get_match_groups().row_data(1).unwrap().checked);
    assert!(sheet.is_open(app));
    // The sheet has no card: its keys are the window's, reached from a
    // focused check and from the focused scope alike. Enter fits
    // nothing (Fit is a button's), Escape closes.
    let (closes, fits) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let (c, f) = (closes.clone(), fits.clone());
    let weak = app.as_weak();
    app.on_match_close(move || {
        c.set(c.get() + 1);
        weak.unwrap().set_match_open(false);
    });
    app.on_match_fit(move || f.set(f.get() + 1));
    for (stop, tabs) in [(names[2].as_str(), 2), ("MatchSheet::scope", 0)] {
        if !sheet.is_open(app) {
            app.set_match_open(true);
            settle(app);
            tab_to(app, sheet, &names[0], false, 3);
            for _ in 0..tabs {
                press(app, Key::Tab);
            }
        }
        assert_eq!(ringed(app, sheet), [stop]);
        press(app, Key::Return);
        assert_eq!(fits.get(), 0, "Enter on {stop} fitted");
        assert!(sheet.is_open(app), "Enter on {stop} closed the sheet");
        let before = closes.get();
        press(app, Key::Escape);
        assert_eq!(closes.get(), before + 1, "Escape on {stop}");
        assert!(!sheet.is_open(app));
    }
}

/// A fit started with the keys on the scope or on a group's check: the
/// scope's arrows and the check's Space do nothing until it ends.
#[test]
fn a_choice_or_check_turned_off_under_the_keys_does_nothing() {
    let sheet = Sheet::Match;
    let opened = open(sheet, 1100.0);
    let app = &opened.app;
    let picked = Rc::new(Cell::new(0));
    let seen = picked.clone();
    app.on_match_scope_picked(move |_| seen.set(seen.get() + 1));
    tab_to(app, sheet, "MatchSheet::scope", false, 3);
    app.set_match_running(true);
    settle(app);
    press(app, Key::RightArrow);
    assert_eq!(app.get_match_scope(), "This folder", "Right mid-run");
    assert_eq!(picked.get(), 0);
    app.set_match_running(false);
    settle(app);
    press(app, Key::Tab);
    assert_eq!(ringed(app, sheet), [format!("Fit {}", GROUPS[0])]);
    app.set_match_running(true);
    settle(app);
    press(app, " ");
    assert_eq!(opened.entered.get(), 0, "Space mid-run");
    assert!(app.get_match_groups().row_data(0).unwrap().checked);
    // Over, the keys work again.
    app.set_match_running(false);
    settle(app);
    press(app, " ");
    assert_eq!(opened.entered.get(), 1);
}

/// The ring pixels inside the option of the choice `id` that starts
/// its row.
fn ring_on_first_option(app: &App, id: &str) -> usize {
    let (x, y, w, h) = bounds(&one(app, id));
    let (ox, oy, ow, oh) = ElementHandle::find_by_element_id(app, "Segmented::seg")
        .map(|e| bounds(&e))
        .filter(|&(sx, sy, _, _)| sx >= x && sx < x + w && sy >= y && sy < y + h)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .expect("the choice's first option");
    let shot = app.window().take_snapshot().expect("a drawing backend");
    let scale = app.window().scale_factor();
    let width = shot.width() as i32;
    let pixels = shot.as_slice();
    let mut count = 0;
    for py in (oy * scale).ceil() as i32..((oy + oh) * scale).floor() as i32 {
        for px in (ox * scale).ceil() as i32..((ox + ow) * scale).floor() as i32 {
            if ring_pixel(pixels[(py * width + px) as usize]) {
                count += 1;
            }
        }
    }
    count
}

/// A choice whose current option it cannot find (none current, or one
/// past the eighth of a long row) rings the row, not a wrong option,
/// and its arrows move nothing.
#[test]
fn a_choice_with_no_option_found_rings_the_row_and_takes_no_arrow() {
    let sheet = Sheet::Match;
    let opened = open(sheet, 1100.0);
    let app = &opened.app;
    let picked = Rc::new(Cell::new(0));
    let seen = picked.clone();
    app.on_match_scope_picked(move |_| seen.set(seen.get() + 1));
    let nine: Vec<slint::SharedString> = (1..=9).map(|i| format!("S{i}").into()).collect();
    app.set_match_scopes(ModelRc::new(VecModel::from(nine)));
    for current in ["S9", ""] {
        app.set_match_scope(current.into());
        settle(app);
        tab_to(app, sheet, "MatchSheet::scope", false, 6);
        assert_eq!(
            ring_on_first_option(app, "MatchSheet::scope"),
            0,
            "{current:?}: the first option ringed"
        );
        press(app, Key::RightArrow);
        press(app, Key::LeftArrow);
        assert_eq!(
            app.get_match_scope(),
            current,
            "{current:?}: an arrow moved"
        );
        assert_eq!(picked.get(), 0);
        // On round, to come back to the scope afresh.
        shift_tab(app);
    }
    // Found, the first option is ringed and the arrows work.
    app.set_match_scope("S1".into());
    settle(app);
    tab_to(app, sheet, "MatchSheet::scope", false, 6);
    assert!(ring_on_first_option(app, "MatchSheet::scope") >= 8);
    press(app, Key::RightArrow);
    assert_eq!(app.get_match_scope(), "S2");
}

/// The files whose choices are the panel's, off the walk, and those
/// whose choices are a sheet's, on it.
const PANEL_FILES: [&str; 9] = [
    "app.slint",
    "panel/scopes.slint",
    "panel/curve.slint",
    "panel/retouch.slint",
    "panel/display.slint",
    "panel/mask.slint",
    "panel/color.slint",
    "panel/cull.slint",
    "panel/edit.slint",
];
const SHEET_FILES: [&str; 7] = [
    "panel/carry-list.slint",
    "panel/sync-sheet.slint",
    "panel/match-sheet.slint",
    "panel/time-shift-sheet.slint",
    "panel/import-sheet.slint",
    "panel/deliver.slint",
    "panel/settings-sheet.slint",
];

/// Every use of a choice in the window's source, by file: the body of
/// each, from its brace to the one that closes it.
fn choice_uses() -> Vec<(String, String)> {
    let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../greycard-slint/ui");
    let mut files = vec![ui.join("app.slint")];
    for e in std::fs::read_dir(ui.join("panel")).unwrap() {
        files.push(e.unwrap().path());
    }
    files.push(ui.join("controls.slint"));
    let mut uses = Vec::new();
    for path in files {
        if path.extension().is_none_or(|e| e != "slint") {
            continue;
        }
        let name = path
            .strip_prefix(&ui)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(&path).unwrap();
        let mut from = 0;
        while let Some(k) = text[from..].find("Segmented {") {
            let open = from + k + "Segmented ".len();
            // The definition, `component Segmented inherits`, has no
            // brace straight after the name; a use does.
            let mut depth = 0;
            let mut end = open;
            for (j, c) in text[open..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + j;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            uses.push((name.clone(), text[open..=end].to_string()));
            from = end;
        }
    }
    uses
}

/// Every choice in the panel's files is off the walk, and every one in
/// a sheet's is on it; a choice in a file on neither list fails until
/// it is put on one.
#[test]
fn every_panel_choice_is_off_the_walk_in_the_source() {
    let uses = choice_uses();
    let off = |body: &str| body.contains("tab-stop: false;");
    let mut panel = 0;
    for (file, body) in &uses {
        if PANEL_FILES.contains(&file.as_str()) {
            panel += 1;
            assert!(off(body), "a choice in {file} is on the walk: {body}");
        } else if SHEET_FILES.contains(&file.as_str()) {
            assert!(!off(body), "a choice in {file} is off the walk: {body}");
        } else {
            panic!("a choice in {file}, on neither list: {body}");
        }
    }
    assert_eq!(panel, 21, "the panel's choices");
    assert!(uses.len() > panel + 20, "{} choices in all", uses.len());
}

/// The panel's choices are no stops of the walk: the panel's Tab is the
/// window's, and none of them offers itself to a screen reader as a
/// stop (a radio group) as a sheet's choice does.
#[test]
fn the_panel_s_choices_are_no_stops_of_the_walk() {
    use i_slint_backend_testing::AccessibleRole;
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 2400.0));
    let (_state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    app.set_collapsed_light(false);
    settle(&app);
    let drawn = ElementHandle::find_by_element_type_name(&app, "Segmented").count();
    assert!(drawn >= 3, "{drawn} choices drawn");
    // A stop of the walk has a scope, and is a radio group to a screen
    // reader: no choice in the panel has either.
    let stops: Vec<_> = ElementHandle::find_by_element_id(&app, "Segmented::fs")
        .map(|s| bounds(&s))
        .chain(
            ElementHandle::find_by_element_type_name(&app, "Segmented")
                .filter(|s| s.accessible_role() == Some(AccessibleRole::RadioGroup))
                .map(|s| bounds(&s)),
        )
        .collect();
    assert!(stops.is_empty(), "panel choices on the walk at {stops:?}");
    // Tab from the panel is the window's: it hides the panes.
    assert!(!app.get_left_hidden());
    press(&app, Key::Tab);
    assert!(app.get_left_hidden(), "Tab reached the window");
}

/// A choice in the panel, clicked, takes no keys: the panel's Tab is
/// the window's, and so is the arrow after the click, which moves the
/// frame.
#[test]
fn a_panel_choice_clicked_leaves_the_arrows_to_the_window() {
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 2400.0));
    let (state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    app.set_collapsed_light(false);
    settle(&app);
    let (x, y, w, h) = bounds(&one(&app, "LightSection::display-curve-choice"));
    click(&app, x + w * 0.75, y + h / 2.0);
    assert_eq!(app.get_display_curve(), "AgX", "the click chose");
    press(&app, Key::RightArrow);
    assert_eq!(state.borrow().current, Some(1), "the window's arrow");
}
