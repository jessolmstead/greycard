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
                // What the Fill painted waits, unseen, for its return;
                // the sliders go back to what they now edit.
                crate::panel::retouch::show_patches(&st, &st.develop.edit, &app);
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
                // The Fill's: painted, made at Generate.
                if method == RetouchMethod::Fill {
                    crate::panel::painted::pressed(&mut st, &app, at);
                    return;
                }
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
                    more: Vec::new(),
                };
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
            if crate::panel::painted::dragged(&mut st, at, MIN_STEP * 4.0) {
                app.window().request_redraw();
                return;
            }
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
            // A Fill's stroke stays painted; nothing is made yet.
            if crate::panel::painted::released(&mut st, &app) {
                return;
            }
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
mod tests;
