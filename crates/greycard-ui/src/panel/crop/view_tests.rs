//! The view across a change of the picture's geometry: a frame zoomed
//! in keeps the picture point under the viewport's center and its zoom
//! while the Angle, the level, a keystone, the aspect, a quarter turn,
//! or a mirror changes the frame it is drawn in.

use std::rc::Rc;

use crate::App;
use crate::panel::viewport::OnShow;
use slint::ComponentHandle;

/// The frame's develop, as wide as a 24 MP raw's.
const SIZE: (u32, u32) = (6000, 4000);

/// A frame open and developed in a window the size of a laptop's,
/// laid out as the render lays it.
fn developed() -> (App, Rc<crate::Shared>, Rc<crate::worker::Worker>) {
    let app = crate::testing::window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    land(&app, &state, SIZE);
    state.borrow_mut().develop.base_white = Some(crate::worker::WhiteBase::IDENTITY);
    lay_out(&app, &state);
    (app, state, worker)
}

/// The open frame's develop of `size` landing.
fn land(app: &App, state: &Rc<crate::Shared>, size: (u32, u32)) {
    land_at(app, state, size, 0);
}

/// The open frame's develop of `size` landing, made at the frame's
/// own `turn`.
fn land_at(app: &App, state: &Rc<crate::Shared>, size: (u32, u32), turn: u8) {
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(
        app,
        crate::worker::Outcome::Developed {
            generation,
            turn,
            kind: crate::finish::Source::SCENE,
            image: crate::worker::Developed::Halves(std::sync::Arc::new(crate::worker::Halves {
                width: size.0,
                height: size.1,
                pixels: Vec::new(),
            })),
            guide: std::sync::Arc::new(crate::finish::Guide::NONE),
            planes: None,
            white: crate::worker::WhiteBase::IDENTITY,
            seconds: 0.1,
            sharpen: None,
            sources: Vec::new(),
            learned: crate::worker::LearnedReport::Off,
            fills: crate::worker::FillReport::default(),
        },
    );
}

/// The render's own layout of the frame on show, as each frame draws
/// it, without a GPU.
fn lay_out(app: &App, state: &Rc<crate::Shared>) -> OnShow {
    let mut st = state.borrow_mut();
    crate::panel::startup::land_pending(&mut st, app);
    let panel = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
    let edit = crate::panel::startup::view_edit(&st, &panel);
    crate::panel::viewport::frame_on_show(&mut st, app, &edit, &panel)
}

/// The zoom, and the source pixel under the viewport's center, once
/// the render has laid the frame out.
fn view(app: &App, state: &Rc<crate::Shared>) -> (f32, (f32, f32)) {
    let shown = lay_out(app, state);
    let st = state.borrow();
    let panel = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
    let geometry = crate::panel::startup::view_edit(&st, &panel).geometry;
    let (cx, cy) = st.develop.center;
    let plane = (shown.frame.origin.0 + cx, shown.frame.origin.1 + cy);
    let (sw, sh) = shown.source;
    (st.develop.zoom, geometry.to_source(plane, sw, sh))
}

/// Zoomed to `zoom` and panned to the source pixel `at`, which lies
/// off the middle, as a pan leaves the view.
fn zoom_to(app: &App, state: &Rc<crate::Shared>, zoom: f32, at: (f32, f32)) {
    let shown = lay_out(app, state);
    let mut st = state.borrow_mut();
    let panel = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
    let geometry = crate::panel::startup::view_edit(&st, &panel).geometry;
    let (sw, sh) = shown.source;
    let plane = geometry.to_plane(at, sw, sh);
    st.develop.zoom = zoom;
    st.develop.center = (
        plane.0 - shown.frame.origin.0,
        plane.1 - shown.frame.origin.1,
    );
}

/// The panel slider with the element id `id`, at its handle's middle.
fn slider_middle(app: &App, id: &str) -> (f32, f32, f32) {
    let (at, size) = crate::testing::by_id(app, id);
    let from = at.x + app.global::<crate::LabelColumn>().get_width();
    let to = at.x + size.width - 60.0;
    ((from + to) / 2.0, at.y + size.height / 2.0, to - from)
}

