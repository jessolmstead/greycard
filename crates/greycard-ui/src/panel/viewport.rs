use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::tr;
use greycard_core::color::{WhitePoint, resolve_white_balance};
use greycard_core::develop::defringe;
use greycard_edit::curve::Channel;
use greycard_edit::geometry::Geometry;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::panel::color::{preview_white, show_mixer_band, store_mixer_band, white_profile};
use crate::panel::crop::read_geometry;
use crate::panel::cull::{control_over_frame, leave_cull};
use crate::panel::curve::{
    current_channel, curve_points, draw_curve, parametric_mode, picked_point, select_point,
    set_curve_points, show_selected,
};
use crate::panel::edit::{current_turn, read_edit, schedule_save};
use crate::panel::startup::{FILE, SYSTEM, monitor_note, monitor_settings};
use crate::render::Renderer;
use crate::worker::{Developed, WhiteBase, Worker};
use crate::{
    App, STATE, ScopeMark, Shared, State, chooser, cull, display, export, finish, gpu, owners,
    placeholder, scope, zoom,
};

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

/// What the viewport finishes the picture on the GPU as, a raw's
/// scene or a picture already rendered: that picture's own kind,
/// which until the open frame's develop lands is the last frame's,
/// whatever the open frame is. The scopes, the clipping lamps and
/// the readout under the pointer read the same view, and the
/// droppers sample the same picture.
pub(crate) fn drawn_kind(st: &State) -> finish::Source {
    st.develop.drawn_kind
}

/// A develop going up, on the frame that draws it first: its kind
/// becomes the drawn picture's ([`drawn_kind`]), and its picture goes
/// onto the GPU with its guide and its planes when there is a GPU.
/// Both are done here and nowhere else.
pub(crate) fn picture_up(develop: &mut owners::Develop, landed: &crate::panel::deliver::Landed) {
    develop.drawn_kind = landed.kind;
    develop.drawn_guide = landed.guide.clone();
    develop.drawn_generation = landed.generation;
    develop.drawn_planes = landed.planes.clone();
    let Some(renderer) = develop.renderer.as_mut() else {
        return;
    };
    match &landed.image {
        Developed::Halves(halves) => renderer.upload(halves),
        Developed::Texture(texture) => match landed.full {
            Some(full) => renderer.set_source_sized(texture.clone(), full),
            None => renderer.set_source(texture.clone()),
        },
    }
    crate::open_note::put_up();
    renderer.set_guide(&landed.guide);
    renderer.set_detail(landed.planes.as_ref());
}

/// The camera's frame on the develop the viewport draws
/// ([`drawn_picture`]) at the turn it is drawn at, as fractions of it
/// (`stages::Facts::camera`): what an edit with no crop is framed to.
/// None for a frame whose camera framed all of it, and for a picture
/// that is not a develop (the camera's picture standing in comes cut).
/// `source_size` is the same picture once the open frame's develop has
/// landed, so the panel measures the crop by this too.
pub(crate) fn camera_crop(st: &State) -> Option<greycard_edit::geometry::Crop> {
    let (frame, tag) = st.develop.drawn_camera?;
    let (_, turn) = st.develop.shown_turn?;
    let lag = own_develop(st).unwrap_or(0);
    let at = tag.turned(i32::from((turn + lag) % 4));
    Some(greycard_edit::geometry::Crop::of(frame.oriented(at)))
}

/// The camera's frame of a raw as its develop is framed by
/// (`stages::Facts::camera`, which says whether it applies), with the
/// tag the develop turns the raw by.
pub(crate) fn camera_of_frame(
    frame: &greycard_core::raw::RawFrame,
) -> Option<(
    greycard_core::raw::CameraFrame,
    greycard_core::raw::Orientation,
)> {
    greycard_engine::stages::Facts::raw(frame, 0).camera()?;
    Some((frame.shot.camera_frame?, frame.orientation))
}

/// Where the camera focused on a raw, with the tag the develop turns
/// the raw by.
pub(crate) fn focus_of_frame(
    frame: &greycard_core::raw::RawFrame,
) -> Option<(
    greycard_core::raw::FocusPoint,
    greycard_core::raw::Orientation,
)> {
    Some((frame.shot.focus_point?, frame.orientation))
}

/// Where the camera focused on the picture the viewport draws, in
/// `develop.center`'s units: the culling loupe's camera picture by its
/// own units (`cull::loupe_focus_center`), the develop by the panel's
/// geometry ([`focus_center`]). None in a compare view.
pub(crate) fn focus_target(st: &State, app: &App) -> Option<(f32, f32)> {
    if compared(st) > 1 {
        None
    } else if crate::panel::cull::loupe_draws(st) {
        crate::panel::cull::loupe_focus_center(st, app)
    } else {
        focus_center(st, app)
    }
}

