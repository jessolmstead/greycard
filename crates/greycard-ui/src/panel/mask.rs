use crate::panel::assets::offer_model;
use crate::panel::cull::leave_cull;
use crate::panel::edit::{develop_soon, read_edit, show_edit, show_names};
use crate::panel::retouch::show_patches;
use crate::panel::viewport::{source_to_view, view_to_source};
use crate::*;

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
    /// A new adjustment, rather than a shape added to one.
    pub(crate) fresh: bool,
    /// An object's press has become a box.
    pub(crate) boxed: bool,
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

/// Put the chosen shape's own controls on the panel.
pub(crate) fn show_component(mask: &Mask, app: &App) {
    let i = (app.get_component().max(0) as usize).min(mask.components.len().saturating_sub(1));
    app.set_component(i as i32);
    match mask.components.get(i) {
        Some(c) => {
            app.set_component_invert(c.invert);
            let feather = match c.shape {
                Shape::Radial { feather, .. } => Some(feather),
                _ => None,
            };
            app.set_has_feather(feather.is_some());
            app.set_mask_feather(feather.unwrap_or(0.5));
            show_range(&c.shape, app);
            app.set_component_kind(c.shape.name().into());
        }
        None => {
            app.set_has_feather(false);
            app.set_component_kind("".into());
        }
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

/// An adjustment chosen whose chosen shape is a range: its sliders
/// are the whole of it, and they are behind the shapes' fold, so the
/// fold opens. Only on choosing, so a fold closed by hand stays shut
/// while the adjustment is edited.
pub(crate) fn reveal_range(edit: &Edit, target: Option<usize>, app: &App) {
    let chosen = app.get_component().max(0) as usize;
    if target
        .and_then(|i| edit.adjustments.get(i))
        .and_then(|a| a.mask.components.get(chosen))
        .is_some_and(|c| c.shape.is_range())
    {
        app.set_shapes_open(true);
    }
}

/// Whether a shape is made whole by its button, with no drag on the
/// picture: a model's subject, or a range of the picture's own.
fn made_at_once(kind: &str) -> bool {
    matches!(kind, "Subject" | "Sky" | "Luminance" | "Color")
}

/// What the status line says once a shape made at once is in.
fn made_hint(kind: &str) -> &'static str {
    match kind {
        "Subject" => "finding the subject",
        "Sky" => "finding the sky",
        "Color" => "click a color in the picture to center the window on it; Esc to keep the skin",
        _ => "",
    }
}

/// A shape as it was when a handle was pressed, and which handle.
pub(crate) struct MaskDrag {
    shape: Shape,
    handle: usize,
    /// The handle's place on the view when pressed, logical pixels.
    at: (f32, f32),
}

/// The adjustments baked for the viewport, their brushes' rasters
/// from `rasters`, brought up to their strokes, their learned masks
/// from `learned` where made for the shape as it is; a raster no
/// shape has any more is dropped. Also the learned shapes still
/// wanting their raster.
pub(crate) fn bake_locals(
    edit: &Edit,
    rasters: &mut HashMap<(u64, usize), Arc<Raster>>,
    learned: &mut HashMap<Key, (Shape, Arc<Raster>)>,
    aspect: f32,
) -> (Vec<Local>, Vec<(Key, Shape)>) {
    let mut live = std::collections::HashSet::new();
    let mut wants = Vec::new();
    let locals = edit
        .adjustments
        .iter()
        .take(MAX_LOCALS)
        .map(|a| Local {
            baked: Baked::of(&a.look),
            mask: a.mask.clone(),
            enabled: a.enabled && !a.mask.is_empty(),
            rasters: a
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
                    // histogram's, keeps the old version; this one moves on.
                    Arc::make_mut(raster).update(strokes);
                    Some(RasterRef(raster.clone()))
                })
                .collect(),
        })
        .collect();
    rasters.retain(|k, _| live.contains(k));
    learned.retain(|k, _| live.contains(k));
    (locals, wants)
}

/// What to do for a learned shape wanted: ask the worker for it, offer
/// its model, or wait (the sheet is busy, or every model it could use
/// was declined).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Ask(&'static greycard_ai::Model),
    /// A model to offer, and whether it is the Subject rewrite offered
    /// in place of an original already in the store — a card that
    /// original falls back to the CPU on — rather than a first-ever
    /// Subject offer, which the sheet's note reads differently.
    Offer(&'static greycard_ai::Model, bool),
    Wait,
}

