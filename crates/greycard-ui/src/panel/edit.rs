use crate::panel::assets::{show_looks, show_profiles};
use crate::panel::browser::{camera_tag, file_name};
use crate::panel::color::{
    band_model, band_values, draw_wheels, grade_model, panel_tint, panel_white, read_grading,
    show_bw_band, show_grade_range, show_mixer_band, show_tint,
};
use crate::panel::crop::{read_geometry, show_geometry};
use crate::panel::cull::{control_over_frame, leave_cull};
use crate::panel::curve::{
    current_channel, curve_points, draw_curve, panel_parametric, select_point, set_curve_points,
    set_panel_parametric, unless_unmoved,
};
use crate::panel::history::show_history;
use crate::panel::mask::{
    component_invert, component_names, component_on, heading_name, show_component,
};
use crate::panel::retouch::show_patches;
use crate::*;
use greycard_edit::LocalWhite;

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

/// Write cell `c`'s file's sidecar, if sidecars are written, and its
/// meta to an XMP beside it when that is asked for: [`write_file`] of
/// the cell's file. The sidecar is the file's whole, every line of it,
/// written once whichever of its cells moved.
pub(crate) fn write_sidecar(st: &mut State, c: usize) {
    if let Some(file) = st.file_at(c) {
        write_file(st, file);
    }
}

/// Write file `file`'s sidecar, if sidecars are written, and its meta
/// to an XMP beside it when that is asked for.
///
/// The XMP goes first so that the mark for what it now holds is in
/// the sidecar before the sidecar is written; otherwise the next
/// load would read back what this build just wrote. An XMP that
/// will not parse costs the interop and nothing else: the `.gcd` is
/// written either way.
pub(crate) fn write_file(st: &mut State, file: usize) {
    if !file_writable(st, file) {
        return;
    }
    // A frame followed to an archive's copy has its XMP written there
    // by the archive write, from the sidecar, off the window's thread.
    if st.window.xmp_sidecars && !crate::sync::followed(st, file) {
        // `tiff:Orientation` is the camera's tag and the frame's
        // quarter turns composed; with no tag to compose onto,
        // whatever the file says about the orientation is left
        // alone and the rest of the meta still goes.
        let turn = st
            .file_cell(file)
            .and_then(|c| camera_tag(st, c))
            .map(|o| xmp::Turn::new(o, st.sidecars[file].turn));
        // The capture time's dates, from the camera's own time, asked
        // only when a shift moves them.
        let (path, meta) = (st.files[file].clone(), st.sidecars[file].meta.clone());
        let taken = || crate::panel::time_shift::camera_time(st, file);
        match xmp::save_dated(&path, &meta, turn, taken) {
            Ok(Some(mark)) => st.sidecars[file].xmp = Some(mark),
            Ok(None) => {}
            Err(e) => tracing::warn!("{}: xmp not written: {e}", file_name(&st.files[file])),
        }
    }
    save_file(st, file);
}

/// [`save_file`] of cell `c`'s file.
pub(crate) fn save_sidecar(st: &mut State, c: usize) -> bool {
    st.file_at(c).is_some_and(|file| save_file(st, file))
}

/// Write file `file`'s sidecar where it lives: for a frame the window
/// follows on an archive's copy (§233), the save is made in memory and
/// the file written off the window's thread by the archive write, with
/// its check against the copy; for every other frame, written here
/// beside it, the index told and the archive's copy queued. One save,
/// one revision, however many of its lines moved. True when the save
/// was made.
pub(crate) fn save_file(st: &mut State, file: usize) -> bool {
    if crate::sync::followed(st, file) {
        st.sidecars[file].advance(&greycard_edit::sync::Stamp::now());
        crate::sync::after_save(st, file);
        for c in st.cells_of_file(file) {
            crate::edited::saved(st, c);
        }
        return true;
    }
    let (path, placement) = (st.files[file].clone(), st.window.placement);
    match st.sidecars[file].save_in(&path, placement) {
        Ok(_) => {
            crate::library::sidecar_written(st, file);
            true
        }
        Err(e) => {
            tracing::warn!("{}: sidecar not saved: {e}", file_name(&path));
            false
        }
    }
}

