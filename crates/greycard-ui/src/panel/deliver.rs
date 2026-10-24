use crate::panel::assets::{offer_lenses_once, offer_model, show_lens, show_looks, show_profiles};
use crate::panel::browser::{
    chosen_frames, count_thumb, end_thumb_turn, file_name, release_thumbnails, show_no_thumb,
    show_thumb, thumb_turns,
};
use crate::panel::cull::develop_landed;
use crate::panel::edit::{current_turn, read_edit, take_sources};
use crate::panel::history::show_history;
use crate::panel::startup::remember_last_file;
use crate::panel::viewport::picking_hint;
use crate::queue;
use crate::settings;
use crate::sheet::{self, ExportPreset, Sheet};
use crate::worker::DetailRan;
use crate::*;
use std::time::{Duration, Instant};

/// A develop the worker sent, waiting for the frame that puts it on
/// the GPU.
///
/// It carries the generation it was asked for as well as the picture:
/// by the time the frame runs, the selection may have moved on again,
/// and a camera picture standing in for the newer frame's develop
/// must not be taken down by this one.
pub(crate) struct Landed {
    pub(crate) image: Developed,
    /// What the picture is, which goes up with it as the drawn
    /// picture's kind (`Develop::drawn_kind`).
    pub(crate) kind: finish::Source,
    /// The tone equalizer's guide plane for it.
    pub(crate) guide: Arc<finish::Guide>,
    /// The masks' detail planes for it, when a mask asks for them.
    pub(crate) planes: Option<Arc<crate::detail::DetailPlanes>>,
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
        subfolder: app.get_export_subfolder().into(),
        metadata: app.get_export_metadata().into(),
        mark: app.get_export_mark().into(),
        mark_text: app.get_export_mark_text().into(),
        mark_image: app.get_export_mark_image().into(),
        mark_color: app.get_export_mark_color().into(),
        mark_position: app.get_export_mark_position().into(),
        mark_size: app.get_export_mark_size(),
        mark_margin: app.get_export_mark_margin(),
        mark_opacity: app.get_export_mark_opacity(),
        border: app.get_export_border(),
        border_width: app.get_export_border_width(),
        border_color: app.get_export_border_color().into(),
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
    app.set_export_subfolder(s.subfolder.as_str().into());
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
    app.set_export_border(s.border);
    app.set_export_border_width(s.border_width);
    app.set_export_border_color(s.border_color.as_str().into());
}

/// The picker's word for no preset. A preset cannot take it as a
/// name, in any case.
pub(crate) const NO_PRESET: &str = sheet::NO_PRESET;

/// The preset the panel has chosen, none for `NO_PRESET`.
pub(crate) fn chosen_preset(app: &App) -> Option<String> {
    let name = app.get_export_preset();
    (!name.is_empty() && !sheet::reserved(&name)).then(|| name.to_string())
}

/// The export preset an export is written under, for the frame's
/// history: the one chosen, when the sheet is still that preset as
/// saved. An edited sheet is not the preset, and says nothing.
pub(crate) fn preset_in_use(app: &App) -> Option<String> {
    chosen_preset(app).filter(|_| !app.get_export_preset_edited())
}

