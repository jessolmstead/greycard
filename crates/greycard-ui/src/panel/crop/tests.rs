use std::cell::RefCell;
use std::rc::Rc;

use super::{refit_crop, show_geometry};
use crate::testing::{drag, retouch_state, window};
use crate::{App, Crop};
use i_slint_backend_testing::ElementHandle;
use slint::ComponentHandle;

/// A source small enough that the whole of it, and the room
/// around it, sit inside a test window's viewport at zoom 1.
const SW: f32 = 400.0;
const SH: f32 = 300.0;

/// The viewport's own local position (logical pixels, no window
/// offset) of a point of the plane at fraction `f`: the inverse
/// of `view_to_plane` at zoom 1, center (0, 0). What the render
/// step works out for the overlay, and what a test works out by
/// hand for where to press.
fn local(app: &App, f: (f32, f32)) -> (f32, f32) {
    let scale = app.window().scale_factor();
    let (vw, vh) = (app.get_view_width() as f32, app.get_view_height() as f32);
    ((f.0 * SW + vw / 2.0) / scale, (f.1 * SH + vh / 2.0) / scale)
}

/// A window in crop mode, over a source of `SW` by `SH`, zoom
/// pinned at one so a test can work out the view's mapping by
/// hand, with a small crop already at the middle so there is
/// room outside it to press on. The overlay's own rectangle is
/// set to match by hand too, exactly as the render step would
/// have left it, since nothing here drives that step to run.
/// The viewport's own top left, in the window's own logical
/// pixels.
fn draw_setup(app: &App) -> (Rc<crate::Shared>, slint::LogicalPosition) {
    let (state, _worker) = retouch_state(app);
    {
        let mut st = state.borrow_mut();
        st.develop.source_size = (SW as u32, SH as u32);
        st.develop.zoom = 1.0;
    }
    app.set_crop_mode(true);
    app.set_geo_has_crop(true);
    app.set_geo_crop_x(0.4);
    app.set_geo_crop_y(0.4);
    app.set_geo_crop_w(0.2);
    app.set_geo_crop_h(0.2);
    let (l, t) = local(app, (0.4, 0.4));
    let (r, b) = local(app, (0.6, 0.6));
    app.set_crop_left(l);
    app.set_crop_top(t);
    app.set_crop_width(r - l);
    app.set_crop_height(b - t);
    let base = ElementHandle::find_by_element_type_name(app, "Viewport")
        .next()
        .expect("the viewport is on screen")
        .absolute_position();
    (state, base)
}

/// The window position (logical pixels) of a point of the plane
/// at fraction `f`: what a test presses or drags to, to land on a
/// fraction it chose.
fn at(app: &App, base: slint::LogicalPosition, f: (f32, f32)) -> (f32, f32) {
    let p = local(app, f);
    (base.x + p.0, base.y + p.1)
}

#[test]
fn a_drag_outside_the_crop_draws_a_new_one_with_the_right_fractions() {
    let app = window(1);
    let (_state, base) = draw_setup(&app);
    // Well clear of the crop at the middle (0.4 to 0.6 each way),
    // to its near corner and past its far one.
    let press = at(&app, base, (0.05, 0.05));
    let to = at(&app, base, (0.9, 0.85));
    drag(&app, &[press, to]);
    assert!(app.get_geo_has_crop());
    let got = (
        app.get_geo_crop_x(),
        app.get_geo_crop_y(),
        app.get_geo_crop_w(),
        app.get_geo_crop_h(),
    );
    let want = (0.05f32, 0.05f32, 0.85f32, 0.8f32);
    assert!(
        (got.0 - want.0).abs() < 5e-3
            && (got.1 - want.1).abs() < 5e-3
            && (got.2 - want.2).abs() < 5e-3
            && (got.3 - want.3).abs() < 5e-3,
        "{got:?} vs {want:?}"
    );
}

#[test]
fn an_aspect_set_holds_while_a_crop_is_drawn() {
    let app = window(1);
    let (_state, base) = draw_setup(&app);
    app.set_aspect_name("1:1".into());
    let press = at(&app, base, (0.05, 0.05));
    let to = at(&app, base, (0.09, 0.45));
    drag(&app, &[press, to]);
    assert!(app.get_geo_has_crop());
    let (w, h) = (app.get_geo_crop_w() * SW, app.get_geo_crop_h() * SH);
    assert!((w - h).abs() < 1.0, "{w} by {h} is not square");
}

#[test]
fn a_tiny_drag_outside_the_crop_changes_nothing() {
    let app = window(1);
    let (_state, base) = draw_setup(&app);
    let press = at(&app, base, (0.05, 0.05));
    let to = (press.0 + 2.0, press.1 + 1.0);
    drag(&app, &[press, to]);
    assert_eq!(app.get_geo_crop_x(), 0.4);
    assert_eq!(app.get_geo_crop_y(), 0.4);
    assert_eq!(app.get_geo_crop_w(), 0.2);
    assert_eq!(app.get_geo_crop_h(), 0.2);
}

#[test]
fn a_drag_inside_the_crop_still_moves_it() {
    let app = window(1);
    let (_state, base) = draw_setup(&app);
    let press = at(&app, base, (0.5, 0.5));
    let to = (press.0 + 40.0, press.1);
    drag(&app, &[press, to]);
    let scale = app.window().scale_factor();
    let want_x = 0.4 + 40.0 * scale / SW;
    assert!(
        (app.get_geo_crop_x() - want_x).abs() < 5e-3,
        "{}",
        app.get_geo_crop_x()
    );
    assert!((app.get_geo_crop_w() - 0.2).abs() < 5e-3);
    assert!((app.get_geo_crop_h() - 0.2).abs() < 5e-3);
}

#[test]
fn drawing_past_the_anchor_flips_the_rectangle() {
    let app = window(1);
    let (_state, base) = draw_setup(&app);
    let press = at(&app, base, (0.2, 0.2));
    let mid = at(&app, base, (0.35, 0.35));
    let past = at(&app, base, (0.05, 0.35));
    drag(&app, &[press, mid, past]);
    let got = (
        app.get_geo_crop_x(),
        app.get_geo_crop_y(),
        app.get_geo_crop_w(),
        app.get_geo_crop_h(),
    );
    // The pointer crossed back over the anchor's x: the anchor is
    // now the rectangle's right edge, not a wall it stopped at.
    let want = (0.05f32, 0.2f32, 0.15f32, 0.15f32);
    assert!(
        (got.0 - want.0).abs() < 5e-3
            && (got.1 - want.1).abs() < 5e-3
            && (got.2 - want.2).abs() < 5e-3
            && (got.3 - want.3).abs() < 5e-3,
        "{got:?} vs {want:?}"
    );
}

#[test]
fn a_press_off_the_picture_is_pulled_onto_it_not_wiped() {
    let app = window(1);
    let (_state, base) = draw_setup(&app);
    // Just past the top left corner of the plane: off the
    // picture, where a zero-size crop there used to fail to fit
    // and fall back to the whole frame, wiping the one set.
    let press = at(&app, base, (-0.05, -0.05));
    let to = at(&app, base, (0.5, 0.4));
    drag(&app, &[press, to]);
    assert!(app.get_geo_has_crop());
    let (w, h) = (app.get_geo_crop_w(), app.get_geo_crop_h());
    assert!(w > 0.3 && w < 0.6 && h > 0.2 && h < 0.5, "{w} by {h}");
}

#[test]
fn a_press_in_a_corner_a_turn_cut_away_is_pulled_onto_the_picture() {
    let app = window(1);
    let (_state, base) = draw_setup(&app);
    // A small straighten: a point just inside the plane's own
    // corner sits in a wedge the turn cut away, off the source
    // though it is within the unit square.
    app.set_geo_angle(3.0);
    let press = at(&app, base, (0.005, 0.005));
    let to = at(&app, base, (0.5, 0.4));
    drag(&app, &[press, to]);
    assert!(app.get_geo_has_crop());
    let (w, h) = (app.get_geo_crop_w(), app.get_geo_crop_h());
    assert!(w > 0.0 && h > 0.0 && w < 0.6 && h < 0.5, "{w} by {h}");
}

#[test]
fn a_plain_click_outside_the_crop_still_zooms() {
    let app = window(1);
    let (_state, base) = draw_setup(&app);
    let zoomed = Rc::new(RefCell::new(false));
    let counted = zoomed.clone();
    app.on_toggle_zoom(move |_, _| *counted.borrow_mut() = true);
    let press = at(&app, base, (0.05, 0.05));
    crate::testing::click(&app, press.0, press.1);
    assert!(*zoomed.borrow(), "a click under the threshold still zooms");
    // And the crop is untouched: a click drew nothing.
    assert_eq!(app.get_geo_crop_x(), 0.4);
    assert_eq!(app.get_geo_crop_w(), 0.2);
}

/// Under the crop tool a middle drag pans, from the crop's body as
/// from outside it: the crop is neither moved nor drawn anew, and
/// the tool stays up.
#[test]
fn a_middle_drag_pans_under_the_crop() {
    use slint::platform::PointerEventButton;
    let app = window(1);
    let (state, base) = draw_setup(&app);
    {
        let mut st = state.borrow_mut();
        st.develop.image_size = (SW as u32, SH as u32);
        st.develop.center = (SW / 2.0, SH / 2.0);
    }
    let middle = PointerEventButton::Middle;
    let center = || state.borrow().develop.center;
    for f in [(0.5, 0.5), (0.05, 0.05)] {
        let (x, y) = at(&app, base, f);
        let before = center();
        crate::testing::drag_with(&app, middle, &[(x, y), (x + 10.0, y), (x + 40.0, y + 20.0)]);
        assert_ne!(center(), before, "a middle drag from {f:?} panned");
        assert!(app.get_crop_mode(), "the crop is still up");
        assert_eq!(
            (app.get_geo_crop_x(), app.get_geo_crop_w()),
            (0.4, 0.2),
            "and untouched"
        );
        assert!(state.borrow().develop.crop_draw.is_none(), "nothing drawn");
    }
    // A left press while the middle pans outside the crop draws
    // nothing, its release or not.
    let (x, y) = at(&app, base, (0.05, 0.05));
    let p = |dx: f32| slint::LogicalPosition::new(x + dx, y);
    let left = PointerEventButton::Left;
    use slint::platform::WindowEvent;
    for event in [
        WindowEvent::PointerMoved { position: p(0.0) },
        WindowEvent::PointerPressed {
            position: p(0.0),
            button: middle,
        },
        WindowEvent::PointerMoved { position: p(10.0) },
        WindowEvent::PointerPressed {
            position: p(10.0),
            button: left,
        },
        WindowEvent::PointerMoved { position: p(60.0) },
        WindowEvent::PointerReleased {
            position: p(60.0),
            button: left,
        },
        WindowEvent::PointerReleased {
            position: p(60.0),
            button: middle,
        },
    ] {
        app.window().dispatch_event(event);
    }
    assert!(state.borrow().develop.crop_draw.is_none(), "nothing drawn");
    assert_eq!((app.get_geo_crop_x(), app.get_geo_crop_w()), (0.4, 0.2));
    // A middle click on the crop's body: 200% there.
    let (x, y) = at(&app, base, (0.5, 0.5));
    crate::testing::click_with(&app, middle, x, y);
    assert_eq!(state.borrow().develop.zoom, 2.0);
    assert_eq!(app.get_geo_crop_x(), 0.4, "the crop untouched");
}