/// A focus point known only after the zoom key took the frame from fit
/// to its zoom about the middle: the view goes to the point if it is
/// still that frame, at that zoom, about the middle, as the key left
/// it; a pan, a zoom, or another frame since leaves it be. Called after
/// each draw lays the frame out, which re-centers a picture of a new
/// size.
pub(crate) fn focus_arrived(st: &mut State, app: &App) {
    let Some(pending) = st.develop.focus_pending.clone() else {
        return;
    };
    let (iw, ih) = st.develop.image_size;
    let (cx, cy) = st.develop.center;
    let middle = (cx - iw as f32 / 2.0).abs() < 0.5 && (cy - ih as f32 / 2.0).abs() < 0.5;
    let same = st.current.and_then(|c| st.key_of(c)).as_ref() == Some(&pending.frame);
    let culling = st.culling.cull.is_some();
    if !same
        || (st.develop.zoom - pending.level).abs() > 1e-3
        || !middle
        || culling != pending.culling
    {
        st.develop.focus_pending = None;
        return;
    }
    if let Some(center) = focus_target(st, app) {
        tracing::info!(
            "zoom key: {}:1 on the camera's focus point, come in since",
            pending.level
        );
        st.develop.focus_pending = None;
        st.develop.center = center;
        app.window().request_redraw();
    } else if focus_answered(st) {
        // The source has said, and it gave no point the view can take:
        // nothing comes for this press.
        st.develop.focus_pending = None;
    }
}

/// The develop's frame on show, as the render lays it out each frame:
/// the source it is of, the quarter turns the develop on the GPU lags
/// the open frame by, and the frame (the crop, or the turned source's
/// bounds while cropping).
pub(crate) struct OnShow {
    pub(crate) source: (f32, f32),
    pub(crate) lag: u8,
    pub(crate) frame: greycard_edit::geometry::Frame,
}

/// The frame on show under `edit` (the edit the view draws; `panel`
/// the panel's), laid out: the picture's size as an export writes it
/// goes to the sheet, a frame of a new size is centered, and a develop
/// that replaces a picture standing in for it takes on the zoom chosen
/// there (`cull::stood_view`) about the same place in the picture.
///
/// The picture is this frame's develop once it has landed, and until
/// then the one still on the GPU, which `edit` is the held one for. A
/// select clears this frame's size, and reading it here would draw
/// whatever is on screen into a one-pixel frame, the canvas over the
/// whole viewport. A frame turned since is drawn turned from that
/// develop ([`drawn_picture`]).
pub(crate) fn frame_on_show(
    st: &mut State,
    app: &App,
    edit: &greycard_edit::Edit,
    panel: &greycard_edit::Edit,
) -> OnShow {
    let ((dw, dh), lag) = drawn_picture(st);
    let (sw, sh) = (dw as f32, dh as f32);
    // An edit with no crop on a frame the camera framed is framed to
    // the camera's frame, as its export.
    let camera = camera_crop(st);
    let frame = if app.get_crop_mode() {
        edit.geometry.bounds(sw, sh)
    } else {
        edit.geometry.frame(sw, sh, camera)
    };
    // The picture's size as an export writes it, for the sheet's
    // arithmetic.
    let exported = panel.geometry.frame(sw, sh, camera).size;
    app.set_frame_width(exported.0 as i32);
    app.set_frame_height(exported.1 as i32);
    let size = (frame.size.0 as u32, frame.size.1 as u32);
    // Only onto the develop it was noted for, of the open frame: a
    // frame chosen since clears it (`browser::current_moved`), and a
    // picture held from another frame is not this one's.
    let stood = st
        .develop
        .stood_view
        .take()
        .filter(|v| v.generation == st.develop.drawn_generation && own_develop(st).is_some());
    if let Some(stood) = stood {
        let (zoom, center) = stood_onto(&stood, &edit.geometry, (sw, sh), &frame, camera);
        st.develop.image_size = size;
        st.develop.zoom = zoom;
        st.develop.center = center;
    } else if size != st.develop.image_size {
        st.develop.image_size = size;
        st.develop.center = (frame.size.0 / 2.0, frame.size.1 / 2.0);
    }
    OnShow {
        source: (sw, sh),
        lag,
        frame,
    }
}

/// A view on a picture standing in, as the zoom and center of the
/// develop's `frame` (of a source `sw` by `sh` under `geometry`,
/// `camera` its camera's frame) on the same place in the picture. A
/// named level (a key, a click to 1:1, the zoom menu) stays that level
/// of the develop; a free zoom (the wheel, a pinch) keeps its scale on
/// screen. The stand-in may be another size than the develop, and the
/// camera's JPEG all of the sensor where the edit crops.
fn stood_onto(
    stood: &crate::owners::StoodView,
    geometry: &Geometry,
    (sw, sh): (f32, f32),
    frame: &greycard_edit::geometry::Frame,
    camera: Option<greycard_edit::geometry::Crop>,
) -> (f32, (f32, f32)) {
    use crate::owners::Stood;
    let (fx, fy) = stood.at;
    let (center, width) = match stood.shows {
        Stood::Frame => ((fx * frame.size.0, fy * frame.size.1), frame.size.0),
        Stood::Camera { cut } => {
            // The camera's picture is the source turned and mirrored
            // as the frame is, cut to the camera's frame or not, and
            // nothing else: the point through it to the source, and
            // through the edit's geometry to its frame.
            let plain = Geometry {
                turns: geometry.turns,
                flip: geometry.flip,
                ..Geometry::default()
            };
            let whole = plain.frame(sw, sh, camera.filter(|_| cut));
            let on = (
                whole.origin.0 + fx * whole.size.0,
                whole.origin.1 + fy * whole.size.1,
            );
            let source = plain.to_source(on, sw, sh);
            let r = geometry.to_plane(source, sw, sh);
            ((r.0 - frame.origin.0, r.1 - frame.origin.1), whole.size.0)
        }
    };
    let center = (
        center.0.clamp(0.0, frame.size.0),
        center.1.clamp(0.0, frame.size.1),
    );
    let zoom = if stood.named {
        stood.zoom
    } else {
        stood.zoom * stood.width / width.max(1.0)
    };
    (zoom, center)
}

