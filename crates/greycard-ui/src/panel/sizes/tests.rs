use i_slint_backend_testing::ElementHandle;
use slint::ComponentHandle;

use super::*;
use crate::testing::{click, drag, settle, window};

const W: f32 = 1500.0;
const H: f32 = 900.0;

/// Long enough for Slint's animated scroll to an item that took
/// the focus to end.
fn rest(app: &App) {
    for _ in 0..10 {
        settle(app);
    }
}

fn sized(app: &App, w: f32, h: f32) {
    app.window().set_size(slint::LogicalSize::new(w, h));
    settle(app);
    settle(app);
}

fn loupe() -> App {
    let app = window(40);
    sized(&app, W, H);
    app
}

/// An element's left, top, width, and height in the window; none
/// when it is not there.
fn at(app: &App, id: &str) -> Option<(f32, f32, f32, f32)> {
    ElementHandle::find_by_element_id(app, &format!("App::{id}"))
        .next()
        .map(|e| {
            let (p, s) = (e.absolute_position(), e.size());
            (p.x, p.y, s.width, s.height)
        })
}

/// A point on a drag edge's line: 100 px down a pane's, clear of
/// the arrow at the viewport's middle, and 700 along the strip's.
fn edge(app: &App, id: &str) -> (f32, f32) {
    let (x, y, w, h) = at(app, id).unwrap_or_else(|| panic!("{id}"));
    if w > h {
        (700.0, y + h / 2.0)
    } else {
        (x + w / 2.0, y + 100.0)
    }
}

/// The pane's line an edge runs along: the left pane's right, the
/// panel's left, the strip's top.
fn line(app: &App, id: &str) -> f32 {
    let (x, y, w, h) = at(app, id).unwrap_or_else(|| panic!("{id}"));
    match id {
        "left-edge" => x + w,
        "right-edge" => x,
        _ => {
            assert!(w > h);
            y
        }
    }
}

fn drag_by(app: &App, id: &str, dx: f32, dy: f32) {
    let (x, y) = edge(app, id);
    drag(
        app,
        &[(x, y), (x + dx / 2.0, y + dy / 2.0), (x + dx, y + dy)],
    );
    settle(app);
}

fn double_click(app: &App, id: &str) {
    let (x, y) = edge(app, id);
    click(app, x, y);
    click(app, x, y);
    settle(app);
}

/// The viewport's width and height, logical pixels.
fn view(app: &App) -> (f32, f32) {
    let (_, _, w, h) = at(app, "view").expect("the viewport");
    (w, h)
}

#[test]
fn the_defaults_are_the_windows() {
    let app = loupe();
    assert_eq!(app.get_left_size(), LEFT);
    assert_eq!(app.get_right_size(), RIGHT);
    assert_eq!(app.get_strip_size(), crate::strip::PHOTO);
    assert_eq!(app.get_strip_photo(), crate::strip::PHOTO);
    // The edges sit inside the panes, along their lines.
    assert_eq!(line(&app, "left-edge"), LEFT);
    assert_eq!(line(&app, "right-edge"), W - RIGHT);
    assert_eq!(line(&app, "strip-edge"), H - app.get_strip_h());
    assert_eq!(view(&app).0, W - LEFT - RIGHT);
}

#[test]
fn the_left_edge_drags_the_pane_within_its_range() {
    let app = loupe();
    let (vw, vh) = view(&app);
    drag_by(&app, "left-edge", 60.0, 0.0);
    assert_eq!(app.get_left_size(), 300.0);
    assert_eq!(line(&app, "left-edge"), 300.0);
    // The viewport gives what the pane took, and its height stays.
    assert_eq!(view(&app), (vw - 60.0, vh));
    // Past a third of the window: held there.
    drag_by(&app, "left-edge", 600.0, 0.0);
    assert_eq!(app.get_left_size(), W / 3.0);
    // Under the floor: held there.
    drag_by(&app, "left-edge", -600.0, 0.0);
    assert_eq!(app.get_left_size(), 200.0);
    // A double-click goes back to the default.
    double_click(&app, "left-edge");
    assert_eq!(app.get_left_size(), LEFT);
    assert_eq!(view(&app), (vw, vh));
}

#[test]
fn the_left_edge_drags_over_the_grid_too() {
    let app = loupe();
    app.set_grid_open(true);
    settle(&app);
    assert!(
        at(&app, "right-edge").is_none(),
        "the grid covers the panel"
    );
    assert!(at(&app, "strip-edge").is_none(), "and the strip");
    drag_by(&app, "left-edge", 40.0, 0.0);
    assert_eq!(app.get_left_size(), 280.0);
}

#[test]
fn the_develop_panels_edge_drags_it_within_its_range() {
    let app = loupe();
    let (vw, _) = view(&app);
    drag_by(&app, "right-edge", -100.0, 0.0);
    assert_eq!(app.get_right_size(), RIGHT + 100.0);
    assert_eq!(line(&app, "right-edge"), W - RIGHT - 100.0);
    assert_eq!(view(&app).0, vw - 100.0);
    drag_by(&app, "right-edge", -600.0, 0.0);
    assert_eq!(app.get_right_size(), W / 3.0);
    drag_by(&app, "right-edge", 600.0, 0.0);
    assert_eq!(app.get_right_size(), 280.0);
    double_click(&app, "right-edge");
    assert_eq!(app.get_right_size(), RIGHT);
}

#[test]
fn the_strips_edge_drags_its_pictures_height_within_its_range() {
    let app = loupe();
    let (vw, vh) = view(&app);
    let strip_h = app.get_strip_h();
    drag_by(&app, "strip-edge", 0.0, -60.0);
    assert_eq!(app.get_strip_size(), crate::strip::PHOTO + 60.0);
    assert_eq!(app.get_strip_photo(), crate::strip::PHOTO + 60.0);
    assert_eq!(app.get_strip_h(), strip_h + 60.0);
    assert_eq!(view(&app), (vw, vh - 60.0));
    let layout = crate::cells::strip_layout(&app);
    let (_, width) = layout.place(0).unwrap();
    assert_eq!(
        width,
        crate::strip::cell_width(0.0, crate::strip::PHOTO + 60.0)
    );
    // The ceiling: the whole strip a third of the window's height.
    drag_by(&app, "strip-edge", 0.0, -900.0);
    assert!(
        (app.get_strip_h() - H / 3.0).abs() < 0.01,
        "{}",
        app.get_strip_h()
    );
    // The floor.
    drag_by(&app, "strip-edge", 0.0, 900.0);
    assert_eq!(app.get_strip_size(), crate::strip::LEAST_PHOTO);
    double_click(&app, "strip-edge");
    assert_eq!(app.get_strip_size(), crate::strip::PHOTO);
    assert_eq!(app.get_strip_h(), strip_h);
}

