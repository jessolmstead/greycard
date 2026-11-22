use std::rc::Rc;

use crate::tr;
use greycard_edit::geometry::{Aspect, Crop, Geometry, Guide, Handle, MAX_ANGLE, MAX_TILT};
use slint::ComponentHandle;

use crate::panel::viewport::{effective_zoom, view_to_plane, view_to_source};
use crate::worker::Worker;
use crate::{App, Shared, State, placeholder, settings};

pub(crate) mod overlay;

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
pub(crate) fn keep_in_file(st: &State, change: impl FnOnce(&mut settings::Settings)) {
    if st.window.batch {
        return;
    }
    if let Some(file) = &st.window.settings_file {
        settings::Settings::change_at(file, change);
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
    let camera = crate::panel::viewport::camera_crop(st);
    pick_aspect(app, sw, sh, camera);
    let held = arrived || aspect == Aspect::Original;
    app.set_aspect_held(held);
    if held {
        return Laid::Held;
    }
    // Taken: the crop fitted as a pick from the list fits it, so the
    // edit stores the crop the box shows, not an aspect alone.
    refit_crop(app, sw, sh, !st.develop.edit.needs_frame(), camera);
    Laid::Taken
}

/// A frame opened with crop mode on: its develop has landed and its
/// shape is known, so the starting aspect goes on the panel, once,
/// held there until the user acts on this frame's crop.
///
/// And a ratio taken with the Crop button before the develop landed
/// has its crop fitted now, as it would have been had the size been
/// known, unless the user has acted on the crop since.
pub(crate) fn start_landed(st: &mut State, app: &App) {
    fit_taken(st, app);
    if !std::mem::take(&mut st.develop.crop_start_due) {
        return;
    }
    if app.get_crop_mode() && open_under_start(st, app, true) != Laid::No {
        app.window().request_redraw();
    }
}

/// A frame as the panel shows it: its file and its version, none for
/// the file's own.
pub(crate) type FrameId = (std::path::PathBuf, Option<std::sync::Arc<str>>);

/// The frame on screen, by its file and version.
fn on_screen(st: &State) -> Option<FrameId> {
    let cell = st.cells.get(st.current?)?;
    Some((st.files.get(cell.file)?.clone(), cell.version.clone()))
}

/// The crop of a ratio taken before the frame's size was known, fitted
/// once it is, and kept as the Crop button's own act would have been.
/// Still due, and tried at the next landing, while the size or the
/// frame's way up is not known.
fn fit_taken(st: &mut State, app: &App) {
    let Some(due) = &st.develop.crop_fit_due else {
        return;
    };
    // Due on another frame than the one whose develop landed: the user
    // went on (by the strip, the grid, or culling) before it landed.
    if on_screen(st).as_ref() != Some(due) {
        st.develop.crop_fit_due = None;
        return;
    }
    let geometry = read_geometry(app);
    if app.get_aspect_held() || !geometry.frames_by_aspect() {
        st.develop.crop_fit_due = None;
        return;
    }
    let (sw, sh) = (
        st.develop.source_size.0 as f32,
        st.develop.source_size.1 as f32,
    );
    let camera = crate::panel::viewport::camera_crop(st);
    refit_crop(app, sw, sh, !st.develop.edit.needs_frame(), camera);
    if app.get_geo_has_crop() {
        st.develop.crop_fit_due = None;
        app.window().request_redraw();
        crate::panel::edit::schedule_save(st, app.as_weak());
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
pub(crate) fn pick_aspect(app: &App, sw: f32, sh: f32, camera: Option<Crop>) -> Option<String> {
    let geometry = read_geometry(app);
    if sw >= 1.0 && sh >= 1.0 {
        let (ow, oh) = Geometry {
            turns: 0,
            ..geometry.clone()
        }
        .own_shape(sw, sh, camera);
        app.set_source_tall(oh > ow);
    }
    if geometry.aspect == Aspect::Free && app.get_aspect_name() == "Custom" {
        return None;
    }
    let fitted = app.get_aspect_fitted();
    let was = Geometry {
        aspect: aspect_of_key(fitted.as_str()),
        ..geometry.clone()
    };
    app.set_portrait(was.with_aspect(geometry.aspect, sw, sh, camera).portrait);
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
///
/// `camera` is the camera's frame on the source
/// ([`crate::panel::viewport::camera_crop`]): with no crop stored, the
/// crop in force is that frame, and Original holds its shape.
pub(crate) fn refit_crop(app: &App, sw: f32, sh: f32, settled: bool, camera: Option<Crop>) {
    // The size is not known until the frame's develop lands (a switch
    // zeroes it); a fit into nothing is NaN, which the sidecar wrote
    // as null and then would not read.
    if !settled || sw <= 0.0 || sh <= 0.0 {
        return;
    }
    let geometry = read_geometry(app);
    let current = geometry.effective_crop(sw, sh, camera);
    let (pw, ph) = geometry.plane_size(sw, sh);
    let ratio = geometry.ratio(sw, sh, camera);
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
    if ![fitted.x, fitted.y, fitted.w, fitted.h]
        .iter()
        .all(|v| v.is_finite())
    {
        log::warn!("the crop's refit on a {sw}x{sh} frame was not finite; the crop stands");
        return;
    }
    set_crop(app, Some(fitted));
    app.set_crop_kept_area(kept);
}

/// Level the picture to a line at `drawn` degrees counter-clockwise
/// on the screen, the level's way: the nearer of horizontal and
/// vertical taken, the crop re-fit, the change recorded.
fn level_by(state: &crate::Shared, app: &App, drawn: f32) {
    let geometry = read_geometry(app);
    app.set_geo_angle(geometry.leveled_by(drawn));
    let (sw, sh, settled, camera) = {
        let st = state.borrow();
        (
            st.develop.source_size.0 as f32,
            st.develop.source_size.1 as f32,
            !st.develop.edit.needs_frame(),
            crate::panel::viewport::camera_crop(&st),
        )
    };
    refit_crop(app, sw, sh, settled, camera);
    app.invoke_view_changed();
}

/// The long edge of the grid Auto level asks the GPU for, before the
/// texture's own width rounds it to a whole step: the detector shrinks
/// it by a whole factor to [`LONG_EDGE`] or under, which averages the
/// grid's points into pixels.
///
/// [`LONG_EDGE`]: greycard_core::horizon::LONG_EDGE
const STRAIGHTEN_GRID: u32 = 2 * greycard_core::horizon::LONG_EDGE as u32;

#[cfg(test)]
thread_local! {
    /// The picture Auto level reads in a test, which has no GPU: the
    /// develop's working-space pixels and the width.
    pub(crate) static STRAIGHTEN_PICTURE: std::cell::RefCell<Option<(Vec<[f32; 3]>, usize)>> =
        const { std::cell::RefCell::new(None) };
}

/// The step between the grid's points on the texture, in texels, as
/// [`crate::render::Renderer::sample_grid`] takes it, and the width it
/// is asked for.
fn straighten_step(texture: (u32, u32)) -> (u32, u32) {
    let (w, h) = texture;
    let across = (u64::from(STRAIGHTEN_GRID) * u64::from(w) / u64::from(w.max(h).max(1))) as u32;
    ((w / across.max(1)).max(1), across.max(1))
}

/// The open frame's develop, small, in the working space, and its
/// width, as the GPU holds it: the source before the edit's turn.
fn straighten_picture(st: &State) -> Option<(Vec<[f32; 3]>, usize)> {
    #[cfg(test)]
    if let Some(picture) = STRAIGHTEN_PICTURE.with(|p| p.borrow_mut().take()) {
        return Some(picture);
    }
    let (_, across) = straighten_step(st.develop.shown_size);
    match st.develop.renderer.as_ref()?.sample_grid(across) {
        Ok(grid) => grid,
        Err(e) => {
            tracing::warn!("straighten: {e:#}");
            None
        }
    }
}

/// A point of the texture (texels, continuous) on the picture as the
/// edit is drawn on it: the texture may still be the develop at the
/// turn before, `lag` quarter turns behind (`placeholder::texel_of`
/// the other way).
fn texture_to_source((x, y): (f32, f32), texture: (u32, u32), lag: u8) -> (f32, f32) {
    let (w, h) = (texture.0 as f32, texture.1 as f32);
    match lag % 4 {
        1 => (h - y, x),
        2 => (w - x, h - y),
        3 => (y, w - x),
        _ => (x, y),
    }
}

/// The dominant line of what Auto level read: its luminance in the
/// working space (Rec. 2020), to a perceptual scale so a horizon in
/// shadow counts for what it looks like.
fn straighten_find(read: &StraightenRead) -> Option<greycard_core::horizon::Line> {
    let gray: Vec<f32> = read.picture[..read.w * read.h]
        .iter()
        .map(|p| {
            (0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2])
                .max(0.0)
                .cbrt()
        })
        .collect();
    greycard_core::horizon::horizon(&gray, read.w, read.h)
}

/// What Straighten read off the GPU, and what it needs to take a line
/// found on it to the screen.
struct StraightenRead {
    /// The develop, a grid of it in the working space, and its width
    /// and height.
    picture: Vec<[f32; 3]>,
    w: usize,
    h: usize,
    /// The texture's size, the quarter turns it lags the open frame
    /// by, and the develop it was: a line found on it is applied only
    /// while that develop is the one shown.
    texture: (u32, u32),
    lag: u8,
    shown: Option<(usize, u8)>,
    started: std::time::Instant,
}

thread_local! {
    /// A Straighten whose line is being found: a second press waits for it.
    static STRAIGHTENING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Straighten's press: the develop read off the GPU here, its line
/// found on a thread of its own (a few tens of milliseconds), and the
/// level set back here, through the level's own path.
fn straighten(state: &Rc<Shared>, app: &App) {
    if STRAIGHTENING.get() {
        return;
    }
    let Some(read) = straighten_read(&state.borrow()) else {
        return;
    };
    #[cfg(test)]
    {
        let line = straighten_find(&read);
        straighten_apply(state, app, read, line);
    }
    #[cfg(not(test))]
    {
        STRAIGHTENING.set(true);
        let app_weak = app.as_weak();
        let spawned = std::thread::Builder::new()
            .name("straighten".into())
            .spawn(move || {
                let line = straighten_find(&read);
                let _ = slint::invoke_from_event_loop(move || {
                    STRAIGHTENING.set(false);
                    let (Some(app), Some(state)) = (
                        app_weak.upgrade(),
                        crate::STATE.with(|s| s.borrow().clone()),
                    ) else {
                        return;
                    };
                    straighten_apply(&state, &app, read, line);
                });
            });
        if let Err(e) = spawned {
            STRAIGHTENING.set(false);
            tracing::warn!("straighten: {e}");
        }
    }
}

/// The open frame's develop as Straighten reads it; `None`, logged,
/// with no develop of this frame to read.
fn straighten_read(st: &State) -> Option<StraightenRead> {
    let lag = crate::panel::viewport::own_develop(st);
    let Some(lag) = lag else {
        tracing::info!("straighten: wait for the develop");
        return None;
    };
    let started = std::time::Instant::now();
    let (picture, w) = straighten_picture(st)?;
    let h = picture.len() / w.max(1);
    tracing::debug!(
        "straighten: read {w}x{h} in {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    Some(StraightenRead {
        picture,
        w,
        h,
        texture: st.develop.shown_size,
        lag,
        shown: st.develop.shown_turn,
        started,
    })
}

/// The line found on what [`straighten_read`] read, leveled the way
/// the level levels a line drawn along it; nothing, logged, when there
/// is no line, when the develop shown is no longer the one read, or
/// when the frame is level already.
///
/// The line was found on the develop before the edit's turn, and is
/// taken to the screen through the geometry in force where it lies (a
/// keystone turns a line by where it is): so a frame Straighten has
/// leveled reads level and a second press changes nothing.
fn straighten_apply(
    state: &Rc<Shared>,
    app: &App,
    read: StraightenRead,
    line: Option<greycard_core::horizon::Line>,
) {
    let StraightenRead {
        w,
        h,
        texture,
        lag,
        shown,
        started,
        ..
    } = read;
    tracing::debug!(
        "straighten: done in {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let Some(line) = line else {
        tracing::info!("straighten: no dominant line");
        return;
    };
    let on_screen = {
        let st = state.borrow();
        if st.develop.shown_turn != shown || crate::panel::viewport::own_develop(&st) != Some(lag) {
            tracing::info!("straighten: the picture changed while its line was found");
            return;
        }
        let drawn = placeholder::turned_size(texture, lag);
        let (sw, sh) = (
            st.develop.source_size.0 as f32,
            st.develop.source_size.1 as f32,
        );
        // A grid point's texel, then the source the edit is measured on.
        let (step, _) = straighten_step(texture);
        let to_source = |(x, y): (f32, f32)| {
            let texel = (
                (step / 2) as f32 + x * step as f32 + 0.5,
                (step / 2) as f32 + y * step as f32 + 0.5,
            );
            let (u, v) = texture_to_source(texel, texture, lag);
            (
                u * sw / drawn.0.max(1) as f32,
                v * sh / drawn.1.max(1) as f32,
            )
        };
        // Two points along the line either side of where it lies, taken
        // to the screen; y runs down.
        let (s, c) = line.angle.to_radians().sin_cos();
        let reach = 0.05 * w.max(h) as f32;
        let (x, y) = line.at;
        let a = to_source((x - reach * c, y + reach * s));
        let b = to_source((x + reach * c, y - reach * s));
        let geometry = read_geometry(app);
        let (a, b) = (geometry.to_plane(a, sw, sh), geometry.to_plane(b, sw, sh));
        let on_screen = (a.1 - b.1).atan2(b.0 - a.0).to_degrees();
        let turn = geometry.leveled_by(on_screen) - geometry.angle;
        tracing::info!(
            "straighten: line at {:.2}° about ({x:.0}, {y:.0}) of the {w}x{h} grid, turning {turn:+.2}°",
            line.angle
        );
        // Already level: the change is not one.
        if turn.abs() < 0.01 {
            return;
        }
        on_screen
    };
    level_by(state, app, on_screen);
}

/// What `geometry` shows of the developed picture where it reaches a
/// Fill of `retouch`, as the develop's retouch step takes it
/// (`greycard_engine::shown`): none where it reaches none, or before
/// the frame's develop has said its size. Two geometries whose
/// outlines differ here make some Fill's hole differ.
pub(crate) fn fill_outline(
    st: &State,
    geometry: &Geometry,
    retouch: &greycard_edit::retouch::Retouch,
) -> Option<greycard_engine::shown::Shown> {
    let (w, h) = (
        st.develop.source_size.0 as f32,
        st.develop.source_size.1 as f32,
    );
    let camera = crate::panel::viewport::camera_crop(st);
    greycard_engine::shown::Shown::of(geometry, w, h, camera).filter(|s| s.reaches(retouch, w, h))
}

/// Whether a change of the geometry is still in hand: the Angle or a
/// keystone slider held (or stepped by wheel or key within its
/// second), or the Custom aspect being typed. Its develop waits for
/// `geometry-settled`, so a pause along it makes no Fill.
pub(crate) fn geometry_held(app: &App) -> bool {
    app.get_angle_moving() || app.get_perspective_moving() || app.get_aspect_typing()
}

/// Whether the panel's geometry, `now`, shows the Fills of the
/// develop's retouch otherwise than the geometry the develop was last
/// made under: the geometry is the finish's alone, drawn by the shader
/// with no develop, but a Fill's engine is not shown what the crop
/// hides, so a crop change that changes that makes the Fill again.
pub(crate) fn fills_see_otherwise(st: &State, now: &Geometry) -> bool {
    let edit = &st.develop.edit;
    *now != edit.geometry
        && fill_outline(st, now, &edit.retouch) != fill_outline(st, &edit.geometry, &edit.retouch)
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

pub(crate) fn install(app: &App, state: &Rc<Shared>, _worker: &Rc<Worker>) {
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
            // A ratio taken before the develop said the frame's size:
            // its crop is fitted when it does (`start_landed`).
            if laid == Laid::Taken {
                st.develop.crop_fit_due = if app.get_geo_has_crop() {
                    None
                } else {
                    on_screen(&st)
                };
            }
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
            // aspect's, which the panel's geometry says; with none,
            // the camera's frame where it framed one.
            let camera = crate::panel::viewport::camera_crop(&st);
            st.develop.crop_drag = Some(read_geometry(&app).effective_crop(sw, sh, camera));
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
            let ratio = geometry.ratio(sw, sh, crate::panel::viewport::camera_crop(&st));
            let moved = geometry.drag_within(
                &start,
                handle,
                dx * scale / zoom / pw,
                dy * scale / zoom / ph,
                ratio,
                (sw, sh),
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
            let camera = crate::panel::viewport::camera_crop(&st);
            drop(st);
            let geometry = read_geometry(&app);
            let ratio = geometry.ratio(sw, sh, camera);
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
            let (sw, sh, settled, camera) = {
                let st = state.borrow();
                (
                    st.develop.source_size.0 as f32,
                    st.develop.source_size.1 as f32,
                    !st.develop.edit.needs_frame(),
                    crate::panel::viewport::camera_crop(&st),
                )
            };
            if let Some(picked) = pick_aspect(&app, sw, sh, camera) {
                remember_last(&mut state.borrow_mut(), picked);
            }
            refit_crop(&app, sw, sh, settled, camera);
            app.invoke_view_changed();
        });
    }
    // A slider let go, an aspect typed and accepted or left: the change
    // is over, and what it does to a Fill may be asked for now
    // (`geometry_held`).
    {
        let app_weak = app.as_weak();
        app.on_geometry_settled(move || {
            if let Some(app) = app_weak.upgrade() {
                app.invoke_view_changed();
            }
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
            let (sw, sh, settled, camera) = {
                let st = state.borrow();
                (
                    st.develop.source_size.0 as f32,
                    st.develop.source_size.1 as f32,
                    !st.develop.edit.needs_frame(),
                    crate::panel::viewport::camera_crop(&st),
                )
            };
            show_geometry(&read_geometry(&app).turned(quarters), &app);
            refit_crop(&app, sw, sh, settled, camera);
            app.invoke_view_changed();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_geometry_flipped(move |vertical| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (sw, sh, settled, camera) = {
                let st = state.borrow();
                (
                    st.develop.source_size.0 as f32,
                    st.develop.source_size.1 as f32,
                    !st.develop.edit.needs_frame(),
                    crate::panel::viewport::camera_crop(&st),
                )
            };
            show_geometry(&read_geometry(&app).flipped(vertical), &app);
            refit_crop(&app, sw, sh, settled, camera);
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
            level_by(&state, &app, drawn);
        });
    }
    // Straighten: the picture's dominant line, leveled as if it had
    // been drawn with the level.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_straightened(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            straighten(&state, &app);
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
            let (sw, sh, settled, camera, pair) = {
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
                let camera = crate::panel::viewport::camera_crop(&st);
                (
                    sw,
                    sh,
                    settled,
                    camera,
                    st.develop.guiding.stroke(axis, ends),
                )
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
                app.set_status(tr!("the guide needs two different lines").into());
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
            refit_crop(&app, sw, sh, settled, camera);
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
    overlay::install(app, state);
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod view_tests;
