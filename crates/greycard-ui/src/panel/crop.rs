use crate::panel::viewport::{effective_zoom, view_to_plane, view_to_source};
use crate::*;

/// Put a geometry on the panel.
pub(crate) fn show_geometry(geometry: &Geometry, app: &App) {
    app.set_geo_turns(geometry.turns as i32);
    app.set_geo_flip(geometry.flip);
    app.set_geo_angle(geometry.angle);
    app.set_geo_vertical(geometry.vertical);
    app.set_geo_horizontal(geometry.horizontal);
    set_crop(app, geometry.crop);
    let (name, custom) = geometry.aspect.name();
    app.set_aspect_name(name.into());
    if let Some(custom) = custom {
        app.set_aspect_custom(custom.into());
    }
    app.set_portrait(geometry.portrait);
}

pub(crate) fn set_crop(app: &App, crop: Option<Crop>) {
    app.set_geo_has_crop(crop.is_some());
    if let Some(c) = crop {
        app.set_geo_crop_x(c.x);
        app.set_geo_crop_y(c.y);
        app.set_geo_crop_w(c.w);
        app.set_geo_crop_h(c.h);
    }
}

/// After the angle or aspect changed: keep the crop where it is if it
/// still fits and keeps the shape asked; otherwise the largest crop
/// about its center with that shape, no bigger than it was.
///
/// `settled` is `!Edit::needs_frame()` for the edit on the panel, and
/// nothing is touched without it. An edit still waiting on the
/// frame's shape has a `portrait` flag that has not been brought up
/// to date, so the aspect it reports is the one version 3 meant and
/// a refit would crop the picture to it — which is the very thing
/// the schema bump exists to prevent. The frame is measured as it is
/// selected, culled or turned, so this only bites where nothing can
/// say which way up the frame is at all; there the crop stays
/// exactly as it was written until something can.
///
/// It is a parameter rather than a check inside, so that the
/// compiler asks the question at every call rather than trusting
/// each of them to remember.
pub(crate) fn refit_crop(app: &App, sw: f32, sh: f32, settled: bool) {
    if !settled {
        return;
    }
    let geometry = read_geometry(app);
    let current = geometry.effective_crop(sw, sh);
    let (pw, ph) = geometry.plane_size(sw, sh);
    let ratio = geometry.aspect.ratio(pw, ph, geometry.portrait);
    let shape_ok = match ratio {
        None => true,
        Some(r) => ((current.w * pw) / (current.h * ph) / r - 1.0).abs() < 0.01,
    };
    if geometry.fits(&current, sw, sh) && shape_ok && geometry.crop.is_some() {
        return;
    }
    let ratio = ratio.unwrap_or((current.w * pw) / (current.h * ph));
    let mut fitted = geometry.largest_fit(current.center(), ratio, sw, sh);
    if fitted.w < 1e-4 || fitted.h < 1e-4 {
        fitted = geometry.largest_fit((0.5, 0.5), ratio, sw, sh);
    }
    if geometry.crop.is_some() {
        let shrink = (current.w / fitted.w).min(current.h / fitted.h).min(1.0);
        let (cx, cy) = fitted.center();
        fitted = Crop {
            x: cx - fitted.w * shrink / 2.0,
            y: cy - fitted.h * shrink / 2.0,
            w: fitted.w * shrink,
            h: fitted.h * shrink,
        };
    }
    set_crop(app, Some(fitted));
}