/// The slider pressed at its handle and dragged right a few steps,
/// each step laid out as the render would, as a hand drags it.
fn drag_slider(app: &App, state: &Rc<crate::Shared>, id: &str, value: fn(&App) -> f32) {
    use slint::platform::{PointerEventButton, WindowEvent};
    let (x, y, track) = slider_middle(app, id);
    let before = value(app);
    let at = |x: f32| slint::LogicalPosition::new(x, y);
    let button = PointerEventButton::Left;
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at(x) });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at(x),
        button,
    });
    for step in 1..=6 {
        let x = x + step as f32 * track / 150.0;
        app.window()
            .dispatch_event(WindowEvent::PointerMoved { position: at(x) });
        lay_out(app, state);
    }
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at(x + 6.0 * track / 150.0),
        button,
    });
    assert!(value(app) > before, "{id}: the drag set it");
}

/// The Angle's value field, typed into: `text` and Enter.
fn type_angle(app: &App, text: &str) {
    use i_slint_backend_testing::ElementHandle;
    let (at, size) = crate::testing::by_id(app, "RotateSection::angle");
    let field = ElementHandle::find_by_element_id(app, "EditSlider::field")
        .find(|f| {
            let p = f.absolute_position();
            p.y >= at.y && p.y <= at.y + size.height && p.x >= at.x && p.x <= at.x + size.width
        })
        .expect("the Angle's value field");
    let (p, s) = (field.absolute_position(), field.size());
    crate::testing::click(app, p.x + s.width / 2.0, p.y + s.height / 2.0);
    for _ in 0..8 {
        crate::testing::press(app, slint::platform::Key::Backspace);
    }
    crate::testing::type_text(app, text);
    crate::testing::press(app, slint::platform::Key::Return);
}

/// Each way a geometry is changed on the Crop tab.
#[derive(Clone, Copy, Debug)]
enum Act {
    Drag,
    Typed,
    Level,
    Vertical,
    Horizontal,
    Aspect,
    Turn,
    Flip,
}

const ACTS: [Act; 8] = [
    Act::Drag,
    Act::Typed,
    Act::Level,
    Act::Vertical,
    Act::Horizontal,
    Act::Aspect,
    Act::Turn,
    Act::Flip,
];

fn act(app: &App, state: &Rc<crate::Shared>, act: Act) {
    match act {
        Act::Drag => drag_slider(app, state, "RotateSection::angle", App::get_geo_angle),
        Act::Typed => {
            type_angle(app, "4");
            assert_eq!(app.get_geo_angle(), 4.0, "the angle typed");
        }
        // A line drawn 3° up from the left, leveled.
        Act::Level => {
            app.invoke_level_end(0.0, 0.0, 500.0, -500.0 * 3f32.to_radians().tan());
            assert!((app.get_geo_angle() + 3.0).abs() < 1e-3, "leveled");
        }
        Act::Vertical => drag_slider(
            app,
            state,
            "PerspectiveSection::vertical",
            App::get_geo_vertical,
        ),
        Act::Horizontal => drag_slider(
            app,
            state,
            "PerspectiveSection::horizontal",
            App::get_geo_horizontal,
        ),
        Act::Aspect => {
            app.set_aspect_name("1:1".into());
            app.invoke_geometry_changed();
        }
        Act::Turn => app.invoke_geometry_turned(1),
        Act::Flip => app.invoke_geometry_flipped(false),
    }
}

/// A source pixel well off the middle, and well inside the picture
/// under every act's geometry.
const AT: (f32, f32) = (3900.0, 1500.0);

fn close(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 1.0 && (a.1 - b.1).abs() < 1.0
}

