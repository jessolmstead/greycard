use crate::panel::color::{preview_white, show_mixer_band, store_mixer_band, white_profile};
use crate::panel::crop::read_geometry;
use crate::panel::cull::{control_over_frame, leave_cull};
use crate::panel::curve::{
    current_channel, curve_points, draw_curve, parametric_mode, picked_point, select_point,
    set_curve_points, show_selected,
};
use crate::panel::edit::{current_turn, read_edit, schedule_save};
use crate::panel::startup::{FILE, SYSTEM, monitor_note, monitor_settings};
use crate::*;

/// The size of the source the viewport draws the open frame's edit
/// on, and the quarter turns clockwise it reads the picture on the
/// GPU through to get there.
///
/// The picture is the open frame's develop once it has landed, and
/// until then the develop still on the GPU (`placeholder::drawn_source`).
/// A frame turned since its develop landed is drawn from that develop
/// turned, at once, rather than a second later when the develop at
/// the new turn lands: its size stands on end with the turn, and the
/// shader reads the texture through the turn it has yet to catch up
/// with, so the crop, the masks and the picture under them agree
/// from the frame the key was pressed on.
///
/// While the develop on the GPU is the open frame's own, its size
/// turned by the lag is the size, whatever `source_size` was last
/// told: a turn from the culling loupe over a selection, or one
/// between a develop's delivery and its frame, would otherwise leave
/// the two a quarter apart.
pub(crate) fn drawn_picture(st: &State) -> ((u32, u32), u8) {
    match own_develop(st) {
        Some(lag) => (placeholder::turned_size(st.develop.shown_size, lag), lag),
        None => (
            placeholder::drawn_source(st.develop.source_size, st.develop.shown_size),
            0,
        ),
    }
}

/// The turn the develop on the GPU lags the open frame by, when that
/// develop is the open frame's own; none for another frame's.
fn own_develop(st: &State) -> Option<u8> {
    let shown = st
        .develop
        .shown_turn
        .filter(|_| st.develop.held.is_none())?;
    (Some(shown.0) == st.current)
        .then(|| placeholder::lagging_turn(Some(shown), st.current, current_turn(st)))
}

/// Why a neutral set no white.
#[derive(Debug, PartialEq)]
pub(crate) enum NeutralMiss {
    /// The picture on the GPU is not the open frame's develop yet: a
    /// frame was chosen and its develop has not landed.
    Waiting,
    /// No camera profile to read the white through.
    NoProfile,
    /// The pixel names no white: a channel at nothing.
    NoWhite,
    /// The white would not resolve to a temperature.
    Unresolved(String),
}

impl NeutralMiss {
    /// The status line's words, for the Neutral dropper or for Auto.
    pub(crate) fn said(&self, auto: bool) -> String {
        let what = if auto {
            "auto white balance"
        } else {
            "white balance"
        };
        match self {
            Self::Waiting => format!("{what}: wait for the develop"),
            Self::NoProfile => format!("{what}: no camera profile for this frame"),
            Self::NoWhite if auto => format!("{what}: nothing neutral to read"),
            Self::NoWhite => "nothing to read there".to_string(),
            Self::Unresolved(e) => format!("{what}: {e}"),
        }
    }
}

/// The white balance the picture on the GPU was developed at, when
/// that picture is the open frame's own develop; `Waiting` in the
/// moment between choosing a frame and its develop landing, when the
/// texture and the base are still the last frame's.
fn own_white(st: &State) -> Result<WhiteBase, NeutralMiss> {
    own_develop(st)
        .and(st.develop.base_white)
        .ok_or(NeutralMiss::Waiting)
}

/// Temperature and tint from a pixel of the developed picture that
/// should be grey: what the Neutral dropper and the Auto button both
/// come to. `px` is in the working space at the white the picture was
/// developed at, as [`sample_view`] reads it; the white it names goes
/// to the panel, As shot off, and the develop follows, one step of
/// the history. On a miss nothing moves, and the caller says why.
pub(crate) fn white_from_neutral(
    mut st: std::cell::RefMut<'_, State>,
    app: &App,
    px: [f32; 3],
) -> Result<(), NeutralMiss> {
    let base = own_white(&st)?;
    // Through the profile the develop used, not the file's own: a
    // neutral read through other matrices comes back as another
    // temperature, and the patch the user clicked would not go
    // neutral.
    let profile = white_profile(&mut st).ok_or(NeutralMiss::NoProfile)?;
    let (frame, _) = st.develop.frame.as_ref().ok_or(NeutralMiss::NoProfile)?;
    let gains = greycard_core::color::neutral_gains(px, base.gains, base.matrix)
        .ok_or(NeutralMiss::NoWhite)?;
    let wb =
        resolve_white_balance(frame, &profile, WhitePoint::Coefficients(gains)).map_err(|e| {
            tracing::warn!("white balance from a neutral: {e}");
            NeutralMiss::Unresolved(e.to_string())
        })?;
    let tt = wb.temp_tint;
    // On a mask's own white balance: the white that makes the patch
    // grey is the light there, the same answer whatever the global
    // says, since a mask's white is absolute. Under the mask at full
    // weight the patch is then grey.
    if st.develop.target.is_some() && app.get_panel_tab() == "Masks" {
        app.set_local_wb_on(true);
        app.set_local_wb_kept(true);
        app.set_local_temperature((tt.cct as f32).clamp(2000.0, 12000.0));
        app.set_local_tint((tt.duv as f32).clamp(-0.05, 0.05));
        drop(st);
        app.invoke_stop_placing();
        app.invoke_view_changed();
        return Ok(());
    }
    app.set_as_shot(false);
    app.set_temperature((tt.cct as f32).clamp(2000.0, 12000.0));
    app.set_tint((tt.duv as f32).clamp(-0.05, 0.05));
    drop(st);
    app.invoke_stop_placing();
    app.invoke_develop_changed();
    Ok(())
}

/// How wide a grid of the developed picture the Auto white balance
/// reads: the scopes' width, enough to average and quick to bring back.
const AUTO_WHITE_ACROSS: u32 = 512;

/// What the Auto white balance says when the picture has too little
/// that is neither clipped nor black to read a white from.
pub(crate) const AUTO_WHITE_TOO_LITTLE: &str =
    "auto white balance: not enough of the picture to read";

#[cfg(test)]
thread_local! {
    /// The picture the Auto white balance reads in a test, which has
    /// no GPU to read one back from, and its width.
    pub(crate) static AUTO_WHITE_GRID: std::cell::RefCell<Option<(Vec<[f32; 3]>, usize)>> =
        const { std::cell::RefCell::new(None) };
}

/// The whole developed picture, small, in the working space at the
/// white it was developed at, and how many points across: what the
/// Auto white balance reads.
fn auto_white_grid(st: &State) -> Option<(Vec<[f32; 3]>, usize)> {
    #[cfg(test)]
    if let Some(grid) = AUTO_WHITE_GRID.with(|g| g.borrow_mut().take()) {
        return Some(grid);
    }
    let started = std::time::Instant::now();
    let grid = match st.develop.renderer.as_ref()?.sample_grid(AUTO_WHITE_ACROSS) {
        Ok(grid) => grid,
        Err(e) => {
            tracing::warn!("auto white balance: {e:#}");
            None
        }
    };
    tracing::debug!(
        "auto white balance: read the picture in {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    grid
}

/// The Auto button: the white [`greycard_core::color::auto_neutral`]
/// reads off the whole picture, against the camera's as-shot white,
/// set the way the Neutral dropper sets the one it is shown.
pub(crate) fn auto_white(mut st: std::cell::RefMut<'_, State>, app: &App) {
    let found = (|| {
        let base = own_white(&st)?;
        let profile = white_profile(&mut st).ok_or(NeutralMiss::NoProfile)?;
        let (frame, _) = st.develop.frame.as_ref().ok_or(NeutralMiss::NoProfile)?;
        let as_shot = resolve_white_balance(frame, &profile, WhitePoint::AsShot)
            .map_err(|e| NeutralMiss::Unresolved(e.to_string()))?
            .coefficients_f32();
        let (grid, width) = auto_white_grid(&st).ok_or(NeutralMiss::Waiting)?;
        Ok(greycard_core::color::auto_neutral(
            &grid,
            width,
            base.gains,
            base.matrix,
            base.clip,
            as_shot,
        ))
    })();
    let miss = match found {
        Ok(Some(px)) => match white_from_neutral(st, app, px) {
            Ok(()) => return,
            Err(miss) => miss,
        },
        Ok(None) => {
            app.set_status(AUTO_WHITE_TOO_LITTLE.into());
            return;
        }
        Err(miss) => miss,
    };
    app.set_status(miss.said(true).into());
}

/// `source_size` brought to the size the open frame is drawn at, for
/// everything that measures the edit by it (the crop, the masks, the
/// droppers): after a turn, a select in culling, culling left, and a
/// develop's frame.
pub(crate) fn settle_source_size(st: &mut State) {
    if let Some(lag) = own_develop(st) {
        st.develop.source_size = placeholder::turned_size(st.develop.shown_size, lag);
    }
}

/// Whether the picture the crop is measured on stands taller than
/// wide, for the Portrait switch (`portrait-shown` in the window),
/// which under "Original" reads the stored flag against it: the
/// picture as the viewport draws it ([`drawn_picture`]).
pub(crate) fn show_source_shape(st: &State, app: &App) {
    let ((w, h), _) = drawn_picture(st);
    let tall = h > w;
    if app.get_source_tall() != tall {
        app.set_source_tall(tall);
    }
}

/// The crop's rectangle on the view while cropping, for the overlay:
/// `geometry`'s crop, on the picture as the viewport draws it
/// ([`drawn_picture`]) and through the same mapping, so the outline
/// stands on the screen exactly as the crop will cut the picture
/// under it.
pub(crate) fn show_crop_on_view(st: &State, app: &App, geometry: &Geometry) {
    let ((dw, dh), _) = drawn_picture(st);
    let (sw, sh) = (dw as f32, dh as f32);
    let frame = geometry.bounds(sw, sh);
    let scale = app.window().scale_factor();
    let (vw, vh) = (
        app.get_view_width().max(1) as u32,
        app.get_view_height().max(1) as u32,
    );
    let zoom = effective_zoom(st, vw, vh);
    let c = geometry.effective_crop(sw, sh);
    let (pw, ph) = geometry.plane_size(sw, sh);
    let to_view = |fx: f32, fy: f32| {
        (
            ((fx - frame.origin.0 - st.develop.center.0) * zoom + vw as f32 / 2.0) / scale,
            ((fy - frame.origin.1 - st.develop.center.1) * zoom + vh as f32 / 2.0) / scale,
        )
    };
    let (l, t) = to_view(c.x * pw, c.y * ph);
    let (r, b) = to_view((c.x + c.w) * pw, (c.y + c.h) * ph);
    app.set_crop_left(l);
    app.set_crop_top(t);
    app.set_crop_width(r - l);
    app.set_crop_height(b - t);
}

/// What a dropper's press chose, to move with the drag after it.
pub(crate) enum Picking {
    /// A point on a curve: which, its y when pressed, and the press's
    /// y on the view.
    Curve {
        channel: Channel,
        index: usize,
        y0: f32,
        at: f32,
    },
    /// A band of the mixer: its hue and saturation when pressed, and
    /// the press on the view.
    Mixer {
        band: usize,
        hue0: f32,
        sat0: f32,
        at: (f32, f32),
    },
}

/// How far a drag on the view goes, in logical pixels, for a curve
/// point's whole height, the mixer's whole saturation, and its hue
/// from one end to the other.
pub(crate) const PICK_DRAG: f32 = 200.0;

/// The shortest Oklab chroma deviation the defringe's dropper will
/// read a hue off. Below it the click is on a flat color, where the
/// direction is noise; the frames' own mean deviation is an order
/// above this, and a box average is shorter than a pixel's.
pub(crate) const DEFRINGE_PICK_FLOOR: f32 = 1.0e-4;

/// The hint the status line shows for a dropper in hand.
pub(crate) fn picking_hint(kind: &str) -> &'static str {
    match kind {
        "White" => "click something neutral; Esc to finish",
        "Curve" => "click a tone to add a point, drag up or down to move it; Esc to finish",
        "Range" => "click a color to center the mask's hue on it; Esc to finish",
        "Defringe" => {
            "defringe is off while picking; zoom to 1:1 and click a fringe; Esc to finish"
        }
        _ => {
            "click a color for its band, drag up or down for saturation, sideways for hue; Esc to finish"
        }
    }
}

pub(crate) fn source_to_view(st: &State, app: &App, u: f32, v: f32) -> (f32, f32) {
    ViewMap::of(st, app).to_view(u, v)
}

/// The view's mapping of the picture's units, read from the panel
/// once and applied to as many points as a frame has.
pub(crate) struct ViewMap {
    scale: f32,
    view: (f32, f32),
    zoom: f32,
    source: (f32, f32),
    geometry: Geometry,
    origin: (f32, f32),
    center: (f32, f32),
}

impl ViewMap {
    pub(crate) fn of(st: &State, app: &App) -> Self {
        let (vw, vh) = (
            app.get_view_width().max(1) as u32,
            app.get_view_height().max(1) as u32,
        );
        let (sw, sh) = (
            st.develop.source_size.0 as f32,
            st.develop.source_size.1 as f32,
        );
        let geometry = read_geometry(app);
        let frame = if app.get_crop_mode() {
            geometry.bounds(sw, sh)
        } else {
            geometry.frame(sw, sh)
        };
        Self {
            scale: app.window().scale_factor(),
            view: (vw as f32, vh as f32),
            zoom: effective_zoom(st, vw, vh),
            source: (sw, sh),
            geometry,
            origin: frame.origin,
            center: st.develop.center,
        }
    }

    /// A point in the masks' units on the view, logical pixels.
    pub(crate) fn to_view(&self, u: f32, v: f32) -> (f32, f32) {
        let (sw, sh) = self.source;
        let r = self.geometry.to_plane((u * sw, v * sw), sw, sh);
        (
            ((r.0 - self.origin.0 - self.center.0) * self.zoom + self.view.0 / 2.0) / self.scale,
            ((r.1 - self.origin.1 - self.center.1) * self.zoom + self.view.1 / 2.0) / self.scale,
        )
    }
}

/// A point of the view (logical pixels) in the masks' units: the
/// source position under it, over the source's width.
pub(crate) fn view_to_source(st: &State, app: &App, x: f32, y: f32) -> (f32, f32) {
    let scale = app.window().scale_factor();
    let (vw, vh) = (
        app.get_view_width().max(1) as u32,
        app.get_view_height().max(1) as u32,
    );
    let zoom = effective_zoom(st, vw, vh);
    let (sw, sh) = (
        st.develop.source_size.0 as f32,
        st.develop.source_size.1 as f32,
    );
    let geometry = read_geometry(app);
    let frame = if app.get_crop_mode() {
        geometry.bounds(sw, sh)
    } else {
        geometry.frame(sw, sh)
    };
    let r = (
        (x * scale - vw as f32 / 2.0) / zoom + st.develop.center.0 + frame.origin.0,
        (y * scale - vh as f32 / 2.0) / zoom + st.develop.center.1 + frame.origin.1,
    );
    let at = geometry.to_source(r, sw, sh);
    (at.0 / sw, at.1 / sw)
}

/// Whether a point of the view (logical pixels) lies on the picture
/// as shown, the crop's frame (or the whole plane in crop mode), not
/// on the grey around it.
pub(crate) fn view_on_picture(st: &State, app: &App, x: f32, y: f32) -> bool {
    let source = (
        st.develop.source_size.0 as f32,
        st.develop.source_size.1 as f32,
    );
    let ((fx, fy), (fw, fh)) = view_in_frame(st, app, x, y, source, &read_geometry(app));
    (0.0..=fw).contains(&fx) && (0.0..=fh).contains(&fy)
}

/// A point of the view (logical pixels) on the picture as shown, as
/// fractions of the frame's width and height, which is how the
/// scopes' analysis picture covers it, and the frame's size in its
/// pixels; `None` off the picture, and while the picture on the GPU
/// is not the open frame's own develop (a held one, or another
/// frame's), whose frame the scopes measure by an edit the panel no
/// longer shows.
pub(crate) fn view_to_frame(
    st: &State,
    app: &App,
    x: f32,
    y: f32,
) -> Option<((f32, f32), (f32, f32))> {
    own_develop(st)?;
    // The picture and the edit the frame's notifier draws and
    // analyzes by: the drawn picture, turned as it is drawn, under
    // the peeked edit or the panel's. The held one is excluded above.
    let ((dw, dh), _) = drawn_picture(st);
    let geometry = match &st.develop.peek {
        Some(peek) => peek.geometry.clone(),
        None => read_geometry(app),
    };
    let ((fx, fy), (fw, fh)) = view_in_frame(st, app, x, y, (dw as f32, dh as f32), &geometry);
    let (u, v) = (fx / fw, fy / fh);
    ((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)).then_some(((u, v), (fw, fh)))
}