/// A middle click during a left drag of the crop's body takes the
/// window's hold on the drag with it: the drag ends there, and the
/// next left drag of the body moves the crop again.
#[test]
fn a_middle_click_in_a_crop_drag_ends_it() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let app = window(1);
    let (state, base) = draw_setup(&app);
    let (left, middle) = (PointerEventButton::Left, PointerEventButton::Middle);
    let (x, y) = at(&app, base, (0.5, 0.5));
    let p = |dx: f32| slint::LogicalPosition::new(x + dx, y);
    for event in [
        WindowEvent::PointerMoved { position: p(0.0) },
        WindowEvent::PointerPressed {
            position: p(0.0),
            button: left,
        },
        WindowEvent::PointerMoved { position: p(20.0) },
        WindowEvent::PointerPressed {
            position: p(20.0),
            button: middle,
        },
        WindowEvent::PointerReleased {
            position: p(20.0),
            button: middle,
        },
    ] {
        app.window().dispatch_event(event);
    }
    assert!(state.borrow().develop.crop_drag.is_none(), "the drag ended");
    let moved = app.get_geo_crop_x();
    assert!(moved > 0.4, "it moved first: {moved}");
    // Off the window and released there: nothing reaches the body.
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: p(400.0),
        button: left,
    });
    let (x, y) = at(&app, base, (moved + 0.1, 0.5));
    crate::testing::drag(&app, &[(x, y), (x + 10.0, y), (x + 20.0, y)]);
    assert!(app.get_geo_crop_x() > moved, "the next drag moves it");
    assert!(state.borrow().develop.crop_drag.is_none());
}

#[test]
fn a_right_click_outside_the_crop_still_opens_the_frame_menu() {
    let app = window(1);
    let (_state, base) = draw_setup(&app);
    let press = at(&app, base, (0.05, 0.05));
    let position = slint::LogicalPosition::new(press.0, press.1);
    let button = slint::platform::PointerEventButton::Right;
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
    assert!(app.get_menu_up(), "a right click still opens the menu");
    // And drew nothing: it is not a draw gesture.
    assert_eq!(app.get_geo_crop_x(), 0.4);
}

#[test]
fn a_pointer_cancel_mid_draw_clears_the_anchor() {
    let app = window(1);
    let (state, base) = draw_setup(&app);
    let press = at(&app, base, (0.05, 0.05));
    let to = at(&app, base, (0.5, 0.4));
    let button = slint::platform::PointerEventButton::Left;
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(press.0, press.1),
        });
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerPressed {
            position: slint::LogicalPosition::new(press.0, press.1),
            button,
        });
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(to.0, to.1),
        });
    assert!(
        state.borrow().develop.crop_draw.is_some(),
        "the drag cleared the threshold and started a draw"
    );
    // The window lets go of the gesture instead of a release
    // ending it (focus lost, here simulated as the pointer
    // leaving the window mid-drag).
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerExited);
    assert!(
        state.borrow().develop.crop_draw.is_none(),
        "the cancel let go of the anchor"
    );
}

/// The crop set to `c` (fractions of the plane) and the overlay's
/// rectangle with it, as the render step would leave them: what
/// a drag on the picture then finds there.
fn crop_at(app: &App, c: [f32; 4]) {
    app.set_geo_has_crop(true);
    app.set_geo_crop_x(c[0]);
    app.set_geo_crop_y(c[1]);
    app.set_geo_crop_w(c[2]);
    app.set_geo_crop_h(c[3]);
    let (l, t) = local(app, (c[0], c[1]));
    let (r, b) = local(app, (c[0] + c[2], c[1] + c[3]));
    app.set_crop_left(l);
    app.set_crop_top(t);
    app.set_crop_width(r - l);
    app.set_crop_height(b - t);
}

fn crop_is(app: &App, want: [f32; 4]) {
    let got = [
        app.get_geo_crop_x(),
        app.get_geo_crop_y(),
        app.get_geo_crop_w(),
        app.get_geo_crop_h(),
    ];
    assert!(
        got.iter().zip(want).all(|(g, w)| (g - w).abs() < 5e-3),
        "{got:?} vs {want:?}"
    );
}

/// The crop at the whole frame, in crop mode, on the Crop tab so
/// the section's Draw button is there to press.
fn full_frame_setup(app: &App) -> slint::LogicalPosition {
    let (_state, base) = draw_setup(app);
    crop_at(app, [0.0, 0.0, 1.0, 1.0]);
    app.set_panel_tab("Crop".into());
    base
}

fn click_draw(app: &App) {
    let (at, size) = crate::testing::by_id(app, "CropSection::draw-button");
    crate::testing::click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
}