/// Whether the picture the viewport draws has said where its camera
/// focused, a point or none: the loupe's picture in hand with its
/// file's head read (or of a file no read can find a point in), the
/// develop the open frame's own. Until then a key's press waits.
pub(crate) fn focus_answered(st: &State) -> bool {
    if compared(st) > 1 {
        true
    } else if crate::panel::cull::loupe_draws(st) {
        crate::panel::cull::loupe_focus_answered(st)
    } else {
        own_develop(st).is_some()
    }
}

/// Where the camera focused, as the image pixel a view centered on it
/// holds at its center (`develop.center`'s units): the point on the
/// develop the viewport draws, turned as [`camera_crop`] turns the
/// camera's frame, taken through the panel's geometry as a click is
/// ([`view_to_source`] backward). None in a compare view, while the
/// picture drawn is not the open frame's own develop, for a frame
/// whose camera does not say, and for a point the crop leaves out.
pub(crate) fn focus_center(st: &State, app: &App) -> Option<(f32, f32)> {
    if compared(st) > 1 || !source_known(st) {
        return None;
    }
    let lag = own_develop(st)?;
    let (point, tag) = st.develop.drawn_focus?;
    let (_, turn) = st.develop.shown_turn?;
    let (u, v) = point.center(tag.turned(i32::from((turn + lag) % 4)));
    let (sw, sh) = (
        st.develop.source_size.0 as f32,
        st.develop.source_size.1 as f32,
    );
    let geometry = read_geometry(app);
    let frame = if app.get_crop_mode() {
        geometry.bounds(sw, sh)
    } else {
        geometry.frame(sw, sh, camera_crop(st))
    };
    let r = geometry.to_plane((u * sw, v * sh), sw, sh);
    let at = (r.0 - frame.origin.0, r.1 - frame.origin.1);
    let (fw, fh) = frame.size;
    ((0.0..=fw).contains(&at.0) && (0.0..=fh).contains(&at.1)).then_some(at)
}