/// The geometry as the panel shows it.
pub(crate) fn read_geometry(app: &App) -> Geometry {
    Geometry {
        turns: (app.get_geo_turns().rem_euclid(4)) as u8,
        flip: app.get_geo_flip(),
        angle: app.get_geo_angle().clamp(-MAX_ANGLE, MAX_ANGLE),
        vertical: app.get_geo_vertical().clamp(-MAX_TILT, MAX_TILT),
        horizontal: app.get_geo_horizontal().clamp(-MAX_TILT, MAX_TILT),
        crop: app.get_geo_has_crop().then(|| Crop {
            x: app.get_geo_crop_x(),
            y: app.get_geo_crop_y(),
            w: app.get_geo_crop_w(),
            h: app.get_geo_crop_h(),
        }),
        aspect: Aspect::from_name(
            app.get_aspect_name().as_str(),
            app.get_aspect_custom().as_str(),
        ),
        portrait: app.get_portrait(),
    }
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, _worker: &Rc<Worker>) {
    // Straighten and crop. Entering or leaving crop mode fits the
    // view to what it shows; handles move the crop as it was when
    // pressed; the angle, aspect and portrait re-fit the crop.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_crop_toggled(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.zoom = 0.0;
            st.image_size = (0, 0); // re-centered on the next frame
            st.crop_drag = None;
            st.crop_draw = None;
            app.window().request_redraw();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_crop_pressed(move |_| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let (sw, sh) = (st.source_size.0 as f32, st.source_size.1 as f32);
            st.crop_drag = Some(read_geometry(&app).effective_crop(sw, sh));
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_crop_dragged(move |handle, dx, dy| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            let (Some(start), Some(handle)) = (st.crop_drag, Handle::from_index(handle)) else {
                return;
            };
            let (sw, sh) = (st.source_size.0 as f32, st.source_size.1 as f32);
            let (vw, vh) = (
                app.get_view_width().max(1) as u32,
                app.get_view_height().max(1) as u32,
            );
            let zoom = effective_zoom(&st, vw, vh);
            let scale = app.window().scale_factor();
            let geometry = read_geometry(&app);
            let (pw, ph) = geometry.plane_size(sw, sh);
            let ratio = geometry.aspect.ratio(pw, ph, geometry.portrait);
            let moved = geometry.drag_within(
                &start,
                handle,
                dx * scale / zoom / pw,
                dy * scale / zoom / ph,
                ratio,
                sw,
                sh,
            );
            set_crop(&app, Some(moved));
            app.window().request_redraw();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_crop_released(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if state.borrow_mut().crop_drag.take().is_some() {
                app.invoke_view_changed();
            }
        });
    }
    // A press outside the crop, on the picture: a fresh crop from the
    // press point, drawn to the pointer and held to the aspect in
    // force, exactly as the Slint side only calls this once a drag
    // has cleared a few pixels.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_crop_draw_pressed(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            let (sw, sh) = (st.source_size.0 as f32, st.source_size.1 as f32);
            let raw = view_to_plane(&st, &app, x, y);
            drop(st);
            // Off the source (the letterbox, or a wedge a turn cut
            // away inside the plane's own square): pulled onto it, or
            // no anchor at all and so no draw, rather than a zero-size
            // crop that would wipe out today's.
            let geometry = read_geometry(&app);
            state.borrow_mut().crop_draw = geometry.anchored(raw, sw, sh);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_crop_draw_dragged(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            let Some(anchor) = st.crop_draw else {
                return;
            };
            let (sw, sh) = (st.source_size.0 as f32, st.source_size.1 as f32);
            let to = view_to_plane(&st, &app, x, y);
            drop(st);
            let geometry = read_geometry(&app);
            let (pw, ph) = geometry.plane_size(sw, sh);
            let ratio = geometry.aspect.ratio(pw, ph, geometry.portrait);
            let Some(drawn) = geometry.drawn(anchor, to, ratio, sw, sh) else {
                return;
            };
            set_crop(&app, Some(drawn));
            app.window().request_redraw();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_crop_draw_released(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if state.borrow_mut().crop_draw.take().is_some() {
                app.invoke_view_changed();
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_geometry_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (sw, sh, settled) = {
                let st = state.borrow();
                (
                    st.source_size.0 as f32,
                    st.source_size.1 as f32,
                    !st.edit.needs_frame(),
                )
            };
            refit_crop(&app, sw, sh, settled);
            app.invoke_view_changed();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_geometry_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_geo_angle(0.0);
            app.set_geo_vertical(0.0);
            app.set_geo_horizontal(0.0);
            app.set_geo_turns(0);
            app.set_geo_flip(false);
            set_crop(&app, None);
            app.set_aspect_name("Free".into());
            app.set_portrait(false);
            app.set_level_mode(false);
            app.set_guide_mode("".into());
            state.borrow_mut().guiding.clear();
            app.invoke_view_changed();
        });
    }
    // Quarter turns and mirrors: the geometry does the bookkeeping, the
    // panel takes the result.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_geometry_turned(move |quarters| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (sw, sh, settled) = {
                let st = state.borrow();
                (
                    st.source_size.0 as f32,
                    st.source_size.1 as f32,
                    !st.edit.needs_frame(),
                )
            };
            show_geometry(&read_geometry(&app).turned(quarters), &app);
            refit_crop(&app, sw, sh, settled);
            app.invoke_view_changed();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_geometry_flipped(move |vertical| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (sw, sh, settled) = {
                let st = state.borrow();
                (
                    st.source_size.0 as f32,
                    st.source_size.1 as f32,
                    !st.edit.needs_frame(),
                )
            };
            show_geometry(&read_geometry(&app).flipped(vertical), &app);
            refit_crop(&app, sw, sh, settled);
            app.invoke_view_changed();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_level_end(move |x0, y0, x1, y1| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_level_mode(false);
            let (dx, dy) = (x1 - x0, y1 - y0);
            if (dx * dx + dy * dy).sqrt() < 8.0 {
                return;
            }
            // Counter-clockwise on the screen is positive; y runs down.
            let drawn = (-dy).atan2(dx).to_degrees();
            let geometry = read_geometry(&app);
            app.set_geo_angle(geometry.leveled_by(drawn));
            let (sw, sh, settled) = {
                let st = state.borrow();
                (
                    st.source_size.0 as f32,
                    st.source_size.1 as f32,
                    !st.edit.needs_frame(),
                )
            };
            refit_crop(&app, sw, sh, settled);
            app.invoke_view_changed();
        });
    }
    // The perspective guide. Each stroke goes to source pixels as it
    // is released, against the view it was drawn on; Rust holds the
    // first one, so which of the two a stroke is is settled in one
    // place. The ends are read through the geometry in force, so the
    // answer does not depend on what the picture has been turned by.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_guide_stroke(move |x0, y0, x1, y1| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(axis) = Guide::from_name(app.get_guide_mode().as_str()) else {
                return;
            };
            let (sw, sh, settled, pair) = {
                let mut st = state.borrow_mut();
                let (sw, sh) = (st.source_size.0 as f32, st.source_size.1 as f32);
                let settled = !st.edit.needs_frame();
                if sw < 1.0 || sh < 1.0 {
                    return;
                }
                // `view_to_source` reads in the masks' units, both over
                // the source's width.
                let at = |st: &State, x, y| {
                    let (u, v) = view_to_source(st, &app, x, y);
                    (u * sw, v * sw)
                };
                let ends = [at(&st, x0, y0), at(&st, x1, y1)];
                (sw, sh, settled, st.guiding.stroke(axis, ends))
            };
            let Some((a, b)) = pair else {
                // The first of the two: it is drawn from now on.
                app.window().request_redraw();
                return;
            };
            let Some(got) = read_geometry(&app).guided_by(a, b, axis, sw, sh) else {
                // Two lines that say nothing: the same line twice, or a
                // pair crossing at the center. Keep the tool in hand
                // rather than setting a wrong angle silently.
                app.set_status("the guide needs two different lines".into());
                app.window().request_redraw();
                return;
            };
            app.set_guide_mode("".into());
            state.borrow_mut().guiding.clear();
            app.set_geo_angle(got.angle);
            match axis {
                Guide::Vertical => app.set_geo_vertical(got.tilt),
                Guide::Horizontal => app.set_geo_horizontal(got.tilt),
            }
            refit_crop(&app, sw, sh, settled);
            app.invoke_view_changed();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_guide_dropped(move || {
            state.borrow_mut().guiding.clear();
            if let Some(app) = app_weak.upgrade() {
                app.window().request_redraw();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::{refit_crop, show_geometry};
    use crate::App;
    use crate::testing::{drag, retouch_state, window};
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
    fn draw_setup(app: &App) -> (Rc<RefCell<crate::State>>, slint::LogicalPosition) {
        let (state, _worker) = retouch_state(app);
        {
            let mut st = state.borrow_mut();
            st.source_size = (SW as u32, SH as u32);
            st.zoom = 1.0;
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
            state.borrow().crop_draw.is_some(),
            "the drag cleared the threshold and started a draw"
        );
        // The window lets go of the gesture instead of a release
        // ending it (focus lost, here simulated as the pointer
        // leaving the window mid-drag).
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerExited);
        assert!(
            state.borrow().crop_draw.is_none(),
            "the cancel let go of the anchor"
        );
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
        state.borrow_mut().source_size = (4000, 6000);

        // One move of the panel, as the Crop tab makes it: the edit
        // on show, the geometry on the controls, then whatever was
        // touched, then the callback the controls invoke.
        let moved = |edit: &greycard_edit::Edit, angle: f32, aspect: &str| {
            state.borrow_mut().edit = edit.clone();
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
        refit_crop(&app, sw, sh, true);
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
        state.borrow_mut().source_size = (5464, 8192);
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
}