#[test]
fn a_drag_inside_a_whole_frame_crop_still_moves_it_and_draws_nothing() {
    let app = window(1);
    let base = full_frame_setup(&app);
    drag(
        &app,
        &[at(&app, base, (0.3, 0.3)), at(&app, base, (0.6, 0.5))],
    );
    // A move with nowhere to go: the crop stays the whole frame,
    // where a draw would have left the drag's own rectangle.
    crop_is(&app, [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn draw_then_a_drag_inside_the_crop_draws_a_fresh_one_once() {
    let app = window(1);
    let base = full_frame_setup(&app);
    click_draw(&app);
    assert!(app.get_crop_draw_armed(), "the button put Draw in hand");
    drag(
        &app,
        &[at(&app, base, (0.3, 0.3)), at(&app, base, (0.6, 0.5))],
    );
    crop_is(&app, [0.3, 0.3, 0.3, 0.2]);
    assert!(!app.get_crop_draw_armed(), "the release put it down");
    // One shot: the next drag inside moves the crop just drawn.
    crop_at(&app, [0.3, 0.3, 0.3, 0.2]);
    let press = at(&app, base, (0.45, 0.4));
    drag(&app, &[press, (press.0 + 40.0, press.1)]);
    let scale = app.window().scale_factor();
    crop_is(&app, [0.3 + 40.0 * scale / SW, 0.3, 0.3, 0.2]);
}

#[test]
fn draw_then_a_click_puts_it_down_and_leaves_the_crop() {
    let app = window(1);
    let base = full_frame_setup(&app);
    let zoomed = Rc::new(RefCell::new(false));
    let counted = zoomed.clone();
    app.on_toggle_zoom(move |_, _| *counted.borrow_mut() = true);
    click_draw(&app);
    let p = at(&app, base, (0.5, 0.5));
    crate::testing::click(&app, p.0, p.1);
    assert!(!app.get_crop_draw_armed());
    assert!(
        !*zoomed.borrow(),
        "the click that put Draw down did not zoom"
    );
    crop_is(&app, [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn escape_puts_draw_down() {
    let app = window(1);
    let base = full_frame_setup(&app);
    click_draw(&app);
    assert!(app.get_crop_draw_armed());
    // The keys are the window's again once the pointer is on the
    // picture: the button clicked last holds them until then, and
    // the disabled handles let the picture's area see the hover.
    let p = at(&app, base, (0.5, 0.5));
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(p.0, p.1),
        });
    crate::testing::press(&app, slint::platform::Key::Escape);
    assert!(!app.get_crop_draw_armed());
    assert!(app.get_crop_mode(), "Escape spent on Draw, not crop mode");
    drag(
        &app,
        &[at(&app, base, (0.3, 0.3)), at(&app, base, (0.6, 0.5))],
    );
    crop_is(&app, [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn leaving_crop_mode_puts_draw_down() {
    let app = window(1);
    let _base = full_frame_setup(&app);
    // By the section's own button.
    click_draw(&app);
    let (at, size) = crate::testing::by_id(&app, "CropSection::crop-done-button");
    crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert!(!app.get_crop_mode());
    assert!(!app.get_crop_draw_armed());
    // From outside the section (the tab, culling): put down by the
    // next event the window handles.
    click_draw(&app);
    assert!(app.get_crop_mode(), "Draw brings crop mode with it");
    assert!(app.get_crop_draw_armed());
    app.set_crop_mode(false);
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
    assert!(!app.get_crop_draw_armed());
}

#[test]
fn alt_and_a_drag_inside_the_crop_draws_without_the_button() {
    let app = window(1);
    let base = full_frame_setup(&app);
    let alt: slint::SharedString = slint::platform::Key::Alt.into();
    app.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: alt.clone() });
    drag(
        &app,
        &[at(&app, base, (0.3, 0.3)), at(&app, base, (0.6, 0.5))],
    );
    app.window()
        .dispatch_event(slint::platform::WindowEvent::KeyReleased { text: alt });
    crop_is(&app, [0.3, 0.3, 0.3, 0.2]);
    assert!(!app.get_crop_draw_armed());
    // Alt let go: a drag inside moves again.
    crop_at(&app, [0.3, 0.3, 0.3, 0.2]);
    let press = at(&app, base, (0.45, 0.4));
    drag(&app, &[press, (press.0 + 40.0, press.1)]);
    let scale = app.window().scale_factor();
    crop_is(&app, [0.3 + 40.0 * scale / SW, 0.3, 0.3, 0.2]);
}

#[test]
fn a_crop_drawn_with_draw_holds_the_aspect() {
    let app = window(1);
    let base = full_frame_setup(&app);
    app.set_aspect_name("3:2".into());
    click_draw(&app);
    drag(
        &app,
        &[at(&app, base, (0.2, 0.2)), at(&app, base, (0.5, 0.7))],
    );
    let (w, h) = (app.get_geo_crop_w() * SW, app.get_geo_crop_h() * SH);
    assert!(
        w < 0.99 * SW,
        "a fresh crop, not the whole frame: {w} by {h}"
    );
    assert!((w / h - 1.5).abs() < 0.02, "{w} by {h} is not 3:2");
}

/// The frame is measured as it is selected, culled or turned, so
/// an edit normally reaches the panel already brought up to
/// date. Where nothing can say which way up the frame is — no
/// develop yet, no thumbnail, a file the metadata read will not
/// answer for — it arrives still waiting, with a `portrait` flag
/// that means what version 3 meant by it. Nothing must refit the
/// crop to that reading: it stays exactly as it was written
/// until something can measure the frame.
#[test]
fn an_edit_still_waiting_on_its_frame_is_never_refitted() {
    let app = window(1);
    let (state, _worker) = retouch_state(&app);
    // The whole plane of an upright frame, under an aspect the
    // unmigrated flag reads as landscape.
    let stored = r#"{"version":3,"geometry":{"portrait":true,
            "aspect":{"kind":"original"},
            "crop":{"x":0.0,"y":0.0,"w":1.0,"h":1.0}}}"#;
    let waiting = greycard_edit::Edit::from_json(stored).expect("an older sidecar reads");
    assert!(waiting.needs_frame());
    state.borrow_mut().develop.source_size = (4000, 6000);

    // One move of the panel, as the Crop tab makes it: the edit
    // on show, the geometry on the controls, then whatever was
    // touched, then the callback the controls invoke.
    let moved = |edit: &greycard_edit::Edit, angle: f32, aspect: &str| {
        state.borrow_mut().develop.edit = edit.clone();
        show_geometry(&edit.geometry, &app);
        app.set_geo_angle(angle);
        app.set_aspect_name(aspect.into());
        app.invoke_geometry_changed();
        (app.get_geo_crop_w(), app.get_geo_crop_h())
    };

    // A square asked for is the change that bites: on a settled
    // edit it takes the crop down to the plane's width.
    assert_eq!(
        moved(&waiting, 0.0, "1:1"),
        (1.0, 1.0),
        "the aspect list re-cropped it"
    );
    // And the angle, which is the slider the bug was reported
    // through, leaves it alone too.
    assert_eq!(
        moved(&waiting, 3.0, "Original"),
        (1.0, 1.0),
        "the angle re-cropped it"
    );

    // The same edit once the frame has been measured: the flag
    // is brought up to date and the square is taken, which is
    // what says the callback and the refit were reached at all.
    let mut settled = waiting;
    settled.migrate_with_frame(true);
    assert!(!settled.needs_frame() && !settled.geometry.portrait);
    let (w, h) = moved(&settled, 0.0, "1:1");
    assert!(
        (w - 1.0).abs() < 1e-3 && (h - 4000.0 / 6000.0).abs() < 1e-3,
        "a square of an upright plane is {w} by {h} of it"
    );
}

/// A refit before the frame's size is known leaves the crop as it
/// is: the window is between a switch and the develop's landing.
#[test]
fn a_refit_on_an_unknown_frame_size_changes_nothing() {
    let app = window(1);
    let geometry = greycard_edit::geometry::Geometry {
        angle: 3.0,
        ..Default::default()
    };
    show_geometry(&geometry, &app);
    refit_crop(&app, 0.0, 0.0, true, None);
    assert!(!app.get_geo_has_crop(), "a crop was fitted into nothing");
    assert_eq!(app.get_crop_kept_area(), 0.0);
}

/// What the old build wrote for "pick Original, then Turn left"
/// on a landscape frame: turns 1, the flag ticked, the whole
/// plane cropped. It still renders on load, because the stored
/// crop fits; what would have re-cropped it is the next refit —
/// the aspect list, the Portrait toggle, the angle, either
/// perspective slider — which used to find the shape wrong and
/// quietly take 4000x2667 out of a 4000x6000 plane. Migrated,
/// the refit leaves it alone.
#[test]
fn a_turned_original_from_an_older_sidecar_is_not_re_cropped() {
    let app = window(1);
    let (sw, sh) = (6000.0f32, 4000.0f32);
    let mut edit = greycard_edit::Edit::from_json(
        r#"{"version":3,"geometry":{"turns":1,"portrait":true,
               "aspect":{"kind":"original"},
               "crop":{"x":0.0,"y":0.0,"w":1.0,"h":1.0}}}"#,
    )
    .expect("an older sidecar reads");
    assert!(edit.needs_frame(), "it waits for the frame");
    // The frame is landscape; its plane, turned once, is upright.
    edit.migrate_with_frame(false);
    assert!(!edit.geometry.portrait);

    show_geometry(&edit.geometry, &app);
    refit_crop(&app, sw, sh, true, None);
    let (w, h) = (app.get_geo_crop_w(), app.get_geo_crop_h());
    assert!(
        (w - 1.0).abs() < 1e-4 && (h - 1.0).abs() < 1e-4,
        "the whole plane became {w} by {h} of it"
    );
    // And it is still the plane's own shape, on end.
    let (pw, ph) = edit.geometry.plane_size(sw, sh);
    let ratio = edit
        .geometry
        .aspect
        .ratio(pw, ph, app.get_portrait())
        .unwrap();
    assert!((ratio - pw / ph).abs() < 1e-6 && ratio < 1.0, "{ratio}");
}

/// A frame shot on end, 5464 by 8192 on the screen: picking
/// "Original" from the aspect list has to crop it portrait. It
/// went straight to landscape, because the ratio was read as a
/// landscape one and turned by a flag that starts false.
#[test]
fn original_on_an_upright_frame_crops_it_upright() {
    let app = window(1);
    let (state, _worker) = retouch_state(&app);
    state.borrow_mut().develop.source_size = (5464, 8192);
    app.set_aspect_name("Original".into());
    assert!(!app.get_portrait());
    app.invoke_geometry_changed();
    assert!(app.get_geo_has_crop());
    let (sw, sh) = (5464.0, 8192.0);
    let (w, h) = (app.get_geo_crop_w() * sw, app.get_geo_crop_h() * sh);
    assert!(h > w, "{w} by {h} is not upright");
    // "Original" is the whole frame, so it is the whole frame.
    assert!((w - sw).abs() < 1.0 && (h - sh).abs() < 1.0, "{w} by {h}");
    // And the Portrait toggle still turns it, as it does a
    // named ratio.
    app.set_portrait(true);
    app.invoke_geometry_changed();
    let (w, h) = (app.get_geo_crop_w() * sw, app.get_geo_crop_h() * sh);
    assert!(w > h, "{w} by {h} is not on its side");
}

/// The crop as the panel holds it, in plane pixels of a source
/// `sw` by `sh` with no turn: left, top, width, height.
fn crop_px(app: &App, sw: f32, sh: f32) -> [f32; 4] {
    [
        app.get_geo_crop_x() * sw,
        app.get_geo_crop_y() * sh,
        app.get_geo_crop_w() * sw,
        app.get_geo_crop_h() * sh,
    ]
}

/// An aspect picked from the list, as the Crop tab picks it.
fn pick(app: &App, name: &str) {
    app.set_aspect_name(name.into());
    app.invoke_geometry_changed();
}

/// The Portrait switch, clicked where it is drawn.
fn click_portrait(app: &App) {
    crate::testing::click_switch_by_id(app, "CropSection::portrait-toggle");
}

/// A frame shot on end, 4000 by 6000 as it is developed: a named
/// ratio picked on it is tall, with the switch on to say so. It
/// went straight to landscape, the switch read false until it
/// was flipped by hand.
#[test]
fn a_ratio_picked_on_a_tall_frame_is_tall_with_the_switch_on() {
    let app = window(1);
    let (state, _worker) = retouch_state(&app);
    let (sw, sh) = (4000.0, 6000.0);
    state.borrow_mut().develop.source_size = (4000, 6000);
    app.set_panel_tab("Crop".into());
    assert!(!app.get_portrait());

    pick(&app, "3:2");
    assert!(app.get_portrait(), "the switch took the frame's way up");
    let [_, _, w, h] = crop_px(&app, sw, sh);
    assert!((h / w - 1.5).abs() < 1e-3, "{w} by {h} is not a tall 3:2");
    // A 3:2 frame on end is its own tall 3:2.
    assert!((w - sw).abs() < 1.0 && (h - sh).abs() < 1.0, "{w} by {h}");

    // Another ratio over it keeps the switch, either way it is.
    pick(&app, "4:3");
    assert!(app.get_portrait());
    let [_, _, w, h] = crop_px(&app, sw, sh);
    assert!((h / w - 4.0 / 3.0).abs() < 1e-3, "{w} by {h}");
    click_portrait(&app);
    assert!(!app.get_portrait(), "the click reached the switch");
    pick(&app, "16:9");
    assert!(!app.get_portrait(), "a ratio over a ratio kept the switch");
    let [_, _, w, h] = crop_px(&app, sw, sh);
    assert!((w / h - 16.0 / 9.0).abs() < 1e-3, "{w} by {h}");

    // "Original" is the frame's own shape whatever the switch
    // said, and reads tall; a ratio after it takes the frame's
    // way up again.
    click_portrait(&app);
    assert!(app.get_portrait());
    pick(&app, "Original");
    assert!(app.get_portrait_shown() && !app.get_portrait());
    let [_, _, w, h] = crop_px(&app, sw, sh);
    assert!(
        (h / w - 1.5).abs() < 1e-3,
        "{w} by {h} is not the frame's shape"
    );
    pick(&app, "5:4");
    assert!(app.get_portrait());

    // Reset puts the switch off and the aspect free.
    app.invoke_geometry_reset();
    assert!(!app.get_portrait() && app.get_aspect_name() == "Free");
    pick(&app, "1:1");
    assert!(app.get_portrait(), "after a reset a ratio is a fresh pick");

    // A landscape frame takes the switch off.
    let app = window(1);
    let (state, _worker) = retouch_state(&app);
    state.borrow_mut().develop.source_size = (6000, 4000);
    app.set_portrait(true);
    pick(&app, "3:2");
    assert!(!app.get_portrait());
}

/// Every toggle of the switch took the crop down to the last
/// one's shorter side, so the crop shrank toggle by toggle.
/// Toggled twice it is the crop it was; a tall crop of a wide
/// frame, which cannot be as large as the wide one, still turns
/// back to the wide one in full.
#[test]
fn toggling_the_switch_twice_gives_back_the_crop() {
    for (source, start) in [
        ((4000u32, 6000u32), None),
        ((6000, 4000), None),
        // A small crop drawn off the middle, that fits both ways.
        (
            (6000, 4000),
            Some(Crop {
                x: 0.1,
                y: 0.2,
                w: 0.3,
                h: 0.3,
            }),
        ),
    ] {
        let app = window(1);
        let (state, _worker) = retouch_state(&app);
        state.borrow_mut().develop.source_size = source;
        let (sw, sh) = (source.0 as f32, source.1 as f32);
        app.set_panel_tab("Crop".into());
        if let Some(c) = start {
            super::set_crop(&app, Some(c));
        }
        pick(&app, "3:2");
        let first = crop_px(&app, sw, sh);
        let area = first[2] * first[3];
        for round in 0..3 {
            click_portrait(&app);
            let turned = crop_px(&app, sw, sh);
            assert!(
                (turned[2] / turned[3] - first[3] / first[2]).abs() < 1e-3,
                "{source:?} round {round}: {turned:?} is not {first:?} turned"
            );
            assert!(
                turned[2] * turned[3] <= area * 1.0001,
                "{source:?} round {round}: grew"
            );
            click_portrait(&app);
            let back = [
                app.get_geo_crop_x(),
                app.get_geo_crop_y(),
                app.get_geo_crop_w(),
                app.get_geo_crop_h(),
            ];
            let want = [first[0] / sw, first[1] / sh, first[2] / sw, first[3] / sh];
            assert!(
                back.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-4),
                "{source:?} round {round}: {back:?} is not {want:?}"
            );
        }
    }

    // Where the turned shape fits, it keeps the area exactly:
    // a 3:2 crop at the middle of a 3:2 frame on end, half its
    // width, turned wide.
    let app = window(1);
    let (state, _worker) = retouch_state(&app);
    state.borrow_mut().develop.source_size = (4000, 6000);
    app.set_panel_tab("Crop".into());
    pick(&app, "3:2");
    super::set_crop(
        &app,
        Some(Crop {
            x: 0.25,
            y: 0.25,
            w: 0.5,
            h: 0.5,
        }),
    );
    click_portrait(&app);
    let [_, _, w, h] = crop_px(&app, 4000.0, 6000.0);
    assert!(
        (w - 3000.0).abs() < 1.0 && (h - 2000.0).abs() < 1.0,
        "{w} by {h}"
    );

    // And a turn of the picture still never grows it: the crop
    // left by the turn is no wider and no taller than before.
    let before = crop_px(&app, 4000.0, 6000.0);
    app.set_geo_angle(8.0);
    app.invoke_geometry_changed();
    let after = crop_px(&app, 4000.0, 6000.0);
    assert!(
        after[2] <= before[2] + 1e-2 && after[3] <= before[3] + 1e-2,
        "{after:?} grew past {before:?}"
    );
    app.set_geo_angle(0.0);
    app.invoke_geometry_changed();
    let level = crop_px(&app, 4000.0, 6000.0);
    assert!(
        level[2] <= after[2] + 1e-2 && level[3] <= after[3] + 1e-2,
        "leveling again grew it: {level:?} past {after:?}"
    );
}

