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

mod adapter;
mod ai;
mod archive;
mod camera_match;
mod carry;
mod cells;
mod chooser;
mod clipboard;
mod collections;
mod cull;
mod delete;
mod display;
mod edited;
mod entry;
mod export_queue;
mod files;
mod filter;
mod finder;
mod fit_lightroom;
mod gpu_errors;
mod gpu_lost;
mod gpu_marker;
mod grid;
mod headless;
mod import;
mod keys;
mod library;
mod lightbox;
mod log;
mod match_compare;
mod measure_baseline;
mod measure_crop;
mod measure_lightroom;
mod mounts;
mod naming;
mod outline;
mod owners;
mod panel;
mod placeholder;
mod previews;
mod queue;
mod ram;
mod reduced;
mod render;
mod report;
mod roots;
mod rows;
mod scope;
mod selection;
mod settings;
mod sheet;
mod sort;
mod stacks;
mod sync;
mod tags;
#[cfg(test)]
pub(crate) mod testing;
mod thumbpool;
mod tree;
mod update;
mod versions;
mod wheel;
mod window_fit;
mod worker;
mod zoom;

// What renders a developed frame into a picture or a file, in a crate
// of its own below the window; named here as the modules were.
pub(crate) use greycard_render::{agx, border, detail, export, finish, geometry, watermark};

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
pub(crate) use greycard_core::color::{Matrix3, WhitePoint, resolve_white_balance};
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
    Tint, Vignette, WhiteBalance,
};