/// The Crop tab, zoomed in and panned off the middle: each act keeps
/// the zoom and the picture point under the viewport's center, at
/// 1:1, 2:1 and 4:1, with the crop shown as the edit frames it and
/// with the Crop button's whole frame. It went back to the middle at
/// every change of the frame's size.
#[test]
fn a_geometry_changed_while_zoomed_keeps_the_point_in_the_middle() {
    for cropping in [false, true] {
        for zoom in [1.0, 2.0, 4.0] {
            for which in ACTS {
                let (app, state, _worker) = developed();
                app.set_panel_tab("Crop".into());
                if cropping {
                    app.set_crop_mode(true);
                    app.invoke_crop_toggled();
                }
                zoom_to(&app, &state, zoom, AT);
                let before = view(&app, &state);
                assert!(close(before.1, AT), "zoomed to {AT:?}: {before:?}");
                act(&app, &state, which);
                let after = view(&app, &state);
                assert_eq!(
                    after.0, zoom,
                    "{which:?} at {zoom} (cropping {cropping}): the zoom"
                );
                assert!(
                    close(after.1, AT),
                    "{which:?} at {zoom} (cropping {cropping}): {:?} under the middle, not {AT:?}",
                    after.1
                );
            }
        }
    }
}

/// Clicking the Crop tab changes nothing of the view; the Crop button
/// shows the whole frame, fitted, as the guide says.
#[test]
fn the_crop_tab_keeps_the_view_and_the_crop_button_fits() {
    let (app, state, _worker) = developed();
    zoom_to(&app, &state, 2.0, AT);
    app.set_panel_tab("Crop".into());
    app.invoke_tab_changed("Crop".into());
    let (zoom, at) = view(&app, &state);
    assert_eq!(zoom, 2.0);
    assert!(close(at, AT), "{at:?}");
    app.set_crop_mode(true);
    app.invoke_crop_toggled();
    assert_eq!(view(&app, &state).0, 0.0, "the Crop button fits");
}

/// A point the new frame no longer holds stays at its edge, not the
/// middle: a point at the right edge of a crop of the whole frame,
/// turned until the refit pulls the crop in past it.
#[test]
fn a_point_the_new_frame_leaves_out_is_held_at_its_edge() {
    let (app, state, _worker) = developed();
    app.set_panel_tab("Crop".into());
    // The whole frame cropped, as a drag of a corner to the edge
    // leaves it: a refit at any angle pulls it in from every side.
    crate::panel::crop::set_crop(
        &app,
        Some(crate::Crop {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        }),
    );
    app.invoke_view_changed();
    let near_edge = (5990.0, 2000.0);
    zoom_to(&app, &state, 2.0, near_edge);
    app.set_geo_angle(10.0);
    app.invoke_geometry_changed();
    let shown = lay_out(&app, &state);
    let st = state.borrow();
    let (cx, cy) = st.develop.center;
    assert_eq!(st.develop.zoom, 2.0);
    assert!(
        (cx - shown.frame.size.0).abs() < 1.0,
        "held at the right edge: {cx} of {}",
        shown.frame.size.0
    );
    assert!((0.0..=shown.frame.size.1).contains(&cy), "{cy}");
}

/// At fit nothing moves: a geometry changed on a fitted frame leaves
/// it fitted and in the middle.
#[test]
fn a_geometry_changed_at_fit_stays_fitted_in_the_middle() {
    for which in ACTS {
        let (app, state, _worker) = developed();
        app.set_panel_tab("Crop".into());
        act(&app, &state, which);
        let shown = lay_out(&app, &state);
        let st = state.borrow();
        assert_eq!(st.develop.zoom, 0.0, "{which:?}");
        let (cx, cy) = st.develop.center;
        assert!(
            (cx - shown.frame.size.0 / 2.0).abs() < 0.5
                && (cy - shown.frame.size.1 / 2.0).abs() < 0.5,
            "{which:?}: ({cx}, {cy}) in a frame of {:?}",
            shown.frame.size
        );
    }
}

