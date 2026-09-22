use crate::panel::viewport::{effective_zoom, view_to_source};
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
    use super::{refit_crop, show_geometry};
    use crate::testing::{retouch_state, window};

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
