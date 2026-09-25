use crate::panel::assets::show_presets;
use crate::panel::browser::{
    grid_filled, load_sidecars, open_folder, open_paths, rebuild_browser, row_of, show_thumb,
    thumb_for, time_select,
};
use crate::panel::color::{preview_white, white_key};
use crate::panel::cull::{cull_frame, develop_landed, show_filter, standing_in};
use crate::panel::curve::{PARAMETRIC, draw_curve};
use crate::panel::deliver::{chosen_preset, deliver, read_sheet, show_presets_picker, show_sheet};
use crate::panel::edit::{read_edit, read_folds, save_edit, show_folds, time_sharpen};
use crate::panel::mask::{ask_for, bake_locals};
use crate::panel::retouch::patch_outlines;
use crate::panel::viewport::{
    ViewMap, display_key, effective_zoom, proof_settings, schedule_snapshot, source_to_view,
    sync_display, write_screenshot, zoom_label,
};
use crate::*;

pub(crate) fn main() -> Result<std::process::ExitCode> {
    let cli = Cli::parse();
    // Before anything is said. The line is for the person at the
    // terminal, not the log, which knows where it is.
    if let Some(log) = log::start(cli.verbose) {
        eprintln!("log: {}", log.display());
    }
    let remembered = settings::Settings::load();
    match settings::path() {
        Some(p) => tracing::info!("settings: {}", p.display()),
        None => tracing::warn!("settings: no configuration directory; nothing is remembered"),
    }
    let last_file = (!remembered.last_file.is_empty())
        .then(|| PathBuf::from(&remembered.last_file))
        .filter(|p| p.is_file());

    // The files to open, and which of them to start on. The command
    // line's path, a file or a directory, as before; without one, the
    // last file's folder when the settings still find it there,
    // otherwise nothing yet: the desktop's folder chooser is asked
    // for one once the window is up, and if that is canceled the
    // empty editor is left showing, its Open folder button waiting.
    let (files, initial_select): (Vec<PathBuf>, Option<usize>) = match &cli.path {
        Some(path) => {
            let files = files::list_files(path)?;
            anyhow::ensure!(!files.is_empty(), "no RAW files at {}", path.display());
            let select = if path.is_dir() {
                files::select_index(&files, last_file.as_deref())
            } else {
                0
            };
            (files, Some(select))
        }
        None => {
            let from_last = last_file
                .as_deref()
                .and_then(Path::parent)
                .and_then(|dir| files::list_files(dir).ok())
                .filter(|files| !files.is_empty());
            match from_last {
                Some(files) => {
                    let select = files::select_index(&files, last_file.as_deref());
                    (files, Some(select))
                }
                None => (Vec::new(), None),
            }
        }
    };

    let mut settings = WGPUSettings::default();
    // Slint asks for downlevel limits, which allow no storage buffers;
    // the histogram needs one. Desktop defaults, and 45 MP frames are
    // 8192 wide and more, exactly the default texture limit.
    settings.device_required_limits = wgpu::Limits {
        max_texture_dimension_2d: 16384,
        ..wgpu::Limits::default()
    };
    slint::BackendSelector::new()
        .require_wgpu_30(WGPUConfiguration::Automatic(settings))
        .select()
        .context("selecting Slint's wgpu backend")?;
    // The event loop is built now and not yet running: the moment to
    // hear the Finder, whose launch event comes as the loop starts.
    #[cfg(target_os = "macos")]
    finder::install();

    let app = App::new()?;
    // Which curve the panel shows at the start, for a dump of its
    // picture with `GREYCARD_UI_CURVE`.
    if let Ok(name) = std::env::var("GREYCARD_UI_CURVE_CHANNEL") {
        app.set_curve_channel(name.into());
    }
    if let Some(tab) = &cli.tab {
        app.set_panel_tab(tab.as_str().into());
    }
    app.set_demosaics(ModelRc::new(VecModel::from(
        Demosaic::ALL
            .iter()
            .map(|d| slint::SharedString::from(d.name()))
            .collect::<Vec<_>>(),
    )));
    app.set_denoise_tiers(ModelRc::new(VecModel::from(
        Learned::ALL
            .iter()
            .map(|t| slint::SharedString::from(t.name()))
            .collect::<Vec<_>>(),
    )));
    // What the last run left, unless the command line says otherwise.
    let (sheet, preset) = opening_sheet(&cli, &remembered)?;
    show_sheet(&app, &sheet);
    show_presets_picker(&app, &remembered.export_presets, preset.as_deref());
    show_folds(&app, &remembered.collapsed);
    app.set_scope(opening_scope(&cli, &remembered).name().into());
    app.set_show_sharpen_mask(cli.sharpen_mask);
    app.set_warn_shadows(cli.clipping || remembered.warn_shadows);
    app.set_warn_highlights(cli.clipping || remembered.warn_highlights);
    {
        let key = cli.proof.as_deref().unwrap_or(&remembered.proof_profile);
        let (choice, file) = match display::ProofProfile::from_key(key) {
            Some(display::ProofProfile::Space(s)) => (s.name().to_string(), String::new()),
            Some(display::ProofProfile::File(p)) => {
                ("File".to_string(), p.to_string_lossy().into_owned())
            }
            None => (export::Space::Srgb.name().to_string(), String::new()),
        };
        app.set_proof_profile(choice.as_str().into());
        app.set_proof_file(file.as_str().into());
        let intent = cli
            .proof_intent
            .as_deref()
            .and_then(display::ProofIntent::from_name)
            .or_else(|| display::ProofIntent::from_name(&remembered.proof_intent))
            .unwrap_or_default();
        app.set_proof_intent(intent.name().into());
        app.set_gamut_warning(cli.gamut_warning || remembered.gamut_warning);
        app.set_proofing(cli.proof.is_some());
    }
    // The monitor's profile, the flags' to open with over the
    // remembered one, and the monitors colord offers for it.
    let monitors = {
        let key = if cli.no_display_profile {
            display::MonitorProfile::Srgb.key()
        } else if let Some(path) = &cli.display_profile {
            path.to_string_lossy().into_owned()
        } else {
            remembered.display_profile.clone()
        };
        let (choice, file) = if key == SYSTEM {
            (SYSTEM.to_string(), String::new())
        } else {
            match display::MonitorProfile::from_key(&key) {
                Some(display::MonitorProfile::File(p)) => {
                    (FILE.to_string(), p.to_string_lossy().into_owned())
                }
                Some(standard) => (standard.name().to_string(), String::new()),
                None => (SYSTEM.to_string(), String::new()),
            }
        };
        app.set_display_profile(choice.as_str().into());
        app.set_display_file(file.as_str().into());
        let monitors = display::colord_monitors().unwrap_or_else(|e| {
            tracing::warn!("colord: {e:#}; no display profile from it");
            Vec::new()
        });
        let known: Vec<slint::SharedString> = monitors
            .iter()
            .filter(|m| m.profile.is_some())
            .map(|m| m.model.as_str().into())
            .collect();
        let picked = known
            .iter()
            .find(|m| m.as_str() == remembered.display_monitor)
            .or(known.first())
            .cloned()
            .unwrap_or_default();
        app.set_display_monitor(picked);
        app.set_monitors(ModelRc::new(VecModel::from(known)));
        monitors
    };
    app.set_canvas_choice(render::canvas_choice(&remembered.canvas_color));
    if remembered.curve_mode == PARAMETRIC {
        app.set_curve_mode(PARAMETRIC.into());
    }
    app.set_scopes(ModelRc::new(VecModel::from(
        scope::Scope::ALL
            .iter()
            .map(|s| slint::SharedString::from(s.name()))
            .collect::<Vec<_>>(),
    )));
    // Each file's edit, and its meta, from its sidecar when there is
    // one. Read before the strip is filled so a folder culled last
    // week opens with its badges on.
    let (mut sidecars, mut seed_blend) = load_sidecars(&files, !cli.no_sidecars);
    let thumbs = Rc::new(VecModel::<Thumb>::default());
    for (f, sidecar) in files.iter().zip(&sidecars) {
        thumbs.push(thumb_for(f, &sidecar.meta));
    }
    app.set_thumbs(ModelRc::from(thumbs.clone()));
    // The grid's geometry, from one place: the layout in the .slint
    // file and the arithmetic in `grid` read the same numbers.
    app.set_grid_gap(grid::GAP);
    app.set_grid_pad(grid::PAD);
    app.set_grid_label(grid::LABEL);
    app.set_meta_stars(meta::STARS as i32);
    app.set_grid_cell(grid::clamp_cell(
        cli.grid_cell.unwrap_or(remembered.grid_cell),
    ));
    app.set_grid_open(cli.grid);
    app.set_panels_hidden(cli.hide_panels);
    // The three names the flag's old three-way Show went by. The
    // chips say more than they can, but a flag on a command line
    // wants one word, and a script written against the old one still
    // means what it meant. Anything else is the filter language: its
    // facet terms wait for the index to name the chips they mean,
    // and the rest go in the text field as if typed there.
    let (filter, facets_wanted) = match cli.filter.as_deref() {
        None => (filter::Filter::default(), Vec::new()),
        Some(name) => match filter::Filter::from_name(name) {
            Some(found) => (found, Vec::new()),
            // One of the three names in the wrong case is a mistyped
            // name, as it always was, and not a word to look for.
            None if filter::Filter::NAMES
                .iter()
                .any(|n| n.eq_ignore_ascii_case(name)) =>
            {
                tracing::warn!("--filter {name}: want All, Picks or \"No rejects\"; showing all");
                (filter::Filter::default(), Vec::new())
            }
            None => {
                let (text, wanted) = filter::from_cli(name);
                let filter = filter::Filter {
                    text,
                    ..filter::Filter::default()
                };
                for e in filter.typed().errors {
                    tracing::warn!("--filter: {e}");
                }
                (filter, wanted)
            }
        },
    };
    let filtering = !filter.is_empty();
    app.set_filter_text(filter.text.clone().into());
    app.set_reject_count(
        sidecars
            .iter()
            .filter(|s| s.meta.flag == meta::Flag::Reject)
            .count() as i32,
    );

    let preset_store = preset::Store::user();
    // The film presets this build ships, put there the first time
    // there is a directory to put them in (`Store::seed`).
    if let Some(store) = &preset_store {
        store.seed();
    }
    let placement = if cli.sidecar_folder || remembered.sidecars_in_folder {
        greycard_edit::Placement::Folder
    } else {
        greycard_edit::Placement::Beside
    };
    if let (Some(name), Some(i)) = (&cli.preset, initial_select) {
        let found = preset_store.as_ref().and_then(|s| s.find(name));
        match found {
            Some(entry) => preset_at_start(
                &mut sidecars[i],
                &mut seed_blend[i],
                &files[i],
                &entry.preset,
                (!cli.no_sidecars).then_some(placement),
            ),
            None => anyhow::bail!("no preset called {name}"),
        }
    }

    let export_into_folder = cli
        .export
        .as_deref()
        .is_some_and(|p| export_folder(p, !cli.also.is_empty()));
    check_also(&cli, export_into_folder, files.len())?;
    let state = Rc::new(RefCell::new(State {
        sidecars,
        seed_blend,
        write_sidecars: !cli.no_sidecars,
        xmp_sidecars: cli.xmp_sidecars || remembered.xmp_sidecars,
        placement,
        zoom: cli.zoom.max(0.0),
        scope: opening_scope(&cli, &remembered),
        show_mask: cli.show_mask.and_then(|i| i.checked_sub(1)),
        show_patch: cli.patch.and_then(|i| i.checked_sub(1)),
        store: greycard_ai::Store::user().ok(),
        lenses_declined: remembered.lenses_declined,
        monitors,
        screenshot: cli.screenshot.clone(),
        snapshot: cli.snapshot.clone(),
        panel_scroll: cli.panel_scroll,
        snapshot_shown: cli
            .sheet
            .or(cli.tool)
            .or(cli.menu.map(crate::panel::viewport::Shown::Menu)),
        export_then_quit: cli.export.clone(),
        export_into_folder,
        export_presets: remembered.export_presets.clone(),
        settings_file: if cli.snapshot.is_some() || cli.screenshot.is_some() || cli.export.is_some()
        {
            None
        } else {
            settings::path()
        },
        presets: preset_store.as_ref().map(|s| s.list()).unwrap_or_default(),
        preset_store,
        batch: cli.snapshot.is_some() || cli.screenshot.is_some() || cli.export.is_some(),
        time_sharpen: cli.time_sharpen.map(|n| (n, None, Vec::new())),
        time_cull: cli.time_cull.map(|n| (n, Vec::new())),
        time_select: cli.time_select.map(|n| (n, Vec::new(), Vec::new())),
        snapshot_placeholder: cli.snapshot_placeholder,
        cull_develop: cli.cull_develop,
        turn_at_start: cli.turn.filter(|q| q.rem_euclid(4) != 0),
        awaiting_turn: cli.turn.is_some_and(|q| q.rem_euclid(4) != 0),
        ask_rejects: cli.ask_rejects || cli.move_rejects,
        also_at_start: cli.also.clone(),
        move_rejects: cli.move_rejects,
        cull_at_start: (cli.cull
            || cli.cull_compare.is_some()
            || cli.time_cull.is_some()
            || cli.cull_develop
            || cli.ask_rejects
            || cli.move_rejects)
            .then(|| cli.cull_compare.unwrap_or(1)),
        filter,
        awaiting_index: !facets_wanted.is_empty(),
        facets_wanted,
        ..State::empty(files.clone(), &app)
    }));
    app.set_compare_tiles(ModelRc::from(state.borrow().compare_tiles.clone()));
    // The settings sheet's two, as this run has them: the flags'
    // over the file's.
    app.set_sidecar_placement(crate::panel::prefs::placement_name(state.borrow().placement).into());
    app.set_xmp_sidecars(state.borrow().xmp_sidecars);
    // The browser's list under the filter asked for, the files it
    // hides left out of the strip and the grid; and the chips, which
    // count the folder whether or not anything is filtered.
    if filtering {
        rebuild_browser(&mut state.borrow_mut(), &app);
    } else {
        show_filter(&state.borrow(), &app);
    }
    app.set_preset_section_names(ModelRc::new(VecModel::from(
        Section::ALL
            .iter()
            .map(|s| slint::SharedString::from(s.title()))
            .collect::<Vec<_>>(),
    )));
    app.set_preset_section_on(ModelRc::from(state.borrow().preset_sections.clone()));
    show_presets(&state.borrow(), &app);

    // Results come back to the UI thread through the event loop.
    let worker = {
        let app_weak = app.as_weak();
        Worker::new(move |outcome| {
            let app_weak = app_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = app_weak.upgrade() {
                    deliver(&app, outcome);
                }
            });
        })
    };
    let worker = Rc::new(worker);
    // The thumbnail cache, before the first thumbnail is asked for. A
    // cap of zero is the cache off, and the cache is still handed over
    // so the Settings sheet can show and clear what an earlier run
    // left in it.
    match greycard_library::Thumbs::user(crate::panel::prefs::cap_bytes(remembered.thumb_cache_mb))
    {
        Ok(cache) => {
            tracing::info!(
                "thumbnail cache {} ({})",
                cache.root().display(),
                match remembered.thumb_cache_mb {
                    0 => "off".to_string(),
                    mb => format!("cap {mb} MB"),
                }
            );
            worker.set_thumb_cache(Some(cache));
        }
        Err(e) => tracing::warn!("no thumbnail cache: {e}"),
    }
    // The library index, on a thread of its own: the folder open is
    // indexed there, so the first frame never waits for a pass, and
    // the facets fill in as it goes.
    match cli
        .library
        .clone()
        .or_else(greycard_library::Library::user_path)
    {
        Some(path) => {
            let app_weak = app.as_weak();
            let started = crate::library::Indexer::start(path, move |told| {
                let app_weak = app_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = app_weak.upgrade() {
                        crate::library::told(&app, told);
                    }
                });
            });
            let mut st = state.borrow_mut();
            match started {
                Ok(indexer) => {
                    st.index = Some(indexer);
                    crate::library::index_open_folder(&mut st);
                }
                Err(e) => {
                    tracing::warn!("no library index: the indexer did not start: {e}");
                    st.awaiting_index = false;
                }
            }
            crate::panel::cull::show_filter(&st, &app);
        }
        None => {
            tracing::warn!("no data directory for the library index; the facets are off");
            state.borrow_mut().awaiting_index = false;
        }
    }
    state.borrow_mut().thumb_run = Some(crate::panel::browser::ThumbRun::new(files.len()));
    for (i, f) in files.iter().enumerate() {
        worker.send(Job::Thumbnail {
            index: i,
            path: f.clone(),
        });
    }

    app.set_mask_handles(ModelRc::from(state.borrow().mask_handles.clone()));
    app.set_mask_boxes(ModelRc::from(state.borrow().mask_boxes.clone()));
    app.set_mask_picks(ModelRc::from(state.borrow().mask_picks.clone()));
    app.set_patch_handles(ModelRc::from(state.borrow().patch_handles.clone()));
    app.set_guide_kept(ModelRc::from(state.borrow().guide_kept.clone()));
    install_callbacks(&app, state.clone(), worker.clone());

    // The viewport is rendered inside Slint's frame, on its device.
    {
        let timing = std::env::var_os("GREYCARD_UI_TIMING").is_some();
        let cpu_ops = cli.cpu_ops;
        let state = state.clone();
        let app_weak = app.as_weak();
        app.window()
            .set_rendering_notifier(move |phase, api| match phase {
                slint::RenderingState::RenderingSetup => {
                    if let slint::GraphicsAPI::WGPU30 { device, queue, .. } = api {
                        // Once per rendering setup, which is once per
                        // device: the line a "black viewport" report needs.
                        // It is kept for a report's GPU field too.
                        let gpu = report::gpu_line(&device.adapter_info());
                        tracing::info!("gpu: {gpu}");
                        let mut st = state.borrow_mut();
                        st.gpu = Some(gpu);
                        st.renderer = Some(Renderer::new(device, queue));
                        st.lut_for = None;
                        // The engine's GPU ops run on the same device,
                        // so what they leave is the viewport's. Handed
                        // over before the first file opens, so its
                        // develop runs there too.
                        if cpu_ops {
                            tracing::info!("engine ops on the CPU (--cpu-ops)");
                        } else {
                            WORKER.with(|w| {
                                if let Some(w) = &*w.borrow() {
                                    w.set_gpu(device, queue);
                                }
                            });
                        }
                    } else {
                        tracing::error!("gpu: not wgpu 30 ({api:?}); the viewport cannot render");
                        state.borrow_mut().gpu = Some(format!("not wgpu 30 ({api:?})"));
                    }
                    // The file to start on, now that the worker knows
                    // where to develop. From the event loop rather than
                    // here: the select borrows the state this holds.
                    // As a row of the browser, which may be filtered.
                    // A file the filter hides opens the nearest one
                    // shown; a filter that hides them all says so, and
                    // ends a batch run that would wait for a picture.
                    // What the Finder sent before now, which a
                    // launch by double-click did, over the folder the
                    // settings remembered; and from here on, whatever
                    // it sends is opened as it comes.
                    let from_finder = {
                        let app_weak = app_weak.clone();
                        finder::ready(move |paths| {
                            let (Some(app), Some(state), Some(worker)) = (
                                app_weak.upgrade(),
                                STATE.with(|s| s.borrow().clone()),
                                WORKER.with(|w| w.borrow().clone()),
                            ) else {
                                return;
                            };
                            open_paths(&state, &app, &worker, &paths);
                        })
                    };
                    let row = {
                        let mut st = state.borrow_mut();
                        let at_start = st.select_at_start.take();
                        at_start
                            .filter(|_| from_finder.is_empty())
                            .map(|i| row_of(&st, i).or_else(|| cull::nearest_row(&st.shown, i)))
                    };
                    if !from_finder.is_empty() {
                        let app_weak = app_weak.clone();
                        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                            let (Some(app), Some(state), Some(worker)) = (
                                app_weak.upgrade(),
                                STATE.with(|s| s.borrow().clone()),
                                WORKER.with(|w| w.borrow().clone()),
                            ) else {
                                return;
                            };
                            open_paths(&state, &app, &worker, &from_finder);
                        });
                    }
                    match row {
                        Some(Some(row)) => {
                            let app_weak = app_weak.clone();
                            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                                if let Some(app) = app_weak.upgrade() {
                                    app.invoke_select(row as i32);
                                    // `--also`: Ctrl+clicks on those rows.
                                    let state = STATE.with(|s| s.borrow().clone());
                                    let (also, rows) = state
                                        .map(|s| {
                                            let mut st = s.borrow_mut();
                                            (std::mem::take(&mut st.also_at_start), st.shown.len())
                                        })
                                        .unwrap_or_default();
                                    for r in also {
                                        // A row the filter left out is
                                        // said, not passed over quietly.
                                        if r >= rows {
                                            tracing::warn!(
                                                "--also {r}: the browser shows {rows} rows (from 0); not in the set"
                                            );
                                            continue;
                                        }
                                        app.invoke_frame_clicked(r as i32, true, false);
                                    }
                                }
                            });
                        }
                        Some(None) => {
                            if let Some(app) = app_weak.upgrade() {
                                app.set_status(filter::NOTHING_SHOWN.into());
                            }
                            tracing::warn!("no frames pass the filter");
                            let mut st = state.borrow_mut();
                            if st.batch {
                                st.failed = true;
                                let _ = slint::quit_event_loop();
                            }
                        }
                        None => {}
                    }
                }
                slint::RenderingState::BeforeRendering => {
                    let Some(app) = app_weak.upgrade() else {
                        return;
                    };
                    let started = std::time::Instant::now();
                    let mut st = state.borrow_mut();
                    let st = &mut *st;
                    let pending = st.pending.take();
                    // The develop the camera's picture was standing in
                    // for has landed — that one, or a later one of the
                    // same frame — so ours takes its place, now, and
                    // never the other way about. A develop asked for
                    // before the frame on the panel was chosen is a
                    // picture of another frame and leaves it standing.
                    if let Some(landed) = &pending {
                        develop_landed(st, &app, landed.generation);
                    }
                    // Culling, or a camera picture standing in for a
                    // develop: neither draws the develop's frame below.
                    if st.cull.is_some() || standing_in(st).is_some() {
                        if let Some(landed) = &pending
                            && let Some(renderer) = st.renderer.as_mut()
                        {
                            match &landed.image {
                                Developed::Halves(halves) => renderer.upload(halves),
                                Developed::Texture(texture) => renderer.set_source(texture.clone()),
                            }
                            renderer.set_guide(&landed.guide);
                        }
                        cull_frame(st, &app, &state);
                        return;
                    }
                    if let Some(landed) = &pending {
                        let (w, h) = (landed.image.width(), landed.image.height());
                        st.source_size = (w, h);
                        app.set_shot_size(size_text(w, h).into());
                        st.base_white = Some(landed.white);
                        st.white_cache = None;
                    }
                    // The frame on show: the crop, or the turned source's
                    // bounds while cropping. A change of size re-centers.
                    // A row under the pointer shows its state instead.
                    let panel = read_edit(&app, &st.edit, st.target);
                    let edit = st
                        .held
                        .clone()
                        .or_else(|| st.peek.clone())
                        .unwrap_or_else(|| panel.clone());
                    // The chosen patch's handles: its center, and its source.
                    let selected = usize::try_from(app.get_patch())
                        .ok()
                        .and_then(|i| edit.retouch.patches.get(i));
                    let handles: Vec<Pt> = match (st.retouching, selected) {
                        (Some(_), Some(p)) => {
                            let c = p.center();
                            let mut v = vec![source_to_view(st, &app, c[0], c[1])];
                            if let Some(s) = p.source {
                                v.push(source_to_view(st, &app, c[0] + s[0], c[1] + s[1]));
                            }
                            v.into_iter().map(|(x, y)| Pt { x, y }).collect()
                        }
                        _ => Vec::new(),
                    };
                    sync_rows(&st.patch_handles, handles);
                    // The guide's kept stroke follows the view: it is
                    // held in source pixels and mapped here, so a zoom
                    // or a pan between the two strokes moves the line
                    // with the picture.
                    let kept: Vec<Pt> = match st.guiding.first {
                        Some(ends) if !app.get_guide_mode().is_empty() => {
                            let sw = st.source_size.0 as f32;
                            ends.iter()
                                .map(|&(x, y)| {
                                    let (vx, vy) = source_to_view(st, &app, x / sw, y / sw);
                                    Pt { x: vx, y: vy }
                                })
                                .collect()
                        }
                        _ => Vec::new(),
                    };
                    sync_rows(&st.guide_kept, kept);
                    // The filmstrip's picture of this file follows its turns.
                    if let Some(i) = st.current
                        && st.thumb_base.get(i).is_some_and(|b| b.is_some())
                    {
                        let (turns, flip) = panel.geometry.shown_turns(st.sidecars[i].turn);
                        if st.thumb_shown[i] != Some((turns, flip)) {
                            show_thumb(st, &app, i, turns, flip);
                        }
                    }
                    // The picture on screen: this frame's develop
                    // once it has landed, and until then the one
                    // still on the GPU, which the edit above is the
                    // held one for. A select clears this frame's
                    // size, and reading it here would draw whatever
                    // is on screen into a one-pixel frame — the
                    // canvas over the whole viewport.
                    let (dw, dh) = placeholder::drawn_source(st.source_size, st.shown_size);
                    let (sw, sh) = (dw as f32, dh as f32);
                    let frame = if app.get_crop_mode() {
                        edit.geometry.bounds(sw, sh)
                    } else {
                        edit.geometry.frame(sw, sh)
                    };
                    let frame_size = (frame.size.0 as u32, frame.size.1 as u32);
                    // The picture's size as an export writes it, for
                    // the sheet's arithmetic.
                    let exported = panel.geometry.frame(sw, sh).size;
                    app.set_frame_width(exported.0 as i32);
                    app.set_frame_height(exported.1 as i32);
                    if frame_size != st.image_size {
                        st.image_size = frame_size;
                        st.center = (frame.size.0 / 2.0, frame.size.1 / 2.0);
                    }
                    let white_key = match &st.peek {
                        Some(p) => white_key(&p.white_balance),
                        None => (app.get_as_shot(), app.get_temperature(), app.get_tint()),
                    };
                    // A held picture was developed at its own white
                    // balance; the open frame is already the next one.
                    let white = if st.held.is_some() {
                        WhiteBase::IDENTITY.matrix
                    } else {
                        preview_white(st, white_key)
                    };
                    let (vw, vh) = (
                        app.get_view_width().max(1) as u32,
                        app.get_view_height().max(1) as u32,
                    );
                    let zoom = effective_zoom(st, vw, vh);
                    if st.placing.is_none()
                        && let Some(was) = st.show_mask_kept.take()
                    {
                        app.set_show_mask(was);
                    }
                    // The readout, from the one place the zoom is settled
                    // per frame, so it cannot go stale.
                    let label = zoom_label(st.zoom);
                    if app.get_zoom_text() != label {
                        app.set_zoom_text(label);
                    }
                    // The chosen shape's handles and outline on the view.
                    let chosen = st
                        .target
                        .filter(|_| !app.get_crop_mode() && st.placing.is_none())
                        .and_then(|i| edit.adjustments.get(i))
                        .and_then(|a| a.mask.components.get(app.get_component().max(0) as usize))
                        .cloned();
                    // The brush's size on the view, per unit of the width.
                    app.set_brush_scale(sw * zoom / app.window().scale_factor());
                    // The chosen patch's shape on the view, while the
                    // Retouch tab shows: its edge, and where its
                    // feather begins.
                    let (edge, feather) =
                        match selected.filter(|_| app.get_panel_tab() == "Retouch") {
                            Some(p) => {
                                let map = ViewMap::of(st, &app);
                                patch_outlines(&mut st.patch_shape, p, &map)
                            }
                            None => (String::new(), String::new()),
                        };
                    if app.get_patch_outline() != edge {
                        app.set_patch_outline(edge.into());
                    }
                    if app.get_patch_feather_outline() != feather {
                        app.set_patch_feather_outline(feather.into());
                    }
                    match &chosen {
                        Some(component) => {
                            let handles: Vec<Pt> = component
                                .shape
                                .handles()
                                .into_iter()
                                .map(|(u, v)| {
                                    let (x, y) = source_to_view(st, &app, u, v);
                                    Pt { x, y }
                                })
                                .collect();
                            match component.shape {
                                Shape::Linear { .. } => {
                                    let (dx, dy) =
                                        (handles[2].x - handles[1].x, handles[2].y - handles[1].y);
                                    app.set_mask_angle(dy.atan2(dx).to_degrees() + 90.0);
                                }
                                Shape::Radial { feather, .. } => {
                                    let (cx, cy) = (handles[0].x, handles[0].y);
                                    let (ax, ay) = (handles[1].x - cx, handles[1].y - cy);
                                    let (bx, by) = (handles[3].x - cx, handles[3].y - cy);
                                    app.set_mask_angle(ay.atan2(ax).to_degrees());
                                    app.set_mask_rx((ax * ax + ay * ay).sqrt());
                                    app.set_mask_ry((bx * bx + by * by).sqrt());
                                    app.set_mask_feather_scale(1.0 - feather.clamp(0.0, 1.0));
                                }
                                Shape::Brush { .. }
                                | Shape::Subject {}
                                | Shape::Object { .. }
                                | Shape::Luminance { .. }
                                | Shape::Color { .. }
                                | Shape::Unknown => {}
                            }
                            sync_rows(&st.mask_handles, handles);
                            app.set_mask_kind(component.shape.name().into());
                        }
                        None => app.set_mask_kind("".into()),
                    }
                    // An object's boxes and picks: the chosen one's,
                    // or, with the tool in hand, the one it adds to.
                    let object = match &st.placing {
                        Some(p) => p.component.and_then(|c| {
                            edit.adjustments
                                .get(p.index)
                                .and_then(|a| a.mask.components.get(c))
                        }),
                        None => chosen.as_ref(),
                    }
                    .and_then(|c| match &c.shape {
                        Shape::Object { picks, boxes } => Some((picks, boxes)),
                        _ => None,
                    });
                    let (boxes, picks) = match object {
                        Some((picks, boxes)) => (
                            boxes
                                .iter()
                                .map(|b| {
                                    let (x0, y0) = source_to_view(st, &app, b[0][0], b[0][1]);
                                    let (x1, y1) = source_to_view(st, &app, b[1][0], b[1][1]);
                                    MaskBox {
                                        x: x0.min(x1),
                                        y: y0.min(y1),
                                        w: (x1 - x0).abs(),
                                        h: (y1 - y0).abs(),
                                    }
                                })
                                .collect(),
                            picks
                                .iter()
                                .map(|k| {
                                    let (x, y) = source_to_view(st, &app, k.pos[0], k.pos[1]);
                                    MaskPick {
                                        x,
                                        y,
                                        positive: k.positive,
                                    }
                                })
                                .collect(),
                        ),
                        None => (Vec::new(), Vec::new()),
                    };
                    sync_rows(&st.mask_boxes, boxes);
                    sync_rows(&st.mask_picks, picks);
                    let st = &mut *st;
                    let (locals, wants) =
                        bake_locals(&edit, &mut st.rasters, &mut st.learned, sh / sw.max(1.0));
                    ask_for(st, &app, wants);
                    // A screenshot waits for the masks asked for.
                    // A snapshot waits for the develop; one of the
                    // grid waits for the pictures the grid shows, or
                    // it would catch a sheet of empty cells.
                    let quiet = st.asked.is_empty() && grid_filled(st, &app);
                    // A timed sharpen move is sent as its frame starts.
                    if pending.is_some() {
                        time_sharpen(st, &app);
                        // The frame that shows the frame the key
                        // landed on: the switch's time, for the log
                        // and `--time-select`.
                        if let Some(at) = st.selected_at.take() {
                            let ms = at.elapsed().as_secs_f64() * 1e3;
                            tracing::debug!("select to develop: {ms:.1} ms");
                            let row = st.current.and_then(|c| cull::row_of_shown(&st.shown, c));
                            let count = st.shown.len();
                            time_select(&mut st.time_select, &app, ms, row, count);
                        }
                        // `--snapshot-placeholder`: the capture is
                        // taken and the develop it stood in for has
                        // landed, so the run ends here rather than in
                        // the middle of that develop.
                        if st.snapshot_placeholder && st.snapshot.is_none() && st.batch {
                            let _ = slint::quit_event_loop();
                        }
                        // And, before it is taken: on to the next
                        // frame, from the event loop, so the capture
                        // is of its camera picture standing in for its
                        // develop rather than of this develop.
                        if st.snapshot_placeholder && st.snapshot.is_some() {
                            let app_weak = app.as_weak();
                            slint::Timer::single_shot(
                                std::time::Duration::from_millis(100),
                                move || {
                                    if let Some(app) = app_weak.upgrade() {
                                        app.invoke_step(1, false);
                                    }
                                },
                            );
                        }
                    }
                    let Some(renderer) = st.renderer.as_mut() else {
                        return;
                    };
                    if let Some(landed) = &pending {
                        match &landed.image {
                            Developed::Halves(halves) => renderer.upload(halves),
                            Developed::Texture(texture) => renderer.set_source(texture.clone()),
                        }
                        renderer.set_guide(&landed.guide);
                    }
                    sync_display(
                        &mut st.lut_for,
                        &mut st.encoded_lut_for,
                        &st.monitors,
                        &app,
                        renderer,
                    );
                    // The look table, read once per choice and handed
                    // to the renderer; the export resolves the same
                    // name through the same cache, so the two agree.
                    if st.look_for.as_ref() != Some(&edit.look_lut) {
                        st.look_for = Some(edit.look_lut.clone());
                        st.look = edit.look_lut.look();
                    }
                    renderer.set_look(st.look.as_ref());
                    let view = View {
                        zoom,
                        center: st.center,
                        light: edit.light.effective(),
                        mixer: edit.acting_mixer(),
                        color: edit.color,
                        bw: edit.bw,
                        tint: edit.tint,
                        matrix: edit.geometry.matrix(),
                        persp: edit.geometry.perspective(sw, sh),
                        plane: edit.geometry.plane_size(sw, sh),
                        cubic: edit.geometry.resamples(),
                        frame_origin: frame.origin,
                        frame_size: frame.size,
                        curves: edit.curves.bake_with(&edit.grading),
                        white,
                        locals,
                        vignette: edit.vignette,
                        grain: edit.grain,
                        warn: render::Warn {
                            shadows: app.get_warn_shadows(),
                            highlights: app.get_warn_highlights(),
                        },
                        show_mask: app.get_show_mask().then_some(st.target).flatten(),
                        show_sharpen: app.get_show_sharpen_mask() && app.get_sharpen(),
                        mask_alone: false,
                        canvas: render::canvas_rgb(app.get_canvas_choice()),
                        source: st.source,
                    };
                    if app.get_crop_mode() {
                        // The crop's rectangle in view pixels, for the overlay.
                        let scale = app.window().scale_factor();
                        let c = edit.geometry.effective_crop(sw, sh);
                        let (sw, sh) = edit.geometry.plane_size(sw, sh);
                        let to_view = |fx: f32, fy: f32| {
                            (
                                ((fx - frame.origin.0 - st.center.0) * zoom + vw as f32 / 2.0)
                                    / scale,
                                ((fy - frame.origin.1 - st.center.1) * zoom + vh as f32 / 2.0)
                                    / scale,
                            )
                        };
                        let (l, t) = to_view(c.x * sw, c.y * sh);
                        let (r, b) = to_view((c.x + c.w) * sw, (c.y + c.h) * sh);
                        app.set_crop_left(l);
                        app.set_crop_top(t);
                        app.set_crop_width(r - l);
                        app.set_crop_height(b - t);
                    }
                    // The navigator's rectangle: what the view shows of
                    // the frame, as fractions of it.
                    {
                        let (fw, fh) = (frame.size.0.max(1.0), frame.size.1.max(1.0));
                        let (shown_w, shown_h) = (vw as f32 / zoom, vh as f32 / zoom);
                        let partial = shown_w < fw - 0.5 || shown_h < fh - 0.5;
                        app.set_nav_partial(partial);
                        if partial {
                            let left = ((st.center.0 - shown_w / 2.0) / fw).clamp(0.0, 1.0);
                            let top = ((st.center.1 - shown_h / 2.0) / fh).clamp(0.0, 1.0);
                            let right = ((st.center.0 + shown_w / 2.0) / fw).clamp(0.0, 1.0);
                            let bottom = ((st.center.1 + shown_h / 2.0) / fh).clamp(0.0, 1.0);
                            app.set_nav_left(left);
                            app.set_nav_top(top);
                            app.set_nav_width(right - left);
                            app.set_nav_height(bottom - top);
                        }
                    }
                    let texture = renderer.render(vw, vh, &view);
                    let (bins, in_flight) = renderer.analyze(&view, st.scope);
                    let fresh = bins.map(|(_, b)| b.to_vec());
                    if let Some((taken, bins)) = bins {
                        app.set_scope_picture(scope::draw(taken, bins));
                    }
                    if let Some(picture) = renderer.take_navigator() {
                        if let Some(path) = std::env::var_os("GREYCARD_UI_NAVIGATOR")
                            && let Some(rgba) = picture.to_rgba8()
                            && let Some(img) = image::RgbaImage::from_raw(
                                rgba.width(),
                                rgba.height(),
                                rgba.as_bytes().to_vec(),
                            )
                            && let Err(e) = img.save(&path)
                        {
                            tracing::warn!("navigator {}: {e}", Path::new(&path).display());
                        }
                        // The navigator's height follows its picture's
                        // shape, and the panel below it with that; a
                        // peek from a row under the pointer leaves it
                        // be, or the rows would move out from under
                        // the pointer and the peek end.
                        if st.peek.is_none() {
                            app.set_navigator(picture);
                        }
                    }
                    if in_flight {
                        // The bins arrive a frame later; have that frame.
                        app.window().request_redraw();
                    }
                    // `--turn` has a key still to press, and the
                    // develop that fires it is this one: a capture
                    // taken now would be of the frame as it was, the
                    // way the culling path's `leave_next` guards it.
                    // A capture asked of a placeholder is never of a
                    // develop: the culling path takes that one.
                    let ready = renderer.has_image()
                        && quiet
                        && !st.awaiting_turn
                        && !st.awaiting_index
                        && !st.snapshot_placeholder;
                    schedule_snapshot(
                        &mut st.snapshot,
                        &mut st.panel_scroll,
                        &mut st.snapshot_shown,
                        &app,
                        &state,
                        ready,
                        true,
                    );
                    write_screenshot(
                        &mut st.screenshot,
                        &mut st.failed,
                        renderer,
                        &texture,
                        ready,
                    );
                    app.set_texture(slint::Image::try_from(texture).expect("the texture imports"));
                    if let Some(b) = fresh {
                        app.set_curve_image(draw_curve(&app, Some(b.as_slice())));
                        let clipping = scope::Clipping::of(&b);
                        let mark = |chans: [bool; 3]| {
                            let c = scope::Clipping::mark(chans).unwrap_or([0; 3]);
                            slint::Color::from_rgb_u8(c[0], c[1], c[2])
                        };
                        app.set_clip_shadows_lit(clipping.shadows.iter().any(|&c| c));
                        app.set_clip_shadows_mark(mark(clipping.shadows));
                        app.set_clip_highlights_lit(clipping.highlights.iter().any(|&c| c));
                        app.set_clip_highlights_mark(mark(clipping.highlights));
                        st.bins = Some(b);
                    }
                    // The variable is the gate; the line is kept at info so
                    // asking for it is enough to get it.
                    if timing {
                        tracing::info!(
                            "frame: {:.2} ms{}",
                            started.elapsed().as_secs_f64() * 1e3,
                            if pending.is_some() {
                                " (with upload)"
                            } else {
                                ""
                            }
                        );
                    }
                }
                slint::RenderingState::RenderingTeardown => {
                    state.borrow_mut().renderer = None;
                }
                _ => {}
            })
            .context("Slint's rendering notifier")?;
    }

    // Open the file to start on, if there is one yet.
    if let Some(i) = initial_select {
        if let Some(t) = cli.develop_temperature {
            state.borrow_mut().sidecars[i].current.white_balance = WhiteBalance::Custom {
                temperature: t as f64,
                tint: 0.0,
            };
        }
        // Into the file's edit, as the temperature is: the panel
        // takes the edit when the file arrives, and would drop a
        // value set on it now.
        if cli.exposure != 0.0 {
            state.borrow_mut().sidecars[i].current.light.exposure = cli.exposure;
        }
        state.borrow_mut().preview_temperature = cli.preview_temperature;
        // Opened from the rendering setup, once the worker has the
        // device, so the first develop takes the same path as the
        // rest of the session.
        state.borrow_mut().select_at_start = Some(i);
    } else {
        // Nothing to open yet: ask the desktop for a folder once the
        // event loop is running. Canceled, the empty editor stays up
        // with its Open folder button.
        let app_weak = app.as_weak();
        let start = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        // From a timer, which fires once the loop runs: a launch from
        // the Finder has handed over its files by then, and the
        // chooser is not wanted over them.
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if finder::arrived() {
                return;
            }
            export::choose_folder("Open a folder", start, move |chosen| {
                let app_weak = app_weak.clone();
                // The state and the worker through their thread locals:
                // this runs on the chooser's own thread, which an `Rc`
                // cannot cross.
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(app) = app_weak.upgrade() else {
                        return;
                    };
                    let (Some(state), Some(worker)) = (
                        STATE.with(|s| s.borrow().clone()),
                        WORKER.with(|w| w.borrow().clone()),
                    ) else {
                        return;
                    };
                    match chosen {
                        Ok(Some(dir)) => open_folder(&state, &app, &worker, &dir),
                        Ok(None) => {}
                        // In the window as well as in the log, in the
                        // Open folder button's words: the log is not
                        // where the empty editor's user is looking.
                        Err(e) => {
                            tracing::warn!("file chooser: {e:#}");
                            app.set_status("the desktop offered no file chooser".into());
                        }
                    }
                });
            });
        });
    }
    app.run()?;
    // The window is closed: the worker finishes what it is in the
    // middle of and puts its buffers down before the process goes.
    worker.stop();
    // And the index's two connections close, the indexer's pass
    // stopping at its next batch, so the write-ahead log and its
    // index are folded in and removed rather than left beside the
    // library.
    {
        let mut st = state.borrow_mut();
        st.index_reader = None;
        if let Some(indexer) = st.index.take() {
            indexer.stop(worker::LEAVING);
        }
    }

    // The panel's choices, for the next run. Not from a screenshot or
    // a batch export, which should leave the user's alone. The last
    // file is kept as whatever it already is: it is written as soon
    // as it develops, not gathered from the panel here.
    if cli.screenshot.is_none() && cli.snapshot.is_none() && cli.export.is_none() {
        let mut settings = remember(&app);
        settings.export_presets = state.borrow().export_presets.clone();
        let kept = settings::Settings::load();
        settings.last_file = kept.last_file;
        settings.xmp_sidecars = kept.xmp_sidecars;
        settings.sidecars_in_folder = kept.sidecars_in_folder;
        settings.lenses_declined = kept.lenses_declined;
        settings.thumb_cache_mb = kept.thumb_cache_mb;
        settings.save();
    }

    // The open file's edit, as left.
    let edit = {
        let st = state.borrow();
        read_edit(&app, &st.edit, st.target)
    };
    save_edit(&mut state.borrow_mut(), edit);
    // A batch run whose picture never developed, or whose export did
    // not happen, is a failure a script can see.
    Ok(if state.borrow().failed {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    })
}

