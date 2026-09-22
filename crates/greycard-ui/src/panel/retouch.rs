use crate::panel::assets::offer_model;
use crate::panel::edit::develop_soon;
use crate::panel::viewport::{ViewMap, source_to_view, view_to_source};
use crate::*;

/// A patch's shape in the picture's units: the rings of its edge and
/// of its feather's start, for the points, radius and feather they
/// were contoured for.
pub(crate) struct PatchShape {
    key: (Vec<greycard_edit::mask::Pos>, f32, f32),
    edge: Vec<Vec<(f32, f32)>>,
    feather: Vec<Vec<(f32, f32)>>,
}

/// A patch's edge on the view and where its feather begins, as a
/// Path's commands: the rings contoured in the picture's units (kept
/// in `shape` until the patch changes), each point mapped as the
/// handles are, so it turns, zooms and pans with the picture.
pub(crate) fn patch_outlines(
    shape: &mut Option<PatchShape>,
    patch: &Patch,
    map: &ViewMap,
) -> (String, String) {
    let key = (patch.points.clone(), patch.radius, patch.feather);
    if shape.as_ref().is_none_or(|s| s.key != key) {
        let points: Vec<(f32, f32)> = patch.points.iter().map(|p| (p[0], p[1])).collect();
        // Cells a quarter of the radius contoured, so a spot is a
        // polygon of two dozen sides or so.
        let rings = |r: f32| outline::contours(&points, r, r / 4.0);
        // The feather's start as the engine draws it: the radius
        // less the feather, and never quite nothing.
        let feather = if patch.feather > 0.01 {
            rings(patch.radius * (1.0 - patch.feather.clamp(0.0, 1.0)).max(0.02))
        } else {
            Vec::new()
        };
        *shape = Some(PatchShape {
            key,
            edge: rings(patch.radius),
            feather,
        });
    }
    let shape = shape.as_ref().expect("just made");
    let mapped = |rings: &[Vec<(f32, f32)>]| {
        let on_view: Vec<Vec<(f32, f32)>> = rings
            .iter()
            .map(|ring| ring.iter().map(|&(u, v)| map.to_view(u, v)).collect())
            .collect();
        outline::commands(&on_view)
    };
    (mapped(&shape.edge), mapped(&shape.feather))
}

