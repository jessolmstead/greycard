use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use crate::tr;
use greycard_edit::brush::{Op, Raster, Stroke};
use greycard_edit::mask::{Component, MIN_RADIUS, Mask, Mode, Pick, Shape, on_angle_step};
use greycard_edit::retouch::{Method as RetouchMethod, Patch};
use greycard_edit::{Adjustment, Edit, Look};
use greycard_render::refine::MaskPicture;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::ai::{Key, prompted};
use crate::finish::{Local, MAX_LOCALS, RasterRef};
use crate::panel::assets::offer_model;
use crate::panel::cull::leave_cull;
use crate::panel::edit::{develop_soon, read_edit, show_edit, show_names};
use crate::panel::retouch::show_patches;
use crate::panel::viewport::{
    ViewMap, source_known, source_to_view, view_on_picture, view_to_source,
};
use crate::worker::{Job, Worker};
use crate::{App, Pt, Shared, State, WORKER};

// What to do for a learned shape wanted, decided in the engine so an
// export with no window picks the same file.
pub(crate) use greycard_engine::ai::{Step, edge_offer, step};

/// The chosen shape's handles and outline on the view, at every frame.
///
/// None until the open frame's size is known ([`source_known`]).
pub(crate) fn show_shape(st: &State, app: &App, edit: &Edit) {
    let known = source_known(st);
    let Some(component) = chosen_shape(st, app, edit).filter(|_| known) else {
        app.set_mask_kind("".into());
        return;
    };
    let map = ViewMap::of(st, app);
    let handles: Vec<Pt> = component
        .shape
        .handles()
        .into_iter()
        .map(|(u, v)| {
            let (x, y) = map.to_view(u, v);
            Pt { x, y }
        })
        .collect();
    let (mut angle, mut rx, mut ry) = (0.0f32, 0.0f32, 0.0f32);
    match component.shape {
        Shape::Linear { .. } => {
            let (dx, dy) = (handles[2].x - handles[1].x, handles[2].y - handles[1].y);
            angle = dy.atan2(dx).to_degrees() + 90.0;
        }
        Shape::Radial { .. } => {
            let (cx, cy) = (handles[0].x, handles[0].y);
            let (ax, ay) = (handles[1].x - cx, handles[1].y - cy);
            let (bx, by) = (handles[3].x - cx, handles[3].y - cy);
            angle = ay.atan2(ax).to_degrees();
            rx = (ax * ax + ay * ay).sqrt();
            ry = (bx * bx + by * by).sqrt();
        }
        Shape::Brush { .. }
        | Shape::Subject {}
        | Shape::Background {}
        | Shape::Sky { .. }
        | Shape::Landscape { .. }
        | Shape::Object { .. }
        | Shape::Part { .. }
        | Shape::Luminance { .. }
        | Shape::Color { .. }
        | Shape::Unknown => {}
    }
    // A point off the plane reaching a path panics in the renderer:
    // whatever else brings one, the outline sits this frame out. Said
    // at debug, since it is said at every frame drawn.
    let finite = handles.iter().all(|p| p.x.is_finite() && p.y.is_finite())
        && [angle, rx, ry].iter().all(|v| v.is_finite());
    if !finite {
        tracing::debug!(
            "mask: the chosen {} shape maps off the view; its outline is left out",
            component.shape.name()
        );
        app.set_mask_kind("".into());
        return;
    }
    match component.shape {
        Shape::Linear { .. } => app.set_mask_angle(angle),
        Shape::Radial { feather, .. } => {
            app.set_mask_angle(angle);
            app.set_mask_rx(rx);
            app.set_mask_ry(ry);
            app.set_mask_feather_scale(1.0 - feather.clamp(0.0, 1.0));
        }
        _ => {}
    }
    crate::panel::startup::sync_rows(&st.develop.mask_handles, handles);
    app.set_mask_kind(component.shape.name().into());
}

/// The shape the panel has chosen, while no crop or placing hides it.
pub(crate) fn chosen_shape(st: &State, app: &App, edit: &Edit) -> Option<Component> {
    st.develop
        .target
        .filter(|_| !app.get_crop_mode() && st.develop.placing.is_none())
        .and_then(|i| edit.adjustments.get(i))
        .and_then(|a| a.mask.components.get(app.get_component().max(0) as usize))
        .cloned()
}

/// Zoom to use for a view of `vw` by `vh`: the state's, or fit.
/// A shape being drawn: what kind, where the press was in the
/// masks' units, and which adjustment it makes.
pub(crate) struct Placing {
    pub(crate) kind: Shape,
    pub(crate) mode: Mode,
    pub(crate) from: (f32, f32),
    /// The adjustment it goes to, and the component it is there once
    /// a press has made it. A brush keeps its component between
    /// strokes.
    pub(crate) index: usize,
    pub(crate) component: Option<usize>,
    /// An object's press has become a box.
    pub(crate) boxed: bool,
    /// Shift at the press, then as each move of the drag reads it: a
    /// linear held to steps of its angle, a radial to a circle.
    pub(crate) held: bool,
}

/// `p` turned about `anchor`, both in the masks' units, to the nearest
/// angle step as the view shows them: a Straighten or a keystone
/// between the two, a step level on the screen stays level.
fn on_view_step(st: &State, app: &App, anchor: (f32, f32), p: (f32, f32)) -> (f32, f32) {
    let map = ViewMap::of(st, app);
    let s = on_angle_step(map.to_view(anchor.0, anchor.1), map.to_view(p.0, p.1));
    view_to_source(st, app, s.0, s.1)
}

/// The brush row back on Add when a brush asked for under Subtract put
/// it on Subtract and is now put down, unless the row was moved since.
fn settle_brush_row(st: &mut State, app: &App) {
    if std::mem::take(&mut st.develop.brush_row_cuts) && app.get_brush_op() == Op::Subtract.name() {
        app.set_brush_op(Op::Add.name().into());
    }
}

/// A stroke starting at `at`, as the panel has the brush.
pub(crate) fn stroke_from(app: &App, at: (f32, f32)) -> Stroke {
    let mut stroke = Stroke::new(
        Op::from_name(app.get_brush_op().as_str()).unwrap_or_default(),
        app.get_brush_size().clamp(0.002, 0.5),
        app.get_brush_feather().clamp(0.0, 1.0),
        app.get_brush_flow().clamp(0.0, 1.0),
    );
    stroke.points.push([at.0, at.1]);
    stroke
}

/// The panel's names for a mask's shapes: the sign of the mode and
/// the kind.
pub(crate) fn component_names(mask: &Mask) -> ModelRc<slint::SharedString> {
    let names: Vec<slint::SharedString> = mask
        .components
        .iter()
        .map(|c| format!("{} {}", c.mode.sign(), c.shape.name()).into())
        .collect();
    ModelRc::new(VecModel::from(names))
}

/// Which of a mask's shapes are switched on, one per row.
pub(crate) fn component_on(mask: &Mask) -> ModelRc<bool> {
    ModelRc::new(VecModel::from(
        mask.components
            .iter()
            .map(|c| c.enabled)
            .collect::<Vec<_>>(),
    ))
}

/// Which of a mask's shapes are inverted, one per row: at a glance
/// beside each, not only the chosen one's.
pub(crate) fn component_invert(mask: &Mask) -> ModelRc<bool> {
    ModelRc::new(VecModel::from(
        mask.components.iter().map(|c| c.invert).collect::<Vec<_>>(),
    ))
}

/// Put the chosen shape's own controls on the panel.
pub(crate) fn show_component(mask: &Mask, app: &App) {
    let i = (app.get_component().max(0) as usize).min(mask.components.len().saturating_sub(1));
    app.set_component(i as i32);
    match mask.components.get(i) {
        Some(c) => {
            let feather = match c.shape {
                Shape::Radial { feather, .. } => Some(feather),
                _ => None,
            };
            app.set_has_feather(feather.is_some());
            app.set_mask_feather(feather.unwrap_or(0.5));
            show_range(&c.shape, app);
            app.set_component_kind(c.shape.name().into());
            app.set_component_heading(component_heading(mask, i).into());
        }
        None => {
            app.set_has_feather(false);
            app.set_component_kind("".into());
            app.set_component_heading("".into());
        }
    }
}

/// What the chosen shape's card is headed: its kind, and its place
/// among the shapes of that kind, in row order, when there are more.
pub(crate) fn component_heading(mask: &Mask, i: usize) -> String {
    let Some(kind) = mask.components.get(i).map(|c| c.shape.name()) else {
        return String::new();
    };
    let same = |c: &&Component| c.shape.name() == kind;
    if mask.components.iter().filter(same).count() < 2 {
        return kind.to_string();
    }
    let n = mask.components[..=i].iter().filter(same).count();
    format!("{kind} {n}")
}

/// The chosen adjustment's name as the look sections' titles show it:
/// trimmed, and by its place in the list when it has none.
pub(crate) fn heading_name(name: &str, index: usize) -> String {
    match name.trim() {
        "" => tr!("Adjustment {n}", n = index + 1),
        name => name.to_string(),
    }
}

/// A range shape's window on the panel's sliders; nothing for any
/// other shape.
pub(crate) fn show_range(shape: &Shape, app: &App) {
    match *shape {
        Shape::Luminance {
            low,
            high,
            low_feather,
            high_feather,
        } => {
            app.set_lum_low(low);
            app.set_lum_high(high);
            app.set_lum_low_feather(low_feather);
            app.set_lum_high_feather(high_feather);
        }
        Shape::Color {
            hue,
            width,
            hue_feather,
            chroma,
            chroma_feather,
        } => {
            app.set_range_hue(hue);
            app.set_range_width(width);
            app.set_range_hue_feather(hue_feather);
            app.set_range_chroma(chroma);
            app.set_range_chroma_feather(chroma_feather);
        }
        _ => {}
    }
}

/// The panel's sliders into a range shape of the same kind; any other
/// shape is left as it is.
pub(crate) fn read_range(shape: &mut Shape, app: &App) {
    match shape {
        Shape::Luminance { .. } => {
            let low = app.get_lum_low().clamp(0.0, 1.0);
            *shape = Shape::Luminance {
                low,
                high: app.get_lum_high().clamp(low, 1.0),
                low_feather: app.get_lum_low_feather().clamp(0.0, 1.0),
                high_feather: app.get_lum_high_feather().clamp(0.0, 1.0),
            }
        }
        Shape::Color { .. } => {
            let chroma = app.get_range_chroma().max(0.0);
            *shape = Shape::Color {
                hue: app.get_range_hue().rem_euclid(360.0),
                width: app.get_range_width().clamp(0.0, 360.0),
                hue_feather: app.get_range_hue_feather().clamp(0.0, 180.0),
                chroma,
                chroma_feather: app.get_range_chroma_feather().clamp(0.0, chroma),
            }
        }
        _ => {}
    }
}

/// Whether a shape is made whole by its button, with no drag on the
/// picture: a model's subject, or a range of the picture's own.
fn made_at_once(kind: &str) -> bool {
    matches!(
        kind,
        "Subject" | "Background" | "Sky" | "Luminance" | "Color"
    )
}

/// What the status line says once a shape made at once is in.
fn made_hint(kind: &str) -> String {
    match kind {
        "Subject" => tr!("finding the subject"),
        "Background" => tr!("finding the background"),
        "Sky" => tr!("finding the sky"),
        "Color" => {
            tr!("click a color in the picture to center the window on it; Esc to keep the skin")
        }
        _ => String::new(),
    }
}

/// A mask fresh from "New mask" carries this placeholder until its
/// first shape lands.
fn placeholder_name(id: u64) -> String {
    Adjustment::placeholder_name(id)
}

/// The mask's name once its first shape lands, as `add_adjustment`
/// used to name one made whole ("Subject 3", "Linear 1"): only while
/// it still carries `placeholder_name`'s text, so a mask already
/// renamed by hand is left alone. Says whether it renamed anything,
/// so a caller only refreshes the panel's own copy of the name when
/// it would otherwise go stale.
pub(crate) fn name_first_shape(a: &mut Adjustment, shape_name: &str) -> bool {
    if a.name == placeholder_name(a.id) {
        a.name = format!("{shape_name} {}", a.id);
        true
    } else {
        false
    }
}

/// A shape as it was when a handle was pressed, and which handle.
pub(crate) struct MaskDrag {
    shape: Shape,
    handle: usize,
    /// The handle's place on the view when pressed, logical pixels.
    at: (f32, f32),
    /// Shift at the press, then as each move reads it
    /// (`Shape::dragged_held`).
    held: bool,
}

/// The adjustments baked for the viewport, their brushes' rasters
/// from `rasters`, brought up to their strokes, their learned masks
/// from `learned` where made for the shape as it is; a raster no
/// shape has any more is dropped. Also the learned shapes still
/// wanting their raster. `white` is what their own white balances are
/// made matrices from (`panel::color::local_white_shift`).
pub(crate) fn bake_locals(
    edit: &Edit,
    rasters: &mut HashMap<(u64, usize), Arc<Raster>>,
    learned: &mut HashMap<Key, (Shape, Arc<Raster>)>,
    aspect: f32,
    white: Option<&greycard_edit::WhiteShift>,
) -> (Vec<Local>, Vec<(Key, Shape)>) {
    let mut live = std::collections::HashSet::new();
    let mut wants = Vec::new();
    let locals = edit
        .adjustments
        .iter()
        .take(MAX_LOCALS)
        .map(|a| {
            let rasters = a
                .mask
                .components
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    if c.shape.is_learned() {
                        live.insert((a.id, i));
                        return match learned.get(&(a.id, i)) {
                            Some((s, r)) if *s == c.shape => Some(RasterRef(r.clone())),
                            _ => {
                                // A shape switched off is not worth a
                                // model's seconds; one it already has
                                // is kept, so the switch comes back
                                // without the wait.
                                if c.enabled && prompted(&c.shape) {
                                    wants.push(((a.id, i), c.shape.clone()));
                                }
                                None
                            }
                        };
                    }
                    let Shape::Brush { strokes } = &c.shape else {
                        return None;
                    };
                    live.insert((a.id, i));
                    let raster = rasters
                        .entry((a.id, i))
                        .and_modify(|r| {
                            if r.aspect != aspect {
                                *r = Arc::new(Raster::new(aspect));
                            }
                        })
                        .or_insert_with(|| Arc::new(Raster::new(aspect)));
                    // A copy elsewhere, the renderer's key or the
                    // histogram's, keeps the old version; this one moves
                    // on. Only when the strokes moved: a copy made for
                    // nothing is a new mask to every key, and an idle
                    // frame would draw and read the scopes again forever.
                    if !raster.is_at(strokes) {
                        Arc::make_mut(raster).update(strokes);
                    }
                    Some(RasterRef(raster.clone()))
                })
                .collect();
            Local::of(a, rasters, white)
        })
        .collect();
    rasters.retain(|k, _| live.contains(k));
    learned.retain(|k, _| live.contains(k));
    (locals, wants)
}

/// A refined mask as the window keeps it, and what it was made from.
pub(crate) struct Refined {
    key: RefinedKey,
    raster: Option<RasterRef>,
}

/// What a refined mask is made from: the mask (a brush's strokes in
/// it, so its raster is not kept here: holding it would have the next
/// stroke's `Arc::make_mut` copy it, and the key miss every frame),
/// its learned shapes' rasters (never changed in place), the picture
/// it is made against (held, so its place cannot be another's), and
/// the exposure and the stops it is shown at.
struct RefinedKey {
    mask: greycard_edit::Mask,
    learned: Vec<Option<RasterRef>>,
    picture: Arc<MaskPicture>,
    aspect: u32,
    exposure: u32,
    stops: u32,
}

/// A local's learned shapes' rasters, a brush's left out.
fn learned_of<'a>(local: &'a Local) -> impl Iterator<Item = Option<&'a RasterRef>> + 'a {
    local
        .mask
        .components
        .iter()
        .enumerate()
        .map(|(i, c)| match c.shape {
            Shape::Brush { .. } => None,
            _ => local.rasters.get(i).and_then(|r| r.as_ref()),
        })
}

impl RefinedKey {
    /// Whether `local` refined against `picture` as given would be made
    /// from what this key says: read off the local, nothing cloned.
    fn serves(
        &self,
        local: &Local,
        picture: &Arc<MaskPicture>,
        aspect: f32,
        exposure: f32,
        stops: f32,
    ) -> bool {
        Arc::ptr_eq(&self.picture, picture)
            && self.aspect == aspect.to_bits()
            && self.exposure == exposure.to_bits()
            && self.stops == stops.to_bits()
            && self.mask == local.mask
            && self.learned.len() == local.mask.components.len()
            && self
                .learned
                .iter()
                .map(Option::as_ref)
                .eq(learned_of(local))
    }
}

/// The baked `locals` of `edit` refined where their Refine edge is on,
/// against `picture` (the one on the GPU's, when it is the open frame's
/// own at its turn), at the global `exposure` and shown `stops` over
/// the picture as developed: each made again only when what it is made
/// from changed, so a look slider moved under a refined mask costs
/// nothing. A mask switched off is refined too, since its overlay can
/// still be shown. Without a picture nothing is refined.
pub(crate) fn refine_locals(
    edit: &Edit,
    locals: &mut [Local],
    refined: &mut HashMap<u64, Refined>,
    picture: Option<&Arc<MaskPicture>>,
    aspect: f32,
    exposure: f32,
    stops: f32,
) {
    let ids: Vec<u64> = edit
        .adjustments
        .iter()
        .take(locals.len())
        .map(|a| a.id)
        .collect();
    refined.retain(|id, _| ids.contains(id));
    for (local, id) in locals.iter_mut().zip(ids) {
        let Some(picture) = picture.filter(|_| local.mask.refine > 0.0) else {
            refined.remove(&id);
            continue;
        };
        if let Some(r) = refined
            .get(&id)
            .filter(|r| r.key.serves(local, picture, aspect, exposure, stops))
        {
            local.refined = r.raster.clone();
            continue;
        }
        local.refine(picture, aspect, exposure, stops);
        let key = RefinedKey {
            mask: local.mask.clone(),
            learned: learned_of(local).map(|r| r.cloned()).collect(),
            picture: picture.clone(),
            aspect: aspect.to_bits(),
            exposure: exposure.to_bits(),
            stops: stops.to_bits(),
        };
        refined.insert(
            id,
            Refined {
                key,
                raster: local.refined.clone(),
            },
        );
    }
}

/// Whether the drawn picture's refined masks want their picture asked
/// of the worker: an enabled mask of `edit` refines, the drawn develop
/// is the open frame's own at its turn and came without one, and it
/// was not asked for that develop already.
pub(crate) fn picture_wanted(st: &State, edit: &Edit) -> bool {
    greycard_render::refine::refines(edit)
        && crate::panel::viewport::own_develop(st) == Some(0)
        && st.develop.drawn_guide.picture.is_none()
        && !st.develop.drawn_guide.data.is_empty()
        && st.develop.picture_asked != Some(st.develop.drawn_generation)
}

/// Frame `file` is sent to the worker to open: the learned rasters held
/// and the asks still out are another picture's, unless it is the same
/// file again. A raster is keyed by adjustment and component, and a
/// Subject's or a Background's shape is the same on every frame that
/// has one, so a mask copied from one frame to the next (same key, same
/// shape) would otherwise be drawn there with the first frame's raster.
pub(crate) fn learned_for(st: &mut State, file: &Path) {
    if st.develop.learned_of.as_deref() != Some(file) {
        st.develop.learned.clear();
        st.develop.asked.clear();
        st.develop.masks_failed.clear();
        st.develop.fill_engines.clear();
        st.develop.learned_of = Some(file.to_path_buf());
    }
}

/// Version `version` of the file `learned_for` was given is sent to the
/// worker to open: the asks still out are let go, and every raster held
/// is asked for again. The worker starts every open afresh, and a raster
/// a fill lies over is made on the retouched picture: one the worker no
/// longer knows it gave would not be made again when a fill is drawn
/// over it. The same frame again keeps its rasters drawn until the
/// answers (from the worker's disk slots, no model run) take their
/// place; another version of the file (a virtual copy, with the same
/// adjustment ids) lets them go, since they are another picture's.
pub(crate) fn learned_anew(st: &mut State, version: Option<Arc<str>>) {
    let same = st.develop.learned_version.as_ref() == Some(&version);
    if !same {
        st.develop.learned.clear();
    }
    st.develop.reasks = st.develop.learned.keys().copied().collect();
    st.develop.quiet.clear();
    st.develop.asked.clear();
    st.develop.masks_failed.clear();
    st.develop.learned_version = Some(version);
}

/// The status line while the first Whole person of a session loads the
/// Subject model for its edge.
pub(crate) fn edge_loading() -> String {
    tr!("finding the whole person; loading Subject for its edge takes a few seconds the first time")
}

/// The rasters kept drawn across a reopen that are to be asked for
/// again now: those of shapes still held and switched on. Each stays in
/// `reasks` until its ask is sent, so one switched off at the reopen,
/// or waiting on its model, is asked for when it can be.
pub(crate) fn reasks_due(st: &State) -> Vec<(Key, Shape)> {
    let mut due: Vec<_> = st
        .develop
        .reasks
        .iter()
        .filter_map(|&key| {
            let c = st
                .develop
                .edit
                .adjustments
                .iter()
                .find(|a| a.id == key.0)
                .and_then(|a| a.mask.components.get(key.1))
                .filter(|c| c.enabled)?;
            let (held, _) = st.develop.learned.get(&key)?;
            (c.shape == *held).then(|| (key, held.clone()))
        })
        .collect();
    due.sort_by_key(|(k, _)| *k);
    due
}

/// The mask of `shape` at `key` was asked for. A raster kept across a
/// reopen, asked for again as it is held, is answered quietly: its
/// answer replaces it without showing the overlay. Any other ask, the
/// user's own, is not, even for a key a quiet one was waiting on.
pub(crate) fn mask_sent(st: &mut State, key: Key, shape: &Shape) {
    let held = st
        .develop
        .learned
        .get(&key)
        .is_some_and(|(s, _)| s == shape);
    if st.develop.reasks.remove(&key) && held {
        st.develop.quiet.insert(key);
    } else {
        st.develop.quiet.remove(&key);
    }
}

/// The step for `shape` as the window would take it now: what the
/// store has, the providers, the session's declines and failed
/// fetches, and whether the sheet is free for an offer.
pub(crate) fn step_now(st: &State, shape: &Shape) -> Option<Step> {
    let store = st.develop.store.as_ref();
    // Cached: the first ask opens a `wgpu::Instance` to name the
    // adapter and every one after reads and parses providers.json,
    // neither of which belongs on the render path this runs from.
    step(
        shape,
        |m: &greycard_ai::Model| store.is_some_and(|s| s.have(m)),
        crate::ai::providers(),
        &st.sheets.declined,
        &st.sheets.fetch_failed,
        st.sheets.fetch.is_none() && !st.sheets.fetching,
        |_: &greycard_ai::Model| store.is_some_and(crate::ai::subject_original_failed_on_webgpu),
    )
}

