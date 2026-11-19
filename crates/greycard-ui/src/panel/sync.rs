//! Sync settings: the current frame's chosen sections laid over the
//! rest of the browser's selection, each frame's history taking it
//! as one step and each sidecar written where the frame keeps it.
//!
//! The sheet is the preset sheet's twin and the work is a preset's:
//! `greycard_edit::apply_preset_into` lays a `Preset`'s sections over
//! each frame, so there is no second copy of what a section is.
//! [`lay_over_targets`] is the loop both a sync and a preset click
//! onto two or more selected frames run: a sync builds its `Preset`
//! from the current frame (`sync_selection`, below); a preset click
//! (`panel::assets::on_preset_applied`) already has one, from the
//! store. Two things are the sync's own, because a sync reads the
//! frames it lands on and a preset cannot: a named camera profile
//! goes only to frames of the body it was made for, and a frame whose
//! learned-denoiser blend was still waiting on its ISO is given it
//! before the sync (or the preset) makes its edit no longer the
//! default.
//!
//! A sync can also move Temperature, Tint and Exposure relatively, as
//! Lightroom's Quick Develop does: each such field set to "shift" on
//! the sheet moves every frame by the source's difference from where a
//! frame starts (its as-shot light, 0 EV) rather than setting it to the
//! source's value ([`Shift`], [`shifted`]). Temperature moves in mired,
//! in which Lightroom's scale is near uniform: a source warmed 12 mired
//! from its as-shot (300 K at 4800 K) moves a frame at 3000 K by 115 K
//! and one at 6500 K by 560 K, each the same look warmer.
//!
//! Copy and paste are a sync with the settings clipboard between
//! (`crate::clipboard`): [`copy_settings`] takes the current frame's
//! edit, and [`paste_selection`] lays the sections chosen on this
//! same sheet over the whole selection, the frame copied from left
//! out, through the same [`lay_over_targets`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use greycard_core::TempTint;
use greycard_core::raw::Mosaic;
use greycard_edit::camera::{self, ProfileChoice};
use greycard_edit::history::{preset_label, preset_over_set_label, sync_label};
use greycard_edit::preset::{Choice, Part, Section};
use greycard_edit::{Edit, Preset, WhiteBalance};
use slint::{ComponentHandle, ModelRc};

use crate::clipboard::Clipboard;
use crate::panel::browser::{
    chosen_frames, file_name, migrate_frame, show_badges, show_thumb, thumb_turns,
};
use crate::panel::cull::leave_cull;
use crate::panel::edit::{panel_state, read_edit, save_edit, write_sidecar};
use crate::panel::history::{show_history, take_current, take_current_with};
use crate::settings::SyncShift;
use crate::worker::Worker;
use crate::{App, Shared, State, carry, selection, tr};

// The fit of a preset's profile to a frame, which the import also
// asks, off the window, in the library.
pub(crate) use crate::preset_fit::{preset_fitted, probed_body, profile_fits};

/// `preset` as it reaches or leaves cell `c`: without the display
/// curve when the cell's file is a picture, which shows no switch for
/// it ([`crate::preset_fit::curve_fitted`]), and with its white balance
/// as the cell's kind takes one ([`preset_for_target`]). The raw's As
/// shot an offset lands at is the panel's, for the frame open and
/// decoded; no file is read here, on the window's thread.
pub(crate) fn preset_for_curve(st: &State, preset: &Preset, c: usize) -> Preset {
    let shot = (st.current == Some(c))
        .then_some(st.develop.as_shot_white)
        .flatten()
        .filter(|_| !curveless(st.path_at(c).map(|p| p.as_path())))
        .map(|(cct, duv)| TempTint {
            cct: cct as f64,
            duv: duv as f64,
        });
    preset_for_target(st, preset, c, shot)
}

/// [`preset_for_curve`] with the raw's As shot given, read before on the
/// pool ([`read_whites`]): an offset onto a raw is the light it names
/// there ([`crate::preset_fit::white_fitted`]), and left out without
/// one; a kelvin light does not go onto a picture.
fn preset_for_target(st: &State, preset: &Preset, c: usize, shot: Option<TempTint>) -> Preset {
    match st.path_at(c) {
        Some(path) => {
            let curved = crate::preset_fit::curve_fitted(preset, path);
            crate::preset_fit::white_fitted(&curved, path, || shot)
        }
        None => preset.clone(),
    }
}

/// `preset` with its tone curve's switch only where the profile list
/// says the DCP it names carries a curve
/// (`Camera::keep_tone_curve_if`): laid on a profile with none, the
/// switch would be written, move the develop key and the history for
/// nothing, and come back unasked with the next profile that has one.
fn curve_settled(preset: &Preset, profiles: &[camera::Entry]) -> Preset {
    preset.settled_by(|name| profiles.iter().any(|e| e.name == name && e.tone_curve))
}

/// Whether laying `preset` reads the raws' own As shot: it carries a
/// picture's offset, which lands on a raw as a light from there.
fn offset_reads_targets(preset: &Preset) -> bool {
    preset.sections.contains(&Section::WhiteBalance)
        && matches!(preset.edit.white_balance, WhiteBalance::Offset { .. })
}

/// Whether `file` is a picture, whose display curve no switch shows
/// and nothing carries.
fn curveless(file: Option<&std::path::Path>) -> bool {
    file.is_some_and(greycard_core::picture::is_picture_path)
}

/// `choice` as it leaves `file`: without the display curve from a
/// picture, so the status line counts what went.
fn choice_leaving(choice: &Choice, file: Option<&std::path::Path>) -> Choice {
    let mut out = choice.clone();
    if curveless(file) {
        out.sections.retain(|s| *s != Section::DisplayCurve);
    }
    out
}

/// The frames a sync goes onto: the set, less the frame it comes
/// from.
pub(crate) fn sync_targets(st: &State) -> Vec<usize> {
    selection::others(&chosen_frames(st), st.current)
}

/// What the sheet says it will do.
fn sync_words(st: &State) -> String {
    let n = sync_targets(st).len();
    let from = st.current.map(|c| frame_name(st, c)).unwrap_or_default();
    tr!(
        "From {from} onto {n} other frame." | "From {from} onto {n} other frames." % n,
        from = from
    )
}

/// Put the sync sheet's checks on the sheet: its rows and how tall
/// the list is.
pub(crate) fn show_sync_checks(st: &mut State, app: &App) {
    let st = &mut *st;
    let lines = st.sheets.sync_rows.show(&st.sheets.sync_checks);
    app.set_sync_rows(ModelRc::from(st.sheets.sync_rows.model.clone()));
    app.set_sync_lines(lines);
    show_shift(st, app);
}

/// The sheet's shift choices, as the checks and the source stand:
/// whether the white balance and the exposure go (a choice for a
/// field that does not go is greyed), and what a frame moves by.
/// A paste's sheet has none.
fn show_shift(st: &State, app: &App) {
    let Some((edit, shot)) = st
        .sheets
        .sync_source
        .as_ref()
        .filter(|_| !app.get_sync_paste())
    else {
        app.set_sync_shift_shown(false);
        return;
    };
    let preset = Preset::from_choice("", edit, &st.sheets.sync_checks.choice());
    app.set_sync_shift_shown(true);
    app.set_sync_shift_white(preset.sections.contains(&Section::WhiteBalance));
    app.set_sync_shift_light(carries_exposure(&preset));
    let modes = shift_modes(app);
    let shift = shift_of(modes, &preset, edit, *shot);
    let unread = (modes.temperature && app.get_sync_shift_white() && shift.temperature.is_none())
        || (modes.tint && app.get_sync_shift_white() && shift.tint.is_none());
    app.set_sync_shift_words(
        if unread {
            tr!(
                "This frame's as-shot light cannot be read (it is no raw, or its raw does not \
                 say): its Temperature and Tint are set."
            )
        } else if shift.is_none() {
            tr!(
                "Set gives every frame this frame's value. Shift moves each frame from its own \
                 by this frame's difference from its as-shot light (Temperature, Tint) or from \
                 0 EV (Exposure)."
            )
        } else {
            tr!(
                "Each frame moves from its own value: {}. That is this frame's difference from \
                 its as-shot light and from 0 EV.",
                shift.sheet_words()
            )
        }
        .into(),
    );
}

/// The sheet's three choices.
pub(crate) fn shift_modes(app: &App) -> SyncShift {
    SyncShift {
        temperature: app.get_sync_shift_temperature(),
        tint: app.get_sync_shift_tint(),
        exposure: app.get_sync_shift_exposure(),
    }
}

/// Frame `file`'s edit as the sheet lists its masks from: the panel's
/// when the panel is the frame's (outside culling), its sidecar's
/// otherwise.
fn edit_shown(st: &State, app: &App, c: usize) -> Edit {
    // A frame of the export queue's view is its queued edit.
    if let Some(q) = crate::panel::queue_view::frame_of(st, c) {
        return q.edit.clone();
    }
    if st.current == Some(c) && st.culling.cull.is_none() && crate::rows::panel_is_frames(st, c) {
        read_edit(app, &st.develop.edit, st.develop.target)
    } else {
        st.line(c).map(|l| l.current.clone()).unwrap_or_default()
    }
}

/// Cell `c` as a sheet or a status line names it: the file's name, or
/// for a version the file's stem and the version's name, "IMG_0001
/// (Warm)", the one form a frame is named by (`versions::Key::said`,
/// `queue::label_of`).
pub(crate) fn frame_name(st: &State, c: usize) -> String {
    st.key_of(c).map(|k| k.said()).unwrap_or_default()
}

/// A raw frame's make, model and ISO, read from its metadata without
/// a pixel decoded; `None` for a picture that is not a raw, or a file
/// that will not say.
pub(crate) fn probe(st: &State, c: usize) -> Option<greycard_core::decode::Probe> {
    let path = st.path_at(c)?;
    if greycard_core::picture::is_picture_path(path) {
        return None;
    }
    greycard_core::decode::probe_path(path)
        .inspect_err(|e| tracing::warn!("{}: metadata: {e}", file_name(path)))
        .ok()
}

