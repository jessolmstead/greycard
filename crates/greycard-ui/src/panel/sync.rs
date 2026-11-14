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
/// `greycard_edit_io::profiles::list` reads it:
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
    let profiles = greycard_edit_io::profiles::list();
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
/// `greycard_edit_io::profiles::list` and a real file's metadata, which
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
            let profiles = greycard_edit_io::profiles::list();
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
    let preset = pasted.preset;
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
                    let profiles = greycard_edit_io::profiles::list();
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
                        let profiles = greycard_edit_io::profiles::list();
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
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::filter;
    use crate::finish::MAX_LOCALS;
    use crate::panel::browser::open_row;
    use crate::testing::{click, folder, press, state_for, strip_cell, window};
    use greycard_edit::mask::{Component, Mask, Shape};
    use greycard_edit::preset::Entry;
    use greycard_edit::{Learned, Sidecar, preset};
    use slint::Model;
    use slint::platform::{Key, WindowEvent};

    /// The sheet's headings as its rows read: each section's title
    /// and how its checks stand, 0 off, 1 some, 2 on.
    fn headings(app: &App) -> Vec<(String, i32)> {
        let rows = app.get_sync_rows();
        (0..rows.row_count())
            .filter_map(|i| rows.row_data(i))
            .filter(|r| r.kind == 0)
            .map(|r| (r.label.to_string(), r.state))
            .collect()
    }

    /// The headings as a sync's sheet opens on them: all on but the
    /// masks.
    fn defaults() -> Vec<(String, i32)> {
        Section::ALL
            .iter()
            .map(|s| {
                (
                    s.title().to_string(),
                    if s.syncs_by_default() { 2 } else { 0 },
                )
            })
            .collect()
    }

    /// The sections `on` names checked whole and nothing else, as the
    /// sheet's Uncheck all and a click on each heading do.
    fn choose(app: &App, on: impl Fn(Section) -> bool) {
        app.invoke_sync_all(false);
        let rows = app.get_sync_rows();
        for i in 0..rows.row_count() {
            let r = rows.row_data(i).unwrap();
            if r.kind == 0 && Section::ALL.iter().any(|s| r.label == s.title() && on(*s)) {
                app.invoke_sync_row_toggled(i as i32);
            }
        }
    }

    fn with_modifier(app: &App, key: Key, f: impl FnOnce()) {
        app.window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        f();
        app.window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }

    #[test]
    fn ctrl_click_grows_the_set_and_the_sync_sheet_opens_on_it() {
        let app = window(6);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, folder(6));
        // A frame open, the set that frame alone.
        let (x, y) = strip_cell(&app, 1);
        click(&app, x, y);
        assert_eq!(state.borrow().current, Some(1));
        assert_eq!(app.get_set_count(), 1);
        // Nothing to sync onto yet: the key does nothing.
        press(&app, Key::Escape);
        with_modifier(&app, Key::Control, || {
            with_modifier(&app, Key::Shift, || press(&app, "S"));
        });
        assert!(!app.get_sync_open());

        // Ctrl+click two more: the set grows, the frame on screen
        // does not move, and the rows say which are chosen.
        with_modifier(&app, Key::Control, || {
            let (x, y) = strip_cell(&app, 3);
            click(&app, x, y);
            let (x, y) = strip_cell(&app, 4);
            click(&app, x, y);
        });
        assert_eq!(chosen_frames(&state.borrow()), vec![1, 3, 4]);
        assert_eq!(state.borrow().current, Some(1));
        assert_eq!(app.get_selected(), 1);
        assert_eq!(app.get_set_count(), 3);
        let rows = app.get_thumbs();
        let chosen: Vec<bool> = (0..6).map(|r| rows.row_data(r).unwrap().chosen).collect();
        assert_eq!(chosen, [false, true, false, true, true, false]);

        // Ctrl+Shift+S opens the sheet, everything on but the masks.
        with_modifier(&app, Key::Control, || {
            with_modifier(&app, Key::Shift, || press(&app, "S"));
        });
        assert!(app.get_sync_open());
        assert_eq!(
            app.get_sync_text(),
            "From IMG_0001.CR3 onto 2 other frames."
        );
        let heads = headings(&app);
        assert_eq!(heads.len(), Section::ALL.len());
        for ((name, on), s) in heads.iter().zip(Section::ALL) {
            assert_eq!(name, s.title());
            assert_eq!(*on, if *s == Section::Adjustments { 0 } else { 2 }, "{s:?}");
        }
        // Escape leaves the sheet, then a second one the set.
        press(&app, Key::Escape);
        assert!(!app.get_sync_open());
        assert_eq!(app.get_set_count(), 3);
        press(&app, Key::Escape);
        assert_eq!(app.get_set_count(), 1);
        assert_eq!(chosen_frames(&state.borrow()), vec![1]);
    }

    #[test]
    fn shift_click_and_shift_arrows_extend_and_a_plain_arrow_collapses() {
        let app = window(6);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, folder(6));
        app.invoke_select(1);
        // Shift+click four rows along: the run, the anchor kept.
        with_modifier(&app, Key::Shift, || {
            let (x, y) = strip_cell(&app, 4);
            click(&app, x, y);
        });
        assert_eq!(chosen_frames(&state.borrow()), vec![1, 2, 3, 4]);
        assert_eq!(state.borrow().current, Some(1));
        // A plain click on the frame on screen: back to it alone.
        let (x, y) = strip_cell(&app, 1);
        click(&app, x, y);
        assert_eq!(chosen_frames(&state.borrow()), vec![1]);
        // Shift and the arrow: the current frame moves and the set
        // keeps what it had.
        with_modifier(&app, Key::Shift, || {
            press(&app, Key::RightArrow);
            press(&app, Key::RightArrow);
        });
        assert_eq!(state.borrow().current, Some(3));
        assert_eq!(chosen_frames(&state.borrow()), vec![1, 2, 3]);
        // The meta keys reach the whole set.
        press(&app, "4");
        let st = state.borrow();
        let ratings: Vec<u8> = st.sidecars.iter().map(|s| s.meta.rating).collect();
        assert_eq!(ratings, [0, 4, 4, 4, 0, 0]);
        drop(st);
        // A plain arrow collapses to the frame it lands on.
        press(&app, Key::RightArrow);
        assert_eq!(state.borrow().current, Some(4));
        assert_eq!(chosen_frames(&state.borrow()), vec![4]);
        assert_eq!(app.get_set_count(), 1);
    }

    #[test]
    fn a_sync_onto_unindexed_frames_reads_their_mosaics_off_the_window() {
        let dir =
            std::env::temp_dir().join(format!("greycard-sync-mosaics-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..4)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        for f in &files {
            std::fs::write(f, b"not a raw").expect("a file");
        }
        let app = window(4);
        let (state, worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.window.placement = greycard_edit::Placement::Folder;
        }
        crate::rows::HOLD_MOSAICS.with(|h| h.set(true));
        app.invoke_select(0);
        app.set_exposure(1.25);
        state.borrow_mut().views.picked = vec![0, 1, 2, 3];
        app.invoke_sync_open_asked();
        app.invoke_sync_applied();
        // The press returns with the reads out and nothing laid yet.
        assert_eq!(app.get_status(), "reading 4 frames...");
        assert!(
            state.borrow().sidecars[1..]
                .iter()
                .all(|s| s.history.is_empty())
        );
        assert_eq!(state.borrow().views.loads.whats(), ["sync"]);
        // In, the sync runs on what they said.
        assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
        assert_eq!(app.get_status(), "synced 19 sections onto 3 frames");
        {
            let st = state.borrow();
            for s in &st.sidecars[1..] {
                assert_eq!(s.current.light.exposure, 1.25);
            }
            assert!(st.views.loads.queue.is_empty());
        }
        for f in &files[1..] {
            let back = Sidecar::load(f).unwrap().expect("the sidecar written");
            assert_eq!(back.current.light.exposure, 1.25);
        }
        // Read once a session: the next sync runs at once.
        app.set_exposure(-0.5);
        app.invoke_sync_open_asked();
        app.invoke_sync_applied();
        assert_eq!(app.get_status(), "synced 19 sections onto 3 frames");
        assert_eq!(crate::rows::land_pending(&state, &app, &worker), 0);
        assert_eq!(state.borrow().sidecars[3].current.light.exposure, -0.5);
        crate::rows::HOLD_MOSAICS.with(|h| h.set(false));
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_sidecar_read_landing_under_a_mosaics_read_leaves_no_bar_behind() {
        let dir = std::env::temp_dir().join(format!("greycard-sync-bar-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..3)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        for f in &files {
            std::fs::write(f, b"not a raw").expect("a file");
        }
        let app = window(3);
        let (state, worker) = state_for(&app, files.clone());
        crate::rows::HOLD_MOSAICS.with(|h| h.set(true));
        app.invoke_select(0);
        app.set_exposure(1.25);
        state.borrow_mut().views.picked = vec![0, 1];
        // A frame standing in from its row, for a request behind the
        // sync that reads its sidecar.
        state.borrow_mut().from_row[2].read = false;
        app.invoke_sync_open_asked();
        app.invoke_sync_applied();
        let ran = Rc::new(std::cell::Cell::new(false));
        let r = ran.clone();
        let key = state.borrow().key_of(2).unwrap();
        crate::rows::request(
            &state,
            &app,
            &worker,
            vec![key],
            crate::rows::Scope::File,
            crate::rows::Request::RatingKey,
            Box::new(move |_, _, _, _| r.set(true)),
        );
        assert_eq!(state.borrow().views.loads.whats(), ["sync", "rating key"]);
        // The sidecar read lands first, the request waiting its turn;
        // then the mosaics, and both run, the bar gone with them.
        let mosaics = crate::rows::read_held_mosaics();
        crate::rows::land_pending(&state, &app, &worker);
        assert!(!ran.get());
        crate::rows::land_mosaics(&state, &app, &worker, mosaics);
        assert!(ran.get());
        assert_eq!(app.get_status(), "synced 19 sections onto 1 frame");
        assert!(state.borrow().views.loads.since.is_none());
        let later = std::time::Instant::now() + std::time::Duration::from_secs(90);
        crate::rows::bar_tick(&state, &app, &worker, later);
        assert!(!app.get_loading_shown());
        assert_eq!(app.get_status(), "synced 19 sections onto 1 frame");
        crate::rows::HOLD_MOSAICS.with(|h| h.set(false));
        crate::testing::remove_dir_retry(&dir);
    }

    /// The window's longest stall over a sync from the first frame in
    /// `GREYCARD_SYNC_FRAMES` onto the rest, unindexed: the press, and
    /// the run once the mosaics are in, with the read in place (as
    /// before) and held off the window.
    #[test]
    #[ignore]
    fn measure_a_sync_onto_unindexed_frames() {
        let dir = PathBuf::from(std::env::var("GREYCARD_SYNC_FRAMES").expect("a folder"));
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .expect("the folder")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "CR3"))
            .collect();
        files.sort();
        for hold in [false, true, false, true] {
            let app = window(files.len());
            let (state, worker) = state_for(&app, files.clone());
            crate::rows::HOLD_MOSAICS.with(|h| h.set(hold));
            app.invoke_select(0);
            app.set_exposure(0.5);
            state.borrow_mut().views.picked = (0..files.len()).collect();
            app.invoke_sync_open_asked();
            let t = std::time::Instant::now();
            app.invoke_sync_applied();
            let press = t.elapsed();
            let t = std::time::Instant::now();
            let read = crate::rows::read_held_mosaics();
            let off: usize = read.iter().map(|(_, r)| r.len()).sum();
            let read_took = t.elapsed();
            let t = std::time::Instant::now();
            crate::rows::land_mosaics(&state, &app, &worker, read);
            let run = t.elapsed();
            eprintln!(
                "hold {hold}: press {:.1} ms, {off} mosaics off the window {:.1} ms, run {:.1} ms; {}",
                press.as_secs_f64() * 1e3,
                read_took.as_secs_f64() * 1e3,
                run.as_secs_f64() * 1e3,
                app.get_status()
            );
        }
        crate::rows::HOLD_MOSAICS.with(|h| h.set(false));
    }

    #[test]
    fn apply_lays_the_current_frame_over_the_others_and_saves_them() {
        let dir = std::env::temp_dir().join(format!("greycard-sync-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..3)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let app = window(3);
        let (state, _worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.window.placement = greycard_edit::Placement::Folder;
            // The frame the sync goes onto has a look of its own and
            // a crop.
            let mut own = Edit::default();
            own.sharpen.radius = 2.0;
            own.geometry.crop = Some(greycard_edit::geometry::Crop {
                x: 0.0,
                y: 0.0,
                w: 0.8,
                h: 0.8,
            });
            st.sidecars[2].record(own);
        }
        app.invoke_select(0);
        // The panel is the frame's: an exposure moved on it.
        app.set_exposure(1.25);
        {
            let mut st = state.borrow_mut();
            st.views.picked = vec![0, 2];
        }
        app.invoke_sync_open_asked();
        assert!(app.get_sync_open());
        app.invoke_sync_applied();
        assert!(!app.get_sync_open());
        assert_eq!(app.get_status(), "synced 19 sections onto 1 frame");

        let st = state.borrow();
        // The frame synced from kept what it had and recorded the
        // panel's state; the frame outside the set is untouched.
        assert_eq!(st.sidecars[0].current.light.exposure, 1.25);
        assert_eq!(st.sidecars[1], Sidecar::default());
        // The frame synced onto has the exposure, keeps its crop,
        // and has one step more.
        let s = &st.sidecars[2];
        assert_eq!(s.current.light.exposure, 1.25);
        assert_eq!(s.current.sharpen, Edit::default().sharpen);
        assert!(s.current.geometry.crop.is_some());
        assert_eq!(s.history.len(), 2);
        // Its step is named for the frame it came from; the frame
        // synced from recorded its panel as a plain step.
        assert_eq!(s.current_label.as_deref(), Some("Sync from IMG_0000.CR3"));
        assert_eq!(st.sidecars[0].current_label, None);
        // Written where the setting says, and it reads back the same.
        let on_disk = Sidecar::path_in(&files[2], greycard_edit::Placement::Folder);
        assert!(on_disk.exists(), "{}", on_disk.display());
        let back = Sidecar::load(&files[2]).unwrap().unwrap();
        assert_eq!(back.current, s.current);
        assert_eq!(back.current_label, s.current_label);
        assert!(!Sidecar::path_in(&files[1], greycard_edit::Placement::Folder).exists());
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    fn entry(name: &str, made_for: Option<&str>) -> camera::Entry {
        camera::Entry {
            name: name.into(),
            path: PathBuf::new(),
            title: None,
            unique_camera_model: made_for.map(str::to_string),
            copyright: None,
            same_as: None,
        }
    }

    fn body(make: &str, model: &str, iso: u32) -> greycard_core::decode::Probe {
        greycard_core::decode::Probe {
            make: make.into(),
            model: model.into(),
            other_names: Vec::new(),
            iso: Some(iso),
            exposure_time: None,
            fnumber: None,
            ..Default::default()
        }
    }

    #[test]
    fn a_profile_fits_only_the_body_it_was_made_for() {
        let r5 = entry("r5", Some("Canon EOS R5"));
        let fits = |e: Option<&camera::Entry>, body: Option<(&str, &str)>| {
            profile_fits(
                e,
                body.map(|(make, model)| camera::Body::new(make, model, &[])),
            )
        };
        assert!(fits(Some(&r5), Some(("Canon", "EOS R5"))));
        assert!(!fits(Some(&r5), Some(("Fujifilm", "X-T5"))));
        // A frame that cannot say what took it does not get it.
        assert!(!fits(Some(&r5), None));
        // A profile that names no camera, or one the directory has
        // not got, cannot be checked, and goes as a preset's would.
        assert!(fits(Some(&entry("any", None)), None));
        assert!(fits(Some(&entry("any", Some(" "))), None));
        assert!(fits(None, Some(("Fujifilm", "X-T5"))));
    }

    /// A preset's profile made under a body's other name goes to the
    /// body's frames: the probe carries the names the decoder gives.
    #[test]
    fn a_preset_s_profile_fits_a_body_by_its_other_name() {
        let mut edit = Edit::default();
        edit.camera.profile = ProfileChoice::Named("t8i".into());
        let preset = Preset::from_edit("Rebel", &edit, &[Section::Camera]);
        let profiles = [entry("t8i", Some("Canon EOS Rebel T8i"))];
        let mut probe = body("Canon", "Canon EOS 850D", 100);
        probe.other_names =
            greycard_core::decode::rawler_backend::other_names("Canon", "Canon EOS 850D");
        let (_, left_off) = preset_fitted(&preset, &profiles, || Some(probe.clone()));
        assert!(!left_off);
        probe.other_names.clear();
        let (_, left_off) = preset_fitted(&preset, &profiles, || Some(probe));
        assert!(left_off);
    }

    /// A body whose EXIF make is not its clean make: the open frame's
    /// names (clean) and a probe's (as the EXIF spells them) fit the
    /// same profile under the body's other name.
    #[test]
    fn a_body_fits_by_its_other_name_whichever_make_names_it() {
        use greycard_core::decode::rawler_backend::other_names;
        let d800e = entry("d800e", Some("Nikon D800E"));
        for (make, model) in [("Nikon", "D800"), ("NIKON CORPORATION", "NIKON D800")] {
            let mut probe = body(make, model, 100);
            probe.other_names = other_names(make, model);
            assert!(
                profile_fits(Some(&d800e), Some(probed_body(&probe))),
                "{make} {model}"
            );
            assert!(d800e.fits(probed_body(&probe)), "{make} {model}");
        }
    }

    /// A Canon DCP synced over a Canon frame and a Fuji frame lands
    /// on the Canon alone, and the Fuji takes everything else; a
    /// frame still waiting on its ISO's learned blend is given it
    /// before the sync makes its edit no longer the default.
    #[test]
    fn a_profile_skips_another_body_and_a_waiting_blend_is_seeded() {
        let app = window(3);
        let (state, worker) = state_for(&app, folder(3));
        {
            let mut st = state.borrow_mut();
            st.sidecars[0].current.camera.profile = ProfileChoice::Named("r5".into());
            st.sidecars[0].current.light.exposure = 0.5;
            // Frames 1 and 2 are raws never developed: each waits for
            // its ISO's blend, as its own line says.
            assert!(!crate::rows::wants_seed(&st, 0));
            assert!(crate::rows::wants_seed(&st, 1));
            assert!(crate::rows::wants_seed(&st, 2));
        }
        open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
        state.borrow_mut().views.picked = vec![0, 1, 2];
        let profiles = [entry("r5", Some("Canon EOS R5"))];
        let bodies = |_: &State, f: usize| match f {
            1 => Some(body("Canon", "EOS R5", 3200)),
            2 => Some(body("Fujifilm", "X-T5", 100)),
            _ => None,
        };
        let sections = [Section::Camera, Section::Light];
        let synced = sync_selection(&mut state.borrow_mut(), &app, &sections, &profiles, bodies);
        assert_eq!(synced.moved, [1, 2]);
        assert_eq!(synced.profile_left_off, [2]);
        let st = state.borrow();
        assert_eq!(st.sidecars[1].current.camera.profile.name(), "r5");
        assert!(st.sidecars[2].current.camera.profile.is_embedded());
        assert_eq!(st.sidecars[1].current.light.exposure, 0.5);
        assert_eq!(st.sidecars[2].current.light.exposure, 0.5);
        // Noise was not synced, so each frame's own ISO seeded its
        // blend, under the sync's step, and is not seeded again.
        let blend = |iso| greycard_edit::Noise::blend_for_iso(Some(iso));
        assert_eq!(
            st.sidecars[1].history[0].edit.noise.learned_strength,
            blend(3200)
        );
        assert_eq!(st.sidecars[2].current.noise.learned_strength, blend(100));
        assert!((0..3).all(|c| !crate::rows::wants_seed(&st, c)));
        assert!(
            synced_words(
                &st,
                &Choice::whole(&[Section::Light, Section::Camera]),
                2,
                &synced
            )
            .ends_with("; the camera profile left off IMG_0002.CR3 (made for another camera)")
        );
        drop(st);

        // With Noise synced the blend is the source's, and nothing
        // is seeded over it on a first open: the lines are at their
        // defaults again, waiting.
        {
            let mut st = state.borrow_mut();
            for f in [1, 2] {
                st.sidecars[f] = Sidecar::default();
            }
            assert!(crate::rows::wants_seed(&st, 1) && crate::rows::wants_seed(&st, 2));
        }
        app.set_denoise_learned_strength(0.6);
        let synced = sync_selection(
            &mut state.borrow_mut(),
            &app,
            &[Section::Noise],
            &profiles,
            |_, _| panic!("nothing to read a frame for"),
        );
        assert_eq!(synced.moved, [1, 2]);
        let st = state.borrow();
        assert!((0..3).all(|c| !crate::rows::wants_seed(&st, c)));
        assert_eq!(
            st.sidecars[2].current.noise.learned_strength,
            st.sidecars[0].current.noise.learned_strength
        );
    }

    /// A sync of the Light section, or of its exposure alone, from an
    /// older edit that keeps the zero it was made against takes that
    /// zero with it, so the target lands where the source sits; from a
    /// frame read off its sidecar as from the one on screen. A new edit
    /// has none to give, and the target's exposure counts from its own
    /// body's default. A shift moves the target's own exposure, so the
    /// target keeps its own zero.
    #[test]
    fn a_sync_carries_the_exposure_s_zero_with_the_exposure() {
        use greycard_edit::preset::{Choice, Part};
        let older = Some(greycard_edit::baseline::BEFORE_BODIES);
        let app = window(3);
        let (state, worker) = state_for(&app, folder(3));
        {
            let mut st = state.borrow_mut();
            st.sidecars[0].current.light.exposure = 0.3;
            st.sidecars[0].current.light.baseline = older;
        }
        let none = |_: &State, _: usize| None;
        let exposure = Choice {
            sections: Vec::new(),
            parts: vec![Part::LightExposure],
            masks: None,
            masks_land: Default::default(),
        };
        for choice in [Choice::whole(&[Section::Light]), exposure] {
            {
                let mut st = state.borrow_mut();
                for f in [1, 2] {
                    st.sidecars[f] = Sidecar::default();
                }
            }
            let (synced, _) = sync_onto(
                &mut state.borrow_mut(),
                &app,
                0,
                &[1],
                &choice,
                Default::default(),
                &[],
                none,
            );
            assert_eq!(synced.moved, [1]);
            let st = state.borrow();
            assert_eq!(st.sidecars[1].current.light.exposure, 0.3);
            assert_eq!(st.sidecars[1].current.light.baseline, older, "{choice:?}");
        }
        // From the frame on screen, through the panel.
        open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
        state.borrow_mut().sidecars[2] = Sidecar::default();
        let choice = Choice::whole(&[Section::Light]);
        sync_onto(
            &mut state.borrow_mut(),
            &app,
            0,
            &[2],
            &choice,
            Default::default(),
            &[],
            none,
        );
        assert_eq!(state.borrow().sidecars[2].current.light.baseline, older);
        // A new edit carries none, over an older one's.
        {
            let mut st = state.borrow_mut();
            st.sidecars[1].current.light.baseline = older;
            st.sidecars[2].current.light.baseline = None;
            st.sidecars[2].current.light.exposure = 0.5;
        }
        sync_onto(
            &mut state.borrow_mut(),
            &app,
            2,
            &[1],
            &choice,
            Default::default(),
            &[],
            none,
        );
        {
            let st = state.borrow();
            assert_eq!(st.sidecars[1].current.light.exposure, 0.5);
            assert_eq!(st.sidecars[1].current.light.baseline, None);
        }
        // Shifted: the older source's +0.3 moves a new frame's own +0.2
        // to +0.5 from that frame's own zero, not the source's.
        {
            let mut st = state.borrow_mut();
            st.sidecars[1] = Sidecar::default();
            st.sidecars[1].current.light.exposure = 0.2;
        }
        let shift = SyncShift {
            exposure: true,
            ..Default::default()
        };
        let (synced, moved) = sync_onto(
            &mut state.borrow_mut(),
            &app,
            0,
            &[1],
            &choice,
            shift,
            &[],
            none,
        );
        assert_eq!(synced.moved, [1]);
        assert_eq!(moved.exposure, Some(0.3));
        let st = state.borrow();
        assert!((st.sidecars[1].current.light.exposure - 0.5).abs() < 1e-6);
        assert_eq!(st.sidecars[1].current.light.baseline, None);
    }

    /// The demosaic goes as the source frame runs it, and only to the
    /// frames of a mosaic that takes it: an X-Trans frame's stored
    /// Bayer choice, never picked there, travels as Markesteijn's three
    /// passes and is left off a Bayer frame; a Bayer frame's RCD is left
    /// off an X-Trans one.
    #[test]
    fn a_demosaic_syncs_as_it_runs_and_only_to_its_own_mosaic() {
        use greycard_core::raw::Mosaic;
        let app = window(3);
        let (state, worker) = state_for(&app, folder(3));
        state.borrow_mut().sidecars[0].current.demosaic = greycard_edit::Demosaic::Amaze;
        open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
        state.borrow_mut().views.picked = vec![0, 1, 2];
        // What the worker has said of each file's mosaic.
        {
            use crate::panel::browser::OpenMosaic;
            let mut st = state.borrow_mut();
            for (f, m) in [(0, Mosaic::XTrans), (1, Mosaic::XTrans), (2, Mosaic::Bayer)] {
                let path = st.path_of(f).clone();
                st.develop.mosaics.insert(path, OpenMosaic::Known(m));
            }
        }
        // The bodies are not asked about: no profile goes, and a
        // frame waiting for its ISO's blend is left waiting.
        let mosaics = |_: &State, _: usize| -> Option<greycard_core::decode::Probe> { None };
        let synced = sync_selection(
            &mut state.borrow_mut(),
            &app,
            &[Section::Demosaic],
            &[],
            mosaics,
        );
        assert_eq!(synced.demosaic_left_off, [2]);
        {
            let st = state.borrow();
            assert_eq!(
                st.sidecars[1].current.demosaic,
                greycard_edit::Demosaic::Markesteijn3
            );
            assert_eq!(st.sidecars[2].current.demosaic, Edit::default().demosaic);
            assert!(
                profile_left_off_words(&st, &synced)
                    .ends_with("; the demosaic left off IMG_0002.CR3 (a method of another mosaic)")
            );
        }

        // From a Bayer frame: its RCD to the Bayer frame alone.
        state.borrow_mut().sidecars[2].current.demosaic = greycard_edit::Demosaic::Rcd;
        state.borrow_mut().sidecars[1] = Sidecar::default();
        open_row(&mut state.borrow_mut(), &app, &worker, 2, false);
        state.borrow_mut().views.picked = vec![2, 1, 0];
        let synced = sync_selection(
            &mut state.borrow_mut(),
            &app,
            &[Section::Demosaic],
            &[],
            mosaics,
        );
        assert_eq!(synced.demosaic_left_off, [0, 1]);
        let st = state.borrow();
        assert_eq!(st.sidecars[1].current.demosaic, Edit::default().demosaic);
        assert_eq!(
            st.sidecars[0].current.demosaic,
            greycard_edit::Demosaic::Amaze
        );
        // And the rule alone.
        let preset = Preset::from_edit(
            "",
            &Edit {
                demosaic: greycard_edit::Demosaic::Markesteijn1,
                ..Edit::default()
            },
            &[Section::Demosaic],
        );
        assert!(demosaic_fits(&preset, Some(Mosaic::XTrans)));
        assert!(!demosaic_fits(&preset, Some(Mosaic::Bayer)));
        assert!(demosaic_fits(&preset, None));
    }

    /// The library's index says each file's mosaic, so a sync over
    /// frames never opened reads none of them: the files are gone from
    /// the disk here, and the demosaic still goes only where it fits.
    #[test]
    fn a_sync_takes_the_mosaics_from_the_library_and_reads_no_file() {
        use greycard_core::raw::Mosaic;
        use greycard_library::fixture::write_mosaic_dng;
        const XTRANS: &str = "GGRGGBGGBGGRBRGRBGGGBGGRGGRGGBRBGBRG";
        let dir = crate::testing::scratch_dir("sync-mosaics");
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<PathBuf> = (0..3)
            .map(|i| dir.join(format!("IMG_{i:04}.dng")))
            .collect();
        write_mosaic_dng(&files[0], XTRANS);
        write_mosaic_dng(&files[1], XTRANS);
        write_mosaic_dng(&files[2], "RGGB");
        let db = dir.join("library.sqlite");
        greycard_library::Library::open(&db)
            .unwrap()
            .index_folder(&dir, &mut |_| {})
            .unwrap();
        for f in &files {
            std::fs::remove_file(f).unwrap();
        }
        let app = window(3);
        let (state, worker) = state_for(&app, files.clone());
        state.borrow_mut().views.index_reader =
            Some(greycard_library::Library::open_read_only(&db).unwrap());
        {
            let mut st = state.borrow_mut();
            assert_eq!(
                crate::panel::browser::mosaic_of(&mut st, 1),
                Some(Mosaic::XTrans)
            );
            assert_eq!(
                crate::panel::browser::mosaic_of(&mut st, 2),
                Some(Mosaic::Bayer)
            );
            // From the index, not a read: nothing was kept as read.
            assert!(st.develop.mosaics.is_empty() && st.develop.stances.is_empty());
            st.sidecars[0].current.demosaic = greycard_edit::Demosaic::Amaze;
        }
        open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
        state.borrow_mut().views.picked = vec![0, 1, 2];
        let synced = sync_selection(
            &mut state.borrow_mut(),
            &app,
            &[Section::Demosaic],
            &[],
            |_: &State, _: usize| None,
        );
        assert_eq!(synced.demosaic_left_off, [2]);
        let st = state.borrow();
        assert_eq!(
            st.sidecars[1].current.demosaic,
            greycard_edit::Demosaic::Markesteijn3
        );
        assert!(
            st.develop.mosaics.is_empty() && st.develop.stances.is_empty(),
            "no file was read"
        );
        drop(st);
        crate::testing::remove_scratch(state, &dir);
    }

    /// The sheet reads the frame behind it, so nothing under it may
    /// move that frame; and an Apply on a selection that moved anyway
    /// does nothing rather than sync what the sheet did not name.
    #[test]
    fn nothing_moves_the_selection_under_the_sheet_and_a_moved_one_is_refused() {
        let app = window(6);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, folder(6));
        let undone = Rc::new(std::cell::Cell::new(0));
        {
            let undone = undone.clone();
            app.on_undo(move || undone.set(undone.get() + 1));
        }
        app.invoke_select(1);
        with_modifier(&app, Key::Control, || {
            for i in [3, 4] {
                let (x, y) = strip_cell(&app, i);
                click(&app, x, y);
            }
        });
        app.invoke_sync_open_asked();
        assert!(app.get_sync_open());
        // The arrows, the zoom and proof keys, undo: none reach the
        // frame behind.
        press(&app, Key::RightArrow);
        with_modifier(&app, Key::Shift, || press(&app, Key::RightArrow));
        with_modifier(&app, Key::Control, || press(&app, "z"));
        press(&app, "s");
        assert_eq!(state.borrow().current, Some(1));
        assert_eq!(chosen_frames(&state.borrow()), vec![1, 3, 4]);
        assert_eq!(undone.get(), 0);
        assert!(!app.get_proofing());
        // The set moves anyway (a click that got through): refused.
        state.borrow_mut().views.picked = vec![1, 3];
        app.invoke_sync_applied();
        assert!(!app.get_sync_open());
        assert_eq!(
            app.get_status(),
            "the selection changed while the sheet was open; nothing synced"
        );
        assert!(state.borrow().sidecars[3].history.is_empty());
        // And no sheet over a tool in hand.
        app.set_level_mode(true);
        with_modifier(&app, Key::Control, || {
            with_modifier(&app, Key::Shift, || press(&app, "S"));
        });
        app.invoke_sync_open_asked();
        assert!(!app.get_sync_open());
        // Escape puts the sheet away before the tool.
        app.set_level_mode(false);
        app.invoke_sync_open_asked();
        app.set_level_mode(true);
        press(&app, Key::Escape);
        assert!(!app.get_sync_open());
        assert!(
            app.get_level_mode(),
            "the tool behind the sheet stays in hand"
        );
    }

    /// A frame the filter has just hidden is out of the set, current
    /// or not: a key does not reach it.
    #[test]
    fn a_hidden_current_frame_takes_no_key() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(1);
        state.borrow_mut().views.picked = vec![1, 2];
        {
            let mut st = state.borrow_mut();
            st.sidecars[1].meta.flag = greycard_edit::meta::Flag::Reject;
            st.views.filter = filter::Filter::from_name("No rejects").expect("it parses");
            st.views.shown = vec![0, 2, 3];
        }
        let st = state.borrow();
        assert_eq!(chosen_frames(&st), vec![2]);
        assert_eq!(st.current, Some(1));
    }

    /// A preset click with three frames selected lays it over each,
    /// through `lay_over_targets` the same way a sync does: one step
    /// per frame that did not have it, a frame that already matches
    /// left alone, and the frame outside the set untouched.
    #[test]
    fn a_preset_click_with_three_selected_lays_it_over_each_and_leaves_the_rest() {
        let dir = std::env::temp_dir().join(format!("greycard-preset-set-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..4)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let app = window(4);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, files.clone());
        let mut edit = Edit::default();
        edit.light.exposure = 0.5;
        let preset = Preset::from_edit("Portra 400", &edit, &[Section::Light]);
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.window.placement = greycard_edit::Placement::Folder;
            st.develop.presets = vec![Entry {
                path: PathBuf::new(),
                preset: preset.clone(),
            }];
            // Frame 2 already has the preset's Light on it: it is left
            // out of what moves.
            st.sidecars[2].record(preset.applied(&Edit::default()));
        }
        app.invoke_select(0);
        with_modifier(&app, Key::Control, || {
            let (x, y) = strip_cell(&app, 1);
            click(&app, x, y);
            let (x, y) = strip_cell(&app, 2);
            click(&app, x, y);
        });
        assert_eq!(chosen_frames(&state.borrow()), vec![0, 1, 2]);

        app.invoke_preset_applied(0);

        let st = state.borrow();
        // The current frame took it, through the panel, as it always
        // has.
        assert_eq!(st.sidecars[0].current.light.exposure, 0.5);
        assert_eq!(st.sidecars[0].history.len(), 1);
        // The other selected frame took it, through the sidecar, as a
        // sync's target does.
        assert_eq!(st.sidecars[1].current.light.exposure, 0.5);
        assert_eq!(st.sidecars[1].history.len(), 1);
        // Already had it: no new step.
        assert_eq!(st.sidecars[2].history.len(), 1);
        // Every frame that took it, the one on screen too, has the
        // step under the set's words; the one that had it, none.
        for f in [0, 1] {
            assert_eq!(
                st.sidecars[f].current_label.as_deref(),
                Some("Preset ×3: Portra 400"),
                "frame {f}"
            );
        }
        assert_eq!(st.sidecars[2].current_label, None);
        assert_eq!(
            app.get_history_rows()
                .row_data(0)
                .map(|r| r.name.to_string())
                .as_deref(),
            Some("Preset ×3: Portra 400")
        );
        // Outside the set: untouched.
        assert_eq!(st.sidecars[3], Sidecar::default());
        assert_eq!(
            app.get_status(),
            "Portra 400 onto 2 frames; 1 had it already"
        );

        // Written where the setting says.
        let on_disk = Sidecar::path_in(&files[1], greycard_edit::Placement::Folder);
        assert!(on_disk.exists(), "{}", on_disk.display());
        let back = Sidecar::load(&files[1]).unwrap().unwrap();
        assert_eq!(back.current, st.sidecars[1].current);
        assert_eq!(back.current_label, st.sidecars[1].current_label);
        assert!(!Sidecar::path_in(&files[3], greycard_edit::Placement::Folder).exists());
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// With one frame selected, a preset click is unchanged: the
    /// current frame alone, "applied" or "is on already", exactly as
    /// before the set existed.
    #[test]
    fn a_preset_click_with_one_frame_selected_is_unchanged() {
        let dir = std::env::temp_dir().join(format!("greycard-preset-one-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..2)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let app = window(2);
        let (state, _worker) = state_for(&app, files.clone());
        let mut edit = Edit::default();
        edit.light.exposure = 0.5;
        let preset = Preset::from_edit("Portra 400", &edit, &[Section::Light]);
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.window.placement = greycard_edit::Placement::Folder;
            st.develop.presets = vec![Entry {
                path: PathBuf::new(),
                preset,
            }];
        }
        app.invoke_select(0);
        assert_eq!(chosen_frames(&state.borrow()), vec![0]);

        app.invoke_preset_applied(0);
        assert_eq!(state.borrow().sidecars[0].current.light.exposure, 0.5);
        assert_eq!(state.borrow().sidecars[0].history.len(), 1);
        assert_eq!(app.get_status(), "Portra 400 applied");
        assert_eq!(
            state.borrow().sidecars[0].current_label.as_deref(),
            Some("Preset: Portra 400")
        );
        assert_eq!(state.borrow().sidecars[1], Sidecar::default());

        // Applied again: already on, and nothing else moves.
        app.invoke_preset_applied(0);
        assert_eq!(app.get_status(), "Portra 400 is on already");
        assert_eq!(state.borrow().sidecars[0].history.len(), 1);

        crate::testing::remove_dir_retry(&dir);
    }

    /// A store preset carrying Noise never brings the learned tier
    /// the way a sync's does: `lay_over_targets` runs with
    /// `learned_from: None` for a preset click, so a frame still
    /// waiting on its first open takes its own ISO's blend, not the
    /// preset's.
    #[test]
    fn a_preset_carrying_noise_leaves_the_learned_tier_to_each_frames_iso() {
        let app = window(3);
        let (state, _worker) = state_for(&app, folder(3));
        let mut edit = Edit::default();
        edit.noise.profiled = true;
        edit.noise.strength = 1.8;
        // Whatever tier the edit it was saved from had, a preset
        // never carries it.
        edit.noise.learned = Learned::Best;
        edit.noise.learned_strength = 0.9;
        let preset = Preset::from_edit("Clean", &edit, &[Section::Noise]);
        assert_eq!(
            preset.edit.noise.learned,
            Learned::Off,
            "a preset leaves it"
        );

        // Frame 1 at its default again waits for its ISO; frame 0,
        // edited, has a blend of its own that no seed touches.
        {
            let mut st = state.borrow_mut();
            st.sidecars[1] = Sidecar::default();
            st.sidecars[0].current.noise.learned_strength = 0.123;
            assert!(!crate::rows::wants_seed(&st, 0));
            assert!(crate::rows::wants_seed(&st, 1));
        }
        let bodies = |_: &State, f: usize| match f {
            0 => Some(body("Canon", "EOS R5", 100)),
            1 => Some(body("Canon", "EOS R5", 3200)),
            _ => None,
        };
        let synced = lay_over_targets(
            &mut state.borrow_mut(),
            &app,
            &preset,
            &[0, 1],
            None,
            Some("Preset ×2: Film"),
            &[],
            bodies,
        );
        assert_eq!(synced.moved, [0, 1]);
        let st = state.borrow();
        let blend = |iso| greycard_edit::Noise::blend_for_iso(Some(iso));
        for f in [0, 1] {
            assert!(st.sidecars[f].current.noise.profiled, "frame {f}");
            assert_eq!(st.sidecars[f].current.noise.strength, 1.8, "frame {f}");
            assert_eq!(
                st.sidecars[f].current.noise.learned,
                Learned::Off,
                "the preset's Noise never carries a tier, frame {f}"
            );
        }
        assert_eq!(
            st.sidecars[0].current.noise.learned_strength, 0.123,
            "its own blend, not the ISO's"
        );
        assert_eq!(st.sidecars[1].current.noise.learned_strength, blend(3200));
        assert!(!crate::rows::wants_seed(&st, 1), "seeded");
        assert!(crate::rows::wants_seed(&st, 2), "not a target: still waits");
    }

    /// A preset naming a camera profile checks every frame's body the
    /// same way, the open frame included: `preset_for_body` (what a
    /// preset click gives its own frame) and `lay_over_targets` (what
    /// it gives the rest of the set) agree over a mixed selection.
    #[test]
    fn a_preset_click_checks_every_bodys_fit_the_open_frame_included() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        let mut edit = Edit::default();
        edit.camera.profile = ProfileChoice::Named("r5".into());
        edit.light.exposure = 0.4;
        let preset = Preset::from_edit("Portra 400", &edit, &[Section::Camera, Section::Light]);
        let profiles = [entry("r5", Some("Canon EOS R5"))];
        let bodies = |_: &State, f: usize| match f {
            0 => Some(body("Canon", "EOS R5", 100)), // the open frame: fits
            1 => Some(body("Canon", "EOS R5", 200)), // a target: fits
            2 => Some(body("Fujifilm", "X-T5", 400)), // a target: does not
            _ => None,
        };

        // The open frame (0) fits: the fit check leaves it whole.
        let (current_preset, current_left_off) =
            preset_for_body(&state.borrow(), &preset, 0, &profiles, bodies);
        assert!(!current_left_off);
        assert_eq!(current_preset.sections, preset.sections);

        // The rest of the set, through the same loop a sync runs.
        let mut synced = lay_over_targets(
            &mut state.borrow_mut(),
            &app,
            &preset,
            &[1, 2],
            None,
            Some("Preset ×2: Film"),
            &profiles,
            bodies,
        );
        assert_eq!(synced.moved, [1, 2]);
        assert_eq!(synced.profile_left_off, [2]);
        if current_left_off {
            synced.profile_left_off.push(0);
            synced.profile_left_off.sort_unstable();
        }
        assert_eq!(
            synced.profile_left_off,
            [2],
            "the open frame fit, so it stands alone"
        );
        let st = state.borrow();
        assert_eq!(st.sidecars[1].current.camera.profile.name(), "r5");
        assert!(st.sidecars[2].current.camera.profile.is_embedded());
        assert_eq!(st.sidecars[1].current.light.exposure, 0.4);
        assert_eq!(
            st.sidecars[2].current.light.exposure, 0.4,
            "Light still lands"
        );
        drop(st);

        // Now the open frame is the one of another body: it gets the
        // same protection the mismatched target got, not the profile
        // just because it is the one on screen.
        let (current_preset, current_left_off) =
            preset_for_body(&state.borrow(), &preset, 2, &profiles, bodies);
        assert!(current_left_off, "the open frame gets the same check");
        assert!(!current_preset.sections.contains(&Section::Camera));
        assert!(
            current_preset.sections.contains(&Section::Light),
            "the rest of the preset still reaches it"
        );
    }

    /// `apply_preset` is what `on_preset_applied` calls: a left-off
    /// open frame is folded into the same report a left-off target
    /// gets, not just the two helpers it is built from
    /// (`preset_for_body` and `lay_over_targets`, tested apart above).
    #[test]
    fn apply_preset_names_the_open_frame_among_a_left_off_profile() {
        let app = window(3);
        let (state, worker) = state_for(&app, folder(3));
        let edit = Edit {
            camera: greycard_edit::Camera {
                profile: ProfileChoice::Named("r5".into()),
                ..Default::default()
            },
            light: greycard_edit::Light {
                exposure: 0.4,
                ..Default::default()
            },
            ..Edit::default()
        };
        let preset = Preset::from_edit("Portra 400", &edit, &[Section::Camera, Section::Light]);
        let profiles = [entry("r5", Some("Canon EOS R5"))];
        let bodies = |_: &State, f: usize| match f {
            0 => Some(body("Fujifilm", "X-T5", 100)), // the open frame: does not fit
            1 => Some(body("Canon", "EOS R5", 200)),  // a target: fits
            _ => None,
        };
        app.invoke_select(0);
        state.borrow_mut().views.picked = vec![0, 1];

        apply_preset(
            &mut state.borrow_mut(),
            &app,
            &worker,
            &preset,
            &profiles,
            bodies,
        );

        let status = app.get_status();
        assert!(status.contains("Portra 400 onto 2 frames"), "{status}");
        assert!(
            status.contains("the camera profile left off IMG_0000.CR3 (made for another camera)"),
            "{status}: the open frame, not just the target"
        );
        let st = state.borrow();
        assert!(
            st.sidecars[0].current.camera.profile.is_embedded(),
            "left off the open frame"
        );
        assert_eq!(
            st.sidecars[0].current.light.exposure, 0.4,
            "Light still lands"
        );
        assert_eq!(st.sidecars[1].current.camera.profile.name(), "r5");
    }

    /// A single-frame click that leaves the camera profile off says
    /// so too, the same tail a set's report gives a left-off frame,
    /// and — outside culling — those words survive a real develop
    /// landing the way a set's do.
    #[test]
    fn a_single_frame_click_names_a_left_off_profile_and_the_words_ride_the_develop() {
        let app = window(1);
        let (state, worker) = state_for(&app, folder(1));
        let edit = Edit {
            camera: greycard_edit::Camera {
                profile: ProfileChoice::Named("r5".into()),
                ..Default::default()
            },
            white_balance: WhiteBalance::Custom {
                temperature: 3200.0,
                tint: 0.0,
            },
            ..Edit::default()
        };
        let preset = Preset::from_edit(
            "Portra 400",
            &edit,
            &[Section::Camera, Section::WhiteBalance],
        );
        let profiles = [entry("r5", Some("Canon EOS R5"))];
        let bodies = |_: &State, _: usize| Some(body("Fujifilm", "X-T5", 100));

        app.invoke_select(0);
        state.borrow_mut().views.picked = vec![0];
        assert!(sync_targets(&state.borrow()).is_empty(), "one frame alone");

        apply_preset(
            &mut state.borrow_mut(),
            &app,
            &worker,
            &preset,
            &profiles,
            bodies,
        );
        assert_eq!(
            app.get_status(),
            "developing...",
            "take_current's own word, for now"
        );
        let generation = state.borrow().develop.generation;
        let said = "Portra 400 applied; the camera profile left off IMG_0000.CR3 \
                     (made for another camera)";
        assert_eq!(
            state.borrow().develop.status_after_develop,
            Some((generation, said.to_string()))
        );
        // The profile was left off; the rest of the preset still
        // landed.
        let st = state.borrow();
        assert!(st.sidecars[0].current.camera.profile.is_embedded());
        assert_eq!(
            st.sidecars[0].current.white_balance,
            WhiteBalance::Custom {
                temperature: 3200.0,
                tint: 0.0
            }
        );
        drop(st);

        crate::panel::deliver::deliver(
            &app,
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
                sharpen: None,
                sources: Vec::new(),
                learned: crate::worker::LearnedReport::Off,
                fills: crate::worker::FillReport::default(),
            },
        );
        assert_eq!(app.get_status(), said);
        assert!(
            state.borrow().develop.status_after_develop.is_none(),
            "taken"
        );
    }

    /// In culling, `leave_cull(.., None)` reads the edit to leave with
    /// off the very sidecar a preset click just recorded onto, so the
    /// two never differ and its own "write once the panel rests"
    /// never fires: the click saves the sidecar itself first, both
    /// with one frame open and with a set of two or more. Each of the
    /// two branches gets its own folder and its own app, so neither
    /// leaves anything behind in the panel for the other to read.
    #[test]
    fn a_preset_click_in_culling_saves_the_open_frames_sidecar() {
        // `also`: the rest of the set beside frame 0, empty for the
        // single-frame branch.
        for (tag, also) in [("one", vec![]), ("set", vec![1])] {
            let dir = std::env::temp_dir()
                .join(format!("greycard-preset-cull-{tag}-{}", std::process::id()));
            crate::testing::remove_dir_retry(&dir);
            std::fs::create_dir_all(&dir).expect("a temp dir");
            let files: Vec<PathBuf> = (0..3)
                .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
                .collect();
            let app = window(3);
            let (state, _worker) = state_for(&app, files.clone());
            let mut edit = Edit::default();
            edit.light.exposure = 0.5;
            let preset = Preset::from_edit("Portra 400", &edit, &[Section::Light]);
            {
                let mut st = state.borrow_mut();
                st.window.write_sidecars = true;
                st.window.placement = greycard_edit::Placement::Folder;
                st.develop.presets = vec![Entry {
                    path: PathBuf::new(),
                    preset,
                }];
            }
            let on_disk = Sidecar::path_in(&files[0], greycard_edit::Placement::Folder);

            app.invoke_select(0);
            app.invoke_cull_toggled();
            assert!(state.borrow().culling.cull.is_some(), "{tag}");
            // The set is made once the frame is open: opening it
            // makes the set that frame alone.
            let set = !also.is_empty();
            state.borrow_mut().views.picked = also;
            assert_eq!(
                chosen_frames(&state.borrow()).len(),
                if set { 2 } else { 1 },
                "{tag}"
            );
            app.invoke_preset_applied(0);
            assert!(
                state.borrow().culling.cull.is_none(),
                "{tag}: the click left culling"
            );
            assert_eq!(
                state.borrow().sidecars[0].current.light.exposure,
                0.5,
                "{tag}"
            );
            assert!(
                on_disk.exists(),
                "{tag}: the step was saved, not just recorded in memory"
            );
            assert_eq!(
                Sidecar::load(&files[0]).unwrap().unwrap().current,
                state.borrow().sidecars[0].current,
                "{tag}"
            );
            let words = if tag == "one" {
                "Preset: Portra 400"
            } else {
                "Preset ×2: Portra 400"
            };
            assert_eq!(
                Sidecar::load(&files[0])
                    .unwrap()
                    .unwrap()
                    .current_label
                    .as_deref(),
                Some(words),
                "{tag}"
            );
            if tag == "set" {
                assert_eq!(
                    state.borrow().sidecars[1].current_label.as_deref(),
                    Some(words)
                );
            }
            crate::testing::remove_dir_retry(&dir);
        }
    }

    /// A preset click over a set whose current frame needs a real
    /// develop leaves its own words for that develop to carry: the
    /// develop's "developing..." does not take them away for good.
    #[test]
    fn a_multi_frame_presets_words_ride_the_develop_that_lands() {
        let app = window(2);
        let (state, _worker) = state_for(&app, folder(2));
        // A white balance, not a light control: the history's
        // `same_develop` reads the raw develop's own inputs, and
        // exposure is the finish's, so only this asks the worker for a
        // fresh one.
        let edit = Edit {
            white_balance: WhiteBalance::Custom {
                temperature: 3200.0,
                tint: 0.0,
            },
            ..Edit::default()
        };
        let preset = Preset::from_edit("Portra 400", &edit, &[Section::WhiteBalance]);
        state.borrow_mut().develop.presets = vec![Entry {
            path: PathBuf::new(),
            preset,
        }];
        app.invoke_select(0);
        state.borrow_mut().views.picked = vec![0, 1];
        app.invoke_preset_applied(0);
        assert_eq!(
            app.get_status(),
            "developing...",
            "take_current's own word, for now"
        );
        let generation = state.borrow().develop.generation;
        assert_eq!(
            state.borrow().develop.status_after_develop,
            Some((generation, "Portra 400 onto 2 frames".to_string()))
        );

        crate::panel::deliver::deliver(
            &app,
            crate::worker::Outcome::Developed {
                generation,
                turn: 0,
                kind: crate::finish::Source::SCENE,
                image: crate::worker::Developed::Halves(std::sync::Arc::new(
                    crate::worker::Halves {
                        width: 100,
                        height: 80,
                        pixels: Vec::new(),
                    },
                )),
                guide: std::sync::Arc::new(crate::finish::Guide::NONE),
                planes: None,
                white: crate::worker::WhiteBase::IDENTITY,
                seconds: 0.42,
                sharpen: None,
                sources: Vec::new(),
                learned: crate::worker::LearnedReport::Off,
                fills: crate::worker::FillReport::default(),
            },
        );
        // The set's words alone: the develop's own are the log's.
        assert_eq!(app.get_status(), "Portra 400 onto 2 frames");
        assert!(
            state.borrow().develop.status_after_develop.is_none(),
            "taken"
        );
    }

    /// The history panel names a preset's step by the preset, a
    /// snapshot restored by the snapshot, and the rest by what moved;
    /// an undo keeps the preset's row as a redo, and a new step after
    /// it drops the row and its words.
    #[test]
    fn the_history_rows_read_a_named_step_by_its_words() {
        let app = window(1);
        let (state, _worker) = state_for(&app, folder(1));
        let mut edit = Edit::default();
        edit.light.exposure = 0.5;
        edit.light.tone.contrast = 0.25;
        let preset = Preset::from_edit("Faded film", &edit, &[Section::Light]);
        state.borrow_mut().develop.presets = vec![Entry {
            path: PathBuf::new(),
            preset,
        }];
        app.invoke_select(0);
        let rows = |app: &App| -> Vec<String> {
            let names = app.get_history_rows();
            (0..names.row_count())
                .map(|r| names.row_data(r).unwrap().name.to_string())
                .collect()
        };

        app.invoke_preset_applied(0);
        assert_eq!(rows(&app), ["Preset: Faded film", "Original"]);

        // Undone, the row is still there to redo, under its words.
        app.invoke_undo();
        assert_eq!(rows(&app), ["Preset: Faded film", "Original"]);
        assert_eq!(app.get_history_current(), 1);
        app.invoke_redo();
        assert_eq!(
            state.borrow().sidecars[0].current_label.as_deref(),
            Some("Preset: Faded film")
        );

        // A snapshot of it, a slider moved, the snapshot restored:
        // the restore is named for the snapshot even though a single
        // section moved.
        app.invoke_snapshot_taken();
        app.set_exposure(1.5);
        app.invoke_snapshot_restored(0);
        assert_eq!(
            rows(&app),
            [
                "Snapshot: Snapshot 1",
                "Exposure +1.50",
                "Preset: Faded film",
                "Original"
            ]
        );

        // Back past the preset, and a new step: the preset's state
        // and its words are gone with the redo stack.
        app.invoke_undo();
        app.invoke_undo();
        app.invoke_undo();
        assert_eq!(app.get_history_current(), 3);
        app.set_exposure(-1.0);
        app.invoke_snapshot_taken();
        assert_eq!(rows(&app), ["Exposure -1.00", "Original"]);
        let st = state.borrow();
        assert!(st.sidecars[0].redo.is_empty());
        assert!((0..st.sidecars[0].states()).all(|i| st.sidecars[0].label(i).is_none()));
    }

    /// Ctrl (a Mac's Command, to Slint) held over `f`.
    fn with_control(app: &App, f: impl FnOnce()) {
        with_modifier(app, Key::Control, f);
    }

    fn history_rows(app: &App) -> Vec<String> {
        let names = app.get_history_rows();
        (0..names.row_count())
            .map(|r| names.row_data(r).unwrap().name.to_string())
            .collect()
    }

    #[test]
    fn copy_and_paste_lay_the_copy_over_the_set_and_leave_its_source() {
        let dir = std::env::temp_dir().join(format!("greycard-paste-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..4)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let app = window(4);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.window.placement = greycard_edit::Placement::Folder;
            // A frame the paste goes onto, with a crop of its own.
            let mut own = Edit::default();
            own.geometry.crop = Some(greycard_edit::geometry::Crop {
                x: 0.1,
                y: 0.1,
                w: 0.5,
                h: 0.5,
            });
            st.sidecars[3].record(own);
        }
        // Nothing copied: Paste is grayed and the key says so.
        assert_eq!(app.get_clip_from(), "");
        app.invoke_select(1);
        with_control(&app, || press(&app, "v"));
        assert!(!app.get_sync_open());
        assert!(app.get_status().starts_with("nothing copied yet"));

        // Frame 1's panel, copied with Ctrl+C, unrecorded as it is.
        app.set_exposure(0.8);
        with_control(&app, || press(&app, "c"));
        assert_eq!(app.get_clip_from(), "IMG_0001.CR3");
        {
            let st = state.borrow();
            let clip = st.sheets.clipboard.as_ref().expect("a copy");
            assert_eq!(clip.from.path, files[1]);
            assert_eq!(clip.edit.light.exposure, 0.8);
        }
        // Onto the frame it came from alone: nothing to do.
        with_control(&app, || press(&app, "v"));
        assert!(!app.get_sync_open());
        assert_eq!(
            app.get_status(),
            "IMG_0001.CR3 is the frame these came from; choose the frames to paste onto"
        );

        // Two more chosen: the sheet, over the two, the source said.
        with_control(&app, || {
            let (x, y) = strip_cell(&app, 0);
            click(&app, x, y);
            let (x, y) = strip_cell(&app, 3);
            click(&app, x, y);
        });
        assert_eq!(chosen_frames(&state.borrow()), vec![0, 1, 3]);
        with_control(&app, || press(&app, "v"));
        assert!(app.get_sync_open());
        assert!(app.get_sync_paste());
        assert_eq!(
            app.get_sync_text(),
            "Onto 2 frames; IMG_0001.CR3, which they came from, is left as it is."
        );
        // Nobody synced yet this session: the sync's defaults.
        assert_eq!(headings(&app), defaults());
        // Light alone, then Paste.
        choose(&app, |s| s == Section::Light);
        app.invoke_sync_applied();
        assert!(!app.get_sync_open());
        assert_eq!(
            app.get_status(),
            "pasted 1 section from IMG_0001.CR3 onto 2 frames"
        );
        {
            let st = state.borrow();
            for f in [0, 3] {
                let s = &st.sidecars[f];
                assert_eq!(s.current.light.exposure, 0.8, "{f}");
                assert_eq!(s.current_label.as_deref(), Some("Paste from IMG_0001.CR3"));
                let back = Sidecar::load(&files[f]).unwrap().unwrap();
                assert_eq!(back.current, s.current);
                assert_eq!(back.current_label, s.current_label);
            }
            // The crop stayed the frame's own; one step more.
            assert!(st.sidecars[3].current.geometry.crop.is_some());
            assert_eq!(st.sidecars[3].history.len(), 2);
            // The source is untouched by the paste; the frame outside
            // the set is untouched altogether.
            assert_eq!(st.sidecars[1].current_label, None);
            assert_eq!(st.sidecars[2], Sidecar::default());
            assert_eq!(
                st.sheets.sync_last.as_ref().map(|c| c.choice()),
                Some(Choice::whole(&[Section::Light]))
            );
        }
        // A second paste opens on the choice the first made, and with
        // nothing new to lay over, records nothing.
        with_control(&app, || press(&app, "v"));
        for ((name, on), s) in headings(&app).iter().zip(Section::ALL) {
            assert_eq!(name, s.title());
            assert_eq!(*on, if *s == Section::Light { 2 } else { 0 }, "{s:?}");
        }
        app.invoke_sync_applied();
        assert_eq!(app.get_status(), "the 2 frames had these already");
        assert_eq!(state.borrow().sidecars[3].history.len(), 2);
        // A paste's sheet left by Escape leaves no paste behind it.
        with_control(&app, || press(&app, "v"));
        assert!(app.get_sync_paste());
        press(&app, Key::Escape);
        slint::platform::update_timers_and_animations();
        assert!(!app.get_sync_open());
        assert!(!app.get_sync_paste());
        // The sync's own sheet still opens on its defaults, as a sync.
        app.invoke_sync_open_asked();
        assert!(!app.get_sync_paste());
        assert_eq!(headings(&app), defaults());
        press(&app, Key::Escape);
        crate::testing::remove_dir_retry(&dir);
    }

    /// Four frames, the third a JPEG: the display curve's switch shows
    /// only on a raw, so a paste never lays a curve on a picture nor
    /// takes one from it.
    fn frames_with_a_jpeg() -> Vec<PathBuf> {
        let mut files = folder(4);
        files[2] = PathBuf::from("/nowhere/IMG_0002.JPG");
        files
    }

    /// The display curve is a row of the paste sheet, on by default:
    /// left on, the source's curve lands on each raw, one step each
    /// under the paste's label, and a JPEG keeps its default; turned
    /// off, each frame keeps its own.
    #[test]
    fn a_paste_carries_the_display_curve_when_its_row_is_on() {
        use greycard_edit::DisplayCurve;
        let app = window(4);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, frames_with_a_jpeg());
        // Frame 3 per channel, frame 0 AgX: each its own.
        state.borrow_mut().sidecars[0].record(Edit {
            display_curve: DisplayCurve::Agx,
            ..Edit::default()
        });
        // Frame 1 under AgX, a touch brighter, copied.
        app.invoke_select(1);
        app.set_display_curve(DisplayCurve::Agx.name().into());
        app.set_exposure(0.4);
        with_control(&app, || press(&app, "c"));
        with_control(&app, || {
            let (x, y) = strip_cell(&app, 2);
            click(&app, x, y);
            let (x, y) = strip_cell(&app, 3);
            click(&app, x, y);
        });
        assert_eq!(chosen_frames(&state.borrow()), vec![1, 2, 3]);

        // The row is there, under its panel name, beside Light, and on.
        with_control(&app, || press(&app, "v"));
        assert!(app.get_sync_paste());
        let heads = headings(&app);
        let at = heads
            .iter()
            .position(|(name, _)| name == "Display curve")
            .expect("a Display curve row");
        assert_eq!(heads[at - 1].0, "Light");
        assert_eq!(heads[at].1, 2);
        // Light alone: frame 3 keeps per channel.
        choose(&app, |s| s == Section::Light);
        app.invoke_sync_applied();
        {
            let st = state.borrow();
            let s = &st.sidecars[3];
            assert_eq!(s.current.light.exposure, 0.4);
            assert_eq!(s.current.display_curve, DisplayCurve::Channels);
        }

        // Light and the curve: frame 3 takes AgX, in one more step
        // under the paste's label; the JPEG takes the light alone.
        with_control(&app, || press(&app, "v"));
        choose(&app, |s| s == Section::Light || s == Section::DisplayCurve);
        app.invoke_sync_applied();
        let st = state.borrow();
        let s = &st.sidecars[3];
        assert_eq!(s.current.display_curve, DisplayCurve::Agx);
        assert_eq!(s.current.light.exposure, 0.4);
        assert_eq!(s.current_label.as_deref(), Some("Paste from IMG_0001.CR3"));
        assert_eq!(s.history.len(), 2);
        let jpeg = &st.sidecars[2];
        assert_eq!(jpeg.current.light.exposure, 0.4);
        assert_eq!(jpeg.current.display_curve, DisplayCurve::Channels);
        // Frame 0, outside the set, is as it was.
        assert_eq!(st.sidecars[0].current.display_curve, DisplayCurve::Agx);
        assert_eq!(st.sidecars[0].current.light.exposure, 0.0);
    }

    /// Pasted onto the frame on screen, the curve reaches the panel's
    /// switch, and is undone as any step is; copied from a JPEG, there
    /// is no curve to carry, and an AgX raw keeps AgX.
    #[test]
    fn a_pasted_display_curve_shows_on_the_frame_on_screen() {
        use greycard_edit::DisplayCurve;
        let app = window(4);
        let (_state, _worker) = state_for(&app, frames_with_a_jpeg());
        let agx = DisplayCurve::Agx.name();
        let channels = DisplayCurve::Channels.name();
        app.invoke_select(1);
        app.set_display_curve(agx.into());
        app.invoke_copy_asked();
        app.invoke_select(3);
        assert_eq!(app.get_display_curve(), channels);
        app.invoke_paste_asked();
        assert_eq!(app.get_sync_text(), "Onto IMG_0003.CR3.");
        app.invoke_sync_applied();
        assert_eq!(app.get_display_curve(), agx);
        assert_eq!(history_rows(&app), ["Paste from IMG_0001.CR3", "Original"]);
        app.invoke_undo();
        assert_eq!(app.get_display_curve(), channels);
        app.invoke_redo();
        assert_eq!(app.get_display_curve(), agx);

        // From the JPEG, the last paste's checks, all on: the curve's
        // row is off, the status counts what went, and frame 3 stays
        // AgX.
        app.invoke_select(2);
        app.set_exposure(0.3);
        app.invoke_copy_asked();
        app.invoke_select(3);
        app.invoke_paste_asked();
        let heads = headings(&app);
        assert!(
            heads.contains(&("Display curve".to_string(), 0)),
            "{heads:?}"
        );
        assert!(heads.contains(&("Light".to_string(), 2)), "{heads:?}");
        app.invoke_sync_applied();
        assert_eq!(app.get_exposure(), 0.3);
        assert_eq!(app.get_display_curve(), agx);
        assert!(
            app.get_status()
                .starts_with("pasted 18 sections from IMG_0002.JPG"),
            "{}",
            app.get_status()
        );
        // The row checked by hand: still nothing goes.
        app.invoke_paste_asked();
        app.invoke_sync_all(true);
        app.invoke_sync_applied();
        assert_eq!(app.get_display_curve(), agx);
    }

    /// A preset saved from a JPEG, every row checked, does not name
    /// the display curve, which would set an AgX raw back to per
    /// channel.
    #[test]
    fn a_preset_saved_from_a_picture_carries_no_display_curve() {
        let app = window(4);
        let (state, _worker) = state_for(&app, frames_with_a_jpeg());
        let dir = crate::testing::scratch_dir("preset-curve");
        state.borrow_mut().develop.preset_store = Some(preset::Store::at(dir.clone()));
        app.invoke_select(2);
        app.set_exposure(0.3);
        app.invoke_preset_save_open();
        app.invoke_preset_all(true);
        app.set_preset_name("Bright".into());
        app.invoke_preset_saved();
        assert!(
            app.get_status().starts_with("saved Bright (19 sections)"),
            "{}",
            app.get_status()
        );
        let saved = preset::Store::at(dir.clone())
            .find("Bright")
            .expect("saved")
            .preset;
        assert!(!saved.sections.contains(&Section::DisplayCurve));
        assert!(saved.sections.contains(&Section::Light));
        let text = std::fs::read_to_string(dir.join("bright.gcp")).expect("the file");
        assert!(!text.contains("display-curve"), "{text}");
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_paste_onto_the_frame_on_screen_goes_through_the_panel_and_undoes() {
        let app = window(3);
        let (state, _worker) = state_for(&app, folder(3));
        app.invoke_select(0);
        app.set_exposure(-0.6);
        app.invoke_copy_asked();
        // Another frame opened: the paste is onto it, through the
        // panel, as one step with the paste's words.
        app.invoke_select(2);
        assert_eq!(app.get_exposure(), 0.0);
        app.invoke_paste_asked();
        assert_eq!(app.get_sync_text(), "Onto IMG_0002.CR3.");
        app.invoke_sync_applied();
        assert_eq!(app.get_exposure(), -0.6);
        assert_eq!(history_rows(&app), ["Paste from IMG_0000.CR3", "Original"]);
        assert_eq!(
            state.borrow().sidecars[2].current_label.as_deref(),
            Some("Paste from IMG_0000.CR3")
        );
        // Undone as any step is: the frame back as it was.
        app.invoke_undo();
        assert_eq!(app.get_exposure(), 0.0);
        assert_eq!(state.borrow().sidecars[2].current.light.exposure, 0.0);
        // The frame it was copied from never took a step of it.
        assert!(state.borrow().sidecars[0].current_label.is_none());
    }

    /// The paste sheet from the keys alone (issue 78): it opens with
    /// Paste holding the keys, so Enter takes the checks as they
    /// stand.
    #[test]
    fn the_paste_sheet_opens_on_paste_and_enter_pastes() {
        let app = window(3);
        let (_state, _worker) = state_for(&app, folder(3));
        app.invoke_select(0);
        app.set_exposure(-0.6);
        app.invoke_copy_asked();
        app.invoke_select(2);
        app.invoke_paste_asked();
        assert!(app.get_sync_open());
        press(&app, Key::Return);
        assert!(!app.get_sync_open());
        assert_eq!(app.get_exposure(), -0.6);
        assert_eq!(history_rows(&app), ["Paste from IMG_0000.CR3", "Original"]);
        // The keys are the window's again: Tab hides the panels.
        assert!(!app.get_left_hidden());
        press(&app, Key::Tab);
        assert!(app.get_left_hidden(), "Tab reached the window");
    }

    /// The states of the sheet's rows, in their order.
    fn row_states(app: &App) -> Vec<i32> {
        let rows = app.get_sync_rows();
        (0..rows.row_count())
            .map(|i| rows.row_data(i).unwrap().state)
            .collect()
    }

    /// Tab goes from Paste round to Check all, Uncheck all, and the
    /// checks, every one a stop, then Cancel and Paste; Space turns a
    /// check over, and Shift+Tab goes back the same way.
    #[test]
    fn tab_and_shift_tab_go_round_the_paste_sheet() {
        let app = window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = state_for(&app, folder(3));
        let shift_tab = |app: &App| with_modifier(app, Key::Shift, || press(app, Key::Tab));
        app.invoke_select(0);
        app.set_exposure(-0.6);
        app.invoke_copy_asked();
        app.invoke_select(2);
        app.invoke_paste_asked();
        let checks = {
            let rows = app.get_sync_rows();
            (0..rows.row_count())
                .filter(|&i| rows.row_data(i).unwrap().kind != 2)
                .count()
        };
        assert!(checks > 2, "{checks}");

        // Past Paste to Check all, then Uncheck all, then the first
        // check: Space turns it over and nothing else.
        let before = row_states(&app);
        press(&app, Key::Tab);
        press(&app, Key::Tab);
        press(&app, Key::Tab);
        press(&app, " ");
        let after = row_states(&app);
        assert_ne!(before[0], after[0], "the first check turned over");
        assert!(app.get_sync_open());
        // Back once to Uncheck all, then to Check all: each pressed.
        shift_tab(&app);
        press(&app, " ");
        assert!(
            row_states(&app).iter().all(|&s| s == 0),
            "{:?}",
            row_states(&app)
        );
        shift_tab(&app);
        press(&app, " ");
        assert!(row_states(&app).iter().any(|&s| s != 0));
        let all = row_states(&app);
        // Back from Check all round to Paste, forward round to Check
        // all again, and back: Enter pastes.
        shift_tab(&app);
        press(&app, Key::Tab);
        assert_eq!(row_states(&app), all, "Tab moved and toggled nothing");
        shift_tab(&app);
        press(&app, Key::Return);
        assert!(!app.get_sync_open());
        assert_eq!(app.get_exposure(), -0.6);

        // Again, and forward through every check: the stop after the
        // last is Cancel, which leaves the sheet and pastes nothing.
        app.invoke_undo();
        assert_eq!(app.get_exposure(), 0.0);
        app.invoke_paste_asked();
        assert!(app.get_sync_open());
        let before = row_states(&app);
        // Paste to Check all, Uncheck all, the checks, then Cancel.
        for _ in 0..2 + checks + 1 {
            press(&app, Key::Tab);
        }
        press(&app, Key::Return);
        assert!(!app.get_sync_open(), "Enter on Cancel closed the sheet");
        assert_eq!(app.get_exposure(), 0.0, "and pasted nothing");
        // Shift+Tab from Paste is Cancel too; Escape still cancels.
        app.invoke_paste_asked();
        assert_eq!(row_states(&app), before);
        shift_tab(&app);
        press(&app, " ");
        assert!(!app.get_sync_open());
        app.invoke_paste_asked();
        shift_tab(&app);
        press(&app, Key::Escape);
        assert!(!app.get_sync_open());
        assert_eq!(app.get_exposure(), 0.0);
        // Enter on a check is Paste, never the check turned over.
        app.invoke_paste_asked();
        for _ in 0..3 {
            press(&app, Key::Tab);
        }
        assert_eq!(row_states(&app), before);
        press(&app, Key::Return);
        assert!(!app.get_sync_open());
        assert_eq!(app.get_exposure(), -0.6);
    }

    /// The frame menu's Paste leaves the keys on the sheet's Paste:
    /// the menu's own hand-back, a moment later, takes nothing from it.
    #[test]
    fn the_menus_paste_opens_on_paste_too() {
        use i_slint_backend_testing::ElementQuery;
        use slint::platform::PointerEventButton;
        let app = window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = state_for(&app, folder(3));
        app.invoke_select(0);
        app.set_exposure(-0.6);
        app.invoke_copy_asked();
        app.invoke_select(2);
        let (x, y) = strip_cell(&app, 2);
        let position = slint::LogicalPosition::new(x, y);
        for event in [
            WindowEvent::PointerMoved { position },
            WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Right,
            },
            WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Right,
            },
        ] {
            app.window().dispatch_event(event);
        }
        assert!(app.get_menu_up());
        let item = ElementQuery::from_root(&app)
            .match_descendants()
            .match_predicate(|e| {
                e.accessible_label()
                    .is_some_and(|l| l.starts_with("Paste settings from IMG_0000.CR3"))
            })
            .find_first()
            .expect("the menu's Paste");
        item.mock_single_click(PointerEventButton::Left);
        assert!(
            app.get_sync_open(),
            "{:?} {:?}",
            item.accessible_label(),
            app.get_status()
        );
        // Past the menu's hand-back of the keys to the window.
        for _ in 0..5 {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(5));
            slint::platform::update_timers_and_animations();
        }
        press(&app, Key::Return);
        assert!(!app.get_sync_open());
        assert_eq!(app.get_exposure(), -0.6);
    }

    /// A check reached by Tab below the list's view is brought into
    /// it, above the sheet's buttons.
    #[test]
    fn a_check_tabbed_to_is_scrolled_into_view() {
        use i_slint_backend_testing::ElementHandle;
        let app = window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1200.0, 420.0));
        let (_state, _worker) = state_for(&app, folder(3));
        app.invoke_select(0);
        app.set_exposure(-0.6);
        app.invoke_copy_asked();
        app.invoke_select(2);
        app.invoke_paste_asked();
        let rows = app.get_sync_rows();
        let checks: Vec<_> = (0..rows.row_count())
            .map(|i| rows.row_data(i).unwrap())
            .filter(|r| r.kind != 2)
            .collect();
        // The lowest part's check, near the foot of the longer column:
        // a part's label names its section too, so no tab or heading
        // elsewhere in the window shares it.
        let (at, low) = checks
            .iter()
            .enumerate()
            .filter(|(_, r)| r.kind == 1)
            .max_by_key(|(_, r)| r.line)
            .unwrap();
        let low = low.a11y.to_string();
        // A button's words: its own label.
        let cancel = ElementHandle::find_by_accessible_label(&app, "Cancel")
            .next()
            .unwrap()
            .absolute_position()
            .y;
        // Where the check's foot is, if it is drawn at all: one
        // scrolled out of the list's view is not.
        let bottom = |label: &str| {
            app.window().dispatch_event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
            ElementHandle::find_by_accessible_label(&app, label)
                .next()
                .map(|e| e.absolute_position().y + e.size().height)
        };
        assert!(
            bottom(&low).is_none_or(|b| b > cancel),
            "the list scrolls in this window"
        );
        // Paste, round to Check all, Uncheck all, then the checks.
        for _ in 0..3 + at {
            press(&app, Key::Tab);
        }
        let b = bottom(&low).expect("the check in view");
        assert!(b <= cancel, "{b} > {cancel}");
        press(&app, Key::Escape);
    }

    /// The primary button's ring is for the keys' focus: Tab onto it
    /// rings it, a click on it does not.
    #[test]
    fn a_primary_buttons_ring_is_for_the_keys() {
        use i_slint_backend_testing::ElementHandle;
        let app = window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = state_for(&app, folder(3));
        let shift_tab = |app: &App| with_modifier(app, Key::Shift, || press(app, Key::Tab));
        let rings = |app: &App| {
            app.window().dispatch_event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
            ElementHandle::find_by_element_id(app, "PanelButton::ring").count()
        };
        app.invoke_select(0);
        app.invoke_copy_asked();
        app.invoke_select(2);
        app.invoke_paste_asked();
        // Open with the keys on Paste: a ring.
        assert_eq!(rings(&app), 1);
        // A click on Uncheck all: the focus there, no ring anywhere.
        let words = ElementHandle::find_by_accessible_label(&app, "Uncheck all")
            .next()
            .unwrap();
        let (at, size) = (words.absolute_position(), words.size());
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert!(app.get_sync_open());
        assert_eq!(rings(&app), 0);
        // Back to Paste by the keys: Shift+Tab twice from Uncheck all
        // goes round through Check all; the ring is back.
        shift_tab(&app);
        shift_tab(&app);
        assert_eq!(rings(&app), 1);
        press(&app, Key::Escape);
    }

    /// A Subject mask pasted onto another frame has the same key there,
    /// adjustment and component, and the same shape: the panel's raster
    /// of one frame is never the other's. Going back to the frame it
    /// came from asks the worker for that frame's own, and a raster the
    /// worker made for a frame no longer open is not taken for the one
    /// open now.
    #[test]
    fn a_pasted_subject_mask_is_made_for_each_frame_and_never_shared() {
        use crate::panel::mask::bake_locals;
        use greycard_edit::brush::Raster;
        let app = window(3);
        let (state, _worker) = state_for(&app, folder(3));
        let files = state.borrow().files.clone();
        let raster = |fill: u8| Arc::new(Raster::from_data(1.5, 4, vec![fill; 24]));
        let made = |file: &PathBuf, fill: u8| crate::worker::Outcome::Mask {
            key: (1, 0),
            shape: Shape::Subject {},
            file: Some(file.clone()),
            raster: raster(fill),
            provider: None,
            seconds: 0.0,
            note: None,
            part: None,
        };
        // What the viewport would draw for (1, 0) now, and what it
        // would ask the worker for.
        let baked = |state: &Rc<Shared>| {
            let st = &mut *state.borrow_mut();
            let edit = read_edit(&app, &st.develop.edit, st.develop.target);
            let (_, wants) = bake_locals(
                &edit,
                &mut st.develop.rasters,
                &mut st.develop.learned,
                1.5,
                None,
            );
            let held = st.develop.learned.get(&(1, 0)).map(|(_, r)| r.data()[0]);
            (held, wants.iter().any(|(k, _)| *k == (1, 0)))
        };
        app.invoke_select(0);
        state
            .borrow_mut()
            .develop
            .edit
            .adjustments
            .push(greycard_edit::Adjustment {
                id: 1,
                name: "Subject 1".into(),
                enabled: true,
                mask: Mask {
                    components: vec![Component {
                        shape: Shape::Subject {},
                        ..Default::default()
                    }],
                    invert: false,
                    ..Default::default()
                },
                look: Default::default(),
                opacity: 1.0,
            });
        crate::panel::deliver::deliver(&app, made(&files[0], 10));
        assert_eq!(baked(&state), (Some(10), false), "the source's own");
        app.invoke_copy_asked();
        // Pasted onto the frame opened next, before any frame is drawn.
        app.invoke_select(2);
        app.invoke_paste_asked();
        choose(&app, |s| s == Section::Adjustments);
        app.invoke_sync_applied();
        assert_eq!(
            state.borrow().sidecars[2].current.adjustments[0].id,
            1,
            "the same key"
        );
        assert_eq!(baked(&state), (None, true), "asked for, not the source's");
        crate::panel::deliver::deliver(&app, made(&files[2], 20));
        assert_eq!(baked(&state), (Some(20), false), "the target's own");
        // Back to the source: its own raster is asked for, and the
        // target's, arriving late, is not taken for it.
        app.invoke_select(0);
        assert_eq!(baked(&state), (None, true), "asked for, not the target's");
        crate::panel::deliver::deliver(&app, made(&files[2], 20));
        assert_eq!(baked(&state), (None, true), "a late one of the target's");
        crate::panel::deliver::deliver(&app, made(&files[0], 10));
        assert_eq!(baked(&state), (Some(10), false), "the source's own again");
    }

    /// A Part carries its person: pasted onto another frame it is the
    /// same person, signature and place, for that frame's raster to
    /// look for, and nothing here resolves it.
    #[test]
    fn a_pasted_part_keeps_its_person() {
        use greycard_edit::mask::{Person, Route, Signature};
        let app = window(3);
        let (state, _worker) = state_for(&app, folder(3));
        let part = Shape::Part {
            phrase: "lips".into(),
            route: Route::Mouth,
            person: Some(Person {
                signature: Signature {
                    kind: "colors-1".into(),
                    values: (0..22).map(|i| i as f32 * 0.5).collect(),
                },
                at: [0.42, 0.31],
                picture: Some("5e1d".into()),
            }),
        };
        app.invoke_select(0);
        {
            // On the panel's edit, as a part chosen there lands.
            let mut st = state.borrow_mut();
            st.develop.edit.adjustments.push(greycard_edit::Adjustment {
                id: 1,
                name: "Lips 1".into(),
                enabled: true,
                mask: Mask {
                    components: vec![Component {
                        shape: part.clone(),
                        ..Default::default()
                    }],
                    invert: false,
                    ..Default::default()
                },
                look: Default::default(),
                opacity: 1.0,
            });
        }
        app.invoke_copy_asked();
        app.invoke_select(2);
        app.invoke_paste_asked();
        choose(&app, |s| s == Section::Adjustments);
        app.invoke_sync_applied();
        assert_eq!(
            app.get_status(),
            "pasted 1 section from IMG_0000.CR3 onto 1 frame"
        );
        let st = state.borrow();
        let pasted = &st.sidecars[2].current.adjustments[0].mask.components[0].shape;
        assert_eq!(pasted, &part);
    }

    #[test]
    fn a_paste_in_culling_goes_onto_the_sidecars_and_stays_in_culling() {
        let dir = std::env::temp_dir().join(format!("greycard-paste-cull-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..3)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let app = window(3);
        let (state, _worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.window.placement = greycard_edit::Placement::Folder;
            let mut edit = Edit::default();
            edit.light.exposure = 1.1;
            st.sidecars[2].record(edit);
        }
        app.invoke_select(0);
        app.invoke_cull_toggled();
        assert!(state.borrow().culling.cull.is_some());
        // In culling the copy is the sidecar's, not the panel's.
        state.borrow_mut().current = Some(2);
        app.invoke_copy_asked();
        assert_eq!(
            state
                .borrow()
                .sheets
                .clipboard
                .as_ref()
                .map(|c| c.edit.light.exposure),
            Some(1.1)
        );
        // Back on frame 0, with frame 1 beside it: both take it on
        // their sidecars, the frame on screen as any other.
        state.borrow_mut().current = Some(0);
        state.borrow_mut().views.picked = vec![0, 1];
        app.invoke_paste_asked();
        app.invoke_sync_applied();
        assert!(
            state.borrow().culling.cull.is_some(),
            "the paste left culling alone"
        );
        for f in [0, 1] {
            let st = state.borrow();
            assert_eq!(st.sidecars[f].current.light.exposure, 1.1, "{f}");
            let back = Sidecar::load(&files[f]).unwrap().unwrap();
            assert_eq!(
                back.current_label.as_deref(),
                Some("Paste from IMG_0002.CR3")
            );
        }
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_selection_moved_under_the_paste_sheet_is_refused() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(0);
        app.set_exposure(0.3);
        app.invoke_copy_asked();
        app.invoke_select(1);
        state.borrow_mut().views.picked = vec![1, 2];
        app.invoke_paste_asked();
        assert!(app.get_sync_open());
        state.borrow_mut().views.picked = vec![1, 3];
        app.invoke_sync_applied();
        assert_eq!(
            app.get_status(),
            "the selection changed while the sheet was open; nothing pasted"
        );
        let st = state.borrow();
        assert!(
            st.sidecars
                .iter()
                .skip(1)
                .all(|s| s.current_label.is_none())
        );
    }

    #[test]
    fn the_filter_field_keeps_its_own_copy_and_paste() {
        let app = window(3);
        let (state, _worker) = state_for(&app, folder(3));
        app.invoke_select(0);
        // The grid's filter field, with the focus, typed into: the
        // key opens the grid, whose field takes the focus on the next
        // turn of the loop.
        press(&app, "/");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
        press(&app, "a");
        assert_eq!(app.get_filter_text(), "a");
        with_control(&app, || press(&app, "c"));
        with_control(&app, || press(&app, "v"));
        assert!(state.borrow().sheets.clipboard.is_none());
        assert!(!app.get_sync_open());
        // Given the keys back, the same Ctrl+C copies the frame.
        press(&app, Key::Escape);
        with_control(&app, || press(&app, "c"));
        assert!(state.borrow().sheets.clipboard.is_some());
    }

    /// Where to click the one element on screen a screen reader knows
    /// as `label`, of `role` when one is given: on a check, its box;
    /// otherwise its middle. It must be inside the window, so a click
    /// there lands on it and on nothing else.
    fn spot(
        app: &App,
        label: &str,
        role: Option<i_slint_backend_testing::AccessibleRole>,
    ) -> (f32, f32) {
        use i_slint_backend_testing::ElementHandle;
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
        let found: Vec<_> = ElementHandle::find_by_accessible_label(app, label)
            .filter(|e| role.is_none() || e.accessible_role() == role)
            .collect();
        assert_eq!(found.len(), 1, "one {label:?} on screen");
        let (at, size) = (found[0].absolute_position(), found[0].size());
        let window = app.window().size().to_logical(app.window().scale_factor());
        assert!(
            at.x >= 0.0
                && at.y >= 0.0
                && at.x + size.width <= window.width
                && at.y + size.height <= window.height,
            "{label:?} is off the window"
        );
        match role {
            Some(_) => (at.x + 7.0, at.y + size.height / 2.0),
            None => (at.x + size.width / 2.0, at.y + size.height / 2.0),
        }
    }

    /// A click on the sheet's check `label` ("Detail: Dehaze").
    fn tick(app: &App, label: &str) {
        let (x, y) = spot(
            app,
            label,
            Some(i_slint_backend_testing::AccessibleRole::Checkbox),
        );
        click(app, x, y);
    }

    /// A click on the words `label`: a button's, or a choice's.
    fn press_words(app: &App, label: &str) {
        let (x, y) = spot(app, label, None);
        click(app, x, y);
    }

    fn heading(app: &App, title: &str) -> i32 {
        headings(app)
            .into_iter()
            .find(|(name, _)| name == title)
            .unwrap_or_else(|| panic!("no heading {title:?}"))
            .1
    }

    fn mask(id: u64, name: &str, shape: Shape) -> greycard_edit::Adjustment {
        greycard_edit::Adjustment {
            id,
            name: name.into(),
            mask: Mask {
                components: vec![Component {
                    shape,
                    ..Default::default()
                }],
                invert: false,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn mask_names(edit: &Edit) -> Vec<String> {
        edit.adjustments.iter().map(|a| a.name.clone()).collect()
    }

    /// Frame 0 on the panel with a look and two masks, frame 2 with a
    /// look and a mask of its own, in a window tall enough for the
    /// sheet's whole list.
    fn two_frames() -> (App, Rc<Shared>, Rc<Worker>) {
        let app = window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 1200.0));
        let (state, worker) = state_for(&app, folder(3));
        {
            let mut st = state.borrow_mut();
            let mut own = Edit::default();
            own.light.tone.shadows = -0.5;
            own.detail.texture = 0.2;
            own.adjustments = vec![mask(1, "Mine 1", Shape::Subject {})];
            st.sidecars[2].record(own);
        }
        app.invoke_select(0);
        app.set_exposure(0.7);
        app.set_contrast(1.3);
        app.set_detail_texture(0.6);
        app.set_detail_dehaze(0.4);
        state.borrow_mut().develop.edit.adjustments = vec![
            mask(1, "Sky 1", Shape::Sky { picks: Vec::new() }),
            mask(2, "Face 2", Shape::Subject {}),
        ];
        (app, state, worker)
    }

    #[test]
    fn the_sheet_sends_one_part_and_one_mask_and_the_rest_stays() {
        let (app, state, _worker) = two_frames();
        state.borrow_mut().views.picked = vec![0, 2];
        app.invoke_sync_open_asked();
        assert!(app.get_sync_open());
        // The frame's masks listed by name under Masks, unchecked as
        // a sync's masks start.
        assert_eq!(heading(&app, "Masks"), 0);
        // Check all, then Uncheck all: every heading follows.
        press_words(&app, "Check all");
        assert!(headings(&app).iter().all(|(_, on)| *on == 2));
        press_words(&app, "Uncheck all");
        assert!(headings(&app).iter().all(|(_, on)| *on == 0));
        // Light's Exposure, Detail's Dehaze and the one mask, joining.
        tick(&app, "Light: Exposure");
        tick(&app, "Detail: Dehaze");
        tick(&app, "Masks: Face 2");
        press_words(&app, "Add to masks");
        assert_eq!(heading(&app, "Light"), 1);
        assert_eq!(heading(&app, "Detail"), 1);
        assert_eq!(heading(&app, "Masks"), 1);
        assert_eq!(heading(&app, "Color"), 0);
        app.invoke_sync_applied();
        assert_eq!(
            app.get_status(),
            "synced 1 section and 2 parts onto 1 frame"
        );
        let st = state.borrow();
        let got = &st.sidecars[2].current;
        assert_eq!(got.light.exposure, 0.7);
        assert_eq!(got.detail.dehaze, 0.4);
        // The rest of Light and Detail is the frame's own.
        assert_eq!(got.light.tone.contrast, 1.0);
        assert_eq!(got.light.tone.shadows, -0.5);
        assert_eq!(got.detail.texture, 0.2);
        // The mask joined the frame's own, after it.
        assert_eq!(mask_names(got), ["Mine 1", "Face 2"]);
        assert_eq!(got.adjustments[1].id, 2);
        assert_eq!(st.sidecars[1], Sidecar::default());
    }

    #[test]
    fn a_check_takes_space_and_tab_moves_on_and_return_applies() {
        let (app, state, _worker) = two_frames();
        state.borrow_mut().views.picked = vec![0, 2];
        app.invoke_sync_open_asked();
        let dehaze = |app: &App| {
            let rows = app.get_sync_rows();
            (0..rows.row_count())
                .filter_map(|i| rows.row_data(i))
                .find(|r| r.a11y == "Detail: Dehaze")
                .unwrap()
                .state
        };
        // A click takes the focus as it turns the check over; the keys
        // then turn it back and again.
        tick(&app, "Detail: Dehaze");
        assert_eq!(dehaze(&app), 0);
        assert_eq!(heading(&app, "Detail"), 1);
        press(&app, " ");
        assert_eq!(dehaze(&app), 2);
        assert_eq!(heading(&app, "Detail"), 2);
        press(&app, " ");
        assert_eq!(dehaze(&app), 0);
        // Tab leaves it: Space then turns over whatever is next, not
        // the Dehaze.
        press(&app, Key::Tab);
        press(&app, " ");
        assert_eq!(dehaze(&app), 0);
        // Escape from a check still leaves the sheet.
        tick(&app, "Detail: Texture");
        press(&app, Key::Escape);
        assert!(!app.get_sync_open());
        // Return on a check is the sheet's Apply, the check left as
        // it was.
        app.invoke_sync_open_asked();
        tick(&app, "Detail: Dehaze");
        let before = dehaze(&app);
        press(&app, Key::Return);
        assert!(!app.get_sync_open());
        assert_eq!(dehaze(&app), before);
    }

    #[test]
    fn a_mask_pasted_alone_replaces_the_frames_and_the_sheet_remembers() {
        let (app, state, _worker) = two_frames();
        app.invoke_copy_asked();
        app.invoke_select(2);
        app.invoke_paste_asked();
        assert!(app.get_sync_paste());
        press_words(&app, "Uncheck all");
        tick(&app, "Masks: Sky 1");
        app.invoke_sync_applied();
        assert_eq!(
            app.get_status(),
            "pasted 1 section from IMG_0000.CR3 onto 1 frame"
        );
        {
            let st = state.borrow();
            let got = &st.sidecars[2].current;
            // Replacing, as masks always have: the frame's own gone,
            // the Sky carried as its ask, to be found on this frame.
            assert_eq!(mask_names(got), ["Sky 1"]);
            assert_eq!(
                got.adjustments[0].mask.components[0].shape,
                Shape::Sky { picks: Vec::new() }
            );
            assert_eq!(got.light.exposure, 0.0, "no Light was chosen");
            assert_eq!(got.light.tone.shadows, -0.5);
        }
        // The next paste opens on that choice: the masks partly on,
        // the Sky checked, Face not, the rest off.
        app.invoke_paste_asked();
        assert_eq!(heading(&app, "Masks"), 1);
        assert_eq!(heading(&app, "Light"), 0);
        let rows = app.get_sync_rows();
        let checked: Vec<String> = (0..rows.row_count())
            .filter_map(|i| rows.row_data(i))
            .filter(|r| r.kind == 1 && r.state == 2)
            .map(|r| r.a11y.to_string())
            .collect();
        assert_eq!(checked, ["Masks: Sky 1"]);
        press(&app, Key::Escape);
    }

    /// How the sheet's masks land: 0 replacing the frame's, 1 joining.
    fn joins(app: &App) -> i32 {
        let rows = app.get_sync_rows();
        (0..rows.row_count())
            .filter_map(|i| rows.row_data(i))
            .find(|r| r.kind == 2)
            .expect("the masks' line")
            .state
    }

    /// The headings with Masks alone on, as Copy masks opens a paste.
    fn masks_alone() -> Vec<(String, i32)> {
        Section::ALL
            .iter()
            .map(|s| {
                let on = *s == Section::Adjustments;
                (s.title().to_string(), if on { 2 } else { 0 })
            })
            .collect()
    }

    /// A mask row's Copy masks: a paste's sheet opens on every mask
    /// and nothing else, replacing as a first paste does, and carries
    /// all of them; a Copy settings after it opens as it always did.
    #[test]
    fn copy_masks_opens_the_paste_on_every_mask_and_nothing_else() {
        let (app, state, _worker) = two_frames();
        app.invoke_masks_copied();
        assert!(state.borrow().sheets.clipboard.is_some());
        assert!(!app.get_mask_held(), "the mask clipboard is apart");
        app.invoke_select(2);
        app.invoke_paste_asked();
        assert!(app.get_sync_paste());
        assert_eq!(headings(&app), masks_alone());
        assert_eq!(joins(&app), 0);
        app.invoke_sync_applied();
        assert_eq!(
            app.get_status(),
            "pasted 1 section from IMG_0000.CR3 onto 1 frame"
        );
        {
            let st = state.borrow();
            let got = &st.sidecars[2].current;
            assert_eq!(mask_names(got), ["Sky 1", "Face 2"]);
            assert_eq!(got.light.exposure, 0.0, "no Light was chosen");
            assert_eq!(got.light.tone.shadows, -0.5);
            assert_eq!(got.detail.texture, 0.2);
        }
        // The same copy pasted again opens on the masks again.
        app.invoke_paste_asked();
        assert_eq!(headings(&app), masks_alone());
        press(&app, Key::Escape);
        // Copy settings is its own again: the sheet as a first paste
        // opens it, the masks paste not remembered as a choice.
        app.invoke_select(0);
        app.invoke_copy_asked();
        app.invoke_select(2);
        app.invoke_paste_asked();
        assert_eq!(headings(&app), defaults());
        press(&app, Key::Escape);
    }

    /// Copy masks keeps how the last paste's masks landed, and a
    /// choice made on its own sheet is kept for the next paste while
    /// the sections stay as that paste had them.
    #[test]
    fn copy_masks_keeps_the_join_and_leaves_the_sections_chosen() {
        let (app, state, _worker) = two_frames();
        // A paste of Light alone, the masks joining.
        app.invoke_copy_asked();
        app.invoke_select(2);
        app.invoke_paste_asked();
        press_words(&app, "Uncheck all");
        tick(&app, "Light: Exposure");
        press_words(&app, "Add to masks");
        app.invoke_sync_applied();
        // Copy masks: every mask, joining as that paste had them.
        app.invoke_select(0);
        app.invoke_masks_copied();
        app.invoke_select(1);
        app.invoke_paste_asked();
        assert_eq!(headings(&app), masks_alone());
        assert_eq!(joins(&app), 1);
        // Replacing, chosen here, is what the next paste opens with.
        press_words(&app, "Replace masks");
        app.invoke_sync_applied();
        assert_eq!(
            mask_names(&state.borrow().sidecars[1].current),
            ["Sky 1", "Face 2"]
        );
        app.invoke_select(0);
        app.invoke_copy_asked();
        app.invoke_select(2);
        app.invoke_paste_asked();
        assert_eq!(joins(&app), 0);
        // The sections as the Light paste chose them, not the masks.
        assert_eq!(heading(&app, "Light"), 1);
        assert_eq!(heading(&app, "Masks"), 0);
        press(&app, Key::Escape);
    }

    /// Copy mask and Paste mask: the one mask, apart from the settings
    /// clipboard, joined to every selected frame that lacks it, one
    /// step each; onto the open frame through the panel, and undone in
    /// one.
    #[test]
    fn a_mask_copied_alone_joins_the_selection_and_the_open_frame() {
        let (app, state, _worker) = two_frames();
        assert!(!app.get_mask_held());
        app.invoke_mask_copied(1);
        assert!(app.get_mask_held());
        assert!(state.borrow().sheets.clipboard.is_none());
        // Onto the selection, the frame it came from among them,
        // which has it already and is left as it is.
        state.borrow_mut().views.picked = vec![0, 1, 2];
        app.invoke_mask_pasted();
        assert_eq!(
            app.get_status(),
            "pasted 1 section from IMG_0000.CR3 onto 2 frames"
        );
        {
            let st = state.borrow();
            assert_eq!(mask_names(&st.sidecars[1].current), ["Face 2"]);
            let got = &st.sidecars[2].current;
            assert_eq!(mask_names(got), ["Mine 1", "Face 2"]);
            assert_eq!(got.adjustments[1].id, 2);
            assert_eq!(
                got.adjustments[1].mask.components[0].shape,
                Shape::Subject {},
                "the subject to be found on this frame"
            );
            assert_eq!(got.light.tone.shadows, -0.5, "its own look kept");
            assert_eq!(got.light.exposure, 0.0, "nothing but the mask");
            assert_eq!(
                st.sidecars[2].current_label.as_deref(),
                Some("Paste from IMG_0000.CR3")
            );
            assert_eq!(mask_names(&st.develop.edit), ["Sky 1", "Face 2"]);
        }
        // Frame 2's own mask copied, and pasted onto frame 0 alone,
        // open on the panel.
        app.invoke_select(2);
        app.invoke_mask_copied(0);
        app.invoke_select(0);
        state.borrow_mut().views.picked = vec![0];
        app.invoke_mask_pasted();
        let names = |state: &Rc<Shared>| mask_names(&state.borrow().develop.edit);
        assert_eq!(names(&state), ["Sky 1", "Face 2", "Mine 1"]);
        assert_eq!(app.get_adjustment_names().row_count(), 4);
        assert_eq!(
            state.borrow().sidecars[0].current_label.as_deref(),
            Some("Paste from IMG_0002.CR3")
        );
        app.invoke_undo();
        assert_eq!(names(&state), ["Sky 1", "Face 2"]);
        assert_eq!(app.get_exposure(), 0.7, "the look before it kept");
    }

    /// A frame holding as many masks as a picture may takes no more
    /// from Paste mask: the 17th would be listed and never drawn.
    #[test]
    fn paste_mask_leaves_a_frame_at_the_most_masks_as_it_is() {
        let (app, state, _worker) = two_frames();
        let full: Vec<_> = (1..=MAX_LOCALS as u64)
            .map(|id| mask(id, &format!("Mine {id}"), Shape::Subject {}))
            .collect();
        {
            let mut st = state.borrow_mut();
            let mut own = st.sidecars[2].current.clone();
            own.adjustments = full.clone();
            st.sidecars[2].record(own);
        }
        app.invoke_mask_copied(0);
        state.borrow_mut().views.picked = vec![0, 2];
        app.invoke_mask_pasted();
        assert_eq!(app.get_status(), "the 2 frames had these already");
        assert_eq!(state.borrow().sidecars[2].current.adjustments, full);
    }

    /// Enter on a check of the preset sheet is its Save, once there
    /// is a name; with none, nothing.
    #[test]
    fn enter_on_a_preset_check_saves_once_named() {
        let (app, state, _worker) = two_frames();
        let dir = crate::testing::scratch_dir("preset-enter");
        state.borrow_mut().develop.preset_store = Some(preset::Store::at(dir.clone()));
        app.invoke_preset_save_open();
        assert!(app.get_preset_open());
        let saves = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = saves.clone();
        app.on_preset_saved(move || seen.set(seen.get() + 1));
        let rows = app.get_preset_rows();
        let check = (0..rows.row_count())
            .filter_map(|i| rows.row_data(i))
            .find(|r| r.kind != 2)
            .unwrap()
            .a11y
            .to_string();
        app.set_preset_name("".into());
        tick(&app, &check);
        press(&app, Key::Return);
        assert_eq!(saves.get(), 0, "no name, no save");
        app.set_preset_name("Warm".into());
        tick(&app, &check);
        press(&app, Key::Return);
        assert_eq!(saves.get(), 1);
        drop(app);
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn the_preset_sheet_saves_a_single_part_and_a_single_mask() {
        let (app, state, _worker) = two_frames();
        let dir = crate::testing::scratch_dir("preset-parts");
        state.borrow_mut().develop.preset_store = Some(preset::Store::at(dir.clone()));
        app.invoke_preset_save_open();
        assert!(app.get_preset_open());
        // On to start: only the parts the edit changed.
        let rows = app.get_preset_rows();
        let on: Vec<String> = (0..rows.row_count())
            .filter_map(|i| rows.row_data(i))
            .filter(|r| r.kind == 1 && r.state == 2)
            .map(|r| r.a11y.to_string())
            .collect();
        assert_eq!(
            on,
            [
                "Light: Exposure",
                "Light: Contrast",
                "Detail: Texture",
                "Detail: Dehaze"
            ]
        );
        press_words(&app, "Uncheck all");
        tick(&app, "Detail: Dehaze");
        tick(&app, "Masks: Face 2");
        app.set_preset_name("Haze".into());
        app.invoke_preset_saved();
        assert!(
            app.get_status()
                .starts_with("saved Haze (1 section and 1 part)")
        );
        let saved = preset::Store::at(dir.clone())
            .find("Haze")
            .expect("saved")
            .preset;
        assert!(saved.sections == [Section::Adjustments]);
        assert_eq!(saved.parts, [preset::Part::DetailDehaze]);
        assert_eq!(mask_names(&saved.edit), ["Face 2"]);
        // Laid over frame 2's edit: the dehaze, the mask in place of
        // its own, and nothing else.
        let out = saved.applied(&state.borrow().sidecars[2].current);
        assert_eq!(out.detail.dehaze, 0.4);
        assert_eq!(out.detail.texture, 0.2);
        assert_eq!(mask_names(&out), ["Face 2"]);
        crate::testing::remove_dir_retry(&dir);
    }

    // The relative sync: shifts, set against shift, and what the
    // frames look like after.

    /// A linear DNG of our own writing, a flat grey card under a light
    /// the camera's `gains` neutralize: a raw the engine opens, whose
    /// as-shot white is the one asked for.
    fn grey_card(path: &Path, gains: [f32; 3]) {
        use greycard_core::raw::{
            Calibration, LevelPattern, Levels, Orientation, Samples, SensorLayout,
        };
        let m = greycard_core::color::WORKING_SPACE
            .from_xyz_matrix()
            .expect("the working space has a matrix");
        let (width, height) = (24usize, 16usize);
        let frame = greycard_core::raw::RawFrame {
            make: "Test".into(),
            model: "Cam".into(),
            other_names: Vec::new(),
            width,
            height,
            channels: 3,
            layout: SensorLayout::Linear,
            samples: Samples::U16(Vec::new()),
            levels: Levels {
                black: LevelPattern::uniform(0.0, 3),
                white: LevelPattern::uniform(65535.0, 3),
                black_deltas: None,
            },
            as_shot_coefficients: Some(gains.map(f64::from)),
            calibrations: vec![Calibration {
                illuminant: 21,
                color_matrix: m.rows.iter().flatten().map(|v| *v as f32).collect(),
                forward_matrix: None,
            }],
            crop: None,
            orientation: Orientation::Normal,
            shot: Default::default(),
            dng: Default::default(),
        };
        // The card as the camera saw it: grey, under the light's color.
        let pixel = [0.18 / gains[0], 0.18 / gains[1], 0.18 / gains[2]];
        let data: Vec<f32> = (0..width * height).flat_map(|_| pixel).collect();
        let image = greycard_core::CameraImage::from_data(width, height, data).unwrap();
        let mut file = std::fs::File::create(path).unwrap();
        greycard_core::dng::write_linear_dng(&mut file, &frame, &image, gains, &Default::default())
            .unwrap();
    }

    /// The light `path`'s As shot means, read the way the develop
    /// reads it: the whole frame decoded, its own profile.
    fn decoded_shot(path: &Path) -> TempTint {
        let frame = greycard_core::decode::decode_path(path).unwrap();
        let profile = greycard_core::color::profile_from_frame(&frame).unwrap();
        greycard_core::color::as_shot_temp_tint(&frame, &profile).unwrap()
    }

    /// The card's middle as `edit` renders it from the raw at `path`:
    /// the develop and the export's finish, sixteen bits, RGB.
    fn card(path: &Path, edit: &Edit) -> [f64; 3] {
        let frame = greycard_core::decode::decode_path(path).unwrap();
        let developed = greycard_core::develop::develop(
            &frame,
            &edit.settings(&greycard_edit_io::Stores::user()),
        )
        .unwrap();
        let image = developed.image;
        let settings = crate::export::Settings {
            format: crate::export::Format::Tiff,
            sharpen: crate::export::Sharpen::Off,
            ..Default::default()
        };
        let rendered = crate::export::render(
            &image,
            edit,
            &greycard_edit_io::Stores::user(),
            (image.width as u32, image.height as u32),
            &settings,
            &Default::default(),
            developed.clip_level,
            None,
            crate::finish::Source::SCENE,
            None,
            None,
        );
        let crate::export::Pixels::Sixteen(px) = rendered.pixels else {
            panic!("a TIFF is sixteen bits");
        };
        let (w, h) = (rendered.width as usize, rendered.height as usize);
        let at = ((h / 2) * w + w / 2) * 3;
        [px[at] as f64, px[at + 1] as f64, px[at + 2] as f64]
    }

    fn custom(temperature: f64, tint: f64) -> WhiteBalance {
        WhiteBalance::Custom { temperature, tint }
    }

    /// The warmth a card renders with: red over blue.
    fn warmth(c: [f64; 3]) -> f64 {
        c[0] / c[2]
    }

    fn at(rgb: [f64; 3], other: [f64; 3]) -> f64 {
        (0..3)
            .map(|i| (rgb[i] - other[i]).abs())
            .fold(0.0, f64::max)
    }

    /// A preset carrying the white balance and the light, as a sync on
    /// the sheet's defaults does.
    fn white_and_light(edit: &Edit) -> Preset {
        Preset::from_choice(
            "",
            edit,
            &Choice::whole(&[Section::WhiteBalance, Section::Light]),
        )
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn a_shift_moves_each_frame_from_its_own_value_and_a_set_does_not() {
        let mut source = Edit::default();
        let shot = TempTint {
            cct: 4800.0,
            duv: 0.002,
        };
        source.white_balance = custom(5100.0, 0.004);
        source.light.exposure = 0.5;
        let preset = white_and_light(&source);
        let all = SyncShift {
            temperature: true,
            tint: true,
            exposure: true,
        };
        let shift = shift_of(all, &preset, &source, Some(shot));
        // The source's difference from its as-shot, in mired.
        let by = 1e6 / 4800.0 - 1e6 / 5100.0;
        assert!(close(shift.temperature.unwrap(), by));
        assert!((shift.tint.unwrap() - 0.002).abs() < 1e-12);
        assert_eq!(shift.exposure, Some(0.5 - Edit::default().light.exposure));
        assert_eq!(shift.from_kelvin, Some(4800.0));
        assert_eq!(
            shift.words(),
            "Temperature +12 mired, Tint +0.002, Exposure +0.50 EV"
        );
        let sheet = shift.sheet_words();
        assert!(sheet.contains("+300 K at this frame's 4800 K"), "{sheet}");
        // Nothing asked, nothing shifted; a section left behind is not
        // shifted either.
        assert!(shift_of(SyncShift::default(), &preset, &source, Some(shot)).is_none());
        let light_only = Preset::from_choice("", &source, &Choice::whole(&[Section::Light]));
        let s = shift_of(all, &light_only, &source, Some(shot));
        assert_eq!((s.temperature, s.tint), (None, None));
        assert!(s.exposure.is_some());
        // The source's As shot unread: its move cannot be known.
        assert_eq!(shift_of(all, &preset, &source, None).temperature, None);

        let a = Edit {
            white_balance: custom(4200.0, 0.0),
            light: greycard_edit::Light {
                exposure: -0.25,
                ..Default::default()
            },
            ..Default::default()
        };
        let out = shifted(&preset, &shift, &a, None);
        let WhiteBalance::Custom { temperature, tint } = out.edit.white_balance else {
            panic!("a shifted custom white is custom");
        };
        // The same mired move from its own light: fewer kelvin than the
        // source's 300, the frame being warmer.
        assert!(close(1e6 / 4200.0 - 1e6 / temperature, by), "{temperature}");
        assert!(temperature - 4200.0 < 300.0);
        assert!((tint - 0.002).abs() < 1e-12);
        assert_eq!(out.edit.light.exposure, -0.25 + shift.exposure.unwrap());
        // Temperature shifted, Tint set: the source's tint.
        let temp_only = shift_of(
            SyncShift {
                temperature: true,
                ..Default::default()
            },
            &preset,
            &source,
            Some(shot),
        );
        let out = shifted(&preset, &temp_only, &a, None);
        assert_eq!(
            out.edit.white_balance,
            custom(kelvin_moved(4200.0, by), 0.004)
        );
        assert_eq!(
            out.edit.light.exposure, source.light.exposure,
            "exposure set"
        );
        // An As shot frame becomes custom at its own as-shot light,
        // moved.
        let b = Edit::default();
        let own = TempTint {
            cct: 3300.0,
            duv: -0.001,
        };
        let out = shifted(&preset, &shift, &b, Some(own));
        let WhiteBalance::Custom { temperature, tint } = out.edit.white_balance else {
            panic!("As shot shifted is custom");
        };
        assert!(close(temperature, kelvin_moved(3300.0, by)));
        assert!((tint - 0.001).abs() < 1e-12);
        // One whose raw will not say keeps its As shot.
        let out = shifted(&preset, &shift, &b, None);
        assert_eq!(out.edit.white_balance, WhiteBalance::AsShot);
        // The sliders' ends hold.
        let hot = Edit {
            white_balance: custom(11900.0, 0.049),
            light: greycard_edit::Light {
                exposure: 4.8,
                ..Default::default()
            },
            ..Default::default()
        };
        let out = shifted(&preset, &shift, &hot, None);
        assert_eq!(out.edit.white_balance, custom(12000.0, 0.05));
        assert_eq!(out.edit.light.exposure, 5.0);
    }

    /// A mired is more kelvin the cooler the light: the one shift
    /// moves a warm frame fewer kelvin than a cool one.
    #[test]
    fn a_temperature_shift_is_the_same_mired_at_every_light() {
        let by = 1e6 / 4800.0 - 1e6 / 5100.0;
        let at = |k: f64| kelvin_moved(k, by) - k;
        assert!((at(4800.0) - 300.0).abs() < 1e-6);
        assert!(at(3000.0) < at(4800.0) && at(4800.0) < at(6500.0));
        assert!((at(3000.0) - 114.5).abs() < 0.5, "{}", at(3000.0));
        // Back by as much is where it was.
        assert!(close(kelvin_moved(kelvin_moved(3000.0, by), -by), 3000.0));
        assert_eq!(kelvin_moved(3000.0, 0.0), 3000.0);
    }

    /// A source at As shot moved nowhere: whatever the other field's
    /// choice, every frame's white is its own, custom or As shot, and
    /// a frame's raw is never read for it.
    #[test]
    fn a_source_at_as_shot_leaves_every_frames_white_whatever_is_set() {
        let mut source = Edit::default();
        source.light.exposure = 0.5;
        let preset = white_and_light(&source);
        let custom_frame = Edit {
            white_balance: custom(4200.0, 0.0),
            ..Default::default()
        };
        let as_shot_frame = Edit::default();
        let read = TempTint {
            cct: 3300.0,
            duv: -0.001,
        };
        for (temperature, tint) in [(true, false), (false, true), (true, true)] {
            let modes = SyncShift {
                temperature,
                tint,
                exposure: true,
            };
            let shift = shift_of(modes, &preset, &source, None);
            assert!(!shift.reads_targets(&preset), "{modes:?}");
            for frame in [&custom_frame, &as_shot_frame] {
                let out = shifted(&preset, &shift, frame, Some(read));
                assert_eq!(out.edit.white_balance, frame.white_balance, "{modes:?}");
                assert_eq!(out.edit.light.exposure, 0.5, "the exposure still shifts");
            }
        }
        // Both set: the source's As shot, as a sync always laid it.
        let out = shifted(
            &preset,
            &shift_of(SyncShift::default(), &preset, &source, None),
            &custom_frame,
            None,
        );
        assert_eq!(out.edit.white_balance, WhiteBalance::AsShot);
    }

    /// Lightroom's Quick Develop, end to end: a batch synced from a
    /// frame warmed 300 K and brightened half a stop from its as-shot
    /// goes as many mired warmer and half a stop brighter each, from
    /// where each was, and the cards they render say so; where a set
    /// would have made the two custom frames' cards one.
    #[test]
    fn a_relative_sync_moves_each_frames_rendered_card_by_the_shift() {
        let dir = crate::testing::scratch_dir("relative-sync");
        let files: Vec<PathBuf> = ["S", "A", "B", "C"]
            .iter()
            .map(|n| dir.join(format!("{n}.dng")))
            .collect();
        grey_card(&files[0], [2.0, 1.0, 1.6]);
        grey_card(&files[1], [2.0, 1.0, 1.6]);
        grey_card(&files[2], [2.0, 1.0, 1.6]);
        grey_card(&files[3], [1.6, 1.0, 2.2]);
        let source_shot = decoded_shot(&files[0]);
        let c_shot = decoded_shot(&files[3]);
        let app = window(4);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, files.clone());
        // The light read without a sample is the develop's own, on the
        // pool as on the window's thread.
        let read = frame_shot(&state.borrow(), 3, &Edit::default().camera);
        assert_eq!(read.map(|t| (t.cct, t.duv)), Some((c_shot.cct, c_shot.duv)));
        let pooled = read_whites(&[(3, files[3].clone(), Edit::default().camera)]);
        assert_eq!(pooled.get(&3).copied().flatten(), read);
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.window.placement = greycard_edit::Placement::Folder;
            st.sidecars[1].record(Edit {
                white_balance: custom(4200.0, 0.0),
                ..Default::default()
            });
            st.sidecars[2].record(Edit {
                white_balance: custom(5000.0, 0.0),
                ..Default::default()
            });
        }
        let before: Vec<Edit> = (0..4)
            .map(|c| state.borrow().sidecars[c].current.clone())
            .collect();
        app.invoke_select(0);
        // The source warmed 300 K from its as-shot and half a stop up.
        app.set_as_shot(false);
        app.set_temperature((source_shot.cct + 300.0) as f32);
        app.set_tint(source_shot.duv as f32);
        app.set_exposure(0.5);
        state.borrow_mut().views.picked = vec![0, 1, 2, 3];
        app.set_set_count(4);
        app.set_sync_shift_temperature(true);
        app.set_sync_shift_tint(true);
        app.set_sync_shift_exposure(true);
        app.invoke_sync_open_asked();
        assert!(app.get_sync_open());
        assert!(app.get_sync_shift_shown());
        let words = app.get_sync_shift_words().to_string();
        assert!(words.contains("mired (+300 K at this frame's"), "{words}");
        assert!(words.contains("Exposure +0.50 EV"), "{words}");
        app.invoke_sync_applied();
        assert!(!app.get_sync_open());
        let status = app.get_status().to_string();
        assert!(
            status.contains("each shifted Temperature +") && status.contains("mired"),
            "{status}"
        );

        let after: Vec<Edit> = (0..4)
            .map(|c| state.borrow().sidecars[c].current.clone())
            .collect();
        // Each from its own by the source's mired move (300 K at its
        // own light, the f32 of the panel's slider aside) and half a
        // stop.
        let temperature = |e: &Edit| match e.white_balance {
            WhiteBalance::Custom { temperature, .. } => temperature,
            WhiteBalance::AsShot | WhiteBalance::Offset { .. } => panic!("As shot after a shift"),
        };
        let by = 1e6 / source_shot.cct - 1e6 / (source_shot.cct + 300.0);
        for (c, own) in [(1, 4200.0), (2, 5000.0), (3, c_shot.cct)] {
            let want = kelvin_moved(own, by);
            assert!((temperature(&after[c]) - want).abs() < 0.05, "{c}");
        }
        for e in &after[1..] {
            assert_eq!(e.light.exposure, 0.5);
        }
        // Geometry and the rest are each frame's own.
        for c in 1..4 {
            assert_eq!(after[c].geometry, before[c].geometry);
            assert_eq!(after[c].adjustments, before[c].adjustments);
        }

        // What the cards render. The As shot frame first: custom at its
        // own as-shot light renders as As shot did, so the shift is the
        // only move.
        let mut c_at_shot = before[3].clone();
        c_at_shot.white_balance = custom(c_shot.cct, c_shot.duv);
        let c_before = card(&files[3], &before[3]);
        assert!(
            at(card(&files[3], &c_at_shot), c_before) <= 40.0,
            "custom at the as-shot light renders as As shot"
        );
        let c_after = card(&files[3], &after[3]);
        assert!(
            warmth(c_after) > warmth(c_before) * 1.01,
            "{c_before:?} {c_after:?}"
        );
        // Every target warmer and brighter than it was, by the shift:
        // each renders as its own light the same mired up, half a stop
        // up.
        for c in 1..4 {
            let (was, now) = (card(&files[c], &before[c]), card(&files[c], &after[c]));
            assert!(warmth(now) > warmth(was), "{c}: {was:?} {now:?}");
            let mut moved = before[c].clone();
            moved.white_balance = after[c].white_balance.clone();
            let same_light = card(&files[c], &moved);
            assert!(now[1] > same_light[1] * 1.05, "{c}: half a stop up");
        }
        // The two custom frames shot under one light stay as far apart
        // as they were; set, they would have rendered one card.
        let (a, b) = (card(&files[1], &after[1]), card(&files[2], &after[2]));
        assert!(warmth(a) < warmth(b), "{a:?} {b:?}");
        let set = white_and_light(&after[0]);
        let set_a = card(&files[1], &set.edit);
        let set_b = card(&files[2], &set.edit);
        assert_eq!(set_a, set_b);
        assert!(at(a, b) > 100.0, "{a:?} {b:?}");
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn the_shift_choices_grey_out_with_their_section_and_hide_for_a_paste() {
        let app = window(3);
        let (state, _worker) = state_for(&app, folder(3));
        app.invoke_select(0);
        state.borrow_mut().views.picked = vec![0, 1, 2];
        app.set_set_count(3);
        app.invoke_sync_open_asked();
        assert!(app.get_sync_shift_shown());
        assert!(app.get_sync_shift_white() && app.get_sync_shift_light());
        // Only the white balance unchecked: its two grey, Exposure not.
        choose(&app, |s| s.syncs_by_default() && s != Section::WhiteBalance);
        assert!(!app.get_sync_shift_white());
        assert!(app.get_sync_shift_light());
        choose(&app, |s| s == Section::WhiteBalance);
        assert!(app.get_sync_shift_white());
        assert!(!app.get_sync_shift_light());
        // A choice made says what a frame moves by.
        app.set_sync_shift_temperature(true);
        app.invoke_sync_shift_changed();
        let words = app.get_sync_shift_words().to_string();
        assert!(words.contains("Temperature +0 mired"), "{words}");
        press(&app, Key::Escape);
        assert!(!app.get_sync_open());
        // A paste's sheet has none.
        app.invoke_copy_asked();
        app.invoke_select(1);
        state.borrow_mut().views.picked = vec![1, 2];
        app.invoke_paste_asked();
        assert!(app.get_sync_open() && app.get_sync_paste());
        assert!(!app.get_sync_shift_shown());
    }

    /// The develop panel's head offers the sync sheet over the
    /// selection, with as many frames named as the sheet goes onto.
    #[test]
    fn the_panels_sync_button_opens_the_sheet_on_the_selection() {
        let app = window(6);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, folder(6));
        let (x, y) = strip_cell(&app, 1);
        click(&app, x, y);
        assert_eq!(
            crate::testing::count_labeled(&app, "Sync"),
            0,
            "one frame, no button"
        );
        with_modifier(&app, Key::Control, || {
            let (x, y) = strip_cell(&app, 3);
            click(&app, x, y);
            let (x, y) = strip_cell(&app, 4);
            click(&app, x, y);
        });
        assert_eq!(chosen_frames(&state.borrow()), vec![1, 3, 4]);
        assert_eq!(crate::testing::count_labeled(&app, "Sync"), 1);
        let (at, size) = crate::testing::labeled(&app, "Sync");
        // In the panel's head, on the right, above the tabs.
        assert!(at.x > 1500.0 - 320.0, "{at:?}");
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert!(app.get_sync_open());
        assert!(!app.get_sync_paste());
        assert_eq!(
            app.get_sync_text(),
            "From IMG_0001.CR3 onto 2 other frames."
        );
        // The frame menu names what it does to the selection.
        app.invoke_frame_menu_asked(-1);
        assert_eq!(app.get_menu_count(), 3);
        assert_eq!(app.get_menu_paste_count(), 0, "nothing copied");
    }

    /// A middle click on the panel's Sync button syncs at once, the
    /// sheet never up, on the choice the last sync took (here Light
    /// alone, so the color stays); a left click still opens the sheet.
    #[test]
    fn a_middle_click_on_the_panels_sync_syncs_without_the_sheet() {
        use slint::platform::PointerEventButton;
        let app = window(6);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, worker) = state_for(&app, folder(6));
        let (x, y) = strip_cell(&app, 1);
        click(&app, x, y);
        with_modifier(&app, Key::Control, || {
            let (x, y) = strip_cell(&app, 3);
            click(&app, x, y);
            let (x, y) = strip_cell(&app, 4);
            click(&app, x, y);
        });
        assert_eq!(chosen_frames(&state.borrow()), vec![1, 3, 4]);
        app.set_exposure(0.6);
        app.set_vibrance(0.3);
        app.invoke_view_changed();
        app.invoke_sync_open_asked();
        choose(&app, |s| s == Section::Light);
        app.invoke_sync_applied();
        crate::rows::land_pending(&state, &app, &worker);
        let edits = || {
            let st = state.borrow();
            [3, 4].map(|f| {
                let e = &st.sidecars[f].current;
                (e.light.exposure, e.color.vibrance)
            })
        };
        assert_eq!(edits(), [(0.6, 0.0); 2], "the sheet's sync, Light alone");

        app.set_exposure(-0.4);
        app.invoke_view_changed();
        let (at, size) = crate::testing::labeled(&app, "Sync");
        let (x, y) = (at.x + size.width / 2.0, at.y + size.height / 2.0);
        crate::testing::click_with(&app, PointerEventButton::Middle, x, y);
        assert!(!app.get_sync_open(), "no sheet");
        crate::rows::land_pending(&state, &app, &worker);
        assert_eq!(edits(), [(-0.4, 0.0); 2], "Light again, and only Light");
        assert_eq!(app.get_status(), "synced 1 section onto 2 frames");

        // The left click is the sheet's, as before.
        click(&app, x, y);
        assert!(app.get_sync_open());
    }

    /// Frame `c`'s line given an exposure and a vibrance of its own.
    fn own_edit(state: &Rc<Shared>, c: usize, exposure: f32, vibrance: f32) {
        let mut st = state.borrow_mut();
        let mut e = st.line(c).unwrap().current.clone();
        e.light.exposure = exposure;
        e.color.vibrance = vibrance;
        assert!(st.line_mut(c).unwrap().record(e));
    }

    /// The status line, or the words waiting on the develop the sync
    /// onto the open frame asked for.
    fn said(state: &Rc<Shared>, app: &App) -> String {
        match &state.borrow().develop.status_after_develop {
            Some((_, said)) => said.clone(),
            None => app.get_status().to_string(),
        }
    }

    /// A middle click on a frame of the set that is not open syncs it
    /// onto the rest, the open frame among them through its panel, on
    /// the last sync's choice (Light alone); the set and the open frame
    /// stay, the source takes nothing, and an undo takes the open
    /// frame's step back.
    #[test]
    fn a_middle_click_on_a_frame_syncs_it_onto_the_rest_of_the_set() {
        use slint::platform::PointerEventButton;
        let app = window(6);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, worker) = state_for(&app, folder(6));
        let (x, y) = strip_cell(&app, 1);
        click(&app, x, y);
        with_modifier(&app, Key::Control, || {
            let (x, y) = strip_cell(&app, 3);
            click(&app, x, y);
            let (x, y) = strip_cell(&app, 4);
            click(&app, x, y);
        });
        app.set_exposure(0.6);
        app.invoke_view_changed();
        app.invoke_sync_open_asked();
        choose(&app, |s| s == Section::Light);
        app.invoke_sync_applied();
        crate::rows::land_pending(&state, &app, &worker);
        own_edit(&state, 3, -1.0, 0.5);
        let source_steps = state.borrow().sidecars[3].history.len();

        let (x, y) = strip_cell(&app, 3);
        crate::testing::click_with(&app, PointerEventButton::Middle, x, y);
        assert!(!app.get_sync_open(), "no sheet");
        crate::rows::land_pending(&state, &app, &worker);
        {
            let st = state.borrow();
            assert_eq!(chosen_frames(&st), vec![1, 3, 4], "the set stays");
            assert_eq!(st.current, Some(1), "the open frame stays");
            let e = &st.sidecars[4].current;
            assert_eq!((e.light.exposure, e.color.vibrance), (-1.0, 0.0));
            assert_eq!(st.sidecars[1].current.light.exposure, -1.0);
            assert_eq!(
                st.sidecars[3].history.len(),
                source_steps,
                "no step on the source"
            );
            assert_eq!(st.sidecars[3].current.color.vibrance, 0.5);
            assert_eq!(
                st.sidecars[4].current_label.as_deref(),
                Some("Sync from IMG_0003.CR3")
            );
        }
        assert_eq!(app.get_selected(), 1);
        assert_eq!(
            (app.get_exposure(), app.get_vibrance()),
            (-1.0, 0.0),
            "the panel"
        );
        assert_eq!(history_rows(&app)[0], "Sync from IMG_0003.CR3");
        assert_eq!(said(&state, &app), "synced 1 section onto 2 frames");
        // The open frame's step is undone as any is.
        app.invoke_undo();
        assert_eq!(app.get_exposure(), 0.6);
        assert_eq!(state.borrow().sidecars[1].current.light.exposure, 0.6);
    }

    /// A middle click on a frame outside the set lands on the whole
    /// set, here the open frame alone, which keeps its place; on the
    /// one frame the set holds there is nothing to sync onto. A middle
    /// press let go on another frame, a tool in hand, and a sheet up
    /// each sync nothing.
    #[test]
    fn a_middle_click_outside_the_set_syncs_onto_the_set() {
        use slint::platform::PointerEventButton;
        let app = window(6);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, worker) = state_for(&app, folder(6));
        let (x, y) = strip_cell(&app, 1);
        click(&app, x, y);
        own_edit(&state, 2, -1.0, 0.0);
        let middle = |c: usize| {
            let (x, y) = strip_cell(&app, c);
            crate::testing::click_with(&app, PointerEventButton::Middle, x, y);
            crate::rows::land_pending(&state, &app, &worker);
        };
        let untouched = || {
            assert_eq!(app.get_exposure(), 0.0);
            assert!(state.borrow().sidecars[1].history.is_empty());
        };

        // Pressed on frame 2 and let go on frame 3: no click.
        app.set_status("".into());
        crate::testing::drag_with(
            &app,
            PointerEventButton::Middle,
            &[strip_cell(&app, 2), strip_cell(&app, 3)],
        );
        crate::rows::land_pending(&state, &app, &worker);
        untouched();
        assert_eq!(app.get_status(), "");
        // On the frame the set is: nothing to sync onto.
        middle(1);
        assert_eq!(
            app.get_status(),
            "choose the frames to sync onto: Ctrl+click or Shift+click"
        );
        untouched();
        // With a tool in hand.
        app.set_level_mode(true);
        middle(2);
        assert_eq!(app.get_status(), "put the tool down first (Esc), then sync");
        untouched();
        app.set_level_mode(false);
        // Under a sheet.
        app.set_status("".into());
        app.set_sync_open(true);
        middle(2);
        untouched();
        assert_eq!(app.get_status(), "");
        app.set_sync_open(false);

        middle(2);
        assert_eq!(app.get_exposure(), -1.0, "onto the open frame");
        let st = state.borrow();
        assert_eq!(chosen_frames(&st), vec![1]);
        assert_eq!(st.current, Some(1));
        assert_eq!(st.sidecars[1].current.light.exposure, -1.0);
        drop(st);
        let said = said(&state, &app);
        assert!(
            said.starts_with("synced ") && said.ends_with(" onto 1 frame"),
            "{said}"
        );
    }

    /// A middle release inside a left press is no middle click, and
    /// neither is one after the pointer left the window mid-press.
    #[test]
    fn a_middle_release_in_a_left_press_or_after_a_cancel_syncs_nothing() {
        use slint::platform::PointerEventButton;
        let app = window(6);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, worker) = state_for(&app, folder(6));
        let (x, y) = strip_cell(&app, 1);
        click(&app, x, y);
        own_edit(&state, 2, -1.0, 0.0);
        let position = {
            let (x, y) = strip_cell(&app, 2);
            slint::LogicalPosition::new(x, y)
        };
        let send = |events: &[WindowEvent]| {
            for e in events {
                app.window().dispatch_event(e.clone());
            }
            crate::rows::land_pending(&state, &app, &worker);
        };
        let pressed = |button| WindowEvent::PointerPressed { position, button };
        let released = |button| WindowEvent::PointerReleased { position, button };
        // Left and middle crossed: the left click opens frame 2.
        send(&[
            WindowEvent::PointerMoved { position },
            pressed(PointerEventButton::Middle),
            pressed(PointerEventButton::Left),
            released(PointerEventButton::Middle),
            released(PointerEventButton::Left),
        ]);
        assert_eq!(
            state.borrow().sidecars[1].current.light.exposure,
            0.0,
            "no sync"
        );
        let (x, y) = strip_cell(&app, 1);
        click(&app, x, y);
        assert_eq!(state.borrow().current, Some(1));
        // A middle press, the pointer out of the window, a release.
        send(&[
            WindowEvent::PointerMoved { position },
            pressed(PointerEventButton::Middle),
            WindowEvent::PointerExited,
        ]);
        send(&[
            WindowEvent::PointerMoved { position },
            released(PointerEventButton::Middle),
        ]);
        assert_eq!(app.get_exposure(), 0.0, "no sync");
        assert!(state.borrow().sidecars[1].history.is_empty());
    }

    /// The last choice a middle click takes, set as a sheet's apply
    /// leaves it.
    fn last_is(state: &Rc<Shared>, on: impl Fn(Section) -> bool) {
        state.borrow_mut().sheets.sync_last = Some(carry::Checks::new(Vec::new(), on));
    }

    /// A slider moved on the open frame and not yet saved (the save
    /// timer still out) reaches its sidecar on disk when a middle click
    /// syncs onto it, whether the sync moved it or found it had the
    /// sections already.
    #[test]
    fn a_middle_click_onto_the_open_frame_writes_what_its_panel_held() {
        let app = window(3);
        let dir = crate::testing::scratch_dir("middle-unsaved");
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<PathBuf> = (0..3)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let (state, worker) = state_for(&app, files.clone());
        state.borrow_mut().window.write_sidecars = true;
        last_is(&state, |s| s == Section::Light);
        own_edit(&state, 1, 0.5, 0.0);
        app.invoke_select(0);
        let on_disk = || {
            Sidecar::load(&files[0])
                .unwrap()
                .map(|s| (s.current.light.exposure, s.current.color.vibrance))
        };
        // The same Light as the source, unsaved: nothing moves.
        app.set_exposure(0.5);
        app.invoke_view_changed();
        assert_ne!(on_disk(), Some((0.5, 0.0)), "the timer has not saved it");
        app.invoke_frame_sync_asked(1);
        crate::rows::land_pending(&state, &app, &worker);
        assert_eq!(app.get_status(), "the other 1 frame had these already");
        assert_eq!(on_disk(), Some((0.5, 0.0)), "written all the same");
        // A Color slider unsaved, and Light from another value: the
        // frame moves, and both are on disk.
        own_edit(&state, 1, -0.5, 0.0);
        app.set_vibrance(0.3);
        app.invoke_view_changed();
        app.invoke_frame_sync_asked(1);
        crate::rows::land_pending(&state, &app, &worker);
        assert_eq!(app.get_exposure(), -0.5);
        assert_eq!(on_disk(), Some((-0.5, 0.3)));
        crate::testing::remove_scratch(state, &dir);
    }

    /// A middle click on a frame sets, whatever shift the sheet was
    /// left on: twice over, the targets are the source's value and the
    /// second click takes no step.
    #[test]
    fn a_second_middle_click_changes_nothing_whatever_the_sheets_shift() {
        let app = window(3);
        let (state, worker) = state_for(&app, folder(3));
        last_is(&state, |s| s == Section::Light);
        app.set_sync_shift_exposure(true);
        own_edit(&state, 1, 1.0, 0.0);
        app.invoke_select(0);
        state.borrow_mut().views.picked = vec![0, 2];
        for _ in 0..2 {
            app.invoke_frame_sync_asked(1);
            crate::rows::land_pending(&state, &app, &worker);
        }
        let st = state.borrow();
        assert_eq!(
            st.sidecars[2].current.light.exposure, 1.0,
            "set, not shifted"
        );
        assert_eq!(st.sidecars[0].current.light.exposure, 1.0);
        assert_eq!(st.sidecars[2].history.len(), 1, "one step");
        drop(st);
        assert_eq!(app.get_exposure(), 1.0);
        assert_eq!(app.get_status(), "the other 2 frames had these already");
    }

    /// The Sync button's middle click sets as a frame's does, whatever
    /// shift the sheet was left on: twice over, the target is the open
    /// frame's value, one step, and the remembered choice and shift
    /// are as they were. The sheet's own apply still shifts.
    #[test]
    fn the_buttons_middle_click_sets_and_the_sheet_still_shifts() {
        let app = window(3);
        let (state, worker) = state_for(&app, folder(3));
        last_is(&state, |s| s == Section::Light);
        app.set_sync_shift_exposure(true);
        own_edit(&state, 2, 0.5, 0.0);
        app.invoke_select(0);
        state.borrow_mut().views.picked = vec![0, 2];
        app.set_exposure(1.0);
        app.invoke_view_changed();
        let last = state.borrow().sheets.sync_last.clone();
        for _ in 0..2 {
            app.invoke_sync_quick_asked();
            crate::rows::land_pending(&state, &app, &worker);
        }
        {
            let st = state.borrow();
            assert_eq!(
                st.sidecars[2].current.light.exposure, 1.0,
                "set, not shifted"
            );
            assert_eq!(st.sidecars[2].history.len(), 2, "its own and one sync");
            assert_eq!(st.sheets.sync_last, last, "the last choice unchanged");
        }
        assert!(app.get_sync_shift_exposure(), "the shift unchanged");
        assert_eq!(app.get_status(), "the other 1 frame had these already");

        // The sheet, Light shifted: the target moves by the open
        // frame's +1 EV from its own value.
        app.invoke_sync_open_asked();
        choose(&app, |s| s == Section::Light);
        app.invoke_sync_applied();
        crate::rows::land_pending(&state, &app, &worker);
        assert_eq!(state.borrow().sidecars[2].current.light.exposure, 2.0);
    }

    /// A middle click onto the open frame with the crop up leaves the
    /// crop up and the frame's own crop, as a paste does; one with a
    /// mask chosen whose masks the sync replaces with none goes back to
    /// the global look, as a paste does.
    #[test]
    fn a_middle_click_onto_the_open_frame_under_the_crop_and_a_mask_does_as_a_paste() {
        let app = window(3);
        let (state, worker) = state_for(&app, folder(3));
        let crop = greycard_edit::geometry::Crop {
            x: 0.1,
            y: 0.1,
            w: 0.8,
            h: 0.8,
        };
        {
            let mut st = state.borrow_mut();
            let mut own = Edit::default();
            own.geometry.crop = Some(crop);
            own.adjustments = vec![mask(1, "Sky 1", Shape::Subject {})];
            st.sidecars[0].record(own);
        }
        own_edit(&state, 1, -1.0, 0.0);
        app.invoke_select(0);
        // The crop up: Light goes, the crop stays as it was.
        last_is(&state, |s| s == Section::Light);
        app.set_crop_mode(true);
        app.invoke_crop_toggled();
        let check_crop = |what: &str| {
            assert!(app.get_crop_mode(), "{what}: the crop still up");
            assert_eq!(app.get_exposure(), -1.0, "{what}");
            let st = state.borrow();
            assert_eq!(st.sidecars[0].current.geometry.crop, Some(crop), "{what}");
        };
        app.invoke_frame_sync_asked(1);
        crate::rows::land_pending(&state, &app, &worker);
        check_crop("the sync");
        app.invoke_undo();
        assert_eq!(app.get_exposure(), 0.0);
        copy_settings_of(&mut state.borrow_mut(), &app, 1);
        app.invoke_paste_asked();
        app.invoke_sync_applied();
        check_crop("the paste");
        app.set_crop_mode(false);
        app.invoke_crop_toggled();

        // The mask chosen, the masks replaced by the source's none.
        app.invoke_undo();
        last_is(&state, |s| s == Section::Light || s == Section::Adjustments);
        app.invoke_target_changed(1);
        assert_eq!(state.borrow().develop.target, Some(0));
        let check_mask = |what: &str| {
            let st = state.borrow();
            assert_eq!(st.develop.target, None, "{what}: back to the global look");
            assert!(st.sidecars[0].current.adjustments.is_empty(), "{what}");
            assert_eq!(st.develop.edit.light.exposure, -1.0, "{what}");
        };
        app.invoke_frame_sync_asked(1);
        crate::rows::land_pending(&state, &app, &worker);
        check_mask("the sync");
        app.invoke_undo();
        app.invoke_target_changed(1);
        assert_eq!(state.borrow().develop.target, Some(0));
        app.invoke_paste_asked();
        app.invoke_sync_applied();
        check_mask("the paste");
    }

    /// In Cull drawn developed a middle click onto the open frame
    /// leaves Cull for Develop, as a paste onto it does: the panel takes
    /// the frame's new state.
    #[test]
    fn a_middle_click_onto_the_open_frame_in_developed_cull_leaves_it_as_a_paste_does() {
        let app = window(3);
        let (state, worker) = state_for(&app, folder(3));
        last_is(&state, |s| s == Section::Light);
        own_edit(&state, 1, -1.0, 0.0);
        app.invoke_select(0);
        crate::panel::cull::enter_developed(&mut state.borrow_mut(), &app);
        assert!(state.borrow().culling.developed.is_some());
        app.invoke_frame_sync_asked(1);
        crate::rows::land_pending(&state, &app, &worker);
        assert!(
            state.borrow().culling.developed.is_none(),
            "the sync left Cull"
        );
        assert_eq!(app.get_exposure(), -1.0);

        app.invoke_undo();
        crate::panel::cull::enter_developed(&mut state.borrow_mut(), &app);
        copy_settings_of(&mut state.borrow_mut(), &app, 1);
        app.invoke_paste_asked();
        app.invoke_sync_applied();
        assert!(
            state.borrow().culling.developed.is_none(),
            "the paste left Cull"
        );
        assert_eq!(app.get_exposure(), -1.0);
    }

    /// A middle click on a cell of the grid syncs as one in the strip.
    #[test]
    fn a_middle_click_in_the_grid_syncs_as_in_the_strip() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;
        let app = window(6);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, worker) = state_for(&app, folder(6));
        let (x, y) = strip_cell(&app, 1);
        click(&app, x, y);
        with_modifier(&app, Key::Control, || {
            let (x, y) = strip_cell(&app, 3);
            click(&app, x, y);
        });
        own_edit(&state, 0, -1.0, 0.0);
        app.set_grid_open(true);
        slint::platform::update_timers_and_animations();
        let cell = ElementHandle::find_by_element_id(&app, "GridSheet::ct")
            .next()
            .expect("a cell on the sheet");
        let (at, size) = (cell.absolute_position(), cell.size());
        let (x, y) = (at.x + size.width / 2.0, at.y + size.height / 2.0);
        crate::testing::click_with(&app, PointerEventButton::Middle, x, y);
        crate::rows::land_pending(&state, &app, &worker);
        let st = state.borrow();
        assert_eq!(chosen_frames(&st), vec![1, 3]);
        assert_eq!(st.current, Some(1));
        assert_eq!(st.sidecars[3].current.light.exposure, -1.0);
        assert_eq!(st.sidecars[1].current.light.exposure, -1.0);
        assert_eq!(
            st.sidecars[3].current_label.as_deref(),
            Some("Sync from IMG_0000.CR3")
        );
    }

    /// The window's hold for a paste onto 300 frames, timed: the frames
    /// under `GREYCARD_BULK_DIR` (a folder on the disk to be measured),
    /// their sidecars beside them. Run by hand with `--ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn time_a_paste_onto_300_frames() {
        let Some(dir) = std::env::var_os("GREYCARD_BULK_DIR").map(PathBuf::from) else {
            return;
        };
        const N: usize = 300;
        for write in [false, true] {
            crate::testing::remove_dir_retry(&dir);
            std::fs::create_dir_all(&dir).expect("the folder");
            let files: Vec<PathBuf> = (0..N)
                .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
                .collect();
            let app = window(N);
            let (state, _worker) = state_for(&app, files.clone());
            state.borrow_mut().window.write_sidecars = write;
            app.invoke_select(0);
            app.set_exposure(0.8);
            with_control(&app, || press(&app, "c"));
            state.borrow_mut().views.picked = (0..N).collect();
            with_control(&app, || press(&app, "v"));
            assert!(app.get_sync_open());
            choose(&app, |s| s == Section::Light);
            crate::panel::saves::tests::HOLD.set(true);
            let t = std::time::Instant::now();
            app.invoke_sync_applied();
            let held = t.elapsed();
            crate::panel::saves::tests::HOLD.set(false);
            // The batch's thread let finish, then its landing on the
            // window's thread timed alone.
            let t = std::time::Instant::now();
            let landing = loop {
                let l = std::time::Instant::now();
                let zero = Some(std::time::Duration::ZERO);
                if crate::panel::saves::finish(&mut state.borrow_mut(), zero) {
                    break l.elapsed();
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            };
            println!(
                "paste onto {} frames, sidecars {}: window held {:.1} ms, the batch's \
                 thread {:.1} ms, its landing {:.1} ms",
                N - 1,
                if write { "written" } else { "off" },
                held.as_secs_f64() * 1e3,
                t.elapsed().as_secs_f64() * 1e3,
                landing.as_secs_f64() * 1e3
            );
            assert!(
                app.get_status().starts_with("pasted"),
                "{}",
                app.get_status()
            );
        }
        crate::testing::remove_dir_retry(&dir);
    }
}
