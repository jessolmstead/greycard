use crate::panel::color::{preview_white, show_mixer_band, store_mixer_band, white_profile};
use crate::panel::crop::read_geometry;
use crate::panel::cull::{control_over_frame, leave_cull};
use crate::panel::curve::{
    current_channel, curve_points, draw_curve, parametric_mode, set_curve_points,
};
use crate::panel::edit::{read_edit, schedule_save};
use crate::panel::startup::{FILE, SYSTEM, monitor_note, monitor_settings};
use crate::*;

/// What a dropper's press chose, to move with the drag after it.
pub(crate) enum Picking {
    /// A point on a curve: which, its y when pressed, and the press's
    /// y on the view.
    Curve {
        channel: Channel,
        index: usize,
        y0: f32,
        at: f32,
    },
    /// A band of the mixer: its hue and saturation when pressed, and
    /// the press on the view.
    Mixer {
        band: usize,
        hue0: f32,
        sat0: f32,
        at: (f32, f32),
    },
}

/// How far a drag on the view goes, in logical pixels, for a curve
/// point's whole height, the mixer's whole saturation, and its hue
/// from one end to the other.
pub(crate) const PICK_DRAG: f32 = 200.0;

/// The shortest Oklab chroma deviation the defringe's dropper will
/// read a hue off. Below it the click is on a flat color, where the
/// direction is noise; the frames' own mean deviation is an order
/// above this, and a box average is shorter than a pixel's.
pub(crate) const DEFRINGE_PICK_FLOOR: f32 = 1.0e-4;

/// The hint the status line shows for a dropper in hand.
pub(crate) fn picking_hint(kind: &str) -> &'static str {
    match kind {
        "White" => "click something neutral in the picture; Esc or the button to leave it",
        "Curve" => {
            "click a tone in the picture for a point there; drag up or down to move it; Esc or the button when done"
        }
        "Range" => {
            "click a color in the picture to center the mask's hue on it; Esc or the button to leave it"
        }
        "Defringe" => {
            "the defringe is off while this is out, so the fringes show: zoom to 1:1 first, a fringe is a few pixels wide; click one to center the nearer hue window on it; Esc or the button when done"
        }
        _ => {
            "click a color in the picture for its band; drag up or down for saturation, sideways for hue; Esc or the button when done"
        }
    }
}

pub(crate) fn source_to_view(st: &State, app: &App, u: f32, v: f32) -> (f32, f32) {
    ViewMap::of(st, app).to_view(u, v)
}

/// The view's mapping of the picture's units, read from the panel
/// once and applied to as many points as a frame has.
pub(crate) struct ViewMap {
    scale: f32,
    view: (f32, f32),
    zoom: f32,
    source: (f32, f32),
    geometry: Geometry,
    origin: (f32, f32),
    center: (f32, f32),
}

impl ViewMap {
    pub(crate) fn of(st: &State, app: &App) -> Self {
        let (vw, vh) = (
            app.get_view_width().max(1) as u32,
            app.get_view_height().max(1) as u32,
        );
        let (sw, sh) = (st.source_size.0 as f32, st.source_size.1 as f32);
        let geometry = read_geometry(app);
        let frame = if app.get_crop_mode() {
            geometry.bounds(sw, sh)
        } else {
            geometry.frame(sw, sh)
        };
        Self {
            scale: app.window().scale_factor(),
            view: (vw as f32, vh as f32),
            zoom: effective_zoom(st, vw, vh),
            source: (sw, sh),
            geometry,
            origin: frame.origin,
            center: st.center,
        }
    }

    /// A point in the masks' units on the view, logical pixels.
    pub(crate) fn to_view(&self, u: f32, v: f32) -> (f32, f32) {
        let (sw, sh) = self.source;
        let r = self.geometry.to_plane((u * sw, v * sw), sw, sh);
        (
            ((r.0 - self.origin.0 - self.center.0) * self.zoom + self.view.0 / 2.0) / self.scale,
            ((r.1 - self.origin.1 - self.center.1) * self.zoom + self.view.1 / 2.0) / self.scale,
        )
    }
}

