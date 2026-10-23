use crate::panel::assets::offer_model;
use crate::panel::edit::develop_soon;
use crate::panel::viewport::{ViewMap, source_to_view, view_to_source};
use crate::*;

/// What the status line says while a repair tool is in hand.
pub(crate) const RETOUCH_HINT: &str =
    "click or drag to repair; scroll for size; drag pins to move; Esc to finish";

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

/// The patches' names on the panel, the chosen one's values on its
/// sliders, and whether its row has "Another": a Fill the generative
/// remove made, by what the last develop said ([`took_engines`]).
pub(crate) fn show_patches(st: &State, edit: &Edit, app: &App) {
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
    let chosen = chosen.and_then(|i| edit.retouch.patches.get(i));
    if let Some(p) = chosen {
        app.set_patch_size(p.radius);
        app.set_patch_feather(p.feather);
        app.set_patch_opacity(p.opacity);
    }
    app.set_patch_varies(chosen.is_some_and(|p| made_by_remove(st, p)));
}

/// Whether `patch` is a Fill the generative remove made.
fn made_by_remove(st: &State, patch: &Patch) -> bool {
    patch.method == RetouchMethod::Fill
        && patch.remove
        && st.develop.fill_engines.get(&patch.id) == Some(&crate::ai::Engine::Remove)
}

/// A develop's word on which engine made each Fill patch, kept for the
/// rows: true when that changed what any row shows.
pub(crate) fn took_engines(st: &mut State, engines: &[(u64, crate::ai::Engine)]) -> bool {
    let mut changed = false;
    for &(id, engine) in engines {
        changed |= st.develop.fill_engines.insert(id, engine) != Some(engine);
    }
    changed
}

/// Whether choosing Fill offers the generative remove's model: LaMa
/// is in the store and the remove's is not, the machine has WebGPU,
/// and it has not been offered or declined this session.
pub(crate) fn offers_remove(have_fill: bool, have_remove: bool, webgpu: bool, asked: bool) -> bool {
    have_fill && !have_remove && webgpu && !asked
}