/// The monitor's profile as the panel has it, and where it is from,
/// for the panel's note: colord's for the monitor named (the primary
/// when none is, or the name is stale), a standard, or a file.
/// `--preset`: lay `preset` over `file`'s edit as one step named for
/// it, and write the sidecar where `placement` says (`None`: the run
/// writes no sidecars). Written here and not left to the quit's
/// `save_edit`, which finds the panel equal to the step already
/// recorded and so writes nothing: the step would be lost on close.
///
/// A raw still waiting on its ISO's learned-denoiser blend (`seed`)
/// is given it first, as `panel::sync::lay_over_targets` gives a
/// target: the step makes the edit no longer the default, and once
/// written the next launch would take that to mean the blend was
/// seeded already. A preset that changes nothing records and writes
/// nothing.
pub(crate) fn preset_at_start(
    sidecar: &mut Sidecar,
    seed: &mut bool,
    file: &Path,
    preset: &Preset,
    placement: Option<greycard_edit::Placement>,
) {
    let applied = preset.applied(&sidecar.current);
    if applied == sidecar.current {
        return;
    }
    let mut over = sidecar.current.clone();
    if *seed
        && !greycard_core::picture::is_picture_path(file)
        && let Ok(p) = greycard_core::decode::probe_path(file)
    {
        over.noise.learned_strength = greycard_edit::Noise::blend_for_iso(p.iso);
        sidecar.current.noise.learned_strength = over.noise.learned_strength;
        *seed = false;
    }
    let label = greycard_edit::history::preset_label(&preset.name);
    if !sidecar.record_as(preset.applied(&over), Some(label)) {
        return;
    }
    if let Some(placement) = placement
        && let Err(e) = sidecar.save_in(file, placement)
    {
        tracing::warn!("{}: sidecar not saved: {e}", file.display());
    }
}