/// A point of the view (logical pixels) in the masks' units: the
/// source position under it, over the source's width.
pub(crate) fn view_to_source(st: &State, app: &App, x: f32, y: f32) -> (f32, f32) {
    let scale = app.window().scale_factor();
    let (vw, vh) = (
        app.get_view_width().max(1) as u32,
        app.get_view_height().max(1) as u32,
    );
    let zoom = effective_zoom(st, vw, vh);
    let (sw, sh) = (st.source_size.0 as f32, st.source_size.1 as f32);
    let geometry = read_geometry(app);
    let frame = if app.get_crop_mode() {
        geometry.bounds(sw, sh)
    } else {
        geometry.frame(sw, sh)
    };
    let r = (
        (x * scale - vw as f32 / 2.0) / zoom + st.center.0 + frame.origin.0,
        (y * scale - vh as f32 / 2.0) / zoom + st.center.1 + frame.origin.1,
    );
    let at = geometry.to_source(r, sw, sh);
    (at.0 / sw, at.1 / sw)
}

/// The developed picture under a point of the view (logical pixels):
/// the mean of a few pixels about it, in the working space at the
/// white it was developed at; `None` off the picture or without one.
pub(crate) fn sample_view(st: &State, app: &App, x: f32, y: f32) -> Option<[f32; 3]> {
    sample_view_at(st, app, x, y, 2)
}

/// The same, over a box of `reach` pixels each way instead of the
/// droppers' usual two: what the defringe's dropper takes its local
/// mean over.
pub(crate) fn sample_view_at(
    st: &State,
    app: &App,
    x: f32,
    y: f32,
    reach: u32,
) -> Option<[f32; 3]> {
    let (u, v) = view_to_source(st, app, x, y);
    let sw = st.source_size.0 as f32;
    let (px, py) = ((u * sw).round() as i64, (v * sw).round() as i64);
    match st.renderer.as_ref()?.sample(px, py, reach) {
        Ok(sample) => sample,
        Err(e) => {
            tracing::warn!("pick: {e:#}");
            None
        }
    }
}

/// A sampled pixel taken to the panel's white and through the
/// panel's look to the stages the droppers read.
pub(crate) fn pick_stages(st: &mut State, app: &App, px: [f32; 3]) -> finish::Picked {
    let key = (app.get_as_shot(), app.get_temperature(), app.get_tint());
    let m = preview_white(st, key);
    let px: [f32; 3] = std::array::from_fn(|r| m[r][0] * px[0] + m[r][1] * px[1] + m[r][2] * px[2]);
    let edit = read_edit(app, &st.edit, st.target);
    finish::pick(
        px,
        &edit.light.effective(),
        &edit.acting_mixer(),
        &edit.color,
        &edit.bw,
        &edit.tint,
        &edit.curves.bake_with(&edit.grading),
        st.source,
    )
}

/// The zoom in words for the panel: fitted, or a percentage of 1:1.
pub(crate) fn zoom_label(zoom: f32) -> slint::SharedString {
    if zoom > 0.0 {
        format!("{}%", (zoom * 100.0).round() as i32).into()
    } else {
        "Fit".into()
    }
}

/// [`zoom::effective`] for the state: the zoom it holds, the cell
/// the compare view gives, and the size of the picture on the GPU.
pub(crate) fn effective_zoom(st: &State, vw: u32, vh: u32) -> f32 {
    zoom::effective(st.zoom, view_cell(st, vw, vh), st.image_size)
}

/// [`zoom::cell`] for the state: how many frames the compare view
/// is showing, if it is showing any.
pub(crate) fn view_cell(st: &State, vw: u32, vh: u32) -> (u32, u32) {
    let compare = st
        .cull
        .as_ref()
        .or(st.hold.as_ref())
        .map_or(1, |c| c.compare);
    zoom::cell(vw, vh, compare)
}