/// The picker's entries and the one chosen, and whether the sheet has
/// moved from it: the presets as `presets` has them.
pub(crate) fn show_presets_picker(app: &App, presets: &[ExportPreset], chosen: Option<&str>) {
    // A preset an older file named "none" in some case is never listed:
    // choosing it would read as the picker's None, and no lookup finds
    // it, so it could not be deleted either.
    let names: Vec<slint::SharedString> = std::iter::once(NO_PRESET.into())
        .chain(
            presets
                .iter()
                .filter(|p| !sheet::reserved(&p.name))
                .map(|p| p.name.as_str().into()),
        )
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
#[cfg(test)]
pub(crate) fn read_export_settings(app: &App) -> export::Settings {
    read_sheet(app).settings()
}

/// Keep the presets in the settings file now, not at the window's
/// close: a preset is worth keeping even from a session that ends
/// badly. Not from a snapshot or a batch run, which have no file to
/// write (`Window::settings_file`).
fn keep_presets(st: &State, app: &App) {
    let Some(path) = &st.window.settings_file else {
        return;
    };
    let mut settings = settings::Settings::load_from(path);
    settings.export_presets = st.work.export_presets.clone();
    settings.export_preset = chosen_preset(app).unwrap_or_default();
    settings.export = read_sheet(app);
    settings.save_to(path);
}

/// The folder an export of `raw` writes to when the sheet names no
/// subfolder: the one the sheet's Folder row shows and its chooser
/// opens on. The folder last chosen, else the folder of `raw`. The
/// disk is not asked here, on the window's thread: whether the folder
/// still takes files is looked at where it is about to be written to
/// (`check_folder`), and a chooser opened on one gone since opens on
/// its nearest parent (`export::ask`).
pub(crate) fn sheet_folder(st: &State, raw: &Path) -> PathBuf {
    st.work
        .export_last_folder
        .clone()
        .or_else(|| raw.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

/// A chooser answered with `folder`: exports go there and the next
/// chooser opens there, in this run and, written at once as a preset
/// is, in the next; the sheet's Folder row reads it again. Not
/// written from a snapshot or a batch run (`Window::settings_file`).
pub(crate) fn remember_export_last_folder(st: &mut State, app: &App, folder: &Path) {
    st.work.export_last_folder = Some(folder.to_path_buf());
    app.set_export_folder_turn(app.get_export_folder_turn().wrapping_add(1));
    let Some(path) = &st.window.settings_file else {
        return;
    };
    let mut settings = settings::Settings::load_from(path);
    settings.export_last_folder = folder.to_string_lossy().into_owned();
    settings.save_to(path);
}

/// One picture from the thumbnails' pool, or word that none could be
/// made, put on its file's row.
pub(crate) fn take_thumbnail(st: &mut State, app: &App, outcome: Outcome) {
    match outcome {
        Outcome::Thumbnail {
            index,
            frame,
            size,
            width,
            height,
            rgb,
            cached,
            seconds,
        } => {
            // A thumbnail of a list since replaced by another folder's,
            // or renumbered under it, would land on a stranger's cell:
            // only its own frame's.
            if st.key_of(index) != Some(frame.clone()) {
                return;
            }
            tracing::debug!(
                "thumbnail {}: {} in {:.1} ms",
                frame.said(),
                if cached { "from the cache" } else { "made" },
                seconds * 1000.0
            );
            count_thumb(st, index, Some(cached), seconds);
            st.views.thumb_base[index] = Some((width, height, rgb));
            st.views.thumb_made[index] = size;
            st.views.thumb_failed[index] = false;
            // One that was being made when the cells grew comes back
            // at the old size. Nothing else would ask for it again
            // until the grid next moved, and a snapshot waiting on
            // the grid would wait for ever, so ask here.
            if grid::wants_bigger(size, st.views.thumb_want)
                && st
                    .views
                    .grid_shown
                    .is_some_and(|(f, l)| (f..=l).contains(&(index as i32)))
            {
                st.views.thumb_asked[index] = st.views.thumb_want;
                let job = crate::rows::thumb_job(st, index);
                WORKER.with(|w| {
                    if let Some(w) = &*w.borrow() {
                        w.send(job);
                    }
                });
            }
            // Shown as its edit turns it: the open file's as the panel
            // has it, another's as its sidecar does.
            let (turns, flip) = thumb_turns(st, app, index);
            show_thumb(st, app, index, turns, flip);
        }
        Outcome::NoThumbnail { index, frame } if st.key_of(index).as_ref() == Some(&frame) => {
            count_thumb(st, index, None, 0.0);
            show_no_thumb(st, app, index);
        }
        _ => {}
    }
}

/// The least time a turn of the window's event loop takes pictures
/// in for, before it lets the window draw and hear its input: the
/// first turn's, which serves the files on screen.
pub(crate) const THUMB_TURN: Duration = Duration::from_millis(8);

/// The most: how long input can wait on the pictures, on top of the
/// frame it was already waiting on.
pub(crate) const THUMB_TURN_MOST: Duration = Duration::from_millis(100);

/// How many pictures are taken from the outbox at a time, between
/// looks at the clock.
const THUMBS_AT_ONCE: usize = 32;

/// How long a turn takes pictures in for, when the window spent
/// `between` on the frame and the input since the last: as long
/// again, within [`THUMB_TURN`] and [`THUMB_TURN_MOST`], so half the
/// window's time goes to the pictures however dear a frame is. When
/// the strip and the grid had a cell for every row, a frame over a
/// folder of twenty thousand cost 45 to 200 ms, and a fixed 8 ms, with
/// a frame after each, took a warm folder of that size 12 to 28 s to
/// fill. They have cells for the screen alone now (`cells.rs`), and a
/// frame is cheap, but a dear one still gets its half.
pub(crate) fn thumb_turn(between: Option<Duration>) -> Duration {
    between.map_or(THUMB_TURN, |b| b.clamp(THUMB_TURN, THUMB_TURN_MOST))
}

/// The pictures waiting in the pool's outbox, taken in for `budget`,
/// the files on screen first. What is left is taken on a timer rather
/// than a call on the event loop: the loop takes in every call queued
/// before it draws, and a timer's turn comes after the frame and the
/// input.
pub(crate) fn take_thumbnails(
    app: &App,
    state: &Rc<RefCell<State>>,
    batch: crate::thumbpool::Batch,
    budget: Duration,
) {
    let started = Instant::now();
    let left = {
        let mut st = state.borrow_mut();
        let left = loop {
            let (outcomes, left) = batch.take(THUMBS_AT_ONCE);
            for outcome in outcomes {
                take_thumbnail(&mut st, app, outcome);
            }
            if !left || started.elapsed() >= budget {
                break left;
            }
        };
        end_thumb_turn(&mut st, started);
        left
    };
    if left {
        let (app_weak, ended) = (app.as_weak(), Instant::now());
        slint::Timer::single_shot(Duration::ZERO, move || {
            let (Some(app), Some(state)) = (app_weak.upgrade(), STATE.with(|s| s.borrow().clone()))
            else {
                return;
            };
            take_thumbnails(&app, &state, batch, thumb_turn(Some(ended.elapsed())));
        });
    }
}

/// A result from the worker, on the UI thread.
pub(crate) fn deliver(app: &App, outcome: Outcome) {
    let Some(state) = STATE.with(|s| s.borrow().clone()) else {
        return;
    };
    match outcome {
        outcome @ (Outcome::Thumbnail { .. } | Outcome::NoThumbnail { .. }) => {
            let started = Instant::now();
            let mut st = state.borrow_mut();
            take_thumbnail(&mut st, app, outcome);
            end_thumb_turn(&mut st, started);
        }
        Outcome::Thumbnails(batch) => take_thumbnails(app, &state, batch, thumb_turn(None)),
        Outcome::Opened {
            generation,
            as_shot,
            frame,
            profile,
            lens,
            kind,
            shot,
            taken,
            blend,
        } => {
            let mut st = state.borrow_mut();
            if generation != st.develop.generation {
                return;
            }
            // A fresh raw's learned-denoiser blend from its ISO: into
            // the open cell's line (and that line alone: a version at
            // its own strength keeps it, one still at the default waits
            // for its own open), the edit under develop and the panel,
            // once.
            // A frame of the export queue's view takes it on its queued
            // edit alone, its sidecar left as it is (`State::line_mut`
            // refuses it there).
            let frozen = crate::panel::queue_view::frozen(&st);
            if let Some(b) = blend
                && let Some(i) = st.current
                && (frozen || crate::rows::seed_landed(&mut st, i, b))
            {
                // The overrides' edit is the panel's, seed and all.
                let file = st.path_at(i).cloned();
                if let Some((f, e)) = st.launch.overridden.as_mut()
                    && file.as_ref() == Some(f)
                {
                    e.noise.learned_strength = b;
                }
                st.develop.edit.noise.learned_strength = b;
                app.set_denoise_learned_strength(b);
            }
            st.develop.frame = frame.zip(profile);
            // The camera the file names, and what the profile
            // directory holds for it: read on open, not on develop.
            st.develop.camera = st
                .develop
                .frame
                .as_ref()
                .map(|(f, _)| (f.make.clone(), f.model.clone()))
                .unwrap_or_default();
            st.develop.profiles = greycard_edit::camera::list();
            st.develop.looks = greycard_edit::look::list();
            // Read on each open, so a look or a profile changed by hand
            // since is caught here too.
            crate::edited::looks_moved(&mut st);
            // Another file, so the embedded profile behind the
            // white balance is another one too.
            st.develop.white_profile = None;
            show_profiles(&st, &st.develop.edit.camera.profile, app);
            // After the directory is read, not before: `show_edit`
            // ran earlier in this arm, when there was nothing to list.
            show_looks(&st, &st.develop.edit.look_lut, app);
            app.set_shot_camera(shot.camera.as_str().into());
            app.set_shot_exposure(shot.exposure.as_str().into());
            // The camera's time from the decode, for a frame the index
            // does not hold (a folder outside the roots).
            if let (Some(taken), Some(c)) = (taken, st.current) {
                let file = st.file_of(c);
                crate::panel::time_shift::seed(&mut st, file, taken);
            }
            crate::panel::time_shift::show_shot_time(&mut st, app);
            match &kind {
                SourceKind::Raw => {
                    app.set_raw_input(true);
                    app.set_source_note("".into());
                    // The learned denoiser has no X-Trans version yet:
                    // the section says so and its tiers rest, while the
                    // profiled denoise works as on any raw.
                    let xtrans = st.develop.frame.as_ref().is_some_and(|(f, _)| {
                        matches!(&f.layout, greycard_core::raw::SensorLayout::Cfa(p) if p.is_xtrans())
                    });
                    app.set_noise_unavailable("".into());
                    app.set_learned_unavailable(
                        if xtrans {
                            "learned: not on X-Trans yet"
                        } else {
                            ""
                        }
                        .into(),
                    );
                    // The raw's mosaic, now known from its pixels: the
                    // menu, if the worker's read ahead had not said.
                    let mosaic = match st.develop.frame.as_ref().map(|(f, _)| &f.layout) {
                        Some(greycard_core::raw::SensorLayout::Cfa(p)) => {
                            crate::panel::browser::OpenMosaic::Known(p.mosaic())
                        }
                        Some(_) => crate::panel::browser::OpenMosaic::NotMosaic,
                        None => crate::panel::browser::OpenMosaic::Unknown,
                    };
                    if let Some(path) = st.current.and_then(|c| st.path_at(c)).cloned() {
                        crate::panel::browser::learn_mosaic(&mut st, app, &path, mosaic);
                    }
                }
                SourceKind::Picture { space, bits } => {
                    app.set_raw_input(false);
                    app.set_noise_unavailable("raw only".into());
                    app.set_learned_unavailable("".into());
                    app.set_source_note(
                        format!(
                            "{}-bit {} read as {space}; already rendered",
                            bits,
                            st.current
                                .and_then(|i| st.path_at(i))
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
            st.develop.lens = lens;
            st.develop.white_cache = None;
            // Show the frame's own white balance on the sliders, so a
            // custom setting starts from it; and keep it, for As shot
            // switched back on.
            st.develop.as_shot_white =
                matches!(kind, SourceKind::Raw).then_some((as_shot.0 as f32, as_shot.1 as f32));
            if st.develop.edit.white_balance == WhiteBalance::AsShot {
                app.set_temperature(as_shot.0 as f32);
                app.set_tint(as_shot.1 as f32);
            }
            if let Some(t) = st.launch.preview_temperature.take() {
                app.set_as_shot(false);
                app.set_temperature(t);
            }
        }
        Outcome::Developed {
            generation,
            turn,
            image,
            kind,
            guide,
            planes,
            white,
            seconds,
            detail,
            sharpen,
            dehaze,
            sources,
            learned,
            fills,
        } => {
            release_thumbnails();
            let mut st = state.borrow_mut();
            if generation != st.develop.generation {
                return;
            }
            if st.culling.cull.is_some() {
                // Culling takes the develop and not its planes: a mask's
                // Detail asks again when the panel is back.
                st.develop.planes_asked = Default::default();
                return;
            }
            // The saved edit's pictures, from this develop.
            crate::edited::landed(&st, turn);
            let detailed = match detail {
                Some((s, ran)) => {
                    let how = match ran {
                        DetailRan::Cpu(secs) => format!("in {secs:.2} s"),
                        DetailRan::Gpu => "on the GPU".into(),
                        DetailRan::Kept => "kept".into(),
                    };
                    format!(
                        ", local contrast at {} and {} px {how}",
                        s.texture_radius, s.clarity_radius
                    )
                }
                None => String::new(),
            };
            // Sources the engine chose go into the edit, and into the
            // sidecar's states in place: they finish a step, they are
            // not one.
            if take_sources(&mut st, &sources) {
                show_history(&st, app);
            }
            // The picture held from the last file gives way, fitted.
            if st.develop.held.take().is_some() {
                st.develop.zoom = 0.0;
            }
            // A new source, or a fit view, is centered on the next
            // frame, by the render, which knows the frame's size (the
            // crop's, not the source's); a zoomed view of the same
            // source keeps its place.
            if st.develop.zoom == 0.0 || st.develop.source_size != (image.width(), image.height()) {
                st.develop.image_size = (0, 0);
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
            // The learned denoiser or a fill failed where the AI
            // features are off until the CPU-only runtime is fetched
            // (their models are not even asked for there): the runtime
            // is offered, then the model if it is still to fetch, and
            // the develop runs again once both are had.
            let denoise_failed = matches!(learned, LearnedReport::Failed(_));
            if (denoise_failed || !fills.failed.is_empty())
                && greycard_ai::runtime::fetchable().is_some()
            {
                use crate::panel::assets::AfterRuntime;
                let have =
                    |m: &greycard_ai::Model| st.develop.store.as_ref().is_some_and(|s| s.have(m));
                let tier = st
                    .develop
                    .edit
                    .noise
                    .learned
                    .tier()
                    .and_then(greycard_ai::denoiser);
                let next = match tier {
                    Some(model) if denoise_failed && !have(model) => {
                        AfterRuntime::Model(model, false)
                    }
                    _ if !fills.failed.is_empty() && !have(&greycard_ai::FILL) => {
                        AfterRuntime::Model(&greycard_ai::FILL, false)
                    }
                    _ => AfterRuntime::Nothing,
                };
                crate::panel::assets::runtime_first(&mut st, app, next);
            }
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
                    if st.sheets.fetch.is_none()
                        && !st.sheets.fetching
                        && !st.sheets.declined.contains(&model.id)
                    {
                        offer_model(&mut st, app, model, false);
                    }
                    ", the engine's denoise until the model is fetched".to_string()
                }
                LearnedReport::Failed(message) => {
                    tracing::warn!("learned denoiser failed: {message}; the engine's denoise");
                    format!(", the engine's denoise: the learned one failed ({message})")
                }
                LearnedReport::Unavailable(why) => format!(", no denoise: {why}"),
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
                let names: Vec<String> = fills.missing.iter().map(|(n, _)| n.clone()).collect();
                fill_note.push_str(&format!(
                    ", {} left as {} until the fill model is fetched",
                    listed(&names),
                    if names.len() == 1 {
                        "it was"
                    } else {
                        "they were"
                    }
                ));
                // The first model waited on that can be had here: the
                // generative remove not on a machine without WebGPU.
                let model = fills
                    .missing
                    .iter()
                    .map(|(_, m)| *m)
                    .find(|m| m.id != greycard_ai::REMOVE.id || crate::ai::on_webgpu());
                if let Some(model) = model
                    && st.sheets.fetch.is_none()
                    && !st.sheets.fetching
                    && !st.sheets.declined.contains(&model.id)
                {
                    offer_model(&mut st, app, model, false);
                }
            }
            // Which engine made each Fill, for its row's "Another".
            if crate::panel::retouch::took_engines(&mut st, &fills.engines) {
                crate::panel::retouch::show_patches(&st, &st.develop.edit, app);
            }
            for name in &fills.no_gpu {
                fill_note.push_str(&format!(", {name} left as it was: the remove needs a GPU"));
            }
            for (name, why) in &fills.failed {
                tracing::warn!("fill {name} failed: {why}");
                fill_note.push_str(&format!(
                    ", {name} left as it was: the fill model failed ({why})"
                ));
            }
            // A preset or a sync's own words ("Muted Slide onto 3
            // frames"), left here for the develop this generation is,
            // so the develop's line does not stand alone as the only
            // word on what changed. `take_if` leaves a stash whose
            // generation does not match in place rather than clearing
            // it, but that never misfires: generations only rise, so
            // a stash an earlier develop's landing did not want is
            // for a generation this one will never equal either, and
            // `take_current` clears it outright once a newer develop
            // is asked for, before it can sit and wait for one.
            let prefix = st
                .develop
                .status_after_develop
                .take_if(|(g, _)| *g == generation)
                .map(|(_, s)| format!("{s}; "))
                .unwrap_or_default();
            // A dropper in hand keeps its hint: the defringe's arms
            // itself by asking for a develop, and the instructions
            // must not be what that develop takes away.
            let picking = app.get_picking();
            if picking.is_empty() {
                // With no GPU, why the time is what it is: the start's
                // own line saying so is gone by now.
                let on_cpu = st.develop.software_tail.unwrap_or_default();
                // The set's own words first: the develop's line runs
                // long with a note or two of its own, and the set's
                // is the one a snapshot's width must not clip.
                app.set_status(
                    format!(
                        "{prefix}{}x{}, developed in {seconds:.2} s{on_cpu}{detailed}{dehazed}{sharpened}{learned_note}{fill_note}",
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
            st.develop.shown_size = (image.width(), image.height());
            st.develop.shown_turn = st.current.map(|c| (c, turn % 4));
            st.develop.planes_wants = planes.as_ref().map(|p| p.wants).unwrap_or_default();
            st.develop.planes_asked = Default::default();
            st.develop.pending = Some(Landed {
                image,
                kind,
                guide,
                planes,
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
            if let Some(quarters) = st.launch.turn_at_start {
                let app_weak = app.as_weak();
                let state = state.clone();
                slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                    if let Some(app) = app_weak.upgrade() {
                        state.borrow_mut().launch.turn_at_start = None;
                        app.invoke_frame_turned(quarters);
                    }
                });
            } else {
                // This develop is the turn's own (or there was never
                // a turn to wait for): a capture may go.
                st.launch.awaiting_turn = false;
            }
        }
        Outcome::Mosaic { path, mosaic } => {
            use crate::panel::browser::OpenMosaic;
            let known = mosaic.map_or(OpenMosaic::NotMosaic, OpenMosaic::Known);
            crate::panel::browser::learn_mosaic(&mut state.borrow_mut(), app, &path, known);
        }
        Outcome::Filling { generation, name } => {
            // The outline on the viewport says where; this says why
            // the picture has not moved yet.
            if generation == state.borrow().develop.generation {
                app.set_status(format!("developing... the fill model is at work on {name}").into());
            }
        }
        Outcome::Failed {
            generation,
            message,
        } => {
            release_thumbnails();
            let mut st = state.borrow_mut();
            // Recorded whether or not the develop is still the wanted one.
            tracing::error!("failed: {message}");
            if generation == st.develop.generation {
                app.set_status(format!("failed: {message}").into());
                app.set_busy(false);
                // An open that failed before anything said what the raw
                // is: the demosaic menu stops waiting.
                crate::panel::browser::mosaic_unreadable(&mut st, app);
                // The planes it was asked for are not coming: a mask's
                // Detail moved again asks for them again.
                st.develop.planes_asked = Default::default();
                // Nothing is coming: the old picture shows the panel's
                // look, and a camera picture standing in for a develop
                // that failed comes down with it.
                let stood_in = develop_landed(&mut st, app, generation);
                if st.develop.held.take().is_some() || stood_in {
                    app.window().request_redraw();
                }
                // A snapshot, a screenshot or an export waits for a
                // picture that will never arrive; end the run instead
                // of hanging on it.
                if st.window.batch {
                    st.launch.failed = true;
                    let _ = slint::quit_event_loop();
                }
            }
        }
        Outcome::Mask {
            key,
            shape,
            file,
            raster,
            provider,
            seconds,
            note,
            part,
        } => {
            let mut st = state.borrow_mut();
            // A Whole person run with Subject's edge: the worker has
            // Subject loaded.
            if crate::ai::takes_subject_edge(&shape) && provider.is_some() && note.is_none() {
                st.sheets.edge_loaded = true;
            }
            // Made on a frame opened before this one: that frame's. The
            // key and the shape may well be this frame's too (a mask
            // pasted across), and its own is asked for already.
            if file.is_some() && st.develop.learned_of.is_some() && file != st.develop.learned_of {
                tracing::debug!(
                    "mask: a {} raster of {} is not the open frame's",
                    shape.name(),
                    file.as_deref().map(file_name).unwrap_or_default()
                );
                return;
            }
            st.develop.asked.remove(&key);
            st.develop.masks_failed.remove(&key);
            // A Part asking which person it is of here: the picking,
            // with the people outlined; settled, it asks no more. An
            // ask made on a picture no longer open is that picture's.
            let open = st.current.and_then(|c| st.path_at(c)).cloned();
            // A part made, though with a word on the status line (a
            // Whole person that kept SAM's edge): shown as any other.
            let settled = matches!(shape, Shape::Part { .. }) && part.is_none();
            match part {
                Some(crate::ai::Unresolved::Ask(ask)) if file.is_none() || file == open => {
                    crate::panel::parts::asked(&mut st, app, key, ask)
                }
                Some(crate::ai::Unresolved::Ask(_)) => {}
                _ => {
                    st.develop.part_asks.remove(&key);
                }
            }
            let name = shape.name().to_lowercase();
            // A subject found is shown, so the find can be judged —
            // unless its shape was switched off while the model ran,
            // when showing a mask it is no longer part of would be a
            // surprise.
            let live = st
                .develop
                .edit
                .adjustments
                .iter()
                .find(|a| a.id == key.0)
                .and_then(|a| a.mask.components.get(key.1))
                .is_some_and(|c| c.enabled);
            if live
                && matches!(
                    shape,
                    Shape::Subject {}
                        | Shape::Background {}
                        | Shape::Sky { .. }
                        | Shape::Landscape { .. }
                        | Shape::Part { .. }
                )
                && provider.is_some()
                && (note.is_none() || settled)
            {
                app.set_show_mask(true);
            }
            st.develop.learned.insert(key, (shape, raster));
            // A sky the gate did not find is said every time, cached or
            // not: the empty mask is the answer, not a failure.
            if let Some(note) = note {
                app.set_status(note.into());
            } else if let Some(p) = provider {
                app.set_status(format!("{name} found on {p} in {seconds:.2} s").into());
            }
            app.window().request_redraw();
        }
        Outcome::Landscape { file, offers } => {
            crate::panel::landscape::found(&mut state.borrow_mut(), app, file, offers);
        }
        Outcome::People { file, people } => {
            let changed =
                crate::panel::parts::people_found(&mut state.borrow_mut(), app, file, people);
            if changed {
                app.invoke_view_changed();
            }
        }
        Outcome::PersonOn {
            batch,
            frame,
            found,
        } => {
            let landed =
                crate::panel::parts_all::landed(&mut state.borrow_mut(), app, batch, frame, found);
            if landed == crate::panel::parts_all::Landed::Open {
                app.invoke_view_changed();
            }
        }
        Outcome::MaskFailed { key, message } => {
            let mut st = state.borrow_mut();
            tracing::error!("mask: {message}");
            if let Some(shape) = st.develop.asked.get(&key).cloned() {
                app.set_status(format!("mask: {message}").into());
                let file = st.develop.learned_of.clone();
                st.develop.masks_failed.insert(key, (shape, file));
                // A capture waiting on this shape is waiting no more.
                app.window().request_redraw();
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
            st.sheets.fetching = false;
            st.develop.asked.clear();
            st.develop.masks_failed.clear();
            st.sheets.fetch_failed.retain(|id| *id != model.id);
            let short = model.name.split(',').next().unwrap_or(model.name);
            app.set_status(format!("{short} is ready").into());
            if model.id == greycard_ai::SAM3.id {
                crate::panel::parts::model_answered(&mut st, app, true);
            }
            // A Landscape press waiting on this download, or on the
            // sheet it held, goes on.
            crate::panel::landscape::resume(&mut st, app);
            // Either Subject file arriving means the worker's loaded
            // one, if any, may no longer be the one to use: drop it,
            // so the next Subject mask picks up the new choice rather
            // than running on the session's first one until restart.
            if model.id == greycard_ai::SUBJECT.id || model.id == greycard_ai::SUBJECT_WEBGPU.id {
                // A Whole person made meanwhile with SAM's edge is
                // asked for again, for Subject's.
                st.develop
                    .learned
                    .retain(|_, (s, _)| !crate::ai::takes_subject_edge(s));
                st.sheets.edge_loaded = false;
                WORKER.with(|w| {
                    if let Some(w) = &*w.borrow() {
                        w.forget_subject();
                    }
                });
            }
            // A denoiser the edit is waiting for, or the fill model
            // with a fill to make: develop again with it.
            develop_for(&mut st, app, |m| m.id == model.id);
            app.window().request_redraw();
        }
        Outcome::RuntimeFetched { runtime } => {
            let mut st = state.borrow_mut();
            st.sheets.fetching = false;
            st.sheets.fetch_failed.retain(|id| *id != runtime.id);
            let short = runtime.name.split(',').next().unwrap_or(runtime.name);
            app.set_status(
                format!("{short} is ready: the AI features run on the CPU, slower").into(),
            );
            runtime_arrived(&mut st, app);
            app.window().request_redraw();
        }
        Outcome::FetchFailed { id, name, message } => {
            let mut st = state.borrow_mut();
            st.sheets.fetching = false;
            // The runtime's fetch failed: what it held up is let go,
            // and the AI features stay off this session.
            if greycard_ai::registry::runtime(id).is_some() {
                st.sheets.runtime_next = None;
                st.develop.part_pending = None;
            }
            tracing::error!("{name} could not be fetched: {message}");
            let status =
                crate::panel::mask::fetch_failed(&mut st.sheets.fetch_failed, id, name, &message);
            app.set_status(status.into());
            if st.develop.landscape_pending.is_some() {
                crate::panel::landscape::resume(&mut st, app);
            }
            // The masks waiting on it are asked about again, and may
            // now turn to another model.
            app.window().request_redraw();
        }
        Outcome::RuntimeUnusable { id, message } => {
            let mut st = state.borrow_mut();
            st.sheets.fetching = false;
            tracing::error!("{message}");
            // Not offered again this session; what it held up is let
            // go, and the masks waiting meet the same line.
            if !st.sheets.fetch_failed.contains(&id) {
                st.sheets.fetch_failed.push(id);
            }
            st.sheets.runtime_next = None;
            st.develop.part_pending = None;
            st.develop.landscape_pending = None;
            app.set_status(message.into());
            app.window().request_redraw();
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
        Outcome::EditedKept { frame } => {
            let mut st = state.borrow_mut();
            if let Some(worker) = WORKER.with(|w| w.borrow().clone()) {
                crate::edited::kept(&mut st, &worker, &frame);
            }
        }
        Outcome::LensesFetched => {
            let mut st = state.borrow_mut();
            st.sheets.fetching = false;
            app.set_status("the lens profiles are ready".into());
            // The develop keys in the index were made with the old
            // database; the next pass over each folder makes them anew.
            if let Some(indexer) = &st.views.index {
                indexer.keys_stale();
            }
            // The worker looks the open file up again on its next
            // open; ask it now, so the panel says what it found. Not
            // in culling, whose leaving opens the file anyway.
            if let Some(i) = st.current.filter(|_| st.culling.cull.is_none()) {
                st.develop.generation += 1;
                app.set_busy(true);
                let edit = st.develop.edit.clone();
                let job = crate::panel::browser::open_job(&mut st, i, edit);
                WORKER.with(|w| {
                    if let Some(w) = &*w.borrow() {
                        w.send(job);
                    }
                });
            }
        }
        Outcome::LensesFetchFailed { message } => {
            let mut st = state.borrow_mut();
            st.sheets.fetching = false;
            tracing::error!("the lens profiles could not be fetched: {message}");
            app.set_status(format!("the lens profiles could not be fetched: {message}").into());
        }
        Outcome::SetFrameStarted { set, index, name } => {
            let mut st = state.borrow_mut();
            if exporting(&st, &set) {
                let line = crate::panel::export_queue::progress_line(&st, &set, index, &name);
                app.set_status(line.into());
            }
            crate::panel::queue_view::began(&mut st, app, &set, index);
        }
        Outcome::SetFrameDone {
            set,
            index,
            source,
            edit,
            version,
            done,
        } => {
            let at = format!("{} of {}", index + 1, set.total);
            let ran = !matches!(done, queue::Done::Canceled);
            match done {
                queue::Done::Exported {
                    path,
                    seconds,
                    note,
                    left_out,
                } => {
                    tracing::info!("exported {at}: {} in {seconds:.2} s", path.display());
                    if let Some(note) = &note {
                        tracing::warn!("exported {}: {note}", path.display());
                    }
                    for item in &left_out {
                        tracing::warn!("{}: {item}", file_name(&source));
                    }
                    // A set of one ends on the file's own line, as the
                    // frame on screen always has: its name, a rename,
                    // what it went without.
                    if set.total == 1 {
                        let line = exported_line(&path, seconds, note.as_deref(), &left_out);
                        state.borrow_mut().work.export_one_line = Some((set.clone(), line));
                    }
                    crate::panel::history::record_export(
                        &mut state.borrow_mut(),
                        app,
                        &source,
                        &edit,
                        version.as_deref(),
                        &path,
                        set.preset.clone(),
                    );
                }
                queue::Done::Skipped { path } => {
                    tracing::warn!("skipped {at}: {} is there already", path.display());
                    if set.total == 1 {
                        let line =
                            format!("{} is there already: nothing exported", file_name(&path));
                        state.borrow_mut().work.export_one_line = Some((set.clone(), line));
                    }
                }
                // One line for the frame; the rest of the set goes on.
                queue::Done::Failed { message } => {
                    tracing::error!("export {at} failed: {}: {message}", source.display());
                }
                queue::Done::Canceled => {}
            }
            crate::panel::export_queue::frame_done(&mut state.borrow_mut(), &set, index, ran);
            crate::panel::queue_view::marks(&state.borrow(), app);
        }
        Outcome::SetDone { set, tally } => {
            let mut st = state.borrow_mut();
            let one = st
                .work
                .export_one_line
                .take()
                .filter(|(of, _)| Arc::ptr_eq(of, &set) && set.total == 1)
                .map(|(_, line)| line);
            let line = one.unwrap_or_else(|| {
                queue::finished_line(
                    &tally,
                    set.total,
                    &set.place(),
                    set.started.elapsed().as_secs_f64(),
                )
            });
            // Said on the terminal without -v when not all went.
            if tally.failed.is_empty() && tally.canceled == 0 {
                tracing::info!("{line}");
            } else {
                tracing::warn!("{line}");
            }
            if exporting(&st, &set) {
                st.work.exporting = None;
                app.set_export_running(false);
                app.set_export_stopping(false);
                app.set_status(line.clone().into());
            }
            // The queue's entry: what is left of it kept, and the next
            // begun unless the set was stopped.
            crate::panel::export_queue::set_done(&mut st, app, &set, &tally, &line);
        }
    }
}

/// Develop again for a model that has become usable, when the edit is
/// waiting on one `arrived` takes: the learned denoiser its tier names,
/// or the fill model with a fill to make. Not while culling.
fn develop_for(st: &mut State, app: &App, arrived: impl Fn(&greycard_ai::Model) -> bool) {
    let denoiser = st
        .develop
        .edit
        .noise
        .learned
        .tier()
        .and_then(greycard_ai::denoiser)
        .is_some_and(&arrived);
    // Either fill model: a patch drawn for the remove is LaMa's where
    // its window is past the remove's.
    let fill = (arrived(&greycard_ai::FILL) || arrived(&greycard_ai::REMOVE))
        && st.develop.edit.retouch.enabled
        && st
            .develop
            .edit
            .retouch
            .patches
            .iter()
            .any(|p| p.method == RetouchMethod::Fill);
    if (denoiser || fill) && st.culling.cull.is_none() {
        st.develop.generation += 1;
        app.set_status("developing...".into());
        app.set_busy(true);
        let turn = current_turn(st);
        WORKER.with(|w| {
            if let Some(w) = &*w.borrow() {
                w.send(Job::Develop {
                    edit: st.develop.edit.clone(),
                    generation: st.develop.generation,
                    turn,
                });
            }
        });
    }
}

/// Microsoft's CPU-only runtime has arrived and loaded: what its offer
/// held up goes on. The model offer it stood in for is made now, if
/// the model is still to fetch; the masks are asked again; a part or a
/// landscape waiting is asked for, or its model offered; and an edit
/// asking for the learned denoiser or a fill is developed again.
fn runtime_arrived(st: &mut State, app: &App) {
    st.develop.asked.clear();
    st.develop.masks_failed.clear();
    let store = st.develop.store.clone();
    let have = |m: &greycard_ai::Model| store.as_ref().is_some_and(|s| s.have(m));
    match st.sheets.runtime_next.take() {
        Some(crate::panel::assets::AfterRuntime::Model(model, falls_back)) if !have(model) => {
            offer_model(st, app, model, falls_back);
        }
        Some(crate::panel::assets::AfterRuntime::Edge(model)) if !have(model) => {
            crate::panel::assets::offer_edge(st, app, model);
        }
        _ => {}
    }
    if st.develop.part_pending.is_some() {
        if have(&greycard_ai::SAM3) {
            crate::panel::parts::model_answered(st, app, true);
        } else if st.sheets.fetch.is_none() && !st.sheets.declined.contains(&greycard_ai::SAM3.id) {
            offer_model(st, app, &greycard_ai::SAM3, false);
        }
    }
    crate::panel::landscape::resume(st, app);
    // A learned denoiser or a fill the edit asks for: developed again,
    // which runs it, or offers its model where it is still to fetch.
    develop_for(st, app, |_| true);
}

/// The frames an export takes as a set, in file order: each file with
/// its own edit, its turn and whether its blend is still to be seeded;
/// the frame on screen under the panel's edit, outside culling. One
/// frame when the selection is one frame.
#[cfg(test)]
pub(crate) fn set_frames(st: &mut State, app: &App) -> SetFrames {
    let chosen = chosen_frames(st);
    set_frames_of(st, app, &chosen)
}

/// `set_frames` over `frames` rather than the selection: what an
/// export pressed on a selection whose sidecars were still to be read
/// takes when they are in.
pub(crate) fn set_frames_of(st: &mut State, app: &App, frames: &[usize]) -> SetFrames {
    // Each frame's whole sidecar first, read where it stands in from
    // its row; a frame under a root that is offline cannot be decoded
    // either, and is left out with a word in the status line.
    let frames = crate::rows::load_frames(st, app, frames);
    frames
        .into_iter()
        .filter_map(|c| {
            // The panel is the current frame's outside culling only:
            // in it, the panel is no frame's (`save_edit`).
            let edit = if Some(c) == st.current && st.culling.cull.is_none() {
                read_edit(app, &st.develop.edit, st.develop.target)
            } else {
                st.line(c)?.current.clone()
            };
            let file = st.file_of(c);
            let seed = crate::rows::wants_seed(st, c);
            Some((
                st.files[file].clone(),
                edit,
                st.sidecars[file].turn,
                seed,
                frame_version(st, c),
            ))
        })
        .collect()
}

/// The capture time's shift of the file at `source`, as an export
/// carries it: the meta the window holds for a file it lists (read, or
/// standing in from its row), and otherwise the sidecar beside it. The
/// shift is the meta's and not the edit's, so it is taken as the set
/// starts, a queued entry's included, rather than when it was queued.
fn time_shift_of(st: &State, source: &Path) -> i64 {
    match st.file_by_path(source) {
        Some(f) => st.sidecars[f].meta.time_shift,
        None => greycard_edit::Sidecar::load(source)
            .ok()
            .flatten()
            .map_or(0, |s| s.meta.time_shift),
    }
}

/// The version cell `c` is, as an export names it: none for the file's
/// own.
pub(crate) fn frame_version(st: &State, c: usize) -> Option<queue::Version> {
    let id = st.version_of(c)?;
    Some(queue::Version {
        id: id.to_string(),
        name: st.version_name(c).unwrap_or_default(),
    })
}

/// The status line for one file written: its name, how long, a rename
/// ("was there already: ...") or a write over, and what the edit went
/// without.
pub(crate) fn exported_line(
    path: &Path,
    seconds: f64,
    note: Option<&str>,
    left_out: &[String],
) -> String {
    let mut said = match note {
        Some(note) => format!(" ({note})"),
        None => String::new(),
    };
    let parts = crate::ai::parts_left_out(left_out);
    let missing = left_out.len() - parts.0 - parts.1;
    if missing > 0 {
        said.push_str(&format!(", without {}", left_out_words(missing)));
    }
    said.push_str(&crate::ai::parts_words(false, parts));
    format!("exported {} in {seconds:.2} s{said}", file_name(path))
}

/// "one thing the edit names that ...", "2 things": what a status line
/// says of an export's left-out items, the log having the list.
pub(crate) fn left_out_words(n: usize) -> String {
    match n {
        1 => "one thing the edit names that this machine has not got (see the log)".into(),
        n => format!("{n} things the edit names that this machine has not got (see the log)"),
    }
}

/// Whether `set` is the one the window is exporting, rather than one
/// it has already let go of.
fn exporting(st: &State, set: &Arc<queue::Set>) -> bool {
    st.work
        .exporting
        .as_ref()
        .is_some_and(|s| Arc::ptr_eq(s, set))
}

/// Send `frames` to the worker as a set, into `folder`, or with none
/// into `sub` under each file's folder or beside each file, and hold
/// the set to cancel it. The set started, or none when there was
/// nothing to send.
#[allow(clippy::too_many_arguments)]
pub(crate) fn start_set(
    st: &mut State,
    app: &App,
    frames: SetFrames,
    folder: Option<PathBuf>,
    sub: Option<PathBuf>,
    settings: export::Settings,
    on_exists: export::OnExists,
    preset: Option<String>,
) -> Option<Arc<queue::Set>> {
    st.work.export_choosing = false;
    // Nothing to write would never be said done: the spinner would run
    // and a batch run would wait for ever.
    if frames.is_empty() {
        app.set_status("nothing to export".into());
        if st.window.batch {
            st.launch.failed = true;
            let _ = slint::quit_event_loop();
        }
        return None;
    }
    let sources: Vec<(PathBuf, Option<String>)> = frames
        .iter()
        .map(|f| (f.0.clone(), f.4.as_ref().map(|v| v.name.clone())))
        .collect();
    let outs = queue::named_of(
        &sources,
        folder.as_deref(),
        sub.as_deref(),
        settings.format,
        on_exists,
    );
    let frames: Vec<queue::Frame> = frames
        .into_iter()
        .zip(outs)
        .map(
            |((source, edit, turn, seed_blend, version), (out, moved_from))| queue::Frame {
                time_shift: time_shift_of(st, &source),
                source,
                edit,
                version,
                turn,
                seed_blend,
                out,
                moved_from,
            },
        )
        .collect();
    let set = Arc::new(
        queue::Set::new(frames.len(), folder, settings, on_exists)
            .with_preset(preset)
            .with_sub(sub),
    );
    tracing::info!(
        "exporting {} {}: {}",
        queue::frames(frames.len()),
        set.place(),
        set.settings.describe()
    );
    app.set_status(format!("exporting {}...", queue::frames(frames.len())).into());
    app.set_export_running(true);
    app.set_export_stopping(false);
    st.work.exporting = Some(set.clone());
    WORKER.with(|w| {
        if let Some(w) = &*w.borrow() {
            w.send(Job::ExportSet {
                set: set.clone(),
                frames,
            });
        }
    });
    Some(set)
}

/// A set held while the overwrite sheet asks about the files it would
/// write over: what `start_set` takes, but the policy, which is the
/// answer's.
pub(crate) struct Pending {
    frames: SetFrames,
    folder: Option<PathBuf>,
    sub: Option<PathBuf>,
    settings: export::Settings,
    preset: Option<String>,
}

/// `start_set`, unless the policy is Overwrite and some of the files
/// the set would write are there: then the overwrite sheet names every
/// one of them and the set waits for its answer. Only an export the
/// editor names the files of comes here; a file the desktop's chooser
/// named was confirmed in the chooser.
#[allow(clippy::too_many_arguments)]
fn start_or_ask(
    st: &mut State,
    app: &App,
    frames: SetFrames,
    folder: Option<PathBuf>,
    sub: Option<PathBuf>,
    settings: export::Settings,
    on_exists: export::OnExists,
    preset: Option<String>,
) {
    if on_exists == export::OnExists::Overwrite {
        let sources: Vec<PathBuf> = frames.iter().map(|f| f.0.clone()).collect();
        let there: Vec<PathBuf> = queue::names(
            &sources,
            folder.as_deref(),
            sub.as_deref(),
            settings.format,
            on_exists,
        )
        .into_iter()
        .filter(|p| p.exists())
        .collect();
        if !there.is_empty() {
            let title = match there.as_slice() {
                [one] => format!("{} is there already", file_name(one)),
                _ => format!("{} files are there already", there.len()),
            };
            let files: Vec<slint::SharedString> = there
                .iter()
                .map(|p| p.display().to_string().into())
                .collect();
            app.set_overwrite_title(title.as_str().into());
            app.set_overwrite_files(ModelRc::new(VecModel::from(files)));
            app.set_export_open(false);
            app.set_overwrite_open(true);
            app.set_status(format!("{title}: overwrite?").into());
            // One question at a time: another Export waits for this.
            st.work.export_choosing = true;
            st.work.export_pending = Some(Pending {
                frames,
                folder,
                sub,
                settings,
                preset,
            });
            return;
        }
    }
    start_set(st, app, frames, folder, sub, settings, on_exists, preset);
}

/// The overwrite sheet's answer: 0 no, 1 keep both (the set under
/// Increment), 2 overwrite.
fn overwrite_answered(st: &mut State, app: &App, answer: i32) {
    app.set_overwrite_open(false);
    let Some(p) = st.work.export_pending.take() else {
        return;
    };
    let on_exists = match answer {
        1 => export::OnExists::Increment,
        2 => export::OnExists::Overwrite,
        _ => {
            st.work.export_choosing = false;
            app.set_status("export canceled".into());
            return;
        }
    };
    start_set(
        st, app, p.frames, p.folder, p.sub, p.settings, on_exists, p.preset,
    );
}

/// The Subfolder field as the export reads it: the line under the
/// field says where the files go, or why they won't.
fn wire_subfolder(app: &App) {
    app.on_export_subfolder_name(|typed| Sheet::subfolder_shown(&typed).0.into());
    app.on_export_subfolder_problem(|typed| Sheet::subfolder_shown(&typed).1.into());
}

/// What the sheet's Folder row says for a Subfolder field reading
/// `typed`, the frame on screen and room for `room` characters: the
/// subfolder beside each original when one is typed, else the folder
/// Export writes to ([`sheet_folder`]), cut at the front so its leaf
/// shows. Empty for a subfolder the export refuses (the line under the
/// field says why) and with no frame.
pub(crate) fn folder_line(st: &State, typed: &str, room: usize) -> String {
    let (sub, problem) = Sheet::subfolder_shown(typed);
    if !problem.is_empty() {
        return String::new();
    }
    if !sub.is_empty() {
        const BESIDE: &str = " beside each original";
        let left = room.saturating_sub(BESIDE.len()).max(4);
        return format!("{}{BESIDE}", Sheet::elide_front(&sub, left));
    }
    let Some(raw) = st.current.and_then(|c| st.path_at(c)) else {
        return String::new();
    };
    Sheet::elide_front(&sheet_folder(st, raw).display().to_string(), room.max(4))
}

#[cfg(test)]
thread_local! {
    /// What the folder chooser answers in a test, in place of asking:
    /// `Some(folder)`, or none (and when nothing is set) a cancel.
    pub(crate) static CHOSEN: RefCell<Option<Option<PathBuf>>> = const { RefCell::new(None) };
    /// The folder the last chooser asked in a test opened on.
    pub(crate) static ASKED: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Ask the desktop for the folder exports go to, opening on `start`;
/// `then` has the answer on the window's thread. A test answers at
/// once with [`CHOSEN`].
fn ask_folder(
    app: &App,
    start: PathBuf,
    then: impl FnOnce(&App, anyhow::Result<Option<PathBuf>>) + Send + 'static,
) {
    #[cfg(test)]
    {
        ASKED.with(|a| *a.borrow_mut() = Some(start));
        let answer = CHOSEN.with(|c| c.borrow_mut().take()).flatten();
        then(app, Ok(answer));
    }
    #[cfg(not(test))]
    {
        let weak = app.as_weak();
        chooser::choose_folder("Export to a folder", start, move |chosen| {
            let _ = weak.upgrade_in_event_loop(move |app| then(&app, chosen));
        });
    }
}

/// Why `folder` cannot take an export, none when it can: it is there,
/// it is a folder, and a file can be made in it (one is, and removed
/// at once: the permission bits do not say what a share or a
/// read-only mount allows).
pub(crate) fn folder_problem(folder: &Path) -> Option<String> {
    folder_problem_probing(folder, probe_name)
}

/// The name of the check's file for `attempt`: of this run and this
/// moment.
fn probe_name(attempt: u32) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        ".greycard-write-check-{}-{nanos}-{attempt}",
        std::process::id()
    )
}

/// [`folder_problem`] with the check's file named by `name`. A name
/// taken already (another check's, left by a crash) is passed over for
/// a second, never read as the folder taking files, and never removed.
fn folder_problem_probing(folder: &Path, name: impl Fn(u32) -> String) -> Option<String> {
    let shown = folder.display();
    match std::fs::metadata(folder) {
        Err(_) => return Some(format!("{shown} is not there")),
        Ok(m) if !m.is_dir() => return Some(format!("{shown} is not a folder")),
        Ok(_) => {}
    }
    for attempt in 0..2 {
        let probe = folder.join(name(attempt));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
        {
            Ok(file) => {
                drop(file);
                let _ = std::fs::remove_file(&probe);
                return None;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => break,
        }
    }
    Some(format!("{shown} cannot be written to"))
}

/// [`folder_problem`] for `folder`, on a thread of its own since the
/// folder may be on a mount that keeps it waiting, and `then` with the
/// answer on the window's thread. A test looks at once.
fn check_folder(
    app: &App,
    folder: PathBuf,
    then: impl FnOnce(&App, Option<String>) + Send + 'static,
) {
    #[cfg(test)]
    {
        let problem = folder_problem(&folder);
        let held = HELD.with(|h| match h.borrow_mut().as_mut() {
            Some(held) => {
                held.push(Box::new(move |app: &App| then(app, problem)));
                None
            }
            None => Some((then, problem)),
        });
        if let Some((then, problem)) = held {
            then(app, problem);
        }
    }
    #[cfg(not(test))]
    {
        let weak = app.as_weak();
        std::thread::spawn(move || {
            let problem = folder_problem(&folder);
            let _ = weak.upgrade_in_event_loop(move |app| then(&app, problem));
        });
    }
}

/// A look at a folder held unanswered in a test: answered by calling it.
#[cfg(test)]
pub(crate) type Held = Box<dyn FnOnce(&App)>;

#[cfg(test)]
thread_local! {
    /// Looks at a folder held unanswered in a test, as a slow mount
    /// holds them, while this is some.
    pub(crate) static HELD: RefCell<Option<Vec<Held>>> = const { RefCell::new(None) };
}

/// The Folder row's Choose button: the chooser, on the folder the row
/// shows. A folder chosen is where exports go from now on, and a typed
/// subfolder is cleared, since it would otherwise still win. An export
/// whose folder is still being looked at is overtaken: its answer is
/// dropped, and Export is pressed again for the new folder.
fn choose_export_folder(state: &Rc<RefCell<State>>, app: &App) {
    let start = {
        let st = state.borrow();
        if st.work.exporting.is_some() || st.work.export_choosing {
            return;
        }
        let Some(raw) = st.current.and_then(|c| st.path_at(c)) else {
            return;
        };
        sheet_folder(&st, raw)
    };
    // An export still waiting on its look is not started: said with
    // the chooser's answer, since the status line shows that last.
    let overtaken = {
        let mut st = state.borrow_mut();
        st.work.export_checks += 1;
        st.work.export_choosing = true;
        st.work.export_looking.take()
    };
    app.set_status("choosing the folder exports go to...".into());
    ask_folder(app, start, move |app, chosen| {
        let Some(state) = STATE.with(|s| s.borrow().clone()) else {
            return;
        };
        let mut st = state.borrow_mut();
        st.work.export_choosing = false;
        let mut line = match chosen {
            Ok(Some(folder)) => {
                remember_export_last_folder(&mut st, app, &folder);
                if !app.get_export_subfolder().trim().is_empty() {
                    app.set_export_subfolder("".into());
                    show_edited(app, &st.work.export_presets);
                }
                format!("exports go to {}", folder.display())
            }
            Ok(None) => "the export folder is as it was".to_string(),
            Err(e) => {
                tracing::warn!("folder chooser: {e:#}");
                chooser::chooser_failed(&e)
            }
        };
        if let Some(waiting) = overtaken {
            line.push_str(&format!(
                "; the export waiting on {} was not started",
                waiting.display()
            ));
        }
        app.set_status(line.into());
    });
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_overwrite_answered(move |answer| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            overwrite_answered(&mut state.borrow_mut(), &app, answer);
        });
    }
    // Stop a set after the frame in hand.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_export_stop(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if let Some(set) = &state.borrow().work.exporting
                && !set.is_canceled()
            {
                set.cancel();
                // Escape is the window's again while the frame in
                // hand finishes.
                app.set_export_stopping(true);
                app.set_status("stopping the export after the frame in hand...".into());
            }
        });
    }
    wire_subfolder(app);
    {
        let state = state.clone();
        app.on_export_folder_line(move |typed, _name, _index, _turn, room| {
            // Read while the state is lent out (a layout asked for from
            // inside a borrow), it says nothing rather than panic.
            let Ok(st) = state.try_borrow() else {
                return slint::SharedString::default();
            };
            folder_line(&st, &typed, room.max(0) as usize).into()
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_export_folder_choose(move || {
            if let Some(app) = app_weak.upgrade() {
                choose_export_folder(&state, &app);
            }
        });
    }
    // The sheet's typed size, read the way the export reads it.
    app.on_custom_edge(|text| export::parse_edge(&text).map_or(0, |n| n as i32));
    // Any choice on the sheet: is it still the preset?
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_export_sheet_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                show_edited(&app, &state.borrow().work.export_presets);
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
            if let Some(p) = sheet::find(&st.work.export_presets, &name) {
                show_sheet(&app, &p.sheet);
                app.set_status(format!("export preset {}", p.name).into());
            }
            show_presets_picker(&app, &st.work.export_presets, Some(&name));
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
            if sheet::save_as(&mut st.work.export_presets, name, &current) {
                show_presets_picker(&app, &st.work.export_presets, Some(name));
                keep_presets(&st, &app);
                app.set_status(format!("export preset {name} saved").into());
            }
        });
    }
    // Whether Save as would replace one: the button says so.
    {
        let state = state.clone();
        app.on_export_preset_exists(move |name| {
            sheet::exists(&state.borrow().work.export_presets, &name)
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
            if sheet::delete(&mut st.work.export_presets, &name) {
                show_presets_picker(&app, &st.work.export_presets, None);
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
            chooser::choose_open(
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
                            app.set_status(chooser::chooser_failed(&e).into());
                        }
                    });
                },
            );
        });
    }
    // Export the current file under the panel's edit, beside it.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_export(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // A request for the set's sidecars, and the export over
            // exactly those frames once they are in, from the frame on
            // screen at the press, whatever is on screen by then.
            let (pressed, paths) = {
                let st = state.borrow();
                let Some(c) = st.current else {
                    return;
                };
                let paths: Vec<crate::versions::Key> = chosen_frames(&st)
                    .into_iter()
                    .filter_map(|c| st.key_of(c))
                    .collect();
                let Some(pressed) = st.key_of(c) else {
                    return;
                };
                (pressed, paths)
            };
            crate::rows::request(
                &state,
                &app,
                &worker,
                paths,
                crate::rows::Scope::Line,
                "export",
                Box::new(move |state, app, _, at| {
                    let frames: Vec<usize> = at.into_iter().flatten().collect();
                    export_frames(state, app, &pressed, &frames);
                }),
            );
        });
    }
}