/// A develop that lands tall, at the size the engine's own
/// orientation step gives a 6000 by 4000 sensor tagged
/// orientation 8, with 3:2 picked: the outline on the viewport
/// stands tall, the way the picture under it does.
#[test]
fn a_tall_develop_shows_a_tall_crop_outline_on_screen() {
    use greycard_core::develop::orient;
    use greycard_core::image::WorkingImage;
    use greycard_core::raw::Orientation;
    let app = window(4);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(4));
    app.invoke_select(1);
    // The engine's own orientation step, on a sensor of the
    // frame's shape, says what size the develop lands at.
    let tag = Orientation::from_exif(8).expect("8 is a tag");
    let upright = orient(WorkingImage::new(6, 4), tag);
    let size = (upright.width as u32 * 1000, upright.height as u32 * 1000);
    assert_eq!(size, (4000, 6000));
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(
        &app,
        crate::worker::Outcome::Developed {
            generation,
            turn: 0,
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
    // What the render does with a develop that has landed.
    {
        let mut st = state.borrow_mut();
        st.develop.pending.take().expect("the develop landed");
        st.develop.source_size = st.develop.shown_size;
        crate::panel::viewport::settle_source_size(&mut st);
        assert_eq!(st.develop.source_size, (4000, 6000));
    }
    app.set_crop_mode(true);
    app.set_panel_tab("Crop".into());
    pick(&app, "3:2");
    assert!(app.get_portrait());
    crate::panel::viewport::show_crop_on_view(&state.borrow(), &app, &super::read_geometry(&app));
    let (w, h) = (app.get_crop_width(), app.get_crop_height());
    assert!(
        w > 0.0 && (h / w - 1.5).abs() < 1e-2,
        "{w} by {h} on screen"
    );
    // Flipped, it lies on its side on the screen too.
    click_portrait(&app);
    crate::panel::viewport::show_crop_on_view(&state.borrow(), &app, &super::read_geometry(&app));
    let (w, h) = (app.get_crop_width(), app.get_crop_height());
    assert!(
        h > 0.0 && (w / h - 1.5).abs() < 1e-2,
        "{w} by {h} on screen"
    );
}

/// The Portrait switch says whether the crop is tall, under
/// "Original" too, where the stored flag means "the other way
/// from the frame": on a tall frame the two read opposite, and
/// the panel maps one to the other. Nothing stored changes
/// meaning, so an edit saved under "Original" with the flag on
/// opens as it was, wide, with the switch reading off.
#[test]
fn the_switch_reads_tall_under_original_as_under_a_ratio() {
    let app = window(1);
    let (state, _worker) = retouch_state(&app);
    let (sw, sh) = (4000.0, 6000.0);
    state.borrow_mut().develop.source_size = (4000, 6000);
    app.set_panel_tab("Crop".into());

    pick(&app, "Original");
    assert!(app.get_portrait_shown(), "a tall frame's own shape is tall");
    assert!(!app.get_portrait(), "and the stored flag is off for it");
    let [_, _, w, h] = crop_px(&app, sw, sh);
    assert!((w - sw).abs() < 1.0 && (h - sh).abs() < 1.0, "{w} by {h}");

    click_portrait(&app);
    assert!(!app.get_portrait_shown(), "the click turned the switch off");
    assert!(app.get_portrait(), "by turning the stored flag on");
    let [_, _, w, h] = crop_px(&app, sw, sh);
    assert!((w / h - 1.5).abs() < 1e-3, "{w} by {h} is not wide");

    // A named ratio on the same frame: tall is the flag on.
    pick(&app, "3:2");
    let [_, _, w, h] = crop_px(&app, sw, sh);
    assert!(h > w && app.get_portrait_shown() && app.get_portrait());

    // A sidecar saved under "Original", flag on: wide on a tall
    // frame, as it rendered when it was saved.
    let app = window(1);
    let (state, _worker) = retouch_state(&app);
    state.borrow_mut().develop.source_size = (4000, 6000);
    app.set_panel_tab("Crop".into());
    let stored = Crop {
        x: 0.0,
        y: (1.0 - 4000.0 / 1.5 / 6000.0) / 2.0,
        w: 1.0,
        h: 4000.0 / 1.5 / 6000.0,
    };
    let geometry = crate::Geometry {
        aspect: crate::Aspect::Original,
        portrait: true,
        crop: Some(stored),
        ..Default::default()
    };
    show_geometry(&geometry, &app);
    crate::panel::viewport::show_source_shape(&state.borrow(), &app);
    assert!(!app.get_portrait_shown() && app.get_portrait());
    assert_eq!(
        super::read_geometry(&app),
        geometry,
        "opening it changed it"
    );
    // A move that is not a pick leaves both alone.
    app.invoke_geometry_changed();
    assert_eq!(super::read_geometry(&app), geometry);
    assert!(!app.get_portrait_shown());
}

/// The custom box asks on every key. "16:" reads as no ratio at
/// all, and taken for a pick of Free it made the "16:9" a key
/// later a fresh pick, which set the switch to the frame's way up
/// over the one the user had set by hand.
#[test]
fn a_custom_ratio_typed_key_by_key_keeps_a_switch_set_by_hand() {
    let app = window(1);
    let (state, _worker) = retouch_state(&app);
    state.borrow_mut().develop.source_size = (4000, 6000);
    app.set_panel_tab("Crop".into());
    pick(&app, "3:2");
    assert!(app.get_portrait());
    click_portrait(&app);
    assert!(!app.get_portrait(), "set wide by hand");
    app.set_aspect_custom("".into());
    pick(&app, "Custom");
    for typed in ["1", "16", "16:", "16:9"] {
        app.set_aspect_custom(typed.into());
        app.invoke_geometry_changed();
        assert!(!app.get_portrait(), "{typed:?} set the switch aside");
    }
    let [_, _, w, h] = crop_px(&app, 4000.0, 6000.0);
    assert!((w / h - 16.0 / 9.0).abs() < 1e-3, "{w} by {h}");
}
/// The user's case: a frame upright from the camera's tag alone,
/// the aspect Free and no crop. Nothing reads the switch under
/// Free, so it is off and greyed, and a click changes neither the
/// flag nor the crop. A click used to flip a flag nothing read and
/// pin the whole frame as a crop, so the switch read on over the
/// same picture and the next click read off.
#[test]
fn the_switch_is_off_and_still_under_free() {
    let app = window(4);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(4));
    app.invoke_select(1);
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(
        &app,
        crate::worker::Outcome::Developed {
            generation,
            turn: 0,
            kind: crate::finish::Source::SCENE,
            image: crate::worker::Developed::Halves(std::sync::Arc::new(crate::worker::Halves {
                width: 4000,
                height: 6000,
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
    {
        let mut st = state.borrow_mut();
        st.develop.pending.take().expect("the develop landed");
        st.develop.source_size = st.develop.shown_size;
        crate::panel::viewport::settle_source_size(&mut st);
        crate::panel::viewport::show_source_shape(&st, &app);
    }
    app.set_crop_mode(true);
    app.set_panel_tab("Crop".into());
    assert_eq!(app.get_aspect_name(), "Free");
    assert!(!app.get_geo_has_crop());
    let before = super::read_geometry(&app);
    for _ in 0..2 {
        click_portrait(&app);
        assert_eq!(
            super::read_geometry(&app),
            before,
            "a click under Free moved it"
        );
        assert!(!app.get_portrait_shown());
        let g = super::read_geometry(&app);
        let c = g.effective_crop(4000.0, 6000.0, None);
        assert!(
            c.h * 6000.0 > c.w * 4000.0,
            "the whole tall frame went wide"
        );
    }
    assert!(!app.get_portrait_live(), "the switch is live under Free");
    // Live again under a ratio, and a click there is a click.
    pick(&app, "3:2");
    assert!(app.get_portrait_live() && app.get_portrait_shown());
    click_portrait(&app);
    assert!(!app.get_portrait_shown());
    let [_, _, w, h] = crop_px(&app, 4000.0, 6000.0);
    assert!((w / h - 1.5).abs() < 1e-3, "{w} by {h} is not wide");
    // A Crop-tab quarter turn lays the tall develop on its side
    // once: the develop does not carry the edit's turns, so the
    // plane is wide and "Original" on it reads off.
    app.invoke_geometry_turned(1);
    pick(&app, "Original");
    assert!(!app.get_plane_tall() && !app.get_portrait_shown());
    let g = super::read_geometry(&app);
    let c = g.effective_crop(4000.0, 6000.0, None);
    let (pw, ph) = g.plane_size(4000.0, 6000.0);
    assert!(
        c.w * pw > c.h * ph,
        "a turned tall frame's own shape is wide"
    );
}

/// A frame of the window's folder opened, as a click on its cell
/// opens it, with its develop landed: `sw` by `sh`, and the
/// crop mode as it was.
fn open_frame(app: &App, state: &Rc<crate::Shared>, i: i32, (sw, sh): (u32, u32)) {
    app.invoke_select(i);
    let mut st = state.borrow_mut();
    st.develop.source_size = (sw, sh);
    super::start_landed(&mut st, app);
}

/// The Crop button, as the Crop tab's own: crop mode on.
fn press_crop(app: &App) {
    app.set_crop_mode(true);
    app.invoke_crop_toggled();
}

/// The Done button: crop mode off.
fn press_done(app: &App) {
    app.set_crop_mode(false);
    app.invoke_crop_toggled();
}

/// The Settings sheet's Aspect row, chosen.
fn choose_start(app: &App, name: &str) {
    app.set_crop_start(name.into());
    app.invoke_crop_start_changed();
}

/// The crop's starting aspect, each of the Settings sheet's three:
/// a frame with no crop of its own opens its crop under it when
/// the Crop button is pressed, and a frame whose sidecar holds a
/// crop keeps that crop and its aspect whichever is chosen.
#[test]
fn the_crop_opens_under_the_starting_aspect() {
    let app = window(8);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(8));
    app.set_panel_tab("Crop".into());
    let kept = Crop {
        x: 0.1,
        y: 0.2,
        w: 0.5,
        h: 0.3,
    };
    {
        let mut st = state.borrow_mut();
        {
            let mut e = st.sidecars[7].current().clone();
            e.geometry.crop = Some(kept);
            st.sidecars[7].record(e);
        }
        {
            let mut e = st.sidecars[7].current().clone();
            e.geometry.aspect = crate::Aspect::Ratio { w: 16.0, h: 9.0 };
            st.sidecars[7].record(e);
        }
    }
    let wide = (6000, 4000);
    let tall = (4000, 6000);

    // Original, the default: the frame's own shape, the switch
    // off for it on a tall frame as on a wide one, and no crop
    // laid down until one is drawn.
    assert_eq!(app.get_crop_start(), "Original");
    open_frame(&app, &state, 0, wide);
    assert_eq!(
        app.get_aspect_name(),
        "Free",
        "nothing before the crop opens"
    );
    press_crop(&app);
    assert_eq!(app.get_aspect_name(), "Original");
    assert!(!app.get_portrait() && !app.get_geo_has_crop());
    // A corner dragged keeps the frame's shape.
    let g = super::read_geometry(&app);
    let (pw, ph) = g.plane_size(6000.0, 4000.0);
    assert_eq!(g.aspect.ratio(pw, ph, g.portrait), Some(1.5));
    press_done(&app);
    open_frame(&app, &state, 1, tall);
    press_crop(&app);
    assert_eq!(app.get_aspect_name(), "Original");
    assert!(!app.get_portrait());
    press_done(&app);

    // A frame cropped already keeps its own.
    open_frame(&app, &state, 7, wide);
    press_crop(&app);
    assert_eq!(app.get_aspect_name(), "16:9");
    assert_eq!(super::read_geometry(&app).crop, Some(kept));
    press_done(&app);

    // Free: as before the setting, any shape.
    choose_start(&app, "Free");
    open_frame(&app, &state, 2, wide);
    press_crop(&app);
    assert_eq!(app.get_aspect_name(), "Free");
    assert!(!app.get_geo_has_crop());

    // Last used: nothing picked yet is Free; a pick from the list
    // on one frame is where the next uncropped one opens, the
    // switch the way that frame stands.
    choose_start(&app, "Last used");
    open_frame(&app, &state, 3, wide);
    press_crop(&app);
    assert_eq!(app.get_aspect_name(), "Free");
    pick(&app, "3:2");
    press_done(&app);
    open_frame(&app, &state, 4, tall);
    press_crop(&app);
    assert_eq!(app.get_aspect_name(), "3:2");
    assert!(app.get_portrait(), "a tall frame's 3:2 is tall");
    // A custom ratio typed is a pick too.
    app.set_aspect_custom("7:5".into());
    pick(&app, "Custom");
    press_done(&app);
    open_frame(&app, &state, 5, wide);
    press_crop(&app);
    assert_eq!(
        (app.get_aspect_name(), app.get_aspect_custom()),
        ("Custom".into(), "7:5".into())
    );
    press_done(&app);
    // And the cropped frame still keeps its own.
    open_frame(&app, &state, 7, wide);
    press_crop(&app);
    assert_eq!(app.get_aspect_name(), "16:9");
    assert_eq!(super::read_geometry(&app).crop, Some(kept));
}

/// A frame opened with crop mode still on, moving through a shoot
/// cropping each: it takes the starting aspect once its develop
/// has said which way up it stands; and once only, so a Free
/// picked on it stands.
#[test]
fn a_frame_opened_while_cropping_opens_under_the_starting_aspect() {
    let app = window(3);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    app.set_panel_tab("Crop".into());
    choose_start(&app, "Last used");
    open_frame(&app, &state, 0, (6000, 4000));
    press_crop(&app);
    pick(&app, "1:1");
    open_frame(&app, &state, 1, (4000, 6000));
    assert!(app.get_crop_mode());
    assert_eq!(app.get_aspect_name(), "1:1");
    pick(&app, "Free");
    super::start_landed(&mut state.borrow_mut(), &app);
    assert_eq!(
        app.get_aspect_name(),
        "Free",
        "laid once, not on every develop"
    );
}

/// The choice and the last aspect picked are kept in the settings
/// file as they change, and a run started afresh on that file
/// opens the crop where the last one left off.
#[test]
fn the_starting_aspect_and_the_last_pick_outlive_the_session() {
    let dir = crate::testing::scratch_dir("crop-start");
    let file = dir.join("settings.json");
    {
        let app = window(2);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
        state.borrow_mut().window.settings_file = Some(file.clone());
        app.set_panel_tab("Crop".into());
        choose_start(&app, "Last used");
        open_frame(&app, &state, 0, (6000, 4000));
        press_crop(&app);
        pick(&app, "4:3");
    }
    let kept = crate::settings::Settings::load_from(&file);
    assert_eq!(
        (kept.crop_start.as_str(), kept.crop_last.as_str()),
        ("Last used", "4:3")
    );

    // The next run, as its launch reads the file.
    let app = window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    super::take_settings(&mut state.borrow_mut(), &kept);
    app.set_panel_tab("Crop".into());
    open_frame(&app, &state, 1, (6000, 4000));
    press_crop(&app);
    assert_eq!(app.get_aspect_name(), "4:3");
    assert!(!app.get_portrait());
    // The sheet shows the choice kept.
    app.invoke_settings_asked();
    assert_eq!(app.get_crop_start(), "Last used");
    crate::testing::remove_dir_retry(&dir);
}

/// The sidecars' rest, past the save's wait.
fn rest() {
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
        crate::panel::edit::SAVE_MS + 100,
    ));
}

/// The bottom right handle dragged up and in, as a hand on it.
fn drag_corner(app: &App) {
    app.invoke_crop_pressed(4);
    app.invoke_crop_dragged(4, -40.0, -20.0);
    app.invoke_crop_released();
}

/// The 3:2 frame an OM System body records of its 4:3 sensor, and
/// the band it is of a 5184 by 3888 develop.
fn camera_three_two() -> greycard_core::raw::CameraFrame {
    use greycard_core::raw::{CameraFrame, Orientation, Rect};
    CameraFrame {
        area: Rect {
            x: 8,
            y: 8,
            width: 5184,
            height: 3888,
        },
        frame: Rect {
            x: 8,
            y: 224,
            width: 5184,
            height: 3456,
        },
        orientation: Orientation::Normal,
    }
}

/// Frame `i` opened as its develop lands, the camera having framed
/// `camera` of it: the develop drawn is the frame's own.
fn open_framed(
    app: &App,
    state: &Rc<crate::Shared>,
    i: i32,
    size: (u32, u32),
    camera: greycard_core::raw::CameraFrame,
) {
    app.invoke_select(i);
    let mut st = state.borrow_mut();
    st.develop.source_size = size;
    st.develop.shown_size = size;
    st.develop.shown_turn = Some((i as usize, 0));
    st.develop.drawn_camera = Some((camera, camera.orientation));
    super::start_landed(&mut st, app);
}

fn ratio_of(c: &Crop, (w, h): (f32, f32)) -> f32 {
    (c.w * w) / (c.h * h)
}

/// A frame the camera framed at 3:2 of its 4:3 sensor, with no crop
/// of its own: the crop in force is the camera's frame; Crop opens
/// on it under Original, which holds its 3:2; Done untouched stores
/// nothing; a drag stores a 3:2 crop as any drag does; Reset is no
/// crop again, the camera's frame; and 4:3 picked reaches the whole
/// sensor, stored.
#[test]
fn a_frame_the_camera_framed_crops_from_the_cameras_frame() {
    let dir = crate::testing::scratch_dir("crop-camera-frame");
    let raw = dir.join("P0000001.ORF");
    std::fs::write(&raw, b"not a raw").unwrap();
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, vec![raw.clone()]);
    state.borrow_mut().window.write_sidecars = true;
    app.set_panel_tab("Crop".into());
    let size = (5184.0f32, 3888.0f32);
    open_framed(&app, &state, 0, (5184, 3888), camera_three_two());
    state.borrow_mut().develop.zoom = 1.0;
    let in_force = |state: &Rc<crate::Shared>| {
        let st = state.borrow();
        let camera = crate::panel::viewport::camera_crop(&st);
        super::read_geometry(&app).effective_crop(size.0, size.1, camera)
    };
    let band = in_force(&state);
    assert!((ratio_of(&band, size) - 1.5).abs() < 1e-3, "{band:?}");
    assert!(
        (band.y - 216.0 / 3888.0).abs() < 1e-5 && band.w == 1.0,
        "{band:?}"
    );
    press_crop(&app);
    assert_eq!(app.get_aspect_name(), "Original", "on the panel");
    assert_eq!(
        in_force(&state),
        band,
        "the box opens on the camera's frame"
    );
    rest();
    press_done(&app);
    rest();
    {
        let st = state.borrow();
        assert_eq!(
            st.develop.edit,
            greycard_edit::Edit::default(),
            "nothing stored"
        );
        assert!(st.sidecars[0].history.is_empty(), "no step");
        assert!(
            !crate::panel::cull::crop_stored(&st, 0),
            "stands in cut to the camera's frame"
        );
    }
    press_crop(&app);
    drag_corner(&app);
    rest();
    {
        let st = state.borrow();
        let geometry = &st.sidecars[0].current().geometry;
        let c = geometry.crop.expect("the drag made a crop");
        assert!(
            (ratio_of(&c, size) - 1.5).abs() < 1e-3,
            "Original holds the camera's 3:2: {c:?}"
        );
        assert!(
            c.y >= band.y - 1e-4 && c.y + c.h <= band.y + band.h + 1e-4,
            "{c:?}"
        );
    }
    // Reset: no crop, the camera's frame again.
    app.invoke_geometry_reset();
    rest();
    assert!(!app.get_geo_has_crop());
    assert_eq!(in_force(&state), band);
    // The whole sensor, by 4:3 picked: a crop stored, all of it.
    pick(&app, "4:3");
    rest();
    let st = state.borrow();
    let c = st.sidecars[0]
        .current()
        .geometry
        .crop
        .expect("4:3 stores a crop");
    assert!(c.x.abs() < 1e-4 && c.y.abs() < 1e-4, "{c:?}");
    assert!(
        (c.w - 1.0).abs() < 1e-4 && (c.h - 1.0).abs() < 1e-4,
        "{c:?}"
    );
    // A crop of its own: the camera's picture stands in whole.
    assert!(crate::panel::cull::crop_stored(&st, 0));
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// Original over no crop is the whole frame, so opening the crop
/// under it stores nothing: no edit, no step, no sidecar, until a
/// drag takes the aspect into the edit.
#[test]
fn opening_the_crop_under_original_stores_nothing() {
    let dir = crate::testing::scratch_dir("crop-original-nothing");
    let raw = dir.join("IMG_0001.CR3");
    std::fs::write(&raw, b"not a raw").unwrap();
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, vec![raw.clone()]);
    state.borrow_mut().window.write_sidecars = true;
    app.set_panel_tab("Crop".into());
    open_frame(&app, &state, 0, (6000, 4000));
    state.borrow_mut().develop.zoom = 1.0;
    press_crop(&app);
    assert_eq!(app.get_aspect_name(), "Original", "on the panel");
    rest();
    press_done(&app);
    rest();
    {
        let st = state.borrow();
        let stored = super::super::edit::read_edit(&app, &st.develop.edit, None);
        assert_eq!(stored, greycard_edit::Edit::default(), "the edit untouched");
        assert_eq!(st.develop.edit, greycard_edit::Edit::default());
        assert!(st.sidecars[0].history.is_empty(), "no step");
    }
    assert!(
        !greycard_edit::Sidecar::path_for(&raw).exists(),
        "no sidecar written"
    );
    // The first drag is the user's: the aspect goes in with it.
    press_crop(&app);
    drag_corner(&app);
    rest();
    let st = state.borrow();
    // As saved: the drag's step on the frame's line.
    let geometry = &st.sidecars[0].current().geometry;
    assert_eq!(geometry.aspect, crate::Aspect::Original);
    let c = geometry.crop.expect("the drag made a crop");
    assert!(
        ((c.w * 6000.0) / (c.h * 4000.0) - 1.5).abs() < 1e-3,
        "held to the frame's shape: {c:?}"
    );
    assert_eq!(st.sidecars[0].history.len(), 1);
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// Moving on to another frame with the crop open under Last used
/// at 1:1: the next frame shows 1:1 on the panel and is not
/// cropped until a drag there takes it.
#[test]
fn a_frame_arrived_at_while_cropping_is_not_cropped_until_touched() {
    let app = window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.set_panel_tab("Crop".into());
    choose_start(&app, "Last used");
    open_frame(&app, &state, 0, (6000, 4000));
    state.borrow_mut().develop.zoom = 1.0;
    press_crop(&app);
    pick(&app, "1:1");
    rest();
    open_frame(&app, &state, 1, (6000, 4000));
    state.borrow_mut().develop.zoom = 1.0;
    assert!(app.get_crop_mode());
    assert_eq!(app.get_aspect_name(), "1:1", "held on the panel");
    rest();
    {
        let st = state.borrow();
        let stored = super::super::edit::read_edit(&app, &st.develop.edit, None);
        assert_eq!(stored.geometry, crate::Geometry::default(), "not cropped");
        assert!(st.sidecars[1].history.is_empty());
    }
    // The box drawn is the held aspect's, as the list says: the
    // viewport draws the panel's geometry while it is held.
    assert!(app.get_aspect_held());
    let shown = super::read_geometry(&app).effective_crop(6000.0, 4000.0, None);
    assert!(
        ((shown.w * 6000.0) / (shown.h * 4000.0) - 1.0).abs() < 1e-3,
        "the box on show is a square: {shown:?}"
    );
    drag_corner(&app);
    rest();
    let st = state.borrow();
    let geometry = &st.sidecars[1].current().geometry;
    assert_eq!(geometry.aspect, crate::Aspect::Ratio { w: 1.0, h: 1.0 });
    let c = geometry.crop.expect("the drag made a crop");
    assert!(
        ((c.w * 6000.0) / (c.h * 4000.0) - 1.0).abs() < 1e-3,
        "a square: {c:?}"
    );
    assert_eq!(st.sidecars[1].history.len(), 1);
}

/// Straighten levels the frame through the level's own path: the
/// turn the found line asks, one step of history; a second press
/// on the leveled frame changes nothing, and undo takes it back.
#[test]
fn straighten_levels_the_dominant_line_once() {
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    let (w, h) = (600usize, 400usize);
    {
        let mut st = state.borrow_mut();
        st.develop.source_size = (w as u32, h as u32);
        st.develop.shown_size = (w as u32, h as u32);
        st.develop.shown_turn = Some((0, crate::panel::edit::current_turn(&st)));
    }
    // A horizon 3 degrees counter-clockwise, y down: dark below.
    let (s, c) = 3.0f32.to_radians().sin_cos();
    let picture: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32 - 300.0, (i / w) as f32 - 200.0);
            let v = 0.05 + 0.35 / (1.0 + (-(x * s + y * c) / 0.7).exp());
            [v; 3]
        })
        .collect();
    let wait = || {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
            crate::panel::edit::SAVE_MS + 100,
        ))
    };
    wait();
    let steps = state.borrow().sidecars[0].history.len();
    let press = || {
        super::STRAIGHTEN_PICTURE.with(|p| *p.borrow_mut() = Some((picture.clone(), w)));
        app.invoke_straightened();
        wait();
    };
    press();
    let first = app.get_geo_angle();
    assert!((first + 3.0).abs() < 0.1, "{first}");
    assert_eq!(state.borrow().sidecars[0].history.len(), steps + 1);
    press();
    assert!((app.get_geo_angle() - first).abs() < 0.1);
    assert_eq!(state.borrow().sidecars[0].history.len(), steps + 1);
    app.invoke_undo();
    assert_eq!(app.get_geo_angle(), 0.0);
    // A picture with no line in it: nothing moves.
    super::STRAIGHTEN_PICTURE.with(|p| *p.borrow_mut() = Some((vec![[0.2; 3]; w * h], w)));
    app.invoke_straightened();
    wait();
    assert_eq!(app.get_geo_angle(), 0.0);
}

