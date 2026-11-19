//! The window's captures for a run that asks for one: `--snapshot`,
//! the window as drawn, and `--screenshot`, the viewport's texture,
//! once the picture is up, with what `--sheet`, `--tool`, `--part`,
//! `--menu`, `--press`, and `--keys` put on screen before them.

use std::path::PathBuf;
use std::rc::Rc;

use slint::{ComponentHandle, Model};

use crate::render::Renderer;
use crate::{App, STATE, Shared, gpu};

/// What `--sheet` or `--tool` puts on screen before a snapshot. No
/// flag and no key reaches a sheet or a Crop-tab tool, so a capture
/// of one asks for it here, once the picture is up; the split of
/// `app.slint` was checked with these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    /// The export sheet.
    Export,
    /// The preset sheet, as Save preset opens it.
    Preset,
    /// The sync sheet over the set `--also` made.
    Sync,
    /// The sync applied on the sheet's defaults in the same turn the
    /// sheet opens, so it is never drawn: the sidecars written, the
    /// status line saying so.
    Synced,
    /// The first stored preset clicked over the set `--also` made, no
    /// sheet involved: the current frame through the panel, the rest
    /// through `panel::sync::lay_over_targets`.
    PresetOntoSet,
    /// The first stored preset's bin clicked: its removal asked,
    /// waiting on the Remove that names it.
    PresetRemove,
    /// The current frame copied and the paste sheet opened over the
    /// set `--also` made.
    Paste,
    /// The same paste applied on the sheet's sections in the same
    /// turn, so the sheet is never drawn.
    Pasted,
    /// The frame menu opened over this row of the strip, or of the
    /// grid when it is open (`--menu`).
    Menu(usize),
    /// The import sheet, as Ctrl+Shift+I opens it.
    Import,
    /// The import sheet's import started as soon as its source is
    /// read, as its button would, for a snapshot of the run or of the
    /// folder it opens.
    Imported,
    /// The import sheet on Add in place, `--import`'s folder read as
    /// its folder.
    ImportInPlace,
    /// The model sheet, with sample text where the download's goes.
    Fetch,
    /// The preset import's report sheet, over sample presets: two in,
    /// one passing things over, and one refused. Nothing is imported.
    PresetReport,
    /// The lens profiles' download as the first launch offers it.
    Lenses,
    /// The Lightroom catalog import's sheet over `--lightroom`'s
    /// catalog, at its roots, its dry run's report, or written (the
    /// write run as the Write button runs it, for a capture of a
    /// scratch catalog's import).
    Lightroom(LightroomStep),
    /// RawTherapee's profiles beside the open folder's frames: the
    /// offer under the folder's name (answered or not), their import's
    /// sheet at its dry run's report, or written (the write run as the
    /// Write button runs it, for a capture of a scratch folder's).
    RawTherapee(RawTherapeeStep),
    /// The settings sheet.
    Settings,
    /// The camera match's sheet, as the Look section's button opens it.
    Match,
    /// The camera match's sheet with its Fit pressed once the scope is
    /// read, for a snapshot of the run's report.
    Matched,
    /// The camera match's sheet as the Look section's refit opens it,
    /// over the chosen look's group.
    Refit,
    /// The remove-a-look sheet, as the Look section's Remove opens it,
    /// over the first look in the directory when none is chosen. Never
    /// answered: a capture never touches the files.
    LookRemove,
    /// The rename-a-look sheet, as the Look section's Rename opens it,
    /// over the first look in the directory when none is chosen. A
    /// capture never renames, whatever `--keys` answers.
    LookRename,
    /// The shift-capture-time sheet over the selection, as the frame
    /// menu's Shift capture time... opens it.
    TimeShift,
    /// The root sheet over the library's first root, as its chip's
    /// Rename... opens it.
    RootName,
    /// The delete sheet over the selection, as the Delete key opens
    /// it. Never answered: a capture never deletes.
    Delete,
    /// The archive sheet over the open folder, as the grid header's
    /// Back up opens it, once its look is in. Never answered: a capture
    /// never copies.
    Archive,
    /// The Remove rejects sheet for the first archive, over the open
    /// folder's rejects folder, once its look is in. Never answered: a
    /// capture never moves or deletes.
    ArchiveRejects,
    /// A meta key (a rating, a flag or a color label) pressed over the
    /// selection (`--press`), for a snapshot of what follows it.
    Key(char),
    /// The crop tool, on the Crop tab.
    Crop,
    /// The level tool, on the Crop tab.
    Level,
    /// A vertical perspective guide in hand, on the Crop tab.
    Guide,
    /// The masks panel's People menu, open over the chosen mask.
    Parts,
    /// The masks panel's Landscape button pressed on the chosen mask:
    /// its menu of what the model found, once the answer is in.
    Landscape,
    /// A part chosen from the People menu by its label (`--part`).
    Part(&'static str),
    /// The grid's Sort menu, open (with `--grid`).
    Sort,
    /// The keys sheet, as ? opens it.
    Keys,
    /// The Help menu at the left pane's foot, open.
    Help,
    /// The Fit button's menu of zooms, open.
    Zoom,
    /// The white balance's presets, open off Auto's chevron.
    WhitePresets,
}

