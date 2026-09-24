//! The editor: Slint for the panels, wgpu for the viewport, the
//! engine on a worker thread, the edit as `greycard-edit` defines it,
//! kept in a sidecar beside each file.

// No console window behind the editor. Windows gives every
// console-subsystem binary one whether or not it wants it, so the
// Start menu would open a black box beside the window; the GUI
// subsystem is what a windowed application asks for. The log is
// unaffected, since it is a file sink and not the terminal, and
// greycard, the command line, stays a console binary. A terminal
// that launches this still receives what it prints, on the handles
// it passes down, but does not wait for it.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod ai;
mod cull;
mod display;
mod export;
mod files;
mod filter;
mod finder;
mod finish;
mod geometry;
mod grid;
mod log;
mod outline;
mod panel;
mod placeholder;
mod render;
mod report;
mod scope;
mod selection;
mod settings;
#[cfg(test)]
pub(crate) mod testing;
mod wheel;
mod worker;
mod zoom;

pub(crate) use std::cell::RefCell;
pub(crate) use std::collections::HashMap;
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::rc::Rc;
pub(crate) use std::sync::Arc;

pub(crate) use anyhow::{Context, Result};
pub(crate) use clap::Parser;
pub(crate) use slint::wgpu_30::{WGPUConfiguration, WGPUSettings, wgpu};
pub(crate) use slint::{ComponentHandle, Model, ModelRc, VecModel};

pub(crate) use greycard_core::CameraProfile;
pub(crate) use greycard_core::color::{Matrix3, WhitePoint, invert3, mul3, resolve_white_balance};
pub(crate) use greycard_core::develop::defringe;
pub(crate) use greycard_core::raw::{Orientation, RawFrame};
pub(crate) use greycard_edit::brush::{Op, Raster, Stroke};
pub(crate) use greycard_edit::curve::{self, Channel, Parametric, Point};
pub(crate) use greycard_edit::geometry::{
    Aspect, Crop, Geometry, Guide, Guiding, Handle, MAX_ANGLE, MAX_TILT,
};
pub(crate) use greycard_edit::grading::{Grading, Range, Wheel};
pub(crate) use greycard_edit::mask::{Component, MIN_RADIUS, Mask, Mode, Pick, Shape};
pub(crate) use greycard_edit::meta::{self, Meta};
pub(crate) use greycard_edit::mixer::BANDS;
pub(crate) use greycard_edit::preset::{self, Entry, Section};
pub(crate) use greycard_edit::retouch::{Method as RetouchMethod, Patch};
pub(crate) use greycard_edit::xmp;
pub(crate) use greycard_edit::{
    Adjustment, Color, Curves, Demosaic, Edit, Grain, Learned, Light, Look, Mixer, Preset, Sidecar,
    Tint, Vignette, WhiteBalance, describe,
};

use crate::panel::color::WhiteKey;
use crate::panel::cull::{Cull, deliver_preview};
use crate::panel::deliver::Landed;
use crate::panel::mask::{MaskDrag, Placing};
use crate::panel::retouch::PatchShape;
use crate::panel::viewport::Picking;
pub(crate) use ai::{Key, model_for, prompted};
pub(crate) use finish::{Baked, Local, MAX_LOCALS, RasterRef};
use panel::startup::main;
pub(crate) use render::{Renderer, View};
pub(crate) use worker::{
    Developed, Job, LearnedReport, LensReport, Outcome, SourceKind, WhiteBase, Worker,
};
slint::include_modules!();