pub(crate) fn monitor_settings(
    app: &App,
    monitors: &[display::Monitor],
) -> (display::MonitorProfile, String) {
    use display::MonitorProfile;
    let choice = app.get_display_profile();
    match choice.as_str() {
        SYSTEM => {
            let wanted = app.get_display_monitor();
            let mut known = monitors.iter().filter(|m| m.profile.is_some());
            let picked = known
                .clone()
                .find(|m| m.model == wanted.as_str())
                .or_else(|| known.next());
            match picked {
                Some(m) => (
                    MonitorProfile::File(m.profile.clone().expect("filtered")),
                    format!("{}'s profile from colord", m.model),
                ),
                None => (
                    MonitorProfile::Srgb,
                    "colord knows no display profile".to_string(),
                ),
            }
        }
        FILE => {
            let file = app.get_display_file();
            if file.is_empty() {
                return (MonitorProfile::Srgb, "No profile chosen".to_string());
            }
            let path = PathBuf::from(file.as_str());
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.to_string());
            (MonitorProfile::File(path), name)
        }
        name => match MonitorProfile::STANDARD
            .into_iter()
            .find(|m| m.name() == name)
        {
            Some(standard) => (standard, name.to_string()),
            None => (MonitorProfile::Srgb, "sRGB".to_string()),
        },
    }
}