/// A window made shorter or narrower holds each size to its new
/// ceiling, never under the default, and gives it back as it grows.
#[test]
fn a_smaller_window_holds_the_sizes_to_its_ceilings() {
    let app = loupe();
    app.set_left_size(480.0);
    app.set_right_size(480.0);
    app.set_strip_size(200.0);
    settle(&app);
    assert_eq!(line(&app, "left-edge"), 480.0);
    sized(&app, 900.0, 600.0);
    assert_eq!(line(&app, "left-edge"), 300.0);
    assert_eq!(line(&app, "right-edge"), 900.0 - RIGHT);
    // The strip a third of the window's height.
    assert!(
        (app.get_strip_h() - 200.0).abs() < 0.01,
        "{}",
        app.get_strip_h()
    );
    assert!(app.get_strip_photo() < 200.0);
    // A third of 360 is short of the default strip: the default.
    sized(&app, 900.0, 360.0);
    assert_eq!(app.get_strip_photo(), crate::strip::PHOTO);
    sized(&app, W, H);
    assert_eq!(line(&app, "left-edge"), 480.0);
    assert_eq!(line(&app, "right-edge"), W - 480.0);
    assert_eq!(app.get_strip_photo(), 200.0);
}

#[test]
fn the_sizes_are_kept_and_read_back_at_the_next_start() {
    let app = loupe();
    assert_eq!(
        (left(&app), right(&app), strip(&app)),
        (None, None, None),
        "nothing written for a size never dragged"
    );
    drag_by(&app, "left-edge", 50.0, 0.0);
    drag_by(&app, "strip-edge", 0.0, -30.0);
    let kept = crate::panel::startup::remember(&app);
    assert_eq!(kept.left_width, Some(290.0));
    assert_eq!(kept.right_width, None);
    assert_eq!(kept.strip_photo, Some(crate::strip::PHOTO + 30.0));
    // Through the file's own text.
    let text = serde_json::to_string(&kept).unwrap();
    assert!(!text.contains("right_width"), "{text}");
    let read_back: Settings = serde_json::from_str(&text).unwrap();
    let next = loupe();
    show(&next, &read_back);
    settle(&next);
    assert_eq!(line(&next, "left-edge"), 290.0);
    assert_eq!(next.get_right_size(), RIGHT);
    assert_eq!(next.get_strip_photo(), crate::strip::PHOTO + 30.0);
}

#[test]
fn a_hidden_pane_comes_back_at_its_size() {
    let app = loupe();
    drag_by(&app, "left-edge", 70.0, 0.0);
    drag_by(&app, "right-edge", -70.0, 0.0);
    drag_by(&app, "strip-edge", 0.0, -40.0);
    let strip_h = app.get_strip_h();
    app.set_left_hidden(true);
    app.set_right_hidden(true);
    app.set_strip_hidden(true);
    settle(&app);
    for id in ["left-edge", "right-edge", "strip-edge"] {
        assert!(at(&app, id).is_none(), "{id} goes with its pane");
    }
    assert_eq!(view(&app).0, W);
    app.set_left_hidden(false);
    app.set_right_hidden(false);
    app.set_strip_hidden(false);
    settle(&app);
    assert_eq!(line(&app, "left-edge"), LEFT + 70.0);
    assert_eq!(line(&app, "right-edge"), W - RIGHT - 70.0);
    assert_eq!(app.get_strip_h(), strip_h);
}

#[test]
fn the_lightbox_puts_sized_panes_away_and_back() {
    let app = loupe();
    drag_by(&app, "left-edge", 70.0, 0.0);
    drag_by(&app, "right-edge", -70.0, 0.0);
    drag_by(&app, "strip-edge", 0.0, -40.0);
    let strip_h = app.get_strip_h();
    app.invoke_lightbox_enter();
    settle(&app);
    assert!(app.get_left_hidden() && app.get_right_hidden() && app.get_strip_hidden());
    for id in ["left-edge", "right-edge", "strip-edge"] {
        assert!(at(&app, id).is_none(), "{id} away in the lightbox");
    }
    // The sizes kept through it.
    let kept = crate::panel::startup::remember(&app);
    assert_eq!(kept.left_width, Some(LEFT + 70.0));
    app.invoke_lightbox_leave();
    app.set_grid_open(false);
    settle(&app);
    assert_eq!(line(&app, "left-edge"), LEFT + 70.0);
    assert_eq!(line(&app, "right-edge"), W - RIGHT - 70.0);
    assert_eq!(app.get_strip_h(), strip_h);
}

/// A drag on any edge leaves the picture where it was: the point of
/// it at the viewport's middle and the zoom, fitted or not, as the
/// window's own resize does.
#[test]
fn the_picture_keeps_its_place() {
    for zoom in [0.0, 1.0] {
        let app = window(1);
        sized(&app, W, H);
        let (state, _worker) = crate::testing::retouch_state(&app);
        {
            let mut st = state.borrow_mut();
            st.develop.image_size = (6000, 4000);
            st.develop.center = (2500.0, 1800.0);
            st.develop.zoom = zoom;
        }
        let was = view(&app);
        drag_by(&app, "left-edge", 80.0, 0.0);
        drag_by(&app, "right-edge", -80.0, 0.0);
        drag_by(&app, "strip-edge", 0.0, -50.0);
        assert_eq!(view(&app), (was.0 - 160.0, was.1 - 50.0));
        let st = state.borrow();
        assert_eq!(st.develop.center, (2500.0, 1800.0), "zoom {zoom}");
        assert_eq!(st.develop.zoom, zoom);
    }
}

/// Culling's Picture row in the narrowest panel: its two buttons
/// whole, the label over them; at the default width, beside them.
#[test]
fn the_culling_picture_row_keeps_its_buttons_whole() {
    let app = loupe();
    app.set_culling(true);
    app.set_panel_tab("Cull".into());
    settle(&app);
    let rows = |app: &App| {
        let button = ElementHandle::find_by_element_id(app, "CullSection::camera-jpeg")
            .next()
            .expect("the Camera JPEG button");
        // The row's words, over the buttons or beside them.
        let label = ElementHandle::find_by_element_id(app, "CullSection::picture-over-words")
            .chain(ElementHandle::find_by_element_id(
                app,
                "CullSection::picture-words",
            ))
            .next()
            .expect("the Picture label");
        (
            label.absolute_position().y,
            button.absolute_position().y,
            button.size().width,
        )
    };
    // At the default the button is as wide as its words need.
    let (label, button, wants) = rows(&app);
    assert!(label >= button, "beside at the default: {label} {button}");
    app.set_right_size(280.0);
    settle(&app);
    settle(&app);
    let (label, button, width) = rows(&app);
    assert!(label < button, "over at 280: {label} {button}");
    assert!(width >= wants, "whole at 280: {width} {wants}");
}