/// A point of the view (logical pixels) in the frame's pixels from
/// its top left, and the frame's size, for a picture of `(sw, sh)`
/// under `geometry`.
fn view_in_frame(
    st: &State,
    app: &App,
    x: f32,
    y: f32,
    (sw, sh): (f32, f32),
    geometry: &Geometry,
) -> ((f32, f32), (f32, f32)) {
    let scale = app.window().scale_factor();
    let (vw, vh) = (
        app.get_view_width().max(1) as u32,
        app.get_view_height().max(1) as u32,
    );
    let zoom = effective_zoom(st, vw, vh);
    let frame = if app.get_crop_mode() {
        geometry.bounds(sw, sh)
    } else {
        geometry.frame(sw, sh)
    };
    let at = (
        (x * scale - vw as f32 / 2.0) / zoom + st.develop.center.0,
        (y * scale - vh as f32 / 2.0) / zoom + st.develop.center.1,
    );
    (at, frame.size)
}

#[cfg(test)]
thread_local! {
    /// The analysis picture the readout under the pointer reads in a
    /// test, which has no GPU to read one back from: its pixels, with
    /// the coverage in the alpha, and its width and height.
    pub(crate) static HOVER_PICTURE: std::cell::RefCell<Option<(Vec<slint::Rgba8Pixel>, usize, usize)>> =
        const { std::cell::RefCell::new(None) };
}

/// [`Renderer::analyzed_at`] for the state.
fn analyzed_at(st: &State, u: f32, v: f32, radius: f32) -> Option<scope::Reading> {
    #[cfg(test)]
    if let Some(read) = HOVER_PICTURE.with(|p| {
        p.borrow()
            .as_ref()
            .map(|(px, w, h)| scope::under_pointer(px, *w, *h, u, v, radius))
    }) {
        return read;
    }
    st.develop.renderer.as_ref()?.analyzed_at(u, v, radius)
}

/// The color under the pointer on the scopes: its marks and its
/// levels, read from the analysis picture the scopes binned, so each
/// mark sits on the trace it is part of. None off the picture, in
/// culling, where there are no scopes, while the camera's JPEG stands
/// in for a develop, which they do not describe, or before the first
/// analysis.
pub(crate) fn show_hover(st: &mut State, app: &App) {
    let at = st
        .develop
        .hover
        .filter(|_| !app.get_culling() && !app.get_placeholder())
        .and_then(|(x, y)| {
            let ((u, v), (fw, _)) = view_to_frame(st, app, x, y)?;
            // A few screen pixels about the pointer, so the marks
            // follow the picture's tones and not each blade of grass
            // in it; at 1:1 that is under one analysis pixel.
            let (vw, vh) = (
                app.get_view_width().max(1) as u32,
                app.get_view_height().max(1) as u32,
            );
            let radius = HOVER_REACH * app.window().scale_factor()
                / effective_zoom(st, vw, vh)
                / fw.max(1.0);
            Some((u, v, radius))
        });
    let reading = at.and_then(|(u, v, radius)| analyzed_at(st, u, v, radius));
    let Some(reading) = reading else {
        // On the open frame's own develop, its analysis not back yet:
        // a re-develop of this frame just landed and dropped the last
        // one's pixels. What is shown on this scope stays until the
        // new picture lands, rather than blink off and come back
        // without the glide.
        if at.is_some()
            && st
                .develop
                .hover_shown
                .is_some_and(|(scope, _)| scope == st.develop.scope)
        {
            return;
        }
        st.develop.hover_shown = None;
        if !app.get_scope_readout().is_empty() || app.get_scope_marks().row_count() > 0 {
            app.set_scope_readout("".into());
            app.set_scope_marks(ModelRc::default());
        }
        return;
    };
    // A move within the same reading, or the two reports of one
    // diagonal move, changes nothing on the scope.
    let shown = (st.develop.scope, reading);
    if st.develop.hover_shown == Some(shown) {
        return;
    }
    st.develop.hover_shown = Some(shown);
    let marks: Vec<ScopeMark> = scope::marks(st.develop.scope, reading.color, reading.across)
        .into_iter()
        .map(|m| ScopeMark {
            x: m.x,
            y: m.y,
            channel: m.channel as i32,
        })
        .collect();
    // The same rows moved where there are as many, so the marks glide
    // to the next color (the `.slint` side animates them) rather than
    // being made anew at it.
    let shown = app.get_scope_marks();
    match shown.as_any().downcast_ref::<VecModel<ScopeMark>>() {
        Some(rows) if rows.row_count() == marks.len() => {
            for (i, m) in marks.into_iter().enumerate() {
                rows.set_row_data(i, m);
            }
        }
        _ => app.set_scope_marks(ModelRc::new(VecModel::from(marks))),
    }
    app.set_scope_readout(scope::readout(reading.color).into());
    // Outside the scopes' selection the pixel is on no trace: its
    // marks show faded where it would be, against the selection's.
    app.set_scope_marks_outside(!reading.selected);
}

/// How far about the pointer the readout reads, logical pixels.
const HOVER_REACH: f32 = 3.0;

/// The pointer left the view: the color under it comes off the
/// scope, now, or a frame's time later while the state is borrowed
/// now: a delay, not the loop's next turn, so a nested event loop
/// holding the borrow does not spin it.
fn hover_left(state: Rc<RefCell<State>>, app_weak: slint::Weak<App>) {
    let Some(app) = app_weak.upgrade() else {
        return;
    };
    if let Ok(mut st) = state.try_borrow_mut() {
        st.develop.hover = None;
        show_hover(&mut st, &app);
        return;
    }
    slint::Timer::single_shot(std::time::Duration::from_millis(16), move || {
        hover_left(state, app_weak)
    });
}

/// A point of the view (logical pixels) in the crop's own units:
/// fractions of the leveled plane, the same a handle's drag reads and
/// writes. Where `view_to_source` goes on to the source through the
/// perspective and the turn, this stops a step earlier, at the plane
/// a fresh crop is drawn on.
pub(crate) fn view_to_plane(st: &State, app: &App, x: f32, y: f32) -> (f32, f32) {
    let scale = app.window().scale_factor();
    let (vw, vh) = (
        app.get_view_width().max(1) as u32,
        app.get_view_height().max(1) as u32,
    );
    let zoom = effective_zoom(st, vw, vh);
    let (sw, sh) = (
        st.develop.source_size.0 as f32,
        st.develop.source_size.1 as f32,
    );
    let geometry = read_geometry(app);
    let frame = if app.get_crop_mode() {
        geometry.bounds(sw, sh)
    } else {
        geometry.frame(sw, sh)
    };
    let (pw, ph) = geometry.plane_size(sw, sh);
    let r = (
        (x * scale - vw as f32 / 2.0) / zoom + st.develop.center.0 + frame.origin.0,
        (y * scale - vh as f32 / 2.0) / zoom + st.develop.center.1 + frame.origin.1,
    );
    (r.0 / pw, r.1 / ph)
}

/// The developed picture under a point of the view (logical pixels):
/// the mean of a few pixels about it, in the working space at the
/// white it was developed at; `None` off the picture or without one.
pub(crate) fn sample_view(st: &State, app: &App, x: f32, y: f32) -> Option<[f32; 3]> {
    sample_view_at(st, app, x, y, 2)
}

/// The same, over a box of `reach` pixels each way instead of the
/// droppers' usual two: what the defringe's dropper takes its local
/// mean over.
pub(crate) fn sample_view_at(
    st: &State,
    app: &App,
    x: f32,
    y: f32,
    reach: u32,
) -> Option<[f32; 3]> {
    let (u, v) = view_to_source(st, app, x, y);
    let sw = st.develop.source_size.0 as f32;
    let (px, py) = ((u * sw).round() as i64, (v * sw).round() as i64);
    let (size, lag) = drawn_picture(st);
    if px < 0 || py < 0 || px >= i64::from(size.0) || py >= i64::from(size.1) {
        return None;
    }
    // The texture may still be the develop at the turn before; the
    // pixel is read where it holds this one.
    let (px, py) = placeholder::texel_of((px, py), size, lag);
    match st.develop.renderer.as_ref()?.sample(px, py, reach) {
        Ok(sample) => sample,
        Err(e) => {
            tracing::warn!("pick: {e:#}");
            None
        }
    }
}

/// A sampled pixel taken to the panel's white and through the
/// panel's look to the stages the droppers read.
pub(crate) fn pick_stages(st: &mut State, app: &App, px: [f32; 3]) -> finish::Picked {
    let key = (app.get_as_shot(), app.get_temperature(), app.get_tint());
    let m = preview_white(st, key);
    let px: [f32; 3] = std::array::from_fn(|r| m[r][0] * px[0] + m[r][1] * px[1] + m[r][2] * px[2]);
    let edit = read_edit(app, &st.develop.edit, st.develop.target);
    finish::pick(
        px,
        &edit.light.effective(),
        &edit.acting_mixer(),
        &edit.color,
        &edit.bw,
        &edit.tint,
        &edit.curves.bake_with(&edit.grading),
        st.develop.source,
        edit.display_curve,
    )
}

/// The zoom in words for the panel: fitted, or a percentage of 1:1.
pub(crate) fn zoom_label(zoom: f32) -> slint::SharedString {
    if zoom > 0.0 {
        format!("{}%", (zoom * 100.0).round() as i32).into()
    } else {
        "Fit".into()
    }
}

/// [`zoom::effective`] for the state: the zoom it holds, the cell
/// the compare view gives, and the size of the picture on the GPU.
pub(crate) fn effective_zoom(st: &State, vw: u32, vh: u32) -> f32 {
    zoom::effective(
        st.develop.zoom,
        view_cell(st, vw, vh),
        st.develop.image_size,
    )
}

/// [`zoom::cell`] for the state: how many frames the compare view
/// is showing, if it is showing any.
pub(crate) fn view_cell(st: &State, vw: u32, vh: u32) -> (u32, u32) {
    let compare = st
        .culling
        .cull
        .as_ref()
        .or(st.culling.hold.as_ref())
        .map_or(1, |c| c.compare);
    zoom::cell(vw, vh, compare)
}

/// The monitor's table on the renderer, for the panel's output
/// space, its proof and its monitor, rebuilt when any of them moved.
pub(crate) fn sync_display(
    lut_for: &mut Option<(
        export::Space,
        Option<display::Proof>,
        display::MonitorProfile,
    )>,
    encoded_lut_for: &mut Option<display::MonitorProfile>,
    monitors: &[display::Monitor],
    app: &App,
    renderer: &mut Renderer,
) {
    // The output space is the export sheet's, and the table takes it
    // to the monitor, through the proof when one is on. The
    // monitor's profile is the panel's.
    let space =
        export::Space::from_name(app.get_export_space().as_str()).unwrap_or(export::Space::Srgb);
    renderer.set_output(space.matrix());
    let proof = app.get_proofing().then(|| proof_settings(app)).flatten();
    let (monitor, from) = monitor_settings(app, monitors);
    let key = (space, proof.clone(), monitor.clone());
    if lut_for.as_ref() != Some(&key) {
        if lut_for.as_ref().map(|k| &k.2) != Some(&monitor) {
            app.set_display_note(monitor_note(&monitor, &from).into());
        }
        let srgb = display::MonitorProfile::Srgb;
        let lut = display::Lut3d::build(space, &monitor, proof.as_ref())
            .or_else(|e| {
                tracing::warn!("proof: {e:#}");
                display::Lut3d::build(space, &monitor, None)
            })
            .or_else(|e| {
                tracing::warn!("monitor profile: {e:#}");
                display::Lut3d::build(space, &srgb, None)
            })
            .unwrap_or_else(|e| {
                tracing::warn!("display table: {e:#}");
                display::Lut3d::identity()
            });
        renderer.set_display_lut(&lut);
        *lut_for = Some(key);
    }
    // The encoded path's table: the camera's JPEG is sRGB whatever
    // the export sheet's space, and a proof is a look at one export,
    // so neither reaches it; the monitor's profile alone.
    if encoded_lut_for.as_ref() != Some(&monitor) {
        let lut = display::Lut3d::build(export::Space::Srgb, &monitor, None).unwrap_or_else(|e| {
            tracing::warn!("monitor profile for the camera's JPEG: {e:#}");
            display::Lut3d::identity()
        });
        renderer.set_encoded_lut(&lut);
        *encoded_lut_for = Some(monitor);
    }
}

/// `--snapshot`: the window as drawn, a moment on, from the event
/// loop rather than from inside a frame, once the picture asked for
/// is on screen (`ready`). A scroll asked for is applied now that the
/// panel's sections have their height (set before the first layout,
/// the ScrollView puts it back to the top) and the capture waits a
/// further moment for that frame.
///
/// `then_quit` ends the run with the capture, which is what a batch
/// run wants. A capture of the camera's picture standing in for a
/// develop does not: that develop is on the worker's GPU queue this
/// moment, and a process that exits in the middle of one dies in the
/// driver rather than at its own hand.
/// What `--sheet` or `--tool` puts on screen before a snapshot. No
/// flag and no key reaches a sheet or a Crop-tab tool, so a capture
/// of one asks for it here, once the picture is up; the split of
/// `app.slint` was checked with these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    /// The export sheet.
    Export,
    /// The preset sheet, as Save preset opens it.
    Preset,
    /// The sync sheet over the set `--also` made.
    Sync,
    /// The sync applied on the sheet's defaults in the same turn the
    /// sheet opens, so it is never drawn: the sidecars written, the
    /// status line saying so.
    Synced,
    /// The first stored preset clicked over the set `--also` made, no
    /// sheet involved: the current frame through the panel, the rest
    /// through `panel::sync::lay_over_targets`.
    PresetOntoSet,
    /// The first stored preset's bin clicked: its removal asked,
    /// waiting on the Remove that names it.
    PresetRemove,
    /// The current frame copied and the paste sheet opened over the
    /// set `--also` made.
    Paste,
    /// The same paste applied on the sheet's sections in the same
    /// turn, so the sheet is never drawn.
    Pasted,
    /// The frame menu opened over this row of the strip, or of the
    /// grid when it is open (`--menu`).
    Menu(usize),
    /// The import sheet, as Ctrl+Shift+I opens it.
    Import,
    /// The import sheet's import started as soon as its source is
    /// read, as its button would, for a snapshot of the run or of the
    /// folder it opens.
    Imported,
    /// The import sheet on Add in place, `--import`'s folder read as
    /// its folder.
    ImportInPlace,
    /// The model sheet, with sample text where the download's goes.
    Fetch,
    /// The preset import's report sheet, over sample presets: two in,
    /// one passing things over, and one refused. Nothing is imported.
    PresetReport,
    /// The lens profiles' download as the first launch offers it.
    Lenses,
    /// The Lightroom catalog import's sheet over `--lightroom`'s
    /// catalog, at its roots, its dry run's report, or written (the
    /// write run as the Write button runs it, for a capture of a
    /// scratch catalog's import).
    Lightroom(LightroomStep),
    /// RawTherapee's profiles beside the open folder's frames: the
    /// offer under the folder's name (answered or not), their import's
    /// sheet at its dry run's report, or written (the write run as the
    /// Write button runs it, for a capture of a scratch folder's).
    RawTherapee(RawTherapeeStep),
    /// The settings sheet.
    Settings,
    /// The camera match's sheet, as the Look section's button opens it.
    Match,
    /// The camera match's sheet with its Fit pressed once the scope is
    /// read, for a snapshot of the run's report.
    Matched,
    /// The camera match's sheet as the Look section's refit opens it,
    /// over the chosen look's group.
    Refit,
    /// The remove-a-look sheet, as the Look section's Remove opens it,
    /// over the first look in the directory when none is chosen. Never
    /// answered: a capture never touches the files.
    LookRemove,
    /// The rename-a-look sheet, as the Look section's Rename opens it,
    /// over the first look in the directory when none is chosen. A
    /// capture never renames, whatever `--keys` answers.
    LookRename,
    /// The shift-capture-time sheet over the selection, as the frame
    /// menu's Shift capture time... opens it.
    TimeShift,
    /// The root sheet over the library's first root, as its chip's
    /// Rename... opens it.
    RootName,
    /// The delete sheet over the selection, as the Delete key opens
    /// it. Never answered: a capture never deletes.
    Delete,
    /// The archive sheet over the open folder, as the grid header's
    /// Back up opens it, once its look is in. Never answered: a capture
    /// never copies.
    Archive,
    /// The Remove rejects sheet for the first archive, over the open
    /// folder's rejects folder, once its look is in. Never answered: a
    /// capture never moves or deletes.
    ArchiveRejects,
    /// A meta key (a rating, a flag or a color label) pressed over the
    /// selection (`--press`), for a snapshot of what follows it.
    Key(char),
    /// The crop tool, on the Crop tab.
    Crop,
    /// The level tool, on the Crop tab.
    Level,
    /// A vertical perspective guide in hand, on the Crop tab.
    Guide,
    /// The masks panel's People menu, open over the chosen mask.
    Parts,
    /// The masks panel's Landscape button pressed on the chosen mask:
    /// its menu of what the model found, once the answer is in.
    Landscape,
    /// A part chosen from the People menu by its label (`--part`).
    Part(&'static str),
    /// The grid's Sort menu, open (with `--grid`).
    Sort,
    /// The keys sheet, as ? opens it.
    Keys,
    /// The Help menu at the left pane's foot, open.
    Help,
}