#[derive(Parser)]
#[command(
    name = "greycard-ui",
    version,
    about = "RAW editor: open a file or a folder, develop it, export it"
)]
struct Cli {
    /// A RAW, JPEG, PNG or TIFF file to open, or a directory of them.
    /// Without one: the last file's folder if the settings remember
    /// one that still exists, else the desktop's folder chooser.
    path: Option<PathBuf>,
    /// Write the viewport to this PNG once the first develop is on screen, then quit
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// Write the whole window, panel included, to this PNG once the
    /// first develop is on screen, then quit
    #[arg(long)]
    snapshot: Option<PathBuf>,
    /// Scroll the panel down this many logical pixels before a
    /// snapshot, so it can show a section below the fold
    #[arg(long, value_name = "PX")]
    panel_scroll: Option<f32>,
    /// Exposure to open with, in stops
    #[arg(long, default_value_t = 0.0)]
    exposure: f32,
    /// Set the panel's temperature after opening, without a develop: the
    /// preview path alone (for checking it against --develop-temperature)
    #[arg(long)]
    preview_temperature: Option<f32>,
    /// Develop at this temperature from the start: the exact path
    #[arg(long)]
    develop_temperature: Option<f32>,
    /// Do not read or write sidecars
    #[arg(long)]
    no_sidecars: bool,
    /// Write the meta to an `.xmp` beside the frame as well, for
    /// Lightroom, Bridge and darktable, for this run; the Settings
    /// sheet's choice otherwise. An XMP that is there is read either way.
    #[arg(long)]
    xmp_sidecars: bool,
    /// Write sidecars under a hidden `.greycard` folder in the
    /// frame's folder rather than beside it, for this run; the
    /// Settings sheet's choice otherwise. A sidecar in either place is
    /// read either way.
    #[arg(long)]
    sidecar_folder: bool,
    /// The monitor's ICC profile to open with, over the panel's
    /// remembered choice (colord's profile for the primary display
    /// unless changed)
    #[arg(long)]
    display_profile: Option<PathBuf>,
    /// Open with plain sRGB output, no monitor profile
    #[arg(long)]
    no_display_profile: bool,
    /// Open at this zoom, display pixels per image pixel (0: fit)
    #[arg(long, default_value_t = 0.0)]
    zoom: f32,
    /// Open on the grid of the folder rather than on the picture
    #[arg(long)]
    grid: bool,
    /// Open with the panels, the strip and the status plate put away,
    /// as Tab does, for a snapshot of the picture alone
    #[arg(long)]
    hide_panels: bool,
    /// The grid's cell, logical pixels (96 to 512); without it,
    /// whichever size it was left at
    #[arg(long, value_name = "PX")]
    grid_cell: Option<f32>,
    /// Say more on the terminal: once, what the log file keeps;
    /// twice, debug lines from greycard's own crates in both.
    /// RUST_LOG, when set, decides instead
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Export the first file under its edit to this path, then quit
    #[arg(long)]
    export: Option<PathBuf>,
    /// The export's long edge in pixels, over the sheet's remembered
    /// size; never enlarges
    #[arg(long)]
    long_edge: Option<u32>,
    /// What an export does when a file of that name is there already:
    /// increment, overwrite or skip; without it, the sheet's choice
    #[arg(long, value_name = "WHAT", value_parser = on_exists_named)]
    on_exists: Option<export::OnExists>,
    /// Paint this adjustment's mask over the picture (its index, from
    /// one), for a screenshot
    #[arg(long)]
    show_mask: Option<usize>,
    /// Choose this repair patch on opening (its index, from one), for
    /// a screenshot of its shape on the Retouch tab
    #[arg(long)]
    patch: Option<usize>,
    /// Open this sheet once the picture is up, for a snapshot:
    /// export, preset, fetch (the model sheet, with sample text),
    /// lenses (the lens profiles' first-launch offer), settings, sync
    /// (over the frames selected) or synced (the sync applied on its
    /// defaults at once, the sheet never drawn)
    #[arg(long, value_name = "NAME", value_parser = panel::viewport::Shown::sheet, conflicts_with = "tool")]
    sheet: Option<panel::viewport::Shown>,
    /// Put this Crop-tab tool in hand once the picture is up, for a
    /// snapshot: crop, level or guide
    #[arg(long, value_name = "NAME", value_parser = panel::viewport::Shown::tool)]
    tool: Option<panel::viewport::Shown>,
    /// Paint the capture sharpening's mask over the picture, for a
    /// screenshot
    #[arg(long)]
    sharpen_mask: bool,
    /// The scope the panel opens on: RGB, Wave, Parade or Vector.
    /// Without it, whichever was on last time
    #[arg(long)]
    scope: Option<String>,
    /// Paint the clipping warnings over the picture, the shadows and
    /// the highlights both, for a screenshot
    #[arg(long)]
    clipping: bool,
    /// Soft proof through this profile from the start: sRGB, Display
    /// P3, Rec.2020 or an ICC file's path
    #[arg(long)]
    proof: Option<String>,
    /// The proof's intent: Perceptual or Relative
    #[arg(long)]
    proof_intent: Option<String>,
    /// Mark what the proof's profile cannot hold
    #[arg(long)]
    gamut_warning: bool,
    /// The panel's tab to show: Develop, Crop, Masks or Retouch
    #[arg(long)]
    tab: Option<String>,
    /// Lay this preset (by name, or a preset file's path) over the
    /// first file's edit on opening, as a step in its history
    #[arg(long, value_name = "NAME")]
    preset: Option<String>,
    /// Run the engine's ops on the CPU even where the GPU could take
    /// them, for checking the one against the other
    #[arg(long)]
    cpu_ops: bool,
    /// Once the first develop is on screen, move the sharpen's radius
    /// this many times, logging the time from each move to its frame,
    /// then quit with the mean
    #[arg(long, value_name = "N", hide = true)]
    time_sharpen: Option<u32>,
    /// Once the first develop is on screen, step to the next frame
    /// this many times, a tenth of a second apart, logging the time
    /// from each step to the frame that develops it, then quit with
    /// the mean
    #[arg(long, value_name = "N", hide = true)]
    time_select: Option<u32>,
    /// Step to the next frame once the first develop is on screen,
    /// and take the snapshot as soon as that frame's camera picture
    /// stands in for its develop, rather than waiting for the develop
    #[arg(long, hide = true)]
    snapshot_placeholder: bool,
    /// Open in culling mode: the camera's JPEG in the viewport, no
    /// develop until Enter
    #[arg(long)]
    cull: bool,
    /// Open the culling mode's compare view with this many frames, 2
    /// or 4 (implies --cull)
    #[arg(long, value_name = "N")]
    cull_compare: Option<usize>,
    /// The browser's filter to open with: All, Picks or "No rejects"
    #[arg(long, value_name = "WHAT")]
    filter: Option<String>,
    /// In culling mode, step to the next frame this many times, a
    /// tenth of a second apart, logging the time from each step to
    /// the frame that shows it, then quit with the mean (implies
    /// --cull)
    #[arg(long, value_name = "N", hide = true)]
    time_cull: Option<u32>,
    /// In culling mode, press Enter once the first picture shows, so
    /// a snapshot or a screenshot catches the develop the leaving
    /// asks for (implies --cull)
    #[arg(long, hide = true)]
    cull_develop: bool,
    /// Turn the selected frame this many quarter turns clockwise
    /// once its first picture shows — the culling preview with
    /// --cull, the develop otherwise — for a snapshot of it
    #[arg(long, value_name = "N", hide = true)]
    turn: Option<i32>,
    /// Put these browser rows (from 0, comma-separated) in the
    /// selection beside the frame opened, as Ctrl+clicks would, for
    /// a snapshot of a set or `--sheet sync`
    #[arg(long, value_name = "ROWS", value_delimiter = ',', hide = true)]
    also: Vec<usize>,
    /// In culling mode, open the move-rejects sheet once the first
    /// picture shows, for a snapshot of it (implies --cull)
    #[arg(long, hide = true)]
    ask_rejects: bool,
    /// As --ask-rejects, and answer the sheet with yes: the rejects
    /// are moved, for a snapshot of the browser after it
    #[arg(long, hide = true)]
    move_rejects: bool,
}