/// The culling panel's filter rows hold all their chips at the
/// default window in English, for a big card's counts: a row that
/// scrolls sideways is for French and a narrowed panel.
#[test]
fn the_culling_filter_rows_fit_at_the_default_width() {
    use slint::Model;
    let app = loupe();
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(4));
    app.set_culling(true);
    app.set_panel_tab("Cull".into());
    crate::panel::cull::show_filter(&state.borrow(), &app);
    // Most frames on a row's first chip, a few on the next.
    let counted = |chips: slint::ModelRc<crate::FilterChip>| {
        let chips: Vec<_> = (chips.iter().enumerate())
            .map(|(i, c)| crate::FilterChip {
                count: [412, 36].get(i).copied().unwrap_or(8),
                ..c
            })
            .collect();
        slint::ModelRc::new(slint::VecModel::from(chips))
    };
    app.set_filter_stars(counted(app.get_filter_stars()));
    app.set_filter_flags(counted(app.get_filter_flags()));
    app.set_filter_labels(counted(app.get_filter_labels()));
    settle(&app);
    settle(&app);
    let wanted = [
        ("stars", app.get_filter_stars().row_count()),
        ("flag", app.get_filter_flags().row_count()),
        ("label", app.get_filter_labels().row_count()),
    ];
    for (row, count) in wanted {
        let view = ElementHandle::find_by_element_id(&app, &format!("FilterBar::{row}-flick"))
            .next()
            .map(|e| crate::testing::bounds_of(&e))
            .expect("the row's view");
        // A chip clipped out of the view whole is not found at all.
        let chips = ElementHandle::find_by_element_id(&app, &format!("FilterBar::{row}-row"))
            .next()
            .expect("the row")
            .query_descendants()
            .match_inherits("Chip")
            .find_all();
        let right = (chips.iter())
            .map(|c| c.absolute_position().x + c.size().width)
            .fold(f32::MIN, f32::max);
        let room = view.0.x + view.1.width;
        assert_eq!(chips.len(), count, "{row}: a chip out of view");
        assert!(
            right <= room,
            "{row}: the chips end at {right}, the panel at {room}"
        );
    }
}

/// The wheel over a chip row that runs past the panel scrolls the
/// row sideways to its end, and from there scrolls the panel.
#[test]
fn the_wheel_leaves_a_chip_row_at_its_end() {
    use slint::Model;
    let app = window(40);
    sized(&app, W, 640.0);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(4));
    app.set_culling(true);
    app.set_panel_tab("Cull".into());
    app.set_right_size(280.0);
    crate::panel::cull::show_filter(&state.borrow(), &app);
    // Counts wide enough that the row runs well past the panel.
    let chips: Vec<_> = (app.get_filter_labels().iter())
        .map(|c| crate::FilterChip { count: 888, ..c })
        .collect();
    app.set_filter_labels(slint::ModelRc::new(slint::VecModel::from(chips)));
    settle(&app);
    settle(&app);
    let row_x = || {
        ElementHandle::find_by_element_id(&app, "FilterBar::label-row")
            .next()
            .expect("the row")
            .absolute_position()
            .x
    };
    let (at, size) = ElementHandle::find_by_element_id(&app, "FilterBar::label-flick")
        .next()
        .map(|e| crate::testing::bounds_of(&e))
        .expect("the row's view");
    let middle = slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
    let wheel = || {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: middle,
                delta_x: 0.0,
                delta_y: -20.0,
            });
        settle(&app);
    };
    // Sideways to the row's end, the panel still.
    let mut x = row_x();
    let mut turns = 0;
    loop {
        wheel();
        let now = row_x();
        if now == x {
            break;
        }
        assert!(now < x, "the row moves sideways");
        assert_eq!(app.get_panel_scroll(), 0.0, "the panel stays");
        x = now;
        turns += 1;
        assert!(turns < 50, "the row reaches its end");
    }
    assert!(turns > 0, "the row scrolled at all");
    // The turn that found the end went on to the panel.
    assert!(
        app.get_panel_scroll() < 0.0,
        "at its end, the panel scrolls"
    );
}

/// A range as the CULLING section shows one: `values` as shown, the
/// keys their first word, both ends at the folder's own.
fn range(name: &str, code: i32, values: &[&str]) -> crate::FacetRange {
    use slint::{ModelRc, SharedString, VecModel};
    let shown: Vec<SharedString> = values.iter().map(|v| (*v).into()).collect();
    let keys: Vec<SharedString> = (values.iter())
        .map(|v| v.split(' ').next().unwrap_or(v).into())
        .collect();
    let widest = (values.iter())
        .max_by_key(|v| v.chars().count())
        .copied()
        .unwrap_or_default();
    crate::FacetRange {
        name: name.into(),
        code,
        values: ModelRc::new(VecModel::from(shown)),
        keys: ModelRc::new(VecModel::from(keys)),
        widest: widest.into(),
        low: 0,
        high: values.len() as i32 - 1,
        on: false,
    }
}

const ISOS: [&str; 11] = [
    "100", "200", "400", "800", "1600", "3200", "6400", "12800", "25600", "51200", "102400",
];
const MMS: [&str; 7] = [
    "14 mm", "23.9 mm", "35 mm", "70 mm", "105 mm", "200 mm", "600 mm",
];

/// The culling panel with its two ranges up, at the right pane's
/// `width`, their captions `names`.
fn ranged(width: f32, names: (&str, &str)) -> (App, std::rc::Rc<crate::Shared>) {
    let app = loupe();
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(4));
    app.set_culling(true);
    app.set_panel_tab("Cull".into());
    app.set_right_size(width);
    crate::panel::cull::show_filter(&state.borrow(), &app);
    app.set_filter_iso_range(range(names.0, 4, &ISOS));
    app.set_filter_focal_range(range(names.1, 5, &MMS));
    settle(&app);
    settle(&app);
    (app, state)
}