/// Under a keystone a line's angle on the screen depends on where
/// it is: Straighten levels the line where it lies, a quarter of
/// the way down here, not a line through the middle.
#[test]
fn straighten_levels_the_line_where_it_lies() {
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    let (w, h) = (600usize, 400usize);
    {
        let mut st = state.borrow_mut();
        st.develop.source_size = (w as u32, h as u32);
        st.develop.shown_size = (w as u32, h as u32);
        st.develop.shown_turn = Some((0, crate::panel::edit::current_turn(&st)));
    }
    app.set_geo_horizontal(10.0);
    // A horizon 2 degrees counter-clockwise a quarter of the way down.
    let (s, c) = 2.0f32.to_radians().sin_cos();
    let picture: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32 - 300.0, (i / w) as f32 - 100.0);
            let v = 0.05 + 0.35 / (1.0 + (-(x * s + y * c) / 0.7).exp());
            [v; 3]
        })
        .collect();
    let press = || {
        super::STRAIGHTEN_PICTURE.with(|p| *p.borrow_mut() = Some((picture.clone(), w)));
        app.invoke_straightened();
    };
    press();
    let geometry = super::read_geometry(&app);
    let at = |x: f32| geometry.to_plane((300.0 + x * c, 100.0 - x * s), w as f32, h as f32);
    let (a, b) = (at(-150.0), at(150.0));
    let shown = (a.1 - b.1).atan2(b.0 - a.0).to_degrees();
    assert!(
        shown.abs() < 0.1,
        "{shown} on screen, at {}",
        app.get_geo_angle()
    );
    let first = app.get_geo_angle();
    press();
    assert!((app.get_geo_angle() - first).abs() < 0.1);
}

