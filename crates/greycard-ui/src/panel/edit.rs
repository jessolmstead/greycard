use crate::panel::assets::{show_looks, show_profiles};
use crate::panel::browser::{camera_tag, file_name};
use crate::panel::color::{
    band_model, band_values, draw_wheels, grade_model, panel_tint, panel_white, read_grading,
    show_bw_band, show_grade_range, show_mixer_band, show_tint,
};
use crate::panel::crop::{read_geometry, show_geometry};
use crate::panel::cull::{control_over_frame, leave_cull};
use crate::panel::curve::{
    curve_points, draw_curve, panel_parametric, set_curve_points, set_panel_parametric,
};
use crate::panel::history::show_history;
use crate::panel::mask::{component_names, component_on, show_component};
use crate::panel::retouch::show_patches;
use crate::*;

/// The edit the worker should develop right now. Normally the edit
/// itself; but a tool in hand may need the picture to show something
/// else while it is out, and the defringe's dropper does. It reads
/// the very fringes the pass takes out, so with the pass on there is
/// nothing left at the click to read a hue off — the residual is
/// usually under [`DEFRINGE_PICK_FLOOR`] — and the button is dead
/// with the defringe off, so there would be no state in which a hue
/// could be picked at all. While the dropper is out the picture is
/// developed with the defringe off, which is both what the user needs
/// to see to aim and what the pass itself takes as its input. The
/// edit is not touched: only what is sent to the worker.
pub(crate) fn edit_to_develop(app: &App, edit: &Edit) -> Edit {
    let mut shown = edit.clone();
    if app.get_picking() == "Defringe" {
        shown.lens.defringe = false;
    }
    shown
}

/// Write the current file's sidecar, if sidecars are written, and
/// its meta to an XMP beside it when that is asked for.
///
/// The XMP goes first so that the mark for what it now holds is in
/// the sidecar before the sidecar is written; otherwise the next
/// load would read back what this build just wrote. An XMP that
/// will not parse costs the interop and nothing else: the `.gcd` is
/// written either way.
pub(crate) fn write_sidecar(st: &mut State, c: usize) {
    if !st.write_sidecars {
        return;
    }
    if st.xmp_sidecars {
        // `tiff:Orientation` is the camera's tag and the frame's
        // quarter turns composed; with no tag to compose onto,
        // whatever the file says about the orientation is left
        // alone and the rest of the meta still goes.
        let turn = camera_tag(st, c).map(|o| xmp::Turn::new(o, st.sidecars[c].turn));
        match xmp::save(&st.files[c], &st.sidecars[c].meta, turn) {
            Ok(Some(mark)) => st.sidecars[c].xmp = Some(mark),
            Ok(None) => {}
            Err(e) => tracing::warn!("{}: xmp not written: {e}", file_name(&st.files[c])),
        }
    }
    if let Err(e) = st.sidecars[c].save(&st.files[c]) {
        tracing::warn!("{}: sidecar not saved: {e}", file_name(&st.files[c]));
    }
}

/// The open frame's quarter turns, for a job the worker develops:
/// the turn is beside the edit on the sidecar, not in it, so every
/// develop has to carry it.
pub(crate) fn current_turn(st: &State) -> u8 {
    st.current
        .and_then(|c| st.sidecars.get(c))
        .map_or(0, |s| s.turn)
}

/// How long the sliders must rest before a develop starts.
pub(crate) const DEBOUNCE_MS: u64 = 300;

