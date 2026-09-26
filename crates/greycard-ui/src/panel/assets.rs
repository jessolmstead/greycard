use crate::panel::browser::file_name;
use crate::panel::cull::{control_over_frame, leave_cull};
use crate::panel::edit::{develop_soon, read_edit, schedule_save};
use crate::panel::sync;
use crate::*;
use greycard_edit::camera;

/// Whether to offer the lens profiles unprompted: only with no
/// database on the machine, never after a Not now, and once a launch.
/// A user who never opens the LENS section would otherwise never
/// learn that nothing is being corrected.
pub(crate) fn offer_lenses_unprompted(database: bool, declined: bool, asked: bool) -> bool {
    !database && !declined && !asked
}

/// The first-launch offer, when `offer_lenses_unprompted` says so and
/// nothing else has the screen: a sheet already up, a download on
/// its way, or a batch run, which nobody is watching and whose
/// capture a sheet would spoil. Called with no database found; the
/// offer waits for a later picture when something is in the way.
pub(crate) fn offer_lenses_once(st: &mut State, app: &App) {
    if !offer_lenses_unprompted(false, st.lenses_declined, st.lenses_asked) {
        return;
    }
    // Any sheet: the window's own list of them, so a sheet added
    // later is covered here without being named.
    let busy = st.fetch.is_some() || st.fetching || app.invoke_sheet_open();
    if busy || st.batch {
        return;
    }
    st.lenses_asked = true;
    tracing::info!("no lens profiles on this machine: offering the download");
    offer_lenses(st, app, true);
}

/// The first line of the lens profiles' unprompted offer.
pub(crate) const LENSES_WHY: &str = "There are no lens profiles on this machine, so no lens is \
     corrected for its distortion, color fringes or vignetting.";

/// Open the download sheet for the lens database: `unprompted` when
/// no one asked for it, and the sheet says why it is up.
pub(crate) fn offer_lenses(st: &mut State, app: &App, unprompted: bool) {
    st.fetch = Some(Fetch::Lenses);
    app.set_fetch_title("Download the lens profiles?".into());
    let details = format!(
        "The lensfun database, about {} KB from {}.
License: {} ({}).
Kept in {}.",
        greycard_lens::store::APPROX_BYTES / 1000,
        greycard_lens::store::SOURCES[0]
            .split('/')
            .nth(2)
            .unwrap_or("its site"),
        greycard_lens::store::LICENSE.0,
        greycard_lens::store::LICENSE.1,
        greycard_lens::Store::user().map_or("no cache directory".to_string(), |s| s
            .root()
            .display()
            .to_string()),
    );
    // Unasked, the sheet says first why it is up: the reason is the
    // news, and the particulars follow it.
    let text = if unprompted {
        format!("{LENSES_WHY}\n\n{details}")
    } else {
        details
    };
    app.set_fetch_text(text.into());
    let note =
        "greycard does not ship the profiles; they are fetched for you under their own license.";
    app.set_fetch_note(
        if unprompted {
            format!("{note} Not now asks no more; the LENS section keeps the button.")
        } else {
            note.to_string()
        }
        .into(),
    );
    app.set_fetch_open(true);
}

/// A model's size in whole MB, rounded rather than truncated: BiRefNet
/// lite's rewrite is 113,778,088 bytes, which truncating division
/// reads as 113 and rounding as the 114 the note gives.
fn mb(bytes: u64) -> u64 {
    (bytes + 500_000) / 1_000_000
}

/// The model sheet's note on where the model comes from: one greycard
/// changed and publishes itself says so, and under whose license.
pub(crate) fn model_note(model: &greycard_ai::Model) -> String {
    match model.modified {
        Some(_) => format!(
            "greycard publishes this copy, changed from the original for speed; it is fetched for you under the original's {} license.",
            model.license.name
        ),
        None => "greycard does not ship this model; it is fetched for you under its own license."
            .to_string(),
    }
}