/// Whether laying `preset` asks what mosaic each frame is: it carries
/// the demosaic, and its method is not one every mosaic takes
/// (bilinear goes anywhere).
pub(crate) fn needs_mosaic(preset: &Preset) -> bool {
    let method = preset.edit.demosaic.method();
    preset.sections.contains(&Section::Demosaic)
        && ![Mosaic::Bayer, Mosaic::XTrans]
            .iter()
            .all(|m| method.takes_kind(*m))
}

/// Whether `preset`'s demosaic may go to a frame of `mosaic`: a preset
/// that does not carry the section, or a frame whose mosaic is not
/// known (a picture, a file that will not say), takes it as it is; a
/// method that mosaic does not take is left off, as a camera profile
/// made for another body is.
pub(crate) fn demosaic_fits(preset: &Preset, mosaic: Option<Mosaic>) -> bool {
    !preset.sections.contains(&Section::Demosaic)
        || mosaic.is_none_or(|m| preset.edit.demosaic.method().takes_kind(m))
}

/// `preset` as it reaches frame `file`, for its demosaic: itself, or
/// with Demosaic taken out, and `true`, when the frame's mosaic does
/// not take the method ([`demosaic_fits`]).
pub(crate) fn preset_for_mosaic(st: &mut State, preset: &Preset, file: usize) -> (Preset, bool) {
    if !needs_mosaic(preset) {
        return (preset.clone(), false);
    }
    let mosaic = crate::panel::browser::mosaic_of(st, file);
    if demosaic_fits(preset, mosaic) {
        (preset.clone(), false)
    } else {
        let mut without = preset.clone();
        without.sections.retain(|s| *s != Section::Demosaic);
        (without, true)
    }
}

/// `edit` as a source of a sync, a copy or a preset made on a frame of
/// `mosaic`: its demosaic is the one the frame's develop runs, the one
/// its menu shows, and not a choice stored for another mosaic and
/// never picked here.
pub(crate) fn as_source(mut edit: Edit, mosaic: Option<Mosaic>) -> Edit {
    if let Some(m) = mosaic {
        edit.demosaic = edit.demosaic.resolve(m);
    }
    edit
}

/// `preset` as it reaches `file`: itself, when it carries no named
/// camera profile or the profile fits `file`'s body; with Camera
/// taken out, and `true`, when it does not. The check
/// [`lay_over_targets`] gives every target in a set — a preset naming
/// a DCP is not a frame's for the taking just because it is the one
/// on screen, so a preset click's own frame goes through this too.
pub(crate) fn preset_for_body(
    st: &State,
    preset: &Preset,
    file: usize,
    profiles: &[camera::Entry],
    probe: impl Fn(&State, usize) -> Option<greycard_core::decode::Probe>,
) -> (Preset, bool) {
    preset_fitted(preset, profiles, || probe(st, file))
}

/// What a sync did: the frames it moved, and the frames the camera
/// profile was left off because it was made for another body.
#[derive(Debug, Default)]
pub(crate) struct Synced {
    pub(crate) moved: Vec<usize>,
    pub(crate) profile_left_off: Vec<usize>,
    /// The frames the demosaic was left off: a method of the other
    /// mosaic ([`demosaic_fits`]).
    pub(crate) demosaic_left_off: Vec<usize>,
    /// The cells whose file was written: the moved and the seeded, each
    /// file once however many of its cells are among them.
    pub(crate) written: Vec<usize>,
}

/// Lay `preset`'s carried sections over `targets`: one step on each
/// frame's history, a frame that already matches left alone, each
/// sidecar written with the placement the setting asks for, each
/// row's picture and badges put out again. No frame outside `targets`
/// is touched, so a caller's current frame (a sync's source, or a
/// preset click's frame on screen) is safe to hand this its own
/// targets alone.
///
/// `learned_from` is a sync's own difference from a plain preset
/// apply: `Some` source edit, with Noise carried, also carries the
/// learned denoiser's tier and blend along
/// (`greycard_edit::sync_learned`); a preset's Noise never does,
/// since the learned tier is each file's own, so a preset click
/// passes `None`.
///
/// `profiles` is the profile directory as
/// `panel::assets::profiles_for_sync` gives it:
/// with Camera carried and a named profile on `preset`, each target
/// is read for its body and a target the profile was not made for
/// takes every other section and keeps its own profile.
///
/// A target still waiting on its first open for the learned blend
/// its ISO asks for (`seed_blend`) is given that blend now when this
/// call does not bring one itself (`learned_from` is `None`): the
/// call makes the target's edit no longer the default, and the next
/// launch would take that to mean the blend was seeded already.
///
/// `label` is the words each target's step is recorded with, which
/// its history row shows in place of the sections that moved: a
/// sync's "Sync from" the frame it came from, a preset's "Preset ×3:"
/// and its name; `None` records the steps with no words.
// `profiles` and `probe` pair through every sync call: the module's change.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lay_over_targets(
    st: &mut State,
    app: &App,
    preset: &Preset,
    targets: &[usize],
    learned_from: Option<&Edit>,
    label: Option<&str>,
    profiles: &[camera::Entry],
    probe: impl Fn(&State, usize) -> Option<greycard_core::decode::Probe>,
) -> Synced {
    lay_shifted_over_targets(
        st,
        app,
        preset,
        targets,
        learned_from,
        label,
        profiles,
        probe,
        None,
    )
}

/// [`lay_over_targets`], each target's preset first [`shifted`] from
/// its own edit when a sync's sheet asked for a shift.
// `profiles` and `probe` pair through every sync call: the module's change.
#[allow(clippy::too_many_arguments)]
fn lay_shifted_over_targets(
    st: &mut State,
    app: &App,
    preset: &Preset,
    targets: &[usize],
    learned_from: Option<&Edit>,
    label: Option<&str>,
    profiles: &[camera::Entry],
    probe: impl Fn(&State, usize) -> Option<greycard_core::decode::Probe>,
    shift: Option<&Shift>,
) -> Synced {
    let preset = &curve_settled(preset, profiles);
    // Each target's whole sidecar first, read where it stands in from
    // its row (on the pool when there are several); one under a root
    // that is offline is left out, and the status line says so.
    let targets = &crate::rows::load_frames(st, app, targets);
    // As a turn does: a sidecar from an older build is brought up to
    // date before anything acts on it, when its shape can be had.
    for &f in targets {
        migrate_frame(st, f);
    }
    let named = match &preset.edit.camera.profile {
        ProfileChoice::Named(name) if preset.sections.contains(&Section::Camera) => {
            Some(profiles.iter().find(|e| &e.name == name))
        }
        _ => None,
    };
    let seeds = learned_from.is_none();
    let wants_mosaic = needs_mosaic(preset);
    let mut fits = Vec::new();
    let mut left_off = Vec::new();
    let mut demosaic_off = Vec::new();
    let mut seeded = Vec::new();
    for &f in targets {
        // The line's own wait for its ISO: a version made from the
        // original before the raw was opened is seeded here as the
        // file's own is, one edited apart is not.
        let wants_seed = seeds && crate::rows::wants_seed(st, f);
        let probed = (named.is_some() || wants_seed)
            .then(|| probe(st, f))
            .flatten();
        // The mosaic only when the method is not every mosaic's, and
        // through the session's cache: a frame read once is not read
        // again.
        if wants_mosaic && !demosaic_fits(preset, crate::panel::browser::mosaic_of(st, f)) {
            demosaic_off.push(f);
        }
        if wants_seed
            && let Some(p) = &probed
            && crate::rows::seed_landed(st, f, greycard_edit::Noise::blend_for_iso(p.iso))
        {
            seeded.push(f);
        }
        match named {
            Some(entry) if !profile_fits(entry, probed.as_ref().map(probed_body)) => {
                left_off.push(f)
            }
            _ => fits.push(f),
        }
    }
    // The as-shot light of every target at As shot that a shift turns
    // custom, read from the raws on the pool before the lay, as the
    // sidecars were above: a read is tens of milliseconds a raw and
    // more for a DNG, too long a batch's worth for the window's thread.
    // So is a raw target's own light that a picture's offset lands at.
    let shift = shift.filter(|s| !s.is_none());
    let offset = offset_reads_targets(preset);
    let whites = if offset || shift.is_some_and(|s| s.reads_targets(preset)) {
        let wanted: Vec<(usize, PathBuf, greycard_edit::camera::Camera)> = targets
            .iter()
            .filter_map(|&c| {
                let current = &st.line(c)?.current;
                let path = st.path_at(c)?;
                let wanted = if offset {
                    !greycard_core::picture::is_picture_path(path)
                } else {
                    current.white_balance == WhiteBalance::AsShot
                };
                if !wanted {
                    return None;
                }
                let camera = if left_off.contains(&c) {
                    current.camera.clone()
                } else {
                    profile_choice(preset, current)
                };
                Some((c, st.path_at(c)?.to_path_buf(), camera))
            })
            .collect();
        read_whites(&wanted)
    } else {
        HashMap::new()
    };
    // Each cell's own line takes the preset as a step of its history: a
    // file and a version of it are two lines of one sidecar.
    let lay = |st: &mut State, cells: &[usize], preset: &Preset| -> Vec<usize> {
        cells
            .iter()
            .copied()
            .filter(|&c| {
                // A shift is the frame's own value moved: read from its
                // line before the lay, its As shot from what was read.
                let own = shift.and_then(|s| {
                    let current = st.line(c)?.current.clone();
                    let shot = whites.get(&c).copied().flatten();
                    Some(shifted(preset, s, &current, shot))
                });
                let shot = whites.get(&c).copied().flatten();
                let preset = &preset_for_target(st, own.as_ref().unwrap_or(preset), c, shot);
                st.line_mut(c).is_some_and(|line| {
                    greycard_edit::apply_preset_to_line(line, preset, learned_from, label)
                })
            })
            .collect()
    };
    // Each target with the sections it cannot take left out: the
    // camera profile for another body, the demosaic for another mosaic.
    let without = |camera: bool, demosaic: bool| {
        let mut p = preset.clone();
        p.sections
            .retain(|s| !(camera && *s == Section::Camera || demosaic && *s == Section::Demosaic));
        p
    };
    let mut moved = Vec::new();
    for camera in [false, true] {
        for demosaic in [false, true] {
            let cells: Vec<usize> = fits
                .iter()
                .chain(&left_off)
                .copied()
                .filter(|f| left_off.contains(f) == camera && demosaic_off.contains(f) == demosaic)
                .collect();
            if !cells.is_empty() {
                moved.extend(lay(st, &cells, &without(camera, demosaic)));
            }
        }
    }
    moved.sort_unstable();
    let written = selection::union(&moved, &seeded);
    // Each file written once, however many of its lines moved.
    let files = st.files_of(&written);
    crate::panel::edit::write_files(st, &files);
    for &f in &written {
        let (turns, flip) = thumb_turns(st, app, f);
        show_thumb(st, app, f, turns, flip);
        show_badges(st, app, f);
    }
    show_history(st, app);
    Synced {
        moved,
        profile_left_off: left_off,
        demosaic_left_off: demosaic_off,
        written,
    }
}