/// The export pressed on `pressed`, the set's sidecars in hand: that
/// frame under the panel's edit, and `frames` as the set, into the
/// sheet's folder. A pressed frame no longer on screen when the
/// sidecars came in (the user moved on while they were read) is
/// exported under its own sidecar's edit, never the panel's.
fn export_frames(
    state: &Rc<RefCell<State>>,
    app: &App,
    pressed: &crate::versions::Key,
    frames: &[usize],
) {
    {
        let st = state.borrow();
        // One set at a time, and one folder asked about for it: a
        // second would start a set the first could not be told from.
        if st.work.exporting.is_some() || st.work.export_choosing {
            return;
        }
    }
    let Some((raw, _, frames, _)) = pressed_frames(state, app, pressed, frames) else {
        return;
    };
    export_gathered(state, app, raw, frames);
}

/// The frames of a set, each as `set_frames` gives it: the file, the
/// edit, the turn, the seed, and the version it is of (none for the
/// file's own).
pub(crate) type SetFrames = Vec<(PathBuf, Edit, u8, bool, Option<queue::Version>)>;

/// What an export pressed on `pressed` takes, the set's sidecars in
/// hand: the pressed frame's file and edit, the frames of the set with
/// theirs, and whether the pressed frame is the one on screen. The
/// panel's edit is made a state first when it is. None when there is
/// nothing to take, the reason on the status line.
pub(crate) fn pressed_frames(
    state: &Rc<RefCell<State>>,
    app: &App,
    pressed: &crate::versions::Key,
    frames: &[usize],
) -> Option<(PathBuf, Edit, SetFrames, bool)> {
    {
        let mut st = state.borrow_mut();
        // The export queue's view exports by its own run: a frame of it
        // pressed again would go out under its sidecar's edit, or its
        // queued one, and neither is what was asked of the view.
        if crate::panel::queue_view::refused(&st, "an export") {
            return None;
        }
        // Gone from the list: the request says so.
        let p = st.find_cell(pressed)?;
        // On screen is the panel's frame: never in culling, where an
        // export asked for before the mode can land (its request
        // queued behind a read) with the panel no frame's, and the
        // frame takes its sidecar's edit as any other does.
        let on_screen = st.current == Some(p) && st.culling.cull.is_none();
        // The frame on screen with a panel that is not its own (its
        // root offline, its sidecar not read): nothing to export, and
        // the worker's open picture is another frame's.
        if on_screen && let Some(why) = crate::rows::refused(&st, p, "nothing is exported") {
            app.set_status(why.into());
            return None;
        }
        let edit = if on_screen {
            let edit = read_edit(app, &st.develop.edit, st.develop.target);
            // What the panel holds is a state first, as for an
            // undo or a snapshot: the export is recorded on the
            // state it was written from once it is done, and a
            // panel that moved on meanwhile would otherwise have
            // saved over it before it ever was one.
            crate::panel::edit::save_edit(&mut st, edit.clone());
            show_history(&st, app);
            edit
        } else {
            if !crate::rows::load_frame(&mut st, app, p) {
                return None;
            }
            st.line(p)?.current.clone()
        };
        let mut frames = set_frames_of(&mut st, app, frames);
        // One frame is the pressed one, as on screen it would be.
        if !on_screen && frames.len() <= 1 {
            let file = st.file_of(p);
            frames = vec![(
                st.files[file].clone(),
                edit.clone(),
                st.sidecars[file].turn,
                crate::rows::wants_seed(&st, p),
                frame_version(&st, p),
            )];
        }
        Some((st.path_of(p).clone(), edit, frames, on_screen))
    }
}