/// The learned shapes of `edit` the window would ask for at once as
/// it opens the frame, each with the model its step names: sent with
/// the open, so those made already come with its first picture. One
/// whose step is an offer or a wait is left out, and asked for (or
/// offered) after the picture as before.
pub(crate) fn asks_at_open(st: &State, edit: &Edit) -> Vec<(Key, &'static greycard_ai::Model)> {
    let mut asks = Vec::new();
    for a in edit.adjustments.iter().take(MAX_LOCALS) {
        for (i, c) in a.mask.components.iter().enumerate() {
            if !c.enabled || !c.shape.is_learned() || !prompted(&c.shape) {
                continue;
            }
            if let Some(Step::Ask(model)) = step_now(st, &c.shape) {
                asks.push(((a.id, i), model));
            }
        }
    }
    asks
}

/// Ask the worker for the learned masks wanted and not yet asked for;
/// where the model is not in the store, offer to fetch it.
pub(crate) fn ask_for(st: &mut State, app: &App, mut wants: Vec<(Key, Shape)>) {
    for want in reasks_due(st) {
        if !wants.iter().any(|(k, _)| *k == want.0) {
            wants.push(want);
        }
    }
    for (key, shape) in wants {
        if st.develop.asked.get(&key) == Some(&shape) {
            continue;
        }
        let store = st.develop.store.clone();
        let have = |m: &greycard_ai::Model| store.as_ref().is_some_and(|s| s.have(m));
        let providers = crate::ai::providers();
        let original_failed_on_webgpu = |_: &greycard_ai::Model| {
            store
                .as_ref()
                .is_some_and(crate::ai::subject_original_failed_on_webgpu)
        };
        let sheet_free = st.sheets.fetch.is_none() && !st.sheets.fetching;
        let Some(next) = step_now(st, &shape) else {
            continue;
        };
        if let Step::Ask(model) = next {
            // The model is here and the runtime is not, but can be
            // fetched: that is offered first, and the mask is asked
            // for once it lands.
            if crate::panel::assets::runtime_first(
                st,
                app,
                crate::panel::assets::AfterRuntime::Nothing,
            ) {
                continue;
            }
            if let Shape::Part { route, .. } = &shape
                && crate::ai::parts_on_cpu()
            {
                app.set_status(crate::ai::part_on_cpu(route).into());
            }
            WORKER.with(|w| {
                if let Some(w) = &*w.borrow() {
                    w.send(Job::Mask {
                        key,
                        shape: shape.clone(),
                        model: Some(model),
                    });
                }
            });
            mask_sent(st, key, &shape);
            // A Whole person goes ahead with SAM's edge; asked for from
            // the menu just now, the Subject model its finer edge runs
            // is offered beside it, once. The first with Subject in the
            // store loads it, some seconds: said.
            if crate::ai::takes_subject_edge(&shape) {
                if !st.sheets.edge_loaded
                    && (have(&greycard_ai::SUBJECT) || have(&greycard_ai::SUBJECT_WEBGPU))
                {
                    app.set_status(edge_loading().into());
                }
                let due = std::mem::take(&mut st.sheets.edge_offer_due);
                if let Some(subject) = edge_offer(
                    &shape,
                    have,
                    providers,
                    &st.sheets.declined,
                    &st.sheets.fetch_failed,
                    due && !st.sheets.edge_declined && sheet_free,
                    original_failed_on_webgpu,
                ) {
                    crate::panel::assets::offer_edge(st, app, subject);
                }
            }
            st.develop.asked.insert(key, shape);
        } else if let Step::Offer(model, falls_back) = next {
            offer_model(st, app, model, falls_back);
        }
    }
}

/// A model's fetch failed: remember it, so a Subject turns to the
/// original, and say what happened. The status line to show.
pub(crate) fn fetch_failed(
    failed: &mut Vec<&'static str>,
    id: &'static str,
    name: &str,
    message: &str,
) -> String {
    if !failed.contains(&id) {
        failed.push(id);
    }
    if id == greycard_ai::SUBJECT_WEBGPU.id {
        tr!(
            "the GPU Subject model could not be fetched ({message}); the original is offered instead",
            message = message
        )
    } else {
        let short = name.split(',').next().unwrap_or(name);
        tr!(
            "{name} could not be fetched: {message}",
            name = short,
            message = message
        )
    }
}

/// A tool has gone in hand: the mask shows while its shape is placed.
/// The toggle as it was comes back once the tool is down, unless it
/// is thrown meanwhile, when the choice stands.
pub(crate) fn tool_in_hand(st: &mut State, app: &App) {
    if st.develop.show_mask_kept.is_none() {
        st.develop.show_mask_kept = Some(app.get_show_mask());
    }
    app.set_show_mask(true);
}

/// A panel control has changed with a mask chosen: if it was one of the
/// mask's look sliders, or its Opacity, and the overlay is showing, the
/// overlay goes off so the edit is seen. Shape and range controls leave
/// the look as it was and so leave the overlay alone.
///
/// Every look change hides it, not only the first: the toggle turned on
/// by hand is for a look, and the next drag hides it again. A tool in
/// hand has its kept toggle dropped, so the tool going down does not
/// bring the overlay back over the edit.
pub(crate) fn look_edited(st: &mut State, app: &App) {
    let now = (crate::panel::edit::read_look(app), app.get_mask_opacity());
    let before = st.develop.look_seen.borrow_mut().replace(now.clone());
    if st.develop.target.is_some() && before.is_some_and(|b| b != now) && app.get_show_mask() {
        app.set_show_mask(false);
        st.develop.show_mask_kept = None;
    }
}

/// Whether the panel's edit asks for a detail plane the shown develop
/// did not come with: a mask's Texture, Clarity, Structure, Dehaze, or
/// Sharpen moved off zero (or its section, its adjustment, or a shape
/// switched on under one) where no mask asked for that plane before,
/// or a range shape put in under a Detail section that does something,
/// whose picture from before that section the develop hands on.
/// Its amounts are the finish's and need no develop; a plane does. One no longer
/// asked for is left where it is until the next develop.
/// A plane already asked for and on its way counts as there, so a
/// slider dragged on while that develop runs asks for no second one;
/// the asking is recorded here.
pub(crate) fn detail_asks(st: &mut State, app: &App) -> bool {
    let wants = read_edit(app, &st.develop.edit, st.develop.target).detail_wants();
    let (has, asked) = (st.develop.planes_wants, st.develop.planes_asked);
    let coming = greycard_edit::DetailWants {
        texture: has.texture || asked.texture,
        clarity: has.clarity || asked.clarity,
        haze: has.haze || asked.haze,
        structure: has.structure || asked.structure,
        sharpen: has.sharpen || asked.sharpen,
        sample: has.sample || asked.sample,
    };
    if coming.covers(&wants) {
        return false;
    }
    st.develop.planes_asked = wants;
    true
}

/// How far the pointer moves, in the masks' units, before a stroke
/// takes another point: a texel of the raster.
pub(crate) const MIN_STEP: f32 = 1.0 / greycard_edit::brush::RASTER_WIDTH as f32;

/// What the status line says while a tool is in hand.
pub(crate) fn placing_hint(kind: &str) -> String {
    match kind {
        "Brush" => tr!("paint; scroll for size; Esc to finish"),
        "Object" => tr!("click to add, right-click to exclude, or drag a box; Esc to finish"),
        "Sky" => tr!("click to add sky, right-click to remove; Esc to finish"),
        "Linear" => {
            tr!("drag to place; drag an end line to turn and stretch, Alt to move just that end")
        }
        _ => tr!("drag to place"),
    }
}

/// Whether Pick on a sky takes the tool in hand, and what the status
/// line says: on a frame the gate found no sky in there is nothing to
/// correct (a click cannot add a sky the model does not see: the gate
/// is the rule that must never fail); without SAM a click that adds
/// sky does nothing, and a right click still takes what is not sky out.
pub(crate) fn sky_pick(no_sky: bool, have_sam: bool) -> Result<String, String> {
    if no_sky {
        Err(tr!("no sky found in this picture"))
    } else if !have_sam {
        Ok(tr!(
            "right-click to remove sky (adding needs the Object model); Esc to finish"
        ))
    } else {
        Ok(placing_hint("Sky"))
    }
}

/// After a shape made at once: its word on the status line, and for a
/// color range the dropper in hand, so the next click on the picture
/// centers the window. Called with the state let go of, since putting
/// a dropper in hand puts any other tool down.
fn arm_range_pick(app: &App, state: &Shared, kind: &str) {
    if kind != "Color" {
        app.set_status(made_hint(kind).into());
        return;
    }
    if app.get_picking() != "Range" {
        app.invoke_pick_started("Range".into());
    }
    crate::panel::deliver::show_hint(app, &mut state.borrow_mut(), &made_hint(kind));
}