/// `--time-sharpen`, on each frame that brought a new picture: the
/// time since the last move was sent, then the next move (the radius
/// between two fixed values, the sharpen on) sent straight to the
/// worker, past the debounce; after the last, the mean on the log and
/// the terminal, and the editor quits.
pub(crate) fn time_sharpen(st: &mut State, app: &App) {
    let Some((left, sent, samples)) = st.time_sharpen.as_mut() else {
        return;
    };
    if let Some(at) = sent.take() {
        let ms = at.elapsed().as_secs_f64() * 1e3;
        tracing::info!("sharpen move to frame: {ms:.1} ms");
        samples.push(ms);
    }
    if *left == 0 {
        let n = samples.len().max(1) as f64;
        let mean = samples.iter().sum::<f64>() / n;
        let min = samples.iter().copied().fold(f64::INFINITY, f64::min);
        let max = samples.iter().copied().fold(0.0, f64::max);
        let line = format!(
            "sharpen move to frame over {} moves: mean {mean:.1} ms, min {min:.1}, max {max:.1}",
            samples.len()
        );
        tracing::info!("{line}");
        eprintln!("{line}");
        st.time_sharpen = None;
        let _ = slint::quit_event_loop();
        return;
    }
    *left -= 1;
    let radius = if left.is_multiple_of(2) { 0.6 } else { 1.0 };
    *sent = Some(std::time::Instant::now());
    st.edit.sharpen.enabled = true;
    st.edit.sharpen.auto_radius = false;
    st.edit.sharpen.radius = radius;
    st.generation += 1;
    app.set_busy(true);
    let job = Job::Develop {
        edit: st.edit.clone(),
        generation: st.generation,
        turn: current_turn(st),
    };
    WORKER.with(|w| {
        if let Some(w) = &*w.borrow() {
            w.send(job);
        }
    });
}

/// How long they must rest before the sidecar is written.
pub(crate) const SAVE_MS: u64 = 800;

/// Write the current file's sidecar once the panel has rested.
pub(crate) fn schedule_save(st: &mut State, app_weak: slint::Weak<App>) {
    let Some(state) = STATE.with(|s| s.borrow().clone()) else {
        return;
    };
    st.save_timer.start(
        slint::TimerMode::SingleShot,
        std::time::Duration::from_millis(SAVE_MS),
        move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let edit = read_edit(&app, &st.edit, st.target);
            save_edit(&mut st, edit);
            show_history(&st, &app);
        },
    );
}

/// Record `edit` as the current file's, and write its sidecar if that
/// changed anything.
pub(crate) fn save_edit(st: &mut State, edit: Edit) {
    let Some(c) = st.current else {
        return;
    };
    if st.sidecars[c].record(edit)
        && st.write_sidecars
        && let Err(e) = st.sidecars[c].save(&st.files[c])
    {
        tracing::warn!("{}: sidecar not saved: {e}", file_name(&st.files[c]));
    }
}

/// The look the panel shows.
pub(crate) fn read_look(app: &App) -> Look {
    let mut light = Light {
        enabled: app.get_light_enabled(),
        exposure: app.get_exposure(),
        ..Light::default()
    };
    light.tone.enabled = app.get_tone_curve();
    light.tone.contrast = app.get_contrast();
    light.tone.highlights = app.get_highlights();
    light.tone.shadows = app.get_shadows();
    light.tone.whites = app.get_whites();
    light.tone.blacks = app.get_blacks();
    Look {
        light,
        curves: Curves {
            enabled: app.get_curves_enabled(),
            parametric: panel_parametric(app),
            rgb: curve_points(app, Channel::Rgb),
            red: curve_points(app, Channel::Red),
            green: curve_points(app, Channel::Green),
            blue: curve_points(app, Channel::Blue),
            red_green: curve_points(app, Channel::RedGreen),
            blue_yellow: curve_points(app, Channel::BlueYellow),
        },
        mixer: Mixer {
            enabled: app.get_mixer_enabled(),
            hue: band_values(app.get_mixer_hues()),
            saturation: band_values(app.get_mixer_saturations()),
            luminance: band_values(app.get_mixer_luminances()),
        },
        color: Color {
            enabled: app.get_color_enabled(),
            saturation: app.get_saturation(),
            vibrance: app.get_vibrance(),
        },
        grading: read_grading(app),
        tint: panel_tint(app),
    }
}