/// What the UI thread holds between events.
/// What the download sheet can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fetch {
    Model(&'static greycard_ai::Model),
    /// The lens database.
    Lenses,
}

pub(crate) struct State {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) current: Option<usize>,
    /// The browser's selection, as files: the current frame and
    /// whatever else Ctrl, Shift or Shift and an arrow put beside it.
    /// Read through `selection::frames`, which puts the current frame
    /// back in whatever this holds.
    pub(crate) picked: Vec<usize>,
    /// The edit the worker is developing, or last developed.
    pub(crate) edit: Edit,
    /// Every file's sidecar: its current edit and history.
    pub(crate) sidecars: Vec<Sidecar>,
    /// Which files are raws with no sidecar yet, whose learned-denoiser
    /// blend the worker seeds from the ISO on their first open.
    pub(crate) seed_blend: Vec<bool>,
    pub(crate) write_sidecars: bool,
    /// Whether a meta written to the `.gcd` is also written to an
    /// `.xmp` beside the frame: `settings.xmp_sidecars`, or
    /// `--xmp-sidecars`. Reading one is not a choice, so nothing
    /// here guards it.
    pub(crate) xmp_sidecars: bool,
    /// Where a sidecar written goes, beside the frame or under the
    /// hidden folder: `settings.sidecars_in_folder`, or
    /// `--sidecar-folder`. Reading looks in both places either way.
    pub(crate) placement: greycard_edit::Placement,
    /// The developed image waiting to go to the GPU, if any.
    pub(crate) pending: Option<Landed>,
    /// `--time-sharpen`: the moves left, when the last was sent, and
    /// the milliseconds each took to its frame.
    pub(crate) time_sharpen: Option<(u32, Option<std::time::Instant>, Vec<f64>)>,
    /// `--time-cull`: the steps left and the milliseconds each took
    /// from the key to the frame that showed the next picture.
    pub(crate) time_cull: Option<(u32, Vec<f64>)>,
    /// `--time-select`: the steps left, and the milliseconds each
    /// step took from the key to the frame that showed the camera's
    /// picture and to the frame that showed the develop.
    pub(crate) time_select: Option<(u32, Vec<f64>, Vec<f64>)>,
    /// When the selection last moved in the develop view, until the
    /// frame that shows the frame it moved to.
    pub(crate) selected_at: Option<std::time::Instant>,
    /// `--cull-develop`: leave the mode once its first picture shows.
    pub(crate) cull_develop: bool,
    /// `--snapshot-placeholder`: step to the next frame once the
    /// first develop is on screen, and let a capture go as soon as
    /// that frame's camera picture is up rather than waiting for its
    /// develop.
    pub(crate) snapshot_placeholder: bool,
    /// `--turn`: quarter turns to put on the selection once the first
    /// picture shows, for a capture.
    pub(crate) turn_at_start: Option<i32>,
    /// `--turn` was asked for and the picture it leads to is not on
    /// screen yet. A capture waits for it, or it would catch the
    /// develop that fired the key rather than the turned frame.
    pub(crate) awaiting_turn: bool,
    /// `--ask-rejects`: open the move-rejects sheet once it does;
    /// `--move-rejects`: and answer it with yes.
    pub(crate) ask_rejects: bool,
    pub(crate) move_rejects: bool,
    /// Culling mode (notes §80), while it is on.
    pub(crate) cull: Option<Cull>,
    /// The camera's pictures the develop view keeps: what the mode
    /// left behind on its way out, and what a select decoded, with
    /// the textures made from them. They are drawn only while
    /// `placeholder` says a develop is pending.
    pub(crate) hold: Option<Cull>,
    /// A frame selected whose develop has not landed: the camera's
    /// JPEG stands in for it meanwhile.
    pub(crate) placeholder: Option<placeholder::Wait>,
    /// The previews decoded ahead of the arrow, on their own threads.
    pub(crate) prefetch: cull::Prefetcher,
    /// Open in culling mode, with the compare view at this many
    /// frames, once the first file is chosen (`--cull`).
    pub(crate) cull_at_start: Option<usize>,
    /// The browser's rows: the files it lists, in order, the filter
    /// applied. `selected` on the window is a row; `current` here is
    /// a file.
    pub(crate) shown: Vec<usize>,
    pub(crate) filter: filter::Filter,
    /// The compare view's tiles on the view, one model for the life
    /// of the window, as the handles' are.
    pub(crate) compare_tiles: Rc<VecModel<CompareTile>>,
    /// The file to open once the window has its device: the first
    /// develop then runs where every later one does, so a screenshot
    /// or an export shows the same path a session would.
    pub(crate) select_at_start: Option<usize>,
    /// `--also`: rows put in the set once the first frame is opened.
    pub(crate) also_at_start: Vec<usize>,
    /// The white balance of the image on the GPU.
    pub(crate) base_white: Option<WhiteBase>,
    /// The open frame and its profile, for the white balance preview.
    pub(crate) frame: Option<(Arc<RawFrame>, Box<CameraProfile>)>,
    /// A raw's scene or a picture already rendered, from the open.
    pub(crate) source: finish::Source,
    /// The last preview matrix, keyed by what it was computed for.
    pub(crate) white_cache: Option<(WhiteKey, Matrix3)>,
    /// Holds a develop back until the sliders rest.
    pub(crate) debounce: slint::Timer,
    /// Holds a sidecar write back until the sliders rest.
    pub(crate) save_timer: slint::Timer,
    /// A temperature to put on the panel once the file opens, for the
    /// preview check.
    pub(crate) preview_temperature: Option<f32>,
    /// The image the viewport shows, in pixels, for fit and zoom.
    pub(crate) image_size: (u32, u32),
    /// Display pixels per image pixel; 0 means fit to the view.
    pub(crate) zoom: f32,
    /// The image pixel at the view's center.
    pub(crate) center: (f32, f32),
    pub(crate) renderer: Option<Renderer>,
    /// The viewport's adapter as the log names it, for a report; none
    /// until the rendering setup has run.
    pub(crate) gpu: Option<String>,
    /// The last scope bins: the histogram, the backdrop of the curve
    /// editor, and after it whatever `scope` asked for.
    pub(crate) bins: Option<Vec<u32>>,
    /// Which scope the panel shows.
    pub(crate) scope: scope::Scope,
    /// The curve point under the pointer while it is held.
    pub(crate) curve_drag: Option<usize>,
    /// The developed picture's size, the source of the geometry.
    /// Cleared on every select: until this frame's develop lands
    /// nothing may take the last frame's shape for this one's.
    pub(crate) source_size: (u32, u32),
    /// The size of the developed picture on the GPU, which is what
    /// the viewport draws while a develop is pending and no camera
    /// picture stands in. Not cleared by a select: the picture stays
    /// on screen until another replaces it.
    pub(crate) shown_size: (u32, u32),
    /// The crop as it was when a handle was pressed.
    pub(crate) crop_drag: Option<Crop>,
    /// Which look the panel edits: an adjustment by index, or the
    /// global one. The panel is the truth for that look; `edit` is
    /// for everything else, the other adjustments included.
    pub(crate) target: Option<usize>,
    /// A mask shape being drawn in the viewport.
    pub(crate) placing: Option<Placing>,
    /// A dropper's press, while the pointer is down.
    pub(crate) picking: Option<Picking>,
    /// A mask to paint whatever the panel says, for a screenshot.
    pub(crate) show_mask: Option<usize>,
    /// A patch to choose on the first file, for a screenshot.
    pub(crate) show_patch: Option<usize>,
    /// A shape's handle being dragged.
    pub(crate) mask_drag: Option<MaskDrag>,
    /// The brushes' rasters, by adjustment id and component index,
    /// kept up with their strokes a stamp at a time.
    pub(crate) rasters: HashMap<(u64, usize), Arc<Raster>>,
    /// The repair tool in hand, the stroke under way (a patch index),
    /// and a patch's handle being dragged: the patch as grabbed, the
    /// handle, and where it was on the view.
    pub(crate) retouching: Option<RetouchMethod>,
    pub(crate) patch_stroke: Option<usize>,
    pub(crate) patch_drag: Option<(Patch, usize, (f32, f32))>,
    /// The patch handles' positions on the view, one model for the
    /// life of the window, as the masks' are.
    pub(crate) patch_handles: Rc<VecModel<Pt>>,
    /// The chosen patch's rings in the picture's units, for the
    /// patch they were made for, so a frame maps them and no more.
    pub(crate) patch_shape: Option<PatchShape>,
    /// The perspective guide's first stroke, while it waits for the
    /// second.
    pub(crate) guiding: Guiding,
    /// That stroke's two ends where they are in the view now, for the
    /// overlay to draw.
    pub(crate) guide_kept: Rc<VecModel<Pt>>,
    /// The filmstrip's pictures as the worker made them, unturned, and
    /// the turns and mirror each is shown with.
    pub(crate) thumb_base: Vec<Option<(u32, u32, Vec<u8>)>>,
    pub(crate) thumb_shown: Vec<Option<(u8, bool)>>,
    /// The long edge each picture was made at, zero until one
    /// arrives, and the long edge each was last asked for; and the
    /// size the grid is asking for now, which follows the cell rather
    /// than climbing with it, so one look at the largest cell does
    /// not make every picture afterwards that size.
    pub(crate) thumb_made: Vec<u32>,
    pub(crate) thumb_asked: Vec<u32>,
    pub(crate) thumb_want: u32,
    /// The frames the grid last said it shows, first and last.
    pub(crate) grid_shown: Option<(i32, i32)>,
    /// How each frame stands in its own file — its orientation tag
    /// and the size a develop comes out at — read once and kept. No
    /// pixel is decoded for it, and nothing asks until a turn is
    /// pressed or an XMP is written, so a folder of frames nobody
    /// turns and nobody shares never pays for it.
    pub(crate) stances: HashMap<PathBuf, greycard_core::decode::Stance>,
    /// The learned masks' rasters as the worker made them, with the
    /// shape each was made for.
    pub(crate) learned: HashMap<Key, (Shape, Arc<Raster>)>,
    /// The shapes asked of the worker and not yet answered.
    pub(crate) asked: HashMap<Key, Shape>,
    /// The model store, to know what is there.
    pub(crate) store: Option<greycard_ai::Store>,
    /// What the sheet offers to fetch, and whether something is coming.
    pub(crate) fetch: Option<Fetch>,
    pub(crate) fetching: bool,
    /// Models declined this session, not to be asked for again.
    pub(crate) declined: Vec<&'static str>,
    /// The lens profiles' first-launch offer: whether it was ever
    /// answered Not now (`settings.lenses_declined`), and whether it
    /// has been made this launch.
    pub(crate) lenses_declined: bool,
    pub(crate) lenses_asked: bool,
    /// What the lens database made of the open file.
    pub(crate) lens: LensReport,
    /// The camera profiles the profile directory holds, and the open
    /// file's camera, for the panel's list and its warning.
    pub(crate) profiles: Vec<greycard_edit::camera::Entry>,
    pub(crate) camera: (String, String),
    /// The look tables the look directory holds, for the panel's list.
    pub(crate) looks: Vec<greycard_edit::look::Entry>,
    /// The look the viewport draws through, and the section it was
    /// resolved for. Kept here rather than read on every frame: the
    /// table is on the GPU already, and reading the section is a stat
    /// of the file. `None` for the section asks for it again, which
    /// is what a re-listing sets it to.
    pub(crate) look: Option<greycard_core::lut::Look>,
    pub(crate) look_for: Option<greycard_edit::LookLut>,
    /// The profile the white balance is solved through, and the name
    /// it was resolved for: the edit's DCP when it names one that
    /// reads, else the file's own. The develop converts temperature
    /// and tint through the chosen profile's matrices, so the panel's
    /// preview and the neutral dropper have to use the same ones or
    /// they answer for a camera the picture was not developed as.
    pub(crate) white_profile: Option<(String, CameraProfile)>,
    /// The handles' positions on the view, one model for the life of
    /// the window: replaced, the handle under the pointer would be
    /// made anew mid-drag and lose the press.
    pub(crate) mask_handles: Rc<VecModel<Pt>>,
    /// An object's boxes and picks on the view.
    pub(crate) mask_boxes: Rc<VecModel<MaskBox>>,
    pub(crate) mask_picks: Rc<VecModel<MaskPick>>,
    pub(crate) screenshot: Option<PathBuf>,
    pub(crate) snapshot: Option<PathBuf>,
    /// How far down the panel is scrolled before the snapshot,
    /// logical pixels.
    pub(crate) panel_scroll: Option<f32>,
    /// A sheet or a tool put on screen before the snapshot.
    pub(crate) snapshot_shown: Option<panel::viewport::Shown>,
    /// The displays colord knows, primary first, for the panel's
    /// "System" choice.
    pub(crate) monitors: Vec<display::Monitor>,
    /// What the display table on the renderer was built for: the
    /// output space, the proof if one is on, and the monitor.
    pub(crate) lut_for: Option<(
        export::Space,
        Option<display::Proof>,
        display::MonitorProfile,
    )>,
    /// What the encoded path's table was built for: the monitor
    /// alone, since the camera's JPEG is sRGB and takes no proof.
    pub(crate) encoded_lut_for: Option<display::MonitorProfile>,
    /// An export to run once the first develop lands, then quit.
    pub(crate) export_then_quit: Option<PathBuf>,
    /// The presets, as the store lists them, and the store.
    pub(crate) presets: Vec<Entry>,
    pub(crate) preset_store: Option<preset::Store>,
    /// The save sheet's choice of sections, in `Section::ALL`'s order.
    pub(crate) preset_sections: Rc<VecModel<bool>>,
    /// The sync sheet's, the same way.
    pub(crate) sync_sections: Rc<VecModel<bool>>,
    /// The frame and the targets the sync sheet was opened on, so an
    /// Apply can refuse a selection that moved under it.
    pub(crate) sync_asked: Option<(usize, Vec<usize>)>,
    /// A state shown in the viewport in place of the panel's while a
    /// history or snapshot row is under the pointer, and the status
    /// line it covers meanwhile.
    pub(crate) peek: Option<Edit>,
    /// The edit of the picture still on the GPU after another file was
    /// chosen: the viewport shows that picture under this look until
    /// the new file's develop lands, rather than the new look over the
    /// old picture.
    pub(crate) held: Option<Edit>,
    pub(crate) status_kept: Option<slint::SharedString>,
    /// The view (the frame's size and the center) before a peek, put
    /// back after it: a hover over a row is not to lose a zoomed place.
    pub(crate) view_kept: Option<((u32, u32), (f32, f32))>,
    /// The mask toggle as it was before a tool went in hand; the mask
    /// shows while a shape is placed, and the toggle comes back after,
    /// unless it was thrown meanwhile.
    pub(crate) show_mask_kept: Option<bool>,
    pub(crate) generation: u64,
    /// Whether the command line asked for a snapshot, a screenshot or
    /// an export. All three wait for a picture, and nobody is reading
    /// the status line, so a failure has to end the run itself.
    pub(crate) batch: bool,
    /// Something a batch run asked for did not happen: the exit code
    /// says so.
    pub(crate) failed: bool,
}