/// A line for the panel on what the monitor's profile does, and the
/// same to the log, where a headless run reads it.
pub(crate) fn monitor_note(monitor: &display::MonitorProfile, from: &str) -> String {
    if *monitor == display::MonitorProfile::Srgb {
        tracing::info!("display profile: none ({from}); sRGB output");
        return format!("{from}: nothing changed on the way to the screen");
    }
    match display::Lut3d::build(export::Space::Srgb, monitor, None) {
        Ok(lut) => {
            let d = lut.max_deviation() * 255.0;
            tracing::info!(
                "display profile {} ({from}): at most {d:.1} of 255 from sRGB",
                monitor.key()
            );
            format!("{from}: up to {d:.0} of 255 from sRGB")
        }
        Err(e) => {
            tracing::warn!(
                "display profile {} ({from}): {e:#}; sRGB output",
                monitor.key()
            );
            format!("{from}: cannot be read; nothing changed on the way to the screen")
        }
    }
}

/// A point in the masks' units on the view, in logical pixels: the
/// inverse of `view_to_source`.
/// `rows` into `model`, in place and only where they differ, so the
/// view is not rebuilt for a row that stayed.
pub(crate) fn sync_rows<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    while model.row_count() > rows.len() {
        model.remove(model.row_count() - 1);
    }
    for (i, row) in rows.into_iter().enumerate() {
        if i < model.row_count() {
            if model.row_data(i).as_ref() != Some(&row) {
                model.set_row_data(i, row);
            }
        } else {
            model.push(row);
        }
    }
}