/// Whether cell `c`'s sidecar may be written now: sidecars are on,
/// the frame is not being deleted, and the sidecar in memory is the
/// frame's own and not its row standing in (`rows`), which would put
/// the default edit over whatever the file holds.
pub(crate) fn writable(st: &State, c: usize) -> bool {
    st.file_at(c).is_some_and(|file| file_writable(st, file))
}

/// [`writable`] by the file.
pub(crate) fn file_writable(st: &State, file: usize) -> bool {
    st.window.write_sidecars
        && !crate::panel::delete::held_file(st, file)
        && st.from_row.get(file).is_none_or(|r| r.read)
}

/// The open frame's quarter turns, for a job the worker develops:
/// the turn is beside the edit on the sidecar, not in it, so every
/// develop has to carry it.
pub(crate) fn current_turn(st: &State) -> u8 {
    // A frame of the export queue turns as it was queued.
    if let Some(f) = st.views.queued_open {
        return f.turn;
    }
    st.current
        .and_then(|c| st.sidecar_at(c))
        .map_or(0, |s| s.turn)
}

/// How long the sliders must rest before a develop starts.
pub(crate) const DEBOUNCE_MS: u64 = 300;

/// The slider a timing run moves: `--time-sharpen` the sharpen's
/// radius, `--time-clarity` the Detail section's Clarity,
/// `--time-dehaze` its Dehaze, and `--time-texture` its Texture with
/// the Dehaze held on, which is the move that pays for both.
#[derive(Clone, Copy)]
enum Timed {
    Sharpen,
    Clarity,
    Dehaze,
    Texture,
}

/// The moves left, when the last was sent, and the milliseconds each
/// took to its frame.
pub(crate) type Timing = (u32, Option<std::time::Instant>, Vec<f64>);

/// `--time-sharpen` and the other timing runs, on each frame that brought
/// a new picture: the time since the last move was sent, then the
/// next move (the slider between two fixed values, its section on)
/// sent straight to the worker, past the debounce; after the last,
/// the mean on the log and the terminal, and the editor quits.
pub(crate) fn time_moves(st: &mut State, app: &App) {
    time_move(st, app, Timed::Sharpen);
    time_move(st, app, Timed::Clarity);
    time_move(st, app, Timed::Dehaze);
    time_move(st, app, Timed::Texture);
}

fn time_move(st: &mut State, app: &App, which: Timed) {
    let (name, timing) = match which {
        Timed::Sharpen => ("sharpen", &mut st.launch.time_sharpen),
        Timed::Clarity => ("clarity", &mut st.launch.time_clarity),
        Timed::Dehaze => ("dehaze", &mut st.launch.time_dehaze),
        Timed::Texture => ("texture", &mut st.launch.time_texture),
    };
    let Some((left, sent, samples)) = timing.as_mut() else {
        return;
    };
    if let Some(at) = sent.take() {
        let ms = at.elapsed().as_secs_f64() * 1e3;
        tracing::info!("{name} move to frame: {ms:.1} ms");
        samples.push(ms);
    }
    if *left == 0 {
        let n = samples.len().max(1) as f64;
        let mean = samples.iter().sum::<f64>() / n;
        let min = samples.iter().copied().fold(f64::INFINITY, f64::min);
        let max = samples.iter().copied().fold(0.0, f64::max);
        let line = format!(
            "{name} move to frame over {} moves: mean {mean:.1} ms, min {min:.1}, max {max:.1}",
            samples.len()
        );
        tracing::info!("{line}");
        eprintln!("{line}");
        *timing = None;
        let _ = slint::quit_event_loop();
        return;
    }
    *left -= 1;
    let low = left.is_multiple_of(2);
    // `GREYCARD_TIME_MOVE_GAP_MS` rests the slider this long before each
    // move, for measuring a move after a pause.
    let gap = std::env::var("GREYCARD_TIME_MOVE_GAP_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if gap == 0 {
        send_move(st, app, which, low);
        return;
    }
    let app_weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(gap), move || {
        if let (Some(app), Some(state)) = (app_weak.upgrade(), STATE.with(|s| s.borrow().clone())) {
            send_move(&mut state.borrow_mut(), &app, which, low);
        }
    });
}