/// The ranges fit the panel at its default width and its narrowest,
/// in English and in French, each no taller than a chip row, their
/// tracks long enough to set and lined up.
#[test]
fn the_culling_ranges_fit_no_taller_than_a_chip_row() {
    for (width, names) in [
        (RIGHT, ("ISO", "Focal")),
        (280.0, ("ISO", "Focal")),
        (RIGHT, ("ISO", "Focale")),
        (280.0, ("ISO", "Focale")),
    ] {
        let (app, _state) = ranged(width, names);
        let (view, size) = ElementHandle::find_by_element_id(&app, "FilterBar::label-flick")
            .next()
            .map(|e| crate::testing::bounds_of(&e))
            .expect("the label row");
        let room = view.x + size.width;
        let rows: Vec<_> = ElementHandle::find_by_element_type_name(&app, "RangeRow").collect();
        assert_eq!(rows.len(), 2, "{width} {names:?}: both ranges");
        for row in &rows {
            let (at, s) = crate::testing::bounds_of(row);
            assert!(
                s.height <= size.height,
                "{width} {names:?}: a range {} tall, a chip row {}",
                s.height,
                size.height
            );
            assert!(
                at.x + s.width <= room + 0.5,
                "{width} {names:?}: a range ends at {}, the panel at {room}",
                at.x + s.width
            );
        }
        let tracks: Vec<_> = ElementHandle::find_by_element_type_name(&app, "RangeSlider")
            .map(|e| crate::testing::bounds_of(&e))
            .collect();
        assert_eq!(tracks.len(), 2);
        for (at, s) in &tracks {
            assert!(
                s.width >= 100.0,
                "{width} {names:?}: a track {} wide",
                s.width
            );
            assert!(
                at.x + s.width <= room,
                "{width} {names:?}: a track past the panel"
            );
        }
        assert_eq!(tracks[0].0.x, tracks[1].0.x, "{width} {names:?}: lined up");
        assert_eq!(tracks[0].1.width, tracks[1].1.width, "{width} {names:?}");
    }
}

/// A handle only drags: held, shown a stop at a time and no further
/// than the other end, and asked for on release only; a press off
/// the handles moves nothing; a key moves a focused end one stop at
/// once; a double-click asks for the whole range and nothing else.
#[test]
fn a_range_moves_by_the_folders_values() {
    use slint::platform::{PointerEventButton, WindowEvent};
    use std::cell::RefCell;
    use std::rc::Rc;
    /// A range's ask: the facet's code, the end, and both keys.
    type Asked = (i32, i32, String, String);
    let (app, _state) = ranged(RIGHT, ("ISO", "Focal"));
    let asked: Rc<RefCell<Vec<Asked>>> = Rc::default();
    let seen = asked.clone();
    app.on_filter_range_moved(move |code, end, low, high| {
        seen.borrow_mut()
            .push((code, end, low.to_string(), high.to_string()));
    });
    let reset: Rc<RefCell<Vec<i32>>> = Rc::default();
    let seen = reset.clone();
    app.on_filter_range_reset(move |code| seen.borrow_mut().push(code));
    let (at, s) = ElementHandle::find_by_element_type_name(&app, "RangeSlider")
        .next()
        .map(|e| crate::testing::bounds_of(&e))
        .expect("the ISO range");
    let y = at.y + s.height / 2.0;
    let stop = |i: usize| at.x + 8.0 + (s.width - 16.0) * i as f32 / (ISOS.len() - 1) as f32;
    let last = || {
        asked
            .borrow()
            .last()
            .cloned()
            .expect("an end asked to move")
    };
    let own = |t: (i32, i32, &str, &str)| (t.0, t.1, t.2.to_string(), t.3.to_string());
    let shows = |words: &str| {
        let words = words.to_string();
        ElementHandle::find_by_element_type_name(&app, "RangeRow")
            .next()
            .and_then(|row| {
                row.query_descendants()
                    .match_predicate(move |e| {
                        e.accessible_label().is_some_and(|l| l.as_str() == words)
                    })
                    .find_first()
            })
            .is_some()
    };
    let pointer = |event: WindowEvent| app.window().dispatch_event(event);
    let to = |x: f32| slint::LogicalPosition::new(x, y);
    let left = PointerEventButton::Left;

    // The low end, from its stop to the third: held and shown on
    // the way, asked for once, on release.
    pointer(WindowEvent::PointerMoved {
        position: to(stop(0)),
    });
    pointer(WindowEvent::PointerPressed {
        position: to(stop(0)),
        button: left,
    });
    pointer(WindowEvent::PointerMoved {
        position: to(stop(1)),
    });
    pointer(WindowEvent::PointerMoved {
        position: to(stop(2)),
    });
    assert!(asked.borrow().is_empty(), "nothing asked during a drag");
    assert!(shows("400"), "the held end's value is shown");
    pointer(WindowEvent::PointerReleased {
        position: to(stop(2)),
        button: left,
    });
    assert_eq!(*asked.borrow(), [own((4, 0, "400", ""))]);
    settle(&app);
    // The high end, past the low one: it stops on the low's stop,
    // which the owner left at the first.
    crate::testing::drag(&app, &[(stop(10), y), (stop(5), y), (at.x - 40.0, y)]);
    settle(&app);
    assert_eq!(last(), own((4, 1, "", "100")));
    // A press off the handles moves nothing: a click on the track,
    // and a drag from it.
    asked.borrow_mut().clear();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
    crate::testing::click(&app, stop(7), y);
    crate::testing::drag(&app, &[(stop(5), y), (stop(6), y), (stop(8), y)]);
    settle(&app);
    assert!(asked.borrow().is_empty(), "{:?}", asked.borrow());
    // A click on a handle and then a drag of it, quick as a
    // double-click: a drag, not a reset.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
    crate::testing::click(&app, stop(10), y);
    crate::testing::drag(&app, &[(stop(10), y), (stop(9), y), (stop(8), y)]);
    settle(&app);
    assert_eq!(*asked.borrow(), [own((4, 1, "", "25600"))]);
    assert!(reset.borrow().is_empty());

    // The keys: back from the words, the focal length's high end,
    // and Left a stop down.
    app.set_filter_focus(true);
    settle(&app);
    crate::testing::press(&app, slint::platform::Key::Backtab);
    settle(&app);
    crate::testing::press(&app, slint::platform::Key::LeftArrow);
    settle(&app);
    assert_eq!(last(), own((5, 1, "", "200")));

    // A double-click, off a handle and on one: the whole range, and
    // no move asked.
    asked.borrow_mut().clear();
    for x in [stop(3), stop(0)] {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
        crate::testing::click(&app, x, y);
        crate::testing::click(&app, x, y);
        settle(&app);
    }
    assert_eq!(*reset.borrow(), [4, 4]);
    assert!(asked.borrow().is_empty(), "{:?}", asked.borrow());
}