/// Which step of the RawTherapee profiles' offer and sheet a capture
/// shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawTherapeeStep {
    Offer,
    Report,
    Written,
}

/// Which step of the Lightroom import's sheet a capture shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LightroomStep {
    Roots,
    Report,
    Written,
}

impl Shown {
    /// `--sheet`'s names.
    pub(crate) fn sheet(name: &str) -> Result<Self, String> {
        match name {
            "export" => Ok(Self::Export),
            "preset" => Ok(Self::Preset),
            "fetch" => Ok(Self::Fetch),
            "lenses" => Ok(Self::Lenses),
            "settings" => Ok(Self::Settings),
            "sync" => Ok(Self::Sync),
            "synced" => Ok(Self::Synced),
            "preset-onto-set" => Ok(Self::PresetOntoSet),
            "preset-remove" => Ok(Self::PresetRemove),
            "preset-report" => Ok(Self::PresetReport),
            "paste" => Ok(Self::Paste),
            "pasted" => Ok(Self::Pasted),
            "import" => Ok(Self::Import),
            "imported" => Ok(Self::Imported),
            "import-in-place" => Ok(Self::ImportInPlace),
            "match" => Ok(Self::Match),
            "matched" => Ok(Self::Matched),
            "refit" => Ok(Self::Refit),
            "look-remove" => Ok(Self::LookRemove),
            "look-rename" => Ok(Self::LookRename),
            "time-shift" => Ok(Self::TimeShift),
            "root-name" => Ok(Self::RootName),
            "delete" => Ok(Self::Delete),
            "archive" => Ok(Self::Archive),
            "archive-rejects" => Ok(Self::ArchiveRejects),
            "parts" => Ok(Self::Parts),
            "landscape" => Ok(Self::Landscape),
            "lightroom" => Ok(Self::Lightroom(LightroomStep::Roots)),
            "lightroom-report" => Ok(Self::Lightroom(LightroomStep::Report)),
            "lightroom-written" => Ok(Self::Lightroom(LightroomStep::Written)),
            "rawtherapee-offer" => Ok(Self::RawTherapee(RawTherapeeStep::Offer)),
            "rawtherapee" => Ok(Self::RawTherapee(RawTherapeeStep::Report)),
            "rawtherapee-written" => Ok(Self::RawTherapee(RawTherapeeStep::Written)),
            "sort" => Ok(Self::Sort),
            "keys" => Ok(Self::Keys),
            "help" => Ok(Self::Help),
            _ => Err(format!(
                "want export, preset, fetch, lenses, settings, sync, synced, \
                 preset-onto-set, preset-remove, preset-report, paste, pasted, import, \
                 imported, import-in-place, match, matched, refit, look-remove, look-rename, \
                 time-shift, root-name, \
                 delete, archive, archive-rejects, parts, landscape, lightroom, \
                 lightroom-report, lightroom-written, rawtherapee-offer, rawtherapee, \
                 rawtherapee-written, sort, keys or help, not {name}"
            )),
        }
    }

    /// `--part`'s labels: the People menu's.
    pub(crate) fn part(label: &str) -> Result<Self, String> {
        match crate::panel::parts::by_label(label) {
            Some(p) => Ok(Self::Part(p.label)),
            None => Err(format!(
                "want one of the People menu's labels ({}), not {label}",
                greycard_edit::mask::PARTS
                    .iter()
                    .map(|p| p.label)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// `--tool`'s names.
    pub(crate) fn tool(name: &str) -> Result<Self, String> {
        match name {
            "crop" => Ok(Self::Crop),
            "level" => Ok(Self::Level),
            "guide" => Ok(Self::Guide),
            _ => Err(format!("want crop, level or guide, not {name}")),
        }
    }

    /// Put it on screen: what the key or the button would have done.
    /// A tool brings the Crop tab with it.
    pub(crate) fn open(self, app: &App) {
        match self {
            Self::Export => app.set_export_open(true),
            Self::Preset => app.invoke_preset_save_open(),
            Self::Sync => app.invoke_sync_open_asked(),
            Self::Synced => {
                app.invoke_sync_open_asked();
                app.invoke_sync_applied();
            }
            Self::PresetOntoSet => app.invoke_preset_applied(0),
            // The row with the longest name, the hardest to fit;
            // nothing with no presets to ask about.
            Self::PresetRemove => {
                let longest = app
                    .get_preset_names()
                    .iter()
                    .enumerate()
                    .max_by_key(|(i, n)| (n.chars().count(), std::cmp::Reverse(*i)))
                    .map(|(i, _)| i as i32);
                if let Some(row) = longest {
                    app.set_preset_confirm_remove(row);
                }
            }
            Self::Paste => {
                app.invoke_copy_asked();
                app.invoke_paste_asked();
            }
            Self::Pasted => {
                app.invoke_copy_asked();
                app.invoke_paste_asked();
                app.invoke_sync_applied();
            }
            Self::Menu(row) => app.set_menu_at(row as i32),
            Self::Key(key) => {
                app.invoke_meta_key(key.to_string().into());
            }
            Self::Import => app.invoke_import_asked(),
            Self::Imported => {
                app.invoke_import_asked();
                crate::panel::import::start_when_read(app, 100);
            }
            Self::ImportInPlace => {
                app.invoke_import_asked();
                app.set_import_mode(crate::panel::import::ADD_IN_PLACE.into());
                if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                    let mut st = state.borrow_mut();
                    match st.import.source.clone() {
                        Some(dir) => crate::panel::import::read_place(&mut st, app, dir),
                        None => tracing::warn!("--sheet import-in-place wants --import DIR"),
                    }
                }
            }
            Self::Fetch => {
                app.set_fetch_title("Download the Subject model?".into());
                app.set_fetch_text(
                    "312 MB from the model registry.\nLicense: Apache-2.0.\nKept in the models folder."
                        .into(),
                );
                app.set_fetch_note("Sample text: this sheet was opened for a snapshot.".into());
                app.set_fetch_open(true);
            }
            Self::PresetReport => {
                use crate::panel::assets::{Brought, import_report, show_import_report};
                let sample = [
                    Brought::In {
                        name: "Soft Matte".into(),
                        carried: "4 sections".into(),
                        passed: vec![
                            "ColorNoiseReduction".into(),
                            "RedHue".into(),
                            "CameraProfile Camera Portrait".into(),
                            "masks".into(),
                        ],
                    },
                    Brought::In {
                        name: "Fine Grain".into(),
                        carried: "1 section".into(),
                        passed: Vec::new(),
                    },
                    Brought::Refused(
                        "warm-film.xmp was not imported: there is a preset called \"Warm Film\" \
                         already."
                            .into(),
                    ),
                ];
                show_import_report(app, import_report(&sample));
            }
            Self::Lenses => {
                if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                    crate::panel::assets::offer_lenses(&mut state.borrow_mut(), app, true);
                }
            }
            Self::Settings => app.invoke_settings_asked(),
            Self::Lightroom(step) => {
                if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                    let mut st = state.borrow_mut();
                    match st.lightroom.catalog.clone() {
                        Some(catalog) => {
                            st.lightroom.then = match step {
                                LightroomStep::Roots => crate::panel::lightroom::Then::Nothing,
                                LightroomStep::Report => crate::panel::lightroom::Then::Report,
                                LightroomStep::Written => crate::panel::lightroom::Then::Write,
                            };
                            crate::panel::lightroom::open(&mut st, app, catalog);
                        }
                        None => tracing::warn!("--sheet lightroom wants --lightroom CATALOG"),
                    }
                }
            }
            Self::RawTherapee(step) => {
                if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                    let mut st = state.borrow_mut();
                    match (step, st.views.recent.open.clone()) {
                        (_, None) => {
                            tracing::warn!("--sheet rawtherapee wants a folder open")
                        }
                        (RawTherapeeStep::Offer, Some(_)) => {
                            st.rawtherapee.force_offer = true;
                            crate::panel::rawtherapee::folder_opened(&mut st, app);
                        }
                        (RawTherapeeStep::Report | RawTherapeeStep::Written, Some(dir)) => {
                            if step == RawTherapeeStep::Written {
                                st.rawtherapee.then = crate::panel::rawtherapee::Then::Write;
                            }
                            crate::panel::rawtherapee::open(&mut st, app, dir);
                        }
                    }
                }
            }
            Self::Match => app.invoke_match_asked(),
            Self::Refit => app.invoke_look_refit_asked(),
            Self::LookRemove | Self::LookRename => {
                if app.get_look_name() == "none"
                    && let Some(state) = STATE.with(|s| s.borrow().clone())
                {
                    let mut st = state.borrow_mut();
                    st.develop.looks = greycard_edit::look::list();
                    if let Some(first) = st.develop.looks.first().map(|e| e.name.clone()) {
                        app.set_look_name(first.into());
                    }
                }
                if self == Self::LookRemove {
                    app.invoke_look_remove_asked();
                } else {
                    app.invoke_look_rename_asked();
                }
            }
            Self::Matched => {
                app.invoke_match_asked();
                crate::panel::camera_match::fit_when_read(app, 100);
            }
            Self::TimeShift => app.invoke_time_shift_asked(),
            Self::Delete => app.invoke_delete_asked("selection".into()),
            Self::Archive => app.invoke_archive_header_pressed(0),
            Self::ArchiveRejects => app.invoke_archive_rejects_asked(0),
            Self::RootName => {
                if let Some(first) = app.get_library_roots().row_data(0) {
                    app.invoke_library_root_rename(first.path);
                }
            }
            Self::Crop => {
                app.set_panel_tab("Crop".into());
                app.set_crop_mode(true);
                app.invoke_crop_toggled();
            }
            Self::Level => {
                app.set_panel_tab("Crop".into());
                app.set_level_mode(true);
            }
            Self::Guide => {
                app.set_panel_tab("Crop".into());
                app.set_guide_mode("Vertical".into());
            }
            Self::Parts => {
                app.set_panel_tab("Masks".into());
                app.set_parts_menu_shown(true);
            }
            Self::Landscape => {
                app.set_panel_tab("Masks".into());
                app.invoke_find_landscape("Add".into());
            }
            Self::Part(label) => {
                app.set_panel_tab("Masks".into());
                app.invoke_add_part(label.into(), "Add".into());
            }
            Self::Sort => app.set_sort_menu_shown(true),
            Self::Keys => app.set_keys_open(true),
            Self::Help => app.set_help_menu_shown(true),
        }
        app.window().request_redraw();
    }
}

/// `--keys`, kept here since the snapshot is scheduled while the
/// state is borrowed.
pub(crate) static SNAPSHOT_KEYS: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// `then`, once no learned mask and no part's people are being waited
/// on (or after `tries` looks a fifth of a second apart), and half a
/// second later, for the frame that shows what came.
fn when_quiet(state: Rc<RefCell<State>>, then: impl FnOnce() + 'static, tries: u32) {
    let quiet = state.try_borrow().is_ok_and(|st| {
        st.develop.asked.is_empty()
            && st.develop.part_pending.is_none()
            && st.develop.landscape_pending.is_none()
            && !st.lightroom.busy()
            && !st.rawtherapee.busy()
    });
    if quiet || tries == 0 {
        slint::Timer::single_shot(std::time::Duration::from_millis(500), then);
    } else {
        slint::Timer::single_shot(std::time::Duration::from_millis(200), move || {
            when_quiet(state, then, tries - 1)
        });
    }
}