/// The step for `shape`, given what the store has, the providers, the
/// models declined this session, those whose fetch failed, whether the
/// sheet is free for an offer, and whether the Subject original is on
/// record as having failed on WebGPU. A Subject whose WebGPU file was
/// declined or could not be fetched is offered the original. A model
/// whose fetch failed is not offered again this session, as one
/// declined is not: offline, the sheet would come straight back.
pub(crate) fn step(
    shape: &Shape,
    have: impl Fn(&greycard_ai::Model) -> bool + Copy,
    providers: &[greycard_ai::Provider],
    declined: &[&'static str],
    failed: &[&'static str],
    sheet_free: bool,
    original_failed_on_webgpu: impl Fn(&greycard_ai::Model) -> bool,
) -> Option<Step> {
    let unavailable: Vec<&str> = declined.iter().chain(failed).copied().collect();
    let model = crate::ai::model_with(
        shape,
        have,
        providers,
        &unavailable,
        original_failed_on_webgpu,
    )?;
    Some(if have(model) {
        Step::Ask(model)
    } else if sheet_free && !declined.contains(&model.id) && !failed.contains(&model.id) {
        // The store already has the Subject original: this offer is
        // the rewrite standing in for it, not the first Subject offer.
        Step::Offer(
            model,
            model.id == greycard_ai::SUBJECT_WEBGPU.id && have(&greycard_ai::SUBJECT),
        )
    } else if matches!(shape, Shape::Subject {}) && have(&greycard_ai::SUBJECT) {
        // Nothing to offer right now (another sheet is up, a fetch is
        // running, or this model was already declined or failed this
        // session), but the store's own original runs: use it rather
        // than wait on an offer that is not coming this round. For a
        // Subject want alone: an Object or a Sky asked for with the
        // Subject model would load the wrong file.
        Step::Ask(&greycard_ai::SUBJECT)
    } else {
        Step::Wait
    })
}

/// Ask the worker for the learned masks wanted and not yet asked for;
/// where the model is not in the store, offer to fetch it.
pub(crate) fn ask_for(st: &mut State, app: &App, wants: Vec<(Key, Shape)>) {
    for (key, shape) in wants {
        if st.asked.get(&key) == Some(&shape) {
            continue;
        }
        let store = st.store.clone();
        let have = |m: &greycard_ai::Model| store.as_ref().is_some_and(|s| s.have(m));
        let providers = crate::ai::providers();
        // Cached: the first ask opens a `wgpu::Instance` to name the
        // adapter and every one after reads and parses providers.json,
        // neither of which belongs on the render path this runs from.
        let original_failed_on_webgpu = |_: &greycard_ai::Model| {
            store
                .as_ref()
                .is_some_and(crate::ai::subject_original_failed_on_webgpu)
        };
        let sheet_free = st.fetch.is_none() && !st.fetching;
        let Some(next) = step(
            &shape,
            have,
            providers,
            &st.declined,
            &st.fetch_failed,
            sheet_free,
            original_failed_on_webgpu,
        ) else {
            continue;
        };
        if let Step::Ask(model) = next {
            WORKER.with(|w| {
                if let Some(w) = &*w.borrow() {
                    w.send(Job::Mask {
                        key,
                        shape: shape.clone(),
                        model: Some(model),
                    });
                }
            });
            st.asked.insert(key, shape);
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
        format!(
            "the GPU Subject model could not be fetched ({message}); the original is offered instead"
        )
    } else {
        let short = name.split(',').next().unwrap_or(name);
        format!("{short} could not be fetched: {message}")
    }
}

/// A tool has gone in hand: the mask shows while its shape is placed.
/// The toggle as it was comes back once the tool is down, unless it
/// is thrown meanwhile, when the choice stands.
pub(crate) fn tool_in_hand(st: &mut State, app: &App) {
    if st.show_mask_kept.is_none() {
        st.show_mask_kept = Some(app.get_show_mask());
    }
    app.set_show_mask(true);
}

/// How far the pointer moves, in the masks' units, before a stroke
/// takes another point: a texel of the raster.
pub(crate) const MIN_STEP: f32 = 1.0 / greycard_edit::brush::RASTER_WIDTH as f32;

/// What the status line says while a tool is in hand.
pub(crate) fn placing_hint(kind: &str) -> &'static str {
    match kind {
        "Brush" => "paint on the picture; scroll for size; Esc or the button when done",
        "Object" => {
            "click a thing, right-click what is not it, or drag a box round it; Esc or the button when done"
        }
        "Sky" => {
            "click sky the mask missed, right-click what is not sky; Esc or the button when done"
        }
        _ => "drag on the picture to place it",
    }
}

/// After a shape made at once: its word on the status line, and for a
/// color range the dropper in hand, so the next click on the picture
/// centers the window. Called with the state let go of, since putting
/// a dropper in hand puts any other tool down.
fn arm_range_pick(app: &App, kind: &str) {
    if kind == "Color" && app.get_picking() != "Range" {
        app.invoke_pick_started("Range".into());
    }
    app.set_status(made_hint(kind).into());
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    // The mask toggle thrown by hand while a tool is in hand: that
    // choice stands once the tool is down.
    {
        let state = state.clone();
        app.on_show_mask_toggled(move || {
            state.borrow_mut().show_mask_kept = None;
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
            let edit = read_edit(&app, &st.edit, st.target);
            st.edit = edit;
            let target = (i > 0).then(|| i as usize - 1);
            st.target = target.filter(|&t| t < st.edit.adjustments.len());
            app.set_component(0);
            show_edit(&st, &st.edit, &app, st.target);
            reveal_range(&st.edit, st.target, &app);
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
            if st.cull.is_some() {
                leave_cull(&mut st, &app, &worker, None);
                app.set_panel_tab(tab.clone());
            }
            if tab != "Masks" {
                if st.placing.take().is_some() {
                    app.set_placing("".into());
                    app.set_status("".into());
                }
                if st.target.is_some() {
                    let edit = read_edit(&app, &st.edit, st.target);
                    st.edit = edit;
                    st.target = None;
                    app.set_component(0);
                    show_edit(&st, &st.edit, &app, None);
                }
            }
            if tab != "Retouch" && st.retouching.take().is_some() {
                app.set_retouch_mode("".into());
                app.set_status("".into());
            }
            app.window().request_redraw();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_add_adjustment(move |kind| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // The button of the tool in hand puts it down.
            if app.get_placing() == kind && !app.get_placing_into() {
                st.placing = None;
                app.set_placing("".into());
                app.set_status("".into());
                return;
            }
            if st.edit.adjustments.len() >= MAX_LOCALS {
                app.set_status(format!("at most {MAX_LOCALS} adjustments").into());
                return;
            }
            // A subject needs no placing: the model finds it; nor does
            // a range, which is the picture's own.
            if made_at_once(kind.as_str()) {
                let edit = read_edit(&app, &st.edit, st.target);
                st.edit = edit;
                let id = st.edit.next_id();
                let shape = Shape::of_kind(kind.as_str());
                st.edit.adjustments.push(Adjustment {
                    id,
                    name: format!("{} {id}", shape.name()),
                    enabled: true,
                    mask: Mask {
                        components: vec![Component {
                            shape,
                            mode: Mode::Add,
                            invert: false,
                            enabled: true,
                        }],
                        invert: false,
                    },
                    look: Look::default(),
                });
                st.target = Some(st.edit.adjustments.len() - 1);
                st.placing = None;
                app.set_placing("".into());
                app.set_component(0);
                if kind != "Subject" && kind != "Sky" {
                    // Its sliders are the shape: show them.
                    app.set_shapes_open(true);
                }
                show_edit(&st, &st.edit, &app, st.target);
                drop(st);
                app.invoke_view_changed();
                arm_range_pick(&app, kind.as_str());
                return;
            }
            st.placing = Some(Placing {
                kind: Shape::of_kind(kind.as_str()),
                mode: Mode::Add,
                from: (0.0, 0.0),
                index: usize::MAX,
                component: None,
                fresh: true,
                boxed: false,
            });
            tool_in_hand(&mut st, &app);
            app.set_placing_into(false);
            app.set_placing(kind.clone());
            app.set_status(placing_hint(kind.as_str()).into());
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
            let Some(index) = st.target else {
                return;
            };
            if app.get_placing() == kind && app.get_placing_into() {
                st.placing = None;
                app.set_placing("".into());
                app.set_status("".into());
                return;
            }
            let chosen = app.get_component().max(0) as usize;
            let sky_chosen = kind == "Sky"
                && st
                    .edit
                    .adjustments
                    .get(index)
                    .and_then(|a| a.mask.components.get(chosen))
                    .is_some_and(|c| matches!(c.shape, Shape::Sky { .. }));
            if made_at_once(kind.as_str()) && !sky_chosen {
                let edit = read_edit(&app, &st.edit, st.target);
                st.edit = edit;
                let Some(a) = st.edit.adjustments.get_mut(index) else {
                    return;
                };
                a.mask.components.push(Component {
                    shape: Shape::of_kind(kind.as_str()),
                    mode: Mode::from_name(mode.as_str()).unwrap_or_default(),
                    invert: false,
                    enabled: true,
                });
                let which = a.mask.components.len() - 1;
                st.placing = None;
                app.set_placing("".into());
                app.set_shapes_open(true);
                app.set_component(which as i32);
                show_edit(&st, &st.edit, &app, st.target);
                drop(st);
                app.invoke_view_changed();
                arm_range_pick(&app, kind.as_str());
                return;
            }
            // A brush chosen in the list takes more strokes rather
            // than a new brush beside it; an object or a sky more picks.
            let component = st
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
            st.placing = Some(Placing {
                kind: Shape::of_kind(kind.as_str()),
                mode: Mode::from_name(mode.as_str()).unwrap_or_default(),
                from: (0.0, 0.0),
                index,
                component,
                fresh: false,
                boxed: false,
            });
            tool_in_hand(&mut st, &app);
            app.set_placing_into(true);
            app.set_shapes_open(true);
            app.set_placing(kind.clone());
            app.set_status(placing_hint(kind.as_str()).into());
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_stop_placing(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if st.placing.take().is_some() {
                app.set_placing("".into());
                app.set_status("".into());
            }
            if st.retouching.take().is_some() {
                app.set_retouch_mode("".into());
                app.set_status("".into());
            }
            st.picking = None;
            let was_defringe = app.get_picking() == "Defringe";
            if !app.get_picking().is_empty() {
                app.set_picking("".into());
                app.set_status("".into());
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
            let Some(i) = st.target else {
                return;
            };
            let edit = read_edit(&app, &st.edit, st.target);
            st.edit = edit;
            let c = c.max(0) as usize;
            if let Some(a) = st.edit.adjustments.get_mut(i)
                && c < a.mask.components.len()
            {
                a.mask.components.remove(c);
            }
            app.set_component(c.saturating_sub(1) as i32);
            show_edit(&st, &st.edit, &app, st.target);
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
            let Some(i) = st.target else {
                return;
            };
            let edit = read_edit(&app, &st.edit, st.target);
            st.edit = edit;
            if let Some(shape) = st
                .edit
                .adjustments
                .get_mut(i)
                .and_then(|a| a.mask.components.get_mut(c.max(0) as usize))
            {
                shape.enabled = !shape.enabled;
            }
            show_edit(&st, &st.edit, &app, st.target);
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
            let edit = read_edit(&app, &st.edit, st.target);
            st.edit = edit;
            if let Some(a) = st.edit.adjustments.get_mut(i.max(0) as usize) {
                a.enabled = !a.enabled;
            }
            show_edit(&st, &st.edit, &app, st.target);
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
            let edit = read_edit(&app, &st.edit, st.target);
            st.edit = edit;
            app.set_component(c);
            if let Some(a) = st.target.and_then(|i| st.edit.adjustments.get(i)) {
                show_component(&a.mask, &app);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_renamed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            let edit = read_edit(&app, &st.edit, st.target);
            show_names(&edit, &app);
            if let Some(a) = st.target.and_then(|i| edit.adjustments.get(i)) {
                app.set_target_name(a.name.as_str().into());
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
            let Some(i) = st.target else {
                return;
            };
            let edit = read_edit(&app, &st.edit, st.target);
            st.edit = edit;
            if i < st.edit.adjustments.len() {
                st.edit.adjustments.remove(i);
            }
            st.target = None;
            show_edit(&st, &st.edit, &app, None);
            drop(st);
            app.invoke_view_changed();
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
            if let Some(method) = st.retouching {
                let at = view_to_source(&st, &app, x, y);
                let patch = Patch {
                    id: st.edit.retouch.next_id(),
                    method,
                    points: vec![[at.0, at.1]],
                    radius: app.get_patch_size().clamp(0.002, 0.3),
                    feather: app.get_patch_feather().clamp(0.0, 1.0),
                    opacity: app.get_patch_opacity().clamp(0.0, 1.0),
                    source: None,
                };
                st.edit.retouch.patches.push(patch);
                let i = st.edit.retouch.patches.len() - 1;
                st.patch_stroke = Some(i);
                app.set_patch(i as i32);
                show_patches(&st.edit, &app);
                app.window().request_redraw();
                return;
            }
            let Some(mut placing) = st.placing.take() else {
                return;
            };
            let at = view_to_source(&st, &app, x, y);
            // The panel's look goes to its target before the new one
            // takes the panel.
            let edit = read_edit(&app, &st.edit, st.target);
            st.edit = edit;
            // Another stroke of a brush already placed, or another
            // pick for an object.
            if let Some(c) = placing.component
                && let Some(shape) = st
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
                st.placing = Some(placing);
                app.window().request_redraw();
                return;
            }
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
                | Shape::Sky { .. }
                | Shape::Luminance { .. }
                | Shape::Color { .. }
                | Shape::Unknown) => s.clone(),
            };
            let name = shape.name();
            let component = Component {
                shape,
                mode: placing.mode,
                invert: false,
                enabled: true,
            };
            let which;
            if placing.fresh {
                let id = st.edit.next_id();
                st.edit.adjustments.push(Adjustment {
                    id,
                    name: format!("{name} {id}"),
                    enabled: true,
                    mask: Mask {
                        components: vec![component],
                        invert: false,
                    },
                    look: Look::default(),
                });
                placing.index = st.edit.adjustments.len() - 1;
                which = 0;
                st.target = Some(placing.index);
            } else {
                let Some(a) = st.edit.adjustments.get_mut(placing.index) else {
                    app.set_placing("".into());
                    return;
                };
                a.mask.components.push(component);
                which = a.mask.components.len() - 1;
            }
            placing.component = Some(which);
            placing.from = at;
            placing.boxed = false;
            app.set_component(which as i32);
            st.placing = Some(placing);
            show_edit(&st, &st.edit, &app, st.target);
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
            if let Some(i) = st.patch_stroke {
                if let Some(p) = st.edit.retouch.patches.get_mut(i)
                    && p.points
                        .last()
                        .is_none_or(|l| (at.0 - l[0]).hypot(at.1 - l[1]) > MIN_STEP * 4.0)
                {
                    p.points.push([at.0, at.1]);
                }
                app.window().request_redraw();
                return;
            }
            let st = &mut *st;
            let Some(placing) = st.placing.as_mut() else {
                return;
            };
            let Some(which) = placing.component else {
                return;
            };
            let (from, index) = (placing.from, placing.index);
            let Some(component) = st
                .edit
                .adjustments
                .get_mut(index)
                .and_then(|a| a.mask.components.get_mut(which))
            else {
                return;
            };
            match &mut component.shape {
                Shape::Linear { to, .. } => *to = [at.0, at.1],
                Shape::Radial { radius, .. } => {
                    *radius = [
                        (at.0 - from.0).abs().max(MIN_RADIUS),
                        (at.1 - from.1).abs().max(MIN_RADIUS),
                    ]
                }
                Shape::Brush { strokes } => {
                    // A point once the pointer has moved a texel.
                    if let Some(stroke) = strokes.last_mut()
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
                | Shape::Sky { .. }
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
            if st.patch_stroke.take().is_some() {
                // The spot or stroke just finished does not stay
                // selected: the sliders and the wheel go back to
                // setting the brush for the next repair, not resizing
                // the one just drawn. A deliberate click on it in the
                // list selects it again.
                app.set_patch(-1);
                show_patches(&st.edit, &app);
                WORKER.with(|w| {
                    if let Some(w) = &*w.borrow() {
                        develop_soon(&mut st, state.clone(), w.clone(), app.as_weak());
                    }
                });
                return;
            }
            let Some(mut placing) = st.placing.take() else {
                return;
            };
            let Some(which) = placing.component else {
                // A release with no press to it.
                st.placing = Some(placing);
                return;
            };
            // A brush stays in hand for the next stroke; a dab is a
            // stroke. An object or a sky stays in hand for the next pick.
            if placing.kind.is_brush()
                || matches!(placing.kind, Shape::Object { .. } | Shape::Sky { .. })
            {
                placing.fresh = false;
                placing.boxed = false;
                st.placing = Some(placing);
                drop(st);
                app.invoke_view_changed();
                return;
            }
            app.set_placing("".into());
            // A press without a drag makes nothing.
            let drawn = st
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
                    | Shape::Sky { .. }
                    | Shape::Object { .. }
                    | Shape::Luminance { .. }
                    | Shape::Color { .. }
                    | Shape::Unknown => true,
                });
            if drawn {
                app.set_status("".into());
            } else if placing.fresh && placing.index < st.edit.adjustments.len() {
                st.edit.adjustments.remove(placing.index);
                st.target = None;
                show_edit(&st, &st.edit, &app, None);
                app.set_status("nothing placed".into());
            } else if let Some(a) = st.edit.adjustments.get_mut(placing.index)
                && which < a.mask.components.len()
            {
                a.mask.components.remove(which);
                app.set_component(which.saturating_sub(1) as i32);
                show_edit(&st, &st.edit, &app, st.target);
                app.set_status("nothing placed".into());
            }
            drop(st);
            app.invoke_view_changed();
        });
    }
    // A shape's handles: the shape as it was when pressed, moved by the
    // pointer's travel since; release records the edit.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_mask_grabbed(move |handle| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(shape) = st
                .target
                .and_then(|i| st.edit.adjustments.get(i))
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
            st.mask_drag = Some(MaskDrag { shape, handle, at });
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_mask_dragged(move |_, dx, dy| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some((shape, handle, at)) = st
                .mask_drag
                .as_ref()
                .map(|d| (d.shape.clone(), d.handle, d.at))
            else {
                return;
            };
            let p = view_to_source(&st, &app, at.0 + dx, at.1 + dy);
            let moved = shape.dragged(handle, p);
            let which = app.get_component().max(0) as usize;
            if let Some(c) = st
                .target
                .and_then(|i| st.edit.adjustments.get_mut(i))
                .and_then(|a| a.mask.components.get_mut(which))
            {
                c.shape = moved;
            }
            app.window().request_redraw();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_mask_dropped(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if state.borrow_mut().mask_drag.take().is_some() {
                app.invoke_view_changed();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::{click, window};
    use std::cell::Cell;

    /// The left bar: the navigator, the snapshots and the history,
    /// and the width the viewport begins at.
    const LEFT_BAR: f32 = 240.0;

    /// A Subject on a WebGPU machine with neither model: the GPU file
    /// is offered; its fetch fails (faked here), and the original is
    /// offered next; declining the original leaves the shape waiting;
    /// the original arriving is asked for. Declining the GPU file also
    /// turns to the original rather than blocking it.
    #[test]
    fn a_subject_turns_to_the_original_when_the_gpu_model_cannot_be_had() {
        use greycard_ai::{Provider, SUBJECT, SUBJECT_WEBGPU};
        let gpu = [Provider::WebGpu, Provider::Cpu];
        let shape = Shape::Subject {};
        let nothing = |_: &greycard_ai::Model| false;
        // Nothing in the store yet: the record cannot say the original
        // failed on WebGPU, since it has never been tried.
        let no_record = nothing;
        let (mut declined, mut failed) = (Vec::new(), Vec::new());

        assert_eq!(
            step(&shape, nothing, &gpu, &declined, &failed, true, no_record),
            Some(Step::Offer(&SUBJECT_WEBGPU, false))
        );
        // The sheet is up or a fetch is on its way: wait.
        assert_eq!(
            step(&shape, nothing, &gpu, &declined, &failed, false, no_record),
            Some(Step::Wait)
        );
        let status = fetch_failed(&mut failed, SUBJECT_WEBGPU.id, SUBJECT_WEBGPU.name, "404");
        assert!(
            status.contains("GPU Subject model could not be fetched"),
            "{status}"
        );
        assert!(status.contains("original"), "{status}");
        assert_eq!(
            step(&shape, nothing, &gpu, &declined, &failed, true, no_record),
            Some(Step::Offer(&SUBJECT, false))
        );
        // Offline, the original fails too: no third offer.
        {
            let mut failed = failed.clone();
            fetch_failed(&mut failed, SUBJECT.id, SUBJECT.name, "offline");
            assert_eq!(
                step(&shape, nothing, &gpu, &declined, &failed, true, no_record),
                Some(Step::Wait)
            );
            // Once it is in the store after all, it is asked for.
            let original = |m: &greycard_ai::Model| m.id == SUBJECT.id;
            assert_eq!(
                step(&shape, original, &gpu, &declined, &failed, true, no_record),
                Some(Step::Ask(&SUBJECT))
            );
        }
        declined.push(SUBJECT.id);
        assert_eq!(
            step(&shape, nothing, &gpu, &declined, &failed, true, no_record),
            Some(Step::Wait)
        );
        let original = |m: &greycard_ai::Model| m.id == SUBJECT.id;
        assert_eq!(
            step(&shape, original, &gpu, &declined, &failed, true, no_record),
            Some(Step::Ask(&SUBJECT))
        );

        // Declined, not failed: the same turn to the original.
        let (declined, failed) = (vec![SUBJECT_WEBGPU.id], Vec::new());
        assert_eq!(
            step(&shape, nothing, &gpu, &declined, &failed, true, no_record),
            Some(Step::Offer(&SUBJECT, false))
        );
        // And on a CPU-only machine the original from the start.
        assert_eq!(
            step(&shape, nothing, &[Provider::Cpu], &[], &[], true, no_record),
            Some(Step::Offer(&SUBJECT, false))
        );
    }

    /// The original already in the store, and on record as failing on
    /// WebGPU for this adapter: the rewrite is offered, and the offer
    /// says why. Declining it leaves the original in use, not waiting.
    #[test]
    fn a_subject_already_in_the_store_offers_the_rewrite_when_it_falls_back() {
        use greycard_ai::{Provider, SUBJECT, SUBJECT_WEBGPU};
        let gpu = [Provider::WebGpu, Provider::Cpu];
        let shape = Shape::Subject {};
        let original = |m: &greycard_ai::Model| m.id == SUBJECT.id;
        let failed_on_webgpu = |_: &greycard_ai::Model| true;

        assert_eq!(
            step(&shape, original, &gpu, &[], &[], true, failed_on_webgpu),
            Some(Step::Offer(&SUBJECT_WEBGPU, true)),
            "the store's original falls back to the CPU here: the rewrite is offered"
        );
        // Declined: the original stands, asked for rather than waited on.
        assert_eq!(
            step(
                &shape,
                original,
                &gpu,
                &[SUBJECT_WEBGPU.id],
                &[],
                true,
                failed_on_webgpu
            ),
            Some(Step::Ask(&SUBJECT))
        );
        // Nothing on record: the original stands from the start.
        let no_record = |_: &greycard_ai::Model| false;
        assert_eq!(
            step(&shape, original, &gpu, &[], &[], true, no_record),
            Some(Step::Ask(&SUBJECT))
        );
        // Another sheet up, or a fetch already running: nothing can be
        // offered this round, but the original already runs, so it is
        // asked for rather than left waiting on an offer that is not
        // coming.
        assert_eq!(
            step(&shape, original, &gpu, &[], &[], false, failed_on_webgpu),
            Some(Step::Ask(&SUBJECT))
        );
    }

    /// A Sky wants the prior, then SAM: each offered in turn, and asked
    /// for once both are in. SAM declined or failing leaves the prior
    /// alone asked for, its own labels the outline.
    #[test]
    fn a_sky_is_offered_the_prior_then_sam_and_runs_without_sam_if_it_must() {
        use greycard_ai::{Provider, SAM, SKY, SUBJECT};
        let gpu = [Provider::WebGpu, Provider::Cpu];
        let shape = Shape::Sky { picks: Vec::new() };
        let no_record = |_: &greycard_ai::Model| false;
        let nothing = |_: &greycard_ai::Model| false;
        assert_eq!(
            step(&shape, nothing, &gpu, &[], &[], true, no_record),
            Some(Step::Offer(&SKY, false))
        );
        let prior = |m: &greycard_ai::Model| m.id == SKY.id;
        assert_eq!(
            step(&shape, prior, &gpu, &[], &[], true, no_record),
            Some(Step::Offer(&SAM, false))
        );
        let both = |m: &greycard_ai::Model| m.id == SKY.id || m.id == SAM.id;
        assert_eq!(
            step(&shape, both, &gpu, &[], &[], true, no_record),
            Some(Step::Ask(&SKY))
        );
        assert_eq!(
            step(&shape, prior, &gpu, &[SAM.id], &[], true, no_record),
            Some(Step::Ask(&SKY))
        );
        assert_eq!(
            step(&shape, prior, &gpu, &[], &[SAM.id], true, no_record),
            Some(Step::Ask(&SKY))
        );
        // The prior declined: the shape waits, whatever else the store
        // has; the Subject original is never asked for in its place.
        let subject_too = |m: &greycard_ai::Model| m.id == SUBJECT.id || m.id == SAM.id;
        assert_eq!(
            step(&shape, subject_too, &gpu, &[SKY.id], &[], true, no_record),
            Some(Step::Wait)
        );
        assert_eq!(
            step(&shape, subject_too, &gpu, &[], &[], false, no_record),
            Some(Step::Wait)
        );
    }

    /// An Object waiting on SAM with the sheet busy waits: the Subject
    /// original in the store is not asked for in SAM's place.
    #[test]
    fn an_object_waiting_on_sam_is_not_given_the_subject_model() {
        use greycard_ai::{Provider, SUBJECT};
        let shape = Shape::Object {
            picks: vec![Pick {
                pos: [0.5, 0.5],
                positive: true,
            }],
            boxes: Vec::new(),
        };
        let subject = |m: &greycard_ai::Model| m.id == SUBJECT.id;
        assert_eq!(
            step(
                &shape,
                subject,
                &[Provider::Cpu],
                &[],
                &[],
                false,
                |_: &greycard_ai::Model| false
            ),
            Some(Step::Wait)
        );
    }

    /// The sheet's note says who publishes the model.
    #[test]
    fn the_offer_says_who_publishes_the_model() {
        let ours = crate::panel::assets::model_note(&greycard_ai::SUBJECT_WEBGPU);
        assert!(ours.contains("greycard publishes this copy"), "{ours}");
        assert!(ours.contains("MIT"), "{ours}");
        let theirs = crate::panel::assets::model_note(&greycard_ai::SUBJECT);
        assert!(theirs.contains("does not ship"), "{theirs}");
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
            app.on_mask_grabbed(move |_| grabbed.set(true));
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
        app.invoke_add_adjustment("Sky".into());
        {
            let st = state.borrow();
            assert_eq!(st.target, Some(0));
            assert_eq!(
                st.edit.adjustments[0].mask.components,
                vec![Component {
                    shape: Shape::Sky { picks: Vec::new() },
                    mode: Mode::Add,
                    invert: false,
                    enabled: true,
                }]
            );
            assert!(st.placing.is_none());
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
            let components = &st.edit.adjustments[0].mask.components;
            assert_eq!(components.len(), 1, "a pick is not a second sky");
            let Shape::Sky { picks } = &components[0].shape else {
                panic!("{:?}", components[0].shape)
            };
            assert_eq!(
                picks.iter().map(|p| p.positive).collect::<Vec<_>>(),
                vec![true, false]
            );
            assert!(st.placing.is_some(), "the tool stays in hand");
        }
        app.invoke_add_shape("Sky".into(), "Add".into());
        assert!(app.get_placing().is_empty());
        assert!(state.borrow().placing.is_none());
    }

    /// The range masks from the panel: a button makes one whole, its
    /// sliders are its window both ways, a color one puts the dropper
    /// in hand, and Skin puts the window at the skin hue.
    #[test]
    fn the_range_masks_come_from_their_buttons_and_live_in_their_sliders() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_panel_tab("Masks".into());
        app.invoke_add_adjustment("Luminance".into());
        {
            let st = state.borrow();
            assert_eq!(st.target, Some(0));
            assert_eq!(
                st.edit.adjustments[0].mask.components[0].shape,
                Shape::LUMINANCE
            );
        }
        assert_eq!(app.get_component_kind(), "Luminance");
        assert!(app.get_shapes_open());
        assert!(app.get_picking().is_empty(), "no dropper for a luminance");
        assert_eq!(app.get_lum_low(), 0.7);
        // The sliders are the shape.
        app.set_lum_low(0.5);
        app.set_lum_high_feather(0.2);
        let read = |app: &App| {
            let st = state.borrow();
            read_edit(app, &st.edit, st.target)
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
}