/// Whether `synced`'s writes covered cell `c`'s file: the step recorded
/// on `c` before the lay went to disk with them, and a write of its own
/// would be a second revision of the one file.
fn wrote_file_of(st: &State, synced: &Synced, c: usize) -> bool {
    let file = st.file_at(c);
    file.is_some() && synced.written.iter().any(|&w| st.file_at(w) == file)
}

/// Lay `sections` of the current frame over the rest of the set,
/// through [`lay_over_targets`]. The current frame's edit is not
/// touched.
#[cfg(test)]
pub(crate) fn sync_selection(
    st: &mut State,
    app: &App,
    sections: &[Section],
    profiles: &[camera::Entry],
    probe: impl Fn(&State, usize) -> Option<greycard_core::decode::Probe>,
) -> Synced {
    let Some(c) = st.current else {
        return Synced::default();
    };
    let targets = sync_targets(st);
    let choice = Choice::whole(sections);
    sync_onto(
        st,
        app,
        c,
        &targets,
        &choice,
        SyncShift::default(),
        profiles,
        probe,
    )
    .0
}

/// Lay what `choice` picks of frame `from` over `targets`:
/// [`sync_selection`]
/// with the frames named rather than read from the selection, which is
/// what a sync pressed on frames whose sidecars were still to be read
/// runs when they are in. `from` is read from the panel when it is the
/// frame on screen outside culling, and from its sidecar otherwise; a
/// frame whose sidecar stands in (its root offline) is no source.
///
/// Each field `modes` shifts moves every target from its own value by
/// `from`'s own move ([`shifted`]); the shift laid comes back beside
/// what moved.
// `profiles` and `probe` pair through every sync call: the module's change.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_onto(
    st: &mut State,
    app: &App,
    from: usize,
    targets: &[usize],
    choice: &Choice,
    modes: SyncShift,
    profiles: &[camera::Entry],
    probe: impl Fn(&State, usize) -> Option<greycard_core::decode::Probe>,
) -> (Synced, Shift) {
    let Some(from_edit) = source_edit(st, app, from) else {
        return (Synced::default(), Shift::default());
    };
    // A picture's curve is no choice to carry.
    let preset = preset_for_curve(st, &Preset::between_frames(&from_edit, choice), from);
    // The source's frame is read for its mosaic only when the demosaic
    // goes.
    let preset = if preset.sections.contains(&Section::Demosaic) {
        let mosaic = crate::panel::browser::mosaic_of(st, from);
        Preset {
            edit: as_source(preset.edit, mosaic),
            ..preset
        }
    } else {
        preset
    };
    let learned_from = preset
        .sections
        .contains(&Section::Noise)
        .then_some(&from_edit);
    let label = Some(sync_label(&frame_name(st, from)));
    // The source's As shot is read only when a shift needs it, to know
    // how far the source moved from it: one at As shot moved nowhere.
    let wants_white = (modes.temperature || modes.tint)
        && preset.sections.contains(&Section::WhiteBalance)
        && from_edit.white_balance != WhiteBalance::AsShot;
    let source_shot = if wants_white {
        frame_shot(st, from, &from_edit.camera)
    } else {
        None
    };
    let shift = shift_of(modes, &preset, &from_edit, source_shot);
    let synced = lay_shifted_over_targets(
        st,
        app,
        &preset,
        targets,
        learned_from,
        label.as_deref(),
        profiles,
        probe,
        Some(&shift),
    );
    // A person picked on one of them is offered for the rest.
    crate::panel::parts_all::reached(st, app, &preset, targets);
    (synced, shift)
}