pub(crate) fn schedule_snapshot(
    snapshot: &mut Option<PathBuf>,
    panel_scroll: &mut Option<f32>,
    shown: &mut Option<Shown>,
    app: &App,
    state: &Rc<RefCell<State>>,
    ready: bool,
    then_quit: bool,
) {
    // `--keys` with no snapshot: the keys are still sent once the
    // picture is up, and the session goes on as any other.
    static KEYS_SENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let path = match snapshot.take_if(|_| ready) {
        // The keys go with the snapshot, once: a later develop (a list
        // a click opened) must not send them again.
        Some(path) => {
            KEYS_SENT.store(true, std::sync::atomic::Ordering::Relaxed);
            Some(path)
        }
        None if ready
            && SNAPSHOT_KEYS.get().is_some()
            && !KEYS_SENT.swap(true, std::sync::atomic::Ordering::Relaxed) =>
        {
            None
        }
        None => return,
    };
    let app_weak = app.as_weak();
    let keys: Vec<String> = SNAPSHOT_KEYS
        .get()
        .map(|k| k.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    let state = state.clone();
    let waiting = state.clone();
    let scroll = panel_scroll.take();
    let shown = shown.take();
    let capture = move || {
        let Some(path) = path else {
            return;
        };
        if let Some(app) = app_weak.upgrade() {
            let wrote = match app.window().take_snapshot() {
                Ok(pixels) => {
                    let saved = image::RgbaImage::from_raw(
                        pixels.width(),
                        pixels.height(),
                        pixels.as_bytes().to_vec(),
                    )
                    .map(|img| img.save(&path));
                    match saved {
                        Some(Ok(())) => {
                            tracing::info!("wrote {}", path.display());
                            true
                        }
                        Some(Err(e)) => {
                            tracing::error!("snapshot: {e}");
                            false
                        }
                        None => {
                            tracing::error!("snapshot: bad size");
                            false
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("snapshot: {e}");
                    false
                }
            };
            // The picture asked for is not there: a failed run,
            // whatever stopped it.
            if !wrote {
                state.borrow_mut().launch.failed = true;
            }
            if then_quit {
                let _ = slint::quit_event_loop();
            }
        }
    };
    let moment = std::time::Duration::from_millis(300);
    // A sheet or a tool asked for is opened after the scroll and given
    // longer to settle: a sheet fades in.
    let opened = std::time::Duration::from_millis(1500);
    // `--keys`: each sent to the window as a press and a release, a
    // fifth of a second apart, once what was asked for is up.
    let then_keys = {
        let app_weak = app.as_weak();
        move || {
            let step = std::time::Duration::from_millis(200);
            let n = keys.len() as u32;
            for (i, key) in keys.into_iter().enumerate() {
                let app_weak = app_weak.clone();
                slint::Timer::single_shot(step * (i as u32 + 1), move || {
                    let Some(app) = app_weak.upgrade() else {
                        return;
                    };
                    use slint::platform::{Key, PointerEventButton, WindowEvent};
                    // `wait`: a step with nothing sent.
                    if key == "wait" {
                        return;
                    }
                    // A pointer token: move:X,Y, press:X,Y, release:X,Y
                    // or click:X,Y (a press and a release), logical
                    // pixels; a right click is rclick:X,Y.
                    if let Some((what, at)) = key.split_once(':')
                        && let Some((x, y)) = at.split_once(',')
                        && let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>())
                    {
                        let position = slint::LogicalPosition::new(x, y);
                        let button = if what == "rclick" {
                            PointerEventButton::Right
                        } else {
                            PointerEventButton::Left
                        };
                        let w = app.window();
                        match what {
                            "move" => w.dispatch_event(WindowEvent::PointerMoved { position }),
                            "press" => {
                                w.dispatch_event(WindowEvent::PointerMoved { position });
                                w.dispatch_event(WindowEvent::PointerPressed { position, button });
                            }
                            "release" => {
                                w.dispatch_event(WindowEvent::PointerReleased { position, button });
                            }
                            _ => {
                                w.dispatch_event(WindowEvent::PointerMoved { position });
                                w.dispatch_event(WindowEvent::PointerPressed { position, button });
                                w.dispatch_event(WindowEvent::PointerReleased { position, button });
                            }
                        }
                        return;
                    }
                    let text: slint::SharedString = match key.as_str() {
                        "down" => Key::DownArrow.into(),
                        "up" => Key::UpArrow.into(),
                        "left" => Key::LeftArrow.into(),
                        "right" => Key::RightArrow.into(),
                        "enter" => Key::Return.into(),
                        "esc" => Key::Escape.into(),
                        "tab" => Key::Tab.into(),
                        "home" => Key::Home.into(),
                        "end" => Key::End.into(),
                        "f6" => Key::F6.into(),
                        "f7" => Key::F7.into(),
                        "f8" => Key::F8.into(),
                        other => other.into(),
                    };
                    tracing::info!("keys: {key}");
                    app.window()
                        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
                    app.window()
                        .dispatch_event(WindowEvent::KeyReleased { text });
                });
            }
            // A mask or a part asked for by what was opened is waited
            // for, so the capture shows it.
            slint::Timer::single_shot(step * (n + 1) + opened, move || {
                when_quiet(waiting, capture, 150)
            });
        }
    };
    let open_then = {
        let app_weak = app.as_weak();
        move || match (shown, app_weak.upgrade()) {
            (Some(what), Some(app)) => {
                what.open(&app);
                slint::Timer::single_shot(opened, then_keys);
            }
            _ => then_keys(),
        }
    };
    let app_weak = app.as_weak();
    slint::Timer::single_shot(moment, move || match scroll {
        Some(px) => {
            if let Some(app) = app_weak.upgrade() {
                app.set_panel_scroll(-px);
                app.window().request_redraw();
            }
            slint::Timer::single_shot(moment, open_then);
        }
        None => open_then(),
    });
}

/// `--screenshot`: the viewport's texture as a PNG, once the picture
/// asked for is on screen, then quit.
pub(crate) fn write_screenshot(
    screenshot: &mut Option<PathBuf>,
    failed: &mut bool,
    renderer: &Renderer,
    texture: &gpu::Texture,
    ready: bool,
) {
    let Some(path) = screenshot.take_if(|_| ready) else {
        return;
    };
    match renderer.read_back(texture) {
        Ok(png) => {
            if let Err(e) = png.save(&path) {
                tracing::error!("screenshot: {e}");
                *failed = true;
            } else {
                tracing::info!("wrote {}", path.display());
            }
        }
        Err(e) => {
            tracing::error!("screenshot: {e}");
            *failed = true;
        }
    }
    let _ = slint::quit_event_loop();
}

/// An ICC profile from the desktop's chooser, opening beside the
/// current one or where profiles are kept; `apply` takes the choice
/// on the event loop.
pub(crate) fn choose_icc(
    app: &App,
    title: &'static str,
    current: PathBuf,
    apply: impl Fn(&App, PathBuf) + Send + 'static,
) {
    let start = current
        .parent()
        .filter(|p| p.is_dir())
        .map(Path::to_path_buf)
        .or_else(|| {
            // Where each platform keeps its profiles, the user's own
            // first.
            let home = dirs::home_dir()?;
            let folders: &[PathBuf] = if cfg!(target_os = "macos") {
                &[
                    home.join("Library/ColorSync/Profiles"),
                    PathBuf::from("/Library/ColorSync/Profiles"),
                ]
            } else if cfg!(windows) {
                &[PathBuf::from(r"C:\Windows\System32\spool\drivers\color")]
            } else {
                &[
                    home.join(".local/share/icc"),
                    PathBuf::from("/usr/share/color/icc"),
                ]
            };
            folders.iter().find(|p| p.is_dir()).cloned().or(Some(home))
        })
        .unwrap_or_else(|| PathBuf::from("/"));
    let app_weak = app.as_weak();
    chooser::choose_open(
        title,
        start,
        (
            "ICC profiles".to_string(),
            vec!["*.icc".into(), "*.icm".into()],
        ),
        move |chosen| {
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                match chosen {
                    Ok(Some(path)) => {
                        apply(&app, path);
                        app.window().request_redraw();
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!("{title}: file chooser: {e:#}");
                        app.set_status(chooser::chooser_failed(&e).into());
                    }
                }
            });
        },
    );
}

/// The monitor's profile as the settings keep it: "System", a
/// standard's name, or the file's path; a file not yet chosen is
/// System again.
pub(crate) fn display_key(app: &App) -> String {
    let choice = app.get_display_profile();
    if choice.as_str() == FILE {
        let file = app.get_display_file();
        if file.is_empty() {
            SYSTEM.to_string()
        } else {
            file.to_string()
        }
    } else {
        choice.to_string()
    }
}

/// The proof's profile file, and with it the file's stem, which the
/// badge on the picture names it by.
pub(crate) fn set_proof_file(app: &App, file: &str) {
    app.set_proof_file(file.into());
    let stem = std::path::Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    app.set_proof_file_stem(stem.into());
}