/// The export sheet to open with, and the preset it is: the last
/// run's, or the preset `--export-preset` names, with `--long-edge`
/// and `--on-exists` over either. A preset the settings do not have
/// is an error, not a quiet export at whatever the sheet was left at.
pub(crate) fn opening_sheet(
    cli: &Cli,
    remembered: &settings::Settings,
) -> Result<(sheet::Sheet, Option<String>)> {
    let (mut s, preset) = match &cli.export_preset {
        Some(name) => {
            let p = sheet::lookup(&remembered.export_presets, name)?;
            (p.sheet.clone(), Some(p.name.clone()))
        }
        None => (
            remembered.export.clone(),
            (!remembered.export_preset.is_empty()).then(|| remembered.export_preset.clone()),
        ),
    };
    if let Some(edge) = cli.long_edge {
        // The flag's size over the sheet's, for this run's export.
        s.size = export::CUSTOM.into();
        s.custom = edge.to_string();
    }
    if let Some(policy) = cli.on_exists {
        // The flag's answer over the sheet's, for this run's export.
        s.on_exists = policy.name().into();
    }
    Ok((s, preset))
}

/// Whether `--export` names a folder for the set rather than a file
/// for one frame: a folder that is there, or a path that ends in a
/// separator, which asks for one. With `--also` (`set`), a path that
/// is not there and names no picture format is a folder too: a set
/// has no one file to go to.
pub(crate) fn export_folder(path: &Path, set: bool) -> bool {
    path.is_dir()
        || path
            .as_os_str()
            .to_string_lossy()
            .ends_with(std::path::is_separator)
        || (set && !path.exists() && export::Format::from_path(path).is_none())
}