/// A screen reader hears both handles of both ranges alike: the stop
/// as the value, among the stops that handle reaches, and the
/// folder's value there as the description.
#[test]
fn both_handles_of_both_ranges_say_their_stop_and_value() {
    use i_slint_backend_testing::AccessibleRole;
    let (app, _state) = ranged(RIGHT, ("ISO", "Focal"));
    let mut focal = range("Focal", 5, &MMS);
    (focal.low, focal.high) = (1, 4);
    app.set_filter_focal_range(focal);
    settle(&app);
    let said: Vec<_> = ElementHandle::find_by_element_type_name(&app, "RangeHandle")
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Slider))
        .map(|e| {
            (
                e.accessible_label().unwrap_or_default().to_string(),
                e.accessible_value().unwrap_or_default().to_string(),
                e.accessible_value_minimum(),
                e.accessible_value_maximum(),
                e.accessible_description().unwrap_or_default().to_string(),
            )
        })
        .collect();
    type Said = (String, String, Option<f32>, Option<f32>, String);
    let one = |label: &str, value: &str, min: f32, max: f32, words: &str| -> Said {
        (
            label.into(),
            value.into(),
            Some(min),
            Some(max),
            words.into(),
        )
    };
    assert_eq!(
        said,
        [
            one("ISO", "0", 0.0, 10.0, "100"),
            one("ISO", "10", 0.0, 10.0, "102400"),
            one("Focal", "1", 0.0, 4.0, "23.9 mm"),
            one("Focal", "4", 1.0, 6.0, "105 mm"),
        ]
    );
}

/// Two ends on one stop: a drag from them to the right moves the
/// high end and one to the left the low end, at the track's middle
/// and at either end; a click on them, or beside them, moves none.
#[test]
fn two_ends_on_one_stop_move_by_the_drags_way() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let top = ISOS.len() - 1;
    for at_stop in [5, 0, top] {
        let (app, _state) = ranged(RIGHT, ("ISO", "Focal"));
        let mut both = range("ISO", 4, &ISOS);
        (both.low, both.high) = (at_stop as i32, at_stop as i32);
        app.set_filter_iso_range(both);
        settle(&app);
        let ends: Rc<RefCell<Vec<i32>>> = Rc::default();
        let seen = ends.clone();
        app.on_filter_range_moved(move |_, end, _, _| seen.borrow_mut().push(end));
        let (at, s) = ElementHandle::find_by_element_type_name(&app, "RangeSlider")
            .next()
            .map(|e| crate::testing::bounds_of(&e))
            .expect("the ISO range");
        let y = at.y + s.height / 2.0;
        let stop = |i: usize| at.x + 8.0 + (s.width - 16.0) * i as f32 / top as f32;
        // Apart in time, so no two presses make a double-click.
        let apart = || {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(2));
            settle(&app);
        };
        let asked = |path: &[(f32, f32)]| -> Option<i32> {
            ends.borrow_mut().clear();
            if path.len() == 1 {
                crate::testing::click(&app, path[0].0, path[0].1);
            } else {
                crate::testing::drag(&app, path);
            }
            apart();
            ends.borrow().last().copied()
        };
        let on = stop(at_stop);
        assert_eq!(asked(&[(on, y)]), None, "{at_stop}: a click on them");
        if at_stop < top {
            let right = stop((at_stop + 3).min(top));
            assert_eq!(asked(&[(right, y)]), None, "{at_stop}: a click right");
            assert_eq!(
                asked(&[(on, y), (on + 5.0, y), (right, y)]),
                Some(1),
                "{at_stop}: a drag right"
            );
        }
        if at_stop > 0 {
            let left = stop(at_stop.saturating_sub(3));
            assert_eq!(asked(&[(left, y)]), None, "{at_stop}: a click left");
            assert_eq!(
                asked(&[(on, y), (on - 5.0, y), (left, y)]),
                Some(0),
                "{at_stop}: a drag left"
            );
        }
    }
}

/// The panel's head keeps its height whether the open frame says
/// when it was shot or not, so a range that opens another frame
/// does not move the CULLING section under the pointer.
#[test]
fn the_heads_shot_time_line_keeps_its_height_when_empty() {
    let (app, _state) = ranged(RIGHT, ("ISO", "Focal"));
    let head = || {
        ElementHandle::find_by_element_type_name(&app, "PanelHead")
            .next()
            .map(|e| crate::testing::bounds_of(&e).1.height)
            .expect("the head")
    };
    app.set_shot_time("2026-03-14 10:41:55".into());
    settle(&app);
    let said = head();
    app.set_shot_time("".into());
    settle(&app);
    assert_eq!(head(), said);
}

/// A range's asks, the end and its key, and its resets, by facet.
type Asks = std::rc::Rc<std::cell::RefCell<Vec<(i32, String)>>>;
type Resets = std::rc::Rc<std::cell::RefCell<Vec<i32>>>;

/// The CULLING section with the ISO range over `values`, its ends at
/// `ends`, what it asks kept and nothing done with it (so the ends
/// stay put), and the x of each stop and the track's middle y.
fn watched(
    values: &[&str],
    ends: (usize, usize),
) -> (App, std::rc::Rc<crate::Shared>, Asks, Resets, Vec<f32>, f32) {
    let (app, state) = ranged(RIGHT, ("ISO", "Focal"));
    let mut iso = range("ISO", 4, values);
    (iso.low, iso.high) = (ends.0 as i32, ends.1 as i32);
    app.set_filter_iso_range(iso);
    settle(&app);
    let asks: Asks = Asks::default();
    let seen = asks.clone();
    app.on_filter_range_moved(move |_, end, low, high| {
        let key = if end == 0 { low } else { high };
        seen.borrow_mut().push((end, key.to_string()));
    });
    let resets: Resets = Resets::default();
    let seen = resets.clone();
    app.on_filter_range_reset(move |code| seen.borrow_mut().push(code));
    let (at, s) = ElementHandle::find_by_element_type_name(&app, "RangeSlider")
        .next()
        .map(|e| crate::testing::bounds_of(&e))
        .expect("the ISO range");
    let last = (values.len() - 1) as f32;
    let stops = (0..values.len())
        .map(|i| at.x + 8.0 + (s.width - 16.0) * i as f32 / last)
        .collect();
    (app, state, asks, resets, stops, at.y + s.height / 2.0)
}

fn pointer(app: &App, event: slint::platform::WindowEvent) {
    app.window().dispatch_event(event);
}

/// Pressed with `button` at `x`, on the track's line.
fn down(app: &App, x: f32, y: f32, button: slint::platform::PointerEventButton) {
    let position = slint::LogicalPosition::new(x, y);
    pointer(app, slint::platform::WindowEvent::PointerMoved { position });
    pointer(
        app,
        slint::platform::WindowEvent::PointerPressed { position, button },
    );
}

fn up(app: &App, x: f32, y: f32, button: slint::platform::PointerEventButton) {
    let position = slint::LogicalPosition::new(x, y);
    pointer(
        app,
        slint::platform::WindowEvent::PointerReleased { position, button },
    );
}

fn to(app: &App, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    pointer(app, slint::platform::WindowEvent::PointerMoved { position });
}

/// Time passing without a pointer move, as `settle` would make.
fn tick(by_ms: u64) {
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(by_ms));
    slint::platform::update_timers_and_animations();
}

