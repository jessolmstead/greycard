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
//! Copy and paste are a sync with the settings clipboard between
//! (`crate::clipboard`): [`copy_settings`] takes the current frame's
//! edit, and [`paste_selection`] lays the sections chosen on this
//! same sheet over the whole selection, the frame copied from left
//! out, through the same [`lay_over_targets`].

use crate::clipboard::Clipboard;
use crate::panel::browser::{
    chosen_frames, file_name, migrate_frame, show_badges, show_thumb, thumb_turns,
};
use crate::panel::cull::leave_cull;
use crate::panel::edit::{read_edit, save_edit, write_sidecar};
use crate::panel::history::{show_history, take_current};
use crate::*;
use greycard_edit::camera::{self, ProfileChoice};
use greycard_edit::history::{preset_label, preset_over_set_label, sync_label};

/// The frames a sync goes onto: the set, less the frame it comes
/// from.
pub(crate) fn sync_targets(st: &State) -> Vec<usize> {
    selection::others(&chosen_frames(st), st.current)
}

/// What the sheet says it will do.
fn sync_words(st: &State) -> String {
    let n = sync_targets(st).len();
    let from = st
        .current
        .map(|c| file_name(&st.files[c]))
        .unwrap_or_default();
    format!(
        "From {from} onto {n} other frame{}.",
        if n == 1 { "" } else { "s" }
    )
}

/// The sheet's word for a section: its panel title, and for the
/// masks what ticking them does to the frames synced onto.
fn sync_title(s: Section) -> String {
    match s {
        Section::Adjustments => "Adjustments (replaces masks)".to_string(),
        s => s.title().to_string(),
    }
}

/// The current frame's edit as a sync reads it. Outside culling the
/// panel is the frame's, and what it holds is recorded and saved
/// first, as a preset's apply does, so the frame synced from has the
/// state that went out in its own history. In culling the panel is
/// not the frame's and the sidecar is the truth.
fn sync_source(st: &mut State, app: &App) -> Option<Edit> {
    let c = st.current?;
    if st.cull.is_some() {
        return Some(st.sidecars[c].current.clone());
    }
    let edit = read_edit(app, &st.edit, st.target);
    save_edit(st, edit.clone());
    Some(edit)
}

/// A raw frame's make, model and ISO, read from its metadata without
/// a pixel decoded; `None` for a picture that is not a raw, or a file
/// that will not say.
pub(crate) fn probe(st: &State, file: usize) -> Option<greycard_core::decode::Probe> {
    let path = st.files.get(file)?;
    if greycard_core::picture::is_picture_path(path) {
        return None;
    }
    greycard_core::decode::probe_path(path)
        .inspect_err(|e| tracing::warn!("{}: metadata: {e}", file_name(path)))
        .ok()
}

/// Whether a camera profile may go to a frame of `body` (make and
/// model). A profile the directory has not got, or one that names no
/// camera, cannot be checked and goes, as it would in a preset; one
/// made for a camera goes only to that camera's frames, and a frame
/// that cannot say what took it does not get it.
pub(crate) fn profile_fits(entry: Option<&camera::Entry>, body: Option<(&str, &str)>) -> bool {
    let Some(entry) = entry else {
        return true;
    };
    if entry
        .unique_camera_model
        .as_deref()
        .is_none_or(|u| u.trim().is_empty())
    {
        return true;
    }
    body.is_some_and(|(make, model)| entry.fits(make, model))
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
    let named = match &preset.edit.camera.profile {
        ProfileChoice::Named(name) if preset.sections.contains(&Section::Camera) => {
            profiles.iter().find(|e| &e.name == name)
        }
        _ => None,
    };
    let Some(entry) = named else {
        return (preset.clone(), false);
    };
    let body = probe(st, file);
    if profile_fits(
        Some(entry),
        body.as_ref().map(|p| (p.make.as_str(), p.model.as_str())),
    ) {
        (preset.clone(), false)
    } else {
        let mut without = preset.clone();
        without.sections.retain(|s| *s != Section::Camera);
        (without, true)
    }
}