/// Open the model sheet for `model`. `falls_back` is set when this
/// offer is the Subject rewrite standing in for an original already in
/// the store — one on record as falling back to the CPU on this
/// card — rather than a first Subject offer, which the sheet's opening
/// line says plainly instead of leaving the reader to guess why a
/// second Subject file is being offered at all.
pub(crate) fn offer_model(
    st: &mut State,
    app: &App,
    model: &'static greycard_ai::Model,
    falls_back: bool,
) {
    st.fetch = Some(Fetch::Model(model));
    let note = model_note(model);
    app.set_fetch_note(
        if falls_back {
            format!(
                "The Subject model in your store falls back to the CPU on this card; a rewrite \
                 of the same weights runs on it, {} MB. {note}",
                mb(model.bytes()),
            )
        } else {
            note
        }
        .into(),
    );
    app.set_fetch_title(format!("Download {}?", model.name).into());
    app.set_fetch_text(
        format!(
            "{} MB from {}.
License: {} ({}).
Kept in {}.",
            mb(model.bytes()),
            model.files[0]
                .url
                .split('/')
                .nth(2)
                .unwrap_or("the model host"),
            model.license.name,
            model.license.url,
            st.store
                .as_ref()
                .map_or("no cache directory".to_string(), |s| s
                    .root()
                    .display()
                    .to_string()),
        )
        .into(),
    );
    app.set_fetch_open(true);
}

/// The presets' names on the panel. A removal waiting on its Remove
/// is dropped: the row it named may be another preset's now.
pub(crate) fn show_presets(st: &State, app: &App) {
    app.set_preset_confirm_remove(-1);
    app.set_preset_names(ModelRc::new(VecModel::from(
        st.presets
            .iter()
            .map(|e| slint::SharedString::from(e.preset.name.as_str()))
            .collect::<Vec<_>>(),
    )));
}

/// List the store again and show it.
pub(crate) fn refresh_presets(st: &mut State, app: &App) {
    st.presets = st
        .preset_store
        .as_ref()
        .map(|s| s.list())
        .unwrap_or_default();
    show_presets(st, app);
    show_presets_missing(st, app);
}

/// The settings' PRESETS row: which shipped presets the store is
/// without, and so whether Restore is offered.
pub(crate) fn show_presets_missing(st: &State, app: &App) {
    let missing = st
        .preset_store
        .as_ref()
        .map(|s| s.missing_shipped())
        .unwrap_or_default();
    app.set_presets_missing(missing.len() as i32);
    app.set_presets_missing_words(
        presets_missing_words(st.preset_store.is_some(), &missing).into(),
    );
}

/// The settings' words for the shipped presets not in the store.
pub(crate) fn presets_missing_words(store: bool, missing: &[String]) -> String {
    if !store {
        return "No configuration directory to keep presets in.".into();
    }
    match missing {
        [] => "Every default preset is in the list.".into(),
        [one] => format!("{one} is not in the list."),
        [rest @ .., last] => format!("{} and {last} are not in the list.", rest.join(", ")),
    }
}

/// Bring a preset file into the store, Lightroom's or this engine's,
/// and say what came across.
pub(crate) fn import_preset(st: &mut State, app: &App, path: &Path) {
    let imported = match preset::import(path) {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!("preset import {}: {e}", file_name(path));
            app.set_status(format!("{}: {e}", file_name(path)).into());
            return;
        }
    };
    let Some(store) = st.preset_store.as_ref() else {
        tracing::warn!("preset import: no configuration directory to save into");
        app.set_status("no configuration directory to save into".into());
        return;
    };
    let name = imported.preset.name.clone();
    match store.save(&imported.preset) {
        Ok(_) => {
            let sections = imported.preset.sections.len();
            let mut status = format!(
                "imported {name} ({sections} section{})",
                if sections == 1 { "" } else { "s" }
            );
            if !imported.unmapped.is_empty() {
                status.push_str(&format!(
                    "; no place here for {}",
                    imported.unmapped.join(", ")
                ));
            }
            app.set_status(status.into());
        }
        Err(e) => {
            tracing::warn!("preset {name}: not saved: {e}");
            app.set_status(format!("saving {name}: {e}").into());
        }
    }
    refresh_presets(st, app);
}

