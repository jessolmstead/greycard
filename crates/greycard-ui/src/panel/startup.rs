use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result};
use clap::Parser;
use greycard_edit::mask::Shape;
use greycard_edit::{Demosaic, Edit, Learned, meta, preset};
use slint::wgpu_30::{WGPUConfiguration, WGPUSettings, wgpu};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::filter::SavedBar;
use crate::panel::assets::show_presets;
use crate::panel::browser::{
    grid_filled, open_folder, rebuild_browser, row_of, show_thumb, time_select,
};
use crate::panel::color::{preview_white, white_key};
use crate::panel::cull::{cull_frame, develop_landed, show_filter, standing_in};
use crate::panel::curve::{PARAMETRIC, draw_curve};
use crate::panel::deliver::{chosen_preset, deliver, read_sheet, show_presets_picker, show_sheet};
use crate::panel::edit::{read_edit, read_folds, save_edit, show_folds, time_moves};
use crate::panel::mask::{ask_for, bake_locals, chosen_shape, show_shape};
use crate::panel::retouch::{patch_handles, pending_outlines, shown_outlines};
use crate::panel::viewport::{
    display_key, drawn_kind, drawn_picture, effective_zoom, picture_up, proof_settings,
    schedule_snapshot, settle_source_size, show_crop_on_view, show_hover, show_source_shape,
    source_known, source_to_view, sync_display, write_screenshot, zoom_label,
};
use crate::render::{Renderer, View};
use crate::worker::{Job, WhiteBase, Worker};
use crate::{
    App, Cli, MaskBox, MaskPick, Pt, STATE, Shared, State, WORKER, chooser, cull, display, export,
    files, filter, finder, finish, grid, install_callbacks, instance, log, render, report, scope,
    settings, sheet, tr, worker,
};
// What the import also lays at its landing, off the window, in the
// library.
pub(crate) use crate::preset_fit::preset_at_start;
// What the command line lays over the first frame's edit, which an
// export with no window lays the same way.
pub(crate) use greycard_engine::headless::Overrides;

