//! The reduced develop: the pictures of an edit made for a frame the
//! window never developed (a sync, a preset or a paste over the
//! selection, a save joined from another machine's copy, a folder
//! opened cold with its edits on disk).
//!
//! The thumbnails' pool runs it behind the camera's thumbnails and the
//! local previews, one or two at a time, never while the window
//! develops: a 45 MP decode is several hundred megabytes, and the
//! window's develop must not wait on it. The frame is decoded, the
//! mosaic prepared in full and binned 2x2 into a linear picture at half
//! the size in place of the demosaic
//! (`greycard_core::develop::reduced`) while the frame, as the edit's
//! crop and straighten leave it, keeps twice 2048 on its long edge; under
//! that the mosaic is demosaiced as in a full develop ([`bin_under`]), so
//! the 2048 picture is never brought up from fewer pixels. An X-Trans
//! mosaic is binned 3x3 to a third, under the same rule at three times
//! 2048. Then the
//! pipeline as it is on that picture: the lens, the defringe, the retouch, the local
//! contrast, the dehaze, the guide plane; then the finish exactly as the
//! open frame's pictures are finished (`edited::make`: the geometry, a
//! resize to 2048, `export::render_framed` with no output sharpening,
//! sRGB), and the made sizes, kept under the edit's key like the open
//! frame's.
//!
//! Left out, since they are measured on the mosaic's own pixels and mean
//! nothing at half size: the capture sharpen, the learned denoiser and
//! the profiled one. So these pictures are kept under a recipe of their
//! own (`edited::REDUCED_RECIPE`), and the open frame's develop makes
//! its own in their place when the frame is opened. The grain stays: it
//! is laid on by the finish at 2048, placed by the output's pixels, on
//! both develops alike. Every op measured in pixels is
//! measured on the half-size picture: Texture's and Clarity's radii and
//! the dehaze's window are fractions of the long edge already, the
//! guide plane is made at its fixed 1024 texels, the masks and the
//! vignette are placed by the frame in `render_framed`, the retouch's
//! patches are in units of the picture's width, and the defringe's
//! radius, the one in pixels, is halved. The CA correction runs on the
//! mosaic in full before the bin, where its radius means what it did.
//!
//! A picture made without something its edit asks for is not kept, as
//! the open frame's is not: a Fill patch (its model runs on the open
//! frame alone), a look not in the look directory, and a learned mask
//! whose raster is not in the mask cache. The look and the masks are
//! asked after before anything is decoded. A mask whose model is not in
//! the store waits for a fetch of one; one whose model is there is asked
//! of the worker, at the lowest priority it has (`Job::MasksFor`), and
//! the frame is queued again when every raster is made. A frame tried
//! and found wanting is not tried again under the same key until then,
//! so nothing is decoded over and over for a picture that cannot be
//! kept; until then the frame shows the camera's JPEG.
//!
//! The rasters the worker makes for this go into the mask cache the
//! open frame's masks are read from: made from the reduced develop's
//! picture, which is what the models see at their own 2048 anyway, they
//! serve the frame when it is opened too.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use greycard_core::develop::DevelopSettings;
use greycard_core::develop::local_contrast::local_contrast;
use greycard_core::develop::{defringe, dehaze, reduced};
use greycard_core::image::WorkingImage;
use greycard_edit::Edit;
use greycard_edit::brush::Raster;

use crate::ai::Ai;
use crate::edited::{Edited, Shows};
use crate::versions::Key;

/// How many reduced develops the pool runs at once: one while a frame
/// is open in the editor, whose develop is what the user is waiting on
/// (two in hand cost it about a third again, one about a twentieth);
/// two while culling or browsing on a machine with four or more
/// thumbnail threads, else one. Each holds a decoded frame (a 45 MP
/// mosaic is 180 MB as floats, and its prepared copy as much again)
/// and runs on the [`Reducer::threads`] the develops share.
pub(crate) fn at_once(threads: usize, editing: bool) -> usize {
    if editing || threads < 4 { 1 } else { 2 }
}

/// How long the editor is left alone (no develop asked for or under
/// way, so no slider in motion either) before a reduced develop begins
/// while a frame is open in it.
pub(crate) const IDLE: std::time::Duration = std::time::Duration::from_millis(1500);

/// How many reduced develops may be in hand now, and when to look again
/// if that is none only for want of quiet: while a frame is open in the
/// editor, none until the window has been idle for [`IDLE`] (`idle`, the
/// time since its last develop was asked for or done), then one;
/// otherwise as [`at_once`] says.
pub(crate) fn begin(
    threads: usize,
    editing: bool,
    idle: std::time::Duration,
) -> (usize, Option<std::time::Duration>) {
    if editing && idle < IDLE {
        return (0, Some(IDLE - idle));
    }
    (at_once(threads, editing), None)
}

/// Why a frame's pictures were not made by the reduced develop.
#[derive(Debug)]
pub(crate) enum Why {
    /// The frame is the open one: the worker keeps its pictures.
    Open,
    /// The frame shows no edit, or its pictures are kept already.
    NotWanted,
    /// The frame has no cache to keep pictures in.
    NoCache,
    /// The develop failed: the decoder's or the engine's words.
    Failed(String),
    /// The picture would be without something the edit asks for, and
    /// so is not kept (the open frame's rule, `edited`).
    Without(Vec<String>),
    /// Learned masks the edit draws are not in the mask cache: the
    /// worker is to make them (`AskMasks`, the first time under a key),
    /// and the frame is queued again after.
    Masks(Vec<String>),
    AskMasks(Vec<String>),
    /// Learned masks the edit draws whose models are not in the store:
    /// tried again when a model is fetched.
    NoModel(Vec<String>),
    /// The frame's edit changed while it was being developed: the new
    /// key's pictures are the ones wanted.
    Stale,
}