impl State {
    /// A blank state for `files`, on `app`'s window: no picture
    /// chosen, no sidecar read, nothing running or remembered. The
    /// one place every field is named; `run` overlays what the
    /// command line and the settings choose with `..Self::empty(...)`,
    /// and a test wires only the handful a callback under test reads,
    /// the same way. Add a field once, here, and both stay whole.
    pub(crate) fn empty(files: Vec<PathBuf>, app: &App) -> Self {
        let count = files.len();
        Self {
            files,
            current: None,
            picked: Vec::new(),
            edit: Edit::default(),
            sidecars: vec![Sidecar::default(); count],
            seed_blend: vec![false; count],
            write_sidecars: false,
            xmp_sidecars: false,
            placement: greycard_edit::Placement::Beside,
            pending: None,
            time_sharpen: None,
            time_cull: None,
            time_select: None,
            selected_at: None,
            cull_develop: false,
            turn_at_start: None,
            awaiting_turn: false,
            ask_rejects: false,
            move_rejects: false,
            snapshot_placeholder: false,
            cull: None,
            hold: None,
            placeholder: None,
            // Previews come back to the UI thread as the worker's do.
            prefetch: {
                let app_weak = app.as_weak();
                cull::Prefetcher::new(move |loaded| {
                    let app_weak = app_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            deliver_preview(&app, loaded);
                        }
                    });
                })
            },
            cull_at_start: None,
            shown: (0..count).collect(),
            filter: filter::Filter::default(),
            compare_tiles: Rc::new(VecModel::default()),
            select_at_start: None,
            also_at_start: Vec::new(),
            base_white: None,
            frame: None,
            source: finish::Source::Scene,
            white_cache: None,
            debounce: slint::Timer::default(),
            save_timer: slint::Timer::default(),
            preview_temperature: None,
            image_size: (1, 1),
            zoom: 0.0,
            center: (0.0, 0.0),
            renderer: None,
            gpu: None,
            bins: None,
            scope: scope::Scope::default(),
            curve_drag: None,
            // Nothing until a develop says so: a turn pressed
            // before one lands must not measure the frame by the
            // last frame's shape, or by a square.
            source_size: (0, 0),
            shown_size: (0, 0),
            crop_drag: None,
            target: None,
            placing: None,
            picking: None,
            show_mask: None,
            show_patch: None,
            mask_drag: None,
            rasters: HashMap::new(),
            retouching: None,
            patch_stroke: None,
            patch_drag: None,
            patch_handles: Rc::new(VecModel::default()),
            patch_shape: None,
            guiding: Guiding::default(),
            guide_kept: Rc::new(VecModel::default()),
            thumb_base: vec![None; count],
            thumb_shown: vec![None; count],
            thumb_made: vec![0; count],
            thumb_asked: vec![worker::THUMB_WIDTH; count],
            thumb_want: worker::THUMB_WIDTH,
            grid_shown: None,
            stances: HashMap::new(),
            learned: HashMap::new(),
            asked: HashMap::new(),
            store: None,
            fetch: None,
            fetching: false,
            declined: Vec::new(),
            lenses_declined: false,
            lenses_asked: false,
            lens: LensReport::NoDatabase,
            profiles: Vec::new(),
            camera: (String::new(), String::new()),
            looks: Vec::new(),
            look: None,
            look_for: None,
            white_profile: None,
            mask_handles: Rc::new(VecModel::default()),
            mask_boxes: Rc::new(VecModel::default()),
            mask_picks: Rc::new(VecModel::default()),
            screenshot: None,
            snapshot: None,
            panel_scroll: None,
            snapshot_shown: None,
            monitors: Vec::new(),
            lut_for: None,
            encoded_lut_for: None,
            export_then_quit: None,
            presets: Vec::new(),
            preset_store: None,
            preset_sections: Rc::new(VecModel::from(vec![false; Section::ALL.len()])),
            sync_sections: Rc::new(VecModel::from(vec![false; Section::ALL.len()])),
            sync_asked: None,
            peek: None,
            held: None,
            status_kept: None,
            view_kept: None,
            show_mask_kept: None,
            generation: 0,
            batch: false,
            failed: false,
        }
    }
}