/// The edit as the panel shows it: `base` with the panel's look put
/// on `target`, the panel's word on that adjustment's mask, and the
/// global-only controls.
/// Develop `st.edit` once the pointer has rested, as the develop
/// sliders do.
pub(crate) fn develop_soon(
    st: &mut State,
    state: Rc<RefCell<State>>,
    worker: Rc<Worker>,
    app_weak: slint::Weak<App>,
) {
    let Some(app) = app_weak.upgrade() else {
        return;
    };
    // A repair reached in culling: out of it, and the frame develops.
    if st.cull.is_some() {
        leave_cull(st, &app, &worker, None);
        return;
    }
    app.window().request_redraw();
    schedule_save(st, app_weak.clone());
    st.debounce.start(
        slint::TimerMode::SingleShot,
        std::time::Duration::from_millis(DEBOUNCE_MS),
        move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.generation += 1;
            app.set_status("developing...".into());
            app.set_busy(true);
            worker.send(Job::Develop {
                edit: edit_to_develop(&app, &st.edit),
                generation: st.generation,
                turn: current_turn(&st),
            });
        },
    );
}

pub(crate) fn read_edit(app: &App, base: &Edit, target: Option<usize>) -> Edit {
    let mut edit = base.clone();
    let mut look = read_look(app);
    if target.is_some() {
        // The tone curve's switch is the picture's, not a mask's.
        look.light.tone.enabled = true;
    }
    edit.set_target_look(target, look);
    edit.light.tone.enabled = app.get_tone_curve();
    if let Some(a) = target.and_then(|i| edit.adjustments.get_mut(i)) {
        a.enabled = app.get_adjustment_enabled();
        a.mask.invert = app.get_mask_invert();
        let name = app.get_adjustment_name();
        if !name.trim().is_empty() {
            a.name = name.trim().to_string();
        }
        let which = app.get_component().max(0) as usize;
        if let Some(c) = a.mask.components.get_mut(which) {
            c.invert = app.get_component_invert();
            if let Shape::Radial { feather, .. } = &mut c.shape {
                *feather = app.get_mask_feather().clamp(0.0, 1.0);
            }
        }
    }
    // Global only, like the vignette and the grain: a mask carries a
    // look and a picture is mono or it is not.
    edit.bw = greycard_edit::BlackWhite {
        enabled: app.get_bw_enabled(),
        weights: band_values(app.get_bw_weights()),
        strength: app
            .get_bw_strength()
            .clamp(0.0, greycard_edit::bw::STRENGTH_MAX),
    };
    edit.white_balance = panel_white((app.get_as_shot(), app.get_temperature(), app.get_tint()));
    edit.noise.enabled = app.get_noise_enabled();
    edit.noise.profiled = app.get_denoise();
    edit.noise.strength = app.get_denoise_strength();
    edit.noise.learned = Learned::from_name(app.get_denoise_learned().as_str()).unwrap_or_default();
    edit.noise.learned_strength = app.get_denoise_learned_strength();
    edit.lens = greycard_edit::Lens {
        enabled: app.get_lens_enabled(),
        profile: app.get_lens_profile_on(),
        distortion: app.get_lens_distortion(),
        chromatic_aberration: app.get_lens_ca(),
        vignetting: app.get_lens_vignetting(),
        manual: app.get_lens_manual(),
        ca_red: app.get_lens_ca_red(),
        ca_blue: app.get_lens_ca_blue(),
        auto_scale: app.get_lens_auto_scale(),
        scale: app.get_lens_scale(),
        defringe: app.get_lens_defringe(),
        defringe_radius: app.get_lens_defringe_radius(),
        defringe_threshold: app.get_lens_defringe_threshold(),
        defringe_purple_center: app.get_lens_defringe_purple_center(),
        defringe_purple_width: app.get_lens_defringe_purple_width(),
        defringe_purple_amount: app.get_lens_defringe_purple_amount(),
        defringe_green_center: app.get_lens_defringe_green_center(),
        defringe_green_width: app.get_lens_defringe_green_width(),
        defringe_green_amount: app.get_lens_defringe_green_amount(),
    };
    edit.geometry = read_geometry(app);
    edit.vignette = Vignette {
        enabled: app.get_vignette_enabled(),
        amount: app.get_vignette_amount(),
        midpoint: app.get_vignette_midpoint(),
        feather: app.get_vignette_feather(),
        roundness: app.get_vignette_roundness(),
    };
    edit.grain = Grain {
        enabled: app.get_grain_enabled(),
        amount: app.get_grain_amount(),
        size: app.get_grain_size(),
        kind: greycard_edit::grain::Kind::from_name(app.get_grain_kind().as_str())
            .unwrap_or_default(),
    };
    edit.detail = greycard_edit::Detail {
        enabled: app.get_detail_enabled(),
        texture: app.get_detail_texture(),
        clarity: app.get_detail_clarity(),
        dehaze: app.get_detail_dehaze(),
    };
    edit.sharpen.enabled = app.get_sharpen();
    edit.sharpen.auto_radius = app.get_sharpen_auto_radius();
    edit.sharpen.radius = app.get_sharpen_radius();
    edit.sharpen.iterations = app.get_sharpen_iterations().round().max(1.0) as u32;
    edit.sharpen.auto_threshold = app.get_sharpen_auto_threshold();
    edit.sharpen.threshold = app.get_sharpen_threshold();
    edit.demosaic = Demosaic::from_name(app.get_demosaic().as_str()).unwrap_or_default();
    edit.camera.profile =
        greycard_edit::camera::ProfileChoice::from_name(app.get_camera_profile().as_str());
    edit.look_lut.lut = greycard_edit::look::LutChoice::from_name(app.get_look_name().as_str());
    edit.look_lut.strength = app.get_look_strength().clamp(0.0, 1.0);
    edit.retouch.enabled = app.get_retouch_enabled();
    edit
}