/// Stops closer than Slint's 10 px for a double-click: a drag of
/// a stop and then a quick click, and two small drags of a stop
/// each, quick and close, are moves and never a reset; a real
/// double-click still resets. Two ends a stop apart, both under
/// the press: the drag's way picks the end.
#[test]
fn close_stops_take_a_drag_and_a_click_for_moves() {
    use slint::platform::PointerEventButton::Left;
    let values: Vec<String> = (0..60).map(|i| (100 + i).to_string()).collect();
    let values: Vec<&str> = values.iter().map(String::as_str).collect();
    let (app, _state, asks, resets, stop, y) = watched(&values, (0, 59));
    assert!(stop[1] - stop[0] < 4.0, "stops {} apart", stop[1] - stop[0]);
    // A stop's drag, under 4 px, then a click at once beside it.
    down(&app, stop[0], y, Left);
    to(&app, stop[1], y);
    up(&app, stop[1], y, Left);
    down(&app, stop[1], y, Left);
    up(&app, stop[1], y, Left);
    assert_eq!(*asks.borrow(), [(0, "101".to_string())]);
    assert!(resets.borrow().is_empty(), "a drag and a click reset");
    // Two small drags, quick and close.
    tick(2000);
    asks.borrow_mut().clear();
    for (from, onto) in [(0, 1), (1, 2)] {
        down(&app, stop[from], y, Left);
        to(&app, stop[onto], y);
        up(&app, stop[onto], y, Left);
    }
    assert_eq!(asks.borrow().len(), 2, "{:?}", asks.borrow());
    assert!(resets.borrow().is_empty(), "two drags reset");
    // A double-click, apart in time from the drags.
    tick(2000);
    asks.borrow_mut().clear();
    for _ in 0..2 {
        down(&app, stop[30], y, Left);
        up(&app, stop[30], y, Left);
    }
    assert_eq!(*resets.borrow(), [4]);
    assert!(asks.borrow().is_empty());

    // Two ends a stop apart: a drag right from between them moves
    // the high end, a drag left the low.
    let (app, _state, asks, _, stop, y) = watched(&values, (20, 21));
    let between = (stop[20] + stop[21]) / 2.0;
    down(&app, between, y, Left);
    to(&app, between + 6.0, y);
    to(&app, stop[30], y);
    up(&app, stop[30], y, Left);
    tick(2000);
    down(&app, between, y, Left);
    to(&app, between - 6.0, y);
    to(&app, stop[10], y);
    up(&app, stop[10], y, Left);
    assert_eq!(
        *asks.borrow(),
        [(1, "130".to_string()), (0, "110".to_string())]
    );
    // Pressed left of both, still within reach of each, and moved a
    // little right: the high end, picked by the way, does not come
    // left to the pointer's stop.
    let (app, _state, asks, _, stop, y) = watched(&values, (20, 21));
    down(&app, stop[20] - 4.4, y, Left);
    to(&app, stop[20] + 0.1, y);
    up(&app, stop[20] + 0.1, y, Left);
    assert!(asks.borrow().is_empty(), "{:?}", asks.borrow());
}

/// A drag that another button interrupts, and whose left release
/// lands outside the range, holds nothing after: the handle is back
/// on its stop, and the next click on the track asks nothing.
#[test]
fn a_drag_cut_short_by_another_button_holds_nothing() {
    use slint::platform::PointerEventButton::{Left, Right};
    let (app, _state, asks, resets, stop, y) = watched(&ISOS, (0, 10));
    down(&app, stop[0], y, Left);
    to(&app, stop[3], y);
    down(&app, stop[3], y, Right);
    up(&app, stop[3], y, Right);
    to(&app, 5.0, 5.0);
    up(&app, 5.0, 5.0, Left);
    tick(2000);
    let shown = ElementHandle::find_by_element_type_name(&app, "RangeHandle")
        .next()
        .and_then(|e| e.accessible_value())
        .unwrap_or_default();
    assert_eq!(shown, "0", "the low handle back on its stop");
    down(&app, stop[6], y, Left);
    up(&app, stop[6], y, Left);
    tick(2000);
    assert!(asks.borrow().is_empty(), "{:?}", asks.borrow());
    assert!(resets.borrow().is_empty());
}

/// The stops read again under a drag (the index got further, a
/// stack opened): a hold whose stop now has another value goes,
/// asking nothing; one whose stop keeps its value carries on.
#[test]
fn new_stops_under_a_drag_let_go_of_it() {
    use slint::platform::PointerEventButton::Left;
    let (app, _state, asks, _, stop, y) = watched(&ISOS, (0, 10));
    down(&app, stop[0], y, Left);
    to(&app, stop[5], y);
    app.set_filter_iso_range(range("ISO", 4, &ISOS[..3]));
    tick(20);
    up(&app, stop[5], y, Left);
    tick(2000);
    assert!(asks.borrow().is_empty(), "{:?}", asks.borrow());
    // The same values in a model of their own: the drag goes on.
    let (app, _state, asks, _, stop, y) = watched(&ISOS, (0, 10));
    down(&app, stop[0], y, Left);
    to(&app, stop[5], y);
    app.set_filter_iso_range(range("ISO", 4, &ISOS));
    tick(20);
    to(&app, stop[6], y);
    up(&app, stop[6], y, Left);
    assert_eq!(*asks.borrow(), [(0, "6400".to_string())]);
}

/// A held arrow steps the handle at each repeat and asks once, when
/// it is let go; an arrow held as the keys go elsewhere asks then.
#[test]
fn a_held_arrow_asks_once_when_let_go() {
    use slint::platform::WindowEvent;
    let (app, _state, asks, _, _, _) = watched(&ISOS, (0, 10));
    app.set_filter_focus(true);
    settle(&app);
    // Back from the words, the focal length's high end; again, the
    // ISO's.
    crate::testing::press(&app, slint::platform::Key::Backtab);
    crate::testing::press(&app, slint::platform::Key::Backtab);
    crate::testing::press(&app, slint::platform::Key::Backtab);
    settle(&app);
    let left: slint::SharedString = slint::platform::Key::LeftArrow.into();
    for _ in 0..3 {
        app.window()
            .dispatch_event(WindowEvent::KeyPressed { text: left.clone() });
    }
    settle(&app);
    assert!(asks.borrow().is_empty(), "{:?}", asks.borrow());
    app.window()
        .dispatch_event(WindowEvent::KeyReleased { text: left.clone() });
    assert_eq!(*asks.borrow(), [(1, "12800".to_string())]);
    // Held again, and the keys taken by Tab before it is let go.
    app.window()
        .dispatch_event(WindowEvent::KeyPressed { text: left.clone() });
    assert_eq!(asks.borrow().len(), 1);
    crate::testing::press(&app, slint::platform::Key::Tab);
    settle(&app);
    assert_eq!(asks.borrow().len(), 2, "{:?}", asks.borrow());
    assert_eq!(asks.borrow()[1], (1, "51200".to_string()));
}

