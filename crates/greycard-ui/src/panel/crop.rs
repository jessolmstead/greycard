use crate::panel::viewport::{effective_zoom, view_to_plane, view_to_source};
use crate::*;

/// Put a geometry on the panel.
pub(crate) fn show_geometry(geometry: &Geometry, app: &App) {
    app.set_aspect_held(false);
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
    app.set_aspect_fitted(aspect_key(geometry.aspect).into());
}

/// An aspect as one string, for the panel to hold what the crop was
/// last fitted under: the list's name, or "Custom" and its ratio.
pub(crate) fn aspect_key(aspect: Aspect) -> String {
    match aspect.name() {
        (name, None) => name.to_string(),
        (_, Some(custom)) => format!("Custom {custom}"),
    }
}

/// The inverse of [`aspect_key`].
fn aspect_of_key(key: &str) -> Aspect {
    match key.strip_prefix("Custom ") {
        Some(custom) => Aspect::from_name("Custom", custom),
        None => Aspect::from_name(key, ""),
    }
}

/// The aspect the crop opens under on a frame with no crop of its
/// own, as the Settings sheet offers it: the frame's own shape, the
/// aspect last picked from the list (kept across sessions), or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Start {
    #[default]
    Original,
    LastUsed,
    Free,
}

impl Start {
    /// The sheet's words, in the order its row shows them.
    pub(crate) const NAMES: [&str; 3] = ["Original", "Last used", "Free"];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Start::Original => Self::NAMES[0],
            Start::LastUsed => Self::NAMES[1],
            Start::Free => Self::NAMES[2],
        }
    }

    /// The choice a word names; Original, the default, for anything
    /// else (a file from before the setting, or one edited by hand).
    pub(crate) fn from_name(name: &str) -> Self {
        match name {
            "Last used" => Start::LastUsed,
            "Free" => Start::Free,
            _ => Start::Original,
        }
    }
}

/// The settings file's two, on the state: read at launch.
pub(crate) fn take_settings(st: &mut State, kept: &settings::Settings) {
    st.develop.crop_start = Start::from_name(&kept.crop_start);
    st.develop.crop_last = kept.crop_last.clone();
}

/// Change the settings file this run writes, now rather than at the
/// close, so a session that never closes cleanly keeps it too; none
/// written by a capture or a batch run, nor by a test that gave no
/// file.
fn keep_in_file(st: &State, change: impl FnOnce(&mut settings::Settings)) {
    if st.window.batch {
        return;
    }
    if let Some(file) = &st.window.settings_file {
        let mut kept = settings::Settings::load_from(file);
        change(&mut kept);
        kept.save_to(file);
    }
}

/// The Settings sheet's row chosen.
pub(crate) fn start_chosen(st: &mut State, name: &str) {
    st.develop.crop_start = Start::from_name(name);
    let word = st.develop.crop_start.name().to_string();
    keep_in_file(st, |s| s.crop_start = word);
}

/// An aspect picked from the list or typed in the custom box: what
/// "Last used" opens under from now on, this session and the next.
fn remember_last(st: &mut State, key: String) {
    if st.develop.crop_last == key {
        return;
    }
    st.develop.crop_last = key.clone();
    keep_in_file(st, |s| s.crop_last = key);
}

/// How the starting aspect went on the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Laid {
    /// Not at all: the frame has its own, or the start is Free.
    No,
    /// On the panel alone (`aspect-held`): the edit keeps its own
    /// until the user acts on the crop, so nothing is stored.
    Held,
    /// On the panel and so in the edit: a ratio, which crops.
    Taken,
}