/// A history row hovered while zoomed shows its own geometry about
/// the same point, and the view after it is the view before: the
/// frame laid out under the row's geometry is not read as a change of
/// the panel's.
#[test]
fn a_peek_at_another_geometry_gives_the_view_back() {
    let (app, state, _worker) = developed();
    zoom_to(&app, &state, 2.0, AT);
    let before = view(&app, &state);
    let mut turned = {
        let st = state.borrow();
        crate::panel::edit::read_edit(&app, &st.develop.edit, st.develop.target)
    };
    turned.geometry.angle = 8.0;
    crate::panel::history::peek(&mut state.borrow_mut(), &app, Some(turned), "a step");
    let peeked = view(&app, &state);
    assert!(
        close(peeked.1, AT),
        "the row about the same point: {peeked:?}"
    );
    crate::panel::history::peek(&mut state.borrow_mut(), &app, None, "");
    let after = view(&app, &state);
    assert_eq!(after.0, before.0);
    assert!(close(after.1, AT), "the view back: {after:?}");
}

/// A develop of the same size landing, as a lens correction switched
/// on or off lands, leaves a zoomed view where it was.
#[test]
fn a_develop_of_the_same_size_keeps_the_view() {
    let (app, state, _worker) = developed();
    zoom_to(&app, &state, 4.0, AT);
    let center = {
        lay_out(&app, &state);
        state.borrow().develop.center
    };
    land(&app, &state, SIZE);
    lay_out(&app, &state);
    let st = state.borrow();
    assert_eq!((st.develop.zoom, st.develop.center), (4.0, center));
}

/// Whether the view's center is the middle of the frame laid out.
fn in_the_middle(state: &Rc<crate::Shared>, shown: &OnShow) -> bool {
    let (cx, cy) = state.borrow().develop.center;
    (cx - shown.frame.size.0 / 2.0).abs() < 0.5 && (cy - shown.frame.size.1 / 2.0).abs() < 0.5
}

/// At fit, a crop in a corner turned until it refits: the picture
/// fitted stays in the middle of the new frame, not carried to the
/// corner's point.
#[test]
fn a_corner_crop_turned_at_fit_stays_in_the_middle() {
    let (app, state, _worker) = developed();
    app.set_panel_tab("Crop".into());
    crate::panel::crop::set_crop(
        &app,
        Some(crate::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.3,
            h: 0.3,
        }),
    );
    app.invoke_view_changed();
    lay_out(&app, &state);
    app.set_geo_angle(20.0);
    app.invoke_geometry_changed();
    let shown = lay_out(&app, &state);
    assert_eq!(state.borrow().develop.zoom, 0.0);
    assert!(
        in_the_middle(&state, &shown),
        "{:?} in {:?}",
        state.borrow().develop.center,
        shown.frame.size
    );
}

/// Mocked time past the save's interval, so the next change is a step
/// of its own in the history.
fn past_the_save() {
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
        crate::panel::edit::SAVE_MS + 100,
    ));
}

/// Undo and redo across an angle change, zoomed in: the same point
/// stays in the middle both ways.
#[test]
fn undo_and_redo_of_an_angle_keep_the_point() {
    let (app, state, _worker) = developed();
    app.set_panel_tab("Crop".into());
    past_the_save();
    app.set_geo_angle(10.0);
    app.invoke_geometry_changed();
    past_the_save();
    lay_out(&app, &state);
    zoom_to(&app, &state, 2.0, AT);
    assert!(close(view(&app, &state).1, AT));
    app.invoke_undo();
    assert_eq!(app.get_geo_angle(), 0.0, "undone");
    let after = view(&app, &state);
    assert_eq!(after.0, 2.0);
    assert!(close(after.1, AT), "undo: {after:?}");
    app.invoke_redo();
    assert_eq!(app.get_geo_angle(), 10.0, "redone");
    let after = view(&app, &state);
    assert!(close(after.1, AT), "redo: {after:?}");
}