pub(crate) fn main() -> Result<std::process::ExitCode> {
    let cli = Cli::parse();
    // A launch with a path alone, or with nothing, hands it to the
    // editor this user has running, and ends. Before the log starts,
    // which would move the running editor's file up a place.
    let claimed = claim(&cli);
    if let Some(instance::Claimed {
        claim: instance::Claim::Handed(pid),
        ..
    }) = &claimed
    {
        log::start_terminal(cli.verbose);
        tracing::info!("handed over to a running instance");
        // One line, so a second editor started from a terminal beside
        // the running one says why no window came.
        if cli.path.is_some() {
            eprintln!("opened in the running greycard (pid {pid})");
        } else {
            eprintln!("brought the running greycard forward (pid {pid})");
        }
        return Ok(std::process::ExitCode::SUCCESS);
    }
    // Before anything is said. The line is for the person at the
    // terminal, not the log, which knows where it is.
    if let Some(log) = log::start(cli.verbose) {
        eprintln!("log: {}", log.display());
    }
    // Whether the models can run here, asked of the processor and the
    // disk and said once; nothing of ONNX Runtime is loaded to find
    // out; it loads at the first model (`greycard_ai::runtime`).
    if let Err(why) = greycard_ai::runtime::status() {
        tracing::warn!("{why}");
    }
    greycard_ai::Store::sweep_user_in_background();
    let instance = claimed.and_then(said);
    // `--match-compare` measures the camera match and writes a report;
    // no window, no device.
    if let Some(out) = cli.match_compare.as_deref() {
        return crate::runners::match_compare(&cli, out);
    }
    // `--measure-baseline` likewise: the default develop against the
    // camera's JPEG, per body.
    if let Some(out) = cli.measure_baseline.as_deref() {
        return crate::runners::measure_baseline(&cli, out);
    }
    // `--fit-lightroom` likewise: the mapper fitted to the blocks the
    // measurement kept.
    if !cli.fit_lightroom.is_empty() {
        return crate::runners::fit_lightroom(&cli, &cli.fit_lightroom);
    }
    // `--measure-lightroom` likewise: a catalog against its exports.
    if let Some(catalog) = cli.measure_lightroom.as_deref() {
        if cli.crops {
            return crate::runners::measure_crop(&cli, catalog);
        }
        return crate::runners::measure_lightroom(&cli, catalog);
    }
    // `--import` with no sheet asked for is a run with no window.
    let import_sheet = matches!(
        cli.sheet,
        Some(
            crate::panel::viewport::Shown::Import
                | crate::panel::viewport::Shown::Imported
                | crate::panel::viewport::Shown::ImportInPlace
        )
    );
    if let Some(source) = cli.import.as_deref().filter(|_| !import_sheet) {
        let to = cli.to.as_deref().context("--import wants --to DEST")?;
        let opts = crate::import::Options {
            source: source.to_path_buf(),
            destination: to.to_path_buf(),
            subfolder: cli.subfolder.clone().unwrap_or_default(),
            name: cli.name.clone().unwrap_or_else(|| "{name}".into()),
            preset: None,
            backup: cli.backup.clone(),
            library: cli
                .library
                .clone()
                .or_else(greycard_library::Library::user_path),
            // Where the Settings sheet puts sidecars, or the flag's
            // choice; none for a run that writes none.
            placement: (!cli.no_sidecars).then(|| {
                if cli.sidecar_folder
                    || settings::Settings::load_untouched()
                        .shared
                        .sidecars_in_folder
                {
                    greycard_edit::Placement::Folder
                } else {
                    greycard_edit::Placement::Beside
                }
            }),
            profiles: Vec::new(),
            verify: cli.verify,
        };
        return crate::panel::import::headless(opts, cli.preset.as_deref());
    }
    // `--export` is a run with no window too: the export never needed
    // one (`headless`).
    if let Some(target) = cli.export.as_deref() {
        return crate::headless::export(&cli, target);
    }
    // A first run is one with no settings file yet: read before
    // anything this run writes one.
    let first_run = settings::path().is_some_and(|p| !p.exists());
    let (remembered, set_aside) = settings::Settings::load_at_start();
    match settings::path() {
        Some(p) => tracing::info!("settings: {}", p.display()),
        None => tracing::warn!("settings: no configuration directory; nothing is remembered"),
    }
    let last_file = (!remembered.shared.last_file.is_empty())
        .then(|| PathBuf::from(&remembered.shared.last_file))
        .filter(|p| p.is_file());

    // The library index, on a thread of its own once the window is up:
    // the folder open is indexed there, so the first frame never waits
    // for a pass, and the facets fill in as it goes. The roots kept
    // beside it, unless the command line names this run's own, are
    // read now, before the files: a start folder with no frames of its
    // own opens as the library's view of it when it lies under a root.
    let library_path = cli
        .library
        .clone()
        .or_else(greycard_library::Library::user_path);
    let roots_kept = library_path
        .as_ref()
        .filter(|_| cli.roots.is_empty())
        .map(|path| {
            let file = greycard_library::Roots::path_beside(path);
            let read = greycard_library::Roots::load(&file);
            (file, read)
        });

    // The files to open, and which of them to start on. The command
    // line's path, a file or a directory, as before; without one, the
    // last file's folder when the settings still find it there,
    // otherwise nothing yet: the desktop's folder chooser is asked
    // for one once the window is up, and if that is canceled the
    // empty editor is left showing, its Open folder button waiting.
    // Which of them to start on is settled off the window's thread
    // with the list's sidecars (`roots::open_listing`), since matching
    // the last file to the list makes folders canonical. Instead of
    // files, the view of the library to open once its index is.
    let (files, select, start_view): (
        Vec<PathBuf>,
        crate::roots::Select,
        Option<crate::roots::View>,
    ) = match &cli.path {
        Some(path) => {
            let files = files::list_files(path, remembered.shared.jpegs_apart)?;
            anyhow::ensure!(!files.is_empty(), "no RAW files at {}", path.display());
            let select = if path.is_dir() {
                crate::roots::Select::Last(last_file.clone())
            } else {
                crate::roots::Select::Row(0)
            };
            (files, select, None)
        }
        // The all-roots view asked for, and no path: the view is what
        // opens, and nothing of the last file's folder is read or
        // developed first.
        None if cli.all_roots || !cli.roots.is_empty() => {
            (Vec::new(), crate::roots::Select::Row(0), None)
        }
        // The folder the Settings sheet names to start in while it is
        // there with frames in it, else the last file's.
        None => {
            let none = greycard_library::Roots::default();
            let roots = roots_kept
                .as_ref()
                .and_then(|(_, read)| read.as_ref().ok())
                .unwrap_or(&none);
            let (files, view) = start_open(
                roots,
                &remembered.start_folder,
                remembered.folder_tree_subfolders,
                last_file.as_deref().and_then(Path::parent),
                |dir| files::list_files(dir, remembered.shared.jpegs_apart).ok(),
            );
            let select = if files.is_empty() {
                crate::roots::Select::Row(0)
            } else {
                crate::roots::Select::Last(last_file.clone())
            };
            (files, select, view)
        }
    };

    // The interface's scale, set before anything of Slint's backend is
    // chosen: the variable its winit backend reads as the window is
    // made, while this thread is still the only one that reads the
    // environment. A capture is drawn at the system's scale, whatever
    // the user's setting, so it comes out the same on any machine; a
    // variable set by hand still wins.
    let capture = cli.snapshot.is_some() || cli.screenshot.is_some();
    // The window as the last run closed it, unless the command line
    // owns this one (`keeps_window`).
    let memory = crate::window_fit::kept::Memory::of(keeps_window(&cli), &remembered.windows);
    let ui_scale = crate::window_fit::scale::start(
        if capture { 100 } else { remembered.ui_scale },
        remembered.ui_scale_system,
    );

    // A start that died setting up the GPU leaves a marker, and the
    // next goes round the backend it died on where there is another:
    // set now, before `WGPUSettings` reads the environment. Taken off
    // once the select comes back (`gpu_marker`).
    let _gpu_marker = crate::gpu_marker::start();

    // The power preference `WGPU_POWER_PREF` names, as Slint's default
    // reads it, and high performance where it names nothing: Slint's
    // own default is none, and wgpu then takes the first adapter
    // listed, on a laptop often the integrated GPU beside a discrete
    // one. greycard-ai's probe asks the same; greycard-gpu's own
    // instances ask high performance.
    let power_preference =
        wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::HighPerformance);
    let asked = match power_preference {
        wgpu::PowerPreference::HighPerformance => "high performance",
        wgpu::PowerPreference::LowPower => "low power",
        wgpu::PowerPreference::None => "no preference",
    };
    match std::env::var("WGPU_POWER_PREF") {
        Ok(value) => tracing::info!("adapter: {asked} (WGPU_POWER_PREF={value})"),
        Err(_) => tracing::info!("adapter: {asked} (greycard's default)"),
    }
    // Made from the environment, which `WGPUSettings` reads for its
    // backends: again if the marker's backend is taken off.
    let wgpu_settings = || {
        let mut settings = WGPUSettings::default();
        settings.power_preference = power_preference;
        // Slint asks for downlevel limits, which allow no storage
        // buffers; the histogram needs one. Desktop defaults, and 45 MP
        // frames are 8192 wide and more, exactly the default texture
        // limit.
        settings.device_required_limits = wgpu::Limits {
            max_texture_dimension_2d: 16384,
            ..wgpu::Limits::default()
        };
        settings
    };
    let mut settings = wgpu_settings();
    // No taller or wider than the monitor it opens on leaves room for,
    // fitted before the window is created: a handler a selector, its
    // slot filled once the window is made. The same handler holds the
    // window's scale to the system's times the setting.
    let select_backend = |settings: WGPUSettings| {
        let (fit, fit_slot) = crate::window_fit::handler(ui_scale, memory.clone());
        slint::BackendSelector::new()
            .require_wgpu_30(WGPUConfiguration::Automatic(settings))
            .with_winit_custom_application_handler(fit)
            .select()
            .map(|()| fit_slot)
    };
    // A machine with no GPU greycard can draw on draws on its software
    // adapter rather than fail here, asked only once Slint has failed:
    // a failed select sets no platform, so a second one can. Slint
    // still chooses the adapter, with the window's surface to choose
    // by.
    use crate::adapter::Drawing;
    let asked = crate::adapter::asked();
    let mut selected = select_backend(settings.clone());
    // A select on the backend a marker sent this start to that comes
    // back an error is no death: the default backends next, before any
    // software adapter is looked for, so a pinned machine is not shut
    // out of its GPU or sent to WARP by it.
    if let Err(first) = &selected
        && crate::gpu_marker::fallback_failed(first)
    {
        settings = wgpu_settings();
        selected = select_backend(settings.clone());
    }
    let (fit_slot, drawing) = match selected {
        Ok(slot) => (
            slot,
            if asked {
                Drawing::AsAsked
            } else {
                Drawing::Gpu
            },
        ),
        Err(first) => match if asked {
            Drawing::AsAsked
        } else {
            crate::adapter::probe()
        } {
            Drawing::Software => (
                select_backend(settings)
                    .context("selecting Slint's wgpu backend on a software adapter")?,
                Drawing::Software,
            ),
            Drawing::Nothing => {
                // Slint's own cause names a variable that would do
                // nothing here; it goes to the log.
                tracing::info!("Slint's select failed: {first}");
                anyhow::bail!(crate::adapter::nothing_error());
            }
            Drawing::Gpu | Drawing::AsAsked => {
                return Err(first).context("selecting Slint's wgpu backend");
            }
        },
    };
    // The select came back: past where a driver killed a start, so the
    // marker comes off here and not at the first frame.
    crate::gpu_marker::selected();
    // The event loop is built now and not yet running: the moment to
    // hear the Finder, whose launch event comes as the loop starts.
    #[cfg(target_os = "macos")]
    finder::install();

    // The window's app id, which Wayland's app_id and X11's WM_CLASS
    // carry, and the desktop entry greycard.desktop is matched by: a
    // dock pins the window to its entry and shows its icon. Set before
    // the window is built, which is when winit reads it; off Linux it
    // does nothing.
    if let Err(e) = slint::set_xdg_app_id("greycard") {
        tracing::warn!("the window's app id was not set: {e}");
    }
    let app = App::new()?;
    // The language, now that a component exists and before the window
    // shows.
    crate::panel::prefs::start_language(&app, &remembered.language);
    // A size asked for on the command line, for a capture, in place of
    // the layout's preferred one; the screen's fit still applies.
    if let Some((w, h)) = cli.window {
        app.set_asked_width(w);
        app.set_asked_height(h);
    }
    *fit_slot.borrow_mut() = Some(app.as_weak());
    // Other launches heard from now on, the event loop there to take
    // what they hand over.
    if let Some(instance) = instance {
        crate::panel::handed::serve(&app, instance);
    }
    // A slider turns the wheel back under natural scrolling: a
    // wheel's way here, a touchpad's set per event where they differ.
    // Read on a thread nothing waits for: on GNOME it is two
    // `gsettings` runs, which a session bus that never answers holds.
    {
        let app = app.as_weak();
        std::thread::spawn(move || {
            let natural = crate::scrolling::natural();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = app.upgrade() {
                    app.global::<crate::SliderKeys>().set_natural(natural.wheel);
                }
            });
        });
    }
    // The camera profiles' headers read ahead, on a thread nothing
    // waits for: the list is read again on every open, on the window's
    // thread, and a tree of thousands read cold for the first open
    // would hold the window for a moment the user sees.
    if !cfg!(test)
        && let Err(e) = std::thread::Builder::new()
            .name("profiles".into())
            .spawn(greycard_edit_io::profiles::warm)
    {
        tracing::warn!("camera profiles not read ahead: {e}");
    }
    // The Settings sheet's scale row: the setting, and the one this
    // window started at, which a change is told it waits for.
    {
        let chosen = crate::window_fit::scale::held(remembered.ui_scale);
        crate::panel::prefs::show_ui_scale(&app, chosen);
        app.set_ui_scale_started(chosen as i32);
        app.set_ui_scale_by_hand(matches!(
            ui_scale,
            crate::window_fit::scale::Scale::ByHand(_)
        ));
    }
    // The Settings sheet opens at the category it was left at.
    app.set_settings_category(settings::category_named(&remembered.settings_category).into());
    // The palette the window was left in, the user's under a capture
    // too, as the system's scale is.
    app.set_theme(settings::theme_named(&remembered.theme).into());
    app.set_scroll_bars(settings::scroll_bars_named(&remembered.scroll_bars).into());
    app.set_scroll_bar_size(settings::scroll_bar_size_named(&remembered.scroll_bar_size).into());
    // Which curve the panel shows at the start, for a dump of its
    // picture with `GREYCARD_UI_CURVE`.
    if let Ok(name) = std::env::var("GREYCARD_UI_CURVE_CHANNEL") {
        app.set_curve_channel(name.into());
    }
    if let Some(tab) = &cli.tab {
        app.set_panel_tab(tab.as_str().into());
    }
    // The Bayer list until a raw is open; each open sets its mosaic's.
    app.set_demosaics(ModelRc::new(VecModel::from(
        Demosaic::for_pattern(&greycard_core::raw::CfaPattern::rggb())
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
    show_presets_picker(&app, &remembered.shared.export_presets, preset.as_deref());
    show_folds(&app, &remembered.collapsed);
    crate::panel::shares::show(&app, &remembered.shares);
    crate::panel::sizes::show(&app, &remembered);
    app.set_scope(opening_scope(&cli, &remembered).name().into());
    app.set_scope_selection(cli.scope_selection);
    app.set_show_sharpen_mask(cli.sharpen_mask);
    show_overlays(&app, cli.clipping, &remembered);
    app.set_sync_shift_temperature(remembered.sync_shift.temperature);
    app.set_sync_shift_tint(remembered.sync_shift.tint);
    app.set_sync_shift_exposure(remembered.sync_shift.exposure);
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
        crate::panel::viewport::set_proof_file(&app, &file);
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
    // remembered one, and the monitors the system's color manager
    // offers for it.
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
        let monitors = display::system_monitors().unwrap_or_else(|e| {
            tracing::warn!(
                "{}: {e:#}; no display profile from it",
                display::SYSTEM_SOURCE
            );
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
    app.set_one_section(remembered.one_section);
    app.set_filter_bar(remembered.filter_bar);
    app.set_camera_profiles_all(remembered.profiles_all_cameras);
    if remembered.curve_mode == PARAMETRIC {
        app.set_curve_mode(PARAMETRIC.into());
    }
    app.set_scopes(ModelRc::new(VecModel::from(
        scope::Scope::ALL
            .iter()
            .map(|s| slint::SharedString::from(s.name()))
            .collect::<Vec<_>>(),
    )));
    // The files' sidecars are not read here: the list lands through
    // `roots::open_listing` once the loop runs, each frame from its row
    // in the index where it has one, the rest read off the window's
    // thread. Until then the strip is empty.
    crate::cells::set_rows(&app, Vec::new());
    // The grid's geometry, from one place: the layout in the .slint
    // file and the arithmetic in `grid` read the same numbers.
    app.set_grid_gap(grid::GAP);
    app.set_grid_pad(grid::PAD);
    app.set_grid_label(grid::LABEL);
    app.set_meta_stars(meta::STARS as i32);
    app.set_grid_cell(grid::clamp_cell(
        cli.grid_cell.unwrap_or(remembered.grid_cell),
    ));
    app.set_wheel_resizes(remembered.grid_wheel);
    app.set_lightbox_across(crate::lightbox::held(remembered.lightbox_across));
    app.set_range_sample(crate::panel::viewport::sample_size(remembered.range_sample));
    // The Library at the start when the Settings sheet chose it, as
    // `--grid` asks; never over a file named, or a run its flags own.
    let flags_own = scripted_start(&cli)
        || cli.cull
        || cli.cull_compare.is_some()
        || cli.cull_key.is_some()
        || cli.cull_develop
        || cli.ask_rejects
        || cli.move_rejects;
    let file_named = cli.path.as_ref().is_some_and(|p| p.is_file());
    app.set_grid_open(settings::opens_on_library(
        cli.grid,
        &remembered.start_view,
        file_named,
        flags_own,
    ));
    // `--hide-panels` puts all three away for this run, whatever the
    // last one left.
    app.set_left_hidden(cli.hide_panels || remembered.hide_left);
    app.set_right_hidden(cli.hide_panels || remembered.hide_right);
    app.set_strip_hidden(cli.hide_panels || remembered.hide_strip);
    // The three names the flag's old three-way Show went by. The
    // chips say more than they can, but a flag on a command line
    // wants one word, and a script written against the old one still
    // means what it meant. Anything else is the filter language: its
    // facet terms wait for the index to name the chips they mean,
    // and the rest go in the text field as if typed there.
    let (filter, facets_wanted) = match cli.filter.as_deref() {
        // The filter as the last session left it, for a session: not
        // for a batch run, whose export or capture is of the file it
        // names, and not over a file named on the command line (a
        // double-click from the desktop), which a filter must never
        // hide from the person who asked for it.
        None if restores_filter(&cli) => (remembered.filter.filter(), Vec::new()),
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
                let mut filter = filter::Filter {
                    text,
                    ..filter::Filter::default()
                };
                let wanted = filter.choose_at_once(wanted);
                for e in filter.typed().errors {
                    tracing::warn!("--filter: {e}");
                }
                (filter, wanted)
            }
        },
    };
    let filtering = !filter.is_empty();
    app.set_filter_text(filter.text.clone().into());
    app.set_reject_count(0);

    let preset_store = preset::Store::user();
    // The film presets this build ships, put there the first time
    // there is a directory to put them in (`Store::seed`).
    if let Some(store) = &preset_store {
        store.seed();
    }
    let placement = if cli.sidecar_folder || remembered.shared.sidecars_in_folder {
        greycard_edit::Placement::Folder
    } else {
        greycard_edit::Placement::Beside
    };
    // What the command line asks of the first frame, applied to its
    // sidecar when the list lands and before it is opened.
    let preset_at_start_wanted = match &cli.preset {
        Some(name) if !files.is_empty() => match preset_store.as_ref().and_then(|s| s.find(name)) {
            Some(entry) => Some(entry.preset.clone()),
            None => anyhow::bail!("no preset called {name}"),
        },
        _ => None,
    };
    // `--develop-temperature` and `--exposure` are laid over the file's
    // edit when it is opened (`Overrides`, through `overrides_at_start`,
    // which names the frame once the list has landed and said which it
    // is).
    let overrides = overrides_of(&cli);
    let tweak_at_start: Option<crate::Tweak> = (preset_at_start_wanted.is_some()
        || !overrides.is_empty())
    .then(|| {
        let preset = preset_at_start_wanted;
        let placement = (!cli.no_sidecars).then_some(placement);
        Box::new(move |st: &mut State, i: usize| {
            if let Some(preset) = preset {
                // Written only where a save would be (`writable`).
                let placement = placement.filter(|_| crate::panel::edit::file_writable(st, i));
                let st = &mut *st;
                let saved =
                    preset_at_start(&mut st.sidecars[i], None, &st.files[i], &preset, placement);
                // Written: the archive's copy is queued as any
                // save's is (§233).
                if saved {
                    crate::sync::after_save(st, i);
                }
            }
            if !overrides.is_empty() {
                st.launch.overrides_at_start = Some((i, overrides));
            }
        }) as crate::Tweak
    });

    if let Some(keys) = cli.keys.clone() {
        let _ = crate::panel::viewport::SNAPSHOT_KEYS.set(keys);
    }
    let state = Rc::new(Shared::new({
        let write_sidecars = !cli.no_sidecars;
        let xmp_sidecars = cli.xmp_sidecars || remembered.shared.xmp_sidecars;
        let cull_move_on = remembered.cull_move_on;
        let (loupe_cull, show_developed) = (remembered.loupe_cull, remembered.cull_developed);
        let jpegs_apart = remembered.shared.jpegs_apart;
        let sort = remembered.sort.clone();
        let zoom = cli.zoom.max(0.0);
        let scope = opening_scope(&cli, &remembered);
        let show_mask = cli.show_mask.and_then(|i| i.checked_sub(1));
        let show_patch = cli.patch.and_then(|i| i.checked_sub(1));
        let store = greycard_ai::Store::user().ok();
        let lenses_declined = remembered.lenses_declined;
        let models_asked = remembered.models_asked;
        let screenshot = cli.screenshot.clone();
        let snapshot = cli.snapshot.clone();
        let panel_scroll = cli.panel_scroll;
        let snapshot_shown = cli
            .sheet
            .or(cli.tool)
            .or(cli.part)
            .or(cli.menu.map(crate::panel::viewport::Shown::Menu))
            .or(cli.press.map(crate::panel::viewport::Shown::Key));
        let export_presets = remembered.shared.export_presets.clone();
        let export_last_folder = remembered_folder(&remembered);
        let settings_file = if cli.snapshot.is_some() || cli.screenshot.is_some() {
            None
        } else {
            settings::path()
        };
        let presets = preset_store.as_ref().map(|s| s.list()).unwrap_or_default();
        let batch = cli.snapshot.is_some() || cli.screenshot.is_some();
        // Only a session someone is at deletes: never a capture or a
        // timing run, which have nobody to have confirmed. (An export
        // from the command line has no window: `headless`.)
        let scripted = scripted_start(&cli);
        let deletes_allowed = !scripted;
        let time_sharpen = cli.time_sharpen.map(|n| (n, None, Vec::new()));
        let time_clarity = cli.time_clarity.map(|n| (n, None, Vec::new()));
        let time_dehaze = cli.time_dehaze.map(|n| (n, None, Vec::new()));
        let time_texture = cli.time_texture.map(|n| (n, None, Vec::new()));
        let time_cull = cli.time_cull.map(|n| (n, Vec::new()));
        let time_select = cli.time_select.map(|n| (n, Vec::new(), Vec::new()));
        let time_scroll = cli
            .time_scroll
            .map(|n| crate::panel::browser::ScrollTiming::new(n, cli.time_scroll_by));
        let snapshot_placeholder = cli.snapshot_placeholder;
        let cull_develop = cli.cull_develop;
        let turn_at_start = cli.turn.filter(|q| q.rem_euclid(4) != 0);
        let awaiting_turn = cli.turn.is_some_and(|q| q.rem_euclid(4) != 0);
        let cull_key_at_start = cli.cull_key.clone();
        let ask_rejects = cli.ask_rejects || cli.move_rejects;
        let also_at_start = cli.also.clone();
        let move_rejects = cli.move_rejects;
        let cull_at_start = (cli.cull
            || cli.cull_compare.is_some()
            || cli.cull_key.is_some()
            || cli.time_cull.is_some()
            || cli.cull_develop
            || cli.ask_rejects
            || cli.move_rejects)
            .then(|| cli.cull_compare.unwrap_or(1));
        let recent = crate::panel::recent::Recent {
            open: None,
            folders: remembered.recent_folders.clone(),
            ..Default::default()
        };
        let tree = crate::tree::Tree::with_subfolders(remembered.folder_tree_subfolders);
        let awaiting_index = !facets_wanted.is_empty();
        let mut st = State::empty(Vec::new(), &app);
        st.launch.tweak_at_start = tweak_at_start;
        st.launch.show_mask = show_mask;
        st.launch.show_patch = show_patch;
        st.launch.screenshot = screenshot;
        st.launch.snapshot = snapshot;
        st.launch.panel_scroll = panel_scroll;
        st.launch.snapshot_shown = snapshot_shown;
        st.window.batch = batch;
        st.launch.time_sharpen = time_sharpen;
        st.launch.time_clarity = time_clarity;
        st.launch.time_dehaze = time_dehaze;
        st.launch.time_texture = time_texture;
        st.launch.time_cull = time_cull;
        st.launch.time_select = time_select;
        st.launch.time_scroll = time_scroll;
        st.launch.snapshot_placeholder = snapshot_placeholder;
        st.launch.cull_develop = cull_develop;
        st.launch.turn_at_start = turn_at_start;
        st.launch.awaiting_turn = awaiting_turn;
        st.launch.cull_key_at_start = cull_key_at_start;
        st.launch.ask_rejects = ask_rejects;
        st.launch.also_at_start = also_at_start;
        st.launch.move_rejects = move_rejects;
        st.launch.cull_at_start = cull_at_start;
        st.launch.awaiting_index = awaiting_index;
        st.launch.facets_wanted = facets_wanted;
        st.window.write_sidecars = write_sidecars;
        st.window.xmp_sidecars = xmp_sidecars;
        st.window.placement = placement;
        st.window.lens_on_new = remembered.shared.lens_on_new;
        st.window.author = remembered.shared.author.clone();
        st.window.copyright = remembered.shared.copyright.clone();
        // The fields as the file has them, so a close of the sheet
        // before any opening takes nothing away.
        app.set_settings_author(st.window.author.as_str().into());
        app.set_settings_copyright(st.window.copyright.as_str().into());
        st.window.settings_file = settings_file;
        st.window.deletes_allowed = deletes_allowed;
        st.launch.scripted = scripted;
        st.sheets.lenses_declined = lenses_declined;
        st.sheets.models.asked = models_asked;
        st.work.export_presets = export_presets;
        st.work.export_last_folder = export_last_folder;
        let apps = remembered.edit_in.iter().map(PathBuf::from).collect();
        crate::panel::edit_in::set_apps(&app, &mut st.work.edit_in, apps);
        st.culling.cull_move_on = cull_move_on;
        crate::panel::cull::take_remembered_mode(&mut st, loupe_cull, show_developed);
        // `--cull-developed`, over whatever the last run left.
        if cli.cull_developed {
            st.culling.show_developed = true;
            st.launch.cull_at_start = None;
            st.launch.developed_at_start = true;
        }
        st.views.jpegs_apart = jpegs_apart;
        st.views.sort = sort;
        st.views.filter = filter;
        st.views.recent = recent;
        st.views.tree = tree;
        st.develop.zoom = zoom;
        st.develop.mat = remembered.mat;
        st.develop.scope = scope;
        st.develop.store = store;
        st.develop.monitors = monitors;
        st.develop.presets = presets;
        st.develop.preset_store = preset_store;
        st.develop.preset_favorites = remembered.preset_favorites.clone();
        st.develop.preset_folds = remembered.preset_folds.clone();
        crate::panel::preset_list::prune(&mut st);
        crate::panel::crop::take_settings(&mut st, &remembered);
        st.develop.fill_choice =
            greycard_edit::retouch::FillChoice::from_name(&remembered.fill_choice);
        st
    }));
    app.set_compare_tiles(ModelRc::from(state.borrow().culling.compare_tiles.clone()));
    // The export queue the last session left: shown, never run. A
    // snapshot or a batch run reads it and leaves the file alone.
    {
        let mut st = state.borrow_mut();
        let write = st.window.settings_file.is_some();
        crate::panel::export_queue::open_at_launch(
            &mut st,
            &app,
            settings::path().as_deref(),
            write,
        );
    }
    // `--sheet lightroom`: the catalog its sheet opens on.
    if let Some(catalog) = &cli.lightroom {
        let mut st = state.borrow_mut();
        st.lightroom.catalog = Some(catalog.clone());
        st.lightroom.seed = cli.root.clone();
    }
    // `--sheet import`: the sheet shows what the import flags name.
    if import_sheet {
        let mut st = state.borrow_mut();
        st.import.source = cli.import.clone();
        st.import.destination = cli.to.clone();
        st.import.backup = cli.backup.clone();
        if cli.name.is_some() || cli.subfolder.is_some() {
            st.import.filled = true;
            app.set_import_name(cli.name.clone().unwrap_or("{name}".into()).into());
            app.set_import_subfolder(cli.subfolder.clone().unwrap_or_default().into());
        }
    }
    // The settings sheet's two, as this run has them: the flags'
    // over the file's.
    app.set_sidecar_placement(
        crate::panel::prefs::placement_name(state.borrow().window.placement).into(),
    );
    app.set_xmp_sidecars(state.borrow().window.xmp_sidecars);
    app.set_cull_move_on(state.borrow().culling.cull_move_on);
    // The browser's list under the filter asked for, the files it
    // hides left out of the strip and the grid; and the chips, which
    // count the folder whether or not anything is filtered.
    if filtering {
        rebuild_browser(&mut state.borrow_mut(), &app);
    } else {
        show_filter(&state.borrow(), &app);
    }
    show_presets(&state.borrow(), &app);
    // A snapshot of the grid waits for the pictures its cells show,
    // and one that never comes would hold it until it is killed.
    if cli.snapshot.is_some() {
        let (state, app_weak) = (Rc::downgrade(&state), app.as_weak());
        slint::Timer::single_shot(crate::panel::browser::GRID_WAIT, move || {
            if let (Some(state), Some(app)) = (state.upgrade(), app_weak.upgrade()) {
                crate::panel::browser::give_up_on_grid(&mut state.borrow_mut(), &app);
            }
        });
    }

    // Results come back to the UI thread through the event loop, the
    // thumbnails in batches: one call a picture, and a warm folder of
    // twenty thousand kept the window from drawing until the last.
    let worker = {
        let app_weak = app.as_weak();
        Worker::batched(move |outcome| {
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
            // The local previews beside the thumbnails, under a cap of
            // their own.
            let cache = cache.with_previews(
                crate::previews::SIZE,
                crate::panel::prefs::cap_bytes(remembered.preview_cache_mb),
            );
            tracing::info!(
                "thumbnail cache {} ({}; previews {})",
                cache.root().display(),
                match remembered.thumb_cache_mb {
                    0 => "off".to_string(),
                    mb => format!("cap {mb} MB"),
                },
                match remembered.preview_cache_mb {
                    0 => "none".to_string(),
                    mb => format!("cap {mb} MB"),
                }
            );
            worker.set_thumb_cache(Some(cache));
        }
        Err(e) => tracing::warn!("no thumbnail cache: {e}"),
    }
    // The culling loupe reads the local previews the pool makes, and
    // makes one from a camera picture it decoded anyway.
    state
        .borrow()
        .culling
        .prefetch
        .set_previews(worker.previews());
    // The roots: the command line's for this run, else the ones kept
    // beside the library, read with the files above.
    {
        let mut st = state.borrow_mut();
        // The collections, kept beside the library whatever the roots,
        // into the sidebar's model the window already holds.
        if let Some(path) = &library_path {
            st.views.collections =
                crate::collections::Store::load(crate::collections::Store::dir_beside(path));
            crate::panel::collections::show(&st, &app);
            // The stacks, in their folder among the collections.
            st.views.stacks = crate::stacks::Store::load(greycard_library::stacks::dir_in(
                &crate::collections::Store::dir_beside(path),
            ));
            // Both carried beside their frames, for another machine.
            if !cli.no_sidecars {
                crate::panel::carried::attach(&mut st, path);
            }
        }
        st.views.library.poll_every = crate::roots::poll_every(remembered.network_poll_minutes);
        if cli.roots.is_empty() {
            if let Some((file, read)) = roots_kept {
                match read {
                    Ok(roots) => {
                        st.views.library.roots = roots;
                        st.views.library.file = Some(file);
                    }
                    // Kept as it is: with no file to save to, nothing
                    // is written over it this session.
                    Err(e) => tracing::warn!("library: roots not read: {e}"),
                }
            }
        } else {
            for dir in &cli.roots {
                if let Err(e) = st.views.library.roots.add(dir) {
                    tracing::warn!("--roots: {e}");
                }
            }
        }
        if !cli.roots.is_empty() || cli.all_roots {
            st.views.library.wanted = Some(crate::roots::View::Roots(None));
            // A capture waits for the view, not the folder under it.
            st.views.library.awaiting = st.window.batch;
        } else if let Some(view) = start_view {
            st.views.library.wanted = Some(view);
            st.views.library.awaiting = st.window.batch;
            st.launch.last_folder = last_file
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf);
        }
    }
    // The launch's folder, the command line's or the last one's, is
    // named in the pane and put at the front of Recently opened when
    // its list lands through the browser's open (`open_loaded`), as
    // any other folder is; the pane is named empty until then.
    crate::panel::recent::show(&state.borrow(), &app);
    match library_path {
        Some(path) => {
            let app_weak = app.as_weak();
            // Known from here, so the launch's own folder can take its
            // rows from the index before the indexer has said it is
            // open; a file that is not there yet is a read that fails
            // and the sidecars read instead.
            state.borrow_mut().views.index_path = Some(path.clone());
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
                    indexer.set_lightroom_takes(st.window.sidecars());
                    st.views.index = Some(indexer);
                    crate::library::index_open_folder(&mut st, None);
                    // After the open folder's own pass is asked for,
                    // so that one goes first.
                    crate::roots::start(&mut st, &app);
                }
                Err(e) => {
                    tracing::warn!("no library index: the indexer did not start: {e}");
                    st.launch.awaiting_index = false;
                    st.views.library.awaiting = false;
                    // No view of the roots will come to take the
                    // launch's folder's place.
                    st.views.library.wanted = None;
                }
            }
            crate::panel::cull::show_filter(&st, &app);
            crate::roots::show(&st, &app);
        }
        None => {
            tracing::warn!("no data directory for the library index; the facets are off");
            let mut st = state.borrow_mut();
            st.launch.awaiting_index = false;
            st.views.library.awaiting = false;
            st.views.library.wanted = None;
        }
    }
    app.set_mask_handles(ModelRc::from(state.borrow().develop.mask_handles.clone()));
    app.set_mask_boxes(ModelRc::from(state.borrow().develop.mask_boxes.clone()));
    app.set_mask_picks(ModelRc::from(state.borrow().develop.mask_picks.clone()));
    app.set_person_boxes(ModelRc::from(state.borrow().develop.person_boxes.clone()));
    app.set_patch_handles(ModelRc::from(state.borrow().develop.patch_handles.clone()));
    app.set_guide_kept(ModelRc::from(state.borrow().develop.guide_kept.clone()));
    install_callbacks(&app, state.clone(), worker.clone());
    // What this machine runs, for the models' list and their offer,
    // asked on a thread now rather than when either is wanted.
    let quiet = state.borrow().window.batch || state.borrow().launch.scripted;
    if !quiet && state.borrow().develop.store.is_some() {
        crate::panel::models::probe(&mut state.borrow_mut(), &app);
    }
    crate::preparing::install(&app);
    // The keys as the user bound them, once the handler and the sheet
    // are wired.
    crate::keys::load(&app, &remembered.keys);
    // The way round the window, once: where the library is and where
    // the keys are, which two testers looked for.
    if first_run && !state.borrow().window.batch {
        app.set_status(first_run_line().into());
    }
    // The pane's entry from what the last check found, and this
    // launch's check if a day has gone.
    crate::update::start(&app, state.borrow().window.batch, &remembered.update);

    // The viewport is rendered inside Slint's frame, on its device.
    {
        let timing = std::env::var_os("GREYCARD_UI_TIMING").is_some();
        let cpu_ops = cli.cpu_ops;
        let state = state.clone();
        let app_weak = app.as_weak();
        // Frames drawn, counted by the minute. A window at rest draws
        // none; one that draws hundreds has something marking it dirty
        // every frame. Said in the log with what was on, so a report
        // of a GPU kept busy by an idle window names the cause.
        // Frames since the count began, those that drew the viewport
        // anew, and those a scope readback asked for: the last two say
        // what a window drawing at rest is drawing for.
        let mut frames: (std::time::Instant, u32, u32, u32) = (std::time::Instant::now(), 0, 0, 0);
        app.window()
            .set_rendering_notifier(move |phase, api| match phase {
                slint::RenderingState::RenderingSetup => {
                    if let slint::GraphicsAPI::WGPU30 { device, queue, .. } = api {
                        // Once per rendering setup, which is once per
                        // device: the line a "black viewport" report needs.
                        // It is kept for a report's GPU field too.
                        let info = device.adapter_info();
                        let gpu = report::gpu_line(&info);
                        tracing::info!("gpu: {gpu}");
                        crate::gpu_marker::device_made();
                        // With no GPU the window draws in software, and
                        // the ops' shaders there take minutes a frame
                        // where their CPU paths take seconds: the ops
                        // go to the CPU, and the status line says so
                        // at the start and once the first develop lands.
                        let in_software = drawing.in_software(info.device_type);
                        // Before the worker has the device: from then
                        // on a resize can race its submits, which
                        // wgpu's own handler would make a panic.
                        crate::gpu_errors::install(device, app_weak.clone());
                        // And a lost device's callback: the quit and the
                        // start again (`gpu_lost`).
                        let teller =
                            WORKER.with(|w| w.borrow().as_ref().map(|w| w.gpu_lost_teller()));
                        if let Some(teller) = teller {
                            crate::gpu_lost::install(device, cpu_ops, teller);
                        }
                        let mut st = state.borrow_mut();
                        if let Some(app) = app_weak.upgrade() {
                            let lines = drawing.lines(info.device_type, &info.name);
                            crate::adapter::show(&app, &mut st, lines, &gpu);
                        }
                        st.develop.gpu = Some(gpu);
                        st.develop.renderer = Some(Renderer::new(device, queue));
                        st.develop.lut_for = None;
                        // The engine's GPU ops run on the same device,
                        // so what they leave is the viewport's. Handed
                        // over before the first file opens, so its
                        // develop runs there too.
                        if crate::adapter::lend_device(in_software, cpu_ops) {
                            WORKER.with(|w| {
                                if let Some(w) = &*w.borrow() {
                                    w.set_gpu(device, queue);
                                }
                            });
                        } else if cpu_ops && !in_software {
                            tracing::info!("engine ops on the CPU (--cpu-ops)");
                        }
                    } else {
                        tracing::error!("gpu: not wgpu 30 ({api:?}); the viewport cannot render");
                        state.borrow_mut().develop.gpu = Some(format!("not wgpu 30 ({api:?})"));
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
                            crate::panel::handed::open(&state, &app, &worker, &paths);
                        })
                    };
                    let row = {
                        let mut st = state.borrow_mut();
                        st.launch.setup_ran = true;
                        let at_start = st.launch.select_at_start.take();
                        at_start.filter(|_| from_finder.is_empty()).map(|i| {
                            row_of(&st, i).or_else(|| cull::nearest_row(&st.views.shown, i))
                        })
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
                            crate::panel::handed::open(&state, &app, &worker, &from_finder);
                        });
                    }
                    match row {
                        Some(Some(row)) => {
                            let app_weak = app_weak.clone();
                            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                                if let (Some(app), Some(state)) =
                                    (app_weak.upgrade(), STATE.with(|s| s.borrow().clone()))
                                {
                                    // With `--also`'s Ctrl+clicks.
                                    crate::panel::browser::open_at_start(&state, &app, row);
                                }
                            });
                        }
                        Some(None) => {
                            if let Some(app) = app_weak.upgrade() {
                                app.set_status(filter::nothing_shown().into());
                            }
                            tracing::warn!("no frames pass the filter");
                            let mut st = state.borrow_mut();
                            if st.window.batch {
                                st.launch.failed = true;
                                let _ = slint::quit_event_loop();
                            }
                        }
                        None => {}
                    }
                }
                slint::RenderingState::BeforeRendering => {
                    frames.1 += 1;
                    if frames.0.elapsed() >= FRAMES_COUNTED_OVER {
                        let (since, n, drawn, for_scopes) =
                            std::mem::replace(&mut frames, (std::time::Instant::now(), 0, 0, 0));
                        // Over the span the frames came in: a window
                        // left at rest for an hour reaches here on its
                        // next frame with that hour behind it.
                        let seconds = since.elapsed().as_secs_f64();
                        if f64::from(n) >= FRAMES_A_SECOND_SAID * seconds
                            && let Some(app) = app_weak.upgrade()
                        {
                            let pictures = state
                                .try_borrow()
                                .map(|st| st.views.thumb_run.is_some())
                                .unwrap_or(false);
                            tracing::info!(
                                "window: {n} frames in the last {:.0} s, {drawn} drawing the \
                                 viewport, {for_scopes} for a scope readback; grid {}, busy {}, \
                                 queue busy {}, pictures coming {pictures}, status \"{}\"",
                                seconds,
                                app.get_grid_open(),
                                app.get_busy(),
                                app.get_queue_busy(),
                                app.get_status(),
                            );
                        }
                    }
                    // On a lost device the viewport draws nothing until
                    // the quit the loss asked for.
                    if crate::gpu_lost::lost() {
                        return;
                    }
                    let Some(app) = app_weak.upgrade() else {
                        return;
                    };
                    let started = std::time::Instant::now();
                    let mut st = state.borrow_mut();
                    let st = &mut *st;
                    // A frame drawn while a folder's pictures come,
                    // for the run's line in the log.
                    if let Some(run) = st.views.thumb_run.as_mut() {
                        run.frame();
                    }
                    // The first frame to draw the rows on screen with
                    // their pictures, and how long since the folder
                    // opened. Asked for no longer than a snapshot waits
                    // for the grid: a view that never says what it
                    // shows is not asked about at every frame after.
                    if let Some(at) = st.views.fill_clock {
                        if crate::panel::browser::screen_filled(st, &app) {
                            tracing::info!(
                                "thumbnails: the rows on screen drawn with their pictures \
                                 {:.2} s from the folder's open",
                                at.elapsed().as_secs_f64()
                            );
                            st.views.fill_clock = None;
                            st.views.fill_over = true;
                        } else if at.elapsed() > crate::panel::browser::GRID_WAIT {
                            st.views.fill_clock = None;
                            st.views.fill_over = true;
                        }
                    }
                    // From the rows on screen, not the whole folder: a
                    // cold root's far frames come only with a scroll.
                    let pictures_in = st.views.fill_over;
                    if let Some(timing) = st.launch.time_scroll.as_mut() {
                        timing.before_frame(pictures_in, &app);
                    }
                    // The first frame to show the all-roots view, or a
                    // list merged, and how long since it was asked for.
                    if let Some((at, what)) = st.views.library.painted.take() {
                        tracing::info!(
                            "library: on screen {:.0} ms after {what}",
                            at.elapsed().as_secs_f64() * 1e3
                        );
                    }
                    let Landing {
                        pending,
                        loupe,
                        kind,
                    } = land_pending(st, &app);
                    if loupe {
                        cull_frame(st, &app, &state);
                        return;
                    }
                    // The frame on show: the crop, or the turned source's
                    // bounds while cropping. A change of size re-centers.
                    // A row under the pointer shows its state instead.
                    let panel = read_edit(&app, &st.develop.edit, st.develop.target);
                    let edit = view_edit(st, &panel);
                    let handles = patch_handles(st, &app, &edit);
                    sync_rows(&st.develop.patch_handles, handles);
                    // The guide's kept stroke follows the view: it is
                    // held in source pixels and mapped here, so a zoom
                    // or a pan between the two strokes moves the line
                    // with the picture.
                    let kept: Vec<Pt> = match st.develop.guiding.first {
                        Some(ends) if !app.get_guide_mode().is_empty() && source_known(st) => {
                            let sw = st.develop.source_size.0 as f32;
                            ends.iter()
                                .map(|&(x, y)| {
                                    let (vx, vy) = source_to_view(st, &app, x / sw, y / sw);
                                    Pt { x: vx, y: vy }
                                })
                                .collect()
                        }
                        _ => Vec::new(),
                    };
                    sync_rows(&st.develop.guide_kept, kept);
                    // The filmstrip's picture of this file follows its turns.
                    if let Some(i) = st.current
                        && st.views.thumb_base.get(i).is_some_and(|b| b.is_some())
                        && crate::rows::is_loaded(st, i)
                    {
                        let (turns, flip) = panel.geometry.shown_turns(st.sidecar(i).turn);
                        if st.views.thumb_shown[i] != Some((turns, flip)) {
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
                    // A frame turned since is drawn turned from that
                    // develop (`drawn_picture`).
                    let ((dw, dh), lag) = drawn_picture(st);
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
                    if frame_size != st.develop.image_size {
                        st.develop.image_size = frame_size;
                        st.develop.center = (frame.size.0 / 2.0, frame.size.1 / 2.0);
                    }
                    if app.get_original_on() != st.develop.original.on {
                        app.set_original_on(st.develop.original.on);
                    }
                    let white_key =
                        match crate::panel::original::shown(st).or(st.develop.peek.as_ref()) {
                            Some(p) => white_key(&p.white_balance),
                            None => (app.get_as_shot(), app.get_temperature(), app.get_tint()),
                        };
                    // A held picture was developed at its own white
                    // balance; the open frame is already the next one.
                    let white = if st.develop.held.is_some() {
                        WhiteBase::IDENTITY.matrix
                    } else {
                        preview_white(st, white_key)
                    };
                    let (vw, vh) = (
                        app.get_view_width().max(1) as u32,
                        app.get_view_height().max(1) as u32,
                    );
                    let zoom = effective_zoom(st, vw, vh);
                    if st.develop.placing.is_none()
                        && let Some(was) = st.develop.show_mask_kept.take()
                    {
                        app.set_show_mask(was);
                    }
                    // The readout, from the one place the zoom is settled
                    // per frame, so it cannot go stale.
                    let label = zoom_label(st.develop.zoom);
                    if app.get_zoom_text() != label {
                        app.set_zoom_text(label);
                    }
                    // The brush's size on the view, per unit of the width.
                    app.set_brush_scale(sw * zoom / app.window().scale_factor());
                    // A picture pixel's side on the view, for the color
                    // pick's square.
                    app.set_pick_scale(zoom / app.window().scale_factor());
                    // The chosen patch's shape on the view, while the
                    // Retouch tab shows: its edge, and where its
                    // feather begins.
                    let (edge, feather) = shown_outlines(st, &app, &edit);
                    if app.get_patch_outline() != edge {
                        app.set_patch_outline(edge.into());
                    }
                    // Those whose fills are still to come, on any tab.
                    let waiting = pending_outlines(st, &app, &edit);
                    if app.get_pending_outline() != waiting {
                        app.set_pending_outline(waiting.into());
                    }
                    if app.get_patch_feather_outline() != feather {
                        app.set_patch_feather_outline(feather.into());
                    }
                    show_shape(st, &app, &edit);
                    let chosen = chosen_shape(st, &app, &edit);
                    // An object's boxes and picks, or a sky's picks:
                    // the chosen one's, or, with the tool in hand, the
                    // one it adds to.
                    let object = match &st.develop.placing {
                        Some(p) => p.component.and_then(|c| {
                            edit.adjustments
                                .get(p.index)
                                .and_then(|a| a.mask.components.get(c))
                        }),
                        None => chosen.as_ref(),
                    }
                    .filter(|_| source_known(st))
                    .and_then(|c| match &c.shape {
                        Shape::Object { picks, boxes } => Some((picks, boxes.as_slice())),
                        Shape::Sky { picks } => Some((picks, &[][..])),
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
                    sync_rows(&st.develop.mask_boxes, boxes);
                    sync_rows(&st.develop.mask_picks, picks);
                    let people = crate::panel::parts::boxes(st, &app);
                    sync_rows(&st.develop.person_boxes, people);
                    let st = &mut *st;
                    let shift = crate::panel::color::local_white_shift(st, white_key);
                    let (mut locals, wants) = bake_locals(
                        &edit,
                        &mut st.develop.rasters,
                        &mut st.develop.learned,
                        sh / sw.max(1.0),
                        shift.as_ref(),
                    );
                    // The refined masks, against the picture of the
                    // open frame's own develop at its turn; asked of
                    // the worker when that develop came without one.
                    let own = crate::panel::viewport::own_develop(st) == Some(0);
                    let guide = st.develop.drawn_guide.clone();
                    crate::panel::mask::refine_locals(
                        &edit,
                        &mut locals,
                        &mut st.develop.refined,
                        guide.picture.as_ref().filter(|_| own),
                        sh / sw.max(1.0),
                        edit.light.exposure,
                        st.develop.drawn_kind.baseline(&edit.light) + edit.light.exposure,
                    );
                    if crate::panel::mask::picture_wanted(st, &edit) {
                        let generation = st.develop.drawn_generation;
                        st.develop.picture_asked = Some(generation);
                        WORKER.with(|w| {
                            if let Some(w) = &*w.borrow() {
                                w.send(Job::MaskPicture {
                                    edit: Box::new(edit.clone()),
                                    generation,
                                });
                            }
                        });
                    }
                    ask_for(st, &app, wants);
                    // A screenshot waits for the masks asked for.
                    // A snapshot waits for the develop; one of the
                    // grid waits for the pictures the grid shows, or
                    // it would catch a sheet of empty cells.
                    // A shape the worker answered by failing, for the
                    // file open now, is answered.
                    let quiet = st.develop.asked.iter().all(|(key, shape)| {
                        st.develop
                            .masks_failed
                            .get(key)
                            .is_some_and(|(s, f)| s == shape && *f == st.develop.learned_of)
                    }) && st.develop.part_pending.is_none()
                        && st.develop.landscape_pending.is_none()
                        && grid_filled(st, &app);
                    // A timed slider move is sent as its frame starts.
                    if pending.is_some() {
                        time_moves(st, &app);
                        // The frame that shows the frame the key
                        // landed on: the switch's time, for the log
                        // and `--time-select`.
                        if let Some(at) = st.views.selected_at.take() {
                            let ms = at.elapsed().as_secs_f64() * 1e3;
                            tracing::debug!("select to develop: {ms:.1} ms");
                            let row = st
                                .current
                                .and_then(|c| cull::row_of_shown(&st.views.shown, c));
                            let count = st.views.shown.len();
                            time_select(&mut st.launch.time_select, &app, ms, row, count);
                        }
                        // `--snapshot-placeholder`: the capture is
                        // taken and the develop it stood in for has
                        // landed, so the run ends here rather than in
                        // the middle of that develop.
                        if st.launch.snapshot_placeholder
                            && st.launch.snapshot.is_none()
                            && st.window.batch
                        {
                            let _ = slint::quit_event_loop();
                        }
                        // And, before it is taken: on to the next
                        // frame, from the event loop, so the capture
                        // is of its camera picture standing in for its
                        // develop rather than of this develop.
                        if st.launch.snapshot_placeholder && st.launch.snapshot.is_some() {
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
                    show_source_shape(st, &app);
                    if app.get_crop_mode() {
                        // An aspect held on the panel draws its box,
                        // though the edit and the picture keep theirs
                        // until the crop is touched.
                        if app.get_aspect_held() {
                            let held = crate::panel::crop::read_geometry(&app);
                            show_crop_on_view(st, &app, &held);
                        } else {
                            show_crop_on_view(st, &app, &edit.geometry);
                        }
                    }
                    // The look table, read once per choice; the export
                    // resolves the same name through the same cache and
                    // the same gate, so the two agree: a table fitted
                    // under another display curve than the picture's is
                    // no look in either. A look previewed from the list
                    // is drawn in its place while it stands.
                    let look_key = (edit.look_lut.clone(), edit.display_curve);
                    if st.develop.look_for.as_ref() != Some(&look_key) {
                        st.develop.look = edit
                            .look_lut
                            .look_under(&greycard_edit_io::Stores::user(), edit.display_curve);
                        st.develop.look_for = Some(look_key);
                    }
                    let look = crate::panel::look_preview::drawn(st, &app, &edit);
                    let Some(renderer) = st.develop.renderer.as_mut() else {
                        return;
                    };
                    sync_display(
                        &mut st.develop.lut_for,
                        &mut st.develop.encoded_lut_for,
                        &st.develop.monitors,
                        &app,
                        renderer,
                    );
                    renderer.set_look(look.as_ref());
                    // The look's fields from `with_look`, the one place
                    // they are read from an edit, named one by one with
                    // every other field, so that a field added to `View`
                    // is a compile error here rather than blank's value.
                    let View {
                        light,
                        mixer,
                        color,
                        bw,
                        tint,
                        curves,
                        display_curve,
                        vignette,
                        grain,
                        output_sharpen,
                        ..
                    } = View::with_look(&edit);
                    let view = View {
                        zoom,
                        center: st.develop.center,
                        light,
                        mixer,
                        color,
                        bw,
                        tint,
                        curves,
                        vignette,
                        grain,
                        output_sharpen,
                        matrix: edit.geometry.matrix(),
                        persp: edit.geometry.perspective(sw, sh),
                        plane: edit.geometry.plane_size(sw, sh),
                        cubic: edit.geometry.resamples(),
                        frame_origin: frame.origin,
                        frame_size: frame.size,
                        white,
                        locals,
                        warn: render::Warn {
                            shadows: app.get_warn_shadows(),
                            highlights: app.get_warn_highlights(),
                        },
                        peaking: peaking_of(&app),
                        show_mask: crate::panel::mask::shown_mask(&app, st.develop.target),
                        show_sharpen: app.get_show_sharpen_mask() && app.get_sharpen(),
                        mask_alone: false,
                        weigh_by: None,
                        canvas: render::canvas_rgb(app.get_canvas_choice()),
                        source: kind,
                        display_curve,
                        source_turn: lag,
                    };
                    // The navigator's rectangle: what the view shows of
                    // the frame, as fractions of it.
                    {
                        let (fw, fh) = (frame.size.0.max(1.0), frame.size.1.max(1.0));
                        let (shown_w, shown_h) = (vw as f32 / zoom, vh as f32 / zoom);
                        let partial = shown_w < fw - 0.5 || shown_h < fh - 0.5;
                        app.set_nav_partial(partial);
                        if partial {
                            let left = ((st.develop.center.0 - shown_w / 2.0) / fw).clamp(0.0, 1.0);
                            let top = ((st.develop.center.1 - shown_h / 2.0) / fh).clamp(0.0, 1.0);
                            let right =
                                ((st.develop.center.0 + shown_w / 2.0) / fw).clamp(0.0, 1.0);
                            let bottom =
                                ((st.develop.center.1 + shown_h / 2.0) / fh).clamp(0.0, 1.0);
                            app.set_nav_left(left);
                            app.set_nav_top(top);
                            app.set_nav_width(right - left);
                            app.set_nav_height(bottom - top);
                        }
                    }
                    let (texture, drew) = renderer.render(vw, vh, &view);
                    if let Some(at) = st.develop.turn_pressed.take() {
                        tracing::info!(
                            "turn: on screen {:.0} ms after the key, the develop there read \
                             through {lag} quarter turns",
                            at.elapsed().as_secs_f64() * 1e3
                        );
                    }
                    // The scopes weighed by the chosen adjustment's
                    // mask, when the panel's selection toggle asks and
                    // there is a mask to weigh by.
                    let selection = st
                        .develop
                        .target
                        .filter(|&k| view.locals.get(k).is_some_and(|l| !l.mask.is_empty()));
                    app.set_scope_mask_active(selection.is_some());
                    let weigh_by = selection.filter(|_| app.get_scope_selection());
                    let (bins, in_flight) = renderer.analyze(&view, st.develop.scope, weigh_by);
                    let fresh = bins.map(|(_, b)| b.to_vec());
                    if let Some((taken, bins)) = bins {
                        app.set_scope_picture(scope::draw(taken, bins));
                    }
                    let picture = renderer.take_navigator();
                    let new_picture = picture.is_some();
                    if let Some(picture) = picture {
                        if let Some(path) = std::env::var_os("GREYCARD_UI_NAVIGATOR")
                            && let Some(img) = image::RgbaImage::from_raw(
                                picture.width,
                                picture.height,
                                picture.as_bytes().to_vec(),
                            )
                            && let Err(e) = img.save(&path)
                        {
                            tracing::warn!("navigator {}: {e}", Path::new(&path).display());
                        }
                        // The frame as shown, a peek's too, as the
                        // scopes have it: the navigator keeps one
                        // height whatever its picture's shape, so
                        // nothing under the pointer moves.
                        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                            picture.as_bytes(),
                            picture.width,
                            picture.height,
                        );
                        app.set_navigator(slint::Image::from_rgba8(buf));
                    }
                    if in_flight {
                        // The bins arrive a frame later; have that frame.
                        frames.3 += 1;
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
                        && !st.launch.awaiting_turn
                        && !st.launch.awaiting_index
                        && !st.views.library.awaiting
                        && !st.launch.snapshot_placeholder;
                    schedule_snapshot(
                        &mut st.launch.snapshot,
                        &mut st.launch.panel_scroll,
                        &mut st.launch.snapshot_shown,
                        &app,
                        &state,
                        ready,
                        true,
                    );
                    write_screenshot(
                        &mut st.launch.screenshot,
                        &mut st.launch.failed,
                        renderer,
                        &texture,
                        ready,
                    );
                    // A new image only for a frame drawn anew: the same
                    // one handed over again would mark the window dirty
                    // and have it draw another frame, without end.
                    if drew {
                        frames.2 += 1;
                        app.set_texture(
                            slint::Image::try_from(texture).expect("the texture imports"),
                        );
                    }
                    if let Some(b) = fresh {
                        bins_read(st, &app, b);
                    }
                    // A new analysis picture under a resting pointer:
                    // the color there has changed, or the scope has.
                    // And a frame drawn anew under it: a zoom, a pan,
                    // the fit or a resize puts another spot there.
                    if new_picture || (drew && st.develop.hover.is_some()) {
                        show_hover(st, &app);
                    }
                    // The variable is the gate; the line is kept at info so
                    // asking for it is enough to get it.
                    if timing {
                        tracing::info!(
                            "frame: {:.2} ms{}{}",
                            started.elapsed().as_secs_f64() * 1e3,
                            if pending.is_some() {
                                " (with upload)"
                            } else {
                                ""
                            },
                            if drew { " (drawn)" } else { "" }
                        );
                    }
                }
                slint::RenderingState::AfterRendering => {
                    if let Some(app) = app_weak.upgrade()
                        && let Some(timing) = state.borrow_mut().launch.time_scroll.as_mut()
                    {
                        timing.after_frame(&app);
                    }
                }
                slint::RenderingState::RenderingTeardown => {
                    state.borrow_mut().develop.renderer = None;
                }
                _ => {}
            })
            .context("Slint's rendering notifier")?;
    }

    state.borrow_mut().launch.preview_temperature = cli.preview_temperature;
    // Open the folder to start on, if there is one yet: listed and its
    // sidecars found off the window's thread once the loop runs, and
    // its frame opened from the rendering setup when the list is in
    // before it, so the first develop takes the same path as the rest
    // of the session.
    if !files.is_empty() {
        let app_weak = app.as_weak();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            let (Some(app), Some(state), Some(worker)) = (
                app_weak.upgrade(),
                STATE.with(|s| s.borrow().clone()),
                WORKER.with(|w| w.borrow().clone()),
            ) else {
                return;
            };
            crate::roots::open_listing(
                &state,
                &app,
                &worker,
                crate::roots::Listing::Files(files),
                select,
            );
        });
    } else if state.borrow().views.library.wanted.is_none() {
        // Nothing to open yet: ask the desktop for a folder once the
        // event loop is running. Canceled, the empty editor stays up
        // with its Open folder button. The all-roots view asked for
        // is something to open.
        let app_weak = app.as_weak();
        let start = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        // From a timer, which fires once the loop runs: a launch from
        // the Finder has handed over its files by then, and the
        // chooser is not wanted over them.
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if finder::arrived() {
                return;
            }
            chooser::choose_folder(tr!("Open a folder"), start, move |chosen| {
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
                            app.set_status(chooser::chooser_failed(&e).into());
                        }
                    }
                });
            });
        });
    }
    // The panel's choices go to the file as they change, and they and
    // a resting edit once more as the loop exits: a Mac's Quit ends
    // the process inside the loop, so nothing after `app.run()` runs.
    let choices = (cli.screenshot.is_none() && cli.snapshot.is_none()).then(|| {
        let choices = Choices::new(&app, &state.borrow(), set_aside, cli.hide_panels);
        let (app_weak, held, at_exit) = (app.as_weak(), state.clone(), choices.clone());
        AT_EXIT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let (Some(app), Ok(mut st)) = (app_weak.upgrade(), held.try_borrow_mut()) else {
                    return;
                };
                // An edit still resting on its save timer would be lost
                // with the process: written now, as the timer would.
                if st.develop.save_timer.running() {
                    st.develop.save_timer.stop();
                    let edit = read_edit(&app, &st.develop.edit, st.develop.target);
                    save_edit(&mut st, edit);
                }
                at_exit.write(&app, &st, true);
                // The collections' and stacks' copies asked for, a
                // moment's wait: one held up goes at the next launch.
                if let Some(c) = &st.views.collections.carrier {
                    c.wait(crate::carried::QUIT_WAIT);
                }
            }));
        });
        (keep_choices(&app, &state, &choices), choices)
    });
    // An event loop that ends in an error (the compositor gone: a
    // logout, the shell crashing) still quits as a closed window does,
    // with every save below and the worker out of the driver, and the
    // error is the run's at the end.
    // A panic of Slint's drawing on a lost device ends the loop as a
    // quit would, so the saves below and the start again still come.
    let ran = match crate::gpu_lost::survive(|| app.run(), Ok(())) {
        // The surface's error on the lost device is the loss's, said.
        Err(e) if crate::gpu_lost::lost() => {
            tracing::warn!("the window's event loop ended on the lost device: {e}");
            Ok(())
        }
        ran => ran,
    };
    crate::gpu_lost::leaving();
    // If the loop's exit did not already.
    crate::panel::handed::leave();
    // An event loop that ended before any frame, on DX12 we chose, is
    // the device refused there (`gpu_marker`).
    crate::gpu_marker::run_ended();
    if let Err(e) = &ran {
        tracing::error!("the window's event loop: {e}");
    }
    // A title typed and not yet sent, the window closed under it: it
    // is the frames' it was typed for, as a click away would have
    // made it. First, while the index is open to hear of the saves;
    // a frame still standing in is read for it here.
    crate::panel::words::send_title(&state, &app);
    // A batch of sidecars still being saved is waited for while it
    // moves, and landed while the index and the worker are there to
    // hear of it.
    crate::panel::saves::finish(&mut state.borrow_mut(), Some(crate::sync::QUIT_WRITE_WAIT));
    // An import running stops as Stop stops it, after the file in
    // hand, which lands whole rather than being cut off with the
    // process.
    crate::panel::import::leave(
        &mut state.borrow_mut().import,
        crate::panel::import::CLOSE_WAIT,
    );
    // The window is closed: the worker finishes what it is in the
    // middle of and puts its buffers down before the process goes.
    // One still in a job after `LEAVING` is left in it, and the run
    // ends below without the C library's exit.
    worker.stop();
    // And the index's two connections close, the indexer's pass
    // stopping at its next batch, so the write-ahead log and its
    // index are folded in and removed rather than left beside the
    // library.
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = None;
        // The watcher first, which holds a way to the indexer's
        // thread.
        st.views.library.watcher = None;
        st.views.library.poll = None;
        if let Some(indexer) = st.views.index.take() {
            indexer.stop(worker::LEAVING);
        }
    }

    // The panel's choices, for the next run. Not from a screenshot or
    // a snapshot, which should leave the user's alone. Written as they
    // change too (`keep_choices`); this write is for the last second.
    if let Some((_, choices)) = &choices {
        choices.write(&app, &state.borrow(), true);
    }

    // The open file's edit, as left.
    let edit = {
        let st = state.borrow();
        read_edit(&app, &st.develop.edit, st.develop.target)
    };
    save_at_quit(&mut state.borrow_mut(), edit, crate::sync::QUIT_WRITE_WAIT);
    // A lost device's run starts again on the CPU's ops, now that its
    // saves are written, on the file open now; a scripted run fails.
    let open_now = {
        let st = state.borrow();
        st.current
            .and_then(|c| st.file_at(c))
            .and_then(|f| st.files.get(f).cloned())
    };
    let scripted = cli.snapshot.is_some() || cli.screenshot.is_some() || cli.keys.is_some();
    let lost_scripted =
        crate::gpu_lost::start_again(cli.path.as_deref(), open_now.as_deref(), scripted);
    // A batch run whose picture never developed is a failure a script
    // can see, as is a window lost under the run.
    let failed = state.borrow().launch.failed || ran.is_err() || lost_scripted;
    // A worker still in a job (a develop of a large frame, a mask, the
    // learned denoiser, each of which can outlast `LEAVING`) may be
    // inside the GPU driver, waiting on a fence in `Device::poll`.
    // Returning from here runs the C library's exit handlers, which
    // tear NVIDIA's driver down under it, and the process dies with
    // SIGSEGV on the worker's thread. So it ends by `_exit` instead,
    // which runs none of them: the kernel ends every thread where it
    // is. What this quit saves is written above (the import's file
    // in hand, the index closed, the settings, the open frame's
    // sidecar), each by a write that has returned. The archive writes
    // are made or noted: a followed frame's write to its copy that
    // outlasted its wait (`save_at_quit`), and an archive job still
    // out, go on on threads of their own, each holding a connection to
    // the index, and end where they are with the process, their rows
    // noted waiting; each writes its file through a temporary and a
    // rename, so the copy is the old save or the new, whole. What is
    // lost is the job in hand, which leaving lost before too. A worker
    // that has ended by now, the usual case, leaves the process to end
    // as it always has, unless a model was opened: then ONNX Runtime's
    // own exit handler would tear Dawn's Vulkan instance down after
    // the driver, the crash `greycard_ai::runtime::leave` is about, and
    // the quit ends by `_exit` too.
    if !worker.finished() || greycard_ai::runtime::loaded() {
        if let Err(e) = &ran {
            eprintln!("Error: {e:?}");
        }
        let index = state.borrow().views.index_path.clone();
        if let Some(index) = index {
            fold_library_log(&index, LOG_FOLD_WAIT);
        }
        return crate::headless::leave(u8::from(failed));
    }
    ran?;
    Ok(if failed {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    })
}