impl std::fmt::Display for Why {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Why::Open => write!(f, "the open frame"),
            Why::NotWanted => write!(f, "nothing wanted"),
            Why::NoCache => write!(f, "no cache to keep it in"),
            Why::Failed(e) => write!(f, "{e}"),
            Why::Without(w) => write!(f, "not kept: {}", w.join("; ")),
            Why::Masks(m) | Why::AskMasks(m) => {
                write!(f, "learned masks to make first: {}", m.join(", "))
            }
            Why::NoModel(m) => write!(f, "not kept until a model is fetched: {}", m.join("; ")),
            Why::Stale => write!(f, "the edit changed meanwhile"),
        }
    }
}

/// A frame developed reduced and ready for the finish.
pub(crate) struct Developed {
    /// The picture after everything up to the finish, at half the
    /// frame's size (or in full for a frame too small to bin).
    pub(crate) image: WorkingImage,
    /// The full develop's size, oriented: what the learned masks'
    /// rasters are checked by.
    pub(crate) full: (u32, u32),
    pub(crate) bin: usize,
    pub(crate) clip_level: f32,
    pub(crate) guide: Arc<crate::finish::Guide>,
    pub(crate) kind: crate::finish::Source,
    pub(crate) white: Option<greycard_edit::WhiteShift>,
    /// The masks' bands, of `image`.
    pub(crate) detail: Option<Arc<crate::detail::DetailPlanes>>,
    /// How long each step took, seconds: the decode, the develop to
    /// the binned picture, the rest to the finish.
    pub(crate) decode: f64,
    pub(crate) develop: f64,
    pub(crate) rest: f64,
}

/// The learned masks' rasters by adjustment id and component.
pub(crate) type Rasters = std::collections::HashMap<(u64, usize), Arc<Raster>>;

/// The engine's settings for the reduced develop of `edit`: the edit's
/// own, less what the reduced develop leaves out (the sharpen, the
/// denoisers) and what runs after it here (the dehaze), with the
/// frame's quarter turns on the camera's tag as the window has them.
/// The grain is the finish's, and stays.
pub(crate) fn settings(
    edit: &Edit,
    camera: greycard_core::raw::Orientation,
    turn: u8,
) -> DevelopSettings {
    DevelopSettings {
        sharpen: None,
        dehaze: None,
        denoise: None,
        orientation: Some(camera.turned(i32::from(turn))),
        ..edit.settings()
    }
}

/// The bin for a frame whose full develop is `full` (oriented) under
/// `geometry`, binned by `cell` ([`reduced::cell_of`] its pattern: 2,
/// or 3 for X-Trans): `cell` only while the framed picture keeps at
/// least `cell` times [`crate::edited::BIG`] on its long edge, so the
/// 2048 picture is always brought down to and never up; 1 (the
/// demosaic, a full develop of the frame) under a tighter crop or a
/// straighten that takes it there. A 24 MP Bayer frame is binned down
/// to about two thirds of its width.
pub(crate) fn bin_under(
    full: (usize, usize),
    cell: usize,
    geometry: &greycard_edit::geometry::Geometry,
) -> usize {
    let (w, h) = crate::edited::framed_size((full.0 as u32, full.1 as u32), geometry);
    reduced::bin_for_cell(cell, w as usize, h as usize)
}

/// What the frame at `path` is: a raw decoded, or a picture file.
enum Opened {
    Raw(Box<greycard_core::raw::RawFrame>),
    Picture(Box<greycard_core::picture::Picture>),
}

fn open(path: &Path) -> anyhow::Result<Opened> {
    if greycard_core::picture::is_picture_path(path) {
        return Ok(Opened::Picture(Box::new(
            greycard_core::picture::decode_picture_path(path)?,
        )));
    }
    Ok(Opened::Raw(Box::new(greycard_core::decode::decode_path(
        path,
    )?)))
}