/// The monitor's table on the renderer, for the panel's output
/// space, its proof and its monitor, rebuilt when any of them moved.
pub(crate) fn sync_display(
    lut_for: &mut Option<(
        export::Space,
        Option<display::Proof>,
        display::MonitorProfile,
    )>,
    encoded_lut_for: &mut Option<display::MonitorProfile>,
    monitors: &[display::Monitor],
    app: &App,
    renderer: &mut Renderer,
) {
    // The output space is the export sheet's, and the table takes it
    // to the monitor, through the proof when one is on. The
    // monitor's profile is the panel's.
    let space =
        export::Space::from_name(app.get_export_space().as_str()).unwrap_or(export::Space::Srgb);
    renderer.set_output(space.matrix());
    let proof = app.get_proofing().then(|| proof_settings(app)).flatten();
    let (monitor, from) = monitor_settings(app, monitors);
    let key = (space, proof.clone(), monitor.clone());
    if lut_for.as_ref() != Some(&key) {
        if lut_for.as_ref().map(|k| &k.2) != Some(&monitor) {
            app.set_display_note(monitor_note(&monitor, &from).into());
        }
        let srgb = display::MonitorProfile::Srgb;
        let lut = display::Lut3d::build(space, &monitor, proof.as_ref())
            .or_else(|e| {
                tracing::warn!("proof: {e:#}");
                display::Lut3d::build(space, &monitor, None)
            })
            .or_else(|e| {
                tracing::warn!("monitor profile: {e:#}");
                display::Lut3d::build(space, &srgb, None)
            })
            .unwrap_or_else(|e| {
                tracing::warn!("display table: {e:#}");
                display::Lut3d::identity()
            });
        renderer.set_display_lut(&lut);
        *lut_for = Some(key);
    }
    // The encoded path's table: the camera's JPEG is sRGB whatever
    // the export sheet's space, and a proof is a look at one export,
    // so neither reaches it; the monitor's profile alone.
    if encoded_lut_for.as_ref() != Some(&monitor) {
        let lut = display::Lut3d::build(export::Space::Srgb, &monitor, None).unwrap_or_else(|e| {
            tracing::warn!("monitor profile for the camera's JPEG: {e:#}");
            display::Lut3d::identity()
        });
        renderer.set_encoded_lut(&lut);
        *encoded_lut_for = Some(monitor);
    }
}

/// `--snapshot`: the window as drawn, a moment on, from the event
/// loop rather than from inside a frame, once the picture asked for
/// is on screen (`ready`). A scroll asked for is applied now that the
/// panel's sections have their height (set before the first layout,
/// the ScrollView puts it back to the top) and the capture waits a
/// further moment for that frame.
///
/// `then_quit` ends the run with the capture, which is what a batch
/// run wants. A capture of the camera's picture standing in for a
/// develop does not: that develop is on the worker's GPU queue this
/// moment, and a process that exits in the middle of one dies in the
/// driver rather than at its own hand.
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
    /// The current frame copied and the paste sheet opened over the
    /// set `--also` made.
    Paste,
    /// The same paste applied on the sheet's sections in the same
    /// turn, so the sheet is never drawn.
    Pasted,
    /// The frame menu opened over this row of the strip, or of the
    /// grid when it is open (`--menu`).
    Menu(usize),
    /// The model sheet, with sample text where the download's goes.
    Fetch,
    /// The lens profiles' download as the first launch offers it.
    Lenses,
    /// The settings sheet.
    Settings,
    /// The crop tool, on the Crop tab.
    Crop,
    /// The level tool, on the Crop tab.
    Level,
    /// A vertical perspective guide in hand, on the Crop tab.
    Guide,
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
            "paste" => Ok(Self::Paste),
            "pasted" => Ok(Self::Pasted),
            _ => Err(format!(
                "want export, preset, fetch, lenses, settings, sync, synced, \
                 preset-onto-set, paste or pasted, not {name}"
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
            Self::Fetch => {
                app.set_fetch_title("Download the Subject model?".into());
                app.set_fetch_text(
                    "312 MB from the model registry.\nLicense: Apache-2.0.\nKept in the models folder."
                        .into(),
                );
                app.set_fetch_note("Sample text: this sheet was opened for a snapshot.".into());
                app.set_fetch_open(true);
            }
            Self::Lenses => {
                if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                    crate::panel::assets::offer_lenses(&mut state.borrow_mut(), app, true);
                }
            }
            Self::Settings => app.invoke_settings_asked(),
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
        }
        app.window().request_redraw();
    }
}