/// The editor this user has running, reached or become, for a launch
/// whose command line is a path alone or nothing; none for every other
/// run (an export, a capture, a measurement), which keeps a window of
/// its own or none and is never reached by another launch. A path that
/// is not there is not handed over: this launch goes on and says so.
fn claim(cli: &Cli) -> Option<instance::Claimed> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if !instance::plain_launch(&args) {
        return None;
    }
    let paths = instance::absolute(cli.path.as_deref())?;
    // Keyed by the configuration folder, so a run with one of its own
    // never reaches the user's editor.
    let config = settings::path();
    let address = instance::Address::user(config.as_deref().and_then(Path::parent))?;
    Some(address.claim(&instance::Launch::here(paths)))
}

/// The claim said in the log, now that it is open, and the editor to
/// serve when this launch became it.
fn said(claimed: instance::Claimed) -> Option<instance::Instance> {
    if claimed.stale_removed {
        tracing::info!("stale instance socket removed: the last editor did not quit cleanly");
    }
    match claimed.claim {
        instance::Claim::Instance(instance) => {
            tracing::info!("became the instance at {}", instance.address());
            Some(instance)
        }
        instance::Claim::Alone(e) => {
            tracing::warn!("no single instance, this window on its own: {e}");
            None
        }
        instance::Claim::Handed(_) => None,
    }
}