/// Two frames of one size, the second under another geometry, shown
/// with the zoom and the view's size left as they were: the view is
/// laid out as a new frame's, never carried through the first frame's
/// geometry.
#[test]
fn another_frame_is_never_carried_through_the_last_ones_geometry() {
    let app = crate::testing::window(2);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.invoke_select(0);
    land(&app, &state, SIZE);
    state.borrow_mut().develop.base_white = Some(crate::worker::WhiteBase::IDENTITY);
    lay_out(&app, &state);
    zoom_to(&app, &state, 2.0, AT);
    lay_out(&app, &state);
    let kept = {
        let st = state.borrow();
        (st.develop.image_size, st.develop.center)
    };
    app.invoke_select(1);
    land(&app, &state, SIZE);
    app.set_geo_angle(10.0);
    app.invoke_geometry_changed();
    {
        // A way to the frame that leaves the zoom and the size set.
        let mut st = state.borrow_mut();
        st.develop.zoom = 2.0;
        (st.develop.image_size, st.develop.center) = kept;
    }
    let shown = lay_out(&app, &state);
    assert_eq!(
        state.borrow().develop.shown_turn.map(|s| s.0),
        Some(1),
        "the second's develop is drawn"
    );
    assert!(
        in_the_middle(&state, &shown),
        "centered as a new frame: {:?} in {:?}",
        state.borrow().develop.center,
        shown.frame.size
    );
}

/// `p`, a point of the develop at no turn of its own, on the develop
/// turned `quarters` quarter turns clockwise, as the frame's own Turn
/// right turns it.
fn turned_at((x, y): (f32, f32), quarters: i32) -> (f32, f32) {
    let (w, h) = (SIZE.0 as f32, SIZE.1 as f32);
    match quarters.rem_euclid(4) {
        1 => (h - y, x),
        2 => (w - x, h - y),
        3 => (y, w - x),
        _ => (x, y),
    }
}

/// What the frame carries before it is zoomed into and turned.
#[derive(Clone, Copy, Debug)]
enum Under {
    Nothing,
    /// A crop about the point, small enough that a turn the wrong way
    /// round would leave the point out of it.
    Crop,
    Angle,
    /// Mirrored, with a keystone: an odd turn of the frame takes the
    /// mirrored edit a half turn more (`under_turned_source`).
    MirrorKeystone,
    /// The edit's own quarter turn under the frame's.
    EditTurn,
}

const UNDER: [Under; 5] = [
    Under::Nothing,
    Under::Crop,
    Under::Angle,
    Under::MirrorKeystone,
    Under::EditTurn,
];

fn put_under(app: &App, state: &Rc<crate::Shared>, under: Under) {
    match under {
        Under::Nothing => {}
        Under::Crop => {
            crate::panel::crop::set_crop(
                app,
                Some(crate::Crop {
                    x: 0.55,
                    y: 0.25,
                    w: 0.2,
                    h: 0.25,
                }),
            );
            app.invoke_view_changed();
        }
        Under::Angle => {
            app.set_geo_angle(10.0);
            app.invoke_geometry_changed();
        }
        Under::MirrorKeystone => {
            app.invoke_geometry_flipped(false);
            app.set_geo_vertical(20.0);
            app.invoke_geometry_changed();
            assert!(app.get_geo_flip(), "mirrored");
        }
        Under::EditTurn => {
            app.invoke_geometry_turned(1);
            assert_eq!(app.get_geo_turns(), 1, "the edit turned");
        }
    }
    lay_out(app, state);
}