/// The frame at `path` developed reduced under `edit` at `turn`, up to
/// the finish; the learned masks are the caller's ([`rasters`]).
pub(crate) fn develop(
    path: &Path,
    edit: &Edit,
    turn: u8,
    lenses: Option<&greycard_lens::Database>,
) -> Result<Developed, Why> {
    let started = Instant::now();
    let opened = open(path).map_err(|e| Why::Failed(format!("{e:#}")))?;
    let decode = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let (mut image, full, bin, clip_level, kind, white, identity) = match &opened {
        Opened::Raw(frame) => {
            let settings = settings(edit, frame.orientation, turn);
            // The full develop's size, oriented, for the bin: the
            // mosaic's crop turned as the settings turn it.
            let crop = frame.effective_crop();
            let turned = settings.orientation.unwrap_or(frame.orientation);
            let full = if matches!(
                turned,
                greycard_core::raw::Orientation::Transpose
                    | greycard_core::raw::Orientation::Rotate90
                    | greycard_core::raw::Orientation::Transverse
                    | greycard_core::raw::Orientation::Rotate270
            ) {
                (crop.height, crop.width)
            } else {
                (crop.width, crop.height)
            };
            let cell = match &frame.layout {
                greycard_core::raw::SensorLayout::Cfa(pattern) => reduced::cell_of(pattern),
                _ => reduced::BIN,
            };
            let bin = bin_under(full, cell, &edit.geometry);
            let r = reduced::develop_reduced_by(frame, &settings, None, bin)
                .map_err(|e| Why::Failed(e.to_string()))?;
            let d = r.developed;
            // The profile the white was resolved through, as the worker
            // has it: the edit's DCP, else the file's own.
            let profile = settings
                .profile
                .as_ref()
                .map(|p| p.camera.clone())
                .or_else(|| greycard_core::color::profile_from_frame(frame).ok());
            let white = profile.map(|profile| {
                let cols = d.white_balance.matrix_f32();
                let matrix: [[f32; 3]; 3] =
                    std::array::from_fn(|r| std::array::from_fn(|c| cols[c][r]));
                greycard_edit::WhiteShift::from_parts(
                    profile,
                    d.white_balance.coefficients_f32(),
                    matrix,
                )
            });
            (
                d.image,
                (r.full.0 as u32, r.full.1 as u32),
                r.bin,
                d.clip_level,
                crate::finish::Source::Scene,
                white,
                (frame.make.clone(), frame.model.clone(), frame.shot.clone()),
            )
        }
        Opened::Picture(picture) => {
            // A picture is its own base; a turn is applied to it as the
            // worker applies it, and a large one is reduced by a whole
            // factor as the open frame's picture is.
            let turned = match crate::worker::quarters(turn) {
                Some(o) => greycard_core::develop::orient(picture.image.clone(), o),
                None => picture.image.clone(),
            };
            let full = (turned.width as u32, turned.height as u32);
            let bin = bin_under((turned.width, turned.height), reduced::BIN, &edit.geometry);
            let image = if bin > 1 {
                crate::edited::reduce(&turned, bin)
            } else {
                turned
            };
            (
                image,
                full,
                bin,
                picture.clip_level(),
                crate::finish::Source::Display,
                None,
                (
                    picture.make.clone(),
                    picture.model.clone(),
                    picture.shot.clone(),
                ),
            )
        }
    };
    let develop = started.elapsed().as_secs_f64();
    let started = Instant::now();
    // The lens and the defringe, as `develop_job` has them after the
    // base; the defringe's radius is in pixels and so is halved.
    let (make, model, shot) = &identity;
    if !edit.lens.is_identity(lenses.is_some()) {
        image = crate::worker::correct_lens_of(image, make, model, shot, edit, lenses);
    }
    if let Some(mut options) = edit.lens.defringe() {
        options.radius /= bin as f32;
        defringe::defringe(&mut image, &options);
    }
    // The guide plane, before the retouch and the local contrast, as
    // the worker makes it: in the developed picture's own coordinates.
    let guide = Arc::new(crate::finish::guide_plane(&image));
    // The retouch: its patches are in units of the picture's width. A
    // Fill is the model's and runs on the open frame alone; a picture
    // without one is not the edit's.
    let mut without = Vec::new();
    if !edit.retouch.is_empty() {
        let chosen = edit.retouch.choose_sources(&image);
        let retouch = edit.retouch.with_sources(&chosen);
        retouch.apply_with(&mut image, |patch, _, _| {
            without.push(format!("the fill {}", patch.name()));
            None
        });
    }
    // The masks' bands, of the picture at this size, as the worker
    // makes them of the full one.
    let detail = crate::detail::DetailPlanes::make(
        &image,
        edit.detail.options(),
        clip_level,
        edit.detail_wants(),
    )
    .map(Arc::new);
    if let Some(options) = edit.detail.options() {
        local_contrast(&mut image, &options, clip_level);
    }
    if let Some(options) = edit.detail.dehaze_options() {
        dehaze::dehaze(&mut image, &options);
    }
    if !without.is_empty() {
        return Err(Why::Without(without));
    }
    Ok(Developed {
        image,
        full,
        bin,
        clip_level,
        guide,
        kind,
        white,
        detail,
        decode,
        develop,
        rest: started.elapsed().as_secs_f64(),
    })
}

/// Whether the look the edit names is in the look directory: asked
/// before anything is decoded, since a picture without it is not kept.
pub(crate) fn look_ready(edit: &Edit) -> Result<(), Why> {
    if let greycard_edit::look::LutChoice::Named(name) = &edit.look_lut.lut
        && edit.look_lut.strength > 0.0
        && edit.look_lut.look_under(edit.display_curve).is_none()
    {
        return Err(Why::Without(vec![format!("the look {name}")]));
    }
    Ok(())
}

/// Whether every learned mask `edit` draws on the frame at `path` can be
/// had, asked before anything is decoded: `Why::NoModel` names the ones
/// whose model is not in the store, `Why::Masks` the ones the worker is
/// to make first.
pub(crate) fn masks_ready(ai: &mut Ai, path: &Path, edit: &Edit, turn: u8) -> Result<(), Why> {
    let mut to_make = Vec::new();
    let mut no_model = Vec::new();
    for a in &edit.adjustments {
        if !a.enabled || a.mask.is_empty() {
            continue;
        }
        for (_, c) in a.mask.live() {
            if !c.shape.is_learned() || !crate::ai::prompted(&c.shape) {
                continue;
            }
            match ai.raster_ready(path, &c.shape, turn) {
                crate::ai::Ready::Cached => {}
                crate::ai::Ready::ToMake => {
                    to_make.push(format!("{}'s {}", a.name, c.shape.name()))
                }
                crate::ai::Ready::NoModel(model) => no_model.push(format!(
                    "{}'s {}: the {model} model is not downloaded",
                    a.name,
                    c.shape.name()
                )),
            }
        }
    }
    if !no_model.is_empty() {
        return Err(Why::NoModel(no_model));
    }
    if !to_make.is_empty() {
        return Err(Why::Masks(to_make));
    }
    Ok(())
}

/// The learned masks `edit` draws, from the mask cache alone, for the
/// frame at `path` whose full develop is `full`. `Err` names the
/// shapes whose rasters are not cached at that aspect.
pub(crate) fn rasters(
    ai: &mut Ai,
    path: &Path,
    edit: &Edit,
    turn: u8,
    full: (u32, u32),
) -> Result<Rasters, Why> {
    let mut rasters = Rasters::new();
    let mut missing = Vec::new();
    let aspect = full.1 as f32 / full.0 as f32;
    for a in &edit.adjustments {
        if !a.enabled || a.mask.is_empty() {
            continue;
        }
        for (i, c) in a.mask.live() {
            if !c.shape.is_learned() || !crate::ai::prompted(&c.shape) {
                continue;
            }
            match ai.cached_raster(path, &c.shape, turn, aspect) {
                Some(raster) => {
                    rasters.insert((a.id, i), raster);
                }
                None => missing.push(format!("{}'s {}", a.name, c.shape.name())),
            }
        }
    }
    if !missing.is_empty() {
        return Err(Why::Masks(missing));
    }
    Ok(rasters)
}