/// What the LENS panel says of the open file: the profile found, or
/// why there is none.
pub(crate) fn show_lens(report: &LensReport, app: &App) {
    app.set_lens_available(matches!(report, LensReport::Found { .. }));
    app.set_lens_database(!matches!(report, LensReport::NoDatabase));
    app.set_lens_profile(
        match report {
            LensReport::NoDatabase => "no lens profiles on this machine".to_string(),
            LensReport::NoLensName => "the file names no lens".to_string(),
            LensReport::Unknown(name) => format!("no profile for {name}"),
            LensReport::Found {
                lens,
                camera: true,
                smaller_sensor: false,
            } => lens.clone(),
            LensReport::Found {
                lens,
                camera: true,
                smaller_sensor: true,
            } => format!("{lens} (measured on a smaller sensor)"),
            LensReport::Found {
                lens,
                camera: false,
                ..
            } => format!("{lens} (body unknown, its own format assumed)"),
        }
        .into(),
    );
}

/// What the CAMERA PROFILE section lists for the open file: the
/// file's own calibrations first, then the profiles in the profile
/// directory that were made for this camera, then the chosen one when
/// it is neither.
pub(crate) fn show_profiles(st: &State, chosen: &greycard_edit::camera::ProfileChoice, app: &App) {
    let (make, model) = (st.camera.0.as_str(), st.camera.1.as_str());
    let mut names = vec![slint::SharedString::from(greycard_edit::camera::EMBEDDED)];
    let mut labels = vec![slint::SharedString::from("Embedded")];
    let embedded = chosen.is_embedded();
    let chosen_name = chosen.name().to_string();
    let chosen = chosen_name.as_str();
    let mut hidden = 0;
    for e in &st.profiles {
        if e.fits(make, model) || e.name == chosen {
            names.push(e.name.as_str().into());
            labels.push(e.label().into());
        } else {
            hidden += 1;
        }
    }
    // A profile the edit names and the directory does not have is
    // still shown, and chosen, so the panel says what the edit says.
    if !embedded && !names.iter().any(|n| n.as_str() == chosen) {
        names.push(chosen.into());
        labels.push(format!("{chosen} (missing)").into());
    }
    app.set_camera_profile_names(ModelRc::new(VecModel::from(names)));
    app.set_camera_profile_labels(ModelRc::new(VecModel::from(labels)));
    app.set_camera_profile(chosen.into());
    app.set_camera_profile_note(
        match (st.profiles.len(), hidden) {
            (0, _) => match greycard_edit::camera::store_dir() {
                Some(dir) => format!("No profiles yet: put DCP files in {}.", dir.display()),
                None => "No profile directory on this machine.".to_string(),
            },
            (_, 0) => String::new(),
            (_, n) => format!("{n} more there, for other cameras."),
        }
        .into(),
    );
    app.set_camera_profile_warning(
        greycard_edit::camera::warning(&st.profiles, &st.camera, chosen_name.as_str()).into(),
    );
}