/// Shift+Tab to a handle cut by the panel's edge scrolls it into
/// view, the panel's view doing so of itself for the focused handle
/// (14 px, unfocused, around the track's middle); the handle asks
/// nothing. (Slint's focus chain passes over a handle clipped out
/// of view whole, as it passes over an `EditSlider`.)
#[test]
fn a_handle_reached_by_the_keys_is_brought_into_view() {
    let (app, _state) = ranged(RIGHT, ("ISO", "Focal"));
    // Short, so the panel can scroll the ranges up past its top:
    // it scrolls no further than its sections reach.
    sized(&app, W, 520.0);
    let sliders = || {
        ElementHandle::find_by_element_type_name(&app, "RangeSlider")
            .map(|e| crate::testing::bounds_of(&e))
            .collect::<Vec<_>>()
    };
    let (view, _) = ElementHandle::find_by_element_id(&app, "App::panel-view")
        .next()
        .map(|e| crate::testing::bounds_of(&e))
        .expect("the panel's view");
    // The keys in the filter's words, then the panel scrolled until
    // the focal range's top half is above the view.
    // Slint scrolls an item that takes the focus into view, and
    // since 1.19 animates the scroll: it is let finish first.
    app.set_filter_focus(true);
    rest(&app);
    let focal = sliders()[1];
    let scrolled = app.get_panel_scroll() - (focal.0.y + focal.1.height / 2.0 - view.y);
    app.set_panel_scroll(scrolled);
    settle(&app);
    let focal = *sliders().last().expect("the focal range, cut");
    assert!(focal.0.y < view.y, "cut by the edge first: {focal:?}");
    crate::testing::press(&app, slint::platform::Key::Backtab);
    rest(&app);
    assert!(app.get_panel_scroll() > scrolled, "the panel scrolled back");
    // The handles, on the track's middle line, in view; the row's
    // edges around them may stay cut. Slint brings the focused
    // handle to the view's very edge, to a float's rounding.
    let (at, s) = *sliders().last().expect("the focal range");
    let middle = at.y + s.height / 2.0;
    assert!(
        middle - 7.0 >= view.y - 0.01 && middle + 7.0 <= 520.0 + 0.01,
        "the focal range's handles in view: {at:?} under {}",
        view.y
    );
}

/// The strip stays still about its selected frame as its edge is
/// dragged, from the first move on: the frame is whole on screen at
/// every step, and its left edge moves by no more than its own cell
/// grows. Far along the strip, and at its very end (End).
#[test]
fn the_strip_holds_its_selected_frame_as_its_edge_is_dragged() {
    use slint::platform::{PointerEventButton, WindowEvent};
    for selected in [60, 98] {
        let app = window(99);
        sized(&app, W, H);
        app.set_selected(selected);
        settle(&app);
        let at = |app: &App| {
            let (left, width) = crate::cells::strip_layout(app).place(selected).unwrap();
            (left - app.get_strip_scroll(), width)
        };
        let whole = |app: &App, (left, width): (f32, f32)| {
            assert!(
                left >= 0.0 && left + width <= app.get_strip_view() + 0.5,
                "frame {selected} whole: {left} {width} in {}",
                app.get_strip_view()
            );
        };
        let mut was = at(&app);
        assert!(app.get_strip_scroll() > 1000.0, "far along the strip");
        whole(&app, was);
        let (x, y) = edge(&app, "strip-edge");
        let event = |e: WindowEvent| app.window().dispatch_event(e);
        event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(x, y),
        });
        event(WindowEvent::PointerPressed {
            position: slint::LogicalPosition::new(x, y),
            button: PointerEventButton::Left,
        });
        for step in 1..=20 {
            event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(x, y - 2.0 * step as f32),
            });
            slint::platform::update_timers_and_animations();
            let now = at(&app);
            whole(&app, now);
            assert!(
                (now.0 - was.0).abs() <= (now.1 - was.1) + 1.0,
                "frame {selected}, step {step}: {was:?} to {now:?}"
            );
            was = now;
        }
        event(WindowEvent::PointerReleased {
            position: slint::LogicalPosition::new(x, y - 40.0),
            button: PointerEventButton::Left,
        });
        settle(&app);
        assert_eq!(app.get_strip_photo(), crate::strip::PHOTO + 40.0);
    }
}

/// A picture made at the strip's old size that lands after a drag
/// made the strip taller is asked for again at the new size, once,
/// as one landing late on a grown grid is.
#[test]
fn a_late_picture_on_a_taller_strip_is_asked_for_again() {
    let app = loupe();
    let files = crate::testing::folder(40);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    app.set_grid_open(false);
    settle(&app);
    drag_by(&app, "strip-edge", 0.0, -69.0);
    assert_eq!(state.borrow().views.thumb_want, 240);
    let (first, _) = state
        .borrow()
        .views
        .strip_shown
        .expect("the strip's frames");
    let row = first as usize;
    let late = crate::worker::Outcome::Thumbnail {
        index: row,
        frame: crate::versions::Key::file(files[row].clone()),
        size: 176,
        width: 2,
        height: 2,
        rgb: vec![0; 12],
        cached: false,
        seconds: 0.0,
    };
    let asked = |st: &crate::State| st.views.thumb_asked[row];
    assert!(asked(&state.borrow()) < 240);
    crate::panel::deliver::deliver(&app, late);
    assert_eq!(
        asked(&state.borrow()),
        240,
        "asked again at the strip's size"
    );
}

/// The strip asks the thumbnail pool for the long edge its row
/// needs: the default's at the default, larger as it grows, and
/// back down as it shrinks.
#[test]
fn a_taller_strip_asks_for_larger_pictures() {
    let app = loupe();
    let files = crate::testing::folder(40);
    let (state, _worker) = crate::testing::state_for(&app, files);
    app.set_grid_open(false);
    settle(&app);
    app.invoke_strip_range(0, 5);
    assert_eq!(state.borrow().views.thumb_want, crate::worker::THUMB_WIDTH);
    drag_by(&app, "strip-edge", 0.0, -69.0);
    assert_eq!(app.get_strip_photo(), 160.0);
    assert!(state.borrow().views.strip_shown.is_some());
    assert_eq!(state.borrow().views.thumb_want, 240);
    // The cells drawn are the taller ones.
    let rings: Vec<f32> = ElementHandle::find_by_element_id(&app, "Filmstrip::frame")
        .map(|e| e.size().height)
        .collect();
    assert!(!rings.is_empty());
    let band = app.global::<crate::Theme>().get_space_1();
    for h in rings {
        assert_eq!(h, 160.0 + band + 2.0 * crate::strip::RING);
    }
    double_click(&app, "strip-edge");
    assert_eq!(state.borrow().views.thumb_want, crate::worker::THUMB_WIDTH);
}