/// The aspect the crop opens under, laid on a frame that has no crop
/// of its own: one never cropped, or reset, which the panel shows as
/// no crop under Free. A frame that holds a crop, or an aspect picked
/// for it, keeps its own. Set as a pick from the list sets it (the
/// Portrait switch the way the frame stands), and the crop left to
/// the aspect.
///
/// Original over no crop is the whole frame, so there is nothing to
/// store: it is held on the panel, and the first drag, draw or pick
/// takes it into the edit. A ratio opened with the Crop button crops
/// the frame, which is what the setting asks; on a frame `arrived`
/// with the crop already open it is held too, so moving through a
/// shoot crops nothing the user has not touched.
pub(crate) fn open_under_start(st: &State, app: &App, arrived: bool) -> Laid {
    let geometry = read_geometry(app);
    if geometry.crop.is_some() || geometry.aspect != Aspect::Free {
        return Laid::No;
    }
    let aspect = match st.develop.crop_start {
        Start::Original => Aspect::Original,
        Start::LastUsed => aspect_of_key(&st.develop.crop_last),
        Start::Free => return Laid::No,
    };
    if aspect == Aspect::Free {
        return Laid::No;
    }
    let (name, custom) = aspect.name();
    app.set_aspect_name(name.into());
    if let Some(custom) = custom {
        app.set_aspect_custom(custom.into());
    }
    let (sw, sh) = (
        st.develop.source_size.0 as f32,
        st.develop.source_size.1 as f32,
    );
    pick_aspect(app, sw, sh);
    let held = arrived || aspect == Aspect::Original;
    app.set_aspect_held(held);
    if held { Laid::Held } else { Laid::Taken }
}

/// A frame opened with crop mode on: its develop has landed and its
/// shape is known, so the starting aspect goes on the panel, once,
/// held there until the user acts on this frame's crop.
pub(crate) fn start_landed(st: &mut State, app: &App) {
    if !std::mem::take(&mut st.develop.crop_start_due) {
        return;
    }
    if app.get_crop_mode() && open_under_start(st, app, true) != Laid::No {
        app.window().request_redraw();
    }
}

/// After the aspect list, the custom box or the Portrait switch:
/// the switch set the way a new aspect asks
/// ([`Geometry::with_aspect`]), against the aspect the crop was
/// fitted under before. Where the switch itself moved, the aspect
/// has not, and the switch stays where the hand put it.
///
/// The custom box asks on every key, and a ratio half typed ("16:")
/// reads as free. That is no pick of Free: were it taken for one,
/// the "16:9" a key later would be a ratio picked over Free and
/// would set aside a switch set by hand. So it is passed over, and
/// the aspect before it stands.
///
/// The aspect it settles on, when it is another than the crop was
/// fitted under: a pick, which "Last used" remembers.
pub(crate) fn pick_aspect(app: &App, sw: f32, sh: f32) -> Option<String> {
    if sw >= 1.0 && sh >= 1.0 {
        app.set_source_tall(sh > sw);
    }
    let geometry = read_geometry(app);
    if geometry.aspect == Aspect::Free && app.get_aspect_name() == "Custom" {
        return None;
    }
    let fitted = app.get_aspect_fitted();
    let was = Geometry {
        aspect: aspect_of_key(fitted.as_str()),
        ..geometry.clone()
    };
    app.set_portrait(was.with_aspect(geometry.aspect, sw, sh).portrait);
    let key = aspect_key(geometry.aspect);
    app.set_aspect_fitted(key.as_str().into());
    (key != fitted.as_str()).then_some(key)
}