/// A histogram read back from the analysis: the backdrop of the curve
/// editor, whichever look it edits, and the clipping lamps at the
/// histogram's corners. A mask's raster landing, or a new weighing,
/// changes the analysis's key, so it comes this way with no touch of
/// the panel.
pub(crate) fn bins_read(st: &mut State, app: &App, bins: Vec<u32>) {
    app.set_curve_image(draw_curve(app, Some(bins.as_slice())));
    let clipping = scope::Clipping::of(&bins);
    let mark = |chans: [bool; 3]| {
        let c = scope::Clipping::mark(chans).unwrap_or([0; 3]);
        slint::Color::from_rgb_u8(c[0], c[1], c[2])
    };
    app.set_clip_shadows_lit(clipping.shadows.iter().any(|&c| c));
    app.set_clip_shadows_mark(mark(clipping.shadows));
    app.set_clip_highlights_lit(clipping.highlights.iter().any(|&c| c));
    app.set_clip_highlights_mark(mark(clipping.highlights));
    st.develop.bins = Some(bins);
}

/// The quit's saves, the window's index reader closed: the open
/// frame's edit as left; a frame followed onto its archive copy, its
/// own root away, has no file here for that save, so its write goes
/// now, waited for at most `wait` and noted waiting in the index if it
/// has not ended; and every other frame's archive write that never
/// went out, this save's among them, is noted waiting for the next
/// window, since no job sent now would land.
/// What the followed frames' writes came to, for a test.
/// How long a quit that ends by `_exit` waits for the library's last
/// other connection to close, so that its own close folds the log in.
const LOG_FOLD_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// The library's write-ahead log folded into the file and removed,
/// before a quit that ends by `_exit`. The index's own connections are
/// closed by then, but a reader on a thread of its own (a folder's
/// look, the tree's counts) can still hold one for some milliseconds,
/// and the last connection to close is the one that folds the log in
/// and removes it with its index; `_exit` closes that reader's for it,
/// with no fold, and the `-wal` and `-shm` were left beside the
/// library (where the return path, slower to end, used to give the
/// reader time to close first). So a connection of our own is opened,
/// checkpointed and closed until the log is gone, up to `within`. A
/// reader still holding a snapshot after that keeps the checkpoint
/// from folding the frames past it: the `-wal` and `-shm` are left,
/// and the library's next open recovers them, as it does after any
/// end that skipped the last close. The file is never whole without
/// them then.
fn fold_library_log(index: &Path, within: std::time::Duration) {
    let mut log = index.as_os_str().to_owned();
    log.push("-wal");
    let log = PathBuf::from(log);
    let asked = std::time::Instant::now();
    while log.exists() {
        match greycard_library::Library::open(index) {
            Ok(lib) => {
                // Short: the loop is the wait, not the busy handler.
                let _ = lib.set_busy_timeout(std::time::Duration::from_millis(50));
                let _ = lib.checkpoint_all();
            }
            Err(e) => {
                tracing::warn!("library: not folded in at the quit: {e}");
                return;
            }
        }
        if !log.exists() {
            return;
        }
        if asked.elapsed() >= within {
            tracing::info!(
                "library: another connection still open at the quit; its log is left beside \
                 it, for the next open to recover"
            );
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

pub(crate) fn save_at_quit(
    st: &mut State,
    edit: greycard_edit::Edit,
    wait: std::time::Duration,
) -> crate::sync::QuitWrites {
    save_edit(st, edit);
    // A batch of sidecars out is waited for while it moves; one that
    // has stopped (a share that stopped answering) is left, its files
    // not yet renamed keeping their old sidecars.
    crate::panel::saves::finish(st, Some(wait));
    let followed = crate::sync::write_followed_at_quit(st, wait);
    crate::sync::note_unsent_at_quit(st);
    followed
}

/// The monitor's profile as the panel has it, and where it is from,
/// for the panel's note: the color manager's for the monitor named
/// (the primary when none is, or the name is stale), a standard, or
/// a file.
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
                    tr!("{}'s profile from {}", m.model, display::SYSTEM_SOURCE),
                ),
                None => (
                    MonitorProfile::Srgb,
                    tr!("{} knows no display profile", display::SYSTEM_SOURCE),
                ),
            }
        }
        FILE => {
            let file = app.get_display_file();
            if file.is_empty() {
                return (MonitorProfile::Srgb, tr!("No profile chosen"));
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
        return tr!(
            "{from}: nothing changed on the way to the screen",
            from = from
        );
    }
    match display::Lut3d::build(export::Space::Srgb, monitor, None) {
        Ok(lut) => {
            let d = lut.max_deviation() * 255.0;
            tracing::info!(
                "display profile {} ({from}): at most {d:.1} of 255 from sRGB",
                monitor.key()
            );
            tr!(
                "{from}: up to {d} of 255 from sRGB",
                from = from,
                d = format!("{d:.0}")
            )
        }
        Err(e) => {
            tracing::warn!(
                "display profile {} ({from}): {e:#}; sRGB output",
                monitor.key()
            );
            tr!(
                "{from}: cannot be read; nothing changed on the way to the screen",
                from = from
            )
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

/// `--develop-temperature`, `--exposure`, and `--agx`: what the
/// command line lays over the first frame's edit (`Overrides`).
pub(crate) fn overrides_of(cli: &Cli) -> Overrides {
    Overrides {
        temperature: cli.develop_temperature,
        exposure: cli.exposure,
        display_curve: cli.agx.then_some(greycard_edit::DisplayCurve::Agx),
    }
}

/// The export sheet to open with, and the preset it is: the last
/// run's, or the preset `--export-preset` names, with `--long-edge`
/// and `--on-exists` over either (`Shared::opening_sheet`).
pub(crate) fn opening_sheet(
    cli: &Cli,
    remembered: &settings::Settings,
) -> Result<(sheet::Sheet, Option<String>)> {
    remembered
        .shared
        .opening_sheet(cli.export_preset.as_deref(), cli.long_edge, cli.on_exists)
}

/// The scope to open on: the command line's, else the last run's.
pub(crate) fn opening_scope(cli: &Cli, remembered: &settings::Settings) -> scope::Scope {
    cli.scope
        .as_deref()
        .and_then(scope::Scope::from_name)
        .or_else(|| scope::Scope::from_name(&remembered.scope))
        .unwrap_or_default()
}

/// Whether a launch puts back the filter the last session left: not
/// for a batch run (a snapshot, a screenshot), and not when
/// the command line names a file rather than a folder.
pub(crate) fn restores_filter(cli: &Cli) -> bool {
    let batch = cli.snapshot.is_some() || cli.screenshot.is_some();
    let names_a_file = cli.path.as_deref().is_some_and(|p| !p.is_dir());
    !batch && !names_a_file
}

/// Where the export chooser opens at the start, from the file.
pub(crate) fn remembered_folder(settings: &settings::Settings) -> Option<PathBuf> {
    (!settings.export_last_folder.is_empty()).then(|| PathBuf::from(&settings.export_last_folder))
}

/// What the state owns of the settings, not the panel: put on the
/// settings the panel's choices made, for the file at close.
pub(crate) fn take_state_choices(settings: &mut settings::Settings, st: &State) {
    settings.mat = st.develop.mat;
    settings.shared.export_presets = st.work.export_presets.clone();
    settings.export_last_folder = st
        .work
        .export_last_folder
        .as_ref()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
}

/// How often the panel's choices are compared with those last written.
pub(crate) const CHOICES_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

thread_local! {
    /// The choices' last write, run as winit's loop exits: on a Mac's
    /// Quit the process ends there, `app.run()` never returning.
    static AT_EXIT: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// The loop is exiting: the choices are written while the window is
/// still here to read them.
pub(crate) fn exiting() {
    // First: a launch from here on starts an editor of its own rather
    // than hand its path to this one, now closing.
    crate::panel::handed::leave();
    if let Some(hook) = AT_EXIT.with(|h| h.borrow_mut().take()) {
        hook();
    }
}

/// The choices the panel and the state own, as the file keeps them.
fn gathered(app: &App, st: &State) -> settings::Settings {
    let mut settings = remember(app);
    take_state_choices(&mut settings, st);
    settings.filter = filter::Saved::of(&st.views.filter);
    settings
}

/// `gathered` over the file's `kept`: every field the panel does not
/// own is the file's, so a field another path wrote as it changed is
/// never put back by a stale copy here. `panes_from_file` keeps the
/// file's panes, for a run opened with `--hide-panels`.
fn merged(
    mut settings: settings::Settings,
    kept: settings::Settings,
    panes_from_file: bool,
) -> settings::Settings {
    settings.shared.last_file = kept.shared.last_file;
    settings.recent_folders = kept.recent_folders;
    settings.start_folder = kept.start_folder;
    settings.start_view = kept.start_view;
    settings.filter_bar = kept.filter_bar;
    settings.grid_wheel = kept.grid_wheel;
    settings.folder_tree_subfolders = kept.folder_tree_subfolders;
    settings.profiles_all_cameras = kept.profiles_all_cameras;
    settings.shared.xmp_sidecars = kept.shared.xmp_sidecars;
    settings.shared.sidecars_in_folder = kept.shared.sidecars_in_folder;
    settings.shared.jpegs_apart = kept.shared.jpegs_apart;
    settings.shared.lens_on_new = kept.shared.lens_on_new;
    settings.shared.author = kept.shared.author;
    settings.shared.copyright = kept.shared.copyright;
    settings.one_section = kept.one_section;
    settings.exported = kept.exported;
    settings.edit_in = kept.edit_in;
    settings.cull_move_on = kept.cull_move_on;
    settings.loupe_cull = kept.loupe_cull;
    settings.cull_developed = kept.cull_developed;
    settings.sort = kept.sort;
    settings.lenses_declined = kept.lenses_declined;
    settings.models_asked = kept.models_asked;
    settings.rawtherapee_answered = kept.rawtherapee_answered;
    settings.preset_favorites = kept.preset_favorites;
    settings.preset_folds = kept.preset_folds;
    settings.thumb_cache_mb = kept.thumb_cache_mb;
    settings.preview_cache_mb = kept.preview_cache_mb;
    settings.network_poll_minutes = kept.network_poll_minutes;
    settings.import = kept.import;
    settings.update = kept.update;
    settings.match_unchecked = kept.match_unchecked;
    settings.crop_start = kept.crop_start;
    settings.crop_last = kept.crop_last;
    settings.fill_choice = kept.fill_choice;
    settings.ui_scale = kept.ui_scale;
    settings.keys = kept.keys;
    settings.settings_category = kept.settings_category;
    settings.theme = kept.theme;
    settings.scroll_bars = kept.scroll_bars;
    settings.scroll_bar_size = kept.scroll_bar_size;
    settings.language = kept.language;
    settings.windows = kept.windows;
    settings.ui_scale_system =
        crate::window_fit::scale::system_seen().unwrap_or(kept.ui_scale_system);
    if panes_from_file {
        settings.hide_left = kept.hide_left;
        settings.hide_right = kept.hide_right;
        settings.hide_strip = kept.hide_strip;
    }
    settings
}

/// The panel's choices as last written, shared by the timer that
/// writes them as they change and the close's write.
#[derive(Clone)]
pub(crate) struct Choices {
    last: Rc<RefCell<settings::Settings>>,
    /// The file would not parse at the start and was set aside: until
    /// something writes it again the close writes nothing, the
    /// defaults on show being no one's choice.
    set_aside: bool,
    panes_from_file: bool,
}

impl Choices {
    /// As the run starts; `set_aside` from `Settings::load_at_start`.
    pub(crate) fn new(app: &App, st: &State, set_aside: bool, panes_from_file: bool) -> Self {
        Choices {
            last: Rc::new(RefCell::new(gathered(app, st))),
            set_aside,
            panes_from_file,
        }
    }

    /// Written into the file as it stands when a choice has changed
    /// since the last write, and at the close (`closing`) unless held.
    /// Merged over the file there, so the close keeps what another
    /// path wrote and what only it writes (the system's scale).
    pub(crate) fn write(&self, app: &App, st: &State, closing: bool) {
        let now = gathered(app, st);
        let held =
            self.set_aside && settings::path().is_some_and(|p| !settings::Settings::written(&p));
        if *self.last.borrow() == now && (!closing || held) {
            return;
        }
        tracing::debug!("settings: the panel's choices written");
        let panes = self.panes_from_file;
        settings::Settings::change(|file| {
            *file = merged(now.clone(), std::mem::take(file), panes);
            // The window as it closes, at the close alone: not as it is
            // dragged, for a write a second.
            if closing {
                crate::window_fit::kept::store(&mut file.windows);
            }
        });
        *self.last.borrow_mut() = now;
    }
}

/// How long the window's frames are counted over before the count is
/// weighed, and the rate over that span from which it is said in the
/// log: five a second, averaged, where a window at rest draws none.
const FRAMES_COUNTED_OVER: std::time::Duration = std::time::Duration::from_secs(60);
const FRAMES_A_SECOND_SAID: f64 = 5.0;

/// A timer that writes the panel's choices whenever they differ from
/// the last written, so a run that ends without the close's write (a
/// Mac's Quit, a crash) keeps them. Polled rather than wired to each
/// control, so a choice added to `remember` is kept with no more work.
pub(crate) fn keep_choices(app: &App, state: &Rc<Shared>, choices: &Choices) -> slint::Timer {
    let (app_weak, state, choices) = (app.as_weak(), state.clone(), choices.clone());
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, CHOICES_EVERY, move || {
        let (Some(app), Ok(st)) = (app_weak.upgrade(), state.try_borrow()) else {
            return;
        };
        choices.write(&app, &st, false);
    });
    timer
}

/// What the last run painted over the picture, the clipping warnings
/// and focus peaking; `clipping` (the command line's) turns both
/// warnings on whatever it left.
pub(crate) fn show_overlays(app: &App, clipping: bool, remembered: &settings::Settings) {
    app.set_warn_shadows(clipping || remembered.warn_shadows);
    app.set_warn_highlights(clipping || remembered.warn_highlights);
    app.set_peaking(remembered.peaking);
}

/// Focus peaking's paint, the theme's, when it is on.
pub(crate) fn peaking_of(app: &App) -> Option<[f32; 3]> {
    app.get_peaking().then(|| {
        let c = app.global::<crate::Theme>().get_peaking();
        [c.red(), c.green(), c.blue()].map(|v| v as f32 / 255.0)
    })
}

/// The panel's choices, to keep for the next run. What the panel
/// does not own — the last file, the Settings sheet's two and the
/// lens offer's answer, each written to the file when it changes —
/// is put back from the file by the caller.
pub(crate) fn remember(app: &App) -> settings::Settings {
    settings::Settings {
        shared: settings::Shared {
            export: read_sheet(app),
            // Kept by the caller: the list is the state's, not the panel's.
            export_presets: Vec::new(),
            export_preset: chosen_preset(app).unwrap_or_default(),
            // Kept apart, all of them: the last file is written as soon
            // as a file develops, the sidecars' two as the Settings sheet
            // changes them. The caller fills them in from disk.
            xmp_sidecars: false,
            sidecars_in_folder: false,
            jpegs_apart: false,
            // As the Settings sheet's switch is flipped.
            lens_on_new: true,
            last_file: String::new(),
            // Kept as the sheet's fields take them: the caller keeps
            // the file's.
            author: String::new(),
            copyright: String::new(),
        },
        // Kept by the caller: written when a chooser answers.
        export_last_folder: String::new(),
        // Written as the first export starts; the caller keeps the file's.
        exported: false,
        // Written as an app is added or taken off; the caller keeps the
        // file's.
        edit_in: Vec::new(),
        scope: app.get_scope().into(),
        curve_mode: app.get_curve_mode().into(),
        warn_shadows: app.get_warn_shadows(),
        warn_highlights: app.get_warn_highlights(),
        peaking: app.get_peaking(),
        proof_profile: proof_settings(app)
            .map(|p| p.profile.key())
            .unwrap_or_default(),
        proof_intent: app.get_proof_intent().into(),
        gamut_warning: app.get_gamut_warning(),
        display_profile: display_key(app),
        display_monitor: app.get_display_monitor().to_string(),
        canvas_color: render::canvas_name(app.get_canvas_choice()).to_string(),
        // The state's, put in by `take_state_choices`.
        mat: false,
        collapsed: read_folds(app),
        shares: crate::panel::shares::read(app),
        // Written as the switch is flipped; the caller keeps the file's.
        one_section: false,
        grid_cell: app.get_grid_cell(),
        // Written as the switch is flipped; the caller keeps the file's.
        grid_wheel: true,
        lightbox_across: app.get_lightbox_across(),
        range_sample: app.get_range_sample(),
        // The lightbox puts the panes away for as long as it is on:
        // what is kept is how they were before it.
        hide_left: if app.get_lightbox() {
            app.get_lightbox_left_was()
        } else {
            app.get_left_hidden()
        },
        hide_right: if app.get_lightbox() {
            app.get_lightbox_right_was()
        } else {
            app.get_right_hidden()
        },
        hide_strip: if app.get_lightbox() {
            app.get_lightbox_strip_was()
        } else {
            app.get_strip_hidden()
        },
        // The sizes stand while their panes are away, the lightbox's
        // or the keys'.
        left_width: crate::panel::sizes::left(app),
        right_width: crate::panel::sizes::right(app),
        strip_photo: crate::panel::sizes::strip(app),
        // Kept apart, both: culling's switch as it is flipped, and the
        // lens and model offers' answers as they are given. The caller
        // fills them in from disk.
        cull_move_on: false,
        // Written as the Loupe's mode and Cull's switch change.
        loupe_cull: false,
        cull_developed: false,
        lenses_declined: false,
        models_asked: false,
        // Written as the RawTherapee offer is answered.
        rawtherapee_answered: Vec::new(),
        // Written as a star or a fold changes; the caller keeps the file's.
        preset_favorites: Vec::new(),
        preset_folds: Vec::new(),
        // Written as each folder opens.
        recent_folders: Vec::new(),
        // Written as the Settings sheet's rows are chosen.
        start_folder: String::new(),
        start_view: settings::START_VIEWS[0].into(),
        filter_bar: true,
        // Written as the tree's switch is flipped.
        folder_tree_subfolders: false,
        // Written as the profile list's switch is flipped.
        profiles_all_cameras: false,
        // Not the panel's either: the settings file is where it is set.
        thumb_cache_mb: 0,
        preview_cache_mb: 0,
        network_poll_minutes: 0,
        // Written as an import starts.
        import: settings::ImportChoices::default(),
        // The state's, filled in by the caller.
        filter: filter::Saved::default(),
        // Written as a view's order is chosen.
        sort: crate::sort::Kept::default(),
        // Written as the check answers and as the switch flips.
        update: crate::update::Kept::default(),
        // Written as the match sheet's boxes are ticked.
        match_unchecked: Vec::new(),
        sync_shift: settings::SyncShift {
            temperature: app.get_sync_shift_temperature(),
            tint: app.get_sync_shift_tint(),
            exposure: app.get_sync_shift_exposure(),
        },
        // Written as the Settings sheet's row is chosen, and as an
        // aspect is picked from the crop's list.
        crop_start: String::new(),
        crop_last: String::new(),
        // Written as the Fill's choice is made, in the panel or the
        // Settings sheet.
        fill_choice: String::new(),
        // Written as the Settings sheet's row is chosen; the system's
        // own scale from what the window saw, by the caller.
        ui_scale: 100,
        ui_scale_system: 0.0,
        // Written as a key is bound in the Settings sheet.
        keys: Default::default(),
        // Written as the Settings sheet's category is chosen.
        settings_category: String::new(),
        // Written as the Settings sheet's theme is chosen.
        theme: String::new(),
        // Written as the Settings sheet's scroll bar rows are chosen.
        scroll_bars: String::new(),
        scroll_bar_size: String::new(),
        // Written as the Settings sheet's language is chosen.
        language: String::new(),
        // The window's own, put on the file at the close (`Choices`).
        windows: Default::default(),
    }
}

/// Whether the run opens as the last closed the window and keeps it for
/// the next: not one the command line owns (a capture, a timing run,
/// keys sent), whose window must open the same on every machine, nor
/// one sized by `--window`, which wins over the kept size. A
/// headless run has no window, and the test harness none of winit's.
pub(crate) fn keeps_window(cli: &crate::Cli) -> bool {
    !scripted_start(cli) && cli.keys.is_none() && cli.window.is_none()
}

/// Whether the command line owns this run's start: a capture or a
/// timing run, which nobody is at. Such a run deletes nothing, opens in
/// no mode the last session left, and keeps none for the next.
pub(crate) fn scripted_start(cli: &crate::Cli) -> bool {
    cli.snapshot.is_some()
        || cli.screenshot.is_some()
        || cli.time_sharpen.is_some()
        || cli.time_clarity.is_some()
        || cli.time_dehaze.is_some()
        || cli.time_texture.is_some()
        || cli.time_cull.is_some()
        || cli.time_select.is_some()
        || cli.time_scroll.is_some()
}

/// The status line on a first run.
pub(crate) fn first_run_line() -> String {
    tr!(
        "Library and Loupe, top left (or G), switch between the grid \
     and one frame; Cull and Develop (C and D) are the Loupe's two modes; ? lists the keys"
    )
}

/// The absolute path of the file now open, kept for the next run to
/// jump back to; skipped for a screenshot, a snapshot or a one-shot
/// export, which should leave the user's settings alone.
pub(crate) fn remember_last_file(st: &State) {
    // `batch` as well: a snapshot's path is taken at the capture, and
    // a develop that lands after it (a `--menu` over another frame)
    // must not write the user's settings either.
    if st.window.batch || st.launch.screenshot.is_some() || st.launch.snapshot.is_some() {
        return;
    }
    let Some(path) = st.current.and_then(|i| st.path_at(i)) else {
        return;
    };
    let Ok(path) = std::fs::canonicalize(path) else {
        return;
    };
    // Every develop lands here; the file only writes when the file
    // open has changed.
    let path = path.to_string_lossy().into_owned();
    settings::Settings::change(|s| s.shared.last_file = path);
}

/// The panel's names for a profile that is the system's, and for one
/// that is a file.
pub(crate) const SYSTEM: &str = "System";

pub(crate) const FILE: &str = "File";

/// What a frame starts from: the develop that landed since the last,
/// if one did, whether the frame is drawn by the culling loupe, and
/// the kind the view finishes the picture on the GPU as.
pub(crate) struct Landing {
    pub(crate) pending: Option<crate::panel::deliver::Landed>,
    /// Culling, or a camera picture standing in for a develop:
    /// `cull_frame` draws the frame, not the develop view.
    pub(crate) loupe: bool,
    pub(crate) kind: finish::Source,
}

/// The start of every frame: a develop that has landed goes up.
///
/// The camera picture standing in for it comes down — for that
/// develop, or a later one of the same frame, and never the other way
/// about; a develop asked for before the frame on the panel was chosen
/// is a picture of another frame and leaves it standing. The picture
/// goes onto the GPU with its kind and, drawn as the develop view's
/// own frame, its size, its turn, the crop it opens under and its
/// white become the open frame's.
pub(crate) fn land_pending(st: &mut State, app: &App) -> Landing {
    let pending = st.develop.pending.take();
    if let Some(landed) = &pending {
        develop_landed(st, app, landed.generation);
        picture_up(&mut st.develop, landed);
        crate::panel::original::went_up(st, landed);
    }
    let loupe = st.culling.cull.is_some() || standing_in(st).is_some();
    if !loupe && let Some(landed) = &pending {
        let (w, h) = (landed.image.width(), landed.image.height());
        st.develop.source_size = (w, h);
        // A turn pressed since its delivery stands it on end again.
        settle_source_size(st);
        crate::panel::crop::start_landed(st, app);
        app.set_shot_size(size_text(w, h).into());
        st.develop.base_white = Some(landed.white);
        st.develop.white_cache = None;
    }
    Landing {
        pending,
        loupe,
        kind: drawn_kind(st),
    }
}

/// The edit the viewport draws the picture on the GPU under: the held
/// one while that picture is an earlier frame's, the frame's Original
/// while it shows, a history or snapshot row's while one is under the
/// pointer, else the panel's.
pub(crate) fn view_edit(st: &State, panel: &Edit) -> Edit {
    st.develop
        .held
        .clone()
        .or_else(|| crate::panel::original::shown(st).cloned())
        .or_else(|| st.develop.peek.clone())
        .unwrap_or_else(|| panel.clone())
}

/// A launch with no path: the frames to open as a folder, or else the
/// view of the library to open once its index is. The chosen start
/// folder with no frames of its own opens as a click on its row in the
/// folder tree does, when it is a root or under one: its folders as
/// tiles, or with `subfolders` the frames under it. One under no root
/// gives way to the last folder, `last`, as one gone does.
fn start_open<T>(
    roots: &greycard_library::Roots,
    chosen: &str,
    subfolders: bool,
    last: Option<&Path>,
    list: impl Fn(&Path) -> Option<Vec<T>>,
) -> (Vec<T>, Option<crate::roots::View>) {
    let folder = match settings::start_listing(chosen, last, &list) {
        settings::Start::Frames(files) => return (files, None),
        settings::Start::Nothing => return (Vec::new(), None),
        settings::Start::Bare(folder) => folder,
    };
    // Roots are kept canonical, as the index keeps its folders; the
    // chosen folder is taken as written, and is there.
    let folder =
        dunce::canonicalize(&folder).unwrap_or_else(|_| greycard_library::roots::tidy(&folder));
    if let Some(root) = roots.root_of(&folder) {
        tracing::info!("start folder {chosen} has no frames of its own; its view in the library");
        return (
            Vec::new(),
            Some(crate::tree::view_for(root, &folder, subfolders)),
        );
    }
    tracing::info!(
        "start folder {chosen} has no frames and is under no root; the last folder, if any"
    );
    match settings::start_listing("", last, &list) {
        settings::Start::Frames(files) => (files, None),
        settings::Start::Bare(_) | settings::Start::Nothing => (Vec::new(), None),
    }
}

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
    use crate::sheet::{ExportPreset, Sheet};
    use greycard_core::image::WorkingImage;
    use greycard_edit::{Preset, Sidecar};
    use greycard_engine::headless::batch_settings;

    fn remembered_with_presets(dir: &Path) -> settings::Settings {
        let png = crate::testing::bar_png(dir, 40);
        settings::Settings {
            shared: settings::Shared {
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
                ..Default::default()
            },
            ..settings::Settings::default()
        }
    }

    #[test]
    fn export_preset_fills_the_sheet_for_a_batch_run() {
        let dir = std::env::temp_dir().join(format!("greycard-cli-preset-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
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
        assert_eq!(sheet, remembered.shared.export_presets[0].sheet);
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
            &greycard_edit_io::Stores::user(),
            (w as u32, h as u32),
            &settings,
            &Default::default(),
            1.0,
            None,
            finish::Source::SCENE,
            None,
            None,
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
        assert_eq!(sheet, remembered.shared.export);
        assert_eq!(preset.as_deref(), Some("Print"));
        // A name the settings do not have is an error that lists them.
        let cli =
            Cli::try_parse_from(["greycard-ui", "frame.cr3", "--export-preset", "Nope"]).unwrap();
        let err = opening_sheet(&cli, &remembered).unwrap_err().to_string();
        assert!(
            err.contains("Nope") && err.contains("Web 2048, Print"),
            "{err}"
        );
        crate::testing::remove_dir_retry(&dir);
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
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let file = dir.join("IMG_0001.CR3");
        let mut edit = Edit::default();
        edit.light.exposure = 0.5;
        let preset = Preset::from_edit("Faded film", &edit, &[greycard_edit::Section::Light]);

        let mut sidecar = Sidecar::default();
        preset_at_start(&mut sidecar, None, &file, &preset, None);
        assert_eq!(sidecar.current.light.exposure, 0.5);
        assert!(Sidecar::find(&file).is_none(), "no sidecars: none written");

        let mut sidecar = Sidecar::default();
        let placement = Some(greycard_edit::Placement::Folder);
        preset_at_start(&mut sidecar, None, &file, &preset, placement);
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
        preset_at_start(&mut again, None, &file, &preset, placement);
        assert_eq!(again.history.len(), 1);
        assert_eq!(Sidecar::load(&file).unwrap().unwrap().saved, saved);

        // A file that will not say its ISO keeps waiting for its
        // blend rather than being given one made up.
        let mut fresh = Sidecar::default();
        preset_at_start(&mut fresh, None, &dir.join("IMG_0002.CR3"), &preset, None);
        assert_eq!(
            fresh.current.noise.learned_strength,
            Edit::default().noise.learned_strength
        );
        // On a version: its line takes the step, the file's own stays.
        let mut with = Sidecar::default();
        let id = with.new_version(None, Some("Warm")).unwrap();
        assert!(!preset_at_start(
            &mut with,
            Some("none"),
            &file,
            &preset,
            None
        ));
        preset_at_start(&mut with, Some(&id), &file, &preset, None);
        assert_eq!(with.current.light.exposure, 0.0);
        assert_eq!(with.line(Some(&id)).unwrap().current.light.exposure, 0.5);
        assert_eq!(
            with.line(Some(&id)).unwrap().current_label.as_deref(),
            Some("Preset: Faded film")
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// A start folder with no frames of its own, a root or under one,
    /// opens as its row in the folder tree does, the switch as it was
    /// left; under no root, the last folder opens; with frames, those.
    #[test]
    fn a_bare_start_folder_under_a_root_opens_its_view_in_the_library() {
        use crate::roots::View;
        let dir = crate::testing::scratch_dir("start-view");
        let root = dir.join("photos");
        let year = root.join("2026");
        let day = year.join("10-30");
        let loose = dir.join("loose");
        let last = dir.join("last");
        for d in [&day, &loose.join("shoot"), &last] {
            std::fs::create_dir_all(d).unwrap();
        }
        // Frames in the day and the last folder only.
        let list = |d: &Path| -> Option<Vec<PathBuf>> {
            Some(if d.ends_with("10-30") || d.ends_with("last") {
                vec![d.join("a.CR3")]
            } else {
                Vec::new()
            })
        };
        let roots = greycard_library::Roots::from_list(vec![root.clone()]);
        let text = |p: &Path| p.to_string_lossy().into_owned();
        let open = |chosen: &str, subfolders: bool| {
            start_open(&roots, chosen, subfolders, Some(&last), list)
        };
        // A folder under the root, the switch off, then on.
        assert_eq!(
            open(&text(&year), false),
            (
                Vec::new(),
                Some(View::Branch {
                    root: root.clone(),
                    folder: year.clone(),
                    deep: false,
                })
            )
        );
        assert_eq!(
            open(&text(&year), true),
            (
                Vec::new(),
                Some(View::Branch {
                    root: root.clone(),
                    folder: year.clone(),
                    deep: true,
                })
            )
        );
        // The root itself, written with a separator at its end.
        let written = format!("{}{}", text(&root), std::path::MAIN_SEPARATOR);
        assert_eq!(
            open(&written, false),
            (
                Vec::new(),
                Some(View::Branch {
                    root: root.clone(),
                    folder: root.clone(),
                    deep: false,
                })
            )
        );
        assert_eq!(
            open(&written, true),
            (Vec::new(), Some(View::Roots(Some(root.clone()))))
        );
        // Under no root: the last folder, or nothing without one.
        let in_last = vec![last.join("a.CR3")];
        assert_eq!(open(&text(&loose), false), (in_last.clone(), None));
        assert_eq!(
            start_open(&roots, &text(&loose), false, None, list),
            (Vec::new(), None)
        );
        // With frames of its own: listed from the disk, root or not.
        assert_eq!(open(&text(&day), true), (vec![day.join("a.CR3")], None));
        // Gone: the last folder.
        assert_eq!(open(&text(&dir.join("gone")), true), (in_last, None));
        crate::testing::remove_dir_retry(&dir);
    }
}