/// A crop the view's whole width has its side handles at the
/// view's edges, where the Hide panel arrows sit, and one its whole
/// height its bottom handle under the strip's: with the crop in
/// hand the arrows move off them, so a press there is the handle's,
/// and the arrow still works where it went.
#[test]
fn a_crops_edge_handles_are_not_under_the_edge_arrows() {
    let app = window(1);
    let (_state, _worker) = retouch_state(&app);
    let view = ElementHandle::find_by_element_type_name(&app, "Viewport")
        .next()
        .expect("the viewport is on screen");
    let (base, size) = (view.absolute_position(), view.size());
    app.set_crop_mode(true);
    app.set_crop_left(0.0);
    app.set_crop_top(size.height / 2.0 - 100.0);
    app.set_crop_width(size.width);
    app.set_crop_height(200.0);
    let pressed = Rc::new(RefCell::new(Vec::new()));
    {
        let pressed = pressed.clone();
        app.on_crop_pressed(move |i| pressed.borrow_mut().push(i));
    }
    let mid = base.y + size.height / 2.0;
    for (x, id) in [(base.x + size.width - 5.0, 3), (base.x + 5.0, 7)] {
        pressed.borrow_mut().clear();
        crate::testing::click(&app, x, mid);
        assert_eq!(*pressed.borrow(), [id], "the side handle at {x}");
    }
    assert!(!app.get_right_hidden());
    let (at, arrow) = crate::testing::by_id(&app, "App::right-arrow");
    crate::testing::click(&app, at.x + arrow.width / 2.0, at.y + arrow.height / 2.0);
    assert!(app.get_right_hidden(), "the arrow, moved, still works");
    // A crop the view's whole height: its bottom handle is under
    // the strip's arrow, which moves along the foot the same way.
    app.set_right_hidden(false);
    app.set_crop_left(size.width / 2.0 - 100.0);
    app.set_crop_top(0.0);
    app.set_crop_width(200.0);
    app.set_crop_height(size.height);
    pressed.borrow_mut().clear();
    crate::testing::click(&app, base.x + size.width / 2.0, base.y + size.height - 5.0);
    assert_eq!(*pressed.borrow(), [5], "the bottom handle");
    assert!(!app.get_strip_hidden());
    let (at, arrow) = crate::testing::by_id(&app, "App::strip-arrow");
    crate::testing::click(&app, at.x + arrow.width / 2.0, at.y + arrow.height / 2.0);
    assert!(
        app.get_strip_hidden(),
        "the strip's arrow, moved, still works"
    );
}

/// One frame of a 4:3 source, sidecars written, in the Crop tab
/// under the starting aspect `start` with `last` the last pick.
fn four_three(
    name: &str,
    start: &str,
    last: &str,
) -> (
    App,
    Rc<crate::Shared>,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let dir = crate::testing::scratch_dir(name);
    let raw = dir.join("P1010001.ORF");
    std::fs::write(&raw, b"not a raw").unwrap();
    let other = dir.join("P1010002.ORF");
    std::fs::write(&other, b"not a raw").unwrap();
    let app = window(2);
    let (state, _worker) = crate::testing::state_for(&app, vec![raw.clone(), other]);
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.develop.crop_last = last.to_string();
    }
    app.set_panel_tab("Crop".into());
    choose_start(&app, start);
    (app, state, raw, dir)
}

/// A 4:3 frame opened with the Crop button under Last used at 3:2,
/// then Done with nothing dragged: the edit stores the 3:2 crop the
/// box showed, so the screen, the export, the kept pictures, and
/// the export sheet's size are all 3:2. It stored the aspect alone,
/// which the screen framed and the export did not.
#[test]
fn a_ratio_opened_with_the_crop_button_stores_the_crop_it_shows() {
    let (app, state, raw, dir) = four_three("crop-taken-stored", "Last used", "3:2");
    open_frame(&app, &state, 0, (4000, 3000));
    press_crop(&app);
    assert_eq!(app.get_aspect_name(), "3:2");
    assert!(!app.get_aspect_held());
    press_done(&app);
    rest();
    {
        let st = state.borrow();
        let stored = &st.sidecars[0].current().geometry;
        assert_eq!(stored.aspect, crate::Aspect::Ratio { w: 3.0, h: 2.0 });
        let c = stored.crop.expect("the crop the box showed");
        assert!(
            ((c.w * 4000.0) / (c.h * 3000.0) - 1.5).abs() < 1e-3,
            "3:2: {c:?}"
        );
        assert_eq!(st.sidecars[0].history.len(), 1, "one step");
        // The screen draws the panel's edit's frame, as the render
        // step does, and the export sheet's size is the same frame.
        let shown = super::super::edit::read_edit(&app, &st.develop.edit, None)
            .geometry
            .frame(4000.0, 3000.0, None)
            .size;
        assert_eq!(shown, (4000.0, 2667.0));
        let export = greycard_engine::stages::framed_size((4000, 3000), stored, None);
        assert_eq!(export, (4000, 2667));
        let (tw, th) = greycard_engine::edited_pictures::fitted_size((4000, 3000), stored, None);
        assert!(
            (tw as f32 / th as f32 - 1.5).abs() < 0.01,
            "the kept picture: {tw}x{th}"
        );
    }
    let saved = greycard_edit::Sidecar::load_as_file(&raw)
        .unwrap()
        .expect("a sidecar written");
    assert_eq!(
        saved.current().geometry,
        state.borrow().sidecars[0].current().geometry
    );
    drop(app);
    crate::testing::remove_scratch(state, &dir);
}

/// The Crop button pressed under Last used at 3:2 before the
/// frame's develop has said its size, and Done pressed or not: the
/// crop is fitted when the develop lands, as it would have been,
/// and stored. A crop the user drew in the meantime stands.
#[test]
fn a_ratio_taken_before_the_develop_lands_is_fitted_when_it_does() {
    for done_first in [false, true] {
        let (app, state, _raw, dir) = four_three("crop-taken-early", "Last used", "3:2");
        app.invoke_select(0);
        assert_eq!(state.borrow().develop.source_size, (0, 0));
        press_crop(&app);
        assert_eq!(app.get_aspect_name(), "3:2");
        assert!(!app.get_geo_has_crop(), "no size to fit to yet");
        if done_first {
            press_done(&app);
        }
        {
            let mut st = state.borrow_mut();
            st.develop.source_size = (4000, 3000);
            super::start_landed(&mut st, &app);
        }
        assert!(app.get_geo_has_crop(), "done first {done_first}");
        rest();
        let st = state.borrow();
        let stored = &st.sidecars[0].current().geometry;
        let c = stored.crop.expect("the fitted crop stored");
        assert!(
            ((c.w * 4000.0) / (c.h * 3000.0) - 1.5).abs() < 1e-3,
            "3:2: {c:?}"
        );
        assert_eq!(st.sidecars[0].history.len(), 1, "one step");
        assert!(st.develop.crop_fit_due.is_none());
        drop(st);
        drop(app);
        crate::testing::remove_scratch(state, &dir);
    }
    // A crop drawn before the develop lands is the user's.
    let (app, state, _raw, dir) = four_three("crop-taken-early-drawn", "Last used", "3:2");
    app.invoke_select(0);
    press_crop(&app);
    let drawn = Crop {
        x: 0.1,
        y: 0.1,
        w: 0.3,
        h: 0.2,
    };
    super::set_crop(&app, Some(drawn));
    {
        let mut st = state.borrow_mut();
        st.develop.source_size = (4000, 3000);
        super::start_landed(&mut st, &app);
    }
    assert_eq!(super::read_geometry(&app).crop, Some(drawn));
    drop(app);
    crate::testing::remove_scratch(state, &dir);
}

/// A late fit that lands after the save's wait is a second step, and
/// both are named as the Crop button's one step is: "Crop", by what
/// moved, with no words of their own.
#[test]
fn a_late_fit_after_the_save_is_named_as_the_crop_buttons_step() {
    let named = |line: &greycard_edit::Sidecar| -> Vec<(Option<String>, String)> {
        let mut states: Vec<_> = line
            .history
            .iter()
            .map(|s| (s.label().map(String::from), s.edit().clone()))
            .collect();
        states.push((
            line.current_label().map(String::from),
            line.current().clone(),
        ));
        states
            .windows(2)
            .map(|w| {
                let label = w[1].0.clone();
                let said = greycard_edit::history::describe_step(
                    &w[0].1,
                    &w[1].1,
                    label.as_deref(),
                    &line.snapshots,
                );
                (label, said)
            })
            .collect()
    };
    // The Crop button with the size known: one step.
    let (app, state, _raw, dir) = four_three("crop-named-now", "Last used", "3:2");
    open_frame(&app, &state, 0, (4000, 3000));
    press_crop(&app);
    rest();
    let now = named(&state.borrow().sidecars[0]);
    assert_eq!(now, [(None, "Crop".to_string())]);
    drop(app);
    crate::testing::remove_scratch(state, &dir);

    // Before the develop, and it lands after the save: two steps,
    // each as that one.
    let (app, state, _raw, dir) = four_three("crop-named-late", "Last used", "3:2");
    app.invoke_select(0);
    press_crop(&app);
    rest();
    {
        let mut st = state.borrow_mut();
        st.develop.source_size = (4000, 3000);
        super::start_landed(&mut st, &app);
    }
    rest();
    let late = named(&state.borrow().sidecars[0]);
    assert_eq!(late, [now[0].clone(), now[0].clone()]);
    assert!(state.borrow().sidecars[0].current().geometry.crop.is_some());
    drop(app);
    crate::testing::remove_scratch(state, &dir);
}