/// Which step of the RawTherapee profiles' offer and sheet a capture
/// shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawTherapeeStep {
    Offer,
    Report,
    Written,
}

/// Which step of the Lightroom import's sheet a capture shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LightroomStep {
    Roots,
    Report,
    Written,
}

impl Shown {
    /// `--sheet`'s names.
    pub(crate) fn sheet(name: &str) -> Result<Self, String> {
        match name {
            "export" => Ok(Self::Export),
            "preset" => Ok(Self::Preset),
            "fetch" => Ok(Self::Fetch),
            "lenses" => Ok(Self::Lenses),
            "settings" => Ok(Self::Settings),
            "sync" => Ok(Self::Sync),
            "synced" => Ok(Self::Synced),
            "preset-onto-set" => Ok(Self::PresetOntoSet),
            "preset-remove" => Ok(Self::PresetRemove),
            "preset-report" => Ok(Self::PresetReport),
            "paste" => Ok(Self::Paste),
            "pasted" => Ok(Self::Pasted),
            "import" => Ok(Self::Import),
            "imported" => Ok(Self::Imported),
            "import-in-place" => Ok(Self::ImportInPlace),
            "match" => Ok(Self::Match),
            "matched" => Ok(Self::Matched),
            "refit" => Ok(Self::Refit),
            "look-remove" => Ok(Self::LookRemove),
            "look-rename" => Ok(Self::LookRename),
            "time-shift" => Ok(Self::TimeShift),
            "root-name" => Ok(Self::RootName),
            "delete" => Ok(Self::Delete),
            "archive" => Ok(Self::Archive),
            "archive-rejects" => Ok(Self::ArchiveRejects),
            "parts" => Ok(Self::Parts),
            "landscape" => Ok(Self::Landscape),
            "lightroom" => Ok(Self::Lightroom(LightroomStep::Roots)),
            "lightroom-report" => Ok(Self::Lightroom(LightroomStep::Report)),
            "lightroom-written" => Ok(Self::Lightroom(LightroomStep::Written)),
            "rawtherapee-offer" => Ok(Self::RawTherapee(RawTherapeeStep::Offer)),
            "rawtherapee" => Ok(Self::RawTherapee(RawTherapeeStep::Report)),
            "rawtherapee-written" => Ok(Self::RawTherapee(RawTherapeeStep::Written)),
            "sort" => Ok(Self::Sort),
            "keys" => Ok(Self::Keys),
            "help" => Ok(Self::Help),
            "zoom" => Ok(Self::Zoom),
            "wb-presets" => Ok(Self::WhitePresets),
            _ => Err(format!(
                "want export, preset, fetch, lenses, settings, sync, synced, \
                 preset-onto-set, preset-remove, preset-report, paste, pasted, import, \
                 imported, import-in-place, match, matched, refit, look-remove, look-rename, \
                 time-shift, root-name, \
                 delete, archive, archive-rejects, parts, landscape, lightroom, \
                 lightroom-report, lightroom-written, rawtherapee-offer, rawtherapee, \
                 rawtherapee-written, sort, keys, help, zoom, or wb-presets, not {name}"
            )),
        }
    }