/// The frames a sync of `choice` from `from` onto `targets` would read
/// for their mosaics, the session and the library not knowing them:
/// the source when the demosaic goes, the targets too when its method,
/// resolved for the source's mosaic as [`sync_onto`] resolves it, is
/// not every mosaic's ([`needs_mosaic`]). While the source's own is
/// still to be read, every target is read with it. Each file once.
fn mosaics_unread(
    st: &mut State,
    app: &App,
    from: usize,
    targets: &[usize],
    choice: &Choice,
) -> Vec<PathBuf> {
    use crate::panel::browser::{OpenMosaic, known_mosaic};
    let Some(from_edit) = source_edit(st, app, from) else {
        return Vec::new();
    };
    let preset = Preset::between_frames(&from_edit, choice);
    if !preset.sections.contains(&Section::Demosaic) {
        return Vec::new();
    }
    let source = known_mosaic(st, from);
    let resolved = Preset {
        edit: as_source(preset.edit, source.kind()),
        ..preset
    };
    let targets = if source == OpenMosaic::Unknown || needs_mosaic(&resolved) {
        targets
    } else {
        &[]
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for &c in std::iter::once(&from).chain(targets) {
        if known_mosaic(st, c) == OpenMosaic::Unknown
            && let Some(p) = st.path_at(c)
            && !paths.contains(p)
        {
            paths.push(p.clone());
        }
    }
    paths
}

/// The sync the sheet asked for, run now on the window's thread, its
/// words on the status line.
///
/// A sync from a frame middle-clicked in the strip or the grid may
/// land on the frame on screen. Outside culling the panel is that
/// frame's, so what it holds is recorded first, the sync lays over its
/// line as over any target's, and the panel takes the line's new state
/// and develops, as a paste onto it does; the words ride that develop.
/// A frame that had the sections already moves nothing, but what the
/// panel held and the save timer had yet to write is the frame's
/// newest state, recorded here, so it is written here too.
fn sync_now(
    state: &Rc<Shared>,
    app: &App,
    worker: &Worker,
    from: usize,
    targets: &[usize],
    choice: &Choice,
    modes: SyncShift,
) {
    let profiles = crate::panel::assets::profiles_for_sync(app);
    let start = std::time::Instant::now();
    let mut st = state.borrow_mut();
    let on_screen = st.current.filter(|&c| {
        c != from
            && targets.contains(&c)
            && st.culling.cull.is_none()
            && crate::rows::panel_is_frames(&st, c)
    });
    let recorded = on_screen.is_some() && crate::panel::edit::record_panel(&mut st, app);
    let (synced, shift) = sync_onto(&mut st, app, from, targets, choice, modes, &profiles, probe);
    let took = start.elapsed();
    let went = choice_leaving(choice, st.path_at(from).map(PathBuf::as_path));
    let mut said = synced_words(&st, &went, targets.len(), &synced);
    if !shift.is_none() && !synced.moved.is_empty() {
        said.push_str(&tr!(", each shifted {}", shift.words()));
    }
    tracing::info!("{said} in {:.1} ms", took.as_secs_f64() * 1e3);
    if let Some(c) = on_screen {
        if synced.moved.contains(&c) {
            // The lay wrote the frame's file with the rest.
            let before = st.develop.generation;
            take_current_with(&mut st, app, worker, false);
            if st.develop.generation != before {
                st.develop.status_after_develop = Some((st.develop.generation, said));
                return;
            }
        } else if recorded
            && !wrote_file_of(&st, &synced, c)
            && crate::panel::edit::writable(&st, c)
        {
            crate::panel::edit::save_sidecar(&mut st, c);
        }
    }
    app.set_status(said.into());
}

/// A middle click's sync from a frame, held while it waits on its
/// reads ([`Sheets::frame_sync_waiting`]); let go however its action
/// ends, run or dropped unrun.
///
/// [`Sheets::frame_sync_waiting`]: crate::owners::Sheets::frame_sync_waiting
struct Waiting(Rc<std::cell::Cell<bool>>);

impl Drop for Waiting {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// The checks a sync from frame `from` without the sheet takes: the
/// last sync's or paste's over `from`'s masks, the sheet's defaults
/// before there was one.
fn last_checks(st: &State, app: &App, from: usize) -> carry::Checks {
    let masks = carry::masks_of(&edit_shown(st, app, from));
    let mut checks = match &st.sheets.sync_last {
        Some(kept) => kept.with_masks(masks),
        None => carry::Checks::new(masks, Section::syncs_by_default),
    };
    if curveless(st.path_at(from).map(PathBuf::as_path)) {
        checks.set_section(Section::DisplayCurve, false);
    }
    checks
}

/// A middle click's sync from frame `from`, without the sheet: a
/// frame's in the strip or the grid, open or not, or the Sync button's
/// from the open frame. Onto [`frame_sync_targets`], on the last
/// choice over `from`'s masks once its sidecar is in. The selection
/// and the frame on screen stay. The guards are the sheet's: the
/// queue's view, a tool in hand, no targets.
///
/// It always sets: the sheet's shifts are left out, so a second click
/// changes nothing, where a shift would move every target again. A
/// click while the last one still waits on its reads is dropped.
fn sync_quick(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, from: usize) {
    let st = state.borrow();
    if crate::panel::queue_view::refused(&st, "a sync") {
        return;
    }
    let waiting = st.sheets.frame_sync_waiting.clone();
    if waiting.get() {
        tracing::info!("a middle click's sync dropped: the last one is still waiting");
        return;
    }
    if tool_in_hand(app) {
        app.set_status(tr!("put the tool down first (Esc), then sync").into());
        return;
    }
    let targets = frame_sync_targets(&st, from);
    if targets.is_empty() {
        app.set_status(tr!("choose the frames to sync onto: Ctrl+click or Shift+click").into());
        return;
    }
    let frames: Vec<crate::versions::Key> = std::iter::once(from)
        .chain(targets.iter().copied())
        .filter_map(|c| st.key_of(c))
        .collect();
    drop(st);
    tracing::info!("sync without the sheet, on the last choice, set");
    waiting.set(true);
    let held = Waiting(waiting);
    run_sync(
        state,
        app,
        worker,
        frames,
        None,
        SyncShift::default(),
        Some(held),
    );
}

/// The frames a middle click's sync from frame `from` (clicked in the
/// strip or the grid, or the open frame for the Sync button) goes onto: the selection less `from`, the frame on screen
/// among them when `from` is another.
pub(crate) fn frame_sync_targets(st: &State, from: usize) -> Vec<usize> {
    chosen_frames(st)
        .into_iter()
        .filter(|&c| c != from)
        .collect()
}

/// The sync from the first of `frames` onto the rest: their sidecars
/// requested, the mosaics the library does not know read off the
/// window's thread, and [`sync_now`] once they are in. `choice` is the
/// sheet's; `None` takes [`last_checks`] once the source is read.
/// `held`, a middle click's, goes with the actions to the end.
fn run_sync(
    state: &Rc<Shared>,
    app: &App,
    worker: &Rc<Worker>,
    frames: Vec<crate::versions::Key>,
    choice: Option<Choice>,
    modes: SyncShift,
    held: Option<Waiting>,
) {
    crate::rows::request(
        state,
        app,
        worker,
        frames,
        crate::rows::Scope::Line,
        crate::rows::Request::Sync,
        Box::new(move |state, app, worker, at| {
            let Some(from) = at.first().copied().flatten() else {
                app.set_status(tr!("the frame to sync from is gone; nothing synced").into());
                return;
            };
            let targets: Vec<usize> = at.into_iter().skip(1).flatten().collect();
            let choice = match choice {
                Some(choice) => choice,
                None => {
                    let st = state.borrow();
                    if !crate::rows::is_loaded(&st, from) {
                        tracing::info!("sync without the sheet: its source's sidecar is not in");
                        crate::rows::say_offline_for(
                            &st,
                            app,
                            from,
                            &tr!(": nothing to sync from"),
                        );
                        return;
                    }
                    // The last choice is read, never written: only the
                    // sheet's apply and a paste remember one.
                    let choice = last_checks(&st, app, from).choice();
                    if choice.is_empty() {
                        app.set_status(tr!("choose what goes across").into());
                        return;
                    }
                    choice
                }
            };
            // The mosaics the library does not know yet are read off
            // this thread first, and the sync runs when they are in.
            let (unread, frames) = {
                let mut st = state.borrow_mut();
                let frames = std::iter::once(from)
                    .chain(targets.iter().copied())
                    .filter_map(|c| st.key_of(c))
                    .collect();
                (
                    mosaics_unread(&mut st, app, from, &targets, &choice),
                    frames,
                )
            };
            crate::rows::after_mosaics(
                state,
                app,
                worker,
                unread,
                frames,
                crate::rows::Scope::Line,
                crate::rows::Request::Sync,
                Box::new(move |state, app, worker, at| {
                    let _held = held;
                    let Some(from) = at.first().copied().flatten() else {
                        app.set_status(
                            tr!("the frame to sync from is gone; nothing synced").into(),
                        );
                        return;
                    };
                    let targets: Vec<usize> = at.into_iter().skip(1).flatten().collect();
                    sync_now(state, app, worker, from, &targets, &choice, modes);
                }),
            );
        }),
    );
}

/// What a shift moves each frame by: the source's difference from where
/// a frame starts (its as-shot light for Temperature and Tint, 0 EV, the
/// default edit's, for Exposure). Temperature in mired, positive
/// toward the higher kelvin; Tint in Duv; Exposure in stops. `None`
/// where the sheet sets rather than shifts, where the section does not
/// go, or where the source's own start could not be read.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Shift {
    pub(crate) temperature: Option<f64>,
    pub(crate) tint: Option<f64>,
    pub(crate) exposure: Option<f32>,
    /// The kelvin the source's As shot means, which the sheet says the
    /// mired shift comes to at; `None` for a source at As shot.
    pub(crate) from_kelvin: Option<f64>,
}

/// Mired, the reciprocal megakelvin.
fn mired(kelvin: f64) -> f64 {
    1e6 / kelvin
}

/// `kelvin` moved by `by` mired toward the higher kelvin, held to the
/// Temperature slider's range.
pub(crate) fn kelvin_moved(kelvin: f64, by: f64) -> f64 {
    let at = mired(kelvin) - by;
    if at <= mired(12000.0) {
        12000.0
    } else {
        (1e6 / at).clamp(2000.0, 12000.0)
    }
}

impl Shift {
    fn moves_white(&self) -> bool {
        self.temperature.is_some() || self.tint.is_some()
    }

    pub(crate) fn is_none(&self) -> bool {
        !self.moves_white() && self.exposure.is_none()
    }

    /// Whether laying `preset` with this shift reads the targets' own
    /// as-shot light: it moves the white of a source that moved (one
    /// at As shot leaves every target's white as it is).
    fn reads_targets(&self, preset: &Preset) -> bool {
        self.moves_white() && preset.edit.white_balance != WhiteBalance::AsShot
    }

    /// The shift in the status line's words: "Temperature +12 mired,
    /// Exposure +0.50 EV"; empty for none.
    pub(crate) fn words(&self) -> String {
        let mut said = Vec::new();
        if let Some(t) = self.temperature {
            said.push(tr!("Temperature {t} mired", t = format!("{t:+.0}")));
        }
        if let Some(d) = self.tint {
            said.push(tr!("Tint {d}", d = format!("{d:+.3}")));
        }
        if let Some(e) = self.exposure {
            said.push(tr!("Exposure {e} EV", e = format!("{e:+.2}")));
        }
        said.join(", ")
    }

    /// The shift as the sheet says it: the mired with the kelvin it
    /// comes to at this frame's own light and at a warm and a cool one,
    /// since a mired is more kelvin the cooler the light.
    pub(crate) fn sheet_words(&self) -> String {
        let mut said = Vec::new();
        if let Some(t) = self.temperature {
            let at = |k: f64| kelvin_moved(k, t) - k;
            let here = self
                .from_kelvin
                .map(|k| {
                    tr!(
                        "{} K at this frame's {k} K, ",
                        format!("{:+.0}", at(k)),
                        k = format!("{k:.0}")
                    )
                })
                .unwrap_or_default();
            said.push(tr!(
                "Temperature {t} mired ({here}{} K at 3000 K, {} K at 6500 K)",
                format!("{:+.0}", at(3000.0)),
                format!("{:+.0}", at(6500.0)),
                t = format!("{t:+.0}"),
                here = here
            ));
        }
        if let Some(d) = self.tint {
            said.push(tr!("Tint {d}", d = format!("{d:+.3}")));
        }
        if let Some(e) = self.exposure {
            said.push(tr!("Exposure {e} EV", e = format!("{e:+.2}")));
        }
        said.join(", ")
    }
}

/// Whether `preset` carries the exposure: Light whole, or its
/// Exposure alone.
fn carries_exposure(preset: &Preset) -> bool {
    preset.sections.contains(&Section::Light) || preset.parts.contains(&Part::LightExposure)
}

/// The shift `modes` asks of laying `preset`, made from `source`,
/// whose As shot is `source_shot`. A source at As shot has moved
/// nowhere: its white's shifts are nought.
pub(crate) fn shift_of(
    modes: SyncShift,
    preset: &Preset,
    source: &Edit,
    source_shot: Option<TempTint>,
) -> Shift {
    let white = preset.sections.contains(&Section::WhiteBalance);
    let (temperature, tint) = match (&source.white_balance, source_shot) {
        (WhiteBalance::AsShot, _) => (Some(0.0), Some(0.0)),
        (WhiteBalance::Custom { temperature, tint }, Some(shot)) => (
            Some(mired(shot.cct) - mired(*temperature)),
            Some(tint - shot.duv),
        ),
        // A picture's offset, or a light with no As shot to measure it
        // from: nothing to shift by.
        (WhiteBalance::Custom { .. }, None) | (WhiteBalance::Offset { .. }, _) => (None, None),
    };
    Shift {
        temperature: temperature.filter(|_| white && modes.temperature),
        tint: tint.filter(|_| white && modes.tint),
        exposure: (carries_exposure(preset) && modes.exposure)
            .then(|| source.light.exposure - Edit::default().light.exposure),
        from_kelvin: source_shot
            .filter(|_| source.white_balance != WhiteBalance::AsShot)
            .map(|t| t.cct),
    }
}

/// `preset` as it goes onto a frame whose edit is `target`: each field
/// `shift` moves is the frame's own value moved by it, held to its
/// slider's range; a field set is the source's. A frame at As shot is
/// read as the light its camera recorded (`target_shot`), so with a
/// Temperature shift it becomes custom at its as-shot kelvin moved.
///
/// The white is the frame's own, untouched, when the source is at As
/// shot (it moved nowhere, and a set field would mean the frame's own
/// as-shot value), when both fields shift by nought, and when the
/// frame's own light cannot be read.
pub(crate) fn shifted(
    preset: &Preset,
    shift: &Shift,
    target: &Edit,
    target_shot: Option<TempTint>,
) -> Preset {
    let mut p = preset.clone();
    if shift.moves_white() {
        let unmoved = shift.temperature == Some(0.0) && shift.tint == Some(0.0);
        let own = match &target.white_balance {
            WhiteBalance::Custom { temperature, tint } => Some((*temperature, *tint)),
            WhiteBalance::AsShot => target_shot.map(|t| (t.cct, t.duv)),
            WhiteBalance::Offset { .. } => None,
        };
        p.edit.white_balance = match (&preset.edit.white_balance, own) {
            (WhiteBalance::Custom { temperature, tint }, Some((t, d))) if !unmoved => {
                WhiteBalance::Custom {
                    temperature: match shift.temperature {
                        Some(by) => kelvin_moved(t, by),
                        None => *temperature,
                    },
                    tint: match shift.tint {
                        Some(by) => (d + by).clamp(-0.05, 0.05),
                        None => *tint,
                    },
                }
            }
            _ => target.white_balance.clone(),
        };
    }
    if let Some(by) = shift.exposure {
        p.edit.light.exposure = (target.light.exposure + by).clamp(-5.0, 5.0);
        // A shift moves the target's own exposure, so it stays against
        // the target's own zero, not the source's.
        p.edit.light.baseline = target.light.baseline;
    }
    p
}

/// The camera profile a frame's As shot is read through once `preset`
/// is on it: the preset's when it carries the camera, the frame's own
/// otherwise.
fn profile_choice(preset: &Preset, target: &Edit) -> greycard_edit::camera::Camera {
    if preset.sections.contains(&Section::Camera) {
        preset.edit.camera.clone()
    } else {
        target.camera.clone()
    }
}

/// The light cell `c`'s camera recorded ([`shot_at`]); `None` for a
/// cell with no file.
pub(crate) fn frame_shot(
    st: &State,
    c: usize,
    camera: &greycard_edit::camera::Camera,
) -> Option<TempTint> {
    shot_at(st.path_at(c)?, camera)
}

/// The light the raw at `path` recorded, as the develop reads it
/// through `camera`'s profile (a DCP's, or the raw's own): the kelvin
/// and Duv its As shot means. Read from the raw without a sample;
/// `None` for a picture, or a raw that will not say.
fn shot_at(path: &Path, camera: &greycard_edit::camera::Camera) -> Option<TempTint> {
    crate::preset_fit::as_shot_at(path, camera)
}

/// [`shot_at`] for each of `wanted` (a cell, its raw, the profile to
/// read it through), on the pool, by cell.
fn read_whites(
    wanted: &[(usize, PathBuf, greycard_edit::camera::Camera)],
) -> HashMap<usize, Option<TempTint>> {
    use rayon::prelude::*;
    let started = std::time::Instant::now();
    let read: Vec<Option<TempTint>> = wanted
        .par_iter()
        .map(|(_, path, camera)| shot_at(path, camera))
        .collect();
    if !wanted.is_empty() {
        tracing::debug!(
            "{} as-shot whites read on the pool in {:.1} ms",
            wanted.len(),
            started.elapsed().as_secs_f64() * 1e3
        );
    }
    wanted.iter().map(|(c, ..)| *c).zip(read).collect()
}

/// Frame `from`'s edit as a sync reads it: the panel's when it is the
/// frame on screen outside culling (recorded and saved first, as
/// `sync_source` did), its sidecar's otherwise; none, said, for a
/// frame whose sidecar stands in.
fn source_edit(st: &mut State, app: &App, from: usize) -> Option<Edit> {
    if !crate::rows::is_loaded(st, from) {
        crate::rows::say_offline_for(st, app, from, &tr!(": nothing to sync from"));
        return None;
    }
    if st.current == Some(from)
        && st.culling.cull.is_none()
        && crate::rows::panel_is_frames(st, from)
    {
        let edit = read_edit(app, &st.develop.edit, st.develop.target);
        save_edit(st, edit.clone());
        return Some(edit);
    }
    st.line(from).map(|l| l.current.clone())
}

/// What a preset click does, for the current frame alone or for a
/// whole selected set, once the preset itself and the profile
/// directory are in hand: `panel::assets::on_preset_applied`'s own
/// logic, pulled out here — as `sync_selection` already is — so a
/// test can hand it fake bodies instead of going through
/// `panel::assets::profiles_for_sync` and a real file's metadata, which
/// the Slint callback itself has no way to fake.
///
/// The current frame takes the preset the way it always has (through
/// the panel outside culling, through the sidecar under it), but with
/// the same camera-profile fit check [`preset_for_body`] gives every
/// other frame in the set: a preset naming a DCP is not the open
/// frame's for the taking just because it is the one on screen. With
/// two or more frames selected, the rest of the set takes it the way
/// a sync lays sections over its targets ([`lay_over_targets`]), and
/// a left-off open frame is folded into the same status line as any
/// left-off target.
#[cfg(test)]
pub(crate) fn apply_preset(
    st: &mut State,
    app: &App,
    worker: &Worker,
    preset: &Preset,
    profiles: &[camera::Entry],
    probe: impl Fn(&State, usize) -> Option<greycard_core::decode::Probe>,
) {
    let frames = chosen_frames(st);
    apply_preset_to(st, app, worker, preset, &frames, profiles, probe);
}

/// The preset's click, from the callback: a request for the
/// selection's sidecars, and the preset over exactly those frames once
/// they are in.
pub(crate) fn preset_pressed(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, preset: Preset) {
    let paths: Vec<crate::versions::Key> = {
        let st = state.borrow();
        chosen_frames(&st)
            .into_iter()
            .filter_map(|c| st.key_of(c))
            .collect()
    };
    crate::rows::request(
        state,
        app,
        worker,
        paths,
        crate::rows::Scope::Line,
        crate::rows::Request::Preset,
        Box::new(move |state, app, worker, at| {
            let frames: Vec<usize> = at.into_iter().flatten().collect();
            let profiles = crate::panel::assets::profiles_for_sync(app);
            apply_preset_to(
                &mut state.borrow_mut(),
                app,
                worker,
                &preset,
                &frames,
                &profiles,
                probe,
            );
        }),
    );
}

/// [`apply_preset`] over `frames` rather than the selection. The frame
/// on screen takes it through the panel when it is among them; the
/// rest as a sync's targets. When the frame on screen is not among
/// them (the selection moved on while their sidecars were read), they
/// all take it as targets and the panel is left alone.
pub(crate) fn apply_preset_to(
    st: &mut State,
    app: &App,
    worker: &Worker,
    preset: &Preset,
    frames: &[usize],
    profiles: &[camera::Entry],
    probe: impl Fn(&State, usize) -> Option<greycard_core::decode::Probe>,
) {
    let preset = &curve_settled(preset, profiles);
    let Some(c) = st.current.filter(|c| frames.contains(c)) else {
        // The pressed-on frames, none of them on screen: as a sync's
        // targets, every one.
        let targets: Vec<usize> = frames.to_vec();
        if targets.is_empty() {
            return;
        }
        let label = preset_over_set_label(targets.len(), &preset.name);
        let synced = lay_over_targets(
            st,
            app,
            preset,
            &targets,
            None,
            Some(&label),
            profiles,
            probe,
        );
        let moved = synced.moved.len();
        app.set_status(preset_onto_words(st, &preset.name, moved, targets.len(), &synced).into());
        return;
    };
    // The frame on screen under a root that is offline: nothing to lay
    // the preset over, said.
    if !crate::rows::load_frame(st, app, c) {
        crate::rows::say_offline_for(st, app, c, &tr!(": the preset is not applied"));
        return;
    }
    // Loaded, the panel is the frame's own (`rows::take` opened it);
    // one still showing a stand-in is never recorded as a state.
    if st.culling.cull.is_none()
        && let Some(why) = crate::rows::not_kept(st, c)
    {
        app.set_status(why.into());
        return;
    }
    let targets: Vec<usize> = frames.iter().copied().filter(|&f| f != c).collect();
    // The same camera-profile fit check either way, one frame or a
    // set. A profile left off this way is still there to choose by
    // hand from the CAMERA PROFILE section, which warns "Made for X,
    // not Y" rather than refusing it.
    let own = preset_for_curve(st, preset, c);
    let (current_preset, current_left_off) = preset_for_body(st, &own, c, profiles, &probe);
    let (current_preset, current_demosaic_off) = preset_for_mosaic(st, &current_preset, c);
    // The same tail a set's report gives a left-off frame
    // (`preset_onto_words`, through `profile_left_off_words`), built
    // from a `Synced` of one frame so the words are exactly the same
    // whether the frame reached it alone or as part of a set.
    let left_off_tail = profile_left_off_words(
        st,
        &Synced {
            profile_left_off: if current_left_off {
                vec![c]
            } else {
                Vec::new()
            },
            demosaic_left_off: if current_demosaic_off {
                vec![c]
            } else {
                Vec::new()
            },
            ..Synced::default()
        },
    );
    // With one frame selected, this is otherwise unchanged from
    // before the set existed: the current frame alone, on the panel
    // outside culling and on the sidecar under it.
    if targets.is_empty() {
        // In culling the panel is not the frame's: the preset goes
        // over the sidecar's current state, and the leaving develops
        // it.
        if st.culling.cull.is_some() {
            let Some(edit) = st.line(c).map(|l| l.current.clone()) else {
                return;
            };
            let applied = current_preset.applied(&edit);
            if applied == edit {
                app.set_status(
                    tr!(
                        "{} is on already{left_off_tail}",
                        preset.name,
                        left_off_tail = left_off_tail
                    )
                    .into(),
                );
                return;
            }
            if let Some(line) = st.line_mut(c) {
                line.record_as(applied, Some(preset_label(&preset.name)));
            }
            // `leave_cull(.., None)` reads the edit to leave with off
            // the sidecar it is itself about to read, so its own "a
            // control's change is written once the panel rests" never
            // sees a difference and never schedules the write: the
            // step just recorded is saved here instead.
            write_sidecar(st, c);
            // The leaving sets its own status; ours, with the
            // left-off tail leave_cull cannot know about, is the one
            // that stands once it has had its say.
            leave_cull(st, app, worker, None);
            app.set_status(
                tr!(
                    "{} applied{left_off_tail}",
                    preset.name,
                    left_off_tail = left_off_tail
                )
                .into(),
            );
            return;
        }
        // Whatever the panel holds is a state first, then the preset
        // over it.
        let edit = read_edit(app, &st.develop.edit, st.develop.target);
        let applied = current_preset.applied(&edit);
        if applied == edit {
            app.set_status(
                tr!(
                    "{} is on already{left_off_tail}",
                    preset.name,
                    left_off_tail = left_off_tail
                )
                .into(),
            );
            return;
        }
        if let Some(edit) = panel_state(st, edit)
            && let Some(line) = st.line_mut(c)
        {
            line.record(edit);
        }
        if let Some(line) = st.line_mut(c) {
            line.record_as(applied, Some(preset_label(&preset.name)));
        }
        let said = tr!(
            "{} applied{left_off_tail}",
            preset.name,
            left_off_tail = left_off_tail
        );
        // As the set's own non-culling branch below: left for the
        // develop this generation is when one is actually asked for,
        // so the tail is not lost the moment it lands.
        let before = st.develop.generation;
        take_current(st, app, worker);
        if st.develop.generation == before {
            app.set_status(said.into());
        } else {
            st.develop.status_after_develop = Some((st.develop.generation, said));
        }
        return;
    }
    // Two or more selected: the current frame takes the preset the
    // way it always has, and the rest of the set the way a sync lays
    // sections over its targets, every frame's step under the same
    // words.
    let label = preset_over_set_label(targets.len() + 1, &preset.name);
    let current_changed = if st.culling.cull.is_some() {
        let edit = st.line(c).map(|l| l.current.clone()).unwrap_or_default();
        let applied = current_preset.applied(&edit);
        let changed = applied != edit;
        if changed && let Some(line) = st.line_mut(c) {
            line.record_as(applied, Some(label.clone()));
        }
        changed
    } else {
        let edit = read_edit(app, &st.develop.edit, st.develop.target);
        let applied = current_preset.applied(&edit);
        let changed = applied != edit;
        if changed {
            if let Some(edit) = panel_state(st, edit)
                && let Some(line) = st.line_mut(c)
            {
                line.record(edit);
            }
            if let Some(line) = st.line_mut(c) {
                line.record_as(applied, Some(label.clone()));
            }
        }
        changed
    };
    let mut synced = lay_over_targets(
        st,
        app,
        preset,
        &targets,
        None,
        Some(&label),
        profiles,
        probe,
    );
    if current_left_off {
        synced.profile_left_off.push(c);
        synced.profile_left_off.sort_unstable();
    }
    if current_demosaic_off {
        synced.demosaic_left_off.push(c);
        synced.demosaic_left_off.sort_unstable();
    }
    // The current frame's step went to disk with the set's write when
    // a target shares its file (a version of it): one write, one
    // revision; else its own.
    let current_written = current_changed && wrote_file_of(st, &synced, c);
    let moved = synced.moved.len() + usize::from(current_changed);
    let asked = targets.len() + 1;
    let said = preset_onto_words(st, &preset.name, moved, asked, &synced);
    if !current_changed {
        // Nothing async is coming for the current frame: what
        // happened across the set is the word that stands.
        app.set_status(said.into());
    } else if st.culling.cull.is_some() {
        // Saved here, as the single-frame culling branch above does:
        // `leave_cull(.., None)` would otherwise never see its own
        // edit differ from the sidecar's and never schedule the
        // write.
        if !current_written {
            write_sidecar(st, c);
        }
        // The leaving develops the frame and sets its own status;
        // culling's placeholder text can replace that again before
        // the develop is on screen (`cull.rs`), so this line is not
        // guaranteed to be the one left standing the way the
        // non-culling one below is.
        leave_cull(st, app, worker, None);
        app.set_status(said.into());
    } else {
        // The develop sets its own "developing..." now. Left here for
        // that generation, `said` is the line the develop's landing
        // puts back (`Outcome::Developed` in `deliver.rs`), so it is
        // not lost the moment the develop finishes.
        let before = st.develop.generation;
        take_current_with(st, app, worker, !current_written);
        if st.develop.generation == before {
            // No develop was asked for after all: the panel's change
            // did not reach the engine.
            app.set_status(said.into());
        } else {
            st.develop.status_after_develop = Some((st.develop.generation, said));
        }
    }
}

/// The tail every report of a sync or a preset onto a set shares: the
/// frames the camera profile was left off because it was made for
/// another body, named up to three, the rest counted.
fn profile_left_off_words(st: &State, synced: &Synced) -> String {
    let named = |cells: &[usize]| {
        let names: Vec<String> = cells.iter().take(3).map(|&f| frame_name(st, f)).collect();
        let more = cells.len().saturating_sub(3);
        format!(
            "{}{}",
            names.join(", "),
            if more > 0 {
                tr!(" and {more} more", more = more)
            } else {
                String::new()
            }
        )
    };
    let mut said = String::new();
    if !synced.profile_left_off.is_empty() {
        said.push_str(&tr!(
            "; the camera profile left off {} (made for another camera)",
            named(&synced.profile_left_off)
        ));
    }
    if !synced.demosaic_left_off.is_empty() {
        said.push_str(&tr!(
            "; the demosaic left off {} (a method of another mosaic)",
            named(&synced.demosaic_left_off)
        ));
    }
    said
}

/// How much a choice carries, in the window's words: greycard-edit's
/// `Choice::words`, "3 sections", "1 part", "2 sections and 4 parts".
fn choice_words(choice: &Choice) -> String {
    carried_words(choice.sections.len(), choice.parts.len())
}

/// greycard-edit's `carried_words` in the window's words: "N sections
/// and M parts", either left out at none.
pub(crate) fn carried_words(sections: usize, parts: usize) -> String {
    let sections_said = || tr!("{n} section" | "{n} sections" % sections);
    let parts_said = || tr!("{n} part" | "{n} parts" % parts);
    match (sections, parts) {
        (_, 0) => sections_said(),
        (0, _) => parts_said(),
        _ => tr!(
            "{sections} and {parts}",
            sections = sections_said(),
            parts = parts_said()
        ),
    }
}

/// The status line after a sync: how much went where, and which
/// frames kept their own camera profile.
fn synced_words(st: &State, choice: &Choice, asked: usize, synced: &Synced) -> String {
    let mut said = if synced.moved.is_empty() {
        tr!("the other {n} frame had these already"
            | "the other {n} frames had these already" % asked)
    } else {
        tr!(
            "synced {what} onto {n} frame" | "synced {what} onto {n} frames" % synced.moved.len(),
            what = choice_words(choice)
        )
    };
    said.push_str(&profile_left_off_words(st, synced));
    said
}

/// The status line after a preset click lays itself over two or more
/// selected frames: `moved` is how many of `total` selected frames
/// took it (the current frame counted in with `synced`'s targets),
/// with the camera profile note a sync's report also carries.
pub(crate) fn preset_onto_words(
    st: &State,
    name: &str,
    moved: usize,
    total: usize,
    synced: &Synced,
) -> String {
    let mut said = if moved == 0 {
        tr!("{name} is on already", name = name)
    } else {
        tr!(
            "{name} onto {n} frame" | "{name} onto {n} frames" % moved,
            name = name
        )
    };
    let already = total.saturating_sub(moved);
    if moved > 0 && already > 0 {
        said.push_str(&tr!("; {already} had it already", already = already));
    }
    said.push_str(&profile_left_off_words(st, synced));
    said
}

/// Copy the current frame's edit, whole, into the settings clipboard:
/// the panel's outside culling, recorded or not, since what is on
/// screen is what is meant; the sidecar's in culling, where the panel
/// is not the frame's.
pub(crate) fn copy_settings(st: &mut State, app: &App) {
    let Some(c) = st.current else {
        app.set_status(tr!("open a frame to copy its settings").into());
        return;
    };
    copy_settings_of(st, app, c);
}

/// Copy frame `file`'s edit, whole, into the clipboard: the frame
/// right-clicked, for the menu's Copy. The panel's edit when `file`
/// is the frame the panel holds; its sidecar's current state
/// otherwise, brought up to date first.
pub(crate) fn copy_settings_of(st: &mut State, app: &App, cell: usize) {
    copy_of(st, app, cell, false);
}

/// A mask row's Copy masks: Copy settings of the open frame, its
/// paste's sheet opening with every mask checked and nothing else
/// (`paste_checks`), so what a paste would carry is the masks.
pub(crate) fn copy_masks(st: &mut State, app: &App) {
    let Some(c) = st.current else {
        return;
    };
    copy_of(st, app, c, true);
}

/// [`copy_settings_of`], the copy marked `masks_only` for Copy masks.
fn copy_of(st: &mut State, app: &App, cell: usize, masks_only: bool) {
    if cell >= st.cells.len() {
        return;
    }
    // The panel's edit is the frame's only when the frame's own was
    // read: under an offline root the panel shows the default that
    // stands in, and there is nothing to copy.
    if !crate::rows::load_frame(st, app, cell) {
        crate::rows::say_offline_for(st, app, cell, &tr!(": nothing to copy"));
        return;
    }
    // A frame of the export queue's view is its queued edit.
    let queued = crate::panel::queue_view::frame_of(st, cell).map(|q| q.edit.clone());
    let edit = if let Some(queued) = queued {
        queued
    } else if st.culling.cull.is_none() && st.current == Some(cell) {
        read_edit(app, &st.develop.edit, st.develop.target)
    } else {
        migrate_frame(st, cell);
        let Some(line) = st.line(cell) else {
            return;
        };
        line.current.clone()
    };
    let Some(from) = st.key_of(cell) else {
        return;
    };
    // The demosaic as this frame runs it, not a choice stored for
    // another mosaic: a paste then leaves it off the other mosaic's
    // frames.
    let edit = if edit.demosaic == greycard_edit::Demosaic::Bilinear {
        edit
    } else {
        as_source(edit, crate::panel::browser::mosaic_of(st, cell))
    };
    let mut clip = Clipboard::copy(edit, from, frame_name(st, cell));
    clip.masks_only = masks_only;
    let name = clip.name();
    if masks_only {
        tracing::info!(
            "copied the {} masks of {name} for a paste",
            clip.edit.adjustments.len()
        );
    }
    st.sheets.clipboard = Some(clip);
    app.set_clip_from(name.clone().into());
    app.set_status(tr!("copied the settings of {name}", name = name).into());
}

/// The checks a paste's sheet opens with, over the copied frame's
/// masks: the last sync's or paste's this session, else what a sync
/// checks by default. A Copy masks copy opens on every mask and
/// nothing else, the masks replacing or joining as the last sync or
/// paste had them.
fn paste_checks(st: &State, clip: &Clipboard) -> carry::Checks {
    let masks = carry::masks_of(&clip.edit);
    let last = st.sheets.sync_last.as_ref();
    let mut checks = if clip.masks_only {
        let mut checks = carry::Checks::new(masks, |s| s == Section::Adjustments);
        checks.join = last.is_some_and(|l| l.join);
        checks
    } else if let Some(last) = last {
        last.with_masks(masks)
    } else {
        carry::Checks::new(masks, Section::syncs_by_default)
    };
    // A picture's curve does not go: its row says so.
    if curveless(Some(&clip.from.path)) {
        checks.set_section(Section::DisplayCurve, false);
    }
    checks
}

/// What an applied paste leaves for the next paste's sheet: its
/// checks, or for a Copy masks copy only how its masks landed, the
/// sections staying as the last sync or paste had them.
fn paste_last(st: &State) -> carry::Checks {
    let checks = &st.sheets.sync_checks;
    if !st.sheets.clipboard.as_ref().is_some_and(|c| c.masks_only) {
        return checks.clone();
    }
    let mut last = match &st.sheets.sync_last {
        Some(last) => last.clone(),
        None => carry::Checks::new(Vec::new(), Section::syncs_by_default),
    };
    last.join = checks.join;
    last
}

/// What the paste sheet says it will do: onto which frames, and the
/// frame copied from left out when it is among them.
fn paste_words(st: &State, clip: &Clipboard, targets: &[usize]) -> String {
    let onto = match targets {
        [one] => frame_name(st, *one),
        _ => tr!("{n} frame" | "{n} frames" % targets.len()),
    };
    let set = chosen_frames(st);
    if set.len() > targets.len() {
        tr!(
            "Onto {onto}; {}, which they came from, is left as it is.",
            clip.name(),
            onto = onto
        )
    } else {
        tr!("Onto {onto}.", onto = onto)
    }
}

/// Ctrl+V, or the menu's Paste settings: the sync sheet, over the
/// selection less the frame the clipboard came from, the sections as
/// the last sync or paste had them.
pub(crate) fn paste_open(st: &mut State, app: &App) {
    let Some(clip) = st.sheets.clipboard.clone() else {
        app.set_status(
            tr!("nothing copied yet: right-click a frame for Copy settings, or Ctrl+C").into(),
        );
        return;
    };
    if tool_in_hand(app) {
        app.set_status(tr!("put the tool down first (Esc), then paste").into());
        return;
    }
    let Some(c) = st.current else {
        return;
    };
    let targets = clip.targets(st, &chosen_frames(st));
    if targets.is_empty() {
        app.set_status(
            tr!(
                "{} is the frame these came from; choose the frames to paste onto",
                clip.name()
            )
            .into(),
        );
        return;
    }
    st.sheets.sync_asked = None;
    st.sheets.sync_source = None;
    st.sheets.paste_asked = Some((c, targets.clone()));
    st.sheets.sync_checks = paste_checks(st, &clip);
    show_sync_checks(st, app);
    app.set_sync_text(paste_words(st, &clip, &targets).into());
    app.set_sync_paste(true);
    app.set_sync_open(true);
}

/// Lay what `choice` picks of the clipboard over `chosen`, the frame it came
/// from left out: a sync with the clipboard between ([`lay_paste`]),
/// each frame's step "Paste from" the frame copied. `chosen` is what
/// the paste was pressed on, whatever the selection is by the time the
/// frames' sidecars are in.
pub(crate) fn paste_onto(
    st: &mut State,
    app: &App,
    worker: &Worker,
    chosen: &[usize],
    choice: &Choice,
    profiles: &[camera::Entry],
    probe: impl Fn(&State, usize) -> Option<greycard_core::decode::Probe>,
) -> Synced {
    let Some(clip) = st.sheets.clipboard.clone() else {
        return Synced::default();
    };
    let targets = clip.targets(st, chosen);
    let preset = clip.preset(choice);
    let pasted = Pasted {
        learned_from: clip.learned_from(&preset),
        preset: &preset,
        label: clip.label(),
        what: tr!(
            "{} from {}",
            choice_words(&choice_leaving(choice, Some(&clip.from.path))),
            clip.name()
        ),
    };
    lay_paste(st, app, worker, &targets, &pasted, profiles, probe)
}

/// Paste mask: the mask clipboard's mask added to `chosen`, the frame
/// it was copied from among them, each frame that moved one step,
/// "Paste from" that frame, as a paste of that one mask with Add to
/// masks; a frame that has it already, or holds as many masks as a
/// picture may, is left as it is, the second said in the log.
pub(crate) fn paste_mask_onto(
    st: &mut State,
    app: &App,
    worker: &Worker,
    chosen: &[usize],
    profiles: &[camera::Entry],
    probe: impl Fn(&State, usize) -> Option<greycard_core::decode::Probe>,
) -> Synced {
    let Some(clip) = st.sheets.mask_clipboard.clone() else {
        return Synced::default();
    };
    let targets: Vec<usize> = chosen
        .iter()
        .copied()
        .filter(|&c| st.key_of(c).is_some())
        .collect();
    let pasted = Pasted {
        learned_from: None,
        preset: &clip.preset,
        label: clip.label(),
        what: tr!(
            "{} from {}",
            choice_words(&Choice::whole(&clip.preset.sections)),
            clip.name()
        ),
    };
    let synced = lay_paste(st, app, worker, &targets, &pasted, profiles, probe);
    let full = targets
        .iter()
        .filter(|c| !synced.moved.contains(c))
        .filter(|&&c| {
            st.line(c)
                .is_some_and(|l| l.current.adjustments.len() >= greycard_edit::MAX_MASKS)
        })
        .count();
    if full > 0 {
        tracing::info!(
            "paste mask: {full} frame(s) at {} masks left as they were",
            greycard_edit::MAX_MASKS
        );
    }
    synced
}

/// What a paste lays over the frames it reaches: the preset, the edit
/// its Noise brings the learned denoiser from, each step's words, and
/// what the status line says went ("1 section from IMG_0001.CR3").
struct Pasted<'a> {
    preset: &'a Preset,
    learned_from: Option<&'a Edit>,
    label: String,
    what: String,
}