/// A timed run's next move of `which`, to its `low` value or its high,
/// sent straight to the worker and its moment noted.
fn send_move(st: &mut State, app: &App, which: Timed, low: bool) {
    let timing = match which {
        Timed::Sharpen => &mut st.launch.time_sharpen,
        Timed::Clarity => &mut st.launch.time_clarity,
        Timed::Dehaze => &mut st.launch.time_dehaze,
        Timed::Texture => &mut st.launch.time_texture,
    };
    if let Some((_, sent, _)) = timing.as_mut() {
        *sent = Some(std::time::Instant::now());
    }
    match which {
        Timed::Sharpen => {
            st.develop.edit.sharpen.enabled = true;
            st.develop.edit.sharpen.auto_radius = false;
            st.develop.edit.sharpen.radius = if low { 0.6 } else { 1.0 };
        }
        Timed::Clarity => {
            st.develop.edit.detail.enabled = true;
            st.develop.edit.detail.clarity = if low { 0.3 } else { 0.6 };
        }
        Timed::Dehaze => {
            st.develop.edit.detail.enabled = true;
            st.develop.edit.detail.dehaze = if low { 0.3 } else { 0.6 };
        }
        Timed::Texture => {
            st.develop.edit.detail.enabled = true;
            st.develop.edit.detail.dehaze = 0.5;
            st.develop.edit.detail.texture = if low { 0.3 } else { 0.6 };
        }
    }
    st.develop.generation += 1;
    app.set_busy(true);
    let job = Job::Develop {
        edit: st.develop.edit.clone(),
        generation: st.develop.generation,
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
    st.develop.save_timer.start(
        slint::TimerMode::SingleShot,
        std::time::Duration::from_millis(SAVE_MS),
        move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            if save_edit(&mut st, edit)
                && let Some(c) = st.current
                && let Some(why) = crate::rows::not_kept(&st, c)
            {
                // The panel moved over the default standing in for a
                // frame whose sidecar is not in (its root offline, its
                // read still out or given up on): said, since nothing
                // of it is kept.
                app.set_status(why.into());
            }
            show_history(&st, &app);
        },
    );
}

/// The panel's `edit`, as the state every action that records the
/// panel first records (a save, an undo, a history click, a snapshot,
/// a preset or a paste over it): `edit` itself, or None while the
/// panel still shows the command line's overrides unmoved.
///
/// `--develop-temperature` and `--exposure` put an edit on the panel
/// that is not the frame's (see [`crate::panel::startup::Overrides`]),
/// and it is never recorded as a state of its own: until the panel
/// moves off it, there is nothing of the panel's to record, and an
/// undo, a click or a preset goes from the sidecar's own current
/// state. The first edit that differs from it ends that, one-shot:
/// the flag goes and the new edit is recorded on top of the sidecar's
/// current state like any other, the override's values that are still
/// on the panel with it, as they would be for anything the user did
/// over what the panel showed. So does anything that puts one of the
/// sidecar's states on the panel instead (`take_current`), and leaving
/// the frame (`open_row`). After that, the override's values are
/// values like any others, and saved as such.
pub(crate) fn panel_state(st: &mut State, edit: Edit) -> Option<Edit> {
    let Some(c) = st.current else {
        return Some(edit);
    };
    match &st.launch.overridden {
        Some((file, shown)) if file == st.path_of(c) && *shown == edit => None,
        Some(_) => {
            st.launch.overridden = None;
            Some(edit)
        }
        None => Some(edit),
    }
}

/// Record what the panel holds as the current file's state, before an
/// action that goes on from it (an undo, a history click, a snapshot,
/// a preset or a paste over it), without writing the sidecar; true
/// when that was news to the history. Nothing while the panel is the
/// command line's overrides unmoved ([`panel_state`]), or in culling.
pub(crate) fn record_panel(st: &mut State, app: &App) -> bool {
    // Never in culling, for the reason `save_edit` gives.
    if st.culling.cull.is_some() {
        return false;
    }
    let Some(c) = st.current else {
        return false;
    };
    if !crate::rows::panel_is_frames(st, c) {
        return false;
    }
    let edit = read_edit(app, &st.develop.edit, st.develop.target);
    panel_state(st, edit).is_some_and(|edit| st.line_mut(c).is_some_and(|l| l.record(edit)))
}