/// The export of what [`pressed_frames`] took, as a set whatever its
/// size, with no dialog: with a subfolder on the sheet, into it beside
/// each frame; else into the folder the sheet's Folder row shows for
/// `raw`, each frame under its own name there.
fn export_gathered(state: &Rc<RefCell<State>>, app: &App, raw: PathBuf, frames: SetFrames) {
    let preset = preset_in_use(app);
    let sheet = read_sheet(app);
    let settings = sheet.settings();
    // A mark asked for with nothing to draw: say so and keep
    // the sheet up, rather than write the picture unmarked.
    if let Some(Err(e)) = settings.watermark.as_ref().map(|m| m.check()) {
        app.set_status(format!("not exported: {e:#}").into());
        app.set_export_open(true);
        return;
    }
    let on_exists = sheet.on_exists();
    match sheet.subfolder() {
        Err(e) => {
            app.set_status(format!("not exported: {e}").into());
            app.set_export_open(true);
            return;
        }
        // The editor names every file, so the sheet's policy holds
        // for each, the frame on screen as much as a set.
        Ok(Some(sub)) => {
            let mut st = state.borrow_mut();
            start_or_ask(
                &mut st,
                app,
                frames,
                None,
                Some(sub),
                settings,
                on_exists,
                preset,
            );
            return;
        }
        Ok(None) => {}
    }
    // Into the folder the Folder row shows, with no dialog, once it
    // is known to take files: one that is gone or will not take them
    // is said, and the chooser asked instead, so the files never go
    // anywhere the row did not name. The look holds nothing up: a
    // later Export or a Choose overtakes it, and its answer is dropped.
    let folder = sheet_folder(&state.borrow(), &raw);
    let check = {
        let mut st = state.borrow_mut();
        st.work.export_checks += 1;
        st.work.export_looking = Some(folder.clone());
        st.work.export_checks
    };
    app.set_status(format!("exporting to {}...", folder.display()).into());
    let go = move |app: &App, folder: PathBuf| {
        let Some(state) = STATE.with(|s| s.borrow().clone()) else {
            return;
        };
        let mut st = state.borrow_mut();
        start_or_ask(
            &mut st,
            app,
            frames,
            Some(folder),
            None,
            settings,
            on_exists,
            preset,
        );
    };
    check_folder(app, folder.clone(), move |app, problem| {
        let Some(state) = STATE.with(|s| s.borrow().clone()) else {
            return;
        };
        {
            let mut st = state.borrow_mut();
            // Overtaken while the folder was looked at, by a later
            // Export or a Choose, which say so themselves.
            if st.work.export_checks != check {
                tracing::info!("a look at {} answered too late", folder.display());
                return;
            }
            st.work.export_looking = None;
            // Another export under way since: this one is not started.
            if st.work.exporting.is_some() || st.work.export_choosing {
                app.set_status("not exported: another export is under way".into());
                return;
            }
        }
        let Some(why) = problem else {
            return go(app, folder);
        };
        tracing::warn!("export folder: {why}");
        app.set_status(format!("{why}: choose another folder").into());
        state.borrow_mut().work.export_choosing = true;
        ask_folder(app, folder, move |app, chosen| {
            if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                let mut st = state.borrow_mut();
                st.work.export_choosing = false;
                if let Ok(Some(folder)) = &chosen {
                    remember_export_last_folder(&mut st, app, folder);
                }
            }
            match chosen {
                Ok(Some(folder)) => go(app, folder),
                Ok(None) => app.set_status(format!("not exported: {why}").into()),
                Err(e) => {
                    tracing::warn!("folder chooser: {e:#}");
                    app.set_status(
                        format!("not exported: {why}; {}", chooser::chooser_failed(&e)).into(),
                    );
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sheet_picks_saves_marks_and_deletes_presets() {
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::retouch_state(&app);
        // A batch state: nothing here may write the user's settings.
        state.borrow_mut().window.batch = true;
        let web = Sheet {
            size: "2048".into(),
            quality: 85.0,
            mark: sheet::MARK_TEXT.into(),
            mark_text: "© me".into(),
            ..Sheet::default()
        };
        state.borrow_mut().work.export_presets = vec![ExportPreset {
            name: "Web".into(),
            sheet: web.clone(),
        }];
        show_presets_picker(&app, &state.borrow().work.export_presets, None);
        assert_eq!(app.get_export_preset(), NO_PRESET);
        assert_eq!(app.get_export_presets().row_count(), 2);
        // One an older file named "none" is not listed beside None.
        let mut with_none = state.borrow().work.export_presets.clone();
        with_none.push(ExportPreset {
            name: "none".into(),
            sheet: Sheet::default(),
        });
        show_presets_picker(&app, &with_none, Some("none"));
        let listed: Vec<String> = app
            .get_export_presets()
            .iter()
            .map(|n| n.to_string())
            .collect();
        assert_eq!(listed, vec![NO_PRESET.to_string(), "Web".to_string()]);
        assert_eq!(app.get_export_preset(), NO_PRESET);
        show_presets_picker(&app, &state.borrow().work.export_presets, None);
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
        assert_eq!(state.borrow().work.export_presets.len(), 2);
        assert_eq!(state.borrow().work.export_presets[1].sheet.quality, 70.0);
        // "None" is the picker's, not a name.
        app.invoke_export_preset_saved(NO_PRESET.into());
        assert_eq!(state.borrow().work.export_presets.len(), 2);

        // Deleted: gone, nothing chosen, the sheet as it was.
        app.invoke_export_preset_deleted();
        assert_eq!(state.borrow().work.export_presets.len(), 1);
        assert_eq!(app.get_export_preset(), NO_PRESET);
        assert_eq!(app.get_export_quality(), 70.0);
        // Nothing was written: the state has no settings file.
        assert!(state.borrow().window.settings_file.is_none());
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
        state.borrow_mut().window.settings_file = Some(file.clone());
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
        crate::testing::remove_dir_retry(&dir);
    }

    /// What the sheet's Folder row says, as the sheet asks it.
    fn row(app: &App, room: i32) -> String {
        let typed = app.get_export_subfolder();
        app.invoke_export_folder_line(typed, "".into(), 0, app.get_export_folder_turn(), room)
            .to_string()
    }

    /// The Folder row shows where Export writes: the frame's own folder
    /// until one is chosen, then the folder last chosen, in this run and
    /// after a restart (the settings file). Its Choose button opens the
    /// chooser on the folder shown, and an answer is the new folder, in
    /// the row and in the file; a cancel leaves it. A preset that names
    /// a subfolder shows that beside each original and leaves the
    /// remembered folder alone; one that names none shows the remembered
    /// folder, and its chooser opens there.
    #[test]
    fn the_folder_row_shows_where_the_export_goes_and_choose_sets_it() {
        let app = crate::testing::window(1);
        let dir = crate::testing::scratch_dir("export-folder");
        let raw = dir.join("shoot").join("a.cr3");
        let (state, _worker) = crate::testing::state_for(&app, vec![raw.clone()]);
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let file = dir.join("greycard").join("settings.json");
        settings::Settings::default().save_to(&file);
        {
            let mut st = state.borrow_mut();
            st.window.settings_file = Some(file.clone());
            st.current = Some(0);
        }
        let shown = |p: &Path| p.display().to_string();

        // Nothing chosen yet: the frame's own folder.
        assert_eq!(sheet_folder(&state.borrow(), &raw), dir.join("shoot"));
        assert_eq!(row(&app, 500), shown(&dir.join("shoot")));

        // Choose: the chooser opens on the folder shown, and its
        // answer is the row's and the file's at once.
        CHOSEN.with(|c| *c.borrow_mut() = Some(Some(out.clone())));
        app.invoke_export_folder_choose();
        assert_eq!(
            ASKED.with(|a| a.borrow_mut().take()),
            Some(dir.join("shoot"))
        );
        assert_eq!(
            state.borrow().work.export_last_folder.as_deref(),
            Some(out.as_path())
        );
        assert_eq!(row(&app, 500), shown(&out));
        assert_eq!(
            settings::Settings::load_from(&file).export_last_folder,
            out.to_string_lossy()
        );
        assert!(!state.borrow().work.export_choosing);
        assert_eq!(app.get_status(), format!("exports go to {}", shown(&out)));

        // A cancel leaves it, and the next chooser opens where the
        // last answer was.
        app.invoke_export_folder_choose();
        assert_eq!(ASKED.with(|a| a.borrow_mut().take()), Some(out.clone()));
        assert_eq!(row(&app, 500), shown(&out));
        assert_eq!(app.get_status(), "the export folder is as it was");

        // Cut at the front, so the folder's own name shows.
        let narrow = row(&app, 12);
        assert_eq!(narrow.chars().count(), 12, "{narrow}");
        assert!(
            narrow.starts_with('\u{2026}') && narrow.ends_with("out"),
            "{narrow}"
        );

        // A preset with a subfolder: beside each original, the
        // remembered folder kept.
        state.borrow_mut().work.export_presets = vec![
            ExportPreset {
                name: "Beside".into(),
                sheet: Sheet {
                    subfolder: "web".into(),
                    ..Sheet::default()
                },
            },
            ExportPreset {
                name: "Plain".into(),
                sheet: Sheet::default(),
            },
        ];
        app.invoke_export_preset_chosen("Beside".into());
        assert_eq!(row(&app, 500), "web beside each original");
        assert_eq!(sheet_folder(&state.borrow(), &raw), out);
        // Choosing a folder there clears the subfolder, which would
        // otherwise still win, and the preset reads as edited.
        let other = dir.join("other");
        CHOSEN.with(|c| *c.borrow_mut() = Some(Some(other.clone())));
        app.invoke_export_folder_choose();
        assert_eq!(app.get_export_subfolder(), "");
        assert!(app.get_export_preset_edited());
        assert_eq!(row(&app, 500), shown(&other));

        // A preset with none: the remembered folder, and Choose opens
        // the chooser there.
        app.invoke_export_preset_chosen("Plain".into());
        assert_eq!(row(&app, 500), shown(&other));
        app.invoke_export_folder_choose();
        assert_eq!(ASKED.with(|a| a.borrow_mut().take()), Some(other.clone()));

        // A subfolder the export refuses: the row says nothing, the line
        // under the field says why.
        app.set_export_subfolder("../out".into());
        assert_eq!(row(&app, 500), "");
        app.set_export_subfolder("".into());

        // After a restart, last, since a second window's state is the
        // one this thread's callbacks find from then on: the close
        // writes the file from the panel and the state, as `run` does;
        // the next start reads it back.
        let mut closing = crate::panel::startup::remember(&app);
        crate::panel::startup::take_state_choices(&mut closing, &state.borrow());
        closing.save_to(&file);
        let kept = settings::Settings::load_from(&file);
        assert_eq!(kept.export_last_folder, other.to_string_lossy());
        let app2 = crate::testing::window(1);
        let (second, _worker) = crate::testing::state_for(&app2, vec![raw.clone()]);
        {
            let mut st = second.borrow_mut();
            st.work.export_last_folder = crate::panel::startup::remembered_folder(&kept);
            st.current = Some(0);
        }
        assert_eq!(row(&app2, 500), shown(&other));
        crate::testing::remove_dir_retry(&dir);
    }

    /// A small picture the editor opens, of our own writing.
    fn picture(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbImage::from_fn(48, 32, |x, y| {
            image::Rgb([(x * 5) as u8, (y * 7) as u8, ((x + y) * 3) as u8])
        })
        .save(path)
        .unwrap();
    }

    /// Export writes into the folder the row shows, with no dialog: the
    /// frame on screen under its own name there, the file on disk. A
    /// file of that name already there is the overwrite sheet's
    /// question under Overwrite, as for a set. A folder that is gone or
    /// will not take files is said, and the chooser asked on it; a
    /// cancel exports nothing, an answer exports there.
    #[test]
    fn export_writes_into_the_shown_folder_with_no_dialog() {
        let app = crate::testing::window(1);
        let dir = crate::testing::scratch_dir("export-shown");
        let source = dir.join("shoot").join("frame.png");
        picture(&source);
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let (state, _worker) = crate::testing::state_for(&app, vec![source.clone()]);
        {
            let mut st = state.borrow_mut();
            st.current = Some(0);
            st.work.export_last_folder = Some(out.clone());
        }
        ASKED.with(|a| a.borrow_mut().take());

        app.invoke_export();
        assert!(ASKED.with(|a| a.borrow().is_none()), "no dialog");
        let set = state
            .borrow()
            .work
            .exporting
            .clone()
            .expect("the set, at once");
        assert_eq!(set.total, 1);
        assert_eq!(set.folder.as_deref(), Some(out.as_path()));
        assert_eq!(set.sub, None);
        // The frame's own name, in the format's extension.
        let written = out.join("frame.jpg");
        let start = Instant::now();
        while !written.exists() {
            assert!(
                start.elapsed() < Duration::from_secs(60),
                "the worker writes {}",
                written.display()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        // Nothing beside the frame, nothing left over from the check.
        let names = |d: &Path| -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(d)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        assert!(
            !names(&dir.join("shoot"))
                .iter()
                .any(|n| n.ends_with(".jpg")),
            "{:?}",
            names(&dir.join("shoot"))
        );
        assert!(
            !names(&out).iter().any(|n| n.contains("write-check")),
            "{:?}",
            names(&out)
        );
        // Wait for the set to be let go of, as its end would.
        state.borrow_mut().work.exporting = None;

        // That name is there now: under Overwrite, the sheet asks.
        app.set_export_on_exists("Overwrite".into());
        app.invoke_export();
        assert!(app.get_overwrite_open());
        assert_eq!(
            app.get_overwrite_files()
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>(),
            [written.display().to_string()]
        );
        app.invoke_overwrite_answered(0);
        assert!(state.borrow().work.exporting.is_none());
        app.set_export_on_exists("Increment".into());

        // Gone since it was chosen: said, and the chooser asked on it.
        let gone = dir.join("unplugged");
        state.borrow_mut().work.export_last_folder = Some(gone.clone());
        app.invoke_export();
        assert_eq!(ASKED.with(|a| a.borrow_mut().take()), Some(gone.clone()));
        assert_eq!(
            app.get_status(),
            format!("not exported: {} is not there", gone.display())
        );
        assert!(state.borrow().work.exporting.is_none());
        assert!(!state.borrow().work.export_choosing);
        assert!(!gone.exists(), "never made");
        // Answered: there, and it is the folder from now on.
        let again = dir.join("again");
        std::fs::create_dir_all(&again).unwrap();
        CHOSEN.with(|c| *c.borrow_mut() = Some(Some(again.clone())));
        app.invoke_export();
        let set = state.borrow().work.exporting.clone().expect("the set");
        assert_eq!(set.folder.as_deref(), Some(again.as_path()));
        assert_eq!(
            state.borrow().work.export_last_folder.as_deref(),
            Some(again.as_path())
        );
        let start = Instant::now();
        while !again.join("frame.jpg").exists() {
            assert!(start.elapsed() < Duration::from_secs(60), "written");
            std::thread::sleep(Duration::from_millis(50));
        }
        state.borrow_mut().work.exporting = None;

        // A folder that will not take files: said the same way.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let locked = dir.join("locked");
            std::fs::create_dir_all(&locked).unwrap();
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
            // Not where permissions are not kept (root).
            if folder_problem(&locked).is_some() {
                state.borrow_mut().work.export_last_folder = Some(locked.clone());
                app.invoke_export();
                assert_eq!(ASKED.with(|a| a.borrow_mut().take()), Some(locked.clone()));
                assert_eq!(
                    app.get_status(),
                    format!("not exported: {} cannot be written to", locked.display())
                );
                assert!(state.borrow().work.exporting.is_none());
            }
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            folder_problem(&written),
            Some(format!("{} is not a folder", written.display()))
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// Microsoft's runtime that arrived and would not load is said as
    /// that, not as a download that failed, lets go of what it held
    /// up, and is not offered again this session.
    #[test]
    fn a_runtime_that_would_not_load_is_said_so_and_not_offered_again() {
        use crate::panel::assets::{AfterRuntime, runtime_first_for};
        use greycard_ai::runtime::Unavailable;
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        let runtime = &greycard_ai::registry::ONNXRUNTIME_CPU_LINUX_X64;
        {
            let mut st = state.borrow_mut();
            st.sheets.fetching = true;
            st.sheets.runtime_next = Some(AfterRuntime::Model(&greycard_ai::SKY, false));
        }
        let message = "the AI features are off: ONNX Runtime at /c/libonnxruntime.so would not \
                       load: GLIBC_2.27 not found; it is removed again, and offered at the next \
                       start";
        deliver(
            &app,
            Outcome::RuntimeUnusable {
                id: runtime.id,
                message: message.to_string(),
            },
        );
        assert_eq!(app.get_status(), message);
        assert!(!app.get_status().contains("could not be fetched"));
        {
            let st = state.borrow();
            assert!(!st.sheets.fetching);
            assert_eq!(st.sheets.runtime_next, None);
            assert!(st.sheets.fetch_failed.contains(&runtime.id));
        }
        let why: &'static Unavailable = Box::leak(Box::new(Unavailable::Fetchable {
            missing: vec!["AVX2"],
            name: None,
            runtime,
        }));
        assert!(!runtime_first_for(
            &mut state.borrow_mut(),
            &app,
            why,
            AfterRuntime::Nothing
        ));
        assert!(!app.get_fetch_open());
    }

    /// A set is read as each frame's own edit, the one on screen as
    /// the panel has it; a running set stops on Escape, and its end
    /// says what came of it and lets the Export button go.
    #[test]
    fn a_set_takes_each_frame_s_edit_and_stops_on_escape() {
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
        {
            let mut st = state.borrow_mut();
            st.current = Some(0);
            st.views.picked = vec![0, 2];
            st.sidecars[2].current.light.exposure = 1.0;
            st.sidecars[2].turn = 3;
            st.sidecars[1].current.light.exposure = -2.0;
        }
        app.set_exposure(0.5);
        let frames = set_frames(&mut state.borrow_mut(), &app);
        let got: Vec<(String, f32, u8)> = frames
            .iter()
            .map(|(f, e, t, _, _)| (file_name(f), e.light.exposure, *t))
            .collect();
        assert_eq!(
            got,
            vec![
                ("IMG_0000.CR3".to_string(), 0.5, 0),
                ("IMG_0002.CR3".to_string(), 1.0, 3),
            ]
        );

        // Running: Escape stops it after the frame in hand.
        let set = Arc::new(queue::Set::new(
            2,
            Some(PathBuf::from("/nowhere/out")),
            export::Settings::default(),
            export::OnExists::Increment,
        ));
        state.borrow_mut().work.exporting = Some(set.clone());
        app.set_export_running(true);
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(set.is_canceled());
        assert!(
            app.get_status().contains("stopping"),
            "{}",
            app.get_status()
        );
        // The set collapses on the next Escape, not this one; the
        // next is the window's again though the frame in hand runs on.
        assert_eq!(state.borrow().views.picked, vec![0, 2]);
        assert!(app.get_export_stopping());
        app.set_set_count(2);
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert_eq!(chosen_frames(&state.borrow()), vec![0]);
        assert!(app.get_export_running(), "the frame in hand still runs");

        // Its end: the line, and the button is Export again.
        let tally = queue::Tally {
            exported: 1,
            canceled: 1,
            ..queue::Tally::default()
        };
        deliver(&app, Outcome::SetDone { set, tally });
        assert!(!app.get_export_running());
        assert!(!app.get_export_stopping());
        assert!(state.borrow().work.exporting.is_none());
        assert!(
            app.get_status()
                .starts_with("export stopped: 1 of 2 exported to"),
            "{}",
            app.get_status()
        );
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

    #[test]
    fn a_subfolder_exports_beside_each_frame_without_a_chooser() {
        let app = crate::testing::window(2);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
        state.borrow_mut().current = Some(0);
        // One that climbs out is refused, and the sheet stays up.
        app.set_export_subfolder("../out".into());
        app.set_export_open(false);
        app.invoke_export();
        assert!(app.get_export_open());
        assert!(state.borrow().work.exporting.is_none());
        assert!(
            app.get_status().starts_with("not exported: the subfolder"),
            "{}",
            app.get_status()
        );
        // One that goes down: the frame on screen is a set of one into
        // it, nothing asked, under the sheet's own policy.
        app.set_export_subfolder(" export ".into());
        app.set_export_on_exists("Skip".into());
        app.invoke_export();
        let st = state.borrow();
        assert!(!st.work.export_choosing);
        let set = st.work.exporting.as_ref().expect("a set, with no chooser");
        assert_eq!(set.total, 1);
        assert_eq!(set.folder, None);
        assert_eq!(set.sub.as_deref(), Some(Path::new("export")));
        assert_eq!(set.on_exists, export::OnExists::Skip);
        assert!(set.place().starts_with("into export"), "{}", set.place());
    }

    /// The sheet's primary button reads Export, for one frame and for a
    /// set alike, since no dialog follows; the chooser is the Folder
    /// row's Choose button, and there is no Choose file. The row names a
    /// typed subfolder by the reading the export goes by, cut at the
    /// front; one the export refuses stops the click.
    #[test]
    fn the_primary_button_reads_export_and_the_row_names_the_subfolder() {
        let app = crate::testing::window(2);
        let (_state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
        app.invoke_select(0);
        let count = |label: &str| crate::testing::count_labeled(&app, label);
        let closed = count("Export");
        app.set_export_open(true);
        assert_eq!(count("Export") - closed, 2, "the title and the button");
        assert_eq!(count("Choose file..."), 0);
        assert_eq!(count("Choose..."), 1, "the Folder row's");
        assert_eq!(count("Add to queue"), 1);
        assert_eq!(count("/nowhere"), 1, "the frame's own folder");
        // A set: the title counts the frames, the button is the same.
        app.set_set_count(3);
        assert_eq!(count("Export 3 frames"), 1);
        assert_eq!(count("Export") - closed, 1, "the button");

        let long = "a-very-long-subfolder/for-the-web-gallery";
        app.set_export_subfolder(long.into());
        assert_eq!(row(&app, 500), format!("{long} beside each original"));
        assert_eq!(row(&app, 30), "\u{2026}-gallery beside each original");
        assert_eq!(app.invoke_export_subfolder_name(long.into()), long);
        for typed in ["../out", "/tmp/x", "."] {
            assert!(!app.invoke_export_subfolder_problem(typed.into()).is_empty());
            // And a click on it is refused.
            let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
            state.borrow_mut().current = Some(0);
            app.set_export_subfolder(typed.into());
            assert_eq!(row(&app, 500), "", "{typed}");
            app.set_export_open(false);
            app.invoke_export();
            assert!(app.get_export_open(), "{typed}");
            assert!(state.borrow().work.exporting.is_none(), "{typed}");
        }
    }

    #[test]
    fn overwrite_names_every_file_there_and_waits_for_an_answer() {
        let dir = std::env::temp_dir().join(format!("greycard-overwrite-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("export")).unwrap();
        let files: Vec<PathBuf> = (0..3)
            .map(|i| {
                let f = dir.join(format!("IMG_{i:04}.CR3"));
                std::fs::write(&f, b"").unwrap();
                f
            })
            .collect();
        // Two of the three are there already.
        for i in [0, 2] {
            std::fs::write(dir.join("export").join(format!("IMG_{i:04}.jpg")), b"").unwrap();
        }
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, files);
        {
            let mut st = state.borrow_mut();
            st.current = Some(0);
            st.views.picked = vec![0, 1, 2];
        }
        app.set_export_subfolder("export".into());
        app.set_export_on_exists("Overwrite".into());
        let ask = || {
            app.invoke_export();
            assert!(app.get_overwrite_open());
            assert!(
                state.borrow().work.exporting.is_none(),
                "nothing written yet"
            );
        };

        ask();
        assert_eq!(app.get_overwrite_title(), "2 files are there already");
        let listed: Vec<String> = app
            .get_overwrite_files()
            .iter()
            .map(|f| f.to_string())
            .collect();
        assert_eq!(
            listed,
            vec![
                dir.join("export")
                    .join("IMG_0000.jpg")
                    .display()
                    .to_string(),
                dir.join("export")
                    .join("IMG_0002.jpg")
                    .display()
                    .to_string(),
            ]
        );
        // Another Export waits for the answer.
        app.set_status("".into());
        app.invoke_export();
        assert!(state.borrow().work.export_pending.is_some());
        // No: nothing goes, and Export is free again.
        app.invoke_overwrite_answered(0);
        assert!(!app.get_overwrite_open());
        assert_eq!(app.get_status(), "export canceled");
        assert!(state.borrow().work.exporting.is_none() && !state.borrow().work.export_choosing);

        // Keep both: the set, under Increment.
        ask();
        app.invoke_overwrite_answered(1);
        let set = state.borrow().work.exporting.clone().expect("the set");
        assert_eq!(set.on_exists, export::OnExists::Increment);
        assert_eq!(set.total, 3);
        state.borrow_mut().work.exporting = None;

        // Overwrite: the set, under Overwrite.
        ask();
        app.invoke_overwrite_answered(2);
        let set = state.borrow().work.exporting.clone().expect("the set");
        assert_eq!(set.on_exists, export::OnExists::Overwrite);
        state.borrow_mut().work.exporting = None;

        // Nothing there: no question.
        for i in [0, 2] {
            std::fs::remove_file(dir.join("export").join(format!("IMG_{i:04}.jpg"))).unwrap();
        }
        app.invoke_export();
        assert!(!app.get_overwrite_open());
        assert!(state.borrow().work.exporting.is_some());
        state.borrow_mut().work.exporting = None;
        crate::testing::remove_dir_retry(&dir);
    }

    /// Where the element a screen reader knows as `label` is, and how
    /// big, if it is on show: the topmost of several.
    fn shown(app: &App, label: &str) -> Option<(slint::LogicalPosition, slint::LogicalSize)> {
        use i_slint_backend_testing::ElementHandle;
        // A control a property has just brought in is built on the
        // next event: a pointer move at the corner, where nothing is.
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
        ElementHandle::find_by_accessible_label(app, label)
            .map(|e| (e.absolute_position(), e.size()))
            .min_by(|a, b| a.0.y.total_cmp(&b.0.y))
    }

    fn wheel(app: &App, x: f32, y: f32, delta_y: f32) {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(x, y),
                delta_x: 0.0,
                delta_y,
            });
    }

    #[test]
    fn a_wheel_over_an_open_sheet_leaves_the_picture_alone() {
        let app = crate::testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 600.0));
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        // Where the picture is and how big: each wheel is judged on its
        // own, as a swipe out and its swipe back would cancel.
        let view = || {
            let st = state.borrow();
            (st.develop.zoom, st.develop.center)
        };
        // Left of any sheet's card, over the picture.
        let (x, y) = (400.0, 300.0);
        let before = view();
        wheel(&app, x, y, 60.0);
        assert_ne!(view(), before, "with no sheet up, the wheel zooms there");
        let still = |at: (f32, f32), what: &str| {
            for delta in [60.0, -60.0, 15.0, -15.0] {
                let before = view();
                wheel(&app, at.0, at.1, delta);
                assert_eq!(view(), before, "{what}, by {delta}");
            }
        };

        type Open = fn(&App, bool);
        let sheets: [(&str, Open); 7] = [
            ("export", |a, on| a.set_export_open(on)),
            ("preset", |a, on| a.set_preset_open(on)),
            ("import", |a, on| a.set_import_open(on)),
            ("settings", |a, on| a.set_settings_open(on)),
            ("archive", |a, on| a.set_archive_open(on)),
            ("sync", |a, on| a.set_sync_open(on)),
            ("match", |a, on| a.set_match_open(on)),
        ];
        for (name, open) in sheets {
            open(&app, true);
            // Its surround, and the middle of the window, where its
            // card is.
            still((x, y), &format!("the {name} sheet's surround"));
            still((750.0, 300.0), &format!("the {name} sheet's card"));
            open(&app, false);
        }

        // The export sheet's own header and buttons, outside its
        // scrolled choices, and those choices scrolled to either end.
        app.set_export_mark("Text".into());
        app.set_export_open(true);
        let (title, _) = shown(&app, "Export").expect("the title");
        let (cancel, size) = shown(&app, "Cancel").expect("Cancel");
        let (format, _) = shown(&app, "Format").expect("the Format row");
        still((title.x + 4.0, title.y + 4.0), "the export sheet's header");
        still(
            (cancel.x + size.width / 2.0, cancel.y + size.height / 2.0),
            "the export sheet's buttons",
        );
        for _ in 0..20 {
            still((format.x + 4.0, format.y + 4.0), "the choices");
        }
    }

    #[test]
    fn the_export_sheet_fits_a_short_window_and_its_choices_scroll() {
        let app = crate::testing::window(1);
        // Just over the shortest the window goes.
        let height = 520.0;
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, height));
        let (_state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        // Everything that makes it tall: a watermark's block and the
        // queue's row.
        app.set_export_mark("Text".into());
        app.set_export_queue_sets(2);
        app.set_export_queue_line("2 sets waiting".into());
        app.set_export_open(true);

        let bottom = |label: &str| {
            let (at, size) = shown(&app, label).unwrap_or_else(|| panic!("{label} on show"));
            at.y + size.height
        };
        // Every button in the window, below the choices, and the
        // Folder row with them.
        for label in [
            "/nowhere",
            "Choose...",
            "Cancel",
            "Add to queue",
            "Export the queue",
            "Clear",
        ] {
            assert!(
                bottom(label) <= height,
                "{label} at {} in {height}",
                bottom(label)
            );
        }
        let (title, _) = shown(&app, "Export").expect("the title");
        let (cancel, _) = shown(&app, "Cancel").expect("Cancel");
        let (folder, _) = shown(&app, "/nowhere").expect("the Folder row");
        // The watermark's strength is past the fold, until the choices
        // are scrolled to it.
        assert!(
            shown(&app, "Opacity").is_none(),
            "the choices are taller than their view"
        );
        let (format, _) = shown(&app, "Format").expect("the Format row");
        for _ in 0..20 {
            wheel(&app, format.x + 4.0, format.y + 4.0, -60.0);
        }
        let (opacity, size) = shown(&app, "Opacity").expect("scrolled to Opacity");
        assert!(
            opacity.y + size.height <= folder.y,
            "above the folder and buttons"
        );
        assert_eq!(
            shown(&app, "Export").expect("the title").0,
            title,
            "the header stays"
        );
        assert_eq!(
            shown(&app, "Cancel").expect("Cancel").0,
            cancel,
            "the buttons stay"
        );
        assert_eq!(
            shown(&app, "/nowhere").expect("the Folder row").0,
            folder,
            "the folder stays"
        );
    }

    /// A set of one ends on the file's own line, as the frame on screen
    /// always did: its name, the rename Increment made and what the edit
    /// went without, none of which a set's count says. A set of two ends
    /// on the count.
    #[test]
    fn a_set_of_one_ends_on_the_file_s_own_line() {
        let app = crate::testing::window(2);
        let dir = crate::testing::scratch_dir("export-one-line");
        std::fs::create_dir_all(&dir).unwrap();
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
        {
            let mut st = state.borrow_mut();
            st.current = Some(0);
            st.work.export_last_folder = Some(dir.clone());
        }
        app.invoke_export();
        let set = state.borrow().work.exporting.clone().expect("the set");
        assert_eq!(set.total, 1);
        // Taken off the worker's own answers: the frame was renamed and
        // went without a look.
        let source = state.borrow().files[0].clone();
        let path = dir.join("IMG_0000-1.jpg");
        let note = "IMG_0000.jpg was there already: wrote IMG_0000-1.jpg";
        let left_out = vec!["look Faded: not on this machine".to_string()];
        let done = |done| Outcome::SetFrameDone {
            set: set.clone(),
            index: 0,
            source: source.clone(),
            edit: Edit::default(),
            version: None,
            done,
        };
        deliver(
            &app,
            done(queue::Done::Exported {
                path: path.clone(),
                seconds: 1.25,
                note: Some(note.into()),
                left_out: left_out.clone(),
            }),
        );
        let tally = queue::Tally {
            exported: 1,
            ..queue::Tally::default()
        };
        deliver(
            &app,
            Outcome::SetDone {
                set: set.clone(),
                tally,
            },
        );
        let line = app.get_status().to_string();
        assert_eq!(line, exported_line(&path, 1.25, Some(note), &left_out));
        assert!(
            line.starts_with("exported IMG_0000-1.jpg in 1.25 s"),
            "{line}"
        );
        assert!(line.contains("was there already"), "{line}");
        assert!(
            line.contains(", without one thing the edit names"),
            "{line}"
        );
        assert!(state.borrow().work.export_one_line.is_none());
        assert!(state.borrow().work.exporting.is_none());

        // Skipped under Skip: the file's own line too, not a count of
        // nothing.
        app.invoke_export();
        let set = state.borrow().work.exporting.clone().expect("the set");
        let done = |done| Outcome::SetFrameDone {
            set: set.clone(),
            index: 0,
            source: source.clone(),
            edit: Edit::default(),
            version: None,
            done,
        };
        deliver(
            &app,
            done(queue::Done::Skipped {
                path: dir.join("IMG_0000.jpg"),
            }),
        );
        let tally = queue::Tally {
            skipped: 1,
            ..queue::Tally::default()
        };
        deliver(&app, Outcome::SetDone { set, tally });
        assert_eq!(
            app.get_status(),
            "IMG_0000.jpg is there already: nothing exported"
        );

        // Two frames: the count, as before.
        state.borrow_mut().views.picked = vec![0, 1];
        app.invoke_export();
        let set = state
            .borrow()
            .work
            .exporting
            .clone()
            .expect("the set of two");
        assert_eq!(set.total, 2);
        let tally = queue::Tally {
            exported: 2,
            ..queue::Tally::default()
        };
        deliver(&app, Outcome::SetDone { set, tally });
        assert!(
            app.get_status().starts_with("exported 2 frames"),
            "{}",
            app.get_status()
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// A look at the folder that has not answered (a mount that keeps
    /// it waiting) holds nothing up: Choose still opens, and a second
    /// Export overtakes it; the first look's answer, come late, starts
    /// nothing, and only the last one's exports.
    #[test]
    fn a_look_at_the_folder_that_is_slow_to_answer_holds_nothing_up() {
        let app = crate::testing::window(1);
        let dir = crate::testing::scratch_dir("export-slow-look");
        let (first, second) = (dir.join("first"), dir.join("second"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        {
            let mut st = state.borrow_mut();
            st.current = Some(0);
            st.work.export_last_folder = Some(first.clone());
        }
        HELD.with(|h| *h.borrow_mut() = Some(Vec::new()));
        app.invoke_export();
        assert!(state.borrow().work.exporting.is_none(), "still looking");
        assert!(!state.borrow().work.export_choosing, "nothing held up");

        // Choose works meanwhile, and overtakes the look.
        CHOSEN.with(|c| *c.borrow_mut() = Some(Some(second.clone())));
        app.invoke_export_folder_choose();
        assert_eq!(
            state.borrow().work.export_last_folder.as_deref(),
            Some(second.as_path())
        );
        assert_eq!(
            app.get_status(),
            format!(
                "exports go to {}; the export waiting on {} was not started",
                second.display(),
                first.display()
            )
        );
        // Export again: a second look, at the new folder.
        app.invoke_export();
        let held = HELD.with(|h| h.borrow_mut().take()).expect("held");
        assert_eq!(held.len(), 2);
        let mut held = held.into_iter();
        // The first answers late: dropped.
        (held.next().unwrap())(&app);
        assert!(
            state.borrow().work.exporting.is_none(),
            "the late answer starts nothing"
        );
        // The second: the export, into the folder chosen.
        (held.next().unwrap())(&app);
        let set = state.borrow().work.exporting.clone().expect("the set");
        assert_eq!(set.folder.as_deref(), Some(second.as_path()));
        set.cancel();

        // A look that answers with an export under way since (the
        // queue's, say): not started, and said.
        HELD.with(|h| *h.borrow_mut() = Some(Vec::new()));
        state.borrow_mut().work.exporting = None;
        app.invoke_export();
        state.borrow_mut().work.exporting = Some(set);
        let held = HELD.with(|h| h.borrow_mut().take()).expect("held");
        for answer in held {
            answer(&app);
        }
        assert_eq!(
            app.get_status(),
            "not exported: another export is under way"
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// The Folder row follows the frame, not its name: two frames of one
    /// name in two folders show two folders.
    #[test]
    fn the_folder_row_follows_the_frame_not_its_name() {
        let app = crate::testing::window(2);
        let files = vec![
            PathBuf::from("/nowhere/card-one/IMG_0001.CR3"),
            PathBuf::from("/nowhere/card-two/IMG_0001.CR3"),
        ];
        let (_state, _worker) = crate::testing::state_for(&app, files);
        app.invoke_select(0);
        app.set_export_open(true);
        let count = |label: &str| crate::testing::count_labeled(&app, label);
        assert_eq!(count("/nowhere/card-one"), 1);
        app.invoke_select(1);
        assert_eq!(app.get_file_name(), "IMG_0001.CR3");
        assert_eq!(count("/nowhere/card-two"), 1);
        assert_eq!(count("/nowhere/card-one"), 0);
    }

    /// A probe name taken already is passed over, never read as the
    /// folder taking files, and no probe is left behind.
    #[test]
    fn the_folder_check_leaves_nothing_and_assumes_nothing() {
        let dir = crate::testing::scratch_dir("export-probe");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(folder_problem(&dir), None);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing left");
        // The first name taken, by a check a crash left: the second.
        let left = dir.join("left-by-a-crash");
        std::fs::write(&left, b"kept").unwrap();
        let names = |attempt: u32| match attempt {
            0 => "left-by-a-crash".to_string(),
            _ => "second".to_string(),
        };
        assert_eq!(folder_problem_probing(&dir, names), None);
        assert!(!dir.join("second").exists(), "removed");
        // Every name taken: not read as the folder taking files.
        assert_eq!(
            folder_problem_probing(&dir, |_| "left-by-a-crash".to_string()),
            Some(format!("{} cannot be written to", dir.display()))
        );
        // And the file that was there is left as it was.
        assert_eq!(std::fs::read(&left).unwrap(), b"kept");
        crate::testing::remove_dir_retry(&dir);
    }
}