/// Lay `pasted` over `targets`. The frame on screen, outside culling,
/// takes it through the panel as a preset click does (the
/// camera-profile fit checked the same way); the rest, and in culling
/// every frame, through [`lay_over_targets`], exactly as a sync's
/// targets. Each frame that moved gets one step. The status line says
/// what happened, and rides the develop the frame on screen asks for.
fn lay_paste(
    st: &mut State,
    app: &App,
    worker: &Worker,
    targets: &[usize],
    pasted: &Pasted,
    profiles: &[camera::Entry],
    probe: impl Fn(&State, usize) -> Option<greycard_core::decode::Probe>,
) -> Synced {
    let preset = &curve_settled(pasted.preset, profiles);
    let label = &pasted.label;
    // Outside culling the frame on screen is the panel's, and what
    // the panel holds is a state first, then the paste over it.
    let panel_frame = st.current.filter(|&c| {
        st.culling.cull.is_none() && targets.contains(&c) && crate::rows::panel_is_frames(st, c)
    });
    let mut current_changed = false;
    let mut current_left_off = false;
    let mut current_demosaic_off = false;
    if let Some(c) = panel_frame {
        let fits = preset_for_curve(st, preset, c);
        let (fits, left_off) = preset_for_body(st, &fits, c, profiles, &probe);
        let (fits, demosaic_off) = preset_for_mosaic(st, &fits, c);
        current_left_off = left_off;
        current_demosaic_off = demosaic_off;
        let edit = read_edit(app, &st.develop.edit, st.develop.target);
        let applied = crate::clipboard::laid(&fits, pasted.learned_from, &edit);
        if applied != edit {
            if let Some(edit) = panel_state(st, edit)
                && let Some(line) = st.line_mut(c)
            {
                line.record(edit);
            }
            if let Some(line) = st.line_mut(c) {
                line.record_as(applied, Some(label.clone()));
            }
            current_changed = true;
        }
    }
    let rest: Vec<usize> = targets
        .iter()
        .copied()
        .filter(|&f| Some(f) != panel_frame)
        .collect();
    let mut synced = lay_over_targets(
        st,
        app,
        preset,
        &rest,
        pasted.learned_from,
        Some(label),
        profiles,
        probe,
    );
    // A person picked on one of them is offered for the rest.
    crate::panel::parts_all::reached(st, app, preset, targets);
    // The current frame's step went to disk with the set's write when
    // a target shares its file (a version of it): one write, one
    // revision; else its own, below.
    let current_written =
        current_changed && panel_frame.is_some_and(|c| wrote_file_of(st, &synced, c));
    if let Some(c) = panel_frame {
        if current_changed {
            synced.moved.push(c);
            synced.moved.sort_unstable();
        }
        if current_left_off {
            synced.profile_left_off.push(c);
            synced.profile_left_off.sort_unstable();
        }
        if current_demosaic_off {
            synced.demosaic_left_off.push(c);
            synced.demosaic_left_off.sort_unstable();
        }
    }
    let mut said = if synced.moved.is_empty() {
        tr!("the {n} frame had these already" | "the {n} frames had these already" % targets.len())
    } else {
        tr!(
            "pasted {what} onto {n} frame" | "pasted {what} onto {n} frames" % synced.moved.len(),
            what = pasted.what
        )
    };
    said.push_str(&profile_left_off_words(st, &synced));
    if current_changed {
        // As a preset click's: the develop sets its own words, and
        // these go first once it lands.
        let before = st.develop.generation;
        take_current_with(st, app, worker, !current_written);
        if st.develop.generation == before {
            app.set_status(said.into());
        } else {
            st.develop.status_after_develop = Some((st.develop.generation, said));
        }
    } else {
        app.set_status(said.into());
    }
    synced
}

