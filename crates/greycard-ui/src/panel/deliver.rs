use crate::panel::assets::{offer_lenses_once, offer_model, show_lens, show_looks, show_profiles};
use crate::panel::browser::{file_name, show_thumb, thumb_turns};
use crate::panel::cull::develop_landed;
use crate::panel::edit::{current_turn, read_edit, schedule_save};
use crate::panel::startup::remember_last_file;
use crate::panel::viewport::picking_hint;
use crate::settings;
use crate::sheet::{self, ExportPreset, Sheet};
use crate::*;

/// A develop the worker sent, waiting for the frame that puts it on
/// the GPU.
///
/// It carries the generation it was asked for as well as the picture:
/// by the time the frame runs, the selection may have moved on again,
/// and a camera picture standing in for the newer frame's develop
/// must not be taken down by this one.
pub(crate) struct Landed {
    pub(crate) image: Developed,
    /// The tone equalizer's guide plane for it.
    pub(crate) guide: Arc<finish::Guide>,
    /// The white balance it was developed at.
    pub(crate) white: WhiteBase,
    pub(crate) generation: u64,
}

/// Names in a sentence: "Fill 1", "Fill 1 and Fill 2", "Fill 1,
/// Fill 2 and Fill 3".
pub(crate) fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

/// Where an export of `raw` goes without a chooser: beside it, as
/// `NAME.greycard.EXT`, which no camera writes.
pub(crate) fn export_path(raw: &std::path::Path, format: export::Format) -> PathBuf {
    let stem = raw
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export".into());
    raw.with_file_name(format!("{stem}.greycard.{}", format.extension()))
}

/// The export sheet's choices as the panel holds them.
pub(crate) fn read_sheet(app: &App) -> Sheet {
    Sheet {
        format: app.get_export_format().into(),
        quality: app.get_export_quality(),
        size: app.get_export_size().into(),
        custom: app.get_export_custom().into(),
        space: app.get_export_space().into(),
        embed: app.get_export_embed(),
        sharpen: app.get_export_sharpen().into(),
        on_exists: app.get_export_on_exists().into(),
        metadata: app.get_export_metadata().into(),
        mark: app.get_export_mark().into(),
        mark_text: app.get_export_mark_text().into(),
        mark_image: app.get_export_mark_image().into(),
        mark_color: app.get_export_mark_color().into(),
        mark_position: app.get_export_mark_position().into(),
        mark_size: app.get_export_mark_size(),
        mark_margin: app.get_export_mark_margin(),
        mark_opacity: app.get_export_mark_opacity(),
    }
}

/// Put a sheet's choices on the panel.
pub(crate) fn show_sheet(app: &App, s: &Sheet) {
    app.set_export_format(s.format.as_str().into());
    app.set_export_quality(s.quality);
    app.set_export_size(s.size.as_str().into());
    app.set_export_custom(s.custom.as_str().into());
    app.set_export_space(s.space.as_str().into());
    app.set_export_embed(s.embed);
    app.set_export_sharpen(s.sharpen.as_str().into());
    app.set_export_on_exists(s.on_exists.as_str().into());
    app.set_export_metadata(s.metadata.as_str().into());
    app.set_export_mark(s.mark.as_str().into());
    app.set_export_mark_text(s.mark_text.as_str().into());
    app.set_export_mark_image(s.mark_image.as_str().into());
    app.set_export_mark_image_name(file_name(Path::new(&s.mark_image)).into());
    app.set_export_mark_color(s.mark_color.as_str().into());
    app.set_export_mark_position(s.mark_position.as_str().into());
    app.set_export_mark_size(s.mark_size);
    app.set_export_mark_margin(s.mark_margin);
    app.set_export_mark_opacity(s.mark_opacity);
}

/// The picker's word for no preset. A preset cannot take it as a
/// name, in any case.
pub(crate) const NO_PRESET: &str = sheet::NO_PRESET;

/// The preset the panel has chosen, none for `NO_PRESET`.
pub(crate) fn chosen_preset(app: &App) -> Option<String> {
    let name = app.get_export_preset();
    (!name.is_empty() && !sheet::reserved(&name)).then(|| name.to_string())
}