    /// `--part`'s labels: the People menu's.
    pub(crate) fn part(label: &str) -> Result<Self, String> {
        match crate::panel::parts::by_label(label) {
            Some(p) => Ok(Self::Part(p.label)),
            None => Err(format!(
                "want one of the People menu's labels ({}), not {label}",
                greycard_edit::mask::PARTS
                    .iter()
                    .map(|p| p.label)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// `--tool`'s names.
    pub(crate) fn tool(name: &str) -> Result<Self, String> {
        match name {
            "crop" => Ok(Self::Crop),
            "level" => Ok(Self::Level),
            "guide" => Ok(Self::Guide),
            _ => Err(format!("want crop, level or guide, not {name}")),
        }
    }

    /// Put it on screen: what the key or the button would have done.
    /// A tool brings the Crop tab with it.
    pub(crate) fn open(self, app: &App) {
        match self {
            Self::Export => app.set_export_open(true),
            Self::Preset => app.invoke_preset_save_open(),
            Self::Sync => app.invoke_sync_open_asked(),
            Self::Synced => {
                app.invoke_sync_open_asked();
                app.invoke_sync_applied();
            }
            Self::PresetOntoSet => app.invoke_preset_applied(0),
            // The row with the longest name, the hardest to fit;
            // nothing with no presets to ask about.
            Self::PresetRemove => {
                let longest = app
                    .get_preset_names()
                    .iter()
                    .enumerate()
                    .max_by_key(|(i, n)| (n.chars().count(), std::cmp::Reverse(*i)))
                    .map(|(i, _)| i as i32);
                if let Some(row) = longest {
                    app.set_preset_confirm_remove(row);
                }
            }
            Self::Paste => {
                app.invoke_copy_asked();
                app.invoke_paste_asked();
            }
            Self::Pasted => {
                app.invoke_copy_asked();
                app.invoke_paste_asked();
                app.invoke_sync_applied();
            }
            Self::Menu(row) => app.set_menu_at(row as i32),
            Self::Key(key) => {
                app.invoke_meta_key(key.to_string().into());
            }
            Self::Import => app.invoke_import_asked(),
            Self::Imported => {
                app.invoke_import_asked();
                crate::panel::import::start_when_read(app, 100);
            }
            Self::ImportInPlace => {
                app.invoke_import_asked();
                app.set_import_mode(crate::panel::import::ADD_IN_PLACE.into());
                if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                    let mut st = state.borrow_mut();
                    match st.import.source.clone() {
                        Some(dir) => crate::panel::import::read_place(&mut st, app, dir),
                        None => tracing::warn!("--sheet import-in-place wants --import DIR"),
                    }
                }
            }
            Self::Fetch => {
                app.set_fetch_title("Download the Subject model?".into());
                app.set_fetch_text(
                    "312 MB from the model registry.\nLicense: Apache-2.0.\nKept in the models folder."
                        .into(),
                );
                app.set_fetch_note("Sample text: this sheet was opened for a snapshot.".into());
                app.set_fetch_open(true);
            }
            Self::PresetReport => {
                use crate::panel::assets::{Brought, import_report, show_import_report};
                let sample = [
                    Brought::In {
                        name: "Soft Matte".into(),
                        carried: "4 sections".into(),
                        passed: vec![
                            "ColorNoiseReduction".into(),
                            "RedHue".into(),
                            "CameraProfile Camera Portrait".into(),
                            "masks".into(),
                        ],
                    },
                    Brought::In {
                        name: "Fine Grain".into(),
                        carried: "1 section".into(),
                        passed: Vec::new(),
                    },
                    Brought::Refused(
                        "warm-film.xmp was not imported: there is a preset called \"Warm Film\" \
                         already."
                            .into(),
                    ),
                ];
                show_import_report(app, import_report(&sample));
            }
            Self::Lenses => {
                if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                    crate::panel::assets::offer_lenses(&mut state.borrow_mut(), app, true);
                }
            }
            Self::Settings => app.invoke_settings_asked(),
            Self::Lightroom(step) => {
                if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                    let mut st = state.borrow_mut();
                    match st.lightroom.catalog.clone() {
                        Some(catalog) => {
                            st.lightroom.then = match step {
                                LightroomStep::Roots => crate::panel::lightroom::Then::Nothing,
                                LightroomStep::Report => crate::panel::lightroom::Then::Report,
                                LightroomStep::Written => crate::panel::lightroom::Then::Write,
                            };
                            crate::panel::lightroom::open(&mut st, app, catalog);
                        }
                        None => tracing::warn!("--sheet lightroom wants --lightroom CATALOG"),
                    }
                }
            }
            Self::RawTherapee(step) => {
                if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                    let mut st = state.borrow_mut();
                    match (step, st.views.recent.open.clone()) {
                        (_, None) => {
                            tracing::warn!("--sheet rawtherapee wants a folder open")
                        }
                        (RawTherapeeStep::Offer, Some(_)) => {
                            st.rawtherapee.force_offer = true;
                            crate::panel::rawtherapee::folder_opened(&mut st, app);
                        }
                        (RawTherapeeStep::Report | RawTherapeeStep::Written, Some(dir)) => {
                            if step == RawTherapeeStep::Written {
                                st.rawtherapee.then = crate::panel::rawtherapee::Then::Write;
                            }
                            crate::panel::rawtherapee::open(&mut st, app, dir);
                        }
                    }
                }
            }
            Self::Match => app.invoke_match_asked(),
            Self::Refit => app.invoke_look_refit_asked(),
            Self::LookRemove | Self::LookRename => {
                if app.get_look_name() == "none"
                    && let Some(state) = STATE.with(|s| s.borrow().clone())
                {
                    let mut st = state.borrow_mut();
                    st.develop.looks = greycard_edit_io::looks::list();
                    if let Some(first) = st.develop.looks.first().map(|e| e.name.clone()) {
                        app.set_look_name(first.into());
                    }
                }
                if self == Self::LookRemove {
                    app.invoke_look_remove_asked();
                } else {
                    app.invoke_look_rename_asked();
                }
            }
            Self::Matched => {
                app.invoke_match_asked();
                crate::panel::camera_match::fit_when_read(app, 100);
            }
            Self::TimeShift => app.invoke_time_shift_asked(),
            Self::Delete => app.invoke_delete_asked("selection".into()),
            Self::Archive => app.invoke_archive_header_pressed(0),
            Self::ArchiveRejects => app.invoke_archive_rejects_asked(0),
            Self::RootName => {
                if let Some(first) = app.get_library_roots().row_data(0) {
                    app.invoke_library_root_rename(first.path);
                }
            }
            Self::Crop => {
                app.set_panel_tab("Crop".into());
                app.set_crop_mode(true);
                app.invoke_crop_toggled();
            }
            Self::Level => {
                app.set_panel_tab("Crop".into());
                app.set_level_mode(true);
            }
            Self::Guide => {
                app.set_panel_tab("Crop".into());
                app.set_guide_mode("Vertical".into());
            }
            Self::Parts => {
                app.set_panel_tab("Masks".into());
                app.set_parts_menu_shown(true);
            }
            Self::Landscape => {
                app.set_panel_tab("Masks".into());
                app.invoke_find_landscape("Add".into());
            }
            Self::Part(label) => {
                app.set_panel_tab("Masks".into());
                app.invoke_add_part(label.into(), "Add".into());
            }
            Self::Sort => app.set_sort_menu_shown(true),
            Self::Keys => app.set_keys_open(true),
            Self::Help => app.set_help_menu_shown(true),
            Self::Zoom => app.set_zoom_menu_shown(true),
            Self::WhitePresets => {
                app.set_panel_tab("Develop".into());
                app.set_wb_presets_shown(true);
            }
        }
        app.window().request_redraw();
    }
}