use crate::panel::color::WhiteKey;
use crate::panel::cull::{Cull, deliver_preview};
use crate::panel::deliver::Landed;
use crate::panel::mask::{MaskDrag, Placing};
use crate::panel::retouch::PatchShape;
use crate::panel::viewport::Picking;
pub(crate) use ai::{Key, prompted};
pub(crate) use finish::{Local, MAX_LOCALS, RasterRef};
use panel::startup::main;
pub(crate) use render::{Renderer, View};
pub(crate) use worker::{
    Developed, Job, LearnedReport, LensReport, Outcome, SourceKind, WhiteBase, Worker,
};
// The window and its panels, compiled from `.slint` in greycard-slint.
pub(crate) use greycard_slint::*;

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
    /// Open with AgX as the display curve, Blender's formation with
    /// its Punchy look, in place of the curve per channel: the LIGHT
    /// section's switch, for a comparison from the command line; on
    /// the frame opened only, as --exposure is (with --export --all,
    /// on every frame)
    #[arg(long)]
    agx: bool,
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
    /// remembered choice (the system's profile for the primary display
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
    /// as Tab does, for a snapshot of the picture alone; not kept for
    /// the next run
    #[arg(long)]
    hide_panels: bool,
    /// The grid's cell, logical pixels (96 to 512); without it,
    /// whichever size it was left at
    #[arg(long, value_name = "PX")]
    grid_cell: Option<f32>,
    /// Open the window at this size in logical pixels, WxH, for a
    /// capture; the screen's fit still applies
    #[arg(long, value_name = "WxH", value_parser = parse_window_size, hide = true)]
    window: Option<(f32, f32)>,
    /// Say more on the terminal: once, what the log file keeps;
    /// twice, debug lines from greycard's own crates in both.
    /// RUST_LOG, when set, decides instead
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Export the first file under its edit to this path, with no
    /// window, then quit. A folder (one that is there, or a path
    /// ending in a separator) takes the set instead, the first file
    /// and the --also rows (or with --all, every frame), each frame
    /// under its own edit and its own name. Exits 0 when every frame
    /// was written as its edit asks (or left, as --on-exists skip
    /// asks); 2 when every frame was written
    /// but some without something the edit names that this machine
    /// has not got (a look, a learned model) or could not run, each
    /// named on stderr; 1, with the reason, when a frame was not
    /// written or the run could not begin
    #[arg(long)]
    export: Option<PathBuf>,
    /// With --export: export this version of each frame, by the name
    /// the user gave it, rather than the file's own edit; a name two
    /// versions share takes the first by position and says so. When
    /// a frame has no version of that name, the run stops before
    /// writing anything and exits 1 naming it. (--version prints the
    /// build's version, so
    /// this flag is --version-name)
    #[arg(long, requires = "export")]
    version_name: Option<String>,
    /// Measure the camera match under each display curve on the same
    /// frames, with no window, and write a report (report.json and
    /// report.md) into this folder: the library's groups (--library,
    /// --roots), the sheet's sample of each, every frame developed once
    /// and finished under per channel and under AgX, each curve fitted
    /// and held out. Writes no look, no sidecar and nothing in the
    /// library; a frame's numbers are kept under the folder, so a second
    /// run redoes only the analysis
    #[arg(long, value_name = "DIR")]
    match_compare: Option<PathBuf>,
    /// With --match-compare, sample each group down to this many frames
    /// rather than the sheet's 40
    #[arg(long, value_name = "N", requires = "match_compare")]
    match_frames: Option<usize>,
    /// With --match-compare, only the groups whose look name contains
    /// this; repeatable
    #[arg(long, value_name = "TEXT", requires = "match_compare")]
    match_group: Vec<String>,
    /// With --match-compare, take the frames' numbers kept by this
    /// other build of the editor (as a report's "Build" line names it)
    /// rather than develop them again; repeatable, and the report lists
    /// the builds its records came from. Only for a change to the
    /// analysis: after a change to the develop, let them be made again
    #[arg(long, value_name = "BUILD", requires = "match_compare")]
    match_reuse_records: Vec<String>,
    /// Measure the default develop's brightness against the camera's
    /// own JPEG per body, with no window, and write a report
    /// (report.md, report.json, frames.jsonl) into this folder: per
    /// body a sample spread over ISO, folders and dates, from the
    /// library (--library, --roots) or from the folder given as the
    /// path, each frame laid under its camera's JPEG and matched in
    /// lightness as the camera match matches it, and under the body's
    /// fitted look where the store has one. Writes nothing else
    #[arg(long, value_name = "DIR")]
    measure_baseline: Option<PathBuf>,
    /// With --measure-baseline, sample each body down to this many
    /// frames rather than 32
    #[arg(long, value_name = "N", requires = "measure_baseline")]
    baseline_frames: Option<usize>,
    /// With --measure-baseline, only the bodies whose name contains
    /// this; repeatable
    #[arg(long, value_name = "TEXT", requires = "measure_baseline")]
    baseline_body: Vec<String>,
    /// With --measure-baseline, measure every body at the one baseline
    /// every body had before the table of bodies, as that build
    /// rendered them
    #[arg(long, requires = "measure_baseline")]
    baseline_before: bool,
    /// Measure how close the Lightroom import's mapping lands on
    /// Lightroom's own exports of this catalog, with no window, and
    /// print the report: each image paired with its export (by the
    /// export history's time, else by name), its raw developed under
    /// the catalog's settings as the import maps them, through the
    /// camera's fitted look where the profile is the camera's own, and
    /// compared block by block in Oklab. Writes nothing but --out
    #[arg(long, value_name = "CATALOG")]
    measure_lightroom: Option<PathBuf>,
    /// With --measure-lightroom, or --lightroom for its sheet: where a
    /// root folder of the catalog is now, as `library import-lightroom
    /// --root` takes it (on the sheet, as if chosen there)
    #[arg(long, value_name = "CATALOG=HERE", value_parser = root_mapping)]
    root: Vec<(String, PathBuf)>,
    /// With --measure-lightroom: another folder to look for exports
    /// in, beside the export folders found near the raws; repeatable
    #[arg(long, value_name = "DIR", requires = "measure_lightroom")]
    exports: Vec<PathBuf>,
    /// With --measure-lightroom: measure this many pairs, spread
    /// through the catalog, rather than all
    #[arg(long, value_name = "N", requires = "measure_lightroom")]
    limit: Option<usize>,
    /// With --measure-lightroom or --fit-lightroom: write the report as
    /// JSON here too
    #[arg(long, value_name = "FILE")]
    out: Option<PathBuf>,
    /// With --measure-lightroom: keep each measured frame's blocks (what
    /// the finish read over each kept block and what both sides came
    /// out as, no pixels) under this folder, for --fit-lightroom
    #[arg(long, value_name = "DIR", requires = "measure_lightroom")]
    blocks: Option<PathBuf>,
    /// With --measure-lightroom: leave the bodies' exposure offsets off,
    /// so the measurement reads where Lightroom sits against the
    /// engine's own default; the report says which offset was not
    /// applied
    #[arg(long, requires = "measure_lightroom")]
    no_body_offset: bool,
    /// With --measure-lightroom: map the settings with this scale at
    /// this value rather than the shipped one (NAME=VALUE, a name as
    /// --fit-lightroom's scale table prints it), to measure an earlier
    /// or a proposed mapper; the report says so. Repeatable
    #[arg(long, value_name = "NAME=VALUE", requires = "measure_lightroom")]
    scale: Vec<String>,
    /// Fit the Lightroom mapper's scales and its per-body exposure
    /// offsets to Lightroom's own exports, from the blocks
    /// --measure-lightroom --blocks kept under these folders (one a
    /// catalog; repeatable), with no window, and print the report:
    /// the error before and after by body and profile, held out by
    /// catalog, body and pasted edit, and each scale's profile.
    /// Writes nothing but --out
    #[arg(long, value_name = "DIR")]
    fit_lightroom: Vec<PathBuf>,
    /// With --fit-lightroom: rounds of the search over the parameters
    #[arg(
        long,
        value_name = "N",
        default_value_t = 4,
        requires = "fit_lightroom"
    )]
    fit_rounds: usize,
    /// With --fit-lightroom: fit the bodies' offsets alone, the scales
    /// left as shipped
    #[arg(long, requires = "fit_lightroom")]
    offsets_only: bool,
    /// With --fit-lightroom: skip the hold-outs (a fit each), for a
    /// quick look at the scales
    #[arg(long, requires = "fit_lightroom")]
    no_hold_out: bool,
    /// With --fit-lightroom: start from this scale at this value rather
    /// than the shipped one (NAME=VALUE, a name as the report's scale
    /// table prints it); the "before" numbers are then at it too.
    /// Repeatable
    #[arg(long, value_name = "NAME=VALUE", requires = "fit_lightroom")]
    fit_scale: Vec<String>,
    /// With --match-compare: write each fitted group's tables under
    /// this folder, as the sheet would write them into the look store
    /// (both curves), so a refit can run with no window and nothing
    /// in the store touched
    #[arg(long, value_name = "DIR", requires = "match_compare")]
    match_write: Option<PathBuf>,
    /// With --measure-lightroom: measure the import's crop rather than
    /// its color. Each cropped pair's raw is developed under the edit
    /// the import writes, its crop with it, finished at the export's
    /// long edge, and the export registered onto it: the sizes, the
    /// scale, the shift and the farthest corner in the export's pixels
    #[arg(long, requires = "measure_lightroom")]
    crops: bool,
    /// Fill the export sheet from this export preset, by name, for
    /// this run: its size, format, metadata and watermark. The format
    /// is still the --export path's extension when it names one, and
    /// --long-edge and --on-exists still win over it
    #[arg(long, value_name = "NAME")]
    export_preset: Option<String>,
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
    /// (over the frames selected), synced (the sync applied on its
    /// defaults at once, the sheet never drawn), preset-onto-set
    /// (the first stored preset clicked over the frames selected, no
    /// sheet involved), preset-remove (the first stored preset's bin
    /// clicked, its Remove waiting), preset-report (the preset
    /// import's report over sample presets; nothing imported), paste
    /// (the first frame's settings copied and the paste sheet opened
    /// over the frames selected), pasted (that
    /// paste applied at once, the sheet never drawn), import (the
    /// import sheet, its source --import's when given, else a card
    /// looked for), imported (the same, its import started once the
    /// source is read), match (the camera match's sheet, as the Look
    /// section's button opens it), matched (the same, its Fit
    /// pressed once the scope is read), refit (the same sheet as the
    /// Look section's refit opens it, over the chosen look's group),
    /// look-remove (the remove-a-look sheet over the chosen look, or
    /// the first in the directory; never answered), look-rename (the
    /// rename-a-look sheet the same way; a capture never renames),
    /// root-name (the library's
    /// first root's name, as its row's Rename... opens it), delete
    /// (the delete sheet over the selection; never answered), archive
    /// (the grid header's Back up or Bring back; never answered),
    /// archive-rejects (Remove rejects from the first archive, over the
    /// open folder's rejects folder; never answered), parts (the
    /// masks panel's People menu, over the mask --show-mask chooses),
    /// landscape (the masks panel's Landscape button pressed on the
    /// mask --show-mask chooses, its menu of what was found), lightroom
    /// (the Lightroom catalog import's sheet over --lightroom's
    /// catalog, at its roots), lightroom-report (the same at its dry
    /// run's report), lightroom-written (the same, written as its
    /// Write button writes it: point it at a scratch catalog),
    /// rawtherapee-offer (the offer of the RawTherapee profiles beside
    /// the open folder's frames, answered or not), rawtherapee (their
    /// import's sheet over the open folder, at its dry run's report),
    /// rawtherapee-written (the same, written as its Write button
    /// writes it: point it at a scratch folder), sort (the grid's
    /// Sort menu, open; with --grid), keys (the keys sheet, as ?
    /// opens it), help (the Help menu, open), or zoom (the Fit
    /// button's menu of zooms, open)
    #[arg(long, value_name = "NAME", value_parser = panel::viewport::Shown::sheet, conflicts_with = "tool")]
    sheet: Option<panel::viewport::Shown>,
    /// Open the frame menu over this row of the strip (from 0), or of
    /// the grid with --grid, once the picture is up, for a snapshot
    #[arg(long, value_name = "ROW", conflicts_with_all = ["sheet", "tool"])]
    menu: Option<usize>,
    /// Press this meta key (0 to 9, p, x or u: a rating, a color label
    /// or a flag) over the selection once the picture is up, for a
    /// snapshot of what follows it
    #[arg(long, value_name = "KEY", conflicts_with_all = ["sheet", "tool", "menu"])]
    press: Option<char>,
    /// Keys sent to the window once what --sheet, --tool or --menu
    /// asked for is up, a fifth of a second apart, before the
    /// snapshot: names (down, up, left, right, home, end, enter, esc,
    /// tab) or single characters, separated by spaces
    #[arg(long, value_name = "KEYS")]
    keys: Option<String>,
    /// Put this Crop-tab tool in hand once the picture is up, for a
    /// snapshot: crop, level or guide
    #[arg(long, value_name = "NAME", value_parser = panel::viewport::Shown::tool)]
    tool: Option<panel::viewport::Shown>,
    /// Choose this part from the masks panel's People menu (its label,
    /// as Iris or Top) once the picture is up, into the mask
    /// --show-mask chooses on the Masks tab, for a snapshot of what
    /// follows it: the part made, or the people to pick among
    #[arg(long, value_name = "LABEL", value_parser = panel::viewport::Shown::part,
        conflicts_with_all = ["sheet", "tool", "menu"])]
    part: Option<panel::viewport::Shown>,
    /// Paint the capture sharpening's mask over the picture, for a
    /// screenshot
    #[arg(long)]
    sharpen_mask: bool,
    /// The scope the panel opens on: RGB, Wave, Parade or Vector.
    /// Without it, whichever was on last time
    #[arg(long)]
    scope: Option<String>,
    /// Weigh the scopes by the chosen adjustment's mask from the
    /// start (the panel's Selection toggle), for a screenshot with
    /// --show-mask
    #[arg(long)]
    scope_selection: bool,
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
    /// edit of the frame opened (the file given, or in a folder the
    /// one last open there), as a step in its history; with --import,
    /// over every frame imported. Not with --export --all
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
    /// Once the first develop is on screen, move Clarity this many
    /// times, logging the time from each move to its frame, then quit
    /// with the mean
    #[arg(long, value_name = "N", hide = true)]
    time_clarity: Option<u32>,
    /// Once the first develop is on screen, move Dehaze this many
    /// times, logging the time from each move to its frame, then quit
    /// with the mean
    #[arg(long, value_name = "N", hide = true)]
    time_dehaze: Option<u32>,
    /// Once the first develop is on screen, move Texture this many
    /// times with Dehaze held at +50, logging the time from each move
    /// to its frame, then quit with the mean
    #[arg(long, value_name = "N", hide = true)]
    time_texture: Option<u32>,
    /// Once the first develop is on screen, step to the next frame
    /// this many times, a tenth of a second apart, logging the time
    /// from each step to the frame that develops it, then quit with
    /// the mean
    #[arg(long, value_name = "N", hide = true)]
    time_select: Option<u32>,
    /// Once the folder's thumbnails are all in, move the grid (with
    /// --grid) or the strip this many times by --time-scroll-by, one
    /// move a frame and back at either end, logging the time from
    /// each move to the end of the frame that draws it, then quit
    #[arg(long, value_name = "N", hide = true)]
    time_scroll: Option<u32>,
    /// How far each of --time-scroll's moves goes, logical pixels: a
    /// fling's 1,000, or a wheel's notch
    #[arg(long, value_name = "PX", default_value_t = 1000.0, hide = true)]
    time_scroll_by: f32,
    /// Step to the next frame once the first develop is on screen,
    /// and take the snapshot as soon as that frame's camera picture
    /// stands in for its develop, rather than waiting for the develop
    #[arg(long, hide = true)]
    snapshot_placeholder: bool,
    /// Open in culling mode: the camera's JPEG in the viewport, no
    /// develop until Enter
    #[arg(long)]
    cull: bool,
    /// Open the Loupe in Cull with its Developed switch on: each frame
    /// developed, under the CULLING section, with no edit panel
    #[arg(
        long,
        conflicts_with_all = [
            "cull",
            "cull_compare",
            "cull_key",
            "ask_rejects",
            "move_rejects",
            "cull_develop",
            "time_cull",
        ]
    )]
    cull_developed: bool,
    /// Open the culling mode's compare view with this many frames, 2
    /// or 4 (implies --cull)
    #[arg(long, value_name = "N")]
    cull_compare: Option<usize>,
    /// The browser's filter to open with: All, Picks or "No rejects",
    /// or terms of the filter language, one argument, spaces
    /// between them (`'camera:R6 iso>=3200 rating>=3'`). A camera,
    /// lens, style, iso, focal, date or keyword term with `:` or `=`
    /// turns on the chips it names once the folder is indexed, and a
    /// kind term (`kind:raw`) its chip at once; the rest go in the
    /// text field
    #[arg(long, value_name = "WHAT")]
    filter: Option<String>,
    /// Keep the library index in this database rather than the
    /// user's (greycard/library.sqlite under the data directory)
    #[arg(long, value_name = "PATH")]
    library: Option<PathBuf>,
    /// The library's roots for this run, in place of the ones kept in
    /// roots.json beside the library, which are left alone; opens on
    /// the all-roots view. Comma-separated
    #[arg(long, value_name = "DIR,DIR", value_delimiter = ',')]
    roots: Vec<PathBuf>,
    /// Open on the all-roots view: every file under the library's
    /// roots
    #[arg(long)]
    all_roots: bool,
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
    /// In culling mode, press this culling key (a star, P, X, U or a
    /// label key) once the first picture shows, for a snapshot of
    /// the frame's badge and the word for the key (implies --cull)
    #[arg(long, value_name = "KEY", hide = true)]
    cull_key: Option<String>,
    /// Put these browser rows (from 0, comma-separated) in the
    /// selection beside the frame opened, as Ctrl+clicks would, for
    /// a snapshot of a set, `--sheet sync`, or `--export DIR` over
    /// the set
    #[arg(long, value_name = "ROWS", value_delimiter = ',')]
    also: Vec<usize>,
    /// With --export into a folder: export every frame of the folder
    /// opened, in its order, rather than the first file and the --also
    /// rows; the frames are the ones --also could name. The path must
    /// name the folder, and --preset is refused with it
    #[arg(long, requires = "export", conflicts_with = "also")]
    all: bool,
    /// In culling mode, open the move-rejects sheet once the first
    /// picture shows, for a snapshot of it (implies --cull)
    #[arg(long, hide = true)]
    ask_rejects: bool,
    /// As --ask-rejects, and answer the sheet with yes: the rejects
    /// are moved, for a snapshot of the browser after it
    #[arg(long, hide = true)]
    move_rejects: bool,
    /// Import the frames under this folder (a card's DCIM, or any
    /// folder of them) into --to, verified, then quit, with no window.
    /// With --sheet import, the import sheet's source instead
    #[arg(long, value_name = "SRC")]
    import: Option<PathBuf>,
    /// The Lightroom catalog `--sheet lightroom` (and its -report and
    /// -written steps) opens the import's sheet on, as the pane's
    /// button would after the catalog is chosen
    #[arg(long, value_name = "CATALOG")]
    lightroom: Option<PathBuf>,
    /// Where --import copies the frames
    #[arg(long, value_name = "DEST")]
    to: Option<PathBuf>,
    /// A second copy of every file --import copies, verified the same
    /// way
    #[arg(long, value_name = "DIR")]
    backup: Option<PathBuf>,
    /// The names --import gives, a pattern of {date}, {yyyy}, {mm},
    /// {dd}, {name}, {camera}, {seq} and {folder} with any text
    /// between; {name} without it
    #[arg(long, value_name = "PATTERN")]
    name: Option<String>,
    /// The folders under --to that --import makes, a pattern as --name
    /// takes, `/` between levels; {folder}, the frame's folder under
    /// --import, keeps the source's subfolders; none without it
    #[arg(long, value_name = "PATTERN")]
    subfolder: Option<String>,
    /// With --import, compare a frame already in the destination with
    /// the card byte for byte, rather than by its size and its first
    /// and last 64 KiB; reads the whole card again on a re-run
    #[arg(long)]
    verify: bool,
}