/// The Crop button pressed on frame A before its develop lands, then
/// Cull entered, moved on to frame B, and left: B's develop landing
/// fits nothing onto B, which keeps no crop, no step and no sidecar.
/// B holds an aspect over no crop of its own (as 0.11.0 wrote one),
/// the edit a fit would take for A's.
#[test]
fn a_late_fit_due_on_one_frame_is_not_made_on_another() {
    let (app, state, raw, dir) = four_three("crop-late-other", "Last used", "3:2");
    let other = raw.with_file_name("P1010002.ORF");
    {
        let mut e = state.borrow_mut().sidecars[1].current().clone();
        e.geometry.aspect = crate::Aspect::Ratio { w: 3.0, h: 2.0 };
        state.borrow_mut().sidecars[1].record(e);
    }
    let steps = state.borrow().sidecars[1].history.len();
    app.invoke_select(0);
    press_crop(&app);
    assert!(state.borrow().develop.crop_fit_due.is_some());
    app.invoke_cull_toggled();
    app.invoke_select(1);
    app.invoke_cull_leave();
    assert_eq!(state.borrow().current, Some(1));
    {
        let mut st = state.borrow_mut();
        st.develop.source_size = (4000, 3000);
        super::start_landed(&mut st, &app);
    }
    rest();
    let st = state.borrow();
    assert!(st.develop.crop_fit_due.is_none(), "let go");
    assert!(!app.get_geo_has_crop());
    assert_eq!(st.sidecars[1].current().geometry.crop, None);
    assert_eq!(st.sidecars[1].history.len(), steps, "no step on B");
    assert!(!greycard_edit::Sidecar::path_for(&other).exists());
    drop(st);
    drop(app);
    crate::testing::remove_scratch(state, &dir);
}

/// Crop and Done under Original or Free, and on a frame arrived at
/// with the crop open under any start, store nothing: no step, no
/// sidecar, the whole frame on screen and in the export.
#[test]
fn crop_and_done_with_nothing_taken_stores_nothing() {
    for (start, arrived) in [
        ("Original", false),
        ("Original", true),
        ("Free", false),
        ("Free", true),
        ("Last used", true),
    ] {
        let (app, state, raw, dir) = four_three("crop-nothing-taken", start, "3:2");
        if arrived {
            open_frame(&app, &state, 1, (4000, 3000));
            press_crop(&app);
            open_frame(&app, &state, 0, (4000, 3000));
        } else {
            open_frame(&app, &state, 0, (4000, 3000));
            press_crop(&app);
        }
        press_done(&app);
        rest();
        {
            let st = state.borrow();
            let stored = super::super::edit::read_edit(&app, &st.develop.edit, None);
            assert_eq!(stored, greycard_edit::Edit::default(), "{start} {arrived}");
            assert!(st.sidecars[0].history.is_empty(), "{start} {arrived}");
            assert_eq!(
                stored.geometry.frame(4000.0, 3000.0, None).size,
                (4000.0, 3000.0),
                "{start} {arrived}"
            );
        }
        assert!(
            !greycard_edit::Sidecar::path_for(&raw).exists(),
            "{start} {arrived}: no sidecar"
        );
        drop(app);
        crate::testing::remove_scratch(state, &dir);
    }
}

/// A window on the Crop tab with a frame open, and what keeps it
/// wired.
fn crop_tab() -> (App, Rc<crate::Shared>, Rc<crate::worker::Worker>) {
    let app = window(1);
    let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    app.set_panel_tab("Crop".into());
    crate::testing::settle(&app);
    (app, state, worker)
}

/// How many alignment grids lie over the picture, once the layout
/// has caught up.
fn grids(app: &App) -> usize {
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
    ElementHandle::find_by_element_id(app, "Viewport::align-grid").count()
}

/// Mocked time `ms` on, the timers run every tenth of a second of
/// it, as a running window runs them.
fn wait(ms: u64) {
    for _ in 0..ms / 100 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(100));
        slint::platform::update_timers_and_animations();
    }
}

type Value = fn(&App) -> f32;

/// The panel slider with the element id `id` turned one notch of
/// the wheel up. A click on its track's middle first, where its
/// handle rests near zero, gives it the wheel and moves nothing.
fn wheel(app: &App, id: &str, value: Value) {
    use slint::platform::WindowEvent;
    let (at, size) = crate::testing::by_id(app, id);
    let from = at.x + app.global::<crate::LabelColumn>().get_width();
    let to = at.x + size.width - 60.0;
    let position = slint::LogicalPosition::new((from + to) / 2.0, at.y + size.height / 2.0);
    let before = value(app);
    crate::testing::click(app, position.x, position.y);
    assert_eq!(value(app), before, "{id}: the click moved it");
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position,
        delta_x: 0.0,
        delta_y: 120.0,
    });
    assert!(value(app) > before, "{id}: the wheel set it");
}

/// Each of the Crop tab's turn sliders, Angle, Vertical, and
/// Horizontal, lays the grid over the picture while the wheel sets
/// it, and the grid goes once the slider rests.
#[test]
fn a_turn_slider_set_lays_the_grid_until_it_rests() {
    let (app, _state, _worker) = crop_tab();
    let sliders = [
        ("RotateSection::angle", App::get_geo_angle as Value),
        ("PerspectiveSection::vertical", App::get_geo_vertical),
        ("PerspectiveSection::horizontal", App::get_geo_horizontal),
    ];
    for (id, value) in sliders {
        assert_eq!(grids(&app), 0, "{id}: no grid before the slider moves");
        wheel(&app, id, value);
        assert_eq!(grids(&app), 1, "{id}: the grid while it moves");
        // A whole interval with no step lets it go.
        wait(2500);
        assert_eq!(grids(&app), 0, "{id}: the grid goes when it rests");
        crate::testing::press(&app, slint::platform::Key::Escape);
    }
}

/// Vertical resting while Horizontal still moves leaves the grid
/// on; it goes when both rest.
#[test]
fn the_grid_stays_while_either_perspective_slider_moves() {
    let (app, _state, _worker) = crop_tab();
    wheel(&app, "PerspectiveSection::vertical", App::get_geo_vertical);
    wait(1500);
    wheel(
        &app,
        "PerspectiveSection::horizontal",
        App::get_geo_horizontal,
    );
    assert_eq!(grids(&app), 1);
    // Vertical has rested by now, Horizontal not yet.
    wait(1000);
    assert_eq!(grids(&app), 1, "the grid while Horizontal moves");
    wait(2500);
    assert_eq!(grids(&app), 0, "the grid goes when both rest");
}

/// A slider dropped while it moves, its timer with it, leaves no
/// grid behind: the Crop tab left, the panel put away, its own
/// section dragged to another place, or the sections' order set
/// anew (a reset), each before the slider rests.
#[test]
fn a_turn_slider_dropped_mid_move_leaves_no_grid() {
    // Each takes the name of the section that holds the slider.
    type Step = fn(&App, &str);
    let drag: Step = |app, section| {
        let order: Vec<String> = slint::Model::iter(&app.get_crop_order())
            .map(|n| n.to_string())
            .collect();
        let from = order.iter().position(|n| n == section).unwrap();
        // To the top, or from the top to the end.
        let slot = if from == 0 { order.len() } else { 0 };
        assert!(super::super::order::moved(
            app,
            "crop",
            from,
            slot,
            from + 1
        ));
    };
    let reset: Step = |app, _| {
        let mut order: Vec<slint::SharedString> =
            slint::Model::iter(&app.get_crop_order()).collect();
        order.reverse();
        app.set_crop_order(slint::ModelRc::new(slint::VecModel::from(order)));
    };
    // The last field: the viewport's own binding drops the grid
    // with no change handler run yet, as it does for the tab and
    // the panel.
    let leaves: [(&str, Step, Step, bool); 4] = [
        (
            "another tab",
            |app, _| app.set_panel_tab("Develop".into()),
            |app, _| app.set_panel_tab("Crop".into()),
            true,
        ),
        (
            "the panel put away",
            |app, _| app.set_right_hidden(true),
            |app, _| app.set_right_hidden(false),
            true,
        ),
        ("its section dragged", drag, |_, _| {}, false),
        ("the order set anew", reset, |_, _| {}, false),
    ];
    let (app, _state, _worker) = crop_tab();
    for (how, leave, back, bound) in leaves {
        for (id, value, section) in [
            (
                "RotateSection::angle",
                App::get_geo_angle as Value,
                "rotate",
            ),
            (
                "PerspectiveSection::vertical",
                App::get_geo_vertical,
                "perspective",
            ),
        ] {
            wheel(&app, id, value);
            assert_eq!(grids(&app), 1, "{how}, {id}: the grid while it moves");
            leave(&app, section);
            // Before any event runs the window's change handlers.
            if bound {
                assert_eq!(
                    ElementHandle::find_by_element_id(&app, "Viewport::align-grid").count(),
                    0,
                    "{how}, {id}: the grid before the handlers"
                );
            }
            crate::testing::settle(&app);
            assert_eq!(grids(&app), 0, "{how}, {id}: the grid at once");
            wait(2500);
            assert_eq!(grids(&app), 0, "{how}, {id}: the grid after");
            back(&app, section);
            crate::testing::settle(&app);
            assert_eq!(grids(&app), 0, "{how}, {id}: the grid back on Crop");
            assert!(
                !app.get_angle_moving() && !app.get_perspective_moving(),
                "{how}, {id}: a flag left on"
            );
            crate::testing::press(&app, slint::platform::Key::Escape);
        }
    }
}

/// A drag of the crop's bottom handle down past the strip's arrow
/// leaves the arrow where it was until the handle is let go: the
/// arrow steps off the handles by where the crop is, and read live
/// it hopped aside and back each time the moving edge crossed its
/// band. Once the handle is down, it steps off as before.
#[test]
fn the_strip_arrow_stays_put_while_a_crop_handle_is_held() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let app = window(1);
    let (_state, _worker) = retouch_state(&app);
    let view = ElementHandle::find_by_element_type_name(&app, "Viewport")
        .next()
        .expect("the viewport is on screen");
    let (base, size) = (view.absolute_position(), view.size());
    app.set_crop_mode(true);
    app.set_crop_left(size.width / 2.0 - 100.0);
    app.set_crop_top(0.0);
    app.set_crop_width(200.0);
    let start = size.height - 100.0;
    app.set_crop_height(start);
    // The crop follows the handle, as the render step would set it.
    {
        let weak = app.as_weak();
        app.on_crop_dragged(move |i, _dx, dy| {
            assert_eq!(i, 5, "the bottom handle is the one dragged");
            let app = weak.upgrade().unwrap();
            app.set_crop_height((start + dy).min(size.height));
        });
    }
    app.on_crop_pressed(|_| {});
    app.on_crop_released(|| {});
    let arrow_x = || {
        ElementHandle::find_by_element_id(&app, "App::strip-arrow")
            .next()
            .expect("the strip's arrow is on screen")
            .absolute_position()
            .x
    };
    let rest = arrow_x();
    let x = base.x + size.width / 2.0;
    let at = |y: f32| slint::LogicalPosition::new(x, base.y + y);
    let button = PointerEventButton::Left;
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: at(start),
    });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at(start),
        button,
    });
    // Down past the foot and back up over it, through the band where
    // the arrow steps aside, a few pixels at a time.
    let mut path: Vec<f32> = (0..=30).map(|k| start + 4.0 * k as f32).collect();
    path.extend((0..=20).map(|k| size.height + 20.0 - 4.0 * k as f32));
    path.extend((0..=10).map(|k| size.height - 60.0 + 6.0 * k as f32));
    for &y in &path {
        app.window()
            .dispatch_event(WindowEvent::PointerMoved { position: at(y) });
        assert_eq!(arrow_x(), rest, "the arrow held still at y {y}");
    }
    let last = *path.last().unwrap();
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at(last),
        button,
    });
    assert!(
        (app.get_crop_height() - size.height).abs() < 1.0,
        "the drag ended at the foot"
    );
    assert_ne!(arrow_x(), rest, "let go, the arrow steps off the handle");
}