/// `--keys`, kept here since the snapshot is scheduled while the
/// state is borrowed.
pub(crate) static SNAPSHOT_KEYS: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// `then`, once no learned mask and no part's people are being waited
/// on (or after `tries` looks a fifth of a second apart), and half a
/// second later, for the frame that shows what came.
fn when_quiet(state: Rc<Shared>, then: impl FnOnce() + 'static, tries: u32) {
    let quiet = state.try_borrow().is_ok_and(|st| {
        st.develop.asked.is_empty()
            && st.develop.part_pending.is_none()
            && st.develop.landscape_pending.is_none()
            && !st.lightroom.busy()
            && !st.rawtherapee.busy()
    });
    if quiet || tries == 0 {
        slint::Timer::single_shot(std::time::Duration::from_millis(500), then);
    } else {
        slint::Timer::single_shot(std::time::Duration::from_millis(200), move || {
            when_quiet(state, then, tries - 1)
        });
    }
}

/// `--snapshot`: the window as drawn, a moment on, from the event
/// loop rather than from inside a frame, once the picture asked for
/// is on screen (`ready`). A scroll asked for is applied now that the
/// panel's sections have their height (set before the first layout,
/// the ScrollPane puts it back to the top) and the capture waits a
/// further moment for that frame.
///
/// `then_quit` ends the run with the capture, which is what a batch
/// run wants. A capture of the camera's picture standing in for a
/// develop does not: that develop is on the worker's GPU queue this
/// moment, and a process that exits in the middle of one dies in the
/// driver rather than at its own hand.
pub(crate) fn schedule_snapshot(
    snapshot: &mut Option<PathBuf>,
    panel_scroll: &mut Option<f32>,
    shown: &mut Option<Shown>,
    app: &App,
    state: &Rc<Shared>,
    ready: bool,
    then_quit: bool,
) {
    // `--keys` with no snapshot: the keys are still sent once the
    // picture is up, and the session goes on as any other.
    static KEYS_SENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let path = match snapshot.take_if(|_| ready) {
        // The keys go with the snapshot, once: a later develop (a list
        // a click opened) must not send them again.
        Some(path) => {
            KEYS_SENT.store(true, std::sync::atomic::Ordering::Relaxed);
            Some(path)
        }
        None if ready
            && SNAPSHOT_KEYS.get().is_some()
            && !KEYS_SENT.swap(true, std::sync::atomic::Ordering::Relaxed) =>
        {
            None
        }
        None => return,
    };
    let app_weak = app.as_weak();
    let keys: Vec<String> = SNAPSHOT_KEYS
        .get()
        .map(|k| k.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    let state = state.clone();
    let waiting = state.clone();
    let scroll = panel_scroll.take();
    let shown = shown.take();
    let capture = move || {
        let Some(path) = path else {
            return;
        };
        if let Some(app) = app_weak.upgrade() {
            let wrote = match app.window().take_snapshot() {
                Ok(pixels) => {
                    let saved = image::RgbaImage::from_raw(
                        pixels.width(),
                        pixels.height(),
                        pixels.as_bytes().to_vec(),
                    )
                    .map(|img| img.save(&path));
                    match saved {
                        Some(Ok(())) => {
                            tracing::info!("wrote {}", path.display());
                            true
                        }
                        Some(Err(e)) => {
                            tracing::error!("snapshot: {e}");
                            false
                        }
                        None => {
                            tracing::error!("snapshot: bad size");
                            false
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("snapshot: {e}");
                    false
                }
            };
            // The picture asked for is not there: a failed run,
            // whatever stopped it.
            if !wrote {
                state.borrow_mut().launch.failed = true;
            }
            if then_quit {
                let _ = slint::quit_event_loop();
            }
        }
    };
    let moment = std::time::Duration::from_millis(300);
    // A sheet or a tool asked for is opened after the scroll and given
    // longer to settle: a sheet fades in.
    let opened = std::time::Duration::from_millis(1500);
    // `--keys`: each sent to the window as a press and a release, a
    // fifth of a second apart, once what was asked for is up.
    let then_keys = {
        let app_weak = app.as_weak();
        move || {
            let step = std::time::Duration::from_millis(200);
            let n = keys.len() as u32;
            for (i, key) in keys.into_iter().enumerate() {
                let app_weak = app_weak.clone();
                slint::Timer::single_shot(step * (i as u32 + 1), move || {
                    let Some(app) = app_weak.upgrade() else {
                        return;
                    };
                    use slint::platform::{Key, PointerEventButton, WindowEvent};
                    // `wait`: a step with nothing sent.
                    if key == "wait" {
                        return;
                    }
                    // A pointer token: move:X,Y, press:X,Y, release:X,Y
                    // or click:X,Y (a press and a release), logical
                    // pixels; a right click is rclick:X,Y.
                    if let Some((what, at)) = key.split_once(':')
                        && let Some((x, y)) = at.split_once(',')
                        && let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>())
                    {
                        let position = slint::LogicalPosition::new(x, y);
                        let button = if what == "rclick" {
                            PointerEventButton::Right
                        } else {
                            PointerEventButton::Left
                        };
                        let w = app.window();
                        match what {
                            "move" => w.dispatch_event(WindowEvent::PointerMoved { position }),
                            "press" => {
                                w.dispatch_event(WindowEvent::PointerMoved { position });
                                w.dispatch_event(WindowEvent::PointerPressed { position, button });
                            }
                            "release" => {
                                w.dispatch_event(WindowEvent::PointerReleased { position, button });
                            }
                            _ => {
                                w.dispatch_event(WindowEvent::PointerMoved { position });
                                w.dispatch_event(WindowEvent::PointerPressed { position, button });
                                w.dispatch_event(WindowEvent::PointerReleased { position, button });
                            }
                        }
                        return;
                    }
                    let text: slint::SharedString = match key.as_str() {
                        "down" => Key::DownArrow.into(),
                        "up" => Key::UpArrow.into(),
                        "left" => Key::LeftArrow.into(),
                        "right" => Key::RightArrow.into(),
                        "enter" => Key::Return.into(),
                        "esc" => Key::Escape.into(),
                        "tab" => Key::Tab.into(),
                        "home" => Key::Home.into(),
                        "end" => Key::End.into(),
                        "f6" => Key::F6.into(),
                        "f7" => Key::F7.into(),
                        "f8" => Key::F8.into(),
                        "f11" => Key::F11.into(),
                        other => other.into(),
                    };
                    tracing::info!("keys: {key}");
                    app.window()
                        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
                    app.window()
                        .dispatch_event(WindowEvent::KeyReleased { text });
                });
            }
            // A mask or a part asked for by what was opened is waited
            // for, so the capture shows it.
            slint::Timer::single_shot(step * (n + 1) + opened, move || {
                when_quiet(waiting, capture, 150)
            });
        }
    };
    let open_then = {
        let app_weak = app.as_weak();
        move || match (shown, app_weak.upgrade()) {
            (Some(what), Some(app)) => {
                what.open(&app);
                slint::Timer::single_shot(opened, then_keys);
            }
            _ => then_keys(),
        }
    };
    let app_weak = app.as_weak();
    slint::Timer::single_shot(moment, move || match scroll {
        Some(px) => {
            if let Some(app) = app_weak.upgrade() {
                app.set_panel_scroll(-px);
                app.window().request_redraw();
            }
            slint::Timer::single_shot(moment, open_then);
        }
        None => open_then(),
    });
}

/// `--screenshot`: the viewport's texture as a PNG, once the picture
/// asked for is on screen, then quit.
pub(crate) fn write_screenshot(
    screenshot: &mut Option<PathBuf>,
    failed: &mut bool,
    renderer: &Renderer,
    texture: &gpu::Texture,
    ready: bool,
) {
    let Some(path) = screenshot.take_if(|_| ready) else {
        return;
    };
    match renderer.read_back(texture) {
        Ok(png) => {
            if let Err(e) = png.save(&path) {
                tracing::error!("screenshot: {e}");
                *failed = true;
            } else {
                tracing::info!("wrote {}", path.display());
            }
        }
        Err(e) => {
            tracing::error!("screenshot: {e}");
            *failed = true;
        }
    }
    let _ = slint::quit_event_loop();
}