/// The sync sheet's ask, ready to open: the frame and the targets it
/// names, the source as it stands, and the sheet's default checks.
/// False, with words on the status line where there are any, when
/// there is nothing to sync.
fn sync_ready(st: &mut State, app: &App) -> bool {
    if crate::panel::queue_view::refused(st, "a sync") {
        return false;
    }
    if tool_in_hand(app) {
        app.set_status(tr!("put the tool down first (Esc), then sync").into());
        return false;
    }
    let (Some(c), targets) = (st.current, sync_targets(st)) else {
        return false;
    };
    if targets.is_empty() {
        app.set_status(tr!("choose the frames to sync onto: Ctrl+click or Shift+click").into());
        return false;
    }
    st.sheets.sync_asked = Some((c, targets));
    st.sheets.paste_asked = None;
    app.set_sync_paste(false);
    let shown = edit_shown(st, app, c);
    let shot = if shown.white_balance == WhiteBalance::AsShot {
        None
    } else {
        frame_shot(st, c, &shown.camera)
    };
    st.sheets.sync_source = Some((shown.clone(), shot));
    let masks = carry::masks_of(&shown);
    st.sheets.sync_checks = carry::Checks::new(masks, Section::syncs_by_default);
    if curveless(st.path_at(c).map(PathBuf::as_path)) {
        st.sheets
            .sync_checks
            .set_section(Section::DisplayCurve, false);
    }
    true
}