/// Record `edit` as the current file's, and write its sidecar if that
/// changed anything; nothing while it is the command line's overrides
/// unmoved ([`panel_state`]), and nothing in culling, where the panel
/// is not the frame's. True when the change was refused because
/// the panel is not the frame's own (its sidecar stands in: its root
/// offline, its read still out or given up on; or the panel still
/// shows the stand-in it had then) and the panel differs from the
/// sidecar in memory: nothing is recorded,
/// since the stand-in cannot be written and the read's landing would
/// replace it whole, and the caller with a status line says why
/// ([`crate::rows::not_kept`]).
pub(crate) fn save_edit(st: &mut State, edit: Edit) -> bool {
    // In culling the panel is no frame's: it holds the last-developed
    // frame's edit, or the default for a run begun in culling, and the
    // selection moves on without loading it (`cull_select`). The panel
    // was saved on the way in (`enter_cull`, before the mode is set);
    // recorded now, by the quit or a timer still running, it would be
    // written onto whichever frame is current, so nothing is.
    if st.culling.cull.is_some() {
        return false;
    }
    let Some(c) = st.current else {
        return false;
    };
    let Some(edit) = panel_state(st, edit) else {
        return false;
    };
    if !crate::rows::panel_is_frames(st, c) {
        return st.line(c).is_some_and(|l| l.current != edit);
    }
    if !st.line_mut(c).is_some_and(|l| l.record(edit)) {
        return false;
    }
    if writable(st, c) {
        save_sidecar(st, c);
    }
    false
}

/// Put the repair sources the engine chose into `st.edit` and into
/// the current file's sidecar, and write it if that changed anything;
/// true when it did. A chosen source completes the state that placed
/// its patch, in place, and is never a step: recorded as one, a slow
/// develop that landed after the save made the source a state of its
/// own, and undoing it re-developed the sourceless state, chose the
/// same source, and recorded it again over the redo (issue 7).
/// `sources` are the patches the develop chose for, whole, so a patch
/// moved, resized or replaced under a reused id while it was in
/// flight takes nothing.
pub(crate) fn take_sources(st: &mut State, sources: &[greycard_edit::retouch::Patch]) -> bool {
    if sources.is_empty() {
        return false;
    }
    st.develop.edit.retouch.take_sources(sources);
    let Some(c) = st.current else {
        return false;
    };
    // The export queue's frame keeps its sidecar as it is.
    if crate::panel::queue_view::frozen(st) {
        return false;
    }
    if !st.line_mut(c).is_some_and(|l| l.take_sources(sources)) {
        return false;
    }
    if writable(st, c) {
        save_sidecar(st, c);
    }
    true
}

/// The look the panel shows.
pub(crate) fn read_look(app: &App) -> Look {
    let mut light = Light {
        enabled: app.get_light_enabled(),
        exposure: app.get_exposure(),
        ..Light::default()
    };
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
        // A mask's alone: the global look's is never read
        // (`Edit::set_look`), and is shown off.
        white_balance: crate::panel::color::panel_local_white(app),
        // A mask's alone too: the picture's Detail is the edit's own
        // (`Edit::set_look`), from the Develop tab's section.
        detail: greycard_edit::Detail {
            enabled: app.get_local_detail_enabled(),
            texture: app.get_local_texture(),
            clarity: app.get_local_clarity(),
            dehaze: app.get_local_dehaze(),
        },
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
    if st.culling.cull.is_some() {
        leave_cull(st, &app, &worker, None);
        return;
    }
    // In Cull drawn developed the panel is the frame's: the mode ends,
    // and the change develops as it would in Develop.
    crate::panel::cull::leave_developed(st, &app);
    app.window().request_redraw();
    schedule_save(st, app_weak.clone());
    st.develop.debounce.start(
        slint::TimerMode::SingleShot,
        std::time::Duration::from_millis(DEBOUNCE_MS),
        move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.develop.generation += 1;
            app.set_status("developing...".into());
            app.set_busy(true);
            worker.send(Job::Develop {
                edit: edit_to_develop(&app, &st.develop.edit),
                generation: st.develop.generation,
                turn: current_turn(&st),
            });
        },
    );
}