/// Every crop the panel takes comes through here, so whatever a
/// change of shape was asked to keep is let go the moment anything
/// else moves the crop.
///
/// A crop taken is the user's act on it (a drag, a draw, a pick, a
/// turn), so a starting aspect held on the panel is the edit's from
/// here.
pub(crate) fn set_crop(app: &App, crop: Option<Crop>) {
    if crop.is_some() {
        app.set_aspect_held(false);
    }
    app.set_crop_kept_area(0.0);
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
/// "No bigger" is measured two ways. The same shape that no longer
/// fits (an angle, a keystone) is never wider or taller than it was:
/// a turn of the picture never grows the crop. A new shape (another
/// ratio, or the Portrait switch) keeps the crop's area instead,
/// since capping each side at the old crop's took a wide crop turned
/// tall down to the old height, and the next turn back down again,
/// so every toggle shrank it. Area rather than the longer edge, so
/// that a square picked from 65:24 keeps as much of the picture as
/// the band did rather than growing to the band's length. The area
/// asked for is kept on the panel (`crop-kept-area`) until anything
/// else moves the crop, so a shape the plane cannot hold at that
/// area, fitted smaller, still turns back to the crop it came from:
/// toggling twice is where it started.
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
    let mut kept = 0.0;
    if geometry.crop.is_some() {
        let shrink = if shape_ok {
            (current.w / fitted.w).min(current.h / fitted.h).min(1.0)
        } else {
            let area = |c: &Crop| c.w * pw * c.h * ph;
            kept = match app.get_crop_kept_area() {
                a if a > 0.0 => a,
                _ => area(&current),
            };
            (kept / area(&fitted)).sqrt().min(1.0)
        };
        let (cx, cy) = fitted.center();
        fitted = Crop {
            x: cx - fitted.w * shrink / 2.0,
            y: cy - fitted.h * shrink / 2.0,
            w: fitted.w * shrink,
            h: fitted.h * shrink,
        };
    }
    set_crop(app, Some(fitted));
    app.set_crop_kept_area(kept);
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
            st.develop.zoom = 0.0;
            st.develop.image_size = (0, 0); // re-centered on the next frame
            // Left in the middle of a drag (by R, whose press is not a
            // pointer's): the crop as dragged so far is the edit, as
            // the release would have made it, which now finds nothing.
            let mid_drag =
                st.develop.crop_drag.take().is_some() | st.develop.crop_draw.take().is_some();
            // The crop opens under the starting aspect, on a frame
            // with none of its own; a ratio taken crops it, an aspect
            // held changes nothing stored.
            let laid = if app.get_crop_mode() {
                open_under_start(&st, &app, false)
            } else {
                Laid::No
            };
            st.develop.crop_start_due = false;
            drop(st);
            if laid == Laid::Taken || mid_drag {
                app.invoke_view_changed();
            }
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
            let (sw, sh) = (
                st.develop.source_size.0 as f32,
                st.develop.source_size.1 as f32,
            );
            // From the box on show: under an aspect held, the held
            // aspect's, which the panel's geometry says.
            st.develop.crop_drag = Some(read_geometry(&app).effective_crop(sw, sh));
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_crop_dragged(move |handle, dx, dy| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            let (Some(start), Some(handle)) = (st.develop.crop_drag, Handle::from_index(handle))
            else {
                return;
            };
            let (sw, sh) = (
                st.develop.source_size.0 as f32,
                st.develop.source_size.1 as f32,
            );
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
            if state.borrow_mut().develop.crop_drag.take().is_some() {
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
            let (sw, sh) = (
                st.develop.source_size.0 as f32,
                st.develop.source_size.1 as f32,
            );
            let raw = view_to_plane(&st, &app, x, y);
            drop(st);
            // Off the source (the letterbox, or a wedge a turn cut
            // away inside the plane's own square): pulled onto it, or
            // no anchor at all and so no draw, rather than a zero-size
            // crop that would wipe out today's.
            let geometry = read_geometry(&app);
            state.borrow_mut().develop.crop_draw = geometry.anchored(raw, sw, sh);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_crop_draw_dragged(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            let Some(anchor) = st.develop.crop_draw else {
                return;
            };
            let (sw, sh) = (
                st.develop.source_size.0 as f32,
                st.develop.source_size.1 as f32,
            );
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
            if state.borrow_mut().develop.crop_draw.take().is_some() {
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
                    st.develop.source_size.0 as f32,
                    st.develop.source_size.1 as f32,
                    !st.develop.edit.needs_frame(),
                )
            };
            if let Some(picked) = pick_aspect(&app, sw, sh) {
                remember_last(&mut state.borrow_mut(), picked);
            }
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
            app.set_aspect_held(false);
            app.set_aspect_name("Free".into());
            app.set_aspect_fitted("Free".into());
            app.set_portrait(false);
            app.set_level_mode(false);
            app.set_guide_mode("".into());
            state.borrow_mut().develop.guiding.clear();
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
                    st.develop.source_size.0 as f32,
                    st.develop.source_size.1 as f32,
                    !st.develop.edit.needs_frame(),
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
                    st.develop.source_size.0 as f32,
                    st.develop.source_size.1 as f32,
                    !st.develop.edit.needs_frame(),
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
                    st.develop.source_size.0 as f32,
                    st.develop.source_size.1 as f32,
                    !st.develop.edit.needs_frame(),
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
                let (sw, sh) = (
                    st.develop.source_size.0 as f32,
                    st.develop.source_size.1 as f32,
                );
                let settled = !st.develop.edit.needs_frame();
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
                (sw, sh, settled, st.develop.guiding.stroke(axis, ends))
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
            state.borrow_mut().develop.guiding.clear();
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
            state.borrow_mut().develop.guiding.clear();
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
    fn draw_setup(app: &App) -> (Rc<RefCell<crate::State>>, slint::LogicalPosition) {
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
        let (at, size) = crate::testing::labeled(app, "Draw");
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
        let (at, size) = crate::testing::labeled(&app, "Done");
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
        let (at, size) = crate::testing::labeled(app, "Portrait");
        crate::testing::click(app, at.x + size.width - 12.0, at.y + size.height / 2.0);
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
                image: crate::worker::Developed::Halves(std::sync::Arc::new(
                    crate::worker::Halves {
                        width: size.0,
                        height: size.1,
                        pixels: Vec::new(),
                    },
                )),
                guide: std::sync::Arc::new(crate::finish::Guide::NONE),
                planes: None,
                white: crate::worker::WhiteBase::IDENTITY,
                seconds: 0.1,
                detail: None,
                sharpen: None,
                dehaze: None,
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
        crate::panel::viewport::show_crop_on_view(
            &state.borrow(),
            &app,
            &super::read_geometry(&app),
        );
        let (w, h) = (app.get_crop_width(), app.get_crop_height());
        assert!(
            w > 0.0 && (h / w - 1.5).abs() < 1e-2,
            "{w} by {h} on screen"
        );
        // Flipped, it lies on its side on the screen too.
        click_portrait(&app);
        crate::panel::viewport::show_crop_on_view(
            &state.borrow(),
            &app,
            &super::read_geometry(&app),
        );
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
                image: crate::worker::Developed::Halves(std::sync::Arc::new(
                    crate::worker::Halves {
                        width: 4000,
                        height: 6000,
                        pixels: Vec::new(),
                    },
                )),
                guide: std::sync::Arc::new(crate::finish::Guide::NONE),
                planes: None,
                white: crate::worker::WhiteBase::IDENTITY,
                seconds: 0.1,
                detail: None,
                sharpen: None,
                dehaze: None,
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
            let c = g.effective_crop(4000.0, 6000.0);
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
        let c = g.effective_crop(4000.0, 6000.0);
        let (pw, ph) = g.plane_size(4000.0, 6000.0);
        assert!(
            c.w * pw > c.h * ph,
            "a turned tall frame's own shape is wide"
        );
    }

    /// A frame of the window's folder opened, as a click on its cell
    /// opens it, with its develop landed: `sw` by `sh`, and the
    /// crop mode as it was.
    fn open_frame(app: &App, state: &Rc<RefCell<crate::State>>, i: i32, (sw, sh): (u32, u32)) {
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
            st.sidecars[7].current.geometry.crop = Some(kept);
            st.sidecars[7].current.geometry.aspect = crate::Aspect::Ratio { w: 16.0, h: 9.0 };
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
        let geometry = &st.sidecars[0].current.geometry;
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
        let shown = super::read_geometry(&app).effective_crop(6000.0, 4000.0);
        assert!(
            ((shown.w * 6000.0) / (shown.h * 4000.0) - 1.0).abs() < 1e-3,
            "the box on show is a square: {shown:?}"
        );
        drag_corner(&app);
        rest();
        let st = state.borrow();
        let geometry = &st.sidecars[1].current.geometry;
        assert_eq!(geometry.aspect, crate::Aspect::Ratio { w: 1.0, h: 1.0 });
        let c = geometry.crop.expect("the drag made a crop");
        assert!(
            ((c.w * 6000.0) / (c.h * 4000.0) - 1.0).abs() < 1e-3,
            "a square: {c:?}"
        );
        assert_eq!(st.sidecars[1].history.len(), 1);
    }
}