/// The look directory on the panel: None first, then the tables it
/// holds, with the chosen one selected and a word under the list.
///
/// A table the edit names and the directory has not got is still
/// shown, and chosen, so the panel says what the edit says — the
/// profile list's rule, and for the same reason: a preset or a
/// sidecar from another machine.
pub(crate) fn show_looks(st: &State, chosen: &greycard_edit::look::LookLut, app: &App) {
    let chosen_name = chosen.lut.name().to_string();
    let chosen_name = chosen_name.as_str();
    let rows = greycard_edit::look::rows(&st.looks, chosen_name);
    let names: Vec<slint::SharedString> = rows.iter().map(|(n, _)| n.as_str().into()).collect();
    let labels: Vec<slint::SharedString> = rows.iter().map(|(_, l)| l.as_str().into()).collect();
    app.set_look_names(ModelRc::new(VecModel::from(names)));
    app.set_look_labels(ModelRc::new(VecModel::from(labels)));
    app.set_look_name(chosen_name.into());
    app.set_look_strength(chosen.strength);
    let note = match st.looks.iter().find(|e| e.name == chosen_name) {
        // What the chosen table is: its kind, its size, and what it
        // says it was made in when that is not the usual sRGB.
        Some(entry) => entry.described(),
        None if st.looks.is_empty() => match greycard_edit::look::store_dir() {
            Some(dir) => format!(
                "No looks yet: put .cube or HaldCLUT .png files in {}.",
                dir.display()
            ),
            None => "No look directory on this machine.".to_string(),
        },
        None => String::new(),
    };
    app.set_look_note(note.into());
    app.set_look_warning(greycard_edit::look::warning(&st.looks, chosen_name).into());
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    // The LENS panel asks for the profiles.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_fetch_lenses(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if st.fetch.is_none() && !st.fetching {
                offer_lenses(&mut st, &app, false);
            }
        });
    }
    // The model sheet answered.
    {
        let (state, app_weak, worker) = (state.clone(), app.as_weak(), worker.clone());
        app.on_fetch_answered(move |yes| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            app.set_fetch_open(false);
            let Some(fetch) = st.fetch.take() else {
                return;
            };
            match (fetch, yes) {
                (Fetch::Model(model), true) => {
                    st.fetching = true;
                    worker.send(Job::Fetch { model });
                }
                (Fetch::Model(model), false) => {
                    st.declined.push(model.id);
                    let lost = if model.id == greycard_ai::SUBJECT_WEBGPU.id {
                        // The masks waiting on it turn to the original,
                        // which the next frame offers.
                        app.window().request_redraw();
                        "without the GPU model the original Subject model is offered"
                    } else if model.id == greycard_ai::FILL.id {
                        "without the model a fill is left as it was"
                    } else if greycard_ai::DENOISERS.iter().any(|(_, m)| m.id == model.id) {
                        "without the model the engine's own denoise stands in"
                    } else {
                        "without the model the shape masks nothing"
                    };
                    app.set_status(lost.into());
                }
                (Fetch::Lenses, true) => {
                    st.fetching = true;
                    worker.send(Job::FetchLenses);
                }
                (Fetch::Lenses, false) => {
                    app.set_status(
                        "without the profiles the lens is corrected by hand only".into(),
                    );
                    // Asked once: the offer does not come back on its
                    // own, this run or the next. Not from a batch run,
                    // which leaves the user's settings alone, nor from
                    // a test, which has no business with them.
                    st.lenses_declined = true;
                    if !st.batch && !cfg!(test) {
                        let mut settings = settings::Settings::load();
                        if !settings.lenses_declined {
                            settings.lenses_declined = true;
                            settings.save();
                        }
                    }
                }
            }
        });
    }
    // The CAMERA PROFILE section opened: read the directory again, so
    // a profile put there while the editor is running is listed
    // without reopening the file.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_camera_profiles_wanted(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.profiles = greycard_edit::camera::list();
            show_profiles(&st, &st.edit.camera.profile, &app);
        });
    }
    // A camera profile chosen in the panel: the edit says so, the
    // warning line follows, and the picture is developed again.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_camera_profile_picked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.edit = read_edit(&app, &st.edit, st.target);
            show_profiles(&st, &st.edit.camera.profile, &app);
            develop_soon(&mut st, state.clone(), worker.clone(), app.as_weak());
        });
    }
    // The LOOK section opened: read the directory again, as the
    // profile directory's is read, and ask for the chosen table
    // again, so a file replaced in place is picked up. Asking is
    // cheap: `look::load` hands back the same table unless its size
    // or its clock moved, and the renderer re-uploads nothing for a
    // table it already holds.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_looks_wanted(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.looks = greycard_edit::look::list();
            st.look_for = None;
            show_looks(&st, &st.edit.look_lut, &app);
            app.window().request_redraw();
        });
    }
    // A look chosen in the panel: the edit says so, the note and the
    // warning follow, and the viewport redraws. No develop — a look
    // is the finish, and the picture the engine hands over is the
    // same one.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_look_picked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // A look reached in culling is the way out of it, as
            // every other look control is.
            if st.cull.is_some() {
                let with = control_over_frame(&mut st, &app);
                leave_cull(&mut st, &app, &worker, with);
                return;
            }
            st.edit = read_edit(&app, &st.edit, st.target);
            show_looks(&st, &st.edit.look_lut, &app);
            schedule_save(&mut st, app.as_weak());
            app.window().request_redraw();
        });
    }
    // The presets: one laid over the edit as a step, one saved from
    // the edit, one brought in from a file, one removed. What a click
    // does is `sync::apply_preset`, so a test can drive the same
    // logic with fake bodies; this closure just resolves the preset
    // and the profile directory `camera::list` reads for real.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_preset_applied(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // Another row clicked is the question left unanswered.
            app.set_preset_confirm_remove(-1);
            let mut st = state.borrow_mut();
            let Some(preset) = st.presets.get(i as usize).map(|e| e.preset.clone()) else {
                return;
            };
            let profiles = camera::list();
            sync::apply_preset(&mut st, &app, &worker, &preset, &profiles, sync::probe);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_save_open(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            if st.current.is_none() {
                return;
            }
            // On to start: what the edit changed, of the sections a
            // preset carries by default.
            let edit = read_edit(&app, &st.edit, st.target);
            let plain = Edit::default();
            for (i, s) in Section::ALL.iter().enumerate() {
                st.preset_sections
                    .set_row_data(i, s.by_default() && !s.same(&edit, &plain));
            }
            app.set_preset_name("".into());
            app.set_preset_open(true);
        });
    }
    {
        let state = state.clone();
        app.on_preset_section_toggled(move |i, on| {
            state.borrow().preset_sections.set_row_data(i as usize, on);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_saved(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let name = app.get_preset_name().trim().to_string();
            if name.is_empty() {
                app.set_status("a preset needs a name".into());
                return;
            }
            let sections: Vec<Section> = Section::ALL
                .iter()
                .enumerate()
                .filter(|(i, _)| st.preset_sections.row_data(*i).unwrap_or(false))
                .map(|(_, s)| *s)
                .collect();
            if sections.is_empty() {
                app.set_status("choose what the preset carries".into());
                return;
            }
            let edit = read_edit(&app, &st.edit, st.target);
            let preset = Preset::from_edit(&name, &edit, &sections);
            app.set_preset_open(false);
            match st.preset_store.as_ref().map(|s| s.save(&preset)) {
                Some(Ok(path)) => {
                    app.set_status(
                        format!(
                            "saved {} ({} section{}) to {}",
                            preset.name,
                            sections.len(),
                            if sections.len() == 1 { "" } else { "s" },
                            path.display()
                        )
                        .into(),
                    );
                }
                Some(Err(e)) => {
                    tracing::warn!("preset {}: not saved: {e}", preset.name);
                    app.set_status(format!("saving {}: {e}", preset.name).into());
                }
                None => {
                    tracing::warn!(
                        "preset {}: no configuration directory to save into",
                        preset.name
                    );
                    app.set_status("no configuration directory to save into".into());
                }
            }
            refresh_presets(&mut st, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_preset_deleted(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(entry) = st.presets.get(i as usize).cloned() else {
                return;
            };
            match st.preset_store.as_ref().map(|s| s.remove(&entry)) {
                Some(Ok(())) => {
                    let name = &entry.preset.name;
                    // Promised only when the Restore would bring one
                    // of that name back: the shipped one, which need
                    // not be what the user had under the name.
                    let restorable = st.preset_store.as_ref().is_some_and(|s| {
                        s.missing_shipped()
                            .iter()
                            .any(|m| m.eq_ignore_ascii_case(name))
                    });
                    app.set_status(
                        if restorable {
                            format!("{name} removed; Settings can restore the default one")
                        } else {
                            format!("{name} removed")
                        }
                        .into(),
                    );
                }
                Some(Err(e)) => {
                    tracing::warn!("preset {}: not removed: {e}", entry.preset.name);
                    app.set_status(format!("removing {}: {e}", entry.preset.name).into());
                }
                None => {}
            }
            refresh_presets(&mut st, &app);
        });
    }
    // The settings' Restore: the shipped presets the store is without
    // written back, and nothing of the user's touched.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_presets_restore(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            match st.preset_store.as_ref().map(|s| s.restore_shipped()) {
                Some(Ok(names)) if names.is_empty() => {
                    app.set_status("every default preset is in the list".into());
                }
                Some(Ok(names)) => {
                    tracing::info!("presets restored: {}", names.join(", "));
                    app.set_status(format!("restored {}", names.join(", ")).into());
                }
                Some(Err(e)) => {
                    tracing::warn!("presets: not restored: {e}");
                    app.set_status(format!("restoring the default presets: {e}").into());
                }
                None => {
                    app.set_status("no configuration directory to restore into".into());
                }
            }
            refresh_presets(&mut st, &app);
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_preset_import(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let start = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
            let app_weak = app.as_weak();
            app.set_status("choosing a preset to import...".into());
            export::choose_open(
                "Import preset",
                start,
                (
                    "Presets".to_string(),
                    vec!["*.xmp".into(), "*.XMP".into(), "*.gcp".into()],
                ),
                move |chosen| {
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(app) = app_weak.upgrade() else {
                            return;
                        };
                        // The state through its thread-local, as the
                        // worker's results reach it.
                        let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                            return;
                        };
                        let mut st = state.borrow_mut();
                        match chosen {
                            Ok(Some(path)) => import_preset(&mut st, &app, &path),
                            Ok(None) => app.set_status("import canceled".into()),
                            Err(e) => {
                                tracing::warn!("file chooser: {e:#}");
                                app.set_status("the desktop offered no file chooser".into());
                            }
                        }
                    });
                },
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lens_profiles_are_offered_once_and_only_without_them() {
        // Every combination: only no database, never declined and
        // not asked yet this launch offers.
        for database in [false, true] {
            for declined in [false, true] {
                for asked in [false, true] {
                    assert_eq!(
                        offer_lenses_unprompted(database, declined, asked),
                        !database && !declined && !asked,
                        "database {database}, declined {declined}, asked {asked}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_first_launch_offer_opens_the_sheet_once() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);

        // Something else on screen: not now, and not spent either.
        app.set_export_open(true);
        offer_lenses_once(&mut state.borrow_mut(), &app);
        assert!(!app.get_fetch_open());
        assert!(!state.borrow().lenses_asked);
        app.set_export_open(false);
        // The settings sheet too: an offer under it would be answered
        // Not now, for good, by the Escape meant for the settings.
        app.invoke_settings_asked();
        assert!(app.get_settings_open());
        offer_lenses_once(&mut state.borrow_mut(), &app);
        assert!(!app.get_fetch_open(), "not under the settings");
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(!app.get_settings_open());
        assert!(
            !state.borrow().lenses_declined,
            "Escape closed the settings only"
        );
        assert!(!state.borrow().lenses_asked);

        offer_lenses_once(&mut state.borrow_mut(), &app);
        assert!(app.get_fetch_open(), "offered");
        assert_eq!(state.borrow().fetch, Some(Fetch::Lenses));
        assert!(app.get_fetch_text().starts_with(LENSES_WHY));

        // Not now: remembered, and the next picture asks nothing.
        app.invoke_fetch_answered(false);
        assert!(!app.get_fetch_open());
        assert!(state.borrow().lenses_declined);
        offer_lenses_once(&mut state.borrow_mut(), &app);
        assert!(!app.get_fetch_open(), "asked once");

        // A launch that was never declined still asks only once.
        {
            let mut st = state.borrow_mut();
            st.lenses_declined = false;
            st.lenses_asked = true;
        }
        offer_lenses_once(&mut state.borrow_mut(), &app);
        assert!(!app.get_fetch_open(), "once a launch");

        // The LENS section's own button is not the offer, and asks
        // whatever was answered before.
        app.invoke_fetch_lenses();
        assert!(app.get_fetch_open());
        assert!(!app.get_fetch_text().contains(LENSES_WHY));
    }

    #[test]
    fn the_missing_presets_are_named() {
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            presets_missing_words(true, &[]),
            "Every default preset is in the list."
        );
        assert_eq!(
            presets_missing_words(true, &names(&["Muted Slide"])),
            "Muted Slide is not in the list."
        );
        assert_eq!(
            presets_missing_words(true, &names(&["A", "B", "C"])),
            "A, B and C are not in the list."
        );
        assert!(presets_missing_words(false, &[]).starts_with("No configuration"));
    }

    /// The bin and the Remove as a pointer clicks them: the bin arms
    /// the row and removes nothing, Escape and Cancel disarm it, and
    /// only the Remove removes. Found by sweeping the presets list's
    /// own strip, top down and never past it: the pane scrolls its
    /// sections at their full height now (no more squeezing one to
    /// fit), so a sweep with no floor of its own could wander past
    /// the list and into what is below the section, or even below the
    /// pane — Settings, Report — a click that must never land here.
    ///
    /// Arming the row grows the section by the question and the
    /// Cancel/Remove row, and that growth is `Section`'s 120ms
    /// `animate full`; the mock clock this backend runs on does not
    /// move on its own, so without a nudge the section's clip stays
    /// at its unarmed size and the Remove is clipped away with
    /// everything below it. `mock_elapsed_time` past 120ms is that
    /// nudge, exactly as other pointer tests in this crate use it to
    /// settle an animation before reading where things landed.
    #[test]
    fn the_bin_asks_and_the_remove_removes() {
        let app = crate::testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = crate::testing::retouch_state(&app);
        let d = std::env::temp_dir().join(format!("greycard-ui-bin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let store = preset::Store::at(d.clone());
        store.seed();
        state.borrow_mut().preset_store = Some(store);
        refresh_presets(&mut state.borrow_mut(), &app);
        app.set_collapsed_navigator(true);
        let first = state.borrow().presets[0].clone();
        assert_eq!(state.borrow().presets.len(), 3, "the three shipped presets");

        // The pane is 240px wide, its bins in the right-hand strip;
        // the list is one of the first things in it with the
        // navigator folded, so a short sweep finds the first row's
        // bin well short of anything below the list.
        let mut bin = None;
        'sweep: for y in (60..280).step_by(3) {
            for x in (150..240).step_by(6) {
                crate::testing::click(&app, x as f32, y as f32);
                // A section header in the way folds its section:
                // unfolded again, so the layout holds still.
                app.set_collapsed_presets(false);
                app.set_collapsed_navigator(true);
                if app.get_preset_confirm_remove() >= 0 {
                    bin = Some((x as f32, y as f32));
                    break 'sweep;
                }
            }
        }
        let (bx, by) = bin.expect("a bin to click within the presets list");
        assert_eq!(app.get_preset_confirm_remove(), 0, "the first row's");
        assert!(first.path.exists(), "a bin alone removes nothing");

        // Escape is its Cancel.
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert_eq!(app.get_preset_confirm_remove(), -1);
        assert!(first.path.exists());

        // Row to row down the same column, from the first bin, to
        // where the list ends: measured, not guessed (three shipped
        // presets, so a third row's bin is the list's last), so the
        // sweep below can start just past it and never land on
        // another row by the time it reaches the Remove. The list
        // itself does not change size when a row arms or disarms, so
        // this part needs no settling.
        crate::testing::click(&app, bx, by);
        assert_eq!(app.get_preset_confirm_remove(), 0);
        let (mut row_step, mut third_row_y) = (None, None);
        for y in (by as i32 + 1)..(by as i32 + 120) {
            crate::testing::click(&app, bx, y as f32);
            match app.get_preset_confirm_remove() {
                1 if row_step.is_none() => row_step = Some(y - by as i32),
                2 => {
                    third_row_y = Some(y);
                    break;
                }
                _ => {}
            }
        }
        let row_step = row_step.expect("a second row's bin below the first") as f32;
        let third_row_y = third_row_y.expect("a third row's bin below the second") as f32;
        // One more row's worth past the third clears the box.
        let list_bottom = third_row_y + row_step;

        // Re-armed on the first row for the removal itself: past
        // 120ms, the section has grown to fit the question and the
        // Cancel/Remove row, so a sweep bounded to a few rows below
        // the list, in the pane's own width, lands on the list, the
        // question or the Cancel/Remove row — never the pane's
        // footer, whatever the settled height turns out to be.
        crate::testing::click(&app, bx, by);
        assert_eq!(app.get_preset_confirm_remove(), 0);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(150));
        let mut removed = false;
        'remove: for y in (list_bottom as i32..list_bottom as i32 + 150).step_by(3) {
            for x in (0..240).step_by(6) {
                crate::testing::click(&app, x as f32, y as f32);
                if !first.path.exists() {
                    removed = true;
                    break 'remove;
                }
                if app.get_preset_confirm_remove() < 0 {
                    // Cancel, or a row: arm it again and go on.
                    crate::testing::click(&app, bx, by);
                }
            }
        }
        assert!(removed, "the Remove removes");
        assert_eq!(state.borrow().presets.len(), 2);
        assert_eq!(app.get_preset_confirm_remove(), -1);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A bin asks and removes nothing; the Remove that names the
    /// preset removes it. A shipped one removed is then missing in the
    /// settings, and their Restore puts it back.
    #[test]
    fn a_preset_is_removed_only_when_confirmed_and_restored_on_asking() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        let d = std::env::temp_dir().join(format!("greycard-ui-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let store = preset::Store::at(d.clone());
        store.seed();
        state.borrow_mut().preset_store = Some(store);
        refresh_presets(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_presets_missing(), 0);
        let count = || state.borrow().presets.len();
        assert_eq!(count(), 3);
        let first = state.borrow().presets[0].clone();
        assert_eq!(first.preset.name, "Muted Slide");

        // The bin, which only arms the row: nothing is removed, and a
        // list shown again (a preset saved, one imported) drops the
        // question, whose row may be another preset's now.
        assert_eq!(app.get_preset_confirm_remove(), -1);
        app.set_preset_confirm_remove(0);
        assert!(first.path.exists(), "a bin alone removes nothing");
        refresh_presets(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_preset_confirm_remove(), -1);
        // So does a preset laid over the edit instead.
        app.set_preset_confirm_remove(0);
        app.invoke_preset_applied(1);
        assert_eq!(app.get_preset_confirm_remove(), -1);
        assert!(first.path.exists());

        // Asked again and confirmed, as the Remove does: gone, the
        // question with it, and the status says where it can be had
        // back.
        app.set_preset_confirm_remove(0);
        app.invoke_preset_deleted(0);
        assert_eq!(app.get_preset_confirm_remove(), -1);
        assert!(!first.path.exists());
        assert_eq!(count(), 2);
        assert_eq!(
            app.get_status(),
            "Muted Slide removed; Settings can restore the default one"
        );
        assert_eq!(app.get_presets_missing(), 1);
        assert_eq!(
            app.get_presets_missing_words(),
            "Muted Slide is not in the list."
        );

        // Restore: back, as shipped.
        app.invoke_presets_restore();
        assert!(first.path.exists());
        assert_eq!(count(), 3);
        assert_eq!(app.get_presets_missing(), 0);
        assert_eq!(app.get_status(), "restored Muted Slide");

        // A preset of the user's own promises nothing when it goes;
        // nor does a shipped name while another preset still has it.
        let mine = Preset::from_edit("Mine", &Edit::default(), &[Section::Light]);
        let dup = Preset::from_edit("warm negative", &Edit::default(), &[Section::Light]);
        {
            let st = state.borrow();
            let store = st.preset_store.as_ref().unwrap();
            store.save(&mine).unwrap();
            dup.save_to(&d.join("warm-negative-copy.gcp")).unwrap();
        }
        refresh_presets(&mut state.borrow_mut(), &app);
        let at = |name: &str| {
            state
                .borrow()
                .presets
                .iter()
                .position(|e| e.preset.name == name)
                .unwrap() as i32
        };
        app.invoke_preset_deleted(at("Mine"));
        assert_eq!(app.get_status(), "Mine removed");
        app.invoke_preset_deleted(at("warm negative"));
        assert_eq!(app.get_status(), "warm negative removed");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_batch_run_is_never_offered_the_lens_profiles() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        state.borrow_mut().batch = true;
        offer_lenses_once(&mut state.borrow_mut(), &app);
        assert!(!app.get_fetch_open());
    }
}