/// The DEMOSAIC menu for `stored`, the edit's choice, on a frame of
/// `mosaic`: that mosaic's methods, and selected the one its develop
/// runs (`Demosaic::resolve`, the engine's resolve). While the mosaic
/// is not known the menu rests with the stored choice and says why,
/// rather than offer another mosaic's methods.
pub(crate) fn show_demosaic(
    mosaic: crate::panel::browser::OpenMosaic,
    stored: Demosaic,
    app: &App,
) {
    let (choices, shown, hint) = demosaic_menu(mosaic, stored);
    app.set_demosaics(ModelRc::new(VecModel::from(
        choices
            .iter()
            .map(|d| slint::SharedString::from(d.name()))
            .collect::<Vec<_>>(),
    )));
    app.set_demosaic(shown.name().into());
    app.set_demosaic_hint(hint.into());
    app.set_demosaic_stored(stored.name().into());
}

/// The menu's choices, its selection and its hint for `stored` on a
/// frame of `mosaic`.
pub(crate) fn demosaic_menu(
    mosaic: crate::panel::browser::OpenMosaic,
    stored: Demosaic,
) -> (Vec<Demosaic>, Demosaic, &'static str) {
    use crate::panel::browser::OpenMosaic;
    match mosaic {
        OpenMosaic::Known(m) if !Demosaic::for_kind(m).is_empty() => {
            (Demosaic::for_kind(m), stored.resolve(m), "")
        }
        OpenMosaic::Known(_) => (vec![stored], stored, "no demosaic for this mosaic"),
        OpenMosaic::NotMosaic => (vec![stored], stored, "not a mosaic"),
        OpenMosaic::Unreadable => (vec![stored], stored, "the raw could not be read"),
        OpenMosaic::Unknown => (vec![stored], stored, "reading the raw"),
    }
}

/// The edit's choice as the menu leaves it: what the edit stores, which
/// only a pick from the menu changes (`demosaic-picked` in the window
/// sets it); opening a frame or moving another slider never rewrites a
/// choice the menu shows resolved.
pub(crate) fn read_demosaic(app: &App) -> Demosaic {
    Demosaic::from_name(app.get_demosaic_stored().as_str()).unwrap_or_default()
}