/// The label column's width at the default panel, and its cap.
const LEAST: f32 = 68.0;
const MOST: f32 = 116.0;
/// The panel width the column starts to grow from.
const FROM: f32 = 320.0;
/// The column at the panel's default width.
const REST: f32 = LEAST + RIGHT - FROM;
/// The gap between a slider's label and its track, and the room
/// its value has at the right.
const GAP: f32 = 8.0;
const VALUE: f32 = 60.0;

/// The part `part` (as `EditSlider::label-words`) of the slider `id`.
fn slider_part(app: &App, id: &str, part: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(app, id)
        .find_map(|e| e.query_descendants().match_id(part).find_first())
        .unwrap_or_else(|| panic!("{id}'s {part}"))
}

/// The slider `id`'s label column and its track's width, as laid
/// out: from the label's left to the track, and from there to the
/// value field less the gap before it.
fn slider_parts(app: &App, id: &str) -> (f32, f32) {
    let (at, size) = crate::testing::by_id(app, id);
    let text = slider_part(app, id, "EditSlider::label-words");
    let field = slider_part(app, id, "EditSlider::field");
    let label = id;
    assert_eq!(text.absolute_position().x, at.x, "{label}");
    let column = text.size().width + GAP;
    let track = field.absolute_position().x - GAP - (at.x + column);
    // The field keeps its place at the row's right.
    assert_eq!(
        field.absolute_position().x,
        at.x + size.width - VALUE + GAP,
        "{label}"
    );
    (column, track)
}

fn right_at(app: &App, width: f32) {
    app.set_right_size(width);
    settle(app);
    settle(app);
}

/// The panel's label column is 68 px at its default width and
/// under; dragged wider, the labels take the extra first, up to the
/// cap, the tracks staying as they were, and only then do the tracks
/// grow. The sliders and the rows with a control share it.
#[test]
fn a_wider_panel_widens_the_labels_first() {
    let app = loupe();
    let column = |app: &App| app.global::<crate::LabelColumn>().get_width();
    assert_eq!(column(&app), REST);
    let (label, track) = slider_parts(&app, ROUNDNESS_SLIDER);
    assert_eq!(label, REST);
    // The narrowest panel gives the labels' extra back, then takes
    // its room from the tracks.
    right_at(&app, 280.0);
    assert_eq!(column(&app), LEAST);
    assert_eq!(slider_parts(&app, ROUNDNESS_SLIDER), (LEAST, track - 40.0));
    // Wider: the labels first.
    for (panel, label) in [(344.0, 92.0), (FROM + MOST - LEAST, MOST)] {
        right_at(&app, panel);
        assert_eq!(column(&app), label, "at {panel}");
        assert_eq!(
            slider_parts(&app, ROUNDNESS_SLIDER),
            (label, track),
            "at {panel}"
        );
        assert_eq!(
            slider_parts(&app, "LightSection::exposure"),
            (label, track),
            "at {panel}"
        );
    }
    // Past the cap, the tracks.
    right_at(&app, 420.0);
    assert_eq!(column(&app), MOST);
    let grown = 420.0 - FROM - (MOST - LEAST);
    assert_eq!(slider_parts(&app, ROUNDNESS_SLIDER), (MOST, track + grown));
    // As wide as the window lets it, a third of it.
    right_at(&app, W / 3.0);
    assert_eq!(column(&app), MOST);
    let grown = W / 3.0 - FROM - (MOST - LEAST);
    assert_eq!(slider_parts(&app, ROUNDNESS_SLIDER), (MOST, track + grown));
    // The Crop tab's rows with a control keep to the same column.
    app.set_panel_tab("Crop".into());
    settle(&app);
    let _ = crate::testing::by_id(&app, "CropSection::aspect-box");
    let aspect_words = |app: &App| {
        ElementHandle::find_by_element_id(app, "CropSection::aspect-words")
            .next()
            .expect("the Aspect label")
    };
    assert_eq!(aspect_words(&app).size().width, MOST);
    // Back to the default: as it was.
    right_at(&app, RIGHT);
    assert_eq!(column(&app), REST);
    assert_eq!(aspect_words(&app).size().width, REST);
}

/// "Roundness", the longest English slider label that the least
/// column cuts, is whole from a 324 px panel on, and so at the
/// panel's default: 62.6 px at the
/// panel's type in Noto Sans, the Linux default (Segoe UI and San
/// Francisco set it narrower), in a column 72 px wide less the gap.
#[test]
fn roundness_is_whole_from_a_324_px_panel() {
    const ROUNDNESS: f32 = 62.6;
    let app = loupe();
    let (label, _) = slider_parts(&app, ROUNDNESS_SLIDER);
    assert!(label - GAP >= ROUNDNESS, "whole at the default: {label}");
    right_at(&app, 320.0);
    let (label, _) = slider_parts(&app, ROUNDNESS_SLIDER);
    assert!(label - GAP < ROUNDNESS, "cut at 320");
    right_at(&app, 324.0);
    let (label, _) = slider_parts(&app, ROUNDNESS_SLIDER);
    assert!(label - GAP >= ROUNDNESS, "whole at 324: {label}");
}

/// A width kept from the last run sets the column with the panel,
/// before anything is drawn; a hidden panel leaves it at the least.
#[test]
fn a_kept_width_sets_the_column_at_the_start() {
    let app = loupe();
    let kept = Settings {
        right_width: Some(400.0),
        ..Settings::default()
    };
    show(&app, &kept);
    // The change handlers' one run, as before a frame.
    slint::platform::update_timers_and_animations();
    assert_eq!(app.global::<crate::LabelColumn>().get_width(), MOST);
    app.set_right_hidden(true);
    slint::platform::update_timers_and_animations();
    assert_eq!(app.global::<crate::LabelColumn>().get_width(), LEAST);
    app.set_right_hidden(false);
    slint::platform::update_timers_and_animations();
    assert_eq!(app.global::<crate::LabelColumn>().get_width(), MOST);
}

/// The vignette's Roundness slider's id.
const ROUNDNESS_SLIDER: &str = "VignetteSection::roundness-slider";

/// The export sheet keeps its own 68 px column, however wide the
/// panel.
#[test]
fn the_export_sheet_keeps_its_column() {
    let app = loupe();
    right_at(&app, 420.0);
    app.set_export_open(true);
    settle(&app);
    settle(&app);
    assert_eq!(
        slider_part(
            &app,
            "ExportSheet::quality-slider",
            "EditSlider::label-words"
        )
        .size()
        .width,
        LEAST - GAP
    );
    // The row's words, which its choice says to a screen reader.
    let format = ElementHandle::find_by_element_id(&app, "ExportSheet::format-words")
        .next()
        .expect("the Format label");
    assert_eq!(format.size().width, LEAST);
}