/// The soft proof as the panel has it, or none while its profile is
/// a file not yet chosen.
pub(crate) fn proof_settings(app: &App) -> Option<display::Proof> {
    let choice = app.get_proof_profile();
    let profile = if choice.as_str() == "File" {
        let file = app.get_proof_file();
        if file.is_empty() {
            return None;
        }
        display::ProofProfile::File(PathBuf::from(file.as_str()))
    } else {
        display::ProofProfile::Space(export::Space::from_name(choice.as_str())?)
    };
    Some(display::Proof {
        profile,
        intent: display::ProofIntent::from_name(app.get_proof_intent().as_str())
            .unwrap_or_default(),
        warn: app.get_gamut_warning(),
    })
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    // A change the shader handles: redraw, and remember it.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_view_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // A look control reached in culling: out of it, and the
            // frame develops so the control has a picture to act on.
            if st.culling.cull.is_some() {
                let with = control_over_frame(&mut st, &app);
                leave_cull(&mut st, &app, &worker, with);
                return;
            }
            crate::panel::cull::leave_developed(&mut st, &app);
            crate::panel::mask::look_edited(&mut st, &app);
            crate::panel::look_preview::panel_moved(&st, &app);
            // A mask's Detail that needs a plane the develop has not
            // made: the panel's edit is the develop's, once the
            // pointer has rested, as a develop slider's is.
            if crate::panel::mask::detail_asks(&mut st, &app) {
                st.develop.edit = read_edit(&app, &st.develop.edit, st.develop.target);
                crate::panel::edit::develop_soon(
                    &mut st,
                    state.clone(),
                    worker.clone(),
                    app.as_weak(),
                );
                return;
            }
            app.window().request_redraw();
            schedule_save(&mut st, app.as_weak());
        });
    }
    // The scope the panel shows. The bins are taken again for it, so
    // the picture arrives with the next read back.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_scope_changed(move |name| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if let Some(chosen) = scope::Scope::from_name(name.as_str()) {
                let mut st = state.borrow_mut();
                st.develop.scope = chosen;
                show_hover(&mut st, &app);
                app.window().request_redraw();
            }
        });
    }
    // The pointer over the view: the color under it, on the scope.
    // Slint reports it as a change, after the event that made it; a
    // borrow held then (a test dispatching events with the state in
    // hand) skips one move rather than panicking. A leave is not
    // skipped, or the readout would stand with the pointer gone: it
    // is tried again from the event loop.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_view_hovered(move |on, x, y| {
            if !on {
                hover_left(state.clone(), app_weak.clone());
            } else if let Some(app) = app_weak.upgrade()
                && let Ok(mut st) = state.try_borrow_mut()
            {
                st.develop.hover = Some((x, y));
                show_hover(&mut st, &app);
            }
        });
    }
    // The clipping warnings, the proof and the scopes' selection
    // toggle are the panel's; a change is a frame.
    {
        let app_weak = app.as_weak();
        app.on_scope_selection_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                app.window().request_redraw();
            }
        });
        let app_weak = app.as_weak();
        app.on_warn_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                app.window().request_redraw();
            }
        });
        let app_weak = app.as_weak();
        app.on_proof_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                app.window().request_redraw();
            }
        });
        // A profile file for the proof, from the desktop's chooser.
        let app_weak = app.as_weak();
        app.on_choose_proof_file(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let current = PathBuf::from(app.get_proof_file().as_str());
            choose_icc(&app, "Proof profile", current, |app, path| {
                set_proof_file(app, &path.to_string_lossy());
                app.set_proof_profile(FILE.into());
                app.set_proofing(true);
            });
        });
        // The monitor's profile: a change is a table and a frame.
        let app_weak = app.as_weak();
        app.on_display_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                app.window().request_redraw();
            }
        });
        let app_weak = app.as_weak();
        app.on_choose_display_file(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let current = PathBuf::from(app.get_display_file().as_str());
            choose_icc(&app, "Monitor profile", current, |app, path| {
                app.set_display_file(path.to_string_lossy().as_ref().into());
                app.set_display_profile(FILE.into());
            });
        });
        // The canvas's color: a frame, the Rectangle behind the
        // viewport following it at once through `canvas-colors`.
        let app_weak = app.as_weak();
        app.on_canvas_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                app.window().request_redraw();
            }
        });
    }
    // The Auto white balance: the white read off the whole picture.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_wb_auto(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            auto_white(state.borrow_mut(), &app);
        });
    }
    // The droppers: a button puts one in hand, or takes it back; a
    // press on the view reads the developed picture there, a drag
    // moves what the press chose, a release records it.
    {
        let app_weak = app.as_weak();
        app.on_pick_started(move |kind| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let same = app.get_picking() == kind;
            app.invoke_stop_placing();
            if !same {
                app.set_picking(kind.clone());
                app.set_status(picking_hint(kind.as_str()).into());
                // The defringe's dropper wants the pass's input on
                // screen, not its output: see `edit_to_develop`.
                if kind == "Defringe" {
                    app.invoke_develop_changed();
                }
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_pick_pressed(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(px) = sample_view(&st, &app, x, y) else {
                return;
            };
            match app.get_picking().as_str() {
                "White" => {
                    if let Err(miss) = white_from_neutral(st, &app, px) {
                        app.set_status(miss.said(false).into());
                    }
                }
                "Curve" => {
                    // A point-curve tool: not while the parametric
                    // curve is showing, where there is no point to see.
                    if parametric_mode(&app) {
                        return;
                    }
                    // The pixel at the panel's white, through the
                    // panel's look to where the curve reads it.
                    let picked = pick_stages(&mut st, &app, px);
                    let channel = current_channel(&app);
                    let value = match channel {
                        Channel::Rgb => picked.luma,
                        Channel::Red => picked.encoded[0],
                        Channel::Green => picked.encoded[1],
                        Channel::Blue => picked.encoded[2],
                        Channel::RedGreen | Channel::BlueYellow => picked.lightness,
                    }
                    .clamp(0.0, 1.0);
                    let mut points = curve_points(&app, channel);
                    let index = picked_point(&mut points, value);
                    st.develop.picking = Some(Picking::Curve {
                        channel,
                        index,
                        y0: points[index][1],
                        at: y,
                    });
                    st.develop.curve_drag = Some(index);
                    set_curve_points(&app, channel, &points);
                    select_point(&app, Some(index));
                    app.set_curve_image(draw_curve(&app, st.develop.bins.as_deref()));
                    app.window().request_redraw();
                }
                "Defringe" => {
                    // The pass gates on the direction a pixel's chroma
                    // departs from its neighborhood's; the dropper
                    // reads that same quantity here, the point against
                    // a wider box around it, and hands the hue to
                    // whichever window is nearer. The two boxes are
                    // flat where the pass takes a Gaussian, and the
                    // outer one is a couple of pixels wider than the
                    // pass's averaging window; neither changes which
                    // way the deviation points, which is all that is
                    // read off it. No white or look between: the
                    // defringe runs on the base, and the base is what
                    // the sample is. The base is developed with the
                    // pass off while this dropper is out, so what is
                    // sampled is the pass's input; see
                    // `edit_to_develop`.
                    let radius = app
                        .get_lens_defringe_radius()
                        .clamp(defringe::MIN_RADIUS, defringe::MAX_RADIUS);
                    let reach = (2.0 * radius).ceil() as u32 + 1;
                    let Some(mean) = sample_view_at(&st, &app, x, y, reach) else {
                        return;
                    };
                    let Some(hue) = defringe::deviation_hue(px, mean, DEFRINGE_PICK_FLOOR) else {
                        app.set_status(
                            "no fringe there: that color does not depart from its neighbors'"
                                .into(),
                        );
                        return;
                    };
                    if defringe::nearer_window(
                        hue,
                        app.get_lens_defringe_purple_center(),
                        app.get_lens_defringe_green_center(),
                    ) {
                        app.set_lens_defringe_purple_center(hue);
                    } else {
                        app.set_lens_defringe_green_center(hue);
                    }
                    // No word about it: the Center slider has moved,
                    // and it carries the number.
                    drop(st);
                    app.invoke_develop_changed();
                }
                "Range" => {
                    // The color range's hue from the picture before any
                    // look, as the mask reads it (`finish::sample`), at
                    // the panel's white; the dropper's box is the
                    // mean the mask reads a hue from, near enough.
                    let key = (app.get_as_shot(), app.get_temperature(), app.get_tint());
                    let m = preview_white(&mut st, key);
                    let px: [f32; 3] =
                        std::array::from_fn(|r| m[r][0] * px[0] + m[r][1] * px[1] + m[r][2] * px[2]);
                    let edit = read_edit(&app, &st.develop.edit, st.develop.target);
                    let s = finish::sample(px, None, edit.light.effective().exposure);
                    app.set_range_hue(s.hue());
                    // Under the floor the hue is the pixel's noise, and
                    // the window would not take it in anyway: say so.
                    let floor = app.get_range_chroma();
                    app.set_status(if s.chroma() < floor {
                        format!(
                            "that color is nearly grey (chroma {:.1}, under the floor's {:.1}): lower Chroma to take it in",
                            s.chroma() * 100.0,
                            floor * 100.0
                        )
                        .into()
                    } else {
                        "".into()
                    });
                    drop(st);
                    let status = app.get_status();
                    app.invoke_stop_placing();
                    app.set_status(status);
                    app.invoke_view_changed();
                }
                "Mixer" => {
                    let picked = pick_stages(&mut st, &app, px);
                    let (band, _) = finish::nearest_band(picked.hue);
                    app.set_mixer_band(band as i32);
                    show_mixer_band(&app);
                    st.develop.picking = Some(Picking::Mixer {
                        band,
                        hue0: app.get_mixer_hue(),
                        sat0: app.get_mixer_saturation(),
                        at: (x, y),
                    });
                    app.window().request_redraw();
                }
                _ => {}
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_pick_dragged(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            match st.develop.picking {
                Some(Picking::Curve {
                    channel,
                    index,
                    y0,
                    at,
                }) => {
                    let mut points = curve_points(&app, channel);
                    if index >= points.len() {
                        return;
                    }
                    points[index][1] = (y0 + (at - y) / PICK_DRAG).clamp(0.0, 1.0);
                    set_curve_points(&app, channel, &points);
                    show_selected(&app);
                    app.set_curve_image(draw_curve(&app, st.develop.bins.as_deref()));
                }
                Some(Picking::Mixer {
                    band,
                    hue0,
                    sat0,
                    at,
                }) => {
                    app.set_mixer_band(band as i32);
                    app.set_mixer_hue((hue0 + (x - at.0) / PICK_DRAG * 60.0).clamp(-30.0, 30.0));
                    app.set_mixer_saturation(
                        (sat0 + (at.1 - y) / PICK_DRAG * 2.0).clamp(-1.0, 1.0),
                    );
                    store_mixer_band(&app);
                }
                None => return,
            }
            app.window().request_redraw();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_pick_released(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(picked) = st.develop.picking.take() else {
                return;
            };
            st.develop.curve_drag = None;
            drop(st);
            app.invoke_view_changed();
            // The mixer's dropper is one pick, as the white's is: the
            // press chose the band, the drag moved it, and the release
            // puts the dropper down. The curve's stays in hand, a
            // point a click, until Esc or its button.
            if matches!(picked, Picking::Mixer { .. }) {
                app.invoke_stop_placing();
            }
        });
    }
    // Zoom about the cursor.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_zoom(move |factor, x, y| {
            // No change at all (Ctrl and a sideways scroll, a gesture
            // that only rotates) keeps a fit a fit rather than pinning
            // it to the zoom the fit happens to be.
            if factor == 1.0 {
                return;
            }
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let scale = app.window().scale_factor();
            let (vw, vh) = (
                app.get_view_width().max(1) as u32,
                app.get_view_height().max(1) as u32,
            );
            let old = effective_zoom(&st, vw, vh);
            let new = (old * factor).clamp(0.02, 16.0);
            // The image pixel under the cursor stays under the cursor.
            let (px, py) = (x * scale - vw as f32 / 2.0, y * scale - vh as f32 / 2.0);
            st.develop.center.0 += px / old - px / new;
            st.develop.center.1 += py / old - py / new;
            st.develop.zoom = new;
            app.window().request_redraw();
        });
    }
    // Zoom to a level with the image pixel at a point held; a click
    // goes to 1:1 there or back to fit; space and z do the same about
    // the view's center, at 1:1 and 3:1.
    {
        let outer = state.clone();
        let (state, app_weak) = (outer.clone(), app.as_weak());
        let zoom_to = move |level: f32, x: f32, y: f32| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if level <= 0.0 {
                st.develop.zoom = 0.0;
                let (iw, ih) = st.develop.image_size;
                st.develop.center = (iw as f32 / 2.0, ih as f32 / 2.0);
            } else {
                let scale = app.window().scale_factor();
                let (vw, vh) = (
                    app.get_view_width().max(1) as u32,
                    app.get_view_height().max(1) as u32,
                );
                let old = effective_zoom(&st, vw, vh);
                let (px, py) = (x * scale - vw as f32 / 2.0, y * scale - vh as f32 / 2.0);
                st.develop.center.0 += px / old - px / level;
                st.develop.center.1 += py / old - py / level;
                st.develop.zoom = level;
            }
            app.window().request_redraw();
        };
        app.on_zoom_to(zoom_to.clone());
        let state = outer.clone();
        let zoom_to2 = zoom_to.clone();
        let app_weak = app.as_weak();
        app.on_toggle_zoom(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // The touch area covers the letterbox too: a click beside
            // the picture is nobody's.
            let st = state.borrow();
            let scale = app.window().scale_factor();
            let (vw, vh) = (
                app.get_view_width().max(1) as u32,
                app.get_view_height().max(1) as u32,
            );
            // In the compare view a click chooses the frame under it.
            if let Some(cull) = st.culling.cull.as_ref()
                && cull.compare > 1
            {
                // As many tiles as the frame drew: a set of four at
                // the end of a short folder is fewer.
                let rows = cull.rows(&st.views.shown);
                let tile = cull::tile_at(vw, vh, rows.len(), x * scale, y * scale);
                let row = tile.and_then(|k| rows.get(k).copied());
                drop(st);
                if let Some(row) = row
                    && row as i32 != app.get_selected()
                {
                    app.invoke_select(row as i32);
                }
                return;
            }
            let (iw, ih) = st.develop.image_size;
            if iw == 0 || ih == 0 {
                return;
            }
            let zoom = effective_zoom(&st, vw, vh);
            let u = (x * scale - vw as f32 / 2.0) / zoom + st.develop.center.0;
            let v = (y * scale - vh as f32 / 2.0) / zoom + st.develop.center.1;
            if u < 0.0 || v < 0.0 || u > iw as f32 || v > ih as f32 {
                return;
            }
            let fitted = st.develop.zoom == 0.0;
            drop(st);
            zoom_to2(if fitted { 1.0 } else { 0.0 }, x, y);
        });
        let (state, app_weak) = (outer.clone(), app.as_weak());
        app.on_zoom_key(move |level| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let at_level = (state.borrow().develop.zoom - level).abs() < 1e-3;
            let scale = app.window().scale_factor();
            let (cx, cy) = (
                app.get_view_width().max(1) as f32 / 2.0 / scale,
                app.get_view_height().max(1) as f32 / 2.0 / scale,
            );
            zoom_to(if at_level { 0.0 } else { level }, cx, cy);
        });
    }
    // The navigator: a press or drag puts that point of the frame at
    // the view's center, kept within the frame while it is larger
    // than the view.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_navigate(move |fx, fy| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let (vw, vh) = (
                app.get_view_width().max(1) as u32,
                app.get_view_height().max(1) as u32,
            );
            let zoom = effective_zoom(&st, vw, vh);
            let (fw, fh) = (
                st.develop.image_size.0 as f32,
                st.develop.image_size.1 as f32,
            );
            let (shown_w, shown_h) = (vw as f32 / zoom, vh as f32 / zoom);
            let place = |f: f32, frame: f32, shown: f32| {
                if shown >= frame {
                    frame / 2.0
                } else {
                    (f * frame).clamp(shown / 2.0, frame - shown / 2.0)
                }
            };
            st.develop.center = (place(fx, fw, shown_w), place(fy, fh, shown_h));
            app.window().request_redraw();
        });
    }
    // Drag to pan.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_pan(move |dx, dy| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let scale = app.window().scale_factor();
            let (vw, vh) = (
                app.get_view_width().max(1) as u32,
                app.get_view_height().max(1) as u32,
            );
            let zoom = effective_zoom(&st, vw, vh);
            st.develop.center.0 -= dx * scale / zoom;
            st.develop.center.1 -= dy * scale / zoom;
            app.window().request_redraw();
        });
    }
    // A touchpad's scroll pans as a drag does, but held to the frame:
    // a stream of small deltas would otherwise carry the picture off
    // the view, and where it fits there is nothing to move.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_wheel_pan(move |dx, dy| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let scale = app.window().scale_factor();
            let (vw, vh) = (
                app.get_view_width().max(1) as u32,
                app.get_view_height().max(1) as u32,
            );
            let at = effective_zoom(&st, vw, vh);
            let (cw, ch) = view_cell(&st, vw, vh);
            let (fw, fh) = (
                st.develop.image_size.0 as f32,
                st.develop.image_size.1 as f32,
            );
            let center = (
                zoom::wheel_pan(st.develop.center.0, dx * scale / at, cw as f32 / at, fw),
                zoom::wheel_pan(st.develop.center.1, dy * scale / at, ch as f32 / at, fh),
            );
            if center != st.develop.center {
                st.develop.center = center;
                app.window().request_redraw();
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_reset_view(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.develop.zoom = 0.0;
            let (iw, ih) = st.develop.image_size;
            st.develop.center = (iw as f32 / 2.0, ih as f32 / 2.0);
            app.window().request_redraw();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    use crate::testing::{click, window};

    fn press_key(app: &App, text: &str) {
        use slint::platform::WindowEvent;
        app.window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.into() });
        app.window()
            .dispatch_event(WindowEvent::KeyReleased { text: text.into() });
    }

    /// The pointer on the picture, so the keys reach the editor.
    fn over_picture(app: &App) {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(600.0, 400.0),
            });
    }

    #[test]
    fn the_proof_badge_is_hidden_with_the_proof_off() {
        let app = window(1);
        assert_eq!(app.get_proof_badge(), "");
    }

    #[test]
    fn s_over_the_picture_shows_the_badge_with_the_profiles_name() {
        let app = window(1);
        over_picture(&app);
        app.set_proof_profile("Display P3".into());
        press_key(&app, "s");
        assert!(app.get_proofing());
        assert_eq!(app.get_proof_badge(), "Proof: Display P3");
        press_key(&app, "s");
        assert_eq!(app.get_proof_badge(), "");
    }

    #[test]
    fn a_profile_file_is_named_by_its_stem() {
        let app = window(1);
        assert_eq!(app.get_proof_badge(), "");
        set_proof_file(&app, "/somewhere/Fogra39 coated.icc");
        app.set_proof_profile("File".into());
        app.set_proofing(true);
        assert_eq!(app.get_proof_badge(), "Proof: Fogra39 coated");
    }

    #[test]
    fn the_badge_says_gamut_only_with_the_warning_on() {
        let app = window(1);
        app.set_proof_profile("sRGB".into());
        app.set_proofing(true);
        assert!(!app.get_gamut_warning(), "off until switched on");
        assert_eq!(app.get_proof_badge(), "Proof: sRGB");
        app.set_gamut_warning(true);
        assert_eq!(app.get_proof_badge(), "Proof: sRGB \u{b7} gamut");
    }

    #[test]
    fn a_click_on_the_badge_turns_the_proof_off() {
        let app = window(1);
        app.set_proof_profile("sRGB".into());
        app.set_proofing(true);
        let changed = Rc::new(Cell::new(0));
        {
            let changed = changed.clone();
            app.on_proof_changed(move || changed.set(changed.get() + 1));
        }
        let base =
            i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "Viewport")
                .next()
                .expect("the viewport is on screen")
                .absolute_position();
        click(&app, base.x + 20.0, base.y + 20.0);
        assert!(!app.get_proofing());
        assert_eq!(app.get_proof_badge(), "");
        assert_eq!(changed.get(), 1, "the engine is told");
    }

    #[test]
    fn the_badge_is_hidden_in_culling_with_the_proof_on() {
        let app = window(1);
        app.set_proofing(true);
        assert_ne!(app.get_proof_badge(), "");
        app.set_culling(true);
        assert_eq!(app.get_proof_badge(), "");
    }

    /// A pick with a single-shot dropper (White, Range, Defringe) does
    /// not zoom: the press puts the dropper down and, for these three,
    /// lets go of `picking` right there — which used to leave the
    /// release reading as a plain click, since it read `picking` fresh
    /// rather than remembering the press. Stands in for
    /// `viewport::install`'s own wiring of `pick_pressed`'s "Range"
    /// arm (`app.invoke_stop_placing()` before the release), which
    /// needs a developed picture on a real device to drive for real;
    /// this drives the same pointer-event routing the `.slint` side
    /// does, through the compiled window, with no GPU behind it.
    #[test]
    fn a_pick_with_a_single_shot_dropper_does_not_zoom() {
        let app = window(1);
        app.set_picking("Range".into());
        let zoomed = Rc::new(Cell::new(false));
        {
            let zoomed = zoomed.clone();
            app.on_toggle_zoom(move |_, _| zoomed.set(true));
        }
        {
            let app_weak = app.as_weak();
            app.on_pick_pressed(move |_, _| {
                let app = app_weak.upgrade().unwrap();
                // As the "Range" dropper's own press does: read the
                // color under the pointer (faked, here, as moving the
                // hue) and let go of `picking` on the press itself.
                app.set_range_hue(200.0);
                app.set_picking("".into());
            });
        }
        click(&app, 600.0, 400.0);
        assert!(!zoomed.get(), "a pick must not zoom the view");
        assert_eq!(app.get_range_hue(), 200.0, "the hue moved");
    }

    /// The mixer's dropper is one pick: the release that ends the
    /// press (and any drag after it) puts the dropper down, so the
    /// cursor is the hand again; its button and Esc put it down
    /// before a pick. Drives the installed callbacks; the press itself
    /// needs a developed picture to sample, so the test stands in for
    /// it with what the press leaves behind.
    #[test]
    fn a_pick_puts_the_mixer_dropper_down() {
        let app = window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);

        app.invoke_pick_started("Mixer".into());
        assert_eq!(app.get_picking(), "Mixer", "the button puts it in hand");
        state.borrow_mut().develop.picking = Some(Picking::Mixer {
            band: 2,
            hue0: 0.0,
            sat0: 0.0,
            at: (10.0, 10.0),
        });
        app.invoke_pick_dragged(30.0, 0.0);
        assert!(app.get_mixer_hue() > 0.0, "the drag moved the band");
        app.invoke_pick_released();
        assert_eq!(app.get_picking(), "", "the pick put the dropper down");
        assert!(state.borrow().develop.picking.is_none());

        // Its button again, before a pick, takes it back.
        app.invoke_pick_started("Mixer".into());
        app.invoke_pick_started("Mixer".into());
        assert_eq!(app.get_picking(), "");

        // And Esc.
        app.invoke_pick_started("Mixer".into());
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert_eq!(app.get_picking(), "");

        // The Curve's dropper is a point a click and stays in hand.
        app.invoke_pick_started("Curve".into());
        state.borrow_mut().develop.picking = Some(Picking::Curve {
            channel: Channel::Rgb,
            index: 0,
            y0: 0.5,
            at: 10.0,
        });
        app.invoke_pick_released();
        assert_eq!(app.get_picking(), "Curve", "a curve pick leaves it in hand");
    }

    /// The pointer over the picture reaches the state, from the
    /// `.slint` side's own report of it, and leaving takes the color
    /// under it off the scope. The color itself needs an analysis
    /// read back from a real device (`render::tests`).
    #[test]
    fn the_readout_follows_the_pointer_and_goes_with_it() {
        let app = window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(600.0, 400.0),
            });
        slint::platform::update_timers_and_animations();
        // In the viewport's own pixels, as the droppers' presses are:
        // the left pane's width in from the window's.
        let hover = state.borrow().develop.hover;
        assert!(hover.is_some_and(|(x, _)| x < 600.0), "{hover:?}");
        // Nothing to read yet, so nothing shown.
        assert_eq!(app.get_scope_readout(), "");
        // A diagonal move within the view, its one report of both.
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(640.0, 430.0),
            });
        slint::platform::update_timers_and_animations();
        let (x0, y0) = hover.unwrap();
        let moved = state.borrow().develop.hover;
        assert_eq!(moved, Some((x0 + 40.0, y0 + 30.0)));

        app.set_scope_readout("R 1  G 2  B 3  L 2".into());
        app.set_scope_marks(ModelRc::new(VecModel::from(vec![ScopeMark {
            x: 0.5,
            y: 0.5,
            channel: 0,
        }])));
        app.invoke_view_hovered(false, 600.0, 400.0);
        assert_eq!(state.borrow().develop.hover, None);
        assert_eq!(app.get_scope_readout(), "");
        assert_eq!(app.get_scope_marks().row_count(), 0);
    }

    /// A window at `scale` with frame 0 open and its own develop of
    /// 6000 by 4000 on the GPU, at the fit: the state `view_to_frame`
    /// and the readout measure by. Nothing is drawn.
    fn hover_frame(scale: f32) -> (App, Rc<RefCell<State>>, Rc<crate::worker::Worker>) {
        let app = window(1);
        if scale != 1.0 {
            app.window()
                .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
        }
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        app.set_placeholder(false);
        {
            let mut st = state.borrow_mut();
            st.develop.held = None;
            st.develop.peek = None;
            st.develop.shown_size = (6000, 4000);
            st.develop.source_size = (6000, 4000);
            st.develop.shown_turn = Some((0, current_turn(&st)));
            st.develop.image_size = (6000, 4000);
            st.develop.center = (3000.0, 2000.0);
            st.develop.zoom = 0.0;
        }
        (app, state, worker)
    }

    /// The view's middle, logical pixels.
    fn view_middle(app: &App) -> (f32, f32) {
        let scale = app.window().scale_factor();
        (
            app.get_view_width() as f32 / scale / 2.0,
            app.get_view_height() as f32 / scale / 2.0,
        )
    }

    /// The view onto the frame as the scopes' analysis picture covers
    /// it, pure geometry: the middle at the fit is the frame's middle,
    /// the grey around the picture is on none of it, and at 1:1 an
    /// off-center view, a crop, crop mode's whole plane and a quarter
    /// turn (of the edit, or one the develop has yet to catch up with)
    /// each give the fraction their arithmetic says, at a scale factor
    /// of one and of two.
    #[test]
    fn the_view_maps_onto_the_frame_the_scopes_cover() {
        for scale in [1.0f32, 2.0] {
            let (app, state, _worker) = hover_frame(scale);
            assert_eq!(app.window().scale_factor(), scale);
            let (mx, my) = view_middle(&app);
            let near = |got: Option<(f32, f32)>, want: (f32, f32), what: &str| {
                let got = got.unwrap_or_else(|| panic!("{what} at {scale}: off the picture"));
                assert!(
                    (got.0 - want.0).abs() < 1e-4 && (got.1 - want.1).abs() < 1e-4,
                    "{what} at {scale}: {got:?}, not {want:?}"
                );
            };
            let at = |x: f32, y: f32| view_to_frame(&state.borrow(), &app, x, y).map(|(at, _)| at);
            near(at(mx, my), (0.5, 0.5), "the fit's middle");

            // Small in the view: grey all about it.
            state.borrow_mut().develop.zoom = 0.05;
            assert_eq!(at(2.0, 2.0), None, "the grey at {scale}");
            near(at(mx, my), (0.5, 0.5), "the small picture's middle");

            // At 1:1 off the frame's middle: a logical pixel is `scale`
            // of the frame's.
            {
                let mut st = state.borrow_mut();
                st.develop.zoom = 1.0;
                st.develop.center = (1000.0, 500.0);
            }
            let (x, y) = (mx + 30.0, my - 20.0);
            let (dx, dy) = (30.0 * scale, -20.0 * scale);
            near(
                at(x, y),
                ((1000.0 + dx) / 6000.0, (500.0 + dy) / 4000.0),
                "1:1 off the middle",
            );

            // A crop of the middle quarter: its frame is the crop, and
            // in crop mode the whole plane about it.
            app.set_geo_has_crop(true);
            app.set_geo_crop_x(0.25);
            app.set_geo_crop_y(0.25);
            app.set_geo_crop_w(0.5);
            app.set_geo_crop_h(0.5);
            near(
                at(x, y),
                ((1000.0 + dx) / 3000.0, (500.0 + dy) / 2000.0),
                "the crop",
            );
            app.set_crop_mode(true);
            near(
                at(x, y),
                ((1000.0 + dx) / 6000.0, (500.0 + dy) / 4000.0),
                "crop mode's bounds",
            );
            app.set_crop_mode(false);
            app.set_geo_has_crop(false);

            // A quarter turn stands the frame on end, the edit's own
            // and one the develop on the GPU lags by alike.
            let turned = ((1000.0 + dx) / 4000.0, (500.0 + dy) / 6000.0);
            app.set_geo_turns(1);
            near(at(x, y), turned, "the edit's quarter turn");
            app.set_geo_turns(0);
            {
                let mut st = state.borrow_mut();
                let turn = current_turn(&st);
                st.develop.shown_turn = Some((0, (turn + 3) % 4));
            }
            near(at(x, y), turned, "a turn the develop lags by");

            // Not the open frame's own develop: another frame's, or one
            // held while the next develops, which the scopes measure by
            // another edit than the panel's.
            state.borrow_mut().develop.shown_turn = Some((7, 0));
            assert_eq!(at(x, y), None, "another frame's develop");
            {
                let mut st = state.borrow_mut();
                st.develop.shown_turn = Some((0, current_turn(&st)));
                st.develop.held = Some(Edit::default());
            }
            assert_eq!(at(x, y), None, "a held develop");
        }
    }

    /// The readout and the marks are those of the analysis picture
    /// under the pointer; a reading the same as the last leaves the
    /// scope be; a zoom under the resting pointer reads the new spot;
    /// and the camera's picture standing in, or a held develop, takes
    /// them off.
    #[test]
    fn the_readout_reads_the_spot_under_the_pointer() {
        let (app, state, _worker) = hover_frame(1.0);
        // The analysis picture: red on its left half, blue on its
        // right, all selected.
        let picture: Vec<slint::Rgba8Pixel> = (0..16 * 8)
            .map(|i| {
                let (r, b) = if i % 16 < 8 { (200, 40) } else { (40, 200) };
                slint::Rgba8Pixel {
                    r,
                    g: 40,
                    b,
                    a: 0xff,
                }
            })
            .collect();
        HOVER_PICTURE.with(|p| *p.borrow_mut() = Some((picture, 16, 8)));
        let red = scope::readout([200.0, 40.0, 40.0].map(|c| c / 255.0));
        let blue = scope::readout([40.0, 40.0, 200.0].map(|c| c / 255.0));
        let (mx, my) = view_middle(&app);
        let show = |x: f32, y: f32| {
            let mut st = state.borrow_mut();
            st.develop.hover = Some((x, y));
            show_hover(&mut st, &app);
        };
        // A quarter of the way across the fitted frame: red.
        let fit = effective_zoom(
            &state.borrow(),
            app.get_view_width() as u32,
            app.get_view_height() as u32,
        );
        let quarter = mx - 1500.0 * fit;
        show(quarter, my);
        assert_eq!(app.get_scope_readout(), red.as_str());
        assert_eq!(
            app.get_scope_marks().row_count(),
            4,
            "three channels and the luma"
        );
        assert!(!app.get_scope_marks_outside());

        // A move that reads the same pixel changes nothing shown.
        app.set_scope_readout("left be".into());
        show(quarter + 0.5, my + 0.5);
        assert_eq!(app.get_scope_readout(), "left be");
        state.borrow_mut().develop.hover_shown = None;
        show(quarter, my);
        assert_eq!(app.get_scope_readout(), red.as_str());

        // Zoomed in on the right half with the pointer resting: the
        // spot under it is blue now.
        {
            let mut st = state.borrow_mut();
            st.develop.zoom = 1.0;
            st.develop.center = (4500.0, 2000.0);
        }
        show_hover(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_scope_readout(), blue.as_str());

        // The camera's picture standing in: none of it describes that.
        app.set_placeholder(true);
        show_hover(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_scope_readout(), "");
        assert_eq!(app.get_scope_marks().row_count(), 0);
        app.set_placeholder(false);
        show_hover(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_scope_readout(), blue.as_str());

        // This frame developed again under the resting pointer: the
        // upload drops the analysis, and what is shown stays until
        // the new one is back, rather than blink.
        let picture = HOVER_PICTURE.with(|p| p.borrow_mut().replace((Vec::new(), 0, 0)));
        show_hover(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_scope_readout(), blue.as_str(), "a re-develop");
        assert_eq!(app.get_scope_marks().row_count(), 4);
        // Not on another scope, whose marks those are not.
        state.borrow_mut().develop.scope = scope::Scope::Vector;
        show_hover(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_scope_readout(), "", "another scope");
        state.borrow_mut().develop.scope = scope::Scope::Rgb;
        HOVER_PICTURE.with(|p| *p.borrow_mut() = picture.clone());
        show_hover(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_scope_readout(), blue.as_str());

        // A develop held under the last frame's edit: none of it.
        state.borrow_mut().develop.held = Some(Edit::default());
        show_hover(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_scope_readout(), "");
        assert_eq!(app.get_scope_marks().row_count(), 0);
        state.borrow_mut().develop.held = None;
        show_hover(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_scope_readout(), blue.as_str());

        // A step to another frame, its develop not landed: the one on
        // the GPU is the last frame's, and the readout goes.
        HOVER_PICTURE.with(|p| *p.borrow_mut() = Some((Vec::new(), 0, 0)));
        state.borrow_mut().develop.shown_turn = Some((7, 0));
        show_hover(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_scope_readout(), "", "a frame step");
        assert_eq!(app.get_scope_marks().row_count(), 0);
        HOVER_PICTURE.with(|p| p.borrow_mut().take());
    }

    /// The pointer leaving while the state is borrowed still takes the
    /// color under it off the scope, a frame's time later.
    #[test]
    fn a_leave_while_the_state_is_borrowed_still_clears_the_readout() {
        let app = window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        state.borrow_mut().develop.hover = Some((10.0, 10.0));
        app.set_scope_readout("R 1  G 2  B 3  L 2".into());
        {
            let _held = state.borrow_mut();
            app.invoke_view_hovered(false, 10.0, 10.0);
        }
        assert_eq!(app.get_scope_readout(), "R 1  G 2  B 3  L 2", "not yet");
        // Not tried again at once, so a borrow held across a nested
        // event loop does not spin it.
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
        assert_eq!(app.get_scope_readout(), "R 1  G 2  B 3  L 2", "not at once");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
        assert_eq!(state.borrow().develop.hover, None);
        assert_eq!(app.get_scope_readout(), "");
    }

    #[test]
    fn a_sheet_or_a_tool_asked_for_is_on_screen() {
        let app = crate::testing::window(1);
        let (_state, _worker) = crate::testing::retouch_state(&app);
        assert_eq!(Shown::sheet("export"), Ok(Shown::Export));
        assert_eq!(Shown::tool("guide"), Ok(Shown::Guide));
        assert_eq!(Shown::sheet("sync"), Ok(Shown::Sync));
        assert_eq!(Shown::sheet("synced"), Ok(Shown::Synced));
        assert_eq!(Shown::sheet("preset-onto-set"), Ok(Shown::PresetOntoSet));
        assert_eq!(Shown::sheet("preset-remove"), Ok(Shown::PresetRemove));
        assert_eq!(Shown::sheet("paste"), Ok(Shown::Paste));
        assert_eq!(Shown::sheet("pasted"), Ok(Shown::Pasted));
        assert!(Shown::sheet("guide").is_err());
        assert!(Shown::tool("export").is_err());

        Shown::Export.open(&app);
        assert!(app.get_export_open());
        Shown::Fetch.open(&app);
        assert!(app.get_fetch_open());
        assert!(!app.get_fetch_title().is_empty());
        app.set_fetch_open(false);
        assert_eq!(Shown::sheet("lenses"), Ok(Shown::Lenses));
        assert_eq!(Shown::sheet("settings"), Ok(Shown::Settings));
        Shown::Lenses.open(&app);
        assert!(app.get_fetch_open());
        assert!(
            app.get_fetch_text()
                .starts_with(crate::panel::assets::LENSES_WHY)
        );

        app.set_fetch_open(false);
        Shown::Settings.open(&app);
        assert!(app.get_settings_open());
        // No presets, nothing asked; else the longest name.
        Shown::PresetRemove.open(&app);
        assert_eq!(app.get_preset_confirm_remove(), -1);
        app.set_preset_names(slint::ModelRc::new(slint::VecModel::from(vec![
            slint::SharedString::from("Muted Slide"),
            "Red-Filter Mono".into(),
            "Warm Negative".into(),
        ])));
        Shown::PresetRemove.open(&app);
        assert_eq!(app.get_preset_confirm_remove(), 1);
        app.set_preset_confirm_remove(-1);
        assert_eq!(Shown::sheet("preset-report"), Ok(Shown::PresetReport));
        assert_eq!(
            Shown::sheet("lightroom-written"),
            Ok(Shown::Lightroom(LightroomStep::Written))
        );
        assert_eq!(
            Shown::sheet("rawtherapee"),
            Ok(Shown::RawTherapee(RawTherapeeStep::Report))
        );
        assert_eq!(
            Shown::sheet("rawtherapee-offer"),
            Ok(Shown::RawTherapee(RawTherapeeStep::Offer))
        );
        assert_eq!(
            Shown::sheet("rawtherapee-written"),
            Ok(Shown::RawTherapee(RawTherapeeStep::Written))
        );
        // With no --lightroom catalog it opens nothing.
        Shown::Lightroom(LightroomStep::Roots).open(&app);
        assert!(!app.get_lr_open());
        Shown::PresetReport.open(&app);
        assert!(app.get_preset_report_open());
        assert_eq!(app.get_preset_report_title(), "Imported 2 of 3 presets");
        app.set_preset_report_open(false);

        // A tool brings the Crop tab with it.
        assert_eq!(app.get_panel_tab(), "Develop");
        Shown::Crop.open(&app);
        assert_eq!(app.get_panel_tab(), "Crop");
        assert!(app.get_crop_mode());
        Shown::Level.open(&app);
        assert!(app.get_level_mode());
        Shown::Guide.open(&app);
        assert_eq!(app.get_guide_mode(), "Vertical");
    }

    #[test]
    fn tab_puts_the_panels_away_and_brings_them_back() {
        let app = crate::testing::window(3);
        let (_state, _worker) = crate::testing::retouch_state(&app);
        let tab = slint::platform::Key::Tab;
        let hidden = |app: &App| {
            (
                app.get_left_hidden(),
                app.get_right_hidden(),
                app.get_strip_hidden(),
            )
        };
        assert_eq!(hidden(&app), (false, false, false));
        crate::testing::press(&app, tab);
        assert_eq!(hidden(&app), (true, true, true), "Tab hides them");
        crate::testing::press(&app, tab);
        assert_eq!(
            hidden(&app),
            (false, false, false),
            "and Tab again shows them"
        );

        // One put away on its own: Tab puts the rest away with it,
        // and brings all three back.
        crate::testing::press(&app, slint::platform::Key::F7);
        assert_eq!(hidden(&app), (true, false, false), "F7 the left pane");
        crate::testing::press(&app, tab);
        assert_eq!(hidden(&app), (true, true, true));
        crate::testing::press(&app, tab);
        assert_eq!(hidden(&app), (false, false, false));

        // A sheet has the keys: Tab under it is not this one.
        app.set_export_open(true);
        crate::testing::press(&app, tab);
        crate::testing::press(&app, slint::platform::Key::F8);
        assert_eq!(hidden(&app), (false, false, false), "not under a sheet");
        app.set_export_open(false);

        // Nor over the grid, which has the window already.
        app.set_grid_open(true);
        crate::testing::press(&app, tab);
        crate::testing::press(&app, slint::platform::Key::F6);
        assert_eq!(hidden(&app), (false, false, false), "not over the grid");
    }

    #[test]
    fn each_panel_puts_away_on_its_own() {
        let app = crate::testing::window(3);
        let (_state, _worker) = crate::testing::retouch_state(&app);
        use slint::platform::Key;
        crate::testing::press(&app, Key::F8);
        assert!(app.get_right_hidden(), "F8 the develop panel");
        assert!(!app.get_left_hidden() && !app.get_strip_hidden());
        crate::testing::press(&app, Key::F6);
        assert!(app.get_strip_hidden(), "F6 the strip");
        assert!(!app.get_left_hidden());
        crate::testing::press(&app, Key::F8);
        crate::testing::press(&app, Key::F6);
        assert!(
            !app.get_right_hidden() && !app.get_strip_hidden(),
            "and back"
        );

        // What the settings keep is what the window shows.
        crate::testing::press(&app, Key::F7);
        let kept = crate::panel::startup::remember(&app);
        assert!(kept.hide_left && !kept.hide_right && !kept.hide_strip);
    }

    /// A raw of a camera that is Rec.2020 itself, calibrated under
    /// D65, its as-shot gains a daylight's: enough of a frame for the
    /// white balance to resolve against.
    fn daylight_frame() -> RawFrame {
        use greycard_core::raw::{Calibration, CfaPattern, LevelPattern, Levels, Samples};
        let m = greycard_core::color::WORKING_SPACE
            .from_xyz_matrix()
            .expect("Rec.2020 inverts");
        RawFrame {
            make: "Test".into(),
            model: "Cam".into(),
            width: 2,
            height: 2,
            channels: 1,
            layout: greycard_core::raw::SensorLayout::Cfa(CfaPattern::rggb()),
            samples: Samples::U16(vec![500; 4]),
            levels: Levels {
                black: LevelPattern::uniform(0.0, 1),
                white: LevelPattern::uniform(1000.0, 1),
            },
            as_shot_coefficients: Some([1.0, 1.0, 1.0]),
            calibrations: vec![Calibration {
                illuminant: 21,
                color_matrix: m.rows.iter().flatten().map(|&v| v as f32).collect(),
                forward_matrix: None,
            }],
            crop: None,
            orientation: Orientation::Normal,
            shot: Default::default(),
        }
    }

    /// The Temperature and Tint sliders are live under As shot, showing
    /// the camera's white; a drag of Temperature turns As shot off by
    /// itself, leaves the slider where the pointer let go, and is one
    /// step of the history; As shot switched back on puts the sliders
    /// and the picture back. Judged on the picture developed: a white
    /// patch, neutral as shot, warms with the move and is neutral again
    /// after.
    #[test]
    fn moving_temperature_turns_as_shot_off_and_as_shot_puts_it_back() {
        use greycard_core::color::{WhitePoint, profile_from_frame, resolve_white_balance};
        let app = window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        app.set_panel_tab("Develop".into());
        // A white patch, 32 by 32, under daylight.
        let frame = RawFrame {
            width: 32,
            height: 32,
            samples: greycard_core::raw::Samples::U16(vec![500; 32 * 32]),
            ..daylight_frame()
        };
        let profile = profile_from_frame(&frame).unwrap();
        let shot = resolve_white_balance(&frame, &profile, WhitePoint::AsShot).unwrap();
        let as_shot = (shot.temp_tint.cct as f32, shot.temp_tint.duv as f32);
        {
            let mut st = state.borrow_mut();
            st.develop.base_white = Some(WhiteBase::from(&shot, 1.0));
            st.develop.frame = Some((Arc::new(frame.clone()), Box::new(profile)));
            st.develop.shown_turn = Some((0, current_turn(&st)));
            // As the decode's landing leaves the panel.
            st.develop.as_shot_white = Some(as_shot);
        }
        app.set_raw_input(true);
        app.set_temperature(as_shot.0);
        app.set_tint(as_shot.1);
        let wait = || {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
                crate::panel::edit::SAVE_MS + 100,
            ))
        };
        wait();
        assert!(app.get_as_shot());
        let steps = state.borrow().sidecars[0].history.len();
        // The white patch's middle as the edit on the panel develops it.
        let patch = |app: &App| {
            let edit = read_edit(app, &state.borrow().develop.edit, None);
            let developed = greycard_core::develop::develop(&frame, &edit.settings())
                .expect("the patch develops");
            developed.image.pixel(16, 16)
        };
        let neutral = patch(&app);
        assert!(
            (neutral[0] / neutral[2] - 1.0).abs() < 0.02,
            "as shot, the white is white: {neutral:?}"
        );

        // A drag of the Temperature handle, a third of the track to
        // the right: live under As shot, and the move turns it off.
        let (at, size) = crate::testing::labeled(&app, "Temperature");
        let (left, right) = (at.x + 68.0, at.x + size.width - 60.0);
        let fraction = (as_shot.0 - 2000.0) / 10000.0;
        let x = left + fraction * (right - left);
        let y = at.y + size.height / 2.0;
        crate::testing::drag(&app, &[(x, y), (x + 10.0, y), (x + 60.0, y)]);
        wait();
        assert!(!app.get_as_shot(), "the move turned As shot off");
        let moved = app.get_temperature();
        assert!(moved > as_shot.0 + 2000.0, "{moved} from {}", as_shot.0);
        assert_eq!(app.get_tint(), as_shot.1, "the tint stays the camera's");
        assert_eq!(
            state.borrow().develop.edit.white_balance,
            WhiteBalance::Custom {
                temperature: moved as f64,
                tint: as_shot.1 as f64,
            }
        );
        assert_eq!(
            state.borrow().sidecars[0].history.len(),
            steps + 1,
            "the move is one step"
        );
        let warmed = patch(&app);
        assert!(
            warmed[0] / warmed[2] > neutral[0] / neutral[2] * 1.1,
            "a higher temperature renders warmer: {warmed:?} against {neutral:?}"
        );

        // As shot back on: the camera's white on the sliders and on
        // the picture.
        let (at, size) = crate::testing::labeled(&app, "As shot");
        crate::testing::click(&app, at.x + size.width - 12.0, at.y + size.height / 2.0);
        wait();
        assert!(app.get_as_shot());
        assert_eq!((app.get_temperature(), app.get_tint()), as_shot);
        assert_eq!(
            state.borrow().develop.edit.white_balance,
            WhiteBalance::AsShot
        );
        assert_eq!(patch(&app), neutral);
        assert_eq!(state.borrow().sidecars[0].history.len(), steps + 2);

        // A custom pair, from a drag of each slider.
        let drag_slider = |label: &str, value: f32, low: f32, high: f32, by: f32| {
            let (at, size) = crate::testing::labeled(&app, label);
            let (left, right) = (at.x + 68.0, at.x + size.width - 60.0);
            let x = left + (value - low) / (high - low) * (right - left);
            let y = at.y + size.height / 2.0;
            crate::testing::drag(&app, &[(x, y), (x + by / 6.0, y), (x + by, y)]);
            wait();
        };
        drag_slider("Temperature", as_shot.0, 2000.0, 12000.0, 60.0);
        drag_slider("Tint", as_shot.1, -0.05, 0.05, -40.0);
        let custom = (app.get_temperature(), app.get_tint());
        assert!(!app.get_as_shot());
        assert!(custom.0 > as_shot.0 && custom.1 < as_shot.1, "{custom:?}");
        let steps = state.borrow().sidecars[0].history.len();
        // A double-click on a slider puts it alone back to the camera's
        // value, and nothing else: As shot stays off. One step each.
        let double_click = |label: &str| {
            // Clear of the last click, so the two here are a pair.
            slint::platform::update_timers_and_animations();
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(600));
            slint::platform::update_timers_and_animations();
            let (at, size) = crate::testing::labeled(&app, label);
            let (x, y) = (at.x + 30.0, at.y + size.height / 2.0);
            crate::testing::click(&app, x, y);
            crate::testing::click(&app, x, y);
            wait();
        };
        double_click("Temperature");
        assert_eq!(
            (app.get_temperature(), app.get_tint()),
            (as_shot.0, custom.1)
        );
        assert!(!app.get_as_shot());
        assert_eq!(state.borrow().sidecars[0].history.len(), steps + 1);
        double_click("Tint");
        assert_eq!((app.get_temperature(), app.get_tint()), as_shot);
        assert!(!app.get_as_shot(), "both at the camera's, and still custom");
        assert_eq!(
            state.borrow().develop.edit.white_balance,
            WhiteBalance::Custom {
                temperature: as_shot.0 as f64,
                tint: as_shot.1 as f64,
            }
        );
        assert_eq!(state.borrow().sidecars[0].history.len(), steps + 2);

        // As shot switched on from a custom pair: both sliders, and the
        // picture, at the camera's.
        drag_slider("Temperature", as_shot.0, 2000.0, 12000.0, -60.0);
        drag_slider("Tint", as_shot.1, -0.05, 0.05, 40.0);
        assert!(!app.get_as_shot());
        assert_ne!(patch(&app), neutral);
        let (at, size) = crate::testing::labeled(&app, "As shot");
        crate::testing::click(&app, at.x + size.width - 12.0, at.y + size.height / 2.0);
        wait();
        assert!(app.get_as_shot());
        assert_eq!((app.get_temperature(), app.get_tint()), as_shot);
        assert_eq!(patch(&app), neutral);
    }

    /// The Auto button reads the whole picture and sets the white it
    /// finds the way the Neutral dropper does: As shot off, the
    /// temperature and tint moved, one step of the history. A picture
    /// that is all grey under a warm light, developed at daylight,
    /// wants a lower temperature than the daylight it was shot at.
    #[test]
    fn auto_white_balance_sets_the_white_in_one_step() {
        use greycard_core::color::{WhitePoint, profile_from_frame, resolve_white_balance};
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        let frame = daylight_frame();
        let profile = profile_from_frame(&frame).unwrap();
        let shot = resolve_white_balance(&frame, &profile, WhitePoint::AsShot).unwrap();
        {
            let mut st = state.borrow_mut();
            st.develop.base_white = Some(WhiteBase::from(&shot, 1.0));
            st.develop.frame = Some((Arc::new(frame), Box::new(profile)));
        }
        let wait = || {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
                crate::panel::edit::SAVE_MS + 100,
            ))
        };
        wait();
        app.set_as_shot(true);
        let (temperature, tint) = (app.get_temperature(), app.get_tint());
        let steps = state.borrow().sidecars[0].history.len();

        // The picture on the GPU is not this frame's develop yet (a
        // frame was chosen and its develop has not landed): neither
        // Auto nor the Neutral dropper reads it.
        AUTO_WHITE_GRID.with(|g| *g.borrow_mut() = Some((vec![[0.3; 3]; 1000], 40)));
        app.invoke_wb_auto();
        assert_eq!(app.get_status(), "auto white balance: wait for the develop");
        assert_eq!(
            white_from_neutral(state.borrow_mut(), &app, [0.3, 0.3, 0.2]),
            Err(NeutralMiss::Waiting)
        );
        assert_eq!(
            NeutralMiss::Waiting.said(false),
            "white balance: wait for the develop"
        );
        assert!(app.get_as_shot());
        AUTO_WHITE_GRID.with(|g| g.borrow_mut().take());
        {
            let mut st = state.borrow_mut();
            st.develop.shown_turn = Some((0, current_turn(&st)));
        }
        // A neutral that names no white says so, in each one's words.
        assert_eq!(
            white_from_neutral(state.borrow_mut(), &app, [0.0, 0.3, 0.3]),
            Err(NeutralMiss::NoWhite)
        );
        assert_eq!(NeutralMiss::NoWhite.said(false), "nothing to read there");
        assert_eq!(
            NeutralMiss::NoWhite.said(true),
            "auto white balance: nothing neutral to read"
        );
        assert!(app.get_as_shot());

        // Too little to read: nothing moves, and the status says why.
        AUTO_WHITE_GRID.with(|g| *g.borrow_mut() = Some((vec![[0.99; 3]; 1000], 40)));
        app.invoke_wb_auto();
        assert_eq!(app.get_status(), AUTO_WHITE_TOO_LITTLE);
        assert!(app.get_as_shot());

        let grid: Vec<[f32; 3]> = (0..2000)
            .map(|i| {
                let level = 0.05 + 0.5 * i as f32 / 2000.0;
                [1.25 * level, level, 0.7 * level]
            })
            .collect();
        AUTO_WHITE_GRID.with(|g| *g.borrow_mut() = Some((grid, 40)));
        app.invoke_wb_auto();
        wait();
        assert!(!app.get_as_shot());
        assert_ne!(app.get_tint(), tint);
        assert_ne!(app.get_temperature(), temperature);
        assert!(
            (app.get_temperature() as f64) < shot.temp_tint.cct - 500.0,
            "{} against {}",
            app.get_temperature(),
            shot.temp_tint.cct
        );
        assert_eq!(state.borrow().sidecars[0].history.len(), steps + 1);
    }

    /// A picture held up while the next frame develops keeps its own
    /// masks' white balances: the frame, its profile and the develop's
    /// white move on to the next file before its develop lands, and the
    /// held picture's masks must not flash back to the global's white.
    #[test]
    fn a_held_picture_keeps_its_masks_whites() {
        use greycard_core::color::{WhitePoint, profile_from_frame, resolve_white_balance};
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        let frame = daylight_frame();
        let profile = profile_from_frame(&frame).unwrap();
        let shot = resolve_white_balance(&frame, &profile, WhitePoint::AsShot).unwrap();
        let mut st = state.borrow_mut();
        st.develop.base_white = Some(WhiteBase::from(&shot, 1.0));
        st.develop.frame = Some((Arc::new(frame), Box::new(profile)));
        let own = crate::panel::color::local_white_shift(&mut st).expect("a raw's");
        // The next file chosen: the picture held, the frame gone and
        // the develop's white not yet the next one's.
        st.develop.held = Some(st.develop.edit.clone());
        st.develop.frame = None;
        st.develop.base_white = None;
        let held = crate::panel::color::local_white_shift(&mut st).expect("the held one's");
        assert_eq!((held.gains, held.rows), (own.gains, own.rows));
        // Its develop landed: the next frame's own, here none.
        st.develop.held = None;
        assert!(crate::panel::color::local_white_shift(&mut st).is_none());
        st.develop.held = Some(st.develop.edit.clone());
        assert!(crate::panel::color::local_white_shift(&mut st).is_none());
    }

    /// A mask's own white balance on the panel: switched on it starts
    /// at the global's white, As shot read as its kelvin and Duv, and
    /// the Neutral dropper on the Masks tab sets the mask's and leaves
    /// the global's alone.
    /// A daylight frame open with one mask, "Lamp", chosen on the Masks
    /// tab: the frame's As shot white as a temperature and tint.
    fn a_lamp_mask(app: &App, state: &Rc<RefCell<State>>) -> greycard_core::TempTint {
        use greycard_core::color::{WhitePoint, profile_from_frame, resolve_white_balance};
        app.invoke_select(0);
        let frame = daylight_frame();
        let profile = profile_from_frame(&frame).unwrap();
        let shot = resolve_white_balance(&frame, &profile, WhitePoint::AsShot).unwrap();
        {
            let mut st = state.borrow_mut();
            st.develop.base_white = Some(WhiteBase::from(&shot, 1.0));
            st.develop.frame = Some((Arc::new(frame), Box::new(profile)));
            st.develop.shown_turn = Some((0, current_turn(&st)));
            st.develop.edit.adjustments.push(greycard_edit::Adjustment {
                id: 1,
                name: "Lamp".into(),
                ..Default::default()
            });
            st.develop.target = Some(0);
        }
        app.set_panel_tab("Masks".into());
        app.set_target(1);
        app.set_as_shot(true);
        shot.temp_tint
    }

    #[test]
    fn a_masks_white_balance_starts_at_the_globals_and_takes_the_dropper() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        let shot = a_lamp_mask(&app, &state);
        app.set_local_temperature(9000.0);
        app.set_local_wb_on(true);
        app.invoke_local_wb_toggled();
        assert!(
            (app.get_local_temperature() as f64 - shot.cct).abs() < 1.0,
            "{} against {}",
            app.get_local_temperature(),
            shot.cct
        );
        assert!((app.get_local_tint() as f64 - shot.duv).abs() < 1e-4);
        // A custom global is taken as it stands, by a mask with no
        // light of its own: one reset.
        app.set_as_shot(false);
        app.set_temperature(4100.0);
        app.set_tint(0.003);
        app.invoke_local_wb_reset();
        app.set_local_wb_on(true);
        app.invoke_local_wb_toggled();
        assert_eq!(app.get_local_temperature(), 4100.0);
        assert_eq!(app.get_local_tint(), 0.003);
        let edit = read_edit(&app, &state.borrow().develop.edit, Some(0));
        assert_eq!(
            edit.adjustments[0].look.white_balance,
            greycard_edit::LocalWhite::Absolute {
                temperature: 4100.0,
                tint: 0.003f32 as f64
            }
        );
        // The dropper under a warm light, on the Masks tab: the mask's
        // white moves, the global's does not.
        app.set_local_wb_on(false);
        white_from_neutral(state.borrow_mut(), &app, [0.4, 0.3, 0.15]).unwrap();
        assert!(app.get_local_wb_on());
        assert!(
            app.get_local_temperature() < 4000.0,
            "{}",
            app.get_local_temperature()
        );
        assert_eq!(app.get_temperature(), 4100.0);
        assert_eq!(app.get_tint(), 0.003);
        // Off, the mask's light reaches nothing, and the global look
        // never carries one.
        app.set_local_wb_on(false);
        let edit = read_edit(&app, &state.borrow().develop.edit, Some(0));
        assert!(edit.adjustments[0].look.white_balance.temp_tint().is_none());
        app.set_local_wb_on(true);
        let edit = read_edit(&app, &state.borrow().develop.edit, None);
        assert!(edit.look().white_balance.is_off());
    }

    /// A mask's white balance switched off keeps its light, and on
    /// again brings it back rather than the global's, so the switch
    /// compares the two; each switch is a step. A double-click on its
    /// Temperature or Tint forgets the light: the switch goes off, the
    /// mask follows the global, and the next switch on starts there.
    #[test]
    fn a_masks_white_balance_keeps_its_light_while_switched_off() {
        use greycard_edit::LocalWhite;
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        let shot = a_lamp_mask(&app, &state);
        let settle = || {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
                crate::panel::edit::SAVE_MS + 100,
            ));
            slint::platform::update_timers_and_animations();
        };
        settle();
        let stored = || {
            state.borrow().sidecars[0].current.adjustments[0]
                .look
                .white_balance
        };
        let steps = || state.borrow().sidecars[0].history.len();
        let lamp = LocalWhite::Absolute {
            temperature: 3300.0,
            tint: -0.004f32 as f64,
        };
        app.set_local_wb_on(true);
        app.invoke_local_wb_toggled();
        app.set_local_temperature(3300.0);
        app.set_local_tint(-0.004);
        app.invoke_view_changed();
        settle();
        assert_eq!(stored(), lamp);
        let before = steps();

        // Off: the light is kept, and the picture is the global's.
        app.set_local_wb_on(false);
        app.invoke_local_wb_toggled();
        settle();
        let LocalWhite::Absolute { temperature, tint } = lamp else {
            unreachable!()
        };
        assert_eq!(stored(), LocalWhite::Kept { temperature, tint });
        assert!(stored().temp_tint().is_none());
        assert_eq!(steps(), before + 1, "switching off is a step");
        assert_eq!(app.get_local_temperature(), 3300.0);

        // On: the same light, not the global's.
        app.set_local_wb_on(true);
        app.invoke_local_wb_toggled();
        settle();
        assert_eq!(stored(), lamp);
        assert_eq!(steps(), before + 2, "switching on is a step");

        // Through the sidecar's file and a fresh panel: off, then shown
        // again, the panel holds the kept light and brings it back.
        app.set_local_wb_on(false);
        app.invoke_local_wb_toggled();
        settle();
        let json = state.borrow().sidecars[0].to_json();
        let read: greycard_edit::Sidecar = serde_json::from_str(&json).unwrap();
        assert_eq!(
            read.current.adjustments[0].look.white_balance,
            LocalWhite::Kept { temperature, tint }
        );
        app.set_local_temperature(9000.0);
        app.set_local_wb_kept(false);
        {
            let mut st = state.borrow_mut();
            st.develop.edit = read.current.clone();
            crate::panel::edit::show_edit(&st, &read.current, &app, Some(0));
        }
        assert!(!app.get_local_wb_on());
        assert!(app.get_local_wb_kept());
        assert_eq!(app.get_local_temperature(), 3300.0);
        app.set_local_wb_on(true);
        app.invoke_local_wb_toggled();
        assert_eq!(app.get_local_temperature(), 3300.0);
        assert_eq!(app.get_local_tint(), -0.004);
        settle();
        assert_eq!(stored(), lamp);

        // The reset gesture: the light forgotten, both at once, the
        // switch off, the sliders at the global's white.
        let before = steps();
        app.invoke_local_wb_reset();
        settle();
        assert!(!app.get_local_wb_on());
        assert!(!app.get_local_wb_kept());
        assert_eq!(stored(), LocalWhite::Off);
        assert_eq!(steps(), before + 1, "a reset is a step");
        assert!((app.get_local_temperature() as f64 - shot.cct).abs() < 1.0);
        // And on again, it starts at the global's, as a new mask does.
        app.set_as_shot(false);
        app.set_temperature(6100.0);
        app.set_tint(0.001);
        app.set_local_wb_on(true);
        app.invoke_local_wb_toggled();
        assert_eq!(app.get_local_temperature(), 6100.0);
        assert_eq!(app.get_local_tint(), 0.001);
    }

    /// A window with a 6000 by 4000 picture on the viewport, centered,
    /// at `zoom` (0 fits).
    fn scrolled_view(zoom: f32) -> (App, Rc<RefCell<State>>, Rc<crate::worker::Worker>) {
        let app = window(1);
        let (state, worker) = crate::testing::retouch_state(&app);
        {
            let mut st = state.borrow_mut();
            st.develop.image_size = (6000, 4000);
            st.develop.center = (3000.0, 2000.0);
            st.develop.zoom = zoom;
        }
        (app, state, worker)
    }

    /// A wheel event at the middle of the view.
    fn scroll(app: &App, delta_x: f32, delta_y: f32) {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(600.0, 400.0),
                delta_x,
                delta_y,
            });
    }

    /// A mouse wheel's click, a single vertical delta of 60 px, zooms a
    /// step about the pointer, from the fit as from a zoom.
    #[test]
    fn a_wheel_click_over_the_picture_zooms() {
        let (app, state, _worker) = scrolled_view(1.0);
        scroll(&app, 0.0, 60.0);
        assert_eq!(state.borrow().develop.zoom, 1.25);
        scroll(&app, 0.0, -60.0);
        assert_eq!(state.borrow().develop.zoom, 1.0);

        let (app, state, _worker) = scrolled_view(0.0);
        let (vw, vh) = (app.get_view_width() as u32, app.get_view_height() as u32);
        let fit = effective_zoom(&state.borrow(), vw, vh);
        scroll(&app, 0.0, 60.0);
        assert!((state.borrow().develop.zoom - fit * 1.25).abs() < 1e-6);
    }

    /// With the brush or the retouch patch in hand the wheel sizes it
    /// by 1.1x for each 60 px of delta, however it arrives: a click is
    /// one step, a touchpad's small deltas add up to one, and a turn of
    /// direction starts the count over.
    #[test]
    fn the_wheel_sizes_the_brush_and_the_patch_by_the_accumulated_delta() {
        type Get = fn(&App) -> f32;
        type Put = fn(&App);
        let brush: (Get, Put) = (App::get_brush_size, |a| a.set_placing("Brush".into()));
        let patch: (Get, Put) = (App::get_patch_size, |a| a.set_retouch_mode("Heal".into()));
        for (size, hold) in [brush, patch] {
            let (app, _state, _worker) = scrolled_view(1.0);
            app.set_brush_size(0.05);
            app.set_patch_size(0.05);
            hold(&app);
            let base = size(&app);
            let near = |a: f32, b: f32| (a - b).abs() < 1e-6;

            scroll(&app, 0.0, 60.0);
            assert!(near(size(&app), base * 1.1), "a click is a step");
            scroll(&app, 0.0, -60.0);
            assert!(near(size(&app), base), "and back");

            for _ in 0..6 {
                scroll(&app, 0.0, 10.0);
            }
            assert!(
                near(size(&app), base * 1.1),
                "six small deltas are one step"
            );
            for _ in 0..6 {
                scroll(&app, 0.0, -10.0);
            }
            assert!(near(size(&app), base), "and six back");

            for _ in 0..3 {
                scroll(&app, 0.0, 10.0);
            }
            for _ in 0..3 {
                scroll(&app, 0.0, -10.0);
            }
            assert!(near(size(&app), base), "a reversal does not step");
        }
    }

    /// A touchpad's small deltas pan the picture with the fingers and
    /// leave the zoom alone, adding up as they come; deltas on both
    /// axes, or a tilted wheel's sideways click, pan too.
    #[test]
    fn a_touchpad_scroll_over_the_picture_pans() {
        let (app, state, _worker) = scrolled_view(1.0);
        let scale = app.window().scale_factor();
        scroll(&app, 0.0, 10.0);
        {
            let st = state.borrow();
            assert_eq!(st.develop.zoom, 1.0, "a small delta does not zoom");
            assert_eq!(st.develop.center, (3000.0, 2000.0 - 10.0 * scale));
        }
        for _ in 0..5 {
            scroll(&app, 3.0, -4.0);
        }
        {
            let st = state.borrow();
            assert_eq!(st.develop.zoom, 1.0);
            assert_eq!(
                st.develop.center,
                (3000.0 - 15.0 * scale, 2000.0 + 10.0 * scale)
            );
        }
        scroll(&app, 60.0, 60.0);
        assert_eq!(state.borrow().develop.zoom, 1.0);
        scroll(&app, 60.0, 0.0);
        assert_eq!(state.borrow().develop.zoom, 1.0);
        assert_eq!(state.borrow().develop.center.0, 3000.0 - 135.0 * scale);
    }

    /// A touchpad's fast flick carries deltas of a click's size and
    /// more: one that is no whole number of clicks pans, and a whole
    /// click within a quarter second of a pan is part of the pan. The
    /// same click after a quiet spell is a wheel's, and zooms.
    #[test]
    fn a_flick_that_reaches_a_clicks_size_still_pans() {
        let (app, state, _worker) = scrolled_view(1.0);
        let scale = app.window().scale_factor();
        scroll(&app, 0.0, 72.0);
        {
            let st = state.borrow();
            assert_eq!(st.develop.zoom, 1.0, "72 px is no whole click");
            assert_eq!(st.develop.center.1, 2000.0 - 72.0 * scale);
        }
        // Well after that: a flick's ramp, which lands on a whole click
        // on its way up.
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1000));
        for dy in [8.0, 24.0, 48.0, 60.0] {
            scroll(&app, 0.0, dy);
        }
        {
            let st = state.borrow();
            assert_eq!(st.develop.zoom, 1.0, "a click's size mid-pan pans");
            assert_eq!(st.develop.center.1, 2000.0 - 212.0 * scale);
        }
        // Still within the quarter second of the last pan.
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(200));
        scroll(&app, 0.0, -120.0);
        assert_eq!(state.borrow().develop.zoom, 1.0);
        // A quiet spell, and the same click zooms.
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(300));
        scroll(&app, 0.0, 60.0);
        assert_eq!(state.borrow().develop.zoom, 1.25);
    }

    /// A high-resolution wheel's fragments pan a little, and its whole
    /// detent, close behind them, still zooms: fragments that small
    /// are no flick.
    #[test]
    fn a_high_resolution_wheels_detent_still_zooms() {
        let (app, state, _worker) = scrolled_view(1.0);
        let scale = app.window().scale_factor();
        for _ in 0..3 {
            scroll(&app, 0.0, 15.0);
        }
        assert_eq!(state.borrow().develop.zoom, 1.0);
        assert_eq!(state.borrow().develop.center.1, 2000.0 - 45.0 * scale);
        scroll(&app, 0.0, 60.0);
        assert_eq!(state.borrow().develop.zoom, 1.25);
    }

    /// A zoom by nothing (Ctrl and a purely sideways scroll; a gesture
    /// that only rotates) leaves a fitted picture fitted.
    #[test]
    fn a_zoom_by_nothing_keeps_the_fit() {
        use slint::platform::{Key, WindowEvent};
        let (app, state, _worker) = scrolled_view(0.0);
        app.invoke_zoom(1.0, 600.0, 400.0);
        assert_eq!(state.borrow().develop.zoom, 0.0);
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        scroll(&app, 60.0, 0.0);
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
        let st = state.borrow();
        assert_eq!(st.develop.zoom, 0.0, "still the fit");
        assert_eq!(st.develop.center, (3000.0, 2000.0));
    }

    /// Where the whole picture is in view a pan has nowhere to go: the
    /// picture stays put, and nothing zooms instead.
    #[test]
    fn a_touchpad_scroll_on_a_fitted_picture_does_nothing() {
        let (app, state, _worker) = scrolled_view(0.0);
        for _ in 0..10 {
            scroll(&app, 7.0, -12.0);
        }
        let st = state.borrow();
        assert_eq!(st.develop.zoom, 0.0);
        assert_eq!(st.develop.center, (3000.0, 2000.0));
    }

    /// With Ctrl held a touchpad's delta zooms, by as much of a step as
    /// the delta is of a click, and does not pan.
    #[test]
    fn ctrl_and_a_touchpad_scroll_zooms() {
        use slint::platform::{Key, WindowEvent};
        let (app, state, _worker) = scrolled_view(1.0);
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        scroll(&app, 0.0, 10.0);
        let zoomed = state.borrow().develop.zoom;
        assert!(
            (zoomed - 1.25f32.powf(10.0 / 60.0)).abs() < 1e-5,
            "zoomed to {zoomed}"
        );
        // Six of them make a click's step.
        for _ in 0..5 {
            scroll(&app, 0.0, 10.0);
        }
        assert!((state.borrow().develop.zoom - 1.25).abs() < 1e-4);
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
        // Let go of Ctrl and the same delta pans again.
        let before = state.borrow().develop.center;
        scroll(&app, 0.0, 10.0);
        assert!((state.borrow().develop.zoom - 1.25).abs() < 1e-4);
        assert_ne!(state.borrow().develop.center, before);
    }

    /// Library and Loupe at the left pane's top: the grid and the
    /// one frame, the one on screen pressed, in the same place in
    /// both. The Loupe's modes are `panel::cull`'s tests.
    #[test]
    fn library_and_loupe_switch_the_view_and_the_one_on_screen_is_pressed() {
        use i_slint_backend_testing::ElementHandle;
        let app = window(6);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = crate::testing::state_for(&app, crate::testing::folder(6));
        app.invoke_select(0);
        let pressed = |label: &str| {
            crate::testing::count_labeled(&app, label);
            ElementHandle::find_by_accessible_label(&app, label)
                .next()
                .and_then(|e| e.accessible_checked())
        };
        let middle = |label: &str| {
            let (at, size) = crate::testing::labeled(&app, label);
            (at.x + size.width / 2.0, at.y + size.height / 2.0, at)
        };
        assert!(!app.get_grid_open());
        assert_eq!(pressed("Loupe view"), Some(true));
        assert_eq!(pressed("Library view"), Some(false));
        let (lx, ly, library_at) = middle("Library view");
        // The pane's top left, above everything that scrolls.
        assert!(
            library_at.x < 240.0 && library_at.y < 40.0,
            "{library_at:?}"
        );
        click(&app, lx, ly);
        assert!(app.get_grid_open());
        assert_eq!(pressed("Library view"), Some(true));
        assert_eq!(pressed("Loupe view"), Some(false));
        // Where it was in the loupe.
        let (_, _, there) = middle("Library view");
        assert_eq!((there.x, there.y), (library_at.x, library_at.y));
        // Library again is still the library; G agrees with the two.
        click(&app, lx, ly);
        assert!(app.get_grid_open());
        let (dx, dy, _) = middle("Loupe view");
        click(&app, dx, dy);
        assert!(!app.get_grid_open());
        crate::testing::press(&app, "g");
        assert!(app.get_grid_open());
        assert_eq!(pressed("Library view"), Some(true));
        // From the lightbox with the pane brought back: Loupe leaves
        // it for the frame, Library for the grid, as its own ways out.
        for (label, grid) in [("Loupe view", false), ("Library view", true)] {
            app.invoke_lightbox_enter();
            assert!(app.get_lightbox());
            app.set_left_hidden(false);
            let (x, y, _) = middle(label);
            click(&app, x, y);
            assert!(!app.get_lightbox(), "{label}");
            assert_eq!(app.get_grid_open(), grid, "{label}");
        }
    }

    /// The Help menu's two of its own: the keys sheet, and the guide,
    /// which a test never opens a browser for.
    #[test]
    fn help_has_the_keys_and_the_guide_and_a_test_opens_no_browser() {
        let app = window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
        assert_eq!(crate::testing::count_labeled(&app, "Help menu"), 1);
        app.invoke_guide_asked();
        assert_eq!(app.get_status(), "the guide is open in the browser");
        assert_eq!(Shown::sheet("keys"), Ok(Shown::Keys));
        assert_eq!(Shown::sheet("help"), Ok(Shown::Help));
        Shown::Keys.open(&app);
        assert!(app.get_keys_open());
        assert!(app.get_keys_rows().row_count() > 20);
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(!app.get_keys_open());
        // The menu asked for opens it, and the ask is spent.
        slint::platform::update_timers_and_animations();
        Shown::Help.open(&app);
        crate::testing::count_labeled(&app, "Help menu");
        slint::platform::update_timers_and_animations();
        assert!(!app.get_help_menu_shown());
    }

    /// The edge arrows and the undo and redo buttons show a picture
    /// and no text: each says its word at the pointer after a rest,
    /// the word names what a press will do, and the press takes it
    /// down.
    #[test]
    fn the_edge_arrows_and_undo_say_their_words_after_a_rest() {
        use crate::testing::rest_on;
        let app = window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        app.set_can_undo(true);
        app.set_can_redo(true);
        let away = || {
            app.window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(2.0, 2.0),
                });
            slint::platform::update_timers_and_animations();
        };
        for (label, words) in [
            ("Undo", "Undo"),
            ("Redo", "Redo"),
            ("Right panel", "Hide panel"),
            ("Strip", "Hide strip"),
        ] {
            assert_eq!(app.get_tip_text(), "", "no word before a rest on {label}");
            rest_on(&app, label);
            assert_eq!(app.get_tip_text(), words, "{label}");
            away();
            assert_eq!(app.get_tip_text(), "", "{label} leaves with the pointer");
        }
        // The panel scrolled under a resting pointer takes the word
        // down; the Develop tab is long enough to scroll.
        app.set_panel_tab("Develop".into());
        rest_on(&app, "Undo");
        assert_eq!(app.get_tip_text(), "Undo");
        app.set_panel_scroll(-40.0);
        assert_ne!(app.get_panel_scroll(), 0.0, "the panel scrolls");
        slint::platform::update_timers_and_animations();
        assert_eq!(app.get_tip_text(), "", "a scroll took the word down");
        away();
        // A key takes the word down, and a panel the key puts away
        // takes its controls with it: F8 over a resting Undo.
        away();
        rest_on(&app, "Undo");
        assert_eq!(app.get_tip_text(), "Undo");
        crate::testing::press(&app, slint::platform::Key::F8);
        slint::platform::update_timers_and_animations();
        assert_eq!(app.get_tip_text(), "", "a key took the word down");
        crate::testing::press(&app, slint::platform::Key::F8);
        away();
        // The press: the word, then the panel gone and the word with it,
        // and the arrow's next word is the way back.
        let (x, y) = rest_on(&app, "Right panel");
        assert_eq!(app.get_tip_text(), "Hide panel");
        click(&app, x, y);
        assert_eq!(app.get_tip_text(), "", "the press took the word down");
        assert!(app.get_right_hidden());
        away();
        rest_on(&app, "Right panel");
        assert_eq!(app.get_tip_text(), "Show panel");
    }
}