pub(crate) fn schedule_snapshot(
    snapshot: &mut Option<PathBuf>,
    panel_scroll: &mut Option<f32>,
    shown: &mut Option<Shown>,
    app: &App,
    state: &Rc<RefCell<State>>,
    ready: bool,
    then_quit: bool,
) {
    let Some(path) = snapshot.take_if(|_| ready) else {
        return;
    };
    let app_weak = app.as_weak();
    let state = state.clone();
    let scroll = panel_scroll.take();
    let shown = shown.take();
    let capture = move || {
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
                state.borrow_mut().failed = true;
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
    let open_then = {
        let app_weak = app.as_weak();
        move || match (shown, app_weak.upgrade()) {
            (Some(what), Some(app)) => {
                what.open(&app);
                slint::Timer::single_shot(opened, capture);
            }
            _ => capture(),
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

/// An ICC profile from the desktop's chooser, opening beside the
/// current one or where profiles are kept; `apply` takes the choice
/// on the event loop.
pub(crate) fn choose_icc(
    app: &App,
    title: &'static str,
    current: PathBuf,
    apply: impl Fn(&App, PathBuf) + Send + 'static,
) {
    let start = current
        .parent()
        .filter(|p| p.is_dir())
        .map(Path::to_path_buf)
        .or_else(|| {
            // Where each platform keeps its profiles, the user's own
            // first.
            let home = dirs::home_dir()?;
            let folders: &[PathBuf] = if cfg!(target_os = "macos") {
                &[
                    home.join("Library/ColorSync/Profiles"),
                    PathBuf::from("/Library/ColorSync/Profiles"),
                ]
            } else if cfg!(windows) {
                &[PathBuf::from(r"C:\Windows\System32\spool\drivers\color")]
            } else {
                &[
                    home.join(".local/share/icc"),
                    PathBuf::from("/usr/share/color/icc"),
                ]
            };
            folders.iter().find(|p| p.is_dir()).cloned().or(Some(home))
        })
        .unwrap_or_else(|| PathBuf::from("/"));
    let app_weak = app.as_weak();
    export::choose_open(
        title,
        start,
        (
            "ICC profiles".to_string(),
            vec!["*.icc".into(), "*.icm".into()],
        ),
        move |chosen| {
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                match chosen {
                    Ok(Some(path)) => {
                        apply(&app, path);
                        app.window().request_redraw();
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!("{title}: file chooser: {e:#}"),
                }
            });
        },
    );
}

/// The monitor's profile as the settings keep it: "System", a
/// standard's name, or the file's path; a file not yet chosen is
/// System again.
pub(crate) fn display_key(app: &App) -> String {
    let choice = app.get_display_profile();
    if choice.as_str() == FILE {
        let file = app.get_display_file();
        if file.is_empty() {
            SYSTEM.to_string()
        } else {
            file.to_string()
        }
    } else {
        choice.to_string()
    }
}

/// The soft proof as the panel has it, or none while its profile is
/// a file not yet chosen.
pub(crate) fn proof_settings(app: &App) -> Option<display::Proof> {
    let choice = app.get_proof_profile();
    let profile = if choice.as_str() == "File" {
        let file = app.get_proof_file();
        if file.is_empty() {
            return None;
        }
        display::ProofProfile::File(PathBuf::from(file.as_str()))
    } else {
        display::ProofProfile::Space(export::Space::from_name(choice.as_str())?)
    };
    Some(display::Proof {
        profile,
        intent: display::ProofIntent::from_name(app.get_proof_intent().as_str())
            .unwrap_or_default(),
        warn: app.get_gamut_warning(),
    })
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    // A change the shader handles: redraw, and remember it.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_view_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // A look control reached in culling: out of it, and the
            // frame develops so the control has a picture to act on.
            if st.cull.is_some() {
                let with = control_over_frame(&mut st, &app);
                leave_cull(&mut st, &app, &worker, with);
                return;
            }
            app.window().request_redraw();
            schedule_save(&mut st, app.as_weak());
        });
    }
    // The scope the panel shows. The bins are taken again for it, so
    // the picture arrives with the next read back.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_scope_changed(move |name| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if let Some(chosen) = scope::Scope::from_name(name.as_str()) {
                state.borrow_mut().scope = chosen;
                app.window().request_redraw();
            }
        });
    }
    // The clipping warnings and the proof are the panel's; a change
    // is a frame.
    {
        let app_weak = app.as_weak();
        app.on_warn_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                app.window().request_redraw();
            }
        });
        let app_weak = app.as_weak();
        app.on_proof_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                app.window().request_redraw();
            }
        });
        // A profile file for the proof, from the desktop's chooser.
        let app_weak = app.as_weak();
        app.on_choose_proof_file(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let current = PathBuf::from(app.get_proof_file().as_str());
            choose_icc(&app, "Proof profile", current, |app, path| {
                app.set_proof_file(path.to_string_lossy().as_ref().into());
                app.set_proof_profile(FILE.into());
                app.set_proofing(true);
            });
        });
        // The monitor's profile: a change is a table and a frame.
        let app_weak = app.as_weak();
        app.on_display_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                app.window().request_redraw();
            }
        });
        let app_weak = app.as_weak();
        app.on_choose_display_file(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let current = PathBuf::from(app.get_display_file().as_str());
            choose_icc(&app, "Monitor profile", current, |app, path| {
                app.set_display_file(path.to_string_lossy().as_ref().into());
                app.set_display_profile(FILE.into());
            });
        });
        // The canvas's color: a frame, the Rectangle behind the
        // viewport following it at once through `canvas-colors`.
        let app_weak = app.as_weak();
        app.on_canvas_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                app.window().request_redraw();
            }
        });
    }
    // The droppers: a button puts one in hand, or takes it back; a
    // press on the view reads the developed picture there, a drag
    // moves what the press chose, a release records it.
    {
        let app_weak = app.as_weak();
        app.on_pick_started(move |kind| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let same = app.get_picking() == kind;
            app.invoke_stop_placing();
            if !same {
                app.set_picking(kind.clone());
                app.set_status(picking_hint(kind.as_str()).into());
                // The defringe's dropper wants the pass's input on
                // screen, not its output: see `edit_to_develop`.
                if kind == "Defringe" {
                    app.invoke_develop_changed();
                }
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_pick_pressed(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(px) = sample_view(&st, &app, x, y) else {
                return;
            };
            match app.get_picking().as_str() {
                "White" => {
                    // Through the profile the develop used, not the
                    // file's own: a neutral read through other
                    // matrices comes back as another temperature, and
                    // the patch the user clicked would not go neutral.
                    let Some(profile) = white_profile(&mut st) else {
                        return;
                    };
                    let (Some(base), Some((frame, _))) = (st.base_white, st.frame.as_ref()) else {
                        return;
                    };
                    let Some(gains) =
                        greycard_core::color::neutral_gains(px, base.gains, base.matrix)
                    else {
                        app.set_status("nothing to read there".into());
                        return;
                    };
                    match resolve_white_balance(frame, &profile, WhitePoint::Coefficients(gains)) {
                        Ok(wb) => {
                            let tt = wb.temp_tint;
                            app.set_as_shot(false);
                            app.set_temperature((tt.cct as f32).clamp(2000.0, 12000.0));
                            app.set_tint((tt.duv as f32).clamp(-0.05, 0.05));
                            drop(st);
                            app.invoke_stop_placing();
                            app.invoke_develop_changed();
                        }
                        Err(e) => {
                            tracing::warn!("white balance from the picked point: {e}");
                            app.set_status(format!("white balance: {e}").into());
                        }
                    }
                }
                "Curve" => {
                    // A point-curve tool: not while the parametric
                    // curve is showing, where there is no point to see.
                    if parametric_mode(&app) {
                        return;
                    }
                    // The pixel at the panel's white, through the
                    // panel's look to where the curve reads it.
                    let picked = pick_stages(&mut st, &app, px);
                    let channel = current_channel(&app);
                    let value = match channel {
                        Channel::Rgb => picked.luma,
                        Channel::Red => picked.encoded[0],
                        Channel::Green => picked.encoded[1],
                        Channel::Blue => picked.encoded[2],
                        Channel::RedGreen | Channel::BlueYellow => picked.lightness,
                    }
                    .clamp(0.0, 1.0);
                    let mut points = curve_points(&app, channel);
                    // A point already close by is the one; else one
                    // on the curve as it is, so the picture holds
                    // still until the drag.
                    let index = match points
                        .iter()
                        .position(|p| (p[0] - value).abs() < curve::MIN_GAP * 2.0)
                    {
                        Some(i) => i,
                        None => {
                            let at = points.partition_point(|p| p[0] < value);
                            points.insert(at, [value, curve::evaluate(&points, value)]);
                            at
                        }
                    };
                    st.picking = Some(Picking::Curve {
                        channel,
                        index,
                        y0: points[index][1],
                        at: y,
                    });
                    st.curve_drag = Some(index);
                    set_curve_points(&app, channel, &points);
                    app.set_curve_image(draw_curve(&app, st.bins.as_deref()));
                    app.window().request_redraw();
                }
                "Defringe" => {
                    // The pass gates on the direction a pixel's chroma
                    // departs from its neighborhood's; the dropper
                    // reads that same quantity here, the point against
                    // a wider box around it, and hands the hue to
                    // whichever window is nearer. The two boxes are
                    // flat where the pass takes a Gaussian, and the
                    // outer one is a couple of pixels wider than the
                    // pass's averaging window; neither changes which
                    // way the deviation points, which is all that is
                    // read off it. No white or look between: the
                    // defringe runs on the base, and the base is what
                    // the sample is. The base is developed with the
                    // pass off while this dropper is out, so what is
                    // sampled is the pass's input; see
                    // `edit_to_develop`.
                    let radius = app
                        .get_lens_defringe_radius()
                        .clamp(defringe::MIN_RADIUS, defringe::MAX_RADIUS);
                    let reach = (2.0 * radius).ceil() as u32 + 1;
                    let Some(mean) = sample_view_at(&st, &app, x, y, reach) else {
                        return;
                    };
                    let Some(hue) = defringe::deviation_hue(px, mean, DEFRINGE_PICK_FLOOR) else {
                        app.set_status(
                            "no fringe there: that color does not depart from its neighbors'"
                                .into(),
                        );
                        return;
                    };
                    if defringe::nearer_window(
                        hue,
                        app.get_lens_defringe_purple_center(),
                        app.get_lens_defringe_green_center(),
                    ) {
                        app.set_lens_defringe_purple_center(hue);
                    } else {
                        app.set_lens_defringe_green_center(hue);
                    }
                    // No word about it: the Center slider has moved,
                    // and it carries the number.
                    drop(st);
                    app.invoke_develop_changed();
                }
                "Range" => {
                    // The color range's hue from the picture before any
                    // look, as the mask reads it (`finish::sample`), at
                    // the panel's white; the dropper's box is the
                    // mean the mask reads a hue from, near enough.
                    let key = (app.get_as_shot(), app.get_temperature(), app.get_tint());
                    let m = preview_white(&mut st, key);
                    let px: [f32; 3] =
                        std::array::from_fn(|r| m[r][0] * px[0] + m[r][1] * px[1] + m[r][2] * px[2]);
                    let edit = read_edit(&app, &st.edit, st.target);
                    let s = finish::sample(px, None, edit.light.effective().exposure);
                    app.set_range_hue(s.hue());
                    // Under the floor the hue is the pixel's noise, and
                    // the window would not take it in anyway: say so.
                    let floor = app.get_range_chroma();
                    app.set_status(if s.chroma() < floor {
                        format!(
                            "that color is nearly grey (chroma {:.1}, under the floor's {:.1}): lower Chroma to take it in",
                            s.chroma() * 100.0,
                            floor * 100.0
                        )
                        .into()
                    } else {
                        "".into()
                    });
                    drop(st);
                    let status = app.get_status();
                    app.invoke_stop_placing();
                    app.set_status(status);
                    app.invoke_view_changed();
                }
                "Mixer" => {
                    let picked = pick_stages(&mut st, &app, px);
                    let (band, _) = finish::nearest_band(picked.hue);
                    app.set_mixer_band(band as i32);
                    show_mixer_band(&app);
                    st.picking = Some(Picking::Mixer {
                        band,
                        hue0: app.get_mixer_hue(),
                        sat0: app.get_mixer_saturation(),
                        at: (x, y),
                    });
                    app.window().request_redraw();
                }
                _ => {}
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_pick_dragged(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            match st.picking {
                Some(Picking::Curve {
                    channel,
                    index,
                    y0,
                    at,
                }) => {
                    let mut points = curve_points(&app, channel);
                    if index >= points.len() {
                        return;
                    }
                    points[index][1] = (y0 + (at - y) / PICK_DRAG).clamp(0.0, 1.0);
                    set_curve_points(&app, channel, &points);
                    app.set_curve_image(draw_curve(&app, st.bins.as_deref()));
                }
                Some(Picking::Mixer {
                    band,
                    hue0,
                    sat0,
                    at,
                }) => {
                    app.set_mixer_band(band as i32);
                    app.set_mixer_hue((hue0 + (x - at.0) / PICK_DRAG * 60.0).clamp(-30.0, 30.0));
                    app.set_mixer_saturation(
                        (sat0 + (at.1 - y) / PICK_DRAG * 2.0).clamp(-1.0, 1.0),
                    );
                    store_mixer_band(&app);
                }
                None => return,
            }
            app.window().request_redraw();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_pick_released(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if st.picking.take().is_some() {
                st.curve_drag = None;
                drop(st);
                app.invoke_view_changed();
            }
        });
    }
    // Zoom about the cursor.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_zoom(move |factor, x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let scale = app.window().scale_factor();
            let (vw, vh) = (
                app.get_view_width().max(1) as u32,
                app.get_view_height().max(1) as u32,
            );
            let old = effective_zoom(&st, vw, vh);
            let new = (old * factor).clamp(0.02, 16.0);
            // The image pixel under the cursor stays under the cursor.
            let (px, py) = (x * scale - vw as f32 / 2.0, y * scale - vh as f32 / 2.0);
            st.center.0 += px / old - px / new;
            st.center.1 += py / old - py / new;
            st.zoom = new;
            app.window().request_redraw();
        });
    }
    // Zoom to a level with the image pixel at a point held; a click
    // goes to 1:1 there or back to fit; space and z do the same about
    // the view's center, at 1:1 and 3:1.
    {
        let outer = state.clone();
        let (state, app_weak) = (outer.clone(), app.as_weak());
        let zoom_to = move |level: f32, x: f32, y: f32| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if level <= 0.0 {
                st.zoom = 0.0;
                let (iw, ih) = st.image_size;
                st.center = (iw as f32 / 2.0, ih as f32 / 2.0);
            } else {
                let scale = app.window().scale_factor();
                let (vw, vh) = (
                    app.get_view_width().max(1) as u32,
                    app.get_view_height().max(1) as u32,
                );
                let old = effective_zoom(&st, vw, vh);
                let (px, py) = (x * scale - vw as f32 / 2.0, y * scale - vh as f32 / 2.0);
                st.center.0 += px / old - px / level;
                st.center.1 += py / old - py / level;
                st.zoom = level;
            }
            app.window().request_redraw();
        };
        app.on_zoom_to(zoom_to.clone());
        let state = outer.clone();
        let zoom_to2 = zoom_to.clone();
        let app_weak = app.as_weak();
        app.on_toggle_zoom(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // The touch area covers the letterbox too: a click beside
            // the picture is nobody's.
            let st = state.borrow();
            let scale = app.window().scale_factor();
            let (vw, vh) = (
                app.get_view_width().max(1) as u32,
                app.get_view_height().max(1) as u32,
            );
            // In the compare view a click chooses the frame under it.
            if let Some(cull) = st.cull.as_ref()
                && cull.compare > 1
            {
                // As many tiles as the frame drew: a set of four at
                // the end of a short folder is fewer.
                let rows = cull::compare_rows(cull.anchor, cull.compare, st.shown.len());
                let tile = cull::tile_at(vw, vh, rows.len(), x * scale, y * scale);
                let row = tile.and_then(|k| rows.get(k).copied());
                drop(st);
                if let Some(row) = row
                    && row as i32 != app.get_selected()
                {
                    app.invoke_select(row as i32);
                }
                return;
            }
            let (iw, ih) = st.image_size;
            if iw == 0 || ih == 0 {
                return;
            }
            let zoom = effective_zoom(&st, vw, vh);
            let u = (x * scale - vw as f32 / 2.0) / zoom + st.center.0;
            let v = (y * scale - vh as f32 / 2.0) / zoom + st.center.1;
            if u < 0.0 || v < 0.0 || u > iw as f32 || v > ih as f32 {
                return;
            }
            let fitted = st.zoom == 0.0;
            drop(st);
            zoom_to2(if fitted { 1.0 } else { 0.0 }, x, y);
        });
        let (state, app_weak) = (outer.clone(), app.as_weak());
        app.on_zoom_key(move |level| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let at_level = (state.borrow().zoom - level).abs() < 1e-3;
            let scale = app.window().scale_factor();
            let (cx, cy) = (
                app.get_view_width().max(1) as f32 / 2.0 / scale,
                app.get_view_height().max(1) as f32 / 2.0 / scale,
            );
            zoom_to(if at_level { 0.0 } else { level }, cx, cy);
        });
    }
    // The navigator: a press or drag puts that point of the frame at
    // the view's center, kept within the frame while it is larger
    // than the view.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_navigate(move |fx, fy| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let (vw, vh) = (
                app.get_view_width().max(1) as u32,
                app.get_view_height().max(1) as u32,
            );
            let zoom = effective_zoom(&st, vw, vh);
            let (fw, fh) = (st.image_size.0 as f32, st.image_size.1 as f32);
            let (shown_w, shown_h) = (vw as f32 / zoom, vh as f32 / zoom);
            let place = |f: f32, frame: f32, shown: f32| {
                if shown >= frame {
                    frame / 2.0
                } else {
                    (f * frame).clamp(shown / 2.0, frame - shown / 2.0)
                }
            };
            st.center = (place(fx, fw, shown_w), place(fy, fh, shown_h));
            app.window().request_redraw();
        });
    }
    // Drag to pan.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_pan(move |dx, dy| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let scale = app.window().scale_factor();
            let (vw, vh) = (
                app.get_view_width().max(1) as u32,
                app.get_view_height().max(1) as u32,
            );
            let zoom = effective_zoom(&st, vw, vh);
            st.center.0 -= dx * scale / zoom;
            st.center.1 -= dy * scale / zoom;
            app.window().request_redraw();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_reset_view(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.zoom = 0.0;
            let (iw, ih) = st.image_size;
            st.center = (iw as f32 / 2.0, ih as f32 / 2.0);
            app.window().request_redraw();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    use crate::testing::{click, window};

    /// A pick with a single-shot dropper (White, Range, Defringe) does
    /// not zoom: the press puts the dropper down and, for these three,
    /// lets go of `picking` right there — which used to leave the
    /// release reading as a plain click, since it read `picking` fresh
    /// rather than remembering the press. Stands in for
    /// `viewport::install`'s own wiring of `pick_pressed`'s "Range"
    /// arm (`app.invoke_stop_placing()` before the release), which
    /// needs a developed picture on a real device to drive for real;
    /// this drives the same pointer-event routing the `.slint` side
    /// does, through the compiled window, with no GPU behind it.
    #[test]
    fn a_pick_with_a_single_shot_dropper_does_not_zoom() {
        let app = window(1);
        app.set_picking("Range".into());
        let zoomed = Rc::new(Cell::new(false));
        {
            let zoomed = zoomed.clone();
            app.on_toggle_zoom(move |_, _| zoomed.set(true));
        }
        {
            let app_weak = app.as_weak();
            app.on_pick_pressed(move |_, _| {
                let app = app_weak.upgrade().unwrap();
                // As the "Range" dropper's own press does: read the
                // color under the pointer (faked, here, as moving the
                // hue) and let go of `picking` on the press itself.
                app.set_range_hue(200.0);
                app.set_picking("".into());
            });
        }
        click(&app, 600.0, 400.0);
        assert!(!zoomed.get(), "a pick must not zoom the view");
        assert_eq!(app.get_range_hue(), 200.0, "the hue moved");
    }

    #[test]
    fn a_sheet_or_a_tool_asked_for_is_on_screen() {
        let app = crate::testing::window(1);
        let (_state, _worker) = crate::testing::retouch_state(&app);
        assert_eq!(Shown::sheet("export"), Ok(Shown::Export));
        assert_eq!(Shown::tool("guide"), Ok(Shown::Guide));
        assert_eq!(Shown::sheet("sync"), Ok(Shown::Sync));
        assert_eq!(Shown::sheet("synced"), Ok(Shown::Synced));
        assert_eq!(Shown::sheet("preset-onto-set"), Ok(Shown::PresetOntoSet));
        assert_eq!(Shown::sheet("paste"), Ok(Shown::Paste));
        assert_eq!(Shown::sheet("pasted"), Ok(Shown::Pasted));
        assert!(Shown::sheet("guide").is_err());
        assert!(Shown::tool("export").is_err());

        Shown::Export.open(&app);
        assert!(app.get_export_open());
        Shown::Fetch.open(&app);
        assert!(app.get_fetch_open());
        assert!(!app.get_fetch_title().is_empty());
        app.set_fetch_open(false);
        assert_eq!(Shown::sheet("lenses"), Ok(Shown::Lenses));
        assert_eq!(Shown::sheet("settings"), Ok(Shown::Settings));
        Shown::Lenses.open(&app);
        assert!(app.get_fetch_open());
        assert!(
            app.get_fetch_text()
                .starts_with(crate::panel::assets::LENSES_WHY)
        );

        app.set_fetch_open(false);
        Shown::Settings.open(&app);
        assert!(app.get_settings_open());

        // A tool brings the Crop tab with it.
        assert_eq!(app.get_panel_tab(), "Develop");
        Shown::Crop.open(&app);
        assert_eq!(app.get_panel_tab(), "Crop");
        assert!(app.get_crop_mode());
        Shown::Level.open(&app);
        assert!(app.get_level_mode());
        Shown::Guide.open(&app);
        assert_eq!(app.get_guide_mode(), "Vertical");
    }

    #[test]
    fn tab_puts_the_panels_away_and_brings_them_back() {
        let app = crate::testing::window(3);
        let (_state, _worker) = crate::testing::retouch_state(&app);
        let tab = slint::platform::Key::Tab;
        assert!(!app.get_panels_hidden());
        crate::testing::press(&app, tab);
        assert!(app.get_panels_hidden(), "Tab hides them");
        crate::testing::press(&app, tab);
        assert!(!app.get_panels_hidden(), "and Tab again shows them");

        // A sheet has the keys: Tab under it is not this one.
        app.set_export_open(true);
        crate::testing::press(&app, tab);
        assert!(!app.get_panels_hidden(), "not under a sheet");
        app.set_export_open(false);

        // Nor over the grid, which has the window already.
        app.set_grid_open(true);
        crate::testing::press(&app, tab);
        assert!(!app.get_panels_hidden(), "not over the grid");
    }
}