/// The handles taken away mid-drag, by the camera's JPEG standing in
/// or by the Original key, end the drag as a release would: the
/// held handle is gone and hears no release, and the arrows were
/// left reading the crop at the press for good. With the handles
/// back, the strip's arrow steps off the crop as it stands.
#[test]
fn a_crop_drag_the_camera_jpeg_ends_lets_the_arrows_go() {
    handles_leave_mid_drag("placeholder");
}

/// [`a_crop_drag_the_camera_jpeg_ends_lets_the_arrows_go`] under the
/// Original key.
#[test]
fn a_crop_drag_the_original_ends_lets_the_arrows_go() {
    handles_leave_mid_drag("original");
}

fn handles_leave_mid_drag(trigger: &str) {
    use slint::platform::{PointerEventButton, WindowEvent};
    let app = window(1);
    let (_state, _worker) = retouch_state(&app);
    let view = ElementHandle::find_by_element_type_name(&app, "Viewport")
        .next()
        .expect("the viewport is on screen");
    let (base, size) = (view.absolute_position(), view.size());
    app.set_crop_mode(true);
    app.set_crop_left(size.width / 2.0 - 100.0);
    app.set_crop_top(0.0);
    app.set_crop_width(200.0);
    let start = size.height - 100.0;
    app.set_crop_height(start);
    let pressed = Rc::new(RefCell::new(0));
    let released = Rc::new(RefCell::new(0));
    {
        let pressed = pressed.clone();
        app.on_crop_pressed(move |_| *pressed.borrow_mut() += 1);
    }
    {
        let released = released.clone();
        app.on_crop_released(move || *released.borrow_mut() += 1);
    }
    app.on_crop_dragged(|_, _, _| {});
    let arrow_x = || {
        ElementHandle::find_by_element_id(&app, "App::strip-arrow")
            .next()
            .expect("the strip's arrow is on screen")
            .absolute_position()
            .x
    };
    let rest = arrow_x();
    let gone = |on: bool| match trigger {
        "placeholder" => app.set_placeholder(on),
        _ => app.set_original_on(on),
    };
    let at = slint::LogicalPosition::new(base.x + size.width / 2.0, base.y + start);
    let button = PointerEventButton::Left;
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button,
    });
    assert_eq!(*pressed.borrow(), 1, "{trigger}: the bottom handle held");
    gone(true);
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button,
    });
    gone(false);
    assert_eq!(
        *released.borrow(),
        1,
        "{trigger}: the drag ended once, with the handles"
    );
    // The crop's foot onto the strip's arrow: it steps aside.
    app.set_crop_height(size.height);
    assert_ne!(
        arrow_x(),
        rest,
        "{trigger}: the arrow reads the crop as it stands"
    );
}

/// A frame of 6000 by 4000 open and developed, its develop's retouch a
/// Fill at `at` (in the picture's widths), in crop mode.
fn filled_frame(at: [f32; 2]) -> (App, Rc<crate::Shared>, Rc<crate::worker::Worker>) {
    let app = window(1);
    let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.set_panel_tab("Crop".into());
    open_frame(&app, &state, 0, (6000, 4000));
    state
        .borrow_mut()
        .develop
        .edit
        .retouch
        .patches
        .push(greycard_edit::retouch::Patch {
            id: 1,
            method: greycard_edit::retouch::Method::Fill,
            points: vec![at],
            radius: 0.03,
            feather: 0.0,
            ..Default::default()
        });
    press_crop(&app);
    // Whatever opening the crop asked has gone out.
    develop_rest();
    (app, state, worker)
}

/// The develop's wait, past.
fn develop_rest() {
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
        crate::panel::edit::DEBOUNCE_MS + 50,
    ));
}

fn generation(state: &Rc<crate::Shared>) -> u64 {
    state.borrow().develop.generation
}

/// A crop released over a Fill's window asks for one develop, which
/// makes the Fill again with what the crop now hides left out of its
/// hole's surround, said as any develop is; the develop's geometry is
/// the panel's. Along the drag nothing is asked.
#[test]
fn a_crop_released_over_a_fill_develops_once() {
    let (app, state, _worker) = filled_frame([0.97, 0.3]);
    let before = generation(&state);
    app.invoke_crop_pressed(4);
    app.invoke_crop_dragged(4, -40.0, -20.0);
    app.invoke_crop_dragged(4, -60.0, -30.0);
    develop_rest();
    assert_eq!(generation(&state), before, "nothing along the drag");
    app.invoke_crop_released();
    develop_rest();
    assert_eq!(generation(&state), before + 1, "one develop at the release");
    assert_eq!(app.get_status(), "developing...");
    assert_eq!(
        state.borrow().develop.edit.geometry,
        super::read_geometry(&app),
        "the develop's geometry is the panel's"
    );
    // Asked again with nothing moved: nothing new to see.
    app.invoke_view_changed();
    develop_rest();
    assert_eq!(generation(&state), before + 1);
}

/// The Custom aspect typed with no Enter, then the crop dragged over a
/// Fill: the press takes no focus, but the typing is over
/// there, and the release asks for one develop; along the drag,
/// nothing.
#[test]
fn a_crop_dragged_after_an_aspect_typed_develops_at_the_release() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let (app, state, _worker) = filled_frame([0.97, 0.3]);
    app.set_aspect_name("Custom".into());
    crate::testing::settle(&app);
    let (field, fsize) = crate::testing::by_id(&app, "CropSection::custom-aspect-field");
    crate::testing::click(
        &app,
        field.x + fsize.width / 2.0,
        field.y + fsize.height / 2.0,
    );
    for _ in 0..8 {
        crate::testing::press(&app, slint::platform::Key::Backspace);
    }
    crate::testing::type_text(&app, "3:2");
    wait(600);
    let before = generation(&state);
    assert!(app.get_aspect_typing(), "typed, not accepted");

    // The crop's body, dragged.
    let base = ElementHandle::find_by_element_type_name(&app, "Viewport")
        .next()
        .expect("the viewport is on screen")
        .absolute_position();
    let x = base.x + app.get_crop_left() + app.get_crop_width() / 2.0;
    let y = base.y + app.get_crop_top() + app.get_crop_height() / 2.0;
    let p = |d: f32| slint::LogicalPosition::new(x + d, y);
    let button = PointerEventButton::Left;
    let crop = super::read_geometry(&app).crop;
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: p(0.0) });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: p(0.0),
        button,
    });
    for d in [10.0, 40.0, 80.0] {
        app.window()
            .dispatch_event(WindowEvent::PointerMoved { position: p(d) });
        wait(600);
        assert_eq!(generation(&state), before, "nothing along the drag");
    }
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: p(80.0),
        button,
    });
    assert_ne!(super::read_geometry(&app).crop, crop, "the drag moved it");
    assert!(!app.get_aspect_typing(), "the typing is over");
    wait(600);
    assert_eq!(generation(&state), before + 1, "one at the release");
}

/// A crop that reaches no Fill's window asks for nothing: the shader
/// draws the geometry, as before.
#[test]
fn a_crop_away_from_every_fill_develops_nothing() {
    let (app, state, _worker) = filled_frame([0.5, 0.3]);
    let before = generation(&state);
    drag_corner(&app);
    develop_rest();
    assert_eq!(generation(&state), before);
}

/// The geometry's sliders and the Custom aspect ask for a Fill's
/// develop once their change is over, never along it: the Angle held
/// and paused past the develop's wait asks nothing and its release one;
/// wheel steps a debounce apart ask one, after the last, once the
/// slider rests; the Custom aspect typed asks nothing until Enter, then
/// one. The Fill sits in the top right corner, which the fitted crop
/// of a turned picture leaves out.
#[test]
fn a_geometry_control_develops_a_fill_once_its_change_is_over() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let (app, state, _worker) = crop_tab();
    {
        let mut st = state.borrow_mut();
        st.develop.source_size = (6000, 4000);
        super::start_landed(&mut st, &app);
        st.develop
            .edit
            .retouch
            .patches
            .push(greycard_edit::retouch::Patch {
                id: 1,
                method: greycard_edit::retouch::Method::Fill,
                points: vec![[0.97, 0.02]],
                radius: 0.03,
                feather: 0.0,
                ..Default::default()
            });
    }
    let id = "RotateSection::angle";
    let (at, size) = crate::testing::by_id(&app, id);
    let from = at.x + app.global::<crate::LabelColumn>().get_width();
    let to = at.x + size.width - 60.0;
    let (x, y) = ((from + to) / 2.0, at.y + size.height / 2.0);
    let at = |x: f32| slint::LogicalPosition::new(x, y);
    let button = PointerEventButton::Left;
    let before = generation(&state);

    // Three wheel steps, each past the develop's wait: one, after the
    // slider rests.
    for step in 1..=3 {
        wheel(&app, id, App::get_geo_angle);
        wait(400);
        assert_eq!(generation(&state), before, "wheel step {step}: nothing yet");
    }
    wait(2500);
    assert_eq!(generation(&state), before + 1, "one once the wheel rests");

    // Held, moved, paused past the develop's wait, moved again.
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at(x) });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at(x),
        button,
    });
    let turned = app.get_geo_angle();
    for step in 1..=3 {
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: at(x + step as f32 * 4.0),
        });
        wait(600);
        assert_eq!(generation(&state), before + 1, "held, step {step}: nothing");
    }
    assert!(app.get_geo_angle() != turned, "the drag turned it");
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at(x + 12.0),
        button,
    });
    wait(600);
    assert_eq!(generation(&state), before + 2, "one at the release");

    // The Custom aspect, typed: nothing until Enter.
    app.set_aspect_name("Custom".into());
    crate::testing::settle(&app);
    let (field, fsize) = crate::testing::by_id(&app, "CropSection::custom-aspect-field");
    crate::testing::click(
        &app,
        field.x + fsize.width / 2.0,
        field.y + fsize.height / 2.0,
    );
    for _ in 0..8 {
        crate::testing::press(&app, slint::platform::Key::Backspace);
    }
    crate::testing::type_text(&app, "2:1");
    wait(600);
    assert_eq!(generation(&state), before + 2, "nothing while typed");
    crate::testing::press(&app, slint::platform::Key::Return);
    wait(600);
    assert_eq!(generation(&state), before + 3, "one at Enter");
}

/// An undo across a crop change over a Fill, as a preset's or a
/// paste's crop, goes through the same settling of the panel: it
/// develops where the Fill sees otherwise, and not where it does not.
#[test]
fn an_undo_across_a_crop_over_a_fill_develops() {
    let (app, state, worker) = filled_frame([0.97, 0.3]);
    let crop_at = |w: f32| greycard_edit::Geometry {
        crop: Some(Crop {
            x: 0.0,
            y: 0.0,
            w,
            h: 1.0,
        }),
        ..Default::default()
    };
    let settle_to = |g: greycard_edit::Geometry| {
        let mut st = state.borrow_mut();
        let mut edit = st.develop.edit.clone();
        edit.geometry = g;
        st.line_mut(0).expect("the open frame's line").record(edit);
        crate::panel::history::take_current_settled(&mut st, &app, &worker);
    };
    let before = generation(&state);
    settle_to(crop_at(0.95));
    assert_eq!(
        generation(&state),
        before + 1,
        "the crop hides the Fill's edge"
    );
    settle_to(crop_at(0.95));
    assert_eq!(generation(&state), before + 1, "the same crop again");
    // No crop: the outline no longer reaches the Fill, which sees
    // otherwise again.
    settle_to(greycard_edit::Geometry::default());
    assert_eq!(generation(&state), before + 2);
}