/// The adjustments' names on the panel's list, Global first.
pub(crate) fn show_names(edit: &Edit, app: &App) {
    let mut names = vec![slint::SharedString::from("Global")];
    names.extend(
        edit.adjustments
            .iter()
            .map(|a| slint::SharedString::from(a.name.as_str())),
    );
    app.set_adjustment_names(ModelRc::new(VecModel::from(names)));
}

/// Put an edit on the panel, its look that of `target`. As-shot
/// temperature and tint arrive with the frame and are filled in then.
pub(crate) fn show_edit(st: &State, edit: &Edit, app: &App, target: Option<usize>) {
    let look = edit.target_look(target);
    app.set_light_enabled(look.light.enabled);
    app.set_exposure(look.light.exposure);
    app.set_tone_curve(edit.light.tone.enabled);
    app.set_contrast(look.light.tone.contrast);
    app.set_highlights(look.light.tone.highlights);
    app.set_shadows(look.light.tone.shadows);
    app.set_whites(look.light.tone.whites);
    app.set_blacks(look.light.tone.blacks);
    // The adjustments' list, and the target's own controls.
    show_names(edit, app);
    show_patches(edit, app);
    let target = target.filter(|&i| i < edit.adjustments.len());
    app.set_target(target.map(|i| i as i32 + 1).unwrap_or(0));
    // A chosen adjustment is edited on the Masks tab.
    if target.is_some() {
        app.set_panel_tab("Masks".into());
    }
    app.set_adjustment_on(ModelRc::new(VecModel::from(
        edit.adjustments
            .iter()
            .map(|a| a.enabled)
            .collect::<Vec<_>>(),
    )));
    if let Some(a) = target.map(|i| &edit.adjustments[i]) {
        app.set_adjustment_enabled(a.enabled);
        app.set_mask_invert(a.mask.invert);
        app.set_adjustment_name(a.name.as_str().into());
        app.set_target_name(a.name.as_str().into());
        app.set_component_names(component_names(&a.mask));
        app.set_component_on(component_on(&a.mask));
        show_component(&a.mask, app);
    } else {
        app.set_target_name("".into());
        app.set_has_feather(false);
    }
    match edit.white_balance {
        WhiteBalance::AsShot => app.set_as_shot(true),
        WhiteBalance::Custom { temperature, tint } => {
            app.set_as_shot(false);
            app.set_temperature(temperature as f32);
            app.set_tint(tint as f32);
        }
    }
    app.set_noise_enabled(edit.noise.enabled);
    app.set_denoise(edit.noise.profiled);
    app.set_denoise_strength(edit.noise.strength);
    app.set_denoise_learned(edit.noise.learned.name().into());
    app.set_denoise_learned_strength(edit.noise.learned_strength);
    app.set_curves_enabled(look.curves.enabled);
    set_panel_parametric(app, &look.curves.parametric);
    for channel in Channel::ALL {
        set_curve_points(app, channel, look.curves.channel(channel));
    }
    let bins = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|st| st.try_borrow().ok().and_then(|st| st.bins.clone()))
    });
    app.set_curve_image(draw_curve(app, bins.as_deref()));
    app.set_lens_enabled(edit.lens.enabled);
    app.set_lens_profile_on(edit.lens.profile);
    app.set_lens_distortion(edit.lens.distortion);
    app.set_lens_ca(edit.lens.chromatic_aberration);
    app.set_lens_vignetting(edit.lens.vignetting);
    app.set_lens_manual(edit.lens.manual);
    app.set_lens_ca_red(edit.lens.ca_red);
    app.set_lens_ca_blue(edit.lens.ca_blue);
    app.set_lens_auto_scale(edit.lens.auto_scale);
    app.set_lens_scale(edit.lens.scale);
    app.set_lens_defringe(edit.lens.defringe);
    app.set_lens_defringe_radius(edit.lens.defringe_radius);
    app.set_lens_defringe_threshold(edit.lens.defringe_threshold);
    app.set_lens_defringe_purple_center(edit.lens.defringe_purple_center);
    app.set_lens_defringe_purple_width(edit.lens.defringe_purple_width);
    app.set_lens_defringe_purple_amount(edit.lens.defringe_purple_amount);
    app.set_lens_defringe_green_center(edit.lens.defringe_green_center);
    app.set_lens_defringe_green_width(edit.lens.defringe_green_width);
    app.set_lens_defringe_green_amount(edit.lens.defringe_green_amount);
    show_geometry(&edit.geometry, app);
    app.set_vignette_enabled(edit.vignette.enabled);
    app.set_vignette_amount(edit.vignette.amount);
    app.set_vignette_midpoint(edit.vignette.midpoint);
    app.set_vignette_feather(edit.vignette.feather);
    app.set_vignette_roundness(edit.vignette.roundness);
    app.set_grain_enabled(edit.grain.enabled);
    app.set_grain_amount(edit.grain.amount);
    app.set_grain_size(edit.grain.size);
    app.set_grain_kind(edit.grain.kind.name().into());
    app.set_mixer_enabled(look.mixer.enabled);
    app.set_mixer_hues(band_model(&look.mixer.hue));
    app.set_mixer_saturations(band_model(&look.mixer.saturation));
    app.set_mixer_luminances(band_model(&look.mixer.luminance));
    show_mixer_band(app);
    app.set_color_enabled(look.color.enabled);
    app.set_saturation(look.color.saturation);
    app.set_vibrance(look.color.vibrance);
    app.set_bw_enabled(edit.bw.enabled);
    app.set_bw_weights(band_model(&edit.bw.weights));
    app.set_bw_strength(edit.bw.strength);
    show_bw_band(app);
    app.set_grading_enabled(look.grading.enabled);
    app.set_grade_hues(grade_model(&Range::ALL.map(|r| look.grading.wheel(r).hue)));
    app.set_grade_saturations(grade_model(
        &Range::ALL.map(|r| look.grading.wheel(r).saturation),
    ));
    app.set_grade_balance(look.grading.balance);
    show_grade_range(app);
    draw_wheels(app);
    app.set_tint_hue(look.tint.hue);
    app.set_tint_amount(look.tint.amount);
    show_tint(app);
    app.set_detail_enabled(edit.detail.enabled);
    app.set_detail_texture(edit.detail.texture);
    app.set_detail_clarity(edit.detail.clarity);
    app.set_detail_dehaze(edit.detail.dehaze);
    app.set_sharpen(edit.sharpen.enabled);
    app.set_sharpen_auto_radius(edit.sharpen.auto_radius);
    app.set_sharpen_radius(edit.sharpen.radius);
    app.set_sharpen_iterations(edit.sharpen.iterations as f32);
    app.set_sharpen_auto_threshold(edit.sharpen.auto_threshold);
    app.set_sharpen_threshold(edit.sharpen.threshold);
    app.set_demosaic(edit.demosaic.name().into());
    // The list and its warning follow the choice, so an undo or a
    // preset that brings another camera's profile says so at once.
    show_profiles(st, &edit.camera.profile, app);
    // The list, its note and the strength follow the choice, so an
    // undo or a preset that brings a look says so at once.
    show_looks(st, &edit.look_lut, app);
    app.set_retouch_enabled(edit.retouch.enabled);
}