/// The picker's entries and the one chosen, and whether the sheet has
/// moved from it: the presets as `presets` has them.
pub(crate) fn show_presets_picker(app: &App, presets: &[ExportPreset], chosen: Option<&str>) {
    let names: Vec<slint::SharedString> = std::iter::once(NO_PRESET.into())
        .chain(presets.iter().map(|p| p.name.as_str().into()))
        .collect();
    app.set_export_presets(ModelRc::new(VecModel::from(names)));
    let chosen = chosen.and_then(|n| sheet::find(presets, n));
    app.set_export_preset(chosen.map_or(NO_PRESET, |p| p.name.as_str()).into());
    show_edited(app, presets);
}

/// "(edited)" beside the name when the sheet writes another file than
/// the preset does.
pub(crate) fn show_edited(app: &App, presets: &[ExportPreset]) {
    let edited = chosen_preset(app)
        .and_then(|n| sheet::find(presets, &n).cloned())
        .is_some_and(|p| !p.sheet.same(&read_sheet(app)));
    if app.get_export_preset_edited() != edited {
        app.set_export_preset_edited(edited);
    }
}

/// The export sheet as the panel shows it.
pub(crate) fn read_export_settings(app: &App) -> export::Settings {
    read_sheet(app).settings()
}

/// A batch export's settings: the sheet's, as remembered or as the
/// preset named on the command line fills it, in the format the
/// path's extension names, or the sheet's when it names none.
pub(crate) fn batch_settings(path: &Path, sheet: export::Settings) -> export::Settings {
    export::Settings {
        format: export::Format::from_path(path).unwrap_or(sheet.format),
        ..sheet
    }
}

/// The sheet's answer to a file of that name being there already.
pub(crate) fn read_on_exists(app: &App) -> export::OnExists {
    read_sheet(app).on_exists()
}

/// Keep the presets in the settings file now, not at the window's
/// close: a preset is worth keeping even from a session that ends
/// badly. Not from a snapshot or a batch run, which have no file to
/// write (`State::settings_file`).
fn keep_presets(st: &State, app: &App) {
    let Some(path) = &st.settings_file else {
        return;
    };
    let mut settings = settings::Settings::load_from(path);
    settings.export_presets = st.export_presets.clone();
    settings.export_preset = chosen_preset(app).unwrap_or_default();
    settings.export = read_sheet(app);
    settings.save_to(path);
}