/// The patches' names on the panel, and the chosen one's values on
/// its sliders.
pub(crate) fn show_patches(edit: &Edit, app: &App) {
    let names: Vec<slint::SharedString> = edit
        .retouch
        .patches
        .iter()
        .map(|p| p.name().into())
        .collect();
    app.set_patch_names(ModelRc::new(VecModel::from(names)));
    let chosen = usize::try_from(app.get_patch())
        .ok()
        .filter(|&i| i < edit.retouch.patches.len());
    app.set_patch(chosen.map_or(-1, |i| i as i32));
    if let Some(p) = chosen.and_then(|i| edit.retouch.patches.get(i)) {
        app.set_patch_size(p.radius);
        app.set_patch_feather(p.feather);
        app.set_patch_opacity(p.opacity);
    }
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    // The repair tools: heal or clone in hand, a patch chosen, its
    // sliders, its handles, its deletion.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_retouch_toggled(move |kind| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let method = RetouchMethod::from_name(kind.as_str());
            if method.is_none() || st.retouching == method {
                st.retouching = None;
                app.set_retouch_mode("".into());
                app.set_status("".into());
                return;
            }
            // A fill needs its model; offer it first.
            if method == Some(RetouchMethod::Fill)
                && !st.store.as_ref().is_some_and(|s| s.have(&greycard_ai::FILL))
            {
                if !st.fetching {
                    offer_model(&mut st, &app, &greycard_ai::FILL);
                }
                return;
            }
            st.retouching = method;
            st.placing = None;
            app.set_placing("".into());
            app.set_retouch_mode(kind);
            app.set_status(
                "click a spot or drag a stroke to repair; scroll for size; drag the pins to move it or its source; Esc or the button when done"
                    .into(),
            );
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_patch_changed(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            app.set_patch(i);
            show_patches(&st.edit, &app);
            app.window().request_redraw();
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_delete_patch(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Ok(i) = usize::try_from(app.get_patch()) else {
                return;
            };
            if i < st.edit.retouch.patches.len() {
                st.edit.retouch.patches.remove(i);
                app.set_patch(i as i32 - 1);
                show_patches(&st.edit, &app);
                develop_soon(&mut st, state.clone(), worker.clone(), app.as_weak());
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_patch_edited(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Ok(i) = usize::try_from(app.get_patch()) else {
                return;
            };
            if let Some(p) = st.edit.retouch.patches.get_mut(i) {
                p.radius = app.get_patch_size().clamp(0.002, 0.3);
                p.feather = app.get_patch_feather().clamp(0.0, 1.0);
                p.opacity = app.get_patch_opacity().clamp(0.0, 1.0);
                develop_soon(&mut st, state.clone(), worker.clone(), app.as_weak());
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_patch_grabbed(move |handle| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(patch) = usize::try_from(app.get_patch())
                .ok()
                .and_then(|i| st.edit.retouch.patches.get(i))
                .cloned()
            else {
                return;
            };
            let c = patch.center();
            let (u, v) = match (handle, patch.source) {
                (0, _) => (c[0], c[1]),
                (_, Some(s)) => (c[0] + s[0], c[1] + s[1]),
                _ => return,
            };
            let at = source_to_view(&st, &app, u, v);
            st.patch_drag = Some((patch, handle.max(0) as usize, at));
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_patch_dragged(move |_, dx, dy| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some((grabbed, handle, at)) = st.patch_drag.clone() else {
                return;
            };
            let p = view_to_source(&st, &app, at.0 + dx, at.1 + dy);
            let c = grabbed.center();
            let moved = if handle == 0 {
                let first = grabbed.points.first().copied().unwrap_or(c);
                grabbed.moved_to([first[0] + p.0 - c[0], first[1] + p.1 - c[1]])
            } else {
                Patch {
                    source: Some([p.0 - c[0], p.1 - c[1]]),
                    ..grabbed
                }
            };
            if let Some(slot) = usize::try_from(app.get_patch())
                .ok()
                .and_then(|i| st.edit.retouch.patches.get_mut(i))
            {
                *slot = moved;
            }
            app.window().request_redraw();
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_patch_dropped(move || {
            let mut st = state.borrow_mut();
            if st.patch_drag.take().is_some() {
                develop_soon(&mut st, state.clone(), worker.clone(), app_weak.clone());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{press, retouch_state, window};
    use slint::platform::Key;

    /// A stroke just drawn does not stay selected: the sliders and the
    /// wheel go back to setting the brush for the next repair, and
    /// only choosing a repair from the list edits it again.
    #[test]
    fn changing_the_brush_size_after_a_stroke_leaves_the_stroke_alone() {
        let app = window(1);
        let (state, _worker) = retouch_state(&app);

        app.invoke_retouch_toggled("Heal".into());
        assert_eq!(app.get_retouch_mode(), "Heal");
        assert_eq!(app.get_patch(), -1, "the brush is in hand, nothing chosen");

        // A first stroke, drawn and released.
        app.invoke_place_pressed(10.0, 10.0, false);
        app.invoke_place_dragged(20.0, 10.0);
        app.invoke_place_released();
        assert_eq!(
            app.get_patch(),
            -1,
            "the stroke just drawn does not stay selected"
        );
        assert_eq!(state.borrow().edit.retouch.patches.len(), 1);
        let first_radius = state.borrow().edit.retouch.patches[0].radius;

        // The brush size changed for the next repair, with nothing
        // selected: the first stroke is untouched.
        app.set_patch_size(0.2);
        app.invoke_patch_edited();
        assert_eq!(
            state.borrow().edit.retouch.patches[0].radius,
            first_radius,
            "a size change with nothing selected must not touch the last stroke"
        );

        // A second stroke takes the brush's new size.
        app.invoke_place_pressed(30.0, 10.0, false);
        app.invoke_place_dragged(40.0, 10.0);
        app.invoke_place_released();
        assert_eq!(state.borrow().edit.retouch.patches.len(), 2);
        assert_eq!(state.borrow().edit.retouch.patches[1].radius, 0.2);

        // Choosing the first stroke from the list selects it: the
        // sliders show its own size, and changing it now does edit
        // it, as choosing a repair has always done.
        app.invoke_patch_changed(0);
        assert_eq!(app.get_patch(), 0);
        assert_eq!(app.get_patch_size(), first_radius);
        app.set_patch_size(0.05);
        app.invoke_patch_edited();
        assert_eq!(state.borrow().edit.retouch.patches[0].radius, 0.05);
        assert_eq!(
            state.borrow().edit.retouch.patches[1].radius,
            0.2,
            "editing the first stroke leaves the second alone"
        );
    }

    /// Esc lets go of a repair chosen from the list, the same way a
    /// click on the empty picture does (both fire `patch-changed(-1)`
    /// in app.slint).
    #[test]
    fn escape_lets_go_of_a_chosen_repair() {
        let app = window(1);
        let (state, _worker) = retouch_state(&app);

        app.invoke_retouch_toggled("Heal".into());
        app.invoke_place_pressed(10.0, 10.0, false);
        app.invoke_place_released();
        assert_eq!(state.borrow().edit.retouch.patches.len(), 1);
        assert_eq!(app.get_patch(), -1);

        app.invoke_patch_changed(0);
        assert_eq!(app.get_patch(), 0, "chosen from the list");
        press(&app, Key::Escape);
        assert_eq!(app.get_patch(), -1, "Esc lets go of it");
    }
}