/// The turn the develop on the GPU lags the open frame by, when that
/// develop is the open frame's own; none for another frame's.
pub(crate) fn own_develop(st: &State) -> Option<u8> {
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
            tr!("auto white balance")
        } else {
            tr!("white balance")
        };
        match self {
            Self::Waiting => tr!("{what}: wait for the develop", what = what),
            Self::NoProfile => tr!("{what}: no camera profile for this frame", what = what),
            Self::NoWhite if auto => tr!("{what}: nothing neutral to read", what = what),
            Self::NoWhite => tr!("nothing to read there"),
            Self::Unresolved(e) => tr!("{what}: {why}", what = what, why = e),
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
/// developed at, as [`sample_view_at`] reads it; the white it names goes
/// to the panel, As shot off, and the develop follows, one step of
/// the history. On a miss nothing moves, and the caller says why.
pub(crate) fn white_from_neutral(
    mut st: crate::shared::HeldMut<'_>,
    app: &App,
    px: [f32; 3],
) -> Result<(), NeutralMiss> {
    let base = own_white(&st)?;
    if let Some((own, shown)) = base.picture {
        return picture_from_neutral(st, app, px, own, shown);
    }
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
    // says. Under the mask at full weight the patch is then grey. With
    // Follow global on, it is stored as that light less the global's.
    if st.develop.target.is_some() && app.get_panel_tab() == "Masks" {
        let Some((temperature, tint)) = crate::panel::color::local_from_light(&mut st, app, tt)
        else {
            return Err(NeutralMiss::NoWhite);
        };
        app.set_local_wb_on(true);
        app.set_local_wb_kept(true);
        app.set_local_temperature(temperature);
        app.set_local_tint(tint);
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

/// [`white_from_neutral`] on a picture: the light that makes the pixel
/// grey from the one it was shown at, stored as the offset from the
/// picture's own white (`own`), on the global or under a mask, where
/// it is an offset from the global's and Follow global is not offered.
fn picture_from_neutral(
    mut st: crate::shared::HeldMut<'_>,
    app: &App,
    px: [f32; 3],
    own: greycard_core::TempTint,
    shown: greycard_core::TempTint,
) -> Result<(), NeutralMiss> {
    let light = greycard_edit::white::picture_neutral(px, shown).ok_or(NeutralMiss::NoWhite)?;
    if st.develop.target.is_some() && app.get_panel_tab() == "Masks" {
        app.set_local_wb_relative(true);
        let Some((mired, tint)) = crate::panel::color::local_from_light(&mut st, app, light) else {
            return Err(NeutralMiss::NoWhite);
        };
        app.set_local_wb_on(true);
        app.set_local_wb_kept(true);
        app.set_local_temperature(mired);
        app.set_local_tint(tint);
        drop(st);
        app.invoke_stop_placing();
        app.invoke_view_changed();
        return Ok(());
    }
    let (mired, tint) = greycard_edit::white::offset_to(light, own);
    app.set_as_shot(false);
    app.set_temperature(mired as f32);
    app.set_tint(tint as f32);
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
pub(crate) fn auto_white_too_little() -> String {
    tr!("auto white balance: not enough of the picture to read")
}

#[cfg(test)]
thread_local! {
    /// The picture the Auto white balance reads in a test, which has
    /// no GPU to read one back from, and its width.
    pub(crate) static AUTO_WHITE_GRID: std::cell::RefCell<Option<(Vec<[f32; 3]>, usize)>> =
        const { std::cell::RefCell::new(None) };
}

/// The whole developed picture, small, in the working space at the
/// white it was developed at, and how many points across: what the
/// Auto white balance and the Light section's Auto read.
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
pub(crate) fn auto_white(mut st: crate::shared::HeldMut<'_>, app: &App) {
    let found = (|| {
        let base = own_white(&st)?;
        if let Some((own, shown)) = base.picture {
            return auto_picture(&st, base.clip, own, shown);
        }
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
            app.set_status(auto_white_too_little().into());
            return;
        }
        Err(miss) => miss,
    };
    app.set_status(miss.said(true).into());
}

/// The Auto white balance's read of a picture, as a pixel as shown: the
/// grid taken back to the picture's own white first, so the prior is
/// its own white wherever the sliders stand and a second press finds
/// what the first did, then grey-world as on a raw with no gains and
/// no matrix, and the answer brought forward to the light shown.
fn auto_picture(
    st: &State,
    clip: f32,
    own: greycard_core::TempTint,
    shown: greycard_core::TempTint,
) -> Result<Option<[f32; 3]>, NeutralMiss> {
    const IDENTITY: greycard_core::color::Matrix3 =
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let (back, forward) = match (
        greycard_edit::white::adaptation(shown, own),
        greycard_edit::white::adaptation(own, shown),
    ) {
        (Some(b), Some(f)) => (b, f),
        _ => return Err(NeutralMiss::NoWhite),
    };
    let (grid, width) = auto_white_grid(st).ok_or(NeutralMiss::Waiting)?;
    let grid: Vec<[f32; 3]> = grid
        .iter()
        .map(|px| greycard_core::color::apply3(&back, *px))
        .collect();
    Ok(
        greycard_core::color::auto_neutral(&grid, width, [1.0; 3], IDENTITY, clip, [1.0; 3])
            .map(|px| greycard_core::color::apply3(&forward, px)),
    )
}

/// The Light section's Auto: the Light section's exposure and its four
/// tone sliders set from the open frame's own histogram
/// ([`greycard_edit::auto`]), read off the same grid of the developed
/// picture the Auto white balance reads, so nothing is developed again.
/// One step of the history, under the button's name; only those five
/// sliders change, and pressing it again changes nothing. A raw's
/// develop only, and only once it is up: a miss is logged, not said.
pub(crate) fn auto_light(st: &mut State, app: &App, worker: &Worker) {
    if crate::panel::queue_view::refused(st, "auto light") {
        return;
    }
    let Some(c) = st.current else {
        return;
    };
    // A panel still showing a stand-in is never recorded as a state,
    // as a preset's is not.
    if st.culling.cull.is_none()
        && let Some(why) = crate::rows::not_kept(st, c)
    {
        app.set_status(why.into());
        return;
    }
    if st.culling.cull.is_some() || own_develop(st).is_none() {
        tracing::info!("auto light: the open frame's develop is not up yet");
        return;
    }
    let kind = drawn_kind(st);
    if !kind.is_scene() {
        tracing::info!("auto light: not a raw");
        return;
    }
    let Some((grid, _)) = auto_white_grid(st) else {
        tracing::info!("auto light: the picture could not be read");
        return;
    };
    let edit = read_edit(app, &st.develop.edit, st.develop.target);
    let baseline = kind.baseline(&edit.light);
    let headroom = kind.auto_headroom(&st.develop.drawn_guide);
    let histogram = greycard_edit::auto::Histogram::of_pixels(&grid);
    let set = greycard_edit::auto::auto_light_kept(&histogram, baseline, headroom);
    tracing::info!(
        "auto light: middle {:+.2}, 0.5/50/99.5% {:+.2} {:+.2} {:+.2} stops, baseline {baseline:.2}, headroom {:.2}: {set:?}",
        histogram.trimmed_mean(0.1, 0.9),
        histogram.quantile(0.005),
        histogram.quantile(0.5),
        histogram.quantile(0.995),
        kind.headroom(),
    );
    let mut auto = edit.clone();
    auto.light = set.applied(&edit.light);
    if auto == edit {
        return;
    }
    if let Some(edit) = crate::panel::edit::panel_state(st, edit)
        && let Some(line) = st.line_mut(c)
    {
        line.record(edit);
    }
    if let Some(line) = st.line_mut(c) {
        line.record_as(auto, Some(greycard_edit::history::AUTO_LIGHT.to_string()));
    }
    crate::panel::history::take_current(st, app, worker);
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
    // The camera's frame's shape where it framed one: what Original
    // holds there.
    let (w, h) = Geometry::default().own_shape(w as f32, h as f32, camera_crop(st));
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
    let c = geometry.effective_crop(sw, sh, camera_crop(st));
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

/// Whether the picture a dropper samples is the open frame's own
/// develop, up on the GPU. Not while the last frame's develop is held
/// up waiting for this one's, nor while a camera picture stands in
/// over the develop, nor between a develop's delivery and the frame
/// that takes it up: the texture is another frame's then, or not
/// what is on screen, and the open frame's size, which the view's
/// mapping reads, is cleared on a select until its develop is up.
pub(crate) fn dropper_reads_develop(st: &State) -> bool {
    own_develop(st).is_some()
        && st.develop.pending.is_none()
        && crate::panel::cull::standing_in(st).is_none()
}

/// What the status line says to a dropper pressed before the open
/// frame's develop is up; the Neutral dropper's words are its
/// [`NeutralMiss::Waiting`]'s.
pub(crate) fn waiting_said(kind: &str) -> String {
    let what = match kind {
        // A tool waiting: 'white balance: wait for the develop'
        "White" => tr!("white balance"),
        "Curve" => tr!("curve"),
        "Range" => tr!("color range"),
        "Defringe" => tr!("defringe"),
        _ => tr!("color mixer"),
    };
    tr!("{what}: wait for the develop", what = what)
}

/// The hint the status line shows for a dropper in hand.
pub(crate) fn picking_hint(kind: &str) -> String {
    match kind {
        "White" => tr!("click something neutral; Esc to finish"),
        "Curve" => tr!("click a tone to add a point, drag up or down to move it; Esc to finish"),
        "Range" => tr!("click a color to center the mask's hue on it; Esc to finish"),
        "Defringe" => {
            tr!("defringe is off while picking; zoom to 1:1 and click a fringe; Esc to finish")
        }
        _ => {
            tr!(
                "click a color for its band, drag up or down for saturation, sideways for hue; Esc to finish"
            )
        }
    }
}

/// Whether the open frame's size is known, which the view's mapping
/// needs: until its first develop lands it has none, and the crop's
/// fit on a source of no size is 0/0 (its aspect, its keystone), so a
/// straightened frame maps every point to NaN. An outline drawn
/// through that panics in the renderer's tessellator; it waits.
pub(crate) fn source_known(st: &State) -> bool {
    st.develop.source_size.0 > 0 && st.develop.source_size.1 > 0
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
            geometry.frame(sw, sh, camera_crop(st))
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
        geometry.frame(sw, sh, camera_crop(st))
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

/// Whether a point of the view (logical pixels) lies on the picture
/// as the pan and the zoom place it, not on the canvas round it: what
/// a drag needs under its press to pan. A compare view's tiles each
/// place their own frame, so every point there counts as on one, and
/// so does every point once a drag has carried the whole picture off
/// the view, so the next drag can bring it back. It reads
/// `image_size`, not the source and the panel's geometry as
/// [`view_on_picture`] does, because that is the drawn picture's
/// frame, whatever a peek or a held picture shows.
pub(crate) fn picture_at(st: &State, app: &App, x: f32, y: f32) -> bool {
    if compared(st) > 1 {
        return true;
    }
    let scale = app.window().scale_factor();
    let view = (
        app.get_view_width().max(1) as u32,
        app.get_view_height().max(1) as u32,
    );
    let zoom = effective_zoom(st, view.0, view.1);
    let (center, image) = (st.develop.center, st.develop.image_size);
    !zoom::in_view(view, zoom, center, image)
        || zoom::on_picture((x * scale, y * scale), view, zoom, center, image)
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
        geometry.frame(sw, sh, camera_crop(st))
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
    /// test, which has no GPU to read one back from, with the coverage
    /// in the alpha.
    pub(crate) static HOVER_PICTURE: std::cell::RefCell<Option<greycard_viewport::Rgba8>> =
        const { std::cell::RefCell::new(None) };
}

/// [`Renderer::analyzed_at`] for the state.
fn analyzed_at(st: &State, u: f32, v: f32, radius: f32) -> Option<scope::Reading> {
    #[cfg(test)]
    if let Some(read) = HOVER_PICTURE.with(|p| {
        p.borrow().as_ref().map(|p| {
            scope::under_pointer(&p.pixels, p.width as usize, p.height as usize, u, v, radius)
        })
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
fn hover_left(state: Rc<Shared>, app_weak: slint::Weak<App>) {
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
        geometry.frame(sw, sh, camera_crop(st))
    };
    let (pw, ph) = geometry.plane_size(sw, sh);
    let r = (
        (x * scale - vw as f32 / 2.0) / zoom + st.develop.center.0 + frame.origin.0,
        (y * scale - vh as f32 / 2.0) / zoom + st.develop.center.1 + frame.origin.1,
    );
    (r.0 / pw, r.1 / ph)
}

/// The color pick's sample sizes, the viewport's.
pub(crate) use greycard_viewport::render::{SAMPLE_SIZES, sample_reach, sample_size};

/// The developed picture under a point of the view (logical pixels):
/// the mean of the square of `reach` pixels each way about it, in the
/// working space at the white it was developed at; `None` off the
/// picture or without one. The droppers' usual reach is two, a 5x5;
/// the color pick's is chosen, and the defringe's dropper takes its
/// local mean over a wider one.
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
    let edit = read_edit(app, &st.develop.edit, st.develop.target);
    pick_through(st, app, px, &edit)
}

/// What a wheel's pick reads: [`pick_stages`] with the grading and
/// the tint out, the rest of the look in. A second pick on a cast the
/// wheel already pushes still answers the cast's hue, not the pushed
/// one, so the pick does not chase itself. The hue, and the Oklab
/// chroma there for a wheel at nothing to take its amount from.
pub(crate) fn wheel_pick(st: &mut State, app: &App, px: [f32; 3]) -> (f32, f32) {
    let edit = greycard_edit::Edit {
        grading: Default::default(),
        tint: greycard_edit::Tint::OFF,
        ..read_edit(app, &st.develop.edit, st.develop.target)
    };
    let picked = pick_through(st, app, px, &edit);
    (picked.shown_hue, picked.shown_chroma)
}

fn pick_through(
    st: &mut State,
    app: &App,
    px: [f32; 3],
    edit: &greycard_edit::Edit,
) -> finish::Picked {
    let key = (app.get_as_shot(), app.get_temperature(), app.get_tint());
    let m = preview_white(st, key);
    let px: [f32; 3] = std::array::from_fn(|r| m[r][0] * px[0] + m[r][1] * px[1] + m[r][2] * px[2]);
    finish::pick(
        px,
        &edit.light.effective(),
        &edit.acting_mixer(),
        &edit.color,
        &edit.bw,
        &edit.tint,
        &edit.curves.bake_with(&edit.grading),
        drawn_kind(st),
        edit.display_curve,
        crate::panel::startup::profile_tone(st, edit).as_ref(),
        st.develop.drawn_guide.frame_range(),
    )
}

/// The zoom in words for the panel: fitted, or a percentage of 1:1.
pub(crate) fn zoom_label(zoom: f32) -> slint::SharedString {
    if zoom > 0.0 {
        format!("{}%", (zoom * 100.0).round() as i32).into()
    } else {
        tr!("zoom" => "Fit").into()
    }
}

/// [`zoom::effective`] for the state: the zoom it holds, the cell
/// the compare view gives, the mat on one frame alone, and the size
/// of the picture on the GPU.
pub(crate) fn effective_zoom(st: &State, vw: u32, vh: u32) -> f32 {
    let cell = view_cell(st, vw, vh);
    let cell = if st.develop.mat && compared(st) <= 1 {
        zoom::matted(cell)
    } else {
        cell
    };
    zoom::effective(st.develop.zoom, cell, st.develop.image_size)
}

/// How many frames the compare view is showing, 1 for none.
fn compared(st: &State) -> usize {
    st.culling
        .cull
        .as_ref()
        .or(st.culling.hold.as_ref())
        .map_or(1, |c| c.compare)
}

/// [`zoom::cell`] for the state: how many frames the compare view
/// is showing, if it is showing any.
pub(crate) fn view_cell(st: &State, vw: u32, vh: u32) -> (u32, u32) {
    zoom::cell(vw, vh, compared(st))
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
/// the ScrollPane puts it back to the top) and the capture waits a
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
    /// The Fit button's menu of zooms, open.
    Zoom,
    /// The white balance's presets, open off Auto's chevron.
    WhitePresets,
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
            "zoom" => Ok(Self::Zoom),
            "wb-presets" => Ok(Self::WhitePresets),
            _ => Err(format!(
                "want export, preset, fetch, lenses, settings, sync, synced, \
                 preset-onto-set, preset-remove, preset-report, paste, pasted, import, \
                 imported, import-in-place, match, matched, refit, look-remove, look-rename, \
                 time-shift, root-name, \
                 delete, archive, archive-rejects, parts, landscape, lightroom, \
                 lightroom-report, lightroom-written, rawtherapee-offer, rawtherapee, \
                 rawtherapee-written, sort, keys, help, zoom, or wb-presets, not {name}"
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
                    st.develop.looks = greycard_edit_io::looks::list();
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
            Self::Zoom => app.set_zoom_menu_shown(true),
            Self::WhitePresets => {
                app.set_panel_tab("Develop".into());
                app.set_wb_presets_shown(true);
            }
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
fn when_quiet(state: Rc<Shared>, then: impl FnOnce() + 'static, tries: u32) {
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
    state: &Rc<Shared>,
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
                        "f11" => Key::F11.into(),
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
    title: impl Into<String>,
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
        // The file chooser's filter name for profiles
        (tr!("ICC profiles"), vec!["*.icc".into(), "*.icm".into()]),
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
                        tracing::warn!("ICC profile chooser: {e:#}");
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

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
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
            crate::panel::original::off(&mut st, &app, false);
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
            choose_icc(&app, tr!("Proof profile"), current, |app, path| {
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
            choose_icc(&app, tr!("Monitor profile"), current, |app, path| {
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
    // The Light section's Auto: the light sliders from the picture.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_light_auto(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            auto_light(&mut state.borrow_mut(), &app, &worker);
        });
    }
    // The Auto white balance: the white read off the whole picture.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_wb_auto(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // No white tints a monochrome frame: nothing to read.
            if crate::panel::color::white_refused(&state.borrow(), &app) {
                return;
            }
            auto_white(state.borrow_mut(), &app);
        });
    }
    // The droppers: a button puts one in hand, or takes it back; a
    // press on the view reads the developed picture there, a drag
    // moves what the press chose, a release records it.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_pick_started(move |kind| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // The white's dropper does not apply to a monochrome
            // frame: nothing is put down or taken up.
            if kind == "White" && crate::panel::color::white_refused(&state.borrow(), &app) {
                return;
            }
            let same = app.get_picking() == kind;
            app.invoke_stop_placing();
            if !same {
                app.set_picking(kind.clone());
                let hint = picking_hint(kind.as_str());
                crate::panel::deliver::show_hint(&app, &mut state.borrow_mut(), &hint);
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
            let picking = app.get_picking();
            // The white's dropper reads nothing on a monochrome frame.
            if picking == "White" && crate::panel::color::white_refused(&st, &app) {
                return;
            }
            if !dropper_reads_develop(&st) {
                crate::panel::deliver::show_hint(&app, &mut st, &waiting_said(picking.as_str()));
                return;
            }
            // The color pick's square is the size chosen beside it;
            // every other dropper's the usual 5x5.
            let reach = if picking == "Range" {
                sample_reach(app.get_range_sample())
            } else {
                2
            };
            let Some(px) = sample_view_at(&st, &app, x, y, reach) else {
                return;
            };
            match picking.as_str() {
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
                    // panel's look to where the curve reads it: past
                    // the curves ahead of it, where its tone stands in
                    // the editor's histogram.
                    let picked = pick_stages(&mut st, &app, px);
                    let channel = current_channel(&app);
                    let value = crate::panel::curve::picked_tone(
                        &crate::panel::curve::panel_curves(&app),
                        channel,
                        &picked,
                    );
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
                    app.set_curve_image(draw_curve(&app, crate::panel::curve::backdrop(&app, &st.develop)));
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
                            tr!("no fringe there: that color does not depart from its neighbors'")
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
                    // the panel's white; the dropper's square, 5x5 or
                    // wider as chosen, is the mean the mask reads a hue
                    // from, near enough.
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
                        tr!(
                            "that color is nearly grey (chroma {chroma}, under the floor's {floor}): lower Chroma to take it in",
                            chroma = format!("{:.1}", s.chroma() * 100.0),
                            floor = format!("{:.1}", floor * 100.0)
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
    // A Ctrl-click on the picture with a wheel in hand: the wheel's
    // hue from the color there, as the picture shows it, sampled as
    // the color pick samples; with Shift, the complement.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_wheel_pick(move |x, y, complement| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let hand = app.get_wheel_hand();
            let mut st = state.borrow_mut();
            if hand < 0 || !dropper_reads_develop(&st) {
                return;
            }
            let reach = sample_reach(app.get_range_sample());
            let Some(px) = sample_view_at(&st, &app, x, y, reach) else {
                return;
            };
            let (hue, chroma) = wheel_pick(&mut st, &app, px);
            drop(st);
            let hue = if complement { hue + 180.0 } else { hue };
            crate::panel::color::set_hand_hue(&app, hand, hue, chroma);
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
                    app.set_curve_image(draw_curve(
                        &app,
                        crate::panel::curve::backdrop(&app, &st.develop),
                    ));
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
            // The image pixel under the cursor stays under the cursor,
            // in the compare view the one in the tile it is over: as
            // many tiles as the frame drew.
            let tiles = st
                .culling
                .cull
                .as_ref()
                .filter(|c| c.compare > 1)
                .map_or(1, |c| c.rows(&st.views.shown).len());
            let (px, py) = zoom::offset_in_tile(vw, vh, tiles, (x * scale, y * scale));
            st.develop.center.0 += px / old - px / new;
            st.develop.center.1 += py / old - py / new;
            st.develop.zoom = new;
            st.develop.zoom_free = true;
            st.develop.zoom_back = None;
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
            st.develop.zoom_back = None;
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
                st.develop.zoom_free = false;
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
            if !zoom::on_picture(
                (x * scale, y * scale),
                (vw, vh),
                zoom,
                st.develop.center,
                (iw, ih),
            ) {
                return;
            }
            let fitted = st.develop.zoom == 0.0;
            drop(st);
            zoom_to2(if fitted { 1.0 } else { 0.0 }, x, y);
        });
        // A middle click: 200% at the point, and at 200% back to the
        // zoom and the place it came from, or to fit when that was
        // another picture's. Not over the canvas, nor in the compare
        // view, where a click chooses a tile.
        let (state, app_weak) = (outer.clone(), app.as_weak());
        let zoom_to3 = zoom_to.clone();
        app.on_toggle_zoom_200(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let scale = app.window().scale_factor();
            let (vw, vh) = (
                app.get_view_width().max(1) as u32,
                app.get_view_height().max(1) as u32,
            );
            if compared(&st) > 1 {
                return;
            }
            let (iw, ih) = st.develop.image_size;
            if iw == 0 || ih == 0 {
                return;
            }
            let zoom = effective_zoom(&st, vw, vh);
            if !zoom::on_picture(
                (x * scale, y * scale),
                (vw, vh),
                zoom,
                st.develop.center,
                (iw, ih),
            ) {
                return;
            }
            let frame = st.current.and_then(|c| st.key_of(c)).map(|k| (k, (iw, ih)));
            if (st.develop.zoom - 2.0).abs() < 1e-3 {
                // Kept only while nothing else has moved the zoom; a
                // pan at 200% keeps it, as the look round is the point.
                match st.develop.zoom_back.take() {
                    Some(back) if back.frame == frame => {
                        tracing::info!("middle click: back from 200% to {}", back.zoom);
                        st.develop.zoom = back.zoom;
                        st.develop.zoom_free = back.free;
                        st.develop.center = back.center;
                        app.window().request_redraw();
                    }
                    _ => {
                        tracing::info!("middle click: back from 200% to fit");
                        drop(st);
                        zoom_to3(0.0, x, y);
                    }
                }
            } else {
                let back = crate::owners::ZoomBack {
                    frame,
                    zoom: st.develop.zoom,
                    free: st.develop.zoom_free,
                    center: st.develop.center,
                };
                tracing::info!("middle click: 200% from {}", st.develop.zoom);
                drop(st);
                zoom_to3(2.0, x, y);
                state.borrow_mut().develop.zoom_back = Some(back);
            }
        });
        let (state, app_weak) = (outer.clone(), app.as_weak());
        app.on_mat_key(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.develop.mat = !st.develop.mat;
            app.window().request_redraw();
        });
        // A focus point read off the window's thread redraws it, so a
        // key's zoom waiting on it goes there (`focus_arrived`).
        let app_weak = app.as_weak();
        crate::focus_points::set_wake(move || {
            let _ = app_weak.upgrade_in_event_loop(|app| app.window().request_redraw());
        });
        let (state, app_weak) = (outer.clone(), app.as_weak());
        app.on_zoom_key(move |level| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let at_level = (state.borrow().develop.zoom - level).abs() < 1e-3;
            // From fit, the key's zoom goes to where the camera focused,
            // when it says; from any other zoom it is about the middle,
            // keeping the place. A point not known yet is waited for.
            {
                let mut st = state.borrow_mut();
                st.develop.focus_pending = None;
                if st.develop.zoom <= 0.0 {
                    if let Some(center) = focus_target(&st, &app) {
                        tracing::info!("zoom key: {level}:1 on the camera's focus point");
                        st.develop.zoom_back = None;
                        st.develop.zoom = level;
                        st.develop.zoom_free = false;
                        st.develop.center = center;
                        app.window().request_redraw();
                        return;
                    }
                    // Waited for only while the picture has not said:
                    // never in a compare view, nor once it said none.
                    if !focus_answered(&st) {
                        let culling = st.culling.cull.is_some();
                        st.develop.focus_pending =
                            st.current.and_then(|c| st.key_of(c)).map(|frame| {
                                crate::owners::FocusPending {
                                    frame,
                                    level,
                                    culling,
                                }
                            });
                    }
                    // A loupe picture from the local previews read
                    // nothing of the file: its head is read now, one
                    // read a path; with none to be had, no wait.
                    if st.develop.focus_pending.is_some()
                        && crate::panel::cull::loupe_draws(&st)
                        && let Some(path) = crate::panel::cull::loupe_focus_owed(&st)
                        && !crate::focus_points::read_then(path)
                    {
                        st.develop.focus_pending = None;
                    }
                }
            }
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
    // Only a drag that starts on the picture pans it; one that starts
    // on the canvas round it does nothing. Asked while a callback
    // holds the state, it says yes, as every point did before.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_picture_at(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return true;
            };
            state
                .try_borrow()
                .map_or(true, |st| picture_at(&st, &app, x, y))
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
pub(crate) mod tests;