/// A policy named on the command line.
fn on_exists_named(name: &str) -> Result<export::OnExists, String> {
    export::OnExists::from_name(name)
        .ok_or_else(|| format!("want increment, overwrite or skip, not {name}"))
}

pub(crate) fn install_callbacks(app: &App, state: Rc<RefCell<State>>, worker: Rc<Worker>) {
    panel::cull::install(app, &state, &worker);
    panel::mask::install(app, &state, &worker);
    panel::browser::install(app, &state, &worker);
    panel::color::install(app, &state, &worker);
    panel::viewport::install(app, &state, &worker);
    panel::deliver::install(app, &state, &worker);
    panel::edit::install(app, &state, &worker);
    panel::assets::install(app, &state, &worker);
    panel::crop::install(app, &state, &worker);
    panel::curve::install(app, &state, &worker);
    panel::history::install(app, &state, &worker);
    panel::retouch::install(app, &state, &worker);
    panel::prefs::install(app, &state, &worker);
    panel::sync::install(app, &state, &worker);
    report::install(app, &state);

    // Deliveries from the worker need the state and the worker too.
    STATE.with(|s| *s.borrow_mut() = Some(state));
    WORKER.with(|w| *w.borrow_mut() = Some(worker));
}

thread_local! {
    static STATE: RefCell<Option<Rc<RefCell<State>>>> = const { RefCell::new(None) };
    static WORKER: RefCell<Option<Rc<Worker>>> = const { RefCell::new(None) };
}

// Keep the wgpu types in one place for the renderer module.
pub(crate) use wgpu as gpu;