/// What a sync did: the frames it moved, and the frames the camera
/// profile was left off because it was made for another body.
#[derive(Debug, Default)]
pub(crate) struct Synced {
    pub(crate) moved: Vec<usize>,
    pub(crate) profile_left_off: Vec<usize>,
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
/// `profiles` is the profile directory as `camera::list` reads it:
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
    let mut fits = Vec::new();
    let mut left_off = Vec::new();
    let mut seeded = Vec::new();
    for &f in targets {
        let wants_seed = seeds && st.seed_blend.get(f).copied().unwrap_or(false);
        let probed = (named.is_some() || wants_seed)
            .then(|| probe(st, f))
            .flatten();
        if wants_seed && let Some(p) = &probed {
            st.sidecars[f].current.noise.learned_strength =
                greycard_edit::Noise::blend_for_iso(p.iso);
            st.seed_blend[f] = false;
            seeded.push(f);
        }
        match named {
            Some(entry)
                if !profile_fits(
                    entry,
                    probed.as_ref().map(|p| (p.make.as_str(), p.model.as_str())),
                ) =>
            {
                left_off.push(f)
            }
            _ => fits.push(f),
        }
    }
    let mut moved =
        greycard_edit::apply_preset_into(&mut st.sidecars, preset, &fits, learned_from, label);
    if !left_off.is_empty() {
        let mut without = preset.clone();
        without.sections.retain(|s| *s != Section::Camera);
        moved.extend(greycard_edit::apply_preset_into(
            &mut st.sidecars,
            &without,
            &left_off,
            learned_from,
            label,
        ));
    }
    moved.sort_unstable();
    if !seeds {
        // The sync brought the learned blend, which is a choice now
        // and not the ISO's to seed over on the frame's first open.
        for &f in &moved {
            if let Some(seed) = st.seed_blend.get_mut(f) {
                *seed = false;
            }
        }
    }
    let written = selection::union(&moved, &seeded);
    for &f in &written {
        write_sidecar(st, f);
        let (turns, flip) = thumb_turns(st, app, f);
        show_thumb(st, app, f, turns, flip);
        show_badges(st, app, f);
    }
    show_history(st, app);
    Synced {
        moved,
        profile_left_off: left_off,
    }
}

/// Lay `sections` of the current frame over the rest of the set,
/// through [`lay_over_targets`]. The current frame's edit is not
/// touched.
pub(crate) fn sync_selection(
    st: &mut State,
    app: &App,
    sections: &[Section],
    profiles: &[camera::Entry],
    probe: impl Fn(&State, usize) -> Option<greycard_core::decode::Probe>,
) -> Synced {
    let Some(from) = sync_source(st, app) else {
        return Synced::default();
    };
    let targets = sync_targets(st);
    let preset = Preset::from_edit("", &from, sections);
    let learned_from = preset.sections.contains(&Section::Noise).then_some(&from);
    let label = st.current.map(|c| sync_label(&file_name(&st.files[c])));
    lay_over_targets(
        st,
        app,
        &preset,
        &targets,
        learned_from,
        label.as_deref(),
        profiles,
        probe,
    )
}