/// `--also` with `--export` names a set: an error, not a quiet export
/// of one frame, when the export is to one file or a row is not in the
/// folder (rows from 0, as the browser has them).
pub(crate) fn check_also(cli: &Cli, into_folder: bool, files: usize) -> Result<()> {
    let Some(path) = &cli.export else {
        return Ok(());
    };
    if cli.also.is_empty() {
        return Ok(());
    }
    if !into_folder {
        anyhow::bail!(
            "--also names a set, and --export {} is one frame's file: give a folder (one that is there, or a path ending in a separator)",
            path.display()
        );
    }
    if let Some(row) = cli.also.iter().find(|r| **r >= files) {
        anyhow::bail!(
            "--also {row}: there {} {files} frame{} to export (rows from 0); open the folder, not a file, to export a set",
            if files == 1 { "is" } else { "are" },
            if files == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

/// The scope to open on: the command line's, else the last run's.
pub(crate) fn opening_scope(cli: &Cli, remembered: &settings::Settings) -> scope::Scope {
    cli.scope
        .as_deref()
        .and_then(scope::Scope::from_name)
        .or_else(|| scope::Scope::from_name(&remembered.scope))
        .unwrap_or_default()
}

/// The panel's choices, to keep for the next run. What the panel
/// does not own — the last file, the Settings sheet's two and the
/// lens offer's answer, each written to the file when it changes —
/// is put back from the file by the caller.
pub(crate) fn remember(app: &App) -> settings::Settings {
    settings::Settings {
        export: read_sheet(app),
        // Kept by the caller: the list is the state's, not the panel's.
        export_presets: Vec::new(),
        export_preset: chosen_preset(app).unwrap_or_default(),
        scope: app.get_scope().into(),
        curve_mode: app.get_curve_mode().into(),
        warn_shadows: app.get_warn_shadows(),
        warn_highlights: app.get_warn_highlights(),
        proof_profile: proof_settings(app)
            .map(|p| p.profile.key())
            .unwrap_or_default(),
        proof_intent: app.get_proof_intent().into(),
        gamut_warning: app.get_gamut_warning(),
        display_profile: display_key(app),
        display_monitor: app.get_display_monitor().to_string(),
        canvas_color: render::canvas_name(app.get_canvas_choice()).to_string(),
        collapsed: read_folds(app),
        grid_cell: app.get_grid_cell(),
        // Kept apart, all of them: the last file is written as soon
        // as a file develops, the sidecars' two as the Settings sheet
        // changes them, and the lens offer's answer as it is given.
        // The caller fills them in from disk.
        xmp_sidecars: false,
        sidecars_in_folder: false,
        lenses_declined: false,
        last_file: String::new(),
        // Not the panel's either: the settings file is where it is set.
        thumb_cache_mb: 0,
    }
}

/// The absolute path of the file now open, kept for the next run to
/// jump back to; skipped for a screenshot, a snapshot or a one-shot
/// export, which should leave the user's settings alone.
pub(crate) fn remember_last_file(st: &State) {
    // `batch` as well: a snapshot's path is taken at the capture, and
    // a develop that lands after it (a `--menu` over another frame)
    // must not write the user's settings either.
    if st.batch || st.screenshot.is_some() || st.snapshot.is_some() || st.export_then_quit.is_some()
    {
        return;
    }
    let Some(path) = st.current.and_then(|i| st.files.get(i)) else {
        return;
    };
    let Ok(path) = std::fs::canonicalize(path) else {
        return;
    };
    // Every develop lands here; the file only writes when the file
    // open has changed.
    let path = path.to_string_lossy().into_owned();
    let mut settings = settings::Settings::load();
    if settings.last_file == path {
        return;
    }
    settings.last_file = path;
    settings.save();
}

/// The panel's names for a profile that is colord's, and for one
/// that is a file.
pub(crate) const SYSTEM: &str = "System";

pub(crate) const FILE: &str = "File";

/// The picture's size for the panel: "6000 \u{d7} 4000 \u{b7} 24 MP".
/// The megapixels are rounded to the nearest whole one, as a camera
/// is named, and to a tenth below ten, where a whole one would be
/// coarse.
pub(crate) fn size_text(width: u32, height: u32) -> String {
    let mp = width as f64 * height as f64 / 1e6;
    let mp = if mp < 10.0 {
        format!("{mp:.1}")
    } else {
        format!("{}", mp.round() as u64)
    };
    format!("{width} \u{d7} {height} \u{b7} {mp} MP")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::deliver::batch_settings;
    use crate::sheet::{ExportPreset, Sheet};
    use greycard_core::image::WorkingImage;

    fn remembered_with_presets(dir: &Path) -> settings::Settings {
        let png = crate::watermark::tests::bar_png(dir, 40);
        settings::Settings {
            export: Sheet {
                size: "Full".into(),
                ..Sheet::default()
            },
            export_presets: vec![
                ExportPreset {
                    name: "Web 2048".into(),
                    sheet: Sheet {
                        format: "PNG".into(),
                        size: "2048".into(),
                        metadata: "None".into(),
                        mark: "Image".into(),
                        mark_image: png.to_string_lossy().into_owned(),
                        mark_position: "Top left".into(),
                        mark_size: 25.0,
                        mark_margin: 2.0,
                        mark_opacity: 100.0,
                        on_exists: "Overwrite".into(),
                        ..Sheet::default()
                    },
                },
                ExportPreset {
                    name: "Print".into(),
                    sheet: Sheet::default(),
                },
            ],
            export_preset: "Print".into(),
            ..settings::Settings::default()
        }
    }

    /// `--also` with `--export` is a set: into a folder, or an error,
    /// never a quiet export of one frame.
    #[test]
    fn also_with_export_names_a_set_or_says_why_not() {
        let parse = |args: &[&str]| Cli::try_parse_from(args).unwrap();
        // A file to write: one frame, so --also is an error.
        let cli = parse(&["greycard-ui", "shoot", "--export", "out.jpg", "--also", "1"]);
        assert!(!export_folder(Path::new("out.jpg"), true));
        let e = check_also(&cli, false, 3).unwrap_err().to_string();
        assert!(e.contains("one frame's file"), "{e}");
        // A path that is not there and names no format: a folder with
        // --also, and the file it always was without.
        let new = std::env::temp_dir().join("greycard-no-such-folder-for-also");
        assert!(export_folder(&new, true));
        assert!(!export_folder(&new, false));
        // A row the folder has not got: an error naming it.
        let cli = parse(&[
            "greycard-ui",
            "x.CR3",
            "--export",
            "out/",
            "--also",
            "0,1,2",
        ]);
        let e = check_also(&cli, true, 1).unwrap_err().to_string();
        assert!(e.starts_with("--also 1: there is 1 frame"), "{e}");
        assert!(check_also(&cli, true, 3).is_ok());
        // Without --export, --also is the snapshot's and not checked.
        let cli = parse(&["greycard-ui", "x.CR3", "--also", "5"]);
        assert!(check_also(&cli, false, 1).is_ok());
    }

    #[test]
    fn export_preset_fills_the_sheet_for_a_batch_run() {
        let dir = std::env::temp_dir().join(format!("greycard-cli-preset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let remembered = remembered_with_presets(&dir);
        let out = dir.join("out.jpg");
        let cli = Cli::try_parse_from([
            "greycard-ui".as_ref(),
            "frame.cr3".as_ref(),
            "--export".as_ref(),
            out.as_os_str(),
            "--export-preset".as_ref(),
            "web 2048".as_ref(),
        ])
        .unwrap();
        let (sheet, preset) = opening_sheet(&cli, &remembered).unwrap();
        assert_eq!(preset.as_deref(), Some("Web 2048"));
        assert_eq!(sheet, remembered.export_presets[0].sheet);
        // The path's extension over the preset's format, the rest the
        // preset's, as the batch run hands it to the worker.
        let settings = batch_settings(&out, sheet.settings());
        assert_eq!(settings.format, export::Format::Jpeg);
        assert_eq!(settings.long_edge, Some(2048));
        assert_eq!(settings.metadata, export::Metadata::None);
        assert!(settings.watermark.is_some());
        assert_eq!(sheet.on_exists(), export::OnExists::Overwrite);

        // And the file it writes: 2048 on its long side, the mark a
        // quarter of it, 41 px in from the top-left corner.
        let (w, h) = (3000usize, 2000usize);
        let image = WorkingImage {
            width: w,
            height: h,
            data: vec![0.05; w * h * 3],
        };
        let mut rendered = export::render(
            &image,
            &greycard_edit::Edit::default(),
            (w as u32, h as u32),
            &settings,
            &Default::default(),
            1.0,
            None,
            finish::Source::Scene,
        );
        export::mark(&mut rendered, &settings).unwrap();
        export::write(&rendered, &settings, &out, None, &export::Origin::default()).unwrap();
        let back = image::open(&out).unwrap().into_rgb8();
        assert_eq!(back.dimensions(), (2048, 1365));
        let ground = back.get_pixel(1500, 1000).0[1];
        let lit = |x: u32, y: u32| back.get_pixel(x, y).0[1] > ground.saturating_add(60);
        // The bar is the PNG's middle half: rows 41 + 64 to 41 + 192.
        assert!(lit(41 + 256, 41 + 128));
        assert!(lit(41 + 2, 41 + 128));
        assert!(!lit(41 - 3, 41 + 128));
        assert!(lit(41 + 511 - 2, 41 + 128));
        assert!(!lit(41 + 512 + 3, 41 + 128));
        assert!(!lit(41 + 256, 41 + 20));

        // A flag's size still wins over the preset's.
        let cli = Cli::try_parse_from([
            "greycard-ui",
            "frame.cr3",
            "--export-preset",
            "Web 2048",
            "--long-edge",
            "800",
        ])
        .unwrap();
        let (sheet, _) = opening_sheet(&cli, &remembered).unwrap();
        assert_eq!(sheet.settings().long_edge, Some(800));
        assert!(sheet.settings().watermark.is_some());
        // Without the flag: the sheet as left, and the preset last on.
        let cli = Cli::try_parse_from(["greycard-ui", "frame.cr3"]).unwrap();
        let (sheet, preset) = opening_sheet(&cli, &remembered).unwrap();
        assert_eq!(sheet, remembered.export);
        assert_eq!(preset.as_deref(), Some("Print"));
        // A name the settings do not have is an error that lists them.
        let cli =
            Cli::try_parse_from(["greycard-ui", "frame.cr3", "--export-preset", "Nope"]).unwrap();
        let err = opening_sheet(&cli, &remembered).unwrap_err().to_string();
        assert!(
            err.contains("Nope") && err.contains("Web 2048, Print"),
            "{err}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn size_text_reads_as_a_camera_is_named() {
        assert_eq!(size_text(6000, 4000), "6000 \u{d7} 4000 \u{b7} 24 MP");
        assert_eq!(size_text(8192, 5464), "8192 \u{d7} 5464 \u{b7} 45 MP");
        assert_eq!(size_text(3000, 2000), "3000 \u{d7} 2000 \u{b7} 6.0 MP");
        assert_eq!(size_text(1, 1), "1 \u{d7} 1 \u{b7} 0.0 MP");
    }

    /// `--preset` leaves a sidecar carrying its named step, where the
    /// setting puts it, and none when the run writes no sidecars; a
    /// preset that changes nothing writes nothing.
    #[test]
    fn a_preset_at_start_is_written_with_its_name() {
        let dir =
            std::env::temp_dir().join(format!("greycard-preset-start-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let file = dir.join("IMG_0001.CR3");
        let mut edit = Edit::default();
        edit.light.exposure = 0.5;
        let preset = Preset::from_edit("Faded film", &edit, &[greycard_edit::Section::Light]);

        let mut sidecar = Sidecar::default();
        let mut seed = false;
        preset_at_start(&mut sidecar, &mut seed, &file, &preset, None);
        assert_eq!(sidecar.current.light.exposure, 0.5);
        assert!(Sidecar::find(&file).is_none(), "no sidecars: none written");

        let mut sidecar = Sidecar::default();
        let placement = Some(greycard_edit::Placement::Folder);
        preset_at_start(&mut sidecar, &mut seed, &file, &preset, placement);
        let back = Sidecar::load(&file).unwrap().expect("the step was written");
        assert_eq!(
            Sidecar::find(&file),
            Some(Sidecar::path_in(&file, greycard_edit::Placement::Folder))
        );
        assert_eq!(back.current.light.exposure, 0.5);
        assert_eq!(back.current_label.as_deref(), Some("Preset: Faded film"));
        assert_eq!(back.describe(1).as_deref(), Some("Preset: Faded film"));

        // Again: on already, so no step and no write.
        let saved = back.saved;
        let mut again = back;
        preset_at_start(&mut again, &mut seed, &file, &preset, placement);
        assert_eq!(again.history.len(), 1);
        assert_eq!(Sidecar::load(&file).unwrap().unwrap().saved, saved);

        // A file that will not say its ISO keeps waiting for its
        // blend rather than being given one made up.
        let mut fresh = Sidecar::default();
        let mut waiting = true;
        preset_at_start(
            &mut fresh,
            &mut waiting,
            &dir.join("IMG_0002.CR3"),
            &preset,
            None,
        );
        assert!(waiting);
        assert_eq!(
            fresh.current.noise.learned_strength,
            Edit::default().noise.learned_strength
        );
        std::fs::remove_dir_all(&dir).expect("the temp dir goes");
    }
}