/// The panel's sections that fold, by the names the settings keep
/// them under, with the panel's property for each.
pub(crate) type Fold = (&'static str, fn(&App) -> bool, fn(&App, bool));

pub(crate) const FOLDS: &[Fold] = &[
    (
        "navigator",
        App::get_collapsed_navigator,
        App::set_collapsed_navigator,
    ),
    (
        "culling",
        App::get_collapsed_culling,
        App::set_collapsed_culling,
    ),
    (
        "snapshots",
        App::get_collapsed_snapshots,
        App::set_collapsed_snapshots,
    ),
    (
        "history",
        App::get_collapsed_history,
        App::set_collapsed_history,
    ),
    (
        "adjustments",
        App::get_collapsed_adjustments,
        App::set_collapsed_adjustments,
    ),
    (
        "white-balance",
        App::get_collapsed_wb,
        App::set_collapsed_wb,
    ),
    ("light", App::get_collapsed_light, App::set_collapsed_light),
    ("color", App::get_collapsed_color, App::set_collapsed_color),
    (
        "curves",
        App::get_collapsed_curves,
        App::set_collapsed_curves,
    ),
    ("mixer", App::get_collapsed_mixer, App::set_collapsed_mixer),
    (
        "black-and-white",
        App::get_collapsed_bw,
        App::set_collapsed_bw,
    ),
    (
        "grading",
        App::get_collapsed_grading,
        App::set_collapsed_grading,
    ),
    ("tint", App::get_collapsed_tint, App::set_collapsed_tint),
    ("noise", App::get_collapsed_noise, App::set_collapsed_noise),
    ("lens", App::get_collapsed_lens, App::set_collapsed_lens),
    (
        "detail",
        App::get_collapsed_detail,
        App::set_collapsed_detail,
    ),
    (
        "sharpen",
        App::get_collapsed_sharpen,
        App::set_collapsed_sharpen,
    ),
    (
        "vignette",
        App::get_collapsed_vignette,
        App::set_collapsed_vignette,
    ),
    ("grain", App::get_collapsed_grain, App::set_collapsed_grain),
    ("proof", App::get_collapsed_proof, App::set_collapsed_proof),
    (
        "monitor",
        App::get_collapsed_monitor,
        App::set_collapsed_monitor,
    ),
    ("crop", App::get_collapsed_crop, App::set_collapsed_crop),
    (
        "rotate",
        App::get_collapsed_rotate,
        App::set_collapsed_rotate,
    ),
    (
        "perspective",
        App::get_collapsed_perspective,
        App::set_collapsed_perspective,
    ),
    (
        "demosaic",
        App::get_collapsed_demosaic,
        App::set_collapsed_demosaic,
    ),
    (
        "camera",
        App::get_collapsed_camera,
        App::set_collapsed_camera,
    ),
    ("look", App::get_collapsed_look, App::set_collapsed_look),
    (
        "retouch",
        App::get_collapsed_retouch,
        App::set_collapsed_retouch,
    ),
    (
        "presets",
        App::get_collapsed_presets,
        App::set_collapsed_presets,
    ),
];

/// The Crop tab's one GEOMETRY section, before it became three.
/// A settings file that folded it folds all three.
pub(crate) const WAS_GEOMETRY: [&str; 3] = ["crop", "rotate", "perspective"];

/// Fold the sections the last run left folded.
pub(crate) fn show_folds(app: &App, collapsed: &[String]) {
    let legacy = collapsed.iter().any(|c| c == "geometry")
        && !collapsed.iter().any(|c| WAS_GEOMETRY.contains(&c.as_str()));
    for (name, _, set) in FOLDS {
        set(
            app,
            collapsed.iter().any(|c| c == name) || (legacy && WAS_GEOMETRY.contains(name)),
        );
    }
}

/// The sections folded now, by name.
pub(crate) fn read_folds(app: &App) -> Vec<String> {
    FOLDS
        .iter()
        .filter(|(_, get, _)| get(app))
        .map(|(name, _, _)| name.to_string())
        .collect()
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    // A change the engine must see: the viewport previews what it can
    // at once, and the develop waits until the sliders rest.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_develop_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // A develop control reached in culling is the way out of
            // it: the frame develops, with that control as set.
            if st.cull.is_some() {
                let with = control_over_frame(&mut st, &app);
                leave_cull(&mut st, &app, &worker, with);
                return;
            }
            st.edit = read_edit(&app, &st.edit, st.target);
            app.window().request_redraw();
            schedule_save(&mut st, app.as_weak());
            let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
            st.debounce.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(DEBOUNCE_MS),
                move || {
                    let Some(app) = app_weak.upgrade() else {
                        return;
                    };
                    let mut st = state.borrow_mut();
                    st.generation += 1;
                    app.set_status("developing...".into());
                    app.set_busy(true);
                    worker.send(Job::Develop {
                        edit: edit_to_develop(&app, &st.edit),
                        generation: st.generation,
                        turn: current_turn(&st),
                    });
                },
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::window;

    /// The defringe's dropper needs the fringes on screen to aim at,
    /// so while it is out the picture is developed with the pass off.
    /// The edit itself is not touched: the sidecar still says the
    /// defringe is on, and every other dropper leaves the develop
    /// alone.
    #[test]
    fn the_defringe_dropper_takes_the_pass_off_the_picture() {
        let app = window(1);
        let mut edit = Edit::default();
        edit.lens.defringe = true;
        edit.lens.defringe_radius = 3.0;

        assert!(app.get_picking().is_empty());
        assert!(edit_to_develop(&app, &edit).lens.defringe);

        app.set_picking("Defringe".into());
        let shown = edit_to_develop(&app, &edit);
        assert!(!shown.lens.defringe, "the pass is off while picking");
        assert!(shown.lens.defringe().is_none());
        // Only the defringe: the rest of the edit goes as it is, and
        // what is saved still has the pass on.
        assert_eq!(shown.lens.defringe_radius, 3.0);
        assert_eq!(
            greycard_edit::Lens {
                defringe: true,
                ..shown.lens
            },
            edit.lens
        );
        assert!(edit.lens.defringe, "the edit itself is untouched");

        // Another dropper changes nothing about the develop.
        app.set_picking("White".into());
        assert!(edit_to_develop(&app, &edit).lens.defringe);
        // And putting it down puts the pass back.
        app.set_picking("".into());
        assert!(edit_to_develop(&app, &edit).lens.defringe);
    }
}