/// The frame's own Turn left and Turn right (`Sidecar::turn`, not the
/// edit's), zoomed in: the picture turns about the view's center, so
/// the same picture point stays there at the same zoom, while the
/// develop at the old turn is drawn turned and once the develop at the
/// new one lands; and so across two turns, over a crop and an angle.
/// It went back to the middle of the frame.
#[test]
fn the_frames_own_turn_while_zoomed_keeps_the_point_in_the_middle() {
    // Each case: the turns pressed, and whether the develop lands
    // after each or only after the last.
    let cases: [(&[i32], bool); 5] = [
        (&[1], false),
        (&[-1], false),
        (&[1, 1], false),
        (&[1, 1], true),
        (&[-1, 1, 1], false),
    ];
    for under in UNDER {
        for (turns, each) in cases {
            let (app, state, _worker) = developed();
            put_under(&app, &state, under);
            zoom_to(&app, &state, 2.0, AT);
            assert!(close(view(&app, &state).1, AT), "{under:?}: zoomed");
            let mut total = 0;
            for (i, &q) in turns.iter().enumerate() {
                app.invoke_frame_turned(q);
                total += q;
                let turn = total.rem_euclid(4) as u8;
                assert_eq!(state.borrow().sidecars[0].turn, turn, "turned");
                let want = turned_at(AT, total);
                let what = format!("{under:?}, {turns:?} to {i}");
                let drawn = view(&app, &state);
                assert_eq!(drawn.0, 2.0, "{what}: the zoom");
                assert!(
                    close(drawn.1, want),
                    "{what}: {:?} under the middle, not {want:?}",
                    drawn.1
                );
                if each || i + 1 == turns.len() {
                    let size = if turn % 2 == 1 {
                        (SIZE.1, SIZE.0)
                    } else {
                        SIZE
                    };
                    land_at(&app, &state, size, turn);
                    let landed = view(&app, &state);
                    assert_eq!(landed.0, 2.0, "{what}, landed: the zoom");
                    assert!(
                        close(landed.1, want),
                        "{what}, landed: {:?} under the middle, not {want:?}",
                        landed.1
                    );
                }
            }
        }
    }
}

/// A point near the frame's edge, turned: held where it is as usual,
/// not thrown to the middle.
#[test]
fn the_frames_own_turn_near_an_edge_keeps_the_point() {
    let (app, state, _worker) = developed();
    let near_edge = (5990.0, 2000.0);
    zoom_to(&app, &state, 2.0, near_edge);
    app.invoke_frame_turned(1);
    let want = turned_at(near_edge, 1);
    let shown = lay_out(&app, &state);
    let st = state.borrow();
    assert_eq!(st.develop.zoom, 2.0);
    let (cx, cy) = st.develop.center;
    // The right edge turned clockwise is the bottom.
    assert!(
        (cx - want.0).abs() < 1.0 && (cy - want.1).abs() < 1.0,
        "({cx}, {cy}), not {want:?}, in {:?}",
        shown.frame.size
    );
}

/// An angle undone after the frame's own turn, zoomed in, keeps the
/// point too: the history's states turned with the frame.
#[test]
fn undo_after_the_frames_own_turn_keeps_the_point() {
    let (app, state, _worker) = developed();
    app.set_panel_tab("Crop".into());
    past_the_save();
    app.set_geo_angle(10.0);
    app.invoke_geometry_changed();
    past_the_save();
    lay_out(&app, &state);
    zoom_to(&app, &state, 2.0, AT);
    app.invoke_frame_turned(1);
    let want = turned_at(AT, 1);
    assert!(close(view(&app, &state).1, want), "turned");
    app.invoke_undo();
    assert_eq!(app.get_geo_angle(), 0.0, "undone");
    let after = view(&app, &state);
    assert_eq!(after.0, 2.0);
    assert!(close(after.1, want), "undo: {after:?}, not {want:?}");
}

/// At fit, the frame's own turn leaves the frame fitted and in the
/// middle.
#[test]
fn the_frames_own_turn_at_fit_stays_fitted_in_the_middle() {
    for under in UNDER {
        for q in [1, -1] {
            let (app, state, _worker) = developed();
            put_under(&app, &state, under);
            app.invoke_frame_turned(q);
            let shown = lay_out(&app, &state);
            assert_eq!(state.borrow().develop.zoom, 0.0, "{under:?} {q}");
            assert!(
                in_the_middle(&state, &shown),
                "{under:?} {q}: {:?} in {:?}",
                state.borrow().develop.center,
                shown.frame.size
            );
        }
    }
}