/// Whether the edit draws a learned mask at all: what decides whether a
/// frame's pictures can wait on the worker's masks.
pub(crate) fn draws_learned(edit: &Edit) -> bool {
    edit.adjustments.iter().any(|a| {
        a.enabled
            && a.mask
                .live()
                .any(|(_, c)| c.shape.is_learned() && crate::ai::prompted(&c.shape))
    })
}

/// What the pool's reduced develops share: the lens database, the
/// reader of the mask cache, the threads the develops run on, what was
/// tried and found wanting, and the roots on a network mount.
pub(crate) struct Reducer {
    /// The lens database as the worker has it; `None` while the worker
    /// has not said, then loaded here once.
    lenses: Mutex<Option<Option<Arc<greycard_lens::Database>>>>,
    /// Reads the mask cache; loads no model.
    ai: Mutex<Ai>,
    /// The develops' threads, made on the first develop and not before:
    /// every worker has a reducer, and a test suite makes a worker a
    /// test, so a pool built at once is two to eight threads a test
    /// that nothing joins, and macOS runs out of threads a thousand
    /// tests in.
    pool: OnceLock<rayon::ThreadPool>,
    /// The frames whose learned masks the worker was asked for and has
    /// not made yet, with the key they were asked under, so the ask is
    /// made once. A frame is a file and a version of it, whose line's
    /// edit is what is made.
    asked: Mutex<std::collections::HashMap<Key, u64>>,
    /// The frames, by frame and key, whose masks the worker said it made
    /// every one of: still missing after that (a raster that will not
    /// read back), the frame is not asked for again.
    masks_made: Mutex<std::collections::HashSet<(Key, u64)>>,
    /// Frames, by frame and key, whose making ended without a picture
    /// kept (a fill, a look not there, masks still to make or whose
    /// model is not there, a failed develop): not tried again under
    /// that key, or every scroll past the cell would develop the frame
    /// again for nothing. A new key is a new try; the worker making
    /// every mask, or a model fetched, lets the frame go.
    tried: Mutex<std::collections::HashSet<(Key, u64)>>,
    /// The frames waiting on a model: their masks' models are not in
    /// the store, or the worker could not make every raster. Tried again
    /// when a model is fetched.
    waiting: Mutex<std::collections::HashSet<Key>>,
    /// The roots on a network mount: a frame under one is developed
    /// alone, since its read is the slow part and two at once only
    /// share the link.
    network: Mutex<Vec<PathBuf>>,
    /// How many develops were begun, for the tests and the log.
    develops: AtomicUsize,
}