/// Whether a viewport tool is in hand, as the window's
/// `tool-in-hand` has it: the sheet is not opened over one.
fn tool_in_hand(app: &App) -> bool {
    !app.get_placing().is_empty()
        || !app.get_picking().is_empty()
        || !app.get_guide_mode().is_empty()
        || app.get_level_mode()
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    // Copy and paste: Ctrl+C and Ctrl+V, and the frame menu's two.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_copy_asked(move || {
            if let Some(app) = app_weak.upgrade() {
                copy_settings(&mut state.borrow_mut(), &app);
            }
        });
    }
    // A mask row's Copy masks: the open frame copied for its masks.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_masks_copied(move || {
            if let Some(app) = app_weak.upgrade() {
                copy_masks(&mut state.borrow_mut(), &app);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_paste_asked(move || {
            if let Some(app) = app_weak.upgrade()
                && !crate::panel::queue_view::refused(&state.borrow(), "a paste")
            {
                paste_open(&mut state.borrow_mut(), &app);
            }
        });
    }
    // Paste mask: the mask clipboard's mask onto the open frame or
    // every selected frame, once their sidecars are in.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_mask_pasted(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            if st.sheets.mask_clipboard.is_none()
                || crate::panel::queue_view::refused(&st, "a paste")
            {
                return;
            }
            if tool_in_hand(&app) {
                app.set_status(tr!("put the tool down first (Esc), then paste").into());
                return;
            }
            let paths: Vec<crate::versions::Key> = chosen_frames(&st)
                .into_iter()
                .filter_map(|c| st.key_of(c))
                .collect();
            drop(st);
            crate::rows::request(
                &state,
                &app,
                &worker,
                paths,
                crate::rows::Scope::Line,
                crate::rows::Request::Paste,
                Box::new(move |state, app, worker, at| {
                    let chosen: Vec<usize> = at.into_iter().flatten().collect();
                    let profiles = crate::panel::assets::profiles_for_sync(app);
                    paste_mask_onto(
                        &mut state.borrow_mut(),
                        app,
                        worker,
                        &chosen,
                        &profiles,
                        probe,
                    );
                }),
            );
        });
    }
    // The sheet, on everything but the masks.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_sync_open_asked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if !sync_ready(&mut st, &app) {
                return;
            }
            show_sync_checks(&mut st, &app);
            app.set_sync_text(sync_words(&st).into());
            app.set_sync_open(true);
        });
    }
    // A middle click on the panel's Sync button: the open frame's
    // sync without the sheet, on the choice the last sync or paste
    // took, set as a frame's middle click is.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_sync_quick_asked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let current = state.borrow().current;
            if let Some(from) = current {
                sync_quick(&state, &app, &worker, from);
            }
        });
    }
    // A shift chosen or set back.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_sync_shift_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                show_shift(&state.borrow(), &app);
            }
        });
    }
    // The sheet's checks: a row, all of them, and how the masks land.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_sync_row_toggled(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if let Some(target) = st.sheets.sync_rows.target(i) {
                st.sheets.sync_checks.toggle(target);
                show_sync_checks(&mut st, &app);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_sync_all(move |on| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.sheets.sync_checks.set_all(on);
            show_sync_checks(&mut st, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_sync_join_chosen(move |join| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.sheets.sync_checks.join = join;
            show_sync_checks(&mut st, &app);
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_sync_applied(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let choice = st.sheets.sync_checks.choice();
            if choice.is_empty() {
                app.set_status(tr!("choose what goes across").into());
                return;
            }
            // Read before the sheet closes: closing it clears the flag.
            let paste = app.get_sync_paste();
            app.set_sync_open(false);
            if paste {
                app.set_sync_paste(false);
                // As a sync's: what the sheet named, or nothing.
                let asked = st.sheets.paste_asked.take();
                let now = match (&st.sheets.clipboard, st.current) {
                    (Some(clip), Some(c)) => Some((c, clip.targets(&st, &chosen_frames(&st)))),
                    _ => None,
                };
                if asked.is_none() || asked != now {
                    tracing::warn!("paste refused: the selection moved while the sheet was open");
                    app.set_status(
                        tr!("the selection changed while the sheet was open; nothing pasted")
                            .into(),
                    );
                    return;
                }
                st.sheets.sync_last = Some(paste_last(&st));
                // A request for the chosen frames' sidecars, and the
                // paste over exactly those frames once they are in.
                let paths: Vec<crate::versions::Key> = chosen_frames(&st)
                    .into_iter()
                    .filter_map(|c| st.key_of(c))
                    .collect();
                drop(st);
                crate::rows::request(
                    &state,
                    &app,
                    &worker,
                    paths,
                    crate::rows::Scope::Line,
                    crate::rows::Request::Paste,
                    Box::new(move |state, app, worker, at| {
                        let chosen: Vec<usize> = at.into_iter().flatten().collect();
                        let profiles = crate::panel::assets::profiles_for_sync(app);
                        let start = std::time::Instant::now();
                        let synced = paste_onto(
                            &mut state.borrow_mut(),
                            app,
                            worker,
                            &chosen,
                            &choice,
                            &profiles,
                            probe,
                        );
                        tracing::info!(
                            "pasted onto {} frames in {:.1} ms",
                            synced.moved.len(),
                            start.elapsed().as_secs_f64() * 1e3
                        );
                    }),
                );
                return;
            }
            // What the sheet said is what is done, or nothing: a
            // selection that moved while the sheet was up is not the
            // one its words named.
            let asked = st.sheets.sync_asked.take();
            let targets = sync_targets(&st);
            let now = st.current.map(|c| (c, targets.clone()));
            if asked.is_none() || asked != now {
                tracing::warn!("sync refused: the selection moved while the sheet was open");
                app.set_status(
                    tr!("the selection changed while the sheet was open; nothing synced").into(),
                );
                return;
            }
            if targets.is_empty() {
                app.set_status(tr!("nothing to sync onto").into());
                return;
            }
            // A paste's sheet opens on this choice from now on.
            st.sheets.sync_last = Some(st.sheets.sync_checks.clone());
            let modes = shift_modes(&app);
            // A request for the source's and the targets' sidecars, and
            // the sync from exactly that frame onto exactly those once
            // they are in: the first path the source, the rest the
            // targets.
            let Some(c) = st.current else {
                return;
            };
            let paths: Vec<crate::versions::Key> = std::iter::once(c)
                .chain(targets.iter().copied())
                .filter_map(|c| st.key_of(c))
                .collect();
            drop(st);
            run_sync(&state, &app, &worker, paths, Some(choice), modes, None);
        });
    }
    // A middle click on a frame of the strip or the grid: that frame's
    // sync onto the rest of the selection, without the sheet.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_frame_sync_asked(move |row| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let from = usize::try_from(row)
                .ok()
                .and_then(|r| state.borrow().views.shown.get(r).copied());
            if let Some(from) = from {
                sync_quick(&state, &app, &worker, from);
            }
        });
    }
}

#[cfg(test)]
mod tests;