/// The mask painted over the picture: the one whose row is under the
/// pointer, whatever Show mask says, else the chosen one while Show
/// mask is on. Its weight is the one the view already computes for
/// every mask, so a hover asks for no develop.
pub(crate) fn shown_mask(app: &App, target: Option<usize>) -> Option<usize> {
    let lit = usize::try_from(app.get_mask_lit())
        .ok()
        .filter(|_| app.get_panel_tab() == "Masks");
    lit.or(target.filter(|_| app.get_show_mask()))
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    // A mask's or a patch's row under the pointer or left: the view
    // drawn again with it lit or not.
    {
        let app_weak = app.as_weak();
        app.on_lit_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                app.window().request_redraw();
            }
        });
    }
    // The mask toggle thrown by hand while a tool is in hand: that
    // choice stands once the tool is down.
    {
        let state = state.clone();
        app.on_show_mask_toggled(move || {
            state.borrow_mut().develop.show_mask_kept = None;
        });
    }
    // Local adjustments: the panel edits one look at a time. A switch
    // of target keeps the old target's look from the panel first; a
    // new adjustment is drawn in the viewport as a shape.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_target_changed(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // A brush, an object or a sky left in hand keeps taking
            // strokes and picks into the mask it was armed for
            // (`placing.index`), not whichever this switches to: put
            // it down first, as leaving the Masks tab already does.
            if st.develop.placing.take().is_some() || st.develop.choosing.take().is_some() {
                app.set_placing("".into());
                crate::panel::deliver::drop_hint(&app, &mut st);
            }
            st.develop.part_pending = None;
            crate::panel::landscape::stopped(&mut st);
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            st.develop.edit = edit;
            let target = (i > 0).then(|| i as usize - 1);
            st.develop.target = target.filter(|&t| t < st.develop.edit.adjustments.len());
            app.set_component(0);
            show_edit(&st, &st.develop.edit, &app, st.develop.target);
            crate::panel::parts::rearm(&mut st, &app);
            app.window().request_redraw();
        });
    }
    // A tab: away from Masks the global look is what the panel edits
    // and no shape is being placed; away from Retouch no tool is in hand.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_tab_changed(move |tab| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // A develop tab reached in culling is the way out of it,
            // on to that tab.
            if st.culling.cull.is_some() {
                leave_cull(&mut st, &app, &worker, None);
                app.set_panel_tab(tab.clone());
            } else if crate::panel::cull::leave_developed(&mut st, &app) {
                app.set_panel_tab(tab.clone());
            }
            if tab != "Masks" {
                if st.develop.placing.take().is_some() || st.develop.choosing.is_some() {
                    app.set_placing("".into());
                    crate::panel::deliver::drop_hint(&app, &mut st);
                }
                crate::panel::parts::stopped(&mut st);
                if st.develop.target.is_some() {
                    let edit = read_edit(&app, &st.develop.edit, st.develop.target);
                    st.develop.edit = edit;
                    st.develop.target = None;
                    app.set_component(0);
                    show_edit(&st, &st.develop.edit, &app, None);
                }
            }
            if tab != "Retouch" && st.develop.retouching.take().is_some() {
                app.set_retouch_mode("".into());
                crate::panel::deliver::drop_hint(&app, &mut st);
            }
            // Back on Masks with a Part on this picture asking which
            // person: its mask chosen, and the picking armed.
            let asking = (tab == "Masks")
                .then(|| crate::panel::parts::asking_mask(&st))
                .flatten();
            drop(st);
            if let Some(i) = asking {
                app.set_panel_tab(tab.clone());
                app.invoke_target_changed(i as i32 + 1);
            }
            app.window().request_redraw();
        });
    }
    // A mask on its own, with no shape yet: the single way to start
    // one now that a shape is chosen afterward, from the one "Add a
    // shape" row the chosen mask always shows.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_new_mask(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if st.develop.edit.adjustments.len() >= MAX_LOCALS {
                app.set_status(
                    tr!("at most {n} adjustment" | "at most {n} adjustments" % MAX_LOCALS).into(),
                );
                return;
            }
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            st.develop.edit = edit;
            let id = st.develop.edit.next_id();
            st.develop.edit.adjustments.push(Adjustment {
                id,
                name: placeholder_name(id),
                enabled: true,
                mask: Mask {
                    components: Vec::new(),
                    invert: false,
                    ..Default::default()
                },
                look: Look::default(),
                opacity: 1.0,
            });
            st.develop.target = Some(st.develop.edit.adjustments.len() - 1);
            st.develop.fresh_mask = Some(id);
            if st.develop.placing.take().is_some() {
                app.set_placing("".into());
            }
            app.set_component(0);
            show_edit(&st, &st.develop.edit, &app, st.develop.target);
            drop(st);
            app.invoke_view_changed();
        });
    }
    // A shape added to the chosen adjustment's mask, joined by the
    // mode asked; a shape taken out of it; the chosen shape; a name.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_add_shape(move |kind, mode| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            settle_brush_row(&mut st, &app);
            let Some(index) = st.develop.target else {
                return;
            };
            if app.get_placing() == kind && app.get_placing_into() {
                st.develop.placing = None;
                app.set_placing("".into());
                crate::panel::deliver::drop_hint(&app, &mut st);
                return;
            }
            let chosen = app.get_component().max(0) as usize;
            let sky_chosen = kind == "Sky"
                && st
                    .develop
                    .edit
                    .adjustments
                    .get(index)
                    .and_then(|a| a.mask.components.get(chosen))
                    .is_some_and(|c| matches!(c.shape, Shape::Sky { .. }));
            // A mask's first shape is always Add: there is nothing
            // before it to join, take from or intersect with, and the
            // Add/Subtract/Intersect row is left over from a shape
            // added to some other mask earlier.
            let first_shape = st
                .develop
                .edit
                .adjustments
                .get(index)
                .is_some_and(|a| a.mask.components.is_empty());
            let joined = if first_shape {
                Mode::Add
            } else {
                Mode::from_name(mode.as_str()).unwrap_or_default()
            };
            if made_at_once(kind.as_str()) && !sky_chosen {
                let edit = read_edit(&app, &st.develop.edit, st.develop.target);
                st.develop.edit = edit;
                let Some(a) = st.develop.edit.adjustments.get_mut(index) else {
                    return;
                };
                a.mask.components.push(Component {
                    shape: Shape::of_kind(kind.as_str()),
                    mode: joined,
                    invert: false,
                    enabled: true,
                });
                name_first_shape(a, kind.as_str());
                let which = a.mask.components.len() - 1;
                st.develop.fresh_mask = None;
                st.develop.placing = None;
                app.set_placing("".into());
                app.set_component(which as i32);
                show_edit(&st, &st.develop.edit, &app, st.develop.target);
                drop(st);
                app.invoke_view_changed();
                arm_range_pick(&app, &state, kind.as_str());
                return;
            }
            // A brush chosen in the list takes more strokes rather
            // than a new brush beside it; an object or a sky more picks.
            let component = st
                .develop
                .edit
                .adjustments
                .get(index)
                .and_then(|a| a.mask.components.get(chosen))
                .filter(|c| {
                    (kind == "Brush" && c.shape.is_brush())
                        || (kind == "Object" && matches!(c.shape, Shape::Object { .. }))
                        || (kind == "Sky" && matches!(c.shape, Shape::Sky { .. }))
                })
                .map(|_| chosen);
            // Picks on a sky: none on a frame the gate found no sky in,
            // and only negative ones without SAM; the status line says
            // which rather than leaving clicks to do nothing.
            let mut hint = placing_hint(kind.as_str());
            if let Some(c) = component
                && kind == "Sky"
            {
                let id = st.develop.edit.adjustments.get(index).map(|a| a.id);
                let no_sky = id
                    .and_then(|id| st.develop.learned.get(&(id, c)))
                    .is_some_and(|(_, r)| r.data().iter().all(|&v| v == 0));
                let have_sam = st
                    .develop
                    .store
                    .as_ref()
                    .is_some_and(|s| s.have(&greycard_ai::SAM));
                match sky_pick(no_sky, have_sam) {
                    Ok(h) => hint = h,
                    Err(why) => {
                        app.set_status(why.into());
                        return;
                    }
                }
            }
            // A brush's strokes take from the mask by its own row, so a
            // new one asked for under Subtract is a brush joined by Add
            // with that row on Subtract: one row, one meaning. Any other
            // shape asked for, or the brush put down, puts the row back
            // (`settle_brush_row`), so the next brush asked for under
            // Add adds.
            let joined = if kind == "Brush" && component.is_none() && joined == Mode::Subtract {
                app.set_brush_op(Op::Subtract.name().into());
                st.develop.brush_row_cuts = true;
                Mode::Add
            } else {
                joined
            };
            st.develop.placing = Some(Placing {
                kind: Shape::of_kind(kind.as_str()),
                mode: joined,
                from: (0.0, 0.0),
                index,
                component,
                boxed: false,
                held: false,
            });
            tool_in_hand(&mut st, &app);
            app.set_placing_into(true);
            app.set_placing(kind.clone());
            crate::panel::deliver::show_hint(&app, &mut st, &hint);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_stop_placing(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            settle_brush_row(&mut st, &app);
            if st.develop.placing.take().is_some() || st.develop.choosing.is_some() {
                app.set_placing("".into());
                crate::panel::deliver::drop_hint(&app, &mut st);
            }
            crate::panel::parts::dismissed(&mut st);
            crate::panel::parts::stopped(&mut st);
            // A repair tool put down mid-stroke: the patch the press
            // began goes with it, never developed, and none is left
            // chosen.
            if let Some(i) = st.develop.patch_stroke.take()
                && i < st.develop.edit.retouch.patches.len()
            {
                st.develop.edit.retouch.patches.remove(i);
                app.set_patch(-1);
            }
            if st.develop.retouching.take().is_some() {
                app.set_retouch_mode("".into());
                crate::panel::deliver::drop_hint(&app, &mut st);
                crate::panel::retouch::show_patches(&st, &st.develop.edit, &app);
            }
            st.develop.picking = None;
            let was_defringe = app.get_picking() == "Defringe";
            if !app.get_picking().is_empty() {
                app.set_picking("".into());
                crate::panel::deliver::drop_hint(&app, &mut st);
            }
            // The defringe's dropper had the pass switched off so it
            // could see the fringes; putting it down puts it back.
            if was_defringe {
                drop(st);
                app.invoke_develop_changed();
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_delete_shape_at(move |c| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(i) = st.develop.target else {
                return;
            };
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            st.develop.edit = edit;
            let c = c.max(0) as usize;
            if let Some(a) = st.develop.edit.adjustments.get_mut(i)
                && c < a.mask.components.len()
            {
                a.mask.components.remove(c);
            }
            app.set_component(c.saturating_sub(1) as i32);
            show_edit(&st, &st.develop.edit, &app, st.develop.target);
            drop(st);
            app.invoke_view_changed();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_shape_toggled(move |c| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(i) = st.develop.target else {
                return;
            };
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            st.develop.edit = edit;
            if let Some(shape) = st
                .develop
                .edit
                .adjustments
                .get_mut(i)
                .and_then(|a| a.mask.components.get_mut(c.max(0) as usize))
            {
                shape.enabled = !shape.enabled;
            }
            show_edit(&st, &st.develop.edit, &app, st.develop.target);
            drop(st);
            app.invoke_view_changed();
        });
    }
    // A shape's own invert, beside it in its row rather than only in
    // the card of whichever shape happens to be chosen.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_invert_shape_toggled(move |c| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(i) = st.develop.target else {
                return;
            };
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            st.develop.edit = edit;
            if let Some(component) = st
                .develop
                .edit
                .adjustments
                .get_mut(i)
                .and_then(|a| a.mask.components.get_mut(c.max(0) as usize))
            {
                component.invert = !component.invert;
            }
            // Only the one list this touches, not the whole panel's
            // `show_edit`: that rebuilds `component-names` as a fresh
            // model every time, which tears down the shapes list's
            // rows (the row just given keyboard focus among them) even
            // though not one of their names or their count changed.
            if let Some(a) = st.develop.edit.adjustments.get(i) {
                app.set_shape_invert(component_invert(&a.mask));
            }
            drop(st);
            app.invoke_view_changed();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_adjustment_toggled(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            st.develop.edit = edit;
            if let Some(a) = st.develop.edit.adjustments.get_mut(i.max(0) as usize) {
                a.enabled = !a.enabled;
            }
            show_edit(&st, &st.develop.edit, &app, st.develop.target);
            drop(st);
            app.invoke_view_changed();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_component_changed(move |c| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // The old shape's controls go to it first.
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            st.develop.edit = edit;
            app.set_component(c);
            if let Some(a) = st
                .develop
                .target
                .and_then(|i| st.develop.edit.adjustments.get(i))
            {
                show_component(&a.mask, &app);
            }
            crate::panel::parts::rearm(&mut st, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_renamed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            show_names(&edit, &app);
            if let Some((i, a)) = st
                .develop
                .target
                .and_then(|i| Some((i, edit.adjustments.get(i)?)))
            {
                app.set_target_name(heading_name(&a.name, i).into());
            }
            drop(st);
            app.invoke_view_changed();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_delete_adjustment(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(i) = st.develop.target else {
                return;
            };
            // A brush, an object or a sky left in hand has its
            // `placing.index` shifted out from under it by the
            // remove below — put it down first rather than let a
            // later stroke or pick land in whatever mask slid into
            // its place.
            if st.develop.placing.take().is_some() || st.develop.choosing.is_some() {
                app.set_placing("".into());
                crate::panel::deliver::drop_hint(&app, &mut st);
            }
            crate::panel::parts::stopped(&mut st);
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            st.develop.edit = edit;
            if i < st.develop.edit.adjustments.len() {
                st.develop.edit.adjustments.remove(i);
            }
            st.develop.target = None;
            show_edit(&st, &st.develop.edit, &app, None);
            drop(st);
            app.invoke_view_changed();
        });
    }
    // The mask list's menu. Duplicate mask: the mask copied just below
    // itself and chosen, one step of the history on its own, the
    // panel's state before it recorded first. Its learned rasters go
    // with it, so a model is not run again for what it has.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_mask_duplicated(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if st.develop.edit.adjustments.len() >= MAX_LOCALS {
                app.set_status(
                    tr!("at most {n} adjustment" | "at most {n} adjustments" % MAX_LOCALS).into(),
                );
                return;
            }
            // As a delete: a tool in hand keeps the index it was armed
            // for, which the insert below shifts.
            if st.develop.placing.take().is_some() || st.develop.choosing.is_some() {
                app.set_placing("".into());
                crate::panel::deliver::drop_hint(&app, &mut st);
            }
            crate::panel::parts::stopped(&mut st);
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            st.develop.edit = edit.clone();
            crate::panel::edit::save_edit(&mut st, edit);
            let Some(at) = st.develop.edit.duplicate_mask(i.max(0) as usize) else {
                return;
            };
            let (from, to) = (
                st.develop.edit.adjustments[at - 1].id,
                st.develop.edit.adjustments[at].id,
            );
            let carried: Vec<_> = st
                .develop
                .learned
                .iter()
                .filter(|((id, _), _)| *id == from)
                .map(|(&(_, c), v)| ((to, c), v.clone()))
                .collect();
            st.develop.learned.extend(carried);
            st.develop.fresh_mask = None;
            st.develop.target = Some(at);
            app.set_component(0);
            show_edit(&st, &st.develop.edit, &app, st.develop.target);
            let edit = st.develop.edit.clone();
            crate::panel::edit::save_edit(&mut st, edit);
            crate::panel::history::show_history(&st, &app);
            drop(st);
            app.invoke_view_changed();
        });
    }
    // Copy mask: that mask alone onto the mask clipboard, as the panel
    // has it.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_mask_copied(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            let Some(id) = edit.adjustments.get(i.max(0) as usize).map(|a| a.id) else {
                return;
            };
            let name = st
                .current
                .map(|c| crate::panel::sync::frame_name(&st, c))
                .unwrap_or_default();
            st.sheets.mask_clipboard = crate::clipboard::MaskClip::copy(&edit, id, name);
            app.set_mask_held(st.sheets.mask_clipboard.is_some());
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_place_pressed(move |x, y, right| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // A repair: a spot here, a stroke if the pointer travels.
            if let Some(method) = st.develop.retouching {
                let at = view_to_source(&st, &app, x, y);
                let patch = Patch {
                    id: st.develop.edit.retouch.next_id(),
                    method,
                    points: vec![[at.0, at.1]],
                    radius: app.get_patch_size().clamp(0.002, 0.3),
                    feather: app.get_patch_feather().clamp(0.0, 1.0),
                    opacity: app.get_patch_opacity().clamp(0.0, 1.0),
                    source: None,
                    seed: 0,
                    remove: false,
                    fast: false,
                };
                // A Fill is made by the choice in hand where it can
                // run, the other remove or LaMa otherwise, for good.
                let mut patch = patch;
                if method == RetouchMethod::Fill {
                    patch.set_fill_choice(crate::panel::retouch::drawn_fill(&st));
                }
                st.develop.edit.retouch.patches.push(patch);
                let i = st.develop.edit.retouch.patches.len() - 1;
                st.develop.patch_stroke = Some(i);
                app.set_patch(i as i32);
                show_patches(&st, &st.develop.edit, &app);
                app.window().request_redraw();
                return;
            }
            // A person picked for a part.
            if let Some(changed) = crate::panel::parts::pressed(&mut st, &app, x, y) {
                drop(st);
                if changed {
                    app.invoke_view_changed();
                }
                return;
            }
            let Some(mut placing) = st.develop.placing.take() else {
                return;
            };
            let at = view_to_source(&st, &app, x, y);
            // The panel's look goes to its target before the new one
            // takes the panel.
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            st.develop.edit = edit;
            // Another stroke of a brush already placed, or another
            // pick for an object.
            if let Some(c) = placing.component
                && let Some(shape) = st
                    .develop
                    .edit
                    .adjustments
                    .get_mut(placing.index)
                    .and_then(|a| a.mask.components.get_mut(c))
                    .map(|c| &mut c.shape)
            {
                match shape {
                    Shape::Brush { strokes } => strokes.push(stroke_from(&app, at)),
                    Shape::Object { picks, .. } | Shape::Sky { picks } => picks.push(Pick {
                        pos: [at.0, at.1],
                        positive: !right,
                    }),
                    _ => {}
                }
                placing.from = at;
                placing.boxed = false;
                st.develop.placing = Some(placing);
                app.window().request_redraw();
                return;
            }
            let mode = placing.mode;
            let shape = match &placing.kind {
                Shape::Linear { .. } => Shape::Linear {
                    from: [at.0, at.1],
                    to: [at.0, at.1],
                },
                Shape::Radial { feather, .. } => Shape::Radial {
                    center: [at.0, at.1],
                    radius: [0.0, 0.0],
                    angle: 0.0,
                    feather: *feather,
                },
                // The brush row says what the stroke does to the mask:
                // a Subtract takes away whatever the shapes before it
                // gave, so a first stroke needs nothing done to it.
                Shape::Brush { .. } => Shape::Brush {
                    strokes: vec![stroke_from(&app, at)],
                },
                Shape::Object { .. } => Shape::Object {
                    picks: vec![Pick {
                        pos: [at.0, at.1],
                        positive: !right,
                    }],
                    boxes: Vec::new(),
                },
                s @ (Shape::Subject {}
                | Shape::Background {}
                | Shape::Sky { .. }
                | Shape::Landscape { .. }
                | Shape::Part { .. }
                | Shape::Luminance { .. }
                | Shape::Color { .. }
                | Shape::Unknown) => s.clone(),
            };
            let component = Component {
                shape,
                mode,
                invert: false,
                enabled: true,
            };
            let Some(a) = st.develop.edit.adjustments.get_mut(placing.index) else {
                app.set_placing("".into());
                return;
            };
            // Named only once it lands (`on_place_released`): a press
            // that turns out to be nothing placed must not have
            // renamed the mask on its way to being taken back.
            a.mask.components.push(component);
            let which = a.mask.components.len() - 1;
            placing.component = Some(which);
            placing.from = at;
            placing.boxed = false;
            placing.held = app.get_place_shift();
            app.set_component(which as i32);
            st.develop.placing = Some(placing);
            show_edit(&st, &st.develop.edit, &app, st.develop.target);
            app.window().request_redraw();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_place_dragged(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let at = view_to_source(&st, &app, x, y);
            if let Some(i) = st.develop.patch_stroke {
                if let Some(p) = st.develop.edit.retouch.patches.get_mut(i)
                    && p.points
                        .last()
                        .is_none_or(|l| (at.0 - l[0]).hypot(at.1 - l[1]) > MIN_STEP * 4.0)
                {
                    p.points.push([at.0, at.1]);
                }
                app.window().request_redraw();
                return;
            }
            // Shift as this move read it: a hold taken or let go in the
            // drag acts from here on.
            let shift = app.get_place_shift();
            if let Some(p) = st.develop.placing.as_mut() {
                p.held = shift;
            }
            let Some((from, index, which, held)) = st
                .develop
                .placing
                .as_ref()
                .and_then(|p| Some((p.from, p.index, p.component?, p.held)))
            else {
                return;
            };
            // The middle button held in the drag: a gradient held as
            // Shift holds it, a stroke straight from its press.
            let line = app.get_place_line();
            let held = held || line;
            // Shift's step, measured on the view.
            let stepped = if held {
                on_view_step(&st, &app, from, at)
            } else {
                at
            };
            let st = &mut *st;
            let Some(placing) = st.develop.placing.as_mut() else {
                return;
            };
            let Some(component) = st
                .develop
                .edit
                .adjustments
                .get_mut(index)
                .and_then(|a| a.mask.components.get_mut(which))
            else {
                return;
            };
            match &mut component.shape {
                Shape::Linear { to, .. } => {
                    *to = [stepped.0, stepped.1];
                }
                Shape::Radial { radius, .. } => {
                    let r = [
                        (at.0 - from.0).abs().max(MIN_RADIUS),
                        (at.1 - from.1).abs().max(MIN_RADIUS),
                    ];
                    *radius = if held { [r[0].max(r[1]); 2] } else { r };
                }
                Shape::Brush { strokes } => {
                    // Straight: the press and here, stamped between
                    // at the brush's spacing as any two points are.
                    if line {
                        if let Some(stroke) = strokes.last_mut() {
                            stroke.points.truncate(1);
                            stroke.points.push([at.0, at.1]);
                        }
                    // A point once the pointer has moved a texel.
                    } else if let Some(stroke) = strokes.last_mut()
                        && stroke
                            .points
                            .last()
                            .is_none_or(|l| (at.0 - l[0]).hypot(at.1 - l[1]) > MIN_STEP)
                    {
                        stroke.points.push([at.0, at.1]);
                    }
                }
                // A press that travels is a box, not a pick.
                Shape::Object { picks, boxes } => {
                    if placing.boxed {
                        if let Some(b) = boxes.last_mut() {
                            b[1] = [at.0, at.1];
                        }
                    } else if (at.0 - from.0).hypot(at.1 - from.1) > MIN_RADIUS {
                        picks.pop();
                        boxes.push([[from.0, from.1], [at.0, at.1]]);
                        placing.boxed = true;
                    }
                }
                Shape::Subject {}
                | Shape::Background {}
                | Shape::Sky { .. }
                | Shape::Landscape { .. }
                | Shape::Part { .. }
                | Shape::Luminance { .. }
                | Shape::Color { .. }
                | Shape::Unknown => {}
            }
            app.window().request_redraw();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_place_released(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if st.develop.patch_stroke.take().is_some() {
                // The spot or stroke just finished does not stay
                // selected: the sliders and the wheel go back to
                // setting the brush for the next repair, not resizing
                // the one just drawn. A deliberate click on it in the
                // list selects it again.
                app.set_patch(-1);
                show_patches(&st, &st.develop.edit, &app);
                WORKER.with(|w| {
                    if let Some(w) = &*w.borrow() {
                        develop_soon(&mut st, state.clone(), w.clone(), app.as_weak());
                    }
                });
                return;
            }
            let Some(mut placing) = st.develop.placing.take() else {
                return;
            };
            let Some(which) = placing.component else {
                // A release with no press to it.
                st.develop.placing = Some(placing);
                return;
            };
            // A brush stays in hand for the next stroke; a dab is a
            // stroke. An object or a sky stays in hand for the next pick.
            if placing.kind.is_brush()
                || matches!(placing.kind, Shape::Object { .. } | Shape::Sky { .. })
            {
                // A stroke, a pick or a box is never nothing placed:
                // the mask it landed in is never taken back for it,
                // and is named for it now if it is still nameless —
                // the panel's own name, the list and the heading
                // refreshed with it, or the next `read_edit` copies
                // the panel's now-stale text straight back over it.
                // Only on the stroke that actually renames it, or
                // every later stroke in the same brush would rebuild
                // the shapes list (and the focus in it) for nothing.
                st.develop.fresh_mask = None;
                let named = st
                    .develop
                    .edit
                    .adjustments
                    .get_mut(placing.index)
                    .is_some_and(|a| name_first_shape(a, placing.kind.name()));
                if named {
                    show_edit(&st, &st.develop.edit, &app, st.develop.target);
                }
                placing.boxed = false;
                st.develop.placing = Some(placing);
                drop(st);
                app.invoke_view_changed();
                return;
            }
            app.set_placing("".into());
            // A press without a drag makes nothing.
            let drawn = st
                .develop
                .edit
                .adjustments
                .get(placing.index)
                .and_then(|a| a.mask.components.get(which))
                .is_some_and(|c| match c.shape {
                    Shape::Linear { from, to } => {
                        (to[0] - from[0]).hypot(to[1] - from[1]) > MIN_RADIUS
                    }
                    Shape::Radial { radius, .. } => {
                        radius[0] > MIN_RADIUS || radius[1] > MIN_RADIUS
                    }
                    Shape::Brush { .. }
                    | Shape::Subject {}
                    | Shape::Background {}
                    | Shape::Sky { .. }
                    | Shape::Landscape { .. }
                    | Shape::Object { .. }
                    | Shape::Part { .. }
                    | Shape::Luminance { .. }
                    | Shape::Color { .. }
                    | Shape::Unknown => true,
                });
            if drawn {
                st.develop.fresh_mask = None;
                let named = st
                    .develop
                    .edit
                    .adjustments
                    .get_mut(placing.index)
                    .is_some_and(|a| name_first_shape(a, placing.kind.name()));
                if named {
                    // The panel's own name, the list and the heading:
                    // left stale, the next `read_edit` (any later
                    // action) would copy the panel's old text straight
                    // back over the name just given here.
                    show_edit(&st, &st.develop.edit, &app, st.develop.target);
                }
                app.set_status("".into());
            } else if let Some(a) = st.develop.edit.adjustments.get_mut(placing.index)
                && which < a.mask.components.len()
            {
                a.mask.components.remove(which);
                // Only the one mask "New mask" left with nothing in
                // it, taking back the shape it was about to take its
                // name and its "Add" from, goes with it — and only
                // while it is still exactly as "New mask" left it: no
                // shape landed (or once did), no look moved, its name
                // still the placeholder, its own Invert still off, and
                // its Refine edge and Opacity where they start.
                // Any one of those is deliberate work on this mask,
                // never taken back for an unrelated abandoned drag.
                let id = a.id;
                let fresh = a.mask.components.is_empty()
                    && a.look == Look::default()
                    && a.name == placeholder_name(id)
                    && !a.mask.invert
                    && a.mask.refine == 0.0
                    && a.opacity == 1.0
                    && st.develop.fresh_mask == Some(id);
                if fresh {
                    st.develop.edit.adjustments.remove(placing.index);
                    st.develop.target = None;
                    st.develop.fresh_mask = None;
                    show_edit(&st, &st.develop.edit, &app, None);
                } else {
                    app.set_component(which.saturating_sub(1) as i32);
                    show_edit(&st, &st.develop.edit, &app, st.develop.target);
                }
                app.set_status(tr!("nothing placed").into());
            }
            drop(st);
            app.invoke_view_changed();
        });
    }
    // A shape's handles: the shape as it was when pressed, moved by the
    // pointer's travel since; release records the edit.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_mask_grabbed(move |handle, held| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(shape) = st
                .develop
                .target
                .and_then(|i| st.develop.edit.adjustments.get(i))
                .and_then(|a| a.mask.components.get(app.get_component().max(0) as usize))
                .map(|c| c.shape.clone())
            else {
                return;
            };
            let handle = handle.max(0) as usize;
            let Some(&(u, v)) = shape.handles().get(handle) else {
                return;
            };
            let at = source_to_view(&st, &app, u, v);
            st.develop.mask_drag = Some(MaskDrag {
                shape,
                handle,
                at,
                held,
            });
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_mask_dragged(move |_, dx, dy, alt, shift| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // Shift as this move read it: a hold taken or let go in the
            // drag acts from here on, the press's read its start.
            if let Some(d) = st.develop.mask_drag.as_mut() {
                d.held = shift;
            }
            let Some((shape, handle, at, held)) = st
                .develop
                .mask_drag
                .as_ref()
                .map(|d| (d.shape.clone(), d.handle, d.at, d.held))
            else {
                return;
            };
            let p = view_to_source(&st, &app, at.0 + dx, at.1 + dy);
            // Plain drags pivot a linear gradient's end about its
            // center; Alt reads the same drag the old way, holding
            // the far end fixed, to pin one edge and lengthen only
            // the fade. Read from the press-time shape either way, so
            // nothing drifts or accumulates from one move to the
            // next — toggling Alt mid-drag keeps the dragged end
            // under the pointer, though the far end does jump between
            // its mirrored and its press-time spot.
            // Shift holds it as placing does.
            let moved = if held {
                shape.dragged_held(handle, p, alt, |a, q| on_view_step(&st, &app, a, q))
            } else if alt {
                shape.dragged_free(handle, p)
            } else {
                shape.dragged(handle, p)
            };
            let which = app.get_component().max(0) as usize;
            if let Some(c) = st
                .develop
                .target
                .and_then(|i| st.develop.edit.adjustments.get_mut(i))
                .and_then(|a| a.mask.components.get_mut(which))
            {
                c.shape = moved;
            }
            app.window().request_redraw();
        });
    }
    // A shape's body: a press there takes hold of the whole, as its
    // center handle does. Read in the masks' units, so the zoom, the
    // pan and the crop are all in `view_to_source`, and the body is
    // the painted shape's own, not the outline's drawing of it. Only on
    // the picture: the grey around it pans, as the mask paints nothing
    // there. Asked by the cursor on every move, so a borrow held
    // elsewhere reads as off the body rather than panicking. The third
    // argument is only the viewport's dependency on the drawn handles.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_mask_body_at(move |x, y, _| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Ok(st) = state.try_borrow() else {
                return false;
            };
            let Some(shape) = st
                .develop
                .target
                .and_then(|i| st.develop.edit.adjustments.get(i))
                .and_then(|a| a.mask.components.get(app.get_component().max(0) as usize))
                .map(|c| &c.shape)
            else {
                return false;
            };
            view_on_picture(&st, &app, x, y) && shape.on_body(view_to_source(&st, &app, x, y))
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_mask_dropped(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if state.borrow_mut().develop.mask_drag.take().is_some() {
                app.invoke_view_changed();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::Pt;
    use crate::testing::{click, count_labeled, window};
    use crate::worker::Outcome;
    use std::cell::Cell;

    use slint::Model;

    /// The learned shapes an open carries are those the window would
    /// ask for at once, each with the model its step names: a Sky whose
    /// model is in the store goes with the open, a Subject whose model
    /// would be offered does not (the offer comes after the picture),
    /// and neither does a shape switched off.
    #[test]
    fn an_open_carries_the_shapes_the_window_would_ask_for() {
        let dir = crate::testing::scratch_dir("open-asks");
        let store = greycard_ai::Store::at(&dir);
        for f in greycard_ai::SKY.files {
            let path = store.path(&greycard_ai::SKY, f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::File::create(&path)
                .unwrap()
                .set_len(f.bytes)
                .unwrap();
        }
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        let shape = |shape: Shape, enabled: bool| Component {
            shape,
            enabled,
            ..Component::default()
        };
        let mut edit = Edit::default();
        edit.adjustments.push(Adjustment {
            id: 4,
            mask: Mask {
                components: vec![
                    shape(Shape::Subject {}, true),
                    shape(Shape::Sky { picks: Vec::new() }, true),
                    shape(Shape::Sky { picks: Vec::new() }, false),
                ],
                ..Default::default()
            },
            ..Adjustment::default()
        });
        {
            let mut st = state.borrow_mut();
            st.develop.store = Some(store.clone());
            // SAM declined: the Sky is the sky model's alone, in the store.
            st.sheets.declined = vec![greycard_ai::SAM.id];
            assert!(matches!(
                step_now(&st, &Shape::Subject {}),
                Some(Step::Offer(..))
            ));
            let asks = asks_at_open(&st, &edit);
            assert_eq!(asks.len(), 1, "the Sky alone");
            assert_eq!(asks[0].0, (4, 1));
            assert_eq!(asks[0].1.id, greycard_ai::SKY.id);
        }
        crate::testing::remove_scratch(state, &dir);
    }

    /// A look slider moved with the overlay showing turns it off, every
    /// time; a shape control (feather, invert, Refine edge, the toggle
    /// itself) leaves it as it is. Refine edge reads into the mask and
    /// is shown again from it.
    #[test]
    fn a_look_change_hides_the_overlay_and_a_shape_change_does_not() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_target_changed(1);
        app.set_show_mask(true);
        app.set_mask_feather(0.3);
        app.invoke_view_changed();
        app.set_mask_invert(true);
        app.invoke_view_changed();
        app.set_mask_refine(0.4);
        app.invoke_view_changed();
        assert!(app.get_show_mask(), "a shape control leaves the overlay");
        let read = || {
            let st = state.borrow();
            read_edit(&app, &st.develop.edit, st.develop.target).adjustments[0]
                .mask
                .refine
        };
        assert_eq!(read(), 0.4);
        app.invoke_target_changed(0);
        app.invoke_target_changed(1);
        assert_eq!(app.get_mask_refine(), 0.4, "shown again from the edit");
        app.set_exposure(0.5);
        app.invoke_view_changed();
        assert!(!app.get_show_mask(), "a look slider hides it");
        // Turned on by hand, the next look change hides it again.
        app.set_show_mask(true);
        app.invoke_view_changed();
        assert!(app.get_show_mask(), "turning it on is not a look change");
        app.set_exposure(0.8);
        app.invoke_view_changed();
        assert!(!app.get_show_mask(), "every look change hides it");
        assert!(state.borrow().develop.show_mask_kept.is_none());
    }

    /// A mask's Opacity: a new mask's is whole; the slider reads into
    /// the mask and not its look, hides the overlay as a look slider
    /// does, is named in the history, and is shown again from the
    /// edit; another mask shows its own.
    #[test]
    fn a_masks_opacity_reads_into_the_mask_and_is_a_look_control() {
        use greycard_edit::history::describe;
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_new_mask();
        app.invoke_target_changed(1);
        let edit = || {
            let st = state.borrow();
            read_edit(&app, &st.develop.edit, st.develop.target)
        };
        let before = edit();
        assert_eq!(before.adjustments[0].opacity, 1.0);
        assert_eq!(app.get_mask_opacity(), 1.0);
        app.set_show_mask(true);
        app.set_mask_opacity(0.7);
        app.invoke_view_changed();
        assert!(!app.get_show_mask(), "Opacity hides the overlay");
        let after = edit();
        assert_eq!(after.adjustments[0].opacity, 0.7);
        assert_eq!(after.adjustments[0].look, before.adjustments[0].look);
        assert_eq!(after.adjustments[1].opacity, 1.0, "the other mask's own");
        let name = app.get_adjustment_name().to_string();
        assert_eq!(describe(&before, &after, &[]), format!("{name}: Opacity"));
        app.invoke_target_changed(2);
        assert_eq!(app.get_mask_opacity(), 1.0, "the second mask's");
        app.invoke_target_changed(1);
        assert_eq!(app.get_mask_opacity(), 0.7, "shown again from the edit");
    }

    /// A mask's Detail: its sliders read into the mask's look and not
    /// the picture's, a slider moved hides the overlay, its history
    /// row, a round trip through the sidecar's JSON and `show_edit`;
    /// and the develop asked for again only when the moved slider asks
    /// for a plane the shown develop did not come with.
    #[test]
    fn a_masks_detail_is_its_own_and_a_look_control() {
        use greycard_edit::history::describe;
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_target_changed(1);
        state.borrow_mut().develop.edit.adjustments[0]
            .mask
            .components
            .push(greycard_edit::mask::Component::default());
        let edit = || {
            let st = state.borrow();
            read_edit(&app, &st.develop.edit, st.develop.target)
        };
        let before = edit();
        assert_eq!(before.adjustments[0].look.detail, Default::default());
        app.set_show_mask(true);
        app.set_local_texture(-0.5);
        app.invoke_view_changed();
        assert!(!app.get_show_mask(), "a Detail slider is a look control");
        let after = edit();
        assert_eq!(after.adjustments[0].look.detail.texture, -0.5);
        assert_eq!(
            after.detail, before.detail,
            "the picture's Detail untouched"
        );
        let name = app.get_adjustment_name().to_string();
        assert_eq!(
            describe(&before, &after, &[]),
            format!("{name}: Texture -50")
        );
        // No texture plane came with the shown develop: the panel's
        // edit is the one sent to be developed.
        assert_eq!(
            state.borrow().develop.edit.adjustments[0]
                .look
                .detail
                .texture,
            -0.5
        );
        // Moved again while that develop is on its way: no second one.
        app.set_local_texture(-0.6);
        app.invoke_view_changed();
        assert_eq!(
            state.borrow().develop.edit.adjustments[0]
                .look
                .detail
                .texture,
            -0.5
        );
        // That develop fails: the plane is not coming, and the next
        // move asks again.
        let generation = state.borrow().develop.generation;
        crate::panel::deliver::deliver(
            &app,
            crate::worker::Outcome::Failed {
                generation,
                message: "failed on purpose".into(),
            },
        );
        assert_eq!(state.borrow().develop.planes_asked, Default::default());
        app.set_local_texture(-0.55);
        app.invoke_view_changed();
        assert_eq!(
            state.borrow().develop.edit.adjustments[0]
                .look
                .detail
                .texture,
            -0.55
        );
        assert!(state.borrow().develop.planes_asked.texture);
        // It lands with the plane; a slider moved is the finish's alone.
        {
            let mut st = state.borrow_mut();
            st.develop.planes_wants.texture = true;
            st.develop.planes_asked = Default::default();
        }
        app.set_local_texture(-0.8);
        app.invoke_view_changed();
        assert_eq!(
            state.borrow().develop.edit.adjustments[0]
                .look
                .detail
                .texture,
            -0.55
        );
        assert_eq!(edit().adjustments[0].look.detail.texture, -0.8);
        // Dehaze asks for the haze, which is not there.
        app.set_local_dehaze(0.3);
        app.invoke_view_changed();
        assert_eq!(
            state.borrow().develop.edit.adjustments[0]
                .look
                .detail
                .dehaze,
            0.3
        );
        // Through the sidecar's JSON and back onto the panel.
        let saved = edit();
        let back = greycard_edit::Edit::from_json(&saved.to_json()).unwrap();
        assert_eq!(back, saved);
        app.set_local_texture(0.0);
        app.set_local_dehaze(0.0);
        app.set_local_detail_enabled(false);
        {
            let st = state.borrow();
            show_edit(&st, &back, &app, st.develop.target);
        }
        assert!(app.get_local_detail_enabled());
        assert_eq!(
            (app.get_local_texture(), app.get_local_dehaze()),
            (-0.8, 0.3)
        );
        assert_eq!(
            edit().adjustments[0].look.detail,
            back.adjustments[0].look.detail
        );
        // The switch: off keeps the sliders, and is its own row.
        app.set_local_detail_enabled(false);
        app.invoke_view_changed();
        let off = edit();
        assert_eq!(describe(&back, &off, &[]), format!("{name}: Detail off"));
        assert_eq!(off.adjustments[0].look.detail.texture, -0.8);
    }

    /// A mask's Sharpen is a look control like its Texture: the
    /// overlay hidden, its history row, a develop asked for its delta
    /// and none for a move after, and through the sidecar's JSON and
    /// back onto the panel; the picture's own sharpen untouched.
    #[test]
    fn a_masks_sharpen_is_its_own_and_a_look_control() {
        use greycard_edit::history::describe;
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_target_changed(1);
        state.borrow_mut().develop.edit.adjustments[0]
            .mask
            .components
            .push(greycard_edit::mask::Component::default());
        let edit = || {
            let st = state.borrow();
            read_edit(&app, &st.develop.edit, st.develop.target)
        };
        let before = edit();
        app.set_show_mask(true);
        app.set_local_sharpen(-1.0);
        app.invoke_view_changed();
        assert!(!app.get_show_mask(), "a Detail slider is a look control");
        let after = edit();
        assert_eq!(after.adjustments[0].look.detail.sharpen, -1.0);
        assert_eq!(
            after.sharpen, before.sharpen,
            "the picture's sharpen untouched"
        );
        assert_eq!(after.detail.sharpen, 0.0);
        let name = app.get_adjustment_name().to_string();
        assert_eq!(
            describe(&before, &after, &[]),
            format!("{name}: Sharpen -100")
        );
        // The delta is asked for: the panel's edit went to be developed.
        let sent = || {
            state.borrow().develop.edit.adjustments[0]
                .look
                .detail
                .sharpen
        };
        assert_eq!(sent(), -1.0);
        assert!(state.borrow().develop.planes_asked.sharpen);
        // It lands with the delta; a move after is the finish's alone.
        {
            let mut st = state.borrow_mut();
            st.develop.planes_wants.sharpen = true;
            st.develop.planes_asked = Default::default();
        }
        app.set_local_sharpen(0.4);
        app.invoke_view_changed();
        assert_eq!(sent(), -1.0);
        let saved = edit();
        assert_eq!(saved.adjustments[0].look.detail.sharpen, 0.4);
        let back = greycard_edit::Edit::from_json(&saved.to_json()).unwrap();
        assert_eq!(back, saved);
        app.set_local_sharpen(0.0);
        {
            let st = state.borrow();
            show_edit(&st, &back, &app, st.develop.target);
        }
        assert_eq!(app.get_local_sharpen(), 0.4);
        assert_eq!(
            edit().adjustments[0].look.detail,
            back.adjustments[0].look.detail
        );
    }

    /// A look change while a tool is in hand hides the overlay and
    /// drops the toggle kept for the tool going down, so it does not
    /// come back over the edit.
    #[test]
    fn a_look_change_with_a_tool_in_hand_leaves_nothing_to_bring_it_back() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_target_changed(1);
        app.set_show_mask(true);
        app.invoke_add_shape("Brush".into(), "Add".into());
        assert!(state.borrow().develop.placing.is_some());
        assert_eq!(state.borrow().develop.show_mask_kept, Some(true));
        app.set_shadows(0.4);
        app.invoke_view_changed();
        assert!(!app.get_show_mask());
        assert_eq!(state.borrow().develop.show_mask_kept, None);
    }

    /// A Subject the worker makes again for a patch drawn over it
    /// replaces the one held, unasked, and does not turn the overlay
    /// on; one asked for does.
    #[test]
    fn a_subject_made_again_for_a_patch_replaces_the_one_held() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_add_shape("Subject".into(), "Add".into());
        let id = state.borrow().develop.edit.adjustments[0].id;
        let raster = |v: u8| Arc::new(Raster::from_data(1.5, 16, vec![v; 16 * 11]));
        let made = |v: u8| Outcome::Mask {
            key: (id, 0),
            shape: Shape::Subject {},
            file: None,
            raster: raster(v),
            provider: Some("CPU"),
            seconds: 0.1,
            note: None,
            part: None,
        };
        state
            .borrow_mut()
            .develop
            .asked
            .insert((id, 0), Shape::Subject {});
        app.set_show_mask(false);
        crate::panel::deliver::deliver(&app, made(200));
        assert!(app.get_show_mask(), "asked for: shown");
        app.set_show_mask(false);
        crate::panel::deliver::deliver(&app, made(9));
        assert!(!app.get_show_mask(), "made again: not shown");
        // Asked again after a reopen: not shown either.
        {
            let mut st = state.borrow_mut();
            st.develop.asked.insert((id, 0), Shape::Subject {});
            st.develop.quiet.insert((id, 0));
        }
        crate::panel::deliver::deliver(&app, made(9));
        assert!(!app.get_show_mask(), "asked again: not shown");
        assert!(state.borrow().develop.quiet.is_empty());
        let st = state.borrow();
        let (shape, held) = &st.develop.learned[&(id, 0)];
        assert_eq!(*shape, Shape::Subject {});
        assert!(held.data().iter().all(|&v| v == 9));
    }

    /// Every open asks for the rasters held again, so the worker, which
    /// starts the open afresh, knows it gave them and a fill drawn after
    /// is noticed. The same frame again keeps them drawn meanwhile;
    /// another copy of the same file lets them go.
    #[test]
    fn every_open_asks_for_the_learned_rasters_again() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        let mut st = state.borrow_mut();
        let st = &mut *st;
        let mut edit = Edit::default();
        edit.adjustments.push(greycard_edit::Adjustment {
            id: 1,
            name: "Subject 1".into(),
            enabled: true,
            mask: greycard_edit::mask::Mask {
                components: vec![greycard_edit::mask::Component {
                    shape: Shape::Subject {},
                    ..Default::default()
                }],
                invert: false,
                ..Default::default()
            },
            look: Default::default(),
            opacity: 1.0,
        });
        st.develop.edit = edit.clone();
        let held = |st: &mut State| {
            st.develop.learned.insert(
                (1, 0),
                (
                    Shape::Subject {},
                    Arc::new(Raster::from_data(1.5, 16, vec![9; 16 * 11])),
                ),
            );
            st.develop.asked.insert((2, 0), Shape::Subject {});
        };
        let open = |st: &mut State, version: Option<&str>| {
            st.cells[0].version = version.map(Arc::from);
            let _ = crate::panel::browser::open_job(st, 0, edit.clone());
            (st.develop.learned.len(), st.develop.asked.len())
        };
        open(st, Some("a"));
        held(st);
        assert_eq!(
            open(st, Some("a")),
            (1, 0),
            "the same copy again: still drawn"
        );
        let (locals, wants) = bake_locals(
            &edit,
            &mut st.develop.rasters,
            &mut st.develop.learned,
            1.5,
            None,
        );
        assert!(
            locals[0].rasters[0].is_some(),
            "drawn while it is asked for"
        );
        assert!(wants.is_empty());
        assert_eq!(
            reasks_due(st),
            vec![((1, 0), Shape::Subject {})],
            "asked again"
        );
        // Switched off at the reopen: not asked for yet, and not let go.
        st.develop.edit.adjustments[0].mask.components[0].enabled = false;
        assert!(reasks_due(st).is_empty());
        assert!(st.develop.reasks.contains(&(1, 0)));
        // On again: asked for then.
        st.develop.edit.adjustments[0].mask.components[0].enabled = true;
        assert_eq!(reasks_due(st), vec![((1, 0), Shape::Subject {})]);
        // Sent as held: quiet. The user's own ask after, of a new shape
        // for the key, is not; nor one sent while it was still due.
        mask_sent(st, (1, 0), &Shape::Subject {});
        assert!(st.develop.reasks.is_empty());
        assert!(st.develop.quiet.contains(&(1, 0)));
        let sky = Shape::Sky { picks: Vec::new() };
        mask_sent(st, (1, 0), &sky);
        assert!(st.develop.quiet.is_empty(), "the user's ask after");
        st.develop.reasks.insert((1, 0));
        mask_sent(st, (1, 0), &sky);
        assert!(st.develop.quiet.is_empty(), "the user's ask while due");
        assert!(st.develop.reasks.is_empty());
        held(st);
        assert_eq!(open(st, Some("b")), (0, 0), "the other copy");
        assert!(st.develop.reasks.is_empty());
        let (_, wants) = bake_locals(
            &edit,
            &mut st.develop.rasters,
            &mut st.develop.learned,
            1.5,
            None,
        );
        assert_eq!(wants, vec![((1, 0), Shape::Subject {})]);
    }

    /// A refined mask is made once and kept while nothing it is made
    /// from moves, its brush painted a stroke at a time as the window
    /// paints it: not again on the next frame, made again for a new
    /// stroke, a new Refine edge, an exposure, or a new picture, and let
    /// go with its Refine edge.
    #[test]
    fn a_refined_mask_is_made_again_only_when_its_making_moves() {
        let image = greycard_core::image::WorkingImage {
            width: 60,
            height: 40,
            data: (0..60 * 40 * 3)
                .map(|k| (k / 3 % 60) as f32 / 60.0)
                .collect(),
        };
        let picture = Arc::new(greycard_render::refine::mask_picture(&image));
        let mut stroke =
            greycard_edit::brush::Stroke::new(greycard_edit::brush::Op::Add, 0.1, 0.5, 1.0);
        stroke.points = vec![[0.3, 0.3], [0.5, 0.35]];
        let mut edit = Edit::default();
        edit.adjustments.push(Adjustment {
            id: 7,
            mask: Mask {
                components: vec![greycard_edit::mask::Component {
                    shape: Shape::Brush {
                        strokes: vec![stroke],
                    },
                    ..Default::default()
                }],
                refine: 0.5,
                ..Default::default()
            },
            ..Default::default()
        });
        let aspect = 40.0 / 60.0;
        let mut rasters = HashMap::new();
        let mut learned = HashMap::new();
        let mut cache = HashMap::new();
        let mut made = |edit: &Edit, picture: &Arc<MaskPicture>, exposure: f32| {
            let (mut locals, _) = bake_locals(edit, &mut rasters, &mut learned, aspect, None);
            refine_locals(
                edit,
                &mut locals,
                &mut cache,
                Some(picture),
                aspect,
                exposure,
                exposure + 0.8,
            );
            locals[0].refined.clone()
        };
        let first = made(&edit, &picture, 0.0).expect("refined");
        let again = made(&edit, &picture, 0.0).expect("refined");
        assert!(Arc::ptr_eq(&first.0, &again.0), "kept the next frame");
        let brighter = made(&edit, &picture, 1.0).expect("refined");
        assert!(!Arc::ptr_eq(&first.0, &brighter.0), "an exposure");
        let Shape::Brush { strokes } = &mut edit.adjustments[0].mask.components[0].shape else {
            unreachable!()
        };
        strokes[0].points.push([0.6, 0.5]);
        let stroked = made(&edit, &picture, 1.0).expect("refined");
        assert!(!Arc::ptr_eq(&brighter.0, &stroked.0), "a stroke");
        let kept = made(&edit, &picture, 1.0).expect("refined");
        assert!(Arc::ptr_eq(&stroked.0, &kept.0), "kept after the stroke");
        edit.adjustments[0].mask.refine = 0.7;
        let moved = made(&edit, &picture, 1.0).expect("refined");
        assert!(!Arc::ptr_eq(&kept.0, &moved.0), "a new Refine edge");
        let other = Arc::new(greycard_render::refine::mask_picture(&image));
        let anew = made(&edit, &other, 1.0).expect("refined");
        assert!(!Arc::ptr_eq(&moved.0, &anew.0), "a new picture");
        edit.adjustments[0].mask.refine = 0.0;
        assert!(made(&edit, &other, 1.0).is_none());
        assert!(cache.is_empty());
    }

    /// A brush baked again with its strokes as they were, while the
    /// last frame's locals are still held (the renderer's key, the
    /// scopes'), is the same mask: not copied, so no key misses and an
    /// idle window settles. A new point, or a new aspect, is a new one.
    #[test]
    fn a_brush_baked_again_unchanged_is_the_same_mask() {
        let mut stroke =
            greycard_edit::brush::Stroke::new(greycard_edit::brush::Op::Add, 0.1, 0.5, 1.0);
        stroke.points = vec![[0.3, 0.3], [0.5, 0.35]];
        let mut edit = Edit::default();
        edit.adjustments.push(Adjustment {
            id: 7,
            mask: Mask {
                components: vec![greycard_edit::mask::Component {
                    shape: Shape::Brush {
                        strokes: vec![stroke],
                    },
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        });
        let mut rasters = HashMap::new();
        let mut learned = HashMap::new();
        let mut bake = |edit: &Edit, aspect: f32| {
            bake_locals(edit, &mut rasters, &mut learned, aspect, None).0
        };
        let held = bake(&edit, 2.0 / 3.0);
        let again = bake(&edit, 2.0 / 3.0);
        assert_eq!(held, again, "the same mask the next frame");
        let (a, b) = (
            held[0].rasters[0].as_ref().unwrap(),
            again[0].rasters[0].as_ref().unwrap(),
        );
        assert!(Arc::ptr_eq(&a.0, &b.0), "not copied");
        // A stroke begun with no points yet leaves the raster as it was
        // (the mask beside it is another, holding the stroke).
        let Shape::Brush { strokes } = &mut edit.adjustments[0].mask.components[0].shape else {
            unreachable!()
        };
        strokes.push(greycard_edit::brush::Stroke::default());
        assert_eq!(
            bake(&edit, 2.0 / 3.0)[0].rasters,
            held[0].rasters,
            "an empty stroke"
        );
        let Shape::Brush { strokes } = &mut edit.adjustments[0].mask.components[0].shape else {
            unreachable!()
        };
        strokes[1].points.push([0.6, 0.5]);
        let stroked = bake(&edit, 2.0 / 3.0);
        assert_ne!(stroked, held, "a stroke");
        assert_eq!(bake(&edit, 2.0 / 3.0), stroked, "kept after the stroke");
        assert_ne!(bake(&edit, 0.75), stroked, "a new aspect");
    }

    /// The window's frame with a brush on the open frame, as the
    /// rendering notifier runs it: bake, draw, analyze. Once the first
    /// analysis lands, a frame with nothing changed draws nothing and
    /// asks no scope read back, so nothing asks for the next frame.
    /// Checked where there is a GPU.
    #[test]
    fn an_idle_frame_with_a_brush_settles() {
        use crate::render::{Renderer, View};
        use greycard_gpu::wgpu::PollType;
        use greycard_viewport::scope::Scope;
        let Some((device, queue)) = greycard_viewport::testing::device("idle brush") else {
            return;
        };
        let mut stroke =
            greycard_edit::brush::Stroke::new(greycard_edit::brush::Op::Add, 0.1, 0.5, 1.0);
        stroke.points = vec![[0.3, 0.3], [0.5, 0.35]];
        let mut edit = Edit::default();
        edit.adjustments.push(Adjustment {
            id: 3,
            mask: Mask {
                components: vec![greycard_edit::mask::Component {
                    shape: Shape::Brush {
                        strokes: vec![stroke],
                    },
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        });
        let n = 64usize;
        let image = greycard_core::image::WorkingImage {
            width: n,
            height: n,
            data: vec![0.2; n * n * 3],
        };
        let mut renderer = Renderer::new(&device, &queue);
        renderer.upload(&greycard_render::halves::Halves::from_image(&image, None));
        let (mut rasters, mut learned) = (HashMap::new(), HashMap::new());
        // One frame: whether it drew, and whether a read back is out.
        let mut frame = |renderer: &mut Renderer| {
            let mut view = View::blank();
            view.center = (n as f32 / 2.0, n as f32 / 2.0);
            view.plane = (n as f32, n as f32);
            view.frame_size = (n as f32, n as f32);
            view.locals = bake_locals(&edit, &mut rasters, &mut learned, 1.0, None).0;
            let (_, drew) = renderer.render(n as u32, n as u32, &view);
            let (_, in_flight) = renderer.analyze(&view, Scope::Rgb, None);
            (drew, in_flight)
        };
        let mut settled = false;
        for _ in 0..100 {
            if !frame(&mut renderer).1 {
                settled = true;
                break;
            }
            let _ = device.poll(PollType::wait_indefinitely());
        }
        assert!(settled, "the first analysis lands");
        for i in 0..5 {
            assert_eq!(frame(&mut renderer), (false, false), "idle frame {i}");
        }
    }

    /// Picking another mask keeps today's rule: the look on show is
    /// that mask's own, and nothing hides the overlay.
    #[test]
    fn choosing_another_mask_leaves_the_overlay_as_it_was() {
        let app = window(1);
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_new_mask();
        app.invoke_target_changed(1);
        app.set_exposure(0.5);
        app.invoke_view_changed();
        app.set_show_mask(true);
        app.invoke_target_changed(2);
        app.invoke_view_changed();
        assert!(app.get_show_mask());
    }

    /// A mask picked with the panel's read-back unlike the stored look
    /// (a local white in f64, on or kept while off, and an empty curve
    /// from a foreign sidecar) does not hide at the first toggle; the
    /// white balance's own switch, a look control, does.
    #[test]
    fn a_stored_look_the_panel_reads_back_differently_does_not_hide() {
        use greycard_edit::white::LocalWhite;
        let (temperature, tint) = (5123.4567, 0.0031);
        // On, and switched off with the light kept.
        for white in [
            LocalWhite::Absolute { temperature, tint },
            LocalWhite::Kept { temperature, tint },
        ] {
            let app = window(1);
            let (state, _worker) = crate::testing::state_for(&app, Vec::new());
            app.set_panel_tab("Masks".into());
            app.invoke_new_mask();
            state.borrow_mut().develop.edit.adjustments[0]
                .look
                .white_balance = white;
            state.borrow_mut().develop.edit.adjustments[0]
                .look
                .curves
                .red = Vec::new();
            app.invoke_target_changed(1);
            app.set_show_mask(true);
            app.invoke_view_changed();
            assert!(app.get_show_mask(), "{white:?}");
            // The white balance's switch is a look control: it hides it.
            app.set_local_wb_on(!app.get_local_wb_on());
            app.invoke_local_wb_toggled();
            assert!(!app.get_show_mask(), "{white:?}");
        }
    }

    /// An undo or a history click that changes only the look rewrites
    /// the panel; showing the overlay after it is not a look edit.
    #[test]
    fn a_restored_look_is_not_taken_for_an_edit() {
        let settle = || {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
                crate::panel::edit::SAVE_MS + 100,
            ));
            slint::platform::update_timers_and_animations();
        };
        for by_history in [false, true] {
            let app = window(1);
            let (_state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
            app.invoke_select(0);
            app.set_panel_tab("Masks".into());
            app.invoke_new_mask();
            app.invoke_target_changed(1);
            settle();
            app.set_show_mask(true);
            app.set_exposure(0.5);
            app.invoke_view_changed();
            settle();
            assert!(!app.get_show_mask(), "the look slider hides it");
            if by_history {
                app.invoke_history_clicked(1);
            } else {
                app.invoke_undo();
            }
            assert_eq!(app.get_exposure(), 0.0, "the look was restored");
            app.set_show_mask(true);
            app.invoke_view_changed();
            assert!(app.get_show_mask(), "showing it after a restore stays");
            app.set_mask_feather(0.2);
            app.invoke_view_changed();
            assert!(app.get_show_mask());
        }
    }

    /// The left bar: the navigator, the snapshots and the history,
    /// and the width the viewport begins at.
    const LEFT_BAR: f32 = 240.0;

    /// A fetch that failed is remembered, and the status line turns a
    /// Subject to the original (the choice itself is the engine's,
    /// `ai::step`).
    #[test]
    fn a_failed_fetch_is_remembered_and_turns_a_subject_to_the_original() {
        use greycard_ai::SUBJECT_WEBGPU;
        let mut failed = Vec::new();
        let status = fetch_failed(&mut failed, SUBJECT_WEBGPU.id, SUBJECT_WEBGPU.name, "404");
        assert!(
            status.contains("GPU Subject model could not be fetched"),
            "{status}"
        );
        assert!(status.contains("original"), "{status}");
        assert_eq!(failed, [SUBJECT_WEBGPU.id]);
    }

    /// The sheet's note says who publishes the model.
    #[test]
    fn the_offer_says_who_publishes_the_model() {
        let ours = crate::panel::assets::model_note(&greycard_ai::SUBJECT_WEBGPU);
        assert!(ours.contains("greycard's faster copy"), "{ours}");
        assert!(ours.contains("MIT"), "{ours}");
        let theirs = crate::panel::assets::model_note(&greycard_ai::SUBJECT);
        assert!(theirs.contains("Its own license"), "{theirs}");
    }

    /// The overlay is drawn in view pixels off a shape that knows
    /// nothing of the window: a handle lands wherever the shape does,
    /// which under a zoom or a pan is off the picture altogether, at
    /// a negative x. The viewport clips, so a handle that far out is
    /// neither drawn nor pressable over the left bar, which owns its
    /// own clicks.
    #[test]
    fn a_handle_off_the_picture_does_not_take_the_left_bars_clicks() {
        let app = window(1);
        app.set_mask_kind("Linear".into());
        let grabbed = Rc::new(Cell::new(false));
        {
            let grabbed = grabbed.clone();
            app.on_mask_grabbed(move |_, _| grabbed.set(true));
        }
        // A handle at these view pixels sits at `LEFT_BAR + x` of the
        // window, so a negative x is over the left bar.
        let at = |x: f32| {
            app.set_mask_handles(ModelRc::new(VecModel::from(vec![Pt { x, y: 400.0 }])));
            grabbed.set(false);
            click(&app, LEFT_BAR + x, 400.0);
            grabbed.get()
        };
        assert!(!at(-180.0), "a handle over the left bar took a click");
        // The same handle on the picture is pressable, so the test is
        // about the clip and not about the wiring.
        assert!(at(360.0), "a handle on the picture is not pressable");
    }

    /// A handle's press tells `mask-grabbed` whether Shift was held,
    /// from the pointer event itself.
    #[test]
    fn a_handles_press_reports_shift() {
        use slint::platform::{Key, WindowEvent};
        let app = window(1);
        app.set_mask_kind("Linear".into());
        app.set_mask_handles(ModelRc::new(VecModel::from(vec![Pt {
            x: 360.0,
            y: 400.0,
        }])));
        let held = Rc::new(Cell::new(None));
        {
            let held = held.clone();
            app.on_mask_grabbed(move |_, shift| held.set(Some(shift)));
        }
        let shift = |down: bool| {
            let text = Key::Shift.into();
            app.window().dispatch_event(if down {
                WindowEvent::KeyPressed { text }
            } else {
                WindowEvent::KeyReleased { text }
            });
        };
        shift(true);
        click(&app, LEFT_BAR + 360.0, 400.0);
        shift(false);
        assert_eq!(held.get(), Some(true), "Shift held at the press");
        click(&app, LEFT_BAR + 360.0, 400.0);
        assert_eq!(held.get(), Some(false), "a plain press");
    }

    /// Sky from its button is made whole, like Subject; on a chosen sky
    /// the same button is Pick, which puts the tool in hand and takes
    /// clicks as picks on that sky, a right click a negative one,
    /// rather than adding a second sky; and the button again puts the
    /// tool down.
    #[test]
    fn a_sky_comes_from_its_button_and_takes_picks() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_add_shape("Sky".into(), "Add".into());
        {
            let st = state.borrow();
            assert_eq!(st.develop.target, Some(0));
            assert_eq!(
                st.develop.edit.adjustments[0].mask.components,
                vec![Component {
                    shape: Shape::Sky { picks: Vec::new() },
                    mode: Mode::Add,
                    invert: false,
                    enabled: true,
                }]
            );
            assert!(st.develop.placing.is_none());
        }
        assert_eq!(app.get_component_kind(), "Sky");
        assert_eq!(app.get_status(), "finding the sky");

        app.invoke_add_shape("Sky".into(), "Add".into());
        assert_eq!(app.get_placing(), "Sky");
        assert!(app.get_placing_into());
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_released();
        app.invoke_place_pressed(320.0, 260.0, true);
        app.invoke_place_released();
        {
            let st = state.borrow();
            let components = &st.develop.edit.adjustments[0].mask.components;
            assert_eq!(components.len(), 1, "a pick is not a second sky");
            let Shape::Sky { picks } = &components[0].shape else {
                panic!("{:?}", components[0].shape)
            };
            assert_eq!(
                picks.iter().map(|p| p.positive).collect::<Vec<_>>(),
                vec![true, false]
            );
            assert!(st.develop.placing.is_some(), "the tool stays in hand");
        }
        app.invoke_add_shape("Sky".into(), "Add".into());
        assert!(app.get_placing().is_empty());
        assert!(state.borrow().develop.placing.is_none());

        // The test's state has no SAM: Pick says a click that adds sky
        // needs it.
        app.invoke_add_shape("Sky".into(), "Add".into());
        assert!(app.get_status().contains("needs the Object model"));
        app.invoke_add_shape("Sky".into(), "Add".into());
        // The gate found no sky: an empty raster made for it. Pick does
        // not take the tool, and says why.
        {
            let mut st = state.borrow_mut();
            let id = st.develop.edit.adjustments[0].id;
            let shape = st.develop.edit.adjustments[0].mask.components[0]
                .shape
                .clone();
            let empty = Arc::new(Raster::from_data(1.5, 16, vec![0u8; 16 * 11]));
            st.develop.learned.insert((id, 0), (shape, empty));
        }
        app.invoke_add_shape("Sky".into(), "Add".into());
        assert!(app.get_placing().is_empty());
        assert!(state.borrow().develop.placing.is_none());
        assert_eq!(app.get_status(), "no sky found in this picture");
    }

    #[test]
    fn pick_on_a_sky_says_what_it_can_do() {
        assert!(sky_pick(true, true).is_err());
        assert!(sky_pick(true, false).is_err());
        assert!(
            sky_pick(false, false)
                .unwrap()
                .contains("needs the Object model")
        );
        assert_eq!(sky_pick(false, true), Ok(placing_hint("Sky")));
    }

    /// The look sections on the Masks tab are headed with the chosen
    /// adjustment's name, as it is renamed; Global's are not.
    #[test]
    fn the_look_sections_are_headed_with_the_chosen_adjustment() {
        let app = window(1);
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_add_shape("Luminance".into(), "Add".into());
        app.invoke_new_mask();
        app.invoke_add_shape("Color".into(), "Add".into());
        let second = app.get_target_name();
        assert!(!second.is_empty());
        app.invoke_target_changed(1);
        let first = app.get_target_name();
        assert_eq!(first, app.get_adjustment_name());
        assert_ne!(first, second);
        app.set_adjustment_name("Warm the far hillside".into());
        app.invoke_renamed();
        assert_eq!(app.get_target_name(), "Warm the far hillside");
        // A name, so shown as typed, never set in the titles' capitals.
        let typed = crate::testing::count_labeled(&app, "Warm the far hillside");
        assert!(typed > 0);
        assert_eq!(
            crate::testing::count_labeled(&app, "WARM THE FAR HILLSIDE"),
            0
        );
        // Never a bare title: a name blanked in the field is not
        // taken, and a blank one from a sidecar is its place.
        app.set_adjustment_name("   ".into());
        app.invoke_renamed();
        assert!(!app.get_target_name().trim().is_empty());
        assert_eq!(heading_name(" ", 1), "Adjustment 2");
        assert_eq!(heading_name(" Sky 3 ", 1), "Sky 3");
        app.invoke_target_changed(0);
        assert_eq!(app.get_target_name(), "");
    }

    /// The chosen shape's card is numbered in row order only when its
    /// kind repeats in the mask.
    #[test]
    fn the_shape_card_is_numbered_when_its_kind_repeats() {
        let one = |shape| Component {
            shape,
            mode: Mode::Add,
            invert: false,
            enabled: true,
        };
        let mask = Mask {
            components: vec![
                one(Shape::of_kind("Radial")),
                one(Shape::LUMINANCE),
                one(Shape::of_kind("Radial")),
            ],
            invert: false,
            ..Default::default()
        };
        assert_eq!(component_heading(&mask, 0), "Radial 1");
        assert_eq!(component_heading(&mask, 1), "Luminance");
        assert_eq!(component_heading(&mask, 2), "Radial 2");
        assert_eq!(component_heading(&mask, 3), "");
    }

    /// The range masks from the panel: a button makes one whole, its
    /// sliders are its window both ways, a color one puts the dropper
    /// in hand, and Skin puts the window at the skin hue.
    #[test]
    fn the_range_masks_come_from_their_buttons_and_live_in_their_sliders() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_add_shape("Luminance".into(), "Add".into());
        {
            let st = state.borrow();
            assert_eq!(st.develop.target, Some(0));
            assert_eq!(
                st.develop.edit.adjustments[0].mask.components[0].shape,
                Shape::LUMINANCE
            );
        }
        assert_eq!(app.get_component_kind(), "Luminance");
        assert!(app.get_picking().is_empty(), "no dropper for a luminance");
        assert_eq!(app.get_lum_low(), 0.7);
        // The sliders are the shape.
        app.set_lum_low(0.5);
        app.set_lum_high_feather(0.2);
        let read = |app: &App| {
            let st = state.borrow();
            read_edit(app, &st.develop.edit, st.develop.target)
        };
        assert_eq!(
            read(&app).adjustments[0].mask.components[0].shape,
            Shape::Luminance {
                low: 0.5,
                high: 1.0,
                low_feather: 0.1,
                high_feather: 0.2,
            }
        );
        // A color range intersected with it: at the skin, with the
        // dropper in hand to move it.
        app.set_lum_low(0.5);
        app.invoke_add_shape("Color".into(), "Intersect".into());
        assert_eq!(app.get_picking(), "Range");
        assert_eq!(app.get_component(), 1);
        assert_eq!(app.get_component_kind(), "Color");
        let edit = read(&app);
        let mask = &edit.adjustments[0].mask;
        assert_eq!(mask.components[1].shape, Shape::skin());
        assert_eq!(mask.components[1].mode, Mode::Intersect);
        // The luminance kept what its sliders said when it was left.
        let Shape::Luminance { low, .. } = mask.components[0].shape else {
            panic!("{:?}", mask.components[0].shape)
        };
        assert_eq!(low, 0.5);
        // The hue slider moves it; its default (the skin hue) is the
        // way back.
        app.set_range_hue(200.0);
        let Shape::Color { hue, .. } = read(&app).adjustments[0].mask.components[1].shape else {
            panic!()
        };
        assert_eq!(hue, 200.0);
        app.set_range_hue(greycard_edit::mask::SKIN_HUE);
        assert_eq!(
            read(&app).adjustments[0].mask.components[1].shape,
            Shape::skin()
        );
        // A fade longer than the floor is read as the floor: it cannot
        // run under no chroma.
        app.set_range_chroma(0.02);
        app.set_range_chroma_feather(0.05);
        let Shape::Color {
            chroma,
            chroma_feather,
            ..
        } = read(&app).adjustments[0].mask.components[1].shape
        else {
            panic!()
        };
        assert_eq!((chroma, chroma_feather), (0.02, 0.02));
        // Back to the luminance: its own numbers on the sliders.
        app.invoke_component_changed(0);
        assert_eq!(app.get_component_kind(), "Luminance");
        assert_eq!(app.get_lum_low(), 0.5);
        assert_eq!(app.get_lum_high_feather(), 0.2);
        // High under Low is read as High at Low.
        app.set_lum_low(0.8);
        app.set_lum_high(0.2);
        let Shape::Luminance { low, high, .. } = read(&app).adjustments[0].mask.components[0].shape
        else {
            panic!()
        };
        assert_eq!((low, high), (0.8, 0.8));
    }

    /// "New mask" starts a mask with no shape; a made-at-once shape
    /// (as `add_shape` used to add straight from `add_adjustment`)
    /// goes into it exactly the same way. The button reading the
    /// layout is its own test below.
    #[test]
    fn a_new_mask_starts_empty_and_takes_its_first_shape_from_add_a_shape() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        {
            let st = state.borrow();
            assert_eq!(st.develop.target, Some(0));
            assert!(st.develop.edit.adjustments[0].mask.components.is_empty());
            // Named while empty, never a blank row: `show_names`
            // reads `Adjustment.name` raw, with no fallback of its
            // own for an empty one.
            assert_eq!(st.develop.edit.adjustments[0].name, "Mask 1");
        }
        assert_eq!(
            app.get_adjustment_names()
                .iter()
                .collect::<Vec<slint::SharedString>>(),
            vec![
                slint::SharedString::from("Global"),
                slint::SharedString::from("Mask 1"),
            ]
        );
        assert_eq!(app.get_component_kind(), "");
        app.invoke_add_shape("Sky".into(), "Add".into());
        assert_eq!(app.get_component_kind(), "Sky");
        let st = state.borrow();
        assert_eq!(
            st.develop.edit.adjustments[0].mask.components,
            vec![Component {
                shape: Shape::Sky { picks: Vec::new() },
                mode: Mode::Add,
                invert: false,
                enabled: true,
            }]
        );
        // Named for its first shape now, as a made-at-once shape from
        // the old `add_adjustment` always was ("Sky 1", not "Mask 1").
        assert_eq!(st.develop.edit.adjustments[0].name, "Sky 1");
    }

    /// A shape's mode is always Add when it is the mask's first, even
    /// with Subtract or Intersect left over on the row from a shape
    /// added to some other mask: there is nothing before it to join,
    /// take from or intersect with, and a first shape read as
    /// anything but Add evaluates to nothing everywhere the mask is
    /// not inverted, silently doing nothing.
    #[test]
    fn a_masks_first_shape_is_always_add_whatever_the_row_says() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_add_shape("Luminance".into(), "Subtract".into());
        {
            let st = state.borrow();
            assert_eq!(
                st.develop.edit.adjustments[0].mask.components[0].mode,
                Mode::Add
            );
            assert_eq!(
                st.develop.edit.adjustments[0].mask.components[0]
                    .mode
                    .sign(),
                "+"
            );
        }
        // Left on Subtract for the next mask's first shape too.
        app.invoke_new_mask();
        app.invoke_add_shape("Subject".into(), "Subtract".into());
        {
            let st = state.borrow();
            assert_eq!(
                st.develop.edit.adjustments[1].mask.components[0].mode,
                Mode::Add
            );
            assert_eq!(
                st.develop.edit.adjustments[1]
                    .mask
                    .at_with(0.5, 0.5, |_, _, _| 1.0),
                1.0
            );
        }
        // A dragged (not made-at-once) first shape is Add as well.
        app.invoke_new_mask();
        app.invoke_add_shape("Linear".into(), "Subtract".into());
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_dragged(600.0, 500.0);
        app.invoke_place_released();
        let st = state.borrow();
        assert_eq!(
            st.develop.edit.adjustments.len(),
            3,
            "the drag was far enough to be drawn, not abandoned"
        );
        assert_eq!(
            st.develop.edit.adjustments[2].mask.components[0].mode,
            Mode::Add
        );
    }

    /// A placed shape (not one made whole) abandoned without a drag,
    /// the would-have-been first shape of a mask "New mask" just made
    /// and nothing has touched since, takes the mask with it: the
    /// same nothing a fresh adjustment left behind before "New mask"
    /// gave a mask no shape yet a target of its own.
    #[test]
    fn an_abandoned_first_shape_in_a_new_mask_leaves_no_empty_mask_behind() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_add_shape("Linear".into(), "Add".into());
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_released();
        let st = state.borrow();
        assert!(
            st.develop.edit.adjustments.is_empty(),
            "an empty mask, untouched since New mask, should not linger: {:?}",
            st.develop.edit.adjustments
        );
        assert_eq!(st.develop.target, None);
    }

    /// The same abandoned first shape, but the mask's look was
    /// touched first (an Exposure moved): the mask stays, empty look
    /// and all, since a look's worth of work is never silently
    /// dropped along with a shape that never landed.
    #[test]
    fn a_masks_look_touched_before_an_abandoned_first_shape_keeps_the_mask() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.set_exposure(1.5);
        app.invoke_add_shape("Linear".into(), "Add".into());
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_released();
        let st = state.borrow();
        assert_eq!(
            st.develop.edit.adjustments.len(),
            1,
            "the mask stays: its look was touched"
        );
        assert!(st.develop.edit.adjustments[0].mask.components.is_empty());
        assert_eq!(st.develop.edit.adjustments[0].look.light.exposure, 1.5);
    }

    /// The same abandoned first shape, the mask's Opacity or its Refine
    /// edge moved first: the mask stays, with what was set on it.
    #[test]
    fn a_masks_opacity_or_refine_set_before_an_abandoned_first_shape_keeps_the_mask() {
        for refine in [false, true] {
            let app = window(1);
            let (state, _worker) = crate::testing::state_for(&app, Vec::new());
            app.set_panel_tab("Masks".into());
            app.invoke_new_mask();
            if refine {
                app.set_mask_refine(0.4);
            } else {
                app.set_mask_opacity(0.6);
            }
            app.invoke_add_shape("Linear".into(), "Add".into());
            app.invoke_place_pressed(300.0, 200.0, false);
            app.invoke_place_released();
            let st = state.borrow();
            assert_eq!(
                st.develop.edit.adjustments.len(),
                1,
                "the mask stays: its {} was set",
                if refine { "Refine edge" } else { "Opacity" }
            );
            let a = &st.develop.edit.adjustments[0];
            assert!(a.mask.components.is_empty());
            if refine {
                assert_eq!(a.mask.refine, 0.4);
            } else {
                assert_eq!(a.opacity, 0.6);
            }
        }
    }

    /// A mask that once had a shape, emptied again by deleting it
    /// (not by an abandoned drag), is never taken by a later abandoned
    /// shape: `fresh_mask` cleared the moment the first shape landed,
    /// whatever became of it since.
    #[test]
    fn a_mask_once_emptied_by_deleting_its_shape_is_never_taken_later() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_add_shape("Luminance".into(), "Add".into());
        app.invoke_delete_shape_at(0);
        {
            let st = state.borrow();
            assert_eq!(st.develop.edit.adjustments.len(), 1);
            assert!(st.develop.edit.adjustments[0].mask.components.is_empty());
        }
        app.invoke_add_shape("Linear".into(), "Add".into());
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_released();
        let st = state.borrow();
        assert_eq!(
            st.develop.edit.adjustments.len(),
            1,
            "a mask that once had a shape is never taken, however it is emptied since"
        );
        assert!(st.develop.edit.adjustments[0].mask.components.is_empty());
    }

    /// A shape is named for its kind only once it lands, at the
    /// release that finds it drawn — never at the press that merely
    /// arms a placing. An abandoned drag (a press with no movement)
    /// must not rename a mask it never gave a shape to: `fresh_mask`
    /// only tracks the one mask "New mask" most recently left with
    /// nothing in it, so a second "New mask" leaves the first one
    /// kept regardless of its own look, name or Invert — and it must
    /// come out of this with the name "New mask" gave it, not the
    /// kind of a shape that never landed.
    #[test]
    fn an_abandoned_first_shape_does_not_rename_the_mask() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_new_mask();
        app.invoke_target_changed(1);
        app.invoke_add_shape("Linear".into(), "Add".into());
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_released();
        let st = state.borrow();
        assert_eq!(
            st.develop.edit.adjustments.len(),
            2,
            "not the tracked fresh mask (the second one is): kept"
        );
        assert_eq!(
            st.develop.edit.adjustments[0].name, "Mask 1",
            "never renamed for a shape that did not land"
        );
        assert!(st.develop.edit.adjustments[0].mask.components.is_empty());
    }

    /// The same abandoned drag, but the mask's look was touched first,
    /// so it is kept even though it is the tracked fresh mask: it must
    /// still come out nameless, not wrongly renamed for the shape that
    /// never landed, so a later made-at-once shape can still name it.
    #[test]
    fn a_kept_but_still_nameless_mask_is_named_by_its_next_real_shape() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.set_exposure(1.5);
        app.invoke_add_shape("Linear".into(), "Add".into());
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_released();
        {
            let st = state.borrow();
            assert_eq!(
                st.develop.edit.adjustments.len(),
                1,
                "kept: its look was touched"
            );
            assert_eq!(
                st.develop.edit.adjustments[0].name, "Mask 1",
                "not renamed for the shape that never landed"
            );
            assert!(st.develop.edit.adjustments[0].mask.components.is_empty());
        }
        app.invoke_add_shape("Subject".into(), "Add".into());
        let st = state.borrow();
        assert_eq!(st.develop.edit.adjustments[0].name, "Subject 1");
    }

    /// The rename a landed, drawn shape gives its mask has to reach
    /// the panel's own copy of the name too — the LineEdit, the list,
    /// the heading — or a later `read_edit` (any panel action that
    /// flushes it, an invert toggle among them) copies the panel's
    /// still-stale "Mask N" straight back over it.
    #[test]
    fn a_drawn_shapes_name_survives_a_later_read_edit() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_add_shape("Linear".into(), "Add".into());
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_dragged(600.0, 500.0);
        app.invoke_place_released();
        {
            let st = state.borrow();
            assert_eq!(st.develop.edit.adjustments[0].name, "Linear 1");
        }
        app.invoke_invert_shape_toggled(0);
        let st = state.borrow();
        assert_eq!(
            st.develop.edit.adjustments[0].name, "Linear 1",
            "a later read_edit must not copy the panel's stale name back over it"
        );
    }

    /// The same, for a brush's first stroke (the "stays in hand"
    /// release path, not the "drawn" one a Linear or a Radial takes).
    #[test]
    fn a_brush_strokes_name_survives_a_later_read_edit() {
        use crate::testing::{labeled, press};
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_add_shape("Brush".into(), "Add".into());
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_released();
        {
            let st = state.borrow();
            assert_eq!(st.develop.edit.adjustments[0].name, "Brush 1");
        }
        app.invoke_invert_shape_toggled(0);
        assert_eq!(state.borrow().develop.edit.adjustments[0].name, "Brush 1");

        // Named once, on the stroke that gave it its name: a later
        // stroke on the same brush does not rename it again, so it
        // must not rebuild the shapes list for nothing either. Click
        // the row's own invert to focus it, then take a second
        // stroke; if the list were rebuilt for it, the row (and the
        // focus on it) would be a fresh one, and Space would do
        // nothing.
        let (at, size) = labeled(&app, "Invert this shape");
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        let inverted_by_click =
            state.borrow().develop.edit.adjustments[0].mask.components[0].invert;
        app.invoke_place_pressed(320.0, 220.0, false);
        app.invoke_place_released();
        press(&app, " ");
        assert_ne!(
            state.borrow().develop.edit.adjustments[0].mask.components[0].invert,
            inverted_by_click,
            "focus survived the second stroke: Space still reaches the same row"
        );
    }

    /// A mask "New mask" left renamed by hand, or with its own Invert
    /// switched on, is never taken back for an abandoned first shape:
    /// "exactly as New mask left it" means the placeholder name and
    /// Invert off too, not only no shapes and a default look.
    #[test]
    fn a_renamed_or_inverted_fresh_mask_is_never_taken_by_an_abandoned_shape() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.set_adjustment_name("Portrait".into());
        app.invoke_renamed();
        app.invoke_add_shape("Linear".into(), "Add".into());
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_released();
        {
            let st = state.borrow();
            assert_eq!(
                st.develop.edit.adjustments.len(),
                1,
                "renamed by hand: kept"
            );
            assert_eq!(st.develop.edit.adjustments[0].name, "Portrait");
        }

        // A second mask, its own Invert switched on instead of a rename.
        app.invoke_new_mask();
        app.set_mask_invert(true);
        app.invoke_add_shape("Linear".into(), "Add".into());
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_released();
        let st = state.borrow();
        assert_eq!(
            st.develop.edit.adjustments.len(),
            2,
            "Invert switched on: kept"
        );
        assert!(st.develop.edit.adjustments[1].mask.invert);
    }

    /// A shape's own invert is a switch on its row, not a control only
    /// the chosen shape's card shows; toggling it does not need the
    /// row to be chosen first.
    #[test]
    fn a_shapes_invert_is_its_own_row_and_does_not_need_it_chosen() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_add_shape("Luminance".into(), "Add".into());
        app.invoke_add_shape("Color".into(), "Add".into());
        assert_eq!(app.get_component(), 1, "the color just added is chosen");
        // The first row's invert, though the second is chosen.
        app.invoke_invert_shape_toggled(0);
        {
            let st = state.borrow();
            let components = &st.develop.edit.adjustments[0].mask.components;
            assert!(components[0].invert, "the row toggled, not the chosen one");
            assert!(!components[1].invert);
        }
        assert_eq!(
            app.get_shape_invert().iter().collect::<Vec<_>>(),
            vec![true, false]
        );
        app.invoke_invert_shape_toggled(0);
        assert!(!state.borrow().develop.edit.adjustments[0].mask.components[0].invert);
    }

    /// A shape's own invert is reachable from the keyboard, not only
    /// a click: it takes focus as any control does, and Space or
    /// Enter toggles it from there.
    #[test]
    fn a_shapes_invert_is_reachable_from_the_keyboard() {
        use crate::testing::{labeled, press};
        use slint::platform::Key;
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_add_shape("Luminance".into(), "Add".into());
        let (at, size) = labeled(&app, "Invert this shape");
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert!(
            state.borrow().develop.edit.adjustments[0].mask.components[0].invert,
            "the click both focuses it and toggles it"
        );
        press(&app, " ");
        assert!(
            !state.borrow().develop.edit.adjustments[0].mask.components[0].invert,
            "Space toggles the focused row back"
        );
        press(&app, Key::Return);
        assert!(
            state.borrow().develop.edit.adjustments[0].mask.components[0].invert,
            "Enter toggles it again"
        );
    }

    /// A brush, an object or a sky left in hand has its
    /// `placing.index` invalidated by a delete: deleting the very
    /// mask being placed into shifts whatever came after it down to
    /// fill the gap, so a stroke taken after the delete would
    /// otherwise land in, and rename, an unrelated mask that happened
    /// to slide into the deleted one's slot.
    #[test]
    fn deleting_a_mask_with_a_tool_in_hand_puts_the_tool_down() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_new_mask();
        app.invoke_target_changed(1); // choose the first mask ("Mask 1")
        app.invoke_add_shape("Brush".into(), "Add".into());
        app.invoke_place_pressed(300.0, 200.0, false);
        app.invoke_place_released();
        assert!(
            state.borrow().develop.placing.is_some(),
            "the brush stays in hand"
        );
        app.invoke_delete_adjustment();
        assert!(
            state.borrow().develop.placing.is_none(),
            "the tool is put down, not left armed for whatever slid into the deleted mask's slot"
        );
        // The remaining mask (the second "New mask") is untouched.
        let st = state.borrow();
        assert_eq!(st.develop.edit.adjustments.len(), 1);
        assert_eq!(st.develop.edit.adjustments[0].name, "Mask 2");
        assert!(st.develop.edit.adjustments[0].mask.components.is_empty());
    }

    /// Duplicate mask: a copy just below, chosen, its shapes and look
    /// the source's under a new mask's name, its learned rasters with
    /// it; one step, which one undo takes back.
    #[test]
    fn duplicate_mask_lands_below_as_one_step() {
        use greycard_edit::brush::Raster;
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        // Global on the panel while the mask is set up under it.
        app.invoke_target_changed(0);
        {
            let mut st = state.borrow_mut();
            let a = &mut st.develop.edit.adjustments[0];
            a.mask.components.push(Component {
                shape: Shape::Subject {},
                ..Default::default()
            });
            a.name = "Subject 1".into();
            a.look.light.exposure = 0.4;
            let raster = Arc::new(Raster::from_data(1.5, 4, vec![9; 24]));
            st.develop
                .learned
                .insert((1, 0), (Shape::Subject {}, raster));
        }
        app.invoke_target_changed(1);
        app.invoke_new_mask();
        app.invoke_mask_duplicated(0);
        let names = |state: &Rc<Shared>| -> Vec<String> {
            let st = state.borrow();
            st.develop
                .edit
                .adjustments
                .iter()
                .map(|a| a.name.clone())
                .collect()
        };
        assert_eq!(names(&state), ["Subject 1", "Subject 3", "Mask 2"]);
        {
            let st = state.borrow();
            let (a, b) = (
                &st.develop.edit.adjustments[0],
                &st.develop.edit.adjustments[1],
            );
            assert_eq!((&b.mask, &b.look), (&a.mask, &a.look));
            assert_eq!(b.look.light.exposure, 0.4);
            assert_eq!(st.develop.target, Some(1));
            assert!(
                st.develop.learned.contains_key(&(3, 0)),
                "its raster carried"
            );
            assert_eq!(
                st.sidecars[0].current.adjustments.len(),
                3,
                "recorded at once"
            );
        }
        assert_eq!(app.get_target(), 2);
        assert_eq!(app.get_adjustment_name(), "Subject 3");
        let top = app.get_history_rows().row_data(0).unwrap().name;
        assert_eq!(top, "Subject 3 added");
        app.invoke_undo();
        assert_eq!(names(&state), ["Subject 1", "Mask 2"]);
        // Past the end, nothing.
        app.invoke_mask_duplicated(7);
        assert_eq!(names(&state), ["Subject 1", "Mask 2"]);
    }

    /// With no mask chosen, "New mask" is there and nothing offers a
    /// shape yet: there is no mask to add one to.
    /// New mask puts the sections back at their top: the new mask's
    /// name and shapes are in the first section, and the button is in
    /// reach from any scroll since the list is pinned.
    #[test]
    fn new_mask_brings_the_sections_back_to_their_top() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.set_panel_scroll(-900.0);
        let (at, size) = crate::testing::labeled(&app, "New mask");
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert_eq!(state.borrow().develop.edit.adjustments.len(), 2);
        assert_eq!(app.get_panel_scroll(), 0.0, "the sections are at their top");
    }

    /// New mask hands the new mask's name the keys, its placeholder
    /// selected: a name typed at once replaces it, and none of its
    /// letters reaches the window's keys. Enter gives them back. A
    /// second mask, made with the section already up, does the same.
    #[test]
    fn a_new_masks_name_takes_the_keys() {
        use crate::testing::{labeled, press, settle, type_text};
        use slint::platform::Key;
        let app = window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.set_collapsed_mask(true);
        for (mask, name) in [(1, "Sky"), (2, "Face")] {
            let (at, size) = labeled(&app, "New mask");
            click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
            settle(&app);
            assert_eq!(app.get_target(), mask);
            assert!(!app.get_collapsed_mask(), "the name's section opens");
            type_text(&app, name);
            assert_eq!(app.get_adjustment_name(), name);
            assert_eq!(
                app.get_adjustment_names().row_data(mask as usize).unwrap(),
                name
            );
            assert!(!app.get_proofing(), "S went to the name, not to the proof");
            press(&app, Key::Return);
            settle(&app);
        }
        press(&app, "g");
        assert!(app.get_grid_open(), "Enter gave the keys back");
    }

    #[test]
    fn new_mask_is_there_with_nothing_chosen_and_no_shape_button_yet() {
        let app = window(1);
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        assert_eq!(count_labeled(&app, "New mask"), 1);
        assert_eq!(count_labeled(&app, "Subject"), 0);
        assert_eq!(count_labeled(&app, "Background"), 0);
    }

    /// The Masks tab's own read of the roadmap line this redesign is
    /// for: the shape buttons (Background beside Subject) sit once,
    /// under the chosen mask's "Add a shape", and "New mask" is the
    /// only other way to start one — never the two ways to reach the
    /// same button that used to double Subject, Sky and the rest.
    #[test]
    fn the_shape_buttons_and_new_mask_each_appear_once() {
        use i_slint_backend_testing::ElementHandle;
        let app = window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 1000.0));
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        // Queried only after the mask exists: a query on this window
        // before its "if" turns true leaves every later one, even of
        // a control already there, finding nothing (a quirk of the
        // testing backend's element cache, not of the layout).
        app.invoke_new_mask();
        assert_eq!(count_labeled(&app, "New mask"), 1, "New mask once");
        settle_layout(&app);
        let bounds = |id: &str| {
            let e = ElementHandle::find_by_element_id(&app, id)
                .next()
                .unwrap_or_else(|| panic!("{id}"));
            (e.absolute_position().y, e.size().height)
        };
        // The panel scrolled as a wheel would, until the MASK
        // section's foot, where the shape buttons are, is in view:
        // the buttons are counted whatever the section's height.
        let (view_top, view_height) = bounds("App::panel-view");
        let (section_top, section_height) = bounds("App::mask-section");
        let over = section_top + section_height - (view_top + view_height);
        if over > 0.0 {
            app.set_panel_scroll(app.get_panel_scroll() - over);
            settle_layout(&app);
        }
        // Counted over the whole window, by what they are: panel
        // buttons carrying the word, so a second way to the same shape
        // anywhere (the list, a menu) is caught, and the mixer's
        // Luminance slider is not one.
        let buttons = |label: &str| {
            let label = slint::SharedString::from(label);
            ElementHandle::find_by_element_type_name(&app, "PanelButton")
                .filter(|b| {
                    let label = label.clone();
                    b.query_descendants()
                        .match_predicate(move |e| e.accessible_label().as_ref() == Some(&label))
                        .find_first()
                        .is_some()
                })
                .count()
        };
        for label in [
            "Linear",
            "Radial",
            "Brush",
            "Subject",
            "Background",
            "Sky",
            "Object",
            "Luminance",
            "Color",
        ] {
            assert_eq!(buttons(label), 1, "{label} should appear once");
        }
    }

    /// The Add/Subtract/Intersect row only makes sense once the mask
    /// already has a shape to join, take from or intersect with — a
    /// first shape is always Add, whatever it says. A mask "New mask"
    /// just made, with none yet, does not show it; one with a shape
    /// does.
    #[test]
    fn the_mode_row_is_hidden_until_the_mask_has_a_shape() {
        let empty = window(1);
        // The tall desktop window the test was written at.
        empty
            .window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = crate::testing::state_for(&empty, Vec::new());
        empty.set_panel_tab("Masks".into());
        empty.invoke_new_mask();
        assert_eq!(count_labeled(&empty, "Next shape mode"), 0);

        let with_shape = window(1);
        // The tall desktop window the test was written at.
        with_shape
            .window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = crate::testing::state_for(&with_shape, Vec::new());
        with_shape.set_panel_tab("Masks".into());
        with_shape.invoke_new_mask();
        with_shape.invoke_add_shape("Subject".into(), "Add".into());
        assert_eq!(count_labeled(&with_shape, "Next shape mode"), 1);
    }

    /// A window with one adjustment whose only shape is `shape`, chosen
    /// as the render step would leave it, over a 400 by 300 source at
    /// zoom one; and the viewport's top left in the window, so a test
    /// can press where `source_to_view` puts a point of the shape.
    fn body_setup(
        shape: Shape,
    ) -> (
        App,
        Rc<Shared>,
        Rc<crate::worker::Worker>,
        slint::LogicalPosition,
    ) {
        let app = window(1);
        let (state, worker) = crate::testing::state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.develop.source_size = (400, 300);
            st.develop.image_size = (400, 300);
            st.develop.zoom = 1.0;
            st.develop.edit.adjustments.push(Adjustment {
                id: 1,
                mask: Mask {
                    components: vec![Component {
                        shape: shape.clone(),
                        ..Default::default()
                    }],
                    invert: false,
                    ..Default::default()
                },
                ..Default::default()
            });
            st.develop.target = Some(0);
        }
        app.set_panel_tab("Masks".into());
        app.set_component(0);
        app.set_mask_kind(shape.name().into());
        let base =
            i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "Viewport")
                .next()
                .expect("the viewport is on screen")
                .absolute_position();
        (app, state, worker, base)
    }

    /// Where in the window a point of the masks' units is shown.
    fn shown_at(
        app: &App,
        state: &Rc<Shared>,
        base: slint::LogicalPosition,
        u: f32,
        v: f32,
    ) -> (f32, f32) {
        let (x, y) = source_to_view(&state.borrow(), app, u, v);
        (base.x + x, base.y + y)
    }

    fn the_shape(state: &Rc<Shared>) -> Shape {
        state.borrow().develop.edit.adjustments[0].mask.components[0]
            .shape
            .clone()
    }

    fn a_radial(rx: f32, ry: f32) -> Shape {
        Shape::Radial {
            center: [0.5, 0.375],
            radius: [rx, ry],
            angle: 0.0,
            feather: 0.5,
        }
    }

    /// The tester's report: a drag anywhere inside a radial's ellipse,
    /// not only on its center handle, moves the radial; the picture
    /// stays where it was.
    #[test]
    fn a_drag_inside_a_radial_moves_it_rather_than_the_picture() {
        let (app, state, _worker, base) = body_setup(a_radial(0.2, 0.1));
        // Halfway out along the long axis, well clear of every handle.
        let press = shown_at(&app, &state, base, 0.6, 0.375);
        let was_center = state.borrow().develop.center;
        let (center_x, center_y) = shown_at(&app, &state, base, 0.5, 0.375);
        crate::testing::drag(
            &app,
            &[
                press,
                (press.0 + 10.0, press.1),
                (press.0 + 40.0, press.1 + 20.0),
            ],
        );
        let want = view_to_source(
            &state.borrow(),
            &app,
            center_x - base.x + 40.0,
            center_y - base.y + 20.0,
        );
        let Shape::Radial { center, radius, .. } = the_shape(&state) else {
            panic!("still a radial");
        };
        assert!(
            (center[0] - want.0).abs() < 1e-4 && (center[1] - want.1).abs() < 1e-4,
            "{center:?} vs {want:?}"
        );
        assert!(center[0] > 0.55, "{center:?} did not move");
        assert_eq!(radius, [0.2, 0.1], "a move is not a resize");
        assert!(
            !app.get_mask_body_moving(),
            "the move cursor outlived the release"
        );
        assert_eq!(
            state.borrow().develop.center,
            was_center,
            "the picture panned"
        );
        assert!(
            state.borrow().develop.mask_drag.is_none(),
            "the drag was let go"
        );
    }

    /// A linear's body is the band between its lines, the whole width
    /// of the picture: a drag there far from its handles moves both.
    #[test]
    fn a_drag_on_a_linears_band_moves_the_gradient() {
        let linear = Shape::Linear {
            from: [0.5, 0.3],
            to: [0.5, 0.45],
        };
        let (app, state, _worker, base) = body_setup(linear);
        let press = shown_at(&app, &state, base, 0.15, 0.375);
        let was_center = state.borrow().develop.center;
        crate::testing::drag(
            &app,
            &[press, (press.0, press.1 + 10.0), (press.0, press.1 + 40.0)],
        );
        let Shape::Linear { from, to } = the_shape(&state) else {
            panic!("still a linear");
        };
        // Forty view pixels at zoom one on a 400 wide source.
        let dv = 40.0 * app.window().scale_factor() / 400.0;
        assert!((from[0] - 0.5).abs() < 1e-4 && (to[0] - 0.5).abs() < 1e-4);
        assert!((from[1] - (0.3 + dv)).abs() < 1e-4, "{from:?}");
        assert!((to[1] - (0.45 + dv)).abs() < 1e-4, "{to:?}");
        assert!(!app.get_mask_body_moving(), "the move outlived the release");
        assert_eq!(
            state.borrow().develop.center,
            was_center,
            "the picture panned"
        );
    }

    /// The brush's ring, and the repair tool's after it, shows under
    /// the pointer with Shift held at rest, where Shift and a bracket
    /// set the feather it draws, and goes only while a drag under
    /// Shift pans the picture.
    #[test]
    fn a_tools_ring_stays_under_shift_until_a_drag_pans() {
        use slint::platform::{Key, PointerEventButton, WindowEvent};
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        {
            let mut st = state.borrow_mut();
            st.develop.source_size = (3000, 2000);
            st.develop.center = (1500.0, 1000.0);
        }
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        rest();
        app.invoke_add_shape("Brush".into(), "Add".into());
        assert_eq!(app.get_placing(), "Brush");
        let rings = || {
            i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "Ring").count()
        };
        let base =
            i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "Viewport")
                .next()
                .expect("the viewport is on screen")
                .absolute_position();
        let at = |x: f32| slint::LogicalPosition::new(base.x + x, base.y + 300.0);
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: at(400.0),
        });
        assert!(rings() > 0, "the ring under the pointer");
        let shift = |down: bool| {
            let text = Key::Shift.into();
            app.window().dispatch_event(if down {
                WindowEvent::KeyPressed { text }
            } else {
                WindowEvent::KeyReleased { text }
            });
        };
        shift(true);
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: at(410.0),
        });
        assert!(rings() > 0, "Shift at rest keeps the ring");
        let was = state.borrow().develop.center;
        app.window().dispatch_event(WindowEvent::PointerPressed {
            position: at(410.0),
            button: PointerEventButton::Left,
        });
        for x in [430.0, 460.0] {
            app.window()
                .dispatch_event(WindowEvent::PointerMoved { position: at(x) });
        }
        assert_ne!(state.borrow().develop.center, was, "Shift's drag pans");
        assert_eq!(rings(), 0, "a drag that pans takes it away");
        app.window().dispatch_event(WindowEvent::PointerReleased {
            position: at(460.0),
            button: PointerEventButton::Left,
        });
        assert!(rings() > 0, "back once the pan ends");
        shift(false);
        assert!(rings() > 0);

        // The repair tool's ring likewise.
        crate::testing::press(&app, slint::platform::Key::Escape);
        app.invoke_retouch_toggled("Heal".into());
        assert_eq!(app.get_retouch_mode(), "Heal");
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: at(400.0),
        });
        assert!(rings() > 0, "the patch's ring");
        shift(true);
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: at(410.0),
        });
        assert!(rings() > 0, "Shift at rest keeps it");
        app.window().dispatch_event(WindowEvent::PointerPressed {
            position: at(410.0),
            button: PointerEventButton::Left,
        });
        for x in [430.0, 460.0] {
            app.window()
                .dispatch_event(WindowEvent::PointerMoved { position: at(x) });
        }
        assert_eq!(rings(), 0, "a drag that pans takes it away");
        app.window().dispatch_event(WindowEvent::PointerReleased {
            position: at(460.0),
            button: PointerEventButton::Left,
        });
        shift(false);
        assert!(rings() > 0, "back once the pan ends");
    }

    /// Shift taken mid-drag holds a radial to a circle from the next
    /// move, and let go mid-drag lets it go, on a placed radial's
    /// handle and on a radial being placed alike.
    #[test]
    fn shift_taken_or_let_go_mid_drag_acts_on_the_next_move() {
        use slint::platform::{Key, PointerEventButton, WindowEvent};
        let shift = |app: &App, down: bool| {
            let text = Key::Shift.into();
            app.window().dispatch_event(if down {
                WindowEvent::KeyPressed { text }
            } else {
                WindowEvent::KeyReleased { text }
            });
        };
        let to = |app: &App, p: (f32, f32)| {
            let position = slint::LogicalPosition::new(p.0, p.1);
            app.window()
                .dispatch_event(WindowEvent::PointerMoved { position });
        };
        let button = |app: &App, p: (f32, f32), down: bool| {
            let position = slint::LogicalPosition::new(p.0, p.1);
            let button = PointerEventButton::Left;
            app.window().dispatch_event(if down {
                WindowEvent::PointerPressed { position, button }
            } else {
                WindowEvent::PointerReleased { position, button }
            });
        };
        let radii = |s: &Shape| {
            let Shape::Radial { radius, .. } = *s else {
                panic!("{s:?}")
            };
            radius
        };

        // A placed radial's long axis end.
        let radial = a_radial(0.2, 0.1);
        let (app, state, _worker, base) = body_setup(radial.clone());
        let pts: Vec<Pt> = radial
            .handles()
            .into_iter()
            .map(|(u, v)| {
                let (x, y) = source_to_view(&state.borrow(), &app, u, v);
                Pt { x, y }
            })
            .collect();
        app.set_mask_handles(ModelRc::new(VecModel::from(pts)));
        let (u, v) = radial.handles()[1];
        let at = shown_at(&app, &state, base, u, v);
        to(&app, at);
        button(&app, at, true);
        to(&app, (at.0 + 10.0, at.1));
        let r = radii(&the_shape(&state));
        assert!(r[0] > 0.2 && r[1] == 0.1, "free so far: {r:?}");
        shift(&app, true);
        to(&app, (at.0 + 20.0, at.1));
        let r = radii(&the_shape(&state));
        assert_eq!(r[0], r[1], "Shift taken: a circle: {r:?}");
        shift(&app, false);
        to(&app, (at.0 + 25.0, at.1));
        let r = radii(&the_shape(&state));
        assert!(r[0] > 0.2 && r[1] == 0.1, "Shift let go: {r:?}");
        button(&app, (at.0 + 25.0, at.1), false);

        // A radial being placed.
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        {
            let mut st = state.borrow_mut();
            st.develop.source_size = (3000, 2000);
            st.develop.center = (1500.0, 1000.0);
        }
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        rest();
        app.invoke_add_shape("Radial".into(), "Add".into());
        assert_eq!(app.get_placing(), "Radial");
        let base =
            i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "Viewport")
                .next()
                .expect("the viewport is on screen")
                .absolute_position();
        let at = (base.x + 400.0, base.y + 300.0);
        let placed = || {
            let st = state.borrow();
            radii(&st.develop.edit.adjustments[0].mask.components[0].shape)
        };
        to(&app, at);
        button(&app, at, true);
        to(&app, (at.0 + 60.0, at.1 + 20.0));
        let r = placed();
        assert!(r[0] != r[1], "free so far: {r:?}");
        shift(&app, true);
        to(&app, (at.0 + 70.0, at.1 + 25.0));
        let r = placed();
        assert_eq!(r[0], r[1], "Shift taken: a circle: {r:?}");
        shift(&app, false);
        to(&app, (at.0 + 80.0, at.1 + 30.0));
        let r = placed();
        assert!(r[0] != r[1], "Shift let go: {r:?}");
        button(&app, (at.0 + 80.0, at.1 + 30.0), false);
    }

    /// The report's shape: a shape placed free, then Shift held at the
    /// press on a handle and through a real drag of it. A radial's
    /// axis end makes it a circle; a linear's end puts it on a step of
    /// its angle. Without Shift the same drags hold nothing.
    #[test]
    fn shift_on_a_placed_shapes_handle_holds_it() {
        use slint::platform::{Key, WindowEvent};
        // A drag of handle `h` of `shape` by (dx, dy) view pixels, Shift
        // held from before the press when `shift`, and the shape after.
        let run = |shape: Shape, h: usize, (dx, dy): (f32, f32), shift: bool| {
            let (app, state, _worker, base) = body_setup(shape.clone());
            let pts: Vec<Pt> = shape
                .handles()
                .into_iter()
                .map(|(u, v)| {
                    let (x, y) = source_to_view(&state.borrow(), &app, u, v);
                    Pt { x, y }
                })
                .collect();
            app.set_mask_handles(ModelRc::new(VecModel::from(pts)));
            let (u, v) = shape.handles()[h];
            let at = shown_at(&app, &state, base, u, v);
            let key = |down: bool| {
                let text = Key::Shift.into();
                app.window().dispatch_event(if down {
                    WindowEvent::KeyPressed { text }
                } else {
                    WindowEvent::KeyReleased { text }
                });
            };
            if shift {
                key(true);
            }
            crate::testing::drag(
                &app,
                &[
                    at,
                    (at.0 + dx / 2.0, at.1 + dy / 2.0),
                    (at.0 + dx, at.1 + dy),
                ],
            );
            if shift {
                key(false);
            }
            the_shape(&state)
        };
        let radii = |s: &Shape| {
            let Shape::Radial { radius, .. } = *s else {
                panic!("{s:?}")
            };
            radius
        };
        // The right end of the long axis, out a little.
        let held = radii(&run(a_radial(0.2, 0.1), 1, (20.0, 3.0), true));
        assert_eq!(held[0], held[1], "a circle: {held:?}");
        assert!(held[0] > 0.2, "the handle reached: {held:?}");
        let free = radii(&run(a_radial(0.2, 0.1), 1, (20.0, 3.0), false));
        assert!(free[0] > 0.2 && free[1] == 0.1, "{free:?}");
        // The top end of the short axis likewise holds by its radius.
        let held = radii(&run(a_radial(0.2, 0.1), 4, (3.0, -15.0), true));
        assert_eq!(held[0], held[1], "a circle: {held:?}");

        let degrees = |s: &Shape| {
            let Shape::Linear { from, to } = *s else {
                panic!("{s:?}")
            };
            (to[1] - from[1]).atan2(to[0] - from[0]).to_degrees()
        };
        let off_step = |a: f32| {
            let r = a.rem_euclid(15.0);
            r.min(15.0 - r)
        };
        let linear = Shape::Linear {
            from: [0.4, 0.3],
            to: [0.6, 0.45],
        };
        assert!(off_step(degrees(&linear)) > 1.0, "placed off a step");
        let held = run(linear.clone(), 2, (17.0, -6.0), true);
        assert_ne!(held, linear, "the end moved");
        assert!(off_step(degrees(&held)) < 1e-3, "{held:?}");
        let free = run(linear.clone(), 2, (17.0, -6.0), false);
        assert!(off_step(degrees(&free)) > 1.0, "{free:?}");
        // The other end too.
        let held = run(linear, 1, (-11.0, 9.0), true);
        assert!(off_step(degrees(&held)) < 1e-3, "{held:?}");
    }

    /// Off the body a drag still pans, and the shape stays put.
    #[test]
    fn a_drag_off_the_body_still_pans() {
        let radial = a_radial(0.1, 0.05);
        let (app, state, _worker, base) = body_setup(radial.clone());
        let press = shown_at(&app, &state, base, 0.1, 0.1);
        let was_center = state.borrow().develop.center;
        crate::testing::drag(
            &app,
            &[press, (press.0 + 10.0, press.1), (press.0 + 40.0, press.1)],
        );
        assert_eq!(the_shape(&state), radial);
        assert_ne!(
            state.borrow().develop.center,
            was_center,
            "the picture did not pan"
        );
        // Nor beyond a linear's lines.
        let linear = Shape::Linear {
            from: [0.5, 0.3],
            to: [0.5, 0.45],
        };
        let (app, state, _worker, base) = body_setup(linear.clone());
        let press = shown_at(&app, &state, base, 0.15, 0.6);
        let was_center = state.borrow().develop.center;
        crate::testing::drag(
            &app,
            &[press, (press.0, press.1 + 10.0), (press.0, press.1 + 40.0)],
        );
        assert_eq!(the_shape(&state), linear);
        assert_ne!(
            state.borrow().develop.center,
            was_center,
            "the picture did not pan"
        );
    }

    /// A press on the body that never clears the drag threshold is a
    /// click, which zooms there as anywhere else on the picture, so a
    /// shape over the whole frame does not take the click-to-zoom.
    #[test]
    fn a_click_on_the_body_still_zooms_and_leaves_the_shape() {
        let radial = a_radial(0.2, 0.1);
        let (app, state, _worker, base) = body_setup(radial.clone());
        let zoomed = Rc::new(Cell::new(false));
        {
            let zoomed = zoomed.clone();
            app.on_toggle_zoom(move |_, _| zoomed.set(true));
        }
        let press = shown_at(&app, &state, base, 0.6, 0.375);
        click(&app, press.0, press.1);
        assert!(zoomed.get(), "the click did not zoom");
        assert_eq!(the_shape(&state), radial);
        assert!(state.borrow().develop.mask_drag.is_none());
    }

    /// With a dropper in hand the picture's presses are the dropper's:
    /// a drag on the body does not move the shape under it.
    #[test]
    fn a_drag_on_the_body_with_a_dropper_in_hand_is_the_droppers() {
        let radial = a_radial(0.2, 0.1);
        let (app, state, _worker, base) = body_setup(radial.clone());
        let grabbed = Rc::new(Cell::new(false));
        {
            let grabbed = grabbed.clone();
            app.on_mask_grabbed(move |_, _| grabbed.set(true));
        }
        app.set_picking("White".into());
        let press = shown_at(&app, &state, base, 0.6, 0.375);
        crate::testing::drag(
            &app,
            &[press, (press.0 + 10.0, press.1), (press.0 + 40.0, press.1)],
        );
        assert!(!grabbed.get(), "the body took a dropper's drag");
        assert_eq!(the_shape(&state), radial);
    }

    /// The body is read where the picture is shown, the crop and the
    /// zoom applied: zoomed in on a cropped corner, a drag on the
    /// radial there moves it, and a press where the radial would sit
    /// on the whole frame at zoom one is off its body and pans.
    #[test]
    fn the_body_is_hit_through_the_crop_and_the_zoom() {
        let radial = Shape::Radial {
            center: [0.7, 0.55],
            radius: [0.05, 0.05],
            angle: 0.0,
            feather: 0.5,
        };
        let (app, state, _worker, base) = body_setup(radial.clone());
        app.set_geo_has_crop(true);
        app.set_geo_crop_x(0.5);
        app.set_geo_crop_y(0.5);
        app.set_geo_crop_w(0.5);
        app.set_geo_crop_h(0.5);
        {
            let mut st = state.borrow_mut();
            st.develop.zoom = 2.0;
            st.develop.center = (100.0, 75.0);
            // The cropped frame, as the next frame would set it.
            st.develop.image_size = (200, 150);
        }
        // Off the radial on the cropped, zoomed view: it pans.
        let off = shown_at(&app, &state, base, 0.7, 0.45);
        assert!(!app.invoke_mask_body_at(off.0 - base.x, off.1 - base.y, 0.0));
        let was_center = state.borrow().develop.center;
        crate::testing::drag(&app, &[off, (off.0 + 10.0, off.1), (off.0 + 40.0, off.1)]);
        assert_eq!(the_shape(&state), radial);
        assert_ne!(
            state.borrow().develop.center,
            was_center,
            "the picture did not pan"
        );
        // On it: the radial moves and the picture stays.
        let press = shown_at(&app, &state, base, 0.72, 0.55);
        assert!(app.invoke_mask_body_at(press.0 - base.x, press.1 - base.y, 0.0));
        let was_center = state.borrow().develop.center;
        crate::testing::drag(
            &app,
            &[press, (press.0 + 10.0, press.1), (press.0 + 30.0, press.1)],
        );
        let Shape::Radial { center, .. } = the_shape(&state) else {
            panic!("still a radial");
        };
        // Thirty view pixels at zoom two on a 400 wide source.
        let du = 30.0 * app.window().scale_factor() / 2.0 / 400.0;
        assert!(
            (center[0] - (0.7 + du)).abs() < 1e-3 && (center[1] - 0.55).abs() < 1e-3,
            "{center:?}"
        );
        assert_eq!(
            state.borrow().develop.center,
            was_center,
            "the picture panned"
        );
    }

    /// The body stops at the picture's edge: a linear's band runs on
    /// into the grey around the picture as drawn, but a press there is
    /// off the picture, so it pans rather than taking hold.
    #[test]
    fn the_grey_around_the_picture_is_not_the_body() {
        let linear = Shape::Linear {
            from: [0.5, 0.3],
            to: [0.5, 0.45],
        };
        let (app, state, _worker, base) = body_setup(linear.clone());
        state.borrow_mut().develop.zoom = 0.25;
        let on = shown_at(&app, &state, base, 0.1, 0.375);
        assert!(app.invoke_mask_body_at(on.0 - base.x, on.1 - base.y, 0.0));
        let grey = shown_at(&app, &state, base, -0.2, 0.375);
        assert!(grey.0 > base.x, "the grey is in the viewport");
        assert!(!app.invoke_mask_body_at(grey.0 - base.x, grey.1 - base.y, 0.0));
        crate::testing::drag(
            &app,
            &[grey, (grey.0, grey.1 + 10.0), (grey.0, grey.1 + 40.0)],
        );
        assert_eq!(the_shape(&state), linear);
        assert!(state.borrow().develop.mask_drag.is_none());
    }

    /// Let the sections' folding and any other animation run out, so
    /// positions read are where the layout settles. An animation
    /// starts when its property is next read, not when what it
    /// follows changes: the layout is read first, then the clock run.
    fn settle_layout(app: &App) {
        for _ in 0..2 {
            placed(app, "Global");
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(500));
            slint::platform::update_timers_and_animations();
        }
    }

    /// Where each element labeled `label` is, whatever its role, top
    /// first.
    fn placed(app: &App, label: &str) -> Vec<(slint::LogicalPosition, slint::LogicalSize)> {
        use i_slint_backend_testing::ElementHandle;
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
        let mut found: Vec<_> = ElementHandle::find_by_accessible_label(app, label)
            .map(|e| (e.absolute_position(), e.size()))
            .collect();
        found.sort_by(|a, b| a.0.y.total_cmp(&b.0.y));
        found
    }

    /// The Adjustments list is pinned above the Masks tab's sections:
    /// with the sections scrolled down to the tint, the last of them, Global
    /// and each mask are still in sight, where they were before the
    /// scroll and above what the sections show, and a click on a row
    /// chooses it.
    #[test]
    fn the_adjustments_list_stays_in_reach_from_far_down_the_tab() {
        use crate::testing::buttons;
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_new_mask();
        app.invoke_target_changed(1);
        settle_layout(&app);
        let global_at = buttons(&app, "Global")[0].0;

        // Far down: the tint's Amount, at the foot of the tab, is in
        // the view under the list.
        let (amount, _) = crate::testing::labeled(&app, "Amount");
        assert!(app.get_panel_scroll() < -1500.0, "the sections scrolled");
        let global = buttons(&app, "Global");
        assert_eq!(global.len(), 1);
        assert_eq!(global[0].0.y, global_at.y, "the list does not scroll");
        let foot = placed(&app, "New mask")[0];
        assert!(
            foot.0.y + foot.1.height <= amount.y,
            "the list is above the view"
        );
        let (at, size) = global[0];
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert_eq!(app.get_target(), 0);
        assert_eq!(state.borrow().develop.target, None);

        // And from far down to another mask.
        app.invoke_target_changed(1);
        settle_layout(&app);
        crate::testing::labeled(&app, "Amount");
        assert!(app.get_panel_scroll() < -1500.0, "the sections scrolled");
        let second = buttons(&app, "Mask 2");
        assert_eq!(second.len(), 1);
        let (at, size) = second[0];
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert_eq!(app.get_target(), 2);
        assert_eq!(state.borrow().develop.target, Some(1));
    }

    /// A long list holds at most six rows and scrolls inside itself,
    /// so New mask and the sections under it keep their place; the
    /// chosen row is kept in sight, at the list's foot after New mask
    /// and back at its head on Global.
    #[test]
    fn a_long_adjustments_list_scrolls_inside_itself() {
        use crate::testing::buttons;
        let app = window(1);
        // A tall desktop window, the whole of the mask's section on it
        // with Refine edge under the toggles.
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 1000.0));
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        settle_layout(&app);
        // Global alone: the list's top, and how far below it New mask is.
        let top = buttons(&app, "Global")[0].0.y;
        let alone = placed(&app, "New mask")[0].0.y - top;
        app.invoke_new_mask();
        settle_layout(&app);
        let pitch = buttons(&app, "Mask 1")[0].0.y - top;
        assert!(pitch > 0.0);

        for _ in 1..10 {
            app.invoke_new_mask();
        }
        settle_layout(&app);
        assert_eq!(app.get_target(), 10);
        let new_mask = placed(&app, "New mask")[0];
        assert!(
            (new_mask.0.y - top - (alone + 5.0 * pitch)).abs() < 1.0,
            "six rows shown, not eleven: New mask {} below the list's top",
            new_mask.0.y - top
        );
        let chosen = buttons(&app, "Mask 10")[0];
        assert!(chosen.0.y >= top - 0.5, "the chosen row is in sight");
        assert!(chosen.0.y + chosen.1.height <= new_mask.0.y);
        // Scrolled up out of the list, and so out of sight.
        assert!(buttons(&app, "Global").is_empty(), "Global scrolled out");

        app.invoke_target_changed(0);
        settle_layout(&app);
        let global = buttons(&app, "Global")[0];
        assert!((global.0.y - top).abs() < 0.5, "Global back at the head");
        let (at, size) = buttons(&app, "Mask 3")[0];
        assert!(at.y + size.height <= new_mask.0.y);
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert_eq!(app.get_target(), 3);

        // Deleted down to three masks from the list's foot (a delete
        // leaves Global chosen), it is short again and nothing is
        // left scrolled away.
        for last in (4..=10).rev() {
            app.invoke_target_changed(last);
            settle_layout(&app);
            app.invoke_delete_adjustment();
            settle_layout(&app);
        }
        assert_eq!(app.get_adjustment_names().row_count(), 4);
        let global = buttons(&app, "Global")[0];
        assert!((global.0.y - top).abs() < 0.5, "Global at the head again");
        let new_mask = placed(&app, "New mask")[0];
        assert!((new_mask.0.y - top - (alone + 3.0 * pitch)).abs() < 1.0);
    }

    /// Folded, the list is its header alone, and the header names
    /// the chosen row.
    #[test]
    fn the_folded_adjustments_list_names_the_chosen_row() {
        let app = window(1);
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_new_mask();
        app.invoke_target_changed(1);
        app.set_collapsed_adjustments(true);
        settle_layout(&app);
        assert!(
            placed(&app, "New mask").is_empty(),
            "the rows are folded away"
        );
        assert!(placed(&app, "Mask 2").is_empty());
        // The header's note, and the mask section's heading, which
        // shows the name as typed.
        assert_eq!(placed(&app, "Mask 1").len(), 2);
    }

    /// Where the bottom of the panel's footer (Fit and Export) is: the
    /// lowest "Fit", since the zoom's words say "Fit" too, higher up.
    fn footer_y(app: &App) -> f32 {
        placed(app, "Fit").last().expect("the footer").0.y
    }

    /// In a short window, or under a tall scope, the list gives up
    /// rows before the panel gives up room: with ten masks, the footer
    /// is where it is on Develop, not pushed down under the strip,
    /// whichever scope is over the panel.
    #[test]
    fn a_short_panel_keeps_its_footer_with_a_long_list() {
        let app = window(1);
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        for _ in 0..10 {
            app.invoke_new_mask();
        }
        let shown = |app: &App| {
            (0..=10)
                .filter(|i| !crate::testing::buttons(app, &format!("Mask {i}")).is_empty())
                .count()
        };
        for scope in ["RGB", "Wave", "Vector"] {
            app.set_scope(scope.into());
            for height in [650.0, 700.0, 800.0, 900.0] {
                app.window()
                    .set_size(slint::LogicalSize::new(1500.0, height));
                app.set_panel_tab("Develop".into());
                settle_layout(&app);
                let develop = footer_y(&app);
                app.set_panel_tab("Masks".into());
                settle_layout(&app);
                let rows = shown(&app);
                // The vectorscope is as tall as the panel is wide:
                // under it, below about 780 px, even one row and New
                // mask are more than the panel has.
                if scope != "Vector" || height >= 800.0 {
                    assert!(
                        (footer_y(&app) - develop).abs() < 0.5,
                        "{scope} at {height}: the footer stays put"
                    );
                }
                assert!(rows >= 1, "{scope} at {height}: a row in sight");
                assert!(
                    !placed(&app, "Mask 10").is_empty(),
                    "{scope} at {height}: the chosen row is in sight"
                );
            }
        }
        // And with the room, all six.
        app.set_scope("RGB".into());
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 1100.0));
        settle_layout(&app);
        assert_eq!(shown(&app), 6, "six rows in a tall window");
    }

    /// The chosen row is in sight when the list is built again: on
    /// coming back to the Masks tab, and when the rows change under a
    /// target that does not.
    #[test]
    fn the_chosen_row_is_in_sight_when_the_list_comes_back() {
        use crate::testing::buttons;
        let app = window(1);
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        for _ in 0..10 {
            app.invoke_new_mask();
        }
        app.invoke_target_changed(9);
        settle_layout(&app);
        assert_eq!(buttons(&app, "Mask 9").len(), 1);
        app.set_panel_tab("Develop".into());
        settle_layout(&app);
        app.set_panel_tab("Masks".into());
        settle_layout(&app);
        assert_eq!(buttons(&app, "Mask 9").len(), 1, "Mask 9 in sight again");

        // Global chosen and the list scrolled to its foot by the wheel,
        // then fewer rows with Global still chosen and the list as tall
        // as before, as another photo or an undo leaves it: the list
        // shows the chosen row again, Global at its head.
        app.invoke_target_changed(0);
        settle_layout(&app);
        let (global, size) = buttons(&app, "Global")[0];
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(global.x + size.width / 2.0, global.y + 40.0),
                delta_x: 0.0,
                delta_y: -400.0,
            });
        settle_layout(&app);
        assert!(
            buttons(&app, "Global").is_empty(),
            "the wheel scrolled the list"
        );
        let names: Vec<slint::SharedString> = std::iter::once("Global".into())
            .chain((1..=8).map(|i| format!("Mask {i}").into()))
            .collect();
        app.set_adjustment_names(std::rc::Rc::new(slint::VecModel::from(names)).into());
        app.set_adjustment_on(std::rc::Rc::new(slint::VecModel::from(vec![true; 8])).into());
        settle_layout(&app);
        let head = buttons(&app, "Global");
        assert_eq!(head.len(), 1, "Global in sight");
        assert_eq!(head[0].0.y, global.y, "Global at the head");
    }

    /// A mask's bin, a picture and no text, says "Remove" at the
    /// pointer once the pointer has rested on it, and the word goes
    /// when the pointer leaves and
    /// when the bin is pressed.
    #[test]
    fn a_mask_s_bin_says_remove_after_a_rest_and_the_word_goes_with_a_press() {
        use crate::testing::rest_on;
        use slint::Model;

        let app = window(1);
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_target_changed(1);
        let name = app.get_adjustment_names().row_data(1).expect("the mask");
        let label = format!("Remove {name}");
        let away = || {
            app.window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(2.0, 2.0),
                });
            slint::platform::update_timers_and_animations();
        };

        assert_eq!(app.get_tip_text(), "", "no word before a rest");
        let (x, y) = rest_on(&app, &label);
        assert_eq!(app.get_tip_text(), "Remove", "the bin's word after a rest");
        away();
        assert_eq!(app.get_tip_text(), "", "gone with the pointer");
        rest_on(&app, &label);
        assert_eq!(app.get_tip_text(), "Remove");
        click(&app, x, y);
        assert_eq!(app.get_tip_text(), "", "gone with the press");
    }

    /// Esc puts a mask's tool or a dropper down and its hint with it,
    /// and leaves whatever has been written over the hint: a develop's
    /// word, a failure, or an export's.
    #[test]
    fn esc_drops_a_tools_hint_and_leaves_what_replaced_it() {
        let app = window(1);
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_target_changed(1);
        let filling = "developing... the fill model is at work on Fill 3";
        let lines = [
            "",
            filling,
            "developing...",
            "failed: no memory",
            "exporting 3 frames...",
        ];
        for keep in lines {
            app.invoke_add_shape("Brush".into(), "Add".into());
            assert_eq!(app.get_status(), placing_hint("Brush"));
            if !keep.is_empty() {
                app.set_status(keep.into());
            }
            app.invoke_stop_placing();
            assert_eq!(app.get_placing(), "");
            assert_eq!(app.get_status(), keep, "the Brush");

            app.invoke_pick_started("Range".into());
            assert_eq!(app.get_picking(), "Range");
            if !keep.is_empty() {
                app.set_status(keep.into());
            }
            app.invoke_stop_placing();
            assert_eq!(app.get_picking(), "");
            assert_eq!(app.get_status(), keep, "the dropper");
        }
    }

    /// One brush row: a Subtract stroke takes from the mask whatever its
    /// shapes gave, on a new brush or one already painted, and still
    /// does after the edit is saved and read back; an Add joins; an
    /// Erase removes the brush's own paint and cut, so on a new brush
    /// it leaves the mask alone. A brush asked for under Subtract lands
    /// joined by Add with its row on Subtract, and the row goes back to
    /// Add when it is put down, after an undo too; under Intersect it
    /// keeps the mode.
    #[test]
    fn the_brush_row_adds_to_and_takes_from_the_mask() {
        use greycard_edit::mask::Sample;
        let bright = Some(Sample {
            lightness: 0.9,
            a: 0.0,
            b: 0.0,
        });
        let aspect = 2000.0 / 3000.0;
        // A window on a frame, its mask holding a Luminance range when
        // `first`.
        let open = |first: bool| {
            let app = window(1);
            let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(1));
            app.invoke_select(0);
            {
                let mut st = state.borrow_mut();
                st.develop.source_size = (3000, 2000);
                st.develop.center = (1500.0, 1000.0);
            }
            app.set_panel_tab("Masks".into());
            app.invoke_new_mask();
            if first {
                app.invoke_add_shape("Luminance".into(), "Add".into());
            }
            (app, state, worker)
        };
        // A stroke with the brush in hand, from (x, 300) to its right.
        let paint = |app: &App, x: f32| {
            app.invoke_place_pressed(x, 300.0, false);
            app.invoke_place_dragged(x + 10.0, 305.0);
            app.invoke_place_dragged(x + 30.0, 315.0);
            app.invoke_place_released();
        };
        let stroke = |app: &App, mode: &str, op: &str| {
            app.set_shape_mode(mode.into());
            app.set_brush_op(op.into());
            app.invoke_add_shape("Brush".into(), mode.into());
            paint(app, 300.0);
        };
        let edit_of = |app: &App, state: &Rc<Shared>| {
            let st = &*state.borrow();
            read_edit(app, &st.develop.edit, st.develop.target)
        };
        let last_brush = |edit: &Edit| {
            let c = edit.adjustments[0].mask.components.last().expect("a shape");
            let Shape::Brush { strokes } = &c.shape else {
                panic!("no brush landed");
            };
            (c.mode, strokes.clone())
        };
        // The mask's weight at `p` for a bright sample, painted, and
        // read back.
        let weights = |edit: &Edit, p: [f32; 2]| {
            let json = serde_json::to_string(edit).unwrap();
            let back: Edit = serde_json::from_str(&json).unwrap();
            [edit.clone(), back].map(|edit| {
                let (mut rasters, mut learned) = (HashMap::new(), HashMap::new());
                let (locals, _) = bake_locals(&edit, &mut rasters, &mut learned, aspect, None);
                locals[0].weight_sampled(p[0], p[1], bright)
            })
        };
        // Subtract on a new brush over the range, under Add and
        // Subtract: a brush joined by Add, its stroke a Subtract, the
        // mask gone there, the row left on Subtract.
        for mode in ["Add", "Subtract"] {
            let (app, state, _worker) = open(true);
            stroke(&app, mode, "Subtract");
            let edit = edit_of(&app, &state);
            let (shape_mode, strokes) = last_brush(&edit);
            assert_eq!((shape_mode, strokes[0].op), (Mode::Add, Op::Subtract));
            for w in weights(&edit, strokes[0].points[0]) {
                assert!(w < 0.01, "{mode}: {w}");
            }
            assert_eq!(app.get_brush_op(), "Subtract", "{mode}: the row");
        }
        // A brush asked for under Subtract with the row on Add cuts too:
        // the shape row's Subtract is the brush row's.
        let (app, state, _worker) = open(true);
        stroke(&app, "Subtract", "Add");
        let edit = edit_of(&app, &state);
        let (shape_mode, strokes) = last_brush(&edit);
        assert_eq!((shape_mode, strokes[0].op), (Mode::Add, Op::Subtract));
        for w in weights(&edit, strokes[0].points[0]) {
            assert!(w < 0.01, "{w}");
        }
        // Put down, the row is back on Add, so the next brush asked for
        // under Add adds.
        app.invoke_stop_placing();
        assert_eq!(app.get_brush_op(), "Add", "the row put back");
        app.set_shape_mode("Add".into());
        app.invoke_add_shape("Brush".into(), "Add".into());
        paint(&app, 600.0);
        let (_, strokes) = last_brush(&edit_of(&app, &state));
        assert_eq!(strokes.last().unwrap().op, Op::Add);
        // After an undo too: the brush asked for under Subtract taken
        // back, the next one asked for under Add adds.
        let settle = || {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
                crate::panel::edit::SAVE_MS + 100,
            ));
            slint::platform::update_timers_and_animations();
        };
        let (app, state, _worker) = open(true);
        settle();
        stroke(&app, "Subtract", "Add");
        settle();
        app.invoke_undo();
        let edit = edit_of(&app, &state);
        assert_eq!(edit.adjustments[0].mask.components.len(), 1);
        app.invoke_stop_placing();
        app.set_shape_mode("Add".into());
        app.invoke_add_shape("Brush".into(), "Add".into());
        assert_eq!(app.get_brush_op(), "Add");
        paint(&app, 300.0);
        let edit = edit_of(&app, &state);
        assert_eq!(edit.adjustments[0].mask.components.len(), 2);
        let (shape_mode, strokes) = last_brush(&edit);
        assert_eq!((shape_mode, strokes[0].op), (Mode::Add, Op::Add), "+ Brush");
        // A brush already painted: Add strokes elsewhere first, then the
        // row on Subtract takes from the range where the testers
        // painted and found nothing happen.
        let (app, state, _worker) = open(true);
        stroke(&app, "Add", "Add");
        app.set_brush_op("Subtract".into());
        paint(&app, 600.0);
        let edit = edit_of(&app, &state);
        let (_, strokes) = last_brush(&edit);
        assert_eq!(strokes.len(), 2, "one brush, two strokes");
        assert_eq!(strokes[1].op, Op::Subtract);
        for w in weights(&edit, strokes[1].points[0]) {
            assert!(w < 0.01, "{w}");
        }
        for w in weights(&edit, strokes[0].points[0]) {
            assert!(w > 0.9, "the first stroke's place: {w}");
        }
        // An Erase over the cut gives the range back.
        app.set_brush_op("Erase".into());
        paint(&app, 600.0);
        let edit = edit_of(&app, &state);
        let (_, strokes) = last_brush(&edit);
        for w in weights(&edit, strokes[1].points[0]) {
            assert!(w > 0.9, "erased: {w}");
        }
        // An Erase on a new brush has nothing of its own to remove.
        let (app, state, _worker) = open(true);
        stroke(&app, "Add", "Erase");
        let edit = edit_of(&app, &state);
        let (shape_mode, strokes) = last_brush(&edit);
        assert_eq!((shape_mode, strokes[0].op), (Mode::Add, Op::Erase));
        for w in weights(&edit, strokes[0].points[0]) {
            assert!(w > 0.9, "{w}");
        }
        // Under Intersect a brush keeps its mode: only where it paints.
        let (app, state, _worker) = open(true);
        stroke(&app, "Intersect", "Add");
        let edit = edit_of(&app, &state);
        let (shape_mode, strokes) = last_brush(&edit);
        assert_eq!((shape_mode, strokes[0].op), (Mode::Intersect, Op::Add));
        for w in weights(&edit, strokes[0].points[0]) {
            assert!(w > 0.9, "{w}");
        }
        // The mask's first shape: a Subtract on nothing is nothing.
        let (app, state, _worker) = open(false);
        stroke(&app, "Add", "Subtract");
        let edit = edit_of(&app, &state);
        let (shape_mode, strokes) = last_brush(&edit);
        assert_eq!((shape_mode, strokes[0].op), (Mode::Add, Op::Subtract));
        for w in weights(&edit, strokes[0].points[0]) {
            assert!(w < 0.01, "{w}");
        }
    }

    /// The edit's rest, past the save's wait, so each change is its
    /// own step.
    fn rest() {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
            crate::panel::edit::SAVE_MS + 100,
        ));
        slint::platform::update_timers_and_animations();
    }

    /// A mask's only shape has its × too: the mask stays, empty, the
    /// step says the shape was removed as any other delete's does, and
    /// one undo gives the shape back.
    #[test]
    fn the_last_shape_is_deleted_by_its_cross_and_one_undo_restores_it() {
        let app = window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        app.invoke_add_shape("Luminance".into(), "Add".into());
        rest();
        crate::testing::settle(&app);
        assert_eq!(count_labeled(&app, "Remove this shape"), 1);
        let (at, size) = crate::testing::labeled(&app, "Remove this shape");
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        rest();
        {
            let st = state.borrow();
            assert_eq!(st.develop.edit.adjustments.len(), 1, "the mask stays");
            assert!(st.develop.edit.adjustments[0].mask.components.is_empty());
        }
        let top = app.get_history_rows().row_data(0).unwrap().name;
        assert!(top.ends_with(" shape removed"), "{top}");
        app.invoke_undo();
        let st = state.borrow();
        assert_eq!(st.develop.edit.adjustments.len(), 1);
        assert_eq!(st.develop.edit.adjustments[0].mask.components.len(), 1);
    }

    /// A straightened frame opened with a linear chosen: before its
    /// develop lands the source has no size, and the shape is not
    /// drawn rather than drawn through NaN; once it has one, the
    /// outline is drawn and every point of it is finite.
    #[test]
    fn a_straightened_frame_draws_its_linear_only_once_its_size_is_known() {
        let linear = Shape::Linear {
            from: [0.5, 0.3],
            to: [0.5, 0.45],
        };
        let (app, state, _worker, _base) = body_setup(linear);
        {
            let mut st = state.borrow_mut();
            st.develop.edit.geometry.angle = 3.0;
            st.develop.source_size = (0, 0);
        }
        {
            let st = state.borrow();
            show_edit(&st, &st.develop.edit, &app, st.develop.target);
            assert_eq!(app.get_geo_angle(), 3.0);
            show_shape(&st, &app, &st.develop.edit);
        }
        assert_eq!(
            app.get_mask_kind(),
            "",
            "nothing drawn on a source of no size"
        );
        state.borrow_mut().develop.source_size = (400, 300);
        {
            let st = state.borrow();
            show_shape(&st, &app, &st.develop.edit);
        }
        assert_eq!(app.get_mask_kind(), "Linear");
        assert!(app.get_mask_angle().is_finite());
        let handles = state.borrow().develop.mask_handles.clone();
        assert_eq!(handles.row_count(), 3);
        assert!(handles.iter().all(|p| p.x.is_finite() && p.y.is_finite()));
        rest();
    }

    /// Esc with nothing in hand puts a placed linear's handles down,
    /// the shape still chosen; a press on its row or on its band
    /// brings them back. With the brush in hand Esc puts the brush
    /// down and leaves the handles be.
    #[test]
    fn esc_puts_a_shapes_handles_down_and_a_press_brings_them_back() {
        use crate::testing::press;
        use slint::platform::Key;
        let linear = Shape::Linear {
            from: [0.5, 0.3],
            to: [0.5, 0.45],
        };
        let (app, state, _worker, base) = body_setup(linear);
        {
            let st = state.borrow();
            show_edit(&st, &st.develop.edit, &app, st.develop.target);
        }
        let handles = |app: &App| {
            let pts = [(0.5, 0.375), (0.5, 0.3), (0.5, 0.45)].map(|(u, v)| {
                let (x, y) = source_to_view(&state.borrow(), app, u, v);
                Pt { x, y }
            });
            app.set_mask_handles(ModelRc::new(VecModel::from(pts.to_vec())));
        };
        handles(&app);
        app.set_mask_kind("Linear".into());
        let target = app.get_target();
        press(&app, Key::Escape);
        assert!(app.get_handles_down(), "Esc put the handles down");
        assert_eq!(app.get_mask_kind(), "Linear", "the shape still chosen");
        assert_eq!(app.get_target(), target);
        assert_eq!(
            i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "CropHandle")
                .count(),
            0,
            "none drawn"
        );
        // Its row, beside its own invert.
        let (at, size) = crate::testing::labeled(&app, "Invert this shape");
        click(&app, at.x - 60.0, at.y + size.height / 2.0);
        assert!(!app.get_handles_down(), "the row brought them back");
        press(&app, Key::Escape);
        assert!(app.get_handles_down());
        // Its band on the picture, clear of the handles.
        let p = shown_at(&app, &state, base, 0.15, 0.375);
        click(&app, p.0, p.1);
        assert!(!app.get_handles_down(), "the band brought them back");
        // The brush in hand: Esc is the brush's alone.
        app.invoke_add_shape("Brush".into(), "Add".into());
        assert_eq!(app.get_placing(), "Brush");
        press(&app, Key::Escape);
        assert_eq!(app.get_placing(), "", "the brush put down");
        assert!(!app.get_handles_down(), "and nothing else");
        handles(&app);
        app.set_mask_kind("Linear".into());
        press(&app, Key::Escape);
        assert!(app.get_handles_down(), "the next Esc is the handles'");
    }

    /// A mask's row under the pointer lights that mask over the
    /// picture in the overlay's red, whatever Show mask says, and the
    /// pointer leaving the row puts back what was shown; the chosen
    /// mask and Show mask are as they were. The pixels are checked
    /// where there is a GPU to draw them.
    #[test]
    fn a_hovered_masks_row_lights_its_mask() {
        use crate::render::{Renderer, View};
        use slint::platform::WindowEvent;
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.develop.source_size = (128, 128);
            st.develop.image_size = (128, 128);
            st.develop.zoom = 1.0;
            let disc = |id, name: &str, u| Adjustment {
                id,
                name: name.into(),
                mask: Mask {
                    components: vec![Component {
                        shape: Shape::Radial {
                            center: [u, 0.5],
                            radius: [0.1, 0.1],
                            angle: 0.0,
                            feather: 0.0,
                        },
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            };
            st.develop.edit.adjustments = vec![disc(1, "Left", 0.25), disc(2, "Right", 0.75)];
            st.develop.target = Some(0);
            show_names(&st.develop.edit, &app);
        }
        app.set_panel_tab("Masks".into());
        app.set_target(1);
        let at = |label: &str| {
            let (p, size) = crate::testing::labeled(&app, label);
            slint::LogicalPosition::new(p.x + size.width / 2.0, p.y + size.height / 2.0)
        };
        let (left, right) = (at("Left"), at("Right"));
        let to = |position| {
            app.window()
                .dispatch_event(WindowEvent::PointerMoved { position })
        };
        let away = slint::LogicalPosition::new(0.0, 0.0);
        let shown = || shown_mask(&app, state.borrow().develop.target);

        // The overlay as the view draws it: red over the right disc's
        // middle or not, and never over the left's.
        let reds = |show: Option<usize>| -> Option<(bool, bool)> {
            let (device, queue) = greycard_viewport::testing::device("hovered_mask_row")?;
            let n = 128usize;
            let image = greycard_core::image::WorkingImage {
                width: n,
                height: n,
                data: vec![0.2; n * n * 3],
            };
            let mut renderer = Renderer::new(&device, &queue);
            renderer.upload(&greycard_render::halves::Halves::from_image(&image, None));
            let mut view = View::blank();
            view.center = (n as f32 / 2.0, n as f32 / 2.0);
            view.plane = (n as f32, n as f32);
            view.frame_size = (n as f32, n as f32);
            let mut st = state.borrow_mut();
            let st = &mut *st;
            view.locals = bake_locals(
                &st.develop.edit,
                &mut st.develop.rasters,
                &mut st.develop.learned,
                1.0,
                None,
            )
            .0;
            view.show_mask = show;
            let target = renderer.render(n as u32, n as u32, &view).0;
            let picture = renderer.read_back(&target).expect("read back");
            let red = |x: u32| {
                let p = picture.get_pixel(x, n as u32 / 2).0;
                p[0] as i32 - p[1] as i32 > 60
            };
            Some((red(32), red(96)))
        };

        assert_eq!(shown(), None);
        to(right);
        assert_eq!(app.get_mask_lit(), 1, "the row under the pointer");
        assert_eq!(shown(), Some(1), "lit with Show mask off");
        if let Some(lit) = reds(shown()) {
            assert_eq!(lit, (false, true), "the right disc painted, the left not");
        }
        to(away);
        assert_eq!(app.get_mask_lit(), -1);
        assert_eq!(shown(), None, "left: nothing shown, as before");
        if let Some(lit) = reds(shown()) {
            assert_eq!(lit, (false, false));
        }

        // The chosen row's bin counts as the row. Its switch does not:
        // the overlay goes down there, so a toggle shows the
        // adjustment's effect alone, and comes back on the row.
        let (row, _) = crate::testing::labeled(&app, "Right");
        to(slint::LogicalPosition::new(row.x + 8.0 + 7.0, right.y));
        assert_eq!(app.get_mask_lit(), -1, "on the switch");
        assert_eq!(shown(), None, "the overlay down on the switch");
        to(right);
        assert_eq!(app.get_mask_lit(), 1, "back on the row");
        to(at("Remove Left"));
        assert_eq!(app.get_mask_lit(), 0, "on the bin");
        to(away);

        // With Show mask on, another row lights its own mask and the
        // chosen one's row its own; leaving puts the chosen back.
        app.set_show_mask(true);
        assert_eq!(shown(), Some(0));
        to(right);
        assert_eq!(shown(), Some(1));
        to(left);
        assert_eq!(shown(), Some(0), "the chosen mask's own row");
        to(away);
        assert_eq!(shown(), Some(0));
        // Off the tab, a row left lit by a tab switch lights nothing.
        to(right);
        app.set_panel_tab("Develop".into());
        assert_eq!(shown(), Some(0));
        app.set_panel_tab("Masks".into());
        to(away);

        assert!(app.get_show_mask(), "Show mask as it was");
        assert_eq!(app.get_target(), 1, "the chosen row as it was");
        assert_eq!(state.borrow().develop.target, Some(0));
    }

    /// Shift at the press holds a shape: a linear placed or turned by
    /// an end lands on a 15-degree step, a radial placed or reached by
    /// an axis end is a circle. The history steps are a plain drag's.
    #[test]
    fn a_shift_drag_holds_a_linear_to_steps_and_a_radial_to_a_circle() {
        // A frame's window, a shape of `kind` placed by a drag from
        // (300, 200) by (dx, dy), then its handle `h` dragged by (60,
        // 25); Shift at both presses when `shift`. The shape after each
        // drag and the history's rows.
        let run = |kind: &str, (dx, dy): (f32, f32), h: i32, shift: bool| {
            let app = window(1);
            let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
            app.invoke_select(0);
            {
                let mut st = state.borrow_mut();
                st.develop.source_size = (3000, 2000);
                st.develop.center = (1500.0, 1000.0);
            }
            app.set_panel_tab("Masks".into());
            app.invoke_new_mask();
            rest();
            app.invoke_add_shape(kind.into(), "Add".into());
            app.set_place_shift(shift);
            app.invoke_place_pressed(300.0, 200.0, false);
            app.invoke_place_dragged(300.0 + dx, 200.0 + dy);
            app.invoke_place_released();
            rest();
            let shape = || {
                let st = state.borrow();
                st.develop.edit.adjustments[0].mask.components[0]
                    .shape
                    .clone()
            };
            let placed = shape();
            app.invoke_mask_grabbed(h, shift);
            app.invoke_mask_dragged(h, 60.0, 25.0, false, shift);
            app.invoke_mask_dropped();
            rest();
            let rows = app.get_history_rows();
            let steps: Vec<String> = (0..rows.row_count())
                .map(|i| rows.row_data(i).unwrap().name.to_string())
                .collect();
            (placed, shape(), steps)
        };
        let degrees = |s: &Shape| {
            let Shape::Linear { from, to } = *s else {
                panic!("{s:?}")
            };
            (to[1] - from[1]).atan2(to[0] - from[0]).to_degrees()
        };
        let off_step = |a: f32| {
            let r = a.rem_euclid(15.0);
            r.min(15.0 - r)
        };
        let (placed, turned, held_steps) = run("Linear", (300.0, 130.0), 2, true);
        assert!(off_step(degrees(&placed)) < 1e-3, "{placed:?}");
        assert!(off_step(degrees(&turned)) < 1e-3, "{turned:?}");
        let (placed, turned, plain_steps) = run("Linear", (300.0, 130.0), 2, false);
        assert!(off_step(degrees(&placed)) > 1.0, "{placed:?}");
        assert!(off_step(degrees(&turned)) > 1.0, "{turned:?}");
        assert_eq!(held_steps, plain_steps);
        let radii = |s: &Shape| {
            let Shape::Radial { radius, .. } = *s else {
                panic!("{s:?}")
            };
            radius
        };
        let (placed, reached, held_steps) = run("Radial", (100.0, 30.0), 1, true);
        let (p, r) = (radii(&placed), radii(&reached));
        assert!(p[0] == p[1] && r[0] == r[1], "{p:?} {r:?}");
        assert!((r[0] - p[0]).abs() > 1e-3, "the handle reached");
        let (placed, reached, plain_steps) = run("Radial", (100.0, 30.0), 1, false);
        let (p, r) = (radii(&placed), radii(&reached));
        assert!(p[0] != p[1] && r[0] != r[1], "{p:?} {r:?}");
        assert_eq!(held_steps, plain_steps);
    }
}