/// What the UI thread holds between events.
/// What the download sheet can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fetch {
    Model(&'static greycard_ai::Model),
    /// A Subject model, offered for a Whole person's finer edge rather
    /// than for a Subject shape: declined, the edge offer is not made
    /// again this session, and a Subject shape is still offered its own.
    Edge(&'static greycard_ai::Model),
    /// The lens database.
    Lenses,
    /// Microsoft's CPU-only ONNX Runtime, where the processor cannot
    /// run greycard's own or greycard ships none for the platform: why
    /// the AI features are off (a fetchable
    /// [`greycard_ai::runtime::Unavailable`]), and what was asked for
    /// that it holds up.
    Runtime(
        &'static greycard_ai::runtime::Unavailable,
        panel::assets::AfterRuntime,
    ),
}

/// A change to one frame's sidecar, by its number in the list, made
/// once when the launch's list lands: see `Launch::tweak_at_start`.
pub(crate) type Tweak = Box<dyn FnOnce(&mut State, usize)>;

pub(crate) struct State {
    /// Every file listed, once each, in the list's order. What is a
    /// fact about a file (its sidecar, its row, its seed) is kept by
    /// this number; what the browser shows is a cell (`cells`).
    pub(crate) files: Vec<PathBuf>,
    /// The browser's list: a cell for each file's own frame and one
    /// for each version its sidecar keeps, in display order
    /// (`versions`). `current`, `picked`, `shown`, `menu_file` and the
    /// thumbnail vectors are by cell.
    pub(crate) cells: Vec<versions::Cell>,
    /// The cell on screen.
    pub(crate) current: Option<usize>,
    /// Every file's sidecar: its current edit and history. A frame the
    /// index listed holds a stand-in from its row until something needs
    /// the whole sidecar; see `rows`.
    pub(crate) sidecars: Vec<Sidecar>,
    /// Whether each file's sidecar was read from disk, and what its
    /// row stands in for while it was not (`rows::FromRow`).
    pub(crate) from_row: Vec<rows::FromRow>,
    /// The camera match's sheet: its scope, what the survey found and
    /// a run under way.
    pub(crate) camera_match: panel::camera_match::Sheet,
    /// The import sheet's folders and what the source holds, and the
    /// import running.
    pub(crate) import: panel::import::Sheet,
    /// The Lightroom catalog import's sheet.
    pub(crate) lightroom: panel::lightroom::Sheet,
    /// The RawTherapee profiles' offer and their import's sheet.
    pub(crate) rawtherapee: panel::rawtherapee::Sheet,
    /// The archive roots: the sheet's question, a copy under way, the
    /// header's count (`panel::archive`).
    pub(crate) archive: panel::archive::Archive,
    /// What the command line asked of this run: see [`owners::Launch`].
    pub(crate) launch: owners::Launch,
    /// The window and the session's policies: see [`owners::Window`].
    pub(crate) window: owners::Window,
    /// The sheets without a struct of their own: see [`owners::Sheets`].
    pub(crate) sheets: owners::Sheets,
    /// The exports and the archive copies' writes: see [`owners::Work`].
    pub(crate) work: owners::Work,
    /// Culling, the lightbox and the camera's pictures: see [`owners::Culling`].
    pub(crate) culling: owners::Culling,
    /// The library views: see [`owners::Views`].
    pub(crate) views: owners::Views,
    /// The open frame and its develop: see [`owners::Develop`].
    pub(crate) develop: owners::Develop,
}

impl State {
    /// What is known of the mosaic of the file the panel is for.
    pub(crate) fn open_mosaic(&self) -> crate::panel::browser::OpenMosaic {
        self.develop
            .panel_mosaic
            .as_ref()
            .map_or(crate::panel::browser::OpenMosaic::Unknown, |(_, m)| *m)
    }

    /// A blank state for `files`, on `app`'s window: no picture
    /// chosen, no sidecar read, nothing running or remembered. The
    /// one place every field is named. The launch builds from it and
    /// assigns what the command line and the settings choose, and a
    /// test builds from it and assigns only the handful a callback
    /// under test reads. Add a field once, here, and both stay whole.
    pub(crate) fn empty(files: Vec<PathBuf>, app: &App) -> Self {
        let count = files.len();
        Self {
            cells: (0..count)
                .map(|file| versions::Cell {
                    file,
                    version: None,
                })
                .collect(),
            files,
            current: None,
            sidecars: vec![Sidecar::default(); count],
            from_row: vec![rows::FromRow::read(); count],
            camera_match: Default::default(),
            import: panel::import::Sheet::default(),
            lightroom: panel::lightroom::Sheet::default(),
            rawtherapee: panel::rawtherapee::Sheet::default(),
            archive: panel::archive::Archive::default(),
            launch: owners::Launch {
                tweak_at_start: None,
                time_sharpen: None,
                time_clarity: None,
                time_dehaze: None,
                time_texture: None,
                time_cull: None,
                time_select: None,
                time_scroll: None,
                cull_develop: false,
                snapshot_placeholder: false,
                turn_at_start: None,
                awaiting_turn: false,
                cull_key_at_start: None,
                ask_rejects: false,
                move_rejects: false,
                cull_at_start: None,
                developed_at_start: false,
                scripted: false,
                facets_wanted: Vec::new(),
                awaiting_index: false,
                select_at_start: None,
                overrides_at_start: None,
                overridden: None,
                setup_ran: false,
                also_at_start: Vec::new(),
                preview_temperature: None,
                show_mask: None,
                show_patch: None,
                opened: false,
                zoom_stands: false,
                grid_wait_over: false,
                screenshot: None,
                snapshot: None,
                panel_scroll: None,
                snapshot_shown: None,
                failed: false,
            },
            window: owners::Window {
                write_sidecars: false,
                xmp_sidecars: false,
                placement: greycard_edit::Placement::Beside,
                deletes_allowed: false,
                settings_file: None,
                batch: false,
                app: app.as_weak(),
            },
            sheets: owners::Sheets {
                delete_asked: None,
                version_delete: None,
                trash_refused: std::collections::HashSet::new(),
                look_remove: None,
                look_remove_generation: 0,
                look_removing: false,
                look_rename: None,
                look_rename_generation: 0,
                look_renaming: None,
                time_shift: Default::default(),
                deleting: None,
                fetch: None,
                fetching: false,
                runtime_next: None,
                declined: Vec::new(),
                remove_offered: false,
                edge_declined: false,
                edge_offer_due: false,
                edge_loaded: false,
                fetch_failed: Vec::new(),
                lenses_declined: false,
                lenses_asked: false,
                preset_checks: carry::Checks::new(Vec::new(), |_| false),
                preset_rows: carry::Shown::default(),
                sync_checks: carry::Checks::new(Vec::new(), |_| false),
                sync_rows: carry::Shown::default(),
                sync_asked: None,
                paste_asked: None,
                sync_last: None,
                sync_source: None,
                clipboard: None,
            },
            work: owners::Work {
                exporting: None,
                export_choosing: false,
                export_checks: 0,
                export_looking: None,
                export_one_line: None,
                export_pending: None,
                export_queue: Vec::new(),
                export_queue_file: None,
                queue_run: None,
                queue_probes: 0,
                queue_busy: None,
                export_presets: Vec::new(),
                export_last_folder: None,
                sync: sync::Sync::default(),
            },
            culling: owners::Culling {
                cull: None,
                cull_move_on: false,
                loupe_cull: false,
                show_developed: false,
                developed: None,
                notice_timer: slint::Timer::default(),
                tags: tags::History::default(),
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
                lightbox: None,
                lightbox_fetch: None,
                compare_tiles: Rc::new(VecModel::default()),
            },
            views: owners::Views {
                companions: HashMap::new(),
                jpegs_apart: false,
                collection_frames: None,
                queue_listed: None,
                grid_heads: Vec::new(),
                queued_open: None,
                queue_relist_due: false,
                queue_cell_ids: Vec::new(),
                cells_by_file: None,
                stacks_listed: Vec::new(),
                stacks: stacks::Store::default(),
                picked: Vec::new(),
                shown: (0..count).collect(),
                pictured: [(0, 0); 2],
                filter: filter::Filter::default(),
                index: None,
                index_reader: None,
                index_ids: vec![None; count],
                index_passed: vec![true; count],
                index_generation: 0,
                index_progress: None,
                index_path: None,
                index_error: None,
                index_tries: 0,
                index_pass_ready: false,
                facets_last: RefCell::new(Vec::new()),
                library: roots::Library::default(),
                collections: collections::Store::default(),
                recent: panel::recent::Recent::default(),
                tree: tree::Tree::default(),
                view: roots::View::Folder,
                sort: sort::Kept::default(),
                sort_reveal: false,
                view_generation: 0,
                loads: rows::Loads::default(),
                pick_pending: None,
                panel_stand_in: None,
                thumb_base: vec![None; count],
                thumb_shown: vec![None; count],
                thumb_made: vec![0; count],
                thumb_failed: vec![false; count],
                thumb_asked: vec![worker::THUMB_WIDTH; count],
                thumb_want: worker::THUMB_WIDTH,
                thumb_run: None,
                fill_clock: None,
                selected_at: None,
                grid_shown: None,
                strip_shown: None,
                menu_file: None,
                words: Default::default(),
            },
            develop: owners::Develop {
                edit: Edit::default(),
                pending: None,
                base_white: None,
                frame: None,
                panel_mosaic: None,
                mosaics: HashMap::new(),
                drawn_kind: finish::Source::SCENE,
                as_shot_white: None,
                white_cache: None,
                shown_white: None,
                debounce: slint::Timer::default(),
                save_timer: slint::Timer::default(),
                image_size: (1, 1),
                zoom: 0.0,
                mat: false,
                center: (0.0, 0.0),
                renderer: None,
                gpu: None,
                software_tail: None,
                bins: None,
                scope: scope::Scope::default(),
                curve_drag: None,
                hover: None,
                hover_shown: None,
                // Nothing until a develop says so: a turn pressed
                // before one lands must not measure the frame by the
                // last frame's shape, or by a square.
                source_size: (0, 0),
                shown_size: (0, 0),
                shown_turn: None,
                turn_pressed: None,
                crop_drag: None,
                crop_draw: None,
                crop_start: Default::default(),
                crop_last: "Free".into(),
                crop_start_due: false,
                target: None,
                fresh_mask: None,
                placing: None,
                choosing: None,
                part_pending: None,
                landscape_pending: None,
                landscape_offer: None,
                part_asks: HashMap::new(),
                part_dismissed: Default::default(),
                people_all: Default::default(),
                picking: None,
                mask_drag: None,
                rasters: HashMap::new(),
                retouching: None,
                patch_stroke: None,
                fill_engines: HashMap::new(),
                patch_drag: None,
                patch_handles: Rc::new(VecModel::default()),
                patch_shape: None,
                guiding: Guiding::default(),
                guide_kept: Rc::new(VecModel::default()),
                stances: HashMap::new(),
                learned: HashMap::new(),
                asked: HashMap::new(),
                masks_failed: HashMap::new(),
                learned_of: None,
                store: None,
                lens: LensReport::NoDatabase,
                profiles: Vec::new(),
                camera: (String::new(), String::new()),
                other_names: Vec::new(),
                looks: Vec::new(),
                look: None,
                look_for: None,
                look_groups_open: Default::default(),
                look_hover: Default::default(),
                white_profile: None,
                mask_handles: Rc::new(VecModel::default()),
                mask_boxes: Rc::new(VecModel::default()),
                mask_picks: Rc::new(VecModel::default()),
                person_boxes: Rc::new(VecModel::default()),
                monitors: Vec::new(),
                lut_for: None,
                encoded_lut_for: None,
                presets: Vec::new(),
                preset_store: None,
                status_after_develop: None,
                peek: None,
                held: None,
                status_kept: None,
                view_kept: None,
                show_mask_kept: None,
                look_seen: RefCell::new(None),
                planes_wants: Default::default(),
                planes_asked: Default::default(),
                generation: 0,
                neighbors_sent: None,
            },
        }
    }
}

/// A `--root` as typed: the catalog's side up to the first `=`, the
/// folder here after it. The `greycard` command parses its own the
/// same way; the two binaries share no crate for it.
fn root_mapping(text: &str) -> Result<(String, PathBuf), String> {
    match text.split_once('=') {
        Some((from, to)) if !from.trim().is_empty() && !to.trim().is_empty() => {
            Ok((from.trim().to_string(), PathBuf::from(to.trim())))
        }
        _ => Err(format!(
            "want <catalog path>=<folder here>, as A:/Photos=/photos, not {text}"
        )),
    }
}

/// A policy named on the command line.
fn on_exists_named(name: &str) -> Result<export::OnExists, String> {
    export::OnExists::from_name(name)
        .ok_or_else(|| format!("want increment, overwrite or skip, not {name}"))
}

pub(crate) fn install_callbacks(app: &App, state: Rc<RefCell<State>>, worker: Rc<Worker>) {
    entry::install(app);
    keys::install(app);
    keys::install_sheet(app, &state);
    panel::cull::install(app, &state, &worker);
    panel::delete::install(app, &state, &worker);
    panel::look_remove::install(app, &state);
    panel::look_rename::install(app, &state);
    panel::time_shift::install(app, &state, &worker);
    panel::look_preview::install(app, &state);
    panel::archive::install(app, &state, &worker);
    sync::install(app, &state);
    panel::mask::install(app, &state, &worker);
    panel::parts::install(app, &state);
    panel::parts_all::install(app, &state, &worker);
    panel::landscape::install(app, &state);
    panel::browser::install(app, &state, &worker);
    panel::words::install(app, &state, &worker);
    panel::color::install(app, &state, &worker);
    panel::viewport::install(app, &state, &worker);
    panel::deliver::install(app, &state, &worker);
    panel::export_queue::install(app, &state, &worker);
    panel::queue_view::install(app, &state, &worker);
    panel::edit::install(app, &state, &worker);
    panel::assets::install(app, &state, &worker);
    panel::crop::install(app, &state, &worker);
    panel::curve::install(app, &state, &worker);
    panel::history::install(app, &state, &worker);
    panel::retouch::install(app, &state, &worker);
    panel::prefs::install(app, &state, &worker);
    panel::sync::install(app, &state, &worker);
    panel::menu::install(app, &state, &worker);
    panel::versions::install(app, &state, &worker);
    panel::import::install(app, &state, &worker);
    panel::lightroom::install(app, &state);
    panel::rawtherapee::install(app, &state);
    panel::recent::install(app, &state, &worker);
    panel::camera_match::install(app, &state);
    roots::install(app, &state, &worker);
    panel::collections::install(app, &state, &worker);
    panel::stacks::install(app, &state, &worker);
    panel::sort::install(app, &state, &worker);
    lightbox::install(app, &state, &worker);
    tree::install(app, &state, &worker);
    report::install(app, &state);
    update::install(app, &state);

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

/// `WxH` in logical pixels, each 200 to 16384.
fn parse_window_size(s: &str) -> Result<(f32, f32), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| "expected WxH, such as 1500x950".to_string())?;
    let read = |p: &str| -> Result<f32, String> {
        let v: f32 = p
            .trim()
            .parse()
            .map_err(|_| format!("{p:?} is not a number"))?;
        if !(200.0..=16384.0).contains(&v) {
            return Err(format!("{v} is outside 200 to 16384"));
        }
        Ok(v)
    };
    Ok((read(w)?, read(h)?))
}