pub(crate) fn read_edit(app: &App, base: &Edit, target: Option<usize>) -> Edit {
    let mut edit = base.clone();
    // A curve the panel holds in order that `base` has out of order,
    // and is otherwise the same, is `base`'s: no edit by itself.
    let mut look = read_look(app);
    let was = base.target_look(target);
    for channel in Channel::ALL {
        let read = std::mem::take(look.curves.channel_mut(channel));
        *look.curves.channel_mut(channel) =
            unless_unmoved(channel, read, was.curves.channel(channel));
    }
    edit.set_target_look(target, look);
    if let Some(a) = target.and_then(|i| edit.adjustments.get_mut(i)) {
        a.enabled = app.get_adjustment_enabled();
        a.mask.invert = app.get_mask_invert();
        let name = app.get_adjustment_name();
        if !name.trim().is_empty() {
            a.name = name.trim().to_string();
        }
        let which = app.get_component().max(0) as usize;
        if let Some(c) = a.mask.components.get_mut(which) {
            if let Shape::Radial { feather, .. } = &mut c.shape {
                *feather = app.get_mask_feather().clamp(0.0, 1.0);
            }
            crate::panel::mask::read_range(&mut c.shape, app);
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
    // A starting aspect held on the panel is not the edit's yet: until
    // the user acts on the crop, the edit keeps its own.
    if app.get_aspect_held() {
        edit.geometry.aspect = base.geometry.aspect;
        edit.geometry.portrait = base.geometry.portrait;
    }
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
    edit.demosaic = read_demosaic(app);
    // Global, as the black and white is: the curve under every look.
    edit.display_curve = greycard_edit::DisplayCurve::from_name(app.get_display_curve().as_str())
        .unwrap_or_default();
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
    app.set_contrast(look.light.tone.contrast);
    app.set_highlights(look.light.tone.highlights);
    app.set_shadows(look.light.tone.shadows);
    app.set_whites(look.light.tone.whites);
    app.set_blacks(look.light.tone.blacks);
    // The adjustments' list, and the target's own controls.
    show_names(edit, app);
    show_patches(st, edit, app);
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
    if let Some((i, a)) = target.map(|i| (i, &edit.adjustments[i])) {
        app.set_adjustment_enabled(a.enabled);
        app.set_mask_invert(a.mask.invert);
        app.set_adjustment_name(a.name.as_str().into());
        app.set_target_name(heading_name(&a.name, i).into());
        app.set_component_names(component_names(&a.mask));
        app.set_component_on(component_on(&a.mask));
        app.set_shape_invert(component_invert(&a.mask));
        show_component(&a.mask, app);
    } else {
        app.set_target_name("".into());
        app.set_has_feather(false);
    }
    match edit.white_balance {
        // The sliders show the camera's white under As shot, once the
        // decode has read it; until then they wait for it.
        WhiteBalance::AsShot => {
            app.set_as_shot(true);
            if let Some((temperature, tint)) = st.develop.as_shot_white {
                app.set_temperature(temperature);
                app.set_tint(tint);
            }
        }
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
    // The selected point outlives a showing of the same curve, not an
    // undo or another frame that changes it.
    let shown = curve_points(app, current_channel(app));
    for channel in Channel::ALL {
        set_curve_points(app, channel, look.curves.channel(channel));
    }
    if curve_points(app, current_channel(app)) != shown {
        select_point(app, None);
    }
    // The histogram behind is the last one read, from the state in
    // hand: every caller holds it borrowed, so a second borrow through
    // the thread's handle would find nothing and draw the editor bare
    // until the next analysis, which a switch of target alone does
    // not bring.
    app.set_curve_image(draw_curve(app, st.develop.bins.as_deref()));
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
    app.set_tint_enabled(look.tint.enabled);
    app.set_tint_hue(look.tint.hue);
    app.set_tint_amount(look.tint.amount);
    show_tint(app);
    // A mask's own white balance: on, or switched off with its light
    // kept, the sliders show that light; off with none kept, they keep
    // what they last said, and turning it on starts them at the
    // global's (`local-wb-toggled`).
    // Held to the sliders' range, as the finish reads it.
    // Follow global on, they show the offset from the global's.
    let white = look.white_balance.clamped();
    match white {
        LocalWhite::Absolute { temperature, tint }
        | LocalWhite::Kept { temperature, tint }
        | LocalWhite::Relative {
            mired: temperature,
            tint,
        }
        | LocalWhite::KeptRelative {
            mired: temperature,
            tint,
        } => {
            app.set_local_wb_on(matches!(
                white,
                LocalWhite::Absolute { .. } | LocalWhite::Relative { .. }
            ));
            app.set_local_wb_kept(true);
            app.set_local_wb_relative(white.is_relative());
            app.set_local_temperature(temperature as f32);
            app.set_local_tint(tint as f32);
        }
        LocalWhite::Off => {
            app.set_local_wb_on(false);
            app.set_local_wb_kept(false);
            app.set_local_wb_relative(false);
        }
    }
    // A mask's own Detail; the global look's is the default, never
    // shown, since its section is on the Masks tab alone.
    app.set_local_detail_enabled(look.detail.enabled);
    app.set_local_texture(look.detail.texture);
    app.set_local_clarity(look.detail.clarity);
    app.set_local_dehaze(look.detail.dehaze);
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
    show_demosaic(st.open_mosaic(), edit.demosaic, app);
    app.set_display_curve(edit.display_curve.name().into());
    // The list and its warning follow the choice, so an undo or a
    // preset that brings another camera's profile says so at once.
    show_profiles(st, &edit.camera.profile, app);
    // The list, its note and the strength follow the choice, so an
    // undo or a preset that brings a look says so at once.
    show_looks(st, &edit.look_lut, app);
    app.set_retouch_enabled(edit.retouch.enabled);
    // "Use this person on all" only while the shape still has them,
    // under the mask it is in.
    crate::panel::parts_all::shown(st, edit, app);
    // The look as the panel now holds it, the baseline for the next
    // change (see `mask::look_edited`).
    *st.develop.look_seen.borrow_mut() = Some(read_look(app));
}

/// The panel's sections that fold, by the names the settings keep
/// them under, with the panel's property for each.
pub(crate) type Fold = (&'static str, fn(&App) -> bool, fn(&App, bool));

pub(crate) const FOLDS: &[Fold] = &[
    ("roots", App::get_collapsed_roots, App::set_collapsed_roots),
    (
        "collections",
        App::get_collapsed_collections,
        App::set_collapsed_collections,
    ),
    (
        "folders",
        App::get_collapsed_folders,
        App::set_collapsed_folders,
    ),
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
        "metadata",
        App::get_collapsed_metadata,
        App::set_collapsed_metadata,
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
    ("mask", App::get_collapsed_mask, App::set_collapsed_mask),
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
            if st.culling.cull.is_some() {
                let with = control_over_frame(&mut st, &app);
                leave_cull(&mut st, &app, &worker, with);
                return;
            }
            crate::panel::cull::leave_developed(&mut st, &app);
            st.develop.edit = read_edit(&app, &st.develop.edit, st.develop.target);
            app.window().request_redraw();
            schedule_save(&mut st, app.as_weak());
            let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
            st.develop.debounce.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(DEBOUNCE_MS),
                move || {
                    let Some(app) = app_weak.upgrade() else {
                        return;
                    };
                    let mut st = state.borrow_mut();
                    st.develop.generation += 1;
                    app.set_status("developing...".into());
                    app.set_busy(true);
                    worker.send(Job::Develop {
                        edit: edit_to_develop(&app, &st.develop.edit),
                        generation: st.develop.generation,
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

    /// One section open at a time: off, sections open side by side;
    /// on, opening one, by its header or by the settings' restore,
    /// folds the others of the tab and leaves the panes' alone.
    #[test]
    fn one_section_open_folds_the_tabs_others() {
        let app = window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let settle = |app: &App| {
            app.window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(0.0, 0.0),
                });
            slint::platform::update_timers_and_animations();
        };
        assert_eq!(app.get_panel_tab(), "Develop");
        for (_, _, set) in FOLDS {
            set(&app, true);
        }
        settle(&app);
        app.set_collapsed_light(false);
        app.set_collapsed_color(false);
        app.set_collapsed_history(false);
        settle(&app);
        assert!(
            !app.get_collapsed_light() && !app.get_collapsed_color(),
            "off: both open"
        );

        app.set_one_section(true);
        app.set_collapsed_grain(false);
        settle(&app);
        assert!(!app.get_collapsed_grain());
        assert!(
            app.get_collapsed_light() && app.get_collapsed_color(),
            "on: the rest fold"
        );
        assert!(
            !app.get_collapsed_history(),
            "the left pane's are not the panel's"
        );
        app.set_collapsed_light(false);
        settle(&app);
        assert!(!app.get_collapsed_light() && app.get_collapsed_grain());
        // Folding one opens nothing.
        app.set_collapsed_light(true);
        settle(&app);
        assert!(read_folds(&app).iter().any(|f| f == "grain"));

        // METADATA opened in the grid's pane shares its fold with the
        // panel's, which the grid covers: the panel behind stays.
        app.set_collapsed_grain(false);
        settle(&app);
        assert!(app.get_collapsed_metadata());
        app.set_grid_open(true);
        settle(&app);
        app.set_collapsed_metadata(false);
        settle(&app);
        assert!(!app.get_collapsed_metadata());
        assert!(
            !app.get_collapsed_grain(),
            "the grid's METADATA folded the panel"
        );
        // Back in Develop, an opening folds as before.
        app.set_grid_open(false);
        settle(&app);
        app.set_collapsed_light(false);
        settle(&app);
        assert!(app.get_collapsed_grain() && app.get_collapsed_metadata());
    }

    /// The DEMOSAIC menu lists the open raw's mosaic's methods and
    /// selects what its develop runs; read back untouched it keeps the
    /// stored choice; a pick through the window's callback is stored,
    /// the method already shown included; while the mosaic is not
    /// known the menu rests and offers no other mosaic's methods.
    #[test]
    fn the_demosaic_menu_follows_the_open_raw_s_mosaic() {
        use crate::panel::browser::OpenMosaic;
        use greycard_core::raw::Mosaic;
        let app = window(1);
        let names = |app: &App| -> Vec<String> {
            app.get_demosaics().iter().map(|s| s.to_string()).collect()
        };
        show_demosaic(OpenMosaic::Known(Mosaic::XTrans), Demosaic::Amaze, &app);
        assert_eq!(names(&app), ["markesteijn-3", "markesteijn-1", "bilinear"]);
        assert_eq!(app.get_demosaic(), "markesteijn-3");
        assert_eq!(app.get_demosaic_hint(), "");
        // Opening, or another slider: the stored choice stays.
        assert_eq!(read_demosaic(&app), Demosaic::Amaze);
        // A click on the method already shown stores it.
        app.invoke_demosaic_picked("markesteijn-3".into());
        assert_eq!(read_demosaic(&app), Demosaic::Markesteijn3);
        // One, then three: three.
        app.invoke_demosaic_picked("markesteijn-1".into());
        app.invoke_demosaic_picked("markesteijn-3".into());
        assert_eq!(read_demosaic(&app), Demosaic::Markesteijn3);

        // A frame opening whose mosaic is not known yet: no Bayer list
        // to click, the stored choice as it is, and why.
        show_demosaic(OpenMosaic::Unknown, Demosaic::Markesteijn1, &app);
        assert_eq!(names(&app), ["markesteijn-1"]);
        assert_eq!(app.get_demosaic(), "markesteijn-1");
        assert_eq!(app.get_demosaic_hint(), "reading the raw");
        assert_eq!(read_demosaic(&app), Demosaic::Markesteijn1);

        // A raw that is no mosaic says so for good, not "reading".
        show_demosaic(OpenMosaic::NotMosaic, Demosaic::Markesteijn1, &app);
        assert_eq!(app.get_demosaic_hint(), "not a mosaic");

        show_demosaic(
            OpenMosaic::Known(Mosaic::Bayer),
            Demosaic::Markesteijn1,
            &app,
        );
        assert_eq!(names(&app).len(), 6);
        assert_eq!(names(&app)[0], "amaze-vng4");
        assert_eq!(app.get_demosaic(), "amaze-vng4");
        assert_eq!(read_demosaic(&app), Demosaic::Markesteijn1);
        app.invoke_demosaic_picked("rcd".into());
        assert_eq!(read_demosaic(&app), Demosaic::Rcd);
    }

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

    /// A mouse wheel's click is one step of the slider, whatever the
    /// platform makes a click in pixels. Slint hands the window 60 a
    /// click, and a slider counting 40-pixel notches made the clicks
    /// alternate one step and two. A touchpad's small deltas still add
    /// up to a step.
    #[test]
    fn a_wheel_click_on_a_slider_is_one_step() {
        use crate::testing;
        use slint::platform::WindowEvent;

        let app = window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        app.set_panel_tab("Develop".into());
        app.set_exposure(0.0);
        let (at, size) = testing::labeled(&app, "Exposure");
        let center = (at.x + size.width / 2.0, at.y + size.height / 2.0);
        // A click on the slider focuses it, so the wheel is its own.
        testing::click(&app, center.0, center.1);
        let scroll = |delta_y: f32| {
            app.window().dispatch_event(WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(center.0, center.1),
                delta_x: 0.0,
                delta_y,
            });
        };
        let exposure = || (app.get_exposure() * 100.0).round() / 100.0;

        for (i, want) in [0.05, 0.10, 0.15, 0.20].into_iter().enumerate() {
            scroll(60.0);
            assert_eq!(exposure(), want, "click {}", i + 1);
        }
        scroll(-60.0);
        assert_eq!(exposure(), 0.15, "a click back is one step back");
        // A platform giving a smaller click, still one step each.
        scroll(45.0);
        scroll(45.0);
        assert_eq!(exposure(), 0.25);
        // A touchpad's deltas add up: two of 15 are nothing, the third
        // is the step, and the remainder does not carry.
        scroll(15.0);
        scroll(15.0);
        assert_eq!(exposure(), 0.25);
        scroll(15.0);
        assert_eq!(exposure(), 0.30);
        scroll(15.0);
        assert_eq!(exposure(), 0.30);
    }
}