/// The chosen Fill's next variation: its seed moved on, which the
/// generative remove makes another fill of, recorded as the patch's
/// other edits are. The fills of earlier seeds stay kept, so an undo
/// is free.
pub(crate) fn another(st: &mut State, app: &App) -> bool {
    let Some(i) = usize::try_from(app.get_patch()).ok() else {
        return false;
    };
    let Some(p) = st.develop.edit.retouch.patches.get(i) else {
        return false;
    };
    if p.method != RetouchMethod::Fill || !p.remove {
        return false;
    }
    let p = &mut st.develop.edit.retouch.patches[i];
    p.seed = p.seed.wrapping_add(1);
    true
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
            if method.is_none() || st.develop.retouching == method {
                st.develop.retouching = None;
                app.set_retouch_mode("".into());
                app.set_status("".into());
                return;
            }
            // A fill needs its model; offer it first.
            let have =
                |m: &greycard_ai::Model| st.develop.store.as_ref().is_some_and(|s| s.have(m));
            let (have_fill, have_remove) = (have(&greycard_ai::FILL), have(&greycard_ai::REMOVE));
            if method == Some(RetouchMethod::Fill) && !have_fill {
                if !st.sheets.fetching {
                    offer_model(&mut st, &app, &greycard_ai::FILL, false);
                }
                return;
            }
            // With LaMa in hand, the generative remove is offered once
            // on a machine that can run it; the tool is in hand
            // meanwhile, and declining it keeps LaMa.
            let asked = st.sheets.remove_offered
                || st.sheets.fetching
                || st.sheets.fetch.is_some()
                || st.sheets.declined.contains(&greycard_ai::REMOVE.id);
            let offer = method == Some(RetouchMethod::Fill)
                && offers_remove(have_fill, have_remove, crate::ai::on_webgpu(), asked);
            st.develop.retouching = method;
            st.develop.placing = None;
            app.set_placing("".into());
            app.set_retouch_mode(kind);
            app.set_status(RETOUCH_HINT.into());
            if offer {
                st.sheets.remove_offered = true;
                offer_model(&mut st, &app, &greycard_ai::REMOVE, false);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_patch_another(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if another(&mut st, &app) {
                develop_soon(&mut st, state.clone(), worker.clone(), app.as_weak());
            }
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
            show_patches(&st, &st.develop.edit, &app);
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
            if i < st.develop.edit.retouch.patches.len() {
                st.develop.edit.retouch.patches.remove(i);
                app.set_patch(i as i32 - 1);
                show_patches(&st, &st.develop.edit, &app);
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
            if let Some(p) = st.develop.edit.retouch.patches.get_mut(i) {
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
                .and_then(|i| st.develop.edit.retouch.patches.get(i))
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
            st.develop.patch_drag = Some((patch, handle.max(0) as usize, at));
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_patch_dragged(move |_, dx, dy| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some((grabbed, handle, at)) = st.develop.patch_drag.clone() else {
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
                .and_then(|i| st.develop.edit.retouch.patches.get_mut(i))
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
            if st.develop.patch_drag.take().is_some() {
                develop_soon(&mut st, state.clone(), worker.clone(), app_weak.clone());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::ai::Engine;
    use crate::testing::{press, retouch_state, window};
    use crate::*;
    use slint::platform::Key;

    /// A develop landing for the state developing now, with which
    /// engine made each Fill.
    fn land(app: &App, state: &Rc<RefCell<State>>, engines: Vec<(u64, Engine)>) {
        let generation = state.borrow().develop.generation;
        crate::panel::deliver::deliver(
            app,
            crate::worker::Outcome::Developed {
                generation,
                turn: 0,
                kind: crate::finish::Source::SCENE,
                image: crate::worker::Developed::Halves(std::sync::Arc::new(
                    crate::worker::Halves {
                        width: 60,
                        height: 40,
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
                fills: crate::worker::FillReport {
                    engines,
                    ..Default::default()
                },
            },
        );
    }

    fn save(app: &App, state: &Rc<RefCell<State>>) {
        let mut st = state.borrow_mut();
        let edit = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
        crate::panel::edit::save_edit(&mut st, edit);
    }

    /// "Another" is on the row of a Fill the generative remove made
    /// and no other; it moves the seed on as a step of the history,
    /// which an undo takes back.
    #[test]
    fn another_moves_the_seed_on_through_the_history() {
        let app = window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        {
            let mut st = state.borrow_mut();
            let fill = |id, remove| Patch {
                id,
                method: RetouchMethod::Fill,
                points: vec![[0.5, 0.5]],
                remove,
                ..Default::default()
            };
            st.develop.edit.retouch.patches = vec![fill(1, true), fill(2, false)];
        }
        save(&app, &state);
        land(&app, &state, vec![(1, Engine::Remove), (2, Engine::Lama)]);

        app.invoke_patch_changed(1);
        assert!(!app.get_patch_varies(), "LaMa's fill has no variations");
        app.invoke_patch_changed(0);
        assert!(app.get_patch_varies());

        let steps = state.borrow().sidecars[0].states();
        app.invoke_patch_another();
        assert_eq!(state.borrow().develop.edit.retouch.patches[0].seed, 1);
        assert_eq!(state.borrow().develop.edit.retouch.patches[1].seed, 0);
        save(&app, &state);
        assert_eq!(state.borrow().sidecars[0].states(), steps + 1);
        assert_eq!(
            state.borrow().sidecars[0].current.retouch.patches[0].seed,
            1
        );
        app.invoke_patch_another();
        save(&app, &state);
        assert_eq!(
            state.borrow().sidecars[0].current.retouch.patches[0].seed,
            2
        );

        app.invoke_undo();
        assert_eq!(state.borrow().develop.edit.retouch.patches[0].seed, 1);
        app.invoke_undo();
        assert_eq!(state.borrow().develop.edit.retouch.patches[0].seed, 0);

        // A Fill the remove was asked for but LaMa made (its window
        // past the remove's) has none either.
        land(&app, &state, vec![(1, Engine::Lama)]);
        app.invoke_patch_changed(0);
        assert!(!app.get_patch_varies());
    }

    /// The seed is kept in the sidecar, and a sidecar from before it
    /// reads as seed 0; at 0, and a patch LaMa makes, nothing new is
    /// written, so no picture kept under an older edit's key is lost.
    #[test]
    fn the_seed_round_trips_and_defaults_to_zero() {
        let mut edit = greycard_edit::Edit::default();
        edit.retouch.patches.push(Patch {
            id: 4,
            method: RetouchMethod::Fill,
            points: vec![[0.25, 0.5]],
            seed: 3,
            remove: true,
            ..Default::default()
        });
        let back = greycard_edit::Edit::from_json(&edit.to_json()).unwrap();
        assert_eq!(back.retouch.patches[0].seed, 3);
        assert!(back.retouch.patches[0].remove);
        assert_eq!(back, edit);

        let sidecar = greycard_edit::Sidecar {
            line: greycard_edit::Line {
                current: edit.clone(),
                ..Default::default()
            },
            ..Default::default()
        };
        let read = greycard_edit::Sidecar::from_json(&sidecar.to_json()).unwrap();
        assert_eq!(read.current.retouch.patches[0].seed, 3);

        edit.retouch.patches[0].seed = 0;
        edit.retouch.patches[0].remove = false;
        let json = edit.to_json();
        assert!(!json.contains("\"seed\""), "{json}");
        assert!(!json.contains("\"remove\""), "{json}");
        // As an older build wrote it.
        let old = json.replace("\"seed\":3,", "");
        let back = greycard_edit::Edit::from_json(&old).unwrap();
        assert_eq!(back.retouch.patches[0].seed, 0);
        assert!(!back.retouch.patches[0].remove);
    }

    /// The remove's model is offered with LaMa in hand, on WebGPU, once
    /// a session; never without WebGPU or LaMa, or with it in the store.
    #[test]
    fn the_remove_is_offered_once_with_webgpu_and_never_without() {
        use super::offers_remove;
        for have_fill in [false, true] {
            for have_remove in [false, true] {
                for webgpu in [false, true] {
                    for asked in [false, true] {
                        assert_eq!(
                            offers_remove(have_fill, have_remove, webgpu, asked),
                            have_fill && !have_remove && webgpu && !asked,
                            "{have_fill} {have_remove} {webgpu} {asked}"
                        );
                    }
                }
            }
        }

        // Through the panel, with LaMa in a store of its own.
        let dir = crate::testing::scratch_dir("remove-offer");
        let store = greycard_ai::Store::at(&dir);
        for f in greycard_ai::FILL.files {
            let path = store.path(&greycard_ai::FILL, f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::File::create(&path)
                .unwrap()
                .set_len(f.bytes)
                .unwrap();
        }
        assert!(store.have(&greycard_ai::FILL));
        let fill = |app: &App| {
            app.invoke_retouch_toggled("Fill".into());
            assert_eq!(app.get_retouch_mode(), "Fill", "the tool is in hand");
            let offered = app.get_fetch_open();
            app.invoke_retouch_toggled("Fill".into());
            offered
        };
        let ran = greycard_ai::runtime::status().is_ok();

        // No WebGPU: never.
        crate::ai::set_test_webgpu(Some(false));
        let app = window(1);
        let (state, _worker) = retouch_state(&app);
        state.borrow_mut().develop.store = Some(store.clone());
        assert!(!fill(&app));
        assert!(!fill(&app));
        assert!(!state.borrow().sheets.remove_offered);

        // WebGPU: offered the first time, declined, and not again.
        crate::ai::set_test_webgpu(Some(true));
        let app = window(1);
        let (state, _worker) = retouch_state(&app);
        state.borrow_mut().develop.store = Some(store.clone());
        app.invoke_retouch_toggled("Fill".into());
        assert_eq!(app.get_retouch_mode(), "Fill");
        assert!(state.borrow().sheets.remove_offered);
        if ran {
            assert!(app.get_fetch_open());
            assert_eq!(
                state.borrow().sheets.fetch,
                Some(crate::Fetch::Model(&greycard_ai::REMOVE))
            );
            let mb = (greycard_ai::REMOVE.bytes() + 500_000) / 1_000_000;
            let text = app.get_fetch_text().to_string();
            assert!(text.contains(&format!("{mb} MB")), "{text}");
            assert!(app.get_fetch_text().contains("RAIL++-M"));
            app.invoke_fetch_answered(false);
            assert!(
                state
                    .borrow()
                    .sheets
                    .declined
                    .contains(&greycard_ai::REMOVE.id)
            );
            assert_eq!(
                app.get_retouch_mode(),
                "Fill",
                "declining keeps LaMa's Fill"
            );
            assert_eq!(app.get_status(), super::RETOUCH_HINT);
        }
        app.invoke_retouch_toggled("Fill".into());
        assert!(!fill(&app), "offered once");
        crate::ai::set_test_webgpu(None);
        crate::testing::remove_scratch(state, &dir);
    }

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
        assert_eq!(state.borrow().develop.edit.retouch.patches.len(), 1);
        let first_radius = state.borrow().develop.edit.retouch.patches[0].radius;

        // The brush size changed for the next repair, with nothing
        // selected: the first stroke is untouched.
        app.set_patch_size(0.2);
        app.invoke_patch_edited();
        assert_eq!(
            state.borrow().develop.edit.retouch.patches[0].radius,
            first_radius,
            "a size change with nothing selected must not touch the last stroke"
        );

        // A second stroke takes the brush's new size.
        app.invoke_place_pressed(30.0, 10.0, false);
        app.invoke_place_dragged(40.0, 10.0);
        app.invoke_place_released();
        assert_eq!(state.borrow().develop.edit.retouch.patches.len(), 2);
        assert_eq!(state.borrow().develop.edit.retouch.patches[1].radius, 0.2);

        // Choosing the first stroke from the list selects it: the
        // sliders show its own size, and changing it now does edit
        // it, as choosing a repair has always done.
        app.invoke_patch_changed(0);
        assert_eq!(app.get_patch(), 0);
        assert_eq!(app.get_patch_size(), first_radius);
        app.set_patch_size(0.05);
        app.invoke_patch_edited();
        assert_eq!(state.borrow().develop.edit.retouch.patches[0].radius, 0.05);
        assert_eq!(
            state.borrow().develop.edit.retouch.patches[1].radius,
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
        assert_eq!(state.borrow().develop.edit.retouch.patches.len(), 1);
        assert_eq!(app.get_patch(), -1);

        app.invoke_patch_changed(0);
        assert_eq!(app.get_patch(), 0, "chosen from the list");
        press(&app, Key::Escape);
        assert_eq!(app.get_patch(), -1, "Esc lets go of it");
    }
}