/// What a preset click does, for the current frame alone or for a
/// whole selected set, once the preset itself and the profile
/// directory are in hand: `panel::assets::on_preset_applied`'s own
/// logic, pulled out here — as `sync_selection` already is — so a
/// test can hand it fake bodies instead of going through
/// `greycard_edit::camera::list` and a real file's metadata, which
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
pub(crate) fn apply_preset(
    st: &mut State,
    app: &App,
    worker: &Worker,
    preset: &Preset,
    profiles: &[camera::Entry],
    probe: impl Fn(&State, usize) -> Option<greycard_core::decode::Probe>,
) {
    let Some(c) = st.current else {
        return;
    };
    let targets = sync_targets(st);
    // The same camera-profile fit check either way, one frame or a
    // set. A profile left off this way is still there to choose by
    // hand from the CAMERA PROFILE section, which warns "Made for X,
    // not Y" rather than refusing it.
    let (current_preset, current_left_off) = preset_for_body(st, preset, c, profiles, &probe);
    // The same tail a set's report gives a left-off frame
    // (`preset_onto_words`, through `profile_left_off_words`), built
    // from a `Synced` of one frame so the words are exactly the same
    // whether the frame reached it alone or as part of a set.
    let left_off_tail = if current_left_off {
        profile_left_off_words(
            st,
            &Synced {
                moved: Vec::new(),
                profile_left_off: vec![c],
            },
        )
    } else {
        String::new()
    };
    // With one frame selected, this is otherwise unchanged from
    // before the set existed: the current frame alone, on the panel
    // outside culling and on the sidecar under it.
    if targets.is_empty() {
        // In culling the panel is not the frame's: the preset goes
        // over the sidecar's current state, and the leaving develops
        // it.
        if st.cull.is_some() {
            let edit = st.sidecars[c].current.clone();
            let applied = current_preset.applied(&edit);
            if applied == edit {
                app.set_status(format!("{} is on already{left_off_tail}", preset.name).into());
                return;
            }
            st.sidecars[c].record_as(applied, Some(preset_label(&preset.name)));
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
            app.set_status(format!("{} applied{left_off_tail}", preset.name).into());
            return;
        }
        // Whatever the panel holds is a state first, then the preset
        // over it.
        let edit = read_edit(app, &st.edit, st.target);
        let applied = current_preset.applied(&edit);
        if applied == edit {
            app.set_status(format!("{} is on already{left_off_tail}", preset.name).into());
            return;
        }
        st.sidecars[c].record(edit);
        st.sidecars[c].record_as(applied, Some(preset_label(&preset.name)));
        let said = format!("{} applied{left_off_tail}", preset.name);
        // As the set's own non-culling branch below: left for the
        // develop this generation is when one is actually asked for,
        // so the tail is not lost the moment it lands.
        let before = st.generation;
        take_current(st, app, worker);
        if st.generation == before {
            app.set_status(said.into());
        } else {
            st.status_after_develop = Some((st.generation, said));
        }
        return;
    }
    // Two or more selected: the current frame takes the preset the
    // way it always has, and the rest of the set the way a sync lays
    // sections over its targets, every frame's step under the same
    // words.
    let label = preset_over_set_label(targets.len() + 1, &preset.name);
    let current_changed = if st.cull.is_some() {
        let edit = st.sidecars[c].current.clone();
        let applied = current_preset.applied(&edit);
        let changed = applied != edit;
        if changed {
            st.sidecars[c].record_as(applied, Some(label.clone()));
        }
        changed
    } else {
        let edit = read_edit(app, &st.edit, st.target);
        let applied = current_preset.applied(&edit);
        let changed = applied != edit;
        if changed {
            st.sidecars[c].record(edit);
            st.sidecars[c].record_as(applied, Some(label.clone()));
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
    let moved = synced.moved.len() + usize::from(current_changed);
    let asked = targets.len() + 1;
    let said = preset_onto_words(st, &preset.name, moved, asked, &synced);
    if !current_changed {
        // Nothing async is coming for the current frame: what
        // happened across the set is the word that stands.
        app.set_status(said.into());
    } else if st.cull.is_some() {
        // Saved here, as the single-frame culling branch above does:
        // `leave_cull(.., None)` would otherwise never see its own
        // edit differ from the sidecar's and never schedule the
        // write.
        write_sidecar(st, c);
        // The leaving develops the frame and sets its own status;
        // culling's placeholder text can replace that again before
        // the develop is on screen (`cull.rs`), so this line is not
        // guaranteed to be the one left standing the way the
        // non-culling one below is.
        leave_cull(st, app, worker, None);
        app.set_status(said.into());
    } else {
        // The develop sets its own "developing..." now and, once it
        // lands, its own "WxH, developed in ...". Left here for that
        // generation, `said` is the line the develop's own words are
        // appended to (`Outcome::Developed` in `deliver.rs`), so it is
        // not lost the moment the develop finishes.
        let before = st.generation;
        take_current(st, app, worker);
        if st.generation == before {
            // No develop was asked for after all: the panel's change
            // did not reach the engine.
            app.set_status(said.into());
        } else {
            st.status_after_develop = Some((st.generation, said));
        }
    }
}

/// The tail every report of a sync or a preset onto a set shares: the
/// frames the camera profile was left off because it was made for
/// another body, named up to three, the rest counted.
fn profile_left_off_words(st: &State, synced: &Synced) -> String {
    if synced.profile_left_off.is_empty() {
        return String::new();
    }
    let names: Vec<String> = synced
        .profile_left_off
        .iter()
        .take(3)
        .map(|&f| file_name(&st.files[f]))
        .collect();
    let more = synced.profile_left_off.len().saturating_sub(3);
    format!(
        "; the camera profile left off {}{} (made for another camera)",
        names.join(", "),
        if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        }
    )
}

/// The status line after a sync: how much went where, and which
/// frames kept their own camera profile.
fn synced_words(st: &State, sections: usize, asked: usize, synced: &Synced) -> String {
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let mut said = if synced.moved.is_empty() {
        format!("the other {asked} frame{} had these already", plural(asked))
    } else {
        format!(
            "synced {sections} section{} onto {} frame{}",
            plural(sections),
            synced.moved.len(),
            plural(synced.moved.len())
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
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let mut said = if moved == 0 {
        format!("{name} is on already")
    } else {
        format!("{name} onto {moved} frame{}", plural(moved))
    };
    let already = total.saturating_sub(moved);
    if moved > 0 && already > 0 {
        said.push_str(&format!("; {already} had it already"));
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
        app.set_status("open a frame to copy its settings".into());
        return;
    };
    copy_settings_of(st, app, c);
}

/// Copy frame `file`'s edit, whole, into the clipboard: the frame
/// right-clicked, for the menu's Copy. The panel's edit when `file`
/// is the frame the panel holds; its sidecar's current state
/// otherwise, brought up to date first.
pub(crate) fn copy_settings_of(st: &mut State, app: &App, file: usize) {
    if file >= st.files.len() {
        return;
    }
    let edit = if st.cull.is_none() && st.current == Some(file) {
        read_edit(app, &st.edit, st.target)
    } else {
        migrate_frame(st, file);
        st.sidecars[file].current.clone()
    };
    let clip = Clipboard::copy(edit, &st.files[file]);
    let name = clip.name();
    st.clipboard = Some(clip);
    app.set_clip_from(name.clone().into());
    app.set_status(format!("copied the settings of {name}").into());
}

/// The sections a paste's sheet opens with: the last sync's or
/// paste's choice this session, else what a sync checks by default.
fn paste_sections(st: &State) -> Vec<bool> {
    Section::ALL
        .iter()
        .map(|s| match &st.sync_last {
            Some(last) => last.contains(s),
            None => s.syncs_by_default(),
        })
        .collect()
}

/// What the paste sheet says it will do: onto which frames, and the
/// frame copied from left out when it is among them.
fn paste_words(st: &State, clip: &Clipboard, targets: &[usize]) -> String {
    let onto = match targets {
        [one] => file_name(&st.files[*one]),
        _ => format!("{} frames", targets.len()),
    };
    let set = chosen_frames(st);
    if set.len() > targets.len() {
        format!(
            "Onto {onto}; {}, which they came from, is left as it is.",
            clip.name()
        )
    } else {
        format!("Onto {onto}.")
    }
}

/// Ctrl+V, or the menu's Paste settings: the sync sheet, over the
/// selection less the frame the clipboard came from, the sections as
/// the last sync or paste had them.
pub(crate) fn paste_open(st: &mut State, app: &App) {
    let Some(clip) = st.clipboard.clone() else {
        app.set_status(
            "nothing copied yet: right-click a frame for Copy settings, or Ctrl+C".into(),
        );
        return;
    };
    if tool_in_hand(app) {
        app.set_status("put the tool down first (Esc), then paste".into());
        return;
    }
    let Some(c) = st.current else {
        return;
    };
    let targets = clip.targets(&chosen_frames(st), &st.files);
    if targets.is_empty() {
        app.set_status(
            format!(
                "{} is the frame these came from; choose the frames to paste onto",
                clip.name()
            )
            .into(),
        );
        return;
    }
    st.sync_asked = None;
    st.paste_asked = Some((c, targets.clone()));
    for (i, on) in paste_sections(st).into_iter().enumerate() {
        st.sync_sections.set_row_data(i, on);
    }
    app.set_sync_section_names(ModelRc::new(VecModel::from(
        Section::ALL
            .iter()
            .map(|s| slint::SharedString::from(sync_title(*s)))
            .collect::<Vec<_>>(),
    )));
    app.set_sync_section_on(ModelRc::from(st.sync_sections.clone()));
    app.set_sync_text(paste_words(st, &clip, &targets).into());
    app.set_sync_paste(true);
    app.set_sync_open(true);
}

/// Lay the clipboard's `sections` over the selection, the frame it
/// came from left out: a sync with the clipboard between. The frame on
/// screen, outside culling, takes it through the panel as a preset
/// click does (the camera-profile fit checked the same way); the rest,
/// and in culling every frame, through [`lay_over_targets`], exactly
/// as a sync's targets. Each frame that moved gets one step, "Paste
/// from" the frame copied. The status line says what happened, and
/// rides the develop the frame on screen asks for.
pub(crate) fn paste_selection(
    st: &mut State,
    app: &App,
    worker: &Worker,
    sections: &[Section],
    profiles: &[camera::Entry],
    probe: impl Fn(&State, usize) -> Option<greycard_core::decode::Probe>,
) -> Synced {
    let Some(clip) = st.clipboard.clone() else {
        return Synced::default();
    };
    let targets = clip.targets(&chosen_frames(st), &st.files);
    let preset = clip.preset(sections);
    let label = clip.label();
    // Outside culling the frame on screen is the panel's, and what
    // the panel holds is a state first, then the paste over it.
    let panel_frame = st
        .current
        .filter(|c| st.cull.is_none() && targets.contains(c));
    let mut current_changed = false;
    let mut current_left_off = false;
    if let Some(c) = panel_frame {
        let (fits, left_off) = preset_for_body(st, &preset, c, profiles, &probe);
        current_left_off = left_off;
        let edit = read_edit(app, &st.edit, st.target);
        let applied = clip.applied(&fits, &edit);
        if applied != edit {
            st.sidecars[c].record(edit);
            st.sidecars[c].record_as(applied, Some(label.clone()));
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
        &preset,
        &rest,
        clip.learned_from(&preset),
        Some(&label),
        profiles,
        probe,
    );
    if let Some(c) = panel_frame {
        if current_changed {
            synced.moved.push(c);
            synced.moved.sort_unstable();
        }
        if current_left_off {
            synced.profile_left_off.push(c);
            synced.profile_left_off.sort_unstable();
        }
    }
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let mut said = if synced.moved.is_empty() {
        format!(
            "the {} frame{} had these already",
            targets.len(),
            plural(targets.len())
        )
    } else {
        format!(
            "pasted {} section{} from {} onto {} frame{}",
            sections.len(),
            plural(sections.len()),
            clip.name(),
            synced.moved.len(),
            plural(synced.moved.len())
        )
    };
    said.push_str(&profile_left_off_words(st, &synced));
    if current_changed {
        // As a preset click's: the develop sets its own words, and
        // these go first once it lands.
        let before = st.generation;
        take_current(st, app, worker);
        if st.generation == before {
            app.set_status(said.into());
        } else {
            st.status_after_develop = Some((st.generation, said));
        }
    } else {
        app.set_status(said.into());
    }
    synced
}

/// The sections checked on the sheet, in `Section::ALL`'s order.
fn checked(st: &State) -> Vec<Section> {
    Section::ALL
        .iter()
        .enumerate()
        .filter(|(i, _)| st.sync_sections.row_data(*i).unwrap_or(false))
        .map(|(_, s)| *s)
        .collect()
}

/// Whether a viewport tool is in hand, as the window's
/// `tool-in-hand` has it: the sheet is not opened over one.
fn tool_in_hand(app: &App) -> bool {
    !app.get_placing().is_empty()
        || !app.get_picking().is_empty()
        || !app.get_guide_mode().is_empty()
        || app.get_level_mode()
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    // Copy and paste: Ctrl+C and Ctrl+V, and the frame menu's two.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_copy_asked(move || {
            if let Some(app) = app_weak.upgrade() {
                copy_settings(&mut state.borrow_mut(), &app);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_paste_asked(move || {
            if let Some(app) = app_weak.upgrade() {
                paste_open(&mut state.borrow_mut(), &app);
            }
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
            if tool_in_hand(&app) {
                app.set_status("put the tool down first (Esc), then sync".into());
                return;
            }
            let (Some(c), targets) = (st.current, sync_targets(&st)) else {
                return;
            };
            if targets.is_empty() {
                app.set_status("choose the frames to sync onto: Ctrl+click or Shift+click".into());
                return;
            }
            st.sync_asked = Some((c, targets));
            st.paste_asked = None;
            app.set_sync_paste(false);
            for (i, s) in Section::ALL.iter().enumerate() {
                st.sync_sections.set_row_data(i, s.syncs_by_default());
            }
            app.set_sync_section_names(ModelRc::new(VecModel::from(
                Section::ALL
                    .iter()
                    .map(|s| slint::SharedString::from(sync_title(*s)))
                    .collect::<Vec<_>>(),
            )));
            app.set_sync_section_on(ModelRc::from(st.sync_sections.clone()));
            app.set_sync_text(sync_words(&st).into());
            app.set_sync_open(true);
        });
    }
    {
        let state = state.clone();
        app.on_sync_section_toggled(move |i, on| {
            state.borrow().sync_sections.set_row_data(i as usize, on);
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_sync_applied(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let sections = checked(&st);
            if sections.is_empty() {
                app.set_status("choose what goes across".into());
                return;
            }
            // Read before the sheet closes: closing it clears the flag.
            let paste = app.get_sync_paste();
            app.set_sync_open(false);
            if paste {
                app.set_sync_paste(false);
                // As a sync's: what the sheet named, or nothing.
                let asked = st.paste_asked.take();
                let now = match (&st.clipboard, st.current) {
                    (Some(clip), Some(c)) => {
                        Some((c, clip.targets(&chosen_frames(&st), &st.files)))
                    }
                    _ => None,
                };
                if asked.is_none() || asked != now {
                    tracing::warn!("paste refused: the selection moved while the sheet was open");
                    app.set_status(
                        "the selection changed while the sheet was open; nothing pasted".into(),
                    );
                    return;
                }
                st.sync_last = Some(sections.clone());
                let profiles = camera::list();
                let start = std::time::Instant::now();
                let synced = paste_selection(&mut st, &app, &worker, &sections, &profiles, probe);
                tracing::info!(
                    "pasted onto {} frames in {:.1} ms",
                    synced.moved.len(),
                    start.elapsed().as_secs_f64() * 1e3
                );
                return;
            }
            // What the sheet said is what is done, or nothing: a
            // selection that moved while the sheet was up is not the
            // one its words named.
            let asked = st.sync_asked.take();
            let targets = sync_targets(&st);
            let now = st.current.map(|c| (c, targets.clone()));
            if asked.is_none() || asked != now {
                tracing::warn!("sync refused: the selection moved while the sheet was open");
                app.set_status(
                    "the selection changed while the sheet was open; nothing synced".into(),
                );
                return;
            }
            if targets.is_empty() {
                app.set_status("nothing to sync onto".into());
                return;
            }
            // A paste's sheet opens on this choice from now on.
            st.sync_last = Some(sections.clone());
            let profiles = camera::list();
            let start = std::time::Instant::now();
            let synced = sync_selection(&mut st, &app, &sections, &profiles, probe);
            let took = start.elapsed();
            let said = synced_words(&st, sections.len(), targets.len(), &synced);
            tracing::info!("{said} in {:.1} ms", took.as_secs_f64() * 1e3);
            app.set_status(said.into());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::browser::open_row;
    use crate::testing::{click, folder, press, state_for, window};
    use slint::platform::{Key, WindowEvent};

    /// Where row `i` of the strip is, in a 1500 by 950 window: the
    /// strip is the window's foot, 148 high, its cells 178 wide and
    /// 186 apart after a pad of 12.
    fn strip_cell(i: usize) -> (f32, f32) {
        (12.0 + i as f32 * 186.0 + 89.0, 950.0 - 148.0 + 60.0)
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
        let (x, y) = strip_cell(1);
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
            let (x, y) = strip_cell(3);
            click(&app, x, y);
            let (x, y) = strip_cell(4);
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
        let on = app.get_sync_section_on();
        let names = app.get_sync_section_names();
        assert_eq!(on.row_count(), Section::ALL.len());
        for (i, s) in Section::ALL.iter().enumerate() {
            assert_eq!(names.row_data(i).unwrap(), sync_title(*s));
            assert_eq!(on.row_data(i).unwrap(), *s != Section::Adjustments, "{s:?}");
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
            let (x, y) = strip_cell(4);
            click(&app, x, y);
        });
        assert_eq!(chosen_frames(&state.borrow()), vec![1, 2, 3, 4]);
        assert_eq!(state.borrow().current, Some(1));
        // A plain click on the frame on screen: back to it alone.
        let (x, y) = strip_cell(1);
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
    fn apply_lays_the_current_frame_over_the_others_and_saves_them() {
        let dir = std::env::temp_dir().join(format!("greycard-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..3)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let app = window(3);
        let (state, _worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.write_sidecars = true;
            st.placement = greycard_edit::Placement::Folder;
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
            st.picked = vec![0, 2];
        }
        app.invoke_sync_open_asked();
        assert!(app.get_sync_open());
        app.invoke_sync_applied();
        assert!(!app.get_sync_open());
        assert_eq!(app.get_status(), "synced 17 sections onto 1 frame");

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
        std::fs::remove_dir_all(&dir).expect("the temp dir goes");
    }

    fn entry(name: &str, made_for: Option<&str>) -> camera::Entry {
        camera::Entry {
            name: name.into(),
            path: PathBuf::new(),
            title: None,
            unique_camera_model: made_for.map(str::to_string),
            copyright: None,
        }
    }

    fn body(make: &str, model: &str, iso: u32) -> greycard_core::decode::Probe {
        greycard_core::decode::Probe {
            make: make.into(),
            model: model.into(),
            iso: Some(iso),
            exposure_time: None,
            fnumber: None,
            ..Default::default()
        }
    }

    #[test]
    fn a_profile_fits_only_the_body_it_was_made_for() {
        let r5 = entry("r5", Some("Canon EOS R5"));
        assert!(profile_fits(Some(&r5), Some(("Canon", "EOS R5"))));
        assert!(!profile_fits(Some(&r5), Some(("Fujifilm", "X-T5"))));
        // A frame that cannot say what took it does not get it.
        assert!(!profile_fits(Some(&r5), None));
        // A profile that names no camera, or one the directory has
        // not got, cannot be checked, and goes as a preset's would.
        assert!(profile_fits(Some(&entry("any", None)), None));
        assert!(profile_fits(Some(&entry("any", Some(" "))), None));
        assert!(profile_fits(None, Some(("Fujifilm", "X-T5"))));
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
            st.seed_blend = vec![false, true, true];
        }
        open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
        state.borrow_mut().picked = vec![0, 1, 2];
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
        assert_eq!(st.seed_blend, [false, false, false]);
        assert!(
            synced_words(&st, 2, 2, &synced)
                .ends_with("; the camera profile left off IMG_0002.CR3 (made for another camera)")
        );
        drop(st);

        // With Noise synced the blend is the source's, and nothing
        // is seeded over it on a first open.
        state.borrow_mut().seed_blend = vec![false, true, true];
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
        assert_eq!(st.seed_blend, [false, false, false]);
        assert_eq!(
            st.sidecars[2].current.noise.learned_strength,
            st.sidecars[0].current.noise.learned_strength
        );
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
                let (x, y) = strip_cell(i);
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
        state.borrow_mut().picked = vec![1, 3];
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
        state.borrow_mut().picked = vec![1, 2];
        {
            let mut st = state.borrow_mut();
            st.sidecars[1].meta.flag = greycard_edit::meta::Flag::Reject;
            st.filter = filter::Filter::from_name("No rejects").expect("it parses");
            st.shown = vec![0, 2, 3];
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
        let _ = std::fs::remove_dir_all(&dir);
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
            st.write_sidecars = true;
            st.placement = greycard_edit::Placement::Folder;
            st.presets = vec![Entry {
                path: PathBuf::new(),
                preset: preset.clone(),
            }];
            // Frame 2 already has the preset's Light on it: it is left
            // out of what moves.
            st.sidecars[2].record(preset.applied(&Edit::default()));
        }
        app.invoke_select(0);
        with_modifier(&app, Key::Control, || {
            let (x, y) = strip_cell(1);
            click(&app, x, y);
            let (x, y) = strip_cell(2);
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
            app.get_history_names().row_data(0).as_deref(),
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
        std::fs::remove_dir_all(&dir).expect("the temp dir goes");
    }

    /// With one frame selected, a preset click is unchanged: the
    /// current frame alone, "applied" or "is on already", exactly as
    /// before the set existed.
    #[test]
    fn a_preset_click_with_one_frame_selected_is_unchanged() {
        let dir = std::env::temp_dir().join(format!("greycard-preset-one-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
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
            st.write_sidecars = true;
            st.placement = greycard_edit::Placement::Folder;
            st.presets = vec![Entry {
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

        std::fs::remove_dir_all(&dir).expect("the temp dir goes");
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

        state.borrow_mut().seed_blend = vec![true, true, true];
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
        assert_eq!(st.sidecars[0].current.noise.learned_strength, blend(100));
        assert_eq!(st.sidecars[1].current.noise.learned_strength, blend(3200));
        assert_eq!(st.seed_blend, [false, false, true], "the two seeded");
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
        state.borrow_mut().picked = vec![0, 1];

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
        state.borrow_mut().picked = vec![0];
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
        let generation = state.borrow().generation;
        let said = "Portra 400 applied; the camera profile left off IMG_0000.CR3 \
                     (made for another camera)";
        assert_eq!(
            state.borrow().status_after_develop,
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
                image: crate::worker::Developed::Halves(std::sync::Arc::new(
                    crate::worker::Halves {
                        width: 60,
                        height: 40,
                        pixels: Vec::new(),
                    },
                )),
                guide: std::sync::Arc::new(crate::finish::Guide::NONE),
                white: crate::worker::WhiteBase::IDENTITY,
                seconds: 0.1,
                detail: None,
                sharpen: None,
                dehaze: None,
                sources: Vec::new(),
                learned: crate::worker::LearnedReport::Off,
                fills: crate::worker::FillReport::default(),
            },
        );
        let status = app.get_status();
        assert!(status.starts_with(&format!("{said}; ")), "{status}");
        assert!(status.ends_with("60x40, developed in 0.10 s"), "{status}");
        assert!(state.borrow().status_after_develop.is_none(), "taken");
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
            let _ = std::fs::remove_dir_all(&dir);
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
                st.write_sidecars = true;
                st.placement = greycard_edit::Placement::Folder;
                st.presets = vec![Entry {
                    path: PathBuf::new(),
                    preset,
                }];
            }
            let on_disk = Sidecar::path_in(&files[0], greycard_edit::Placement::Folder);

            app.invoke_select(0);
            app.invoke_cull_toggled();
            assert!(state.borrow().cull.is_some(), "{tag}");
            // The set is made once the frame is open: opening it
            // makes the set that frame alone.
            let set = !also.is_empty();
            state.borrow_mut().picked = also;
            assert_eq!(
                chosen_frames(&state.borrow()).len(),
                if set { 2 } else { 1 },
                "{tag}"
            );
            app.invoke_preset_applied(0);
            assert!(
                state.borrow().cull.is_none(),
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
            std::fs::remove_dir_all(&dir).expect("the temp dir goes");
        }
    }

    /// A preset click over a set whose current frame needs a real
    /// develop leaves its own words for that develop to carry: the
    /// develop's own "WxH, developed in ..." does not stand alone the
    /// moment it lands, the two status lines racing.
    #[test]
    fn a_multi_frame_presets_words_ride_the_develop_that_lands() {
        let app = window(2);
        let (state, _worker) = state_for(&app, folder(2));
        // A white balance, not a light control: `same_develop` reads
        // the raw develop's own inputs, and exposure is the finish's,
        // so only this asks the worker for a fresh one.
        let edit = Edit {
            white_balance: WhiteBalance::Custom {
                temperature: 3200.0,
                tint: 0.0,
            },
            ..Edit::default()
        };
        let preset = Preset::from_edit("Portra 400", &edit, &[Section::WhiteBalance]);
        state.borrow_mut().presets = vec![Entry {
            path: PathBuf::new(),
            preset,
        }];
        app.invoke_select(0);
        state.borrow_mut().picked = vec![0, 1];
        app.invoke_preset_applied(0);
        assert_eq!(
            app.get_status(),
            "developing...",
            "take_current's own word, for now"
        );
        let generation = state.borrow().generation;
        assert_eq!(
            state.borrow().status_after_develop,
            Some((generation, "Portra 400 onto 2 frames".to_string()))
        );

        crate::panel::deliver::deliver(
            &app,
            crate::worker::Outcome::Developed {
                generation,
                image: crate::worker::Developed::Halves(std::sync::Arc::new(
                    crate::worker::Halves {
                        width: 100,
                        height: 80,
                        pixels: Vec::new(),
                    },
                )),
                guide: std::sync::Arc::new(crate::finish::Guide::NONE),
                white: crate::worker::WhiteBase::IDENTITY,
                seconds: 0.42,
                detail: None,
                sharpen: None,
                dehaze: None,
                sources: Vec::new(),
                learned: crate::worker::LearnedReport::Off,
                fills: crate::worker::FillReport::default(),
            },
        );
        // The set's own words first, so they are not the ones a long
        // develop line clips.
        let status = app.get_status();
        assert!(status.starts_with("Portra 400 onto 2 frames; "), "{status}");
        assert!(status.ends_with("100x80, developed in 0.42 s"), "{status}");
        assert!(state.borrow().status_after_develop.is_none(), "taken");
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
        state.borrow_mut().presets = vec![Entry {
            path: PathBuf::new(),
            preset,
        }];
        app.invoke_select(0);
        let rows = |app: &App| -> Vec<String> {
            let names = app.get_history_names();
            (0..names.row_count())
                .map(|r| names.row_data(r).unwrap().to_string())
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
        let names = app.get_history_names();
        (0..names.row_count())
            .map(|r| names.row_data(r).unwrap().to_string())
            .collect()
    }

    #[test]
    fn copy_and_paste_lay_the_copy_over_the_set_and_leave_its_source() {
        let dir = std::env::temp_dir().join(format!("greycard-paste-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
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
            st.write_sidecars = true;
            st.placement = greycard_edit::Placement::Folder;
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
            let clip = st.clipboard.as_ref().expect("a copy");
            assert_eq!(clip.from, files[1]);
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
            let (x, y) = strip_cell(0);
            click(&app, x, y);
            let (x, y) = strip_cell(3);
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
        let on = app.get_sync_section_on();
        for (i, s) in Section::ALL.iter().enumerate() {
            assert_eq!(on.row_data(i).unwrap(), s.syncs_by_default(), "{s:?}");
        }
        // Light alone, then Paste.
        for (i, s) in Section::ALL.iter().enumerate() {
            app.invoke_sync_section_toggled(i as i32, *s == Section::Light);
        }
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
            assert_eq!(st.sync_last.as_deref(), Some(&[Section::Light][..]));
        }
        // A second paste opens on the choice the first made, and with
        // nothing new to lay over, records nothing.
        with_control(&app, || press(&app, "v"));
        let on = app.get_sync_section_on();
        for (i, s) in Section::ALL.iter().enumerate() {
            assert_eq!(on.row_data(i).unwrap(), *s == Section::Light, "{s:?}");
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
        let on = app.get_sync_section_on();
        for (i, s) in Section::ALL.iter().enumerate() {
            assert_eq!(on.row_data(i).unwrap(), s.syncs_by_default(), "{s:?}");
        }
        press(&app, Key::Escape);
        std::fs::remove_dir_all(&dir).expect("the temp dir goes");
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

    #[test]
    fn a_paste_in_culling_goes_onto_the_sidecars_and_stays_in_culling() {
        let dir = std::env::temp_dir().join(format!("greycard-paste-cull-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..3)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let app = window(3);
        let (state, _worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.write_sidecars = true;
            st.placement = greycard_edit::Placement::Folder;
            let mut edit = Edit::default();
            edit.light.exposure = 1.1;
            st.sidecars[2].record(edit);
        }
        app.invoke_select(0);
        app.invoke_cull_toggled();
        assert!(state.borrow().cull.is_some());
        // In culling the copy is the sidecar's, not the panel's.
        state.borrow_mut().current = Some(2);
        app.invoke_copy_asked();
        assert_eq!(
            state
                .borrow()
                .clipboard
                .as_ref()
                .map(|c| c.edit.light.exposure),
            Some(1.1)
        );
        // Back on frame 0, with frame 1 beside it: both take it on
        // their sidecars, the frame on screen as any other.
        state.borrow_mut().current = Some(0);
        state.borrow_mut().picked = vec![0, 1];
        app.invoke_paste_asked();
        app.invoke_sync_applied();
        assert!(
            state.borrow().cull.is_some(),
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
        std::fs::remove_dir_all(&dir).expect("the temp dir goes");
    }

    #[test]
    fn a_selection_moved_under_the_paste_sheet_is_refused() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(0);
        app.set_exposure(0.3);
        app.invoke_copy_asked();
        app.invoke_select(1);
        state.borrow_mut().picked = vec![1, 2];
        app.invoke_paste_asked();
        assert!(app.get_sync_open());
        state.borrow_mut().picked = vec![1, 3];
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
        assert!(state.borrow().clipboard.is_none());
        assert!(!app.get_sync_open());
        // Given the keys back, the same Ctrl+C copies the frame.
        press(&app, Key::Escape);
        with_control(&app, || press(&app, "c"));
        assert!(state.borrow().clipboard.is_some());
    }
}