/// A result from the worker, on the UI thread.
pub(crate) fn deliver(app: &App, outcome: Outcome) {
    let Some(state) = STATE.with(|s| s.borrow().clone()) else {
        return;
    };
    match outcome {
        Outcome::Thumbnail {
            index,
            path,
            size,
            width,
            height,
            rgb,
        } => {
            let mut st = state.borrow_mut();
            // A thumbnail of a list since replaced by another folder's
            // would land on a stranger's slot: only its own file's.
            if st.files.get(index) != Some(&path) {
                return;
            }
            st.thumb_base[index] = Some((width, height, rgb));
            st.thumb_made[index] = size;
            // One that was being made when the cells grew comes back
            // at the old size. Nothing else would ask for it again
            // until the grid next moved, and a snapshot waiting on
            // the grid would wait for ever, so ask here.
            if grid::wants_bigger(size, st.thumb_want)
                && st
                    .grid_shown
                    .is_some_and(|(f, l)| (f..=l).contains(&(index as i32)))
            {
                st.thumb_asked[index] = st.thumb_want;
                let path = path.clone();
                WORKER.with(|w| {
                    if let Some(w) = &*w.borrow() {
                        w.send(Job::Thumbnail { index, path });
                    }
                });
            }
            // Shown as its edit turns it: the open file's as the panel
            // has it, another's as its sidecar does.
            let (turns, flip) = thumb_turns(&st, app, index);
            show_thumb(&mut st, app, index, turns, flip);
        }
        Outcome::Opened {
            generation,
            as_shot,
            frame,
            profile,
            lens,
            kind,
            shot,
            blend,
        } => {
            let mut st = state.borrow_mut();
            if generation != st.generation {
                return;
            }
            // A fresh raw's learned-denoiser blend from its ISO: into
            // its sidecar, the edit under develop and the panel, once.
            if let Some(b) = blend {
                if let Some(i) = st.current {
                    if let Some(seed) = st.seed_blend.get_mut(i) {
                        *seed = false;
                    }
                    if let Some(s) = st.sidecars.get_mut(i) {
                        s.current.noise.learned_strength = b;
                    }
                }
                st.edit.noise.learned_strength = b;
                app.set_denoise_learned_strength(b);
            }
            st.frame = frame.zip(profile);
            // The camera the file names, and what the profile
            // directory holds for it: read on open, not on develop.
            st.camera = st
                .frame
                .as_ref()
                .map(|(f, _)| (f.make.clone(), f.model.clone()))
                .unwrap_or_default();
            st.profiles = greycard_edit::camera::list();
            st.looks = greycard_edit::look::list();
            // Another file, so the embedded profile behind the
            // white balance is another one too.
            st.white_profile = None;
            show_profiles(&st, &st.edit.camera.profile, app);
            // After the directory is read, not before: `show_edit`
            // ran earlier in this arm, when there was nothing to list.
            show_looks(&st, &st.edit.look_lut, app);
            app.set_shot_camera(shot.camera.as_str().into());
            app.set_shot_exposure(shot.exposure.as_str().into());
            match &kind {
                SourceKind::Raw => {
                    st.source = finish::Source::Scene;
                    app.set_raw_input(true);
                    app.set_source_note("".into());
                }
                SourceKind::Picture { space, bits } => {
                    st.source = finish::Source::Display;
                    app.set_raw_input(false);
                    app.set_source_note(
                        format!(
                            "{}-bit {} taken as {space}; its white balance, demosaic and noise are as rendered",
                            bits,
                            st.current
                                .and_then(|i| st.files.get(i))
                                .and_then(|f| f.extension())
                                .map(|e| e.to_string_lossy().to_ascii_uppercase())
                                .unwrap_or_else(|| "picture".into())
                        )
                        .into(),
                    );
                }
            }
            show_lens(&lens, app);
            // No database on the machine: the first time a picture
            // is up, the download is offered without being asked for.
            if matches!(lens, LensReport::NoDatabase) {
                offer_lenses_once(&mut st, app);
            }
            st.lens = lens;
            st.white_cache = None;
            // Show the frame's own white balance on the sliders, so a
            // custom setting starts from it.
            if st.edit.white_balance == WhiteBalance::AsShot {
                app.set_temperature(as_shot.0 as f32);
                app.set_tint(as_shot.1 as f32);
            }
            if let Some(t) = st.preview_temperature.take() {
                app.set_as_shot(false);
                app.set_temperature(t);
            }
        }
        Outcome::Developed {
            generation,
            image,
            guide,
            white,
            seconds,
            detail,
            sharpen,
            dehaze,
            sources,
            learned,
            fills,
        } => {
            let mut st = state.borrow_mut();
            if generation != st.generation || st.cull.is_some() {
                return;
            }
            let detailed = match detail {
                Some((s, Some(secs))) => format!(
                    ", local contrast at {} and {} px in {secs:.2} s",
                    s.texture_radius, s.clarity_radius
                ),
                Some((s, None)) => format!(
                    ", local contrast at {} and {} px kept",
                    s.texture_radius, s.clarity_radius
                ),
                None => String::new(),
            };
            // Sources the engine chose go into the edit, and so the sidecar.
            if !sources.is_empty() {
                st.edit.retouch = st.edit.retouch.with_sources(&sources);
                schedule_save(&mut st, app.as_weak());
            }
            // The picture held from the last file gives way, fitted.
            if st.held.take().is_some() {
                st.zoom = 0.0;
            }
            // A new source, or a fit view, is centered on the next
            // frame, by the render, which knows the frame's size (the
            // crop's, not the source's); a zoomed view of the same
            // source keeps its place.
            if st.zoom == 0.0 || st.source_size != (image.width(), image.height()) {
                st.image_size = (0, 0);
            }
            let dehazed = match dehaze {
                Some(d) => format!(
                    ", dehazed {:+.0} to a mean transmission of {:.2}",
                    d.amount * 100.0,
                    d.transmission_mean
                ),
                None => String::new(),
            };
            let sharpened = match sharpen {
                Some(s) => {
                    app.set_sharpen_measured_radius(s.radius);
                    app.set_sharpen_measured_threshold(s.threshold);
                    format!(
                        ", sharpened at radius {:.2} over {:.0}% of it",
                        s.radius,
                        s.blend_mean * 100.0
                    )
                }
                None => String::new(),
            };
            let learned_note = match learned {
                LearnedReport::Off | LearnedReport::Kept => String::new(),
                LearnedReport::Ran {
                    version,
                    provider,
                    seconds,
                } => format!(", learned denoiser {version} on {provider} in {seconds:.1} s"),
                LearnedReport::Cached { seconds } => {
                    format!(", learned denoiser from the cache in {seconds:.1} s")
                }
                LearnedReport::Missing(model) => {
                    if st.fetch.is_none() && !st.fetching && !st.declined.contains(&model.id) {
                        offer_model(&mut st, app, model);
                    }
                    ", the engine's denoise until the model is fetched".to_string()
                }
                LearnedReport::Failed(message) => {
                    tracing::warn!("learned denoiser failed: {message}; the engine's denoise");
                    format!(", the engine's denoise: the learned one failed ({message})")
                }
            };
            // The fills: made, or left as they were and why. A missing
            // model is offered, as the denoiser's is.
            let mut fill_note = String::new();
            if !fills.made.is_empty() {
                fill_note.push_str(&format!(
                    ", {} filled by the model in {:.1} s",
                    listed(&fills.made),
                    fills.seconds
                ));
            }
            if !fills.missing.is_empty() {
                fill_note.push_str(&format!(
                    ", {} left as {} until the fill model is fetched",
                    listed(&fills.missing),
                    if fills.missing.len() == 1 {
                        "it was"
                    } else {
                        "they were"
                    }
                ));
                let model = &greycard_ai::FILL;
                if st.fetch.is_none() && !st.fetching && !st.declined.contains(&model.id) {
                    offer_model(&mut st, app, model);
                }
            }
            for (name, why) in &fills.failed {
                tracing::warn!("fill {name} failed: {why}");
                fill_note.push_str(&format!(
                    ", {name} left as it was: the fill model failed ({why})"
                ));
            }
            // A dropper in hand keeps its hint: the defringe's arms
            // itself by asking for a develop, and the instructions
            // must not be what that develop takes away.
            let picking = app.get_picking();
            if picking.is_empty() {
                app.set_status(
                    format!(
                        "{}x{}, developed in {seconds:.2} s{detailed}{dehazed}{sharpened}{learned_note}{fill_note}",
                        image.width(),
                        image.height()
                    )
                    .into(),
                );
            } else {
                app.set_status(picking_hint(picking.as_str()).into());
            }
            // The picture the viewport draws from the next frame on,
            // whether it takes a camera picture's place or stands
            // where the last frame's develop stood.
            st.shown_size = (image.width(), image.height());
            st.pending = Some(Landed {
                image,
                guide,
                white,
                generation,
            });
            remember_last_file(&st);
            app.set_busy(false);
            app.window().request_redraw();
            // `--turn` outside culling: the key, once there is a
            // developed picture to measure the frame by, from the
            // event loop. Cleared in the callback, so the capture
            // this develop would otherwise be caught by waits for
            // the turn's own develop.
            if let Some(quarters) = st.turn_at_start {
                let app_weak = app.as_weak();
                let state = state.clone();
                slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                    if let Some(app) = app_weak.upgrade() {
                        state.borrow_mut().turn_at_start = None;
                        app.invoke_frame_turned(quarters);
                    }
                });
            } else {
                // This develop is the turn's own (or there was never
                // a turn to wait for): a capture may go.
                st.awaiting_turn = false;
            }
            if let Some(path) = st.export_then_quit.take() {
                app.set_busy(true);
                // A batch run has no panel to read the warning off, so
                // a look the edit names and the directory has not got
                // is said out loud. The file is still written, without
                // it: an export is not worth failing over a look, but
                // it is worth a line saying what came out.
                if let greycard_edit::look::LutChoice::Named(name) = &st.edit.look_lut.lut
                    && st.edit.look_lut.look().is_none()
                {
                    tracing::warn!(
                        "look {name}: not in the look directory; {} is written without it",
                        path.display()
                    );
                }
                // The sheet's choices as remembered, the format from
                // the path's extension.
                let settings = batch_settings(&path, read_export_settings(app));
                WORKER.with(|w| {
                    if let Some(w) = &*w.borrow() {
                        w.send(Job::Export {
                            edit: st.edit.clone(),
                            path,
                            settings,
                            on_exists: read_on_exists(app),
                        });
                    }
                });
            }
        }
        Outcome::Filling { generation, name } => {
            // The outline on the viewport says where; this says why
            // the picture has not moved yet.
            if generation == state.borrow().generation {
                app.set_status(format!("developing... the fill model is at work on {name}").into());
            }
        }
        Outcome::Failed {
            generation,
            message,
        } => {
            let mut st = state.borrow_mut();
            // Recorded whether or not the develop is still the wanted one.
            tracing::error!("failed: {message}");
            if generation == st.generation {
                app.set_status(format!("failed: {message}").into());
                app.set_busy(false);
                // Nothing is coming: the old picture shows the panel's
                // look, and a camera picture standing in for a develop
                // that failed comes down with it.
                let stood_in = develop_landed(&mut st, app, generation);
                if st.held.take().is_some() || stood_in {
                    app.window().request_redraw();
                }
                // A snapshot, a screenshot or an export waits for a
                // picture that will never arrive; end the run instead
                // of hanging on it.
                if st.batch {
                    st.failed = true;
                    let _ = slint::quit_event_loop();
                }
            }
        }
        Outcome::Exported {
            path,
            seconds,
            note,
        } => {
            let said = match &note {
                Some(note) => format!(" ({note})"),
                None => String::new(),
            };
            app.set_status(format!("exported {} in {seconds:.2} s{said}", file_name(&path)).into());
            tracing::info!("exported {} in {seconds:.2} s", path.display());
            // A rename or a file written over is worth the terminal.
            if let Some(note) = &note {
                tracing::warn!("exported {}: {note}", path.display());
            }
            app.set_busy(false);
            if state.borrow().screenshot.is_none() && std::env::args().any(|a| a == "--export") {
                let _ = slint::quit_event_loop();
            }
        }
        Outcome::ExportSkipped { path } => {
            app.set_status(
                format!("{} is there already: nothing exported", file_name(&path)).into(),
            );
            tracing::warn!("skipped {}: it is there already", path.display());
            app.set_busy(false);
            if state.borrow().screenshot.is_none() && std::env::args().any(|a| a == "--export") {
                let _ = slint::quit_event_loop();
            }
        }
        Outcome::Mask {
            key,
            shape,
            raster,
            provider,
            seconds,
        } => {
            let mut st = state.borrow_mut();
            st.asked.remove(&key);
            let name = shape.name().to_lowercase();
            // A subject found is shown, so the find can be judged —
            // unless its shape was switched off while the model ran,
            // when showing a mask it is no longer part of would be a
            // surprise.
            let live = st
                .edit
                .adjustments
                .iter()
                .find(|a| a.id == key.0)
                .and_then(|a| a.mask.components.get(key.1))
                .is_some_and(|c| c.enabled);
            if live && matches!(shape, Shape::Subject {}) && provider.is_some() {
                app.set_show_mask(true);
            }
            st.learned.insert(key, (shape, raster));
            if let Some(p) = provider {
                app.set_status(format!("{name} found on {p} in {seconds:.2} s").into());
            }
            app.window().request_redraw();
        }
        Outcome::MaskFailed { key, message } => {
            let st = state.borrow();
            tracing::error!("mask: {message}");
            if st.asked.contains_key(&key) {
                app.set_status(format!("mask: {message}").into());
            }
        }
        Outcome::Fetching { name, done, total } => {
            let short = name.split(',').next().unwrap_or(name);
            app.set_status(
                format!(
                    "downloading {short}: {} of {} MB",
                    done / 1_000_000,
                    total / 1_000_000
                )
                .into(),
            );
        }
        Outcome::Fetched { model } => {
            let mut st = state.borrow_mut();
            st.fetching = false;
            st.asked.clear();
            let short = model.name.split(',').next().unwrap_or(model.name);
            app.set_status(format!("{short} is ready").into());
            // A denoiser the edit is waiting for, or the fill model
            // with a fill to make: develop again with it.
            let denoiser = st
                .edit
                .noise
                .learned
                .tier()
                .and_then(greycard_ai::denoiser)
                .is_some_and(|m| m.id == model.id);
            let fill = model.id == greycard_ai::FILL.id
                && st.edit.retouch.enabled
                && st
                    .edit
                    .retouch
                    .patches
                    .iter()
                    .any(|p| p.method == RetouchMethod::Fill);
            if (denoiser || fill) && st.cull.is_none() {
                st.generation += 1;
                app.set_status("developing...".into());
                app.set_busy(true);
                let turn = current_turn(&st);
                WORKER.with(|w| {
                    if let Some(w) = &*w.borrow() {
                        w.send(Job::Develop {
                            edit: st.edit.clone(),
                            generation: st.generation,
                            turn,
                        });
                    }
                });
            }
            app.window().request_redraw();
        }
        Outcome::FetchFailed { name, message } => {
            let mut st = state.borrow_mut();
            st.fetching = false;
            let short = name.split(',').next().unwrap_or(name);
            tracing::error!("{name} could not be fetched: {message}");
            app.set_status(format!("{short} could not be fetched: {message}").into());
        }
        Outcome::LensesFetching { done, total } => {
            app.set_status(
                format!(
                    "downloading the lens profiles: {} of {} KB",
                    done / 1000,
                    total / 1000
                )
                .into(),
            );
        }
        Outcome::LensesFetched => {
            let mut st = state.borrow_mut();
            st.fetching = false;
            app.set_status("the lens profiles are ready".into());
            // The worker looks the open file up again on its next
            // open; ask it now, so the panel says what it found. Not
            // in culling, whose leaving opens the file anyway.
            if let Some(i) = st.current.filter(|_| st.cull.is_none()) {
                st.generation += 1;
                app.set_busy(true);
                let (path, edit, generation) =
                    (st.files[i].clone(), st.edit.clone(), st.generation);
                let seed_blend = st.seed_blend.get(i).copied().unwrap_or(false);
                let turn = st.sidecars[i].turn;
                WORKER.with(|w| {
                    if let Some(w) = &*w.borrow() {
                        w.send(Job::Open {
                            path,
                            edit,
                            generation,
                            seed_blend,
                            turn,
                        });
                    }
                });
            }
        }
        Outcome::LensesFetchFailed { message } => {
            let mut st = state.borrow_mut();
            st.fetching = false;
            tracing::error!("the lens profiles could not be fetched: {message}");
            app.set_status(format!("the lens profiles could not be fetched: {message}").into());
        }
        Outcome::ExportFailed { message } => {
            app.set_status(format!("export failed: {message}").into());
            app.set_busy(false);
            tracing::error!("export failed: {message}");
            // `--export` would otherwise wait for an Exported that is
            // not coming.
            let mut st = state.borrow_mut();
            if st.batch {
                st.failed = true;
                let _ = slint::quit_event_loop();
            }
        }
    }
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, _worker: &Rc<Worker>) {
    // The sheet's typed size, read the way the export reads it.
    app.on_custom_edge(|text| export::parse_edge(&text).map_or(0, |n| n as i32));
    // Any choice on the sheet: is it still the preset?
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_export_sheet_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                show_edited(&app, &state.borrow().export_presets);
            }
        });
    }
    // A preset chosen fills the sheet; None leaves it as it is.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_export_preset_chosen(move |name| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            if let Some(p) = sheet::find(&st.export_presets, &name) {
                show_sheet(&app, &p.sheet);
                app.set_status(format!("export preset {}", p.name).into());
            }
            show_presets_picker(&app, &st.export_presets, Some(&name));
            keep_presets(&st, &app);
        });
    }
    // Save as: the sheet under a name, over one of that name.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_export_preset_saved(move |name| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let name = name.trim();
            if name.is_empty() || sheet::reserved(name) {
                app.set_status(format!("{NO_PRESET} is the picker's; choose another name").into());
                return;
            }
            let mut st = state.borrow_mut();
            let current = read_sheet(&app);
            if sheet::save_as(&mut st.export_presets, name, &current) {
                show_presets_picker(&app, &st.export_presets, Some(name));
                keep_presets(&st, &app);
                app.set_status(format!("export preset {name} saved").into());
            }
        });
    }
    // Whether Save as would replace one: the button says so.
    {
        let state = state.clone();
        app.on_export_preset_exists(move |name| {
            sheet::exists(&state.borrow().export_presets, &name)
        });
    }
    // Delete the preset chosen; the sheet keeps its choices.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_export_preset_deleted(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(name) = chosen_preset(&app) else {
                return;
            };
            let mut st = state.borrow_mut();
            if sheet::delete(&mut st.export_presets, &name) {
                show_presets_picker(&app, &st.export_presets, None);
                keep_presets(&st, &app);
                app.set_status(format!("export preset {name} deleted").into());
            }
        });
    }
    // The watermark's PNG, from the desktop's chooser.
    {
        let app_weak = app.as_weak();
        app.on_export_mark_choose(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let current = app.get_export_mark_image().to_string();
            let start = Path::new(&current)
                .parent()
                .filter(|p| p.is_dir())
                .map(Path::to_path_buf)
                .or_else(dirs::picture_dir)
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from("/"));
            let weak = app.as_weak();
            export::choose_open(
                "Watermark",
                start,
                ("PNG".into(), vec!["*.png".into(), "*.PNG".into()]),
                move |chosen| {
                    let _ = weak.upgrade_in_event_loop(move |app| match chosen {
                        Ok(Some(path)) => {
                            app.set_export_mark_image(path.to_string_lossy().as_ref().into());
                            app.set_export_mark_image_name(file_name(&path).into());
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!("file chooser: {e:#}");
                            app.set_status("the desktop offered no file chooser".into());
                        }
                    });
                },
            );
        });
    }
    // Export the current file under the panel's edit, beside it.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_export(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (raw, edit) = {
                let st = state.borrow();
                let Some(c) = st.current else {
                    return;
                };
                (st.files[c].clone(), read_edit(&app, &st.edit, st.target))
            };
            let settings = read_export_settings(&app);
            // A mark asked for with nothing to draw: say so and keep
            // the sheet up, rather than write the picture unmarked.
            if let Some(Err(e)) = settings.watermark.as_ref().map(|m| m.check()) {
                app.set_status(format!("not exported: {e:#}").into());
                app.set_export_open(true);
                return;
            }
            let on_exists = read_on_exists(&app);
            let suggested = raw.with_extension(settings.format.extension());
            let weak = app.as_weak();
            app.set_status("choosing where to export...".into());
            export::choose_path(suggested, settings.format, move |chosen| {
                let _ = weak.upgrade_in_event_loop(move |app| {
                    // A path the user picked in the desktop's chooser
                    // is a path the chooser asked them to confirm; the
                    // sheet's policy is for the paths the editor
                    // decides by itself.
                    let (path, on_exists) = match chosen {
                        Ok(Some(path)) => (path, export::OnExists::Overwrite),
                        Ok(None) => {
                            app.set_status("export canceled".into());
                            return;
                        }
                        Err(e) => {
                            // No chooser to ask: beside the file, under a
                            // name no camera writes.
                            tracing::warn!("file chooser: {e:#}; exporting beside the file");
                            (export_path(&raw, settings.format), on_exists)
                        }
                    };
                    app.set_status(format!("exporting {}...", file_name(&path)).into());
                    app.set_busy(true);
                    WORKER.with(|w| {
                        if let Some(w) = &*w.borrow() {
                            w.send(Job::Export {
                                edit,
                                path,
                                settings,
                                on_exists,
                            });
                        }
                    });
                });
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sheet_picks_saves_marks_and_deletes_presets() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        // A batch state: nothing here may write the user's settings.
        state.borrow_mut().batch = true;
        let web = Sheet {
            size: "2048".into(),
            quality: 85.0,
            mark: sheet::MARK_TEXT.into(),
            mark_text: "© me".into(),
            ..Sheet::default()
        };
        state.borrow_mut().export_presets = vec![ExportPreset {
            name: "Web".into(),
            sheet: web.clone(),
        }];
        show_presets_picker(&app, &state.borrow().export_presets, None);
        assert_eq!(app.get_export_preset(), NO_PRESET);
        assert_eq!(app.get_export_presets().row_count(), 2);
        assert!(!app.get_export_preset_edited());

        // Chosen: the sheet is the preset's, and not edited.
        app.invoke_export_preset_chosen("Web".into());
        assert_eq!(app.get_export_preset(), "Web");
        assert!(read_sheet(&app).same(&web));
        assert_eq!(read_export_settings(&app).long_edge, Some(2048));
        assert!(!app.get_export_preset_edited());
        // A field moved: edited; moved back: not.
        app.set_export_mark_position("Top left".into());
        app.invoke_export_sheet_changed();
        assert!(app.get_export_preset_edited());
        app.set_export_mark_position(web.mark_position.as_str().into());
        app.invoke_export_sheet_changed();
        assert!(!app.get_export_preset_edited());

        // Saved as a new name: a second preset, chosen, not edited.
        app.set_export_quality(70.0);
        app.invoke_export_sheet_changed();
        assert!(app.get_export_preset_edited());
        app.invoke_export_preset_saved(" Small ".into());
        assert_eq!(app.get_export_preset(), "Small");
        assert!(!app.get_export_preset_edited());
        assert_eq!(state.borrow().export_presets.len(), 2);
        assert_eq!(state.borrow().export_presets[1].sheet.quality, 70.0);
        // "None" is the picker's, not a name.
        app.invoke_export_preset_saved(NO_PRESET.into());
        assert_eq!(state.borrow().export_presets.len(), 2);

        // Deleted: gone, nothing chosen, the sheet as it was.
        app.invoke_export_preset_deleted();
        assert_eq!(state.borrow().export_presets.len(), 1);
        assert_eq!(app.get_export_preset(), NO_PRESET);
        assert_eq!(app.get_export_quality(), 70.0);
        // Nothing was written: the state has no settings file.
        assert!(state.borrow().settings_file.is_none());
    }

    #[test]
    fn a_pick_a_save_and_a_delete_are_written_at_once() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        let dir = std::env::temp_dir().join(format!("greycard-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("greycard").join("settings.json");
        // What else the file holds is kept.
        settings::Settings {
            scope: "Parade".into(),
            ..settings::Settings::default()
        }
        .save_to(&file);
        state.borrow_mut().settings_file = Some(file.clone());
        let read = || settings::Settings::load_from(&file);

        app.set_export_size("1024".into());
        app.invoke_export_preset_saved("Small".into());
        let on_disk = read();
        assert_eq!(on_disk.export_presets.len(), 1);
        assert_eq!(on_disk.export_presets[0].name, "Small");
        assert_eq!(on_disk.export_presets[0].sheet.size, "1024");
        assert_eq!(on_disk.export_preset, "Small");
        assert_eq!(on_disk.scope, "Parade");

        // Another saved, then the first picked back: the pick is kept.
        app.set_export_size("2048".into());
        app.invoke_export_preset_saved("Web".into());
        assert_eq!(read().export_presets.len(), 2);
        app.invoke_export_preset_chosen("Small".into());
        let on_disk = read();
        assert_eq!(on_disk.export_preset, "Small");
        assert_eq!(on_disk.export.size, "1024");

        // Deleted: gone from the file, nothing chosen.
        app.invoke_export_preset_deleted();
        let on_disk = read();
        assert_eq!(on_disk.export_presets.len(), 1);
        assert_eq!(on_disk.export_presets[0].name, "Web");
        assert_eq!(on_disk.export_preset, "");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_empty_mark_keeps_the_sheet_up_and_says_why() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        state.borrow_mut().current = Some(0);
        app.set_export_mark(sheet::MARK_IMAGE.into());
        app.set_export_mark_image("".into());
        app.set_export_open(false);
        app.invoke_export();
        assert!(app.get_export_open());
        assert!(app.get_status().contains("no PNG"), "{}", app.get_status());
        app.set_export_mark(sheet::MARK_TEXT.into());
        app.set_export_mark_text("  ".into());
        app.set_export_open(false);
        app.invoke_export();
        assert!(app.get_export_open());
        assert!(
            app.get_status().contains("text is empty"),
            "{}",
            app.get_status()
        );
    }
}