impl Reducer {
    /// How many threads the reduced develops run on: a quarter of the
    /// machine's, two at least and eight at most, one pool the develops
    /// in hand share, so the window's develop, which has the thumbnails'
    /// pool held while it runs, finds the rest free.
    pub(crate) fn threads() -> usize {
        std::env::var("GREYCARD_REDUCED_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or_else(|| {
                (std::thread::available_parallelism().map_or(2, |n| n.get()) / 4).clamp(2, 8)
            })
    }

    pub(crate) fn new() -> Self {
        Self::with_ai(Ai::new())
    }

    /// A reducer reading the mask cache and the store through `ai`.
    pub(crate) fn with_ai(ai: Ai) -> Self {
        Self {
            lenses: Mutex::new(None),
            ai: Mutex::new(ai),
            pool: OnceLock::new(),
            asked: Mutex::default(),
            masks_made: Mutex::default(),
            tried: Mutex::default(),
            waiting: Mutex::default(),
            network: Mutex::default(),
            develops: AtomicUsize::new(0),
        }
    }

    /// The develops' pool, built on the first call.
    fn pool(&self) -> &rayon::ThreadPool {
        self.pool.get_or_init(|| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(Self::threads())
                .thread_name(|n| format!("greycard reduced develop {n}"))
                .build()
                .expect("the reduced develops' threads")
        })
    }

    /// Whether `frame` wants the reduced develop of its pictures
    /// (`Edited::wants_develop`) under a key not tried and found wanting
    /// already: the pool's ask after each thumbnail.
    pub(crate) fn wants(&self, edited: &Edited, frame: &Key) -> bool {
        edited.wants_develop(frame).is_some_and(|key| {
            !self
                .tried
                .lock()
                .expect("tried keys")
                .contains(&(frame.clone(), key))
        })
    }

    /// The lens database the develops correct with: the worker's, or
    /// one loaded here the first time it is asked for.
    pub(crate) fn lenses(&self) -> Option<Arc<greycard_lens::Database>> {
        let mut held = self.lenses.lock().expect("reduced lenses");
        held.get_or_insert_with(|| {
            greycard_lens::Store::user()
                .ok()
                .and_then(|s| s.load())
                .map(|(db, _)| Arc::new(db))
        })
        .clone()
    }

    /// The worker's lens database, loaded or fetched: shared here so it
    /// is not loaded twice.
    pub(crate) fn set_lenses(&self, lenses: Option<Arc<greycard_lens::Database>>) {
        *self.lenses.lock().expect("reduced lenses") = Some(lenses);
    }

    /// The roots on a network mount, as the window has them now.
    pub(crate) fn set_network(&self, roots: Vec<PathBuf>) {
        *self.network.lock().expect("network roots") = roots;
    }

    /// Whether `frame` is developed alone: its file is under a root on
    /// a network mount.
    pub(crate) fn alone(&self, frame: &Key) -> bool {
        self.network
            .lock()
            .expect("network roots")
            .iter()
            .any(|root| frame.path.starts_with(root))
    }

    /// How many reduced develops were begun.
    #[cfg(test)]
    pub(crate) fn develops(&self) -> usize {
        self.develops.load(Ordering::SeqCst)
    }

    /// The reader of the mask cache, for a test that puts a raster in
    /// it as the worker would.
    #[cfg(test)]
    fn ai(&self) -> std::sync::MutexGuard<'_, Ai> {
        self.ai.lock().expect("reduced ai")
    }

    /// `frame`'s masks under `key` are to be asked of the worker; whether
    /// this is the first ask since they were last made.
    fn ask_masks(&self, frame: &Key, key: u64) -> bool {
        let mut asked = self.asked.lock().expect("asked masks");
        if asked.contains_key(frame) {
            return false;
        }
        asked.insert(frame.clone(), key);
        true
    }

    /// The worker made `frame`'s masks: every one (`all`), when the next
    /// try of the frame is a making, or not every one, when the frame
    /// stays tried under its key and waits for a model to be fetched.
    pub(crate) fn masks_done(&self, frame: &Key, all: bool) {
        let key = self.asked.lock().expect("asked masks").remove(frame);
        if all {
            if let Some(key) = key {
                self.masks_made
                    .lock()
                    .expect("masks made")
                    .insert((frame.clone(), key));
            }
            self.tried
                .lock()
                .expect("tried keys")
                .retain(|(p, _)| p != frame);
            self.waiting.lock().expect("waiting").remove(frame);
        } else {
            self.waiting.lock().expect("waiting").insert(frame.clone());
        }
    }

    /// A model was fetched: every frame waiting on one is let go, to be
    /// tried again when it is next asked for. Answers them, for their
    /// cells to ask.
    pub(crate) fn models_arrived(&self) -> Vec<Key> {
        let waiting: Vec<Key> = self.waiting.lock().expect("waiting").drain().collect();
        if !waiting.is_empty() {
            self.tried
                .lock()
                .expect("tried keys")
                .retain(|(p, _)| !waiting.contains(p));
            self.masks_made
                .lock()
                .expect("masks made")
                .retain(|(p, _)| !waiting.contains(p));
        }
        waiting
    }

    /// The pictures of `frame`, made by the reduced develop on the
    /// pool's threads and kept under its key: `Ok(true)` when they were,
    /// or when what the frame shows changed for another reason (its
    /// sidecar's key was not the one it was held under, and those
    /// pictures are kept, or its edit was reset), so its cell asks
    /// again. The frame's edit is read from its sidecar as the window
    /// reads it, the line of the frame's version; the open frame, a
    /// frame with no edit and one whose pictures are kept already are
    /// left alone.
    pub(crate) fn make(&self, edited: &Edited, frame: &Key) -> Result<bool, Why> {
        let mut key = None;
        let mut made = self.make_under(edited, frame, &mut key);
        // Masks to make: asked of the worker once a key; still missing
        // once it has made every one, they never will be.
        if let (Err(Why::Masks(which)), Some(key)) = (&made, key) {
            let which = which.clone();
            made = if self
                .masks_made
                .lock()
                .expect("masks made")
                .contains(&(frame.clone(), key))
            {
                self.waiting.lock().expect("waiting").insert(frame.clone());
                Err(Why::Without(vec![format!(
                    "the masks made for it do not read back: {}",
                    which.join(", ")
                )]))
            } else if self.ask_masks(frame, key) {
                Err(Why::AskMasks(which))
            } else {
                Err(Why::Masks(which))
            };
        }
        if let (
            Err(
                Why::Failed(_)
                | Why::Without(_)
                | Why::Masks(_)
                | Why::AskMasks(_)
                | Why::NoModel(_),
            ),
            Some(key),
        ) = (&made, key)
        {
            self.tried
                .lock()
                .expect("tried keys")
                .insert((frame.clone(), key));
        }
        if let Err(Why::NoModel(_)) = &made {
            self.waiting.lock().expect("waiting").insert(frame.clone());
        }
        made
    }

    /// [`Reducer::make`], with `tried` set to the key the making was
    /// under once it is known: the one a failure is recorded under.
    fn make_under(
        &self,
        edited: &Edited,
        frame: &Key,
        tried: &mut Option<u64>,
    ) -> Result<bool, Why> {
        if edited.is_open(frame) {
            return Err(Why::Open);
        }
        if !edited.cache_on() {
            return Err(Why::NoCache);
        }
        let Shows::Edit(held) = edited.shows(frame) else {
            return Err(Why::NotWanted);
        };
        *tried = Some(held);
        let path = frame.path.as_path();
        let name = frame.said();
        let hash = greycard_library::hash_file(path).map_err(|e| Why::Failed(e.to_string()))?;
        let (mut sidecar, trouble) = crate::panel::browser::load_sidecar_said(path, true);
        if let Some(t) = trouble {
            return Err(Why::Failed(format!("its sidecar: {t:?}")));
        }
        // The frame's own line: a version gone from the sidecar since
        // the frame was listed (deleted, or taken out by a join) has no
        // edit to make pictures of.
        let Some(line) = sidecar.line_mut(frame.version()) else {
            edited.reset(frame, Some(&hash));
            return Err(Why::Failed("no such version in its sidecar".into()));
        };
        let line = std::mem::take(line);
        // The sidecar read now has the last word on the key: the one the
        // frame was held under may be its row's, made by the index under
        // a look, a lens database or a recipe since moved, or a save may
        // have come between. The frame is settled on the sidecar's, and
        // that is what is made.
        let iso = edited.iso_of(frame);
        let key = edited.key_of(&line.current);
        let mut moved = false;
        if key != held {
            if !crate::edited::is_edited(path, &line.current, iso) {
                tracing::info!("the edit's pictures of {name}: its edit was reset; the camera's");
                edited.reset(frame, Some(&hash));
                return Ok(true);
            }
            tracing::info!(
                "the edit's pictures of {name}: held under {held:016x}, its sidecar makes \
                 {key:016x}; made under the sidecar's"
            );
            edited.settle_key(frame, &hash, key, iso);
            *tried = Some(key);
            moved = true;
        }
        if edited.all_kept(&hash, key) {
            return if moved { Ok(true) } else { Err(Why::NotWanted) };
        }
        let edit = line.current;
        let turn = sidecar.turn;
        // What would keep the picture from being kept, asked before the
        // decode: the look, and the learned masks' rasters or models.
        look_ready(&edit)?;
        masks_ready(&mut self.ai.lock().expect("reduced ai"), path, &edit, turn)?;
        let Some(marked) = edited.mark(&hash, key, crate::edited::REDUCED_RECIPE) else {
            // Being made by another of these threads (another link to
            // the same content).
            return Err(Why::NotWanted);
        };
        let lenses = self.lenses();
        let started = Instant::now();
        self.develops.fetch_add(1, Ordering::SeqCst);
        let developed = self
            .pool()
            .install(|| develop(path, &edit, turn, lenses.as_deref()))?;
        let rasters = rasters(
            &mut self.ai.lock().expect("reduced ai"),
            path,
            &edit,
            turn,
            developed.full,
        )?;
        // The edit may have moved while the develop ran. A frame opened
        // meanwhile keeps this picture all the same, its key read from
        // its sidecar: the open frame's develop makes its own in its
        // place when it lands.
        if edited.shows(frame) != Shows::Edit(key) {
            return Err(Why::Stale);
        }
        let finish = crate::edited::Finish {
            edit: edit.clone(),
            turn,
            source: (developed.image.width as u32, developed.image.height as u32),
            full: developed.full,
            rasters,
            clip_level: developed.clip_level,
            guide: Some(developed.guide.clone()),
            kind: developed.kind,
            white: developed.white.clone(),
            detail: developed.detail.clone(),
        };
        let made = self
            .pool()
            .install(|| crate::edited::make(&developed.image, &finish));
        let making =
            started.elapsed().as_secs_f64() - developed.decode - developed.develop - developed.rest;
        let kept = edited
            .store_reduced(&hash, key, &made)
            .map_err(|e| Why::Failed(e.to_string()))?;
        drop(marked);
        match kept {
            Some(kept) => tracing::info!(
                "the edit's pictures of {name}, reduced: {}x{} by {} in {:.0} ms (decoded in \
                 {:.0} ms, developed to {}x{} in {:.0} ms, lens and detail in {:.0} ms, finished \
                 in {:.0} ms); {} KB kept",
                developed.full.0,
                developed.full.1,
                developed.bin,
                started.elapsed().as_secs_f64() * 1e3,
                developed.decode * 1e3,
                developed.image.width,
                developed.image.height,
                developed.develop * 1e3,
                developed.rest * 1e3,
                making * 1e3,
                kept / 1024,
            ),
            None => tracing::info!(
                "the edit's pictures of {name}, reduced: the open frame's develop kept its own \
                 meanwhile; these are let go"
            ),
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    /// A reducer makes no threads until its first develop: a suite of
    /// a thousand tests makes a worker, and so a reducer, in each.
    #[test]
    fn a_reducer_has_no_pool_until_it_develops() {
        let r = Reducer::new();
        assert!(r.pool.get().is_none());
        let _ = r.pool();
        assert!(r.pool.get().is_some());
    }
    use super::*;

    #[test]
    fn the_settings_leave_the_sharpen_the_denoisers_and_the_dehaze_out() {
        let mut edit = Edit::default();
        edit.detail.dehaze = 0.5;
        edit.noise.strength = 0.5;
        let s = settings(&edit, greycard_core::raw::Orientation::Normal, 1);
        assert!(s.sharpen.is_none());
        assert!(s.dehaze.is_none());
        assert!(s.denoise.is_none());
        assert_eq!(
            s.orientation,
            Some(greycard_core::raw::Orientation::Rotate90)
        );
        assert_eq!(s.demosaic, edit.settings().demosaic);
    }

    /// While editing, nothing begins until the window has been quiet
    /// for the idle time, and the pool is told when to look again;
    /// culling and browsing are not held.
    #[test]
    fn while_editing_nothing_begins_until_the_window_is_idle() {
        let ms = std::time::Duration::from_millis;
        assert_eq!(begin(8, true, ms(200)), (0, Some(IDLE - ms(200))));
        assert_eq!(begin(8, true, IDLE), (1, None));
        assert_eq!(begin(8, false, ms(0)), (2, None));
        assert_eq!(begin(2, false, ms(0)), (1, None));
    }

    #[test]
    fn two_at_once_on_a_machine_with_four_threads_and_one_while_editing() {
        assert_eq!(at_once(2, false), 1);
        assert_eq!(at_once(3, false), 1);
        assert_eq!(at_once(4, false), 2);
        assert_eq!(at_once(8, false), 2);
        assert_eq!(at_once(8, true), 1, "a frame open in the editor");
        assert_eq!(at_once(2, true), 1);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-reduced-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A version of a frame is a frame of its own to the reduced develop:
    /// held by its own frame, its line's edit read from the sidecar and
    /// its pictures made and kept under that edit's key, while the
    /// file's own frame, at the default, shows the camera's.
    #[test]
    fn a_version_s_line_is_developed_by_its_own_frame() {
        let dir = scratch("version");
        let (file, edited) = frame(&dir, "pair.png", &Edit::for_picture(), None);
        let mut sidecar = greycard_edit::Sidecar::load(&file.path).unwrap().unwrap();
        let mut warm = Edit::for_picture();
        warm.light.exposure = 1.0;
        let id = sidecar.new_version(None, Some("Warm")).unwrap();
        sidecar.line_mut(Some(&id)).unwrap().record(warm.clone());
        sidecar.save(&file.path).unwrap();
        let version = Key::of(&file.path, Some(&id));
        edited.note_all([
            (file.clone(), false, None, None),
            (version.clone(), true, None, None),
        ]);
        let reducer = Reducer::new();
        assert!(!reducer.wants(&edited, &file), "the file's own has no edit");
        assert!(reducer.wants(&edited, &version));
        assert!(matches!(reducer.make(&edited, &version), Ok(true)));
        let hash = greycard_library::hash_file(&file.path).unwrap();
        let key = edited.key_of(&warm);
        assert!(
            edited.all_kept(&hash, key),
            "the version's pictures, under its key"
        );
        assert_eq!(edited.shows(&version), Shows::Edit(key));
        assert_eq!(edited.shows(&file), Shows::Camera, "the file's own, apart");
        assert!(
            edited.lookup(&hash, 128, Shows::Edit(key)).is_some(),
            "found by the version's key"
        );
        assert!(!reducer.wants(&edited, &version), "kept");
        // The version gone from the sidecar: nothing to make, and the
        // frame is let go.
        sidecar.delete_version(&id);
        sidecar.save(&file.path).unwrap();
        edited.note(&version, true, None, None);
        assert!(matches!(
            reducer.make(&edited, &version),
            Err(Why::NotWanted) | Err(Why::Failed(_))
        ));
        assert_eq!(edited.shows(&version), Shows::Camera);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A 600 by 400 PNG at `dir/name` with `edit` in its sidecar, and an
    /// `Edited` over a cache of its own holding it as edited, under the
    /// key `row` its row gave.
    fn frame(dir: &Path, name: &str, edit: &Edit, row: Option<u64>) -> (Key, Arc<Edited>) {
        let path = dir.join(name);
        image::RgbImage::from_fn(600, 400, |x, y| {
            image::Rgb([(30 + x / 12) as u8, (40 + y / 10) as u8, 50])
        })
        .save(&path)
        .unwrap();
        greycard_edit::Sidecar {
            line: greycard_edit::Line::of(edit.clone()),
            ..Default::default()
        }
        .save(&path)
        .unwrap();
        let cache: crate::worker::ThumbCache = Arc::new(Mutex::new(Some(
            greycard_library::thumbs::Thumbs::at(
                dir.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            )
            .with_previews(
                crate::edited::BIG,
                greycard_library::thumbs::DEFAULT_PREVIEW_CAP,
            ),
        )));
        let edited = Arc::new(Edited::new(cache));
        let frame = Key::file(&path);
        edited.note_all([(frame.clone(), true, None, row)]);
        (frame, edited)
    }

    /// A brightening with a Sky mask over it.
    fn with_sky() -> Edit {
        let mut edit = Edit::for_picture();
        edit.light.exposure = 1.0;
        let mut a = greycard_edit::Adjustment {
            id: 3,
            name: "sky".into(),
            ..Default::default()
        };
        a.mask.components.push(greycard_edit::mask::Component {
            shape: greycard_edit::mask::Shape::Sky { picks: Vec::new() },
            ..Default::default()
        });
        a.look.light.exposure = -0.5;
        edit.adjustments.push(a);
        edit
    }

    /// A frame whose edit draws a Sky with no Sky model in the store is
    /// tried once and found wanting before anything is decoded, and not
    /// wanted again under that key, so nothing loops; the camera's JPEG
    /// stands. When a model is fetched it is let go, and with the
    /// raster in the cache (as the worker makes it) its pictures are
    /// made. A model in the store whose rasters the worker could not
    /// all make leaves the frame tried the same way.
    #[test]
    fn a_mask_whose_model_is_absent_is_tried_once_and_made_when_it_arrives() {
        let dir = scratch("no-model");
        let edit = with_sky();
        let (frame, edited) = frame(&dir, "sky.png", &edit, None);
        let store = greycard_ai::Store::at(dir.join("models"));
        let reducer = Reducer::with_ai(Ai::for_test_on_disk(store.clone()));
        let hash = greycard_library::hash_file(&frame.path).unwrap();
        let key = edited.key_of(&edit);
        assert!(reducer.wants(&edited, &frame));
        match reducer.make(&edited, &frame) {
            Err(Why::NoModel(which)) => assert!(which[0].contains("Sky"), "{which:?}"),
            other => panic!("not kept for want of the model: {other:?}"),
        }
        assert_eq!(reducer.develops(), 0, "nothing decoded for it");
        assert!(!reducer.wants(&edited, &frame), "tried, and not again");
        assert!(
            edited.lookup(&hash, 128, edited.shows(&frame)).is_none(),
            "the camera's stands"
        );
        // The model is fetched: the frame is let go and asked for again.
        assert_eq!(reducer.models_arrived(), std::slice::from_ref(&frame));
        assert!(reducer.models_arrived().is_empty(), "once");
        assert!(reducer.wants(&edited, &frame));
        // With the model in the store and no raster yet, the worker is
        // asked, once a key, and the frame is not wanted meanwhile.
        for file in greycard_ai::SKY.files {
            let at = store.path(&greycard_ai::SKY, file);
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::fs::File::create(&at)
                .unwrap()
                .set_len(file.bytes)
                .unwrap();
        }
        assert!(matches!(
            reducer.make(&edited, &frame),
            Err(Why::AskMasks(_))
        ));
        assert!(!reducer.wants(&edited, &frame));
        assert_eq!(reducer.develops(), 0);
        // The worker could not make it (a model that fails): still tried.
        reducer.masks_done(&frame, false);
        assert!(!reducer.wants(&edited, &frame), "waits for a fetch");
        assert_eq!(reducer.models_arrived(), std::slice::from_ref(&frame));
        // The raster in its slot, as the worker makes it, at the frame's
        // aspect: the pictures are made, and kept under the key.
        let slot = reducer
            .ai()
            .cached_path_of(
                &frame.path,
                &edit.adjustments[0].mask.components[0].shape,
                0,
            )
            .expect("a slot in the mask cache");
        std::fs::create_dir_all(slot.parent().unwrap()).unwrap();
        let height = (greycard_edit::brush::RASTER_WIDTH as f32 * 400.0 / 600.0).round() as u32;
        image::GrayImage::from_pixel(
            greycard_edit::brush::RASTER_WIDTH as u32,
            height,
            image::Luma([255]),
        )
        .save_with_format(&slot, image::ImageFormat::Png)
        .unwrap();
        reducer.masks_done(&frame, true);
        assert!(reducer.wants(&edited, &frame));
        assert!(matches!(reducer.make(&edited, &frame), Ok(true)));
        assert_eq!(reducer.develops(), 1);
        assert!(edited.all_kept(&hash, key));
        assert!(!edited.all_kept_by(&hash, key, crate::edited::EDITED_RECIPE));
        assert!(!reducer.wants(&edited, &frame), "kept");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A row's key made under what has moved since (a look refitted, the
    /// lens database's cache wiped) is a guess: online the sidecar's key
    /// is read and wins, offline the row's stands. A frame held under a
    /// key its sidecar no longer makes is settled on the sidecar's and
    /// made, not given up as changed meanwhile.
    #[test]
    fn a_row_key_gone_stale_gives_way_to_the_sidecar_s() {
        let dir = scratch("stale-row");
        // A raw's default on a picture corrects the lens by its profile,
        // so its key names the lens database.
        let mut edit = Edit::default();
        edit.light.exposure = 1.0;
        let wiped = edit.develop_key(Some("a lens database since wiped"));
        let (frame, edited) = frame(&dir, "stale.png", &edit, Some(wiped));
        let key = edited.key_of(&edit);
        assert_ne!(key, wiped);
        assert_eq!(edited.shows_offline(&frame), Shows::Edit(wiped), "offline");
        assert_eq!(edited.shows(&frame), Shows::Edit(key), "online, read");
        assert_eq!(edited.shows_offline(&frame), Shows::Edit(key));
        let reducer = Reducer::with_ai(Ai::for_test_on_disk(greycard_ai::Store::at(
            dir.join("models"),
        )));
        let hash = greycard_library::hash_file(&frame.path).unwrap();
        assert!(matches!(reducer.make(&edited, &frame), Ok(true)));
        assert!(edited.all_kept(&hash, key));
        // The sidecar moves under a frame held under its old key (a
        // refit: what it makes is another picture): made under the
        // sidecar's, and shown.
        let mut refit = edit.clone();
        refit.light.exposure = 1.5;
        greycard_edit::Sidecar {
            line: greycard_edit::Line::of(refit.clone()),
            ..Default::default()
        }
        .save(&frame.path)
        .unwrap();
        let now = edited.key_of(&refit);
        edited.settle_key(&frame, &hash, 77, None);
        assert!(matches!(reducer.make(&edited, &frame), Ok(true)));
        assert_eq!(edited.shows(&frame), Shows::Edit(now));
        assert!(edited.all_kept(&hash, now));
        assert_eq!(reducer.develops(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A look refitted while the editor runs (the camera match's fit, a
    /// table replaced): a frame not open that names it keeps its old
    /// picture until the window says the directory moved
    /// (`Edited::files_moved`, from the fit's and the look sheets'
    /// hooks), and is then read again and made anew under its new key.
    /// With nothing moved, nothing is read again.
    #[test]
    fn a_look_refitted_mid_session_remakes_a_frame_not_open() {
        let dir = scratch("refit");
        let (looks, profiles) = (dir.join("looks"), dir.join("profiles"));
        std::fs::create_dir_all(&looks).unwrap();
        std::fs::create_dir_all(&profiles).unwrap();
        let table = looks.join("Fitted.cube");
        std::fs::write(&table, "LUT_3D_SIZE 2\n").unwrap();
        // The look named at no strength: in the key by its contents, and
        // nothing for the develop to load.
        let mut edit = Edit::for_picture();
        edit.light.exposure = 1.0;
        edit.look_lut.lut = greycard_edit::look::LutChoice::Named("Fitted".into());
        edit.look_lut.strength = 0.0;
        let (frame, edited) = frame(&dir, "refit.png", &edit, None);
        edited.set_dirs(looks.clone(), profiles.clone());
        let reducer = Reducer::with_ai(Ai::for_test_on_disk(greycard_ai::Store::at(
            dir.join("models"),
        )));
        let hash = greycard_library::hash_file(&frame.path).unwrap();
        let fitted = edited.key_of(&edit);
        assert!(matches!(reducer.make(&edited, &frame), Ok(true)));
        assert!(edited.all_kept(&hash, fitted));
        assert!(!edited.files_moved(), "nothing moved");
        assert!(!reducer.wants(&edited, &frame));
        // The refit: other contents, another length.
        std::fs::write(&table, "LUT_3D_SIZE 2\n0 0 0.5\n").unwrap();
        let refit = edited.key_of(&edit);
        assert_ne!(refit, fitted, "a refit is another picture");
        assert!(
            !reducer.wants(&edited, &frame),
            "held under the key found, until told"
        );
        assert!(edited.files_moved(), "the directory moved");
        assert!(!edited.files_moved(), "once");
        assert!(reducer.wants(&edited, &frame), "read again");
        assert!(matches!(reducer.make(&edited, &frame), Ok(true)));
        assert_eq!(edited.shows(&frame), Shows::Edit(refit));
        assert!(edited.all_kept(&hash, refit));
        // The old key's pictures are an orphan now, left to the
        // eviction: read again, the frame no longer names the key it
        // showed, so nothing takes them out at once.
        assert!(edited.all_kept(&hash, fitted));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
